#!/usr/bin/env bash
# Install or update the Incentium explorer on server B (Ubuntu, user administrator).
#
#   cd ~/incentium-chain && git pull && bash explorer/deploy/install-explorer.sh
#
# Optional environment:
#   RPC=wss://rpc.incentium.org     archive RPC the indexer reads (B's own RPC node is pruned)
#   PING_URL=https://hc-ping.com/…  healthchecks.io check for the indexer
#   DOMAIN=explorer.incentium.org
#
# Sets up:
#   ~/incentium-explorer/{venv,explorer.db,labels.json}
#   systemd: incentium-explorer-indexer, incentium-explorer-api (127.0.0.1:8090)
#   Caddy:   https://$DOMAIN -> page from /var/www/incentium-explorer, /api/* -> 127.0.0.1:8090
# Safe to run again: it updates the code and the page and restarts the services.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"          # .../incentium-chain/explorer
D="$HOME/incentium-explorer"
RPC="${RPC:-wss://rpc.incentium.org}"
DOMAIN="${DOMAIN:-explorer.incentium.org}"
PING_URL="${PING_URL:-}"
WEB=/var/www/incentium-explorer

echo "== packages"
sudo apt-get update -qq
sudo apt-get install -y -qq python3-venv rsync curl gnupg >/dev/null
if ! command -v caddy >/dev/null; then
  sudo apt-get install -y -qq debian-keyring debian-archive-keyring apt-transport-https >/dev/null
  curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/gpg.key | sudo gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt | sudo tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
  sudo apt-get update -qq && sudo apt-get install -y -qq caddy >/dev/null
fi

echo "== python environment"
mkdir -p "$D"
[ -x "$D/venv/bin/python" ] || python3 -m venv "$D/venv"
"$D/venv/bin/pip" install -q --upgrade pip
"$D/venv/bin/pip" install -q -r "$HERE/requirements.txt"
[ -s "$D/labels.json" ] || cp "$HERE/deploy/labels.example.json" "$D/labels.json"

echo "== page"
sudo mkdir -p "$WEB"
sudo rsync -a --delete "$HERE/web/" "$WEB/"
sudo chmod -R a+rX "$WEB"

echo "== services"
PING_ARG=""; [ -n "$PING_URL" ] && PING_ARG="--ping-url $PING_URL"
sudo tee /etc/systemd/system/incentium-explorer-indexer.service >/dev/null <<EOF
[Unit]
Description=Incentium explorer indexer
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
WorkingDirectory=$HERE
ExecStart=$D/venv/bin/python -m indexer.indexer --url $RPC --db $D/explorer.db --labels $D/labels.json $PING_ARG
Restart=always
RestartSec=10
Nice=5

[Install]
WantedBy=multi-user.target
EOF
sudo tee /etc/systemd/system/incentium-explorer-api.service >/dev/null <<EOF
[Unit]
Description=Incentium explorer API
After=network-online.target

[Service]
User=$USER
WorkingDirectory=$HERE
ExecStart=$D/venv/bin/python -m indexer.api --db $D/explorer.db --port 8090 --web $WEB
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable incentium-explorer-indexer incentium-explorer-api >/dev/null
sudo systemctl restart incentium-explorer-indexer
sleep 3   # the indexer creates the database before the API opens it read-only
sudo systemctl restart incentium-explorer-api

echo "== caddy"
sudo mkdir -p /etc/caddy/sites
sudo tee /etc/caddy/sites/explorer.caddy >/dev/null <<EOF
$DOMAIN {
	encode gzip
	header {
		X-Content-Type-Options nosniff
		Referrer-Policy strict-origin-when-cross-origin
		X-Frame-Options DENY
	}
	handle /api/* {
		reverse_proxy 127.0.0.1:8090
	}
	handle {
		root * $WEB
		file_server
	}
}
EOF
# keep the explorer behind Incentium sign-in when the sign-in service is installed (auth/deploy/install-auth.sh)
if [ -f /etc/caddy/sites/00-auth.caddy ]; then
  sudo python3 "$(cd "$(dirname "$0")/../.." && pwd)/auth/deploy/protect-caddy.py" /etc/caddy/sites/explorer.caddy --all --no-snippet >/dev/null
fi
if ! sudo grep -q "import sites/\*.caddy" /etc/caddy/Caddyfile 2>/dev/null; then
  # A fresh Caddy install ships a demo site on :80; replace it, keep anything else.
  if sudo grep -q "^:80" /etc/caddy/Caddyfile 2>/dev/null; then
    echo "import sites/*.caddy" | sudo tee /etc/caddy/Caddyfile >/dev/null
  else
    echo "import sites/*.caddy" | sudo tee -a /etc/caddy/Caddyfile >/dev/null
  fi
fi
sudo caddy validate --config /etc/caddy/Caddyfile >/dev/null
sudo systemctl enable caddy >/dev/null
sudo systemctl reload caddy || sudo systemctl restart caddy
if command -v ufw >/dev/null && sudo ufw status | grep -q "Status: active"; then
  sudo ufw allow 80/tcp >/dev/null; sudo ufw allow 443/tcp >/dev/null
fi

echo
echo "Explorer installed."
echo "  Follow the indexer:   journalctl -fu incentium-explorer-indexer"
echo "  Preview before DNS:   ssh -L 8090:127.0.0.1:8090 administrator@<B> , then open http://localhost:8090"
echo "  Go live:              point the A record of $DOMAIN to this server ($(hostname -I | awk '{print $1}')) at Njalla;"
echo "                        Caddy gets the certificate by itself once DNS points here."
