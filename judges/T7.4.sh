#!/usr/bin/env bash
# Verification for T7.4: drive `bim sheet` and measure the SVG, PDF and DXF sheets it writes.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -x .oracles/py/bin/python ] || fail "Python oracle missing (.oracles/py); run loop/setup.sh"
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
exec .oracles/py/bin/python -I judges/T7.4/check.py "$CARGO_TARGET_DIR/debug/bim"
