"""Show SN0's open model list, or declare the model a miner runs.

  python -m incentium_neuron.tier                              # the list and who declared what
  python -m incentium_neuron.tier --key //Charlie --model-id 0 # declare model 0 for this hotkey

Model ids are the former tier numbers: 0 is Qwen3.5-2B, the only open model in the launch
period. A neuron that never declared runs model 0. (The module keeps its old name so existing
scripts keep working.)
"""
from __future__ import annotations

import argparse

from .validator import SN0, SubstrateChain


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--key", help="secret URI of the neuron hotkey, e.g. //Charlie")
    ap.add_argument("--model-id", "--tier", dest="model", type=int)
    a = ap.parse_args()
    kp = None
    if a.key:
        from substrateinterface import Keypair

        kp = Keypair.create_from_uri(a.key)
    chain = SubstrateChain(a.url, kp)
    if a.model is not None:
        if kp is None:
            ap.error("--model-id needs --key")
        chain.declare_model(SN0, a.model)
        print(f"{kp.ss58_address} declared model {a.model}")
    for t, i in sorted(chain.tiers().items()):
        pinned = "pinned" if i.model_hash.strip("0") else "not pinned"
        holders = chain.model_holders(t) if chain.model_list() else "-"
        print(f"model {t}: {i.model:14} {i.status:9} G {i.g:<5} tolerance {i.tolerance:<5} "
              f"memory {i.mem_mb or '?'} MB  hash {pinned}  validators {holders}")
    for uid, hotkey, endpoint, model in chain.miners(SN0):
        print(f"uid {uid}: model {model}  {hotkey}  {endpoint or '-'}")


if __name__ == "__main__":
    main()
