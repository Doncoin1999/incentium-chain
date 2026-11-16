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
"""
import argparse
import json
import subprocess
import sys

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
    a = ap.parse_args()
    keys = json.load(open(a.keys))
    plain = subprocess.run([a.node, "export-chain-spec", "--chain", "bootstrap-template"],
                           check=True, capture_output=True, text=True).stdout
    spec = json.loads(plain)
    apply_keys(find_patch(spec), keys)
    spec["bootNodes"] = keys.get("bootnodes", [])
    with open(f"{a.out}.json", "w") as f:
        json.dump(spec, f, indent=2)
    raw = subprocess.run([a.node, "export-chain-spec", "--chain", f"{a.out}.json", "--raw"],
                         check=True, capture_output=True, text=True).stdout
    with open(f"{a.out}-raw.json", "w") as f:
        f.write(raw)
    print(f"wrote {a.out}.json and {a.out}-raw.json")


if __name__ == "__main__":
    main()
