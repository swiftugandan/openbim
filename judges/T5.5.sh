#!/usr/bin/env bash
# Verification for T5.5: ifcJSON export/import. doc -> ifcJSON -> doc keeps the content hash; ifcJSON
# converted by the judge from an ifcopenshell-authored IFC imports like the .ifc itself.
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
timed() { # timed <seconds> <cmd...>: run with a timeout (macOS has no `timeout`); exit 124 on expiry
  "$py" -I -S -c 'import subprocess,sys
try: sys.exit(subprocess.run(sys.argv[2:], timeout=float(sys.argv[1])).returncode)
except subprocess.TimeoutExpired: sys.exit(124)' "$@"
}
run() { # run <what> <cmd...>: must exit 0 within 60 s; stdout in $tmp/out
  local what=$1; shift
  set +e; timed 60 "$@" >"$tmp/out" 2>"$tmp/err"; local rc=$?; set -e
  [ "$rc" = 124 ] && fail "$what timed out after 60 s"
  [ "$rc" = 0 ] || fail "$what exited $rc: $(head -c 400 "$tmp/err")"
}
t55() { "$py" -I -B judges/T5.5/t55.py "$@"; }

# 1. doc -> ifcJSON -> doc preserves the content hash.
t55 fixture "$tmp" >/dev/null || fail "fixture generation failed"
run "bim init A" "$bim" init "$tmp/A"
run "bim apply ops.json" "$bim" apply "$tmp/A" "$tmp/ops.json"
run "bim export --format ifcjson" "$bim" export "$tmp/A" --format ifcjson --out "$tmp/a.json"
out=$(t55 check-export "$tmp" "$tmp/a.json") || fail "$out"
run "bim init B" "$bim" init "$tmp/B"
run "bim import a.json" "$bim" import "$tmp/B" "$tmp/a.json"
run "bim hash A" "$bim" hash "$tmp/A"; ha=$(cat "$tmp/out")
run "bim hash B" "$bim" hash "$tmp/B"; hb=$(cat "$tmp/out")
[ "$ha" = "$hb" ] || fail "doc -> ifcJSON -> doc changed the content hash ($ha -> $hb)"
run "bim diff A B" "$bim" diff "$tmp/A" "$tmp/B"
# ... and a second trip through ifcJSON is stable too.
run "bim export B --format ifcjson" "$bim" export "$tmp/B" --format ifcjson --out "$tmp/b.json"
run "bim init C" "$bim" init "$tmp/C"
run "bim import b.json" "$bim" import "$tmp/C" "$tmp/b.json"
run "bim hash C" "$bim" hash "$tmp/C"
[ "$(cat "$tmp/out")" = "$ha" ] || fail "second ifcJSON round trip changed the content hash"

# 2. Judge-converted ifcJSON imports like the .ifc it came from.
t55 gen-ifc "$tmp" >/dev/null || fail "IFC generation failed"
out=$(t55 to-ifcjson "$tmp/source.ifc" "$tmp/source.json") || fail "converter: $out"
run "bim init X" "$bim" init "$tmp/X"
run "bim import source.ifc" "$bim" import "$tmp/X" "$tmp/source.ifc"
run "bim init Y" "$bim" init "$tmp/Y"
run "bim import source.json" "$bim" import "$tmp/Y" "$tmp/source.json"
for d in X Y; do
  run "bim solve --trade arch ($d)" "$bim" solve "$tmp/$d" --trade arch --json; cp "$tmp/out" "$tmp/$d.solve.json"
  run "bim export --format ifc ($d)" "$bim" export "$tmp/$d" --format ifc --out "$tmp/$d.ifc"
done
out=$(t55 compare "$tmp/X.solve.json" "$tmp/Y.solve.json" "$tmp/X.ifc" "$tmp/Y.ifc") || fail "$out"

# Malformed ifcJSON is invalid input: exit 2, nothing changed.
run "bim hash Y" "$bim" hash "$tmp/Y"; hy=$(cat "$tmp/out")
printf '{"type":"ifcJSON","version":"0.0.1","data":[{"type":"IfcWall","globalId":' > "$tmp/bad.json"
set +e; timed 60 "$bim" import "$tmp/Y" "$tmp/bad.json" >/dev/null 2>&1; rc=$?; set -e
[ "$rc" = 2 ] || fail "import of truncated ifcJSON exited $rc, want 2"
run "bim hash Y" "$bim" hash "$tmp/Y"
[ "$(cat "$tmp/out")" = "$hy" ] || fail "rejected ifcJSON import changed the document"
echo "T5.5 judge: all checks passed"
