#!/usr/bin/env bash
# Verification for T8.3: build the Studio (apps/studio -> dist) and drive it in headless system Chrome.
set -euo pipefail
cd "$(dirname "$0")/.."
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
[ -d .oracles/node/node_modules/playwright-core ] || fail "playwright-core oracle missing; run loop/setup.sh"
[ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ] || fail "Google Chrome not installed"
[ -f apps/studio/package.json ] || fail "apps/studio missing"
[ -f packages/bim-wasm/build.sh ] || fail "packages/bim-wasm/build.sh missing (T7.1)"
export CARGO_TARGET_DIR="$PWD/.build/judges"
bash packages/bim-wasm/build.sh >/dev/null || fail "packages/bim-wasm/build.sh failed"
npm run build --silent --workspace packages/web-viewport >/dev/null || fail "npm run build -w packages/web-viewport failed"
rm -rf apps/studio/dist
npm run build --silent --workspace apps/studio >/dev/null || fail "npm run build -w apps/studio failed"
[ -f apps/studio/dist/index.html ] || fail "apps/studio/dist/index.html not built"
exec node judges/T8.3/run.mjs
