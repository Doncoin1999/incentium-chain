#!/usr/bin/env bash
# Install an Incentium network operator on a Linux server (spec Section 19).
#
#   bash install-operator.sh mainnet|testnet [roles] [--name <node name>] [--healthchecks <ping URL>]
#
# Roles, any combination (without a role the server runs a plain full node):
#   --producer <stash address>    block producer: session keys with an ownership proof for your stash
#   --sn0-validator [--uid <n>]   SN0 validator: validator software, reference model and a hotkey
#   --rpc <domain>                public RPC endpoint at wss://<domain>, with a TLS certificate (Caddy)
#   --archive                     archive node: keeps the full history (choose this before the first sync)
#
# Ports, to run beside another node on the same server (defaults in brackets):
#   --p2p-port <n> [30333]  --rpc-port <n> [9944]  --validator-port <n> [8100]  --model-port <n> [8095]
# Services and files are named per network (e.g. incentium-node-testnet), so a testnet node can
# run on a server that already runs a mainnet node.
#
# What it does, step by step (read it before you run it):
#   1. installs build tools (Rust, protoc, clang) if missing,
#   2. builds the node from the open source code (no binary from anyone) and prints the commit,
#   3. writes one systemd service for the node, with the flags the chosen roles need,
#   4. waits until the node has synced,
#   5. per role: creates session keys with a proof (producer); installs the validator, the
#      reference model and a hotkey (SN0 validator); sets up Caddy with a certificate (RPC),
#   6. installs the watchdog (optional Healthchecks URL),
#   7. prints what you sign in your own wallet: bond, set keys, validate, the SN0 registration
#      and the entry in the endpoint register. Your stash key never touches this server.
#
# Needs: Ubuntu/Debian, 4 cores, 8 GB memory (more with an SN0 reference model), 200 GB SSD
# (more for an archive node), a public IP with the p2p port open, and port 443 for --rpc.
set -euo pipefail

usage() { sed -n '2,17p' "$0"; exit 1; }
NET="${1:-}"; shift || true
case "$NET" in mainnet|testnet) ;; *) usage ;; esac

PRODUCER="" ; SN0=0 ; UID_ARG="" ; RPC_DOMAIN="" ; ARCHIVE=0
NAME="incentium-$(hostname -s)" ; HC_URL=""
P2P_PORT=30333 ; RPC_PORT=9944 ; VAL_PORT=8100 ; LLAMA_PORT=8095
while [ $# -gt 0 ]; do
  case "$1" in
    --producer)      PRODUCER="${2:?--producer needs your stash address}"; shift 2 ;;
    --sn0-validator) SN0=1; shift ;;
    --uid)           UID_ARG="${2:?--uid needs a number}"; shift 2 ;;
    --rpc)           RPC_DOMAIN="${2:?--rpc needs a domain name, e.g. rpc.example.org}"; shift 2 ;;
    --archive)       ARCHIVE=1; shift ;;
    --name)          NAME="${2:?}"; shift 2 ;;
    --healthchecks)  HC_URL="${2:?}"; shift 2 ;;
    --p2p-port)      P2P_PORT="${2:?}"; shift 2 ;;
    --rpc-port)      RPC_PORT="${2:?}"; shift 2 ;;
    --validator-port) VAL_PORT="${2:?}"; shift 2 ;;
    --model-port)    LLAMA_PORT="${2:?}"; shift 2 ;;
    -h|--help)       usage ;;
    *) echo "unknown option: $1"; usage ;;
  esac
done

REPO="${INCENTIUM_REPO:-https://git.incentium.org/incentium/incentium-chain.git}"
REF="${INCENTIUM_REF:-main}"
D="$HOME/incentium-operator"
SRC="$D/src"
BIN="$D/incentium-node"
BASE="$D/data-$NET"
SPEC="$SRC/chain-specs/incentium-$NET-raw.json"
SERVICE="incentium-node-$NET"
PY="$D/venv/bin/python"
HOTKEY="$D/sn0-hotkey-$NET.json"
PUBLIC_RPC=$([ "$NET" = mainnet ] && echo wss://rpc.incentium.org || echo wss://testnet-rpc.incentium.org)

say() { printf '\n== %s\n' "$*"; }
die() { printf '\nError: %s\n' "$*" >&2; exit 1; }
ROLES=""
[ -n "$PRODUCER" ] && ROLES="$ROLES producer"
[ "$SN0" = 1 ] && ROLES="$ROLES sn0-validator"
[ -n "$RPC_DOMAIN" ] && ROLES="$ROLES rpc"
[ "$ARCHIVE" = 1 ] && ROLES="$ROLES archive"
echo "Incentium operator on $NET; roles:${ROLES:- full node only}"
[ -n "$PRODUCER" ] && [ -n "$RPC_DOMAIN" ] && \
  echo "Note: a block producer and a public RPC endpoint on one server work, but separate servers are safer."

# ---------------------------------------------------------------- checks
[ "$(uname -s)" = Linux ] || die "this installer is for Linux (Ubuntu/Debian)."
command -v sudo >/dev/null || die "sudo is needed to install packages and services."
MEM_GB=$(awk '/MemTotal/ {print int($2/1024/1024)}' /proc/meminfo)
[ "$MEM_GB" -ge 7 ] || die "at least 8 GB of memory is needed (found ${MEM_GB} GB)."
mkdir -p "$D"
FREE_GB=$(df -Pk "$D" | awk 'NR==2 {print int($4/1024/1024)}')
NEED_GB=$([ "$ARCHIVE" = 1 ] && echo 150 || echo 60)
[ "$FREE_GB" -ge "$NEED_GB" ] || die "at least ${NEED_GB} GB of free disk is needed here (found ${FREE_GB} GB)."
if [ -d "$BASE/chains" ] && [ "$ARCHIVE" = 1 ] && [ ! -f "$BASE/ARCHIVE" ]; then
  die "the node already synced without archive pruning; an archive node must sync from scratch. Move $BASE away and run again."
fi

say "1/7 build tools"
if ! command -v cargo >/dev/null; then
  sudo apt-get update -y
  sudo apt-get install -y build-essential clang libclang-dev protobuf-compiler pkg-config libssl-dev git curl jq
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"
# the runtime is compiled to WASM: the target and rust-src are needed even when cargo was already installed
rustup target add wasm32-unknown-unknown >/dev/null
rustup component add rust-src >/dev/null
command -v jq >/dev/null || sudo apt-get install -y jq

say "2/7 build the node from source ($REPO @ $REF)"
if [ -d "$SRC/.git" ]; then
  git -C "$SRC" fetch --quiet origin "$REF" && git -C "$SRC" checkout --quiet FETCH_HEAD
else
  git clone --quiet "$REPO" "$SRC" && git -C "$SRC" checkout --quiet "$REF"
fi
echo "source commit: $(git -C "$SRC" rev-parse HEAD)"
[ -s "$SPEC" ] || die "chain spec $SPEC not found in the source"
echo "chain spec sha256: $(sha256sum "$SPEC" | cut -d' ' -f1)  (compare with incentium.org/validators)"
(cd "$SRC" && cargo build --release -p incentium-node)
install -m 755 "$SRC/target/release/incentium-node" "$BIN"

# The producer needs the node's key methods once, on localhost only; the service then runs
# with safe methods, which is also what a public RPC endpoint must expose.
node_flags() {
  local methods="$1"
  local f="--chain $SPEC --base-path $BASE --name \"$NAME\" --port $P2P_PORT --rpc-port $RPC_PORT --no-telemetry"
  [ -n "$PRODUCER" ] && f="$f --validator"
  [ "$ARCHIVE" = 1 ] && f="$f --state-pruning archive --blocks-pruning archive"
  if [ -n "$RPC_DOMAIN" ]; then
    f="$f --rpc-methods safe --rpc-cors all --rpc-max-connections 500"
  else
    f="$f --rpc-methods $methods"
  fi
  echo "$f"
}
write_service() {
  sudo tee "/etc/systemd/system/$SERVICE.service" >/dev/null <<EOF
[Unit]
Description=Incentium node ($NET;${ROLES:- full node})
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
ExecStart=$BIN $(node_flags "$1")
Restart=always
RestartSec=10
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
  sudo systemctl daemon-reload
}

say "3/7 systemd service $SERVICE"
# Key creation needs unsafe methods; with a public RPC endpoint they are never exposed, so the
# keys are made by a short-lived second process instead (see step 5).
write_service "$([ -n "$PRODUCER" ] && echo unsafe || echo safe)"
[ "$ARCHIVE" = 1 ] && { mkdir -p "$BASE"; touch "$BASE/ARCHIVE"; }
sudo systemctl enable --now "$SERVICE"

rpc() { curl -s -H 'Content-Type: application/json' \
  -d "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"$1\",\"params\":$2}" "http://127.0.0.1:${3:-$RPC_PORT}"; }

say "4/7 waiting for the node to sync (this can take a while)"
until rpc system_health '[]' | jq -e '.result.isSyncing == false and .result.peers > 0' >/dev/null 2>&1; do
  printf '.'; sleep 20
done
echo " synced at block $(rpc chain_getHeader '[]' | jq -r '.result.number' | xargs printf '%d')"
PEER_ID="$(rpc system_localPeerId '[]' | jq -r '.result')"

KEYS="" ; PROOF="" ; SN0_ADDR=""
say "5/7 roles"
if [ -n "$PRODUCER" ]; then
  echo "-- block producer: session keys with an ownership proof for $PRODUCER"
  OWNER_HEX="$("$BIN" key inspect "$PRODUCER" 2>/dev/null | awk '/Account ID:/ {print $3}')"
  [ -n "$OWNER_HEX" ] || die "could not read the stash address $PRODUCER"
  if [ -n "$RPC_DOMAIN" ]; then
    # The service exposes safe methods only; stop it and make the keys with a local process.
    sudo systemctl stop "$SERVICE"
    "$BIN" --chain "$SPEC" --base-path "$BASE" --validator --rpc-port $((RPC_PORT + 44)) --rpc-methods unsafe \
      --port $((P2P_PORT + 66)) --no-telemetry >/dev/null 2>&1 &
    TMP_PID=$!
    until rpc system_health '[]' $((RPC_PORT + 44)) >/dev/null 2>&1; do sleep 2; done
    OUT="$(rpc author_rotateKeysWithOwner "[\"$OWNER_HEX\"]" $((RPC_PORT + 44)))"
    kill "$TMP_PID"; wait "$TMP_PID" 2>/dev/null || true
    sudo systemctl start "$SERVICE"
  else
    OUT="$(rpc author_rotateKeysWithOwner "[\"$OWNER_HEX\"]")"
    # From now on the service needs no key methods.
    write_service safe
    sudo systemctl restart "$SERVICE"
  fi
  KEYS="$(echo "$OUT" | jq -r '.result.keys')"
  PROOF="$(echo "$OUT" | jq -r '.result.proof')"
  [ "$KEYS" != null ] || die "the node did not create keys: $OUT"
  printf '%s\n%s\n' "$KEYS" "$PROOF" > "$D/session-keys-$NET.txt"
fi

if [ "$SN0" = 1 ]; then
  echo "-- SN0 validator: validator software, reference model and hotkey"
  sudo apt-get install -y -qq python3 python3-venv cmake libgomp1 >/dev/null
  [ -x "$PY" ] || python3 -m venv "$D/venv"
  "$PY" -m pip install -q --upgrade pip substrate-interface
  LLAMA="$D/llama/build/bin/llama-server"
  if [ ! -x "$LLAMA" ]; then
    echo "building llama.cpp (5-15 minutes)"
    rm -rf "$D/llama"; git clone -q --depth 1 https://github.com/ggml-org/llama.cpp "$D/llama/src"
    cmake -S "$D/llama/src" -B "$D/llama/build" -DCMAKE_BUILD_TYPE=Release -DLLAMA_CURL=OFF >/dev/null
    cmake --build "$D/llama/build" --target llama-server -j "$(nproc)" >/dev/null
  fi
  CHOICE=$(cd "$SRC/neuron" && "$PY" -m incentium_neuron.setup_miner choose --url "ws://127.0.0.1:$RPC_PORT" --cpu-only) \
    || die "could not read the open models on chain"
  field() { echo "$CHOICE" | "$PY" -c "import json,sys; print(json.load(sys.stdin)['$1'])"; }
  TIER=$(field tier); MODEL_NAME=$(field model); MODEL_URL=$(field url); WANT=$(field model_hash)
  mkdir -p "$D/models"; MODEL="$D/models/$(field file)"
  [ -s "$MODEL" ] || { echo "downloading $MODEL_NAME"; curl -fL --progress-bar "$MODEL_URL" -o "$MODEL.part"; mv "$MODEL.part" "$MODEL"; }
  if [ -n "$WANT" ]; then
    [ "${WANT#0x}" = "$(sha256sum "$MODEL" | cut -d' ' -f1)" ] || die "the model file does not match the hash on chain; delete $MODEL and run again."
  fi
  # Creates the hotkey once; on later runs it only prints the address.
  SN0_ADDR=$(cd "$SRC/neuron" && "$PY" -m incentium_neuron.setup_miner keygen "$HOTKEY")
  chmod 600 "$HOTKEY"
  sudo tee /etc/systemd/system/incentium-reference-$NET.service >/dev/null <<EOF
[Unit]
Description=Incentium SN0 reference model ($MODEL_NAME)
After=network-online.target

[Service]
User=$USER
ExecStart=$LLAMA -m $MODEL --host 127.0.0.1 --port $LLAMA_PORT --alias $MODEL_NAME -np 2 -c 4096 -t $(nproc)
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
  PUBLIC_IP="$(curl -fsS https://api.ipify.org 2>/dev/null || hostname -I | awk '{print $1}')"
  sudo tee /etc/systemd/system/incentium-sn0-validator-$NET.service >/dev/null <<EOF
[Unit]
Description=Incentium SN0 validator
After=$SERVICE.service incentium-reference-$NET.service

[Service]
User=$USER
WorkingDirectory=$SRC/neuron
ExecStart=$PY -m incentium_neuron.validator --url ws://127.0.0.1:$RPC_PORT --key-file $HOTKEY --uid ${UID_ARG:-0} --reference http://127.0.0.1:$LLAMA_PORT --model $MODEL_NAME --tier $TIER --listen 0.0.0.0:$VAL_PORT --public-url http://$PUBLIC_IP:$VAL_PORT --state-file $D/validator-state-$NET.json
Restart=always
RestartSec=30

[Install]
WantedBy=multi-user.target
EOF
  sudo systemctl daemon-reload
  sudo systemctl enable --now incentium-reference-$NET >/dev/null
  if [ -n "$UID_ARG" ]; then
    sudo systemctl enable --now incentium-sn0-validator-$NET >/dev/null
  else
    echo "the validator service is ready but not started: register the hotkey first (see the end)."
  fi
fi

if [ -n "$RPC_DOMAIN" ]; then
  echo "-- public RPC at wss://$RPC_DOMAIN"
  command -v caddy >/dev/null || sudo apt-get install -y caddy
  sudo mkdir -p /etc/caddy/conf.d
  grep -q 'import /etc/caddy/conf.d/\*' /etc/caddy/Caddyfile 2>/dev/null \
    || echo 'import /etc/caddy/conf.d/*' | sudo tee -a /etc/caddy/Caddyfile >/dev/null
  sudo tee "/etc/caddy/conf.d/incentium-rpc-$NET.caddy" >/dev/null <<EOF
$RPC_DOMAIN {
	reverse_proxy 127.0.0.1:$RPC_PORT
}
EOF
  sudo systemctl reload caddy || sudo systemctl restart caddy
  sleep 5
  curl -fsS -H 'Content-Type: application/json' -d '{"id":1,"jsonrpc":"2.0","method":"system_health","params":[]}' \
    "https://$RPC_DOMAIN" >/dev/null && echo "https://$RPC_DOMAIN answers" \
    || echo "https://$RPC_DOMAIN does not answer yet: check that the domain points to this server and port 443 is open."
fi

say "6/7 watchdog"
if [ -z "$HC_URL" ]; then
  echo "skipped: no --healthchecks URL given (the watchdog reports to Healthchecks; run again with --healthchecks <url> to add it)"
else
[ -x "$PY" ] || { sudo apt-get install -y -qq python3 python3-venv >/dev/null; python3 -m venv "$D/venv"; "$PY" -m pip install -q substrate-interface; }
WD_ARGS="--url ws://127.0.0.1:$RPC_PORT"
[ -n "$HC_URL" ] && WD_ARGS="$WD_ARGS --ping-url $HC_URL"
[ -n "$RPC_DOMAIN" ] && WD_ARGS="$WD_ARGS --public-rpc https://$RPC_DOMAIN"
sudo tee /etc/systemd/system/incentium-watchdog-$NET.service >/dev/null <<EOF
[Unit]
Description=Incentium watchdog
After=$SERVICE.service

[Service]
User=$USER
ExecStart=$PY $SRC/scripts/node_watchdog.py $WD_ARGS
Restart=always
RestartSec=30

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now incentium-watchdog-$NET >/dev/null
echo "watchdog running${HC_URL:+, pinging $HC_URL}"
fi

say "7/7 what you sign in your own wallet"
cat <<EOF
The node runs as $SERVICE (logs: journalctl -fu $SERVICE).
Wallet: Talisman, SubWallet or polkadot.js, connected to $PUBLIC_RPC:
  https://polkadot.js.org/apps/?rpc=$PUBLIC_RPC#/extrinsics
EOF
if [ -n "$PRODUCER" ]; then cat <<EOF

Block producer, signed by the stash $PRODUCER (also at https://incentium.org/validators):
  1. staking.bond(value, payee)              at least 100 ICT
  2. session.setKeys(keys, proof)
       keys:  $KEYS
       proof: $PROOF
     (saved in $D/session-keys-$NET.txt)
  3. staking.validate({commission, blocked: false})
  The chain elects producers once a day.
EOF
fi
if [ "$SN0" = 1 ]; then cat <<EOF

SN0 validator, hotkey $SN0_ADDR (kept on this server in $HOTKEY):
  1. Register the hotkey in SN0 (registry.register; during the launch period the launch
     operator registers validators), then note its uid.
  2. Send ICT to the hotkey and stake it: consensus.stake(amount), signed by the hotkey;
     at least 1,000 ICT from day 90.
  3. Start the validator:  bash $0 $NET --sn0-validator --uid <uid>
     (or edit --uid in /etc/systemd/system/incentium-sn0-validator-$NET.service and run
      sudo systemctl enable --now incentium-sn0-validator-$NET)
EOF
fi
if [ -n "$RPC_DOMAIN" ] || [ "$ARCHIVE" = 1 ]; then
  KIND=$([ "$ARCHIVE" = 1 ] && echo Archive || echo Rpc)
  URL=$([ -n "$RPC_DOMAIN" ] && echo "wss://$RPC_DOMAIN" || echo "wss://<your domain>   (an archive node needs --rpc <domain> to be reachable)")
  cat <<EOF

Endpoint register (up to 5 % of issuance is shared by measured availability), signed by any
account of yours, with a deposit of 100 ICT held while the entry stands:
  endpoints.register(kind: $KIND, url: $URL, region: "<e.g. eu-west>")
EOF
fi
cat <<EOF

Bootnode (optional, unpaid): endpoints.register(kind: Bootnode,
  url: /dns/<your domain>/tcp/$P2P_PORT/p2p/$PEER_ID, region: "...")
EOF
