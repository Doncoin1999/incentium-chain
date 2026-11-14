#!/usr/bin/env bash
# Start a closed 3-validator IPM devnet on one machine (Alice, Bob, Charlie).
# Usage: scripts/devnet.sh [--fast]   (--fast builds with 10-block epochs)
set -euo pipefail
cd "$(dirname "$0")/.."

FEATURES=""
if [[ "${1:-}" == "--fast" ]]; then FEATURES="--features fast-epochs"; fi

cargo build --release -p ipm-node $FEATURES
BIN=./target/release/ipm-node
DATA=${DEVNET_DIR:-/tmp/ipm-devnet}
mkdir -p "$DATA"

$BIN export-chain-spec --chain local --raw > "$DATA/devnet-raw.json"

# Fixed node key for Alice so the others can find her.
ALICE_KEY=0000000000000000000000000000000000000000000000000000000000000001
ALICE_PEER=12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp
BOOT="/ip4/127.0.0.1/tcp/30333/p2p/$ALICE_PEER"

$BIN --chain "$DATA/devnet-raw.json" --base-path "$DATA/alice" --alice --validator \
  --node-key $ALICE_KEY --port 30333 --rpc-port 9944 --rpc-cors all --unsafe-rpc-external \
  > "$DATA/alice.log" 2>&1 &
sleep 3
$BIN --chain "$DATA/devnet-raw.json" --base-path "$DATA/bob" --bob --validator \
  --port 30334 --rpc-port 9945 --bootnodes "$BOOT" > "$DATA/bob.log" 2>&1 &
$BIN --chain "$DATA/devnet-raw.json" --base-path "$DATA/charlie" --charlie --validator \
  --port 30335 --rpc-port 9946 --bootnodes "$BOOT" > "$DATA/charlie.log" 2>&1 &

echo "Devnet starting. Logs in $DATA. RPC: ws://127.0.0.1:9944"
echo "Next: python3 scripts/bootstrap_sn0.py --url ws://127.0.0.1:9944"
wait
