"""SN0 reference validator.

Each epoch:
 1. Read miners and their endpoints from chain.
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
from typing import Protocol

from .scale import commit_hash
from .scoring import MinerStats, passes_spot_check, to_weights

SN0 = 0


class Chain(Protocol):
    def current_epoch(self) -> int: ...
    def miners(self, netuid: int) -> list[tuple[int, str, str]]: ...  # (uid, hotkey, endpoint)
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
        stats = []
        for uid, hotkey, endpoint in self.chain.miners(SN0):
            if uid == self.own_uid or not endpoint:
                continue
            s = MinerStats(uid=uid, tier=1)
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
                    if passes_spot_check(s.tier, ans["top_logprobs"], ref.top_logprobs):
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
            self.chain.reveal_weights(SN0, p.uids, p.weights, p.salt)
            self.log.append(f"revealed epoch {p.epoch}")
        stats = self.measure(epoch)
        for s in stats:
            for ev in s.failed_check_evidence[:1]:
                self.chain.report_failed_check(epoch, s.uid, bytes.fromhex(ev))
                self.log.append(f"reported uid {s.uid} in epoch {epoch}")
        uids, weights = to_weights(stats)
        if uids:
            salt = os.urandom(32)
            self.chain.commit_weights(SN0, commit_hash(SN0, uids, weights, salt))
            self.pending = PendingReveal(epoch, uids, weights, salt)
            self.log.append(f"committed epoch {epoch}: {dict(zip(uids, weights))}")


class SubstrateChain:
    """Chain access through py-substrate-interface (pip install substrate-interface)."""

    def __init__(self, url: str, keypair, epoch_length: int = 360):
        from substrateinterface import SubstrateInterface  # optional dependency

        self.s = SubstrateInterface(url=url)
        self.kp = keypair
        self.epoch_length = epoch_length

    def _submit(self, module: str, fn: str, params: dict):
        call = self.s.compose_call(call_module=module, call_function=fn, call_params=params)
        xt = self.s.create_signed_extrinsic(call=call, keypair=self.kp)
        r = self.s.submit_extrinsic(xt, wait_for_inclusion=True)
        if not r.is_success:
            raise RuntimeError(f"{module}.{fn} failed: {r.error_message}")

    def current_epoch(self) -> int:
        return self.s.get_block_number(None) // self.epoch_length

    def miners(self, netuid: int):
        out = []
        for key, hotkey in self.s.query_map("Registry", "Neurons", [netuid]):
            uid = key.value
            ep = self.s.query("Registry", "Endpoints", [netuid, hotkey.value])
            endpoint = bytes(ep.value).decode() if ep.value else ""
            out.append((uid, hotkey.value, endpoint))
        return sorted(out)

    def commit_weights(self, netuid, h):
        self._submit("Weights", "commit_weights", {"netuid": netuid, "hash": "0x" + h.hex()})

    def reveal_weights(self, netuid, uids, weights, salt):
        self._submit("Weights", "reveal_weights",
                     {"netuid": netuid, "uids": uids, "weights": weights, "salt": "0x" + salt.hex()})

    def report_failed_check(self, epoch, uid, evidence):
        self._submit("WorkVerify", "report_failed_check",
                     {"epoch": epoch, "uid": uid, "evidence": "0x" + evidence.hex()})


def main():
    """python -m ipm_neuron.validator --url ws://127.0.0.1:9944 --key //Alice --uid 0 \
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
    ap.add_argument("--epoch-length", type=int, default=360)
    a = ap.parse_args()
    chain = SubstrateChain(a.url, Keypair.create_from_uri(a.key), a.epoch_length)
    v = Validator(chain=chain, reference=OpenAICompatibleBackend(a.reference, a.model), own_uid=a.uid)
    last = -1
    while True:
        e = chain.current_epoch()
        if e != last:
            v.step()
            print("\n".join(v.log[-3:]), flush=True)
            last = e
        time.sleep(12)


if __name__ == "__main__":
    main()
