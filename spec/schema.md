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
| `Wall` | `level` Ref Level, `x1 y1 x2 y2` centreline, `height`, `type` Ref WallType, `base_offset`† (0), `finish_left`† `finish_right`† Ref Product |
| `Opening` | `host` Ref Wall, `kind` Text (`door`/`window`), `offset` (along centreline from (x1,y1) to the opening's near edge), `sill`, `width`, `height`, `product`† Ref Product |
| `Slab` | `level` Ref Level, `boundary` List[Num] (x0,y0,x1,y1,… counter-clockwise), `thickness` (slab top at level elevation), `finish_top`† `finish_bottom`† Ref Product |
| `SlabOpening` | `host` Ref Slab, `boundary` List[Num] |
| `CurtainWall` | `level`, `x1 y1 x2 y2`, `height`, `u_spacing`, `v_spacing`, `mullion_width`, `mullion_depth`, `panel_thickness` |
| `Stair` | `base` Ref Level, `top` Ref Level, `x y` start of the run, `angle` run direction, `width`, `riser_count`, `tread` |
| `Ramp` | `base`, `top`, `x y`, `angle`, `width`, `length` |
| `Roof` | `level` Ref Level, `boundary` List[Num] (counter-clockwise footprint), `slope` (rise per run, ≥ 0), `angle` (plan direction of rise), `thickness` (vertical), `base`† (0, height of the underside at the first boundary vertex above the level), `covering`† Ref Product |
| `Furniture` | `level` Ref Level, `x y` centre, `angle` (plan rotation of the product's width axis from +x), `product` Ref Product, `z_offset`† (0), `size`† List[Num] |
| `Product` | `catalog`, `catalog_version`, `sku`, `category`, `name`, `manufacturer` (Text) and the catalogue product's own props — spec/catalogs.md |
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
| `Shape`, `ComponentDefinition`, `ComponentInstance`, `Guide`, `Scene` | Direct modelling: spec/sketch.md |
| `ConflictDiagnostic` | `entity` Ref, `key` Text, `target` Ref†, `reason` Text — **materialized by the kernel**, never authored |

## Extension kinds (trade packages)

Kinds outside the table above come from trade packages (spec/plugins.md), so new trades need no change to
this schema. An extension kind is named `<package>:<Kind>` (package `[a-z][a-z0-9-]*`, Kind `[A-Z][A-Za-z0-9]*`)
and is declared in the package's `trade.json`:

```json
{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0",
 "kinds": {
   "fire:Sprinkler": {"props": {"level": "ref", "x": "num", "y": "num", "z": "num", "coverage_radius": "num"},
                      "geometry": {"box": {"centre": ["x", "y", "z"], "size": [0.1, 0.1, 0.1]}}},
   "fire:Main": {"props": {"x1": "num", "y1": "num", "z1": "num", "x2": "num", "y2": "num", "z2": "num", "diameter": "num"},
                 "geometry": {"segment": {"from": ["x1", "y1", "z1"], "to": ["x2", "y2", "z2"], "diameter": "diameter"}}}
 },
 "engine": "fire.wasm"}
```

- Prop types: `num`, `text`, `bool`, `ref`, `list` (the Values above). A `trade.json` with an unknown prop type, an
  unknown geometry template, or a geometry entry that is neither a prop name nor a number is malformed. Every declared prop is optional; a present prop
  must have its declared type, and an extension entity may carry only declared props (plus reserved `_` props).
- `geometry` is optional (no solid when absent) and is one of a closed set of templates. Each coordinate or size entry
  is a prop name or a number literal; a solid exists only when every named prop is present.
- Extension entities behave like core entities everywhere that is generic: ids, hashing, CRDT merge, diff, dangling
  refs, `bim show`, Datalog `entity/prop/ref`, clash detection and rendering (through their solid), and IFC export
  (as `IfcBuildingElementProxy`, T5.8).
- Op files, the CLI and MCP reject (exit 2 / tool error) a create whose kind is neither a core kind nor declared by a
  loaded package, and any create or set that breaks an extension kind's prop declarations. Core kinds keep their
  current behaviour (their props are not type-checked).

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
- Roof: underside z(x, y) = level elevation + `base` + `slope` · ((x, y) − (x0, y0)) · (cos `angle`, sin `angle`), where
  (x0, y0) is the first boundary vertex; the solid spans the footprint from the underside up by `thickness` (vertical).
  Volume = plan area × `thickness`; sloped (top) area = plan area × √(1 + `slope`²). Gable and hipped roofs are several Roofs.
- Furniture: a cuboid of effective `size` [width, depth, height] centred on (x, y) in plan, width along `angle`, base at
  level elevation + `z_offset`.
- Effective props: an element with a `product` takes each prop it lacks from that Product (spec/catalogs.md).
- Extension `box` template: an axis-aligned cuboid centred at `centre` (absolute x, y, z in metres) with edge lengths
  `size` along x, y, z. Extension `segment` template: meshed exactly as a round MEP segment from `from` to `to` with
  diameter `diameter`.
