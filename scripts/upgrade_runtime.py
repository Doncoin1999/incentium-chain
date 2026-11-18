#!/usr/bin/env python3
"""Live runtime upgrade: take the wasm embedded in a node binary and install it with sudo.

  # devnet (fast-epochs binary, sudo = //Alice)
  python scripts/upgrade_runtime.py --node ~/incentium-node/incentium-node --url ws://127.0.0.1:9950 --key //Alice
  # testnet (normal binary, sudo = operator key)
  python scripts/upgrade_runtime.py --node ~/incentium-node-testnet/incentium-node --url ws://127.0.0.1:9960 \
      --key-file ~/incentium-testnet/secrets/operator.json

The wasm is read from `export-chain-spec --chain local --raw` (storage key :code). Sudo reports
success even if the inner set_code fails, so the script compares spec_version before and after.
"""
import argparse
import json
import os
import subprocess
import sys
import time

CODE_KEY = "0x3a636f6465"  # ":code"


def wasm_from_binary(node: str) -> str:
    out = subprocess.run([os.path.expanduser(node), "export-chain-spec", "--chain", "local", "--raw"],
                         check=True, capture_output=True, text=True).stdout
    return json.loads(out)["genesis"]["raw"]["top"][CODE_KEY]


def read_secret(a) -> str:
    if a.key_file:
        text = open(os.path.expanduser(a.key_file)).read().strip()
        return json.loads(text)["secretPhrase"] if text.startswith("{") else text
    return a.key


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", required=True, help="node binary whose embedded runtime to install")
    ap.add_argument("--url", required=True, help="RPC of the chain to upgrade")
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--key", help="sudo secret (phrase or //Alice)")
    g.add_argument("--key-file", help="sudo key: phrase or JSON with secretPhrase")
    a = ap.parse_args()

    from substrateinterface import Keypair, SubstrateInterface

    code = wasm_from_binary(a.node)
    s = SubstrateInterface(url=a.url)
    before = s.get_block_runtime_version(s.get_chain_head())["specVersion"]
    kp = Keypair.create_from_uri(read_secret(a))
    sudo_key = s.query("Sudo", "Key").value
    if Keypair(ss58_address=sudo_key).public_key != kp.public_key:
        print(f"this key is not the sudo key ({sudo_key})")
        return 1
    print(f"spec_version now {before}; wasm {len(code) // 2 - 1:,} bytes; submitting set_code ...")
    call = s.compose_call("Sudo", "sudo_unchecked_weight", {
        "call": s.compose_call("System", "set_code", {"code": code}),
        "weight": {"ref_time": 0, "proof_size": 0}})
    r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
    print(f"included in block {r.block_hash}")
    for _ in range(12):  # the new version applies from the next block
        time.sleep(6)
        s = SubstrateInterface(url=a.url)
        after = s.get_block_runtime_version(s.get_chain_head())["specVersion"]
        if after != before:
            print(f"OK: spec_version {before} -> {after}")
            return 0
    print(f"spec_version still {before}: the upgrade did not apply (same or older wasm?)")
    return 1


if __name__ == "__main__":
    sys.exit(main())
