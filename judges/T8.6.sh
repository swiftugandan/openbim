#!/usr/bin/env bash
# Verification for T8.6: bim-wasm Model.sketch / Model.infer (Node, against `bim sketch` and closed-form
# inference), then the Studio's drawing tools, push/pull, inference feedback, measurements box, keys and
# navigation in headless system Chrome (document read back through `bim sync serve` + a CLI peer, results
# compared with `bim sketch`), and Model.infer latency in Chrome on a 100,000-Shape model.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
# Every subprocess runs under an alarm so a hang in the code under test cannot hang the judge.
to() { perl -e 'alarm shift; exec @ARGV' "$@"; }
[ -d .oracles/node/node_modules/playwright-core ] || fail "playwright-core oracle missing; run loop/setup.sh"
[ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ] || fail "Google Chrome not installed"
[ -f apps/studio/package.json ] || fail "apps/studio missing"
[ -f packages/bim-wasm/build.sh ] || fail "packages/bim-wasm/build.sh missing (T7.1)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
to 600 cargo build -q -p bim-cli || fail "cargo build -p bim-cli failed (or timed out)"
to 600 bash packages/bim-wasm/build.sh >/dev/null || fail "packages/bim-wasm/build.sh failed (or timed out)"
[ -f packages/bim-wasm/pkg-web/package.json ] || fail "packages/bim-wasm/pkg-web not built"
to 300 npm run build --silent --workspace packages/web-viewport >/dev/null || fail "npm run build -w packages/web-viewport failed"
rm -rf apps/studio/dist
to 300 npm run build --silent --workspace apps/studio >/dev/null || fail "npm run build -w apps/studio failed"
[ -f apps/studio/dist/index.html ] || fail "apps/studio/dist/index.html not built"
bim="$CARGO_TARGET_DIR/debug/bim"
# The node scripts print their own JUDGE FAIL line; only an alarm kill (exit 142) needs one here.
node_step() { local secs=$1 label=$2; shift 2; local rc=0; to "$secs" node "$@" || rc=$?; [ $rc -eq 0 ] && return 0; [ $rc -eq 142 ] && fail "$label timed out after $secs s"; exit 1; }
node_step 90 "wasm checks" judges/T8.6/wasm.mjs "$bim" "$tmp"
node_step 170 "Studio checks" judges/T8.6/run.mjs "$bim"
