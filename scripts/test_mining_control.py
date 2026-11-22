"""Tests for the mining control board: python -m pytest scripts/test_mining_control.py"""
import http.client
import json
import os
import sys
import threading
import time
from http.server import ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(__file__))
import mining_control as M  # noqa: E402
from incentium_neuron import control  # noqa: E402

HK1 = "5" + "A" * 47
HK2 = "5" + "B" * 47
VAL = "5" + "V" * 47


class FakeReader:
    def __init__(self, first=None):
        self.first = first if first is not None else time.time() - 10 * 86400

    def first_block_time(self):
        return self.first

    def miners(self):
        return [{"uid": 0, "hotkey": VAL, "tier": 0, "endpoint": "http://v", "balance": 20.0, "validator": True},
                {"uid": 1, "hotkey": HK1, "tier": 0, "endpoint": "", "balance": 1.5, "validator": False},
                {"uid": 2, "hotkey": HK2, "tier": 1, "endpoint": "", "balance": 0.0, "validator": False}]


def serve(tmp_path, first=None):
    state = str(tmp_path / "validator-testnet-state.json")
    net = M.Network("testnet", FakeReader(first), state)
    srv = ThreadingHTTPServer(("127.0.0.1", 0), M.make_handler({"testnet": net}))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return net, srv, state


def req(port, method, path, body=None, headers=None):
    c = http.client.HTTPConnection("127.0.0.1", port)
    h = {"Content-Type": "application/json", "Origin": "https://control.incentium.org"}
    h.update(headers or {})
    c.request(method, path, body=json.dumps(body) if body is not None else None, headers=h)
    r = c.getresponse()
    return r.status, json.loads(r.read() or b"null")


def test_window_is_90_days_from_first_block_and_written_once(tmp_path):
    net, srv, state = serve(tmp_path)
    try:
        s = net.state()
        assert s["open"] and s["days_left"] == 80
        until = s["until"]
        net.start_override = time.time()  # a later start never moves the window
        assert net.state()["until"] == until
        assert control.load(control.blocklist_path(state))["until"] == until
    finally:
        srv.shutdown()


def test_switch_off_and_on(tmp_path):
    net, srv, state = serve(tmp_path)
    port = srv.server_address[1]
    try:
        assert req(port, "POST", "/api/switch", {"net": "testnet", "hotkey": HK1, "off": True}) == (200, {"ok": True, "state": "off"})
        assert control.blocked_now(control.blocklist_path(state)) == {HK1}
        code, d = req(port, "GET", "/api/state")
        m = {x["uid"]: x for x in d["networks"][0]["miners"]}
        assert m[1]["off"] and not m[2]["off"] and m[0]["validator"]
        assert req(port, "POST", "/api/switch", {"net": "testnet", "hotkey": HK1, "off": False})[0] == 200
        assert control.blocked_now(control.blocklist_path(state)) == set()
    finally:
        srv.shutdown()


def test_switch_refused_after_window_and_from_other_origins(tmp_path):
    net, srv, state = serve(tmp_path, first=time.time() - 91 * 86400)
    port = srv.server_address[1]
    try:
        assert not net.state()["open"]
        assert req(port, "POST", "/api/switch", {"net": "testnet", "hotkey": HK1, "off": True})[0] == 409
        assert req(port, "POST", "/api/switch", {"net": "testnet", "hotkey": HK1, "off": True},
                   headers={"Origin": "https://evil.example"})[0] == 403
        assert req(port, "POST", "/api/switch", {"net": "nope", "hotkey": HK1, "off": True})[0] == 400
        assert req(port, "POST", "/api/switch", {"net": "testnet", "hotkey": "x/../y", "off": True})[0] == 400
    finally:
        srv.shutdown()


def test_parse_net():
    name, url, state, start = M.parse_net("mainnet=ws://127.0.0.1:9944=~/s-state.json=2026-12-01")
    assert (name, url) == ("mainnet", "ws://127.0.0.1:9944") and state.endswith("s-state.json")
    assert time.strftime("%Y-%m-%d", time.gmtime(start)) == "2026-12-01"
    assert M.parse_net("t=ws://x=/s.json=")[3] is None
