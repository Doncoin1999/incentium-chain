"""Testnet rehearsal of Alpha: open subnet registration, buy SN1, register its neurons and run
a validator that sets weights every epoch, then watch the pool open after 10 days.

  python scripts/sn1_rehearsal.py status
  python scripts/sn1_rehearsal.py upgrade --wasm F # sudo: install a runtime now (testnet only)
  python scripts/sn1_rehearsal.py unlock          # sudo: open registration now (testnet only)
  python scripts/sn1_rehearsal.py buy             # the operator buys the slot on auction
  python scripts/sn1_rehearsal.py neurons         # a validator and two miners on SN1
  python scripts/sn1_rehearsal.py validate        # runs until stopped: weights every epoch
  python scripts/sn1_rehearsal.py open-now        # sudo: skip the 10 closed days (testnet only)

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
        self.s.init_runtime()
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
            detail = r.error_message or [e.value for e in r.triggered_events if e.value["event_id"] in ("ExtrinsicFailed", "Sudid")]
            sys.exit(f"{module}.{fn} failed: {detail}")
        if sudo:
            for e in r.triggered_events:
                v = e.value
                if v["event_id"] == "Sudid" and "Err" in str(v["attributes"]):
                    sys.exit(f"{module}.{fn} under sudo failed: {v['attributes']}")
        print(f"{module}.{fn} ok")
        return r

    def head(self):
        return self.s.get_block_number(self.s.get_chain_head())

    def free(self, addr):
        return self.q("System", "Account", [addr])["data"]["free"]

    def fund(self, addr, amount):
        """Testnet ICT for the rehearsal: by sudo if the operator holds it, else from the faucet."""
        if self.q("Sudo", "Key") == self.operator.ss58_address:
            self.submit(self.operator, "Balances", "force_set_balance",
                        {"who": addr, "new_free": self.free(addr) + amount}, sudo=True)
        elif self.free(self.faucet.ss58_address) > amount + 10 * UNIT:
            self.submit(self.faucet, "Balances", "transfer_keep_alive", {"dest": addr, "value": amount})
        else:
            sys.exit(f"cannot fund {addr}: the faucet is short and the operator is not sudo")


def ict(x):
    return f"{x / UNIT:,.2f}"


def cmd_status(c, a):
    now = c.head()
    print(f"block {now:,}; runtime version {c.s.runtime_version}; operator {c.operator.ss58_address} "
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
    need = a.max_price * UNIT
    if c.free(c.operator.ss58_address) < need + 10 * UNIT:
        c.fund(c.operator.ss58_address, need + 20 * UNIT)
    c.submit(c.operator, "Registry", "register_subnet", {
        "max_price": need, "name": "Rehearsal", "description": "Testnet rehearsal of Alpha pools",
        "code_hash": "0x" + "00" * 32})


def cmd_neurons(c, a):
    for n, kp in c.keys.items():
        if c.free(kp.ss58_address) < 5 * UNIT:
            c.fund(kp.ss58_address, 20 * UNIT)
        if c.q("Registry", "NeuronUid", [NETUID, kp.ss58_address]) is None:
            # in the launch (bootstrap) period only the operator registers neurons, fee waived
            bootstrap = (c.q("Registry", "BootstrapUntil") or 0) > c.head()
            c.submit(c.operator if bootstrap else kp, "Registry", "register_neuron",
                     {"netuid": NETUID, "hotkey": kp.ss58_address})


def cmd_upgrade(c, a):
    """sudo authorize_upgrade(hash), then apply_authorized_upgrade(code): the same final step
    a passed referendum uses, without the referendum."""
    from hashlib import blake2b

    if c.q("Sudo", "Key") != c.operator.ss58_address:
        sys.exit("the operator key is not the testnet's sudo key")
    code = open(os.path.expanduser(a.wasm), "rb").read()
    h = "0x" + blake2b(code, digest_size=32).hexdigest()
    before = c.s.runtime_version
    print(f"code {len(code):,} bytes, hash {h}; runtime now {before}")
    c.submit(c.operator, "System", "authorize_upgrade", {"code_hash": h}, sudo=True)
    c.submit(c.operator, "System", "apply_authorized_upgrade", {"code": "0x" + code.hex()})
    for _ in range(30):
        time.sleep(12)
        c.s.init_runtime()
        if c.s.runtime_version != before:
            return print(f"runtime is now {c.s.runtime_version}")
    print("the upgrade was applied; the new version shows from the next block")


def cmd_open_now(c, a):
    """Move the pool's start 10 days back by sudo, so it opens in the next block."""
    if c.q("Sudo", "Key") != c.operator.ss58_address:
        sys.exit("the operator key is not the testnet's sudo key")
    p = c.q("SubnetPools", "Pools", [NETUID])
    if not p:
        sys.exit("SN1 has no pool yet: run buy first")
    if p["trading"]:
        return print("the pool is already open")
    now = c.head()
    p["opened"] = max(0, now - 10 * 7200 - 1)
    sf = c.s.get_metadata_storage_function("SubnetPools", "Pools")
    key = c.s.create_storage_key("SubnetPools", "Pools", [NETUID]).to_hex()
    val = c.s.encode_scale(sf.get_value_type_string(), p).to_hex()
    c.submit(c.operator, "System", "set_storage", {"items": [(key, val)]}, sudo=True)
    time.sleep(15)
    p = c.q("SubnetPools", "Pools", [NETUID])
    print("pool open" if p["trading"] else "not open yet: check status in a minute (it needs issued stock)")


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
    ap.add_argument("cmd", choices=["status", "upgrade", "unlock", "buy", "neurons", "validate", "open-now"])
    ap.add_argument("--wasm", help="runtime file for upgrade")
    ap.add_argument("--url", default="ws://127.0.0.1:9960")
    ap.add_argument("--secrets", default="~/incentium-testnet/secrets")
    ap.add_argument("--max-price", type=int, default=50_000, help="most ICT to pay for the slot")
    a = ap.parse_args()
    c = Chain(a)
    {"status": cmd_status, "upgrade": cmd_upgrade, "unlock": cmd_unlock, "buy": cmd_buy,
     "neurons": cmd_neurons, "validate": cmd_validate, "open-now": cmd_open_now}[a.cmd](c, a)


if __name__ == "__main__":
    main()
