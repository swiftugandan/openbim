# `bim-mcp` contract

Frozen contract. `bim-mcp <dir>` speaks the Model Context Protocol (protocol version `2025-06-18`)
over stdio, JSON-RPC 2.0, one message per line. It must interoperate with the official
`@modelcontextprotocol/sdk` client (`initialize`, `tools/list`, `tools/call`, `resources/list`,
`resources/read`). Every tool declares a JSON Schema `inputSchema` and returns `structuredContent`
(plus a text rendering) matching the shapes below. Tool errors use `isError: true`, never a crash. Each tool's `inputSchema` marks every input field shown
without `?` as `required` and types `clearance` as a number.

## Tools

| Tool | Input | Output (`structuredContent`) |
|---|---|---|
| `query` | `{"program": "<datalog>", "query": "<atom>"}` | `{"rows": [[…], …]}` one value per query-atom argument in order (ids as their `_name`), sorted, deduplicated; a parse error or unstratifiable program is a tool error |
| `get_entity` | `{"id"}` | as `bim show --json` |
| `propose_transaction` | `{"ops": [JSON ops]}` | `{"tx", "valid", "solver": {"status", "dof"}, "structural": {"max_utilization_before", "max_utilization_after"}, "mep": {"max_dp_before", "max_dp_after"}, "clashes": {"before", "after"}, "violations": {"before", "after", "new": [violation]}, "conflicts": [diagnostic]}` — the document is not changed |
| `commit_transaction` | `{"tx"}` | `{"hash"}`; error if the document changed since the proposal (stale) or tx unknown |
| `render_view` | `{"view", "format": "svg"}` | `{"svg": "<svg…>"}` same as `bim export --format svg` |
| `clash_matrix` | `{"clearance"?}` | as `bim clash --json` |
| `structural_utilization` | `{}` | `{"members": [{"id", "utilization"}]}` sorted by utilization descending |

## Resources

`bim://doc/hash` (text), `bim://doc/entities` (JSON list of `{"id","kind"}`),
`bim://view/{view}.svg` (image/svg+xml; `{view}` is the literal view string, e.g. `bim://view/plan:L1@1.2.svg`, listed in
`resources/list` or `resources/templates/list`), `bim://clash/matrix` (same JSON as `bim clash --json`).
Malformed tool arguments (missing or wrongly typed fields, unknown ids or views) return `isError: true`, never a
JSON-RPC error. `bim-mcp` re-reads `<dir>` on every call; commits persist to `<dir>`.

## Query language (Datalog)

Programs are rules `head(X, Y) :- body1(X, Z), body2(Z, Y).`; facts `name("a", 1).`; stratified
negation `not p(X)`; variables start upper-case; constants are double-quoted strings or numbers.
Evaluation is semi-naive bottom-up to a fixpoint. Built-in (extensional) relations:

| Relation | Meaning |
|---|---|
| `entity(Id, Kind)` | every live entity |
| `prop(Id, Key, Value)` | every Num/Text/Bool prop |
| `ref(Id, Key, Target)` | every Ref prop (and each Ref in a List, same key) |
| `room(Room, Level)` | rooms derived by the arch engine (`<level>/<n>` ids) |
| `adjacent(RoomA, RoomB)` | rooms sharing a wall segment, symmetric |
| `bounds(Wall, Room)` | wall bounds room |
| `bears_on(Upper, Lower)` | load path: slab→beam, beam→column, column→column below, column→support |
| `feeds(A, B)` | MEP directed flow from port A to port B through one segment (from solved flow sign) |
