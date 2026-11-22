#!/usr/bin/env python3
"""Upgrade a chain's runtime with sudo(set_code) and check the result.

Rehearsal on a throwaway dev chain (sudo is //Alice):
    python3 scripts/upgrade_runtime.py --url ws://127.0.0.1:9990 --suri //Alice \
        --wasm target/release/wbuild/incentium-runtime/incentium_runtime.compact.compressed.wasm

Real chain: leave out --suri and type the sudo account's secret phrase when asked (not echoed).

Needs: pip install substrate-interface
"""
import argparse
import getpass
import sys
import time

from substrateinterface import Keypair, SubstrateInterface

# pallet-revive's own account, PalletId(*b"py/reviv"); created by the CreateReviveAccount migration.
REVIVE_ACCOUNT = "5EYCAe5ijiYfhaAUBd6H9WGRTsvwFFc7GnhQkiHvBYxdvpbV"


def runtime_version(url):
    s = SubstrateInterface(url=url)
    v = s.rpc_request("state_getRuntimeVersion", [])["result"]
    return s, v["specVersion"], v["transactionVersion"]


def block_number(s):
    return s.get_block_header()["header"]["number"]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", required=True, help="node RPC, e.g. ws://127.0.0.1:9990")
    ap.add_argument("--wasm", required=True, help="the .compact.compressed.wasm to install")
    ap.add_argument("--suri", help="sudo key as a URI (//Alice on a dev chain); omit to be asked")
    ap.add_argument("--yes", action="store_true", help="do not ask for confirmation")
    a = ap.parse_args()

    code = open(a.wasm, "rb").read()
    s, spec, txv = runtime_version(a.url)
    chain = s.rpc_request("system_chain", [])["result"]
    print(f"Chain:   {chain} at {a.url}")
    print(f"Now:     specVersion {spec}, transactionVersion {txv}, block #{block_number(s)}")
    print(f"New:     {a.wasm} ({len(code) / 1e6:.2f} MB)")

    suri = a.suri or getpass.getpass("Sudo account secret phrase or URI (not shown): ")
    kp = Keypair.create_from_uri(suri.strip())
    sudo_key = s.query("Sudo", "Key").value
    if kp.ss58_address != sudo_key:
        sys.exit(f"That key ({kp.ss58_address}) is not the sudo key ({sudo_key}). Nothing sent.")

    if not a.yes:
        if input(f"Upgrade {chain}? Type 'yes': ").strip() != "yes":
            sys.exit("Cancelled. Nothing sent.")

    set_code = s.compose_call("System", "set_code", {"code": "0x" + code.hex()})
    call = s.compose_call(
        "Sudo",
        "sudo_unchecked_weight",
        {"call": set_code, "weight": {"ref_time": 0, "proof_size": 0}},
    )
    xt = s.create_signed_extrinsic(call=call, keypair=kp)
    print("Sending sudo(set_code) and waiting for inclusion...")
    r = s.submit_extrinsic(xt, wait_for_inclusion=True)
    if not r.is_success:
        sys.exit(f"Extrinsic failed: {r.error_message}")

    events = [(e.value["module_id"], e.value["event_id"], e.value["attributes"]) for e in r.triggered_events]
    sudid = [attrs for mod, ev, attrs in events if mod == "Sudo" and ev == "Sudid"]
    if sudid and "Err" in str(sudid[0]):
        sys.exit(f"Sudo call failed: {sudid[0]}")
    print(f"Included in block {r.block_hash}")

    # The new runtime applies from the next block.
    for _ in range(40):
        time.sleep(3)
        s2, new_spec, new_txv = runtime_version(a.url)
        if new_spec != spec:
            break
    else:
        sys.exit(f"specVersion is still {spec} after 2 minutes. Check the node log.")

    print(f"Now:     specVersion {new_spec}, transactionVersion {new_txv}")
    ok = True

    n1 = block_number(s2)
    time.sleep(15)
    n2 = block_number(s2)
    if n2 > n1:
        print(f"Blocks:  advancing (#{n1} -> #{n2})")
    else:
        print(f"Blocks:  NOT advancing (#{n1} -> #{n2}). Check the node log!")
        ok = False

    if s2.get_metadata_module("Revive") is None:
        print("Revive:  pallet missing from metadata!")
        ok = False
    else:
        print("Revive:  pallet present")
        acct = s2.query("System", "Account", [REVIVE_ACCOUNT]).value
        free = acct["data"]["free"]
        if free > 0:
            print(f"Account: revive account funded ({free / 1e9:.3f} ICT)")
        else:
            print(f"Account: revive account EMPTY. Send 0.001 ICT to {REVIVE_ACCOUNT}.")
            ok = False

    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
