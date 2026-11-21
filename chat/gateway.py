"""chat.incentium.org: a free chat window and an OpenAI-compatible API, answered by the miners' Qwen
models. Prototype: free tier only, a daily quota per visitor, no accounts and no payments.

  python3 chat/gateway.py --rpc wss://testnet-rpc.incentium.org --port 8400
  python3 chat/gateway.py --dev            # accept any signed miner, no chain (local testing)

People:   GET  /                      the chat page (chat/web/index.html)
          GET  /v1/models             models that have miners online now
          POST /v1/chat/completions   OpenAI-compatible, "stream": true for server-sent events
          GET  /api/status            miners online per tier, queue, this visitor's free quota
Miners:   POST /miner/poll            signed by the hotkey; held open until a job comes (long poll)
          POST /miner/chunk           the answer, piece by piece (see neuron/incentium_neuron/chat.py)

Only registered miners serve, and only the tier they declared on chain, so validators' spot checks
apply to the same model. The gateway keeps counters, never the text of conversations.
If no miner takes a job within a few seconds, an optional fallback model (--fallback) answers, so
the page works while few miners are online.
"""
from __future__ import annotations

import argparse
import json
import os
import queue
import secrets
import sys
import threading
import time
import urllib.request
from collections import deque
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "neuron"))

from incentium_neuron.chat import poll_message  # noqa: E402
from incentium_neuron.hw import CATALOG  # noqa: E402

NAMES = {0: "fast", 1: "smart", 2: "pro"}          # what people pick; higher tiers: "tier-N"
LABELS = {0: "Fast", 1: "Smart", 2: "Pro"}


def tier_name(t: int) -> str:
    return NAMES.get(t, f"tier-{t}")


def tier_label(t: int) -> str:
    return LABELS.get(t, f"Tier {t}")


@dataclass
class Limits:
    free_per_day: int = 40           # chat requests per visitor per UTC day
    concurrent_per_visitor: int = 1
    max_tokens: int = 768            # longest answer
    max_input_chars: int = 16_000    # older messages are dropped to stay under this
    poll_hold: float = 25.0          # a miner's poll is held this long waiting for a job
    online_window: float = 60.0      # a miner counts as online this long after its last poll
    pickup_timeout: float = 6.0      # no miner took the job by then: fallback (if any)
    queue_timeout: float = 45.0      # no answer by then: "busy"
    first_piece_timeout: float = 40.0
    idle_timeout: float = 45.0       # silence from the miner in the middle of an answer
    max_attempts: int = 2            # miners tried before giving up on a job


class Refused(Exception):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code, self.message = code, message


@dataclass
class Job:
    id: str
    tier: int
    messages: list
    max_tokens: int
    temperature: float
    visitor: str
    created: float = field(default_factory=time.monotonic)
    token: str = ""
    miner: str | None = None             # hotkey, "fallback", or None while queued
    taken: float = 0.0
    last: float = 0.0
    pieces: int = 0
    attempts: int = 0
    finished: bool = False
    cancelled: bool = False
    out: queue.Queue = field(default_factory=queue.Queue)   # ("piece", text) | ("done", info) | ("error", msg)

    def public(self) -> dict:
        return {"id": self.id, "token": self.token, "messages": self.messages,
                "max_tokens": self.max_tokens, "temperature": self.temperature}


@dataclass
class MinerInfo:
    tier: int
    seen: float
    waiting: int = 0
    busy: int = 0
    served: int = 0
    failed: int = 0


def sr25519_verify(hotkey: str, message: bytes, signature_hex: str) -> bool:
    from incentium_neuron.tasks import sr25519_verify as v

    return v(hotkey, message, signature_hex)


class ChainRegistry:
    """Declared tier of each registered SN0 miner, read from chain at most once a minute."""

    def __init__(self, url: str, ttl: float = 60.0):
        self.url, self.ttl = url, ttl
        self.chain, self.at, self.tiers = None, 0.0, {}
        self.lock = threading.Lock()

    def __call__(self, hotkey: str) -> int | None:
        from incentium_neuron.addr import pubkey

        with self.lock:
            if time.monotonic() - self.at > self.ttl:
                try:
                    if self.chain is None:
                        from incentium_neuron.validator import SubstrateChain

                        self.chain = SubstrateChain(self.url, None)
                    self.tiers = {pubkey(hk): int(tier) for _uid, hk, _ep, tier in self.chain.miners(0)}
                    self.at = time.monotonic()
                except Exception as e:           # keep the last list; try again on the next poll
                    print(f"chain read failed: {e!r}", flush=True)
                    self.chain = None
            return self.tiers.get(pubkey(hotkey))


class Gateway:
    def __init__(self, limits: Limits | None = None, verify=sr25519_verify, registry=None,
                 fallback: str | None = None, fallback_tier: int = 0):
        self.limits = limits or Limits()
        self.verify = verify
        self.registry = registry                 # hotkey -> declared tier, or None if not registered
        self.fallback, self.fallback_tier = fallback, fallback_tier
        self.cond = threading.Condition()
        self.pending: dict[int, deque[Job]] = {}
        self.jobs: dict[str, Job] = {}
        self.miners: dict[str, MinerInfo] = {}
        self.used: dict[str, int] = {}           # visitor -> requests today
        self.inflight: dict[str, int] = {}
        self.day = time.strftime("%Y-%m-%d", time.gmtime())
        self.served = {"miner": 0, "fallback": 0}
        self.stop = threading.Event()
        threading.Thread(target=self._watch, daemon=True).start()

    # ------------------------------------------------------------ what is online
    def online(self) -> dict[int, dict]:
        now = time.monotonic()
        out: dict[int, dict] = {}
        with self.cond:
            for m in self.miners.values():
                if now - m.seen <= self.limits.online_window:
                    o = out.setdefault(m.tier, {"miners": 0, "idle": 0})
                    o["miners"] += 1
                    o["idle"] += m.waiting > 0
        return out

    def models(self) -> list[dict]:
        on = self.online()
        tiers = sorted(set(on) | ({self.fallback_tier} if self.fallback else set()))
        out = [{"id": tier_name(t), "label": tier_label(t), "tier": t,
                "model": CATALOG[t]["model"] if t in CATALOG else f"tier-{t}",
                "miners": on.get(t, {}).get("miners", 0),
                "fallback": bool(self.fallback) and t == self.fallback_tier} for t in tiers]
        if out:
            out.insert(0, {"id": "auto", "label": "Auto", "tier": None, "model": "best available",
                           "miners": sum(o["miners"] for o in on.values()), "fallback": bool(self.fallback)})
        return out

    def pick_tier(self, name: str) -> int:
        on = self.online()
        if name in ("", "auto", None):
            idle = [t for t, o in on.items() if o["idle"]]
            if idle:
                return max(idle)
            if on:
                return max(on)
            if self.fallback:
                return self.fallback_tier
            raise Refused(503, "No miners are online right now. Please try again in a minute.")
        for t, n in NAMES.items():
            if name == n:
                break
        else:
            t = next((t for t, m in CATALOG.items() if m["model"] == name), None)
            if t is None and name.startswith("tier-") and name[5:].isdigit():
                t = int(name[5:])
            if t is None:
                raise Refused(404, f"Unknown model {name!r}. See /v1/models.")
        if t not in on and not (self.fallback and t == self.fallback_tier):
            raise Refused(503, f"No miners are serving {tier_label(t)} right now. Try Auto.")
        return t

    # ------------------------------------------------------------ people
    def quota(self, visitor: str) -> dict:
        with self.cond:
            self._new_day()
            used = self.used.get(visitor, 0)
        return {"used": used, "limit": self.limits.free_per_day, "left": max(0, self.limits.free_per_day - used)}

    def _new_day(self):
        d = time.strftime("%Y-%m-%d", time.gmtime())
        if d != self.day:
            self.day, self.used = d, {}

    def submit(self, visitor: str, model: str, messages, max_tokens=None, temperature=None) -> Job:
        msgs = clean_messages(messages, self.limits.max_input_chars)
        tier = self.pick_tier(model)
        mt = max(16, min(int(max_tokens or self.limits.max_tokens), self.limits.max_tokens))
        temp = max(0.0, min(float(0.7 if temperature is None else temperature), 1.5))
        with self.cond:
            self._new_day()
            if self.used.get(visitor, 0) >= self.limits.free_per_day:
                raise Refused(429, f"You have used today's {self.limits.free_per_day} free messages. "
                                   "They come back at midnight UTC.")
            if self.inflight.get(visitor, 0) >= self.limits.concurrent_per_visitor:
                raise Refused(429, "Wait for the answer you are already getting.")
            self.used[visitor] = self.used.get(visitor, 0) + 1
            self.inflight[visitor] = self.inflight.get(visitor, 0) + 1
            job = Job(secrets.token_hex(8), tier, msgs, mt, temp, visitor)
            self._queue(job, front=False)
        return job

    def _queue(self, job: Job, front: bool):          # holds self.cond
        job.token, job.miner, job.taken, job.pieces = secrets.token_urlsafe(16), None, 0.0, 0
        self.jobs[job.id] = job
        q = self.pending.setdefault(job.tier, deque())
        q.appendleft(job) if front else q.append(job)
        self.cond.notify_all()

    def cancel(self, job: Job):
        """The visitor went away: stop the miner at its next chunk."""
        with self.cond:
            if not job.finished:
                job.cancelled = True
                self._finish(job, refund=job.pieces == 0)

    def _finish(self, job: Job, refund: bool = False):  # holds self.cond
        if job.finished:
            return
        job.finished = True
        self.jobs.pop(job.id, None)
        self.inflight[job.visitor] = max(0, self.inflight.get(job.visitor, 1) - 1)
        if refund:
            self.used[job.visitor] = max(0, self.used.get(job.visitor, 1) - 1)
        m = self.miners.get(job.miner or "")
        if m:
            m.busy = max(0, m.busy - 1)

    def _fail(self, job: Job, message: str):           # holds self.cond
        refund = job.pieces == 0
        self._finish(job, refund=refund)
        job.out.put(("error", message))

    # ------------------------------------------------------------ miners
    def poll(self, req: dict) -> Job | None:
        try:
            hotkey, tier, model, ts = str(req["hotkey"]), int(req["tier"]), str(req["model"]), int(req["ts"])
            sig = str(req["signature"])
        except (KeyError, TypeError, ValueError):
            raise Refused(400, "malformed poll") from None
        if abs(time.time() - ts) > 120:
            raise Refused(403, "clock is off by more than two minutes")
        if tier not in CATALOG or CATALOG[tier]["model"] != model:
            raise Refused(403, f"tier {tier} does not serve {model}")
        if not self.verify(hotkey, poll_message(hotkey, tier, model, ts), sig):
            raise Refused(403, "bad signature")
        if self.registry is not None:
            declared = self.registry(hotkey)
            if declared is None:
                raise Refused(403, "this hotkey is not a registered SN0 miner")
            if declared != tier:
                raise Refused(403, f"this hotkey declared tier {declared} on chain, not {tier}")
        deadline = time.monotonic() + self.limits.poll_hold
        with self.cond:
            m = self.miners.get(hotkey)
            if m is None or m.tier != tier:
                m = self.miners[hotkey] = MinerInfo(tier, time.monotonic())
            m.waiting += 1
            try:
                while not self.stop.is_set():
                    m.seen = time.monotonic()
                    q = self.pending.get(tier)
                    while q:
                        job = q.popleft()
                        if job.finished or job.miner:
                            continue
                        job.miner, job.taken, job.last = hotkey, time.monotonic(), time.monotonic()
                        job.attempts += 1
                        m.busy += 1
                        return job
                    left = deadline - time.monotonic()
                    if left <= 0:
                        return None
                    self.cond.wait(left)
            finally:
                m.waiting -= 1
                m.seen = time.monotonic()
        return None

    def chunk(self, req: dict) -> int:
        """HTTP status for the miner: 200 go on, 410 stop (user left), 404 not yours."""
        with self.cond:
            job = self.jobs.get(str(req.get("id", "")))
            if job is None:
                return 410
            if not secrets.compare_digest(str(req.get("token", "")), job.token):
                return 404
            job.last = time.monotonic()
            piece = str(req.get("delta") or "")
            if piece:
                job.pieces += 1
                job.out.put(("piece", piece))
            if req.get("error"):
                m = self.miners.get(job.miner or "")
                if m:
                    m.failed += 1
                if job.pieces == 0 and job.attempts < self.limits.max_attempts:
                    self._requeue(job)
                else:
                    self._fail(job, str(req["error"])[:200])
                return 200
            if req.get("done"):
                m = self.miners.get(job.miner or "")
                if m:
                    m.served += 1
                self.served["miner"] += 1
                self._finish(job)
                job.out.put(("done", {"tokens": int(req.get("tokens") or 0)}))
            return 200

    def _requeue(self, job: Job):                      # holds self.cond
        m = self.miners.get(job.miner or "")
        if m:
            m.busy = max(0, m.busy - 1)
        self._queue(job, front=True)

    # ------------------------------------------------------------ timeouts and fallback
    def _watch(self):
        while not self.stop.wait(0.5):
            now = time.monotonic()
            to_fallback = []
            with self.cond:
                for job in list(self.jobs.values()):
                    if job.finished:
                        continue
                    if job.miner is None:
                        waited = now - job.created
                        if self.fallback and job.tier == self.fallback_tier and waited > self.limits.pickup_timeout:
                            self._take_for_fallback(job)
                            to_fallback.append(job)
                        elif waited > self.limits.queue_timeout:
                            self._fail(job, "The network is busy. Please try again in a minute.")
                    elif job.miner != "fallback":
                        silent = now - job.last
                        limit = self.limits.idle_timeout if job.pieces else self.limits.first_piece_timeout
                        if silent > limit:
                            m = self.miners.get(job.miner)
                            if m:
                                m.failed += 1
                            if job.pieces == 0 and job.attempts < self.limits.max_attempts:
                                self._requeue(job)
                            else:
                                self._fail(job, "The miner stopped answering.")
            for job in to_fallback:
                threading.Thread(target=self._run_fallback, args=(job,), daemon=True).start()

    def _take_for_fallback(self, job: Job):            # holds self.cond
        q = self.pending.get(job.tier)
        if q and job in q:
            q.remove(job)
        job.miner, job.taken, job.last = "fallback", time.monotonic(), time.monotonic()

    def _run_fallback(self, job: Job):
        body = {"messages": job.messages, "max_tokens": job.max_tokens, "temperature": job.temperature,
                "stream": True, "chat_template_kwargs": {"enable_thinking": False}}
        req = urllib.request.Request(self.fallback.rstrip("/") + "/v1/chat/completions", data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"})
        tokens = 0
        try:
            with urllib.request.urlopen(req, timeout=300) as r:
                for raw in r:
                    if job.cancelled:
                        return
                    line = raw.decode("utf-8", "replace").strip()
                    if not line.startswith("data:") or line[5:].strip() == "[DONE]":
                        continue
                    try:
                        piece = (json.loads(line[5:])["choices"][0].get("delta") or {}).get("content")
                    except (ValueError, KeyError, IndexError):
                        continue
                    if piece:
                        tokens += 1
                        with self.cond:
                            job.pieces += 1
                        job.out.put(("piece", piece))
            with self.cond:
                self.served["fallback"] += 1
                self._finish(job)
            job.out.put(("done", {"tokens": tokens}))
        except Exception as e:
            with self.cond:
                self._fail(job, f"The model did not answer ({e.__class__.__name__}).")

    def status(self, visitor: str) -> dict:
        with self.cond:
            queued = sum(len(q) for q in self.pending.values())
            served = dict(self.served)
        return {"models": self.models(), "queue": queued, "served": served, "quota": self.quota(visitor)}


def clean_messages(messages, max_chars: int) -> list[dict]:
    """Keep role and text only; drop the oldest messages (not the system one) to fit max_chars."""
    if not isinstance(messages, list) or not messages:
        raise Refused(400, "messages must be a non-empty list")
    out = []
    for m in messages:
        if not isinstance(m, dict) or m.get("role") not in ("system", "user", "assistant"):
            raise Refused(400, "each message needs a role of system, user or assistant")
        c = m.get("content")
        if isinstance(c, list):                  # OpenAI content parts: keep the text ones
            c = "".join(p.get("text", "") for p in c if isinstance(p, dict) and p.get("type") == "text")
        if not isinstance(c, str):
            raise Refused(400, "message content must be text")
        out.append({"role": m["role"], "content": c})
    if out[-1]["role"] != "user":
        raise Refused(400, "the last message must be from the user")
    system = out[:1] if out[0]["role"] == "system" else []
    rest = out[len(system):]
    total = lambda: sum(len(m["content"]) for m in system + rest)  # noqa: E731
    while len(rest) > 1 and total() > max_chars:
        rest.pop(0)
    if total() > max_chars:
        raise Refused(413, f"The message is too long (more than {max_chars} characters).")
    return system + rest


# ---------------------------------------------------------------- HTTP
def make_handler(gw: Gateway, page: str):
    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        server_version = "incentium-chat"

        def log_message(self, *args):            # no request logs: they would hold visitors' IPs
            pass

        def visitor(self) -> str:
            ip = self.client_address[0]
            fwd = self.headers.get("X-Forwarded-For")
            if fwd and ip in ("127.0.0.1", "::1"):      # behind Caddy on this server
                ip = fwd.split(",")[-1].strip()
            return ip

        def send(self, code: int, obj=None, ctype="application/json", extra=None):
            body = obj if isinstance(obj, bytes) else json.dumps(obj if obj is not None else {}).encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body)

        def error(self, code: int, message: str):
            self.send(code, {"error": {"message": message, "code": code}})

        def body(self) -> dict:
            n = int(self.headers.get("Content-Length") or 0)
            if n > 1 << 20:
                raise Refused(413, "request too large")
            try:
                return json.loads(self.rfile.read(n) or b"{}")
            except ValueError:
                raise Refused(400, "body is not JSON") from None

        def do_OPTIONS(self):
            self.send_response(204)
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Access-Control-Allow-Headers", "Content-Type, Authorization")
            self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
            self.send_header("Content-Length", "0")
            self.end_headers()

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            if path in ("/", "/index.html"):
                return self.send(200, page.encode(), "text/html; charset=utf-8")
            if path == "/v1/models":
                return self.send(200, {"object": "list", "data": [
                    {"id": m["id"], "object": "model", "owned_by": "incentium", **m} for m in gw.models()]},
                    extra={"Access-Control-Allow-Origin": "*"})
            if path == "/api/status":
                return self.send(200, gw.status(self.visitor()))
            if path == "/healthz":
                return self.send(200, {"ok": True})
            self.error(404, "not found")

        def do_POST(self):
            path = self.path.split("?", 1)[0]
            try:
                if path == "/miner/poll":
                    job = gw.poll(self.body())
                    if job is None:
                        self.send_response(204)
                        self.send_header("Content-Length", "0")
                        return self.end_headers()
                    return self.send(200, {"job": job.public()})
                if path == "/miner/chunk":
                    return self.send(gw.chunk(self.body()), {})
                if path == "/v1/chat/completions":
                    return self.chat(self.body())
                self.error(404, "not found")
            except Refused as e:
                self.error(e.code, e.message)

        def chat(self, req: dict):
            job = gw.submit(self.visitor(), str(req.get("model") or "auto"), req.get("messages"),
                            req.get("max_tokens") or req.get("max_completion_tokens"), req.get("temperature"))
            model = CATALOG[job.tier]["model"] if job.tier in CATALOG else tier_name(job.tier)
            created = int(time.time())
            quota = gw.quota(job.visitor)
            info = {"tier": job.tier, "label": tier_label(job.tier), "free_left": quota["left"]}
            if not req.get("stream"):
                parts, err, done = [], None, {}
                while True:
                    kind, val = job.out.get()
                    if kind == "piece":
                        parts.append(val)
                    elif kind == "error":
                        err = val
                        break
                    else:
                        done = val
                        break
                if err and not parts:
                    return self.error(503, err)
                return self.send(200, {
                    "id": "chatcmpl-" + job.id, "object": "chat.completion", "created": created, "model": model,
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "".join(parts)},
                                 "finish_reason": "stop" if not err else "error"}],
                    "usage": {"completion_tokens": done.get("tokens", 0)},
                    "incentium": {**info, "served_by": served_by(job)}}, extra={"Access-Control-Allow-Origin": "*"})
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Accel-Buffering", "no")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True

            def event(delta: dict, finish=None, extra=None):
                obj = {"id": "chatcmpl-" + job.id, "object": "chat.completion.chunk", "created": created,
                       "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                if extra:
                    obj["incentium"] = extra
                self.wfile.write(b"data: " + json.dumps(obj).encode() + b"\n\n")
                self.wfile.flush()

            try:
                event({"role": "assistant"}, extra={**info, "served_by": None})
                announced = False
                while True:
                    try:
                        kind, val = job.out.get(timeout=15)
                    except queue.Empty:
                        self.wfile.write(b": waiting\n\n")      # keeps proxies from closing the stream
                        self.wfile.flush()
                        continue
                    if kind == "piece":
                        extra = None
                        if not announced:
                            extra, announced = {**info, "served_by": served_by(job)}, True
                        event({"content": val}, extra=extra)
                    elif kind == "error":
                        event({}, "error", {**info, "error": val, "served_by": served_by(job) if announced else None})
                        break
                    else:
                        event({}, "stop", {**info, "tokens": val.get("tokens", 0), "served_by": served_by(job)})
                        break
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, OSError):
                gw.cancel(job)

    return H


def served_by(job: Job) -> str | None:
    if job.miner == "fallback":
        return "fallback"
    if job.miner:
        return f"miner {job.miner[:6]}…{job.miner[-4:]}"
    return None


def serve(gw: Gateway, host="127.0.0.1", port=8400, page_path=None) -> ThreadingHTTPServer:
    page_path = page_path or os.path.join(HERE, "web", "index.html")
    page = open(page_path, encoding="utf-8").read()
    srv = ThreadingHTTPServer((host, port), make_handler(gw, page))
    srv.daemon_threads = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8400)
    ap.add_argument("--rpc", help="chain RPC; only registered SN0 miners on their declared tier may serve")
    ap.add_argument("--dev", action="store_true", help="no chain check: any miner with a valid signature serves")
    ap.add_argument("--fallback", help="OpenAI-compatible URL that answers when no miner takes a job")
    ap.add_argument("--fallback-tier", type=int, default=0)
    ap.add_argument("--free-per-day", type=int, default=Limits.free_per_day)
    a = ap.parse_args()
    if not a.rpc and not a.dev:
        ap.error("give --rpc (normal) or --dev (testing without a chain)")
    registry = ChainRegistry(a.rpc) if a.rpc else None
    gw = Gateway(Limits(free_per_day=a.free_per_day), registry=registry, fallback=a.fallback,
                 fallback_tier=a.fallback_tier)
    serve(gw, a.host, a.port)
    print(f"chat gateway on http://{a.host}:{a.port} ({'chain ' + a.rpc if a.rpc else 'dev mode'}"
          f"{', fallback ' + a.fallback if a.fallback else ''})", flush=True)
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
