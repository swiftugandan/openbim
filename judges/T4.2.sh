#!/usr/bin/env bash
# Verification for T4.2: drive `bim solve --trade arch` and recompute rooms with shapely.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
[ -x "$bim" ] || fail "binary bim not built"
.oracles/py/bin/python -I judges/T4.2/judge.py "$bim"
