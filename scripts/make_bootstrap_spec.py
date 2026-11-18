"""Create the mainnet chain spec for the disclosed 90-day bootstrap period.

1. Generate keys on an offline machine and keep the secrets there:
     incentium-node key generate --scheme sr25519      # operator account (sudo + bootstrap operator)
     incentium-node key generate --scheme sr25519      # x3: Aura key per validator node
     incentium-node key generate --scheme ed25519      # x3: Grandpa key per validator node
2. Put the PUBLIC SS58 addresses in a JSON file:
     {"operator": "5...", "authorities": [{"aura": "5...", "grandpa": "5..."}, x3]}
3. python3 scripts/make_bootstrap_spec.py keys.json --node ./target/release/incentium-node --out incentium-mainnet

Writes incentium-mainnet.json (readable) and incentium-mainnet-raw.json (what the nodes run).
Insert each node's Aura and Grandpa secret keys with `incentium-node key insert` before starting it.

Testnet (a dress rehearsal of the mainnet launch) uses the same template with its own name and
id, and may give a faucet a genesis allocation of test ICT (keys.json: "faucet":
{"address": "5...", "ict": 100000}); it counts in the issuance like the operator's 100 ICT.
A launch rehearsal (scripts/launch-rehearsal.sh) shortens the launch period with
--bootstrap-blocks, allowed only for ids ending in _rehearsal.
Never use a faucet allocation on mainnet:
  python3 scripts/make_bootstrap_spec.py keys.json --out incentium-testnet \
      --name "Incentium Testnet" --id incentium_testnet --protocol-id ict-testnet
"""
import argparse
import json
import subprocess
import sys

UNIT = 10**9

PLACEHOLDER_SR = {
    "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",  # Alice
    "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty",  # Bob
    "5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y",  # Charlie
}
PLACEHOLDER_ED = {
    "5FA9nQDVg267DEd8m1ZypXLBnvN7SFxYwV7ndqSYGiN9TTpu",  # Alice
    "5GoNkf6WdbxCFnPdAnYYQyCjAKPJgLNxXwPjwTh6DGg6gN3E",  # Bob
    "5DbKjhNLpqX3zqZdNBc9BGb4fHU1cRBaDhJUskrvkwfraDi6",  # Charlie
}


def find_patch(spec: dict) -> dict:
    g = spec["genesis"]
    rg = g.get("runtimeGenesis", g)
    for k in ("patch", "runtimeGenesisConfigPatch", "config"):
        if k in rg:
            return rg[k]
    sys.exit("could not find the genesis patch in the chain spec")


def apply_keys(patch: dict, keys: dict) -> None:
    op = keys["operator"]
    auths = keys["authorities"]
    if not 1 <= len(auths) <= 32:
        sys.exit("need 1-32 authorities")
    patch["aura"]["authorities"] = [a["aura"] for a in auths]
    patch["grandpa"]["authorities"] = [[a["grandpa"], 1] for a in auths]
    alloc = patch["balances"]["balances"][0][1]
    patch["balances"]["balances"] = [[op, alloc]]
    patch["sudo"]["key"] = op
    patch["registry"]["bootstrapOperator"] = op
    # Curator of the end-of-launch bounties (whitepaper 5.1): a 2-of-3 multisig address.
    if "launch" in patch:
        patch["launch"]["curator"] = keys.get("curator", op)
    faucet = keys.get("faucet")
    if faucet:
        amount = int(faucet["ict"]) * UNIT
        patch["balances"]["balances"].append([faucet["address"], amount])
        patch["emission"]["preIssued"] = patch["emission"]["preIssued"] + amount
    # sanity: no development key may survive into the mainnet spec
    text = json.dumps(patch)
    leftover = [p for p in PLACEHOLDER_SR | PLACEHOLDER_ED if p in text]
    if leftover:
        sys.exit(f"development keys still present: {leftover}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("keys")
    ap.add_argument("--node", default="./target/release/incentium-node")
    ap.add_argument("--out", default="incentium-mainnet")
    ap.add_argument("--name", help="chain name shown in wallets, e.g. 'Incentium Testnet'")
    ap.add_argument("--id", help="chain id, e.g. incentium_testnet")
    ap.add_argument("--protocol-id", help="p2p protocol id, e.g. ict-testnet")
    ap.add_argument("--bootstrap-blocks", type=int,
                    help="shorter launch period, for a rehearsal chain only (id ending in _rehearsal)")
    a = ap.parse_args()
    if a.bootstrap_blocks is not None and not (a.id or "").endswith("_rehearsal"):
        sys.exit("--bootstrap-blocks is for rehearsal chains only (pass --id ..._rehearsal)")
    keys = json.load(open(a.keys))
    if keys.get("faucet") and not (a.id or "").endswith("testnet"):
        sys.exit("a faucet allocation is for testnet only (pass --id ..._testnet)")
    plain = subprocess.run([a.node, "export-chain-spec", "--chain", "bootstrap-template"],
                           check=True, capture_output=True, text=True).stdout
    spec = json.loads(plain)
    patch = find_patch(spec)
    apply_keys(patch, keys)
    if a.bootstrap_blocks is not None:
        patch["registry"]["bootstrapBlocks"] = a.bootstrap_blocks
    spec["bootNodes"] = keys.get("bootnodes", [])
    if a.name:
        spec["name"] = a.name
    if a.id:
        spec["id"] = a.id
    if a.protocol_id:
        spec["protocolId"] = a.protocol_id
    with open(f"{a.out}.json", "w") as f:
        json.dump(spec, f, indent=2)
    raw = subprocess.run([a.node, "export-chain-spec", "--chain", f"{a.out}.json", "--raw"],
                         check=True, capture_output=True, text=True).stdout
    with open(f"{a.out}-raw.json", "w") as f:
        f.write(raw)
    print(f"wrote {a.out}.json and {a.out}-raw.json")


if __name__ == "__main__":
    main()
