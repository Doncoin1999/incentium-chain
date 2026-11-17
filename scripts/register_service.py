"""Sponsored miner registration for the launch period (spec Section 12).

While the launch period runs, only the launch operator can register neurons. This service lets
anyone who runs the miner install get a hotkey registered in SN0 by the operator, for free:

  GET  /          -> {"network", "genesis", "open"}   open = the service registers now
  POST /register  {"hotkey", "signature"}            signature = the hotkey signing
                  "incentium-register:<genesis>:<hotkey>" (proves the caller owns the key)

Limits: one registration per IP every 24 h, a daily total, tier-0 miners only (they declare no
tier). After the launch period anyone registers directly on chain, and the service answers 409.

  python scripts/register_service.py --url ws://127.0.0.1:9954 --port 8202
Behind Caddy:  register.incentium.org { reverse_proxy 127.0.0.1:8202 }
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Callable

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "neuron"))

from faucet import Limits, client_ip  # noqa: E402
from incentium_neuron.addr import pubkey  # noqa: E402
from incentium_neuron.setup_miner import registration_message  # noqa: E402


class Registrar:
    def __init__(self, genesis: str, register: Callable[[str], int], verify: Callable[[str, bytes, str], bool],
                 is_registered: Callable[[str], bool], is_open: Callable[[], bool], valid_address: Callable[[str], bool],
                 limits: Limits, network: str = "testnet"):
        self.genesis, self.register, self.verify = genesis, register, verify
        self.is_registered, self.is_open, self.valid_address = is_registered, is_open, valid_address
        self.limits, self.network = limits, network
        self.lock = threading.Lock()

    def info(self) -> dict:
        return {"network": self.network, "genesis": self.genesis, "open": self.is_open()}

    def handle(self, hotkey: str, signature: str, ip: str) -> tuple[int, dict]:
        if not hotkey or len(hotkey) > 64 or not self.valid_address(hotkey):
            return 400, {"error": "That is not a valid Incentium address."}
        if not self.verify(hotkey, registration_message(self.genesis, hotkey), signature or ""):
            return 403, {"error": "The signature does not match the hotkey."}
        if not self.is_open():
            return 409, {"error": "The launch period is over: register directly on chain."}
        with self.lock:
            if self.is_registered(hotkey):
                return 200, {"hotkey": hotkey, "already": True}
            err = self.limits.check(pubkey(hotkey), ip, 1)
            if err:
                return 429, {"error": err}
            try:
                block = self.register(hotkey)
            except Exception as e:  # subnet full, node down, ...
                return 503, {"error": f"Registration failed: {e}"}
            self.limits.record(pubkey(hotkey), ip, 1)
        return 200, {"hotkey": hotkey, "block": block}


def make_handler(r: Registrar):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def _send(self, code, obj):
            body = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path == "/":
                return self._send(200, r.info())
            self._send(404, {"error": "not found"})

        def do_POST(self):
            if self.path != "/register":
                return self._send(404, {"error": "not found"})
            try:
                n = int(self.headers.get("Content-Length", "0"))
                d = json.loads(self.rfile.read(min(n, 4096)))
                hotkey, sig = str(d.get("hotkey", "")).strip(), str(d.get("signature", "")).strip()
            except (ValueError, AttributeError):
                return self._send(400, {"error": "bad request"})
            code, body = r.handle(hotkey, sig, client_ip(self))
            print(f"register {hotkey} from {client_ip(self)}: {code}", flush=True)
            self._send(code, body)

    return H


def main():
    from substrateinterface import Keypair, SubstrateInterface
    from substrateinterface.utils.ss58 import ss58_decode

    sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "neuron"))
    from incentium_neuron.tasks import sr25519_verify

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9954")
    ap.add_argument("--port", type=int, default=8202)
    ap.add_argument("--operator", default="~/incentium-testnet/secrets/operator.json")
    ap.add_argument("--daily-total", type=int, default=50, help="registrations per 24 h for everyone together")
    ap.add_argument("--state", default="~/.incentium/register-state.json")
    ap.add_argument("--network", default="testnet")
    a = ap.parse_args()
    op = Keypair.create_from_mnemonic(json.load(open(os.path.expanduser(a.operator)))["secretPhrase"])
    conn = {"s": SubstrateInterface(url=a.url)}
    send_lock = threading.Lock()

    def q(module, name, params=None):
        for attempt in (1, 2):
            try:
                return conn["s"].query(module, name, params or []).value
            except (BrokenPipeError, ConnectionError, OSError):
                if attempt == 2:
                    raise
                conn["s"] = SubstrateInterface(url=a.url)

    def register(hotkey: str) -> int:
        with send_lock:
            s = conn["s"]
            call = s.compose_call("Registry", "register_neuron", {"netuid": 0, "hotkey": hotkey})
            r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=op), wait_for_inclusion=True)
            if not r.is_success:
                raise RuntimeError(str(r.error_message))
            return s.get_block_number(r.block_hash)

    def is_open() -> bool:
        until = q("Registry", "BootstrapUntil")
        return bool(until) and conn["s"].get_block_number(None) < until

    def valid(addr: str) -> bool:
        try:
            ss58_decode(addr)  # any SS58 format: ic... or 5...
            return True
        except Exception:
            return False

    genesis = conn["s"].get_block_hash(0)
    reg = Registrar(genesis, register, sr25519_verify, lambda hk: q("Registry", "NeuronUid", [0, hk]) is not None,
                    is_open, valid, Limits(os.path.expanduser(a.state), a.daily_total), a.network)
    print(f"registration service for {a.network} (genesis {genesis}), operator {op.ss58_address}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(reg)).serve_forever()


if __name__ == "__main__":
    main()
