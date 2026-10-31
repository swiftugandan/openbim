#!/usr/bin/env bash
# Verification for T1.7: bim-crdt semantic evaluation (materialized ConflictDiagnostic nodes,
# constraint and cycle conflicts) on hand-picked cases and 10,000 proptest multi-peer histories.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
for c in bim-doc bim-crdt bim-solver; do
  [ -f "crates/$c/Cargo.toml" ] || fail "crate crates/$c does not exist"
done
cargo test -q --manifest-path judges/T1.7/Cargo.toml --no-run || fail "judge does not compile against bim-crdt/bim-doc"
# perl's alarm is a portable timeout: a hung evaluate() must fail, not stall the loop. 600 s plus a
# cold proptest compile (~190 s) stays inside the loop's 900 s gate; a warm run should take well under a minute.
perl -e 'alarm shift; exec @ARGV or die' 600 cargo test -q --manifest-path judges/T1.7/Cargo.toml \
  || fail "bim-crdt semantic layer tests failed or timed out (see above)"
echo "T1.7 judge: all checks passed"
