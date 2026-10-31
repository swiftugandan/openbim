#!/usr/bin/env bash
# Verification for T5.7: `bim validate --standard ibc|eurocode|accessibility|<path>` against fixtures
# 1e-6 inside/outside every shipped limit (closed-form measured values within 1e-9), a judge pack for the
# rest of the check vocabulary, hot-swapping a pack file between runs, and exit 2 for unknown checks.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
out=$("$py" -I -B judges/T5.7/t57.py "$bim" "$PWD" "$tmp") || fail "$out"
echo "T5.7 judge: all checks passed"
