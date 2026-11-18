#!/usr/bin/env bash
# Mainnet step 3, on EACH validator server: load the keys and start the nodes as services.
# Starting server A produces block 1, which starts the 90-day launch period.
#
#   bash scripts/mainnet-start.sh a      # on server A (2 validators + public archive RPC)
#   bash scripts/mainnet-start.sh b      # on server B (2 validators + reserve RPC)
#
# Needs ~/incentium-mainnet/incentium-mainnet-raw.json (from mainnet-genesis.sh; copy it to B).
# Ports: p2p 30333-30335 (open to the internet), RPC 9944-9946 (local only; Caddy fronts 9946).
set -euo pipefail
ROLE="${1:?usage: mainnet-start.sh a|b}"
case "$ROLE" in a|b) ;; *) echo "role must be a or b"; exit 1 ;; esac
D="$HOME/incentium-mainnet"
S="$D/secrets"
BIN="$D/incentium-node"
SPEC="$D/incentium-mainnet-raw.json"
PUBLIC_IP="${PUBLIC_IP:-$(hostname -I | awk '{print $1}')}"

[ -x "$BIN" ] || { echo "missing $BIN"; exit 1; }
[ -s "$SPEC" ] || { echo "missing $SPEC"; exit 1; }
field() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$S/$1.json" "$2"; }
echo "chain spec sha256: $(sha256sum "$SPEC" | cut -d' ' -f1)  (must be the same on both servers)"

for i in 1 2; do
  ks="$D/v$i/chains/incentium/keystore"
  if [ -z "$(ls -A "$ks" 2>/dev/null)" ]; then
    "$BIN" key insert --chain "$SPEC" --base-path "$D/v$i" --scheme sr25519 --key-type aura --suri "$(field "aura$i" secretPhrase)"
    "$BIN" key insert --chain "$SPEC" --base-path "$D/v$i" --scheme ed25519 --key-type gran --suri "$(field "gran$i" secretPhrase)"
  fi
done

unit() {  # unit <name> <description> <args...>
  local name=$1 desc=$2; shift 2
  sudo tee "/etc/systemd/system/incentium-$name.service" >/dev/null <<EOF
[Unit]
Description=Incentium mainnet $desc
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
ExecStart=$BIN --chain $SPEC --base-path $D/$name --name incentium-$ROLE-$name $*
Restart=always
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
}
unit v1 "validator 1" --validator --node-key-file "$S/node-v1.key" --port 30333 --rpc-port 9944 \
  --prometheus-port 9615 --public-addr "/ip4/$PUBLIC_IP/tcp/30333"
unit v2 "validator 2" --validator --node-key-file "$S/node-v2.key" --port 30334 --rpc-port 9945 \
  --prometheus-port 9616 --public-addr "/ip4/$PUBLIC_IP/tcp/30334"
if [ "$ROLE" = a ]; then
  unit rpc "public RPC node (safe methods, archive)" --node-key-file "$S/node-rpc.key" --port 30335 \
    --rpc-port 9946 --prometheus-port 9617 --rpc-methods safe --rpc-cors all --rpc-max-connections 500 \
    --state-pruning archive --blocks-pruning archive --public-addr "/ip4/$PUBLIC_IP/tcp/30335"
else
  unit rpc "reserve RPC node (safe methods)" --node-key-file "$S/node-rpc.key" --port 30335 \
    --rpc-port 9946 --prometheus-port 9617 --rpc-methods safe --rpc-cors all --rpc-max-connections 500 \
    --public-addr "/ip4/$PUBLIC_IP/tcp/30335"
fi

if command -v ufw >/dev/null && sudo ufw status | grep -q "Status: active"; then
  sudo ufw allow 30333:30335/tcp >/dev/null
fi
sudo systemctl daemon-reload
sudo systemctl enable --now incentium-v1 incentium-v2 incentium-rpc

echo
echo "Server $ROLE started. Logs: journalctl -u incentium-v1 -f"
echo "Check: curl -s -H 'Content-Type: application/json' -d '{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"system_health\"}' http://127.0.0.1:9944"
