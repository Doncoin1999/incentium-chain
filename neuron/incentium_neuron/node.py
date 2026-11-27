"""Incentium Node: runs an SN0 miner on an ordinary computer, with a dashboard in the browser.

One process does everything, so the installer only has to start this:
  * picks the Qwen3.5 tier this computer can run (incentium_neuron.hw) among the tiers open on chain,
  * downloads the model (checked against the hash on chain once that is locked),
  * runs llama-server and restarts it if it stops,
  * asks to be registered (from the dashboard) and waits until the hotkey is on chain,
  * answers validators' tasks, keeps the computer awake while it works (macOS, Windows),
  * answers chat.incentium.org visitors with the same model (incentium_neuron.chat; can be switched off),
  * serves the dashboard on http://127.0.0.1:7777 (only this computer; a token in the link).

  python -m incentium_neuron.node --home ~/.incentium-node
"""
from __future__ import annotations

import argparse
import hashlib
import http.cookies
import json
import os
import secrets
import signal
import socket
import subprocess
import sys
import threading
import time
import traceback
import urllib.error
import urllib.request
from collections import deque
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

from . import hw as HW

UNIT = 10**9
SN0 = 0
NETWORKS = {
    "testnet": {"rpc": "wss://testnet-rpc.incentium.org", "register": "https://register.incentium.org",
                "chat": "https://chat.incentium.org",
                "explorer": "https://polkadot.js.org/apps/?rpc=wss%3A%2F%2Ftestnet-rpc.incentium.org#/accounts"},
    "mainnet": {"rpc": "wss://rpc.incentium.org", "register": "https://register-mainnet.incentium.org",
                "chat": "",                       # the chat prototype runs on testnet only
                "explorer": "https://polkadot.js.org/apps/?rpc=wss%3A%2F%2Frpc.incentium.org#/accounts"},
}
MIN_TOKENS_PER_S = {0: 0.5, 1: 2.0, 2: 2.0}     # slower than this after start-up: drop a tier


# ------------------------------------------------------------------ configuration and state
class Config:
    """~/.incentium-node/config.json; written by the installer, updated by the node."""

    def __init__(self, home: str):
        self.home = home
        self.path = os.path.join(home, "config.json")
        self.data = json.load(open(self.path)) if os.path.exists(self.path) else {}
        d = self.data
        d.setdefault("network", "testnet")
        d.setdefault("token", secrets.token_urlsafe(24))
        d.setdefault("port", 7777)
        d.setdefault("llama_port", 8096)
        d.setdefault("paused", False)
        d.setdefault("backed_up", False)
        d.setdefault("request", None)        # {"name","email","note","at","status","message"}
        d.setdefault("tier", None)           # tier of the model now on disk / running
        d.setdefault("day", {})
        d.setdefault("chat", True)           # answer chat.incentium.org visitors while mining
        self.lock = threading.Lock()
        self.save()

    def __getitem__(self, k):
        return self.data[k]

    def set(self, **kw):
        with self.lock:
            self.data.update(kw)
            self.save()

    def save(self):
        tmp = self.path + ".tmp"
        with os.fdopen(os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as f:
            json.dump(self.data, f, indent=2)
        os.replace(tmp, self.path)

    def net(self) -> dict:
        n = dict(NETWORKS[self.data["network"]])
        n.update({k: v for k, v in self.data.get("overrides", {}).items() if v})
        return n


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.phase = "starting"      # starting, connecting, downloading, verifying, loading, request,
        self.message = ""            # approval, mining, paused, error
        self.hardware = None
        self.max_tier = 0
        self.open_tiers: list[int] = []
        self.target_tier = 0
        self.running_tier = None
        self.download = {"done": 0, "total": 0}
        self.speed = None            # tokens/s from the start-up test
        self.address = ""
        self.uid = None
        self.declared_tier = None
        self.free = None
        self.locked = None
        self.today = {"tasks": 0, "earned": 0}
        self.chat = {"state": "off", "message": "", "answered": 0}
        self.validators = 0
        self.last_task = None
        self.log = deque(maxlen=200)

    def set(self, **kw):
        with self.lock:
            for k, v in kw.items():
                setattr(self, k, v)

    def say(self, msg: str):
        line = time.strftime("%H:%M:%S ") + msg
        print(line, flush=True)
        with self.lock:
            self.log.append(line)


# ------------------------------------------------------------------ helpers
def download(url: str, dest: str, progress, chunk=1 << 20):
    """Download with resume (dest + '.part'); `progress(done, total)` while it runs."""
    part = dest + ".part"
    have = os.path.getsize(part) if os.path.exists(part) else 0
    req = urllib.request.Request(url, headers={"User-Agent": "incentium-node", **({"Range": f"bytes={have}-"} if have else {})})
    with urllib.request.urlopen(req, timeout=60) as r:
        if have and r.status != 206:          # server ignored Range: start over
            have = 0
        total = have + int(r.headers.get("Content-Length") or 0)
        with open(part, "ab" if have else "wb") as f:
            done = have
            while True:
                b = r.read(chunk)
                if not b:
                    break
                f.write(b)
                done += len(b)
                progress(done, total)
    os.replace(part, dest)


def sha256_file(path: str, progress=None) -> str:
    h, size, done = hashlib.sha256(), os.path.getsize(path), 0
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 22), b""):
            h.update(b)
            done += len(b)
            if progress:
                progress(done, size)
    return h.hexdigest()


def free_port(start: int) -> int:
    for p in range(start, start + 50):
        with socket.socket() as s:
            try:
                s.bind(("127.0.0.1", p))
                return p
            except OSError:
                continue
    raise RuntimeError("no free local port")


def same_account(a: str, b: str) -> bool:
    from .addr import pubkey

    try:
        return pubkey(a) == pubkey(b)
    except Exception:
        return a == b


# ------------------------------------------------------------------ the node
class Node:
    def __init__(self, cfg: Config, chain_factory=None, register_fn=None, detect=HW.detect):
        self.cfg, self.st = cfg, State()
        self.key_file = os.path.join(cfg.home, "miner-key.json")
        self.models = os.path.join(cfg.home, "models")
        os.makedirs(self.models, exist_ok=True)
        self.chain_factory = chain_factory or self._chain
        self.register_fn = register_fn or self._register
        self.detect = detect
        self.chain = None
        self.llama = None
        self.awake = None
        self.wake = threading.Event()         # set by the dashboard to skip waiting
        self.stop = threading.Event()
        self.llama_url = None
        self.chat_serve = None                # replaced in tests; default incentium_neuron.chat.serve_once

    # -- outside world (replaced in tests)
    def _chain(self, url):
        from .validator import SubstrateChain

        return SubstrateChain(url, None)

    def _register(self, contact: dict) -> tuple[int, dict]:
        from .setup_miner import register

        return register(self.key_file, self.cfg.net()["register"], contact=contact)

    # -- steps
    def ensure_key(self):
        from .setup_miner import keygen

        self.st.set(address=keygen(self.key_file))

    def connect(self):
        while not self.stop.is_set():
            try:
                self.chain = self.chain_factory(self.cfg.net()["rpc"])
                return
            except Exception as e:
                self.st.set(phase="connecting", message=f"Cannot reach the Incentium network yet ({e.__class__.__name__}). Retrying.")
                self.wake.wait(20)
                self.wake.clear()

    def tiers(self) -> dict:
        try:
            return self.chain.tiers()
        except Exception:
            self.chain = None
            self.connect()
            return self.chain.tiers()

    def model_path(self, tier: int) -> str:
        return os.path.join(self.models, HW.CATALOG[tier]["url"].rsplit("/", 1)[1])

    def ensure_model(self, tier: int, info) -> bool:
        """Download and check the model for `tier`. False if it could not be had."""
        path, m = self.model_path(tier), HW.CATALOG[tier]
        want = (info.model_hash or "").lower().removeprefix("0x") if info else ""
        want = want if want.strip("0") else ""
        checked = path + ".sha256"
        if not os.path.exists(path):
            self.st.say(f"downloading {m['model']} ({m['size_gb']} GB)")
            self.st.set(phase="downloading", message=f"Downloading the {m['model']} model ({m['size_gb']} GB).",
                        download={"done": 0, "total": int(m["size_gb"] * HW.GB)})
            for attempt in range(5):
                try:
                    download(m["url"], path, lambda d, t: self.st.set(download={"done": d, "total": t}))
                    break
                except Exception as e:
                    self.st.say(f"download interrupted ({e!r}); retrying")
                    if isinstance(e, urllib.error.HTTPError) and e.code in (403, 404):
                        return False
                    time.sleep(10 * (attempt + 1))
            else:
                return False
        if want and (not os.path.exists(checked) or open(checked).read().strip() != want):
            self.st.set(phase="verifying", message="Checking the model against the reference on chain.")
            got = sha256_file(path, lambda d, t: self.st.set(download={"done": d, "total": t}))
            if got != want:
                self.st.say(f"model {m['model']} does not match the hash on chain; deleting it")
                os.remove(path)
                return False
            open(checked, "w").write(got)
        return True

    def start_llama(self, tier: int):
        self.stop_llama()
        hwinfo = self.st.hardware
        port = free_port(self.cfg["llama_port"])
        exe = self.cfg.data.get("llama_server")
        if not exe or not os.path.exists(exe):
            raise RuntimeError("llama-server is missing: run the installer again")
        threads = max(1, (hwinfo.cores if hwinfo else os.cpu_count() or 2) - 1)
        cmd = [exe, "-m", self.model_path(tier), "--host", "127.0.0.1", "--port", str(port),
               "--alias", HW.CATALOG[tier]["model"], "-np", "2", "-c", "4096", "-t", str(threads),
               "-ngl", str(hwinfo.gpu_layers(tier) if hwinfo else 0)]
        log = open(os.path.join(self.cfg.home, "llama.log"), "ab")
        self.llama = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, cwd=os.path.dirname(exe))
        self.llama_url = f"http://127.0.0.1:{port}"
        self.st.set(phase="loading", message=f"Loading {HW.CATALOG[tier]['model']} into memory.")
        for _ in range(600):
            if self.llama.poll() is not None:
                raise RuntimeError("llama-server stopped while loading (see llama.log)")
            try:
                with urllib.request.urlopen(self.llama_url + "/health", timeout=2) as r:
                    if r.status == 200:
                        return
            except Exception:
                pass
            time.sleep(1)
        raise RuntimeError("llama-server did not become ready")

    def stop_llama(self):
        if self.llama and self.llama.poll() is None:
            self.llama.terminate()
            try:
                self.llama.wait(15)
            except subprocess.TimeoutExpired:
                self.llama.kill()
        self.llama = None
        self.llama_url = None

    def speed_test(self, tier: int) -> float:
        body = json.dumps({"model": HW.CATALOG[tier]["model"], "prompt": "The Incentium network is",
                           "max_tokens": 32, "temperature": 0}).encode()
        t0 = time.time()
        req = urllib.request.Request(self.llama_url + "/v1/completions", data=body,
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=300) as r:
            data = json.loads(r.read())
        n = data.get("usage", {}).get("completion_tokens") or 32
        tps = (data.get("timings") or {}).get("predicted_per_second") or n / max(time.time() - t0, 1e-3)
        return float(tps)

    def run_model(self, want: int, tiers: dict) -> int:
        """Get the highest workable tier <= want running. Returns the tier that runs."""
        for tier in range(want, -1, -1):
            if not self.ensure_model(tier, tiers.get(tier)):
                continue
            try:
                self.start_llama(tier)
                tps = self.speed_test(tier)
            except Exception as e:
                self.st.say(f"{HW.CATALOG[tier]['model']} would not run: {e}")
                self.stop_llama()
                continue
            self.st.set(speed=round(tps, 1))
            if tier > 0 and tps < MIN_TOKENS_PER_S[tier]:
                self.st.say(f"{HW.CATALOG[tier]['model']} runs at {tps:.1f} tokens/s here: too slow, using a smaller model")
                self.stop_llama()
                continue
            self.st.say(f"{HW.CATALOG[tier]['model']} runs at {tps:.1f} tokens/s")
            self.cfg.set(tier=tier)
            self.st.set(running_tier=tier)
            return tier
        raise RuntimeError("no model could be started on this computer")

    def registration(self):
        """(uid, declared tier) of our hotkey in SN0, or (None, None)."""
        try:
            for row in self.chain.miners(SN0):
                if same_account(row[1], self.st.address):
                    return row[0], (row[3] if len(row) > 3 else 0)
        except Exception:
            self.chain = None
            self.connect()
        return None, None

    def balances(self):
        try:
            s = self.chain.s
            free = s.query("System", "Account", [self.st.address]).value["data"]["free"]
            locked = s.query("Consensus", "LaunchLocked", [self.st.address]).value or 0
            self.st.set(free=free, locked=locked)
            day = time.strftime("%Y-%m-%d")
            d = self.cfg["day"]
            if d.get("date") != day:
                d = {"date": day, "start": free + locked, "tasks": 0}
                self.cfg.set(day=d)
            self.st.set(today={"tasks": d.get("tasks", 0), "earned": max(0, free + locked - d["start"])})
        except Exception:
            pass

    def keep_awake(self, on: bool):
        if sys.platform == "win32":
            # ES_CONTINUOUS | ES_SYSTEM_REQUIRED keeps Windows from sleeping; ES_CONTINUOUS alone lets it again
            try:
                import ctypes
                ctypes.windll.kernel32.SetThreadExecutionState(0x80000000 | (0x00000001 if on else 0))
            except Exception:
                pass
            return
        if sys.platform != "darwin":
            return
        if on and not (self.awake and self.awake.poll() is None):
            self.awake = subprocess.Popen(["caffeinate", "-i", "-w", str(os.getpid())])
        elif not on and self.awake:
            self.awake.terminate()
            self.awake = None

    # -- main loop
    def run(self):
        try:
            self._run()
        except Exception as e:
            traceback.print_exc()
            self.st.set(phase="error", message=str(e))
            self.st.say(f"error: {e}")

    def _run(self):
        from .backend import OpenAICompatibleBackend
        from .miner import Miner

        self.ensure_key()
        h = self.detect(self.cfg.home)
        ok, why = h.can_run_at_all()
        self.st.set(hardware=h, max_tier=h.max_tier())
        if not ok:
            raise RuntimeError(why)
        self.st.say(f"{h.cpu}, {h.ram_gb:.0f} GB memory{', ' + h.gpu if h.gpu else ''}: up to tier {h.max_tier()}")
        self.connect()
        miner = None
        validators, looked = [], 0.0
        last_check = 0.0
        while not self.stop.is_set():
            if self.cfg["paused"]:
                self.stop_llama()
                self.keep_awake(False)
                self.st.set(phase="paused", message="Paused. Press Resume to continue.", running_tier=None)
                self.wake.wait(5)
                self.wake.clear()
                miner = None
                continue
            now = time.time()
            if now - last_check > 60 or miner is None:
                last_check = now
                uid, declared = self.registration()
                self.st.set(uid=uid, declared_tier=declared)
                tiers = self.tiers()
                open_tiers = sorted(t for t, i in tiers.items() if i.open and t in HW.CATALOG)
                target = HW.choose_tier(h, set(open_tiers))
                self.st.set(open_tiers=open_tiers, target_tier=target)
                # Run the tier this hotkey has declared on chain (tier 0 until it declares more):
                # validators check answers against the declared tier's model.
                want = declared if declared is not None else 0
                want = min(want, h.max_tier())
                if self.llama is None or self.llama.poll() is not None or self.st.running_tier != want:
                    self.run_model(want, tiers)
                    miner = None
                self.balances()
                if uid is None:
                    req = self.cfg["request"]
                    if not req:
                        self.st.set(phase="request", message="Ready. Send your request to start mining.")
                    else:
                        if req.get("status") in (None, "sending"):
                            self.send_request(req)
                        msg = self.cfg["request"].get("message") or "Your request is waiting for approval."
                        self.st.set(phase="approval", message=msg)
                    self.keep_awake(False)
                    self.wake.wait(30)
                    self.wake.clear()
                    continue
            if miner is None:
                from substrateinterface import Keypair

                from .addr import SS58_FORMAT

                kp = Keypair.create_from_mnemonic(json.load(open(self.key_file))["secretPhrase"], ss58_format=SS58_FORMAT)
                miner = Miner(OpenAICompatibleBackend(self.llama_url, HW.CATALOG[self.st.running_tier]["model"]),
                              kp.ss58_address, lambda m: kp.sign(m).hex())
                self.st.say(f"mining as uid {self.st.uid} with {HW.CATALOG[self.st.running_tier]['model']}")
            if now - looked > 300:
                try:
                    validators = self.chain.validator_endpoints(SN0)
                except Exception:
                    validators = validators or []
                looked = now
                self.st.set(validators=len(validators))
            self.keep_awake(True)
            self.st.set(phase="mining", message="Mining. Keep this computer on and connected.")
            for v in validators:
                n = miner.work_once(v)
                if n:
                    d = dict(self.cfg["day"])
                    d["tasks"] = d.get("tasks", 0) + n
                    self.cfg.set(day=d)
                    self.st.set(last_task=time.time(), today={**self.st.today, "tasks": d["tasks"]})
                    self.st.say(f"answered {n} tasks")
            self.wake.wait(5)
            self.wake.clear()

    def send_request(self, req: dict):
        try:
            code, body = self.register_fn({k: req.get(k, "") for k in ("name", "email", "note")})
        except Exception as e:
            self.cfg.set(request={**req, "status": None, "message": f"Could not reach the registration service ({e.__class__.__name__}). Trying again."})
            return
        if code == 200:
            status, msg = "registered", "Approved. Your miner is being registered on the network."
        elif code == 202:
            status, msg = "pending", body.get("message") or "Your request is waiting for approval."
        else:
            status, msg = "refused", body.get("error") or f"The registration service answered {code}."
        self.st.say(f"registration request: {status} ({msg})")
        self.cfg.set(request={**req, "status": status, "message": msg})

    # -- chat.incentium.org
    def chat_loop(self):
        """Answer chat visitors while the node is mining. Runs in its own thread."""
        from substrateinterface import Keypair

        from . import chat
        from .addr import SS58_FORMAT

        serve = self.chat_serve or chat.serve_once
        kp, pause = None, 0.0
        while not self.stop.is_set():
            if pause:
                self.stop.wait(pause)
                pause = 0.0
                continue
            gateway = self.cfg.net().get("chat") or ""
            tier = self.st.running_tier
            if not (self.cfg["chat"] and gateway and self.st.phase == "mining" and tier is not None and self.llama_url):
                self.st.set(chat={**self.st.chat, "state": "off" if not self.cfg["chat"] else "waiting",
                                  "message": "" if self.cfg["chat"] else "Chat answers are switched off."})
                pause = 5.0
                continue
            if kp is None:
                kp = Keypair.create_from_mnemonic(json.load(open(self.key_file))["secretPhrase"], ss58_format=SS58_FORMAT)
            try:
                n = serve(gateway, kp.ss58_address, lambda m: kp.sign(m).hex(), tier,
                          HW.CATALOG[tier]["model"], self.llama_url)
                answered = self.st.chat.get("answered", 0) + n
                self.st.set(chat={"state": "serving", "message": "Ready to answer chat visitors.", "answered": answered})
                if n:
                    d = dict(self.cfg["day"])
                    d["chats"] = d.get("chats", 0) + n
                    self.cfg.set(day=d)
            except PermissionError as e:
                self.st.set(chat={**self.st.chat, "state": "refused", "message": f"The chat gateway said no: {e}"})
                pause = 300.0
            except Exception as e:
                self.st.set(chat={**self.st.chat, "state": "unreachable",
                                  "message": f"Cannot reach the chat gateway ({e.__class__.__name__}). Retrying."})
                pause = 30.0

    def shutdown(self, *_):
        self.stop.set()
        self.wake.set()
        self.stop_llama()
        self.keep_awake(False)


# ------------------------------------------------------------------ dashboard
def status_json(node: Node) -> dict:
    st, cfg = node.st, node.cfg
    with st.lock:
        h = st.hardware
        return {
            "phase": st.phase, "message": st.message, "network": cfg["network"],
            "address": st.address, "uid": st.uid, "explorer": cfg.net()["explorer"],
            "hardware": asdict(h) if h else None, "max_tier": st.max_tier, "open_tiers": st.open_tiers,
            "target_tier": st.target_tier, "running_tier": st.running_tier, "declared_tier": st.declared_tier,
            "models": {t: {"model": m["model"], "size_gb": m["size_gb"]} for t, m in HW.CATALOG.items()},
            "download": st.download, "speed": st.speed,
            "free": st.free, "locked": st.locked, "today": st.today, "validators": st.validators,
            "last_task": st.last_task, "paused": cfg["paused"], "backed_up": cfg["backed_up"],
            "chat": {**st.chat, "enabled": cfg["chat"], "today": cfg["day"].get("chats", 0)
                     if cfg["day"].get("date") == time.strftime("%Y-%m-%d") else 0},
            "request": {k: v for k, v in (cfg["request"] or {}).items() if k in ("name", "email", "status", "message")} or None,
            "log": list(st.log)[-60:], "unit": UNIT,
        }


def make_handler(node: Node):
    page = open(os.path.join(os.path.dirname(__file__), "dashboard.html"), "rb").read()
    cfg = node.cfg

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def host_ok(self) -> bool:
            host = (self.headers.get("Host") or "").lower()
            return host in (f"127.0.0.1:{cfg['port']}", f"localhost:{cfg['port']}")

        def authed(self) -> bool:
            c = http.cookies.SimpleCookie(self.headers.get("Cookie", ""))
            return "inc_node" in c and secrets.compare_digest(c["inc_node"].value, cfg["token"])

        def send(self, code, body=b"", ctype="application/json", headers=()):
            if isinstance(body, (dict, list)):
                body = json.dumps(body).encode()
            elif isinstance(body, str):
                body = body.encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Frame-Options", "DENY")
            self.send_header("Referrer-Policy", "no-referrer")
            for k, v in headers:
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if not self.host_ok():
                return self.send(403, {"error": "forbidden"})
            u = urlsplit(self.path)
            if u.path == "/":
                t = parse_qs(u.query).get("t", [""])[0]
                if t and secrets.compare_digest(t, cfg["token"]):
                    cookie = f"inc_node={cfg['token']}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000"
                    return self.send(303, headers=[("Location", "/"), ("Set-Cookie", cookie)])
                if not self.authed():
                    return self.send(403, "<h1>Open Incentium from the icon on your desktop.</h1>", "text/html")
                return self.send(200, page, "text/html; charset=utf-8")
            if not self.authed():
                return self.send(403, {"error": "forbidden"})
            if u.path == "/api/status":
                return self.send(200, status_json(node))
            if u.path == "/api/backup/file":
                return self.send(200, open(node.key_file, "rb").read(), "application/json",
                                 headers=[("Content-Disposition", 'attachment; filename="incentium-miner-key.json"')])
            self.send(404, {"error": "not found"})

        def do_POST(self):
            if not self.host_ok() or not self.authed() or self.headers.get("X-Incentium") != "1":
                return self.send(403, {"error": "forbidden"})
            n = int(self.headers.get("Content-Length") or 0)
            try:
                body = json.loads(self.rfile.read(min(n, 10000)) or b"{}")
            except ValueError:
                return self.send(400, {"error": "bad request"})
            p = urlsplit(self.path).path
            if p == "/api/request":
                name, email = str(body.get("name", "")).strip()[:100], str(body.get("email", "")).strip()[:200]
                if not name or "@" not in email or not body.get("understood"):
                    return self.send(400, {"error": "Fill in your name and email, and confirm that you have read the note."})
                cfg.set(request={"name": name, "email": email, "note": str(body.get("note", ""))[:500],
                                 "at": int(time.time()), "status": None, "message": "Sending your request…"})
                node.wake.set()
                return self.send(200, {"ok": True})
            if p == "/api/backup/phrase":
                return self.send(200, {"phrase": json.load(open(node.key_file))["secretPhrase"]})
            if p == "/api/backup/done":
                cfg.set(backed_up=True)
                return self.send(200, {"ok": True})
            if p == "/api/chat":
                cfg.set(chat=bool(body.get("enabled")))
                return self.send(200, {"ok": True})
            if p == "/api/pause":
                cfg.set(paused=bool(body.get("paused")))
                node.wake.set()
                return self.send(200, {"ok": True})
            self.send(404, {"error": "not found"})

    return H


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--home", default=os.path.expanduser("~/.incentium-node"))
    a = ap.parse_args()
    os.makedirs(a.home, exist_ok=True)
    cfg = Config(a.home)
    node = Node(cfg)
    signal.signal(signal.SIGTERM, lambda *_: (node.shutdown(), sys.exit(0)))
    srv = ThreadingHTTPServer(("127.0.0.1", cfg["port"]), make_handler(node))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    threading.Thread(target=node.chat_loop, daemon=True).start()
    node.st.say(f"Incentium Node on http://127.0.0.1:{cfg['port']} ({cfg['network']})")
    try:
        node.run()
        if not node.stop.is_set():          # after an error: show it for a while, then let the
            node.stop.wait(300)             # service manager start the node again
            sys.exit(1)
    finally:
        node.shutdown()


if __name__ == "__main__":
    main()
