"""Pick an Incentium RPC endpoint from the on-chain register (spec Section 19.2).

The miner and validator software ship with a few seed endpoints. `pick()` reads the register
(pallet `endpoints`) through the first seed that answers, keeps active RPC and archive entries
with a wss:// URL, measures how fast each answers, and returns them fastest first with the seeds
last as a fallback. The result is cached for an hour, so a restart does not probe again. If the
chain has no register yet, or nothing answers, the seeds are returned unchanged.
"""

from __future__ import annotations

import json
import os
import time
from typing import Callable, Iterable

SEEDS = {
    "mainnet": ["wss://rpc.incentium.org", "wss://rpc2.incentium.org"],
    "testnet": ["wss://testnet-rpc.incentium.org"],
}
TTL = 3600
TIMEOUT = 3.0
MAX_PROBED = 8


def read_register(url: str) -> list[str]:
    """Active RPC and archive endpoints in the register, read through `url`."""
    from substrateinterface import SubstrateInterface

    s = SubstrateInterface(url=url, ws_options={"timeout": TIMEOUT})
    try:
        out = []
        for _id, entry in s.query_map("Endpoints", "Entries", page_size=200):
            e = entry.value
            u = e["url"]
            if isinstance(u, (bytes, bytearray)):
                u = u.decode()
            elif isinstance(u, str) and u.startswith("0x"):
                u = bytes.fromhex(u[2:]).decode()
            if e.get("active") and e.get("kind") in ("Rpc", "Archive") and u.startswith("wss://"):
                out.append(u)
        return out
    finally:
        s.close()


def latency(url: str) -> float:
    """Seconds for `system_health` to answer, or infinity."""
    import websocket

    t0 = time.monotonic()
    try:
        ws = websocket.create_connection(url, timeout=TIMEOUT)
        try:
            ws.send(json.dumps({"id": 1, "jsonrpc": "2.0", "method": "system_health", "params": []}))
            json.loads(ws.recv())
        finally:
            ws.close()
        return time.monotonic() - t0
    except Exception:
        return float("inf")


def _cache_path(home: str, net: str) -> str:
    return os.path.join(home, f"rpc-{net}.json")


def pick(
    net: str,
    home: str | None = None,
    register: Callable[[str], Iterable[str]] = read_register,
    measure: Callable[[str], float] = latency,
    now: Callable[[], float] = time.time,
) -> list[str]:
    """RPC endpoints for `net`, fastest first, seeds last."""
    seeds = list(SEEDS[net])
    if home:
        try:
            c = json.load(open(_cache_path(home, net)))
            if now() - c["at"] < TTL and c["list"]:
                return c["list"]
        except Exception:
            pass
    found: list[str] = []
    for s in seeds:
        try:
            found = list(register(s))
            break
        except Exception:
            continue
    cands: list[str] = []
    for u in found[:MAX_PROBED] + seeds:
        if u not in cands:
            cands.append(u)
    timed = [(measure(u), u) for u in cands]
    out = [u for t, u in sorted(timed, key=lambda x: x[0]) if t != float("inf")]
    out += [s for s in seeds if s not in out]
    if home:
        try:
            os.makedirs(home, exist_ok=True)
            with open(_cache_path(home, net), "w") as f:
                json.dump({"at": now(), "list": out}, f)
        except Exception:
            pass
    return out
