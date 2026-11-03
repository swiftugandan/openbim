# TASKS

The full build plan for openbim: a deterministic, document-first, local-first BIM and
spatial-engineering platform in Rust (native CLI + WebAssembly) with a web Studio.
`loop/run.sh` works every `[ ]` task whose `Opens:` are all `[x]`. Marks: `[ ]` todo, `[x]` done, `[!]` blocked (with a `Blocked:` bullet).

Fields per task:
- `Files:` the only paths the task may change. `Opens:` tasks whose output must exist (a `[!]` there blocks this task).
- `Needs:` paths that must exist before building (oracles from `loop/setup.sh`, the IFC corpus); missing → blocked.
- `Builder:` model tier. `sonnet` (tightly specified, oracle-checked work): an opus PLAN, sonnet BUILD at max effort, opus AUDIT. `opus` (default): opus BUILD at medium effort, opus AUDIT. `fable` (high stakes: exact-geometry foundations and the incremental engine that most later tasks inherit): fable for every stage. A retry after a failed attempt escalates one tier.
- `Verification:` one command, an external judge in `judges/` written before any building. Builders never edit `judges/` or `spec/`.

Contracts live in `spec/` (schema, CLI, MCP, plugins, web) and are frozen. Judges use independent
open oracles wherever one exists (installed by `loop/setup.sh` into `.oracles/`): ifcopenshell, ezdxf,
pypdf, manifold3d, trimesh, shapely, PyNite, WNTR/EPANET, python-solvespace, xmlschema + buildingSMART
BCF XSDs, Khronos glTF-Validator, the official MCP SDK, Playwright (system Chrome) and axe-core;
otherwise closed-form textbook results. Verifications target under a minute on a warm `.build/` cache
(Build warms it); cold first compiles of large dependencies take longer, and a few judges are slower by design:
T7.5 (100k-element bench, ≤ 300 s), T5.4 (≤ 60 s per corpus file), T8.1 and T8.3 to T8.7 (wasm + app rebuild and a
browser session, ≤ 150 s), T1.7 (10,000-case proptest with a solver call per case, ≤ 600 s). The loop's gate allows 900 s per judge.
External crates and npm packages are allowed when a task needs them (pin versions; no network services at run time).
The loop's gate also reruns every earlier `[x]` task's Verification, so regressions fail the task that caused them.

## Coverage

| Spec section | Tasks |
|---|---|
| 1. Document graph, ids, provenance, dependencies, op log | T1.1, T1.6 |
| 1. CRDT syntactic merge, semantic evaluation, ConflictDiagnostic nodes | T1.2, T1.7 |
| 1. SQLite store, revisions; WebSocket sync; IndexedDB | T3.2, T3.3, T8.3 |
| 1. Incremental (salsa-style) recomputation | T3.1 |
| 2. 2D/3D constraint solver, symbolic Jacobians, DOF decomposition | T1.3, T2.4 |
| 2. Exact predicates, extrusions, sweeps, B-Rep | T2.1 |
| 2. Boolean CSG (wall joins, penetrations, openings, mitres) | T2.2, T2.7 |
| 2. R*-tree, BVH, raycasting | T2.3 |
| 2. Plane sectioning, exact HLR, layered drawings, associative dims | T2.5, T2.6, T2.8, T2.9 |
| 3. WASM zero-copy views, Three.js adapter | T7.1, T8.1 |
| 3. GPU instancing, culling, ID picking, section box caps, SSAO (native wgpu) | T7.2, T7.3 |
| 3. WebGPU + WebGL2 viewport, Canvas2D sheet viewer | T8.1, T8.2 |
| 3. Sheets, ISO linetypes, hatches, PDF/SVG/DXF composition | T2.9, T7.4 |
| 3. 60 FPS / <50 ms budget on 100k elements, 2M triangles | T7.5, T8.4 |
| 4. TradeEngine / StandardRuleSet traits, WASM component plugins | T4.1 |
| 4. Architectural engine (walls, curtain walls, stairs, ramps, rooms, takeoff) | T2.7, T4.2, T4.3 |
| 4. Structural engine (extraction, combinations, tributary, 3D frame, envelopes) | T1.4, T4.4, T4.5 |
| 4. MEP engine (port networks, Darcy-Weisbach, sizing, circuits, clash) | T4.6, T4.7 |
| 4. Standards: IFC4 STEP/JSON, BCF, code rule packs | T5.1–T5.7 |
| 5. Headless CLI (`init apply validate solve clash export diff bench`) | T1.5, T1.6, T3.2, and each engine task |
| 5. MCP server, Datalog queries, propose/commit transactions, visual feedback | T6.1, T6.2, T6.3 |
| 5. UI/UX: Studio app (tools, panels, undo, persistence, collaboration, a11y) | T8.3, T8.4 |
| 4. Pluggable trades: extension kinds, trade packages with WebAssembly engines, generic metrics | T1.8, T4.8, T5.8, T6.3 |
| 4. Manufacturer and brand catalogues: products, furniture, roofs, finishes, schedules, IFC types, Studio | T4.9, T5.8, T8.5 |
| 5. Direct modelling (SketchUp-style): drawing, push/pull, follow-me, inference, measurements, components | T2.10, T2.11, T2.12, T5.9, T8.6, T8.7 |
| 6.1 IFC reference-corpus judge (ifcopenshell oracle) | T5.4 |
| 6.2 Property-based CRDT judge (10,000 cases) | T1.2, T1.7 |
| 6.3 Analytical engineering judge (frames, Timoshenko, pipe networks) | T1.4, T4.4, T4.6 |
| 6.4 Performance judge (criterion, frame-time harnesses) | T7.5, T8.4 |

## M1 Kernel

- [x] T1.1: bim-doc: content-addressed semantic document graph with op log
  - Files: Cargo.toml, Cargo.lock, crates/bim-doc/
  - Opens: none
  - Verification: bash judges/T1.1.sh
  - Spec: Cargo workspace with `resolver = "2"` and members = ["crates/bim-*", "crates/engines/*"]. Crate `bim-doc` exports `EntityId(u128)` with `EntityId::derive(seed: &str)` (deterministic, distinct seeds give distinct ids), `Value` enum (`Num(f64)`, `Text(String)`, `Ref(EntityId)`), `Entity { id, kind: String, props: BTreeMap<String, Value> }`, `Op` enum (`Create { id: EntityId, kind: String }`, `Set { id: EntityId, key: String, value: Value }`, `Delete { id: EntityId }`), `Document::new()`, `Document::apply(&mut self, op: Op) -> Result<(), DocError>` (Set or Delete on a missing id is an error; Create of an existing id is an error), `get(id) -> Option<&Entity>`, `len()`, `content_hash() -> u64` (deterministic, independent of insertion order, changes on any prop change), `dangling_refs() -> Vec<(EntityId, String)>` (props whose `Ref` target does not exist), `log() -> &[Op]` (applied ops only, in order). All public types derive Debug, Clone, PartialEq; `EntityId` also Copy, Eq, Ord, Hash; `DocError` derives Debug.

- [x] T1.2: bim-crdt: state-based CRDT replica with conflict diagnostics
  - Files: Cargo.lock, crates/bim-crdt/
  - Opens: T1.1
  - Verification: bash judges/T1.2.sh
  - Spec: Crate `bim-crdt` (depends on bim-doc). `Replica::new(peer: u32)`; local edits `create(&mut self, id: EntityId, kind: String)`, `set(&mut self, id: EntityId, key: String, value: Value)`, `delete(&mut self, id: EntityId)`, each stamped with a Lamport clock `(counter, peer)`; `merge(&mut self, other: &Replica)` is commutative, associative and idempotent (last-writer-wins per (entity, key) by stamp; delete is a tombstone that wins over a create or set with a lower stamp, and an entity re-created with a higher stamp revives). `document() -> bim_doc::Document` materializes the live state. `diagnostics() -> Vec<ConflictDiagnostic>` reports, without panicking, every live `Ref` whose target is tombstoned or missing: `ConflictDiagnostic { entity: EntityId, key: String, target: EntityId }` (derives Debug, Clone, PartialEq, Eq, PartialOrd, Ord), sorted. Local edits never fail. Liveness: an entity is live iff its greatest create stamp exceeds its greatest delete stamp; a prop is visible iff the entity is live and the prop's stamp exceeds the entity's greatest delete stamp. `Replica` derives Clone.

- [x] T1.3: bim-solver: 2D geometric constraint solver with DOF analysis
  - Files: Cargo.lock, crates/bim-solver/
  - Opens: T1.1
  - Verification: bash judges/T1.3.sh
  - Spec: Crate `bim-solver`. `Sketch::new()`, `add_point(x, y) -> PointId`, `add(c: Constraint)`; `Constraint` variants: `Fix(PointId, f64, f64)`, `Coincident(PointId, PointId)`, `Distance(PointId, PointId, f64)`, `Horizontal(PointId, PointId)`, `Vertical(PointId, PointId)`, `Parallel(PointId, PointId, PointId, PointId)`, `Perpendicular(PointId, PointId, PointId, PointId)`, `Angle(PointId, PointId, PointId, PointId, f64)` (radians, line a→b to line c→d). `solve(&mut self) -> SolveReport` uses Newton or Levenberg–Marquardt with analytic Jacobians. `SolveReport { status: Status, residual: f64, dof: usize }` with `Status::{WellConstrained, UnderConstrained, OverConstrained, Inconsistent}`: dof = 2·points − rank(J); redundant-but-consistent equations give OverConstrained; unsatisfiable ones give Inconsistent. `point(id) -> (f64, f64)`. `PointId` and `Constraint` derive Debug, Clone, Copy; `Status` derives Debug, PartialEq. Angle is the signed counter-clockwise angle from direction a→b to direction c→d. `residual` is the Euclidean norm of the equation residual vector; rank uses a relative singular-value tolerance of 1e-9. Status priority: Inconsistent (residual > 1e-6 after solving), then UnderConstrained (dof > 0), then OverConstrained (more scalar equations than rank), else WellConstrained.

- [x] T1.4: bim-struct: 2D frame direct-stiffness solver
  - Files: Cargo.lock, crates/engines/structural/
  - Opens: T1.1
  - Builder: sonnet
  - Verification: bash judges/T1.4.sh
  - Spec: Crate `bim-struct` at crates/engines/structural (the T1.1 workspace glob already includes it). `Frame::new()`, `add_node(x, y) -> usize`, `add_member(i, j, e, a, iz) -> usize` (Euler–Bernoulli beam-column), `fix(node, ux: bool, uy: bool, rz: bool)`, `load(node, fx, fy, mz)`, `analyze() -> Result<Results, String>` (Err on a singular/unstable structure). `Results::displacement(node) -> (ux, uy, rz)`, `reaction(node) -> (fx, fy, mz)`, `member_end_forces(m) -> [f64; 6]` in local axes (N1, V1, M1, N2, V2, M2). Conventions: global x right, y up, rotation counter-clockwise positive; a reaction is the force the support exerts on the structure; member end forces are the forces the nodes exert on the member, in local axes (x from node i to node j).

- [x] T1.5: bim-cli: headless `bim` command over the document
  - Files: Cargo.lock, crates/bim-cli/
  - Opens: T1.1, T1.4
  - Builder: sonnet
  - Verification: bash judges/T1.5.sh
  - Spec: Crate `bim-cli` producing binary `bim`. `bim init <dir>` creates `<dir>/doc.oplog` (empty). `bim apply <dir> <ops-file>` appends ops, one per line: `create <seed> <kind>`, `set <seed> <key> num <f64>|text <word>|ref <seed>`, `delete <seed>` (ids are `EntityId::derive(seed)`); an invalid op exits 2 and leaves the log unchanged. `bim hash <dir>` prints `content_hash` as 16 lowercase hex digits. `bim validate <dir>` exits 0 when there are no dangling refs, else prints one line per dangling ref and exits 1. `bim diff <dirA> <dirB>` prints `same` and exits 0 when hashes match, else `differ` and exits 1. Blank lines in an ops file are ignored; any other error (missing directory, unreadable file) prints a message to stderr and exits 2.

- [x] T1.6: bim-doc schema v1: ULIDs, Bool/List values, provenance, dependency graph, JSON ops
  - Files: Cargo.lock, crates/bim-doc/, crates/bim-cli/
  - Opens: T1.5
  - Verification: bash judges/T1.6.sh
  - Spec: Implement the identity, value, reserved-prop and dependency rules of spec/schema.md. `EntityId::parse(s)` takes a ULID (Crockford base32, 26 chars) or derives from a seed; `EntityId::new_ulid(ms, rand: u128)` is deterministic given its inputs. `Value` gains `Bool(bool)` and `List(Vec<Value>)` (existing variants unchanged). `Document::deps(id) -> Vec<EntityId>` (sorted Ref targets, including Refs inside Lists), `dependents(id)`, `topo_order() -> Result<Vec<EntityId>, Cycle>`; `Document::SCHEMA_VERSION == 1`. CLI: `bim apply` accepts JSON ops files and `bim show` per spec/cli.md, setting reserved props `_name` and `_schema` at create. All T1.1 and T1.5 behaviour is preserved.
  - Details: `EntityId::parse(s: &str) -> EntityId` is infallible: a 26-char uppercase Crockford-base32 string ≤ `7ZZZZZZZZZZZZZZZZZZZZZZZZZ` is a ULID, any other string is a seed (`derive(s)`). `new_ulid(ms: u64, rand: u128)` = `(ms << 80) | (rand & (2^80 − 1))` and `parse(<its encoding>) == new_ulid(ms, rand)`. `deps`/`dependents` are sorted, deduplicated, include Refs nested in Lists at any depth, and `deps` includes missing targets. `topo_order()` lists every live entity once, dependencies first, ignoring edges to missing targets; a self-reference is a cycle; the error type is `bim_doc::Cycle`. JSON ops: `props` on create is optional; `schema` ≠ 1, a `null` value, an unknown op, or a create/set of a `_` key is invalid (exit 2, nothing applied). Every create (line or JSON) sets `_name` (Text, the id as written) and `_schema` (Num 1). `bim show <dir> <id> --json` prints `{"id","kind","props"}` with `id` = `_name`, includes reserved props, renders Refs as `{"ref": <target _name>}`, prints Nums with round-trip precision, and exits non-zero for an unknown or deleted id.

- [x] T1.7: CRDT semantic layer: materialized ConflictDiagnostic nodes, constraint conflicts, proptest
  - Files: Cargo.lock, crates/bim-crdt/
  - Opens: T1.2, T1.6, T1.3
  - Verification: bash judges/T1.7.sh
  - Spec: Separate syntactic merge (T1.2, unchanged guarantees) from semantic evaluation: `Replica::evaluate() -> Evaluated { document, diagnostics }` where `document` contains one `ConflictDiagnostic` entity per diagnostic (id derived from `"conflict:" + entity + key + target`, props per spec/schema.md, `reason` one of `dangling_ref`, `constraint_unsatisfiable`, `dependency_cycle`). Constraint entities (spec/schema.md `Constraint`, with `a`/`b` referencing `Point` entities) are evaluated with bim-solver after merge; when concurrent edits make a connected constraint group Inconsistent, every constraint in that group gets a `constraint_unsatisfiable` diagnostic and no geometry is changed. A Ref cycle yields `dependency_cycle` diagnostics. Never panics. Judge uses proptest (10,000 cases) over creates, sets, deletes, constraint mutations and offline splits/merges among 3–5 peers.
  - Details: `Replica::evaluate(&self) -> Evaluated`, `pub struct Evaluated { pub document: bim_doc::Document, pub diagnostics: Vec<Diagnostic> }` with one entry per ConflictDiagnostic node; T1.2's `ConflictDiagnostic` and `diagnostics()` are unchanged; `evaluate` does not mutate the replica and `document()` never contains diagnostic nodes. Node props: `entity` (Ref), `key` (Text), `target` (Ref, when there is one), `reason` (Text). `dangling_ref`: the dangling top-level Ref (same set as `diagnostics()`). `constraint_unsatisfiable`: `entity` = the Constraint. `dependency_cycle`: one node per entity lying on a Ref cycle among live entities (self-references included). Only Constraints of kind `distance` (Num `value`) or `coincident` whose `a`/`b` reference live Points with Num `x`/`y` are solved; groups are connected components over shared Points; malformed or dangling constraints are skipped, not errors; satisfiable groups' solved `x`/`y` are written into `Evaluated.document`; Points in unsatisfiable groups and unconstrained Points keep their merged coordinates.

- [x] T1.8: kind registry: core kinds, extension kinds from trade packages, op validation
  - Files: Cargo.toml, Cargo.lock, crates/bim-schema/, crates/bim-cli/
  - Opens: T1.6
  - Builder: sonnet
  - Verification: bash judges/T1.8.sh
  - Spec: Crate `bim-schema` knows the core kinds of spec/schema.md (including `Roof`, `Furniture`, `Product` and the direct-modelling kinds of spec/sketch.md) and loads the extension kinds declared by `<dir>/plugins/*/trade.json` (spec/schema.md, Extension kinds; package rules in spec/plugins.md, Trade packages). `bim apply` (JSON ops and the line format) exits 2 and changes nothing on a create of an undeclared kind, or a create or set that breaks an extension kind's prop declarations; a malformed or inconsistent package makes every command on the project exit 2 naming the file. `bim plugin list <dir> --json` per spec/cli.md, without `trades` (added by T4.8).
  - Details: `bim-schema` exports at its root `KindRegistry` with `core() -> KindRegistry`, `load(dir: &std::path::Path) -> Result<KindRegistry, SchemaError>` (core kinds plus packages), `kind(&self, name: &str) -> Option<&KindDef>`, `kinds(&self) -> Vec<String>` (sorted), `check_create(&self, kind: &str, props: &[(String, bim_doc::Value)]) -> Result<(), SchemaError>` and `check_set(&self, kind: &str, key: &str, value: &bim_doc::Value) -> Result<(), SchemaError>`; `pub struct KindDef { pub name: String, pub package: Option<String>, pub props: std::collections::BTreeMap<String, PropType>, pub geometry: Option<Geometry> }` (core kinds: `package` None, `props` empty, `geometry` None); `pub enum PropType { Num, Text, Bool, Ref, List }`; `pub enum Geometry { Box { centre: [Term; 3], size: [Term; 3] }, Segment { from: [Term; 3], to: [Term; 3], diameter: Term } }`; `pub enum Term { Prop(String), Num(f64) }`; `SchemaError` implements `Display` and `std::error::Error`. `bim_doc::Value` is the kernel's value type (spec/schema.md Values). Core kinds' props are not type-checked. Reserved `_` keys keep their existing rule. `KindRegistry::core().kinds()` and `plugin list`'s `core_kinds` list every row of the spec/schema.md kinds table, `ConflictDiagnostic` included.

## M2 Geometry kernel

- [x] T2.1: bim-geom: exact predicates, polygons, half-edge B-Rep, extrusions and sweeps
  - Files: Cargo.toml, Cargo.lock, crates/bim-geom/
  - Opens: T1.1
  - Needs: .oracles/py/bin/python
  - Builder: fable
  - Verification: bash judges/T2.1.sh
  - Spec: Crate `bim-geom`: `Point2/Point3/Vec3` (f64), robust `orient2d`, `orient3d`, `incircle` (adaptive exact arithmetic: correct sign for all f64 inputs), `Polygon { outer, holes }` with `area`, `contains`, `triangulate` (ear clipping with holes, returns index triples). `Solid` = closed half-edge mesh with planar polygonal faces: `extrude(&Polygon, z0, height)`, `sweep(&Polygon, path: &[Point3])` (profile in the plane normal to the path start, mitred at each path vertex), `volume`, `area`, `centroid`, `is_closed_manifold`, `euler_characteristic` (V − E + F, = 2 for genus 0), `to_stl(&Path)` ASCII. Judge compares against trimesh on exported STL and closed forms.
  - Details: `Point2 { x, y }`, `Point3 { x, y, z }`, `Vec3 { x, y, z }` are Copy with pub f64 fields and `new`. `orient2d(a, b, c)`, `orient3d(a, b, c, d)`, `incircle(a, b, c, d)` are crate-root functions taking points by value, returning f64 with exact sign for |coords| in {0} ∪ [2^-100, 2^100] (Shewchuk conventions: orient2d > 0 for CCW; orient3d = det[a−d; b−d; c−d] > 0 when d is below CCW abc; incircle > 0 when d is inside CCW abc). `Polygon { pub outer: Vec<Point2>, pub holes: Vec<Vec<Point2>> }`, outer CCW, holes CW; `area()` net positive; `contains(&self, p: Point2) -> bool` (boundary unspecified); `triangulate(&self) -> Vec<[usize; 3]>` indexes outer ++ holes in order, edge-to-edge, no added points. `Solid::extrude(&Polygon, z0, height) -> Solid` and `Solid::sweep(&Polygon, path: &[Point3]) -> Solid` are associated functions; sweep puts the profile's 2D origin on `path[0]`. Methods `volume()`, `area()`, `centroid() -> Point3`, `is_closed_manifold()`, `euler_characteristic()` (integer), `to_stl(&self, path: &Path) -> io::Result<()>` (ASCII, round-trip precision, watertight edge-to-edge triangulation).

- [x] T2.2: Boolean CSG on closed polyhedral B-Reps
  - Known issue: #24 (landed with this bug on purpose; its audit failed, its judge passes)
  - Files: Cargo.lock, crates/bim-geom/
  - Opens: T2.1
  - Needs: .oracles/py/bin/python
  - Builder: fable
  - Verification: bash judges/T2.2.sh
  - Spec: `bim_geom::boolean(a: &Solid, b: &Solid, op: BoolOp) -> Result<Solid, GeomError>` with `BoolOp::{Union, Difference, Intersection}`, exact-predicate classification, handling coplanar faces, shared edges and touching vertices; results are closed 2-manifolds (or empty) with merged coplanar faces. Judge: random boxes, prisms and sweeps, including coplanar/touching cases, volumes vs manifold3d within 1e-9 relative, watertight per trimesh.
  - Details: `boolean` and `BoolOp` are exported at the crate root; `GeomError` derives Debug. `Solid::face_count() -> usize` counts maximal edge-connected coplanar regions (a face may have inner loops). `Difference` is a − b. An empty or zero-volume result is `Ok` with `face_count() == 0`, `volume() == 0`, and `to_stl` writes no facets.

- [x] T2.3: spatial index: R*-tree and SAH BVH with raycasting
  - Files: Cargo.toml, Cargo.lock, crates/bim-spatial/
  - Opens: T2.1
  - Verification: bash judges/T2.3.sh
  - Spec: Crate `bim-spatial`: `Aabb { min: [f64;3], max: [f64;3] }`; `RStarTree<T>` (insert with forced reinsert, remove, bulk load (STR), `query_intersects(&Aabb)`, `nearest(p, k)`); `Bvh` built with SAH over triangles (`Bvh::build(&[[ [f64;3];3 ]])`), `raycast(origin, dir) -> Option<(tri, t)>` (closest hit, Möller–Trumbore), `overlaps(&Bvh) -> Vec<(usize, usize)>` triangle-pair candidates. Judge checks equality with brute force on 100k random items and timing in release: 100k R*-tree bulk load < 300 ms, mean raycast on 1M triangles < 1 ms.
  - Details: `Aabb` is Copy with pub fields; intersection is closed (touching counts). `RStarTree<T>`: `new()`, `bulk_load(Vec<(Aabb, T)>) -> Self`, `insert(&mut self, b: Aabb, item: T)`, `remove(&mut self, b: &Aabb, item: &T) -> bool where T: PartialEq` (removes one entry with equal box and item), `len()`, `query_intersects(&self, q: &Aabb) -> Vec<&T>` (each match once), `nearest(&self, p: [f64; 3], k: usize) -> Vec<&T>` (min(k, len) items by ascending point-to-box distance, 0 inside). `Bvh::build(&[[[f64; 3]; 3]]) -> Bvh`; `raycast(&self, origin, dir) -> Option<(usize, f64)>` closest hit with t > 0 in units of `dir`, two-sided; `overlaps(&self, other: &Bvh) -> Vec<(usize, usize)>` each (self, other) pair whose triangle AABBs intersect, once.

- [x] T2.4: 3D constraint solver with symbolic Jacobians and DOF decomposition
  - Files: Cargo.lock, crates/bim-solver/
  - Opens: T1.3
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T2.4.sh
  - Spec: Extend bim-solver (T1.3 API unchanged): `Sketch3::new()`, `add_point3(x,y,z)`, `add_datum(Plane { normal, offset })`; constraints `Coincident3, Distance3, Parallel3, Perpendicular3, Angle3, OnDatum(point, datum)` (datum-locked). Equations are built as expression trees and differentiated symbolically (`Expr::diff`), not by finite differences. `decompose() -> Vec<Subsystem>` splits the constraint graph into independently solvable components (connected components, then rigid-cluster ordering), and `solve()` returns `SolveReport3 { subsystems: Vec<SubReport { points, status, dof }> }` so under/over-constrained parts are reported individually. Judge compares DOF counts and solved positions with python-solvespace.
  - Details: `Sketch3::add_point3(x, y, z) -> Point3Id`, `add_datum(Plane { normal: [f64; 3], offset: f64 }) -> DatumId` (plane {p : normal·p = offset}), `add(c: Constraint3)`, `point3(id) -> (f64, f64, f64)`; `Point3Id`, `DatumId` derive Debug, Clone, Copy, PartialEq. `Constraint3` is a new enum (2D `Constraint` unchanged): `Coincident3(p, q)`, `Distance3(p, q, d)`, `Parallel3(a, b, c, d)`, `Perpendicular3(a, b, c, d)`, `Angle3(a, b, c, d, θ)` (unsigned, radians in [0, π]), `OnDatum(p, datum)`; datums are fixed. `Expr` is Clone with `Expr::var(i)`, `Expr::constant(c)`, `sqrt(self)`, `+ - * /` and unary `-` on owned `Expr`, `diff(&self, var) -> Expr`, `eval(&self, &[f64]) -> f64`. `decompose(&self) -> Vec<Subsystem>`, `Subsystem { points: Vec<Point3Id>, .. }`: connected components of the graph joining each constraint's points (an unconstrained point is its own subsystem); `SolveReport3.subsystems` has the same partition. Per subsystem dof = 3·points − rank(J); status uses T1.3's tolerances: Inconsistent if residual > 1e-6, else UnderConstrained if dof > 0, else OverConstrained if some constraint is redundant (removing its equations leaves rank(J) unchanged), else WellConstrained. Solving one subsystem never moves another's points; an Inconsistent subsystem does not stop the others solving.

- [x] T2.5: analytical plane sectioning of solids
  - Files: Cargo.lock, crates/bim-geom/
  - Opens: T2.2
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T2.5.sh
  - Spec: `bim_geom::section(&Solid, &Plane3) -> Vec<Polygon3>` closed loops on the plane, outer loops CCW about the plane normal, holes CW, nested correctly; `section_area`. Handles planes through vertices, edges and coplanar faces (coplanar faces are included as section regions). Judge compares with trimesh section + shapely areas on random solids and booleans.
  - Details: `Plane3 { pub normal: Vec3, pub offset: f64 }` is the plane normal·p = offset (normal need not be unit); `Polygon3 { pub outer: Vec<Point3>, pub holes: Vec<Vec<Point3>> }` is one region (closing vertex optional); `section_area(&Solid, &Plane3) -> f64` is the total enclosed area; a plane meeting the solid only in edges or vertices yields an empty Vec.

- [ ] T2.6: exact hidden-line removal for polyhedral scenes
  - Files: Cargo.lock, crates/bim-geom/
  - Opens: T2.5, T2.3
  - Needs: .oracles/py/bin/python
  - Builder: fable
  - Verification: bash judges/T2.6.sh
  - Spec: `bim_geom::hlr(solids: &[Solid], view: &OrthoView) -> HlrResult { visible: Vec<Seg2>, hidden: Vec<Seg2> }` for orthographic views (`OrthoView { dir, up }`): feature edges (dihedral > 1e-9) and silhouettes are split at every projected crossing and classified exactly (visibility changes only at crossings), with collinear overlaps merged. Each segment carries the source solid index. Judge: closed-form scenes (cube, stacked boxes, occluding prisms) plus random scenes checked by casting rays from many points on each segment with trimesh.
  - Details: `OrthoView { pub dir: Vec3, pub up: Vec3 }` (dir from eye into the scene); projected coordinates are (p·r, p·v) with r = normalize(dir × up), v = r × normalize(dir); `Seg2 { pub a: Point2, pub b: Point2, pub solid: usize }`; `HlrResult { pub visible: Vec<Seg2>, pub hidden: Vec<Seg2> }`; `hlr`, `OrthoView`, `Seg2`, `HlrResult` at the crate root. Input solids have disjoint interiors (they may touch); edges shared by touching solids are reported once.

- [x] T2.7: building geometry compiler: schema → solids, joins, openings, glTF export
  - Files: Cargo.toml, Cargo.lock, crates/bim-model/, crates/bim-cli/
  - Opens: T2.2, T1.6
  - Needs: .oracles/py/bin/python, .oracles/node/node_modules/gltf-validator
  - Verification: bash judges/T2.7.sh
  - Spec: Crate `bim-model` compiles the document into solids per the "Derived geometry rules" of spec/schema.md: layered walls, L/T joins (mitred, no overlap), openings cut through hosts, slabs with openings, columns, beams. `bim export --format gltf` writes one mesh node per entity (`extras.id`). `bim solve --trade arch --json` reports `walls[].volume`, `side_area`, per-layer volumes, `slabs[]` (other arch fields may be empty until T4.2/T4.3). Judge rebuilds each element independently with manifold3d from the schema rules and compares volumes within 1e-6 relative; glTF passes Khronos glTF-Validator.
  - Details: Geometry, glTF frame and arch-output rules are in spec/schema.md and spec/cli.md (side_area, layer order, net slab area, T-joins, unequal L-join mitres, glTF y-up).

- [ ] T2.8: drawings: plans, sections, elevations with lineweights, hatches, associative dimensions → SVG
  - Files: Cargo.lock, crates/bim-draw/, crates/bim-cli/
  - Opens: T2.6, T2.7
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T2.8.sh
  - Spec: Crate `bim-draw` builds a `Drawing` (layers, polylines, hatched regions, dimension objects with entity references) for the views in spec/cli.md: cut geometry from T2.5 on layer `CUT` with hatches on `HATCH`, beyond geometry from T2.6 HLR on `PROJECTION`/`HIDDEN`, `Dimension` entities on `DIMENSION`. `bim export --format svg --view …` writes it with the layer ids, stroke widths and `data-entity` attributes of spec/cli.md. Associative: after an edit, the dimension text equals the new measured distance. Judge recomputes cut areas with shapely from manifold3d sections.
  - Details: View geometry, SVG units, ring/hatch, dash, inheritance and Dimension rules are in spec/cli.md ("View geometry").

- [ ] T2.9: DXF and PDF drawing export
  - Files: Cargo.lock, crates/bim-draw/, crates/bim-cli/
  - Opens: T2.8
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T2.9.sh
  - Spec: `bim export --format dxf|pdf --view …`. DXF: AutoCAD 2013 ASCII (AC1027) with layers named as in spec/cli.md, lineweights, `DASHED` ISO linetype on HIDDEN, HATCH entities, DIMENSION or MTEXT for dimensions, XDATA app `OPENBIM` holding the entity id; ezdxf `audit()` reports zero errors. PDF 1.7 with vector paths, one optional-content group per layer, correct line widths in points; pypdf reads it and finds the OCG names and dimension texts. Geometry equals the SVG export of T2.8 (judge compares coordinates).
  - Details: DXF strokes are LINE, LWPOLYLINE or POLYLINE with BYLAYER (or equal) lineweight; HIDDEN uses BYLAYER or DASHED linetype; every entity not on ANNOTATION carries XDATA `OPENBIM` whose first 1000-code string is the entity id; HATCH entities carry the cut entity's id and fill exactly its cut region; dimension text is explicit (DIMENSION text override, or MTEXT/TEXT on DIMENSION). PDF: one page, every stroke inside `/OC /<name> BDC … EMC` referencing an OCG in the page's `/Resources/Properties`; OCG names are exactly the layer names.

- [x] T2.10: direct modelling I: shapes, drawing, face finding, push/pull, erase, `bim sketch`
  - Files: Cargo.toml, Cargo.lock, crates/bim-sketch/, crates/bim-model/, crates/bim-cli/
  - Opens: T2.7, T1.8
  - Builder: fable
  - Verification: bash judges/T2.10.sh
  - Spec: spec/sketch.md for the `Shape` kind and its rules (planar faces, solid vs surface, stable face ids) and the commands `line`, `rect`, `circle`, `arc`, `pushpull` (including `new` and punch-through) and `erase`, applied atomically by `bim sketch <dir> <file.json>`. Shapes compile into the geometry model: glTF export has one node per Shape (its triangulated faces, `extras.id` = the Shape id), and a closed Shape's solid volume is exact.
  - Details: `bim-sketch` exports `run(doc: &bim_doc::Document, json: &str) -> Result<Vec<bim_doc::Op>, SketchError>` returning the ops that apply the whole command file (the CLI applies them atomically; nothing on error). After `rect` 4 × 3 and `pushpull` 2.5 the Shape is a closed solid of 30 m³ with 6 faces. Face finding works in any plane, not only z = 0. Within 1e-9 m points are equal.

- [ ] T2.11: direct modelling II: offset, follow-me, move/rotate/scale with copies, paint, components
  - Files: Cargo.lock, crates/bim-sketch/, crates/bim-model/
  - Opens: T2.10
  - Verification: bash judges/T2.11.sh
  - Spec: spec/sketch.md commands `offset`, `followme`, `move`, `rotate`, `scale` (stretching, triangle fan for non-planar faces, `copy` and `array`), `paint` and `make_component`, plus `ComponentDefinition`/`ComponentInstance` placement: each instance is its definition's Shape under the instance transform in glTF export and solids; the definition's Shape is not placed itself; editing it changes every instance.
  - Details: Same entry point `bim_sketch::run`. Volumes, areas and face counts after each command follow closed forms (e.g. follow-me of a square along an L path; offset of a rectangle by d leaves an inner (w − 2d)(h − 2d) face).

- [ ] T2.12: direct modelling III: inference engine and typed measurements
  - Files: Cargo.lock, crates/bim-sketch/
  - Opens: T2.10, T2.3
  - Verification: bash judges/T2.12.sh
  - Spec: spec/sketch.md Inference and Measurements: candidate kinds, the 10 CSS px radius, priority order and tie-breaks; the measurement grammar with units, feet and inches, two-value input, arrays and division, and angles.
  - Details: `bim_sketch::InferScene::new(scene: &str) -> Result<InferScene, SketchError>` parses the scene and builds its spatial index; `InferScene::infer_json(&self, query: &str) -> Result<String, SketchError>` answers a query; `bim_sketch::infer_json(scene: &str, query: &str) -> Result<String, SketchError>` equals `InferScene::new(scene)?.infer_json(query)`. Scene JSON `{"edges":[{"entity","index","a":[x,y,z],"b":[x,y,z]}],"faces":[{"entity","id","loops":[[[x,y,z],…],…]}],"guides":[{"entity","point","direction"?}],"level_elevation"}`; query `{"camera":{"eye","target","fov","width","height"},"cursor":[x,y],"from"?,"reference_edge"?:[[x,y,z],[x,y,z]]}`; result `{"point":[x,y,z]|null,"kind","ref":{"entity","index"|"id"}|null}`. `bim_sketch::parse_measurement(text: &str, default_unit: &str, angle: bool) -> Result<Measurement, SketchError>` with `pub enum Measurement { Length(f64), Lengths(f64, f64), Angle(f64), Array(u32), Divide(u32) }` (metres, radians). `InferScene::infer_json` answers a query on a 100,000-edge scene in under 2 ms (mean after construction, release build) using a spatial index.

## M3 Incremental engine, storage, sync

- [ ] T3.1: demand-driven incremental computation (salsa-style)
  - Files: Cargo.toml, Cargo.lock, crates/bim-incr/, crates/bim-model/, crates/bim-draw/
  - Opens: T2.8
  - Builder: fable
  - Verification: bash judges/T3.1.sh
  - Spec: Crate `bim-incr`: inputs with revisions, memoized derived queries with recorded dependencies, red-green validation and early cutoff (a recomputed query whose value is unchanged does not dirty its dependents). `bim-model` and `bim-draw` compute solids and drawings through it. `bim_model::Session::apply(ops) -> RecomputeStats { solids: Vec<String>, views: Vec<String> }` lists exactly what was recomputed. Editing one wall's `height` recomputes that wall, walls joined to it and its openings, and only drawings that show them; editing a level's `elevation` recomputes elements on that level; a no-op edit recomputes nothing. Judge also checks incremental results equal a from-scratch compile.
  - Details: `Session::new() -> Session` starts empty; `Session::apply(&mut self, ops: Vec<bim_doc::Op>) -> RecomputeStats` applies ops in order (Rust ops may set `_name`) and brings every element solid and registered view up to date. `RecomputeStats.solids` holds the `_name` of every entity whose solid query re-executed (walls, slabs, columns, beams, openings); `views` holds every registered view string whose drawing re-executed. `Session::add_view(&mut self, view: &str)` registers a view (computed at registration, not reported later). `Session::solid(&self, id: &str) -> Option<&bim_geom::Solid>` is the whole current solid; `Session::svg(&self, view: &str) -> Option<String>` equals `bim export --format svg`. Elements on a level are those whose geometry references it via `level`, `base` or `top`.

- [x] T3.2: SQLite store, revisions, commit/log/diff
  - Files: Cargo.toml, Cargo.lock, crates/bim-store/, crates/bim-cli/
  - Opens: T1.7
  - Builder: sonnet
  - Verification: bash judges/T3.2.sh
  - Spec: Crate `bim-store` (rusqlite, bundled): `<dir>/bim.sqlite` holds the CRDT state, the op log (content-addressed: op id = hash of its canonical encoding) and revisions; every `bim apply` is one SQLite transaction; `doc.oplog` stays as the text journal. CLI `commit`, `log`, `diff <dir> <revA> <revB>` per spec/cli.md. Judge: sqlite3 `PRAGMA integrity_check`, a process killed mid-apply leaves the previous state, revision diffs match.
  - Details: `log` op ids, revision diff keys and exit codes are in spec/cli.md.

- [x] T3.3: WebSocket sync between peers
  - Files: Cargo.toml, Cargo.lock, crates/bim-sync/, crates/bim-cli/
  - Opens: T3.2
  - Builder: sonnet
  - Verification: bash judges/T3.3.sh
  - Spec: Crate `bim-sync`: state-based delta sync over WebSocket (tungstenite): peers exchange version vectors, then the missing CRDT deltas; idempotent and order-insensitive. `bim sync serve` / `bim sync` per spec/cli.md. Judge: three project dirs edited offline (including concurrent edits to the same entity and a delete under a reference) sync through one server in different orders and end with equal hashes and equal diagnostics; a server restart mid-session does not lose data.
  - Details: `bim init` gives each project dir a random 32-bit CRDT peer id persisted in `bim.sqlite` (copies share it). The server persists every accepted delta to its project dir before replying (the server dir holds the merged state) and handles concurrent clients. `bim sync` exits non-zero without changing the local document when the server is unreachable.

## M4 Trade engines and plugin boundary

- [ ] T4.1: plugin boundary: TradeEngine/StandardRuleSet traits, registry, WASM component host
  - Files: Cargo.toml, Cargo.lock, crates/bim-plugin/, crates/bim-cli/
  - Opens: T2.7
  - Needs: .oracles/rust-wasip2
  - Verification: bash judges/T4.1.sh
  - Spec: Crate `bim-plugin` with the traits, `EngineContext`, `Violation` and `Registry` of spec/plugins.md; the CLI resolves `solve --trade` and `validate --standard` only through the registry. A wasmtime host loads `wasm32-wasip2` components of the `rule-set` WIT world with no WASI filesystem/network, bounded fuel and memory. Judge builds its own sample plugin from source (in judges/), runs `bim validate --standard plugin.wasm`, and checks a trapping plugin and an infinite-loop plugin both exit 3.
  - Details: `bim-plugin` exports `EngineContext`, `EngineError`, `Violation`, `TradeEngine`, `StandardRuleSet`, `Registry` at the crate root. `Registry::new()` is empty; `register_engine(&mut self, Box<dyn TradeEngine>)` and `register_rule_set(&mut self, Box<dyn StandardRuleSet>)` key by `name()`/`id()` in separate namespaces; `engine(&self, name) -> Option<&dyn TradeEngine>`, `rule_set(&self, id) -> Option<&dyn StandardRuleSet>`. The CLI builds its registry from the shipped engines (no built-ins inside bim-plugin, which would create a dependency cycle). Host rules and exit codes: spec/plugins.md.

- [ ] T4.2: architectural engine: rooms from wall topology, areas, volumes, takeoff
  - Files: Cargo.toml, Cargo.lock, crates/engines/arch/, crates/bim-cli/
  - Opens: T4.1
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T4.2.sh
  - Spec: Crate `bim-arch` (TradeEngine "arch"): planar arrangement of wall centrelines per level (splitting at intersections, T-junctions, dangling walls ignored), bounded faces → rooms with the area/volume rule of spec/schema.md, each room labelled by the `Space` whose `x y` lies inside it; per-layer material takeoff. Output per spec/cli.md `solve --trade arch`. Judge computes rooms independently with shapely (polygonize + per-wall inner-face offsets) on rectangular, L-shaped and non-orthogonal plans.
  - Details: Room rules (inner faces, next level, slab above, numbering, `space`) are in spec/schema.md and spec/cli.md. `rooms[].boundary` is the room's inner-face polygon (the polygon whose area is `area`), one `[x, y]` per vertex, closing vertex optional. The crate at `crates/engines/arch/` is `bim-arch`; it exports `pub struct ArchEngine` with `pub fn new() -> Self`, implementing `bim_plugin::TradeEngine` with `name()` = `arch`.

- [ ] T4.3: curtain walls, stairs and ramps
  - Files: Cargo.lock, crates/engines/arch/, crates/bim-model/
  - Opens: T4.2
  - Needs: .oracles/py/bin/python
  - Builder: sonnet
  - Verification: bash judges/T4.3.sh
  - Spec: Curtain wall: grid of `u_spacing × v_spacing` panels (last bay takes the remainder), mullions as rectangular solids on grid lines and the boundary, panels between mullions; Stair: `riser_count` risers of height (top − base)/riser_count with treads of depth `tread` as a stepped solid of `width`; Ramp: sloped solid. `solve --trade arch` fills `stairs[]` and `curtain_walls[]`. Judge checks counts, riser/tread values and volumes against closed forms and manifold3d.
  - Details: Curtain-wall grid lines along the wall lie at 0, u, 2u, … and L (a last bay shorter than 1e-9 is not created), heights at 0, v, … and H; panels = bays_u × bays_v; mullion_length = (bays_u + 1)·H + (bays_v + 1)·L. Mullions are `mullion_width` (in the wall plane) × `mullion_depth` (normal to the wall, centred on the centreline), centred on every grid line; verticals span level elevation to elevation + H, horizontals the full length. Panels fill each bay between mullion faces, `panel_thickness` thick, centred. A CurtainWall's glTF node is one closed mesh equal to the union of its non-overlapping pieces. Stair = union of `riser_count` blocks: block k spans [(k−1)·tread, k·tread] along the run from (x, y) in direction `angle`, full `width` centred on the run line, from base elevation to base + k·riser; `stairs[]` reports riser = (top − base)/riser_count, tread, risers. Ramp = wedge over the `length` × `width` rectangle starting at (x, y) in direction `angle`, centred, underside at base, top rising linearly from base at (x, y) to top at the far end.

- [x] T4.4: 3D frame direct-stiffness solver with Timoshenko beams, combinations, envelopes
  - Files: Cargo.lock, crates/engines/structural/
  - Opens: T1.4
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T4.4.sh
  - Spec: Extend bim-struct (2D API unchanged): `Frame3` with 6-DOF nodes, 12-DOF members (`E, G, A, Iy, Iz, J`, optional shear areas `Asy, Asz` → Timoshenko shear deformation), member orientation by a reference vector, nodal loads per load case, `LoadCombination`s, `analyze() -> Results3` with displacements, reactions, local end forces and per-member envelopes (min/max of N, Vy, Vz, T, My, Mz and deflection) over combinations, sampled at ≥ 11 stations. Judge: textbook closed forms (Euler-Bernoulli and Timoshenko cantilevers, torsion, portal frames) within 0.1%, and random space frames against PyNite within 0.1%.
  - Details: API: `Frame3::new()`, `add_node(x, y, z) -> usize`, `add_member(i, j, s: Section3, reference: [f64; 3]) -> usize`, `fix(node, [bool; 6])` (ux uy uz rx ry rz), `load(node, case: &str, [f64; 6])` (accumulating), `add_combination(LoadCombination { name: String, factors: Vec<(String, f64)> })`, `analyze() -> Result<Results3, String>` (Err when singular/unstable); `Section3 { e, g, a, iy, iz, j: f64, asy: Option<f64>, asz: Option<f64> }` (pub); `Results3::displacement(&self, combo: &str, node) -> [f64; 6]`, `reaction(combo, node) -> [f64; 6]`, `member_end_forces(combo, m) -> [f64; 12]`, `envelope(m) -> Envelope { n, vy, vz, t, my, mz: [f64; 2] /* [min, max] */, deflection: f64 }`. Local x runs i→j; local z is the unit component of the reference orthogonal to x (parallel is an error); y = z × x. Iz and Asy govern the x–y plane (Φy = 12·E·Iz/(G·Asy·L²)), Iy and Asz the x–z plane; a missing shear area means none. End forces are those the nodes exert on the member, local, ordered [N1, Vy1, Vz1, T1, My1, Mz1, N2, …, Mz2]; reactions are support-on-structure, global; displacements global. Envelopes: internal actions the part beyond a station exerts on the part from node i (tension positive), at ≥ 11 equally spaced stations, min/max over combinations (cases outside every combination ignored); `deflection` = max |local transverse displacement of the axis relative to the chord between displaced ends|.

- [ ] T4.5: structural engine: analytical model extraction, tributary areas, utilization
  - Files: Cargo.toml, Cargo.lock, crates/engines/structural/, crates/bim-cli/
  - Opens: T4.4, T4.1
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T4.5.sh
  - Spec: TradeEngine "struct": extract an analytical stick model from Column/Beam entities (member axes through section centroids, nodes merged within 1e-6, beams split where other members frame in, `Support`s and column bases at the lowest level fixed unless a Support says otherwise); AreaLoads distributed to supporting beams by tributary area (each slab point goes to its nearest supporting beam); PointLoads applied at nearest node; combinations from LoadCombination entities; utilization = |N|/(A·fy) + |My|/(Wy·fy) + |Mz|/(Wz·fy). Output per spec/cli.md. Judge: tributary areas for rectangular bays by closed form, reactions equal total load, envelopes vs PyNite.
  - Details: Analytical members: reference vector +z for Beams, +y for Columns; section_b along local y, section_h along local z: A = b·h, Iy = b·h³/12, Iz = h·b³/12, Wy = b·h²/6, Wz = h·b²/6, J = a·c³·(1/3 − 0.21·(c/a)·(1 − c⁴/(12·a⁴))) with a ≥ c; Euler–Bernoulli, no self-weight. A Beam's axis is at level elevation − z_offset − section_h/2; a Column's runs base to top through (x, y); axes are split wherever another member's end lies on them (1e-6). Tributary loads reach each beam as a distributed load following the width of its nearest-beam region. Tributary areas are measured on the slab's net plan region (SlabOpenings excluded); `tributary[].area` for a split member covers only that member's span. Output conventions: spec/cli.md. `bim_struct` exports `pub struct StructEngine` with `pub fn new() -> Self`, implementing `bim_plugin::TradeEngine` with `name()` = `struct`.

- [ ] T4.6: MEP hydraulics: port networks, continuity, Darcy-Weisbach, network solve
  - Files: Cargo.toml, Cargo.lock, crates/engines/mep/, crates/bim-cli/
  - Opens: T4.1
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T4.6.sh
  - Spec: Crate `bim-mep` (TradeEngine "mep"): directed graph of Ports and Segments per MepSystem; sinks' `flow` demands, sources with fixed `pressure`; loop networks solved by Newton (global gradient / Todini) with Hardy-Cross as a reference mode; head loss Darcy-Weisbach with friction factor from Colebrook-White solved to 1e-12 (laminar 64/Re below Re 2000), plus minor losses `minor_k·ρv²/2`; length from port coordinates; continuity residual reported. Output per spec/cli.md. Judge: single pipes vs closed form (0.1%), a textbook Hardy-Cross loop with constant f (0.1%), and random networks vs EPANET (WNTR, D-W headloss; 1% because EPANET approximates Colebrook).
  - Details: Output conventions are in spec/cli.md; `--method hardy-cross` selects the reference solver and agrees with the default Newton solver to 0.1%. Elevation head (ρ·g·Δz) is not part of the contract. Above Re 2000 the friction factor is Colebrook-White (no transition interpolation). The crate at `crates/engines/mep/` is `bim-mep`; it exports `pub struct MepEngine` with `pub fn new() -> Self`, implementing `bim_plugin::TradeEngine` with `name()` = `mep`.

- [ ] T4.7: duct/pipe sizing, electrical circuits, hard/soft clash detection
  - Files: Cargo.lock, crates/engines/mep/, crates/bim-model/, crates/bim-cli/
  - Opens: T4.6, T2.3
  - Needs: .oracles/py/bin/python
  - Builder: sonnet
  - Verification: bash judges/T4.7.sh
  - Spec: Sizing: smallest nominal size (round: DN 15,20,25,32,40,50,65,80,100,125,150,200,250,300 mm; rect ducts in 50 mm steps) keeping velocity ≤ 2.0 m/s (water) or ≤ 6.0 m/s (air) and pressure gradient ≤ 400 Pa/m (water) or 1.5 Pa/m (air); rect hydraulic diameter 2wh/(w+h). Circuits: current = Σwatts/voltage, voltage drop = I·2·ρ·L/A. Segments get solids (round as 16-gon prisms). `bim clash` per spec/cli.md using the BVH: hard = positive-volume intersection, soft = clearance violation. Judge: closed forms for sizing/circuits; clash volumes vs manifold3d intersections, soft distances vs trimesh proximity.
  - Details: Nominal DN is the inside diameter in mm. A rect duct keeps its authored height; its width is the smallest multiple of 50 mm meeting the limits; velocity = |flow|/(w·h); the gradient is the Darcy-Weisbach friction loss per metre with Colebrook f at the hydraulic diameter for the solved flow. Ports referenced only by Circuits are not part of a hydraulic network; a MepSystem without Segments solves trivially. Segment solids (spec/schema.md) appear in the glTF export with `extras.id`.

- [ ] T4.8: trade packages: WebAssembly trade engines, extension geometry, generic trade metrics
  - Files: Cargo.lock, crates/bim-plugin/, crates/bim-model/, crates/bim-cli/, crates/engines/arch/, crates/engines/structural/, crates/engines/mep/
  - Opens: T1.8, T4.1, T4.2, T4.5, T4.7
  - Needs: .oracles/rust-wasip2
  - Verification: bash judges/T4.8.sh
  - Spec: Trade packages per spec/plugins.md: a package's `engine` component (`openbim:trade@0.1.0`, world `trade-engine`) registers as a trade engine and as a rule set under the package name; `bim solve --trade <package>` and `bim validate --standard <package>` run it under the rule-set sandbox; `TradeEngine::metrics` and `Registry::engine_names` exist and the built-in engines report the metrics of spec/plugins.md; `bim plugin list` gains `trades`. Extension kinds with `box` or `segment` geometry compile to solids (spec/schema.md), so they appear in glTF export and in `bim clash`. The judge builds its own sample trade package from source.
  - Details: The host passes `rooms` exactly as `bim solve --trade arch` reports them and `solids` for every entity with a derived solid (core and extension), each list sorted by id; `min`/`max` are [x, y, z]. Clash `matrix` keys for extension kinds use the full kind name (e.g. `fire:Sprinkler`). Sandbox and exit codes as for `rule-set` components.

- [ ] T4.9: product catalogues: Product, Furniture and Roof, effective props, finish take-off
  - Files: Cargo.toml, Cargo.lock, crates/bim-catalog/, crates/bim-model/, crates/engines/arch/, crates/bim-cli/
  - Opens: T1.8, T4.3, T4.7
  - Verification: bash judges/T4.9.sh
  - Spec: spec/catalogs.md: `bim catalog list` and `bim catalog use` with the catalogue validation rules; `Product` entities as specified; effective props for `Opening.product` and `Furniture.product` in geometry and engines; Roof and Furniture solids per spec/schema.md in glTF export and `bim clash`; `bim solve --trade arch` adds `products` and `roofs` (Take-off).
  - Details: `bim catalog use` is atomic and idempotent (twice with an unchanged catalogue leaves `bim hash` unchanged); after the catalogue file changes, rerunning it updates the Product (props removed from the catalogue are removed) and every element's effective props. An Opening's effective `width`/`height` drive the host wall's cut and the arch outputs.

## M5 Standards packs and interoperability

- [x] T5.1: STEP (ISO 10303-21) reader/writer and typed IFC4 entity layer
  - Files: Cargo.toml, Cargo.lock, crates/standards/ifc/
  - Opens: T1.6
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T5.1.sh
  - Spec: Add `"crates/standards/*"` to workspace members. Crate `bim-ifc`: streaming Part 21 lexer/parser (header, entity instances, typed values, lists, enums, `$`, `*`, derived, binary, string escapes incl. `\X2\`), a writer that emits files ifcopenshell parses identically, and typed access to the IFC4 entities needed by T5.2/T5.3. Judge round-trips ifcopenshell-generated files and checks every instance's attributes match via ifcopenshell.
  - Details: `bim_ifc::StepFile::read<R: BufRead>(r) -> Result<StepFile, StepError>`, `StepFile::write<W: Write>(&self, w) -> io::Result<()>`, `StepFile::instances()` iterates `&Instance { id: u64, entity: String /* upper-case */, params: Vec<Param> }`; `pub enum Param { Unset, Derived, Integer(i64), Real(f64), String(String) /* decoded */, Binary(String) /* bit string, unused leading bits dropped */, Enum(String) /* no dots */, Ref(u64), List(Vec<Param>), Typed(String, Box<Param>) }`. The writer keeps instance ids and header fields. Truncated input, a missing `ENDSEC;`/`END-ISO-10303-21;`, unterminated strings and unbalanced parentheses are errors.

- [x] T5.2: IFC4 export with geometry and property sets
  - Files: Cargo.lock, crates/standards/ifc/, crates/bim-cli/
  - Opens: T5.1, T2.7
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T5.2.sh
  - Spec: `bim export --format ifc`: IfcProject/Site/Building/BuildingStorey hierarchy, IfcWall (with IfcMaterialLayerSet), IfcSlab, IfcColumn, IfcBeam, IfcOpeningElement + IfcRelVoidsElement, IfcDoor/IfcWindow fillings, IfcSpace, Pset_*Common property sets, stable GlobalIds derived from entity ids. Judge: `ifcopenshell.validate` zero errors; ifcopenshell.geom volumes per element equal bim-model volumes within 1e-4 relative.
  - Details: The GlobalId of the object exported for entity e is `ifcopenshell.guid.compress(format(u128(e.id), '032x'))`; storeys carry their Level's GlobalId and an IfcOpeningElement its Opening's. Door/window fillings get `Pset_DoorCommon`/`Pset_WindowCommon`; every exported IfcWall/IfcSlab/IfcColumn/IfcBeam/IfcSpace carries its `Pset_<Class>Common`.

- [ ] T5.3: IFC4 import into the document graph
  - Files: Cargo.lock, crates/standards/ifc/, crates/bim-cli/
  - Opens: T5.2, T2.10
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T5.3.sh
  - Spec: `bim import <dir> f.ifc`: storeys → Levels, walls/slabs/columns/beams/openings/spaces → schema entities (parametric where the IFC is a standard extrusion; otherwise imported as a `Shape`, spec/sketch.md), placements resolved, property sets kept as props. Judge generates IFC files at run time with ifcopenshell.api (independent producer) and checks element counts and volumes within 1e-4; import → export → ifcopenshell volumes conserve within 1e-4.
  - Details: Import reads IFC4 and IFC2X3. An imported entity's id is the 26-char ULID spelling of the 128 bits of its GlobalId (so re-export reproduces it). Property-set values become props `<PsetName>.<PropName>`: labels/text → Text, numeric measures and integers → Num, IfcBoolean → Bool. An Opening's `kind` comes from its filling (IfcDoor → door, IfcWindow → window). A non-parametric element becomes a `Shape` whose `classification` is its IFC class, whose `level` is its storey and whose faces are its body's faces (a non-planar face is split into triangles fanned from its first vertex); export writes a placed closed Shape as an occurrence of its `classification` class with an IfcFacetedBrep of its faces (the rule T5.9 builds on).

- [ ] T5.4: reference-corpus round-trip judge (Duplex, Schependomlaan, buildingSMART samples)
  - Files: Cargo.lock, crates/standards/ifc/, crates/bim-model/, crates/bim-cli/
  - Opens: T5.3, T2.9
  - Needs: corpus/ifc, .oracles/py/bin/python
  - Verification: bash judges/T5.4.sh
  - Spec: Every `corpus/ifc/*.ifc` file imports without error, runs `solve --trade arch|struct|mep` without error, exports to IFC4 and to SVG/DXF plan sheets per storey; per-element volume and total storey slab area computed by ifcopenshell on the original vs on the round-tripped file agree within 1e-4 relative; DXF passes ezdxf audit. Per-file runtime < 60 s.
  - Details: Corpus files may be IFC2X3 or IFC4. Per-element volume covers IfcWall, IfcSlab, IfcColumn, IfcBeam (and subtypes) with a Body representation. Storey sheets use `plan:<storey Level id>@1.2`. The 60 s limit applies per corpus file.

- [ ] T5.5: ifcJSON import/export
  - Files: Cargo.lock, crates/standards/ifc/, crates/bim-cli/
  - Opens: T5.3
  - Needs: .oracles/py/bin/python
  - Builder: sonnet
  - Verification: bash judges/T5.5.sh
  - Spec: `bim export --format ifcjson` and `bim import <dir> f.json` (ifcJSON object list with `type`, `globalId`, nested objects); round trip doc → ifcJSON → doc preserves the content hash; ifcJSON produced from an ifcopenshell file (judge converts) imports with the same element counts and volumes as importing the .ifc.
  - Details: ifcJSON is `{"type":"ifcJSON","version":"0.0.1","schemaIdentifier":"IFC4","data":[…]}`: one object per IfcRoot, `type` = IFC entity name, `globalId` in UUID form (or the 22-char form), lowerCamelCase attributes, references to IfcRoot as `{"type","ref": <globalId>}`, other entities nested inline, typed values `{"type":"IfcLabel","value":…}`. A UUID globalId maps to the same entity id as its 22-char form. (`content_hash` excludes `_created_by`/`_stamp`, spec/schema.md.)

- [x] T5.6: BCF 3.0 issue tracking
  - Files: Cargo.toml, Cargo.lock, crates/standards/bcf/, crates/bim-cli/
  - Opens: T3.2, T5.2
  - Needs: .oracles/py/bin/python, .oracles/bcf-xsd
  - Builder: sonnet
  - Verification: bash judges/T5.6.sh
  - Spec: Crate `bim-bcf`: topics, comments, viewpoints (camera, components by IfcGuid derived as in T5.2), `Issue` entities mirrored in the document; `bim bcf` create/export/import per spec/cli.md. Judge validates every XML file in the bcfzip against the official buildingSMART BCF 3.0 XSDs with xmlschema and checks import(export(x)) preserves topics, comments and components.
  - Details: `bim bcf <dir> create --title <t> --entity <id> [<id>…]` prints the new topic Guid as its last stdout line; the mirrored Issue's id is that Guid (as a seed), `status` = TopicStatus, `entities` = Refs to the components. Component IfcGuids are derived as in T5.2. import(export(x)) preserves topic Guid, title, type, status, creation date (as an instant), author, description, labels; comments (Guid, date, author, text, viewpoint link); and each viewpoint's component IfcGuids. A non-zip or a reference to a missing entity exits 2 with nothing applied. `bim bcf import` mirrors every imported topic the same way (Issue id = topic Guid as a seed, `status` = TopicStatus, `entities` = Refs to its viewpoint components).

- [ ] T5.7: declarative code-rule packs (IBC, Eurocode, accessibility), hot-swappable
  - Files: Cargo.toml, Cargo.lock, crates/standards/codes/, crates/bim-cli/
  - Opens: T4.3, T4.1
  - Builder: sonnet
  - Verification: bash judges/T5.7.sh
  - Spec: Crate `bim-codes` (StandardRuleSet) loading packs per spec/plugins.md with the full check vocabulary and the three shipped packs. `bim validate --standard ibc|eurocode|accessibility|<path>`. Judge fixtures: corridors, doors, stairs, ramps and walls just inside and just outside each limit (closed-form measured values within 1e-9), a pack file edited between two runs changes the result with no rebuild, unknown check names exit 2.
  - Details: Pack semantics, exit codes and the shipped rules are in spec/plugins.md.

- [ ] T5.8: IFC4 export of catalogue products, furniture, roofs, finishes and extension kinds
  - Files: Cargo.lock, crates/standards/ifc/, crates/bim-cli/
  - Opens: T5.3, T4.8, T4.9
  - Verification: bash judges/T5.8.sh
  - Spec: `bim export --format ifc` also writes: each Product as a type object by category (`door` IfcDoorType, `window` IfcWindowType, `furniture` IfcFurnitureType; `wall_finish`, `floor_finish`, `ceiling_finish`, `roof_covering` IfcCoveringType with PredefinedType CLADDING, FLOORING, CEILING, ROOFING; other categories IfcBuildingElementProxyType) linked to its occurrences by IfcRelDefinesByType, with Pset_ManufacturerTypeInformation (Manufacturer, ModelReference = sku, ModelLabel = name) and a property set `openbim_Catalogue` holding the product's own props; Furniture as IfcFurniture and Roof as IfcRoof with their solids; each finish reference as an IfcCovering related to its element by IfcRelCoversBldgElements; each extension entity as IfcBuildingElementProxy with ObjectType = its kind and a property set `<package>_<Kind>` holding its Num/Text/Bool props. All readable by ifcopenshell; T5.2 behaviour unchanged.
  - Details: Custom property sets never use the reserved `Pset_` prefix. Occurrence `Name` = the entity's `_name`; type `Name` = the Product's `name` and `Tag` = the Product id. Elements with a `level` Ref are contained in that storey. List-valued props export as IfcPropertyListValue.

- [ ] T5.9: IFC4 export of direct-modelling shapes and components
  - Files: Cargo.lock, crates/standards/ifc/, crates/bim-cli/
  - Opens: T5.8, T2.11
  - Builder: sonnet
  - Verification: bash judges/T5.9.sh
  - Spec: Each placed closed Shape exports as an occurrence of its `classification` class (default IfcBuildingElementProxy) with an IfcFacetedBrep of its faces (as T5.3 already does); a ComponentInstance exports as an occurrence of its definition Shape's `classification` class (its type object is that class's type); each ComponentDefinition as a type object (the occurrence class's type, IfcBuildingElementProxyType by default) with an IfcRepresentationMap, and each ComponentInstance as an occurrence using an IfcMappedItem with the instance transform, linked by IfcRelDefinesByType. Open surfaces export as IfcBuildingElementProxy with an IfcShellBasedSurfaceModel. Painted faces carry IfcStyledItem colours from the product's `colour` prop when present.
  - Details: Volumes computed by ifcopenshell match the shape volumes within 1e-6 relative; instance placements match their transforms within 1e-9, and a placed Shape's IfcFacetedBrep vertices equal the Shape's vertices under its `transform` within 1e-9. GlobalIds follow T5.2; a ComponentDefinition's type object carries the definition's GlobalId. The IfcFacetedBrep has one IfcFace per Shape face. IfcRepresentationMap.MappingOrigin is the identity placement. A painted face's colour is an IfcColourRgb whose Red, Green and Blue are the sRGB channels of the product's `colour` divided by 255, in an IfcSurfaceStyle attached by an IfcStyledItem to that IfcFace.

## M6 Agent interface

- [ ] T6.1: Datalog query engine over the semantic graph
  - Files: Cargo.toml, Cargo.lock, crates/bim-query/, crates/bim-cli/
  - Opens: T4.5, T4.6, T4.2
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T6.1.sh
  - Spec: Crate `bim-query`: parser for the language in spec/mcp.md, stratification (reject unstratifiable negation with an error), semi-naive bottom-up evaluation, the built-in relations derived from the document and engines. `bim query <dir> <program-file> <atom> --json` prints `{"rows": …}`. Judge compares with networkx (transitive closure, adjacency, reachability) on generated buildings and checks semi-naive and naive evaluation agree.
  - Details: `bim query --json` rows are sorted and deduplicated exactly as the MCP `query` tool's. `bim query … --naive` evaluates by naive bottom-up iteration (reference mode) and prints identical rows. Rows hold the query atom's arguments in order; strings and ids (`_name`) are JSON strings, numbers JSON numbers; numeric constants match Num values by numeric equality (`3` matches `3.0`). A program that fails to parse or cannot be stratified exits 2. `bears_on`: Slab→Beam for each beam receiving tributary area from the slab; Beam→Column for each column whose `top` level is the beam's level and whose axis meets the beam at a node; Column→Column for the column directly below (same x, y, its `top` = this one's `base`); Column→Support for a Support at the column base. `bounds(Wall, Room)` holds only for walls with a centreline segment on the room's boundary (dangling walls bound nothing). `feeds` uses the solved flow sign.

- [ ] T6.2: MCP server over stdio
  - Files: Cargo.toml, Cargo.lock, crates/bim-mcp/
  - Opens: T6.1, T4.7, T5.7, T2.8
  - Needs: .oracles/node/node_modules/@modelcontextprotocol/sdk
  - Verification: bash judges/T6.2.sh
  - Spec: Binary `bim-mcp` implementing spec/mcp.md tools `query`, `get_entity`, `render_view`, `clash_matrix`, `structural_utilization` and the resources. Judge drives it with the official MCP TypeScript SDK client: initialize, list tools (schemas valid JSON Schema), call each tool, read each resource, malformed input → `isError`, server survives.
  - Details: Resource naming, query rows and error conventions are in spec/mcp.md.

- [ ] T6.3: transactional mutations: propose_transaction / commit_transaction
  - Files: Cargo.lock, crates/bim-mcp/
  - Opens: T6.2, T4.8
  - Needs: .oracles/node/node_modules/@modelcontextprotocol/sdk
  - Builder: sonnet
  - Verification: bash judges/T6.3.sh
  - Spec: Dry-run validation per spec/mcp.md: proposing never changes `bim hash`; the report's clash, violation and per-trade metric numbers equal those of running the CLI on a copy with the ops applied; commit applies exactly the proposed ops; a commit after an intervening change is rejected as stale; invalid ops give `valid: false` with reasons.
  - Details: An invalid proposal returns `{"valid": false, "reasons": [string, …]}` (non-empty) and its tx cannot be committed; missing or non-list `ops`, or a missing `tx`, is a tool error. `clashes.*` = number of `hard` entries of `bim clash --json`; `violations.*` = total violations of `bim validate --standard ibc|eurocode|accessibility --json` summed over the three packs; `violations.new` = violation objects present after but not before, matched by (rule, entity); `trades.<trade>.<metric>.before/after` = that engine's metrics (spec/plugins.md) on the document before and after, for every `Registry::engine_names()` entry (so `trades.struct.max_utilization`, `trades.mep.max_dp`, `trades.arch.total_area`); `conflicts` = the `conflicts` of `bim validate --json` with the ops applied.

## M7 Rendering and WebAssembly

- [ ] T7.1: bim-wasm: WebAssembly build with zero-copy typed-array views
  - Files: Cargo.toml, Cargo.lock, crates/bim-wasm/, packages/bim-wasm/
  - Opens: T3.1, T4.2
  - Needs: .oracles/node/node_modules
  - Verification: bash judges/T7.1.sh
  - Spec: wasm-bindgen crate exposing the `Model` API of spec/web.md, built with wasm-pack by `packages/bim-wasm/build.sh` into `packages/bim-wasm/pkg-node` and `pkg-web` (git-ignored; the judge runs the build script). Judge (node): `positions().buffer === memory.buffer`, values equal the native glTF export, `apply` bumps `generation`, `hash()` equals `bim hash` on the same ops, solve output equals the CLI's.
  - Details: Typed-array semantics and package entry points are in spec/web.md. wasm-pack fetches wasm-bindgen-cli on its first build (cached afterwards).

- [ ] T7.2: render core: tessellation, family instancing, BVH frustum culling
  - Files: Cargo.toml, Cargo.lock, crates/bim-render/
  - Opens: T3.1, T2.3
  - Verification: bash judges/T7.2.sh
  - Spec: Crate `bim-render` (no GPU): builds render buffers from compiled solids; identical solids up to rigid transform share one family mesh with per-instance transforms; `cull(&Frustum) -> Vec<u32>` via BVH; per-triangle entity ids; incremental buffer update touching only changed entities (reports dirty ranges). Judge: instance dedupe counts on repeated columns/doors, culling equals brute force on random cameras, dirty ranges minimal.
  - Details: API: `RenderScene::new()`, `set_entity(&mut self, id: bim_doc::EntityId, solid: &bim_geom::Solid)` (insert or replace), `flush(&mut self) -> Vec<DirtyRange>`; accessors `positions()`/`normals() -> &[f32]` (world xyz per vertex), `indices() -> &[u32]`, `triangle_entity() -> &[u32]`, `entity_id(index: u32) -> EntityId`, `instances() -> &[f32]` (16 per instance, column-major, proper rigid), `instance_mesh()`/`instance_entity() -> &[u32]`, `mesh_positions(mesh: u32) -> &[f32]`, `mesh_indices(mesh: u32) -> &[u32]`. Every entity has exactly one instance; solids congruent under a proper rigid motion (1e-6) share one family mesh; entity indices are stable, new entities appended. `Frustum::new(planes: [[f64; 4]; 6])` with a·x + b·y + c·z + d ≥ 0 inside; `cull(&self, &Frustum) -> Vec<u32>` returns sorted instance indices whose world AABB is not entirely outside any plane. `pub struct DirtyRange { pub kind: BufferKind, pub start: usize, pub end: usize }` (end exclusive) indexes the accessor's slice; `BufferKind::{Positions, Normals, Indices, TriangleEntity, Instances, InstanceMesh, InstanceEntity}` derives Debug, Clone, Copy, PartialEq. A flush reports ranges covering every changed element and only elements of changed entities; a flush with no changes reports none.

- [ ] T7.3: native wgpu renderer: picking, section box with capped cuts, SSAO
  - Files: Cargo.lock, crates/bim-render/, crates/bim-cli/
  - Opens: T7.2
  - Verification: bash judges/T7.3.sh
  - Spec: wgpu (Metal/Vulkan/DX12; WebGPU/WebGL2 in the browser via T8.1) offscreen renderer: instanced draws from T7.2, depth, ID buffer, 6-plane section box with stencil-capped cut faces in a cap colour, SSAO. CLI `bim render <dir> --eye x,y,z --target x,y,z --size WxH --out f.png [--section-box x0,y0,z0,x1,y1,z1] [--ids ids.png] [--no-ssao]` writing PNGs, and `bim render … --pick px,py` printing the entity id. Judge: pick returns the expected wall, section box exposes cap colour where the cut crosses a solid and background elsewhere, SSAO darkens an inside corner relative to an open face.
  - Details: `bim render` uses a perspective camera with up = +z; image x grows to the right of the view direction and PNG row 0 is the top; `--pick px,py` uses the same pixel coordinates, may be combined with `--out`, and prints only the entity id. `--ids` writes one uniform value for background and a distinct value per entity (instance). Section-box cut faces are filled with one flat, unlit cap colour (no lighting or SSAO), distinct from background and material shading.

- [ ] T7.4: sheet composition, ISO linetypes, hatch library
  - Files: Cargo.lock, crates/bim-draw/, crates/bim-cli/
  - Opens: T2.9
  - Needs: .oracles/py/bin/python
  - Builder: sonnet
  - Verification: bash judges/T7.4.sh
  - Spec: `bim sheet <dir> sheet.json --format svg|pdf|dxf`: sheet.json `{ "size": "A1"|"A3"|[w,h], "viewports": [{ "view", "scale", "x", "y", "w", "h" }], "title": {…} }`; viewports clipped to their frames, scaled lineweights, ISO 128 linetypes (dash patterns scale with lineweight), hatch patterns (ANSI31, concrete, insulation) per material, title block. Judge: sheet size in mm matches ISO 216, each viewport's content lies inside its frame (ezdxf/pypdf/SVG parse), dash lengths per ISO 128-20 for the given pen.
  - Details: Sheet coordinates are mm from the sheet's lower-left corner, y up; a viewport's `x, y` is its frame's lower-left corner; `scale` is n of 1:n; named sizes are ISO 216 landscape. SVG: root `width`/`height` in absolute units with a `viewBox`; layer groups keep the layer name as a token of `id`/`class`/`data-layer` (ids unique per viewport). PDF: MediaBox is the sheet. DXF: `$INSUNITS` = 4, `$LIMMIN` = (0,0), `$LIMMAX` = (w,h), model space at 1:1 mm with viewport content clipped geometrically to the frames. Lineweights are the spec/cli.md pens in paper mm at every scale; HIDDEN uses ISO 128-20 type 02 (dash 12d, gap 3d, d = pen width). Hatch by layer material: `concrete` → a pattern whose name contains `CONC`, `insulation` → one containing `INSUL`, others → ANSI31. `title` is an object of strings, each rendered as extractable text in the title block.

- [ ] T7.5: stress corpus, criterion benchmarks, `bim bench` performance gate
  - Files: Cargo.toml, Cargo.lock, crates/bim-bench/, crates/bim-cli/, crates/bim-render/, crates/bim-incr/, crates/bim-model/
  - Opens: T7.3
  - Needs: .oracles/py/bin/python
  - Verification: bash judges/T7.5.sh
  - Spec: Crate `bim-bench`: deterministic synthetic hospital/city generator (100,000 parametric elements, ≥ 2,000,000 triangles: floors of rooms, walls with openings, columns, beams, ducts), criterion benches for compile, incremental edit, raycast and culling; `bim bench --elements 100000 --json` per spec/cli.md, measuring incremental edit latency (one wall thickness and one level elevation) and native wgpu frame time (orbit + section-box moves, 300 frames). Judge runs it and requires incremental_ms < 50, frame_ms_p95 < 16.6, triangles ≥ 2e6. Measured honestly on this machine (Intel i5-6600, Radeon R9 M395).
  - Details: Criterion benchmark ids in `bim-bench` contain `compile`, `incremental`, `raycast` and `cull`. `elements` reported is at least `--elements`. The 100k run is allowed up to 300 s of wall time.

## M8 Web viewport and Studio UI/UX

- [ ] T8.1: web-viewport: WebGPU renderer with WebGL2 fallback, picking, section box, Three.js adapter
  - Files: package.json, package-lock.json, packages/web-viewport/
  - Opens: T7.1
  - Needs: .oracles/node/node_modules/playwright-core
  - Verification: bash judges/T8.1.sh
  - Spec: TypeScript package per spec/web.md, consuming zero-copy views from bim-wasm (no geometry copies: GPU uploads read directly from the wasm-memory views), instanced draws, section box with caps, ID-buffer picking, `frameStats`; `mountThree` returns a Group whose BufferAttribute arrays are the wasm views. Root `package.json` defines npm workspaces. Judge (Playwright + system Chrome, headless) loads its own test page: explicit `webgl2` renders and picks the expected id, `auto` chooses a working backend, section box hides geometry outside, `mountThree` attributes share `memory.buffer`.
  - Details: Build, camera, picking and section-box conventions are in spec/web.md.

- [ ] T8.2: 2D sheet viewer: high-DPI Canvas2D with lineweights and linetypes
  - Files: package-lock.json, packages/web-viewport/
  - Opens: T8.1, T7.4
  - Needs: .oracles/node/node_modules/playwright-core
  - Builder: sonnet
  - Verification: bash judges/T8.2.sh
  - Spec: `createSheetView` renders T7.4 SVG sheets to Canvas2D at devicePixelRatio (backing store = CSS size × dpr), stroke widths in device pixels = mm × px/mm × zoom, dashed linetypes from the SVG dash arrays, hatches, pan/zoom with pointer and wheel, snap glyphs at endpoints under the cursor. Judge: pixel-measured stroke widths at dpr 1 and 2, dash periods, snap glyph appears near an endpoint.
  - Details: Sheet-view conventions are in spec/web.md.

- [ ] T8.3: Studio app: modelling tools, views, properties, undo/redo, IndexedDB
  - Files: package.json, package-lock.json, apps/studio/
  - Opens: T8.2
  - Needs: .oracles/node/node_modules/playwright-core
  - Verification: bash judges/T8.3.sh
  - Spec: Vite + TypeScript app per spec/web.md with a professional layout (toolbar, level picker, 3D viewport and 2D plan/sheet side by side or tabbed, properties panel, history, status bar), designed with consistent spacing, type scale and both light/dark themes. Judge (Playwright): draw walls by clicking on the plan, pick in 3D fills the properties panel, editing height updates both views, undo/redo, reload restores from IndexedDB, keyboard shortcuts, no console errors.
  - Details: Studio behaviour conventions are in spec/web.md.

- [ ] T8.4: Studio collaboration, checks and performance: presence/sync, issues, validation, a11y, frame-time
  - Files: Cargo.lock, package-lock.json, apps/studio/, packages/web-viewport/, crates/bim-wasm/, packages/bim-wasm/
  - Opens: T8.3, T3.3, T5.6, T5.7
  - Needs: .oracles/node/node_modules/playwright-core, .oracles/node/node_modules/@axe-core/playwright
  - Verification: bash judges/T8.4.sh
  - Spec: `?sync=` collaboration through `bim sync serve` with presence avatars; Issues panel (BCF topics linked to selected entities); Validate panel listing code violations; section-box tool; responsive at 390 px and 1440 px with no horizontal overflow; zero serious/critical axe-core violations; in-browser frame-time harness on a 10,000-element model reports p95 < 16.6 ms in the status bar. Judge: two browser contexts converge after concurrent edits, axe and overflow checks, frame-time read from the status bar.
  - Details: Adds `validate(standard)` and `issues()` to the bim-wasm `Model` (spec/web.md) for the Validate and Issues panels. Studio behaviour conventions are in spec/web.md.

- [ ] T8.5: Studio product catalogues: import, browse, place furniture, assign products and finishes
  - Files: package-lock.json, apps/studio/
  - Opens: T8.4, T4.9
  - Verification: bash judges/T8.5.sh
  - Spec: The Studio imports a `catalog.json` (spec/catalogs.md) through `catalog-import`, lists products by category in `catalog-products` (one `product-<catalog>:<sku>` item each, showing name and manufacturer), places the selected furniture product with `tool-furniture` by clicking in the plan, and `assign-product` applies the selected product to the selected element by category: door or window to an Opening's `product`; wall finish to both faces of a Wall; floor finish to a Slab's `finish_top`; ceiling finish to `finish_bottom`; roof covering to a Roof's `covering`. Placing or assigning creates the Product entity exactly as `bim catalog use` would. `props-panel` shows effective props. Zero serious/critical axe-core violations with the catalogue panel open.

- [ ] T8.6: Studio direct modelling I: drawing tools, push/pull, inference feedback, measurements box
  - Files: Cargo.lock, package-lock.json, apps/studio/, packages/web-viewport/, crates/bim-wasm/, packages/bim-wasm/
  - Opens: T8.4, T2.12
  - Verification: bash judges/T8.6.sh
  - Spec: wasm `Model.sketch` and `Model.infer` per spec/web.md. In `viewport-3d`, `tool-line`, `tool-rect`, `tool-circle`, `tool-arc` and `tool-pushpull` draw with live inference: the snap point follows `bim_sketch::infer`, `inference-tip` names the kind (Endpoint, Midpoint, Intersection, On Edge, Guide, On Red/Green/Blue Axis, Parallel, Perpendicular, On Face) and axis guides draw red, green and blue. Typing while a tool runs fills `measurements`; Enter applies it (e.g. `4000,3000` sizes a rectangle, `2500` a push/pull). Double-click with push/pull repeats the last distance; Ctrl/Cmd makes a new face; Esc cancels. Keys: L line, R rectangle, C circle, A arc, P push/pull, Space select (S stays select, as in T8.3). Orbit with the middle button, pan with Shift + middle, zoom with the wheel, `zoom-extents` (Shift+Z) frames the model. `Model.infer` answers under 8 ms p95 per query in Chrome (after one warm-up query) on a 100,000-Shape model (1,200,000 edges).

- [ ] T8.7: Studio direct modelling II: move, rotate, scale, offset, follow-me, tape, eraser, paint, components, scenes
  - Files: package-lock.json, apps/studio/, packages/web-viewport/
  - Opens: T8.6, T2.11, T8.5
  - Verification: bash judges/T8.7.sh
  - Spec: `tool-move` (M, with Ctrl/Cmd to copy and `x5`, `5x` or `/5` typed after a copy for arrays), `tool-rotate` (Q, typed degrees), `tool-scale` (Shift+S), `tool-offset` (F), `tool-followme`, `tool-tape` (T, creates Guides), `tool-eraser` (E), `tool-paint` (B, applies the selected catalogue product to faces), `make-component` (turns the selection into a component; editing one instance changes all), and `scenes` (save and restore camera views as Scene entities). Every tool uses the inference and `measurements` behaviour of T8.6, and every result equals the corresponding `bim sketch` command. Zero serious/critical axe-core violations; no horizontal overflow at 390 px or 1440 px.
