#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"
PROJECT_ROOT="$(cd "${APP_REPO}/.." && pwd)"
ZKMOVE_REPO="${ZKMOVE_REPO:-${PROJECT_ROOT}/zkmove}"
ZKMOVE_VM_REPO="${ZKMOVE_VM_REPO:-${PROJECT_ROOT}/zkmove-vm}"

RUN_ID="${RUN_ID:-mint-min-$(date +%Y%m%d-%H%M%S)}"
export RUN_ID
export RUN_DIR="${RUN_DIR:-${APP_REPO}/.zkmove/runs/${RUN_ID}}"
export HALO2_REPO="${HALO2_REPO:-${PROJECT_ROOT}/halo2-verifier.move}"
export SUI_BIN="${SUI_BIN:-${PROJECT_ROOT}/zkmove_sui/target/debug/sui}"
export ZKMOVE_BIN="${ZKMOVE_BIN:-${ZKMOVE_VM_REPO}/target/debug/zkmove}"
export PARAMS_PATH="${PARAMS_PATH:-${ZKMOVE_VM_REPO}/cli/params/kzg_bn254_12.srs}"
export OFFCHAIN_PACKAGE="${OFFCHAIN_PACKAGE:-${ZKMOVE_REPO}/examples/confidential-asset/off-chain}"
export MINT_PACKAGE="${MINT_PACKAGE:-${SCRIPT_DIR}}"

exec "${ZKMOVE_VM_REPO}/examples/confidential-asset/mint-min/run-localnet-e2e.sh" "$@"
