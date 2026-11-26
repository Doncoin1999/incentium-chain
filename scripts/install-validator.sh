#!/usr/bin/env bash
# Install an Incentium block producer (validator) on a Linux server.
# docs/OPEN-BLOCK-PRODUCTION.md · runtime 12
#
#   bash install-validator.sh mainnet <your stash address> [node name]
#   bash install-validator.sh testnet <your stash address> [node name]
#
# What it does, step by step (read it before you run it):
#   1. installs build tools (Rust, protoc, clang) if missing,
#   2. builds the node from the open source code at a pinned commit (no binary from anyone),
#   3. writes a systemd service that runs the node as a validator,
#   4. waits until the node has synced,
#   5. creates session keys in the node's own keystore, with a proof that they belong to your
#      stash address, and prints them.
# Your stash key never touches this server: you sign `bond`, `session.setKeys` and `validate`
# in your own wallet (Talisman, SubWallet or polkadot.js), as printed at the end.
#
# Needs: Ubuntu/Debian, 4 cores, 8 GB memory, 200 GB disk, a public IP with port 30333 open.
set -euo pipefail

NET="${1:?usage: install-validator.sh mainnet|testnet <stash address> [node name]}"
STASH="${2:?give the stash address that will bond the ICT}"
NAME="${3:-incentium-validator-$(hostname -s)}"
case "$NET" in mainnet|testnet) ;; *) echo "network must be mainnet or testnet"; exit 1 ;; esac

REPO="${INCENTIUM_REPO:-https://git.incentium.org/incentium/incentium-chain.git}"
REF="${INCENTIUM_REF:-main}"
D="$HOME/incentium-validator"
SRC="$D/src"
BIN="$D/incentium-node"
BASE="$D/data-$NET"
SPEC="$SRC/chain-specs/incentium-$NET-raw.json"
RPC_PORT=9944
# Boot nodes beyond the two in the chain spec (the spec file stays unchanged so its sha256 holds).
EXTRA_BOOT_mainnet="/ip4/94.72.107.58/tcp/30333/p2p/12D3KooWQdXvwW1R5u5bwqDqb6jAo1dRWGLbHTn8XDGKLuPQubyu"
EXTRA_BOOT_testnet=""
EXTRA_BOOT="$(eval echo "\$EXTRA_BOOT_$NET")"

say() { printf '\n== %s\n' "$*"; }

say "1/5 build tools"
if ! command -v cargo >/dev/null; then
  sudo apt-get update -y
  sudo apt-get install -y build-essential clang libclang-dev protobuf-compiler pkg-config libssl-dev git curl jq
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"
command -v jq >/dev/null || sudo apt-get install -y jq

say "2/5 build the node from source ($REPO @ $REF)"
mkdir -p "$D"
if [ -d "$SRC/.git" ]; then
  git -C "$SRC" fetch --quiet origin "$REF" && git -C "$SRC" checkout --quiet FETCH_HEAD
else
  git clone --quiet "$REPO" "$SRC" && git -C "$SRC" checkout --quiet "$REF"
fi
echo "source commit: $(git -C "$SRC" rev-parse HEAD)"
[ -s "$SPEC" ] || { echo "chain spec $SPEC not found in the source"; exit 1; }
echo "chain spec sha256: $(sha256sum "$SPEC" | cut -d' ' -f1)  (compare with incentium.org/validators)"
(cd "$SRC" && cargo build --release -p incentium-node)
install -m 755 "$SRC/target/release/incentium-node" "$BIN"

say "3/5 systemd service incentium-validator-$NET"
UNIT="/etc/systemd/system/incentium-validator-$NET.service"
sudo tee "$UNIT" >/dev/null <<EOF
[Unit]
Description=Incentium validator ($NET)
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
ExecStart=$BIN --chain $SPEC --base-path $BASE --validator --name "$NAME" \
  --port 30333 --rpc-port $RPC_PORT --rpc-methods unsafe --no-telemetry${EXTRA_BOOT:+ --bootnodes $EXTRA_BOOT}
Restart=always
RestartSec=10
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now "incentium-validator-$NET"

rpc() { curl -s -H 'Content-Type: application/json' \
  -d "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"$1\",\"params\":$2}" "http://127.0.0.1:$RPC_PORT"; }

say "4/5 waiting for the node to sync (this can take a while)"
until rpc system_health '[]' | jq -e '.result.isSyncing == false and .result.peers > 0' >/dev/null 2>&1; do
  printf '.'; sleep 20
done
echo " synced at block $(rpc chain_getHeader '[]' | jq -r '.result.number' | xargs printf '%d')"

say "5/5 session keys with an ownership proof for $STASH"
OWNER_HEX="$("$BIN" key inspect "$STASH" 2>/dev/null | awk '/Account ID:/ {print $3}')"
[ -n "$OWNER_HEX" ] || { echo "could not read the stash address $STASH"; exit 1; }
OUT="$(rpc author_rotateKeysWithOwner "[\"$OWNER_HEX\"]")"
KEYS="$(echo "$OUT" | jq -r '.result.keys')"
PROOF="$(echo "$OUT" | jq -r '.result.proof')"
[ "$KEYS" != "null" ] || { echo "the node did not create keys: $OUT"; exit 1; }
printf '%s\n%s\n' "$KEYS" "$PROOF" > "$D/session-keys-$NET.txt"

cat <<EOF

Done. The node runs as incentium-validator-$NET; logs: journalctl -fu incentium-validator-$NET

Session keys (public) and proof, also saved in $D/session-keys-$NET.txt:
  keys:  $KEYS
  proof: $PROOF

Now, in your own wallet, signed by the stash $STASH:
  1. staking.bond(value, payee)            at least 100 ICT
  2. session.setKeys(keys, proof)          the two lines above
  3. staking.validate({commission, blocked: false})
You can do this at https://incentium.org/validators or in polkadot.js:
  https://polkadot.js.org/apps/?rpc=$( [ "$NET" = mainnet ] && echo wss://rpc.incentium.org || echo wss://testnet-rpc.incentium.org )#/staking/actions
The chain elects the set once a day; a validator elected for a day that writes no block is
taken off the candidate list and can stand again with staking.validate.
EOF
