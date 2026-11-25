#!/usr/bin/env python3
"""Incentium governance from the command line: runtime upgrades and other decisions by
referendum, so nothing needs sudo after the launch period (day 90).

A runtime upgrade is decided by hash, not by uploading the code:

  1. propose-upgrade: a referendum on `authorize_upgrade(blake2_256(wasm))`. The call is small
     enough to sit inside the referendum (no preimage, no large deposit). Root track (weeks) or
     Emergency track (hours, bug fixes only).
  2. vote: ICT holders vote; when it passes and enacts, the chain stores the authorized hash.
  3. apply-upgrade: anyone uploads the wasm (`system.apply_authorized_upgrade`). The chain takes
     it only if the hash matches, the spec name is the same and the spec version is higher, and
     charges no fee for a valid upgrade.

Commands (every command also takes --url, default ws://127.0.0.1:9944):

  status [INDEX]                                    open referenda, or one in detail
  propose-upgrade --wasm F --track root|emergency --key-file K [--source REPO] [--no-deposit]
  propose --call Pallet.function --args JSON --track root|treasurer|small_spender --key-file K
  deposit INDEX --key-file K                        place the decision deposit
  vote INDEX aye|nay --amount ICT [--conviction 0-6] --key-file K
  apply-upgrade --wasm F --key-file K               after the referendum has enacted
  hash --wasm F                                     the code hash, to check by hand

--source REPO compares the pallets the chain has with those in REPO/runtime/src/lib.rs and stops
if the upgrade would remove one. --yes skips the "type yes" question.

Needs: pip install substrate-interface
"""
import argparse
import hashlib
import json
import os
import sys
import time

UNIT = 10**9
TRACKS = {"root": 0, "treasurer": 1, "small_spender": 2, "emergency": 3}
# Origins of the tracks, as the runtime's OriginCaller names them.
ORIGINS = {
    "root": {"system": "Root"},
    "treasurer": {"Origins": "Treasurer"},
    "small_spender": {"Origins": "SmallSpender"},
    "emergency": {"Origins": "Emergency"},
}
INLINE_MAX = 128  # bytes a call may have to sit inside a referendum


def connect(url):
    from substrateinterface import SubstrateInterface
    return SubstrateInterface(url=url)


def keypair(path):
    from substrateinterface import Keypair
    text = open(os.path.expanduser(path)).read().strip()
    return Keypair.create_from_uri(json.loads(text)["secretPhrase"] if text.startswith("{") else text)


def code_hash(path):
    return "0x" + hashlib.blake2b(open(path, "rb").read(), digest_size=32).hexdigest()


def confirm(a, text):
    print(text)
    if a.yes:
        return True
    return input('Type "yes" to go ahead: ').strip() == "yes"


def submit(s, kp, call):
    r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
    if not r.is_success:
        raise SystemExit(f"FAILED in block {r.block_hash}: {r.error_message}")
    print(f"included in block {r.block_hash}")
    return r


def event(r, module, name):
    for e in r.triggered_events:
        v = e.value
        if v["module_id"] == module and v["event_id"] == name:
            return v["attributes"]
    return None


def head(s):
    return s.get_block_header()["header"]["number"]


def ict(x):
    return f"{int(x) / UNIT:,.2f} ICT"


def blocks_to_text(n):
    h = n * 12 / 3600
    return f"{h:.1f} h" if h < 48 else f"{h / 24:.1f} days"


def check_pallets(s, repo):
    import re
    text = open(os.path.join(repo, "runtime", "src", "lib.rs")).read()
    will = set(re.findall(r"#\[runtime::pallet_index\(\d+\)\]\s*pub type (\w+)", text))
    s.init_runtime()
    have = {p.name for p in s.metadata.pallets}
    gone = sorted(have - will)
    if gone:
        raise SystemExit(f"STOP: the upgrade would remove {', '.join(gone)}. Wrong branch for this chain?")
    new = sorted(will - have)
    print(f"pallets: none removed{'; new: ' + ', '.join(new) if new else ''}")


def make_proposal(s, kp, a, call):
    """Inline if small, else note the preimage first and refer to it."""
    data = bytes.fromhex(call.data.to_hex()[2:])
    if len(data) <= INLINE_MAX:
        return {"Inline": "0x" + data.hex()}
    h = "0x" + hashlib.blake2b(data, digest_size=32).hexdigest()
    print(f"the call is {len(data):,} bytes: noting it as a preimage first (a deposit is held)")
    submit(s, kp, s.compose_call("Preimage", "note_preimage", {"bytes": "0x" + data.hex()}))
    return {"Lookup": {"hash": h, "len": len(data)}}


def open_referendum(s, kp, a, call, what):
    track = a.track
    deposit = s.get_constant("Referenda", "SubmissionDeposit").value
    try:
        tr = [t for t in s.get_constant("Referenda", "Tracks").value if t[0] == TRACKS[track]]
        info = tr[0][1] if tr else {}
    except Exception:  # display only
        info = {}
    text = (f"Open a referendum on the {track} track: {what}\n"
            f"submission deposit {ict(deposit)}"
            + (f", decision deposit {ict(info.get('decision_deposit', 0))}" if not a.no_deposit else "")
            + f"\nsigned by {kp.ss58_address}")
    if not confirm(a, text):
        print("stopped, nothing sent")
        return 1
    proposal = make_proposal(s, kp, a, call)
    r = submit(s, kp, s.compose_call("Referenda", "submit", {
        "proposal_origin": ORIGINS[track],
        "proposal": proposal,
        "enactment_moment": {"After": 0},
    }))
    ev = event(r, "Referenda", "Submitted")
    index = ev["index"] if ev else None
    print(f"referendum #{index} submitted")
    if index is not None and not a.no_deposit:
        submit(s, kp, s.compose_call("Referenda", "place_decision_deposit", {"index": index}))
        print(f"decision deposit placed for #{index}")
    print(f"Next: vote with  python3 {sys.argv[0]} vote {index} aye --amount <ICT> --key-file <file> --url {a.url}")
    return 0


def cmd_hash(a):
    print(code_hash(a.wasm))
    return 0


def cmd_propose_upgrade(a):
    s = connect(a.url)
    kp = keypair(a.key_file)
    h = code_hash(a.wasm)
    size = os.path.getsize(a.wasm)
    if a.source:
        check_pallets(s, a.source)
    if a.track == "root":
        call = s.compose_call("System", "authorize_upgrade", {"code_hash": h})
    elif a.track == "emergency":
        call = s.compose_call("Emergency", "authorize_upgrade", {"code_hash": h})
    else:
        raise SystemExit("an upgrade goes on the root or the emergency track")
    v = s.rpc_request("state_getRuntimeVersion", [])["result"]["specVersion"]
    return open_referendum(s, kp, a, call,
                           f"authorize runtime {h} ({size:,} bytes); the chain is at spec {v} now")


def cmd_propose(a):
    s = connect(a.url)
    kp = keypair(a.key_file)
    module, function = a.call.split(".", 1)
    call = s.compose_call(module, function, json.loads(a.args))
    return open_referendum(s, kp, a, call, f"{a.call}({a.args})")


def cmd_deposit(a):
    s = connect(a.url)
    submit(s, keypair(a.key_file), s.compose_call("Referenda", "place_decision_deposit", {"index": a.index}))
    return 0


def cmd_vote(a):
    s = connect(a.url)
    kp = keypair(a.key_file)
    aye = a.choice == "aye"
    vote_byte = (0x80 if aye else 0) | a.conviction
    amount = int(round(a.amount * UNIT))
    lock = {0: "no lock, 0.1x", 1: "7 days, 1x", 2: "14 days, 2x", 3: "28 days, 3x", 4: "56 days, 4x",
            5: "112 days, 5x", 6: "224 days, 6x"}[a.conviction]
    if not confirm(a, f"Vote {a.choice.upper()} on #{a.index} with {ict(amount)} (conviction {a.conviction}: {lock})"):
        print("stopped, nothing sent")
        return 1
    try:
        call = s.compose_call("ConvictionVoting", "vote", {
            "poll_index": a.index,
            "vote": {"Standard": {"vote": vote_byte, "balance": amount}},
        })
    except Exception:  # `Vote` is a one-field composite around the byte in some metadata
        call = s.compose_call("ConvictionVoting", "vote", {
            "poll_index": a.index,
            "vote": {"Standard": {"vote": [vote_byte], "balance": amount}},
        })
    submit(s, kp, call)
    return 0


def describe(s, index, info, now):
    if "Ongoing" not in info:
        kind = next(iter(info))
        return f"#{index}: {kind}"
    o = info["Ongoing"]
    track = {v: k for k, v in TRACKS.items()}.get(o["track"], o["track"])
    t = o["tally"]
    total = t["ayes"] + t["nays"]
    approval = t["ayes"] / total * 100 if total else 0
    lines = [f"#{index} on the {track} track, submitted at block {o['submitted']:,}"]
    lines.append(f"   aye {ict(t['ayes'])}, nay {ict(t['nays'])} (approval {approval:.1f} %), support {ict(t['support'])}")
    if not o.get("decision_deposit"):
        lines.append("   waiting for the decision deposit")
    d = o.get("deciding")
    if d:
        lines.append(f"   deciding since block {d['since']:,} ({blocks_to_text(now - d['since'])} ago)")
        if d.get("confirming"):
            lines.append(f"   CONFIRMING: passes at block {d['confirming']:,} (in {blocks_to_text(d['confirming'] - now)}) if it holds")
    else:
        lines.append("   preparing (not yet deciding)")
    return "\n".join(lines)


def cmd_status(a):
    s = connect(a.url)
    now = head(s)
    v = s.rpc_request("state_getRuntimeVersion", [])["result"]["specVersion"]
    auth = s.query("System", "AuthorizedUpgrade").value
    print(f"block {now:,}, spec {v}" + (f", authorized upgrade waiting: {auth['code_hash']}" if auth else ""))
    if a.index is not None:
        info = s.query("Referenda", "ReferendumInfoFor", [a.index]).value
        print(describe(s, a.index, info, now) if info else f"#{a.index}: not found")
        return 0
    count = s.query("Referenda", "ReferendumCount").value
    shown = 0
    for i in range(max(0, count - 50), count):
        info = s.query("Referenda", "ReferendumInfoFor", [i]).value
        if info and "Ongoing" in info:
            print(describe(s, i, info, now))
            shown += 1
    print(f"{shown} open of {count} referenda in total")
    return 0


def cmd_apply_upgrade(a):
    s = connect(a.url)
    kp = keypair(a.key_file)
    h = code_hash(a.wasm)
    auth = s.query("System", "AuthorizedUpgrade").value
    if not auth:
        raise SystemExit("no upgrade is authorized on chain (has the referendum enacted?)")
    if auth["code_hash"] != h:
        raise SystemExit(f"STOP: the chain authorized {auth['code_hash']}, but this wasm is {h}")
    if a.source:
        check_pallets(s, a.source)
    before = s.rpc_request("state_getRuntimeVersion", [])["result"]["specVersion"]
    code = open(a.wasm, "rb").read()
    if not confirm(a, f"Upload the authorized runtime {h} ({len(code):,} bytes); the chain is at spec {before}."):
        print("stopped, nothing sent")
        return 1
    r = submit(s, kp, s.compose_call("System", "apply_authorized_upgrade", {"code": "0x" + code.hex()}))
    if event(r, "System", "RejectedInvalidAuthorizedUpgrade"):
        raise SystemExit("REJECTED: same or lower spec_version, or another spec name")
    for _ in range(20):
        time.sleep(6)
        after = connect(a.url).rpc_request("state_getRuntimeVersion", [])["result"]["specVersion"]
        if after != before:
            print(f"OK: spec_version {before} -> {after}")
            return 0
    print(f"spec_version still {before}")
    return 1


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--url", default="ws://127.0.0.1:9944")
    common.add_argument("--yes", action="store_true", help="do not ask before sending")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("status", parents=[common])
    p.add_argument("index", nargs="?", type=int)
    p = sub.add_parser("hash", parents=[common])
    p.add_argument("--wasm", required=True)
    p = sub.add_parser("propose-upgrade", parents=[common])
    p.add_argument("--wasm", required=True)
    p.add_argument("--track", choices=["root", "emergency"], required=True)
    p.add_argument("--key-file", required=True)
    p.add_argument("--source", help="branch checkout to compare pallets with")
    p.add_argument("--no-deposit", action="store_true", help="do not place the decision deposit")
    p = sub.add_parser("propose", parents=[common])
    p.add_argument("--call", required=True, help="Pallet.function, e.g. Registry.attest_swap")
    p.add_argument("--args", default="{}", help='JSON, e.g. \'{"contract": "0x..", "audit_hash": "0x.."}\'')
    p.add_argument("--track", choices=["root", "treasurer", "small_spender"], required=True)
    p.add_argument("--key-file", required=True)
    p.add_argument("--no-deposit", action="store_true")
    p = sub.add_parser("deposit", parents=[common])
    p.add_argument("index", type=int)
    p.add_argument("--key-file", required=True)
    p = sub.add_parser("vote", parents=[common])
    p.add_argument("index", type=int)
    p.add_argument("choice", choices=["aye", "nay"])
    p.add_argument("--amount", type=float, required=True, help="ICT")
    p.add_argument("--conviction", type=int, default=1, choices=range(0, 7))
    p.add_argument("--key-file", required=True)
    p = sub.add_parser("apply-upgrade", parents=[common])
    p.add_argument("--wasm", required=True)
    p.add_argument("--key-file", required=True)
    p.add_argument("--source", help="branch checkout to compare pallets with")

    a = ap.parse_args()
    return {
        "status": cmd_status, "hash": cmd_hash, "propose-upgrade": cmd_propose_upgrade,
        "propose": cmd_propose, "deposit": cmd_deposit, "vote": cmd_vote,
        "apply-upgrade": cmd_apply_upgrade,
    }[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
