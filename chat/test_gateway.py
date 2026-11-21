"""End-to-end tests of the chat gateway with real sr25519 miner keys, the node's chat client
(incentium_neuron.chat.serve_once) and a fake llama-server that streams a fixed answer."""
from __future__ import annotations

import json
import os
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "..", "neuron"))

import gateway as G  # noqa: E402
from incentium_neuron import chat as C  # noqa: E402

substrateinterface = pytest.importorskip("substrateinterface")
from substrateinterface import Keypair  # noqa: E402

PIECES = ["Hello", " from", " the", " network", "!"]


class FakeLlama:
    """Streams PIECES as an OpenAI chat completion; remembers the last request body."""

    def __init__(self, delay=0.01):
        self.last = None
        outer = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                outer.last = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                for p in PIECES:
                    ev = {"choices": [{"index": 0, "delta": {"content": p}, "finish_reason": None}]}
                    self.wfile.write(b"data: " + json.dumps(ev).encode() + b"\n\n")
                    self.wfile.flush()
                    time.sleep(delay)
                self.wfile.write(b"data: [DONE]\n\n")

        self.srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.srv.server_address[1]}"


def start(limits=None, registry=None, fallback=None):
    gw = G.Gateway(limits or G.Limits(poll_hold=1.0, pickup_timeout=1.0, queue_timeout=4.0,
                                      first_piece_timeout=1.5, idle_timeout=2.0),
                   registry=registry, fallback=fallback)
    srv = G.serve(gw, "127.0.0.1", 0)
    return gw, f"http://127.0.0.1:{srv.server_address[1]}"


class MinerThread:
    def __init__(self, base, llama, tier=0, kp=None):
        self.kp = kp or Keypair.create_from_mnemonic(Keypair.generate_mnemonic())
        self.stop, self.answered, self.error = threading.Event(), 0, None
        model = G.CATALOG[tier]["model"]

        def run():
            while not self.stop.is_set():
                try:
                    self.answered += C.serve_once(base, self.kp.ss58_address, lambda m: self.kp.sign(m).hex(),
                                                  tier, model, llama, timeout=10)
                except Exception as e:      # noqa: BLE001
                    self.error = e
                    return

        threading.Thread(target=run, daemon=True).start()


def post(base, body, timeout=20):
    req = urllib.request.Request(base + "/v1/chat/completions", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    return urllib.request.urlopen(req, timeout=timeout)


def stream(base, body):
    """(text, last incentium info) from a streamed request."""
    text, info = "", {}
    with post(base, {**body, "stream": True}) as r:
        assert r.headers["Content-Type"] == "text/event-stream"
        for raw in r:
            line = raw.decode().strip()
            if not line.startswith("data:") or line == "data: [DONE]":
                continue
            ev = json.loads(line[5:])
            info.update({k: v for k, v in (ev.get("incentium") or {}).items() if v is not None})
            text += ev["choices"][0]["delta"].get("content") or ""
    return text, info


def get(base, path):
    with urllib.request.urlopen(base + path, timeout=10) as r:
        return json.loads(r.read())


def wait_online(base, n=1):
    for _ in range(50):
        if get(base, "/api/status")["models"] and get(base, "/api/status")["models"][0]["miners"] >= n:
            return
        time.sleep(0.1)
    raise AssertionError("miner never came online")


USER = [{"role": "user", "content": "Hi"}]


def test_no_miners_means_503_and_no_models():
    gw, base = start()
    assert get(base, "/v1/models")["data"] == []
    with pytest.raises(urllib.error.HTTPError) as e:
        post(base, {"messages": USER})
    assert e.value.code == 503
    assert get(base, "/api/status")["quota"]["used"] == 0
    gw.stop.set()


def test_page_is_served():
    gw, base = start()
    with urllib.request.urlopen(base + "/", timeout=5) as r:
        assert b"Incentium Chat" in r.read()
    gw.stop.set()


def test_miner_answers_streamed_and_plain():
    llama = FakeLlama()
    gw, base = start()
    m = MinerThread(base, llama.url)
    wait_online(base)
    models = get(base, "/v1/models")["data"]
    assert [x["id"] for x in models] == ["auto", "fast"]
    text, info = stream(base, {"model": "auto", "messages": USER})
    assert text == "".join(PIECES)
    assert info["served_by"].startswith("miner " + m.kp.ss58_address[:6])
    assert info["tier"] == 0
    # the miner got the conversation and the gateway's cap on answer length
    assert llama.last["messages"] == USER and llama.last["max_tokens"] == G.Limits().max_tokens
    with post(base, {"model": "fast", "messages": USER}) as r:
        out = json.loads(r.read())
    assert out["choices"][0]["message"]["content"] == "".join(PIECES)
    assert out["model"] == "qwen3.5-2b"
    st = get(base, "/api/status")
    assert st["served"]["miner"] == 2 and st["quota"]["used"] == 2
    assert m.error is None
    m.stop.set()
    gw.stop.set()


def test_unknown_or_offline_model_is_refused():
    llama = FakeLlama()
    gw, base = start()
    MinerThread(base, llama.url)
    wait_online(base)
    for name, code in (("pro", 503), ("gpt-4", 404)):
        with pytest.raises(urllib.error.HTTPError) as e:
            post(base, {"model": name, "messages": USER})
        assert e.value.code == code
    gw.stop.set()


def test_daily_quota():
    llama = FakeLlama()
    gw, base = start(G.Limits(free_per_day=2, poll_hold=1.0))
    MinerThread(base, llama.url)
    wait_online(base)
    for _ in range(2):
        assert stream(base, {"messages": USER})[0]
    with pytest.raises(urllib.error.HTTPError) as e:
        post(base, {"messages": USER})
    assert e.value.code == 429
    assert get(base, "/api/status")["quota"]["left"] == 0
    gw.stop.set()


def test_registry_admits_only_registered_miners_on_their_declared_tier():
    good = Keypair.create_from_mnemonic(Keypair.generate_mnemonic())
    declared = {good.ss58_address: 0}
    gw, base = start(registry=lambda hk: declared.get(hk))
    llama = FakeLlama()
    stranger = Keypair.create_from_mnemonic(Keypair.generate_mnemonic())
    sign = lambda kp: (lambda m: kp.sign(m).hex())  # noqa: E731
    with pytest.raises(PermissionError, match="not a registered"):
        C.serve_once(base, stranger.ss58_address, sign(stranger), 0, "qwen3.5-2b", llama.url)
    with pytest.raises(PermissionError, match="declared tier 0"):
        C.serve_once(base, good.ss58_address, sign(good), 1, "qwen3.5-9b", llama.url)
    with pytest.raises(PermissionError, match="does not serve"):
        C.serve_once(base, good.ss58_address, sign(good), 0, "qwen3.5-9b", llama.url)
    # a signature from another key is refused
    with pytest.raises(PermissionError, match="bad signature"):
        C.serve_once(base, good.ss58_address, sign(stranger), 0, "qwen3.5-2b", llama.url)
    MinerThread(base, llama.url, kp=good)
    wait_online(base)
    assert stream(base, {"messages": USER})[0] == "".join(PIECES)
    gw.stop.set()


def test_fallback_answers_when_no_miner_is_online():
    llama = FakeLlama()
    gw, base = start(fallback=llama.url)
    assert [x["id"] for x in get(base, "/v1/models")["data"]] == ["auto", "fast"]
    text, info = stream(base, {"messages": USER})
    assert text == "".join(PIECES) and info["served_by"] == "fallback"
    assert get(base, "/api/status")["served"]["fallback"] == 1
    gw.stop.set()


def test_silent_miner_loses_the_job_to_another():
    llama = FakeLlama()
    gw, base = start()
    lazy = Keypair.create_from_mnemonic(Keypair.generate_mnemonic())
    got = {}

    def take_and_sleep():           # takes a job, never answers
        ts = int(time.time())
        req = {"hotkey": lazy.ss58_address, "tier": 0, "model": "qwen3.5-2b", "ts": ts,
               "signature": lazy.sign(C.poll_message(lazy.ss58_address, 0, "qwen3.5-2b", ts)).hex()}
        for _ in range(20):
            r = urllib.request.urlopen(urllib.request.Request(base + "/miner/poll", data=json.dumps(req).encode()), timeout=10)
            if r.status == 200:
                got["job"] = json.loads(r.read())["job"]
                return

    t = threading.Thread(target=take_and_sleep, daemon=True)
    t.start()
    wait_online(base)
    result = {}
    threading.Thread(target=lambda: result.update(zip(("text", "info"), stream(base, {"messages": USER}))), daemon=True).start()
    t.join(10)
    assert "job" in got
    MinerThread(base, llama.url)            # a working miner arrives
    for _ in range(100):
        if "text" in result:
            break
        time.sleep(0.1)
    assert result["text"] == "".join(PIECES)
    assert not result["info"]["served_by"].startswith("miner " + lazy.ss58_address[:6])
    # the lazy miner's late answer is refused: the job was handed on with a new token
    late = urllib.request.Request(base + "/miner/chunk", data=json.dumps(
        {"id": got["job"]["id"], "token": got["job"]["token"], "delta": "x", "done": True}).encode())
    with pytest.raises(urllib.error.HTTPError) as e:
        urllib.request.urlopen(late, timeout=5)
    assert e.value.code in (404, 410)
    gw.stop.set()


def test_cancel_stops_the_miner_and_refunds_an_unstarted_answer():
    gw = G.Gateway(G.Limits(poll_hold=0.2))
    kp = Keypair.create_from_mnemonic(Keypair.generate_mnemonic())
    # no miner online: put one "online" by polling once, then submit
    ts = int(time.time())
    gw.poll({"hotkey": kp.ss58_address, "tier": 0, "model": "qwen3.5-2b", "ts": ts,
             "signature": kp.sign(C.poll_message(kp.ss58_address, 0, "qwen3.5-2b", ts)).hex()})
    job = gw.submit("1.2.3.4", "fast", USER)
    taken = gw.poll({"hotkey": kp.ss58_address, "tier": 0, "model": "qwen3.5-2b", "ts": ts,
                     "signature": kp.sign(C.poll_message(kp.ss58_address, 0, "qwen3.5-2b", ts)).hex()})
    assert taken is job and gw.quota("1.2.3.4")["used"] == 1
    gw.cancel(job)
    assert gw.chunk({"id": job.id, "token": job.token, "delta": "late"}) == 410
    assert gw.quota("1.2.3.4")["used"] == 0
    gw.stop.set()


def test_long_history_is_trimmed_but_kept_valid():
    msgs = [{"role": "system", "content": "s"}] + [
        {"role": "user" if i % 2 == 0 else "assistant", "content": "x" * 1000} for i in range(30)] + [
        {"role": "user", "content": "last"}]
    out = G.clean_messages(msgs, 5000)
    assert out[0]["role"] == "system" and out[-1]["content"] == "last"
    assert sum(len(m["content"]) for m in out) <= 5000
    with pytest.raises(G.Refused):
        G.clean_messages([{"role": "assistant", "content": "no user"}], 100)
    with pytest.raises(G.Refused):
        G.clean_messages([{"role": "tool", "content": "x"}], 100)
