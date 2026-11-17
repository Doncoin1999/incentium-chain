import json
import threading
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer

from faucet import DAY, UNIT, Faucet, Limits, make_handler


class Clock:
    def __init__(self):
        self.t = 1_000_000.0

    def __call__(self):
        return self.t


def make(tmp_path, clock, daily_total=25):
    sent = []

    def send(dest, amount):
        sent.append((dest, amount))
        return 42

    limits = Limits(str(tmp_path / "state.json"), daily_total * UNIT, now=clock)
    return Faucet(send, lambda a: a.startswith("5"), limits, 10 * UNIT), sent


def test_limits_per_address_ip_and_day(tmp_path):
    clock = Clock()
    f, sent = make(tmp_path, clock)
    assert f.drip("not-an-address", "1.1.1.1")[0] == 400
    assert f.drip("5Alice", "1.1.1.1") == (200, {"amount": 10, "block": 42})
    assert f.drip("5Alice", "2.2.2.2")[0] == 429  # same address
    assert f.drip("5Bob", "1.1.1.1")[0] == 429  # same IP
    assert f.drip("5Bob", "2.2.2.2")[0] == 200
    assert f.drip("5Carol", "3.3.3.3")[0] == 429  # 30 ICT would pass the daily total of 25
    clock.t += DAY + 1
    assert f.drip("5Alice", "1.1.1.1")[0] == 200
    assert [d for d, _ in sent] == ["5Alice", "5Bob", "5Alice"]


def test_limits_survive_restart(tmp_path):
    clock = Clock()
    f, _ = make(tmp_path, clock)
    assert f.drip("5Alice", "1.1.1.1")[0] == 200
    f2, _ = make(tmp_path, clock)
    assert f2.drip("5Alice", "9.9.9.9")[0] == 429


def test_failed_transfer_is_not_counted(tmp_path):
    def broken(dest, amount):
        raise RuntimeError("node down")

    f = Faucet(broken, lambda a: True, Limits(None, 100 * UNIT), 10 * UNIT)
    assert f.drip("5Alice", "1.1.1.1")[0] == 503
    f.send = lambda d, a: 7
    assert f.drip("5Alice", "1.1.1.1")[0] == 200


def test_http_page_and_drip_behind_proxy(tmp_path):
    clock = Clock()
    f, sent = make(tmp_path, clock)
    srv = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(f))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{srv.server_address[1]}"
    try:
        assert b"Incentium faucet" in urllib.request.urlopen(url + "/").read()

        def post(address, xff):
            req = urllib.request.Request(url + "/drip", data=json.dumps({"address": address}).encode(),
                                         headers={"Content-Type": "application/json", "X-Forwarded-For": xff})
            try:
                return urllib.request.urlopen(req).status
            except urllib.error.HTTPError as e:
                return e.code

        assert post("5Alice", "8.8.8.8") == 200
        # A spoofed first hop does not help: the proxy appends the real client last.
        assert post("5Bob", "6.6.6.6, 8.8.8.8") == 429
    finally:
        srv.shutdown()
