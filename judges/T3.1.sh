#!/usr/bin/env bash
# Verification for T3.1: bim-incr drives bim-model/bim-draw; Session::apply reports exactly what it recomputed.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }

for c in bim-incr bim-model bim-draw; do
  [ -f "crates/$c/Cargo.toml" ] || fail "crate crates/$c is missing"
done

# bim-model and bim-draw must compute through bim-incr (a direct normal dependency of each).
for c in bim-model bim-draw; do
  deps=$(cargo tree -q -p "$c" --depth 1 -e normal --prefix none 2>&1) || fail "cargo tree -p $c failed: $deps"
  printf '%s\n' "$deps" | grep -q '^bim-incr ' || fail "$c does not depend on bim-incr (cargo tree -p $c --depth 1 -e normal)"
done

# The tests rebuild the fixture from scratch after every edit (many compiles with HLR), so they run
# in release; the alarm keeps a hang in the code under test from hanging the judge.
perl -e 'alarm 600; exec @ARGV' cargo test -q --release --manifest-path judges/T3.1/Cargo.toml -- --test-threads=4 \
  || fail "judge tests in judges/T3.1/tests/judge.rs failed or timed out (see above)"
echo "T3.1 judge: all checks passed"
