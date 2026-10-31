#!/usr/bin/env bash
# Verification for T5.4: every corpus/ifc/*.ifc file round-trips through bim (import, solve all trades,
# IFC4 + per-storey SVG/DXF export) with ifcopenshell volumes/slab areas conserved and ezdxf-clean DXF.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
ls corpus/ifc/*.ifc >/dev/null 2>&1 || fail "corpus/ifc/ has no *.ifc files (add the reference corpus; see loop/setup.sh)"
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
bim="$CARGO_TARGET_DIR/debug/bim"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
"$py" -I -B judges/T5.4/t54.py "$bim" corpus/ifc "$tmp" > "$tmp/log" 2>&1 || fail "$(tail -n 1 "$tmp/log")"
cat "$tmp/log"
echo "T5.4 judge: all checks passed"
