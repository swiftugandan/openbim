#!/usr/bin/env bash
# Verification for T6.1: drive `bim query` black-box; expected rows come from networkx and closed forms.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q -p bim-cli 2>"$tmp/build.log" || { cat "$tmp/build.log" >&2; fail "cannot build bim-cli"; }
bim="$CARGO_TARGET_DIR/debug/bim"
[ -x "$bim" ] || fail "binary $bim not found"
.oracles/py/bin/python -I judges/T6.1/judge.py "$bim" "$tmp"
