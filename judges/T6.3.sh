#!/usr/bin/env bash
# Verification for T6.3: drive the real `bim-mcp` binary with the official MCP TypeScript SDK client
# (StdioClientTransport) and check propose_transaction / commit_transaction against the CLI run on a copy (spec/mcp.md).
# The project also holds a trade package `fire` whose engine is the judge's own component (judges/T6.3/plugins,
# built here from source for wasm32-wasip2), so `trades.fire` exercises a WebAssembly engine's output.metrics.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -d .oracles/node/node_modules/@modelcontextprotocol/sdk ] || fail "MCP SDK oracle missing (run loop/setup.sh)"
[ -f crates/bim-mcp/Cargo.toml ] || fail "crate crates/bim-mcp does not exist"
cargo build -q --release --target wasm32-wasip2 --manifest-path judges/T6.3/plugins/Cargo.toml \
  || fail "judge trade-engine component did not build (needs rustup target wasm32-wasip2)"
# Release builds: every proposal and every `solve --trade fire` compiles the WebAssembly engine.
cargo build -q --release -p bim-cli -p bim-mcp || fail "cargo build --release -p bim-cli -p bim-mcp failed"
bim="$CARGO_TARGET_DIR/release/bim"; mcp="$CARGO_TARGET_DIR/release/bim-mcp"
[ -x "$mcp" ] || fail "binary bim-mcp not produced"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
"$bim" init "$tmp/p" >/dev/null || fail "bim init failed"
mkdir -p "$tmp/p/plugins/fire"
cp judges/T6.3/package/trade.json "$tmp/p/plugins/fire/trade.json"
cp "$CARGO_TARGET_DIR/wasm32-wasip2/release/judge_t6_3_fire.wasm" "$tmp/p/plugins/fire/fire.wasm"
"$bim" apply "$tmp/p" judges/T6.3/fixture.json || fail "bim apply of the fixture failed"
node judges/T6.3/judge.mjs "$bim" "$mcp" "$tmp/p" "$tmp"
