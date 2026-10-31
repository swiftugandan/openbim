#!/usr/bin/env bash
# Verification for T4.7: drive the real `bim` binary (sizing, circuits, glTF segment solids, clash) against closed forms, manifold3d and trimesh.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
[ -x "$bim" ] || fail "binary bim not built"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
.oracles/py/bin/python -I judges/T4.7/judge.py "$bim" "$tmp"
