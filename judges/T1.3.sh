#!/usr/bin/env bash
# Verification for T1.3: run the external judge tests against the builder's crate.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
cargo test -q --manifest-path judges/T1.3/Cargo.toml
