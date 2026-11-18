#!/usr/bin/env bash
# The launch operator's mainnet SN0 miner, on a machine that already runs a tier-0 model
# (the old devnet/testnet server: llama-server with Qwen3.5-2B on 127.0.0.1:8080).
#
#   scp administrator@SERVER_A:~/incentium-mainnet/secrets/miner.json ~/incentium-mainnet-miner/
#   bash scripts/mainnet-miner.sh [RPC] [BACKEND] [MODEL_ALIAS]
#
# Defaults: wss://rpc.incentium.org, http://127.0.0.1:8080, qwen3.5-9b (the alias the old
# server's qwen service uses). The miner only makes outgoing connections.
set -euo pipefail
RPC="${1:-wss://rpc.incentium.org}"
BACKEND="${2:-http://127.0.0.1:8080}"
ALIAS="${3:-qwen3.5-9b}"
K="$HOME/incentium-mainnet-miner"
PY="$HOME/incentium-venv/bin/python"

[ -s "$K/miner.json" ] || { echo "missing $K/miner.json (copy it from server A)"; exit 1; }
chmod 700 "$K"; chmod 600 "$K/miner.json"
[ -x "$PY" ] || { echo "missing $PY"; exit 1; }
curl -fsS "$BACKEND/health" >/dev/null || { echo "no model at $BACKEND"; exit 1; }

sudo tee /etc/systemd/system/incentium-mainnet-miner.service >/dev/null <<EOF
[Unit]
Description=Incentium mainnet SN0 miner (launch operator)
After=network-online.target qwen.service

[Service]
User=$USER
WorkingDirectory=$HOME/incentium-chain/neuron
Environment=PYTHONUNBUFFERED=1
ExecStart=$PY -m incentium_neuron.miner --url $RPC --key-file $K/miner.json --backend $BACKEND --model $ALIAS
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now incentium-mainnet-miner
echo "Mainnet miner started. Logs: journalctl -u incentium-mainnet-miner -f"
