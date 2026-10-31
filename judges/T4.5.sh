#!/usr/bin/env bash
# Verification for T4.5: drive `bim solve --trade struct --json` on authored documents; tributary areas
# and statics by closed form, reactions/envelopes/utilization of a space frame against PyNite.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -x .oracles/py/bin/python ] || fail "oracle Python missing (.oracles/py, run loop/setup.sh)"
cargo build -q -p bim-cli || fail "cannot build package bim-cli (binary bim)"
bim="$CARGO_TARGET_DIR/debug/bim"  # each bim call in judge.py has a 60 s timeout
[ -x "$bim" ] || fail "binary $bim not built"
.oracles/py/bin/python -I judges/T4.5/judge.py "$bim" || fail "judges/T4.5/judge.py reported failures (see above)"
