#!/usr/bin/env bash
# Verification for T2.2: bim_geom::boolean against manifold3d (volumes within 1e-9 relative,
# merged coplanar faces) and trimesh (watertight STL), on random and touching/coplanar cases.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -f crates/bim-geom/Cargo.toml ] || fail "crate crates/bim-geom does not exist"
cargo build -q --manifest-path judges/T2.2/Cargo.toml || fail "judge driver does not compile against bim-geom"
.oracles/py/bin/python -I judges/T2.2/check.py "$CARGO_TARGET_DIR/debug/judge-t2-2" || fail "boolean checks failed (see above)"
echo "T2.2 judge: all checks passed"
