#!/usr/bin/env bash
# Verification for T1.5: drive the real `bim` binary through the spec in TASKS.md.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.build/judges"
cargo build -q -p bim-cli
bim="$CARGO_TARGET_DIR/debug/bim"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fail() { echo "JUDGE FAIL: $*" >&2; exit 1; }
expect_exit() { # expect_exit <code> <cmd...>
  local want=$1; shift
  set +e; "$@" >"$tmp/out" 2>&1; local got=$?; set -e
  [ "$got" = "$want" ] || fail "'$*' exited $got, want $want: $(cat "$tmp/out")"
}

printf 'create wall Wall\ncreate lvl Level\nset wall height num 3.5\nset wall host ref lvl\nset wall name text north\n' > "$tmp/a.ops"
printf 'create lvl Level\ncreate wall Wall\nset wall name text north\nset wall height num 9\nset wall height num 3.5\nset wall host ref lvl\n' > "$tmp/b.ops"

"$bim" init "$tmp/A"; "$bim" init "$tmp/B"
[ -f "$tmp/A/doc.oplog" ] || fail "init did not create doc.oplog"
"$bim" apply "$tmp/A" "$tmp/a.ops"
"$bim" apply "$tmp/B" "$tmp/b.ops"

hash_a=$("$bim" hash "$tmp/A")
[[ "$hash_a" =~ ^[0-9a-f]{16}$ ]] || fail "hash format: '$hash_a'"
[ "$hash_a" = "$("$bim" hash "$tmp/A")" ] || fail "hash not deterministic"
expect_exit 0 "$bim" diff "$tmp/A" "$tmp/B"; grep -qx same "$tmp/out" || fail "diff should print same"
expect_exit 0 "$bim" validate "$tmp/A"

# A file with one bad line is rejected whole and leaves the log untouched.
printf 'set wall height num 4\nbogus op here\n' > "$tmp/bad.ops"
expect_exit 2 "$bim" apply "$tmp/A" "$tmp/bad.ops"
printf 'set ghost height num 1\n' > "$tmp/ghost.ops"
expect_exit 2 "$bim" apply "$tmp/A" "$tmp/ghost.ops"
[ "$hash_a" = "$("$bim" hash "$tmp/A")" ] || fail "rejected apply changed the document"

# Deleting the level leaves a dangling host ref.
printf 'delete lvl\n' > "$tmp/del.ops"
"$bim" apply "$tmp/A" "$tmp/del.ops"
expect_exit 1 "$bim" validate "$tmp/A"
[ "$(wc -l < "$tmp/out")" -ge 1 ] || fail "validate printed no dangling ref"
expect_exit 1 "$bim" diff "$tmp/A" "$tmp/B"; grep -qx differ "$tmp/out" || fail "diff should print differ"
echo "T1.5 judge: all checks passed"
