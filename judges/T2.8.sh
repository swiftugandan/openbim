#!/usr/bin/env bash
# Verification for T2.8: export SVG drawings (plans, a section, elevations) with the real `bim`
# and check them against manifold3d sections (cut areas via shapely), exact ray-cast visibility
# of model edges (PROJECTION/HIDDEN), the layer/lineweight/data-entity contract of spec/cli.md,
# and associative dimensions after an edit.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
py=".oracles/py/bin/python"
j=judges/T2.8
tmp=$(mktemp -d)
pids=()
# Background oracle checks are killed on any exit (bash 3.2-safe empty-array expansion).
trap 'kill ${pids[@]+"${pids[@]}"} 2>/dev/null || true; rm -rf "$tmp"' EXIT

[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"

# Every subprocess runs under an alarm so a hang in the code under test cannot hang the judge.
to() { perl -e 'alarm shift; exec @ARGV' "$@"; }

run() { # run <label> <cmd...>: must exit 0 within 120 s
  local label=$1; shift
  to 120 "$@" >"$tmp/out" 2>"$tmp/err" || fail "$label exited $? (142 = timeout): $(head -c 400 "$tmp/err")"
}

round() { # round <tag> <ops files> -- <views...>: export every view, then check all in parallel
  local tag=$1; shift; local ops=()
  while [ "$1" != "--" ]; do ops+=("$1"); shift; done; shift
  local n=0
  pids=()
  for view in "$@"; do
    n=$((n + 1))
    run "bim export --format svg --view $view ($tag)" \
      "$bim" export "$tmp/P" --format svg --view "$view" --out "$tmp/$tag-$n.svg"
    [ -s "$tmp/$tag-$n.svg" ] || fail "export --view $view wrote nothing"
    to 300 "$py" -I -B "$j/check_svg.py" "$tmp/$tag-$n.svg" "$view" "${ops[@]}" >"$tmp/chk-$tag-$n" 2>&1 &
    pids+=($!)
  done
  local i=0
  for pid in "${pids[@]}"; do
    i=$((i + 1))
    wait "$pid" || true
    cat "$tmp/chk-$tag-$i" >&2
    grep -q '^JUDGE FAIL' "$tmp/chk-$tag-$i" && exit 1
    grep -q '^T2.8 check' "$tmp/chk-$tag-$i" || fail "($tag) oracle check $i did not complete (crash or timeout)"
  done
  pids=()
}

run "bim init" "$bim" init "$tmp/P"
run "bim apply drawing.ops.json" "$bim" apply "$tmp/P" "$j/drawing.ops.json"

# Invalid views are invalid input (spec/cli.md exit 2).
for bad in "plan:NOPE@1.2" "section:1,2,3" "elevation:q" "isometric"; do
  set +e; to 120 "$bim" export "$tmp/P" --format svg --view "$bad" --out "$tmp/bad.svg" >/dev/null 2>&1; rc=$?; set -e
  [ "$rc" = 2 ] || fail "export --view $bad exited $rc, want 2"
done

round base "$j/drawing.ops.json" -- "plan:L1@1.2" "plan:L2@1.0" "section:-1,2,9,2" "elevation:s" "elevation:e"
# Associativity: move a dimension point, a window and the column; drawings follow the document.
run "bim apply edit.ops.json" "$bim" apply "$tmp/P" "$j/edit.ops.json"
round edit "$j/drawing.ops.json" "$j/edit.ops.json" -- "plan:L1@1.2" "section:-1,2,9,2" "elevation:n"
echo "T2.8 judge: all checks passed"
