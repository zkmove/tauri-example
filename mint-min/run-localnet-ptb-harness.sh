#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"

export ZKMOVE_MINT_MANIFEST="${ZKMOVE_MINT_MANIFEST:-${APP_REPO}/mint-min.manifest.json}"

exec cargo run \
  --manifest-path "${APP_REPO}/backend/Cargo.toml" \
  --bin localnet_ptb_harness \
  -- "$@"
