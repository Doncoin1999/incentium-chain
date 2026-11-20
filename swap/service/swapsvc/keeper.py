"""Incentium Swap keeper and API (spec Section 8).

One process that
- follows both chains and keeps the order book in SQLite,
- relays secrets: once a maker's confirmation reveals a secret on one chain, it claims the
  matching reservation on the other chain for the taker,
- sends expiries and refunds after deadlines,
- serves the trading page's API, including two keeper services:
  POST /api/reserve  reserve a fill for a taker (the taker needs no gas on the maker's chain),
  POST /api/confirm  submit a maker's confirmation (claim the taker's lock with the secret).

The keeper never holds user funds: every call it sends pays a fixed recipient. If it stops,
users can send the same calls themselves.

  python -m swapsvc.keeper --config /etc/incentium-swap.json
"""
from __future__ import annotations

import argparse
import hashlib
import json
import logging
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from . import store
from .chains import Base, Incentium, normalize_ss58

log = logging.getLogger("swap")

MIN_FILL_USDC = 5_000_000
QTY_STEP = 10_000_000
ICT = 1_000_000_000
MAX_OPEN_RES = 3
CONFIRM_MARGIN = 30 * 60      # the maker's page and the keeper refuse to reveal later than this before T_lock
RETRY_AFTER = 300
HEX32 = re.compile(r"^0x[0-9a-f]{64}$")


def sha256_hex(secret_hex: str) -> str:
    return "0x" + hashlib.sha256(bytes.fromhex(secret_hex[2:])).hexdigest()


class Keeper:
    def __init__(self, cfg: dict):
        self.cfg = cfg
        self.con = store.connect(cfg["db"])
        self.inc = Incentium(cfg["incentium_rpc"], cfg.get("incentium_keeper_seed"))
        self.base = Base(cfg["base_rpc"], cfg["contract"], cfg.get("base_keeper_key"))
        self.chain_lock = threading.Lock()   # substrate-interface and the nonce are not thread-safe
        self.pending: dict[str, float] = {}
        self.rate: dict[str, list[float]] = {}

    # ---------------------------------------------------------------- loop

    def once(self):
        with self.chain_lock:
            self.inc.sync(self.con, self.cfg.get("incentium_start", 0))
            self.base.sync(self.con, self.cfg.get("base_start", 0))
            self.relay_secrets()
            self.expire_things()

    def run(self):
        while True:
            try:
                self.once()
            except Exception as e:
                log.warning("loop error: %r", e)
                self.inc.reset()
                time.sleep(10)
            time.sleep(self.cfg.get("interval", 6))

    def _once_per(self, key: str) -> bool:
        t = self.pending.get(key)
        if t and time.time() - t < RETRY_AFTER:
            return False
        self.pending[key] = time.time()
        return True

    def relay_secrets(self):
        # Secret revealed on Base (a sell maker confirmed) -> release the ICT reservation to the buyer.
        for l in self.con.execute("""SELECT l.order_id, l.fill, l.secret FROM locks l JOIN reservations r
                                     ON r.chain = 'inc' AND r.order_id = l.order_id AND r.fill = l.fill
                                     WHERE l.chain = 'base' AND l.state = 'claimed' AND r.state = 'active'""").fetchall():
            if self._once_per(f"inc-claim-{l['order_id']}-{l['fill']}"):
                self._try(lambda: self.inc.send("claim_reservation",
                                                {"order": l["order_id"], "fill": l["fill"], "secret": l["secret"]}),
                          "relay to Incentium")
        # Secret revealed on Incentium (a buy maker confirmed) -> pay the USDC reservation to the seller.
        for l in self.con.execute("""SELECT l.order_id, l.fill, l.secret FROM locks l JOIN reservations r
                                     ON r.chain = 'base' AND r.order_id = l.order_id AND r.fill = l.fill
                                     WHERE l.chain = 'inc' AND l.state = 'claimed' AND r.state = 'active'""").fetchall():
            if self._once_per(f"base-claim-{l['order_id']}-{l['fill']}"):
                self._try(lambda: self.base.send("claimReservation", bytes.fromhex(l["order_id"][2:]), l["fill"],
                                                 bytes.fromhex(l["secret"][2:])), "relay to Base")

    def expire_things(self):
        t_inc, t_base = self.inc.now(), self.base.now()
        for chain, now, client in (("inc", t_inc, self.inc), ("base", t_base, self.base)):
            for r in self.con.execute("SELECT order_id, fill FROM reservations WHERE chain = ? AND state = 'active' AND expiry <= ?",
                                      (chain, now)).fetchall():
                if self._once_per(f"{chain}-exp-{r['order_id']}-{r['fill']}"):
                    if chain == "inc":
                        self._try(lambda: client.send("expire_reservation", {"order": r["order_id"], "fill": r["fill"]}), "expire")
                    else:
                        self._try(lambda: client.send("expireReservation", bytes.fromhex(r["order_id"][2:]), r["fill"]), "expire")
            for l in self.con.execute("SELECT id FROM locks WHERE chain = ? AND state = 'active' AND expiry <= ?",
                                      (chain, now)).fetchall():
                if self._once_per(f"{chain}-refund-{l['id']}"):
                    if chain == "inc":
                        self._try(lambda: client.send("refund_lock", {"lock": l["id"]}), "refund lock")
                    else:
                        self._try(lambda: client.send("refundLock", bytes.fromhex(l["id"][2:])), "refund lock")
            for o in self.con.execute("SELECT id FROM orders WHERE chain = ? AND open = 1 AND expiry <= ?",
                                      (chain, now)).fetchall():
                if self._once_per(f"{chain}-close-{o['id']}"):
                    if chain == "inc":
                        self._try(lambda: client.send("refund_expired", {"order": o["id"]}), "refund order")
                    else:
                        self._try(lambda: client.send("refundExpired", bytes.fromhex(o["id"][2:])), "refund order")

    def _try(self, fn, what):
        try:
            tx = fn()
            log.info("%s: %s", what, tx)
            return tx
        except Exception as e:
            log.warning("%s failed: %r", what, e)
            return None

    # ---------------------------------------------------------------- services for the page

    def reserve(self, body: dict) -> dict:
        chain, oid = body.get("chain"), str(body.get("order", "")).lower()
        if chain not in ("inc", "base") or not HEX32.match(oid):
            raise ValueError("chain must be inc or base, order a 32-byte id")
        qty = int(body.get("qty", 0))
        if qty <= 0 or qty % QTY_STEP:
            raise ValueError("quantity must be a multiple of 0.01 ICT")
        with self.chain_lock:
            o = store.order_detail(self.con, chain, oid)
            if not o or not o["open"]:
                raise ValueError("order not open")
            now = self.inc.now() if chain == "inc" else self.base.now()
            if now >= o["expiry"] - 3600:
                raise ValueError("order expires within the hour")
            if qty > o["remaining"] or qty * o["price"] // ICT < MIN_FILL_USDC:
                raise ValueError("quantity too large for the order, or under 5 USDC")
            if sum(1 for f in o["fills"] if f["state"] == "active") >= MAX_OPEN_RES:
                raise ValueError("this order has 3 open reservations; try again later")
            fill = store.next_free_fill(self.con, chain, oid)
            if fill is None:
                raise ValueError("no fills left on this order")
            if chain == "inc":
                recipient = normalize_ss58(str(body.get("recipient", "")))
                if not recipient:
                    raise ValueError("recipient must be an Incentium address (5...)")
                tx = self.inc.send("reserve", {"order": oid, "fill": fill, "qty": qty, "recipient": recipient}, signed=True)
            else:
                from web3 import Web3
                rec = str(body.get("recipient", ""))
                if not Web3.is_address(rec):
                    raise ValueError("recipient must be a Base address (0x...)")
                tx = self.base.send("reserve", bytes.fromhex(oid[2:]), fill, qty, Web3.to_checksum_address(rec))
            self.once_sync_quiet()
        return {"fill": fill, "tx": tx}

    def confirm(self, body: dict) -> dict:
        """A maker's confirmation: claim the taker's lock on `chain` with the secret."""
        chain, lid, secret = body.get("chain"), str(body.get("lock", "")).lower(), str(body.get("secret", "")).lower()
        if chain not in ("inc", "base") or not HEX32.match(lid) or not HEX32.match(secret):
            raise ValueError("chain, lock and secret required")
        h = sha256_hex(secret)
        with self.chain_lock:
            # Check the live chain, not only the book: a reverted claim would still publish the secret.
            if chain == "base":
                l = self.base.lock(lid)
                ok = l["state"] == 1 and "0x" + bytes(l["hash"]).hex() == h and l["expiry"] > self.base.now() + 300
                order_chain, oid, fill = "inc", "0x" + bytes(l["incOrderId"]).hex(), l["fill"]
            else:
                l = self.inc.lock(lid)
                ok = bool(l) and l["state"] == "Active" and str(l["hash"]).lower() == h and l["expiry"] > self.inc.now() + 300
                order_chain, oid, fill = "base", str(l["base_order"]).lower() if l else "", l["fill"] if l else 0
            if not ok:
                raise ValueError("no active lock with this secret's hash, or too close to its deadline")
            r = self.inc.reservation(oid, fill) if order_chain == "inc" else self.base.reservation(oid, fill)
            active = (r and r.get("state") == "Active") if order_chain == "inc" else r["state"] == 1
            if not active:
                raise ValueError("the matching reservation is not active; confirming now would not pay the taker")
            if chain == "base":
                tx = self.base.send("claimLock", bytes.fromhex(lid[2:]), bytes.fromhex(secret[2:]))
            else:
                tx = self.inc.send("claim_lock", {"lock": lid, "secret": secret})
            self.once_sync_quiet()
        return {"tx": tx}

    def once_sync_quiet(self):
        try:
            self.inc.sync(self.con, self.cfg.get("incentium_start", 0))
            self.base.sync(self.con, self.cfg.get("base_start", 0))
        except Exception as e:
            log.warning("sync after call: %r", e)

    def rate_ok(self, ip: str, limit: int = 20, window: int = 3600) -> bool:
        now = time.time()
        hits = [t for t in self.rate.get(ip, []) if now - t < window]
        if len(hits) >= limit:
            self.rate[ip] = hits
            return False
        self.rate[ip] = hits + [now]
        return True


def make_handler(k: Keeper, public_cfg: dict, web: str | None = None):
    import mimetypes
    import os

    class H(BaseHTTPRequestHandler):
        def static(self, path):
            """Serve the trading page (preview and testnet only; production uses Caddy)."""
            rel = path.lstrip("/") or "index.html"
            full = os.path.realpath(os.path.join(web, rel))
            if not full.startswith(os.path.realpath(web) + os.sep) or not os.path.isfile(full):
                return self.send(404, {"error": "not found"})
            data = open(full, "rb").read()
            self.send_response(200)
            self.send_header("Content-Type", mimetypes.guess_type(full)[0] or "application/octet-stream")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        server_version = "incentium-swap"

        def log_message(self, *a):
            pass

        def send(self, code, body):
            data = json.dumps(body, default=str).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def ro(self):
            return store.connect(k.cfg["db"], readonly=True)

        def do_GET(self):
            u = urlparse(self.path)
            p = [x for x in u.path.split("/") if x]
            q = parse_qs(u.query)
            if web and (not p or p[0] != "api"):
                return self.static(u.path)
            if p == ["api", "config"]:
                return self.send(200, public_cfg)
            c = self.ro()
            try:
                if p == ["api", "book"]:
                    return self.send(200, store.book(c, int(time.time())))
                if p == ["api", "status"]:
                    return self.send(200, {"incentium": store.get_meta(c, "inc_head"), "base": store.get_meta(c, "base_head")})
                if len(p) == 4 and p[:2] == ["api", "order"] and p[2] in ("inc", "base"):
                    o = store.order_detail(c, p[2], p[3].lower())
                    return self.send(200, o) if o else self.send(404, {"error": "no such order"})
                if p == ["api", "account"]:
                    addrs = [a if a.startswith("5") else a.lower() for a in q.get("addr", [])][:8]
                    norm = [normalize_ss58(a) or a for a in addrs]
                    return self.send(200, store.for_account(c, norm or [""]))
                return self.send(404, {"error": "not found"})
            finally:
                c.close()

        def do_POST(self):
            p = urlparse(self.path).path
            ip = self.headers.get("X-Forwarded-For", self.client_address[0]).split(",")[0].strip()
            try:
                body = json.loads(self.rfile.read(min(int(self.headers.get("Content-Length", 0)), 10_000)) or b"{}")
            except Exception:
                return self.send(400, {"error": "bad JSON"})
            if p not in ("/api/reserve", "/api/confirm"):
                return self.send(404, {"error": "not found"})
            if not k.rate_ok(ip):
                return self.send(429, {"error": "too many requests from this address; try again in an hour"})
            try:
                res = k.reserve(body) if p == "/api/reserve" else k.confirm(body)
                return self.send(200, res)
            except ValueError as e:
                return self.send(400, {"error": str(e)})
            except Exception as e:
                log.warning("%s failed: %r", p, e)
                return self.send(502, {"error": "the chain call failed; try again shortly"})
    return H


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("-v", action="store_true")
    ap.add_argument("--web", help="also serve the trading page from this folder (preview/testnet)")
    a = ap.parse_args()
    logging.basicConfig(level=logging.DEBUG if a.v else logging.INFO, format="%(asctime)s %(message)s")
    cfg = json.load(open(a.config))
    for key in ("incentium_keeper_seed_file", "base_keeper_key_file"):
        if cfg.get(key):
            cfg[key.removesuffix("_file")] = open(cfg[key]).read().strip()
    k = Keeper(cfg)
    public = {k2: cfg[k2] for k2 in ("incentium_rpc_public", "base_rpc_public", "base_chain_id", "contract", "usdc",
                                    "genesis_time", "explorer") if k2 in cfg}
    srv = ThreadingHTTPServer((cfg.get("host", "127.0.0.1"), cfg.get("port", 8095)), make_handler(k, public, a.web))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    log.info("swap keeper on %s:%s", cfg.get("host", "127.0.0.1"), cfg.get("port", 8095))
    k.run()


if __name__ == "__main__":
    main()
