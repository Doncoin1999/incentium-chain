#!/usr/bin/env bash
# Dress rehearsal of the end of the launch period, on a throwaway one-node chain.
#
# Builds a chain from the mainnet bootstrap template with fresh keys and a launch period of
# BLOCKS blocks (default 100 = 20 minutes), starts one node, and runs launch_rehearsal_check.py,
# which checks the rules before the end, waits for the end, and checks what the runtime did:
# sudo removed, fund bounties created and approved with the curator, registration opened.
#
#   bash scripts/launch-rehearsal.sh [BIN] [BLOCKS]
#   BIN defaults to ~/ny-devnet/incentium-node. Safe to run again: it starts from scratch.
#   Uses ports 9970 (RPC) and 30450 (p2p); nothing else on the server is touched.
set -euo pipefail
BIN="${1:-$HOME/ny-devnet/incentium-node}"
BLOCKS="${2:-100}"
D="$HOME/incentium-rehearsal"
PY="${PY:-$HOME/incentium-venv/bin/python}"
HERE="$(cd "$(dirname "$0")" && pwd)"
[ -x "$BIN" ] || { echo "missing node binary $BIN"; exit 1; }

pkill -f "base-path $D/data" 2>/dev/null || true
sleep 1
rm -rf "$D"; mkdir -p "$D"; chmod 700 "$D"

gen() { "$BIN" key generate --scheme "$2" --output-type json > "$D/$1.json"; }
field() { "$PY" -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$D/$1.json" "$2"; }
gen operator sr25519
gen aura sr25519
gen gran ed25519

cat > "$D/keys.json" <<EOF
{"operator": "$(field operator ss58Address)",
 "authorities": [{"aura": "$(field aura ss58Address)", "grandpa": "$(field gran ss58Address)"}]}
EOF
"$PY" "$HERE/make_bootstrap_spec.py" "$D/keys.json" --node "$BIN" --out "$D/rehearsal" \
  --name "Incentium Launch Rehearsal" --id incentium_rehearsal --protocol-id ict-rehearsal \
  --bootstrap-blocks "$BLOCKS"

SPEC="$D/rehearsal-raw.json"
"$BIN" key insert --chain "$SPEC" --base-path "$D/data" --scheme sr25519 --key-type aura --suri "$(field aura secretPhrase)"
"$BIN" key insert --chain "$SPEC" --base-path "$D/data" --scheme ed25519 --key-type gran --suri "$(field gran secretPhrase)"

"$BIN" key generate-node-key --file "$D/node.key" 2>/dev/null
nohup "$BIN" --chain "$SPEC" --base-path "$D/data" --name rehearsal --validator --force-authoring \
  --node-key-file "$D/node.key" \
  --port 30450 --rpc-port 9970 --no-prometheus --no-telemetry --no-mdns > "$D/node.log" 2>&1 &
echo "node started (log: $D/node.log); launch period ends at block $BLOCKS (about $((BLOCKS / 5)) minutes)"

set +e
"$PY" "$HERE/launch_rehearsal_check.py" --url ws://127.0.0.1:9970 --operator "$D/operator.json"
rc=$?
pkill -f "base-path $D/data"
echo "node stopped; files kept in $D"
exit $rc
