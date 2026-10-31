#!/usr/bin/env bash
# Verification for T8.1: build bim-wasm (pkg-web) and packages/web-viewport, then drive them in
# headless system Chrome from the judge's own page (judges/T8.1/page.html).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -d .oracles/node/node_modules/playwright-core ] || fail "playwright-core oracle missing; run loop/setup.sh"
[ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ] || fail "Google Chrome not installed"
[ -f package.json ] || fail "root package.json (npm workspaces) missing"
[ -x packages/bim-wasm/build.sh ] || [ -f packages/bim-wasm/build.sh ] || fail "packages/bim-wasm/build.sh missing (T7.1)"
bash packages/bim-wasm/build.sh >/dev/null || fail "packages/bim-wasm/build.sh failed"
[ -f packages/bim-wasm/pkg-web/package.json ] || fail "packages/bim-wasm/pkg-web not built"
[ -f packages/web-viewport/package.json ] || fail "packages/web-viewport missing"
[ -f node_modules/three/build/three.module.js ] || fail "node_modules/three missing (run npm install at the repo root)"
npm run build --silent --workspace packages/web-viewport >/dev/null || fail "npm run build -w packages/web-viewport failed"
[ -f packages/web-viewport/dist/index.js ] || fail "packages/web-viewport/dist/index.js not built"
exec node judges/T8.1/run.mjs
