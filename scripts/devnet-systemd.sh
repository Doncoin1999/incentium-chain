#!/usr/bin/env bash
# Install the closed 3-validator devnet (Alice, Bob, Charlie) as systemd services,
# so the nodes start on boot and restart if they crash.
# Assumes the node binary at ~/incentium-node and a chain spec made by scripts/devnet.sh
# at ~/incentium-devnet/devnet-raw.json. Run as the user that owns those files.
set -euo pipefail

BIN="$HOME/incentium-node"
DATA="$HOME/incentium-devnet"
K=00000000000000000000000000000000000000000000000000000000000000
BOOT=/ip4/127.0.0.1/tcp/30333/p2p/12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp

mk() {
  local name=$1; shift
  sudo tee "/etc/systemd/system/incentium-$name.service" >/dev/null <<EOF
[Unit]
Description=Incentium devnet validator $name
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
ExecStart=$BIN --chain $DATA/devnet-raw.json --base-path $DATA/$name --$name --validator $*
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
}

mk alice   --node-key ${K}01 --port 30333 --rpc-port 9944 --prometheus-port 9615
mk bob     --node-key ${K}02 --port 30334 --rpc-port 9945 --prometheus-port 9616 --bootnodes $BOOT
mk charlie --node-key ${K}03 --port 30335 --rpc-port 9946 --prometheus-port 9617 --bootnodes $BOOT

sudo systemctl daemon-reload
sudo systemctl enable --now incentium-alice incentium-bob incentium-charlie
echo "Installed. Logs: journalctl -u incentium-alice -f"
