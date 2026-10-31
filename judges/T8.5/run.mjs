// External judge for T8.5 (Studio product catalogues). usage: node run.mjs <path-to-bim>
// The Studio runs with ?sync= against a real `bim sync serve`; a fresh CLI peer (`bim sync` into a new dir,
// then `bim show`) is the independent view of the document, as in T8.4. Expected Products come from the
// catalogue contract (spec/catalogs.md) and from `bim catalog use` on the same catalogue files.
// The plan's pixel <-> model mapping is calibrated from two walls drawn with tool-wall (an affine fit, so
// scale, rotation and a flipped y axis are all absorbed); the other fixture elements are then created by a
// CLI peer at model coordinates that land in known parts of the plan.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { oracle, CHROME, ROOT } from "../lib/oracle.mjs";
import { serve, deadline } from "./serve.mjs";
import { JudgeFail, fail, check, tid, sleep, shot, diff, until, ready, reveal, park, clickIn } from "./studio.mjs";

deadline(150_000, "T8.5 checks");
const BIM = process.argv[2];
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "judge-t85-"));
const children = [];
process.on("exit", () => { for (const c of children) c.kill("SIGKILL"); fs.rmSync(tmp, { recursive: true, force: true }); });
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(sig, () => process.exit(1));

function bim(args, { ok = [0], timeout = 60_000 } = {}) {
  const r = spawnSync(BIM, args, { encoding: "utf8", timeout });
  if (r.error) fail(`'bim ${args.join(" ")}' failed: ${r.error.message}`);
  if (!ok.includes(r.status)) fail(`'bim ${args.join(" ")}' exited ${r.status}: ${(r.stderr || "").trim().slice(0, 300)}`);
  return r.stdout;
}
function writeJson(name, doc) { const f = path.join(tmp, name); fs.mkdirSync(path.dirname(f), { recursive: true }); fs.writeFileSync(f, JSON.stringify(doc, null, 1)); return f; }
let opsN = 0;
function applyOps(dir, ops) { bim(["apply", dir, writeJson(`ops${opsN++}.json`, { schema: 1, ops })]); }
async function syncServe(dir) {
  const p = spawn(BIM, ["sync", "serve", dir, "--port", "0"], { stdio: ["ignore", "pipe", "pipe"] });
  children.push(p);
  let out = "";
  p.stdout.on("data", (b) => { out += b; });
  const port = await until(() => { const m = out.match(/^listening (\d+)\s*$/m); return m ? +m[1] : null; }, 10_000, 50);
  check(port, `'bim sync serve' did not print 'listening <port>' (got '${out.slice(0, 80)}')`);
  return port;
}
let peerN = 0;
function peer(port) { // fresh CLI peer holding the server's current state
  const d = path.join(tmp, `peer${peerN++}`);
  bim(["init", d]);
  bim(["sync", d, `ws://127.0.0.1:${port}`], { timeout: 30_000 });
  return d;
}
const show = (d, id) => JSON.parse(bim(["show", d, id, "--json"]));
const tryShow = (d, id) => { const r = spawnSync(BIM, ["show", d, id, "--json"], { encoding: "utf8", timeout: 30_000 }); return r.status === 0 ? JSON.parse(r.stdout) : null; };
const wallIds = (d) => (JSON.parse(bim(["solve", d, "--trade", "arch", "--json"])).walls || []).map((w) => w.id);
const productCounts = (d) => Object.fromEntries((JSON.parse(bim(["solve", d, "--trade", "arch", "--json"])).products || []).map((p) => [p.id, p.count]));
const refOf = (v) => (v && typeof v === "object" && !Array.isArray(v) ? v.ref : undefined);

// ----------------------------------------------------------------------------- catalogues
// Category order deliberately interleaved in the files; a product-level manufacturer override; a boolean,
// a list and a text prop for type fidelity; the same sku in two catalogues.
const ACME = { catalog: "acme", version: "2026.1", manufacturer: "Acme Building Products Ltd", products: [
  { sku: "FD30-926", category: "door", name: "Oak fire door 926 x 2040", props: { width: 0.926, height: 2.04, fire_rating: "FD30", u_value: 1.8, self_closing: true } },
  { sku: "SOFA-3", category: "furniture", name: "Three-seat sofa", props: { size: [2.1, 0.9, 0.85], colour: "slate" } },
  { sku: "CW-1210", category: "window", name: "Casement window 1210 x 1050", props: { width: 1.21, height: 1.05, u_value: 1.2, glazing: "double" } },
  { sku: "EM-WHT", category: "wall_finish", name: "Matt emulsion, white", props: { colour: "white", coats: 2 } },
  { sku: "TBL-2", category: "furniture", name: "Dining table", manufacturer: "Nordic Tables AB", props: { size: [1.6, 0.8, 0.75] } },
  { sku: "OAK-14", category: "floor_finish", name: "Engineered oak 14 mm", props: { thickness: 0.014 } },
  { sku: "CT-600", category: "ceiling_finish", name: "Mineral ceiling tile 600", manufacturer: "Ceilings Direct GmbH", props: { tile: [0.6, 0.6], colour: "white" } },
  { sku: "SL-NAT", category: "roof_covering", name: "Natural slate", props: { mass_per_area: 25 } },
] };
const HAB = { catalog: "habitat-b", version: "7", manufacturer: "Habitat Brand Co", products: [
  { sku: "SOFA-3", category: "furniture", name: "Corner sofa", props: { size: [2.4, 1.6, 0.8], colour: "ochre" } },
  { sku: "LAMP-1", category: "lighting", name: "Arc floor lamp", props: { height: 1.9 } },
] };
const CATS = [ACME, HAB];
const PRODUCTS = CATS.flatMap((c) => c.products.map((p) => ({ ...p, catalog: c.catalog, id: `${c.catalog}:${p.sku}`,
  manufacturer: p.manufacturer ?? c.manufacturer, expected: { catalog: c.catalog, catalog_version: c.version, sku: p.sku,
    category: p.category, name: p.name, manufacturer: p.manufacturer ?? c.manufacturer, ...p.props } })));
const P = Object.fromEntries(PRODUCTS.map((p) => [p.id, p]));

/** Deep equality of JSON-encoded Values (numbers within 1e-12 relative). */
function sameValue(a, b) {
  if (typeof a === "number" && typeof b === "number") return Math.abs(a - b) <= 1e-12 * Math.max(1, Math.abs(a), Math.abs(b));
  if (Array.isArray(a) || Array.isArray(b)) return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((x, i) => sameValue(x, b[i]));
  if (a && b && typeof a === "object" && typeof b === "object") {
    const ka = Object.keys(a).sort(), kb = Object.keys(b).sort();
    return ka.length === kb.length && ka.every((k, i) => k === kb[i] && sameValue(a[k], b[k]));
  }
  return a === b;
}
const userProps = (props) => Object.fromEntries(Object.entries(props).filter(([k]) => !k.startsWith("_")));

// ----------------------------------------------------------------------------- affine plan calibration
/** Least-squares affine map pixel -> model from pairs [[px, py], [X, Y]]. */
function fitAffine(pairs) {
  const A = [[0, 0, 0], [0, 0, 0], [0, 0, 0]], bx = [0, 0, 0], by = [0, 0, 0];
  for (const [[u, v], [X, Y]] of pairs) {
    const r = [u, v, 1];
    for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) A[i][j] += r[i] * r[j]; bx[i] += r[i] * X; by[i] += r[i] * Y; }
  }
  const solve3 = (M, b) => { // Cramer's rule
    const det = (m) => m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    const D = det(M);
    return [0, 1, 2].map((c) => det(M.map((row, i) => row.map((x, j) => (j === c ? b[i] : x)))) / D);
  };
  const [a, b, c] = solve3(A, bx), [d, e, f] = solve3(A, by);
  const toModel = ([u, v]) => [a * u + b * v + c, d * u + e * v + f];
  const D = a * e - b * d;
  const toPixel = ([X, Y]) => [(e * (X - c) - b * (Y - f)) / D, (-d * (X - c) + a * (Y - f)) / D];
  const mPerPx = Math.sqrt(Math.abs(D));
  const residualPx = Math.max(...pairs.map(([p, m]) => { const q = toPixel(m); return Math.hypot(q[0] - p[0], q[1] - p[1]); }));
  return { toModel, toPixel, mPerPx, residualPx, D };
}

// ----------------------------------------------------------------------------- page helpers
/** Value shown by a prop-<key> element of props-panel (input value, else its text), or null if absent. */
async function propRead(page, key) {
  const loc = page.locator(`${tid("props-panel")} ${tid(`prop-${key}`)}`).first();
  if (!(await loc.count())) return null;
  if (!(await loc.isVisible().catch(() => false))) {
    try { await reveal(page, "props-panel"); } catch { return null; }
    if (!(await loc.isVisible().catch(() => false))) return null;
  }
  return loc.evaluate((el) => (el.matches("input, select, textarea") ? el.value : el.innerText).trim());
}
const near = (s, want, tol) => s !== null && Math.abs(parseFloat(s) - want) <= tol;

const { chromium } = await oracle("playwright-core");
const { AxeBuilder } = await oracle("@axe-core/playwright");
const srv = await serve(path.join(ROOT, "apps/studio/dist"), { spa: true });
const browser = await chromium.launch({ executablePath: CHROME, headless: true, args: ["--enable-unsafe-webgpu"] });
const errors = [];
const sentences = [];

let code = 0;
try {
  // ------------------------------------------------- reference Products from the CLI (spec/catalogs.md)
  const cli = path.join(tmp, "cli");
  bim(["init", cli]);
  for (const c of CATS) writeJson(`cli/catalogs/${c.catalog}/catalog.json`, c);
  const acmeFile = writeJson("upload-acme/catalog.json", ACME);
  const habFile = writeJson("upload-hab/catalog.json", HAB);
  const USED = ["acme:FD30-926", "acme:CW-1210", "acme:SOFA-3", "acme:EM-WHT", "acme:OAK-14", "acme:CT-600", "acme:SL-NAT", "habitat-b:SOFA-3"];
  const REF = {};
  for (const id of USED) {
    const printed = bim(["catalog", "use", cli, id]).trim();
    check(printed.split(/\s+/).pop() === id, `'bim catalog use ${id}' printed '${printed}', want the id`);
    const e = show(cli, id);
    check(e.kind === "Product", `bim catalog use ${id} created a ${e.kind}`);
    check(sameValue(userProps(e.props), P[id].expected),
      `bim catalog use ${id} gives ${JSON.stringify(userProps(e.props))}, spec/catalogs.md gives ${JSON.stringify(P[id].expected)} (T4.9 regression)`);
    REF[id] = userProps(e.props);
  }

  // --------------------------------------------------------------- Studio, synced to a one-Level document
  const srvDir = path.join(tmp, "srv");
  bim(["init", srvDir]);
  applyOps(srvDir, [{ op: "create", id: "L0", kind: "Level", props: { elevation: 0 } }]);
  const port = await syncServe(srvDir);
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
  const page = await ctx.newPage();
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (e) => errors.push(`${e}`));
  await page.goto(`http://localhost:${srv.port}/?sync=ws://127.0.0.1:${port}`);
  await ready(page, "initial load");

  // ------------------------------------------------------------------ 1. import two catalogues
  for (const id of ["catalog-import", "catalog-products", "tool-furniture", "assign-product"])
    check(await page.locator(tid(id)).count() > 0, `missing data-testid="${id}"`);
  const imp = page.locator(tid("catalog-import")).first();
  check(await imp.evaluate((el) => el.tagName === "INPUT" && el.type === "file"), "catalog-import is not a file input (spec/web.md)");
  const item = (id) => page.locator(`${tid("catalog-products")} ${tid(`product-${id}`)}`);
  for (const [file, cat] of [[acmeFile, ACME], [habFile, HAB]]) {
    await imp.setInputFiles(file);
    const listed = await until(async () => (await item(`${cat.catalog}:${cat.products[0].sku}`).count()) > 0, 8000, 200);
    check(listed, `after importing ${cat.catalog}'s catalog.json, catalog-products shows no product-${cat.catalog}:${cat.products[0].sku}`);
  }
  await reveal(page, "catalog-products");

  // ------------------------------------------- 2. one item per product, name and manufacturer, by category
  for (const p of PRODUCTS) {
    const loc = item(p.id);
    check((await loc.count()) === 1, `catalog-products has ${await loc.count()} product-${p.id} items, want 1`);
    await loc.first().scrollIntoViewIfNeeded().catch(() => {});
    const text = await loc.first().innerText();
    check(text.includes(p.name), `product-${p.id} does not show its name '${p.name}' (shows '${text.trim().slice(0, 120)}')`);
    check(text.includes(p.manufacturer), `product-${p.id} does not show its manufacturer '${p.manufacturer}' (shows '${text.trim().slice(0, 120)}')`);
  }
  const order = await page.locator(tid("catalog-products")).first().evaluate((el) =>
    [...el.querySelectorAll('[data-testid^="product-"]')].map((e) => e.getAttribute("data-testid").slice("product-".length)));
  for (const c of CATS) { // within each catalogue, a category's items are consecutive
    const cats = order.filter((id) => P[id] && P[id].catalog === c.catalog).map((id) => P[id].category);
    const seen = new Set();
    cats.forEach((k, i) => {
      check(!(seen.has(k) && cats[i - 1] !== k), `catalog-products does not list ${c.catalog}'s products by category: order ${cats.join(", ")}`);
      seen.add(k);
    });
  }

  // ---------------------------------------------- 3. axe-core with the catalogue panel open
  check(await page.locator(tid("catalog-products")).first().isVisible(), "catalog-products is not visible after importing");
  {
    const res = await new AxeBuilder({ page }).analyze();
    const bad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    check(bad.length === 0, `axe-core with the catalogue panel open: ${bad.map((v) => `${v.id} (${v.impact}, ${v.nodes.length} nodes)`).join("; ")}`);
  }

  // ------------------------------------------- 4. calibrate the plan with two walls drawn by tool-wall
  const plan = await reveal(page, "sheet-2d");
  const pb = await plan.boundingBox();
  check(pb.width > 300 && pb.height > 200, `sheet-2d is too small (${pb.width}x${pb.height}) to place the fixture`);
  const px = (fx, fy) => [pb.width * fx, pb.height * fy];
  const drawWall = async (a, b) => {
    await page.keyboard.press("Escape");
    await page.locator(tid("tool-wall")).first().click();
    await clickIn(page, "sheet-2d", a);
    await clickIn(page, "sheet-2d", b);
    await page.keyboard.press("Escape");
    await park(page);
  };
  const WA = [px(0.12, 0.22), px(0.56, 0.22)], WB = [px(0.88, 0.12), px(0.88, 0.62)];
  await drawWall(...WA);
  let ids = await until(() => { const w = wallIds(peer(port)); return w.length === 1 ? w : null; }, 15_000, 500);
  check(ids, "drawing a wall with tool-wall did not reach the sync server");
  const wa = ids[0];
  await drawWall(...WB);
  ids = await until(() => { const w = wallIds(peer(port)); return w.length === 2 ? w : null; }, 15_000, 500);
  check(ids, "drawing a second wall with tool-wall did not reach the sync server");
  const wb = ids.find((i) => i !== wa);
  let state = peer(port);
  const A = show(state, wa).props, B = show(state, wb).props;
  const ends = (w) => [[w.x1, w.y1], [w.x2, w.y2]];
  let cal = null;
  for (const ra of [false, true]) for (const rb of [false, true]) { // which click became (x1, y1)
    const ea = ra ? ends(A).reverse() : ends(A), eb = rb ? ends(B).reverse() : ends(B);
    const c = fitAffine([[WA[0], ea[0]], [WA[1], ea[1]], [WB[0], eb[0]], [WB[1], eb[1]]]);
    if (Number.isFinite(c.residualPx) && (!cal || c.residualPx < cal.residualPx)) cal = c;
  }
  const lenPx = Math.hypot(WA[1][0] - WA[0][0], WA[1][1] - WA[0][1]);
  check(cal && cal.residualPx <= 0.08 * lenPx,
    `the drawn walls' coordinates are not an affine image of the clicked points (residual ${cal && cal.residualPx.toFixed(1)} px)`);
  const L = Math.hypot(A.x2 - A.x1, A.y2 - A.y1), hA = A.height;
  check(L > 0 && hA > 0, `wall ${wa} has length ${L} and height ${hA}`);

  // ------------------------- 5. the rest of the fixture, created by a CLI peer and located in the plan
  const r3 = (x) => Math.round(x * 1000) / 1000;
  const D1 = { offset: r3(0.15 * L), width: r3(Math.min(0.9, 0.12 * L)), sill: 0, height: r3(Math.min(2.1, 0.8 * hA)) };
  // The window spans sill..sill+height = ~0.4..1.9 m so that any plan cut height a Studio may choose above
  // 0.4 m (spec/web.md only promises "above 0.3 m") cuts it and it is drawn in sheet-2d.
  const N1 = { offset: r3(0.6 * L), width: r3(Math.min(1.2, 0.12 * L)), sill: r3(Math.min(0.4, 0.15 * hA)), height: r3(Math.min(1.5, 0.5 * hA)) };
  const rectModel = (fx0, fy0, fx1, fy1) => {
    let pts = [[fx0, fy0], [fx1, fy0], [fx1, fy1], [fx0, fy1]].map(([fx, fy]) => cal.toModel(px(fx, fy)).map(r3));
    let a2 = 0;
    for (let i = 0; i < 4; i++) a2 += pts[i][0] * pts[(i + 1) % 4][1] - pts[(i + 1) % 4][0] * pts[i][1];
    if (a2 < 0) pts = pts.reverse(); // counter-clockwise (spec/schema.md)
    return pts.flat();
  };
  const SLAB = rectModel(0.12, 0.42, 0.40, 0.72), ROOF = rectModel(0.50, 0.70, 0.72, 0.92);
  const pointOnWall = (off) => { const t = off / L; return [A.x1 + t * (A.x2 - A.x1), A.y1 + t * (A.y2 - A.y1)]; };
  const ELEMS = {
    D1: { op: { op: "create", id: "D1", kind: "Opening", props: { host: { ref: wa }, kind: "door", ...D1 } }, centre: pointOnWall(D1.offset + D1.width / 2) },
    N1: { op: { op: "create", id: "N1", kind: "Opening", props: { host: { ref: wa }, kind: "window", ...N1 } }, centre: pointOnWall(N1.offset + N1.width / 2) },
    S1: { op: { op: "create", id: "S1", kind: "Slab", props: { level: { ref: "L0" }, boundary: SLAB, thickness: 0.25 } }, frac: [0.26, 0.57] },
    R1: { op: { op: "create", id: "R1", kind: "Roof", props: { level: { ref: "L0" }, boundary: ROOF, slope: 0.1, angle: 0, thickness: 0.2, base: 0 } }, frac: [0.61, 0.81] },
  };
  const feeder = peer(port);
  await page.keyboard.press("Escape");
  for (const [name, e] of Object.entries(ELEMS)) {
    await park(page);
    const before = await shot(page, "sheet-2d");
    const noise = diff(before, await shot(page, "sheet-2d")).length;
    applyOps(feeder, [e.op]);
    bim(["sync", feeder, `ws://127.0.0.1:${port}`], { timeout: 30_000 });
    const changed = await until(async () => { const d = diff(before, await shot(page, "sheet-2d")); return d.length > noise + 15 ? d : null; }, 10_000, 300);
    check(changed, `${name} (${e.op.kind}) created by a sync peer did not appear in sheet-2d`);
    await sleep(400);
    e.pixels = diff(before, await shot(page, "sheet-2d"));
    e.expected = e.centre ? cal.toPixel(e.centre) : px(...e.frac);
  }

  /** Select an element with tool-select by clicking its plan pixels until `isIt()` confirms the selection. */
  const selectEl = async (name, isIt) => {
    const e = ELEMS[name] ?? name;
    const cands = [e.expected];
    const byDist = [...e.pixels].sort((p, q) => Math.hypot(p[0] - e.expected[0], p[1] - e.expected[1]) - Math.hypot(q[0] - e.expected[0], q[1] - e.expected[1]));
    for (const p of byDist) { if (cands.length >= 10) break; if (cands.every((c) => Math.hypot(c[0] - p[0], c[1] - p[1]) >= 4)) cands.push(p); }
    for (const c of cands) {
      await page.keyboard.press("Escape");
      await page.locator(tid("tool-select")).first().click();
      await reveal(page, "sheet-2d");
      await clickIn(page, "sheet-2d", c);
      if (await until(isIt, 1000, 100)) return;
    }
    fail(`cannot select ${typeof name === "string" ? name : e.label} by clicking it in sheet-2d (props-panel never showed it)`);
  };
  const IS = {
    D1: async () => near(await propRead(page, "offset"), D1.offset, 5e-4) && (await propRead(page, "sill")) !== null,
    N1: async () => near(await propRead(page, "offset"), N1.offset, 5e-4) && (await propRead(page, "sill")) !== null,
    S1: async () => near(await propRead(page, "thickness"), 0.25, 5e-4) && (await propRead(page, "slope")) === null,
    R1: async () => near(await propRead(page, "slope"), 0.1, 5e-4),
    WB: async () => near(await propRead(page, "x1"), B.x1, 2e-3) && near(await propRead(page, "y1"), B.y1, 2e-3) &&
      near(await propRead(page, "x2"), B.x2, 2e-3) && (await propRead(page, "sill")) === null,
  };
  // Wall B: pixels along its drawn segment (it was drawn, not fed, so no arrival diff).
  ELEMS.WB = { expected: cal.toPixel([(B.x1 + B.x2) / 2, (B.y1 + B.y2) / 2]),
    pixels: [0.3, 0.4, 0.6, 0.7].map((t) => cal.toPixel([B.x1 + t * (B.x2 - B.x1), B.y1 + t * (B.y2 - B.y1)])) };

  const pickProduct = async (id) => {
    await reveal(page, "catalog-products");
    const loc = item(id).first();
    await loc.scrollIntoViewIfNeeded().catch(() => {});
    await loc.click();
  };

  // ------------------------------------------------------- 6. assign products by category
  const assign = async (el, id, done, what) => {
    await selectEl(el, IS[el]);
    await pickProduct(id);
    await page.locator(tid("assign-product")).first().click();
    if (await until(() => done(peer(port)), 8000, 500)) return;
    // The order of element and product selection is not pinned: try product first, then element.
    await pickProduct(id);
    await selectEl(el, IS[el]);
    await page.locator(tid("assign-product")).first().click();
    const ok = await until(() => done(peer(port)), 10_000, 500);
    check(ok, `assign-product with ${id} selected and ${el} selected did not set ${what}`);
    sentences.push(`assign-product worked only with the product picked before the element (${el})`);
  };
  const propOf = (d, id, key) => show(d, id).props[key];
  await assign("D1", "acme:FD30-926", (d) => refOf(propOf(d, "D1", "product")) === "acme:FD30-926", "Opening D1.product");
  // props-panel shows effective props: inherited from the product, the element's own value winning.
  await selectEl("D1", IS.D1);
  const eff = await until(async () => {
    const fr = await propRead(page, "fire_rating"), u = await propRead(page, "u_value"), w = await propRead(page, "width");
    return fr === "FD30" && near(u, 1.8, 1e-6) && near(w, D1.width, 5e-4) ? true : null;
  }, 4000, 200);
  check(eff, `props-panel for door D1 with product acme:FD30-926 should show effective props prop-fire_rating 'FD30', ` +
    `prop-u_value 1.8 (from the product) and prop-width ${D1.width} (its own); shows ` +
    `${await propRead(page, "fire_rating")}, ${await propRead(page, "u_value")}, ${await propRead(page, "width")}`);
  await assign("N1", "acme:CW-1210", (d) => refOf(propOf(d, "N1", "product")) === "acme:CW-1210", "Opening N1.product");
  await assign("WB", "acme:EM-WHT", (d) => { const p = show(d, wb).props; return refOf(p.finish_left) === "acme:EM-WHT" && refOf(p.finish_right) === "acme:EM-WHT"; },
    `both finish_left and finish_right of wall ${wb}`);
  await assign("S1", "acme:OAK-14", (d) => refOf(propOf(d, "S1", "finish_top")) === "acme:OAK-14", "Slab S1.finish_top");
  await assign("S1", "acme:CT-600", (d) => refOf(propOf(d, "S1", "finish_bottom")) === "acme:CT-600", "Slab S1.finish_bottom");
  await assign("R1", "acme:SL-NAT", (d) => refOf(propOf(d, "R1", "covering")) === "acme:SL-NAT", "Roof R1.covering");

  // ------------------------------------------------------- 7. place furniture with tool-furniture
  const PLACE = [["acme:SOFA-3", px(0.55, 0.45)], ["acme:SOFA-3", px(0.72, 0.45)], ["habitat-b:SOFA-3", px(0.24, 0.88)]];
  const placedCount = (d, id) => productCounts(d)[id] || 0;
  for (const [k, [id, at]] of PLACE.entries()) {
    const want = PLACE.slice(0, k + 1).filter(([i]) => i === id).length;
    await page.keyboard.press("Escape");
    await pickProduct(id);
    await page.locator(tid("tool-furniture")).first().click();
    await reveal(page, "sheet-2d");
    await clickIn(page, "sheet-2d", at);
    await park(page);
    const ok = await until(() => placedCount(peer(port), id) === want, 10_000, 500);
    check(ok, `placing ${id} with tool-furniture (placement ${k + 1}) did not create a Furniture referencing it ` +
      `(bim solve products count ${placedCount(peer(port), id)}, want ${want})`);
  }
  await page.keyboard.press("Escape");

  // ------------------------------------------------------- 8. final document, read by a fresh CLI peer
  state = peer(port);
  for (const id of USED) {
    const e = tryShow(state, id);
    check(e, `the document has no Product ${id} after the Studio used it`);
    check(e.kind === "Product", `${id} is a ${e.kind}, want Product`);
    check(sameValue(userProps(e.props), REF[id]),
      `Product ${id} created by the Studio has props ${JSON.stringify(userProps(e.props))}; 'bim catalog use' creates ${JSON.stringify(REF[id])}`);
  }
  const fin = (id) => show(state, id).props;
  check(refOf(fin("D1").product) === "acme:FD30-926" && refOf(fin("N1").product) === "acme:CW-1210", "Opening products did not persist");
  check(refOf(fin(wb).finish_left) === "acme:EM-WHT" && refOf(fin(wb).finish_right) === "acme:EM-WHT", `wall ${wb} finishes did not persist`);
  check(refOf(fin("S1").finish_top) === "acme:OAK-14" && refOf(fin("S1").finish_bottom) === "acme:CT-600",
    `Slab S1 should end with finish_top acme:OAK-14 and finish_bottom acme:CT-600, has ${JSON.stringify(fin("S1").finish_top)}, ${JSON.stringify(fin("S1").finish_bottom)}`);
  check(refOf(fin("R1").covering) === "acme:SL-NAT", "Roof R1.covering did not persist");
  check(fin(wa).finish_left === undefined && fin(wa).finish_right === undefined, `wall ${wa} got a finish it was never assigned`);

  // Furniture: enumerate elements through the glTF export (extras.id per element, spec/cli.md).
  const gl = path.join(tmp, "final.gltf");
  bim(["export", state, "--format", "gltf", "--out", gl]);
  const nodes = JSON.parse(fs.readFileSync(gl, "utf8")).nodes || [];
  const furn = [...new Set(nodes.map((n) => n.extras && n.extras.id).filter(Boolean))]
    .map((id) => ({ id, e: show(state, id) })).filter((x) => x.e.kind === "Furniture");
  check(furn.length === PLACE.length, `the document has ${furn.length} Furniture elements, want ${PLACE.length}`);
  const tolM = 25 * cal.mPerPx;
  const unmatched = [...furn];
  for (const [id, at] of PLACE) {
    const [X, Y] = cal.toModel(at);
    const i = unmatched.findIndex((x) => refOf(x.e.props.product) === id && Math.hypot(x.e.props.x - X, x.e.props.y - Y) <= tolM);
    check(i >= 0, `no Furniture with product ${id} at the clicked plan point (${X.toFixed(2)}, ${Y.toFixed(2)}) ± ${tolM.toFixed(2)} m; ` +
      `have ${unmatched.map((x) => `${refOf(x.e.props.product)}@(${x.e.props.x}, ${x.e.props.y})`).join(", ")}`);
    const f = unmatched.splice(i, 1)[0].e.props;
    check(typeof f.angle === "number" && Math.abs(f.angle) < 1e-9, `Furniture for ${id} has angle ${JSON.stringify(f.angle)}, want 0 (spec/web.md tool-furniture)`);
    const lvl = tryShow(state, refOf(f.level));
    check(lvl && lvl.kind === "Level", `Furniture for ${id} has level ${JSON.stringify(f.level)}, not a Level`);
  }

  // props-panel of the Habitat sofa shows the product's colour (effective props).
  const hab = furn.find((x) => refOf(x.e.props.product) === "habitat-b:SOFA-3").e.props;
  await selectEl({ label: "the habitat-b:SOFA-3 Furniture", expected: cal.toPixel([hab.x, hab.y]), pixels: [[-1, 0], [1, 0], [0, -1], [0, 1]].map(([u, v]) => cal.toPixel([hab.x + 0.2 * u, hab.y + 0.2 * v])) },
    async () => (await propRead(page, "colour")) === "ochre");

  check(errors.length === 0, `page errors: ${errors.slice(0, 5).join(" | ")}`);
  console.log(`T8.5 judge: all checks passed${sentences.length ? ` (note: ${sentences.join("; ")})` : ""}`);
} catch (e) {
  code = 1;
  console.error(e instanceof JudgeFail ? `JUDGE FAIL: ${e.message}` : `JUDGE FAIL: unexpected error: ${e && e.stack || e}`);
  if (errors.length) console.error(`page errors: ${errors.slice(0, 5).join(" | ")}`);
} finally {
  await browser.close().catch(() => {});
  srv.server.closeAllConnections();
  await srv.close();
}
process.exit(code);
