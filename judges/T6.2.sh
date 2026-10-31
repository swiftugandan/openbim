#!/usr/bin/env bash
# Verification for T6.2: drive the real `bim-mcp` binary with the official MCP TypeScript SDK client
# (StdioClientTransport) and compare each tool/resource with the CLI contract (spec/mcp.md, spec/cli.md).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -d .oracles/node/node_modules/@modelcontextprotocol/sdk ] || fail "MCP SDK oracle missing (run loop/setup.sh)"
cargo build -q -p bim-cli -p bim-mcp || fail "cargo build -p bim-cli -p bim-mcp failed"
bim="$CARGO_TARGET_DIR/debug/bim"; mcp="$CARGO_TARGET_DIR/debug/bim-mcp"
[ -x "$mcp" ] || fail "binary bim-mcp not produced"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
"$bim" init "$tmp/p" >/dev/null || fail "bim init failed"
"$bim" apply "$tmp/p" judges/T6.2/fixture.json || fail "bim apply of the fixture failed"
node judges/T6.2/judge.mjs "$bim" "$mcp" "$tmp/p" "$tmp"
