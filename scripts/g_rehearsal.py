"""Testnet rehearsal of runtime 13 (work item G): the SN0 open model list, the endpoint register
with probes and the daily payout, and swap v2's runtime side.

  python scripts/g_rehearsal.py status
  python scripts/g_rehearsal.py upgrade --wasm F   # sudo: install runtime 13 now (testnet only)
  python scripts/g_rehearsal.py validators         # three SN0 validators with 1,000 ICT stake each
  python scripts/g_rehearsal.py models             # propose, declare x3, open, miner declares, retire, close
  python scripts/g_rehearsal.py endpoints          # register the testnet RPC, switch on the 5 % share
  python scripts/g_rehearsal.py probe              # runs until stopped: probes every 30 min, reports daily
  python scripts/g_rehearsal.py check              # yesterday's reports, scores and payouts
  python scripts/g_rehearsal.py cleanup            # unstake the three validators

Options: --url (default ws://127.0.0.1:9960, the testnet node on the test server),
--secrets (default ~/incentium-testnet/secrets), --rpc-url (the public endpoint to register,
default wss://testnet-rpc.incentium.org), --archive-url (an archive endpoint to register too).

Keys for the rehearsal are written to <secrets>/g/ (mode 600). Every step can be run again and
only does what is still missing. Like sn1_rehearsal.py it refuses any chain whose name does not
contain "testnet": the sudo shortcuts here must never touch mainnet.

Endpoint scores count only when reporters holding more than half of all reporter stake (block
producers plus SN0 validators) report. The block producers' stash keys are not on the server,
so `check` shows how much of that stake reported; add producer stash key files with
--reporter-key to report with them as well.
"""
import argparse
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "neuron"))

import sn1_rehearsal as base  # noqa: E402
from incentium_neuron.validator import u8_list  # noqa: E402

UNIT = base.UNIT
SN0 = 0
STAKE = 1_000 * UNIT
REHEARSAL_HASH = "0x" + hashlib.sha256(b"incentium runtime 13 rehearsal model").hexdigest()


class Chain(base.Chain):
    def __init__(self, a):
        super().__init__(a)
        self.vals = {n: base.load_or_create(f"{self.d}/g/{n}.json") for n in ("val1", "val2", "val3")}
        self.miner = base.load_or_create(f"{self.d}/g/miner.json")

    def sudo_ok(self):
        if self.q("Sudo", "Key") != self.operator.ss58_address:
            sys.exit("the operator key is not the testnet's sudo key")

    def ensure_neuron(self, kp):
        if self.q("Registry", "NeuronUid", [SN0, kp.ss58_address]) is None:
            self.submit(self.operator, "Registry", "register_neuron", {"netuid": SN0, "hotkey": kp.ss58_address})
        return self.q("Registry", "NeuronUid", [SN0, kp.ss58_address])

    def models(self):
        out = {}
        for k, v in self.s.query_map("Registry", "Models"):
            out[int(k.value)] = v.value
        return out


def status_of(m):
    s = m["status"]
    return s if isinstance(s, str) else next(iter(s))


def cmd_status(c, a):
    print(f"block {c.head():,}; runtime {c.s.runtime_version}; sudo {'operator' if c.q('Sudo', 'Key') == c.operator.ss58_address else 'other'}")
    for p in ("Endpoints",):
        print(f"{p} pallet: {'yes' if c.has(p) else 'NO - needs runtime 13'}")
    if c.s.get_metadata_storage_function("Registry", "Models") is None:
        return print("model list: NO - needs runtime 13")
    print("model list:")
    for i, m in sorted(c.models().items()):
        holders = sum(1 for k, v in c.s.query_map("Registry", "ValidatorModels") if i in u8_list(v.value))
        print(f"  {i}: {base_name(m):22} {status_of(m):9} G {m['g_ppm'] / 1e6:<5} "
              f"memory {m['mem_mb']} MB  hash {'pinned' if str(m['weights_hash']).strip('0x') else 'not pinned'}  "
              f"declared by {holders}  deposit {base.ict(m['deposit'])}")
    for n, kp in {**c.vals, "miner": c.miner}.items():
        uid = c.q("Registry", "NeuronUid", [SN0, kp.ss58_address])
        stake = c.q("Consensus", "Stake", [kp.ss58_address]) or 0
        print(f"  {n:<5} {kp.ss58_address}  uid {uid}  stake {base.ict(stake)}  "
              f"model {c.q('Registry', 'NeuronModel', [SN0, kp.ss58_address])}  "
              f"declares {u8_list(c.q('Registry', 'ValidatorModels', [kp.ss58_address]))}")
    if c.has("Endpoints"):
        print(f"endpoint share: {c.q('Emission', 'EndpointShare') / 1e4:.1f} %; "
              f"last settled day {c.q('Endpoints', 'LastSettled')}; today {c.head() // 7200}")
        for k, v in c.s.query_map("Endpoints", "Entries"):
            e = v.value
            print(f"  endpoint {k.value}: {e['kind']} {base_name({'name': e['url']})} active {e['active']} "
                  f"last score {e['last_score']} failing days {e['failing_days']}")
    print(f"swap v2 (Swap.Terms storage): {'yes' if c.s.get_metadata_storage_function('Swap', 'Terms') else 'no'}")


def base_name(m):
    from incentium_neuron.validator import decode_bytes

    return decode_bytes(m["name"])


def cmd_upgrade(c, a):
    base.cmd_upgrade(c, a)


def cmd_validators(c, a):
    for n, kp in c.vals.items():
        need = STAKE + 50 * UNIT
        have = c.free(kp.ss58_address)
        staked = c.q("Consensus", "Stake", [kp.ss58_address]) or 0
        if staked < STAKE and have < need:
            c.fund(kp.ss58_address, need - have)
        uid = c.ensure_neuron(kp)
        if staked < STAKE:
            c.submit(kp, "Consensus", "stake", {"amount": STAKE - staked})
        print(f"{n}: uid {uid}, stake {base.ict(c.q('Consensus', 'Stake', [kp.ss58_address]))} ICT")
    if c.free(c.miner.ss58_address) < 5 * UNIT:
        c.fund(c.miner.ss58_address, 10 * UNIT)
    print(f"miner: uid {c.ensure_neuron(c.miner)}")


def cmd_models(c, a):
    c.sudo_ok()
    models = c.models()
    mine = [i for i, m in models.items() if base_name(m) == "rehearsal-4B" and status_of(m) != "Closed"]
    if mine:
        mid = mine[0]
    else:
        if c.free(c.operator.ss58_address) < 200 * UNIT:
            c.fund(c.operator.ss58_address, 200 * UNIT)
        before = c.free(c.operator.ss58_address)
        c.submit(c.operator, "Registry", "propose_model", {
            "name": "rehearsal-4B", "weights_hash": REHEARSAL_HASH, "engine": "llama.cpp",
            "mem_mb": 3300, "g_ppm": 200_000, "tolerance_ppm": 350_000})
        mid = max(c.models())
        print(f"proposed model {mid}; deposit taken: {base.ict(before - c.free(c.operator.ss58_address))} ICT")
        # Withdraw path: a second proposal is withdrawn at once.
        c.submit(c.operator, "Registry", "propose_model", {
            "name": "rehearsal-withdrawn", "weights_hash": REHEARSAL_HASH, "engine": "llama.cpp",
            "mem_mb": 1000, "g_ppm": 50_000, "tolerance_ppm": 350_000})
        c.submit(c.operator, "Registry", "withdraw_model", {"model": max(c.models())})
    m = c.models()[mid]
    if status_of(m) == "Proposed":
        # Opening must fail before three validators declare it.
        call = c.s.compose_call("Registry", "set_model", {"model": mid, "g_ppm": 200_000, "tolerance_ppm": 350_000,
                                                          "status": "Open", "weights_hash": None})
        r = c.s.submit_extrinsic(c.s.create_signed_extrinsic(
            call=c.s.compose_call("Sudo", "sudo", {"call": call}), keypair=c.operator), wait_for_inclusion=True)
        # Under sudo the error sits in the Sudid event as a module error index, not by name.
        failed = any(e.value["event_id"] == "Sudid" and "Err" in str(e.value["attributes"])
                     for e in r.triggered_events)
        print(f"opening with fewer than three declarations refused: {failed}")
        for n, kp in c.vals.items():
            have = u8_list(c.q("Registry", "ValidatorModels", [kp.ss58_address]))
            if mid not in have:
                c.submit(kp, "Registry", "declare_validator_models", {"models": sorted(set(have) | {0, mid})})
        before = c.free(c.operator.ss58_address)
        c.submit(c.operator, "Registry", "set_model", {"model": mid, "g_ppm": 200_000, "tolerance_ppm": 350_000,
                                                       "status": "Open", "weights_hash": None}, sudo=True)
        print(f"model {mid} open; deposit returned: {base.ict(c.free(c.operator.ss58_address) - before)} ICT")
    if status_of(c.models()[mid]) == "Open":
        c.submit(c.miner, "Registry", "declare_model", {"netuid": SN0, "model": mid})
        print(f"miner declares model {c.q('Registry', 'NeuronModel', [SN0, c.miner.ss58_address])}")
        c.submit(c.operator, "Registry", "set_model", {"model": mid, "g_ppm": 200_000, "tolerance_ppm": 350_000,
                                                       "status": "Retiring", "weights_hash": None}, sudo=True)
    if status_of(c.models()[mid]) == "Retiring":
        # The rehearsal model has no real weights file: close it, and move the miner back to 0.
        c.submit(c.operator, "Registry", "set_model", {"model": mid, "g_ppm": 200_000, "tolerance_ppm": 350_000,
                                                       "status": "Closed", "weights_hash": None}, sudo=True)
        c.submit(c.miner, "Registry", "declare_model", {"netuid": SN0, "model": 0})
    print(f"model {mid}: {status_of(c.models()[mid])}")


def cmd_endpoints(c, a):
    c.sudo_ok()
    have = {base_name({"name": v.value["url"]}) for k, v in c.s.query_map("Endpoints", "Entries")}
    wanted = [("Rpc", a.rpc_url)] + ([("Archive", a.archive_url)] if a.archive_url else [])
    for kind, url in wanted:
        if url in have:
            continue
        if c.free(c.operator.ss58_address) < 200 * UNIT:
            c.fund(c.operator.ss58_address, 200 * UNIT)
        c.submit(c.operator, "Endpoints", "register", {"kind": kind, "url": url, "region": "eu-central"})
    if c.q("Emission", "EndpointShare") == 0:
        c.submit(c.operator, "Emission", "set_endpoint_share", {"ppm": 50_000}, sudo=True)
    cmd_status(c, a)


def reporters(c, a):
    from substrateinterface import Keypair

    keys = list(c.vals.values())
    for f in a.reporter_key or []:
        d = json.load(open(os.path.expanduser(f)))
        keys.append(Keypair.create_from_mnemonic(d["secretPhrase"]))
    return keys


def cmd_probe(c, a):
    """Each reporter gets its own chain connection and state file; one probe round serves all."""
    from incentium_neuron import probes
    from incentium_neuron.validator import SubstrateChain

    provers = []
    for kp in reporters(c, a):
        ch = probes.attach(SubstrateChain(a.url, kp))
        st = probes.ProbeState.load(f"{c.d}/g/probes-{kp.ss58_address[:8]}.json")
        provers.append(probes.Prober(ch, st, log=lambda m, k=kp.ss58_address[:8]: print(f"[{k}] {m}", flush=True)))
    while True:
        try:
            first = provers[0]
            first.report_due()
            n = first.chain.block_number()
            day = n // probes.BLOCKS_PER_DAY
            ref = first.chain.reference()
            results = {ep.id: probes.probe(ep, ref) for ep in first.chain.endpoints() if ep.kind != "Bootnode"}
            for p in provers:
                p.report_due()
                for i, ok in results.items():
                    p.state.record(day, i, ok)
                p.state.save()
            print(f"day {day} block {n}: {results}", flush=True)
        except Exception as e:
            print(f"probe round failed: {e!r}", flush=True)
        time.sleep(probes.PROBE_EVERY)


def cmd_check(c, a):
    today = c.head() // 7200
    day = today - 1
    print(f"today is day {today}; reports for day {day}:")
    for k, v in c.s.query_map("Endpoints", "Tallies", [day]):
        t = v.value
        print(f"  endpoint {k.value}: {len(t['reports'])} reports, reporter stake {base.ict(t['stake'])} ICT, "
              f"scores {[r[0] for r in t['reports']]}")
    sn0 = c.q("Consensus", "TotalStake") or 0
    producers = c.q("Session", "Validators") or []
    print(f"SN0 stake {base.ict(sn0)} ICT; {len(producers)} block producers (their bonded stake also counts "
          f"towards the total, so the scores count only if more than half of all of it reported)")
    print(f"last settled day: {c.q('Endpoints', 'LastSettled')} (day d is settled at the start of day d + 2)")
    pot = c.q("Emission", "EndpointShare")
    print(f"endpoint share {pot / 1e4:.1f} %")
    for k, v in c.s.query_map("Endpoints", "Entries"):
        e = v.value
        print(f"  endpoint {k.value}: last score {e['last_score']}, owner balance {base.ict(c.free(e['owner']))} ICT")


def cmd_cleanup(c, a):
    for n, kp in c.vals.items():
        staked = c.q("Consensus", "Stake", [kp.ss58_address]) or 0
        if staked:
            c.submit(kp, "Consensus", "unstake", {"amount": staked})
    print("unstaked; withdraw after the unbonding period with Consensus.withdraw_unstaked")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cmd", choices=["status", "upgrade", "validators", "models", "endpoints", "probe", "check", "cleanup"])
    ap.add_argument("--wasm")
    ap.add_argument("--url", default="ws://127.0.0.1:9960")
    ap.add_argument("--secrets", default="~/incentium-testnet/secrets")
    ap.add_argument("--rpc-url", default="wss://testnet-rpc.incentium.org")
    ap.add_argument("--archive-url")
    ap.add_argument("--reporter-key", action="append", help="extra reporter key file (JSON with secretPhrase)")
    a = ap.parse_args()
    c = Chain(a)
    globals()["cmd_" + a.cmd.replace("-", "_")](c, a)


if __name__ == "__main__":
    main()
