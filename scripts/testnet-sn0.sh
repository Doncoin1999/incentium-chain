#!/usr/bin/env bash
# Bootstrap SN0 on the testnet and run its validator and miner as services, next to the devnet ones.
#   bash scripts/testnet-sn0.sh
# Uses the shared llama-server (Qwen3.5-2B on 127.0.0.1:8080). Validator task server: port 8101.
set -euo pipefail
cd "$(dirname "$0")/.."
PY="$HOME/incentium-venv/bin/python"
S="$HOME/incentium-testnet/secrets"
PUBLIC_IP="${PUBLIC_IP:-85.190.108.120}"

OUT=$("$PY" scripts/testnet_sn0.py | tee /dev/stderr | tail -n 1)
VUID=$(echo "$OUT" | "$PY" -c "import json,sys; print(json.load(sys.stdin)['validator_uid'])")

unit() {
  local name=$1 desc=$2; shift 2
  sudo tee "/etc/systemd/system/incentium-testnet-$name.service" >/dev/null <<UNIT
[Unit]
Description=Incentium testnet $desc
After=incentium-testnet-v1.service qwen.service

[Service]
User=$USER
WorkingDirectory=$HOME/incentium-chain/neuron
Environment=PYTHONUNBUFFERED=1
ExecStart=$PY -m $*
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
UNIT
}
unit sn0-validator "SN0 validator" incentium_neuron.validator --url ws://127.0.0.1:9954 --key-file "$S/validator.json" \
  --uid "$VUID" --reference http://127.0.0.1:8080 --model qwen3.5-9b --epoch-length 360 --prompts 8 --max-tokens 32 \
  --listen 0.0.0.0:8101 --public-url "http://$PUBLIC_IP:8101" --state-file "$HOME/.incentium/validator-testnet-state.json"
unit sn0-miner "SN0 miner" incentium_neuron.miner --url ws://127.0.0.1:9954 --key-file "$S/miner.json" \
  --backend http://127.0.0.1:8080 --model qwen3.5-9b

sudo systemctl daemon-reload
sudo systemctl enable --now incentium-testnet-sn0-validator
sleep 30
sudo systemctl enable --now incentium-testnet-sn0-miner
echo "SN0 on the testnet is running. Logs: journalctl -u incentium-testnet-sn0-validator -f"
