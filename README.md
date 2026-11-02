# openbim

An open, deterministic BIM and spatial-engineering platform in Rust, for the native CLI and WebAssembly.

**Status: under construction.** Project page: https://swiftugandan.github.io/openbim/ The plan, the frozen contracts and the judges exist; the product code is
being built task by task. See `TASKS.md` for progress (`[x]` done, `[ ]` to do, `[!]` blocked).

## What it will be

- **Kernel:** a content-addressed semantic document graph with an op log, a CRDT with conflict diagnostics,
  SQLite revisions, WebSocket sync and salsa-style incremental recomputation.
- **Geometry:** 2D/3D constraint solving, exact predicates, half-edge B-Rep, Boolean CSG, R*-tree/BVH,
  plane sectioning and exact hidden-line removal for plans, sections and elevations.
- **Rendering:** a native wgpu renderer, a WebGPU/WebGL2 web viewport with zero-copy Three.js interop, and a
  Canvas2D sheet viewer.
- **Engines and standards:** architectural, structural and MEP engines behind a plugin boundary
  (WASM components), IFC4 STEP/JSON, BCF 3.0 and declarative code-rule packs.
- **Direct modelling:** SketchUp-style drawing, push/pull, follow-me, inference snapping and typed measurements.
- **Plugins and catalogues:** new trades as sandboxed WebAssembly trade packages with their own element types, and
  manufacturers' product catalogues (doors, windows, furniture, finishes, roof coverings) with schedules and IFC types.
- **Interfaces:** the `bim` CLI, the `bim-mcp` Model Context Protocol server, and the Studio web app.

## How it is built

`loop/run.sh` works through `TASKS.md`, starting a fresh Claude Code session for each task whose dependencies are done:
SCOUT → (PLAN) → BUILD → CHECK → AUDIT, then a gate on this machine, then a PR that lands on `main`. Every task's
Verification is an external judge in `judges/`, written before any building, and most judges compare against
independent open oracles (ifcopenshell, manifold3d, PyNite, WNTR, python-solvespace, the Khronos glTF validator,
the MCP SDK, axe-core and others). The gate reruns each judge and every earlier one; a session never grades itself.

Sessions run in parallel as cloud sessions, except the tasks in `LOCAL_TASKS` (GPU and frame-time gates, the Chrome-driven
UI judges, the IFC corpus), which run here one at a time.

```
bash loop/setup.sh       # install the oracles into .oracles/
bash loop/test-run.sh    # check the loop itself, using stubs
bash loop/run.sh         # run the loop from a clean, pushed main (PARALLEL=4, RUNNER=local to stay on this machine)
bash loop/watch.sh       # dashboard for local sessions, in a second terminal; .loop/events.log covers cloud ones
```

Cloud sessions need the Claude GitHub App on this repo and a cloud environment used only for this project (`CLOUD_ENV`,
named `openbim-build`) with full network access, `BASH_DEFAULT_TIMEOUT_MS=900000`, `BASH_MAX_TIMEOUT_MS=900000` and
`loop/cloud-env-setup.sh` as its setup script.
`touch .loop/stop` stops new sessions and lets running ones finish.

Contracts live in `spec/` (schema, CLI, MCP, plugins, web).

## License

MIT, see `LICENSE`.
