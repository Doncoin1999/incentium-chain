"""Order book state for both chains, rebuilt from chain events (SQLite).

Chains are 'inc' (Incentium, sell orders and ICT locks) and 'base' (Base, buy orders and USDC
locks). Amounts are integers in base units: ICT 9 decimals, USDC 6 decimals. Ids are 0x-hex.
"""
from __future__ import annotations

import json
import sqlite3

SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS orders (
  chain TEXT NOT NULL, id TEXT NOT NULL, side TEXT NOT NULL, maker TEXT NOT NULL,
  recipient TEXT NOT NULL,           -- where the maker is paid, on the other chain
  max_qty INTEGER NOT NULL, price INTEGER NOT NULL, remaining INTEGER NOT NULL,
  expiry INTEGER NOT NULL, open INTEGER NOT NULL, placed_at INTEGER, hashes TEXT NOT NULL,
  PRIMARY KEY (chain, id));
CREATE TABLE IF NOT EXISTS reservations (
  chain TEXT NOT NULL, order_id TEXT NOT NULL, fill INTEGER NOT NULL,
  recipient TEXT NOT NULL,           -- the taker, paid on this chain
  qty INTEGER NOT NULL, usdc INTEGER NOT NULL, expiry INTEGER NOT NULL,
  state TEXT NOT NULL, secret TEXT, PRIMARY KEY (chain, order_id, fill));
CREATE TABLE IF NOT EXISTS locks (
  chain TEXT NOT NULL, id TEXT NOT NULL,
  order_id TEXT NOT NULL, fill INTEGER NOT NULL,   -- the order on the other chain
  taker TEXT NOT NULL, payee TEXT NOT NULL, qty INTEGER NOT NULL, amount INTEGER NOT NULL,
  hash TEXT NOT NULL, expiry INTEGER NOT NULL, state TEXT NOT NULL, secret TEXT,
  PRIMARY KEY (chain, id));
CREATE INDEX IF NOT EXISTS locks_by_order ON locks (order_id, fill);
CREATE TABLE IF NOT EXISTS deposits (       -- reservation deposits (review fix H2)
  chain TEXT NOT NULL, order_id TEXT NOT NULL, fill INTEGER NOT NULL,
  depositor TEXT NOT NULL, amount INTEGER NOT NULL,
  state TEXT NOT NULL,               -- held, returned, half_burned
  dispute_until INTEGER,             -- unused since swap v2 (no arbiter)
  PRIMARY KEY (chain, order_id, fill));
CREATE INDEX IF NOT EXISTS orders_by_maker ON orders (maker);
"""

OTHER = {"inc": "base", "base": "inc"}


def connect(path: str, readonly: bool = False) -> sqlite3.Connection:
    if readonly:
        con = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=10, check_same_thread=False)
    else:
        con = sqlite3.connect(path, timeout=30, check_same_thread=False)
        con.execute("PRAGMA journal_mode=WAL")
        con.executescript(SCHEMA)
    con.row_factory = sqlite3.Row
    return con


def get_meta(con, key, default=None):
    r = con.execute("SELECT value FROM meta WHERE key = ?", (key,)).fetchone()
    return json.loads(r[0]) if r else default


def set_meta(con, key, value):
    con.execute("INSERT INTO meta VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, json.dumps(value)))


# ------------------------------------------------------------------ applying events
# Each chain client turns its events into these calls. Event names follow the contract and pallet.

def order_placed(con, chain, oid, maker, recipient, max_qty, price, expiry, hashes, at=None):
    side = "sell" if chain == "inc" else "buy"
    con.execute("""INSERT OR REPLACE INTO orders VALUES (?,?,?,?,?,?,?,?,?,1,?,?)""",
                (chain, oid, side, maker, recipient, int(max_qty), int(price), int(max_qty), int(expiry), at,
                 json.dumps(hashes)))


def order_closed(con, chain, oid):
    con.execute("UPDATE orders SET open = 0, remaining = 0 WHERE chain = ? AND id = ?", (chain, oid))


def reserved(con, chain, oid, fill, recipient, qty, usdc, expiry):
    con.execute("INSERT OR REPLACE INTO reservations VALUES (?,?,?,?,?,?,?, 'active', NULL)",
                (chain, oid, int(fill), recipient, int(qty), int(usdc), int(expiry)))
    con.execute("UPDATE orders SET remaining = remaining - ? WHERE chain = ? AND id = ?", (int(qty), chain, oid))


def reservation_claimed(con, chain, oid, fill, secret):
    con.execute("UPDATE reservations SET state = 'claimed', secret = ? WHERE chain = ? AND order_id = ? AND fill = ?",
                (secret, chain, oid, int(fill)))


def reservation_expired(con, chain, oid, fill, now):
    r = con.execute("SELECT qty FROM reservations WHERE chain = ? AND order_id = ? AND fill = ?",
                    (chain, oid, int(fill))).fetchone()
    con.execute("UPDATE reservations SET state = 'expired' WHERE chain = ? AND order_id = ? AND fill = ?",
                (chain, oid, int(fill)))
    o = con.execute("SELECT open, expiry FROM orders WHERE chain = ? AND id = ?", (chain, oid)).fetchone()
    if r and o and o["open"] and now < o["expiry"]:
        con.execute("UPDATE orders SET remaining = remaining + ? WHERE chain = ? AND id = ?", (r["qty"], chain, oid))


def locked(con, chain, lid, order_id, fill, taker, payee, qty, amount, h, expiry):
    con.execute("INSERT OR REPLACE INTO locks VALUES (?,?,?,?,?,?,?,?,?,?, 'active', NULL)",
                (chain, lid, order_id, int(fill), taker, payee, int(qty), int(amount), h, int(expiry)))


def lock_claimed(con, chain, lid, secret):
    con.execute("UPDATE locks SET state = 'claimed', secret = ? WHERE chain = ? AND id = ?", (secret, chain, lid))


def lock_refunded(con, chain, lid):
    con.execute("UPDATE locks SET state = 'refunded' WHERE chain = ? AND id = ?", (chain, lid))


# ------------------------------------------------------------------ queries

def row(r):
    return dict(r) if r else None


def book(con, now):
    """Open orders with something left, best price first: sells low to high, buys high to low."""
    q = """SELECT o.*, (SELECT COUNT(*) FROM reservations r WHERE r.chain = o.chain AND r.order_id = o.id
                        AND r.state = 'active') AS open_reservations,
                       (SELECT COUNT(*) FROM reservations r WHERE r.chain = o.chain AND r.order_id = o.id) AS used_fills
           FROM orders o WHERE o.open = 1 AND o.expiry > ? AND o.remaining > 0 AND o.side = ?"""
    sells = [dict(r) for r in con.execute(q + " ORDER BY price ASC, placed_at ASC", (now, "sell"))]
    buys = [dict(r) for r in con.execute(q + " ORDER BY price DESC, placed_at ASC", (now, "buy"))]
    return {"sells": sells, "buys": buys}


def order_detail(con, chain, oid):
    o = row(con.execute("SELECT * FROM orders WHERE chain = ? AND id = ?", (chain, oid)).fetchone())
    if not o:
        return None
    o["hashes"] = json.loads(o["hashes"])
    fills = []
    for r in con.execute("SELECT * FROM reservations WHERE chain = ? AND order_id = ? ORDER BY fill", (chain, oid)):
        f = dict(r)
        f["locks"] = [dict(x) for x in con.execute(
            "SELECT * FROM locks WHERE chain = ? AND order_id = ? AND fill = ?", (OTHER[chain], oid, f["fill"]))]
        fills.append(f)
    o["fills"] = fills
    return o


def next_free_fill(con, chain, oid, used_mask: int = 0):
    """First fill not used in the store nor in `used_mask` (bit i set = fill i used on chain)."""
    used = {r[0] for r in con.execute("SELECT fill FROM reservations WHERE chain = ? AND order_id = ?", (chain, oid))}
    return next((i for i in range(16) if i not in used and not (used_mask >> i) & 1), None)


def for_account(con, addrs):
    marks = ",".join("?" * len(addrs))
    orders = [dict(r) for r in con.execute(
        f"SELECT * FROM orders WHERE maker IN ({marks}) ORDER BY placed_at DESC LIMIT 200", addrs)]
    takes = [dict(r) for r in con.execute(
        f"SELECT * FROM locks WHERE taker IN ({marks}) ORDER BY expiry DESC LIMIT 200", addrs)]
    received = [dict(r) for r in con.execute(
        f"SELECT * FROM reservations WHERE recipient IN ({marks}) ORDER BY expiry DESC LIMIT 200", addrs)]
    return {"orders": orders, "locks": takes, "reservations": received}


def matching_lock(con, chain, oid, fill):
    """The taker lock on the other chain that pays this reservation's maker, if any."""
    return [dict(r) for r in con.execute(
        "SELECT * FROM locks WHERE chain = ? AND order_id = ? AND fill = ? AND state = 'active'",
        (OTHER[chain], oid, int(fill)))]


# ------------------------------------------------------------------ reservation deposits (review fix H2)



def deposit_held(con, chain, oid, fill, depositor, amount):
    con.execute("INSERT OR REPLACE INTO deposits VALUES (?,?,?,?,?, 'held', NULL)",
                (chain, oid, int(fill), depositor, int(amount)))


def deposit_settled(con, chain, oid, fill, state):
    con.execute("UPDATE deposits SET state = ? WHERE chain = ? AND order_id = ? AND fill = ?",
                (state, chain, oid, int(fill)))


def keeper_fronted_open(con, payer):
    """Open reservations whose deposit the keeper paid for this taker (`payer` noted at reserve time)."""
    return con.execute("""SELECT COUNT(*) FROM meta m JOIN reservations r
        ON m.key = 'front:' || r.chain || ':' || r.order_id || ':' || r.fill
        WHERE r.state = 'active' AND json_extract(m.value, '$') = ?""", (payer,)).fetchone()[0]
