# Judges

External Verifications, written before any building. Builders never edit this folder.

Conventions:
- `judges/Tx.y.sh` is the task's single Verification command. It exits 0 only when the task is done.
- Rust API judges are standalone cargo packages `judges/Tx.y/` with an empty `[workspace]` table and
  path dependencies on the crates under test; they build with `CARGO_TARGET_DIR=.build/judges`.
- Black-box judges drive `bim`, `bim-mcp`, the wasm package or the Studio app through the contracts in `spec/`.
- Expected values come from independent open oracles in `.oracles/` (see `loop/setup.sh`) or from
  closed-form results, never from the code under test. Random cases use fixed seeds.
- Python oracles run as `.oracles/py/bin/python -I judges/Tx.y/<script>.py`; Node scripts resolve
  packages from `.oracles/node/` (see `judges/lib/oracle.mjs`).
- Each judge prints `JUDGE FAIL: <reason>` before a failing exit and finishes in under a minute on a warm cache.
