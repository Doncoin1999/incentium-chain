"""End to end: the real IncentiumSwap contract on a local Base chain (anvil) and a fake Incentium
chain that follows the ict-swap pallet's rules, with the real keeper between them.

  cd swap/service && python -m pytest -q tests   (needs anvil and `forge build` in swap/base)
"""
import hashlib
import json
import os
import shutil
import socket
import subprocess
import time

import pytest

from swapsvc import store
from swapsvc.keeper import Keeper

HERE = os.path.dirname(__file__)
OUT = os.path.join(HERE, "..", "..", "base", "out")
ICT = 10**9
PRICE = 250_000
# anvil's default funded accounts
KEYS = ['0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80', '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d', '0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a', '0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6']
SELLER_INC = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
BUYER_INC = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"


def sha(b: bytes) -> str:
    return "0x" + hashlib.sha256(b).hexdigest()


def secret(i: int) -> str:
    return "0x" + hashlib.sha256(b"seed" + bytes([i])).hexdigest()


# ---------------------------------------------------------------- a fake Incentium chain

class FakeIncentium:
    """Implements the pallet's rules that the keeper relies on, and emits its events."""
    chain = "inc"

    def __init__(self):
        self.t = int(time.time())
        self.orders, self.res, self.locks, self.events, self.balances = {}, {}, {}, [], {}

    def now(self):
        return self.t

    def reset(self):
        pass

    def emit(self, name, **a):
        self.events.append((name, a))

    def place_sell(self, maker, oid, max_qty, price, base_recipient, hashes):
        self.orders[oid] = dict(maker=maker, base_recipient=base_recipient, max_qty=max_qty, price=price,
                                remaining=max_qty, expiry=self.t + 86_400, open=True, hashes=hashes)
        self.emit("SellPlaced", order=oid, maker=maker, base_recipient=base_recipient, max_qty=max_qty, price=price,
                  expiry=self.t + 86_400, hashes=hashes)

    def send(self, fn, p, signed=False):
        if fn == "reserve":
            o = self.orders[p["order"]]
            assert o["open"] and p["qty"] <= o["remaining"] and (p["order"], p["fill"]) not in self.res
            o["remaining"] -= p["qty"]
            r = dict(state="Active", recipient=p["recipient"], qty=p["qty"], usdc=p["qty"] * o["price"] // ICT,
                     expiry=self.t + 19 * 3600)
            self.res[(p["order"], p["fill"])] = r
            self.emit("Reserved", order=p["order"], fill=p["fill"], recipient=p["recipient"], qty=p["qty"],
                      usdc=r["usdc"], expiry=r["expiry"])
        elif fn == "claim_reservation":
            r = self.res[(p["order"], p["fill"])]
            assert r["state"] == "Active" and self.t < r["expiry"]
            assert sha(bytes.fromhex(p["secret"][2:])) == self.orders[p["order"]]["hashes"][p["fill"]]
            r["state"] = "Claimed"
            self.balances[r["recipient"]] = self.balances.get(r["recipient"], 0) + r["qty"]
            self.emit("ReservationClaimed", order=p["order"], fill=p["fill"], secret=p["secret"])
        elif fn == "lock":
            pass
        elif fn == "claim_lock":
            l = self.locks[p["lock"]]
            assert l["state"] == "Active" and sha(bytes.fromhex(p["secret"][2:])) == l["hash"] and self.t < l["expiry"]
            l["state"] = "Claimed"
            self.balances[l["payee"]] = self.balances.get(l["payee"], 0) + l["qty"]
            self.emit("LockClaimed", lock=p["lock"], secret=p["secret"])
        elif fn == "refund_lock":
            l = self.locks[p["lock"]]
            assert l["state"] == "Active" and self.t >= l["expiry"]
            l["state"] = "Expired"
            self.emit("LockRefunded", lock=p["lock"])
        elif fn in ("expire_reservation", "refund_expired"):
            pass
        else:
            raise AssertionError(fn)
        return "0xfake"

    def taker_lock(self, lid, base_order, fill, taker, payee, h, qty, ttl):
        self.locks[lid] = dict(state="Active", taker=taker, payee=payee, qty=qty, hash=h, base_order=base_order,
                               fill=fill, expiry=self.t + ttl)
        self.emit("Locked", lock=lid, base_order=base_order, fill=fill, taker=taker, payee=payee, hash=h, qty=qty,
                  expiry=self.t + ttl)

    def lock(self, lid):
        return self.locks.get(lid)

    def reservation(self, oid, fill):
        return self.res.get((oid, fill))

    def sync(self, con, start, max_blocks=500):
        evs, self.events = self.events, []
        for name, a in evs:
            if name == "SellPlaced":
                store.order_placed(con, "inc", a["order"], a["maker"], a["base_recipient"], a["max_qty"], a["price"],
                                   a["expiry"], a["hashes"], at=self.t)
            elif name == "Reserved":
                store.reserved(con, "inc", a["order"], a["fill"], a["recipient"], a["qty"], a["usdc"], a["expiry"])
            elif name == "ReservationClaimed":
                store.reservation_claimed(con, "inc", a["order"], a["fill"], a["secret"])
            elif name == "Locked":
                store.locked(con, "inc", a["lock"], a["base_order"], a["fill"], a["taker"], a["payee"], a["qty"],
                             a["qty"], a["hash"], a["expiry"])
            elif name == "LockClaimed":
                store.lock_claimed(con, "inc", a["lock"], a["secret"])
            elif name == "LockRefunded":
                store.lock_refunded(con, "inc", a["lock"])
        con.commit()
        return len(evs)


# ---------------------------------------------------------------- local Base chain

def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


@pytest.fixture
def env(tmp_path):
    anvil = shutil.which("anvil") or os.path.expanduser("~/.foundry/bin/anvil")
    if not os.path.exists(anvil) or not os.path.exists(OUT):
        pytest.skip("needs anvil and forge build output")
    port = free_port()
    proc = subprocess.Popen([anvil, "--port", str(port), "--silent"], stdout=subprocess.DEVNULL)
    from web3 import Web3
    w3 = Web3(Web3.HTTPProvider(f"http://127.0.0.1:{port}"))
    for _ in range(50):
        if w3.is_connected():
            break
        time.sleep(0.1)

    def deploy(path, name, *args, key=KEYS[0]):
        j = json.load(open(os.path.join(OUT, path, name + ".json")))
        acct = w3.eth.account.from_key(key)
        c = w3.eth.contract(abi=j["abi"], bytecode=j["bytecode"]["object"])
        tx = c.constructor(*args).build_transaction({"from": acct.address, "nonce": w3.eth.get_transaction_count(acct.address)})
        r = w3.eth.wait_for_transaction_receipt(w3.eth.send_raw_transaction(acct.sign_transaction(tx).raw_transaction))
        return w3.eth.contract(address=r["contractAddress"], abi=j["abi"])

    accts = [w3.eth.account.from_key(k) for k in KEYS]
    keeper_a, seller, buyer, fees = accts
    genesis = w3.eth.get_block("latest")["timestamp"] - 2 * 86_400
    usdc = deploy("IncentiumSwap.t.sol", "MockUSDC")
    swap = deploy("IncentiumSwap.sol", "IncentiumSwap", usdc.address, genesis, fees.address, keeper_a.address)

    def tx(fn, key):
        a = w3.eth.account.from_key(key)
        t = fn.build_transaction({"from": a.address, "nonce": w3.eth.get_transaction_count(a.address)})
        r = w3.eth.wait_for_transaction_receipt(w3.eth.send_raw_transaction(a.sign_transaction(t).raw_transaction))
        assert r["status"] == 1
        return r

    tx(usdc.functions.mint(buyer.address, 1_000 * 10**6), KEYS[0])
    tx(usdc.functions.approve(swap.address, 2**255), KEYS[2])
    cfg = {"db": str(tmp_path / "swap.db"), "incentium_rpc": "fake", "base_rpc": f"http://127.0.0.1:{port}",
           "contract": swap.address, "base_keeper_key": KEYS[0], "incentium_keeper_seed": None}
    k = Keeper(cfg)
    k.inc = FakeIncentium()
    k.inc.t = w3.eth.get_block("latest")["timestamp"]
    k.base.confirmations = 0
    k.base.safe_block = lambda: w3.eth.block_number
    yield dict(w3=w3, swap=swap, usdc=usdc, k=k, tx=tx, seller=seller, buyer=buyer, fees=fees)
    proc.terminate()


def mine(w3, secs):
    w3.provider.make_request("evm_increaseTime", [secs])
    w3.provider.make_request("evm_mine", [])


def test_sell_order_on_incentium_filled_by_buyer_on_base(env):
    w3, swap, usdc, k, tx = env["w3"], env["swap"], env["usdc"], env["k"], env["tx"]
    seller, buyer = env["seller"], env["buyer"]
    oid = "0x" + "11" * 32
    hashes = [sha(bytes.fromhex(secret(i)[2:])) for i in range(16)]
    k.inc.place_sell(SELLER_INC, oid, 100 * ICT, PRICE, seller.address.lower(), hashes)
    k.once()
    assert store.book(k.con, k.inc.t)["sells"][0]["id"] == oid

    # 1. the buyer's page asks the keeper to reserve fill 0 for the buyer's Incentium account
    res = k.reserve({"chain": "inc", "order": oid, "qty": 40 * ICT, "recipient": BUYER_INC})
    assert res["fill"] == 0
    # 2. the buyer locks 10 USDC on Base under hash 0, payable to the seller
    expiry = w3.eth.get_block("latest")["timestamp"] + 12 * 3600
    r = tx(swap.functions.lock(bytes.fromhex(oid[2:]), 0, bytes.fromhex(hashes[0][2:]), seller.address, 40 * ICT, PRICE, expiry),
           KEYS[2])
    lid = "0x" + swap.events.Locked().process_receipt(r)[0]["args"]["lockId"].hex().removeprefix("0x")
    k.once()
    detail = store.order_detail(k.con, "inc", oid)
    assert detail["fills"][0]["locks"][0]["id"] == lid

    # a wrong secret is refused before anything is published
    with pytest.raises(ValueError):
        k.confirm({"chain": "base", "lock": lid, "secret": secret(1)})
    # 3. the seller confirms: the keeper claims the lock, the seller is paid minus 0.1 %
    k.confirm({"chain": "base", "lock": lid, "secret": secret(0)})
    assert usdc.functions.balanceOf(seller.address).call() == 10 * 10**6 - 10_000
    assert usdc.functions.balanceOf(env["fees"].address).call() == 10_000
    # 4. the keeper relays the revealed secret: the buyer receives the ICT on Incentium
    k.once()
    assert k.inc.balances[BUYER_INC] == 40 * ICT
    assert k.inc.res[(oid, 0)]["state"] == "Claimed"


def test_buy_order_on_base_filled_by_seller_on_incentium(env):
    w3, swap, usdc, k, tx = env["w3"], env["swap"], env["usdc"], env["k"], env["tx"]
    seller, buyer = env["seller"], env["buyer"]
    from swapsvc.chains import bytes32_from_ss58
    hashes = [bytes.fromhex(sha(bytes.fromhex(secret(i + 20)[2:]))[2:]) for i in range(16)]
    expiry = w3.eth.get_block("latest")["timestamp"] + 86_400
    r = tx(swap.functions.placeBuy(7, 100 * ICT, PRICE, bytes32_from_ss58(BUYER_INC), expiry, hashes), KEYS[2])
    oid = "0x" + swap.events.BuyPlaced().process_receipt(r)[0]["args"]["orderId"].hex().removeprefix("0x")
    k.once()
    o = store.book(k.con, k.inc.t)["buys"][0]
    assert o["id"] == oid and o["recipient"] == BUYER_INC

    # 1. the seller's page has the keeper reserve fill 0 on Base for the seller's Base address
    res = k.reserve({"chain": "base", "order": oid, "qty": 60 * ICT, "recipient": seller.address})
    assert res["fill"] == 0
    k.once()
    # 2. the seller locks 60 ICT on Incentium under hash 0, payable to the buyer's account
    lid = "0x" + "22" * 32
    k.inc.taker_lock(lid, oid, 0, SELLER_INC, BUYER_INC, "0x" + hashes[0].hex(), 60 * ICT, 12 * 3600)
    k.once()
    # 3. the buyer confirms on Incentium (the keeper sends the unsigned claim)
    k.confirm({"chain": "inc", "lock": lid, "secret": secret(20)})
    assert k.inc.balances[BUYER_INC] == 60 * ICT
    # 4. the keeper relays the secret to Base: the seller receives 15 USDC minus the fee
    k.once()
    assert usdc.functions.balanceOf(seller.address).call() == 15 * 10**6 - 15_000


def test_keeper_refunds_after_deadlines(env):
    w3, swap, usdc, k, tx = env["w3"], env["swap"], env["usdc"], env["k"], env["tx"]
    seller, buyer = env["seller"], env["buyer"]
    before = usdc.functions.balanceOf(buyer.address).call()
    expiry = w3.eth.get_block("latest")["timestamp"] + 3600
    h = bytes.fromhex(sha(b"x")[2:])
    tx(swap.functions.lock(b"\x33" * 32, 1, h, seller.address, 40 * ICT, PRICE, expiry), KEYS[2])
    k.once()
    assert usdc.functions.balanceOf(buyer.address).call() == before - 10 * 10**6
    mine(w3, 3601)
    k.once()                          # sees the deadline, sends refundLock
    k.once()
    assert usdc.functions.balanceOf(buyer.address).call() == before
    assert k.con.execute("SELECT state FROM locks").fetchone()[0] == "refunded"


def test_reserve_rules(env):
    k = env["k"]
    oid = "0x" + "44" * 32
    hashes = [sha(bytes([i])) for i in range(16)]
    k.inc.place_sell(SELLER_INC, oid, 100 * ICT, PRICE, env["seller"].address.lower(), hashes)
    k.once()
    with pytest.raises(ValueError):
        k.reserve({"chain": "inc", "order": oid, "qty": 10 * ICT, "recipient": BUYER_INC})   # 2.5 USDC < 5
    with pytest.raises(ValueError):
        k.reserve({"chain": "inc", "order": oid, "qty": 20 * ICT, "recipient": "0x1234"})
    for f in range(3):
        assert k.reserve({"chain": "inc", "order": oid, "qty": 20 * ICT, "recipient": BUYER_INC})["fill"] == f
    with pytest.raises(ValueError, match="3 open reservations"):
        k.reserve({"chain": "inc", "order": oid, "qty": 20 * ICT, "recipient": BUYER_INC})
