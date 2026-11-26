"""Testnet rehearsal of Alpha: open subnet registration, buy SN1, register its neurons and run
a validator that sets weights every epoch, then watch the pool open after 10 days.

  python scripts/sn1_rehearsal.py status
  python scripts/sn1_rehearsal.py unlock          # sudo: open registration now (testnet only)
  python scripts/sn1_rehearsal.py buy             # the operator buys the slot on auction
  python scripts/sn1_rehearsal.py neurons         # a validator and two miners on SN1
  python scripts/sn1_rehearsal.py validate        # runs until stopped: weights every epoch

Options: --url (default ws://127.0.0.1:9960), --secrets (default ~/incentium-testnet/secrets).
Keys for SN1 are written to <secrets>/sn1/ (mode 600). Every step can be run again: it only
does what is still missing. It refuses to run on any chain whose name does not contain
"testnet": opening registration by writing storage must never happen on mainnet.

Needs runtime 12 (Alpha) on the testnet: `status` says whether the SubnetPools pallet is there.
"""
import argparse
import json
import os
import secrets
import sys
import time

UNIT = 10**9
NETUID = 1


def load_or_create(path):
    from substrateinterface import Keypair

    if os.path.exists(path):
        return Keypair.create_from_mnemonic(json.load(open(path))["secretPhrase"])
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    phrase = Keypair.generate_mnemonic()
    kp = Keypair.create_from_mnemonic(phrase)
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
        json.dump({"secretPhrase": phrase, "ss58Address": kp.ss58_address}, f)
    return kp


class Chain:
    def __init__(self, a):
        from substrateinterface import SubstrateInterface

        self.s = SubstrateInterface(url=a.url)
        name = self.s.rpc_request("system_chain", [])["result"]
        if "testnet" not in name.lower():
            sys.exit(f"refusing: this is '{name}', not a testnet")
        self.d = os.path.expanduser(a.secrets)
        self.operator = load_or_create(f"{self.d}/operator.json")
        self.faucet = load_or_create(f"{self.d}/faucet.json")
        self.keys = {n: load_or_create(f"{self.d}/sn1/{n}.json") for n in ("validator", "miner1", "miner2")}

    def q(self, module, item, params=None):
        return self.s.query(module, item, params or []).value

    def has(self, pallet):
        return any(p.name == pallet for p in self.s.get_metadata().pallets)

    def submit(self, kp, module, fn, params, sudo=False):
        call = self.s.compose_call(module, fn, params)
        if sudo:
            call = self.s.compose_call("Sudo", "sudo", {"call": call})
        r = self.s.submit_extrinsic(self.s.create_signed_extrinsic(call=call, keypair=kp), wait_for_inclusion=True)
        if not r.is_success:
            sys.exit(f"{module}.{fn} failed: {r.error_message}")
        print(f"{module}.{fn} ok")
        return r

    def head(self):
        return self.s.get_block_number(self.s.get_chain_head())

    def free(self, addr):
        return self.q("System", "Account", [addr])["data"]["free"]


def ict(x):
    return f"{x / UNIT:,.2f}"


def cmd_status(c, a):
    now = c.head()
    print(f"block {now:,}; runtime {c.s.runtime_version}; operator {c.operator.ss58_address} "
          f"({ict(c.free(c.operator.ss58_address))} ICT)")
    sudo = c.q("Sudo", "Key") if c.has("Sudo") else None
    print(f"sudo key: {sudo or 'none'}{' (operator)' if sudo == c.operator.ss58_address else ''}")
    print(f"Alpha module (SubnetPools): {'yes' if c.has('SubnetPools') else 'NO - needs runtime 12'}")
    print(f"registration open: {c.q('Registry', 'Phase2Enabled')}; auction starts at {c.q('Registry', 'AuctionStart')}")
    sub = c.q("Registry", "Subnets", [NETUID])
    print(f"SN{NETUID}: {'sold to ' + str(sub['owner']) if sub else 'not sold'}")
    for n, kp in c.keys.items():
        uid = c.q("Registry", "NeuronUid", [NETUID, kp.ss58_address])
        print(f"  {n:<9} {kp.ss58_address}  uid {uid}  {ict(c.free(kp.ss58_address))} ICT")
    if c.has("SubnetPools"):
        p = c.q("SubnetPools", "Pools", [NETUID])
        if p:
            opens = p["opened"] + 10 * 7200
            state = "open" if p["trading"] else f"closed until block {opens:,} (about {max(0, opens - now) * 12 / 86400:.1f} days)"
            print(f"pool: {state}; ICT {ict(p['x'])}, Alpha {ict(p['y'])}, waiting stock {ict(p['waiting'])}, "
                  f"held ICT {ict(p['reserved_ict'])}, issued {ict(p['issued'])}")


def cmd_unlock(c, a):
    if c.q("Registry", "Phase2Enabled"):
        return print("registration is already open")
    if c.q("Sudo", "Key") != c.operator.ss58_address:
        sys.exit("the operator key is not the testnet's sudo key; open it by a Root referendum instead")
    now = c.head()
    items = []
    for item, typ, val in (("Phase2Enabled", "bool", True), ("UnlockedAt", "u32", now), ("AuctionStart", "u32", now)):
        key = c.s.create_storage_key("Registry", item).to_hex()
        items.append((key, c.s.encode_scale(typ, val).to_hex()))
    c.submit(c.operator, "System", "set_storage", {"items": items}, sudo=True)
    print(f"registration open from block {now:,}; the SN1 auction starts at 50,000 ICT and falls 25 % a day")


def cmd_buy(c, a):
    if c.q("Registry", "Subnets", [NETUID]):
        return print("SN1 is already sold")
    price = c.s.runtime_call("SubnetRegistryApi", "auction", []).value
    print(f"auction: {price}")
    need = a.max_price * UNIT
    if c.free(c.operator.ss58_address) < need + 10 * UNIT:
        c.submit(c.faucet, "Balances", "transfer_keep_alive", {"dest": c.operator.ss58_address, "value": need + 20 * UNIT})
    c.submit(c.operator, "Registry", "register_subnet", {
        "max_price": need, "name": "Rehearsal", "description": "Testnet rehearsal of Alpha pools",
        "code_hash": "0x" + "00" * 32})


def cmd_neurons(c, a):
    for n, kp in c.keys.items():
        if c.free(kp.ss58_address) < 5 * UNIT:
            c.submit(c.faucet, "Balances", "transfer_keep_alive", {"dest": kp.ss58_address, "value": 20 * UNIT})
        if c.q("Registry", "NeuronUid", [NETUID, kp.ss58_address]) is None:
            c.submit(kp, "Registry", "register_neuron", {"netuid": NETUID, "hotkey": kp.ss58_address})


def cmd_validate(c, a):
    """Every epoch: reveal last epoch's commit, then commit weights for the miners."""
    from hashlib import blake2b

    v = c.keys["validator"]
    epoch_len = 360
    last = None
    while True:
        try:
            now = c.head()
            epoch = now // epoch_len
            if epoch != last:
                uids, weights = [], []
                for n in ("miner1", "miner2"):
                    u = c.q("Registry", "NeuronUid", [NETUID, c.keys[n].ss58_address])
                    if u is not None:
                        uids.append(u)
                        weights.append(3 if n == "miner1" else 1)
                state = f"{c.d}/sn1/commit.json"
                prev = json.load(open(state)) if os.path.exists(state) else None
                if prev and prev["epoch"] == epoch - 1:
                    c.submit(v, "Weights", "reveal_weights", {"netuid": NETUID, "uids": prev["uids"],
                                                              "weights": prev["weights"], "salt": prev["salt"]})
                if uids:
                    salt = "0x" + secrets.token_hex(32)
                    enc = (c.s.encode_scale("u16", NETUID).data + c.s.encode_scale("Vec<u16>", uids).data
                           + c.s.encode_scale("Vec<u16>", weights).data + bytes.fromhex(salt[2:]))
                    h = "0x" + blake2b(bytes(enc), digest_size=32).hexdigest()
                    c.submit(v, "Weights", "commit_weights", {"netuid": NETUID, "hash": h})
                    with os.fdopen(os.open(state, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as f:
                        json.dump({"epoch": epoch, "uids": uids, "weights": weights, "salt": salt}, f)
                last = epoch
                print(f"epoch {epoch}: committed weights {dict(zip(uids, weights))}", flush=True)
        except SystemExit as e:
            print(e, flush=True)
        except Exception as e:  # keep going through node restarts
            print(f"error: {e}", flush=True)
        time.sleep(60)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cmd", choices=["status", "unlock", "buy", "neurons", "validate"])
    ap.add_argument("--url", default="ws://127.0.0.1:9960")
    ap.add_argument("--secrets", default="~/incentium-testnet/secrets")
    ap.add_argument("--max-price", type=int, default=50_000, help="most ICT to pay for the slot")
    a = ap.parse_args()
    c = Chain(a)
    {"status": cmd_status, "unlock": cmd_unlock, "buy": cmd_buy, "neurons": cmd_neurons,
     "validate": cmd_validate}[a.cmd](c, a)


if __name__ == "__main__":
    main()
