#!/usr/bin/env bash
# Verification for T5.9: IFC4 export of direct-modelling shapes and components. Builds shapes with
# `bim sketch` (push/pulled boxes, an L-shaped component with two instances, a second component with an
# explicit class, an open surface of two separate faces, two faces painted with coloured products), exports IFC and
# checks it with ifcopenshell (validate, occurrence classes, IfcFacetedBrep volumes, IfcRepresentationMap +
# IfcMappedItem placements, IfcRelDefinesByType, IfcShellBasedSurfaceModel, IfcStyledItem colours).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
export PYTHONDONTWRITEBYTECODE=1
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
# Every subprocess runs under an alarm so a hang in the code under test cannot hang the judge.
to() { perl -e 'alarm shift; exec @ARGV or die "exec: $!\n"' "$@"; }
to 900 cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed (see above)"
bim="$CARGO_TARGET_DIR/debug/bim"
[ -x "$bim" ] || fail "no bim binary at $bim"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# drive: the python helper runs every bim command itself (60 s timeout each) and writes state.json + out.ifc.
out=$(to 300 "$py" -I -B judges/T5.9/t59.py drive "$bim" "$tmp") || fail "${out:-fixture driver exited $? (142 = timeout)}"
[ -s "$tmp/out.ifc" ] || fail "bim export --format ifc wrote no file"
out=$(to 300 "$py" -I -B judges/T5.9/t59.py check "$tmp") || fail "${out:-checker exited $? (142 = timeout)}"
[ "$out" = ok ] || fail "checker did not complete: $out"
echo "T5.9 judge: all checks passed"
