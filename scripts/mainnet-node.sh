#!/usr/bin/env bash
# Mainnet step 1, on EACH validator server: create this server's keys.
#
#   bash scripts/mainnet-node.sh a      # on server A
#   bash scripts/mainnet-node.sh b      # on server B
#
# Needs the normal-epoch node binary (CI artifact `incentium-node`) at ~/incentium-mainnet/incentium-node.
# Creates two validator identities (Aura + Grandpa keys) and three p2p node keys. The secrets stay on
# this server in ~/incentium-mainnet/secrets (mode 700): back that folder up off the server.
# Writes the PUBLIC half to ~/incentium-mainnet/public-ROLE.json for mainnet-genesis.sh.
# Safe to run again: keys that exist are never regenerated.
set -euo pipefail
ROLE="${1:?usage: mainnet-node.sh a|b}"
case "$ROLE" in a|b) ;; *) echo "role must be a or b"; exit 1 ;; esac
HERE="$(cd "$(dirname "$0")" && pwd)"
D="$HOME/incentium-mainnet"
S="$D/secrets"
BIN="$D/incentium-node"
PUBLIC_IP="${PUBLIC_IP:-$(hostname -I | awk '{print $1}')}"

[ -x "$BIN" ] || { echo "missing $BIN (CI artifact incentium-node, chmod +x)"; exit 1; }
mkdir -p "$D" "$S"; chmod 700 "$S"

gen() {
  local f="$S/$1.json"
  [ -s "$f" ] || { "$BIN" key generate --scheme "$2" --output-type json > "$f"; chmod 600 "$f"; }
}
for i in 1 2; do gen "aura$i" sr25519; gen "gran$i" ed25519; done
for n in v1 v2 rpc; do
  [ -s "$S/node-$n.key" ] || { "$BIN" key generate-node-key --file "$S/node-$n.key" 2>/dev/null; chmod 600 "$S/node-$n.key"; }
done
peer() { "$BIN" key inspect-node-key --file "$S/node-$1.key"; }

python3 "$HERE/mainnet_keys.py" public "$ROLE" "$S" "$PUBLIC_IP" "$(peer v1)" "$(peer v2)" "$(peer rpc)" "$D/public-$ROLE.json"
echo "server $ROLE ($PUBLIC_IP): keys ready"
cat "$D/public-$ROLE.json"
echo
echo "Secrets: $S  <- back this folder up off the server."
