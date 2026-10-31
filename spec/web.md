# WebAssembly, viewport and Studio contracts

Frozen contract.

## `crates/bim-wasm` → `packages/bim-wasm/` (wasm-pack, target `web` and `nodejs`)

```ts
class Model {
  static fromOps(json: string): Model;          // JSON ops (spec/cli.md)
  apply(json: string): void;                    // throws on invalid ops; atomic
  hash(): string;                               // 16 hex, equals `bim hash`
  generation(): number;                         // bumps on every successful apply
  positions(): Float32Array;                    // xyz per vertex — a VIEW on wasm memory, no copy
  normals(): Float32Array;                      // view
  indices(): Uint32Array;                       // view, triangles
  triangleEntity(): Uint32Array;                // view, entity index per triangle
  entityName(index: number): string;            // index → id string
  instances(): Float32Array;                    // view, 16 floats (column-major mat4) per instance
  instanceMesh(): Uint32Array;                  // view, family mesh index per instance
  solve(trade: string): string;                 // JSON as `bim solve --json`
  exportView(view: string, format: string): string;
  validate(standard: string): string;           // JSON as `bim validate --standard <s> --json` (added by T8.4)
  issues(): string;                             // JSON [{id, title, status, entities: [id strings]}] of Issue entities (added by T8.4)
}
export const memory: WebAssembly.Memory;
```

`positions/normals/indices/triangleEntity` are the flattened world-space triangles of every entity (per entity equal to
the glTF export, in model coordinates); `instances/instanceMesh` are an additional instanced view. `exportView(view, "svg")`
returns the same text as `bim export --format svg`. Both `pkg-node` and `pkg-web` export `Model` and `memory` from their
`package.json` entry; `pkg-web`'s default export is the wasm-bindgen `init`.

Zero-copy means `model.positions().buffer === memory.buffer`. Views stay valid until the next
`apply` (callers re-fetch after `generation()` changes).

## `packages/web-viewport` (TypeScript, ESM)

```ts
createViewport(canvas: HTMLCanvasElement, model: Model, opts?: { backend?: "webgpu" | "webgl2" | "auto" }): Promise<Viewport>;
interface Viewport {
  backend: "webgpu" | "webgl2";
  setCamera(eye: [n,n,n], target: [n,n,n], fovY: number): void;
  setSectionBox(min: [n,n,n], max: [n,n,n] | null): void;   // 6-plane clip, capped cut faces
  pick(x: number, y: number): Promise<string | null>;         // GPU ID buffer → entity id
  refresh(): void;                                            // after model.apply
  frameStats(): { p50: number, p95: number, frames: number };
}
mountThree(model: Model, THREE: typeof import("three")): THREE.Group;  // BufferAttributes share wasm memory
createSheetView(canvas: HTMLCanvasElement, svgOrDrawing: string, opts?: { dpr?: number }): SheetView;
```

`npm run build -w packages/web-viewport` emits browser ESM at `packages/web-viewport/dist/index.js` whose only bare
imports are `bim-wasm` and `three`; `three` is a devDependency so the root `npm install` provides `node_modules/three`.
`setCamera` fovY is the vertical field of view in radians, up = +z; `pick(x, y)` takes CSS pixels from the canvas's
top-left and returns `null` on background; `setSectionBox(min, null)` clears the box; picking ignores geometry the box
hides. `createSheetView` opens with the whole sheet visible at a uniform scale, draws strokes in the SVG's colours;
primary-button drag pans, the wheel zooms, and a snap glyph is drawn centred on a polyline endpoint while the pointer is
within at least 5 CSS px of it, removed when the pointer leaves.

## `apps/studio` (Vite app; `npm run build` → `apps/studio/dist`, served statically)

Stable test hooks (`data-testid`), required for end-to-end judges:
`viewport-3d`, `sheet-2d`, `tool-select`, `tool-wall`, `tool-slab`, `tool-door`, `tool-section-box`,
`props-panel`, `prop-<key>` (inputs), `history`, `undo`, `redo`, `presence`, `issues`, `validate`,
`violations`, `level-picker`, `status-bar` (shows backend and fps).

Behaviour: clicking two points on the plan with `tool-wall` active creates a Wall; selecting an
entity (3D pick or 2D click) fills `props-panel`; editing `prop-height` updates the model and both
views within one frame of the wasm apply; documents persist in IndexedDB and reload; with
`?sync=ws://host:port` two tabs converge through `bim sync serve`; keyboard: `W` wall, `S` select,
`Ctrl/Cmd+Z` undo, `Ctrl/Cmd+Shift+Z` redo, `Esc` cancels a tool. No horizontal page overflow at 390 px
or 1440 px widths; zero serious/critical axe-core violations.

Prop inputs show SI values (metres) and commit on Enter/change; the plan's default cut height is above 0.3 m. A fresh
load has no selection; drawing does not reframe the plan; a new element is visible in `viewport-3d` without camera
interaction. The Studio follows `prefers-color-scheme`; `status-bar` text includes the backend name and `fps`.
`?bench=1` runs the in-browser frame-time harness (camera orbit over the loaded document) and `status-bar` then shows
`p95 <ms> ms`. Validate runs the `ibc` pack by default and lists each violation's `rule` and `entity`; Issues lists the
BCF topics linked to the current selection; activating `tool-section-box` shows a section box in `viewport-3d`.
