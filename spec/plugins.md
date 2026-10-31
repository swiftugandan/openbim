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
