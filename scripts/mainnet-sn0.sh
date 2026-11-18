#!/usr/bin/env bash
# SN0 on mainnet, server A.
#
#   bash scripts/mainnet-sn0.sh prepare   # any time before launch: Python, llama.cpp, the tier-0
#                                         # model and the reference-model service (no chain needed)
#   bash scripts/mainnet-sn0.sh start     # after the chain runs: register and stake the operator's
#                                         # validator and miner, lock the tier-0 model hash, start
#                                         # the validator (task server on port 8100)
#
# The operator's miner runs on another machine: see mainnet-miner.sh.
set -euo pipefail
STEP="${1:?usage: mainnet-sn0.sh prepare|start}"
HERE="$(cd "$(dirname "$0")" && pwd)"
D="$HOME/incentium-mainnet"
S="$D/secrets"
PY="$HOME/incentium-venv/bin/python"
MODEL="$HOME/models/Qwen_Qwen3.5-2B-Q4_K_M.gguf"
MODEL_URL="https://huggingface.co/bartowski/Qwen_Qwen3.5-2B-GGUF/resolve/main/Qwen_Qwen3.5-2B-Q4_K_M.gguf"
MODEL_SHA="57a1085840f497d764a7fc5d346922dbde961efb54cc792ea81d694fd846a1d8"
LLAMA_DIR="$HOME/llama.cpp"
PUBLIC_IP="${PUBLIC_IP:-$(hostname -I | awk '{print $1}')}"

prepare() {
  echo "==> packages"
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3-venv build-essential cmake git curl >/dev/null
  echo "==> Python environment"
  [ -x "$PY" ] || python3 -m venv "$HOME/incentium-venv"
  "$PY" -m pip install -q --upgrade pip
  "$PY" -m pip install -q substrate-interface
  echo "==> llama.cpp (building llama-server; 5-15 minutes the first time)"
  if [ ! -x "$LLAMA_DIR/build/bin/llama-server" ]; then
    [ -d "$LLAMA_DIR" ] || git clone -q --depth 1 https://github.com/ggml-org/llama.cpp "$LLAMA_DIR"
    cmake -S "$LLAMA_DIR" -B "$LLAMA_DIR/build" -DCMAKE_BUILD_TYPE=Release -DLLAMA_CURL=OFF >/dev/null
    cmake --build "$LLAMA_DIR/build" --target llama-server -j "$(nproc)" >/dev/null
  fi
  echo "==> tier-0 model"
  mkdir -p "$(dirname "$MODEL")"
  if [ ! -s "$MODEL" ]; then
    curl -fL --progress-bar "$MODEL_URL" -o "$MODEL.part"
    mv "$MODEL.part" "$MODEL"
  fi
  [ "$(sha256sum "$MODEL" | cut -d' ' -f1)" = "$MODEL_SHA" ] || { echo "model file does not match $MODEL_SHA"; exit 1; }
  echo "==> reference-model service (127.0.0.1:8080)"
  # Leave one core for the chain nodes.
  local threads=$(( $(nproc) > 2 ? $(nproc) - 1 : 1 ))
  sudo tee /etc/systemd/system/incentium-qwen.service >/dev/null <<EOF
[Unit]
Description=Incentium tier-0 reference model (llama-server, Qwen3.5-2B)
After=network-online.target

[Service]
User=$USER
ExecStart=$LLAMA_DIR/build/bin/llama-server -m $MODEL --host 127.0.0.1 --port 8080 --alias qwen3.5-2b -np 2 -c 4096 -t $threads
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
  sudo systemctl daemon-reload
  sudo systemctl enable --now incentium-qwen
  sleep 20
  if curl -fsS http://127.0.0.1:8080/health >/dev/null; then echo "reference model is up"; else echo "reference model still loading; check: journalctl -u incentium-qwen -f"; fi
}

start() {
  [ -x "$PY" ] || { echo "run: bash scripts/mainnet-sn0.sh prepare"; exit 1; }
  OUT=$(cd "$HERE/.." && "$PY" scripts/mainnet_sn0.py --model-file "$MODEL" | tee /dev/stderr | tail -n 1)
  VUID=$(echo "$OUT" | "$PY" -c "import json,sys; print(json.load(sys.stdin)['validator_uid'])")
  mkdir -p "$HOME/.incentium"
  sudo tee /etc/systemd/system/incentium-sn0-validator.service >/dev/null <<EOF
[Unit]
Description=Incentium mainnet SN0 validator
After=incentium-v1.service incentium-qwen.service

[Service]
User=$USER
WorkingDirectory=$HOME/incentium-chain/neuron
Environment=PYTHONUNBUFFERED=1
ExecStart=$PY -m incentium_neuron.validator --url ws://127.0.0.1:9944 --key-file $S/validator.json --uid $VUID --reference http://127.0.0.1:8080 --model qwen3.5-2b --epoch-length 360 --prompts 8 --max-tokens 32 --listen 0.0.0.0:8100 --public-url http://$PUBLIC_IP:8100 --state-file $HOME/.incentium/validator-mainnet-state.json
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
  if command -v ufw >/dev/null && sudo ufw status | grep -q "Status: active"; then sudo ufw allow 8100/tcp >/dev/null; fi
  sudo systemctl daemon-reload
  sudo systemctl enable --now incentium-sn0-validator
  echo
  echo "Validator started (uid $VUID, task server http://$PUBLIC_IP:8100). Logs: journalctl -u incentium-sn0-validator -f"
  echo "Miner hotkey: $S/miner.json -> copy it to the miner machine and run scripts/mainnet-miner.sh there."
}

case "$STEP" in
  prepare) prepare ;;
  start) start ;;
  *) echo "usage: mainnet-sn0.sh prepare|start"; exit 1 ;;
esac
