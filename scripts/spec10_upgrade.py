#!/usr/bin/env python3
"""Checks and the sudo(set_code) step for the spec 10 upgrade (subnet registration).

Used by scripts/upgrade-spec10.sh; it can also be run by hand:

  python3 scripts/spec10_upgrade.py preflight --url ws://127.0.0.1:9960 --source <repo> \
      --key-file ~/incentium-testnet/secrets/operator.json
  python3 scripts/spec10_upgrade.py upgrade --url ... --key-file ... --wasm <file> [--yes]
  python3 scripts/spec10_upgrade.py verify --url ... --before <file written by preflight>

preflight: the chain is reachable and producing blocks, its spec_version is below 10, the key
           is the sudo key, and every pallet the chain has today is also in the runtime source
           that will be built (so the upgrade cannot remove one, e.g. Revive on the testnet).
upgrade:   submits sudo_unchecked_weight(system.set_code(wasm)) after typing "yes".
verify:    spec_version is 10, blocks advance, no pallet disappeared, the registry's new storage
           reads as expected and the SubnetRegistryApi runtime API answers.

Needs: pip install substrate-interface
"""
import argparse
import json
import os
import re
import sys
import time

TARGET_SPEC = 10


def read_secret(path):
    text = open(os.path.expanduser(path)).read().strip()
    return json.loads(text)["secretPhrase"] if text.startswith("{") else text


def connect(url):
    from substrateinterface import SubstrateInterface
    return SubstrateInterface(url=url)


def spec_version(s):
    return s.rpc_request("state_getRuntimeVersion", [])["result"]["specVersion"]


def head_number(s):
    return s.get_block_header()["header"]["number"]


def chain_pallets(s):
    s.init_runtime()
    return sorted(p.name for p in s.metadata.pallets)


def source_pallets(repo):
    text = open(os.path.join(repo, "runtime", "src", "lib.rs")).read()
    return sorted(re.findall(r"#\[runtime::pallet_index\(\d+\)\]\s*pub type (\w+)", text))


def source_spec(repo):
    m = re.search(r"spec_version:\s*(\d+)", open(os.path.join(repo, "runtime", "src", "lib.rs")).read())
    return int(m.group(1)) if m else None


def advancing(s, wait=40):
    a = head_number(s)
    t = time.time()
    while time.time() - t < wait:
        time.sleep(4)
        if head_number(s) > a:
            return True
    return False


def cmd_preflight(a):
    from substrateinterface import Keypair
    ok = True
    s = connect(a.url)
    v = spec_version(s)
    print(f"chain:          {s.chain} at {a.url}")
    print(f"spec_version:   {v} on chain, {source_spec(a.source)} in the source")
    if source_spec(a.source) != TARGET_SPEC:
        print(f"STOP: the source is not spec {TARGET_SPEC}")
        ok = False
    if v >= TARGET_SPEC:
        print(f"STOP: the chain is already at {v}")
        ok = False
    if not advancing(s):
        print("STOP: no new block in 40 seconds")
        ok = False
    else:
        print(f"blocks:         advancing (head {head_number(s)})")
    have, will = chain_pallets(s), source_pallets(a.source)
    missing = [p for p in have if p not in will]
    added = [p for p in will if p not in have]
    print(f"pallets:        {len(have)} on chain, {len(will)} in the source")
    if added:
        print(f"                new with this upgrade: {', '.join(added)}")
    if missing:
        print(f"STOP: the upgrade would remove {', '.join(missing)}. Wrong branch for this chain?")
        ok = False
    sudo_key = s.query("Sudo", "Key").value
    kp = Keypair.create_from_uri(read_secret(a.key_file))
    if not sudo_key:
        print("STOP: the chain has no sudo key any more; the upgrade must go through a referendum")
        ok = False
    elif Keypair(ss58_address=sudo_key).public_key != kp.public_key:
        print(f"STOP: {a.key_file} is not the sudo key ({sudo_key})")
        ok = False
    else:
        free = s.query("System", "Account", [kp.ss58_address]).value["data"]["free"]
        print(f"sudo key:       matches ({kp.ss58_address}), {free / 1e9:,.3f} ICT for fees")
    if a.save:
        json.dump({"spec": v, "pallets": have}, open(a.save, "w"))
    print("PREFLIGHT OK" if ok else "PREFLIGHT FAILED")
    return 0 if ok else 1


def cmd_upgrade(a):
    from substrateinterface import Keypair
    code = open(a.wasm, "rb").read()
    s = connect(a.url)
    before = spec_version(s)
    kp = Keypair.create_from_uri(read_secret(a.key_file))
    print(f"About to install {a.wasm} ({len(code):,} bytes) on {s.chain} at {a.url}.")
    print(f"spec_version {before} -> {TARGET_SPEC}. This cannot be undone, only fixed by a later upgrade.")
    if not a.yes and input('Type "yes" to go ahead: ').strip() != "yes":
        print("stopped, nothing sent")
        return 1
    call = s.compose_call("Sudo", "sudo_unchecked_weight", {
        "call": s.compose_call("System", "set_code", {"code": "0x" + code.hex()}),
        "weight": {"ref_time": 0, "proof_size": 0}})
    r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
    print(f"included in block {r.block_hash}")
    sudid = [e.value for e in r.triggered_events if e.value["event_id"] == "Sudid"]
    if sudid and "Err" in json.dumps(sudid[0]):
        print(f"set_code FAILED inside sudo: {sudid[0]}")
        return 1
    for _ in range(20):
        time.sleep(6)
        after = spec_version(connect(a.url))
        if after != before:
            print(f"spec_version {before} -> {after}")
            return 0 if after == TARGET_SPEC else 1
    print(f"spec_version still {before}: the upgrade did not apply")
    return 1


def cmd_verify(a):
    s = connect(a.url)
    ok = True
    v = spec_version(s)
    print(f"spec_version:   {v}")
    ok &= v == TARGET_SPEC
    adv = advancing(s)
    print(f"blocks:         {'advancing' if adv else 'STALLED'} (head {head_number(s)})")
    ok &= adv
    if a.before:
        before = json.load(open(a.before))["pallets"]
        gone = [p for p in before if p not in chain_pallets(s)]
        print(f"pallets:        {'none removed' if not gone else 'REMOVED: ' + ', '.join(gone)}")
        ok &= not gone
    max_subnets = s.query("Registry", "MaxSubnets").value
    unlocked = s.query("Registry", "Phase2Enabled").value
    print(f"registry:       MaxSubnets {max_subnets}, unlocked {unlocked}")
    ok &= max_subnets == 8 and unlocked is False
    res = s.rpc_request("state_call", ["SubnetRegistryApi_unlock_status", "0x"])
    raw = bytes.fromhex(res["result"][2:]) if "result" in res else b""
    if len(raw) >= 9:
        block = int.from_bytes(raw[1:5], "little")
        min_block = int.from_bytes(raw[5:9], "little")
        print(f"runtime API:    unlock_status answers (block {block:,}, day 180 = block {min_block:,})")
    else:
        print(f"runtime API:    NO ANSWER ({res.get('error')})")
        ok = False
    print("VERIFY OK" if ok else "VERIFY FAILED")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("preflight")
    p.add_argument("--url", required=True)
    p.add_argument("--source", required=True, help="checkout of the branch that will be built")
    p.add_argument("--key-file", required=True)
    p.add_argument("--save", help="write the chain's pallet list here, for verify")
    p = sub.add_parser("upgrade")
    p.add_argument("--url", required=True)
    p.add_argument("--key-file", required=True)
    p.add_argument("--wasm", required=True)
    p.add_argument("--yes", action="store_true", help="do not ask")
    p = sub.add_parser("verify")
    p.add_argument("--url", required=True)
    p.add_argument("--before", help="pallet list written by preflight --save")
    a = ap.parse_args()
    return {"preflight": cmd_preflight, "upgrade": cmd_upgrade, "verify": cmd_verify}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
