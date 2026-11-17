"""Pull-based SN0 work: validators publish tasks, miners fetch them and post signed results.

Miners only make outgoing HTTPS requests, so a home miner needs no open port, no fixed IP
and no endpoint on chain. Validators serve:

  GET  /sn0/tasks?hotkey=<ss58>  -> {"epoch": e, "deadline": unix_s, "tasks": [task, ...]}
       task = {"task_id", "prompt", "max_tokens", "seed"}; only tasks not yet answered
  POST /sn0/result               {"task_id", "hotkey", "text", "tokens", "top_logprobs", "signature"}
       -> 200 accepted, 400 malformed or late, 403 bad signature, 404 unknown task, 409 duplicate

A result is signed by the miner's hotkey over `result_message(...)`, so nobody can post
answers in another miner's name (a bad answer could get that miner reported).
"""
from __future__ import annotations

import hashlib
import json
import threading
import time
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Callable
from urllib.parse import parse_qs, urlparse

MAX_BODY = 1 << 20


def result_message(task_id: str, text: str, tokens: int, top_logprobs) -> bytes:
    """Bytes the miner signs: the task id and a hash of exactly what it answered."""
    body = json.dumps({"text": text, "tokens": tokens, "top_logprobs": top_logprobs},
                      sort_keys=True, separators=(",", ":")).encode()
    return f"incentium-sn0-result:{task_id}:{hashlib.sha256(body).hexdigest()}".encode()


def sr25519_verify(hotkey: str, message: bytes, signature_hex: str) -> bool:
    from substrateinterface import Keypair

    try:
        return bool(Keypair(ss58_address=hotkey).verify(message, bytes.fromhex(signature_hex.removeprefix("0x"))))
    except Exception:
        return False


@dataclass
class Task:
    task_id: str
    hotkey: str
    uid: int
    prompt: str
    max_tokens: int
    seed: int
    result: dict | None = None

    def public(self) -> dict:
        return {"task_id": self.task_id, "prompt": self.prompt, "max_tokens": self.max_tokens, "seed": self.seed}


@dataclass
class TaskBoard:
    """Tasks of the current epoch, shared between the validator loop and the HTTP server."""

    verify: Callable[[str, bytes, str], bool] = sr25519_verify
    epoch: int = -1
    deadline: float = 0.0
    tasks: dict[str, Task] = field(default_factory=dict)
    lock: threading.Lock = field(default_factory=threading.Lock)

    def publish(self, epoch: int, deadline: float, tasks: list[Task]) -> None:
        with self.lock:
            self.epoch, self.deadline = epoch, deadline
            self.tasks = {t.task_id: t for t in tasks}

    def open_tasks(self, hotkey: str) -> dict:
        with self.lock:
            open_ = [t.public() for t in self.tasks.values() if t.hotkey == hotkey and t.result is None]
            return {"epoch": self.epoch, "deadline": self.deadline, "tasks": open_}

    def submit(self, r: dict) -> int:
        """Returns an HTTP status code."""
        try:
            task_id, hotkey, sig = str(r["task_id"]), str(r["hotkey"]), str(r["signature"])
            text, tokens, lp = str(r["text"]), int(r["tokens"]), r["top_logprobs"]
            if not isinstance(lp, list):
                return 400
        except (KeyError, TypeError, ValueError):
            return 400
        with self.lock:
            t = self.tasks.get(task_id)
            if t is None or t.hotkey != hotkey:
                return 404
            if t.result is not None:
                return 409
            if time.time() > self.deadline:
                return 400
        if not self.verify(hotkey, result_message(task_id, text, tokens, lp), sig):
            return 403
        with self.lock:
            if t.result is not None:
                return 409
            t.result = {"text": text, "tokens": tokens, "top_logprobs": lp}
        return 200

    def snapshot(self) -> list[Task]:
        with self.lock:
            return list(self.tasks.values())


def make_handler(board: TaskBoard):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):  # quiet
            pass

        def _send(self, code: int, obj: dict | None = None):
            body = json.dumps(obj if obj is not None else {"status": code}).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            u = urlparse(self.path)
            if u.path != "/sn0/tasks":
                return self._send(404)
            hotkey = parse_qs(u.query).get("hotkey", [""])[0]
            self._send(200, board.open_tasks(hotkey))

        def do_POST(self):
            if self.path != "/sn0/result":
                return self._send(404)
            n = int(self.headers.get("Content-Length", "0"))
            if n <= 0 or n > MAX_BODY:
                return self._send(400)
            try:
                r = json.loads(self.rfile.read(n))
            except ValueError:
                return self._send(400)
            self._send(board.submit(r) if isinstance(r, dict) else 400)

    return Handler


def serve_tasks(board: TaskBoard, host: str = "0.0.0.0", port: int = 8100) -> ThreadingHTTPServer:
    srv = ThreadingHTTPServer((host, port), make_handler(board))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv
