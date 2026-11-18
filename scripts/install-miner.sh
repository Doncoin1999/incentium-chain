#!/usr/bin/env bash
# Incentium miner: one-command install on Linux (Ubuntu/Debian, x86_64 or arm64).
#
#   curl -sSL https://raw.githubusercontent.com/Doncoin1999/incentium-chain/main/scripts/install-miner.sh | bash
#
# What it does, all under ~/.incentium-miner:
#   1. Installs Python and llama.cpp's llama-server (prebuilt release, or built from source).
#   2. Downloads the tier-0 reference model (Qwen3.5-2B, Q4_K_M) and checks it against the
#      model hash on chain once that is locked.
#   3. Creates your miner key (BACK IT UP) and asks the launch operator's registration service
#      to register it in SN0 (free during the launch period).
#   4. Runs llama-server and the miner as systemd services that start on boot.
# The miner only makes outgoing connections: no open port, no fixed IP.
# Run it again at any time to update; your key and model are kept.
set -euo pipefail

RPC="${INCENTIUM_RPC:-wss://testnet-rpc.incentium.org}"
REGISTER="${INCENTIUM_REGISTER:-https://register.incentium.org}"
REPO="${INCENTIUM_REPO:-Doncoin1999/incentium-chain}"
MODEL_URL="${INCENTIUM_MODEL_URL:-https://huggingface.co/bartowski/Qwen_Qwen3.5-2B-GGUF/resolve/main/Qwen_Qwen3.5-2B-Q4_K_M.gguf}"
LLAMA_PORT="${INCENTIUM_LLAMA_PORT:-8095}"
D="$HOME/.incentium-miner"
KEY="$D/miner-key.json"
MODEL="$D/models/Qwen_Qwen3.5-2B-Q4_K_M.gguf"
PY="$D/venv/bin/python"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\n\033[31mError: %s\033[0m\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- checks
[ "$(uname -s)" = "Linux" ] || die "this installer is for Linux (Ubuntu/Debian)."
case "$(uname -m)" in
  x86_64) ARCH=x64 ;;
  aarch64|arm64) ARCH=arm64 ;;
  *) die "unsupported CPU: $(uname -m)" ;;
esac
MEM_GB=$(awk '/MemTotal/ {print int($2/1024/1024)}' /proc/meminfo)
[ "$MEM_GB" -ge 3 ] || die "at least 4 GB of RAM is needed (found ${MEM_GB} GB)."
mkdir -p "$D/models" "$D/src" "$D/llama"
FREE_GB=$(df -Pk "$D" | awk 'NR==2 {print int($4/1024/1024)}')
[ "$FREE_GB" -ge 3 ] || die "at least 3 GB of free disk is needed (found ${FREE_GB} GB)."
command -v sudo >/dev/null || die "sudo is needed to install packages and services."

# ---------------------------------------------------------------- packages
say "Installing system packages"
if command -v apt-get >/dev/null; then
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3 python3-venv curl unzip ca-certificates >/dev/null
else
  for c in python3 curl unzip; do command -v $c >/dev/null || die "please install $c first"; done
fi

# ---------------------------------------------------------------- miner code
say "Installing the Incentium miner"
[ -x "$PY" ] || python3 -m venv "$D/venv"
"$PY" -m pip install -q --upgrade pip
"$PY" -m pip install -q substrate-interface
rm -rf "$D/src/neuron"
curl -fsSL "https://github.com/$REPO/archive/refs/heads/main.tar.gz" \
  | tar xz -C "$D/src" --strip-components=1 --wildcards '*/neuron'
[ -d "$D/src/neuron/incentium_neuron" ] || die "could not download the miner code."

# ---------------------------------------------------------------- llama.cpp
LLAMA="$(find "$D/llama" -name llama-server -type f 2>/dev/null | head -n 1 || true)"
if [ -z "$LLAMA" ]; then
  say "Installing llama.cpp (llama-server)"
  ASSET=$(curl -fsSL https://api.github.com/repos/ggml-org/llama.cpp/releases/latest \
    | grep -o "https://[^\"]*bin-ubuntu-${ARCH}\.zip" | head -n 1 || true)
  if [ -n "$ASSET" ]; then
    curl -fsSL "$ASSET" -o "$D/llama/llama.zip"
    unzip -q -o "$D/llama/llama.zip" -d "$D/llama"
    rm -f "$D/llama/llama.zip"
  else
    say "No prebuilt llama.cpp for this machine: building it (takes 5-15 minutes)"
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential cmake git >/dev/null
    rm -rf "$D/llama/src"
    git clone -q --depth 1 https://github.com/ggml-org/llama.cpp "$D/llama/src"
    cmake -S "$D/llama/src" -B "$D/llama/build" -DCMAKE_BUILD_TYPE=Release -DLLAMA_CURL=OFF >/dev/null
    cmake --build "$D/llama/build" --target llama-server -j "$(nproc)" >/dev/null
  fi
  LLAMA="$(find "$D/llama" -name llama-server -type f | head -n 1)"
  [ -n "$LLAMA" ] || die "could not install llama-server."
  chmod +x "$LLAMA"
fi

# ---------------------------------------------------------------- model
if [ ! -s "$MODEL" ]; then
  say "Downloading the tier-0 model (about 1.3 GB)"
  curl -fL --progress-bar "$MODEL_URL" -o "$MODEL.part"
  mv "$MODEL.part" "$MODEL"
fi
TIER0=$(cd "$D/src/neuron" && "$PY" -m incentium_neuron.setup_miner tier0 --url "$RPC" 2>/dev/null || echo '{"model_hash": ""}')
WANT=$(echo "$TIER0" | "$PY" -c "import json,sys; print(json.load(sys.stdin)['model_hash'] or '')")
if [ -n "$WANT" ]; then
  say "Checking the model against the hash on chain"
  HAVE=$(sha256sum "$MODEL" | cut -d' ' -f1)
  [ "${WANT#0x}" = "$HAVE" ] || die "the model file does not match the reference model on chain. Delete $MODEL and run again."
fi

# ---------------------------------------------------------------- key and registration
say "Creating your miner key"
ADDR=$(cd "$D/src/neuron" && "$PY" -m incentium_neuron.setup_miner keygen "$KEY")
say "Registering $ADDR in SN0"
if ! (cd "$D/src/neuron" && "$PY" -m incentium_neuron.setup_miner register --key-file "$KEY" --service "$REGISTER"); then
  echo "Registration did not go through (see the message above). The miner is installed anyway;"
  echo "run this installer again later, or register the key on chain yourself."
fi

# ---------------------------------------------------------------- services
say "Starting llama-server and the miner as services"
sudo tee /etc/systemd/system/incentium-llama.service >/dev/null <<EOF
[Unit]
Description=Incentium tier-0 model (llama-server, Qwen3.5-2B)
After=network-online.target

[Service]
User=$USER
ExecStart=$LLAMA -m $MODEL --host 127.0.0.1 --port $LLAMA_PORT --alias qwen3.5-2b -np 2 -c 4096 -t $(nproc)
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
sudo tee /etc/systemd/system/incentium-miner.service >/dev/null <<EOF
[Unit]
Description=Incentium SN0 miner
After=incentium-llama.service network-online.target

[Service]
User=$USER
WorkingDirectory=$D/src/neuron
Environment=PYTHONUNBUFFERED=1
ExecStart=$PY -m incentium_neuron.miner --url $RPC --key-file $KEY --backend http://127.0.0.1:$LLAMA_PORT --model qwen3.5-2b
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now incentium-llama incentium-miner >/dev/null
sudo systemctl restart incentium-llama incentium-miner

cat <<EOF

$(printf '\033[1m')Incentium miner installed.$(printf '\033[0m')
  Miner address:  $ADDR
  Key file:       $KEY   <- BACK THIS UP. Whoever has it controls the miner's rewards.
  Logs:           journalctl -u incentium-miner -f
  Rewards:        https://polkadot.js.org/apps/?rpc=wss%3A%2F%2Ftestnet-rpc.incentium.org#/accounts

Launch-period rewards are locked on chain and released over 22 months after day 90.
EOF
