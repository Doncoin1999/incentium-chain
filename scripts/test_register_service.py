import hashlib

from faucet import Limits
from register_service import Registrar

from incentium_neuron.setup_miner import registration_message

GEN = "0xabc"


def sign(hotkey, msg):
    return hashlib.sha256(hotkey.encode() + msg).hexdigest()


def verify(hotkey, msg, sig):
    return sign(hotkey, msg) == sig


def make(open_=True, daily=3):
    registered, calls = set(), []

    def register(hk):
        calls.append(hk)
        registered.add(hk)
        return 7

    r = Registrar(GEN, register, verify, lambda hk: hk in registered, lambda: open_,
                  lambda a: a.startswith("5"), Limits(None, daily))
    return r, calls


def req(r, hk, ip, good=True):
    sig = sign(hk if good else "mallory", registration_message(GEN, hk))
    return r.handle(hk, sig, ip)


def test_registers_signed_requests_once_per_ip_and_day():
    r, calls = make()
    assert req(r, "nope", "1.1.1.1")[0] == 400
    assert req(r, "5Alice", "1.1.1.1", good=False)[0] == 403
    assert req(r, "5Alice", "1.1.1.1") == (200, {"hotkey": "5Alice", "block": 7})
    assert req(r, "5Alice", "2.2.2.2") == (200, {"hotkey": "5Alice", "already": True})
    assert req(r, "5Bob", "1.1.1.1")[0] == 429  # same IP within 24 h
    assert req(r, "5Bob", "2.2.2.2")[0] == 200
    assert req(r, "5Carol", "3.3.3.3")[0] == 200
    assert req(r, "5Dave", "4.4.4.4")[0] == 429  # daily total of 3 reached
    assert calls == ["5Alice", "5Bob", "5Carol"]


def test_signature_is_bound_to_the_network():
    r, _ = make()
    other = sign("5Alice", registration_message("0xother", "5Alice"))
    assert r.handle("5Alice", other, "1.1.1.1")[0] == 403


def test_closed_after_the_launch_period():
    r, calls = make(open_=False)
    assert req(r, "5Alice", "1.1.1.1")[0] == 409
    assert calls == []
