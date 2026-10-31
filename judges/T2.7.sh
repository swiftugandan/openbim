#!/usr/bin/env bash
# Verification for T2.7: compile the fixture building with the real `bim`, then compare
# `bim solve --trade arch --json` and the glTF export against an independent manifold3d
# reconstruction of spec/schema.md's derived geometry rules (judges/T2.7/model.py), and run the
# Khronos glTF-Validator. Repeats after an edit (join removed, T-join broken, door moved).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
py=".oracles/py/bin/python"
j=judges/T2.7
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
[ -d .oracles/node/node_modules/gltf-validator ] || fail "missing oracle gltf-validator (run loop/setup.sh)"

# Every subprocess runs under an alarm so a hang in the code under test cannot hang the judge.
to() { perl -e 'alarm shift; exec @ARGV' "$@"; }

run() { # run <label> <cmd...>: must exit 0 within 120 s
  local label=$1; shift
  to 120 "$@" >"$tmp/out" 2>"$tmp/err" || fail "$label exited $? (142 = timeout): $(head -c 400 "$tmp/err")"
}

round() { # round <tag> <ops files...>
  local tag=$1; shift
  run "bim solve --trade arch --json ($tag)" "$bim" solve "$tmp/P" --trade arch --json
  cp "$tmp/out" "$tmp/solve-$tag.json"
  run "bim export --format gltf ($tag)" "$bim" export "$tmp/P" --format gltf --out "$tmp/$tag.gltf"
  [ -s "$tmp/$tag.gltf" ] || fail "export wrote no $tag.gltf"
  to 120 node "$j/validate_gltf.mjs" "$tmp/$tag.gltf" >"$tmp/val" 2>&1 || fail "($tag) $(cat "$tmp/val")"
  to 300 "$py" -I -B "$j/check.py" "$tmp/solve-$tag.json" "$tmp/$tag.gltf" "$@" >"$tmp/chk" 2>&1 || true
  cat "$tmp/chk" >&2
  grep -q '^JUDGE FAIL' "$tmp/chk" && exit 1
  grep -q '^T2.7 check:' "$tmp/chk" || fail "($tag) oracle check did not complete"
}

run "bim init" "$bim" init "$tmp/P"
run "bim apply model.ops.json" "$bim" apply "$tmp/P" "$j/model.ops.json"
round base "$j/model.ops.json"
run "bim apply edit.ops.json" "$bim" apply "$tmp/P" "$j/edit.ops.json"
round edit "$j/model.ops.json" "$j/edit.ops.json"
echo "T2.7 judge: all checks passed"
