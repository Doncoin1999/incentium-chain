"""Bootstrap SN0 on the testnet with the operator key (launch-period rules): register a
validator and a miner hotkey, fund them from the faucet account and stake the validator.

  python scripts/testnet_sn0.py [--url ws://127.0.0.1:9954] [--secrets ~/incentium-testnet/secrets]

Safe to run again: it only does what is still missing. Hotkeys are written to the secrets
folder as validator.json and miner.json; the neuron services read them with --key-file.
Prints a JSON line {"validator_uid": .., "miner_uid": .., ...} at the end.
"""
import argparse
import json
import os

UNIT = 10**9


def load_or_create(path):
    from substrateinterface import Keypair

    if os.path.exists(path):
        return Keypair.create_from_mnemonic(json.load(open(path))["secretPhrase"])
    phrase = Keypair.generate_mnemonic()
    kp = Keypair.create_from_mnemonic(phrase)
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
        json.dump({"secretPhrase": phrase, "ss58Address": kp.ss58_address}, f)
    return kp


def main():
    from substrateinterface import SubstrateInterface

    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9954")
    ap.add_argument("--secrets", default="~/incentium-testnet/secrets")
    ap.add_argument("--validator-stake", type=int, default=20, help="ICT")
    a = ap.parse_args()
    d = os.path.expanduser(a.secrets)
    s = SubstrateInterface(url=a.url)
    operator = load_or_create(f"{d}/operator.json")
    faucet = load_or_create(f"{d}/faucet.json")
    validator = load_or_create(f"{d}/validator.json")
    miner = load_or_create(f"{d}/miner.json")

    def submit(kp, module, fn, params):
        call = s.compose_call(module, fn, params)
        r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
        if not r.is_success:
            raise SystemExit(f"{module}.{fn} failed: {r.error_message}")
        print(f"{module}.{fn} ok")

    free = lambda kp: s.query("System", "Account", [kp.ss58_address]).value["data"]["free"]
    uid = lambda kp: s.query("Registry", "NeuronUid", [0, kp.ss58_address]).value

    for name, kp in (("validator", validator), ("miner", miner)):
        if uid(kp) is None:
            submit(operator, "Registry", "register_neuron", {"netuid": 0, "hotkey": kp.ss58_address})
    for kp, want in ((validator, a.validator_stake + 30), (miner, 10)):
        if free(kp) < want * UNIT // 2:
            submit(faucet, "Balances", "transfer_keep_alive", {"dest": kp.ss58_address, "value": want * UNIT})
    if not s.query("Consensus", "Stake", [validator.ss58_address]).value:
        submit(validator, "Consensus", "stake", {"amount": a.validator_stake * UNIT})

    print(json.dumps({"validator_uid": uid(validator), "miner_uid": uid(miner),
                      "validator": validator.ss58_address, "miner": miner.ss58_address,
                      "stake_ict": s.query("Consensus", "Stake", [validator.ss58_address]).value / UNIT}))


if __name__ == "__main__":
    main()
