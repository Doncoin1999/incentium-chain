#!/usr/bin/env bash
# Rebuild the vendored browser libraries (run from swap/web/vendor-src). Versions are pinned.
set -euo pipefail
npm init -y >/dev/null
npm i -s @polkadot/api@17.0.2 @polkadot/extension-dapp@0.64.1 @polkadot/util-crypto @polkadot/util ethers@6.17.0 esbuild
npx esbuild polkadot-entry.js --bundle --format=iife --global-name=Polka --minify --platform=browser --target=es2020 --outfile=../vendor/polkadot.bundle.js
cp node_modules/ethers/dist/ethers.umd.min.js ../vendor/
sha256sum ../vendor/*.js
