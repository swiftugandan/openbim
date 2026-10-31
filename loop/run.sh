#!/usr/bin/env bash
# Outer loop: pick the first [ ] task in TASKS.md, run one fresh `claude -p` session for it,
# then gate its verdict and commit (PASS) or retry/block (FAIL). All state lives in files and git.
set -uo pipefail
cd "$(dirname "$0")/.."

MAX_ITER=${MAX_ITER:-12}              # session attempts before the loop stops on its own
SESSION_TIMEOUT=${SESSION_TIMEOUT:-1800}  # seconds; only cuts off a hung session
MODEL=${LOOP_MODEL:-opus}             # the session itself; stages pick their own models
mkdir -p .loop/sessions .loop/verdict .loop/attempts .loop/probes

event() { echo "$(date +%H:%M:%S) LOOP $*" >> .loop/events.log; echo "[loop] $*"; }

# --- TASKS.md helpers -------------------------------------------------------
next_task() { grep -m1 -E '^- \[ \] T[0-9.]+:' TASKS.md | sed -E 's/^- \[ \] (T[0-9.]+):.*/\1/'; }
mark_of()   { grep -m1 -E "^- \[.\] $1:" TASKS.md | cut -c4; }
block_of()  { awk -v id="$1" '/^- \[.\] /{on = (index($0, "] " id ":") > 0)} on' TASKS.md; }
field()     { block_of "$1" | sed -nE "s/^  - $2: //p" | head -1; }
files_of()  { field "$1" Files | tr ',' '\n' | sed -E 's/^ +| +$//g' | grep -v '^$'; }

tick() { ID=$1 perl -pi -e 's/^- \[ \] (\Q$ENV{ID}\E:)/- [x] $1/' TASKS.md; }
block() { # block <id> <reason>: mark [!] with its reason and commit the mark
  local id=$1 reason=$2
  ID=$id R=$reason perl -0pi -e 's/^- \[ \] (\Q$ENV{ID}\E:[^\n]*\n)/- [!] $1  - Blocked: $ENV{R}\n/m' TASKS.md
  git commit -q -m "loop: block $id" -m "Blocked: $reason" -- TASKS.md
  event "BLOCKED $id: $reason"
}

# --- the gate: never trust the session's own word ---------------------------
gate() { # gate <id>: prints a reason and returns 1 if the work must not be committed
  local id=$1 verdict path ok f
  verdict=$(head -1 ".loop/verdict/$id" 2>/dev/null)
  [ "$verdict" = PASS ] || { echo "${verdict:-no verdict written}"; return 1; }
  if [ -n "$(git status --porcelain -- judges loop .gitignore TASKS.md)" ]; then
    echo "session modified judges/, loop/ or TASKS.md"; return 1
  fi
  while read -r path; do
    ok=0
    while read -r f; do [[ "$path" == "$f" || "$path" == "$f"* ]] && ok=1; done < <(files_of "$id")
    [ $ok = 1 ] || { echo "changed $path outside Files:"; return 1; }
  done < <(git status --porcelain --untracked-files=all | cut -c4-)
  perl -e 'alarm 300; exec @ARGV' bash -c "$(field "$id" Verification)" > ".loop/gate-$id.log" 2>&1 \
    || { echo "loop's own Verification run failed"; return 1; }
}

commit_pass() {
  local id=$1
  tick "$id"
  files_of "$id" | while read -r f; do git add -A -- "$f" 2>/dev/null; done
  git add TASKS.md
  git commit -q -F - <<EOF || { event "COMMIT FAILED for $id; stopping"; exit 1; }
$id: $(grep -m1 -E "^- \[.\] $id:" TASKS.md | sed -E 's/^- \[.\] [^:]+: //')

Verification: $(field "$id" Verification) (passed, rerun by loop/run.sh)
Audit: PASS (session log $(basename "$2"))

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
  event "COMMITTED $id $(git rev-parse --short HEAD)"
}

# --- one session ------------------------------------------------------------
run_session() { # run_session <id> <attempt> <log>
  local id=$1 attempt=$2 log=$3 previous=""
  [ "$attempt" -gt 1 ] && previous="
Attempt $((attempt - 1)) failed: $(cat ".loop/fail-$id" 2>/dev/null). You start again from the
committed tree; avoid that failure.
"
  ID=$id ATTEMPT=$attempt BLOCK=$(block_of "$id") VERIFY=$(field "$id" Verification) PREVIOUS=$previous \
    perl -pe 's/\{\{(\w+)\}\}/$ENV{$1}/g' loop/prompt.md > "${log%.jsonl}.prompt.md"
  perl -e 'alarm shift; exec @ARGV' "$SESSION_TIMEOUT" \
    claude -p --model "$MODEL" --permission-mode bypassPermissions \
      --strict-mcp-config --mcp-config loop/mcp-empty.json \
      --output-format stream-json --verbose \
      < "${log%.jsonl}.prompt.md" > "$log" 2>&1
}

# --- main loop ---------------------------------------------------------------
event "START max_iter=$MAX_ITER timeout=${SESSION_TIMEOUT}s model=$MODEL"
for ((iter = 1; iter <= MAX_ITER; iter++)); do
  [ -e .loop/stop ] && { event "STOP file found"; break; }
  id=$(next_task)
  [ -z "$id" ] && { event "DONE no [ ] tasks remain"; break; }

  dep_blocked=""
  for dep in $(field "$id" Opens | tr ',' ' '); do
    [ "$(mark_of "$dep" 2>/dev/null)" = "!" ] && dep_blocked=$dep
  done
  if [ -n "$dep_blocked" ]; then block "$id" "Opens $dep_blocked, which is blocked"; ((iter--)); continue; fi

  attempt=$(( $(cat ".loop/attempts/$id" 2>/dev/null || echo 0) + 1 ))
  echo "$attempt" > ".loop/attempts/$id"
  log=".loop/sessions/$(date +%Y%m%d-%H%M%S)-$id-a$attempt.jsonl"
  echo "$id $attempt $log" > .loop/current
  rm -f ".loop/verdict/$id"
  event "SESSION $id attempt $attempt -> $log"

  run_session "$id" "$attempt" "$log"
  status=$?
  [ $status = 142 ] && echo "FAIL: session timed out after ${SESSION_TIMEOUT}s" > ".loop/verdict/$id"
  event "GATE $id (session exit $status)"

  if reason=$(gate "$id"); then
    commit_pass "$id" "$log"
  else
    reason=${reason#FAIL: }
    echo "$reason" > ".loop/fail-$id"
    # Set the failed work aside (kept in `git stash list`) so the next session starts clean.
    git stash push -u -q -m "loop: $id attempt $attempt failed: $reason" 2>/dev/null
    event "FAILED $id attempt $attempt: $reason"
    [ "$attempt" -ge 2 ] && block "$id" "$reason"
  fi
done
rm -f .loop/current
event "EXIT"
