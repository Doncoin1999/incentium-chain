#!/usr/bin/env bash
# Install or update the Incentium Swap keeper and trading page on server B.
#
#   cd ~/incentium-chain && git checkout swap && git pull && bash swap/service/deploy/install-swap.sh
#
# Before the first run:
#   1. The runtime with the Swap pallet must be live on the chain the keeper reads (testnet first).
#   2. The IncentiumSwap contract must be deployed on Base (Base Sepolia first); put its address,
#      the block it was deployed in (base_start) and genesis_time in ~/incentium-swap/config.json.
#   3. Put the keeper keys in ~/incentium-swap/incentium-keeper.seed and base-keeper.key (chmod 600).
#      They only pay fees; keep a small balance (a few ICT, a little ETH on Base).
#
# Optional environment:
#   DOMAIN=trade.incentium.org   the public name of the trading page
#   NAME=incentium-swap          instance name: folder ~/$NAME, service $NAME, page /var/www/$NAME.
#                                Use a second name to run mainnet and testnet on one server, e.g.
#                                NAME=incentium-swap-mainnet DOMAIN=trade.incentium.org (and "port": 8096 in its config.json)
# Sets up: ~/$NAME/{venv,swap.db,config.json}, systemd $NAME (127.0.0.1:<port from config.json>),
#          Caddy https://$DOMAIN -> page from /var/www/$NAME, /api/* -> the keeper
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"          # .../incentium-chain/swap/service
WEBSRC="$(cd "$HERE/../web" && pwd)"
NAME="${NAME:-incentium-swap}"
D="$HOME/$NAME"
DOMAIN="${DOMAIN:-trade.incentium.org}"
WEB="/var/www/$NAME"
CADDYFILE=/etc/caddy/sites/swap.caddy
[ "$NAME" = incentium-swap ] || CADDYFILE="/etc/caddy/sites/$NAME.caddy"

mkdir -p "$D" && chmod 700 "$D"
if [ ! -s "$D/config.json" ]; then
  TEMPLATE="$HERE/deploy/config.example.json"                      # testnet: Base Sepolia
  case "$NAME" in *mainnet*) TEMPLATE="$HERE/deploy/config.mainnet.example.json" ;; esac   # Base mainnet, port 8096
  # the templates name their folder /home/administrator/incentium-swap[-mainnet]: point it at $D, once
  python3 - "$TEMPLATE" "$D" "$HOME" > "$D/config.json" <<'PY'
import re, sys
t, d, home = sys.argv[1:]
s = re.sub(r"/home/administrator/incentium-swap(-mainnet)?(?=/)", d, open(t).read())
print(s.replace("/home/administrator", home), end="")
PY
  echo "Created $D/config.json. Fill in contract, base_start and genesis_time, add the key files, then run this again."
  exit 0
fi
python3 - "$D/config.json" <<'PY'
import json, os, sys
c = json.load(open(sys.argv[1]))
bad = [k for k in ("contract",) if int(c.get(k, "0x0"), 16) == 0] + [k for k in ("genesis_time",) if not c.get(k)]
bad += [f for f in (c.get("incentium_keeper_seed_file"), c.get("base_keeper_key_file")) if f and not os.path.exists(f)]
if bad: sys.exit("config.json is not complete yet: " + ", ".join(map(str, bad)))
PY
chmod 600 "$D"/*.seed "$D"/*.key 2>/dev/null || true
PORT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("port", 8095))' "$D/config.json")"

echo "== python environment"
sudo apt-get install -y -qq python3-venv rsync >/dev/null
[ -x "$D/venv/bin/python" ] || python3 -m venv "$D/venv"
"$D/venv/bin/pip" install -q --upgrade pip
"$D/venv/bin/pip" install -q -r "$HERE/requirements.txt"

echo "== page"
sudo mkdir -p "$WEB"
sudo rsync -a --delete "$WEBSRC/" "$WEB/"
sudo chmod -R a+rX "$WEB"

echo "== service"
sudo tee "/etc/systemd/system/$NAME.service" >/dev/null <<UNIT
[Unit]
Description=Incentium Swap keeper and API ($NAME, $DOMAIN)
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
WorkingDirectory=$HERE
ExecStart=$D/venv/bin/python -m swapsvc.keeper --config $D/config.json
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable --now "$NAME" >/dev/null
sudo systemctl restart "$NAME"

echo "== caddy"
sudo mkdir -p /etc/caddy/sites
sudo grep -q "import sites/\*.caddy" /etc/caddy/Caddyfile || echo "import sites/*.caddy" | sudo tee -a /etc/caddy/Caddyfile >/dev/null
sudo tee "$CADDYFILE" >/dev/null <<CADDY
$DOMAIN {
	encode gzip
	handle /api/* {
		reverse_proxy 127.0.0.1:$PORT
	}
	handle {
		root * $WEB
		file_server
	}
	header {
		X-Content-Type-Options nosniff
		Referrer-Policy no-referrer
		X-Frame-Options DENY
	}
}
CADDY
# keep the page behind Incentium sign-in when the sign-in service is installed (auth/deploy/install-auth.sh)
if [ -f /etc/caddy/sites/00-auth.caddy ]; then
  sudo python3 "$HERE/../../auth/deploy/protect-caddy.py" "$CADDYFILE" --all --no-snippet >/dev/null
fi
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
sudo systemctl reload caddy

sleep 3
curl -fsS "http://127.0.0.1:$PORT/api/status" && echo && echo "OK: https://$DOMAIN (DNS for $DOMAIN must point to this server)"
