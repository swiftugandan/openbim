// External judge for T6.2: drive `bim-mcp` with the official MCP TypeScript SDK client over stdio
// and compare every tool/resource against the CLI contract it is defined by (spec/mcp.md).
// Usage: node judge.mjs <bim> <bim-mcp> <project-dir> <tmp-dir>
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { oracle } from "../lib/oracle.mjs";

const [bim, bimMcp, dir, tmp] = process.argv.slice(2);
const fail = (msg) => { console.error(`JUDGE FAIL: ${msg}`); process.exit(1); };
setTimeout(() => fail("timed out after 50 s"), 50_000).unref();

const { Client } = await oracle("@modelcontextprotocol/sdk/client/index.js");
const { StdioClientTransport } = await oracle("@modelcontextprotocol/sdk/client/stdio.js");
const AjvMod = await oracle("ajv");
const Ajv2020Mod = await oracle("ajv/dist/2020.js");
const Ajv = AjvMod.Ajv ?? AjvMod.default;
const Ajv2020 = Ajv2020Mod.Ajv2020 ?? Ajv2020Mod.default;

// ---------- helpers ----------
function cli(args, okCodes = [0]) {
  const r = spawnSync(bim, args, { encoding: "utf8", timeout: 30_000 });
  if (!okCodes.includes(r.status)) fail(`bim ${args.join(" ")} exited ${r.status}: ${r.stderr}`);
  return r.stdout;
}
const cliJson = (args, ok) => {
  const out = cli(args, ok);
  try { return JSON.parse(out); } catch { fail(`bim ${args.join(" ")} did not print JSON: ${out.slice(0, 200)}`); }
};
function close(a, b, where, rel = 1e-9) {
  if (typeof a === "number" && typeof b === "number") {
    if (Math.abs(a - b) <= rel * Math.max(1, Math.abs(a), Math.abs(b))) return;
    fail(`${where}: ${a} != ${b}`);
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) fail(`${where}: length ${a.length} != ${b.length}`);
    a.forEach((x, i) => close(x, b[i], `${where}[${i}]`, rel));
    return;
  }
  if (a && b && typeof a === "object" && typeof b === "object") {
    const ka = Object.keys(a).sort(), kb = Object.keys(b).sort();
    if (JSON.stringify(ka) !== JSON.stringify(kb)) fail(`${where}: keys ${ka} != ${kb}`);
    for (const k of ka) close(a[k], b[k], `${where}.${k}`, rel);
    return;
  }
  if (a !== b) fail(`${where}: ${JSON.stringify(a)} != ${JSON.stringify(b)}`);
}
const textOf = (res) => (res.content ?? []).filter((c) => c.type === "text").map((c) => c.text);

// ---------- connect ----------
const transport = new StdioClientTransport({ command: bimMcp, args: [dir], stderr: "pipe" });
let stderr = "";
transport.stderr?.on("data", (d) => { stderr += d; });
const client = new Client({ name: "openbim-judge", version: "1.0.0" });
const T = { timeout: 30_000 };
try {
  await client.connect(transport, T);
} catch (e) { fail(`initialize failed: ${e.message}; server stderr: ${stderr}`); }

const alive = () => {
  try { process.kill(transport.pid, 0); return true; } catch { return false; }
};

async function call(name, args, expectError = false) {
  let res;
  try { res = await client.callTool({ name, arguments: args }, undefined, T); }
  catch (e) {
    fail(`tools/call ${name} ${JSON.stringify(args)} raised a protocol error instead of a result` +
      `${expectError ? " with isError: true" : ""}: ${e.message}; stderr: ${stderr}`);
  }
  if (expectError) {
    if (res.isError !== true) fail(`tools/call ${name} ${JSON.stringify(args)} should return isError: true`);
    if (!alive()) fail(`server died after malformed ${name} call; stderr: ${stderr}`);
    return res;
  }
  if (res.isError) fail(`tools/call ${name} ${JSON.stringify(args)} returned isError: ${textOf(res).join(" ")}`);
  if (!res.structuredContent || typeof res.structuredContent !== "object")
    fail(`tools/call ${name} returned no structuredContent`);
  if (textOf(res).length === 0) fail(`tools/call ${name} returned no text rendering in content`);
  return res.structuredContent;
}

try {
  // ---------- initialize ----------
  const caps = client.getServerCapabilities() ?? {};
  if (!caps.tools) fail("server capabilities lack `tools`");
  if (!caps.resources) fail("server capabilities lack `resources`");

  // ---------- tools/list ----------
  const { tools } = await client.listTools(undefined, T);
  const byName = Object.fromEntries(tools.map((t) => [t.name, t]));
  const required = {
    query: { valid: { program: "p(\"a\").", query: "p(X)" }, missing: { program: "p(\"a\")." } },
    get_entity: { valid: { id: "W1" }, missing: {} },
    render_view: { valid: { view: "plan:L1@1.2", format: "svg" }, missing: { format: "svg" } },
    clash_matrix: { valid: {}, badType: { clearance: "abc" } },
    structural_utilization: { valid: {} },
  };
  for (const [name, samples] of Object.entries(required)) {
    const t = byName[name];
    if (!t) fail(`tools/list lacks tool ${name}`);
    const s = t.inputSchema;
    if (!s || s.type !== "object") fail(`${name}.inputSchema is not an object schema`);
    const ajv = String(s.$schema ?? "").includes("2020-12") ? new Ajv2020({ strict: false }) : new Ajv({ strict: false });
    let validate;
    try { validate = ajv.compile(s); } catch (e) { fail(`${name}.inputSchema is not valid JSON Schema: ${e.message}`); }
    if (!validate(samples.valid)) fail(`${name}.inputSchema rejects a valid input ${JSON.stringify(samples.valid)}`);
    if (samples.missing && validate(samples.missing))
      fail(`${name}.inputSchema accepts input missing a required field: ${JSON.stringify(samples.missing)}`);
    if (samples.badType && validate(samples.badType))
      fail(`${name}.inputSchema accepts ${JSON.stringify(samples.badType)}`);
  }

  // ---------- fixture sanity (non-vacuous oracles) ----------
  const ids = JSON.parse(fs.readFileSync(path.join(path.dirname(new URL(import.meta.url).pathname), "fixture.json"), "utf8"))
    .ops.map((o) => ({ id: o.id, kind: o.kind }));
  const struct = cliJson(["solve", dir, "--trade", "struct", "--json"]);
  if (!struct.members?.some((m) => m.utilization > 0)) fail("fixture: CLI struct solve has no member with utilization > 0");
  const clashCli = cliJson(["clash", dir, "--json"], [0, 1]);
  if (!(clashCli.hard?.length >= 1)) fail("fixture: CLI clash finds no hard clash (pipe G1 crosses beams B3/B4)");

  // ---------- query ----------
  const progFile = path.join(tmp, "q.dl");
  const graph = [["a", "b"], ["b", "c"], ["c", "d"], ["x", "y"]];
  const program = graph.map(([a, b]) => `edge("${a}", "${b}").`).join("\n") + `
path(X, Y) :- edge(X, Y).
path(X, Y) :- path(X, Z), edge(Z, Y).
node(X) :- edge(X, Y).
node(Y) :- edge(X, Y).
unreach(X, Y) :- node(X), node(Y), not path(X, Y).
wall(W) :- entity(W, "Wall").
height(W, H) :- wall(W), prop(W, "height", H).
hosted(D, W) :- ref(D, "host", W).
rooms(R, L) :- room(R, L).
bounding(W, R) :- bounds(W, R).
flows(A, B) :- feeds(A, B).
`;
  fs.writeFileSync(progFile, program);
  // Closed-form expectations: transitive closure of `graph` by Floyd–Warshall style iteration.
  const nodes = [...new Set(graph.flat())].sort();
  const reach = new Set(graph.map((e) => e.join("\u0000")));
  for (let changed = true; changed;) {
    changed = false;
    for (const ab of [...reach]) for (const bc of [...reach]) {
      const [a, b] = ab.split("\u0000"), [b2, c] = bc.split("\u0000");
      if (b === b2 && !reach.has(a + "\u0000" + c)) { reach.add(a + "\u0000" + c); changed = true; }
    }
  }
  const pathRows = [...reach].map((s) => s.split("\u0000")).sort((p, q) => (p[0] + "\u0000" + p[1] < q[0] + "\u0000" + q[1] ? -1 : 1));
  const unreachRows = [];
  for (const a of nodes) for (const b of nodes) if (!reach.has(a + "\u0000" + b)) unreachRows.push([a, b]);
  const walls = ["W1", "W2", "W3", "W4"];
  const expected = {
    "path(X, Y)": pathRows,
    "unreach(X, Y)": unreachRows,
    "wall(W)": walls.map((w) => [w]),
    "height(W, H)": walls.map((w) => [w, 2.8]),
    "hosted(D, W)": [["D1", "W1"]],
    "rooms(R, L)": [["L1/1", "L1"]],
    "bounding(W, R)": walls.map((w) => [w, "L1/1"]),
    "flows(A, B)": [["P1", "P2"]],
  };
  for (const [atom, rows] of Object.entries(expected)) {
    const got = await call("query", { program, query: atom });
    if (!Array.isArray(got.rows)) fail(`query ${atom}: structuredContent.rows missing`);
    close(got.rows, rows, `query ${atom} rows`);
    const viaCli = cliJson(["query", dir, progFile, atom, "--json"]);
    close(got, viaCli, `query ${atom} vs bim query --json`);
  }

  // ---------- get_entity ----------
  for (const { id } of ids) {
    const got = await call("get_entity", { id });
    close(got, cliJson(["show", dir, id, "--json"]), `get_entity ${id} vs bim show --json`);
  }

  // ---------- render_view ----------
  for (const view of ["plan:L1@1.2", "section:-1,2,7,2", "elevation:s"]) {
    const got = await call("render_view", { view, format: "svg" });
    const out = path.join(tmp, "v.svg");
    cli(["export", dir, "--format", "svg", "--view", view, "--out", out]);
    const want = fs.readFileSync(out, "utf8");
    if (typeof got.svg !== "string" || !got.svg.includes("<svg")) fail(`render_view ${view}: no svg string`);
    if (got.svg.trim() !== want.trim()) fail(`render_view ${view}: svg differs from bim export --format svg`);
  }

  // ---------- clash_matrix ----------
  close(await call("clash_matrix", {}), clashCli, "clash_matrix {} vs bim clash --json");
  const clash05 = cliJson(["clash", dir, "--clearance", "0.5", "--json"], [0, 1]);
  if (!(clash05.soft?.length >= 1)) fail("fixture: clearance 0.5 should give at least one soft clash");
  close(await call("clash_matrix", { clearance: 0.5 }), clash05, "clash_matrix {clearance:0.5} vs bim clash --clearance 0.5 --json");

  // ---------- structural_utilization ----------
  const util = await call("structural_utilization", {});
  if (!Array.isArray(util.members)) fail("structural_utilization: members missing");
  for (let i = 1; i < util.members.length; i++)
    if (util.members[i - 1].utilization < util.members[i].utilization) fail("structural_utilization: not sorted by utilization descending");
  const wantUtil = struct.members.map((m) => ({ id: m.id, utilization: m.utilization }))
    .sort((a, b) => b.utilization - a.utilization || (a.id < b.id ? -1 : 1));
  const gotUtil = [...util.members].sort((a, b) => b.utilization - a.utilization || (a.id < b.id ? -1 : 1));
  close(gotUtil, wantUtil, "structural_utilization vs bim solve --trade struct members");

  // ---------- resources ----------
  const { resources } = await client.listResources(undefined, T);
  let templates = [];
  try { templates = (await client.listResourceTemplates(undefined, T)).resourceTemplates ?? []; } catch { /* optional */ }
  const listed = new Set([...resources.map((r) => r.uri), ...templates.map((t) => t.uriTemplate)]);
  for (const uri of ["bim://doc/hash", "bim://doc/entities", "bim://clash/matrix"])
    if (!listed.has(uri)) fail(`resources/list lacks ${uri}`);
  if (![...listed].some((u) => u.startsWith("bim://view/"))) fail("neither resources/list nor resources/templates/list offers bim://view/{view}.svg");

  const read = async (uri) => {
    let r;
    try { r = await client.readResource({ uri }, T); } catch (e) { fail(`resources/read ${uri}: ${e.message}`); }
    const c = r.contents?.[0];
    if (!c || typeof c.text !== "string") fail(`resources/read ${uri}: no text contents`);
    return c;
  };
  const hashRes = await read("bim://doc/hash");
  if (hashRes.text.trim() !== cli(["hash", dir]).trim()) fail("bim://doc/hash differs from bim hash");
  const entRes = await read("bim://doc/entities");
  let ents;
  try { ents = JSON.parse(entRes.text); } catch { fail("bim://doc/entities is not JSON"); }
  const key = (e) => `${e.id}\u0000${e.kind}`;
  close(ents.map(key).sort(), ids.map(key).sort(), "bim://doc/entities {id,kind} set");
  const svgRes = await read("bim://view/plan:L1@1.2.svg");
  if (svgRes.mimeType !== "image/svg+xml") fail(`bim://view/…svg mimeType ${svgRes.mimeType}`);
  const out = path.join(tmp, "r.svg");
  cli(["export", dir, "--format", "svg", "--view", "plan:L1@1.2", "--out", out]);
  if (svgRes.text.trim() !== fs.readFileSync(out, "utf8").trim()) fail("bim://view/plan:L1@1.2.svg differs from bim export");
  const clashRes = await read("bim://clash/matrix");
  if (clashRes.mimeType !== "application/json") fail(`bim://clash/matrix mimeType ${clashRes.mimeType}`);
  close(JSON.parse(clashRes.text), clashCli, "bim://clash/matrix vs bim clash --json");

  // ---------- malformed input: isError, never a crash ----------
  const bad = [
    ["query", { program: "p(X :- .", query: "p(X)" }],
    ["query", { program: "n(\"a\").\np(X) :- n(X), not p(X).", query: "p(X)" }],
    ["query", { program: "p(\"a\").", query: "p(X" }],
    ["get_entity", { id: "NO_SUCH_ENTITY" }],
    ["get_entity", {}],
    ["get_entity", { id: 42 }],
    ["render_view", { view: "plan:NO_SUCH_LEVEL@1", format: "svg" }],
    ["render_view", { view: "bogus", format: "svg" }],
    ["clash_matrix", { clearance: "abc" }],
  ];
  for (const [name, args] of bad) {
    await call(name, args, true);
    const again = await call("get_entity", { id: "W1" });
    if (again.id !== "W1") fail(`server unusable after malformed ${name} call`);
  }
  if (!alive()) fail(`server process exited; stderr: ${stderr}`);
  if (cli(["hash", dir]).trim() !== hashRes.text.trim()) fail("read-only tools changed the document hash");
} finally {
  await client.close().catch(() => {});
}
console.log("T6.2 judge: all checks passed");
process.exit(0);
