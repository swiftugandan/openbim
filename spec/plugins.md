# Plugin boundary and standards packs

Frozen contract.

## Rust traits (`crates/bim-plugin`)

```rust
pub trait TradeEngine: Send + Sync {
    fn name(&self) -> &str;                       // "arch" | "struct" | "mep" | …
    fn solve(&self, ctx: &EngineContext) -> Result<serde_json::Value, EngineError>;
}
pub trait StandardRuleSet: Send + Sync {
    fn id(&self) -> &str;                         // "ibc" | "eurocode" | …
    fn check(&self, ctx: &EngineContext) -> Vec<Violation>;
}
pub struct Violation { pub rule: String, pub entity: String, pub message: String, pub measured: f64, pub limit: f64 }
```

Added by T4.8, so generic surfaces (MCP, Studio) can report any trade without knowing its name:

```rust
pub struct Metric { pub key: String, pub value: f64 }
// on TradeEngine, with a default that returns no metrics:
fn metrics(&self, result: &serde_json::Value) -> Vec<Metric> { Vec::new() }
// on Registry: every registered engine name, sorted
fn engine_names(&self) -> Vec<String>;
```

Built-in metrics: `arch` → `total_area` = Σ `rooms[].area`; `struct` → `max_utilization` = max `members[].utilization`;
`mep` → `max_dp` = max |`segments[].dp`|; each is 0 when its list is empty.

`EngineContext` gives read-only access to the document and to memoized derived data (solids,
rooms, analytical model). Engines are pure projections: they never mutate the document.
A `Registry` maps names to engines and rule sets; the CLI and MCP server resolve through it only.

## WASM Component Model plugins

Rule sets may also be WebAssembly components (target `wasm32-wasip2`) implementing this WIT world,
loaded at run time by the host (wasmtime) with no filesystem or network capabilities:

```wit
package openbim:plugin@0.1.0;

interface types {
  record entity { id: string, kind: string, props-json: string }
  record violation { rule: string, entity: string, message: string, measured: f64, limit: f64 }
}

world rule-set {
  use types.{entity, violation};
  export id: func() -> string;
  export check: func(entities: list<entity>) -> list<violation>;
}
```

`bim validate <dir> --standard path/to/plugin.wasm` runs such a component: the host calls `check` once with one
`entity` per live entity (`id` = `_name`, `props-json` = its props in the spec/schema.md JSON encoding, Refs as
`{"ref": "<target _name>"}`), reports the returned violations unchanged and exits 1 if any. The host links WASI 0.2
with no preopened directories, no inherited environment and no network, caps linear memory at 512 MiB and meters fuel
so a non-terminating `check` stops within 10 s; a trap, fuel exhaustion or memory failure exits 3 with a message,
never a host crash. An unknown `--trade`, a `--standard` that is neither a pack id nor an existing file, and a file
that is not a valid `rule-set` component exit 2.

## Trade packages

A trade package adds a trade without rebuilding openbim. It is a directory `<dir>/plugins/<package>/` holding
`trade.json` (spec/schema.md, Extension kinds) and, optionally, the WebAssembly component named by `engine`.
Every command that reads `<dir>` loads every package; the directory name must equal `package`, `package` must not be
a built-in trade (`arch`, `struct`, `mep`) or a shipped pack id (`ibc`, `eurocode`, `accessibility`), every kind must use the package's namespace, and `abi` must be
`openbim:trade@0.1.0`. A package that breaks any of these, or malformed `trade.json`, makes the command exit 2 naming
the file. A missing or invalid `engine` file exits 2 only when that engine is invoked.

Only packages that declare `engine` appear in `Registry::engine_names()` and `trades`. A package with an `engine` registers a trade engine under its package name and also acts as a rule set under the same
name. The component implements this world and runs under the same host rules as `rule-set` components (no filesystem,
network or environment, 512 MiB memory, fuel stops a non-terminating call within 10 s, trap → exit 3):

```wit
package openbim:trade@0.1.0;

interface types {
  record entity { id: string, kind: string, props-json: string }
  record room { id: string, level: string, area: f64, boundary: list<f64> }
  record solid { id: string, volume: f64, min: list<f64>, max: list<f64> }
  record metric { key: string, value: f64 }
  record violation { rule: string, entity: string, message: string, measured: f64, limit: f64 }
  record output { json: string, metrics: list<metric>, violations: list<violation> }
}

world trade-engine {
  use types.{entity, room, solid, output};
  export name: func() -> string;
  export solve: func(entities: list<entity>, rooms: list<room>, solids: list<solid>) -> result<output, string>;
}
```

The host calls `solve` once per command with one `entity` per live entity (as for `rule-set`), the rooms of
`bim solve --trade arch` (`boundary` flattened x0, y0, x1, y1, …) and one `solid` per entity that has a derived solid
(`min`/`max` its axis-aligned bounds as [x, y, z]), each list sorted by its `id` string (byte-wise). `name()` must return the package name
(else exit 2; it is checked only when the engine is invoked, so a mismatch does not affect other commands). `bim solve <dir> --trade <package> --json` prints `output.json` (exit 3 if it is not valid JSON);
`bim validate <dir> --standard <package> --json` reports `output.violations` unchanged; `metrics` feed the
`metrics` method above. `err(message)` exits 1 with the message on stderr.

## Declarative packs (`crates/standards/codes/packs/*.json`, or any path)

```json
{"pack": "ibc", "version": "2021", "rules": [
  {"id": "IBC-1020.2", "applies_to": "Space", "where": {"use": "corridor"}, "check": "min_width", "limit": 1.118},
  {"id": "IBC-1011.5.2-riser", "applies_to": "Stair", "check": "riser_max", "limit": 0.1778}
]}
```

Packs are data, reloaded on every run (hot-swappable): editing a pack file changes the next result
with no rebuild. `where` matches when every listed prop is equal; `Violation.entity` is the entity's `_name`; for
`blondel` the reported `limit` is the bound crossed. Shipped packs contain exactly the rules listed below. An unknown
pack, malformed pack JSON or an unknown `check` exits 2. Check vocabulary (closed set):

| `check` | Measured quantity | Violation when |
|---|---|---|
| `min_width` | Space: shorter side of the room polygon's minimum-area bounding rectangle; Opening: `width` | measured < limit |
| `min_area` | Space: room area | measured < limit |
| `min_clear_height` | Space: floor-to-underside of slab above | measured < limit |
| `riser_max` / `riser_min` | Stair: (top − base elevation) / riser_count | > / < limit |
| `tread_min` | Stair: `tread` | < limit |
| `blondel` | Stair: 2·riser + tread; `limit` is `[lo, hi]` | outside [lo, hi] |
| `u_value_max` | Wall: U = 1 / (0.13 + Σ tᵢ/λᵢ + 0.04) W/m²K | > limit |
| `slope_max` | Ramp: (top − base elevation) / `length` | > limit |
| `egress_distance_max` | Space: longest shortest path inside the room polygon from any vertex to the nearest door midpoint (door Openings on the room's bounding walls; midpoint = point at `offset + width/2` on the host centreline, projected onto the room boundary) | > limit |

Shipped packs: `ibc` (corridor 1.118 m, door clear 0.813 m, riser ≤ 0.1778, tread ≥ 0.2794),
`eurocode` (blondel [0.60, 0.65], U-wall ≤ 0.35), `accessibility` (door ≥ 0.85 m, corridor ≥ 1.5 m,
ramp slope ≤ 1:12 via `slope_max` on Ramp: (top − base)/length).
