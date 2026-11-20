"""Chain access for the swap service: Incentium (substrate-interface) and Base (web3.py).

Each client can (1) turn new finalized events into store calls and (2) send the keeper's
transactions. Addresses are normalised: Incentium accounts as SS58 (prefix 42), Base addresses
and all 32-byte ids as lowercase 0x-hex.
"""
from __future__ import annotations

import json
import logging
import os
import time

from . import store

log = logging.getLogger("swap")
SS58 = 42


def hx(v) -> str:
    if isinstance(v, (bytes, bytearray)):
        return "0x" + bytes(v).hex()
    s = str(v)
    return s.lower() if s.startswith("0x") else s


def ss58_from_bytes32(b) -> str:
    from substrateinterface.utils.ss58 import ss58_encode
    raw = bytes(b) if isinstance(b, (bytes, bytearray)) else bytes.fromhex(str(b)[2:])
    return ss58_encode(raw.hex(), SS58)


def bytes32_from_ss58(addr: str) -> bytes:
    from substrateinterface.utils.ss58 import ss58_decode
    return bytes.fromhex(ss58_decode(addr))


def normalize_ss58(addr: str) -> str | None:
    try:
        return ss58_from_bytes32(bytes32_from_ss58(addr))
    except Exception:
        return None


# ====================================================================== Incentium

class Incentium:
    chain = "inc"

    def __init__(self, url: str, keeper_seed: str | None = None):
        self.url, self.seed, self.s, self.keypair = url, keeper_seed, None, None

    def conn(self):
        if self.s is None:
            from substrateinterface import Keypair, SubstrateInterface
            self.s = SubstrateInterface(url=self.url, ss58_format=SS58)
            if self.seed and not self.keypair:
                self.keypair = Keypair.create_from_uri(self.seed, ss58_format=SS58)
        return self.s

    def reset(self):
        try:
            self.s and self.s.close()
        except Exception:
            pass
        self.s = None

    def now(self) -> int:
        return int(self.conn().query("Timestamp", "Now").value) // 1000

    def finalized(self) -> int:
        s = self.conn()
        return s.get_block_number(s.get_chain_finalised_head())

    # ---------------------------------------------------------------- reading

    def apply_events(self, con, n: int) -> int:
        s = self.conn()
        h = s.get_block_hash(n)
        count, ts = 0, None
        for e in s.get_events(block_hash=h):
            v = e.value
            if v["module_id"] != "Swap":
                continue
            a = v["attributes"]
            name = v["event_id"]
            if ts is None:
                ts = int(s.query("Timestamp", "Now", block_hash=h).value) // 1000
            if name == "SellPlaced":
                hashes = [hx(x) for x in s.query("Swap", "OrderHashes", [a["order"]], block_hash=h).value]
                store.order_placed(con, "inc", hx(a["order"]), a["maker"], hx(a["base_recipient"]), a["max_qty"],
                                   a["price"], a["expiry"], hashes, at=ts)
            elif name == "SellClosed":
                store.order_closed(con, "inc", hx(a["order"]))
            elif name == "Reserved":
                store.reserved(con, "inc", hx(a["order"]), a["fill"], a["recipient"], a["qty"], a["usdc"], a["expiry"])
            elif name == "ReservationClaimed":
                store.reservation_claimed(con, "inc", hx(a["order"]), a["fill"], hx(a["secret"]))
            elif name == "ReservationExpired":
                store.reservation_expired(con, "inc", hx(a["order"]), a["fill"], ts)
            elif name == "Locked":
                store.locked(con, "inc", hx(a["lock"]), hx(a["base_order"]), a["fill"], a["taker"], a["payee"],
                             a["qty"], a["qty"], hx(a["hash"]), a["expiry"])
            elif name == "LockClaimed":
                store.lock_claimed(con, "inc", hx(a["lock"]), hx(a["secret"]))
            elif name == "LockRefunded":
                store.lock_refunded(con, "inc", hx(a["lock"]))
            count += 1
        return count

    def sync(self, con, start: int, max_blocks: int = 500) -> int:
        nxt = store.get_meta(con, "inc_next", start)
        fin = self.finalized()
        end = min(fin, nxt + max_blocks - 1)
        for n in range(nxt, end + 1):
            self.apply_events(con, n)
            store.set_meta(con, "inc_next", n + 1)
        store.set_meta(con, "inc_head", {"finalized": fin, "at": int(time.time())})
        con.commit()
        return end - nxt + 1

    def reservation(self, oid: str, fill: int):
        v = self.conn().query("Swap", "Reservations", [oid, fill]).value
        return v

    def lock(self, lid: str):
        return self.conn().query("Swap", "Locks", [lid]).value

    def order(self, oid: str):
        return self.conn().query("Swap", "SellOrders", [oid]).value

    # ---------------------------------------------------------------- writing

    def _submit(self, call, signed: bool):
        s = self.conn()
        if signed:
            if not self.keypair:
                raise RuntimeError("no keeper key for Incentium")
            xt = s.create_signed_extrinsic(call=call, keypair=self.keypair)
        else:
            xt = s.create_unsigned_extrinsic(call=call)
        r = s.submit_extrinsic(xt, wait_for_inclusion=True)
        if not r.is_success:
            raise RuntimeError(f"Incentium call failed: {r.error_message}")
        return r.extrinsic_hash

    def send(self, fn: str, params: dict, signed: bool = False):
        call = self.conn().compose_call(call_module="Swap", call_function=fn, call_params=params)
        return self._submit(call, signed)


# ====================================================================== Base

ABI = json.load(open(os.path.join(os.path.dirname(__file__), "IncentiumSwap.abi.json")))


class Base:
    chain = "base"

    def __init__(self, url: str, contract: str, keeper_key: str | None = None, confirmations: int = 10):
        from web3 import Web3
        self.w3 = Web3(Web3.HTTPProvider(url, request_kwargs={"timeout": 30}))
        self.c = self.w3.eth.contract(address=Web3.to_checksum_address(contract), abi=ABI)
        self.acct = self.w3.eth.account.from_key(keeper_key) if keeper_key else None
        self.confirmations = confirmations
        self.events = {e["name"]: getattr(self.c.events, e["name"]) for e in ABI if e["type"] == "event"}
        self.topics = {self.w3.keccak(text=f"{e['name']}({','.join(i['type'] for i in e['inputs'])})").hex().removeprefix("0x"): e["name"]
                       for e in ABI if e["type"] == "event"}

    def safe_block(self) -> int:
        try:
            return self.w3.eth.get_block("safe")["number"]
        except Exception:
            return max(0, self.w3.eth.block_number - self.confirmations)

    def now(self) -> int:
        return int(self.w3.eth.get_block("latest")["timestamp"])

    def apply_log(self, con, lg):
        name = self.topics.get(lg["topics"][0].hex().removeprefix("0x"))
        if not name:
            return
        a = self.events[name]().process_log(lg)["args"]
        if name == "BuyPlaced":
            oid = hx(a["orderId"])
            hashes = [hx(x) for x in self.c.functions.hashes(a["orderId"]).call()]
            ts = self.w3.eth.get_block(lg["blockNumber"])["timestamp"]
            store.order_placed(con, "base", oid, a["maker"].lower(), ss58_from_bytes32(a["ictRecipient"]),
                               a["maxQty"], a["price"], a["expiry"], hashes, at=ts)
        elif name == "BuyClosed":
            store.order_closed(con, "base", hx(a["orderId"]))
        elif name == "Reserved":
            store.reserved(con, "base", hx(a["orderId"]), a["fill"], a["payee"].lower(), a["qty"], a["amount"], a["expiry"])
        elif name == "ReservationClaimed":
            store.reservation_claimed(con, "base", hx(a["orderId"]), a["fill"], hx(a["secret"]))
        elif name == "ReservationExpired":
            ts = self.w3.eth.get_block(lg["blockNumber"])["timestamp"]
            store.reservation_expired(con, "base", hx(a["orderId"]), a["fill"], ts)
        elif name == "Locked":
            store.locked(con, "base", hx(a["lockId"]), hx(a["incOrderId"]), a["fill"], a["taker"].lower(),
                         a["payee"].lower(), a["qty"], a["amount"], hx(a["hash"]), a["expiry"])
        elif name == "LockClaimed":
            store.lock_claimed(con, "base", hx(a["lockId"]), hx(a["secret"]))
        elif name == "LockRefunded":
            store.lock_refunded(con, "base", hx(a["lockId"]))

    def sync(self, con, start: int, step: int = 2000) -> int:
        nxt = store.get_meta(con, "base_next", start)
        safe = self.safe_block()
        if nxt > safe:
            return 0
        end = min(safe, nxt + step - 1)
        logs = self.w3.eth.get_logs({"address": self.c.address, "fromBlock": nxt, "toBlock": end})
        for lg in sorted(logs, key=lambda x: (x["blockNumber"], x["logIndex"])):
            self.apply_log(con, lg)
        store.set_meta(con, "base_next", end + 1)
        store.set_meta(con, "base_head", {"safe": safe, "at": int(time.time())})
        con.commit()
        return end - nxt + 1

    def lock(self, lid: str):
        r = self.c.functions.locks(bytes.fromhex(lid[2:])).call()
        keys = ["state", "expiry", "taker", "payee", "qty", "amount", "hash", "incOrderId", "fill"]
        return dict(zip(keys, r))

    def reservation(self, oid: str, fill: int):
        r = self.c.functions.reservations(bytes.fromhex(oid[2:]), fill).call()
        return dict(zip(["state", "expiry", "payee", "qty", "amount"], r))

    def order(self, oid: str):
        r = self.c.functions.buyOrders(bytes.fromhex(oid[2:])).call()
        keys = ["maker", "expiry", "open", "openReservations", "usedFills", "ictRecipient", "maxQty", "price",
                "remainingQty", "freeUsdc"]
        return dict(zip(keys, r))

    def send(self, fn: str, *args):
        if not self.acct:
            raise RuntimeError("no keeper key for Base")
        f = getattr(self.c.functions, fn)(*args)
        tx = f.build_transaction({"from": self.acct.address, "nonce": self.w3.eth.get_transaction_count(self.acct.address, "pending")})
        signed = self.acct.sign_transaction(tx)
        h = self.w3.eth.send_raw_transaction(signed.raw_transaction)
        r = self.w3.eth.wait_for_transaction_receipt(h, timeout=180)
        if r["status"] != 1:
            raise RuntimeError(f"Base call {fn} reverted: {h.hex()}")
        return "0x" + h.hex().removeprefix("0x")
