#!/usr/bin/env bash
# Verification for T7.2: run the external judge tests against the builder's bim-render crate.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
cargo test -q --manifest-path judges/T7.2/Cargo.toml || { echo "JUDGE FAIL: judges/T7.2 tests failed (see above)" >&2; exit 1; }
