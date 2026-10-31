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
- **Interfaces:** the `bim` CLI, the `bim-mcp` Model Context Protocol server, and the Studio web app.

## How it is built

`loop/run.sh` works through `TASKS.md` one task per fresh Claude Code session:
SCOUT → (PLAN) → BUILD → CHECK → AUDIT → COMMIT. Every task's Verification is an external judge in `judges/`,
written before any building, and most judges compare against independent open oracles (ifcopenshell,
manifold3d, PyNite, WNTR, python-solvespace, the Khronos glTF validator, the MCP SDK, axe-core and others).
The loop's gate reruns each judge and every earlier one; a session never grades itself.

```
bash loop/setup.sh   # install the oracles into .oracles/
bash loop/run.sh     # run the loop
bash loop/watch.sh   # dashboard, in a second terminal
```

Contracts live in `spec/` (schema, CLI, MCP, plugins, web).

## License

MIT, see `LICENSE`.
