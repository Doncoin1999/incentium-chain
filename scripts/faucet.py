"""Incentium test-ICT faucet (devnet and testnet only; never on mainnet).

  python scripts/faucet.py --url ws://127.0.0.1:9944 --port 8200

On first start it creates its own key in ~/.incentium/faucet-key and prints the address;
fund that address once (e.g. from //Alice on devnet). Behind Caddy:
  faucet.incentium.org { reverse_proxy 127.0.0.1:8200 }

Limits: one drip per address and per IP every 24 h, and a daily total.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Callable

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "neuron"))
from incentium_neuron.addr import pubkey, to_incentium  # noqa: E402

UNIT = 10**9
DAY = 24 * 3600

PAGE = """<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Incentium faucet</title>
<style>
body{font-family:system-ui,sans-serif;max-width:560px;margin:8vh auto;padding:0 20px;color:#1d1d1f}
h1{font-size:1.6rem;margin-bottom:.2rem}p{color:#555;line-height:1.5}
input{width:100%;box-sizing:border-box;padding:12px;font-size:1rem;border:1px solid #ccc;border-radius:8px}
button{margin-top:12px;padding:12px 20px;font-size:1rem;border:0;border-radius:8px;background:#e6007a;color:#fff;cursor:pointer}
button:disabled{opacity:.5}#msg{margin-top:16px;min-height:1.5em}.ok{color:#137333}.err{color:#b3261e}
small{color:#888}
</style></head><body>
<h1>Incentium faucet</h1>
<p>Get <b>__AMOUNT__ test ICT</b> on the Incentium __NETWORK__. Test ICT has no value.
Paste your address from Talisman, SubWallet or Polkadot.js. One request per address and per
IP every 24 hours.</p>
<input id="a" placeholder="Your Incentium address (5...)" autocomplete="off" spellcheck="false">
<button id="b">Send me test ICT</button>
<div id="msg"></div>
<p><small>Network: <code>wss://rpc.incentium.org</code> &middot;
<a href="https://explorer.incentium.org">Explorer</a></small></p>
<script>
const b=document.getElementById('b'),m=document.getElementById('msg');
b.onclick=async()=>{b.disabled=true;m.className='';m.textContent='Sending...';
 try{const r=await fetch('/drip',{method:'POST',headers:{'Content-Type':'application/json'},
  body:JSON.stringify({address:document.getElementById('a').value.trim()})});
  const j=await r.json();m.className=r.ok?'ok':'err';
  m.textContent=r.ok?`Sent ${j.amount} ICT in block ${j.block}.`:j.error;}
 catch(e){m.className='err';m.textContent='Could not reach the faucet.';}
 b.disabled=false;};
</script></body></html>"""


class Limits:
    """Who got what and when, kept in a JSON file so restarts don't reset it."""

    def __init__(self, path: str | None, per_day_total: int, now: Callable[[], float] = time.time):
        self.path, self.per_day_total, self.now = path, per_day_total, now
        self.last: dict[str, float] = {}
        self.sent: list[tuple[float, int]] = []
        if path and os.path.exists(path):
            try:
                d = json.load(open(path))
                self.last, self.sent = d.get("last", {}), [tuple(x) for x in d.get("sent", [])]
            except (OSError, ValueError):
                pass

    def check(self, address: str, ip: str, amount: int) -> str | None:
        t = self.now()
        for k in ("addr:" + address, "ip:" + ip):
            if t - self.last.get(k, 0) < DAY:
                wait = int((DAY - (t - self.last[k])) / 3600) + 1
                return f"Already used in the last 24 hours. Try again in about {wait} h."
        if sum(a for ts, a in self.sent if t - ts < DAY) + amount > self.per_day_total:
            return "The faucet has reached today's limit. Try again tomorrow."
        return None

    def record(self, address: str, ip: str, amount: int) -> None:
        t = self.now()
        self.last["addr:" + address] = self.last["ip:" + ip] = t
        self.last = {k: v for k, v in self.last.items() if t - v < DAY}
        self.sent = [(ts, a) for ts, a in self.sent if t - ts < DAY] + [(t, amount)]
        if self.path:
            tmp = self.path + ".tmp"
            json.dump({"last": self.last, "sent": self.sent}, open(tmp, "w"))
            os.replace(tmp, self.path)


class Faucet:
    def __init__(self, send: Callable[[str, int], int], valid_address: Callable[[str], bool],
                 limits: Limits, amount: int, network: str = "devnet"):
        self.send, self.valid_address, self.limits = send, valid_address, limits
        self.amount, self.network = amount, network
        self.lock = threading.Lock()  # one transfer at a time: no nonce clashes

    def drip(self, address: str, ip: str) -> tuple[int, dict]:
        if not address or len(address) > 64 or not self.valid_address(address):
            return 400, {"error": "That is not a valid Incentium address."}
        with self.lock:
            err = self.limits.check(pubkey(address), ip, self.amount)
            if err:
                return 429, {"error": err}
            try:
                block = self.send(address, self.amount)
            except Exception as e:  # node down, faucet empty, ...
                return 503, {"error": f"Transfer failed: {e}"}
            self.limits.record(pubkey(address), ip, self.amount)
        return 200, {"amount": self.amount // UNIT, "block": block}


def client_ip(handler: BaseHTTPRequestHandler) -> str:
    """Behind Caddy on localhost the client is the last X-Forwarded-For hop."""
    peer = handler.client_address[0]
    xff = handler.headers.get("X-Forwarded-For")
    if peer in ("127.0.0.1", "::1") and xff:
        return xff.split(",")[-1].strip()
    return peer


def make_handler(f: Faucet):
    page = PAGE.replace("__AMOUNT__", str(f.amount // UNIT)).replace("__NETWORK__", f.network).encode()

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def _send(self, code, body: bytes, ctype="application/json"):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path in ("/", "/index.html"):
                return self._send(200, page, "text/html; charset=utf-8")
            self._send(404, b'{"error":"not found"}')

        def do_POST(self):
            if self.path != "/drip":
                return self._send(404, b'{"error":"not found"}')
            try:
                n = int(self.headers.get("Content-Length", "0"))
                address = str(json.loads(self.rfile.read(min(n, 4096))).get("address", "")).strip()
            except (ValueError, AttributeError):
                return self._send(400, b'{"error":"bad request"}')
            code, body = f.drip(address, client_ip(self))
            print(f"drip {address} from {client_ip(self)}: {code}", flush=True)
            self._send(code, json.dumps(body).encode())

    return H


def load_or_create_key(path: str):
    from substrateinterface import Keypair

    path = os.path.expanduser(path)
    if os.path.exists(path):
        return Keypair.create_from_mnemonic(open(path).read().strip())
    os.makedirs(os.path.dirname(path), exist_ok=True)
    mnemonic = Keypair.generate_mnemonic()
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT, 0o600), "w") as fh:
        fh.write(mnemonic)
    return Keypair.create_from_mnemonic(mnemonic)


def main():
    from substrateinterface import SubstrateInterface
    from substrateinterface.utils.ss58 import ss58_decode

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--port", type=int, default=8200)
    ap.add_argument("--amount", type=int, default=10, help="ICT per drip")
    ap.add_argument("--daily-total", type=int, default=5_000, help="ICT per 24 h for everyone together")
    ap.add_argument("--network", default="devnet")
    ap.add_argument("--key-file", default="~/.incentium/faucet-key")
    ap.add_argument("--state", default="~/.incentium/faucet-state.json")
    a = ap.parse_args()

    kp = load_or_create_key(a.key_file)
    lock = threading.Lock()
    conn = {"s": SubstrateInterface(url=a.url)}

    def send(dest: str, amount: int) -> int:
        with lock:
            for attempt in (1, 2):
                try:
                    s = conn["s"]
                    call = s.compose_call("Balances", "transfer_keep_alive", {"dest": dest, "value": amount})
                    r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
                    if not r.is_success:
                        raise RuntimeError(str(r.error_message))
                    return s.get_block_number(r.block_hash)
                except (BrokenPipeError, ConnectionError, OSError):
                    if attempt == 2:
                        raise
                    conn["s"] = SubstrateInterface(url=a.url)

    def valid(address: str) -> bool:
        try:
            ss58_decode(address)  # any SS58 format: ic... or 5...
            return True
        except Exception:
            return False

    acct = conn["s"].query("System", "Account", [kp.ss58_address]).value["data"]["free"]
    print(f"faucet address {to_incentium(kp.ss58_address)}, balance {acct / UNIT:,.2f} ICT", flush=True)
    f = Faucet(send, valid, Limits(os.path.expanduser(a.state), a.daily_total * UNIT), a.amount * UNIT, a.network)
    ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(f)).serve_forever()


if __name__ == "__main__":
    main()
