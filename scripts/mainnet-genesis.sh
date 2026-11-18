#!/usr/bin/env bash
# Mainnet step 2, on server A only: create the operator and curator keys and the chain spec.
#
#   scp administrator@SERVER_B:~/incentium-mainnet/public-b.json ~/incentium-mainnet/
#   bash scripts/mainnet-genesis.sh
#
# Needs ~/incentium-mainnet/public-a.json and public-b.json (from mainnet-node.sh on each server).
# Creates in ~/incentium-mainnet/secrets: operator (sudo and launch operator for the 90-day launch
# period, 100 ICT at genesis) and curator1..3 (the 2-of-3 multisig that curates the fund bounties).
# Move curator2 and curator3 off the server after backing them up; they are only needed to sign.
# Writes ~/incentium-mainnet/incentium-mainnet-raw.json, the chain spec both servers run.
# Nothing starts here: the chain starts with mainnet-start.sh.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
D="$HOME/incentium-mainnet"
S="$D/secrets"
BIN="$D/incentium-node"
SPEC="$D/incentium-mainnet-raw.json"

for f in "$D/public-a.json" "$D/public-b.json"; do
  [ -s "$f" ] || { echo "missing $f"; exit 1; }
done
if [ -s "$SPEC" ]; then
  echo "$SPEC already exists; not overwriting it (delete it first only if the chain has never run)"
  exit 1
fi
gen() {
  local f="$S/$1.json"
  [ -s "$f" ] || { "$BIN" key generate --scheme sr25519 --output-type json > "$f"; chmod 600 "$f"; }
}
if [ -s "$D/keys.json" ]; then
  # Rebuilding the spec (e.g. with a newer runtime before launch): keep the same keys.
  echo "reusing $D/keys.json (operator, curator and authorities unchanged)"
else
  gen operator
  for i in 1 2 3; do gen "curator$i"; done
  python3 "$HERE/mainnet_keys.py" genesis "$S" "$D/public-a.json" "$D/public-b.json" "$D/keys.json"
fi
python3 "$HERE/make_bootstrap_spec.py" "$D/keys.json" --node "$BIN" --out "$D/incentium-mainnet" \
  --name "Incentium" --id incentium --protocol-id ict

echo
echo "Operator (sudo and launch operator): $(python3 -c "import json;print(json.load(open('$D/keys.json'))['operator'])")"
echo "Chain spec: $SPEC"
echo "sha256: $(sha256sum "$SPEC" | cut -d' ' -f1)"
echo "Copy it to server B:  scp $SPEC administrator@$(python3 -c "import json;print(json.load(open('$D/public-b.json'))['ip'])"):~/incentium-mainnet/"
echo "Back up $S off the server now."
