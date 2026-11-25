#!/usr/bin/env bash
# Runtime upgrade with sudo (until day 90; afterwards use scripts/governance.py): build, test,
# check, install and verify, on the server that runs the chain. Testnet first.
#
#   bash scripts/upgrade-runtime-sudo.sh testnet          # checks, builds, tests, asks before set_code
#   bash scripts/upgrade-runtime-sudo.sh testnet --check  # everything except the upgrade itself
#   bash scripts/upgrade-runtime-sudo.sh mainnet          # later, once the testnet has run well
#
# Each network has its own branch: the testnet runs the EVM (Revive), mainnet does not.
#   testnet -> testnet-spec10      (testnet-evm + spec 10 and later)
#   mainnet -> feat/subnet-auction (mainnet-swap + spec 10 and later)
# The target spec_version is read from the branch.
# The script stops before anything is sent if the chain has a pallet the branch lacks, if the
# key is not the sudo key, or if a test fails. Nothing on the chain changes until you type "yes".
#
# Output goes to the screen and to ~/incentium-upgrade-<network>.log.
set -euo pipefail

NET="${1:-}"; CHECK_ONLY=0; [ "${2:-}" = "--check" ] && CHECK_ONLY=1
case "$NET" in
  testnet) BRANCH=testnet-spec10; URL=ws://127.0.0.1:9960; KEY="$HOME/incentium-testnet/secrets/operator.json" ;;
  mainnet) BRANCH=feat/subnet-auction; URL=ws://127.0.0.1:9946; KEY="$HOME/incentium-mainnet/secrets/operator.json" ;;
  *) echo "usage: bash $0 testnet|mainnet [--check]"; exit 2 ;;
esac
SOURCE_URL="${SOURCE_URL:-https://github.com/Doncoin1999/incentium-chain}"
REPO="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[ -n "$REPO" ] || { echo "run this from inside your incentium-chain checkout (e.g. cd ~/incentium-chain)"; exit 1; }
BUILD="$HOME/incentium-build/upgrade-$NET"
PY="$HOME/incentium-venv/bin/python"
LOG="$HOME/incentium-upgrade-$NET.log"
exec > >(tee -a "$LOG") 2>&1
step() { printf '\n=== %s\n' "$*"; }
echo "runtime upgrade with sudo, $NET, $(date -u '+%Y-%m-%d %H:%M UTC')"

step "1/7 Key and node"
[ -f "$KEY" ] || { echo "missing sudo key file $KEY"; exit 1; }
systemctl list-units --type=service --no-legend 2>/dev/null | grep -E "incentium" | awk '{print "  " $1, $4}' || true

step "2/7 Tools"
command -v cargo >/dev/null || { echo "cargo not found: install Rust from https://rustup.rs first"; exit 1; }
rustup target list --installed | grep -q wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
rustup component list --installed | grep -q rust-src || rustup component add rust-src
command -v clang >/dev/null || { sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq clang; }
[ -x "$PY" ] || python3 -m venv "$HOME/incentium-venv"
"$PY" -c "import substrateinterface" 2>/dev/null || "$PY" -m pip install -q substrate-interface
echo "  rust $(rustc --version | cut -d' ' -f2), python venv $PY"

step "3/7 Source ($BRANCH)"
git -C "$REPO" fetch -q "$SOURCE_URL" "$BRANCH" || {
  echo "Could not fetch $BRANCH from $SOURCE_URL."
  echo "If the branch is on git.incentium.org: SOURCE_URL=https://git.incentium.org/incentium/incentium-chain bash $0 $NET"
  echo "Or load a bundle: SOURCE_URL=/path/to/file.bundle bash $0 $NET"
  exit 1; }
COMMIT="$(git -C "$REPO" rev-parse FETCH_HEAD)"
if [ -d "$BUILD" ]; then git -C "$BUILD" checkout -q --detach "$COMMIT"
else mkdir -p "$(dirname "$BUILD")"; git -C "$REPO" worktree add -q --detach "$BUILD" "$COMMIT"; fi
TARGET="$(grep -oE 'spec_version: [0-9]+' "$BUILD/runtime/src/lib.rs" | grep -oE '[0-9]+')"
echo "  building $BRANCH at ${COMMIT:0:10} (spec $TARGET) in $BUILD (your checkout in $REPO is not touched)"

step "4/7 Preflight on the chain (nothing is sent)"
"$PY" "$BUILD/scripts/runtime_upgrade.py" preflight --url "$URL" --source "$BUILD" \
  --key-file "$KEY" --save "$HOME/incentium-build/upgrade-$NET-before.json"

step "5/7 Tests"
cd "$BUILD"
if ! SKIP_WASM_BUILD=1 cargo test -q -p ict-economics -p pallet-subnet-registry -p pallet-ict-consensus \
    -p pallet-weights -p pallet-work-verify -p pallet-ict-emission -p pallet-ict-swap -p incentium-runtime \
    > "$HOME/incentium-build/upgrade-$NET-tests.log" 2>&1; then
  grep -E "FAILED|panicked|^error" "$HOME/incentium-build/upgrade-$NET-tests.log" | head -20
  echo "TESTS FAILED (full output: ~/incentium-build/upgrade-$NET-tests.log): stopping, nothing was sent"
  exit 1
fi
grep -E "^test result" "$HOME/incentium-build/upgrade-$NET-tests.log" | sed 's/^/  /'
echo "  all tests passed"

step "6/7 Build the runtime (wasm)"
cargo build --release -q -p incentium-runtime
WASM="$BUILD/target/release/wbuild/incentium-runtime/incentium_runtime.compact.compressed.wasm"
[ -f "$WASM" ] || { echo "wasm not found at $WASM"; exit 1; }
cp "$WASM" "$HOME/incentium-runtime-spec$TARGET-$NET.wasm"
echo "  $(du -h "$WASM" | cut -f1)  sha256 $(sha256sum "$WASM" | cut -c1-16)…  copied to ~/incentium-runtime-spec$TARGET-$NET.wasm"

if [ "$CHECK_ONLY" = 1 ]; then
  step "Stopped before the upgrade (--check). Everything above passed."
  exit 0
fi

step "7/7 Upgrade and verify"
"$PY" "$BUILD/scripts/runtime_upgrade.py" upgrade --url "$URL" --key-file "$KEY" --wasm "$WASM" --target "$TARGET"
"$PY" "$BUILD/scripts/runtime_upgrade.py" verify --url "$URL" --target "$TARGET" --before "$HOME/incentium-build/upgrade-$NET-before.json"
echo
echo "Done. Send the log ($LOG) to Claude."
