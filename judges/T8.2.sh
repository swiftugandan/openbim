#!/usr/bin/env bash
# Verification for T8.2: render T7.4-style SVG sheets with createSheetView in headless system Chrome
# at dpr 1 and 2 and measure the pixels.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -d .oracles/node/node_modules/playwright-core ] || fail "playwright-core oracle missing; run loop/setup.sh"
[ -x .oracles/py/bin/python ] || fail "Python oracle missing; run loop/setup.sh"
[ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ] || fail "Google Chrome not installed"
[ -f packages/web-viewport/package.json ] || fail "packages/web-viewport missing"
cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed"
if [ ! -f packages/bim-wasm/pkg-web/package.json ]; then
  bash packages/bim-wasm/build.sh >/dev/null || fail "packages/bim-wasm/build.sh failed"
fi
npm run build --silent --workspace packages/web-viewport >/dev/null || fail "npm run build -w packages/web-viewport failed"
[ -f packages/web-viewport/dist/index.js ] || fail "packages/web-viewport/dist/index.js not built"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
.oracles/py/bin/python -I judges/T8.2/realsheet.py "$CARGO_TARGET_DIR/debug/bim" "$tmp/real.svg" "$tmp/hatch.json"
node judges/T8.2/run.mjs "$tmp/real.svg" "$tmp/hatch.json"
