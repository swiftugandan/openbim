#!/usr/bin/env bash
# Verification for T4.1: bim-plugin API (Rust judge) + the WASM component host driven through `bim`.
# The sample plugins under judges/T4.1/plugins are built here from source for wasm32-wasip2.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
py=.oracles/py/bin/python
J=judges/T4.1

cargo build -q -p bim-cli || fail "cannot build bim-cli"
bim="$CARGO_TARGET_DIR/debug/bim"
cargo build -q --release --target wasm32-wasip2 --manifest-path "$J/plugins/Cargo.toml" \
  || fail "judge plugins did not build (needs rustup target wasm32-wasip2)"
cargo test -q --manifest-path "$J/Cargo.toml" || fail "bim-plugin API tests failed"

tmp=$(mktemp -d)
lpid=""
trap '[ -n "$lpid" ] && kill "$lpid" 2>/dev/null; rm -rf "$tmp"' EXIT
mkdir -p "$tmp/wasm" "$tmp/cwd"
for p in sample trap spin probe hog; do
  cp "$CARGO_TARGET_DIR/wasm32-wasip2/release/judge_t4_1_$p.wasm" "$tmp/wasm/$p.wasm"
  # Component-model binary: "\0asm", version 0x0d, layer 1 (a core module has layer 0).
  "$py" -I -c 'import sys; b = open(sys.argv[1], "rb").read(8); sys.exit(b[:4] != b"\0asm" or b[6:8] != b"\x01\x00")' \
    "$tmp/wasm/$p.wasm" || fail "judge plugin $p.wasm is not a WebAssembly component"
done

# run <seconds> <cmd…>: bounded run; sets rc, stdout in $tmp/out, stderr in $tmp/err. 142 = timed out.
run() {
  local secs=$1; shift
  set +e; perl -e '$t = shift; alarm $t; exec @ARGV or die "exec: $!\n"' "$secs" "$@" >"$tmp/out" 2>"$tmp/err"; rc=$?; set -e
  [ "$rc" != 142 ] || fail "'$*' did not finish within ${secs}s"
}
expect_rc() { # expect_rc <want> <what>
  [ "$rc" = "$1" ] || fail "$2: exit $rc, want $1; stdout: $(head -c 600 "$tmp/out"); stderr: $(head -c 600 "$tmp/err")"
}
check() { # check <expected violations json> <what>
  "$py" -I "$J/check.py" "$tmp/out" "$1" 2>"$tmp/chk" || fail "$2: $(cat "$tmp/chk")"
}

wall() { # wall <id> <y> <height>
  printf '{"op":"create","id":"%s","kind":"Wall","props":{"level":{"ref":"L1"},"type":{"ref":"WT"},"x1":0,"y1":%s,"x2":4,"y2":%s,"height":%s}}' "$1" "$2" "$2" "$3"
}
A="$tmp/A"
run 30 "$bim" init "$A"; expect_rc 0 "bim init"
cat > "$tmp/a.json" <<JSON
{"schema": 1, "ops": [
  {"op":"create","id":"L1","kind":"Level","props":{"elevation":0}},
  {"op":"create","id":"WT","kind":"WallType","props":{"layer_thickness":[0.2],"layer_material":["concrete"],"layer_conductivity":[1.6]}},
  $(wall W1 0 2.5), $(wall W2 5 3.2), $(wall W3 10 4.0), $(wall W4 15 5.0),
  {"op":"delete","id":"W4"}
]}
JSON
run 30 "$bim" apply "$A" "$tmp/a.json"; expect_rc 0 "bim apply (JSON ops)"

# 1. Sample plugin: walls taller than 3.0 violate; the host passes all 5 live entities (W4 is deleted),
#    ids as created, Refs as {"ref": id}, Lists as arrays (checked inside the plugin).
run 60 "$bim" validate "$A" --standard "$tmp/wasm/sample.wasm" --json
expect_rc 1 "validate with sample plugin (2 violations)"
check '[["H-MAX","W2","n=5",3.2,3.0],["H-MAX","W3","n=5",4.0,3.0]]' "sample plugin"

printf 'set W2 height num 3.0\nset W3 height num 2\n' > "$tmp/fix.ops"
run 30 "$bim" apply "$A" "$tmp/fix.ops"; expect_rc 0 "bim apply (line ops)"
run 60 "$bim" validate "$A" --standard "$tmp/wasm/sample.wasm" --json
expect_rc 0 "validate with sample plugin on a compliant model (3.0 is not > 3.0)"
check '[]' "sample plugin, compliant model"
run 30 "$bim" hash "$A"; expect_rc 0 "bim hash"; h0=$(cat "$tmp/out")

# 2. A trapping plugin and a never-returning plugin (fuel) exit 3 with a message; so does one that
#    tries to touch 1 GiB of linear memory (memory bound).
for p in trap spin hog; do
  run 60 "$bim" validate "$A" --standard "$tmp/wasm/$p.wasm" --json
  [ "$rc" = 3 ] || fail "$p plugin: exit $rc, want 3; stdout: $(head -c 400 "$tmp/out"); stderr: $(head -c 400 "$tmp/err")"
  [ -s "$tmp/err" ] || fail "$p plugin: exit 3 without a message on stderr"
done

# 3. No filesystem or network: the probe gets no file it could read/list/write and no TCP connection.
"$py" -I "$J/listen.py" "$tmp/connected" > "$tmp/port" & lpid=$!
for _ in $(seq 100); do [ -s "$tmp/port" ] && break; perl -e 'select undef,undef,undef,0.05'; done
port=$(head -1 "$tmp/port"); [ -n "$port" ] || fail "judge TCP listener did not start"
echo "top secret" > "$tmp/secret.txt"
P="$tmp/P"
run 30 "$bim" init "$P"; expect_rc 0 "bim init (probe)"
printf '{"schema":1,"ops":[{"op":"create","id":"I1","kind":"Issue","props":{"title":"%s","status":"%s","entities":[]}}]}' \
  "$tmp/secret.txt" "$port" > "$tmp/p.json"
run 30 "$bim" apply "$P" "$tmp/p.json"; expect_rc 0 "bim apply (probe)"
cd "$tmp/cwd"
run 60 "$bim" validate "$P" --standard "$tmp/wasm/probe.wasm" --json
cd - >/dev/null
if [ "$rc" = 1 ]; then
  check '[["PROBE-DONE","*","path=",0,0]]' "sandbox probe (any ESCAPE-* rule means the plugin got access)"
elif [ "$rc" != 3 ]; then
  fail "sandbox probe: exit $rc, want 1 (ran, all access denied) or 3 (refused); stdout: $(head -c 400 "$tmp/out")"
fi
perl -e 'select undef,undef,undef,0.3'
[ ! -e "$tmp/connected" ] || fail "plugin opened a TCP connection to the judge's listener"
[ ! -e "$tmp/cwd/probe-rel.pwned" ] && [ ! -e "$tmp/secret.txt.pwned" ] && [ ! -e "$P/probe-rel.pwned" ] \
  || fail "plugin wrote a file on the host"

# 4. Plugins never change the document; bad inputs are usage errors, never crashes.
run 30 "$bim" hash "$A"; [ "$(cat "$tmp/out")" = "$h0" ] || fail "running plugins changed the document hash"
printf 'not a wasm component' > "$tmp/wasm/garbage.wasm"
run 60 "$bim" validate "$A" --standard "$tmp/wasm/garbage.wasm" --json
expect_rc 2 "a file that is not a valid rule-set component (spec/plugins.md: exit 2)"
run 30 "$bim" validate "$A" --standard no-such-pack-name --json
expect_rc 2 "unknown --standard"
run 30 "$bim" solve "$A" --trade no-such-trade --json
expect_rc 2 "unknown --trade (resolved through the registry)"
run 60 "$bim" solve "$A" --trade arch --json
expect_rc 0 "solve --trade arch through the registry"
"$py" -I -c 'import json,sys; d=json.load(open(sys.argv[1])); assert isinstance(d.get("walls"), list) and len(d["walls"]) == 3, d' \
  "$tmp/out" 2>/dev/null || fail "solve --trade arch did not report the 3 live walls"
echo "T4.1 judge: all checks passed"
