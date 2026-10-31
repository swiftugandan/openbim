#!/usr/bin/env bash
# Verification for T3.1: bim-incr drives bim-model/bim-draw; Session::apply reports exactly what it recomputed.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }

for c in bim-incr bim-model bim-draw; do
  [ -f "crates/$c/Cargo.toml" ] || fail "crate crates/$c is missing"
done

# bim-model and bim-draw must compute through bim-incr (a direct dependency of each).
meta=$(cargo metadata -q --format-version 1 --no-deps) || fail "cargo metadata failed on the workspace"
printf '%s' "$meta" | .oracles/py/bin/python -I -c '
import json, sys
pk = {p["name"]: p for p in json.load(sys.stdin)["packages"]}
for name in ("bim-model", "bim-draw"):
    if name not in pk:
        sys.exit(f"{name} is not a workspace member")
    deps = {d["name"] for d in pk[name]["dependencies"] if d.get("kind") in (None, "normal")}
    if "bim-incr" not in deps:
        sys.exit(f"{name} does not depend on bim-incr")
' || fail "dependency structure (see above)"

cargo test -q --manifest-path judges/T3.1/Cargo.toml -- --test-threads=4 \
  || fail "judge tests in judges/T3.1/tests/judge.rs failed (see above)"
echo "T3.1 judge: all checks passed"
