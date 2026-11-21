"""End-to-end test of the Incentium Node with a fake chain, model server, validator and download.

  python -m pytest neuron/tests/test_node.py
"""
import http.client
import json
import os
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from incentium_neuron import hw as HW  # noqa: E402
from incentium_neuron import node as N  # noqa: E402
from incentium_neuron.validator import TierInfo  # noqa: E402

FAKE_LLAMA = r'''
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
port = int(sys.argv[sys.argv.index("--port") + 1])
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b"{}")
    def do_POST(self):
        self.rfile.read(int(self.headers["Content-Length"]))
        b = json.dumps({"choices": [{"text": " a test", "logprobs": {}}], "usage": {"completion_tokens": 2},
                        "timings": {"predicted_per_second": 12.5}}).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers(); self.wfile.write(b)
HTTPServer(("127.0.0.1", port), H).serve_forever()
'''


def serve(handler):
    s = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=s.serve_forever, daemon=True).start()
    return s, f"http://127.0.0.1:{s.server_address[1]}"


class Files(BaseHTTPRequestHandler):
    data = b"GGUF" + b"x" * 300_000

    def log_message(self, *a):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.data)))
        self.end_headers()
        self.wfile.write(self.data)


class Validator(BaseHTTPRequestHandler):
    results = []

    def log_message(self, *a):
        pass

    def do_GET(self):
        b = json.dumps({"epoch": 1, "deadline": time.time() + 600,
                        "tasks": [{"task_id": "t1", "prompt": "p", "max_tokens": 8, "seed": 1}]}).encode()
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b)

    def do_POST(self):
        Validator.results.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"{}")


class FakeChain:
    def __init__(self, validator_url):
        self.registered = []
        self.v = validator_url
        self.s = SimpleNamespace(query=self.query)

    def tiers(self):
        return {0: TierInfo(0.15, 0.35, "qwen3.5-2b", "0" * 64, True),
                1: TierInfo(1.0, 0.35, "qwen3.5-9b", "0" * 64, False)}

    def miners(self, netuid):
        return [(7, a, "", 0) for a in self.registered]

    def validator_endpoints(self, netuid):
        return [self.v]

    def query(self, pallet, item, args):
        if pallet == "System":
            return SimpleNamespace(value={"data": {"free": 3 * N.UNIT}})
        return SimpleNamespace(value=9 * N.UNIT)


def wait_for(cond, secs=40):
    t = time.time()
    while time.time() - t < secs:
        if cond():
            return True
        time.sleep(0.2)
    return False


def test_node_end_to_end(monkeypatch):
    home = tempfile.mkdtemp()
    fake = os.path.join(home, "llama-server")
    open(fake, "w").write(f"#!{sys.executable}\n" + FAKE_LLAMA)
    os.chmod(fake, 0o755)
    _, files = serve(Files)
    _, vurl = serve(Validator)
    monkeypatch.setitem(HW.CATALOG, 0, {**HW.CATALOG[0], "url": files + "/Qwen_Qwen3.5-2B-Q4_K_M.gguf"})
    cfg = N.Config(home)
    port = N.free_port(17777)
    cfg.set(llama_server=fake, port=port, llama_port=N.free_port(18096))
    chain = FakeChain(vurl)
    sent = []

    def register(contact):
        sent.append(contact)
        chain.registered.append(node.st.address)
        return 200, {"ok": True}

    big_mac = HW.Hardware("macos", "arm64", "Apple M2", 8, 16, "Apple M2 (Metal)", 0, True, 200)
    node = N.Node(cfg, chain_factory=lambda url: chain, register_fn=register, detect=lambda home: big_mac)
    cfg.set(overrides={"chat": "http://gateway.test"})
    chats = []

    def chat_serve(gateway, hotkey, sign, tier, model, llama_url):
        chats.append((gateway, hotkey, tier, model, llama_url, sign(b"x")))
        time.sleep(0.2)
        return 1

    node.chat_serve = chat_serve
    srv = ThreadingHTTPServer(("127.0.0.1", port), N.make_handler(node))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    threading.Thread(target=node.run, daemon=True).start()
    threading.Thread(target=node.chat_loop, daemon=True).start()
    try:
        # it downloads tier 0 (tier 1 fits this Mac but is closed on chain), runs it and asks to join
        assert wait_for(lambda: node.st.phase == "request"), (node.st.phase, node.st.message, list(node.st.log))
        assert node.st.max_tier == 1 and node.st.running_tier == 0 and node.st.speed == 12.5
        assert os.path.exists(os.path.join(home, "models", "Qwen_Qwen3.5-2B-Q4_K_M.gguf"))

        def call(method, path, body=None, token=None, host=None, hdr=True):
            c = http.client.HTTPConnection("127.0.0.1", port)
            h = {"Host": host or f"127.0.0.1:{port}"}
            if token:
                h["Cookie"] = f"inc_node={token}"
            if hdr:
                h["X-Incentium"] = "1"
            c.request(method, path, body=json.dumps(body) if body is not None else None, headers=h)
            r = c.getresponse()
            return r.status, dict(r.getheaders()), r.read()

        tok = cfg["token"]
        # without the token, or from another host name, nothing is served
        assert call("GET", "/api/status")[0] == 403
        assert call("GET", "/api/status", token=tok, host="evil.example")[0] == 403
        assert call("POST", "/api/backup/phrase", {}, token=tok, hdr=False)[0] == 403
        # the link with ?t= sets the cookie
        s, h, _ = call("GET", f"/?t={tok}")
        assert s == 303 and "inc_node=" in h["Set-Cookie"] and "SameSite=Strict" in h["Set-Cookie"]
        st = json.loads(call("GET", "/api/status", token=tok)[2])
        assert st["phase"] == "request" and st["free"] == 3 * N.UNIT and st["locked"] == 9 * N.UNIT
        # request needs name, email and the launch-lock confirmation
        assert call("POST", "/api/request", {"name": "Far", "email": "far@example.org"}, token=tok)[0] == 400
        assert call("POST", "/api/request", {"name": "Far", "email": "far@example.org", "understood": True}, token=tok)[0] == 200
        # approved -> registered -> mining, answering the validator's task
        assert wait_for(lambda: node.st.phase == "mining" and Validator.results), (node.st.phase, list(node.st.log))
        assert sent[0]["email"] == "far@example.org" and node.st.uid == 7
        assert Validator.results[0]["hotkey"] == node.st.address
        # while mining it also answers chat visitors with the same model, signed by its hotkey
        assert wait_for(lambda: chats), list(node.st.log)
        gw, hk, tier, model, url, sig = chats[0]
        assert (gw, hk, tier, model, url) == ("http://gateway.test", node.st.address, 0, "qwen3.5-2b", node.llama_url)
        assert len(sig) == 128
        assert wait_for(lambda: json.loads(call("GET", "/api/status", token=tok)[2])["chat"]["today"] >= 1)
        # it can be switched off from the dashboard
        assert call("POST", "/api/chat", {"enabled": False}, token=tok)[0] == 200
        assert wait_for(lambda: json.loads(call("GET", "/api/status", token=tok)[2])["chat"]["state"] == "off")
        n = len(chats)
        time.sleep(0.6)
        assert len(chats) <= n + 1
        call("POST", "/api/chat", {"enabled": True}, token=tok)
        # backup: words once, then marked done
        words = json.loads(call("POST", "/api/backup/phrase", {}, token=tok)[2])["phrase"].split()
        assert len(words) == 12
        call("POST", "/api/backup/done", {}, token=tok)
        assert cfg["backed_up"]
        # pause stops the model, resume starts it again
        call("POST", "/api/pause", {"paused": True}, token=tok)
        assert wait_for(lambda: node.st.phase == "paused" and node.llama is None)
        call("POST", "/api/pause", {"paused": False}, token=tok)
        assert wait_for(lambda: node.st.phase == "mining")
    finally:
        node.shutdown()
        srv.shutdown()


def test_tier_choice():
    pc = HW.Hardware("windows", "x64", "CPU", 8, 16, "", 0, False, 100)
    assert pc.max_tier() == 0                                   # no GPU: tier 0
    rtx = HW.Hardware("windows", "x64", "CPU", 8, 16, "RTX 3060", 12, False, 100)
    assert rtx.max_tier() == 1
    m1 = HW.Hardware("macos", "arm64", "Apple M1", 4, 8, "Apple M1 (Metal)", 0, True, 100)
    assert m1.max_tier() == 0
    m3 = HW.Hardware("macos", "arm64", "Apple M3 Max", 12, 64, "Apple M3 Max (Metal)", 0, True, 100)
    assert m3.max_tier() == 2
    assert HW.choose_tier(m3, {0}) == 0 and HW.choose_tier(m3, {0, 1}) == 1
    tiny = HW.Hardware("linux", "x64", "CPU", 2, 2, "", 0, False, 100)
    assert not tiny.can_run_at_all()[0]
    full = HW.Hardware("macos", "arm64", "Apple M3 Max", 12, 64, "x", 0, True, 3)
    assert full.max_tier() == 0                                 # not enough disk for the bigger files
