#!/usr/bin/env bash
# Verification for T1.6: bim-doc schema v1 API tests, then the `bim` CLI (JSON ops, show)
# per spec/cli.md and spec/schema.md, then the T1.1 and T1.5 judges (behaviour preserved).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -f crates/bim-doc/Cargo.toml ] || fail "crate crates/bim-doc does not exist"
[ -f crates/bim-cli/Cargo.toml ] || fail "crate crates/bim-cli does not exist"
cargo test -q --manifest-path judges/T1.6/Cargo.toml --no-run || fail "judge does not compile against bim-doc"
# perl's alarm is a portable timeout: a looping topo_order() must fail, not stall the loop.
perl -e 'alarm shift; exec @ARGV or die' 60 cargo test -q --manifest-path judges/T1.6/Cargo.toml \
  || fail "bim-doc schema v1 API tests failed or timed out (see above)"
cargo build -q -p bim-cli || fail "bim-cli does not build"
.oracles/py/bin/python -I judges/T1.6/cli.py "$CARGO_TARGET_DIR/debug/bim" || fail "bim CLI checks failed (see above)"
bash judges/T1.1.sh >/dev/null || fail "T1.1 behaviour not preserved (bash judges/T1.1.sh)"
bash judges/T1.5.sh >/dev/null || fail "T1.5 behaviour not preserved (bash judges/T1.5.sh)"
echo "T1.6 judge: all checks passed"
