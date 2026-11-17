"""Show the tier table, or declare the tier a miner serves.

  python -m incentium_neuron.tier                          # show tiers and who declared what
  python -m incentium_neuron.tier --key //Charlie --tier 0 # declare tier 0 for this hotkey

Tier 0 (Qwen3.5-2B, CPU) is the only open tier in the launch period. A neuron that never
declared counts as tier 0.
"""
from __future__ import annotations

import argparse

from .validator import SN0, SubstrateChain


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--key", help="secret URI of the neuron hotkey, e.g. //Charlie")
    ap.add_argument("--tier", type=int)
    a = ap.parse_args()
    kp = None
    if a.key:
        from substrateinterface import Keypair

        kp = Keypair.create_from_uri(a.key)
    chain = SubstrateChain(a.url, kp)
    if a.tier is not None:
        if kp is None:
            ap.error("--tier needs --key")
        chain.declare_tier(SN0, a.tier)
        print(f"{kp.ss58_address} declared tier {a.tier}")
    for t, i in sorted(chain.tiers().items()):
        locked = "locked" if i.model_hash.strip("0") else "not locked"
        print(f"tier {t}: {i.model:14} G {i.g:<5} tolerance {i.tolerance:<5} "
              f"{'open' if i.open else 'closed':6} model hash {locked}")
    for uid, hotkey, endpoint, tier in chain.miners(SN0):
        print(f"uid {uid}: tier {tier}  {hotkey}  {endpoint or '-'}")


if __name__ == "__main__":
    main()
