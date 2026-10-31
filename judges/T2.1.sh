#!/usr/bin/env bash
# Verification for T2.1: bim-geom predicates, polygons, extrusions and sweeps against exact
# rational arithmetic, shapely, trimesh (on the exported STL) and closed forms.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -f crates/bim-geom/Cargo.toml ] || fail "crate crates/bim-geom does not exist"
cargo build -q --manifest-path judges/T2.1/Cargo.toml || fail "judge driver does not compile against bim-geom"
.oracles/py/bin/python -I judges/T2.1/check.py "$CARGO_TARGET_DIR/debug/judge-t2-1" || fail "bim-geom checks failed (see above)"
echo "T2.1 judge: all checks passed"
