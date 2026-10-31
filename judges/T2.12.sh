#!/usr/bin/env bash
# Verification for T2.12: bim_sketch::infer_json (every candidate kind, the 10 CSS px radius, priority order and
# tie-breaks, with cursors placed by the judge's own pinhole projection), bim_sketch::parse_measurement (units,
# feet and inches, two values, arrays, division, angles, errors), and the 2 ms mean on 100,000 edges (release).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
# Run a command with a wall-clock limit (macOS has no coreutils timeout).
limit() { local s=$1; shift; perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$s" "$@"; }
[ -f crates/bim-sketch/Cargo.toml ] || fail "crate crates/bim-sketch does not exist"
limit 900 cargo test -q --release --manifest-path judges/T2.12/Cargo.toml --no-run \
  || fail "judge does not compile against bim-sketch (API per TASKS.md T2.12 Details)"
# One test thread so the timing check is not disturbed by the other tests.
limit 200 cargo test -q --release --manifest-path judges/T2.12/Cargo.toml -- --test-threads=1 \
  || fail "inference/measurement tests failed or timed out (see the assertion above)"
echo "T2.12 judge: all checks passed"
