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
# Optional environment: DOMAIN=trade.incentium.org
# Sets up: ~/incentium-swap/{venv,swap.db,config.json}, systemd incentium-swap (127.0.0.1:8095),
#          Caddy https://$DOMAIN -> page from /var/www/incentium-swap, /api/* -> 127.0.0.1:8095
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"          # .../incentium-chain/swap/service
WEBSRC="$(cd "$HERE/../web" && pwd)"
D="$HOME/incentium-swap"
DOMAIN="${DOMAIN:-trade.incentium.org}"
WEB=/var/www/incentium-swap

mkdir -p "$D" && chmod 700 "$D"
if [ ! -s "$D/config.json" ]; then
  sed "s#/home/administrator#$HOME#g" "$HERE/deploy/config.example.json" > "$D/config.json"
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
sudo tee /etc/systemd/system/incentium-swap.service >/dev/null <<UNIT
[Unit]
Description=Incentium Swap keeper and API
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
sudo systemctl enable --now incentium-swap >/dev/null
sudo systemctl restart incentium-swap

echo "== caddy"
sudo mkdir -p /etc/caddy/sites
sudo grep -q "import sites/\*.caddy" /etc/caddy/Caddyfile || echo "import sites/*.caddy" | sudo tee -a /etc/caddy/Caddyfile >/dev/null
sudo tee /etc/caddy/sites/swap.caddy >/dev/null <<CADDY
$DOMAIN {
	encode gzip
	handle /api/* {
		reverse_proxy 127.0.0.1:8095
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
  sudo python3 "$HERE/../../auth/deploy/protect-caddy.py" /etc/caddy/sites/swap.caddy --all --no-snippet >/dev/null
fi
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
sudo systemctl reload caddy

sleep 3
curl -fsS http://127.0.0.1:8095/api/status && echo && echo "OK: https://$DOMAIN (DNS for $DOMAIN must point to this server)"
