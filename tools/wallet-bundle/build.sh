#!/usr/bin/env bash
# Rebuilds site/assets/wallet.js: polkadot.js api + extension-dapp in one file, served by
# incentium.org itself so signing needs no outside script host. Versions are pinned in
# package-lock.json; the build is reproducible: compare the sha256 with the committed file.
set -euo pipefail
cd "$(dirname "$0")"
npm ci --silent
npx esbuild entry.js --bundle --minify --format=iife --target=es2020 --outfile=../../site/assets/wallet.js
sha256sum ../../site/assets/wallet.js
