#!/usr/bin/env bash
# Outer loop: work the [ ] tasks in TASKS.md whose Opens: are all [x], one fresh session per attempt,
# then gate each verdict on this machine and land it on main through a PR (PASS) or retry/block (FAIL).
# Sessions run in the cloud (`claude --cloud`, up to PARALLEL at once, each pushing its own branch) or,
# for LOCAL_TASKS and when RUNNER=local, here with `claude -p`, one at a time.
# All state lives in files and git. Run it from a clean, pushed main.
set -uo pipefail
cd "$(dirname "$0")/.."

MAX_ITER=${MAX_ITER:-100}             # session attempts before the loop stops on its own
SESSION_TIMEOUT=${SESSION_TIMEOUT:-3600}  # seconds; only cuts off a hung session
MODEL=${LOOP_MODEL:-opus}             # the session itself; stages pick their own models
RUNNER=${RUNNER:-cloud}                # cloud | local (local runs every task here)
PARALLEL=${PARALLEL:-4}                # cloud sessions at once
CLOUD_TIMEOUT=${CLOUD_TIMEOUT:-14400}   # seconds before a cloud session counts as a failed attempt
POLL=${POLL:-60}                       # seconds between checks of in-flight branches
CLOUD_ENV=${CLOUD_ENV:-env_01BpHcnvu9sLjWWCLHVgChPt}  # the cloud environment holding loop/cloud-env-setup.sh
# Tasks the cloud cannot judge: GPU and frame-time gates, Chrome-driven UI judges, the git-ignored IFC corpus.
LOCAL_TASKS=${LOCAL_TASKS:-'^T(5\.4|7\.3|7\.5|8\..+)$'}
mkdir -p .loop/sessions .loop/verdict .loop/attempts .loop/rebases .loop/inflight .loop/probes

event() { echo "$(date +%H:%M:%S) LOOP $*" >> .loop/events.log; echo "[loop] $*"; }

# --- TASKS.md helpers -------------------------------------------------------
next_task() { grep -m1 -E '^- \[ \] T[0-9.]+:' TASKS.md | sed -E 's/^- \[ \] (T[0-9.]+):.*/\1/'; }
mark_of()   { grep -m1 -E "^- \[.\] $1:" TASKS.md | cut -c4; }
block_of()  { awk -v id="$1" '/^- \[.\] /{on = (index($0, "] " id ":") > 0)} on' TASKS.md; }
field()     { block_of "$1" | sed -nE "s/^  - $2: //p" | head -1; }
list_of()   { field "$1" "$2" | tr ',' '\n' | sed -E 's/^ +| +$//g' | grep -v '^$'; }
files_of()  { list_of "$1" Files; }

tick() { ID=$1 perl -pi -e 's/^- \[ \] (\Q$ENV{ID}\E:)/- [x] $1/' TASKS.md; }
block() { # block <id> <reason>: mark [!] with its reason and commit the mark
  local id=$1 reason=$2
  ID=$id R=$reason perl -0pi -e 's/^- \[ \] (\Q$ENV{ID}\E:[^\n]*\n)/- [!] $1  - Blocked: $ENV{R}\n/m' TASKS.md
  git commit -q -m "loop: block $id" -m "Blocked: $reason" -- TASKS.md
  land "block-$id" "loop: block $id" || { event "LANDING FAILED for block of $id; stopping"; exit 1; }
  event "BLOCKED $id: $reason"
}
open_tasks() { grep -E '^- \[ \] T[0-9.]+:' TASKS.md | sed -E 's/^- \[ \] (T[0-9.]+):.*/\1/'; }
runs_here() { [ "$RUNNER" = local ] || [[ "$1" =~ $LOCAL_TASKS ]]; }

# --- the gate: never trust the session's own word ---------------------------
gate() { # gate <id>: prints a reason and returns 1 if the work must not be committed
  local id=$1 verdict path ok f
  verdict=$(head -1 ".loop/verdict/$id" 2>/dev/null)
  [ "$(git rev-parse HEAD)" = "$pre" ] || { echo "session made its own commits"; return 1; }
  [ "$verdict" = PASS ] || { echo "${verdict:-no verdict written (session exit $status: $(tail -c 120 "$log" | tr '\n' ' '))}"; return 1; }
  if [ -n "$(git status --porcelain -- judges loop spec .gitignore TASKS.md)" ]; then
    echo "session modified judges/, loop/, spec/ or TASKS.md"; return 1
  fi
  while read -r path; do
    ok=0
    while read -r f; do [[ "$path" == "$f" || "$path" == "$f"* ]] && ok=1; done < <(files_of "$id")
    [ $ok = 1 ] || { echo "changed $path outside Files:"; return 1; }
  done < <(git status --porcelain --untracked-files=all | cut -c4-)
  perl -e 'alarm 900; exec @ARGV' bash -c "$(field "$id" Verification)" > ".loop/gate-$id.log" 2>&1 \
    || { echo "loop's own Verification run failed"; return 1; }
  # Regression gate: every task already done must still pass its own judge.
  for done in $(grep -E '^- \[x\] T[0-9.]+:' TASKS.md | sed -E 's/^- \[x\] (T[0-9.]+):.*/\1/'); do
    perl -e 'alarm 900; exec @ARGV' bash -c "$(field "$done" Verification)" >> ".loop/gate-$id.log" 2>&1 \
      || { echo "regression: $done's Verification now fails"; return 1; }
  done
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
Tier: $(tier_of "$id" "$(cat ".loop/attempts/$id")")

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
  land "$id" "$(git log -1 --format=%s)" || { event "LANDING FAILED for $id; stopping"; exit 1; }
  event "COMMITTED $id $(git rev-parse --short HEAD)"
}

# main is protected, so every commit reaches it through a PR. Land before dispatching dependents:
# cloud sessions clone origin/main. --merge keeps the local commit an ancestor, so main fast-forwards.
land() { # land <name> <title>
  local head; head="gate/$1-$(git rev-parse --short HEAD)"
  git push -q origin "HEAD:refs/heads/$head" || return 1
  gh pr create --base main --head "$head" --title "$2" \
    --body "$(printf 'Gated by loop/run.sh.\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)')" > /dev/null || return 1
  local try merged=0   # mergeability is computed asynchronously, so an immediate merge can fail
  for try in 1 2 3 4 5; do
    gh pr merge "$head" --merge --delete-branch > /dev/null 2>&1 && { merged=1; break; }
    sleep $((try * 5))
  done
  [ $merged = 1 ] || return 1
  git fetch -q origin main && git merge -q --ff-only origin/main
}

# --- model tiers ------------------------------------------------------------
# A task's `Builder:` (sonnet | opus | fable, default opus) sets its tier. A fresh retry after a
# failed attempt escalates one tier, so fable runs only on high-stakes tasks or on a task about
# to block its dependents.
tier_of() { # tier_of <id> <attempt>
  local t; t=$(field "$1" Builder); t=${t:-opus}
  if [ "$2" -ge 2 ]; then case $t in sonnet) t=opus ;; opus) t=fable ;; esac; fi
  echo "$t"
}
stage_model() { # stage_model <tier> build|fix|audit -> "`model`, effort `e`" as the prompt renders it
  case "$1/$2" in
    sonnet/build|sonnet/fix) echo '`sonnet`, effort `max`' ;;
    sonnet/audit|opus/audit|opus/fix) echo '`opus`, effort `high`' ;;
    opus/build) echo '`opus`, effort `medium`' ;;
    fable/*) echo '`fable`' ;;
  esac
}
plan_stage() { # plan_stage <tier> <id>: sonnet builds from an opus plan
  [ "$1" = sonnet ] || return 0
  cat <<EOF
- **PLAN**: Agent, model \`opus\`, effort \`high\`, description \`PLAN: $2\`.
  Give it the task block, the hard rules, the relevant \`spec/\` files, the judge files and the Scout report.
  It writes \`.loop/probes/$2/plan.md\`: the module layout and public signatures, the algorithm for each
  Spec clause with its edge cases and tolerances, the order to build and test in, and the pitfalls the judge
  will catch. It writes no product code. Hand the whole plan to BUILD and to the fix round.
EOF
}

# --- one session ------------------------------------------------------------
render_prompt() { # render_prompt <id> <attempt> <prompt-file> <git-rule> <finish> [<resume-branch>]
  local id=$1 attempt=$2 previous=""
  [ "$attempt" -gt 1 ] && previous="
Attempt $((attempt - 1)) failed: $(cat ".loop/fail-$id" 2>/dev/null). You start again from the
committed tree; avoid that failure.
"
  [ -n "${6:-}" ] && previous="$previous
An earlier run of this task passed CHECK and AUDIT on branch \`$6\`, but it conflicts with work landed on
main since. Do not build from scratch. Run \`git fetch origin refs/heads/$6 && git merge --squash --no-commit FETCH_HEAD\`,
then resolve the conflicts so both the landed behaviour and this task's are kept. For \`Cargo.lock\`, restore HEAD's
copy and regenerate it with \`cargo metadata --format-version 1\`. Skip SCOUT and PLAN, and give BUILD only the
conflict resolution and whatever the judge then reports. Run CHECK and AUDIT in full as written below.
"
  local tier; tier=$(tier_of "$id" "$attempt")
  ID=$id ATTEMPT=$attempt BLOCK=$(block_of "$id") VERIFY=$(field "$id" Verification) PREVIOUS=$previous \
    TIER=$tier PLAN=$(plan_stage "$tier" "$id") GIT_RULE=$4 FINISH=$5 \
    BUILD_MODEL=$(stage_model "$tier" build) FIX_MODEL=$(stage_model "$tier" fix) \
    AUDIT_MODEL=$(stage_model "$tier" audit) \
    perl -pe 's/\{\{(\w+)\}\}/$ENV{$1}/g' loop/prompt.md > "$3"
}

run_session() { # run_session <id> <attempt> <log>: here, blocking
  local id=$1 attempt=$2 log=$3
  render_prompt "$id" "$attempt" "${log%.jsonl}.prompt.md" \
    '- Do not run `git commit`, `git add`, `git stash`, `git reset` or `git checkout`; the loop commits for you.' \
    "Finish by writing the file \`.loop/verdict/$id\` with exactly one line:
\`PASS\` if the final CHECK and AUDIT both passed, otherwise \`FAIL: <one-line reason>\`.
Then stop. Do not tick TASKS.md and do not commit; the loop does both after re-checking."
  perl -e 'alarm shift; exec @ARGV' "$SESSION_TIMEOUT" \
    claude -p --model "$MODEL" --permission-mode bypassPermissions \
      --strict-mcp-config --mcp-config loop/mcp-empty.json \
      --output-format stream-json --verbose \
      < "${log%.jsonl}.prompt.md" > "$log" 2>&1
}

dispatch() { # dispatch <id> <attempt> <log> <branch>: start a cloud session and return
  local id=$1 attempt=$2 log=$3 branch=$4 resume=${5:-} prompt="${3%.jsonl}.prompt.md" out="${3%.jsonl}.dispatch"
  render_prompt "$id" "$attempt" "$prompt" \
    "- Do not run \`git stash\`, \`git reset\` or \`git checkout\`, do not open a pull request, and never push to \`main\`. Only the final step below commits or pushes." \
    "Finish by committing every change under Files: and pushing once, as the very last thing you do, even on failure
(use \`git commit --allow-empty\` if there is nothing to commit). Do not push before this point.
The commit message's last line must be exactly \`Verdict: PASS\` if the final CHECK and AUDIT both passed,
otherwise \`Verdict: FAIL: <one-line reason>\`. Then run \`git push origin HEAD:refs/heads/$branch\` and stop.
Do not tick TASKS.md; the loop does that after re-checking on its own machine." "$resume"
  # --cloud insists on a terminal even though it only creates the session and exits; `script` gives it one.
  perl -e 'alarm 180; exec @ARGV' script -q "$out" claude \
    --settings "{\"remote\":{\"defaultEnvironmentId\":\"$CLOUD_ENV\"}}" --cloud "$(cat "$prompt")" < /dev/null > /dev/null 2>&1
  LC_ALL=C grep -aoE 'session_[A-Za-z0-9]+' "$out" | head -1
}

read_verdict() { # read_verdict <id> <branch>: 0 and the branch fetched once its tip carries a verdict
  git fetch -q origin "refs/heads/$2" 2>/dev/null || return 1
  local v; v=$(git log -1 --format=%B FETCH_HEAD | sed -nE 's/^Verdict: (.*)$/\1/p' | tail -1)
  [ -n "$v" ] || return 1
  echo "$v" > ".loop/verdict/$1"
}

bring_in() { # bring_in: squash FETCH_HEAD into the index, HEAD untouched, so gate() sees it as a session's work
  git merge -q --squash --no-commit FETCH_HEAD > /dev/null 2>&1 && return 0
  if [ "$(git diff --name-only --diff-filter=U)" = Cargo.lock ]; then
    git checkout -q HEAD -- Cargo.lock
    cargo metadata --format-version 1 > /dev/null 2>&1 && git add Cargo.lock && return 0
  fi
  git reset -q --hard HEAD
  return 1
}

settle() { # settle <id> <attempt> <log>: gate the work now in the tree, then land it or set it aside
  local id=$1 attempt=$2 reason; log=$3
  event "GATE $id (session exit $status)"
  if reason=$(gate "$id"); then
    commit_pass "$id" "$log"
  else
    reason=${reason#FAIL: }
    echo "$reason" > ".loop/fail-$id"
    # Set the failed work aside (kept in `git stash list`) so the next session starts clean;
    # commits the session made on its own are unwound into the stash too.
    git reset -q --soft "$pre"
    git stash push -u -q -m "loop: $id attempt $attempt failed: $reason" 2>/dev/null
    event "FAILED $id attempt $attempt: $reason"
    [ "$attempt" -ge 2 ] && block "$id" "$reason"
  fi
}

poll_inflight() { # gate and land each finished cloud session, one at a time
  local f id branch deadline session attempt
  for f in .loop/inflight/*; do
    [ -e "$f" ] || continue
    id=${f##*/}; read -r branch deadline session attempt log < "$f"
    pre=$(git rev-parse HEAD); status=0
    if read_verdict "$id" "$branch"; then
      rm -f "$f"; acted=1
      [ "$(head -1 ".loop/verdict/$id")" = PASS ] || { settle "$id" "$attempt" "$log"; continue; }
      if ! bring_in; then
        # A conflict with work landed since this session started is not the task's failure: run it
        # again from the new main, reusing this branch, without using up an attempt or escalating its tier.
        echo "$branch" > ".loop/resume-$id"
        echo $(( $(cat ".loop/rebases/$id" 2>/dev/null || echo 0) + 1 )) > ".loop/rebases/$id"
        echo $(( attempt - 1 )) > ".loop/attempts/$id"
        event "CONFLICT $id on $branch; will run again from new main"
        continue
      fi
      settle "$id" "$attempt" "$log"
    elif [ "$(date +%s)" -ge "$deadline" ]; then
      rm -f "$f"; acted=1
      echo "FAIL: cloud session $session gave no verdict within ${CLOUD_TIMEOUT}s" > ".loop/verdict/$id"
      settle "$id" "$attempt" "$log"
    fi
  done
}

schedule() { # block what can never start, then start what can
  local id dep marks missing need attempt log rebases branch session
  for id in $(open_tasks); do
    [ -e ".loop/inflight/$id" ] && continue
    marks=""
    for dep in $(field "$id" Opens | tr ',' ' '); do
      [ "$dep" = none ] && continue
      case "$(mark_of "$dep" 2>/dev/null)" in
        '!') block "$id" "Opens $dep, which is blocked"; acted=1; continue 2 ;;
        x) ;;
        *) marks=waiting ;;
      esac
    done
    [ -z "$marks" ] || continue
    missing=""
    for need in $(list_of "$id" Needs); do [ -e "$need" ] || missing=$need; done
    if [ -n "$missing" ]; then block "$id" "Needs $missing, which does not exist (see loop/setup.sh)"; acted=1; continue; fi

    ready=1
    if runs_here "$id"; then
      # A local session works in this checkout, so it needs the tree to itself.
      [ -z "$(ls .loop/inflight)" ] || continue
    else
      [ "$(ls .loop/inflight | wc -l)" -lt "$PARALLEL" ] || continue
    fi
    [ "$iter" -lt "$MAX_ITER" ] || { limit=1; return; }

    iter=$((iter + 1)); acted=1
    attempt=$(( $(cat ".loop/attempts/$id" 2>/dev/null || echo 0) + 1 ))
    echo "$attempt" > ".loop/attempts/$id"
    log=".loop/sessions/$(date +%Y%m%d-%H%M%S)-$id-a$attempt.jsonl"
    rm -f ".loop/verdict/$id"
    event "SESSION $id attempt $attempt tier $(tier_of "$id" "$attempt") -> $log"

    if runs_here "$id"; then
      echo "$id $attempt $log" > .loop/current
      pre=$(git rev-parse HEAD)
      run_session "$id" "$attempt" "$log"
      status=$?
      [ $status = 142 ] && echo "FAIL: session timed out after ${SESSION_TIMEOUT}s" > ".loop/verdict/$id"
      settle "$id" "$attempt" "$log"
      rm -f .loop/current
      return   # marks changed; rescan from the top
    fi

    rebases=$(cat ".loop/rebases/$id" 2>/dev/null || echo 0)
    branch="loop/$id-a$attempt-r$rebases"
    session=$(dispatch "$id" "$attempt" "$log" "$branch" "$(cat ".loop/resume-$id" 2>/dev/null)")
    if [ -z "$session" ]; then
      echo $(( attempt - 1 )) > ".loop/attempts/$id"
      dispatch_failures=$((dispatch_failures + 1))
      event "DISPATCH FAILED $id ($dispatch_failures in a row; see ${log%.jsonl}.dispatch)"
      [ "$dispatch_failures" -lt 3 ] || { event "STOPPING: cloud sessions cannot be created"; exit 1; }
      return
    fi
    dispatch_failures=0
    echo "$branch $(( $(date +%s) + CLOUD_TIMEOUT )) $session $attempt $log" > ".loop/inflight/$id"
    event "DISPATCHED $id to $branch ($session)"
  done
}

# --- main loop ---------------------------------------------------------------
[ "$(git symbolic-ref --short HEAD)" = main ] || { echo "run this from main" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "the working tree must be clean" >&2; exit 1; }
git fetch -q origin main && git merge -q --ff-only origin/main || { echo "main must match origin/main" >&2; exit 1; }
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || { echo "main has unlanded commits; land them first" >&2; exit 1; }

event "START runner=$RUNNER parallel=$PARALLEL max_iter=$MAX_ITER timeout=${SESSION_TIMEOUT}s model=$MODEL"
iter=0; dispatch_failures=0; limit=0
while :; do
  acted=0; ready=0
  poll_inflight
  stopping=0; [ -e .loop/stop ] && stopping=1
  [ $stopping = 1 ] || [ $limit = 1 ] || schedule
  [ $acted = 1 ] && continue   # marks may have changed; look again before waiting
  [ -n "$(ls .loop/inflight)" ] && { sleep "$POLL"; continue; }
  [ $stopping = 1 ] && { event "STOP file found"; break; }
  [ $limit = 1 ] && { event "LIMIT max_iter=$MAX_ITER reached"; break; }
  [ -z "$(open_tasks)" ] && { event "DONE no [ ] tasks remain"; break; }
  [ $ready = 1 ] && { sleep "$POLL"; continue; }
  event "STUCK no task can start"; break
done
event "EXIT"
