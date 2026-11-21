#!/usr/bin/env bash
# Install the Incentium sign-in service (auth.incentium.org) on server B and put trade-test behind it.
#
#   cd ~/incentium-chain && git pull && bash auth/deploy/install-auth.sh
#
# Needs: DNS A record auth.incentium.org -> this server; Caddy installed. Standard-library Python only.
# Afterwards create your admin account once:
#   python3 ~/incentium-chain/auth/auth_service.py add-admin you@example.org
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"          # .../incentium-chain/auth
D="$HOME/incentium-auth"
mkdir -p "$D" && chmod 700 "$D"

echo "== service"
sudo tee /etc/systemd/system/incentium-auth.service >/dev/null <<UNIT
[Unit]
Description=Incentium sign-in (auth.incentium.org)
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
ExecStart=/usr/bin/python3 $HERE/auth_service.py serve --db $D/auth.db --port 8300
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable --now incentium-auth >/dev/null
sudo systemctl restart incentium-auth

echo "== caddy"
sudo mkdir -p /etc/caddy/sites
sudo grep -q "import sites/\*.caddy" /etc/caddy/Caddyfile || echo "import sites/*.caddy" | sudo tee -a /etc/caddy/Caddyfile >/dev/null
sudo tee /etc/caddy/sites/00-auth.caddy >/dev/null <<'CADDY'
(incentium_auth) {
	forward_auth 127.0.0.1:8300 {
		uri /verify
		header_up X-Incentium-Host {host}
	}
}

auth.incentium.org {
	encode gzip
	reverse_proxy 127.0.0.1:8300
}
CADDY
# the trading page and the explorer on this server go behind sign-in
for f in /etc/caddy/sites/swap.caddy /etc/caddy/sites/explorer.caddy; do
  [ -f "$f" ] && sudo python3 "$HERE/deploy/protect-caddy.py" "$f" --all --no-snippet
done
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
sudo systemctl reload caddy

sleep 2
curl -fsS -o /dev/null -w "local check: HTTP %{http_code}\n" http://127.0.0.1:8300/login
if ! python3 "$HERE/auth_service.py" list --db "$D/auth.db" | grep -q admin; then
  echo "No admin yet. Create yours:  python3 $HERE/auth_service.py add-admin you@example.org"
fi
echo "OK: https://auth.incentium.org (DNS for auth.incentium.org must point to this server)"
