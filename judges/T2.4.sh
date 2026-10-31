#!/usr/bin/env bash
# Verification for T2.4: 3D constraint solver vs python-solvespace and closed forms, plus Expr::diff.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
py=.oracles/py/bin/python
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
# Run a command with a wall-clock limit (macOS has no coreutils timeout).
limit() { local s=$1; shift; perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$s" "$@"; }

[ -x "$py" ] || fail "oracle python missing at $py (run loop/setup.sh)"
[ -f crates/bim-solver/Cargo.toml ] || fail "crate crates/bim-solver does not exist"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cargo build -q --manifest-path judges/T2.4/Cargo.toml --bin judge-t2-4 --tests \
  || fail "judge package does not compile against bim-solver (API per TASKS.md T2.4 / spec gaps)"
limit 120 cargo test -q --manifest-path judges/T2.4/Cargo.toml \
  || fail "Expr::diff / T1.3-API / Sketch3 API tests failed (see output above)"

"$py" -I judges/T2.4/gen_cases.py "$tmp/cases.json" || fail "case generator failed"
set +e
limit 60 "$CARGO_TARGET_DIR/debug/judge-t2-4" "$tmp/cases.json" >"$tmp/results.json"
rc=$?
set -e
[ "$rc" = 0 ] || fail "runner exited $rc (panic, crash or >60 s)"
"$py" -I judges/T2.4/check.py "$tmp/cases.json" "$tmp/results.json"
echo "T2.4 judge: all checks passed"
