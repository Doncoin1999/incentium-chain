"""Bootstrap SN0 on mainnet with the launch operator's key (launch-period rules).

  ~/incentium-venv/bin/python scripts/mainnet_sn0.py --model-file ~/models/Qwen_Qwen3.5-2B-Q4_K_M.gguf

- Registers the operator's SN0 validator and miner hotkeys (free during the launch period).
- Funds them from the operator account (the validator's stake and fees; the miner's fees).
- Stakes the validator.
- Locks tier 0 to the reference weights file's SHA-256 (sudo), unless already locked.

Hotkeys are kept in the secrets folder as validator.json and miner.json (created if missing).
Safe to run again: it only does what is still missing. Prints a JSON line at the end.
"""
import argparse
import hashlib
import json
import os

UNIT = 10**9


def sha256_file(path):
    h = hashlib.sha256()
    with open(os.path.expanduser(path), "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load_or_create(path):
    from substrateinterface import Keypair

    if os.path.exists(path):
        return Keypair.create_from_mnemonic(json.load(open(path))["secretPhrase"])
    phrase = Keypair.generate_mnemonic()
    kp = Keypair.create_from_mnemonic(phrase)
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
        json.dump({"secretPhrase": phrase, "ss58Address": kp.ss58_address}, f)
    return kp


def hexbytes(v):
    if isinstance(v, str):
        return bytes.fromhex(v[2:]) if v.startswith("0x") else v.encode()
    return bytes(v)


def main():
    from substrateinterface import SubstrateInterface

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--secrets", default="~/incentium-mainnet/secrets")
    ap.add_argument("--validator-stake", type=int, default=20, help="ICT")
    ap.add_argument("--validator-fees", type=int, default=5, help="ICT for the validator's fees")
    ap.add_argument("--miner-fees", type=int, default=1, help="ICT for the miner's fees")
    ap.add_argument("--model-file", help="tier-0 weights file; locks its SHA-256 on chain")
    a = ap.parse_args()
    d = os.path.expanduser(a.secrets)
    s = SubstrateInterface(url=a.url)
    from substrateinterface import Keypair

    if not os.path.exists(f"{d}/operator.json"):
        raise SystemExit(f"missing {d}/operator.json (the launch operator key from mainnet-genesis.sh)")
    operator = Keypair.create_from_mnemonic(json.load(open(f"{d}/operator.json"))["secretPhrase"])
    validator = load_or_create(f"{d}/validator.json")
    miner = load_or_create(f"{d}/miner.json")

    def submit(kp, module, fn, params):
        call = s.compose_call(module, fn, params)
        r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
        if not r.is_success:
            raise SystemExit(f"{module}.{fn} failed: {r.error_message}")
        print(f"{module}.{fn} ok", flush=True)

    free = lambda kp: s.query("System", "Account", [kp.ss58_address]).value["data"]["free"]
    uid = lambda kp: s.query("Registry", "NeuronUid", [0, kp.ss58_address]).value

    if s.query("Sudo", "Key").value != operator.ss58_address:
        raise SystemExit("the operator key in the secrets folder is not this chain's sudo key")

    for kp in (validator, miner):
        if uid(kp) is None:
            submit(operator, "Registry", "register_neuron", {"netuid": 0, "hotkey": kp.ss58_address})
    staked = s.query("Consensus", "Stake", [validator.ss58_address]).value or 0
    for kp, want in ((validator, (0 if staked else a.validator_stake) + a.validator_fees), (miner, a.miner_fees)):
        if free(kp) < want * UNIT // 2:
            submit(operator, "Balances", "transfer_keep_alive", {"dest": kp.ss58_address, "value": want * UNIT})
    if not staked:
        submit(validator, "Consensus", "stake", {"amount": a.validator_stake * UNIT})

    if a.model_file:
        t = s.query("Registry", "Tiers", [0]).value
        digest = sha256_file(a.model_file)
        have = hexbytes(t["model_hash"]).hex()
        if have == digest:
            print("tier 0 already locked to this model file")
        else:
            inner = s.compose_call("Registry", "set_tier", {
                "tier": 0,
                "weight_ppm": t["weight_ppm"],
                "tolerance_ppm": t["tolerance_ppm"],
                "model": "0x" + hexbytes(t["model"]).hex(),
                "model_hash": "0x" + digest,
                "open": t["open"],
            })
            call = s.compose_call("Sudo", "sudo", {"call": inner})
            r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=operator), wait_for_inclusion=True)
            ok = r.is_success and any(
                e.value["event_id"] == "Sudid" and "Ok" in json.dumps(e.value["attributes"]) for e in r.triggered_events)
            if not ok:
                raise SystemExit(f"locking tier 0 failed: {r.error_message}")
            print(f"tier 0 locked to sha256 {digest}")

    print(json.dumps({
        "validator_uid": uid(validator), "miner_uid": uid(miner),
        "validator": validator.ss58_address, "miner": miner.ss58_address,
        "stake_ict": (s.query("Consensus", "Stake", [validator.ss58_address]).value or 0) / UNIT,
        "operator_ict": free(operator) / UNIT,
    }))


if __name__ == "__main__":
    main()
