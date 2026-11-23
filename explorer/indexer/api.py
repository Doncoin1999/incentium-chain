"""Read-only JSON API over the indexer's database, for the explorer page and anyone else.

Amounts are strings in base units (1 ICT = 10^9), so no precision is lost in JavaScript.
Runs behind Caddy on 127.0.0.1:8090:

  python -m indexer.api --db ~/incentium-explorer/explorer.db --port 8090
"""
from __future__ import annotations

import argparse
import csv
import io
import json
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from . import common, db

DB_PATH = ""
WEB_DIR = ""  # optional: also serve the page (for a preview over an SSH tunnel)
STATIC = {"/": ("index.html", "text/html"), "/index.html": ("index.html", "text/html"),
          "/app.js": ("app.js", "text/javascript"), "/style.css": ("style.css", "text/css")}
KINDS = {"transfer", "reward", "stake", "vault", "swap", "slash", "other", "protocol"}
AMOUNT_FIELDS = ("free", "reserved", "stake", "unstaking", "bond", "unbonding", "vault_locked",
                 "vault_claimed", "total", "amount", "change_24h", "issued", "vault", "fund", "total_stake")


def s(v):
    return None if v is None else str(v)


def money(d: dict) -> dict:
    return {k: (s(v) if k in AMOUNT_FIELDS else v) for k, v in d.items()}


def con():
    return db.connect(DB_PATH, readonly=True)


def block_time(c, n: int) -> int | None:
    g = db.get_meta(c, "genesis_ts")
    return None if g is None else g + (n - 1) * common.BLOCK_SECONDS


def stats(c) -> dict:
    heads = db.get_meta(c, "heads", {})
    snap = db.get_meta(c, "snapshot", {})
    fin = heads.get("finalized", 0)
    until = snap.get("bootstrap_until", 0)
    out = dict(snap)
    out.update({"best": heads.get("best"), "finalized": fin, "heads_at": heads.get("at"),
                "indexed_to": db.get_meta(c, "next_block", 0) - 1,
                "launch": {"until_block": until, "in_launch": bool(until) and fin < until,
                           "day": fin // common.DAY, "days": until // common.DAY if until else None,
                           "ends_at": block_time(c, until) if until else None,
                           "first_release_at": block_time(c, until + common.LAUNCH_CLIFF) if until else None,
                           "full_release_at": block_time(c, until + common.LAUNCH_CLIFF + 9 * common.LAUNCH_STEP) if until else None}})
    return money(out)


def blocks(c, limit: int) -> list:
    rows = c.execute("SELECT * FROM blocks ORDER BY n DESC LIMIT ?", (limit,)).fetchall()
    return [dict(r) for r in rows]


def activity(c, limit: int) -> list:
    rows = c.execute("""SELECT e.*, b.ts, aa.label AS account_label, ca.label AS counterparty_label
                        FROM account_events e LEFT JOIN blocks b ON b.n = e.block
                        LEFT JOIN accounts aa ON aa.addr = e.account LEFT JOIN accounts ca ON ca.addr = e.counterparty
                        WHERE e.kind != 'protocol' AND e.title != 'Weights revealed'
                        ORDER BY e.block DESC, e.idx DESC LIMIT ?""", (limit,)).fetchall()
    return [money(dict(r)) for r in rows]


def change_24h(c) -> dict:
    snap = db.get_meta(c, "snapshot", {}).get("block", 0)
    row = c.execute("SELECT MIN(block) FROM account_history WHERE block >= ?", (snap - common.DAY,)).fetchone()
    then = row[0] if row and row[0] is not None else None
    # Only once the history really reaches back about a day (a fresh indexer has minutes of it).
    if then is None or then > snap - common.DAY + 2 * 50:
        return {}
    return dict(c.execute("SELECT addr, total FROM account_history WHERE block = ?", (then,)).fetchall())


def accounts(c, which: str, offset: int, limit: int) -> dict:
    where = {"holders": "protocol = 0", "protocol": "protocol = 1"}.get(which, "1=1")
    total_rows = c.execute(f"SELECT COUNT(*) FROM accounts WHERE {where} AND total > 0").fetchone()[0]
    rows = c.execute(f"SELECT * FROM accounts WHERE {where} AND total > 0 ORDER BY total DESC LIMIT ? OFFSET ?",
                     (limit, offset)).fetchall()
    issued = int(db.get_meta(c, "snapshot", {}).get("issued", 0)) or 1
    before = change_24h(c)
    out = []
    for i, r in enumerate(rows):
        d = dict(r)
        d["rank"] = offset + i + 1
        d["share_ppm"] = d["total"] * 1_000_000 // issued
        d["change_24h"] = d["total"] - before[d["addr"]] if d["addr"] in before else None
        out.append(money(d))
    return {"count": total_rows, "offset": offset, "limit": limit, "rows": out,
            "snapshot_block": db.get_meta(c, "snapshot", {}).get("block")}


def account(c, addr: str) -> dict | None:
    r = c.execute("SELECT * FROM accounts WHERE addr = ?", (addr,)).fetchone()
    has_events = c.execute("SELECT 1 FROM account_events WHERE account = ? LIMIT 1", (addr,)).fetchone()
    if not r and not has_events:
        return {"addr": addr, "known": False}
    d = dict(r) if r else {"addr": addr, "label": None, "protocol": 0, "role": None, "total": 0}
    d["known"] = True
    if r:
        d["rank"] = c.execute("SELECT COUNT(*) + 1 FROM accounts WHERE protocol = ? AND total > ?",
                              (d["protocol"], d["total"])).fetchone()[0]
        d["holders"] = c.execute("SELECT COUNT(*) FROM accounts WHERE protocol = 0 AND total > 0").fetchone()[0]
        issued = int(db.get_meta(c, "snapshot", {}).get("issued", 0)) or 1
        d["share_ppm"] = d["total"] * 1_000_000 // issued
        fin = db.get_meta(c, "heads", {}).get("finalized", 0)
        until = db.get_meta(c, "snapshot", {}).get("bootstrap_until", 0)
        v = common.vault_schedule(d.get("vault_locked") or 0, d.get("vault_claimed") or 0, until, fin)
        for rel in v["releases"]:
            rel["at"] = block_time(c, rel["block"])
            rel["unlocked"] = s(rel["unlocked"])
        d["vault_schedule"] = {k: (s(x) if k != "releases" else x) for k, x in v.items()}
        d["vault_schedule"]["growing"] = bool(until) and fin < until
        before = change_24h(c)
        d["change_24h"] = d["total"] - before[addr] if addr in before else None
    d["tx_count"] = c.execute("SELECT COUNT(*) FROM account_events WHERE account = ?", (addr,)).fetchone()[0]
    return money(d)


def account_events(c, addr: str, kind: str | None, offset: int, limit: int) -> dict:
    cond, args = "e.account = ?", [addr]
    if kind in KINDS:
        cond += " AND e.kind = ?"
        args.append(kind)
    n = c.execute(f"SELECT COUNT(*) FROM account_events e WHERE {cond}", args).fetchone()[0]
    rows = c.execute(f"""SELECT e.*, b.ts, ca.label AS counterparty_label FROM account_events e
                         LEFT JOIN blocks b ON b.n = e.block LEFT JOIN accounts ca ON ca.addr = e.counterparty
                         WHERE {cond} ORDER BY e.block DESC, e.idx DESC LIMIT ? OFFSET ?""",
                     args + [limit, offset]).fetchall()
    return {"count": n, "offset": offset, "rows": [money(dict(r)) for r in rows]}


def block(c, key: str) -> dict | None:
    if key.isdigit():
        r = c.execute("SELECT * FROM blocks WHERE n = ?", (int(key),)).fetchone()
    else:
        r = c.execute("SELECT * FROM blocks WHERE hash = ?", (key.lower(),)).fetchone()
    if not r:
        return None
    d = dict(r)
    d["events"] = [dict(e, data=json.loads(e["data"])) for e in
                   c.execute("SELECT * FROM events WHERE block = ? ORDER BY idx", (d["n"],)).fetchall()]
    return d


def search(c, q: str) -> dict:
    q = (q or "").strip()
    if re.fullmatch(r"#?\d+", q):
        return {"type": "block", "id": q.lstrip("#")}
    if re.fullmatch(r"0x[0-9a-fA-F]{64}", q):
        if c.execute("SELECT 1 FROM blocks WHERE hash = ?", (q.lower(),)).fetchone():
            return {"type": "block", "id": q.lower()}
    addr = common.normalize_address(q)
    if addr:
        return {"type": "account", "id": addr}
    return {"type": "none"}


def to_csv(rows: list[dict], fields: list[str]) -> str:
    buf = io.StringIO()
    w = csv.DictWriter(buf, fieldnames=fields, extrasaction="ignore")
    w.writeheader()
    for r in rows:
        r = dict(r)
        for k in fields:
            if k in AMOUNT_FIELDS and r.get(k) not in (None, ""):
                v = int(r[k])
                r[k] = f"{'-' if v < 0 else ''}{abs(v) // common.UNIT}.{abs(v) % common.UNIT:09d}"
        w.writerow(r)
    return buf.getvalue()


class Handler(BaseHTTPRequestHandler):
    server_version = "incentium-explorer"

    def log_message(self, *a):
        pass

    def send(self, code: int, body, ctype="application/json", filename=None):
        data = body.encode() if isinstance(body, str) else json.dumps(body, default=str).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype + "; charset=utf-8")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Cache-Control", "no-store" if code != 200 else "public, max-age=5")
        if filename:
            self.send_header("Content-Disposition", f'attachment; filename="{filename}"')
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        u = urlparse(self.path)
        p = [x for x in u.path.split("/") if x]
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        intq = lambda k, d, hi: max(0, min(hi, int(q.get(k, d)) if str(q.get(k, d)).isdigit() else d))
        csv_out = q.get("format") == "csv"
        if p[:1] != ["api"]:
            if WEB_DIR and u.path in STATIC:
                name, ctype = STATIC[u.path]
                with open(f"{WEB_DIR}/{name}", encoding="utf-8") as f:
                    return self.send(200, f.read(), ctype)
            return self.send(404, {"error": "not found"})
        p = p[1:]
        c = con()
        try:
            if p == ["stats"]:
                return self.send(200, stats(c))
            if p == ["blocks"]:
                return self.send(200, blocks(c, intq("limit", 10, 100)))
            if p == ["activity"]:
                return self.send(200, activity(c, intq("limit", 10, 100)))
            if p == ["accounts"]:
                res = accounts(c, q.get("filter", "holders"), intq("offset", 0, 10**6), intq("limit", 100, 500))
                if csv_out:
                    return self.send(200, to_csv(res["rows"], ["rank", "addr", "label", "total", "free", "stake",
                                     "bond", "vault_locked", "vault_claimed", "share_ppm"]), "text/csv", "incentium-accounts.csv")
                return self.send(200, res)
            if p == ["search"]:
                return self.send(200, search(c, q.get("q", "")))
            if len(p) >= 2 and p[0] == "account":
                addr = common.normalize_address(p[1])
                if not addr:
                    return self.send(400, {"error": "not a valid address"})
                if len(p) == 2:
                    return self.send(200, account(c, addr))
                if p[2] == "events":
                    lim = intq("limit", 25, 100_000 if csv_out else 200)
                    res = account_events(c, addr, q.get("kind"), intq("offset", 0, 10**9), lim)
                    if csv_out:
                        return self.send(200, to_csv(res["rows"], ["block", "ts", "kind", "title", "counterparty",
                                         "amount", "note"]), "text/csv", f"incentium-{addr[:8]}.csv")
                    return self.send(200, res)
            if len(p) == 2 and p[0] == "block":
                b = block(c, p[1])
                return self.send(200, b) if b else self.send(404, {"error": "block not indexed"})
            return self.send(404, {"error": "not found"})
        finally:
            c.close()


def main():
    global DB_PATH, WEB_DIR
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8090)
    ap.add_argument("--web", help="folder with index.html, app.js and style.css, to serve the page too")
    a = ap.parse_args()
    DB_PATH, WEB_DIR = a.db, a.web or ""
    ThreadingHTTPServer((a.host, a.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
