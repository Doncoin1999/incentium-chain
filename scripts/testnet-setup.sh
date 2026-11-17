#!/usr/bin/env bash
# Set up the Incentium testnet on this server, next to the devnet: a dress rehearsal of the
# mainnet launch (bootstrap template, 72-minute epochs, 3 validator nodes, real keys).
#
#   bash scripts/testnet-setup.sh
#
# Needs the normal-epoch node binary at ~/incentium-node-testnet (CI artifact `incentium-node`).
# Safe to run again: existing keys, chain spec and keystores are kept, never regenerated.
# Secrets are written to ~/incentium-testnet/secrets (mode 700). Back that folder up.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="$HOME/incentium-node-testnet"
D="$HOME/incentium-testnet"
S="$D/secrets"
PY="$HOME/incentium-venv/bin/python"
PUBLIC_IP="${PUBLIC_IP:-85.190.108.120}"
FAUCET_ICT="${FAUCET_ICT:-100000}"
SPEC="$D/incentium-testnet-raw.json"

[ -x "$BIN" ] || { echo "missing $BIN (download the CI artifact incentium-node)"; exit 1; }
mkdir -p "$D" "$S"; chmod 700 "$S"

gen() {  # gen <name> <scheme>: generate a key once, keep it as JSON in secrets/
  local f="$S/$1.json"
  [ -s "$f" ] || { "$BIN" key generate --scheme "$2" --output-type json > "$f"; chmod 600 "$f"; }
}
field() { "$PY" -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$S/$1.json" "$2"; }

gen operator sr25519
gen faucet sr25519
for i in 1 2 3; do gen "aura$i" sr25519; gen "gran$i" ed25519; done
for n in v1 v2 v3 rpc; do
  [ -s "$S/node-$n.key" ] || { "$BIN" key generate-node-key --file "$S/node-$n.key" 2>/dev/null; chmod 600 "$S/node-$n.key"; }
done
peer() { "$BIN" key inspect-node-key --file "$S/node-$1.key"; }
P1=$(peer v1); PR=$(peer rpc)

if [ ! -s "$SPEC" ]; then
  "$PY" - "$S" "$D/keys.json" "$PUBLIC_IP" "$P1" "$PR" "$FAUCET_ICT" <<'EOF'
import json, sys
s, out, ip, p1, pr, faucet = sys.argv[1:]
f = lambda n: json.load(open(f"{s}/{n}.json"))["ss58Address"]
json.dump({
    "operator": f("operator"),
    "authorities": [{"aura": f(f"aura{i}"), "grandpa": f(f"gran{i}")} for i in (1, 2, 3)],
    "faucet": {"address": f("faucet"), "ict": int(faucet)},
    "bootnodes": [f"/ip4/{ip}/tcp/30433/p2p/{p1}", f"/ip4/{ip}/tcp/30436/p2p/{pr}"],
}, open(out, "w"), indent=2)
EOF
  python3 scripts/make_bootstrap_spec.py "$D/keys.json" --node "$BIN" --out "$D/incentium-testnet" \
    --name "Incentium Testnet" --id incentium_testnet --protocol-id ict-testnet
fi

for i in 1 2 3; do
  ks="$D/v$i/chains/incentium_testnet/keystore"
  if [ -z "$(ls -A "$ks" 2>/dev/null)" ]; then
    "$BIN" key insert --chain "$SPEC" --base-path "$D/v$i" --scheme sr25519 --key-type aura --suri "$(field "aura$i" secretPhrase)"
    "$BIN" key insert --chain "$SPEC" --base-path "$D/v$i" --scheme ed25519 --key-type gran --suri "$(field "gran$i" secretPhrase)"
  fi
done

unit() {  # unit <name> <description> <args...>
  local name=$1 desc=$2; shift 2
  sudo tee "/etc/systemd/system/incentium-testnet-$name.service" >/dev/null <<EOF
[Unit]
Description=Incentium testnet $desc
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
ExecStart=$BIN --chain $SPEC --base-path $D/$name --name incentium-testnet-$name $*
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
}
BOOT="/ip4/127.0.0.1/tcp/30433/p2p/$P1"
unit v1 "validator 1" --validator --node-key-file "$S/node-v1.key" --port 30433 --rpc-port 9954 --prometheus-port 9625 \
  --public-addr "/ip4/$PUBLIC_IP/tcp/30433"
unit v2 "validator 2" --validator --node-key-file "$S/node-v2.key" --port 30434 --rpc-port 9955 --prometheus-port 9626 --bootnodes "$BOOT"
unit v3 "validator 3" --validator --node-key-file "$S/node-v3.key" --port 30435 --rpc-port 9956 --prometheus-port 9627 --bootnodes "$BOOT"
unit rpc "public RPC node (safe methods, archive)" --node-key-file "$S/node-rpc.key" --port 30436 --rpc-port 9960 \
  --prometheus-port 9628 --rpc-methods safe --rpc-cors all --rpc-max-connections 500 \
  --state-pruning archive --blocks-pruning archive --bootnodes "$BOOT" --public-addr "/ip4/$PUBLIC_IP/tcp/30436"

sudo systemctl daemon-reload
sudo systemctl enable --now incentium-testnet-v1 incentium-testnet-v2 incentium-testnet-v3 incentium-testnet-rpc

echo
echo "Testnet started. Operator (sudo, bootstrap operator): $(field operator ss58Address)"
echo "Faucet account ($FAUCET_ICT test ICT at genesis):       $(field faucet ss58Address)"
echo "Bootnode: /ip4/$PUBLIC_IP/tcp/30433/p2p/$P1"
echo "Secrets: $S  <- back this folder up somewhere safe, off this server."
echo "Logs: journalctl -u incentium-testnet-v1 -f"
