#!/usr/bin/env bash
# Verification for T7.1: run packages/bim-wasm/build.sh (wasm-pack, nodejs + web targets), then check the
# spec/web.md Model contract in Node: zero-copy views on wasm memory, geometry equal to the native glTF
# export, generation/hash/solve/exportView equal to the CLI on the same ops.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
[ -f packages/bim-wasm/build.sh ] || fail "packages/bim-wasm/build.sh missing"
rm -rf packages/bim-wasm/pkg-node packages/bim-wasm/pkg-web
bash packages/bim-wasm/build.sh >/dev/null || fail "packages/bim-wasm/build.sh failed"
for d in pkg-node pkg-web; do [ -d "packages/bim-wasm/$d" ] || fail "build.sh did not produce packages/bim-wasm/$d"; done
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
node judges/T7.1/judge.mjs "$CARGO_TARGET_DIR/debug/bim" "$tmp"
