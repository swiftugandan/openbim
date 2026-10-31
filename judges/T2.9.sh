#!/usr/bin/env bash
# Verification for T2.9: export the same views as SVG, DXF and PDF with the real `bim`; check the
# DXF with ezdxf (AC1027, audit, layers/lineweights/DASHED, HATCH, dimension text, XDATA OPENBIM)
# and the PDF with pypdf (1.7, one OCG per layer, line widths in points, dimension text), and
# that both carry the same stroke geometry as the SVG export (T2.8).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
py=".oracles/py/bin/python"
j=judges/T2.9
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

run() { # run <label> <cmd...>: must exit 0
  local label=$1; shift
  "$@" >"$tmp/out" 2>"$tmp/err" || fail "$label exited $?: $(head -c 400 "$tmp/err")"
}

check_view() { # check_view <n> <view> <ops files...>
  local n=$1 view=$2; shift 2
  for fmt in svg dxf pdf; do
    run "bim export --format $fmt --view $view" \
      "$bim" export "$tmp/P" --format "$fmt" --view "$view" --out "$tmp/v$n.$fmt"
    [ -s "$tmp/v$n.$fmt" ] || fail "export --format $fmt --view $view wrote nothing"
  done
  "$py" -I -B "$j/check_cad.py" "$tmp/v$n.svg" "$tmp/v$n.dxf" "$tmp/v$n.pdf" "$view" "$@" >"$tmp/chk" 2>&1 || true
  cat "$tmp/chk" >&2
  grep -q '^JUDGE FAIL' "$tmp/chk" && exit 1
  grep -q '^T2.9 check' "$tmp/chk" || fail "oracle check for $view did not complete"
}

run "bim init" "$bim" init "$tmp/P"
run "bim apply drawing.ops.json" "$bim" apply "$tmp/P" "$j/drawing.ops.json"
for bad in "plan:NOPE@1.2" "elevation:q"; do
  for fmt in dxf pdf; do
    set +e; "$bim" export "$tmp/P" --format "$fmt" --view "$bad" --out "$tmp/bad.$fmt" >/dev/null 2>&1; rc=$?; set -e
    [ "$rc" = 2 ] || fail "export --format $fmt --view $bad exited $rc, want 2"
  done
done
check_view 1 "plan:L1@1.2" "$j/drawing.ops.json"
check_view 2 "section:-1,2,9,2" "$j/drawing.ops.json"
check_view 3 "elevation:s" "$j/drawing.ops.json"
run "bim apply edit.ops.json" "$bim" apply "$tmp/P" "$j/edit.ops.json"
check_view 4 "plan:L1@1.2" "$j/drawing.ops.json" "$j/edit.ops.json"
check_view 5 "elevation:e" "$j/drawing.ops.json" "$j/edit.ops.json"
echo "T2.9 judge: all checks passed"
