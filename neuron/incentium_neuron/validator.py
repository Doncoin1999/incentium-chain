"""SN0 reference validator.

Each epoch:
 1. Read miners, their endpoints and declared tiers, and the tier table, from chain.
    A validator checks the tier its reference model serves (`--tier`); miners of other
    tiers are left to validators that run those references.
 2. Send prompts (seeded by the epoch) to every miner; count tokens and answered requests.
 3. Spot-check a random share of answers by re-running them on the validator's own
    reference backend and comparing top-k log-probabilities.
 4. Commit the weights hash now; reveal last epoch's weights; report failed checks.

The chain is abstracted behind `Chain` so the loop is testable without a node.
"""
from __future__ import annotations

import hashlib
import json
import os
import random
import urllib.request
from dataclasses import dataclass, field
from typing import NamedTuple, Protocol

from .scale import commit_hash
from .scoring import MinerStats, passes_spot_check, to_weights

SN0 = 0
PPM = 1_000_000


class TierInfo(NamedTuple):
    g: float  # tier weight G
    tolerance: float  # max spot-check distance
    model: str
    model_hash: str  # hex, all zero until locked
    open: bool


class Chain(Protocol):
    def current_epoch(self) -> int: ...
    def miners(self, netuid: int) -> list[tuple]: ...  # (uid, hotkey, endpoint[, tier])
    def tiers(self) -> dict[int, TierInfo]: ...
    def commit_weights(self, netuid: int, h: bytes) -> None: ...
    def reveal_weights(self, netuid: int, uids: list[int], weights: list[int], salt: bytes) -> None: ...
    def report_failed_check(self, epoch: int, uid: int, evidence: bytes) -> None: ...


@dataclass
class PendingReveal:
    epoch: int
    uids: list[int]
    weights: list[int]
    salt: bytes


@dataclass
class Validator:
    chain: Chain
    reference: object  # backend with .complete(prompt, max_tokens, seed)
    own_uid: int
    tier: int = 0  # the tier `reference` serves
    prompts_per_miner: int = 8
    spot_check_share: float = 0.25  # high on devnet; 1-5 % on mainnet
    max_tokens: int = 32
    timeout: float = 30.0
    pending: PendingReveal | None = None
    log: list[str] = field(default_factory=list)

    def query(self, endpoint: str, prompt: str, seed: int) -> dict | None:
        body = json.dumps({"prompt": prompt, "max_tokens": self.max_tokens, "seed": seed}).encode()
        req = urllib.request.Request(endpoint.rstrip("/") + "/sn0/complete", data=body,
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                return json.loads(r.read())
        except Exception:
            return None

    def measure(self, epoch: int) -> list[MinerStats]:
        rng = random.Random(f"epoch-{epoch}-uid-{self.own_uid}")
        tiers = self.chain.tiers() if hasattr(self.chain, "tiers") else {}
        info = tiers.get(self.tier)
        g = info.g if info else None
        tol = info.tolerance if info else None
        stats = []
        for m in self.chain.miners(SN0):
            uid, hotkey, endpoint = m[:3]
            miner_tier = m[3] if len(m) > 3 else 0
            if uid == self.own_uid or not endpoint:
                continue
            if miner_tier != self.tier:
                self.log.append(f"epoch {epoch} uid {uid}: tier {miner_tier}, this validator checks "
                                f"tier {self.tier}; skipped")
                continue
            s = MinerStats(uid=uid, tier=miner_tier, g=g)
            for i in range(self.prompts_per_miner):
                prompt = f"SN0 task {epoch}:{uid}:{i}: {rng.getrandbits(64):x}"
                seed = rng.getrandbits(31)
                s.requests += 1
                ans = self.query(endpoint, prompt, seed)
                if not ans:
                    continue
                s.answered += 1
                s.tokens += int(ans.get("tokens", 0))
                if rng.random() < self.spot_check_share:
                    s.checks += 1
                    ref = self.reference.complete(prompt, self.max_tokens, seed)
                    if passes_spot_check(s.tier, ans["top_logprobs"], ref.top_logprobs, tol):
                        s.checks_passed += 1
                    else:
                        ev = hashlib.blake2b(json.dumps([prompt, seed, ans]).encode(), digest_size=32).digest()
                        s.failed_check_evidence.append(ev.hex())
            stats.append(s)
        return stats

    def step(self) -> None:
        """Run once per epoch."""
        epoch = self.chain.current_epoch()
        if self.pending and self.pending.epoch == epoch - 1:
            p = self.pending
            try:
                self.chain.reveal_weights(SN0, p.uids, p.weights, p.salt)
                self.log.append(f"revealed epoch {p.epoch}")
            except RuntimeError as e:
                self.log.append(f"reveal for epoch {p.epoch} failed: {e}")
            self.pending = None
        stats = self.measure(epoch)
        for s in stats:
            self.log.append(f"epoch {epoch} uid {s.uid}: answered {s.answered}/{s.requests}, "
                            f"tokens {s.tokens}, spot checks passed {s.checks_passed}/{s.checks}")
        for s in stats:
            for ev in s.failed_check_evidence[:1]:
                self.chain.report_failed_check(epoch, s.uid, bytes.fromhex(ev))
                self.log.append(f"reported uid {s.uid} in epoch {epoch}")
        uids, weights = to_weights(stats)
        if uids:
            salt = os.urandom(32)
            # Measuring can take longer than an epoch: the chain records the commit in the
            # epoch it is included in, and the reveal must come in the epoch after that.
            commit_epoch = self.chain.current_epoch()
            try:
                self.chain.commit_weights(SN0, commit_hash(SN0, uids, weights, salt))
            except RuntimeError as e:
                self.log.append(f"commit in epoch {commit_epoch} failed: {e}")
                return
            self.pending = PendingReveal(commit_epoch, uids, weights, salt)
            self.log.append(f"committed in epoch {commit_epoch}: {dict(zip(uids, weights))}")


class SubstrateChain:
    """Chain access through py-substrate-interface (pip install substrate-interface)."""

    def __init__(self, url: str, keypair, epoch_length: int = 360):
        from substrateinterface import SubstrateInterface  # optional dependency

        self.url = url
        self.s = SubstrateInterface(url=url)
        self.kp = keypair
        self.epoch_length = epoch_length

    def _retry(self, f):
        """The node drops idle websocket connections while the validator is busy measuring;
        reconnect once and retry."""
        try:
            return f()
        except (BrokenPipeError, ConnectionError, OSError) as e:
            from substrateinterface import SubstrateInterface

            print(f"chain connection lost ({e!r}); reconnecting", flush=True)
            self.s = SubstrateInterface(url=self.url)
            return f()

    def _submit(self, module: str, fn: str, params: dict):
        def go():
            call = self.s.compose_call(call_module=module, call_function=fn, call_params=params)
            xt = self.s.create_signed_extrinsic(call=call, keypair=self.kp)
            return self.s.submit_extrinsic(xt, wait_for_inclusion=True)

        r = self._retry(go)
        if not r.is_success:
            raise RuntimeError(f"{module}.{fn} failed: {r.error_message}")

    def current_epoch(self) -> int:
        return self._retry(lambda: self.s.get_block_number(None)) // self.epoch_length

    def miners(self, netuid: int):
        return self._retry(lambda: self._miners(netuid))

    def _miners(self, netuid: int):
        out = []
        for key, hotkey in self.s.query_map("Registry", "Neurons", [netuid]):
            uid = key.value
            ep = self.s.query("Registry", "Endpoints", [netuid, hotkey.value])
            endpoint = decode_bytes(ep.value)
            tier = self.s.query("Registry", "NeuronTier", [netuid, hotkey.value]).value or 0
            out.append((uid, hotkey.value, endpoint, int(tier)))
        return sorted(out)

    def tiers(self) -> dict[int, TierInfo]:
        return self._retry(self._tiers)

    def _tiers(self) -> dict[int, TierInfo]:
        out = {}
        for key, v in self.s.query_map("Registry", "Tiers"):
            t = v.value
            h = t["model_hash"]
            h = h[2:] if isinstance(h, str) and h.startswith("0x") else bytes(h).hex() if not isinstance(h, str) else h
            out[int(key.value)] = TierInfo(t["weight_ppm"] / PPM, t["tolerance_ppm"] / PPM,
                                           decode_bytes(t["model"]), h, bool(t["open"]))
        return out

    def declare_tier(self, netuid: int, tier: int):
        self._submit("Registry", "declare_tier", {"netuid": netuid, "tier": tier})

    def commit_weights(self, netuid, h):
        self._submit("Weights", "commit_weights", {"netuid": netuid, "hash": "0x" + h.hex()})

    def reveal_weights(self, netuid, uids, weights, salt):
        self._submit("Weights", "reveal_weights",
                     {"netuid": netuid, "uids": uids, "weights": weights, "salt": "0x" + salt.hex()})

    def report_failed_check(self, epoch, uid, evidence):
        self._submit("WorkVerify", "report_failed_check",
                     {"epoch": epoch, "uid": uid, "evidence": "0x" + evidence.hex()})


def decode_bytes(v) -> str:
    """substrate-interface returns Vec<u8> as a str (plain or 0x-hex) or as a list of ints."""
    if not v:
        return ""
    if isinstance(v, str):
        if v.startswith("0x"):
            try:
                return bytes.fromhex(v[2:]).decode()
            except ValueError:
                return v
        return v
    return bytes(v).decode()


def main():
    """python -m incentium_neuron.validator --url ws://127.0.0.1:9944 --key //Alice --uid 0 \
    --reference http://127.0.0.1:8080 --model qwen3.5-9b-q4"""
    import argparse
    import time

    from substrateinterface import Keypair

    from .backend import OpenAICompatibleBackend

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--key", required=True, help="secret URI of the validator hotkey, e.g. //Alice")
    ap.add_argument("--uid", type=int, required=True)
    ap.add_argument("--reference", required=True, help="OpenAI-compatible base URL of the reference model")
    ap.add_argument("--model", required=True)
    ap.add_argument("--tier", type=int, default=0, help="tier the reference model serves (0 = 2B)")
    ap.add_argument("--epoch-length", type=int, default=360)
    ap.add_argument("--prompts", type=int, default=8, help="prompts per miner per epoch")
    ap.add_argument("--max-tokens", type=int, default=32)
    a = ap.parse_args()
    chain = SubstrateChain(a.url, Keypair.create_from_uri(a.key), a.epoch_length)
    info = chain.tiers().get(a.tier)
    if info:
        print(f"checking tier {a.tier}: {info.model}, G {info.g}, tolerance {info.tolerance}", flush=True)
    v = Validator(chain=chain, reference=OpenAICompatibleBackend(a.reference, a.model), own_uid=a.uid,
                  tier=a.tier, prompts_per_miner=a.prompts, max_tokens=a.max_tokens)
    last = -1
    while True:
        e = chain.current_epoch()
        if e != last:
            n = len(v.log)
            v.step()
            print("\n".join(v.log[n:]), flush=True)
            last = chain.current_epoch()  # a step can outlast an epoch; wait for the next one
        time.sleep(12)


if __name__ == "__main__":
    main()
