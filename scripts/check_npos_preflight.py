#!/usr/bin/env python3
"""Checks a running chain before the runtime 12 upgrade (open block production).

The migration pairs Aura and GRANDPA authorities by position and makes each pair a session
validator whose account is the Aura key. This prints the pairs, checks that the lists have the
same length, that every GRANDPA weight is 1, and that session is not yet in the runtime.

  python3 scripts/check_npos_preflight.py --url ws://127.0.0.1:9960

Read-only: it signs nothing. Needs: pip install substrate-interface
"""
import argparse
import sys

from substrateinterface import SubstrateInterface


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    a = ap.parse_args()
    s = SubstrateInterface(url=a.url)
    print(f"chain {s.chain}  spec {s.runtime_version}  block #{s.get_block_number(None)}")

    ok = True
    names = {m["name"] for m in s.get_metadata_modules()}
    if "Session" in names:
        print("Session is already in the runtime: the migration will do nothing.")
    aura = s.query("Aura", "Authorities").value
    grandpa = s.query("Grandpa", "Authorities").value
    print(f"\n{len(aura)} Aura and {len(grandpa)} GRANDPA authorities")
    if len(aura) != len(grandpa):
        print("STOP: the lists differ in length; the pairing by position would be wrong.")
        ok = False
    for i, (a_key, g) in enumerate(zip(aura, grandpa)):
        g_key, weight = (g[0], g[1]) if isinstance(g, (list, tuple)) else (g, 1)
        acct = s.ss58_encode(a_key)
        print(f"  {i}: validator {acct}\n     aura {a_key}\n     grandpa {g_key}  weight {weight}")
        if weight != 1:
            print("STOP: a GRANDPA weight is not 1.")
            ok = False
    print()
    if ok:
        print("OK: the migration will keep these keys producing and finalising blocks.")
        print("Check that every node above still holds its keys in its keystore before you upgrade.")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
