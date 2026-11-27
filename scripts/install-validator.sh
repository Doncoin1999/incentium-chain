#!/usr/bin/env bash
# Install an Incentium block producer. Kept for old links: it runs install-operator.sh with
# the block producer role (spec Section 19).
#
#   bash install-validator.sh mainnet|testnet <your stash address> [node name]
set -euo pipefail
NET="${1:?usage: install-validator.sh mainnet|testnet <stash address> [node name]}"
STASH="${2:?give the stash address that will bond the ICT}"
DIR="$(cd "$(dirname "$0")" && pwd)"
if [ -f "$DIR/install-operator.sh" ]; then OP="$DIR/install-operator.sh"; else
  OP="$(mktemp)"; curl -fsSL https://git.incentium.org/incentium/incentium-chain/raw/branch/main/scripts/install-operator.sh -o "$OP"
fi
exec bash "$OP" "$NET" --producer "$STASH" ${3:+--name "$3"}
