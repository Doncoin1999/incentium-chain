"""Watchdog for the Incentium nodes, reported to a dead man's switch (healthchecks.io).

Every minute it checks the chain and pings the switch: `<ping-url>` when all is well,
`<ping-url>/fail` with the list of problems when not. healthchecks.io emails when a check
fails *or* when the pings stop, so a server that is down or unreachable is caught too.

  python scripts/node_watchdog.py --ping-url https://hc-ping.com/<uuid> \
      --validator 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY \
      --faucet <faucet address> --public-rpc https://rpc.incentium.org
"""
from __future__ import annotations

import argparse
import json
import shutil
import time
import urllib.request
from dataclasses import dataclass, field

UNIT = 10**9


@dataclass
class Snapshot:
    now: float  # wall clock, seconds
    last_block_time: float  # timestamp of the best block, seconds
    best: int
    finalized: int
    epoch: int
    commits: dict[str, int | None] = field(default_factory=dict)  # validator -> epoch of last commit
    faucet_balance: int | None = None  # base units
    public_rpc_ok: bool | None = None
    disk_used_share: float = 0.0


def problems(s: Snapshot, max_block_age: float = 90, max_finality_lag: int = 10,
             max_commit_age: int = 2, min_faucet: int = 100 * UNIT, max_disk: float = 0.9) -> list[str]:
    out = []
    age = s.now - s.last_block_time
    if age > max_block_age:
        out.append(f"no new block for {age:.0f} s (best #{s.best})")
    if s.best - s.finalized > max_finality_lag:
        out.append(f"finality is {s.best - s.finalized} blocks behind (best #{s.best}, finalized #{s.finalized})")
    for v, e in s.commits.items():
        if e is None:
            out.append(f"validator {v[:8]}... has never committed weights")
        elif s.epoch - e > max_commit_age:
            out.append(f"validator {v[:8]}... last committed in epoch {e}, now epoch {s.epoch}")
    if s.faucet_balance is not None and s.faucet_balance < min_faucet:
        out.append(f"faucet balance is low: {s.faucet_balance / UNIT:,.2f} ICT")
    if s.public_rpc_ok is False:
        out.append("public RPC does not answer")
    if s.disk_used_share > max_disk:
        out.append(f"disk {s.disk_used_share:.0%} full")
    return out


def public_rpc_ok(url: str) -> bool:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "system_health", "params": []}).encode()
    req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            return "result" in json.loads(r.read())
    except Exception:
        return False


def take_snapshot(s, a, epoch_length: int) -> Snapshot:
    best_hash = s.get_chain_head()
    best = s.get_block_number(best_hash)
    finalized = s.get_block_number(s.get_chain_finalised_head())
    ts = s.query("Timestamp", "Now", block_hash=best_hash).value / 1000
    commits = {}
    for v in a.validator:
        c = s.query("Weights", "Commits", [0, v]).value
        commits[v] = int(c[1]) if c else None
    faucet = s.query("System", "Account", [a.faucet]).value["data"]["free"] if a.faucet else None
    du = shutil.disk_usage("/")
    return Snapshot(now=time.time(), last_block_time=ts, best=best, finalized=finalized,
                    epoch=best // epoch_length, commits=commits, faucet_balance=faucet,
                    public_rpc_ok=public_rpc_ok(a.public_rpc) if a.public_rpc else None,
                    disk_used_share=du.used / du.total)


def ping(url: str, fail: bool, text: str) -> None:
    try:
        urllib.request.urlopen(urllib.request.Request(url + ("/fail" if fail else ""), data=text.encode()),
                               timeout=15)
    except Exception as e:
        print(f"could not ping {url}: {e!r}", flush=True)


def main():
    from substrateinterface import SubstrateInterface

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--ping-url", required=True)
    ap.add_argument("--validator", action="append", default=[], help="validator hotkey to watch (repeatable)")
    ap.add_argument("--faucet", help="faucet address, to warn when it runs low")
    ap.add_argument("--public-rpc", help="public HTTPS RPC to check, e.g. https://rpc.incentium.org")
    ap.add_argument("--epoch-length", type=int, default=360)
    ap.add_argument("--every", type=int, default=60, help="seconds between checks")
    a = ap.parse_args()
    s = None
    while True:
        try:
            s = s or SubstrateInterface(url=a.url)
            found = problems(take_snapshot(s, a, a.epoch_length))
        except Exception as e:
            s = None
            found = [f"local node does not answer: {e!r}"]
        text = "\n".join(found) if found else "all checks passed"
        if found:
            print(text, flush=True)
        ping(a.ping_url, bool(found), text)
        time.sleep(a.every)


if __name__ == "__main__":
    main()
