#!/usr/bin/env bash
# Verification for T4.4: closed forms (Euler-Bernoulli, Timoshenko, torsion, portals) and
# seeded random space frames against PyNite (fixture from judges/T4.4/gen_fixture.py), all within 0.1%.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -f crates/engines/structural/Cargo.toml ] || fail "crate bim-struct (crates/engines/structural) does not exist"
cargo test -q --manifest-path judges/T4.4/Cargo.toml || fail "judges/T4.4 tests failed (see output above)"
echo "T4.4 judge: all checks passed"
