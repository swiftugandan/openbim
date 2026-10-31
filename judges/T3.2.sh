#!/usr/bin/env bash
# Verification for T3.2: drive the real `bim` binary through the store/revision contract (spec/cli.md).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }

[ -f crates/bim-store/Cargo.toml ] || fail "crate crates/bim-store is missing"
meta=$(cargo metadata -q --format-version 1 --no-deps) || fail "cargo metadata failed on the workspace"
printf '%s' "$meta" | .oracles/py/bin/python -I -c '
import json, sys
pk = {p["name"]: p for p in json.load(sys.stdin)["packages"]}
st = pk.get("bim-store") or sys.exit("bim-store is not a workspace member")
rq = [d for d in st["dependencies"] if d["name"] == "rusqlite" and d.get("kind") is None]
if not rq or "bundled" not in rq[0]["features"]:
    sys.exit("bim-store must depend on rusqlite with the bundled feature")
' || fail "crate structure (see above)"

cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
.oracles/py/bin/python -I judges/T3.2/judge.py "$CARGO_TARGET_DIR/debug/bim"
