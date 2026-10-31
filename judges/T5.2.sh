#!/usr/bin/env bash
# Verification for T5.2: `bim export --format ifc` checked by ifcopenshell (validate, hierarchy,
# relationships, property sets, material layers, GlobalIds, per-element geometry volumes).
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
run() { # run <what> <cmd...>: must exit 0
  local what=$1; shift
  set +e; "$@" >"$tmp/out" 2>"$tmp/err"; local rc=$?; set -e
  [ "$rc" = 0 ] || fail "$what exited $rc: $(head -c 400 "$tmp/err")"
}

"$py" -I -B judges/T5.2/t52.py fixture "$tmp" || fail "fixture generation failed"
build() { # build <dir> <ops...>
  local d=$1; shift
  run "bim init $d" "$bim" init "$d"
  for ops in "$@"; do run "bim apply $(basename "$ops")" "$bim" apply "$d" "$ops"; done
}
build "$tmp/A" "$tmp/ops.json"
build "$tmp/B" "$tmp/ops-permuted.json"
build "$tmp/E" "$tmp/ops.json" "$tmp/edit.json"
for d in A B E; do
  run "bim export --format ifc ($d)" "$bim" export "$tmp/$d" --format ifc --out "$tmp/$d.ifc"
  [ -s "$tmp/$d.ifc" ] || fail "export wrote no file for $d"
done
run "bim solve --trade arch" "$bim" solve "$tmp/A" --trade arch --json; cp "$tmp/out" "$tmp/A.solve.json"
run "bim solve --trade arch (edited)" "$bim" solve "$tmp/E" --trade arch --json; cp "$tmp/out" "$tmp/E.solve.json"

# Exporting twice is byte-stable in GlobalIds (the checker compares the second export too).
run "second export" "$bim" export "$tmp/A" --format ifc --out "$tmp/A2.ifc"
out=$("$py" -I -B judges/T5.2/t52.py check "$tmp" "$tmp/A.ifc" "$tmp/A.solve.json" "$tmp/B.ifc" "$tmp/E.ifc" "$tmp/E.solve.json") \
  || fail "$out"
out=$("$py" -I -B judges/T5.2/t52.py check "$tmp" "$tmp/A2.ifc" "$tmp/A.solve.json" "$tmp/A.ifc" "$tmp/E.ifc" "$tmp/E.solve.json") \
  || fail "second export: $out"
echo "T5.2 judge: all checks passed"
