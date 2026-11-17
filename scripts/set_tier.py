"""Change one tier's parameters with sudo (devnet and launch period; governance later).

Lock tier 0 to the reference weights file and keep everything else as it is:
  python scripts/set_tier.py --tier 0 --model-file ~/models/Qwen3.5-2B-Q4_K_M.gguf
Open tier 1 at day 90:
  python scripts/set_tier.py --tier 1 --open yes
"""
import argparse
import hashlib
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "neuron"))

from substrateinterface import Keypair, SubstrateInterface  # noqa: E402

from incentium_neuron.validator import PPM, SubstrateChain  # noqa: E402


def sha256_file(path):
    h = hashlib.sha256()
    with open(os.path.expanduser(path), "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--sudo", default="//Alice")
    ap.add_argument("--tier", type=int, required=True)
    ap.add_argument("--g", type=float, help="tier weight G, e.g. 0.15")
    ap.add_argument("--tolerance", type=float)
    ap.add_argument("--model", help="reference model name")
    ap.add_argument("--model-file", help="weights file to hash (SHA-256)")
    ap.add_argument("--open", choices=["yes", "no"])
    a = ap.parse_args()
    cur = SubstrateChain(a.url, None).tiers()[a.tier]
    model_hash = sha256_file(a.model_file) if a.model_file else cur.model_hash
    params = {
        "tier": a.tier,
        "weight_ppm": round((a.g if a.g is not None else cur.g) * PPM),
        "tolerance_ppm": round((a.tolerance if a.tolerance is not None else cur.tolerance) * PPM),
        "model": (a.model or cur.model).encode(),
        "model_hash": "0x" + model_hash,
        "open": (a.open == "yes") if a.open else cur.open,
    }
    s = SubstrateInterface(url=a.url)
    call = s.compose_call("Registry", "set_tier", params)
    sudo = s.compose_call("Sudo", "sudo", {"call": call})
    r = s.submit_extrinsic(s.create_signed_extrinsic(call=sudo, keypair=Keypair.create_from_uri(a.sudo)),
                           wait_for_inclusion=True)
    print("ok" if r.is_success else f"failed: {r.error_message}")
    for t, i in sorted(SubstrateChain(a.url, None).tiers().items()):
        print(t, i)


if __name__ == "__main__":
    main()
