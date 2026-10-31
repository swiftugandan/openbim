#!/usr/bin/env bash
# Verification for T5.3: `bim import` of an IFC4 file authored at run time with ifcopenshell.api
# (independent producer): kinds, resolved placements, props, property sets, counts and volumes,
# and import -> export volume conservation, all against ifcopenshell.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
run() { # run <what> <cmd...>: must exit 0; stdout in $tmp/out
  local what=$1; shift
  set +e; "$@" >"$tmp/out" 2>"$tmp/err"; local rc=$?; set -e
  [ "$rc" = 0 ] || fail "$what exited $rc: $(head -c 400 "$tmp/err")"
}

"$py" -I -B judges/T5.3/t53.py gen "$tmp" || fail "fixture generation failed"
run "bim init" "$bim" init "$tmp/P"
run "bim hash (empty)" "$bim" hash "$tmp/P"; h0=$(cat "$tmp/out")

# Invalid input changes nothing (spec/cli.md exit 2): a truncated copy of the source file.
head -c "$(( $(wc -c < "$tmp/source.ifc") / 2 ))" "$tmp/source.ifc" > "$tmp/truncated.ifc"
set +e; "$bim" import "$tmp/P" "$tmp/truncated.ifc" >/dev/null 2>&1; rc=$?; set -e
[ "$rc" = 2 ] || fail "import of a truncated IFC exited $rc, want 2"
run "bim hash" "$bim" hash "$tmp/P"
[ "$(cat "$tmp/out")" = "$h0" ] || fail "rejected import changed the document"

run "bim import source.ifc" "$bim" import "$tmp/P" "$tmp/source.ifc"
run "bim solve --trade arch" "$bim" solve "$tmp/P" --trade arch --json; cp "$tmp/out" "$tmp/solve.json"
run "bim export --format ifc" "$bim" export "$tmp/P" --format ifc --out "$tmp/reexport.ifc"
out=$("$py" -I -B judges/T5.3/t53.py check "$tmp" "$bim" "$tmp/P" "$tmp/solve.json" "$tmp/reexport.ifc") || fail "$out"
echo "T5.3 judge: all checks passed"
