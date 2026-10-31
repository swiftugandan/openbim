# `bim` command-line contract

Frozen contract. Every capability of the GUI is reachable here. All commands take a project
directory `<dir>`. `--json` makes a command print exactly one JSON document on stdout.

Exit codes: `0` success, `1` the command ran and found a problem (violations, clashes, differences,
budget missed), `2` invalid input or usage (nothing is changed), `3` internal error. Errors go to stderr.

## Project and ops

| Command | Contract |
|---|---|
| `bim init <dir>` | Creates `<dir>/doc.oplog` (text journal, see T1.5) and `<dir>/bim.sqlite` (from T3.2 on). |
| `bim apply <dir> <file>` | `*.json` → JSON ops; anything else → the line format of T1.5. Atomic: all ops or none (exit 2). Kinds and extension props are validated against core kinds and the packages in `<dir>/plugins/` (spec/schema.md). |
| `bim hash <dir>` | 16 lowercase hex digits: content hash of the live document. |
| `bim show <dir> <id> --json` | `{"id","kind","props":{…}}` with Values in JSON encoding (spec/schema.md). |
| `bim commit <dir> -m <msg>` | Records a revision; prints its id (16 hex digits = content hash at that moment). Only `bim commit` records revisions; `bim init` records none. |
| `bim log <dir> --json` | `[{"rev","message","ops"}]` oldest first; `ops` = ids (16 hex) of the ops recorded since the previous revision, in apply order. |
| `bim diff <dirA> <dirB>` | T1.5 behaviour (same/differ). |
| `bim plugin list <dir> --json` | `{"packages":[{"name","version","abi","kinds":[…],"engine"}],"core_kinds":[…],"trades":[…]}`: packages sorted by name, their kinds sorted, `engine` the component path relative to `<dir>` (`plugins/<package>/<engine>`) or null; `core_kinds` sorted; `trades` = `Registry::engine_names()` (added by T4.8). Consumers ignore keys they do not know. |
| `bim diff <dir> <revA> <revB> --json` | `{"added":[id],"removed":[id],"changed":[{"id","keys":[…]}]}`, ids sorted (reserved `_` keys may appear). Exit 0 if empty, else 1; unknown revision exits 2. |

JSON ops file:

```json
{"schema": 1, "ops": [
  {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 0}},
  {"op": "set", "id": "L1", "key": "elevation", "value": 3.0},
  {"op": "delete", "id": "L1"}
]}
```

## Engines and checks

| Command | Contract |
|---|---|
| `bim solve <dir> --trade arch --json` | `{"rooms":[{"id","level","area","volume","boundary":[[x,y],…]}],"walls":[{"id","volume","side_area","layers":[{"material","volume"}]}],"slabs":[{"id","volume","area"}],"stairs":[{"id","riser","tread","risers"}],"curtain_walls":[{"id","panels","mullion_length"}]}` — `rooms[].id` is `<level id>/<n>`, n from 1, numbered by ascending centroid x (centroids within 1e-6 in x ordered by y); `rooms[].space` is the id of the Space whose `x y` lies inside, or null. `walls[].side_area` = area of the wall solid's section by the vertical plane through its centreline; `walls[].layers` in WallType order (left first); `slabs[].area` = net plan area. With products or roofs in the model the output also has `products` and `roofs` (spec/catalogs.md, Take-off). |
| `bim solve <dir> --trade struct --json` | `{"nodes":[{"id","x","y","z","ux","uy","uz","rx","ry","rz"}],"members":[{"id","N":[min,max],"Vy":[…],"Vz":[…],"T":[…],"My":[…],"Mz":[…],"deflection":max_abs,"utilization":ratio}],"reactions":[{"node","fx","fy","fz","mx","my","mz","combination"}],"tributary":[{"member","area"}]}` — envelopes over all LoadCombinations. Node ids are `n<k>` in order of first appearance. A Column/Beam split into k > 1 analytical members is reported as `<id>#1`…`<id>#k` from its start ((x1,y1) / base); `tributary[].member` uses the same ids. Envelopes follow the T4.4 internal-action convention; `utilization` = max over stations and combinations of \|N\|/(A·fy)+\|My\|/(Wy·fy)+\|Mz\|/(Wz·fy); `reactions[]` lists supported nodes only, `combination` = its name; `nodes[]` displacements are for the first LoadCombination. |
| `bim solve <dir> --trade <package> --json` | A trade package's engine output (spec/plugins.md, Trade packages). |
| `bim solve <dir> --trade mep --json` | `{"segments":[{"id","flow","velocity","dp","reynolds","friction_factor","size"}],"ports":[{"id","pressure"}],"continuity_residual":r,"circuits":[{"id","current","voltage_drop","voltage_drop_pct"}]}` — `size` is the recommended nominal size: `DN<n>` (round) or `<w>x<h>` mm (rect). `flow` is signed, + from `from` to `to`; `dp` = p(from) − p(to) (friction + minor); `velocity` = \|flow\|/area; `ports[].pressure` is gauge static pressure (a source reports its fixed value); `continuity_residual` = max nodal imbalance over non-source ports; `voltage_drop_pct` = 100·drop/voltage. `--method newton|hardy-cross` (default newton). |
| `bim clash <dir> [--clearance <m>] --json` | `{"hard":[{"a","b","volume"}],"soft":[{"a","b","distance"}],"matrix":{"<KindA>":{"<KindB>":count}}}`; exit 1 if any hard clash. Joined walls and an opening's host are excluded from both lists; `matrix` counts hard clashes by unordered kind pair; soft = non-hard pairs closer than `--clearance`. |
| `bim validate <dir> [--standard <pack>] --json` | `{"violations":[{"rule","entity","message","measured","limit"}],"dangling":[…],"conflicts":[…]}`; exit 1 if any. Packs: `ibc`, `eurocode`, `accessibility`, a path to a pack file, a path to a `rule-set` component, or a trade package name. |

## Export, import, drawings

| Command | Contract |
|---|---|
| `bim export <dir> --format ifc --out f.ifc` | IFC4 (ISO 10303-21) readable by ifcopenshell; geometry as IfcExtrudedAreaSolid / IfcFacetedBrep. |
| `bim export <dir> --format ifcjson --out f.json` | ifcJSON 0.0.x object list. |
| `bim export <dir> --format gltf --out f.gltf` | glTF 2.0 (embedded buffers) passing Khronos glTF-Validator with zero errors; one closed, consistently oriented mesh node per element (open-surface Shapes excepted, spec/sketch.md) with `extras.id` = entity id; metres, glTF's y-up frame: glTF (x, y, z) = model (x, z, −y). |
| `bim export <dir> --format svg|pdf|dxf --view <view> --out f` | 2D drawing. `<view>`: `plan:<level>@<cut height>`, `section:<x0>,<y0>,<x1>,<y1>` (vertical cut plane through that line, looking left of the direction), `elevation:<n|s|e|w>`. |
| `bim sheet <dir> <sheet.json> --format svg|pdf|dxf --out f` | Multi-viewport sheet (T7.4). |
| `bim import <dir> <file.ifc|file.json>` | Adds the file's building elements as ops; reads IFC4 and IFC2X3 (exports are IFC4). Invalid or truncated input exits 2 with nothing applied. |
| `bim sketch <dir> <file.json>` | Direct-modelling commands, applied atomically (spec/sketch.md). |
| `bim catalog list <dir> --json` / `bim catalog use <dir> <catalog>:<sku>` | Product catalogues (spec/catalogs.md). |
| `bim bcf <dir> create --title <t> --entity <id>…` / `bim bcf <dir> export <f.bcfzip>` / `bim bcf <dir> import <f.bcfzip>` | BCF 3.0. |

Drawing layers (SVG `<g id>`, DXF layer names, PDF optional-content groups): `CUT` (0.50 mm),
`PROJECTION` (0.25 mm), `HIDDEN` (0.18 mm, ISO 128 dashed), `HATCH` (0.13 mm), `DIMENSION` (0.18 mm),
`ANNOTATION` (0.18 mm). Drawing units: millimetres on paper at the view's scale (default 1:100).
Every element carries `data-entity="<id>"` in SVG and XDATA `OPENBIM` id in DXF.
Dimensions (`Dimension` entities) render as text equal to the measured distance in mm, rounded to integer.

View geometry: `plan:<level>@<h>` cuts at level elevation + h; `elevation:s` looks north (+y) with the viewer's right along +x
(n, e, w analogous); in a section the viewer's right is the line direction and the plane is unbounded. An unknown or
unparseable `--view` exits 2. In a plan or section, PROJECTION/HIDDEN geometry shows only the model on the far side of the
cut plane; what the cut removes is not drawn. SVG user units are paper mm, y down, model up is up on paper, scale 1:100, origin free,
coordinates to ≥ 0.01 mm; CUT outlines are closed rings (holes are extra rings) with HATCH inside; HIDDEN strokes carry a
`stroke-dasharray`; stroke width and `data-entity` may be inherited from an enclosing (non-layer) group. A Dimension
referencing two Points measures their 3D distance and is drawn in the view equal to its `view` prop as a `<text>` on
DIMENSION whose content is the integer mm value. DXF/PDF coordinates equal the SVG's with y up, up to translation; PDF
units are points (1 mm = 72/25.4 pt), one page, each stroke inside `/OC` marked content of its layer's OCG.

## Sync, bench, servers

| Command | Contract |
|---|---|
| `bim sync serve <dir> --port <p>` | WebSocket server (port 0 → prints `listening <port>` on stdout first). |
| `bim sync <dir> <ws-url>` | Exchanges state with the server until both sides converge, then exits 0. |
| `bim bench --elements <n> --json` | Generates the synthetic stress corpus (T7.5) and prints `{"elements","triangles","build_ms","incremental_ms","frame_ms_p50","frame_ms_p95","raycast_us"}`; exit 1 if `incremental_ms ≥ 50` or `frame_ms_p95 ≥ 16.6`. |
| `bim-mcp <dir>` | MCP server over stdio (spec/mcp.md). |
