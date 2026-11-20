"""Incentium explorer indexer.

Follows the finalized chain block by block and stores blocks, events and a readable per-account
history in SQLite. Every SNAPSHOT_EVERY blocks it also takes a balance snapshot of every account
(free, reserved, validator stake, work bond, launch vault) for the top-accounts list.

Historical events need an archive node: point --url at the public archive RPC
(wss://rpc.incentium.org); a pruned node only serves the last few hundred blocks.

  python -m indexer.indexer --url wss://rpc.incentium.org --db ~/incentium-explorer/explorer.db
"""
from __future__ import annotations

import argparse
import json
import logging
import time

from . import common, db

log = logging.getLogger("indexer")

SNAPSHOT_EVERY = 50          # blocks, about 10 minutes
HISTORY_KEEP = 3 * common.DAY
BATCH = 200                  # blocks per commit while catching up


# ----------------------------------------------------------------------------- event classification

def _acct(v):
    return v if isinstance(v, str) else None


def _amt(v) -> int:
    try:
        return int(v)
    except (TypeError, ValueError):
        return 0


def classify(module: str, name: str, attrs, proto: dict[str, str], ctx: dict) -> list[tuple]:
    """Turn one chain event into rows (account, kind, title, counterparty, signed amount, note).

    `proto` maps protocol account -> label; `ctx` carries per-block facts (accounts that withdrew
    a work bond in this block). Amounts are base units; locked rewards carry note "locked".
    """
    a = attrs if isinstance(attrs, dict) else {}
    lbl = proto.get
    vault = ctx.get("vault")
    pot = ctx.get("pot")
    stake = ctx.get("stake")
    work = ctx.get("work")
    rows: list[tuple] = []

    if module == "Balances" and name == "Transfer":
        frm, to, amt = _acct(a.get("from")), _acct(a.get("to")), _amt(a.get("amount"))
        if not frm or not to:
            return rows
        if frm == pot and to == vault:
            return rows  # the owner's side comes from Consensus.LaunchRewardLocked
        if frm == pot and to == work:
            return rows  # pool credit, shown from WorkVerify.PoolCredited
        if frm == pot:
            rows.append((to, "reward", "Reward paid", frm, amt, None))
        elif frm == vault:
            rows.append((to, "vault", "Claimed from launch vault", frm, amt, None))
        elif to == stake:
            rows.append((frm, "stake", "Staked", to, -amt, "still yours, held as validator stake"))
        elif frm == stake:
            rows.append((to, "stake", "Unstaked", frm, amt, None))
        elif to == work:
            rows.append((frm, "stake", "Work bond posted", to, -amt, "still yours, held as bond"))
        elif frm == work:
            if to in ctx.get("work_withdrawn", ()):
                rows.append((to, "stake", "Work bond returned", frm, amt, None))
            else:
                rows.append((to, "reward", "Pool payout", frm, amt, None))
        else:
            rows.append((frm, "transfer", "Transfer out", to, -amt, None))
            rows.append((to, "transfer", "Transfer in", frm, amt, None))
        # The protocol account's own side, so its page has a history too.
        for me, other, sign in ((frm, to, -1), (to, frm, 1)):
            if me in proto and not any(r[0] == me for r in rows):
                rows.append((me, "protocol", "Transfer in" if sign > 0 else "Transfer out", other, sign * amt, lbl(me)))
        return rows

    if module == "Consensus" and name == "LaunchRewardLocked":
        who = _acct(a.get("who"))
        if who:
            rows.append((who, "reward", "Reward locked in launch vault", vault, _amt(a.get("amount")), "locked"))
    elif module == "WorkVerify" and name == "PoolCredited":
        pool = _acct(a.get("pool"))
        if pool:
            rows.append((pool, "reward", "Reward credited to pool", pot, _amt(a.get("amount")), "held by work-verify until distributed"))
    elif module == "WorkVerify" and name == "MinerPenalized":
        hk = _acct(a.get("hotkey"))
        if hk:
            rows.append((hk, "slash", "Failed spot check, slashed", None, -_amt(a.get("slashed")), f"epoch {a.get('epoch')}"))
    elif module == "WorkVerify" and name == "HotkeyBanned":
        hk = _acct(a.get("hotkey"))
        if hk:
            rows.append((hk, "slash", "Banned after three failed checks", None, None, None))
    elif module == "Registry" and name == "NeuronRegistered":
        hk = _acct(a.get("hotkey"))
        if hk:
            rows.append((hk, "other", f"Registered in SN{a.get('netuid')} as uid {a.get('uid')}", None, None, None))
    elif module == "Registry" and name == "TierDeclared":
        hk = _acct(a.get("hotkey"))
        if hk:
            rows.append((hk, "other", f"Declared tier {a.get('tier')}", None, None, None))
    elif module == "Consensus" and name == "UnstakeRequested":
        who = _acct(a.get("who"))
        if who:
            rows.append((who, "stake", "Unstake requested", None, None, f"available from epoch {a.get('available_at')}"))
    elif module == "Weights" and name == "WeightsRevealed":
        v = _acct(a.get("validator"))
        if v:
            rows.append((v, "other", "Weights revealed", None, None, f"epoch {a.get('epoch')}"))
    elif module == "Balances" and name == "Endowed":
        who = _acct(a.get("account"))
        if who:
            rows.append((who, "other", "Account created", None, None, None))
    return rows


# ----------------------------------------------------------------------------- chain access

class Chain:
    def __init__(self, url: str):
        self.url = url
        self.s = None
        self._authorities: list = []

    def conn(self):
        if self.s is None:
            from substrateinterface import SubstrateInterface
            self.s = SubstrateInterface(url=self.url, ss58_format=common.SS58_PREFIX)
        return self.s

    def reset(self):
        try:
            if self.s:
                self.s.close()
        except Exception:
            pass
        self.s = None

    def heads(self) -> tuple[int, int]:
        s = self.conn()
        best = s.get_block_number(None)
        fin = s.get_block_number(s.get_chain_finalised_head())
        return best, fin

    def author(self, block_hash: str, header: dict) -> str | None:
        """Aura author: authorities[slot % len] from the pre-runtime digest."""
        try:
            for log_item in header.get("digest", {}).get("logs", []):
                v = log_item.value if hasattr(log_item, "value") else log_item
                pre = v.get("PreRuntime") if isinstance(v, dict) else None
                if not pre or pre[0] not in ("0x61757261", "aura"):
                    continue
                slot = int.from_bytes(bytes.fromhex(pre[1][2:])[:8], "little")
                if not self._authorities:  # the four genesis Aura keys; fixed on this chain
                    self._authorities = list(self.conn().query("Aura", "Authorities", block_hash=block_hash).value or [])
                if self._authorities:
                    a = str(self._authorities[slot % len(self._authorities)])
                    return common.ss58_encode(bytes.fromhex(a[2:])) if a.startswith("0x") and len(a) == 66 else a
        except Exception as e:
            log.debug("author: %r", e)
        return None

    def block(self, n: int) -> dict:
        s = self.conn()
        h = s.get_block_hash(n)
        blk = s.get_block(block_hash=h)
        header = blk["header"]
        exts = blk.get("extrinsics", [])
        ts = None
        try:
            ts = int(s.query("Timestamp", "Now", block_hash=h).value) // 1000
        except Exception:
            pass
        events = []
        for i, e in enumerate(s.get_events(block_hash=h)):
            v = e.value
            events.append({"idx": i, "extrinsic": v.get("extrinsic_idx"), "module": v["module_id"],
                           "name": v["event_id"], "attributes": v.get("attributes")})
        return {"n": n, "hash": h, "ts": ts, "author": self.author(h, header),
                "n_extrinsics": len(exts), "events": events}

    def snapshot(self, block_hash: str) -> dict:
        s = self.conn()
        q = lambda m, st, params=None: s.query_map(m, st, params or [], block_hash=block_hash, page_size=500)
        out = {"accounts": {}, "stake": {}, "unstaking": {}, "bond": {}, "unbonding": {},
               "vault_locked": {}, "vault_claimed": {}, "neurons": {}, "tiers": {}}
        for k, v in q("System", "Account"):
            d = v.value["data"]
            out["accounts"][k.value] = (int(d["free"]), int(d["reserved"]))
        for key, st, m in (("stake", "Consensus", "Stake"), ("vault_locked", "Consensus", "LaunchLocked"),
                           ("vault_claimed", "Consensus", "LaunchClaimed"), ("bond", "WorkVerify", "Bonds")):
            for k, v in q(st, m):
                out[key][k.value] = int(v.value)
        for key, st, m in (("unstaking", "Consensus", "Unstaking"), ("unbonding", "WorkVerify", "Unbonding")):
            for k, v in q(st, m):
                out[key][k.value] = int(v.value[0])
        for k, v in q("Registry", "Neurons", [0]):
            out["neurons"][v.value] = int(k.value)
        for k, v in q("Registry", "NeuronTier", [0]):
            out["tiers"][k.value] = int(v.value)
        out["issued"] = int(s.query("Balances", "TotalIssuance", block_hash=block_hash).value)
        out["bootstrap_until"] = int(s.query("Registry", "BootstrapUntil", block_hash=block_hash).value or 0)
        op = s.query("Registry", "BootstrapOperator", block_hash=block_hash).value
        out["operator"] = op
        return out


# ----------------------------------------------------------------------------- writing

def context(proto_by_label: dict[str, str]) -> dict:
    return {"vault": proto_by_label["Launch vault"], "pot": proto_by_label["Subnet pot"],
            "stake": proto_by_label["Validator stake (held for stakers)"],
            "work": proto_by_label["Work bonds (held for miners and pools)"]}


def store_block(con, b: dict, proto: dict[str, str], base_ctx: dict) -> None:
    ctx = dict(base_ctx)
    ctx["work_withdrawn"] = {e["attributes"].get("who") for e in b["events"]
                             if e["module"] == "WorkVerify" and e["name"] == "Withdrawn" and isinstance(e["attributes"], dict)}
    con.execute("INSERT OR REPLACE INTO blocks VALUES (?,?,?,?,?,?)",
                (b["n"], b["hash"], b["ts"], b["author"], len(b["events"]), b["n_extrinsics"]))
    for e in b["events"]:
        if e["module"] == "System" and e["name"] in ("ExtrinsicSuccess",):
            continue
        con.execute("INSERT OR REPLACE INTO events VALUES (?,?,?,?,?,?)",
                    (b["n"], e["idx"], e["extrinsic"], e["module"], e["name"], json.dumps(e["attributes"], default=str)))
        for acc, kind, title, cp, amt, note in classify(e["module"], e["name"], e["attributes"], proto, ctx):
            con.execute("INSERT OR REPLACE INTO account_events VALUES (?,?,?,?,?,?,?,?)",
                        (b["n"], e["idx"], acc, kind, title, cp, amt, note))


def build_accounts(snap: dict, proto: dict[str, str], labels: dict[str, str]) -> list[dict]:
    """Combine a snapshot into one row per account. ICT held by protocol accounts on behalf of
    someone (stake, bonds, vault) is counted with that someone; the protocol rows show what they hold."""
    addrs = set(snap["accounts"]) | set(snap["stake"]) | set(snap["bond"]) | set(snap["vault_locked"]) | set(snap["neurons"])
    rows = []
    for a in addrs:
        free, reserved = snap["accounts"].get(a, (0, 0))
        st, unst = snap["stake"].get(a, 0), snap["unstaking"].get(a, 0)
        bd, unbd = snap["bond"].get(a, 0), snap["unbonding"].get(a, 0)
        vl, vc = snap["vault_locked"].get(a, 0), snap["vault_claimed"].get(a, 0)
        is_proto = a in proto
        total = free + reserved if is_proto else free + reserved + st + unst + bd + unbd + (vl - vc)
        role = None
        if a in snap["neurons"]:
            role = "validator" if st else "miner"
        label = labels.get(a) or proto.get(a)
        if not label and a == snap.get("operator"):
            label = "Launch operator"
        if not label and role:
            label = f"SN0 {role} uid {snap['neurons'][a]}"
        rows.append({"addr": a, "label": label, "protocol": int(is_proto), "role": role,
                     "free": free, "reserved": reserved, "stake": st, "unstaking": unst, "bond": bd,
                     "unbonding": unbd, "vault_locked": vl, "vault_claimed": vc, "total": total,
                     "tier": snap["tiers"].get(a) if role else None, "uid": snap["neurons"].get(a)})
    return rows


def store_snapshot(con, n: int, snap: dict, proto: dict[str, str], labels: dict[str, str]) -> None:
    rows = build_accounts(snap, proto, labels)
    first = dict(con.execute("SELECT account, MIN(block) FROM account_events GROUP BY account").fetchall())
    con.execute("DELETE FROM accounts")
    for r in rows:
        con.execute("""INSERT INTO accounts (addr,label,protocol,role,free,reserved,stake,unstaking,bond,unbonding,
                       vault_locked,vault_claimed,total,tier,uid,first_block) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
                    (r["addr"], r["label"], r["protocol"], r["role"], r["free"], r["reserved"], r["stake"], r["unstaking"],
                     r["bond"], r["unbonding"], r["vault_locked"], r["vault_claimed"], r["total"], r["tier"], r["uid"],
                     first.get(r["addr"])))
        con.execute("INSERT OR REPLACE INTO account_history VALUES (?,?,?)", (n, r["addr"], r["total"]))
    con.execute("DELETE FROM account_history WHERE block < ?", (n - HISTORY_KEEP,))
    holders = [r for r in rows if not r["protocol"]]
    by_label = {v: k for k, v in proto.items()}
    def bal(label):
        return next((r["free"] + r["reserved"] for r in rows if r["addr"] == by_label[label]), 0)
    stats = {
        "block": n, "issued": snap["issued"], "bootstrap_until": snap["bootstrap_until"],
        "vault": bal("Launch vault"), "fund": bal("Development fund"),
        "validators": sum(1 for r in rows if r["role"] == "validator"),
        "miners": sum(1 for r in rows if r["role"] == "miner"),
        "holders": sum(1 for r in holders if r["total"] > 0),
        "total_stake": sum(r["stake"] for r in rows),
    }
    db.set_meta(con, "snapshot", stats)


def load_labels(path: str | None) -> dict[str, str]:
    if not path:
        return {}
    try:
        raw = json.load(open(path))
    except FileNotFoundError:
        return {}
    out = {}
    for addr, label in raw.items():
        norm = common.normalize_address(addr)
        if norm:
            out[norm] = label
    return out


def ping(url: str | None, ok: bool, text: str) -> None:
    """healthchecks.io-style ping: <url> when healthy, <url>/fail otherwise."""
    if not url:
        return
    import urllib.request
    try:
        urllib.request.urlopen(urllib.request.Request(url if ok else url + "/fail", data=text.encode()), timeout=10)
    except Exception as e:
        log.warning("could not ping %s: %r", url, e)


def run(url: str, path: str, labels_path: str | None, start: int | None, ping_url: str | None = None) -> None:
    con = db.connect(path)
    last_ping = 0.0
    chain = Chain(url)
    proto = common.protocol_accounts()
    ctx = context({v: k for k, v in proto.items()})
    while True:
        try:
            best, fin = chain.heads()
            db.set_meta(con, "heads", {"best": best, "finalized": fin, "at": int(time.time())})
            nxt = db.get_meta(con, "next_block", start if start is not None else 0)
            if db.get_meta(con, "genesis_ts") is None and fin >= 1:
                db.set_meta(con, "genesis_ts", chain.block(1)["ts"])
            done = 0
            while nxt <= fin:
                store_block(con, chain.block(nxt), proto, ctx)
                nxt += 1
                done += 1
                db.set_meta(con, "next_block", nxt)
                if done % BATCH == 0:
                    con.commit()
                    log.info("indexed up to #%d (finalized #%d)", nxt - 1, fin)
                    if time.time() - last_ping >= 60:  # still alive while catching up
                        ping(ping_url, True, f"catching up: #{nxt - 1} of #{fin}")
                        last_ping = time.time()
            con.commit()
            last = db.get_meta(con, "snapshot", {}).get("block", -SNAPSHOT_EVERY)
            if fin - last >= SNAPSHOT_EVERY or db.get_meta(con, "snapshot") is None:
                h = chain.conn().get_block_hash(fin)
                store_snapshot(con, fin, chain.snapshot(h), proto, load_labels(labels_path))
                con.commit()
                log.info("snapshot at #%d", fin)
            if time.time() - last_ping >= 60:
                lag = fin - (nxt - 1)
                ping(ping_url, lag <= 50, f"indexed #{nxt - 1}, finalized #{fin}, lag {lag} blocks")
                last_ping = time.time()
            time.sleep(6 if done == 0 else 0.5)
        except KeyboardInterrupt:
            raise
        except Exception as e:
            con.rollback()
            log.warning("error, retrying in 10 s: %r", e)
            chain.reset()
            time.sleep(10)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="wss://rpc.incentium.org", help="archive node RPC")
    ap.add_argument("--db", required=True)
    ap.add_argument("--labels", help="JSON file {address: label} for known accounts (curator multisig, exchanges, ...)")
    ap.add_argument("--start", type=int, help="first block to index on an empty database (default 0)")
    ap.add_argument("--ping-url", help="healthchecks.io URL; pinged every minute, /fail when more than 50 blocks behind")
    ap.add_argument("-v", action="store_true")
    a = ap.parse_args()
    logging.basicConfig(level=logging.DEBUG if a.v else logging.INFO, format="%(asctime)s %(message)s")
    run(a.url, a.db, a.labels, a.start, a.ping_url)


if __name__ == "__main__":
    main()
