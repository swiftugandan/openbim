#!/usr/bin/env bash
# Verification for T5.6: `bim bcf` create/export/import. Every XML file of every exported bcfzip validates
# against the buildingSMART BCF 3.0 XSDs (xmlschema); import(export(x)) preserves topics, comments and
# components; topics are mirrored as Issue entities; components carry the T5.2 IfcGuids.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
[ -f .oracles/bcf-xsd/markup.xsd ] || fail "missing BCF 3.0 XSDs in .oracles/bcf-xsd (run loop/setup.sh)"
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
run() { # run <what> <cmd...>: must exit 0; stdout in $tmp/out
  local what=$1; shift
  set +e; "$@" >"$tmp/out" 2>"$tmp/err"; local rc=$?; set -e
  [ "$rc" = 0 ] || fail "$what exited $rc: $(head -c 400 "$tmp/err")"
}
t56() { "$py" -I -B judges/T5.6/t56.py "$@"; }
project() { run "bim init $1" "$bim" init "$tmp/$1"; run "bim apply ($1)" "$bim" apply "$tmp/$1" "$tmp/ops.json"; }
id() { "$py" -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["ids"][sys.argv[2]])' "$tmp/expected.json" "$1"; }

out=$(t56 fixture "$tmp") || fail "fixture: $out"

# 1. Create two topics; export; validate; components = IfcGuids of the entities; Issues mirrored.
project P
run "bim bcf create #1" "$bim" bcf "$tmp/P" create --title "Crack above door" --entity "$(id W1)"
tail -n 1 "$tmp/out" | tr -d '[:space:]' > "$tmp/g1"; echo >> "$tmp/g1"
run "bim bcf create #2" "$bim" bcf "$tmp/P" create --title "Column too close to wall W2" --entity "$(id C1)" "$(id W2)"
tail -n 1 "$tmp/out" | tr -d '[:space:]' >> "$tmp/g1"; echo >> "$tmp/g1"
grep -Eqx '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' <(head -n 1 "$tmp/g1") \
  || fail "bim bcf create should print the new topic's Guid, got: $(head -n 1 "$tmp/g1")"
run "bim bcf export (P)" "$bim" bcf "$tmp/P" export "$tmp/p.bcfzip"
out=$(t56 validate "$tmp/p.bcfzip") || fail "created topics: $out"
out=$(t56 created "$tmp" "$tmp/p.bcfzip" "$tmp/g1") || fail "$out"
out=$(t56 issues "$tmp" "$bim" "$tmp/P" "$tmp/p.bcfzip") || fail "$out"

# 2. import(export(x)) for a judge-authored BCF 3.0 archive with comments and several viewpoints.
project Q
run "bim bcf import x.bcfzip" "$bim" bcf "$tmp/Q" import "$tmp/x.bcfzip"
out=$(t56 issues "$tmp" "$bim" "$tmp/Q" "$tmp/x.bcfzip") || fail "after import: $out"
run "bim bcf export (Q)" "$bim" bcf "$tmp/Q" export "$tmp/q.bcfzip"
out=$(t56 validate "$tmp/q.bcfzip") || fail "re-exported archive: $out"
out=$(t56 same "$tmp/x.bcfzip" "$tmp/q.bcfzip") || fail "import(export(x)): $out"

# 3. import(export(x)) for bim's own export, into a fresh project.
project R
run "bim bcf import p.bcfzip" "$bim" bcf "$tmp/R" import "$tmp/p.bcfzip"
run "bim bcf export (R)" "$bim" bcf "$tmp/R" export "$tmp/r.bcfzip"
out=$(t56 validate "$tmp/r.bcfzip") || fail "R export: $out"
out=$(t56 same "$tmp/p.bcfzip" "$tmp/r.bcfzip") || fail "import(export(created)): $out"

# 4. Invalid input exits 2 and changes nothing.
run "bim hash" "$bim" hash "$tmp/R"; h=$(cat "$tmp/out")
printf 'not a zip' > "$tmp/bad.bcfzip"
set +e; "$bim" bcf "$tmp/R" import "$tmp/bad.bcfzip" >/dev/null 2>&1; rc=$?; set -e
[ "$rc" = 2 ] || fail "import of a non-zip bcfzip exited $rc, want 2"
set +e; "$bim" bcf "$tmp/R" create --title "ghost" --entity no-such-entity >/dev/null 2>&1; rc=$?; set -e
[ "$rc" = 2 ] || fail "bcf create with a missing entity exited $rc, want 2"
run "bim hash" "$bim" hash "$tmp/R"
[ "$(cat "$tmp/out")" = "$h" ] || fail "rejected bcf commands changed the document"
echo "T5.6 judge: all checks passed"
