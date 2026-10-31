You are the session for task {{ID}}, attempt {{ATTEMPT}}, in an unattended build loop.
Work in the current directory. Nobody will answer questions; never ask any.

## The task (from TASKS.md)

{{BLOCK}}

The Spec bullet above is the contract. Its Verification is: `{{VERIFY}}`
{{PREVIOUS}}
## Hard rules for you and every subagent

- Change only paths listed under Files:. Never edit `judges/`, `loop/`, `TASKS.md` or `.gitignore`.
- Do not run `git commit`, `git add`, `git stash`, `git reset` or `git checkout`; the loop commits for you.
- Scratch work and probes go under `.loop/probes/{{ID}}/` (git-ignored). Cargo builds for probes use `CARGO_TARGET_DIR=.build/probes`.
- No network services, no credentials, no external crates unless the Spec names them.

## Stages

Run these in order. Spawn each subagent with the Agent tool, set `model` (and `effort` where given) exactly as stated,
and start its `description` with the stage label shown (the dashboard reads these labels).
Give each subagent everything it needs in the prompt: it starts with fresh context.

1. **SCOUT**: Agent, model `haiku`, description `SCOUT: {{ID}}`.
   Read-only. Report what already exists that this task touches or depends on: the workspace
   layout, the crates named under Opens:, their public APIs, and the judge files for this task.
   Keep the report short.

2. **BUILD**: Agent, model `opus`, effort `medium`, description `BUILD: {{ID}}`.
   Give it the task block, the hard rules and the Scout report. It implements the Spec with
   structurally sound code (no special-casing the judge's inputs), runs `{{VERIFY}}` until it
   exits 0, and reports what it built and which claims it makes.

3. **CHECK**: you, not a subagent. Run `{{VERIFY}}` exactly once. Run `git status --porcelain`
   and confirm every changed path is under Files:. If either fails, treat it as an Audit FAIL.

4. **AUDIT**: Agent, model `fable`, description `AUDIT: {{ID}}`.
   Fresh eyes: give it the Spec and the Builder's claims, not your opinion. It must write at least
   one probe of its own under `.loop/probes/{{ID}}/` that tries to break the claims (edge cases the
   judge does not cover, spec clauses the judge does not test), run it, and end its reply with a
   final line that is exactly `PASS` or `FAIL: <one-line reason>`.

If CHECK or AUDIT fails: one fix round only. Spawn `BUILD: {{ID}} fix` (model `fable`) with the
failure details (a failed attempt is the tough case, so it gets `fable`), then repeat CHECK, then `AUDIT: {{ID}} recheck` (model `fable`).

## Verdict

Finish by writing the file `.loop/verdict/{{ID}}` with exactly one line:
`PASS` if the final CHECK and AUDIT both passed, otherwise `FAIL: <one-line reason>`.
Then stop. Do not tick TASKS.md and do not commit; the loop does both after re-checking.
