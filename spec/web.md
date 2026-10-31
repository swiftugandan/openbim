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
  sketch(json: string): void;                   // sketch commands as `bim sketch` (spec/sketch.md); throws on invalid, atomic (added by T8.6)
  infer(json: string): string;                  // T2.12 query JSON in, result JSON out, against the model (added by T8.6)
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
Added by T8.5 (catalogues): `catalog-import` (file input), `catalog-products`, `product-<catalog>:<sku>`, `tool-furniture`,
`assign-product`. `catalog-products` holds one `product-<catalog>:<sku>` item for every product of every imported
catalogue at once, grouped by category (within a catalogue, a category's items are consecutive). Clicking an item selects
that product, which stays selected while elements are selected in the views; `assign-product` is a button that applies
it to the selected element. `tool-furniture` creates a Furniture on the active level at the clicked plan point with
`angle` 0. Props an element inherits from its product appear in `props-panel` as `prop-<key>` with the product's value;
the element's own props show its own values. Changes arriving through sync do not reframe the plan. Added by T8.6 (direct modelling): `tool-line`, `tool-rect`, `tool-circle`, `tool-arc`, `tool-pushpull`,
`tool-move`, `tool-rotate`, `tool-scale`, `tool-offset`, `tool-followme`, `tool-tape`, `tool-eraser`, `tool-paint`,
`tool-orbit`, `tool-pan`, `zoom-extents`, `make-component`, `scenes`, `measurements` (the typed-value box),
`inference-tip`.

Direct modelling in the Studio (T8.6, T8.7):
- `infer(json)` takes the T2.12 query JSON (`camera`, `cursor`, optional `from` and `reference_edge`, optional
  `level_elevation`, default 0) and answers it against the model's world-space edges, faces and guides, returning the
  T2.12 result JSON (`ref.entity` is the entity's id string; for a face, `ref.id` is the face id).
- `viewport-3d` carries `data-camera`, the JSON `{"eye":[x,y,z],"target":[x,y,z],"fov":<vertical radians>}` of its current
  camera (up +z, perspective over the element's CSS box), updated on every camera change; wheel zoom and pan keep the
  view direction. The active tool's button has `aria-pressed="true"` and every other `tool-*` button `aria-pressed="false"`.
- Each direct-modelling tool commits one `bim sketch` command built from its clicks (each click uses the inference point
  under the cursor) and the typed value, which replaces the length, angle or factor of the pointer's offset from the
  tool's last point and keeps its direction or side: push/pull extrudes towards the pointer, offset goes to the
  pointer's side (towards the face centre = inward, positive), rotate turns towards the pointer.
- Clicking a Shape or ComponentInstance with `tool-select` selects it; move, rotate, scale and make-component act on the
  selected Shape as all its faces. Face and edge tools on a ComponentInstance edit its definition's Shape.
- `tool-move`: click the start, then click or type the distance; pressing and releasing Ctrl/Cmd while it runs toggles
  copy, and `x5`/`5x`/`/5` typed after a copy make the result the single `move` with `copy` and `array`.
  `tool-rotate`: click the centre, then the start direction, then click or type degrees, about +z. `tool-scale`: click
  the origin, then type a factor (`2`) or `x,y,z` factors. `tool-pushpull`: click the face, then click or type the
  distance; pressing and releasing Ctrl/Cmd while it runs sets `new: true`; double-clicking a face repeats the last
  distance. `tool-offset`: click the face, then click or type the distance. `tool-followme`: click the face, then each
  path point; Enter sweeps. `tool-tape`: click the start, then click or type the length; it creates a Guide whose `point`
  is the end point (measured from an edge, with that edge's direction). `tool-eraser` erases the edge under the cursor.
  `tool-paint` applies the product selected in `catalog-products` to the face under the cursor, creating its Product
  entity as `bim catalog use` would. `make-component` makes the selected Shape a component (`make_component`; the Studio
  chooses the ids and name).
- A push/pull, offset, move, rotate or scale whose distance, angle or factor is zero commits nothing; double-clicking a
  face with `tool-pushpull` commits only the repeated distance. `zoom-extents` (Shift+Z) frames the model: every vertex of
  every placed Shape and ComponentInstance projects inside `viewport-3d`, and the model's projected extent spans at least
  30 % of the viewport's width or height.
- `scenes` contains `scene-save`, which saves the current camera as a new Scene entity, and one `scene-<id>` button per
  Scene, which restores that camera.

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
