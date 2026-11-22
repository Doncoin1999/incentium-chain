#!/usr/bin/env bash
# Put the mining control board on control.incentium.org, on the server that runs the SN0 validators.
#
#   cd ~/incentium-chain && git pull && bash scripts/install-mining-control.sh
#   TESTNET_START=2026-11-22 bash scripts/install-mining-control.sh   # count the 90 days from this date
#
# Needs: DNS A record control.incentium.org -> this server; Caddy; the neuron venv; auth.incentium.org
# with /verify-admin (only admin accounts get in). Networks are added when their SN0 validator
# service exists on this server. The validators are restarted so they read the switch file.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"
PY="${PY:-$HOME/incentium-venv/bin/python}"
[ -x "$PY" ] || { echo "no python at $PY (set PY=...)"; exit 1; }

grep -q "blocked_now" neuron/incentium_neuron/validator.py || {
  echo "This checkout ($(git rev-parse --abbrev-ref HEAD)) has no switch support in the validator."
  echo "The validators run from $REPO: update it to main first (git status; git pull origin main)."; exit 1; }

NETS=() VALIDATORS=()
if systemctl cat incentium-testnet-sn0-validator >/dev/null 2>&1; then
  NETS+=(--net "testnet=ws://127.0.0.1:9954=$HOME/.incentium/validator-testnet-state.json=${TESTNET_START:-}")
  VALIDATORS+=(incentium-testnet-sn0-validator)
fi
if systemctl cat incentium-sn0-validator >/dev/null 2>&1; then
  NETS+=(--net "mainnet=ws://127.0.0.1:9944=$HOME/.incentium/validator-mainnet-state.json=${MAINNET_START:-}")
  VALIDATORS+=(incentium-sn0-validator)
fi
[ ${#NETS[@]} -gt 0 ] || { echo "No SN0 validator service on this server (incentium-testnet-sn0-validator / incentium-sn0-validator)."; exit 1; }

echo "== service"
sudo tee /etc/systemd/system/incentium-mining-control.service >/dev/null <<UNIT
[Unit]
Description=Incentium mining control board (control.incentium.org)
After=network-online.target

[Service]
User=$USER
WorkingDirectory=$REPO
Environment=PYTHONUNBUFFERED=1
ExecStart=$PY $REPO/scripts/mining_control.py ${NETS[*]} --listen 127.0.0.1:8500
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable --now incentium-mining-control >/dev/null
sudo systemctl restart incentium-mining-control

echo "== caddy"
sudo mkdir -p /etc/caddy/sites
sudo grep -q "import sites/\*.caddy" /etc/caddy/Caddyfile || echo "import sites/*.caddy" | sudo tee -a /etc/caddy/Caddyfile >/dev/null
sudo tee /etc/caddy/sites/mining-control.caddy >/dev/null <<'CADDY'
control.incentium.org {
	encode gzip
	forward_auth https://auth.incentium.org {
		uri /verify-admin
		header_up Host {upstream_hostport}
		header_up X-Incentium-Host {host}
		copy_headers X-Incentium-User
	}
	reverse_proxy 127.0.0.1:8500
}
CADDY
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
sudo systemctl reload caddy

echo "== validators (they now read the switch file every epoch)"
for v in "${VALIDATORS[@]}"; do sudo systemctl restart "$v"; echo "restarted $v"; done

sleep 3
curl -fsS http://127.0.0.1:8500/api/state > /tmp/mining-control-state.json
"$PY" - /tmp/mining-control-state.json <<'PYEOF'
import json, sys, time
for n in json.load(open(sys.argv[1]))["networks"]:
    if n.get("error") and not n.get("miners"):
        print(n["name"] + ": " + n["error"])
        continue
    miners = sum(1 for m in n["miners"] if not m["validator"])
    until = time.strftime("%Y-%m-%d", time.gmtime(n["until"]))
    print(f"{n['name']}: {miners} miners, switch open until {until} ({n['days_left']} days left)")
PYEOF
echo "OK: https://control.incentium.org (sign in with your admin account)"
