#!/usr/bin/env bash
# Verification for T2.6: bim_geom::hlr on closed-form and random scenes of disjoint convex prisms,
# checked by exact ray casting (trimesh cross-checked) in judges/T2.6/oracle.py.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
py=.oracles/py/bin/python
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }

[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
cargo build -q --manifest-path judges/T2.6/Cargo.toml 2>"$tmp/build.log" \
  || fail "judge driver does not build against crates/bim-geom: $(tail -n 30 "$tmp/build.log")"
"$py" -I judges/T2.6/oracle.py gen "$tmp/scenes.txt" "$tmp/expected.json" || fail "oracle case generation failed"
set +e
perl -e 'alarm 120; exec @ARGV' "$CARGO_TARGET_DIR/debug/judge-t2-6" "$tmp/scenes.txt" >"$tmp/out.jsonl" 2>"$tmp/run.log"
rc=$?
set -e
[ "$rc" = 0 ] || fail "driver exited $rc (panic or timeout): $(tail -n 20 "$tmp/run.log")"
"$py" -I judges/T2.6/oracle.py check "$tmp/expected.json" "$tmp/out.jsonl"
