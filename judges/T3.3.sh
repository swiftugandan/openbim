#!/usr/bin/env bash
# Verification for T3.3: drive `bim sync serve` / `bim sync` (spec/cli.md) with three peers through one server.
# Every server and client run is bounded by a timeout inside judges/T3.3/judge.py; servers are always killed.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }

[ -f crates/bim-sync/Cargo.toml ] || fail "crate crates/bim-sync is missing"
meta=$(cargo metadata -q --format-version 1 --no-deps) || fail "cargo metadata failed on the workspace"
printf '%s' "$meta" | .oracles/py/bin/python -I -c '
import json, sys
pk = {p["name"]: p for p in json.load(sys.stdin)["packages"]}
sy = pk.get("bim-sync") or sys.exit("bim-sync is not a workspace member")
deps = {d["name"] for d in sy["dependencies"] if d.get("kind") is None}
if not deps & {"tungstenite", "tokio-tungstenite"}:
    sys.exit("bim-sync must use tungstenite for its WebSocket transport")
' || fail "crate structure (see above)"

cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
.oracles/py/bin/python -I judges/T3.3/judge.py "$CARGO_TARGET_DIR/debug/bim"
