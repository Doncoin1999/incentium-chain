#!/usr/bin/env bash
# Incentium Node for Mac: one command, no administrator password, nothing from the App Store.
#
#   curl -fsSL https://git.incentium.org/incentium/incentium-chain/raw/branch/main/scripts/install-node.sh | bash
#
# Everything goes into ~/.incentium-node:
#   * Python (through uv) and llama.cpp's llama-server (Metal on Apple Silicon),
#   * the Incentium node, which picks the Qwen3.5 model this Mac can run, downloads it,
#     asks to join the network and mines; it starts by itself when you log in,
#   * a dashboard at http://127.0.0.1:7777, opened now and from the "Incentium" icon on the desktop.
# Run the same command again to update; your key, model and settings are kept.
# Remove:  curl -fsSL https://git.incentium.org/incentium/incentium-chain/raw/branch/main/scripts/install-node.sh | bash -s -- --uninstall
set -euo pipefail

REPO="${INCENTIUM_REPO:-incentium/incentium-chain}"
BRANCH="${INCENTIUM_BRANCH:-main}"
NETWORK="${INCENTIUM_NETWORK:-testnet}"
D="$HOME/.incentium-node"
LABEL="org.incentium.node"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
PORT=7777

say()  { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die()  { printf '\n\033[31mSomething went wrong: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "this installer is for Mac. On Linux, use scripts/install-miner.sh."

# ---------------------------------------------------------------- uninstall
if [ "${1:-}" = "--uninstall" ]; then
  launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
  rm -f "$PLIST" "$HOME/Desktop/Incentium.webloc"
  if [ -f "$D/miner-key.json" ]; then
    cp "$D/miner-key.json" "$HOME/incentium-miner-key-backup.json"
    echo "Your miner key was copied to ~/incentium-miner-key-backup.json. Keep it if you want your rewards."
  fi
  rm -rf "$D"
  echo "Incentium Node is removed."
  exit 0
fi

case "$(uname -m)" in
  arm64)  PLAT=macos-arm64 ;;
  x86_64) PLAT=macos-x64 ;;
  *) die "unsupported processor: $(uname -m)" ;;
esac
MEM_GB=$(( $(sysctl -n hw.memsize) / 1073741824 ))
[ "$MEM_GB" -ge 8 ] || die "at least 8 GB of memory is needed, so that mining never takes more than a quarter of it (this Mac has ${MEM_GB} GB)."
mkdir -p "$D"/{bin,models,llama,src} "$HOME/Library/LaunchAgents"
FREE_GB=$(df -Pk "$D" | awk 'NR==2 {print int($4/1024/1024)}')
[ "$FREE_GB" -ge 3 ] || die "at least 3 GB of free disk is needed (found ${FREE_GB} GB)."

echo "Installing Incentium Node in $D. This takes a few minutes."

# ---------------------------------------------------------------- Python (uv, no admin, no Xcode)
say "Preparing Python"
export UV_INSTALL_DIR="$D/bin" UV_PYTHON_INSTALL_DIR="$D/python" UV_CACHE_DIR="$D/cache" UV_NO_MODIFY_PATH=1 INSTALLER_NO_MODIFY_PATH=1
[ -x "$D/bin/uv" ] || curl -LsSf https://astral.sh/uv/install.sh | sh >/dev/null 2>&1 || die "could not install uv"
UV="$D/bin/uv"
[ -x "$D/venv/bin/python" ] || "$UV" venv -q --python 3.12 "$D/venv" || die "could not prepare Python"
"$UV" pip install -q --python "$D/venv/bin/python" substrate-interface || die "could not install Python packages"
PY="$D/venv/bin/python"

# ---------------------------------------------------------------- node code
say "Downloading Incentium Node"
TMP="$(mktemp -d)"
curl -fsSL "https://git.incentium.org/$REPO/archive/$BRANCH.tar.gz" | tar xz -C "$TMP" || die "could not download the code"
rm -rf "$D/src/neuron"
mv "$TMP"/*/neuron "$D/src/neuron"
rm -rf "$TMP"
[ -f "$D/src/neuron/incentium_neuron/node.py" ] || die "the downloaded code has no node"

# ---------------------------------------------------------------- llama.cpp
LLAMA="$(find "$D/llama" -name llama-server -type f 2>/dev/null | head -n 1 || true)"
if [ -z "$LLAMA" ] || [ "${INCENTIUM_UPDATE_LLAMA:-0}" = 1 ]; then
  say "Downloading llama.cpp"
  # llama.cpp publishes several releases a day and the Mac files are often uploaded after the
  # release itself, so the newest release can lack them. Take the newest release that has them.
  LLAMA_DL="https://github.com/ggml-org/llama.cpp/releases/download"
  llama_asset_for_tag() {  # prints the download URL if release $1 has a build for $PLAT
    local ext url
    for ext in tar.gz zip; do
      url="$LLAMA_DL/$1/llama-$1-bin-$PLAT.$ext"
      curl -fsIL -o /dev/null "$url" 2>/dev/null && { echo "$url"; return 0; }
    done
    return 1
  }
  ASSET=""
  # 1) a release chosen by hand: INCENTIUM_LLAMA_TAG=b12606
  [ -n "${INCENTIUM_LLAMA_TAG:-}" ] && ASSET=$(llama_asset_for_tag "$INCENTIUM_LLAMA_TAG" || true)
  # 2) the GitHub API, newest releases first (can be rate limited)
  [ -n "$ASSET" ] || ASSET=$(curl -fsSL "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20" 2>/dev/null \
    | grep -Eo "https://[^\"]*-bin-${PLAT}\.(tar\.gz|zip)" | head -n 1 || true)
  # 3) without the API: step back from the latest release tag
  if [ -z "$ASSET" ]; then
    TAG=$(curl -fsSIL -o /dev/null -w '%{url_effective}' https://github.com/ggml-org/llama.cpp/releases/latest 2>/dev/null \
      | grep -Eo 'b[0-9]+$' || true)
    if [ -n "$TAG" ]; then
      N=${TAG#b}
      for i in $(seq 0 40); do
        ASSET=$(llama_asset_for_tag "b$((N - i))" || true)
        [ -n "$ASSET" ] && break
      done
    fi
  fi
  # 4) a release known to work
  [ -n "$ASSET" ] || ASSET=$(llama_asset_for_tag b12606 || true)
  [ -n "$ASSET" ] || die "could not find llama.cpp for $PLAT (check the internet connection, or set INCENTIUM_LLAMA_TAG)"
  rm -rf "$D/llama" && mkdir -p "$D/llama"
  case "$ASSET" in
    *.zip) curl -fsSL "$ASSET" -o "$D/llama/l.zip" && unzip -q -o "$D/llama/l.zip" -d "$D/llama" && rm -f "$D/llama/l.zip" ;;
    *)     curl -fsSL "$ASSET" | tar xz -C "$D/llama" ;;
  esac
  LLAMA="$(find "$D/llama" -name llama-server -type f | head -n 1)"
  [ -n "$LLAMA" ] || die "llama-server was not in the download"
fi
xattr -dr com.apple.quarantine "$D/llama" 2>/dev/null || true
chmod +x "$LLAMA"
if ! "$LLAMA" --version >/dev/null 2>&1; then
  # some downloads need a local (ad-hoc) signature before macOS lets them run
  find "$D/llama" -type f \( -name 'llama-*' -o -name '*.dylib' \) -exec codesign --force -s - {} \; 2>/dev/null || true
  "$LLAMA" --version >/dev/null 2>&1 || die "llama-server does not start on this Mac"
fi

# ---------------------------------------------------------------- settings (kept on update)
say "Saving settings"
"$PY" - "$D" "$NETWORK" "$LLAMA" "$PORT" <<'PYEOF'
import json, os, secrets, sys
home, network, llama, port = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
p = os.path.join(home, "config.json")
c = json.load(open(p)) if os.path.exists(p) else {}
c.setdefault("token", secrets.token_urlsafe(24))
c.setdefault("network", network)
c["llama_server"], c["port"] = llama, port
fd = os.open(p + ".tmp", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as f:
    json.dump(c, f, indent=2)
os.replace(p + ".tmp", p)
PYEOF
TOKEN="$("$PY" -c "import json,sys; print(json.load(open(sys.argv[1]))['token'])" "$D/config.json")"
URL="http://127.0.0.1:$PORT/?t=$TOKEN"

# ---------------------------------------------------------------- start at login
say "Starting Incentium Node"
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$PY</string><string>-m</string><string>incentium_neuron.node</string>
    <string>--home</string><string>$D</string>
  </array>
  <key>WorkingDirectory</key><string>$D/src/neuron</string>
  <key>EnvironmentVariables</key><dict><key>PYTHONUNBUFFERED</key><string>1</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>Nice</key><integer>5</integer>
  <key>StandardOutPath</key><string>$D/node.log</string>
  <key>StandardErrorPath</key><string>$D/node.log</string>
</dict>
</plist>
EOF
launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST" || die "could not start the node (launchctl)"

# ---------------------------------------------------------------- desktop icon
cat > "$HOME/Desktop/Incentium.webloc" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>URL</key><string>$URL</string></dict></plist>
EOF

for _ in $(seq 1 30); do
  curl -fs -o /dev/null "http://127.0.0.1:$PORT/?t=$TOKEN" 2>/dev/null && break
  sleep 1
done
open "$URL"

cat <<EOF

$(printf '\033[1m')Incentium Node is running.$(printf '\033[0m')
  Your dashboard has opened in the browser. Later, open it with the "Incentium" icon on your desktop.
  It downloads the right model, then asks you to send a short request to join.
  You can close this window.
EOF
