#!/usr/bin/env bash
# Verification for T5.1: bim-ifc reads ifcopenshell-written IFC4 files and writes them back so that
# ifcopenshell parses the result identically (header, every instance id, type and attribute).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
grep -q '"crates/standards/\*"' Cargo.toml 2>/dev/null || fail 'workspace members do not include "crates/standards/*"'
cargo build -q --manifest-path judges/T5.1/Cargo.toml || fail "judge driver does not build against crate bim-ifc (crates/standards/ifc)"
drv="$CARGO_TARGET_DIR/debug/judge-t5-1"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

"$py" -I -B judges/T5.1/gen.py "$tmp" >/dev/null || fail "fixture generation failed"
# Fixture self-check: the commented/re-wrapped variant must mean the same to the oracle.
"$py" -I -B judges/T5.1/compare.py "$tmp/ok/building.ifc" "$tmp/ok/commented.ifc" >/dev/null \
  || fail "oracle self-check: commented fixture differs from building.ifc"

for f in "$tmp"/ok/*.ifc; do
  name=$(basename "$f")
  # The parse itself: every instance, entity name and typed value as ifcopenshell reads them.
  set +e; "$drv" dump "$f" "$tmp/dump-$name.jsonl" 2>"$tmp/err"; rc=$?; set -e
  [ "$rc" = 0 ] || fail "bim-ifc could not parse $name (exit $rc): $(head -c 400 "$tmp/err")"
  out=$("$py" -I -B judges/T5.1/dump_compare.py "$f" "$tmp/dump-$name.jsonl") || fail "$name parse: $out"
  set +e; "$drv" roundtrip "$f" "$tmp/rt-$name" 2>"$tmp/err"; rc=$?; set -e
  [ "$rc" = 0 ] || fail "round trip of $name failed (exit $rc): $(head -c 400 "$tmp/err")"
  # commented.ifc must round-trip to the same content as building.ifc
  ref="$f"; [ "$name" = commented.ifc ] && ref="$tmp/ok/building.ifc"
  out=$("$py" -I -B judges/T5.1/compare.py "$ref" "$tmp/rt-$name") || fail "$name: $out"
  # writing the written file again must be stable (writer output parses back identically)
  set +e; "$drv" roundtrip "$tmp/rt-$name" "$tmp/rt2-$name" 2>"$tmp/err"; rc=$?; set -e
  [ "$rc" = 0 ] || fail "bim-ifc cannot re-read its own output for $name: $(head -c 400 "$tmp/err")"
  out=$("$py" -I -B judges/T5.1/compare.py "$ref" "$tmp/rt2-$name") || fail "$name (second round trip): $out"
done

for f in "$tmp"/bad/*.ifc; do
  set +e; "$drv" reject "$f" 2>"$tmp/err"; rc=$?; set -e
  [ "$rc" = 0 ] || fail "malformed $(basename "$f") was not rejected with an error (exit $rc): $(head -c 300 "$tmp/err")"
done
echo "T5.1 judge: all checks passed"
