"""Serve chat.incentium.org from a miner: fetch a chat job from the gateway, run it on the local
llama-server and stream the answer back.

Like SN0 tasks, this only makes outgoing HTTPS requests, so it works behind a home router with no
open port. The gateway knows the miner by its hotkey: each poll is signed, and the gateway checks
that the hotkey is registered on chain and serves the tier it declared.

  POST {gateway}/miner/poll   {"hotkey","tier","model","ts","signature"}
       -> 200 {"job": {"id","token","messages","max_tokens","temperature"}} | 204 no job yet
          | 403 not allowed (unregistered, wrong tier, bad signature)
  POST {gateway}/miner/chunk  {"id","token","delta","done","error"?,"tokens"?}
       -> 200 | 410 the user went away (stop) | 404 unknown or reassigned job
"""
from __future__ import annotations

import http.client
import json
import time
import urllib.error
import urllib.request
from typing import Callable
from urllib.parse import urlsplit

FLUSH_SECONDS = 0.25          # send what the model wrote at most this often


def poll_message(hotkey: str, tier: int, model: str, ts: int) -> bytes:
    """Bytes the miner signs to ask the gateway for a job."""
    return f"incentium-chat-poll:{hotkey}:{tier}:{model}:{ts}".encode()


class Gone(Exception):
    """The gateway no longer wants this answer (user stopped it or the job was given to another miner)."""


class _Poster:
    """Posts chunks over one kept-open connection to the gateway."""

    def __init__(self, base: str, timeout: float):
        u = urlsplit(base)
        self.path = (u.path.rstrip("/") or "") + "/miner/chunk"
        cls = http.client.HTTPSConnection if u.scheme == "https" else http.client.HTTPConnection
        self.conn = cls(u.hostname, u.port, timeout=timeout)

    def post(self, obj: dict) -> int:
        body = json.dumps(obj).encode()
        for attempt in (0, 1):                  # a kept-open connection may have been closed: retry once
            try:
                self.conn.request("POST", self.path, body=body, headers={"Content-Type": "application/json"})
                r = self.conn.getresponse()
                r.read()
                return r.status
            except (http.client.HTTPException, OSError):
                self.conn.close()
                if attempt:
                    raise
        return 0

    def close(self):
        self.conn.close()


def _stream_llama(llama_url: str, job: dict, timeout: float):
    """Yield text pieces from llama-server's OpenAI-compatible streaming chat endpoint."""
    body = {
        "messages": job["messages"],
        "max_tokens": int(job["max_tokens"]),
        "temperature": float(job.get("temperature", 0.7)),
        "stream": True,
        # Qwen3.5 can think out loud first; a chat answer should start at once
        "chat_template_kwargs": {"enable_thinking": False},
    }
    req = urllib.request.Request(llama_url.rstrip("/") + "/v1/chat/completions", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        for raw in r:
            line = raw.decode("utf-8", "replace").strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                return
            try:
                ch = json.loads(data)["choices"][0]
            except (ValueError, KeyError, IndexError):
                continue
            piece = (ch.get("delta") or {}).get("content")
            if piece:
                yield piece


def serve_once(gateway: str, hotkey: str, sign: Callable[[bytes], str], tier: int, model: str,
               llama_url: str, timeout: float = 40.0, llama_timeout: float = 300.0) -> int:
    """Ask the gateway for one job and answer it. Returns 1 if a job was answered, 0 if there was
    none. Raises PermissionError when the gateway refuses this miner, other errors on network trouble."""
    base = gateway.rstrip("/")
    ts = int(time.time())
    req = {"hotkey": hotkey, "tier": tier, "model": model, "ts": ts,
           "signature": sign(poll_message(hotkey, tier, model, ts))}
    r = urllib.request.Request(base + "/miner/poll", data=json.dumps(req).encode(),
                               headers={"Content-Type": "application/json", "User-Agent": "incentium-node"})
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            if resp.status == 204:
                return 0
            job = json.loads(resp.read())["job"]
    except urllib.error.HTTPError as e:
        if e.code == 403:
            raise PermissionError(e.read().decode("utf-8", "replace")[:200]) from None
        raise
    poster = _Poster(base, timeout)
    ident = {"id": job["id"], "token": job["token"]}
    buf, last, tokens = [], time.monotonic(), 0

    def flush(done=False, **extra):
        nonlocal buf, last
        status = poster.post({**ident, "delta": "".join(buf), "done": done, **extra})
        buf, last = [], time.monotonic()
        if status in (404, 410):
            raise Gone()

    try:
        for piece in _stream_llama(llama_url, job, llama_timeout):
            buf.append(piece)
            tokens += 1
            if time.monotonic() - last >= FLUSH_SECONDS:
                flush()
        flush(done=True, tokens=tokens)
    except Gone:
        pass
    except Exception as e:
        try:
            poster.post({**ident, "delta": "", "done": True, "error": f"the miner's model failed ({e.__class__.__name__})"})
        except Exception:
            pass
    finally:
        poster.close()
    return 1
