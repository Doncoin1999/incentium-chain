"""Bootstrap SN0 on a fresh devnet: register neurons, stake validators, set miner
endpoints and register a pool. Uses the well-known dev accounts.

pip install substrate-interface
python3 scripts/bootstrap_sn0.py --url ws://127.0.0.1:9944 --miner-endpoint http://127.0.0.1:8091

Roles: Alice, Bob = validators (stake 10,000 IPM each); Charlie = solo miner;
Dave = pool (bond 5,000 IPM, 2 % fee); Eve and Ferdie = pool sub-workers.
"""
import argparse

from substrateinterface import Keypair, SubstrateInterface

UNIT = 10**9


def submit(s, kp, module, fn, params):
    call = s.compose_call(call_module=module, call_function=fn, call_params=params)
    r = s.submit_extrinsic(s.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
    status = "ok" if r.is_success else f"FAILED: {r.error_message}"
    print(f"{kp.ss58_address[:8]}… {module}.{fn}: {status}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://127.0.0.1:9944")
    ap.add_argument("--miner-endpoint", default="http://127.0.0.1:8091")
    ap.add_argument("--pool-endpoint", default="http://127.0.0.1:8092")
    a = ap.parse_args()
    s = SubstrateInterface(url=a.url)
    k = {n: Keypair.create_from_uri(f"//{n}") for n in ["Alice", "Bob", "Charlie", "Dave"]}

    for name, kp in k.items():
        submit(s, kp, "Registry", "register_neuron", {"netuid": 0, "hotkey": kp.ss58_address})
    for name in ["Alice", "Bob"]:
        submit(s, k[name], "Consensus", "stake", {"amount": 10_000 * UNIT})
    submit(s, k["Charlie"], "Registry", "set_endpoint", {"netuid": 0, "endpoint": a.miner_endpoint})
    submit(s, k["Dave"], "Registry", "set_endpoint", {"netuid": 0, "endpoint": a.pool_endpoint})
    submit(s, k["Dave"], "WorkVerify", "bond", {"amount": 5_000 * UNIT})
    submit(s, k["Dave"], "WorkVerify", "register_pool", {"fee_ppm": 20_000})
    print("SN0 ready. Start miners (ipm_neuron.miner) and validators (ipm_neuron.validator).")


if __name__ == "__main__":
    main()
