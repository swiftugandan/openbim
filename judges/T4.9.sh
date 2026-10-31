#!/usr/bin/env bash
# Verification for T4.9: product catalogues (validation, list, use, idempotent updates), effective props in the
# wall cut and arch outputs, Furniture/Roof solids in glTF (manifold3d references) and clash (closed forms),
# `products`/`roofs` take-off. Logic in judges/T4.9/judge.py.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
export PYTHONDONTWRITEBYTECODE=1
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
[ -d .oracles/node/node_modules/gltf-validator ] || fail "missing oracle gltf-validator (run loop/setup.sh)"
to() { perl -e 'alarm shift; exec @ARGV' "$@"; }
to 1800 cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed (or timed out)"
bim="$CARGO_TARGET_DIR/debug/bim"
[ -x "$bim" ] || fail "binary bim not built"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
set +e
to 600 "$py" -I -B judges/T4.9/judge.py "$bim" "$tmp"
rc=$?
set -e
# judge.py exits 1 after printing its own `JUDGE FAIL:` line; anything else is a crash or a timeout.
[ "$rc" = 0 ] || { [ "$rc" = 142 ] && fail "judge timed out (600 s)"; [ "$rc" = 1 ] || fail "judge.py crashed (exit $rc, see above)"; exit 1; }
