#!/usr/bin/env bash
# Integration test of loop/run.sh in cloud mode, in a throwaway repo with stub claude, gh and cargo.
# The stub session behaves according to its task id; see stub_claude below. No network, no real sessions.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d); trap 'chmod -R u+w "$work" 2>/dev/null; find "$work" -delete 2>/dev/null' EXIT
fail() { echo "TEST FAIL: $*" >&2; [ -f "$work/repo/.loop/events.log" ] && cat "$work/repo/.loop/events.log" >&2; exit 1; }

git init -q --bare -b main "$work/origin.git"
git clone -q "$work/origin.git" "$work/repo" 2>/dev/null
cd "$work/repo"
git config user.email t@example.com; git config user.name test
mkdir -p loop judges spec bin
cp "$here/run.sh" "$here/prompt.md" "$here/mcp-empty.json" loop/
printf '.loop/\nbin/\n' > .gitignore
task() { # task <id> <opens> <files>
  printf -- '- [ ] %s: task %s\n  - Files: %s\n  - Opens: %s\n  - Verification: test -f ok-%s\n\n' "$1" "$1" "$3" "$2" "$1"
}
{
  echo '# TASKS'; echo
  task T1.1 none "Cargo.lock, ok-T1.1"          # lock conflict with T1.2 (whichever lands second)
  task T1.2 none "Cargo.lock, ok-T1.2"
  task T2.1 none "shared.txt, ok-T2.1"          # real conflict with T2.2: the second one must run again
  task T2.2 none "shared.txt, ok-T2.2"
  task T3.1 T1.1 "ok-T3.1"                      # waits for T1.1; first attempt strays outside Files:
  task T4.1 none "ok-T4.1"                      # the session always reports FAIL, so it gets blocked
  task T4.2 T4.1 "ok-T4.2"                      # blocked because T4.1 is
} > TASKS.md
echo base > Cargo.lock
git add -A; git commit -q -m init; git push -q origin main

cat > bin/claude <<'EOF'
#!/usr/bin/env bash
# stub_claude: reads the task and branch out of the prompt, does the work in its own clone, pushes.
while [ $# -gt 0 ]; do [ "$1" = --cloud ] && { prompt=$2; break; }; shift; done
id=$(printf '%s\n' "$prompt" | sed -nE 's/^You are the session for task (T[0-9.]+), attempt ([0-9]+).*/\1/p')
attempt=$(printf '%s\n' "$prompt" | sed -nE 's/^You are the session for task (T[0-9.]+), attempt ([0-9]+).*/\2/p')
branch=$(printf '%s\n' "$prompt" | grep -oE 'refs/heads/loop/[^`]+' | tail -1 | sed 's#refs/heads/##')
d=$(mktemp -d); git clone -q "$ORIGIN" "$d" 2>/dev/null; cd "$d" || exit 1
git config user.email t@example.com; git config user.name stub
verdict=PASS
touch "ok-$id"
case $id in
  T1.1|T1.2) echo "lock $id" > Cargo.lock ;;
  T2.1|T2.2) echo "written by $id" > shared.txt ;;
  T3.1) [ "$attempt" = 1 ] && echo stray > stray.txt ;;
  T4.1) rm "ok-$id"; verdict="FAIL: stub says no" ;;
esac
git add -A; git commit -q --allow-empty -m "work" -m "Verdict: $verdict"
git push -q origin "HEAD:refs/heads/$branch"
echo "Created cloud session: stub"; echo "View: https://claude.ai/code/session_stub$RANDOM?from=cli"
EOF
cat > bin/gh <<'EOF'
#!/usr/bin/env bash
# stub_gh: `pr create` does nothing; `pr merge <head> --merge` merges into main with a merge commit.
[ "$2" = merge ] || exit 0
d=$(mktemp -d); git clone -q "$ORIGIN" "$d" 2>/dev/null; cd "$d" || exit 1
git config user.email t@example.com; git config user.name stub
git merge -q --no-ff -m "Merge $3" "origin/$3" && git push -q origin main && git push -q origin --delete "$3"
EOF
cat > bin/cargo <<'EOF'
#!/usr/bin/env bash
# stub_cargo: `cargo metadata` regenerates the lockfile.
echo regenerated > Cargo.lock
EOF
chmod +x bin/*

ORIGIN="$work/origin.git" PATH="$PWD/bin:$PATH" POLL=1 PARALLEL=8 MAX_ITER=30 \
  perl -e 'alarm 120; exec @ARGV' bash loop/run.sh > "$work/out.log" 2>&1 || { cat "$work/out.log" >&2; fail "run.sh exited non-zero"; }

mark() { grep -m1 -E "^- \[.\] $1:" TASKS.md | cut -c4; }
for t in T1.1 T1.2 T2.1 T2.2 T3.1; do [ "$(mark $t)" = x ] || fail "$t should be done, is [$(mark $t)]"; done
[ "$(mark T4.1)" = '!' ] || fail "T4.1 should be blocked"
[ "$(mark T4.2)" = '!' ] || fail "T4.2 should be blocked through T4.1"
[ "$(cat .loop/attempts/T4.1)" = 2 ] || fail "T4.1 should have had exactly two attempts"
[ "$(cat .loop/attempts/T3.1)" = 2 ] || fail "T3.1's stray file should have failed its first attempt"
[ ! -e stray.txt ] || fail "T3.1's stray file reached main"
grep -q 'CONFLICT T2\.' .loop/events.log || fail "T2.x should have conflicted"
resumed=$(grep -l 'conflicts with work landed' .loop/sessions/*-T2.*.prompt.md 2>/dev/null | wc -l | tr -d ' ')
[ "$resumed" = 1 ] || fail "exactly one T2.x rerun should be told to reuse its branch, got $resumed"
grep -q 'refs/heads/loop/T2\.[12]-a1-r0 && git merge --squash' $(grep -l 'conflicts with work landed' .loop/sessions/*-T2.*.prompt.md) \
  || fail "the rerun's prompt should name the conflicting branch"
[ "$(cat Cargo.lock)" = regenerated ] || fail "Cargo.lock conflict should have been regenerated"
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || fail "main should have landed"
git ls-remote origin 'refs/heads/gate/*' | grep -q . && fail "landed gate branches should be gone"
[ -z "$(git status --porcelain)" ] || fail "tree should be clean: $(git status --porcelain)"
[ "$(ls .loop/inflight | wc -l | tr -d ' ')" = 0 ] || fail "nothing should be left in flight"
tail -1 .loop/events.log | grep -q EXIT || fail "run should have exited normally"
for f in T3.1 T4.1; do [ -s ".loop/fail-$f" ] || fail "$f's failure reason was not recorded"; done
grep -q 'outside Files' .loop/fail-T3.1 || fail "T3.1 should fail on the Files: check: $(cat .loop/fail-T3.1)"
grep -q 'stub says no' .loop/fail-T4.1 || fail "T4.1's reason should come from its verdict: $(cat .loop/fail-T4.1)"
echo "PASS: $(grep -c DISPATCHED .loop/events.log) dispatches, $(grep -c CONFLICT .loop/events.log) conflicts, $(grep -c COMMITTED .loop/events.log) landed, $(grep -c BLOCKED .loop/events.log) blocked"
