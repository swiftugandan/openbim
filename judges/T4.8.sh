#!/usr/bin/env bash
# Verification for T4.8: bim-plugin API additions (Metric, default metrics, engine_names), then trade
# packages driven through `bim` with the judge's own `openbim:trade@0.1.0` components, built here from
# source for wasm32-wasip2 (judges/T4.8/plugins), the built-in engines' metrics (tests/metrics.rs on
# fixtures.py solve outputs), engine inputs, solve/validate, glTF and clash of
# extension solids, plugin list trades, and the host rules for trapping/spinning/misnamed/missing engines.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
export PYTHONDONTWRITEBYTECODE=1
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
J=judges/T4.8
[ -x "$py" ] || fail "missing oracle $py (run loop/setup.sh)"
[ -d .oracles/node/node_modules/gltf-validator ] || fail "missing oracle gltf-validator (run loop/setup.sh)"
[ -f crates/bim-plugin/Cargo.toml ] || fail "crate crates/bim-plugin does not exist"
[ -f crates/bim-cli/Cargo.toml ] || fail "crate crates/bim-cli does not exist"

cargo build -q --release --target wasm32-wasip2 --manifest-path "$J/plugins/Cargo.toml" \
  || fail "judge trade-engine components did not build (needs rustup target wasm32-wasip2)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/wasm" "$tmp/run"
for p in fire misnamed trap spin; do
  cp "$CARGO_TARGET_DIR/wasm32-wasip2/release/judge_t4_8_$p.wasm" "$tmp/wasm/$p.wasm"
  # Component-model binary: "\0asm", version 0x0d, layer 1 (a core module has layer 0).
  "$py" -I -c 'import sys; b = open(sys.argv[1], "rb").read(8); sys.exit(b[:4] != b"\0asm" or b[6:8] != b"\x01\x00")' \
    "$tmp/wasm/$p.wasm" || fail "judge component $p.wasm is not a WebAssembly component"
done

# Release build: every engine invocation compiles a component, and the spin check times the fuel bound.
cargo build -q --release -p bim-cli || fail "bim-cli does not build (release)"
bim="$CARGO_TARGET_DIR/release/bim"
mkdir -p "$tmp/fix"
perl -e 'alarm shift; exec @ARGV or die' 300 "$py" -I "$J/fixtures.py" "$bim" "$tmp/fix" \
  || fail "could not produce built-in solve outputs for the metrics test (see above)"
T48_ARCH_JSON="$tmp/fix/arch.json" T48_STRUCT_JSON="$tmp/fix/struct.json" T48_MEP_JSON="$tmp/fix/mep.json" \
  perl -e 'alarm shift; exec @ARGV or die' 300 cargo test -q --manifest-path "$J/Cargo.toml" \
  || fail "bim-plugin API / built-in metrics tests failed or timed out (see above)"
perl -e 'alarm shift; exec @ARGV or die' 600 "$py" -I "$J/judge.py" "$bim" "$tmp/run" "$tmp/wasm" \
  || fail "trade package checks failed or timed out (see above)"
echo "T4.8 judge: all checks passed"
