# TASKS

The plan for the openbim kernel. `loop/run.sh` works the first `[ ]` task.
Marks: `[ ]` todo, `[x]` done, `[!]` blocked (with a `Blocked:` bullet).
`Opens:` names the tasks whose output must already exist; `none` means nothing.
Verifications are external judges in `judges/`, which builders may not edit.
Crates are std-only (no external dependencies), so judges build offline in seconds.

- [ ] T1.1: bim-doc: content-addressed semantic document graph with op log
  - Files: Cargo.toml, Cargo.lock, crates/bim-doc/
  - Opens: none
  - Verification: bash judges/T1.1.sh
  - Spec: Cargo workspace with `resolver = "2"` and members = ["crates/bim-*", "crates/engines/*"]. Crate `bim-doc` exports `EntityId(u128)` with `EntityId::derive(seed: &str)` (deterministic, distinct seeds give distinct ids), `Value` enum (`Num(f64)`, `Text(String)`, `Ref(EntityId)`), `Entity { id, kind: String, props: BTreeMap<String, Value> }`, `Op` enum (`Create { id: EntityId, kind: String }`, `Set { id: EntityId, key: String, value: Value }`, `Delete { id: EntityId }`), `Document::new()`, `Document::apply(&mut self, op: Op) -> Result<(), DocError>` (Set or Delete on a missing id is an error; Create of an existing id is an error), `get(id) -> Option<&Entity>`, `len()`, `content_hash() -> u64` (deterministic, independent of insertion order, changes on any prop change), `dangling_refs() -> Vec<(EntityId, String)>` (props whose `Ref` target does not exist), `log() -> &[Op]` (applied ops only, in order). All public types derive Debug, Clone, PartialEq; `EntityId` also Copy, Eq, Ord, Hash; `DocError` derives Debug.

- [ ] T1.2: bim-crdt: state-based CRDT replica with conflict diagnostics
  - Files: Cargo.lock, crates/bim-crdt/
  - Opens: T1.1
  - Verification: bash judges/T1.2.sh
  - Spec: Crate `bim-crdt` (depends on bim-doc). `Replica::new(peer: u32)`; local edits `create(&mut self, id: EntityId, kind: String)`, `set(&mut self, id: EntityId, key: String, value: Value)`, `delete(&mut self, id: EntityId)`, each stamped with a Lamport clock `(counter, peer)`; `merge(&mut self, other: &Replica)` is commutative, associative and idempotent (last-writer-wins per (entity, key) by stamp; delete is a tombstone that wins over a create or set with a lower stamp, and an entity re-created with a higher stamp revives). `document() -> bim_doc::Document` materializes the live state. `diagnostics() -> Vec<ConflictDiagnostic>` reports, without panicking, every live `Ref` whose target is tombstoned or missing: `ConflictDiagnostic { entity: EntityId, key: String, target: EntityId }` (derives Debug, Clone, PartialEq, Eq, PartialOrd, Ord), sorted. Local edits never fail. Liveness: an entity is live iff its greatest create stamp exceeds its greatest delete stamp; a prop is visible iff the entity is live and the prop's stamp exceeds the entity's greatest delete stamp. `Replica` derives Clone.

- [ ] T1.3: bim-solver: 2D geometric constraint solver with DOF analysis
  - Files: Cargo.lock, crates/bim-solver/
  - Opens: T1.1
  - Verification: bash judges/T1.3.sh
  - Spec: Crate `bim-solver`. `Sketch::new()`, `add_point(x, y) -> PointId`, `add(c: Constraint)`; `Constraint` variants: `Fix(PointId, f64, f64)`, `Coincident(PointId, PointId)`, `Distance(PointId, PointId, f64)`, `Horizontal(PointId, PointId)`, `Vertical(PointId, PointId)`, `Parallel(PointId, PointId, PointId, PointId)`, `Perpendicular(PointId, PointId, PointId, PointId)`, `Angle(PointId, PointId, PointId, PointId, f64)` (radians, line a→b to line c→d). `solve(&mut self) -> SolveReport` uses Newton or Levenberg–Marquardt with analytic Jacobians. `SolveReport { status: Status, residual: f64, dof: usize }` with `Status::{WellConstrained, UnderConstrained, OverConstrained, Inconsistent}`: dof = 2·points − rank(J); redundant-but-consistent equations give OverConstrained; unsatisfiable ones give Inconsistent. `point(id) -> (f64, f64)`. `PointId` and `Constraint` derive Debug, Clone, Copy; `Status` derives Debug, PartialEq. Angle is the signed counter-clockwise angle from direction a→b to direction c→d. `residual` is the Euclidean norm of the equation residual vector; rank uses a relative singular-value tolerance of 1e-9. Status priority: Inconsistent (residual > 1e-6 after solving), then UnderConstrained (dof > 0), then OverConstrained (more scalar equations than rank), else WellConstrained.

- [ ] T1.4: bim-struct: 2D frame direct-stiffness solver
  - Files: Cargo.lock, crates/engines/structural/
  - Opens: T1.1
  - Verification: bash judges/T1.4.sh
  - Spec: Crate `bim-struct` at crates/engines/structural (the T1.1 workspace glob already includes it). `Frame::new()`, `add_node(x, y) -> usize`, `add_member(i, j, e, a, iz) -> usize` (Euler–Bernoulli beam-column), `fix(node, ux: bool, uy: bool, rz: bool)`, `load(node, fx, fy, mz)`, `analyze() -> Result<Results, String>` (Err on a singular/unstable structure). `Results::displacement(node) -> (ux, uy, rz)`, `reaction(node) -> (fx, fy, mz)`, `member_end_forces(m) -> [f64; 6]` in local axes (N1, V1, M1, N2, V2, M2). Conventions: global x right, y up, rotation counter-clockwise positive; a reaction is the force the support exerts on the structure; member end forces are the forces the nodes exert on the member, in local axes (x from node i to node j).

- [ ] T1.5: bim-cli: headless `bim` command over the document
  - Files: Cargo.lock, crates/bim-cli/
  - Opens: T1.1
  - Verification: bash judges/T1.5.sh
  - Spec: Crate `bim-cli` producing binary `bim`. `bim init <dir>` creates `<dir>/doc.oplog` (empty). `bim apply <dir> <ops-file>` appends ops, one per line: `create <seed> <kind>`, `set <seed> <key> num <f64>|text <word>|ref <seed>`, `delete <seed>` (ids are `EntityId::derive(seed)`); an invalid op exits 2 and leaves the log unchanged. `bim hash <dir>` prints `content_hash` as 16 lowercase hex digits. `bim validate <dir>` exits 0 when there are no dangling refs, else prints one line per dangling ref and exits 1. `bim diff <dirA> <dirB>` prints `same` and exits 0 when hashes match, else `differ` and exits 1. Blank lines in an ops file are ignored; any other error (missing directory, unreadable file) prints a message to stderr and exits 2.
