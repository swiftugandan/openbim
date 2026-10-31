// External judge for T7.1: load the wasm-pack builds of bim-wasm in Node and check the spec/web.md
// `Model` contract against the native CLI (glTF export, hash, solve, svg export).
// Usage: node judge.mjs <bim> <tmp-dir>
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";

const [bim, tmp] = process.argv.slice(2);
const HERE = path.dirname(fileURLToPath(import.meta.url));
const PKG = path.resolve(HERE, "../../packages/bim-wasm");
const fail = (msg) => { console.error(`JUDGE FAIL: ${msg}`); process.exit(1); };
setTimeout(() => fail("timed out after 50 s"), 50_000).unref();

function cli(args, okCodes = [0]) {
  const r = spawnSync(bim, args, { encoding: "utf8", timeout: 30_000 });
  if (!okCodes.includes(r.status)) fail(`bim ${args.join(" ")} exited ${r.status}: ${r.stderr}`);
  return r.stdout;
}
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

// ---------- per-entity geometry summaries ----------
function newAcc() { return { tris: 0, vol: 0, area: 0, min: [Infinity, Infinity, Infinity], max: [-Infinity, -Infinity, -Infinity] }; }
function addTri(acc, a, b, c) {
  acc.tris++;
  acc.vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6;
  const u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]], v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
  acc.area += Math.hypot(u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]) / 2;
  for (const p of [a, b, c]) for (let k = 0; k < 3; k++) { acc.min[k] = Math.min(acc.min[k], p[k]); acc.max[k] = Math.max(acc.max[k], p[k]); }
}
function compareSummaries(got, want, label) {
  const gk = Object.keys(got).sort(), wk = Object.keys(want).sort();
  if (JSON.stringify(gk) !== JSON.stringify(wk)) fail(`${label}: entities with geometry ${gk} != glTF ${wk}`);
  for (const id of wk) {
    const g = got[id], w = want[id];
    if (g.tris !== w.tris) fail(`${label}: ${id} has ${g.tris} triangles, glTF has ${w.tris}`);
    close(g.vol, w.vol, `${label}: ${id} signed volume`, 1e-5);
    close(g.area, w.area, `${label}: ${id} surface area`, 1e-5);
    for (let k = 0; k < 3; k++) {
      if (Math.abs(g.min[k] - w.min[k]) > 1e-4 || Math.abs(g.max[k] - w.max[k]) > 1e-4) fail(`${label}: ${id} bounding box differs from glTF`);
    }
  }
}

// ---------- glTF 2.0 reader (embedded buffers, node hierarchy, extras.id) ----------
function mat4FromNode(n) {
  if (n.matrix) return n.matrix;
  const [tx, ty, tz] = n.translation ?? [0, 0, 0], [x, y, z, w] = n.rotation ?? [0, 0, 0, 1], [sx, sy, sz] = n.scale ?? [1, 1, 1];
  return [
    (1 - 2 * (y * y + z * z)) * sx, (2 * (x * y + z * w)) * sx, (2 * (x * z - y * w)) * sx, 0,
    (2 * (x * y - z * w)) * sy, (1 - 2 * (x * x + z * z)) * sy, (2 * (y * z + x * w)) * sy, 0,
    (2 * (x * z + y * w)) * sz, (2 * (y * z - x * w)) * sz, (1 - 2 * (x * x + y * y)) * sz, 0,
    tx, ty, tz, 1];
}
const mul = (a, b) => { const r = new Array(16).fill(0); for (let i = 0; i < 4; i++) for (let j = 0; j < 4; j++) for (let k = 0; k < 4; k++) r[j * 4 + i] += a[k * 4 + i] * b[j * 4 + k]; return r; };
const xf = (m, p) => [0, 1, 2].map((i) => m[i] * p[0] + m[4 + i] * p[1] + m[8 + i] * p[2] + m[12 + i]);
const I4 = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];

function gltfSummaries(file) {
  const g = JSON.parse(fs.readFileSync(file, "utf8"));
  const bufs = (g.buffers ?? []).map((b) => {
    if (!b.uri?.startsWith("data:")) fail("glTF buffer is not embedded (data: URI)");
    return Buffer.from(b.uri.slice(b.uri.indexOf(",") + 1), "base64");
  });
  const read = (ai) => {
    const a = g.accessors[ai], bv = g.bufferViews[a.bufferView];
    const n = { SCALAR: 1, VEC3: 3, VEC4: 4, VEC2: 2, MAT4: 16 }[a.type];
    const size = { 5126: 4, 5125: 4, 5123: 2, 5121: 1 }[a.componentType];
    const stride = bv.byteStride ?? n * size, base = (bv.byteOffset ?? 0) + (a.byteOffset ?? 0), buf = bufs[bv.buffer];
    const rd = { 5126: (o) => buf.readFloatLE(o), 5125: (o) => buf.readUInt32LE(o), 5123: (o) => buf.readUInt16LE(o), 5121: (o) => buf.readUInt8(o) }[a.componentType];
    const out = [];
    for (let i = 0; i < a.count; i++) { const e = []; for (let k = 0; k < n; k++) e.push(rd(base + i * stride + k * size)); out.push(n === 1 ? e[0] : e); }
    return out;
  };
  const sums = {};
  const visit = (ni, parentM, owner) => {
    const n = g.nodes[ni], m = mul(parentM, mat4FromNode(n));
    const id = n.extras?.id ?? owner;
    if (n.mesh !== undefined) {
      if (id === undefined) fail(`glTF node ${ni} has a mesh but no extras.id`);
      const acc = (sums[id] ??= newAcc());
      for (const p of g.meshes[n.mesh].primitives) {
        if ((p.mode ?? 4) !== 4) fail("glTF primitive is not TRIANGLES");
        const pos = read(p.attributes.POSITION).map((q) => xf(m, q));
        const idx = p.indices !== undefined ? read(p.indices) : pos.map((_, i) => i);
        for (let t = 0; t + 2 < idx.length; t += 3) addTri(acc, pos[idx[t]], pos[idx[t + 1]], pos[idx[t + 2]]);
      }
    }
    for (const c of n.children ?? []) visit(c, m, id);
  };
  const scene = g.scenes?.[g.scene ?? 0];
  for (const r of scene?.nodes ?? g.nodes.map((_, i) => i)) visit(r, I4, undefined);
  return sums;
}

// ---------- the wasm Model ----------
function wasmSummaries(model, label) {
  const P = model.positions(), N = model.normals(), X = model.indices(), E = model.triangleEntity();
  if (!(P instanceof Float32Array) || !(N instanceof Float32Array)) fail(`${label}: positions/normals must be Float32Array`);
  if (!(X instanceof Uint32Array) || !(E instanceof Uint32Array)) fail(`${label}: indices/triangleEntity must be Uint32Array`);
  if (P.length % 3 || X.length % 3) fail(`${label}: positions/indices length not a multiple of 3`);
  if (N.length !== P.length) fail(`${label}: normals length ${N.length} != positions length ${P.length}`);
  if (E.length !== X.length / 3) fail(`${label}: triangleEntity length ${E.length} != triangle count ${X.length / 3}`);
  const nv = P.length / 3;
  for (let v = 0; v < nv; v++) {
    const l = Math.hypot(N[3 * v], N[3 * v + 1], N[3 * v + 2]);
    if (Math.abs(l - 1) > 1e-3) fail(`${label}: normal ${v} has length ${l}`);
  }
  const names = new Map(), sums = {};
  for (let t = 0; t < E.length; t++) {
    if (!names.has(E[t])) {
      const nm = model.entityName(E[t]);
      if (typeof nm !== "string" || !nm) fail(`${label}: entityName(${E[t]}) is not an id string`);
      names.set(E[t], nm);
    }
    const v = [X[3 * t], X[3 * t + 1], X[3 * t + 2]];
    if (v.some((i) => i >= nv)) fail(`${label}: index out of range in triangle ${t}`);
    const p = v.map((i) => [P[3 * i], P[3 * i + 1], P[3 * i + 2]]);
    addTri((sums[names.get(E[t])] ??= newAcc()), p[0], p[1], p[2]);
  }
  return sums;
}
function zeroCopy(model, memory, label) {
  if (!(memory instanceof WebAssembly.Memory)) fail(`${label}: exported memory is not a WebAssembly.Memory`);
  for (const f of ["positions", "normals", "indices", "triangleEntity", "instances", "instanceMesh"]) {
    const v = model[f]();
    if (!ArrayBuffer.isView(v)) fail(`${label}: ${f}() is not a typed array`);
    if (v.buffer !== memory.buffer) fail(`${label}: ${f}().buffer !== memory.buffer (copied, not a view)`);
  }
  if (!(model.instances() instanceof Float32Array) || !(model.instanceMesh() instanceof Uint32Array)) fail(`${label}: instances/instanceMesh types`);
  if (model.instances().length !== 16 * model.instanceMesh().length) fail(`${label}: instances().length != 16 * instanceMesh().length`);
}

async function loadPkg(dir, label) {
  const pj = path.join(dir, "package.json");
  if (!fs.existsSync(pj)) fail(`${label}: ${pj} missing (build.sh must produce it)`);
  const meta = JSON.parse(fs.readFileSync(pj, "utf8"));
  const entry = meta.module ?? meta.main;
  if (!entry || !fs.existsSync(path.join(dir, entry))) fail(`${label}: package.json entry ${entry} missing`);
  if (!fs.readdirSync(dir).some((f) => f.endsWith(".wasm"))) fail(`${label}: no .wasm file in ${dir}`);
  let mod;
  try { mod = await import(pathToFileURL(path.join(dir, entry)).href); } catch (e) { fail(`${label}: cannot import ${entry}: ${e.message}`); }
  const pick = (k) => mod[k] ?? mod.default?.[k];
  return { mod, Model: pick("Model"), memory: () => pick("memory") };
}

const fixture = fs.readFileSync(path.join(HERE, "fixture.json"), "utf8");
const proj = path.join(tmp, "p");
cli(["init", proj]);
cli(["apply", proj, path.join(HERE, "fixture.json")]);
const nativeGltf = (label) => { const f = path.join(tmp, `${label}.gltf`); cli(["export", proj, "--format", "gltf", "--out", f]); return gltfSummaries(f); };
const nativeSvg = (view) => { const f = path.join(tmp, "v.svg"); cli(["export", proj, "--format", "svg", "--view", view, "--out", f]); return fs.readFileSync(f, "utf8"); };

// ---------- pkg-node ----------
const node = await loadPkg(path.join(PKG, "pkg-node"), "pkg-node");
if (typeof node.Model?.fromOps !== "function") fail("pkg-node does not export Model.fromOps");
let model;
try { model = node.Model.fromOps(fixture); } catch (e) { fail(`Model.fromOps(fixture) threw: ${e}`); }
zeroCopy(model, node.memory(), "fromOps");
if (model.hash() !== cli(["hash", proj]).trim()) fail(`hash() ${model.hash()} != bim hash ${cli(["hash", proj]).trim()}`);
const g0 = model.generation();
if (typeof g0 !== "number") fail("generation() is not a number");
compareSummaries(wasmSummaries(model, "fromOps"), nativeGltf("a"), "fromOps vs native glTF");
for (const trade of ["arch"]) {
  let got;
  try { got = JSON.parse(model.solve(trade)); } catch (e) { fail(`solve("${trade}") did not return JSON: ${e}`); }
  close(got, JSON.parse(cli(["solve", proj, "--trade", trade, "--json"])), `solve("${trade}") vs bim solve --json`);
}
for (const view of ["plan:L1@1.2", "section:-1,2,7,2"]) {
  if (model.exportView(view, "svg").trim() !== nativeSvg(view).trim()) fail(`exportView("${view}", "svg") differs from bim export --format svg`);
}

// apply: generation bumps, geometry/hash follow the CLI, views re-fetched stay zero-copy.
const edit = { schema: 1, ops: [{ op: "set", id: "W2", key: "height", value: 3.2 }, { op: "set", id: "C1", key: "x", value: 1.5 }] };
fs.writeFileSync(path.join(tmp, "edit.json"), JSON.stringify(edit));
try { model.apply(JSON.stringify(edit)); } catch (e) { fail(`apply(valid ops) threw: ${e}`); }
cli(["apply", proj, path.join(tmp, "edit.json")]);
const g1 = model.generation();
if (!(g1 > g0)) fail(`generation() did not bump on apply (${g0} -> ${g1})`);
zeroCopy(model, node.memory(), "after apply");
if (model.hash() !== cli(["hash", proj]).trim()) fail("hash() after apply != bim hash after the same ops");
const s1 = wasmSummaries(model, "after apply");
if (Math.abs(s1.W2.max[2] - 3.2) > 1e-5) fail(`after setting W2 height 3.2 its top is at z=${s1.W2.max[2]}`);
if (Math.abs((s1.C1.min[0] + s1.C1.max[0]) / 2 - 1.5) > 1e-5) fail("after moving C1 to x=1.5 its geometry did not follow");
compareSummaries(s1, nativeGltf("b"), "after apply vs native glTF");
close(JSON.parse(model.solve("arch")), JSON.parse(cli(["solve", proj, "--trade", "arch", "--json"])), "solve(arch) after apply");

// invalid apply: throws, atomic (hash, generation and geometry unchanged).
const h1 = model.hash();
for (const bad of [
  JSON.stringify({ schema: 1, ops: [{ op: "set", id: "W1", key: "height", value: 2.0 }, { op: "set", id: "NO_SUCH", key: "height", value: 1 }] }),
  JSON.stringify({ schema: 1, ops: [{ op: "create", id: "W1", kind: "Wall", props: {} }] }),
  "{not json",
]) {
  let threw = false;
  try { model.apply(bad); } catch { threw = true; }
  if (!threw) fail(`apply(${bad.slice(0, 80)}) should throw`);
  if (model.hash() !== h1) fail("a rejected apply changed hash()");
  if (model.generation() !== g1) fail("a rejected apply bumped generation()");
}
const s2 = wasmSummaries(model, "after rejected apply");
if (Math.abs(s2.W1.max[2] - 2.8) > 1e-5) fail("a rejected apply changed W1's geometry");
let threw = false;
try { node.Model.fromOps("{not json"); } catch { threw = true; }
if (!threw) fail("Model.fromOps(invalid json) should throw");

// ---------- pkg-web (target web): same contract after init with the wasm bytes ----------
const webDir = path.join(PKG, "pkg-web");
const web = await loadPkg(webDir, "pkg-web");
const wasmFile = fs.readdirSync(webDir).find((f) => f.endsWith(".wasm"));
const bytes = fs.readFileSync(path.join(webDir, wasmFile));
const init = web.mod.default;
if (typeof init !== "function") fail("pkg-web has no default init function");
try { await init({ module_or_path: bytes }); } catch { try { await init(bytes); } catch (e) { fail(`pkg-web init failed: ${e}`); } }
const wm = web.Model.fromOps(fixture);
zeroCopy(wm, web.memory(), "pkg-web");
const nm = node.Model.fromOps(fixture);
if (wm.hash() !== nm.hash()) fail("pkg-web hash differs from pkg-node hash on the same ops");
compareSummaries(wasmSummaries(wm, "pkg-web"), wasmSummaries(nm, "pkg-node"), "pkg-web vs pkg-node");

console.log("T7.1 judge: all checks passed");
process.exit(0);
