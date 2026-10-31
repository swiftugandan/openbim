#!/usr/bin/env bash
# Verification for T2.3: bim-spatial against brute force, plus release-mode timing budgets
# (100k R*-tree bulk load < 300 ms, mean raycast on 1M triangles < 1 ms).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -f crates/bim-spatial/Cargo.toml ] || fail "crate crates/bim-spatial does not exist"
cargo test -q --release --manifest-path judges/T2.3/Cargo.toml --no-run || fail "judge does not compile against bim-spatial"
# One test thread so the timing checks are not disturbed by the other tests.
cargo test -q --release --manifest-path judges/T2.3/Cargo.toml -- --test-threads=1 \
  || fail "bim-spatial tests failed (see the assertion above)"
echo "T2.3 judge: all checks passed"
