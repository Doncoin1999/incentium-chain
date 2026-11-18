#!/usr/bin/env python3
"""Checks for the launch rehearsal (run by scripts/launch-rehearsal.sh).

Before the end of the launch period: only the operator can register neurons; sudo works.
At the end (block = BootstrapUntil): the runtime emits Launch.LaunchEnded, removes sudo and
creates the planned fund bounties, approved with the curator.
After the end: anyone can register a neuron by paying the fee; sudo and end_bootstrap are gone.

Prints PASS/FAIL per check and exits 1 if anything failed.
"""
import argparse
import json
import os
import sys
import time

UNIT = 10**9
FUND = "5EYCAe5hKNwfLdpcWpRqPdDJPyknPxmPQVAe3tk47Ban17nZ"  # PalletId ict/fund
failures = []


def check(name, ok, detail=""):
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  ({detail})" if detail else ""), flush=True)
    if not ok:
        failures.append(name)


def main() -> int:
    from substrateinterface import Keypair, SubstrateInterface

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9970")
    ap.add_argument("--operator", required=True, help="operator key JSON (secretPhrase)")
    a = ap.parse_args()
    op = Keypair.create_from_mnemonic(json.load(open(os.path.expanduser(a.operator)))["secretPhrase"])

    s = None
    for _ in range(60):  # wait for the node's RPC
        try:
            s = SubstrateInterface(url=a.url)
            break
        except Exception:
            time.sleep(2)
    if s is None:
        print("node RPC did not come up")
        return 1

    def connect():
        return SubstrateInterface(url=a.url)

    def head(c):
        return c.get_block_number(c.get_chain_head())

    def submit(kp, module, fn, params):
        c = connect()
        call = c.compose_call(module, fn, params)
        r = c.submit_extrinsic(c.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
        if r.is_success:
            return True, ""
        err = r.error_message or {}
        return False, err.get("name", str(err)) if isinstance(err, dict) else str(err)

    def sudo_ok(c):
        """True if the sudo call itself went through and the inner call succeeded."""
        call = c.compose_call("Sudo", "sudo", {"call": c.compose_call("System", "remark", {"remark": "0x01"})})
        r = c.submit_extrinsic(c.create_signed_extrinsic(call=call, keypair=op), wait_for_inclusion=True)
        if not r.is_success:
            return False, (r.error_message or {}).get("name", "")
        for e in r.triggered_events:
            v = e.value
            if v["module_id"] == "Sudo" and v["event_id"] == "Sudid":
                res = v["attributes"].get("sudo_result") if isinstance(v["attributes"], dict) else v["attributes"]
                return "Ok" in json.dumps(res), json.dumps(res)
        return False, "no Sudid event"

    until = s.query("Registry", "BootstrapUntil").value
    print(f"launch period ends at block {until}; now at block {head(s)}")
    check("sudo key is the operator", s.query("Sudo", "Key").value == op.ss58_address)
    check("curator set", s.query("Launch", "Curator").value is not None, str(s.query("Launch", "Curator").value))
    planned = s.query("Launch", "PlannedBounties").value or []
    check("two fund bounties planned", len(planned) == 2, str([(p[0], p[1] / UNIT) for p in planned]))

    # Before the end: only the operator registers.
    outsider = Keypair.create_from_mnemonic(Keypair.generate_mnemonic())
    ok, _ = submit(op, "Balances", "transfer_keep_alive", {"dest": outsider.ss58_address, "value": 5 * UNIT})
    check("operator funds an outside account", ok)
    ok, err = submit(outsider, "Registry", "register_neuron", {"netuid": 0, "hotkey": outsider.ss58_address})
    check("outsider cannot register in the launch period", not ok and err == "BootstrapRestricted", err)
    hot = Keypair.create_from_mnemonic(Keypair.generate_mnemonic())
    ok, err = submit(op, "Registry", "register_neuron", {"netuid": 0, "hotkey": hot.ss58_address})
    check("operator registers a neuron for free", ok, err)
    ok, detail = sudo_ok(connect())
    check("sudo works in the launch period", ok, detail)
    if head(connect()) >= until:
        print("the launch period ended before the checks finished; run again with more blocks")
        return 1

    # Wait for the end.
    while True:
        n = head(connect())
        if n >= until + 2:
            break
        print(f"  block {n} / {until}", flush=True)
        time.sleep(30 if until - n > 10 else 6)

    c = connect()
    h = c.get_block_hash(until)
    ended = [e.value for e in c.get_events(h)
             if e.value["module_id"] == "Launch" and e.value["event_id"] == "LaunchEnded"]
    failed = [e.value for e in c.get_events(h)
              if e.value["module_id"] == "Launch" and e.value["event_id"] == "BountyFailed"]
    check("LaunchEnded emitted at the last block", len(ended) == 1, json.dumps(ended))
    check("no BountyFailed events", not failed, json.dumps(failed))
    check("Launch.Done is set", c.query("Launch", "Done").value is True)
    check("sudo key removed", c.query("Sudo", "Key").value is None)
    count = c.query("Bounties", "BountyCount").value
    check("two bounties created", count == 2, f"BountyCount={count}")
    for i in range(count or 0):
        b = c.query("Bounties", "Bounties", [i]).value
        desc = c.query("Bounties", "BountyDescriptions", [i]).value
        print(f"      bounty {i}: {b['value'] / UNIT:,.0f} ICT, status {json.dumps(b['status'])}, {desc}")
        check(f"bounty {i} approved with the curator",
              any(k in json.dumps(b["status"]) for k in ("ApprovedWithCurator", "Funded", "CuratorProposed")))
    fund = c.query("System", "Account", [FUND]).value["data"]
    print(f"      fund account: free {fund['free'] / UNIT:,.3f} ICT, reserved {fund['reserved'] / UNIT:,.3f} ICT")

    # After the end.
    ok, detail = sudo_ok(c)
    check("sudo no longer works", not ok, detail)
    ok, err = submit(op, "Registry", "end_bootstrap", {})
    check("end_bootstrap refused after the end", not ok, err)
    ok, err = submit(outsider, "Registry", "register_neuron", {"netuid": 0, "hotkey": outsider.ss58_address})
    check("outsider can register after the end, paying the fee", ok, err)
    n1 = head(connect())
    time.sleep(40)
    n2 = head(connect())
    check("blocks still produced after the end", n2 > n1, f"{n1} -> {n2}")

    print()
    print("ALL CHECKS PASSED" if not failures else f"{len(failures)} CHECK(S) FAILED: {failures}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
