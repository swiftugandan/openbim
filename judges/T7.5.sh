#!/usr/bin/env bash
# Verification for T7.5: release-build `bim`, run `bim bench --elements 100000 --json` on this machine and
# enforce the spec/cli.md gate (incremental_ms < 50, frame_ms_p95 < 16.6, triangles >= 2e6), plus the
# criterion benches of crate bim-bench.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
cargo build -q --release -p bim-cli || fail "cargo build --release -p bim-cli failed"
bim="$CARGO_TARGET_DIR/release/bim"
# Criterion benches for compile, incremental edit, raycast and culling must exist and build.
list=$(cargo bench -q -p bim-bench -- --list 2>"$CARGO_TARGET_DIR/t7.5-bench.err") \
  || fail "cargo bench -p bim-bench -- --list failed: $(tail -20 "$CARGO_TARGET_DIR/t7.5-bench.err" 2>/dev/null)"
for w in compile incremental raycast cull; do
  grep -qi "$w.*: bench" <<<"$list" || fail "no criterion benchmark whose id contains '$w' (cargo bench -p bim-bench -- --list)"
done
.oracles/py/bin/python -I judges/T7.5/check.py "$bim"
echo "T7.5 judge: all checks passed"
