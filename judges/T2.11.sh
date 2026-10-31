#!/usr/bin/env bash
# Verification for T2.11: direct modelling II (spec/sketch.md). Drives the real `bim sketch` and checks offset,
# follow-me (vs an independent mitred-sweep construction), move/rotate/scale with stretching, the triangle-fan
# rule, copy/array, paint (face_products) and components (definition not placed, instances under their
# transform, edits reach every instance) via `bim show --json` and the glTF export, plus atomicity and determinism.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
export PYTHONDONTWRITEBYTECODE=1
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
# Run a command with a wall-clock limit (macOS has no coreutils timeout).
limit() { local s=$1; shift; perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$s" "$@"; }
py=.oracles/py/bin/python

[ -x "$py" ] || fail "oracle python missing at $py (run loop/setup.sh)"
[ -d .oracles/node/node_modules/gltf-validator ] || fail "missing oracle gltf-validator (run loop/setup.sh)"
[ -f crates/bim-sketch/Cargo.toml ] || fail "crate crates/bim-sketch does not exist"
[ -f crates/bim-cli/Cargo.toml ] || fail "crate crates/bim-cli does not exist"
limit 600 cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
[ -x "$bim" ] || fail "no bim binary at $bim"
limit 300 "$py" -I -B judges/T2.11/check.py "$bim" || fail "direct-modelling checks failed or timed out (see above)"
echo "T2.11 judge: all checks passed"
