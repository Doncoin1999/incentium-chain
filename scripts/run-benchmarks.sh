#!/usr/bin/env bash
# Benchmark the Incentium pallets on this machine and write weight files.
#
#   bash scripts/run-benchmarks.sh [RUNTIME_WASM]
#
# RUNTIME_WASM is the CI artifact incentium-runtime-benchmarks (default:
# ~/benchmark/incentium_runtime.compact.compressed.wasm). frame-omni-bencher is downloaded
# from the Polkadot SDK release the runtime is built with. Results go to ~/benchmark/weights/,
# one file per pallet, for runtime/src/weights/. Takes 10-30 minutes; run it when the server
# is otherwise quiet, since the numbers are measured on this CPU.
set -euo pipefail
B="$HOME/benchmark"
WASM="${1:-$B/incentium_runtime.compact.compressed.wasm}"
BENCH="$B/frame-omni-bencher"
OUT="$B/weights"
SDK_TAG="polkadot-stable2610"
PALLETS="pallet_ict_emission pallet_subnet_registry pallet_weights pallet_work_verify pallet_ict_consensus"
mkdir -p "$B" "$OUT"

[ -s "$WASM" ] || { echo "missing $WASM (download the CI artifact incentium-runtime-benchmarks)"; exit 1; }
if [ ! -x "$BENCH" ]; then
  for url in \
    "https://github.com/paritytech/polkadot-sdk/releases/download/$SDK_TAG/frame-omni-bencher" \
    "https://github.com/paritytech/polkadot-sdk/releases/latest/download/frame-omni-bencher"; do
    echo "downloading frame-omni-bencher from $url"
    if curl -fsSL -o "$BENCH" "$url"; then chmod +x "$BENCH"; break; fi
  done
fi
[ -x "$BENCH" ] || { echo "could not download frame-omni-bencher"; exit 1; }
"$BENCH" --version

for p in $PALLETS; do
  echo "=== $p"
  "$BENCH" v1 benchmark pallet \
    --runtime "$WASM" \
    --pallet "$p" \
    --extrinsic "*" \
    --steps 50 \
    --repeat 20 \
    --output "$OUT/$p.rs"
  # The runtime reaches FRAME through the polkadot-sdk umbrella crate.
  sed -i 's/^use frame_support::{traits::Get, weights::Weight};/use polkadot_sdk::{frame_support, frame_system};\n&/' "$OUT/$p.rs"
done

echo
echo "Done. Weight files:"
ls -l "$OUT"
echo "Send these five files to Claude (or upload them to runtime/src/weights/ on GitHub)."
