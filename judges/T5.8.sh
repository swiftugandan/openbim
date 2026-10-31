#!/usr/bin/env bash
# Verification for T5.8: IFC4 export of catalogue products (type objects, IfcRelDefinesByType, manufacturer and
# catalogue property sets), IfcFurniture/IfcRoof solids, finishes as IfcCovering, extension kinds as
# IfcBuildingElementProxy, read back with ifcopenshell (validate, geom volumes vs closed forms).
# Logic in judges/T5.8/t58.py.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
export PYTHONDONTWRITEBYTECODE=1
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
to() { perl -e 'alarm shift; exec @ARGV' "$@"; }
to 1800 cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed (or timed out)"
bim="$CARGO_TARGET_DIR/debug/bim"
[ -x "$bim" ] || fail "binary bim not built"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
set +e
to 600 "$py" -I -B judges/T5.8/t58.py "$bim" "$tmp"
rc=$?
set -e
# t58.py exits 1 after printing its own `JUDGE FAIL:` line; anything else is a crash or a timeout.
[ "$rc" = 0 ] || { [ "$rc" = 142 ] && fail "judge timed out (600 s)"; [ "$rc" = 1 ] || fail "t58.py crashed (exit $rc, see above)"; exit 1; }
