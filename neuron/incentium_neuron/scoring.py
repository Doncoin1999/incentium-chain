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

# Fallback values. The chain's tier table (Registry.Tiers) is authoritative; validators read
# it every epoch and pass G and the tolerance in explicitly.
TIER_WEIGHTS = {0: 0.15, 1: 1.0, 2: 3.0, 3: 5.0}
# Maximum mean absolute difference between top-k log-probabilities, per tier.
LOGIT_TOLERANCE = {0: 0.35, 1: 0.35, 2: 0.25, 3: 0.40}
U16_MAX = 65_535


@dataclass
class MinerStats:
    uid: int
    tier: int = 0
    g: float | None = None  # tier weight from chain; None = TIER_WEIGHTS fallback
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
        g = self.g if self.g is not None else TIER_WEIGHTS.get(self.tier, 0.0)
        return self.tokens * self.quality * self.availability * g


def logprob_distance(miner: list[dict[str, float]], reference: list[dict[str, float]]) -> float:
    """Mean absolute difference of top-k log-probabilities over shared positions and tokens.

    Each list item is the top-k {token: logprob} at one output position. A token missing from
    one side's top-k is below that side's k-th value, so it is counted at that side's smallest
    shown log-probability. Honest runs differ only in near-ties at the tail of the top-k, which
    this treats gently; a different model has different leading tokens and values and still
    scores far apart. (An earlier fixed floor of log(1e-4) failed honest CPU runs.)
    """
    n = min(len(miner), len(reference))
    if n == 0:
        return math.inf
    total, count = 0.0, 0
    for i in range(n):
        m, r = miner[i], reference[i]
        if not m or not r:
            continue
        m_floor, r_floor = min(m.values()), min(r.values())
        for k in set(m) | set(r):
            total += abs(m.get(k, m_floor) - r.get(k, r_floor))
            count += 1
    return total / count if count else math.inf


def passes_spot_check(tier: int, miner_lp, reference_lp, tolerance: float | None = None) -> bool:
    tol = tolerance if tolerance is not None else LOGIT_TOLERANCE.get(tier, 0.0)
    return logprob_distance(miner_lp, reference_lp) <= tol


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
