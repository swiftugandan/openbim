# Canonical document schema (schema version 1)

Frozen contract. Builders read it; only a human edits it. Judges test against it.

## Units and frames

- SI throughout: metres, newtons, pascals, kelvin, seconds, watts, m³/s. Angles in radians.
- Right-handed global frame, z up. Plan views look down −z. Levels are horizontal datums at `elevation`.

## Identity and provenance

- Every entity has a 128-bit `EntityId`. In JSON an id is a string: either a 26-character
  Crockford-base32 ULID (used as-is) or any other string, a *seed*, mapped by `EntityId::derive(seed)`.
- JSON outputs always report an entity by the string it was created with (seed or ULID), stored in
  the reserved prop `_name` at create time.
- Reserved props (set by the kernel, never by users): `_name`, `_schema` (schema version, Num),
  `_created_by` (peer, Num), `_stamp` (Lamport counter of the last change, Num). Op files, the CLI
  and MCP reject creates/sets of keys starting with `_` (exit 2); in-process Rust `Op`s may set them.
- `content_hash` covers kinds and props except the provenance props `_created_by` and `_stamp`.

## Values

`Num(f64)`, `Text(String)`, `Ref(EntityId)`, `Bool(bool)`, `List(Vec<Value>)`.
JSON encoding: number → Num, string → Text, `true/false` → Bool, array → List, `{"ref": "<id>"}` → Ref.

## Entity kinds

Props marked † are optional. A `Ref` prop is a parametric dependency edge (the dependency graph).

| Kind | Props |
|---|---|
| `Level` | `elevation` |
| `WallType` | `layer_thickness` List[Num], `layer_material` List[Text], `layer_conductivity` List[Num] (W/m·K) |
| `Wall` | `level` Ref Level, `x1 y1 x2 y2` centreline, `height`, `type` Ref WallType, `base_offset`† (0) |
| `Opening` | `host` Ref Wall, `kind` Text (`door`/`window`), `offset` (along centreline from (x1,y1) to the opening's near edge), `sill`, `width`, `height` |
| `Slab` | `level` Ref Level, `boundary` List[Num] (x0,y0,x1,y1,… counter-clockwise), `thickness` (slab top at level elevation) |
| `SlabOpening` | `host` Ref Slab, `boundary` List[Num] |
| `CurtainWall` | `level`, `x1 y1 x2 y2`, `height`, `u_spacing`, `v_spacing`, `mullion_width`, `mullion_depth`, `panel_thickness` |
| `Stair` | `base` Ref Level, `top` Ref Level, `x y` start of the run, `angle` run direction, `width`, `riser_count`, `tread` |
| `Ramp` | `base`, `top`, `x y`, `angle`, `width`, `length` |
| `Space` | `level` Ref Level, `x y` (a point inside the room it labels), `name` Text, `use` Text† (e.g. `corridor`, `office`) — authored label; geometry is derived |
| `Point` | `x y`, `z`† (0) — a sketch point that `Constraint`s act on |
| `Material` | `name`, `E` (Pa), `G` (Pa), `density`, `fy` (Pa) |
| `Column` | `x y`, `base` Ref Level, `top` Ref Level, `section_b`, `section_h`, `material` Ref Material |
| `Beam` | `level` Ref Level, `x1 y1 x2 y2`, `section_b`, `section_h`, `material`, `z_offset`† (0, below level) |
| `Support` | `x y z`, `ux uy uz rx ry rz` Bool (true = restrained) |
| `PointLoad` | `x y z`, `fx fy fz mx my mz`, `case` Text |
| `AreaLoad` | `host` Ref Slab, `pressure` (Pa, acting −z), `case` Text |
| `LoadCombination` | `name`, `cases` List[Text], `factors` List[Num] |
| `MepSystem` | `name`, `medium` Text (`water`/`air`), `density`, `viscosity` (Pa·s) |
| `Port` | `system` Ref MepSystem, `x y z`, `kind` Text (`source`/`sink`/`junction`), `flow`† (m³/s; + for sinks' demand), `pressure`† (Pa; fixed head at a source) |
| `Segment` | `from` Ref Port, `to` Ref Port, `shape` Text (`round`/`rect`), `diameter`† or `width`†+`height`†, `roughness` (m), `minor_k`† (0) |
| `Circuit` | `panel` Ref Port, `voltage`, `loads` List[Ref Port], `load_watts` List[Num], `conductor_area` (m²), `resistivity` (Ω·m), `length` |
| `Constraint` | `kind` Text (`coincident`/`parallel`/`perpendicular`/`distance`/`angle`/`datum`), `a` Ref, `b` Ref†, `value`† — between entity endpoints/datums |
| `Dimension` | `a` Ref, `b` Ref, `view` Text — associative; its measured value is derived, never stored |
| `Issue` | `title`, `status`, `entities` List[Ref] — mirrors a BCF topic |
| `ConflictDiagnostic` | `entity` Ref, `key` Text, `target` Ref†, `reason` Text — **materialized by the kernel**, never authored |

## Derived geometry rules (shared by all engines and judges)

- Wall solid: centreline offset ±thickness/2 (thickness = sum of `layer_thickness`), extruded from
  `level.elevation + base_offset` by `height`. Layers ordered from the left side looking from (x1,y1) to (x2,y2).
- Wall joins: walls whose endpoints coincide (within 1e-6) are mitred at L-joins; at a T-join the
  abutting wall stops at the face of the through wall. Joined solids never overlap.
- Openings are boolean-subtracted from their host wall's full thickness.
- Slab solid: `boundary` extruded down by `thickness` from the level elevation, minus SlabOpenings.
- Column: `section_b × section_h` rectangle centred on (x,y), from base to top elevation.
- Beam: rectangle `section_b` wide × `section_h` deep, top at level elevation − `z_offset`.
- Rooms: the bounded faces of the planar arrangement of wall centrelines on a level; room area is the
  area inside the walls' inner faces; room volume = area × (next level elevation − level elevation − slab thickness above, if any).
  The inner face of a room edge is that edge's centreline offset by half the total thickness of the wall carrying it;
  consecutive offset lines meet at mitred corners. The next level is the Level with the smallest elevation above;
  the slab thickness above is that of a Slab on the next level whose boundary contains the room centroid (0 if none).
- A T-join is a wall endpoint within 1e-6 of the interior of another wall's centreline; a T-join partner counts as joined.
  At an L-join of unequal thickness the mitre plane passes through the intersections of the outer faces and of the inner faces.
- An opening's `sill` is measured from its host wall's base. Column `section_b` runs along global x, `section_h` along y;
  a beam's `section_b` is centred on its axis.
- MEP segment solids run from the `from` port to the `to` port: round = 16-gon prism with every vertex at radius D/2;
  rect = `width` horizontal and perpendicular to the axis, `height` vertical.
