#!/usr/bin/env bash
# Verification for T1.8: bim-schema KindRegistry API tests (judges/T1.8/tests), then the `bim` CLI
# (kind and extension-prop validation in apply, package loading on every command, plugin list) per
# spec/schema.md (Extension kinds), spec/plugins.md (Trade packages) and spec/cli.md.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
export PYTHONDONTWRITEBYTECODE=1
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
[ -f crates/bim-schema/Cargo.toml ] || fail "crate crates/bim-schema does not exist"
[ -f crates/bim-cli/Cargo.toml ] || fail "crate crates/bim-cli does not exist"
cargo test -q --manifest-path judges/T1.8/Cargo.toml --no-run \
  || fail "judge does not compile against the pinned bim-schema API (see above)"
perl -e 'alarm shift; exec @ARGV or die' 120 cargo test -q --manifest-path judges/T1.8/Cargo.toml \
  || fail "bim-schema KindRegistry API tests failed or timed out (see above)"
cargo build -q -p bim-cli || fail "bim-cli does not build"
perl -e 'alarm shift; exec @ARGV or die' 240 "$py" -I judges/T1.8/cli.py "$CARGO_TARGET_DIR/debug/bim" \
  || fail "bim CLI kind-registry checks failed or timed out (see above)"
echo "T1.8 judge: all checks passed"
