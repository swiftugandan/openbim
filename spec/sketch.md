# Direct modelling

Frozen contract. openbim includes SketchUp-style direct modelling: draw edges, faces appear, push/pull them into
solids, with inference snapping and typed measurements. The geometry operations and the inference engine live in the
Rust crate `bim-sketch`, so the CLI, MCP and the Studio (through WebAssembly) share one deterministic implementation.

## Kinds

| Kind | Props |
|---|---|
| `Shape` | `vertices` List[Num] (x, y, z, … in the shape's local frame), `faces` List (one entry per face: a List of loops, each a List of vertex indices; the first loop is the outer boundary, counter-clockwise seen from the front, holes clockwise), `face_ids` List[Num] (a stable id per face), `edges` List[Num] (loose edges as index pairs: edges on no face), `transform`† List[Num] (16, column-major, identity), `level`† Ref Level, `face_products`† List (per face: Ref Product or `""`), `classification`† Text (an IFC class, default `IfcBuildingElementProxy`) |
| `ComponentDefinition` | `name` Text, `shape` Ref Shape |
| `ComponentInstance` | `definition` Ref ComponentDefinition, `transform` List[Num] (16, column-major), `level`† Ref Level |
| `Guide` | `point` List[Num] (3), `direction`† List[Num] (3; a guide line through `point`; without it a guide point) |
| `Scene` | `name` Text, `eye` List[Num] (3), `target` List[Num] (3), `fov` (vertical, radians) |

Rules:
- Every face is planar (all its vertices within 1e-9 m of its plane). A face's normal follows its outer loop by the
  right-hand rule; the front is the side the normal points to.
- A Shape whose faces form a closed, consistently oriented 2-manifold (every edge on exactly two faces, used in
  opposite directions) has a solid; any other Shape is an open surface, rendered and drawn but with no volume.
- A Shape referenced by a ComponentDefinition is not placed itself; each ComponentInstance places it under the
  instance's `transform`. Editing the definition's Shape changes every instance.
- New face ids are the shape's largest face id + 1 (1 for a shape with no faces), in the order the operation creates
  faces. Operations keep the ids of faces they only move, stretch or shrink. When a face is split, the part containing
  its first outer-loop vertex keeps the id. Adjacent coplanar faces never merge on their own; only `erase` merges them.
- A vertex lying on a face's boundary is a vertex of that face's loop: edges are split, never left as T-junctions.
- Operations keep vertex indices; new vertices (including copies) are appended. "Lowest-index" always refers to these
  indices.
- A Shape is placed in the model at `transform` · local coordinates.
- A command naming a face, edge or vertex the shape does not have, or breaking a field constraint (such as u ⟂ v), is
  invalid.
- Shapes and instances join every generic surface through their solid or surface: glTF export, rendering, clash
  (solids only), drawings, Datalog, and IFC export (T5.9). In glTF each placed Shape and each ComponentInstance is one
  mesh node with `extras.id` = its id; a definition's Shape has none. An open-surface Shape's node holds its
  triangulated faces once (single-sided) and is not closed: the one exception to the closed-mesh rule of spec/cli.md.

## Sketch commands

A `normal` or `axis` field is a direction and need not be unit length.

`bim sketch <dir> <file.json>` applies a list of commands atomically: an invalid command exits 2 and changes nothing.
Coordinates are in metres in the named shape's local frame. A drawing command on a missing shape creates it.

```json
{"schema": 1, "commands": [
  {"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]},
  {"op": "pushpull", "shape": "box", "face": 1, "distance": 2.5}
]}
```

| `op` | Fields | Effect |
|---|---|---|
| `line` | `from`, `to` | Adds an edge, splitting edges it crosses or touches. When edges close a planar loop, each smallest planar region it bounds becomes a face. A line on a face joining two points of that face's boundary splits the face in two. A closed loop drawn inside a face and touching nothing else makes the region inside it a new face; the old face keeps its id with that loop as a hole. |
| `rect` | `corner`, `u`, `v` (u ⟂ v) | The four lines corner → +u → +u+v → +v → corner. |
| `circle` | `centre`, `normal`, `radius`, `segments`† (24) | A regular polygon of lines; the first vertex lies in the direction of x̂ projected onto the plane (ŷ if x̂ ∥ `normal`). |
| `arc` | `centre`, `start`, `normal`, `angle` (radians, right-hand about `normal`), `segments`† (12) | A polyline of lines along the arc. |
| `pushpull` | `face`, `distance`, `new`† (false) | Extrudes the face along its normal (negative pushes in). Where a pushed face's boundary edge lies on a face perpendicular to it, that face is stretched or shrunk and keeps its id; elsewhere side faces are new. The moved face keeps its id. Pushing a face onto a parallel face of the same solid removes both and leaves a hole through the solid. With `new: true` the original face stays and the moved copy is a new face. |
| `offset` | `face`, `distance` | Adds a loop inside (positive) or outside (negative) the face's outer loop at that distance, mitred. Inward, the face becomes a ring and the new inner face gets a new id; outward, the face is unchanged and a new ring face around it gets a new id. |
| `followme` | `face`, `path` (List of points) | Sweeps the face along the polyline (first segment perpendicular to the face, starting on its plane), with mitred joints, replacing the face with the swept solid. |
| `move` / `rotate` / `scale` | `vertices`† `edges`† `faces`† (indices, index pairs, face ids), `vector` / `centre` `axis` `angle` / `origin` `factors`; `copy`† (false), `array`† (1) | Transforms the selection, stretching connected faces. A face made non-planar is split into triangles fanned from its lowest-index vertex; fan order runs along the outer loop from the fan vertex, and the triangle containing the face's first outer-loop vertex keeps the id (the first triangle when that vertex is the fan vertex) while the others take new ids in fan order. With `copy`, the transformed copy is added `array` times to the same Shape (the nth copy transformed n times), its faces numbered after the existing ones. |
| `erase` | `edges` (index pairs) | Removes edges and the faces they bound; an edge between two coplanar faces merges them (the merged face keeps the lower id). |
| `paint` | `faces`, `product` (Product id or null) | Sets `face_products`. |
| `make_component` | `shape`, `definition`, `instance`, `name` | Creates a ComponentDefinition `definition` for the shape and a ComponentInstance `instance` with the shape's former `transform`; the shape's `transform` becomes identity. |

## Inference

`bim_sketch::infer` (Rust; the Studio calls it through WebAssembly) picks the point a drawing tool should use. Input:
a camera (eye, target, up +z, vertical `fov`, viewport width and height in CSS px), the cursor in CSS px from the
top-left, the model's world-space edges, faces and guides, an optional `from` point (the tool's last point) and an
optional `reference_edge` (the last edge the cursor hovered). Output: `{point, kind, ref}`.

CSS px (0, 0) is the viewport's top-left corner: ndc = (2x / width − 1, 1 − 2y / height). The camera's up is +z, or +y
when the view direction is parallel to z. Candidates count when their projection is within 10 CSS px of the cursor.
The first numbered line below that has a candidate wins; kinds on the same line share one priority, and among all
candidates of that line the nearest on screen wins, then the lowest entity id (byte-wise), then the lowest index:

1. `endpoint` (edge end), `midpoint`, `intersection` (two edges crossing on screen and in 3D within 1e-6 m; edges that
   only share an endpoint are not an intersection)
2. `on_edge` (closest point on the edge to the cursor ray), `guide`
3. `axis_x`, `axis_y`, `axis_z` (from `from`: the closest point on that axis line to the cursor ray)
4. `parallel`, `perpendicular` (from `from`, to `reference_edge`; perpendicular is the line through `from` perpendicular
   to the edge in the plane containing both, horizontal when `from` is on the edge's line)
5. `on_face` (nearest face hit by the cursor ray), then `ground` (hit on the plane z = active level elevation)

With no candidate, `kind` is `none` and `point` is null. `ref` is `{"entity", "index"}` for edge kinds, `{"entity", "id"}`
for `on_face`, `{"entity"}` for `guide`, and null for axis, parallel, perpendicular, ground and none. The Studio colours axis inferences red (x), green (y) and
blue (z), parallel and perpendicular magenta, and shows the kind as a tooltip.

## Measurements

`bim_sketch::parse_measurement(text, default_unit)` parses what users type while a tool runs:

| Input | Meaning |
|---|---|
| `2500`, `2500mm`, `250cm`, `2.5m` | a length (bare numbers use the default unit, `mm` unless set) |
| `8'6"`, `8'`, `6"` | feet and inches |
| `4000,3000` | two lengths (rectangle sides) |
| `x5`, `5x`, `/5` | after a copy: 5 copies at the same spacing, or 5 equal spaces between the original and the copy |
| `45` with an angle tool | degrees |

Anything else is an error, shown to the user and changing nothing.
