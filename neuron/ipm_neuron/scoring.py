"""Work units and spot checks for SN0 (spec Section 5).

W = T x Q x A x G
  T: output tokens served and accepted
  Q: quality factor 0..1 (share of passed checks this epoch)
  A: availability 0..1 (share of requests answered in time)
  G: tier weight
"""
from __future__ import annotations

import math
from dataclasses import dataclass, field

# Tier weights are calibrated on testnet; these are placeholders.
TIER_WEIGHTS = {1: 1.0, 2: 3.0, 3: 10.0}
# Maximum mean absolute difference between top-k log-probabilities, per tier.
LOGIT_TOLERANCE = {1: 0.35, 2: 0.25, 3: 0.40}
U16_MAX = 65_535


@dataclass
class MinerStats:
    uid: int
    tier: int = 1
    tokens: int = 0
    requests: int = 0
    answered: int = 0
    checks: int = 0
    checks_passed: int = 0
    failed_check_evidence: list[str] = field(default_factory=list)

    @property
    def availability(self) -> float:
        return self.answered / self.requests if self.requests else 0.0

    @property
    def quality(self) -> float:
        return self.checks_passed / self.checks if self.checks else 1.0

    def work_units(self) -> float:
        if self.failed_check_evidence:
            return 0.0
        return self.tokens * self.quality * self.availability * TIER_WEIGHTS.get(self.tier, 0.0)


def logprob_distance(miner: list[dict[str, float]], reference: list[dict[str, float]]) -> float:
    """Mean absolute difference of top-k log-probabilities over shared positions and tokens.

    Each list item is the top-k {token: logprob} at one output position. Tokens missing on one
    side count with a floor of log(1e-4), so a different distribution is penalised.
    """
    floor = math.log(1e-4)
    n = min(len(miner), len(reference))
    if n == 0:
        return math.inf
    total, count = 0.0, 0
    for i in range(n):
        keys = set(miner[i]) | set(reference[i])
        for k in keys:
            total += abs(miner[i].get(k, floor) - reference[i].get(k, floor))
            count += 1
    return total / count if count else math.inf


def passes_spot_check(tier: int, miner_lp, reference_lp) -> bool:
    return logprob_distance(miner_lp, reference_lp) <= LOGIT_TOLERANCE.get(tier, 0.0)


def to_weights(stats: list[MinerStats]) -> tuple[list[int], list[int]]:
    """Scale work units to u16 weights for commit/reveal (largest = 65,535)."""
    units = [(s.uid, s.work_units()) for s in stats]
    top = max((u for _, u in units), default=0.0)
    if top <= 0:
        return [], []
    uids, weights = [], []
    for uid, u in sorted(units):
        w = int(round(u / top * U16_MAX))
        if w > 0:
            uids.append(uid)
            weights.append(w)
    return uids, weights
