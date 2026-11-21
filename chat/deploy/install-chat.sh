#!/usr/bin/env bash
# Install the chat.incentium.org gateway on server B (prototype: free tier, testnet miners).
#
#   cd ~/incentium-chain && git pull && bash chat/deploy/install-chat.sh
#   WITH_FALLBACK=1 bash chat/deploy/install-chat.sh   # also run Qwen3.5-2B here as a backup model
#
# Needs: DNS A record chat.incentium.org -> this server; Caddy with "import sites/*.caddy".
# Run it again after git pull to update; it restarts the gateway.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"            # .../incentium-chain/chat
D="$HOME/incentium-chat"
RPC="${INCENTIUM_RPC:-wss://testnet-rpc.incentium.org}"
PORT=8400
FB_PORT=8401
mkdir -p "$D"

echo "== python"
[ -x "$D/venv/bin/python" ] || python3 -m venv "$D/venv"
"$D/venv/bin/pip" install -q --upgrade pip
"$D/venv/bin/pip" install -q "substrate-interface>=1.7"

FALLBACK_ARGS=""
if [ "${WITH_FALLBACK:-0}" = 1 ]; then
  echo "== backup model (llama.cpp + Qwen3.5-2B)"
  case "$(uname -m)" in x86_64) ARCH=x64 ;; aarch64|arm64) ARCH=arm64 ;; *) echo "unsupported CPU"; exit 1 ;; esac
  if [ -z "$(find "$D/llama" -name llama-server -type f 2>/dev/null | head -n 1)" ]; then
    ASSET=$(curl -fsSL "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20" \
      | grep -Eo "https://[^\"]*bin-ubuntu-${ARCH}\.(tar\.gz|zip)" | head -n 1 || true)
    [ -n "$ASSET" ] || { echo "could not find a llama.cpp build for ubuntu-$ARCH"; exit 1; }
    rm -rf "$D/llama" && mkdir -p "$D/llama"
    case "$ASSET" in
      *.zip) curl -fsSL "$ASSET" -o "$D/llama/l.zip" && unzip -q -o "$D/llama/l.zip" -d "$D/llama" && rm -f "$D/llama/l.zip" ;;
      *)     curl -fsSL "$ASSET" | tar xz -C "$D/llama" ;;
    esac
  fi
  LLAMA="$(find "$D/llama" -name llama-server -type f | head -n 1)"
  MODEL="$D/Qwen_Qwen3.5-2B-Q4_K_M.gguf"
  URL="$("$D/venv/bin/python" -c "import sys; sys.path.insert(0, '$HERE/../neuron'); from incentium_neuron.hw import CATALOG; print(CATALOG[0]['url'])")"
  [ -s "$MODEL" ] || { curl -fL --progress-bar "$URL" -o "$MODEL.part" && mv "$MODEL.part" "$MODEL"; }
  THREADS=$(( $(nproc) > 2 ? $(nproc) - 1 : 1 ))
  sudo tee /etc/systemd/system/incentium-chat-llama.service >/dev/null <<UNIT
[Unit]
Description=Incentium chat backup model (Qwen3.5-2B)
After=network-online.target

[Service]
User=$USER
WorkingDirectory=$(dirname "$LLAMA")
ExecStart=$LLAMA -m $MODEL --host 127.0.0.1 --port $FB_PORT --alias qwen3.5-2b -c 8192 -np 2 -t $THREADS
Restart=always
RestartSec=5
Nice=5

[Install]
WantedBy=multi-user.target
UNIT
  sudo systemctl daemon-reload
  sudo systemctl enable --now incentium-chat-llama >/dev/null
  sudo systemctl restart incentium-chat-llama
  FALLBACK_ARGS="--fallback http://127.0.0.1:$FB_PORT --fallback-tier 0"
fi

echo "== gateway"
sudo tee /etc/systemd/system/incentium-chat.service >/dev/null <<UNIT
[Unit]
Description=Incentium chat gateway (chat.incentium.org)
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
ExecStart=$D/venv/bin/python $HERE/gateway.py --rpc $RPC --port $PORT $FALLBACK_ARGS
Restart=always
RestartSec=5
Environment=PYTHONUNBUFFERED=1

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable --now incentium-chat >/dev/null
sudo systemctl restart incentium-chat

echo "== caddy"
sudo mkdir -p /etc/caddy/sites
sudo grep -q "import sites/\*.caddy" /etc/caddy/Caddyfile || echo "import sites/*.caddy" | sudo tee -a /etc/caddy/Caddyfile >/dev/null
# No compression and no buffering: answers stream to the browser word by word.
sudo tee /etc/caddy/sites/chat.caddy >/dev/null <<CADDY
chat.incentium.org {
	reverse_proxy 127.0.0.1:$PORT {
		flush_interval -1
	}
}
CADDY
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
sudo systemctl reload caddy

sleep 2
curl -fsS -o /dev/null -w "local check: HTTP %{http_code}\n" "http://127.0.0.1:$PORT/healthz"
echo "OK: https://chat.incentium.org (DNS for chat.incentium.org must point to this server)"
echo "Logs: journalctl -u incentium-chat -f"
