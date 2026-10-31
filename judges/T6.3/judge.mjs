// External judge for T6.3: propose_transaction / commit_transaction through the official MCP SDK
// client. Every number in a proposal report is checked against the CLI run on a copy of the project
// with the same ops applied (the "before" numbers against the CLI on the live project).
// Usage: node judge.mjs <bim> <bim-mcp> <project-dir> <tmp-dir>
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { oracle } from "../lib/oracle.mjs";

const [bim, bimMcp, dir, tmp] = process.argv.slice(2);
let transport = null; // set once the server is spawned, so a failing exit never orphans it
const fail = (msg) => {
  console.error(`JUDGE FAIL: ${msg}`);
  if (transport?.pid) { try { process.kill(transport.pid, "SIGKILL"); } catch { /* already gone */ } }
  process.exit(1);
};
setTimeout(() => fail("timed out after 55 s"), 55_000).unref();

const { Client } = await oracle("@modelcontextprotocol/sdk/client/index.js");
const { StdioClientTransport } = await oracle("@modelcontextprotocol/sdk/client/stdio.js");

// ---------- CLI oracle ----------
function cli(args, okCodes = [0]) {
  const r = spawnSync(bim, args, { encoding: "utf8", timeout: 30_000 });
  if (!okCodes.includes(r.status)) fail(`bim ${args.join(" ")} exited ${r.status}: ${r.stderr}`);
  return r.stdout;
}
const cliJson = (args, ok) => {
  const out = cli(args, ok);
  try { return JSON.parse(out); } catch { fail(`bim ${args.join(" ")} did not print JSON: ${out.slice(0, 200)}`); }
};
const hash = (d) => cli(["hash", d]).trim();
const PACKS = ["ibc", "eurocode", "accessibility"];
const vkey = (v) => `${v.rule}\u0000${v.entity}`;

/** The numbers a proposal report must reproduce, computed by the CLI on project dir `d`. */
function metrics(d) {
  const clash = cliJson(["clash", d, "--json"], [0, 1]);
  const struct = cliJson(["solve", d, "--trade", "struct", "--json"]);
  const mep = cliJson(["solve", d, "--trade", "mep", "--json"]);
  const arch = cliJson(["solve", d, "--trade", "arch", "--json"]);
  const violations = PACKS.flatMap((p) => cliJson(["validate", d, "--standard", p, "--json"], [0, 1]).violations);
  const conflicts = cliJson(["validate", d, "--json"], [0, 1]).conflicts;
  // The judge's fire engine (judges/T6.3/plugins/fire) writes the metrics it returns in output.metrics
  // into its output.json as well, so this is what the engine computed on `d`.
  const fire = cliJson(["solve", d, "--trade", "fire", "--json"]).metrics;
  if (!fire || typeof fire.sprinklers !== "number" || typeof fire.coverage_total !== "number")
    fail(`bim solve --trade fire did not print the judge engine's output.json: ${JSON.stringify(fire)}`);
  return {
    fire,
    clashes: clash.hard.length,
    // spec/plugins.md built-in metrics; each is 0 when its list is empty
    maxUtil: Math.max(0, ...struct.members.map((m) => m.utilization)),
    maxDp: Math.max(0, ...mep.segments.map((s) => Math.abs(s.dp))),
    totalArea: arch.rooms.reduce((a, r) => a + r.area, 0),
    violations, conflicts,
  };
}
let copyN = 0;
/** Copy the live project, apply `ops` with the CLI, return the copy's dir. */
function applied(ops) {
  const c = path.join(tmp, `copy${copyN++}`);
  fs.cpSync(dir, c, { recursive: true });
  const f = path.join(tmp, `ops${copyN}.json`);
  fs.writeFileSync(f, JSON.stringify({ schema: 1, ops }));
  cli(["apply", c, f]);
  return c;
}

function close(a, b, where, rel = 1e-9) {
  if (typeof a === "number" && typeof b === "number") {
    if (Math.abs(a - b) <= rel * Math.max(1, Math.abs(a), Math.abs(b))) return;
    fail(`${where}: ${a} != ${b}`);
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) fail(`${where}: length ${a.length} != ${b.length} (${JSON.stringify(a)} vs ${JSON.stringify(b)})`);
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
// Order-insensitive comparison key: objects serialised with sorted keys, so the server's and the
// CLI's key order cannot change how a list of violation/diagnostic objects sorts.
const stable = (v) => Array.isArray(v) ? v.map(stable)
  : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, stable(v[k])])) : v;
const canon = (list) => [...list].sort((x, y) => (JSON.stringify(stable(x)) < JSON.stringify(stable(y)) ? -1 : 1));

// ---------- MCP client ----------
transport = new StdioClientTransport({ command: bimMcp, args: [dir], stderr: "pipe" });
let stderr = "";
transport.stderr?.on("data", (d) => { stderr += d; });
const client = new Client({ name: "openbim-judge", version: "1.0.0" });
const T = { timeout: 40_000 };
try { await client.connect(transport, T); } catch (e) { fail(`initialize failed: ${e.message}; stderr: ${stderr}`); }

async function callRaw(name, args) {
  try { return await client.callTool({ name, arguments: args }, undefined, T); }
  catch (e) { fail(`tools/call ${name} raised a protocol error: ${e.message}; stderr: ${stderr}`); }
}
async function call(name, args) {
  const r = await callRaw(name, args);
  if (r.isError) fail(`tools/call ${name} ${JSON.stringify(args).slice(0, 300)} returned isError: ${JSON.stringify(r.content)}`);
  if (!r.structuredContent) fail(`tools/call ${name} returned no structuredContent`);
  return r.structuredContent;
}
async function callErr(name, args, why) {
  const r = await callRaw(name, args);
  if (r.isError !== true) fail(`${why}: expected isError, got ${JSON.stringify(r.structuredContent ?? r.content)}`);
}

const oplog = () => fs.readFileSync(path.join(dir, "doc.oplog"));
/** propose, asserting the live document is untouched. */
async function propose(ops) {
  const h0 = hash(dir), log0 = oplog();
  const rep = await call("propose_transaction", { ops });
  if (hash(dir) !== h0) fail("propose_transaction changed `bim hash`");
  if (!oplog().equals(log0)) fail("propose_transaction changed doc.oplog");
  return rep;
}

/** Check a valid proposal's report against the CLI before (live dir) and after (copy with ops). */
function checkReport(rep, ops, before, label) {
  if (rep.valid !== true) fail(`${label}: valid should be true, got ${JSON.stringify(rep.valid)} ${JSON.stringify(rep.reasons ?? "")}`);
  if (typeof rep.tx !== "string" || rep.tx.length === 0) fail(`${label}: tx id missing`);
  if (typeof rep.solver?.status !== "string") fail(`${label}: solver.status missing`);
  if (!Number.isInteger(rep.solver?.dof) || rep.solver.dof < 0) fail(`${label}: solver.dof not a non-negative integer`);
  const after = metrics(applied(ops));
  close(rep.clashes?.before, before.clashes, `${label} clashes.before`);
  close(rep.clashes?.after, after.clashes, `${label} clashes.after`);
  // spec/mcp.md: one `trades` entry per registered engine: the built-ins plus the project's fire package
  const tradeNames = Object.keys(rep.trades ?? {}).sort();
  if (JSON.stringify(tradeNames) !== JSON.stringify(["arch", "fire", "mep", "struct"]))
    fail(`${label}: trades keys should be [arch, fire, mep, struct], got ${JSON.stringify(tradeNames)}`);
  // A WebAssembly engine's output.metrics reach `trades.<package>` through TradeEngine::metrics.
  const fireKeys = Object.keys(rep.trades.fire ?? {}).sort();
  if (JSON.stringify(fireKeys) !== JSON.stringify(["coverage_total", "sprinklers"]))
    fail(`${label}: trades.fire should have the engine's metrics [coverage_total, sprinklers], got ${JSON.stringify(fireKeys)}`);
  for (const k of fireKeys) {
    close(rep.trades.fire[k]?.before, before.fire[k], `${label} trades.fire.${k}.before`);
    close(rep.trades.fire[k]?.after, after.fire[k], `${label} trades.fire.${k}.after`);
  }
  close(rep.trades.struct.max_utilization?.before, before.maxUtil, `${label} trades.struct.max_utilization.before`);
  close(rep.trades.struct.max_utilization?.after, after.maxUtil, `${label} trades.struct.max_utilization.after`);
  close(rep.trades.mep.max_dp?.before, before.maxDp, `${label} trades.mep.max_dp.before`);
  close(rep.trades.mep.max_dp?.after, after.maxDp, `${label} trades.mep.max_dp.after`);
  close(rep.trades.arch.total_area?.before, before.totalArea, `${label} trades.arch.total_area.before`);
  close(rep.trades.arch.total_area?.after, after.totalArea, `${label} trades.arch.total_area.after`);
  close(rep.violations?.before, before.violations.length, `${label} violations.before`);
  close(rep.violations?.after, after.violations.length, `${label} violations.after`);
  const old = new Set(before.violations.map(vkey));
  const wantNew = canon(after.violations.filter((v) => !old.has(vkey(v))));
  if (!Array.isArray(rep.violations?.new)) fail(`${label}: violations.new missing`);
  close(canon(rep.violations.new), wantNew, `${label} violations.new`);
  if (!Array.isArray(rep.conflicts)) fail(`${label}: conflicts missing`);
  close(canon(rep.conflicts), canon(after.conflicts), `${label} conflicts`);
  return after;
}

try {
  const { tools } = await client.listTools(undefined, T);
  for (const n of ["propose_transaction", "commit_transaction"])
    if (!tools.some((t) => t.name === n)) fail(`tools/list lacks ${n}`);

  // ---- A: a transaction that moves every reported quantity ----
  const txA = [
    { op: "set", id: "G1", key: "diameter", value: 0.04 },          // dp up
    { op: "set", id: "B1", key: "section_b", value: 0.12 },         // utilization up (axis unchanged)
    { op: "set", id: "AL1", key: "pressure", value: 50000 },        // scales every member force under case D
    { op: "set", id: "D1", key: "width", value: 0.8 },              // ibc 0.813 and accessibility 0.85 door limits
    { op: "create", id: "C5", kind: "Column", props: { x: 3, y: 2, base: { ref: "L1" }, top: { ref: "LT" },
      section_b: 0.3, section_h: 0.3, material: { ref: "M1" } } },  // stands in pipe G1's path → new hard clash
    { op: "create", id: "SPR3", kind: "fire:Sprinkler", props: { level: { ref: "L1" }, x: 3, y: 3.5, z: 2,
      coverage_radius: 3 } },                                       // fire engine: sprinklers 2 → 3
  ];
  const before = metrics(dir);
  const repA = await propose(txA);
  const afterA = checkReport(repA, txA, before, "proposal A");
  if (afterA.clashes <= before.clashes) fail("fixture: column C5 should add a hard clash with pipe G1");
  if (!(afterA.maxUtil > before.maxUtil)) fail("fixture: a 10x area load and a narrower B1 should raise max utilization");
  if (!(afterA.maxDp > before.maxDp)) fail("fixture: shrinking G1 should raise max dp");
  // independent of the engine: the fixture has sprinklers SPR1 (r 2) and SPR2 (r 2.5); txA adds SPR3 (r 3)
  close([before.fire.sprinklers, before.fire.coverage_total, afterA.fire.sprinklers, afterA.fire.coverage_total],
    [2, 4.5, 3, 7.5], "fire engine metrics before/after proposal A (sprinkler count and coverage from the fixture)");
  const d1New = repA.violations.new.filter((v) => v.entity === "D1");
  if (d1New.length < 2 || d1New.some((v) => Math.abs(v.measured - 0.8) > 1e-9))
    fail(`proposal A: expected >= 2 new door violations on D1 measured 0.8, got ${JSON.stringify(d1New)}`);

  // ---- conflicts: deleting the door leaves Issue I1 with a dangling ref ----
  const txDel = [{ op: "delete", id: "D1" }];
  checkReport(await propose(txDel), txDel, before, "proposal delete D1");

  // ---- B: commit applies exactly the proposed ops; A becomes stale ----
  const txB = [{ op: "set", id: "W2", key: "height", value: 2.7 }, { op: "set", id: "SPR1", key: "coverage_radius", value: 1 },
    { op: "create", id: "I2", kind: "Issue",
    props: { title: "height", status: "open", entities: [{ ref: "W2" }] } }];
  const repB = await propose(txB);
  checkReport(repB, txB, before, "proposal B");
  const wantB = hash(applied(txB));
  const comB = await call("commit_transaction", { tx: repB.tx });
  if (comB.hash !== wantB) fail(`commit B returned hash ${comB.hash}, CLI on a copy with the ops gives ${wantB}`);
  if (hash(dir) !== wantB) fail("after commit B, `bim hash` on the project differs from the copy with the same ops");
  await callErr("commit_transaction", { tx: repA.tx }, "commit of A after B was committed (stale)");
  if (hash(dir) !== wantB) fail("a rejected stale commit changed the document");
  await callErr("commit_transaction", { tx: repB.tx }, "second commit of B (stale)");
  await callErr("commit_transaction", { tx: "no-such-tx" }, "commit of an unknown tx");

  // ---- C: an intervening change made outside the server makes a proposal stale ----
  const txC = [{ op: "set", id: "W3", key: "height", value: 2.6 }];
  const repC = await propose(txC);
  const ext = path.join(tmp, "ext.json");
  fs.writeFileSync(ext, JSON.stringify({ schema: 1, ops: [{ op: "set", id: "W4", key: "height", value: 2.65 }] }));
  cli(["apply", dir, ext]);
  const hExt = hash(dir);
  await callErr("commit_transaction", { tx: repC.tx }, "commit after an intervening `bim apply` (stale)");
  if (hash(dir) !== hExt) fail("a rejected stale commit changed the document");

  // ---- D: fresh proposal on the current state commits cleanly ----
  const before2 = metrics(dir);
  const repA2 = await propose(txA);
  checkReport(repA2, txA, before2, "proposal A (second)");
  const wantA2 = hash(applied(txA));
  const comA2 = await call("commit_transaction", { tx: repA2.tx });
  if (comA2.hash !== wantA2 || hash(dir) !== wantA2) fail("commit A (second) does not equal the CLI applying the same ops");
  const shown = cliJson(["show", dir, "C5", "--json"]);
  if (shown.kind !== "Column") fail("committed create of C5 not visible to `bim show`");

  // ---- invalid ops: valid:false with reasons, nothing applied, tx not committable ----
  const invalid = [
    ["set on a missing entity", [{ op: "set", id: "NO_SUCH", key: "height", value: 1 }]],
    ["create of an existing id", [{ op: "create", id: "W1", kind: "Wall", props: {} }]],
    ["delete of a missing entity", [{ op: "delete", id: "NO_SUCH" }]],
    ["valid op followed by an invalid one", [{ op: "set", id: "W1", key: "height", value: 3.1 },
      { op: "set", id: "NO_SUCH", key: "height", value: 1 }]],
  ];
  for (const [why, ops] of invalid) {
    const h0 = hash(dir);
    const rep = await propose(ops);
    if (rep.valid !== false) fail(`${why}: valid should be false`);
    if (!Array.isArray(rep.reasons) || rep.reasons.length === 0 || rep.reasons.some((r) => typeof r !== "string" || !r))
      fail(`${why}: reasons should be a non-empty list of strings, got ${JSON.stringify(rep.reasons)}`);
    if (typeof rep.tx === "string") await callErr("commit_transaction", { tx: rep.tx }, `${why}: committing an invalid proposal`);
    if (hash(dir) !== h0) fail(`${why}: document changed`);
  }
  // Malformed requests are tool errors, and the server survives them.
  await callErr("propose_transaction", {}, "propose_transaction without ops");
  await callErr("propose_transaction", { ops: "set W1 height 3" }, "propose_transaction with ops not a list");
  await callErr("commit_transaction", {}, "commit_transaction without tx");
  const last = await propose([{ op: "set", id: "W1", key: "height", value: 2.9 }]);
  if (last.valid !== true) fail("server unusable after malformed requests");
} finally {
  await client.close().catch(() => {});
}
console.log("T6.3 judge: all checks passed");
process.exit(0);
