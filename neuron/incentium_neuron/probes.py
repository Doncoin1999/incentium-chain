"""Endpoint probes (spec Section 19.2): SN0 validators test the public RPC and archive nodes in
the on-chain register (`Endpoints.Entries`) during the day and report the share of passed
probes the next day (`endpoints.report`). The chain turns the reports into a stake-weighted
median score per endpoint and pays the endpoint pot from them.

A probe passes when the endpoint answers within the time limit and
  * returns the same hash for a recent finalized block as this validator's own node, and
  * for an archive node also returns the state of an old block: `System::Number` at a block
    about 100,000 blocks back must decode to that block's number. A pruned node no longer has
    that state and fails, so an RPC node registered as an archive does not get the archive weight.
Bootnodes are listed but not paid, and are not probed.

  python -m incentium_neuron.probes --url ws://127.0.0.1:9944 --key-file hotkey.json   # probe and report
  python -m incentium_neuron.probes --url ws://127.0.0.1:9944 --once --dry-run         # one round, print

The validator runs the same loop in a thread (`validator --probe-endpoints`, on by default when
the runtime has the register). State is kept in a JSON file, so a restart keeps the day's counts.
"""
from __future__ import annotations

import json
import os
import threading
import time
import urllib.request
from dataclasses import dataclass, field
from typing import Callable

BLOCKS_PER_DAY = 7_200
PROBE_EVERY = 30 * 60          # seconds between rounds: 48 probes per endpoint per day
TIMEOUT = 5.0                  # seconds an endpoint has to answer
FINALITY_MARGIN = 2            # probe a block a little below our finalized head
ARCHIVE_DEPTH = 100_000        # an archive node must still have the state this far back
FULL = 100
# twox128("System") ++ twox128("Number")
SYSTEM_NUMBER_KEY = "0x26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac"


@dataclass
class Endpoint:
    id: int
    kind: str      # Rpc | Archive | Bootnode
    url: str
    active: bool = True


@dataclass
class Reference:
    """What a probe is compared with, read from this validator's own node."""
    number: int                 # a recent finalized block
    hash: str
    old_number: int | None      # a block about ARCHIVE_DEPTH back, for archive probes
    old_hash: str | None


RpcCall = Callable[[str, str, list, float], object]


def http_url(url: str) -> str:
    """JSON-RPC over HTTP works on the same port as the websocket for Substrate nodes."""
    if url.startswith("wss://"):
        return "https://" + url[6:]
    if url.startswith("ws://"):
        return "http://" + url[5:]
    return url


def rpc_http(url: str, method: str, params: list, timeout: float) -> object:
    body = json.dumps({"id": 1, "jsonrpc": "2.0", "method": method, "params": params}).encode()
    req = urllib.request.Request(http_url(url), data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        d = json.loads(r.read())
    if "error" in d:
        raise RuntimeError(str(d["error"]))
    return d.get("result")


def probe(ep: Endpoint, ref: Reference, rpc: RpcCall = rpc_http, timeout: float = TIMEOUT) -> bool:
    """One probe of one endpoint. Any error, a slow answer or a wrong answer fails it."""
    t0 = time.monotonic()
    try:
        if rpc(ep.url, "chain_getBlockHash", [ref.number], timeout) != ref.hash:
            return False
        if ep.kind == "Archive":
            if ref.old_hash is None:
                return False
            raw = rpc(ep.url, "state_getStorage", [SYSTEM_NUMBER_KEY, ref.old_hash], timeout)
            if not isinstance(raw, str) or int.from_bytes(bytes.fromhex(raw[2:]), "little") != ref.old_number:
                return False
    except Exception:
        return False
    return time.monotonic() - t0 <= timeout * (2 if ep.kind == "Archive" else 1)


@dataclass
class ProbeState:
    """Per day and endpoint: [passed, probes]; and the days already reported."""
    path: str | None = None
    days: dict[int, dict[int, list[int]]] = field(default_factory=dict)
    reported: set[int] = field(default_factory=set)

    @classmethod
    def load(cls, path: str | None) -> "ProbeState":
        st = cls(path)
        if path and os.path.exists(path):
            try:
                d = json.load(open(path))
                st.days = {int(k): {int(i): v for i, v in m.items()} for k, m in d.get("days", {}).items()}
                st.reported = set(d.get("reported", []))
            except (OSError, ValueError):
                pass
        return st

    def save(self):
        if not self.path:
            return
        os.makedirs(os.path.dirname(os.path.abspath(self.path)), exist_ok=True)
        tmp = self.path + ".tmp"
        with open(tmp, "w") as f:
            json.dump({"days": self.days, "reported": sorted(self.reported)}, f)
        os.replace(tmp, self.path)

    def record(self, day: int, ep_id: int, ok: bool):
        c = self.days.setdefault(day, {}).setdefault(ep_id, [0, 0])
        c[0] += int(ok)
        c[1] += 1

    def scores(self, day: int) -> list[tuple[int, int]]:
        """(endpoint id, percent of passed probes) for every endpoint probed that day."""
        return sorted((i, round(FULL * p / n)) for i, (p, n) in self.days.get(day, {}).items() if n)

    def prune(self, today: int):
        for d in [d for d in self.days if d < today - 2]:
            del self.days[d]
        self.reported = {d for d in self.reported if d >= today - 2}


@dataclass
class Prober:
    chain: object               # SubstrateChain (or a fake): endpoints(), reference(), block_number(), report_endpoints()
    state: ProbeState
    rpc: RpcCall = rpc_http
    log: Callable[[str], None] = print
    dry_run: bool = False

    def round(self) -> int:
        """Probe every registered RPC and archive endpoint once. Returns how many passed."""
        n = self.chain.block_number()
        day = n // BLOCKS_PER_DAY
        ref = self.chain.reference()
        passed = 0
        for ep in self.chain.endpoints():
            if ep.kind == "Bootnode":
                continue
            ok = probe(ep, ref, self.rpc)
            self.state.record(day, ep.id, ok)
            passed += ok
        self.state.save()
        return passed

    def report_due(self) -> None:
        """Report yesterday once, if anything was probed."""
        today = self.chain.block_number() // BLOCKS_PER_DAY
        day = today - 1
        if day < 0 or day in self.state.reported:
            return
        scores = self.state.scores(day)
        if not scores:
            self.state.reported.add(day)
            return
        if self.dry_run:
            self.log(f"endpoint report for day {day} (dry run): {scores}")
        else:
            try:
                self.chain.report_endpoints(day, scores)
                self.log(f"reported endpoint scores for day {day}: {scores}")
            except RuntimeError as e:
                msg = str(e)
                if "NotReporter" in msg:
                    self.log("this key has no reporter stake; endpoint reports are not counted")
                elif "AlreadyReported" not in msg:
                    self.log(f"endpoint report for day {day} failed: {msg}")
                    return  # retry next round
        self.state.reported.add(day)
        self.state.prune(today)
        self.state.save()

    def step(self) -> None:
        self.report_due()
        passed = self.round()
        self.log(f"probed endpoints: {passed} passed")

    def run_forever(self, stop: threading.Event | None = None, every: float = PROBE_EVERY) -> None:
        stop = stop or threading.Event()
        while not stop.is_set():
            try:
                self.step()
            except Exception as e:  # the node may be restarting; try again next round
                self.log(f"endpoint probe round failed: {e!r}")
            stop.wait(every)


# -- chain access (substrate-interface), added to SubstrateChain by `attach` ------------------

def chain_endpoints(chain) -> list[Endpoint]:
    from .validator import decode_bytes

    def go():
        out = []
        for key, v in chain.s.query_map("Endpoints", "Entries"):
            e = v.value
            kind = e["kind"] if isinstance(e["kind"], str) else next(iter(e["kind"]))
            if e.get("active", True):
                out.append(Endpoint(int(key.value), kind, decode_bytes(e["url"]), True))
        return sorted(out, key=lambda x: x.id)
    return chain._retry(go)


def chain_reference(chain) -> Reference:
    def go():
        head = chain.s.rpc_request("chain_getFinalizedHead", [])["result"]
        fin = int(chain.s.rpc_request("chain_getHeader", [head])["result"]["number"], 16)
        number = max(fin - FINALITY_MARGIN, 0)
        h = chain.s.rpc_request("chain_getBlockHash", [number])["result"]
        old = fin - ARCHIVE_DEPTH if fin > ARCHIVE_DEPTH else (1 if fin > 1 else None)
        old_hash = chain.s.rpc_request("chain_getBlockHash", [old])["result"] if old else None
        return Reference(number, h, old, old_hash)
    return chain._retry(go)


def has_register(chain) -> bool:
    return chain.has_storage("Endpoints", "Entries")


def attach(chain):
    """Give a validator.SubstrateChain the calls a Prober needs."""
    chain.endpoints = lambda: chain_endpoints(chain)
    chain.reference = lambda: chain_reference(chain)
    chain.block_number = lambda: chain._retry(lambda: chain.s.get_block_number(None))
    chain.report_endpoints = lambda day, scores: chain._submit(
        "Endpoints", "report", {"day": day, "scores": [[i, s] for i, s in scores]})
    return chain


def start_thread(chain, state_file: str, log=print) -> threading.Thread:
    p = Prober(attach(chain), ProbeState.load(state_file), log=log)
    t = threading.Thread(target=p.run_forever, name="endpoint-probes", daemon=True)
    t.start()
    return t


def main():
    import argparse

    from .validator import SubstrateChain, read_key

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944", help="this operator's own node")
    ap.add_argument("--key", help="secret URI of the reporting key (an SN0 validator hotkey)")
    ap.add_argument("--key-file")
    ap.add_argument("--state-file", default=os.path.expanduser("~/.incentium/endpoint-probes.json"))
    ap.add_argument("--once", action="store_true", help="one round, then exit")
    ap.add_argument("--dry-run", action="store_true", help="probe and print; never report")
    a = ap.parse_args()
    kp = None
    if not a.dry_run:
        from substrateinterface import Keypair

        from .addr import SS58_FORMAT

        kp = Keypair.create_from_uri(read_key(a, ap), ss58_format=SS58_FORMAT)
    chain = attach(SubstrateChain(a.url, kp))
    if not has_register(chain):
        raise SystemExit("this chain has no endpoint register (Endpoints pallet) yet")
    p = Prober(chain, ProbeState.load(a.state_file), dry_run=a.dry_run,
               log=lambda m: print(m, flush=True))
    if a.once:
        ref = chain.reference()
        for ep in chain.endpoints():
            if ep.kind != "Bootnode":
                print(f"{ep.id:4} {ep.kind:8} {ep.url:50} {'pass' if probe(ep, ref) else 'FAIL'}", flush=True)
        return
    p.run_forever()


if __name__ == "__main__":
    main()
