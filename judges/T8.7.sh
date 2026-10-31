#!/usr/bin/env bash
# Verification for T8.7: the Studio's move/copy/array, rotate, scale, offset, follow-me, tape, eraser, paint,
# make-component, component editing and scenes in headless system Chrome. The document is read back through
# `bim sync serve` + a CLI peer and every result is compared with the same `bim sketch` command (or
# `bim catalog use`) replayed on the Studio's pre-state; axe-core and overflow at 390 and 1440 px.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
# Every subprocess runs under an alarm so a hang in the code under test cannot hang the judge.
to() { perl -e 'alarm shift; exec @ARGV' "$@"; }
[ -d .oracles/node/node_modules/playwright-core ] || fail "playwright-core oracle missing; run loop/setup.sh"
[ -d .oracles/node/node_modules/@axe-core/playwright ] || fail "@axe-core/playwright oracle missing; run loop/setup.sh"
[ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ] || fail "Google Chrome not installed"
[ -f apps/studio/package.json ] || fail "apps/studio missing"
[ -f packages/bim-wasm/build.sh ] || fail "packages/bim-wasm/build.sh missing (T7.1)"
to 600 cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed (or timed out)"
to 600 bash packages/bim-wasm/build.sh >/dev/null || fail "packages/bim-wasm/build.sh failed (or timed out)"
to 300 npm run build --silent --workspace packages/web-viewport >/dev/null || fail "npm run build -w packages/web-viewport failed"
rm -rf apps/studio/dist
to 300 npm run build --silent --workspace apps/studio >/dev/null || fail "npm run build -w apps/studio failed"
[ -f apps/studio/dist/index.html ] || fail "apps/studio/dist/index.html not built"
# run.mjs prints its own JUDGE FAIL line; only an alarm kill (exit 142) needs one here.
rc=0
to 170 node judges/T8.7/run.mjs "$CARGO_TARGET_DIR/debug/bim" || rc=$?
[ $rc -eq 142 ] && fail "Studio checks timed out after 170 s"
exit $rc
