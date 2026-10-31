// External judge for T8.4 (Studio collaboration, checks, a11y, performance).
// usage: node run.mjs <path-to-bim>
// Two browser contexts sync through a real `bim sync serve`; a CLI peer (`bim sync` into a fresh
// dir) is the independent view of the shared document. Accessibility by axe-core, layout overflow
// measured in the page, frame time read from the status bar after the in-app harness (?bench=1).
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { oracle, CHROME, ROOT } from "../lib/oracle.mjs";
import { serve, deadline } from "./serve.mjs";
import {
  JudgeFail, fail, check, tid, sleep, shot, diff, segDist, centralPixel, until, ready, reveal, park,
  propValue, clickIn, setProp,
} from "./studio.mjs";

deadline(150_000, "T8.4 checks");
const BIM = process.argv[2];
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "judge-t84-"));
const children = [];
process.on("exit", () => { for (const c of children) c.kill("SIGKILL"); fs.rmSync(tmp, { recursive: true, force: true }); });

function bim(args, { ok = [0], timeout = 60_000 } = {}) {
  const r = spawnSync(BIM, args, { encoding: "utf8", timeout });
  if (r.error) fail(`'bim ${args.join(" ")}' failed: ${r.error.message}`);
  if (!ok.includes(r.status)) fail(`'bim ${args.join(" ")}' exited ${r.status}: ${(r.stderr || "").trim().slice(0, 300)}`);
  return r.stdout;
}
function project(name, ops) {
  const d = path.join(tmp, name);
  bim(["init", d]);
  const f = path.join(tmp, `${name}.json`);
  fs.writeFileSync(f, JSON.stringify({ schema: 1, ops }));
  bim(["apply", d, f]);
  return d;
}
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
/** Fresh CLI peer: pull the server's state into a new dir. */
function peer(port) {
  const d = path.join(tmp, `peer${peerN++}`);
  bim(["init", d]);
  bim(["sync", d, `ws://127.0.0.1:${port}`], { timeout: 30_000 });
  return d;
}
const walls = (d) => JSON.parse(bim(["solve", d, "--trade", "arch", "--json"])).walls || [];
const show = (d, id) => JSON.parse(bim(["show", d, id, "--json"]));

// ----------------------------------------------------------------------------- fixtures
const ref = (id) => ({ ref: id });
// Stair: (3.5 m - 0 m) / 15 risers = 0.2333 m > IBC 0.1778 m riser limit; 2r + t = 0.717 > 0.65 (Blondel).
const FIX = [
  { op: "create", id: "L0", kind: "Level", props: { elevation: 0 } },
  { op: "create", id: "L1", kind: "Level", props: { elevation: 3.5 } },
  { op: "create", id: "S1", kind: "Stair", props: { base: ref("L0"), top: ref("L1"), x: 30, y: 30, angle: 0, width: 1.2, riser_count: 15, tread: 0.25 } },
];
const BIG = [
  { op: "create", id: "L0", kind: "Level", props: { elevation: 0 } },
  { op: "create", id: "L1", kind: "Level", props: { elevation: 3 } },
  { op: "create", id: "WT", kind: "WallType", props: { layer_thickness: [0.2], layer_material: ["concrete"], layer_conductivity: [1.7] } },
  { op: "create", id: "M", kind: "Material", props: { name: "steel", E: 2.1e11, G: 8.1e10, density: 7850, fy: 3.55e8 } },
];
// 10,000 building elements: 5,000 separate walls and 5,000 columns on a 4 m grid.
for (let i = 0; i < 100; i++) for (let j = 0; j < 50; j++) {
  BIG.push({ op: "create", id: `w${i}_${j}`, kind: "Wall", props: { level: ref("L0"), x1: i * 4 + 0.3, y1: j * 4, x2: i * 4 + 3.7, y2: j * 4, height: 3, type: ref("WT") } });
  BIG.push({ op: "create", id: `c${i}_${j}`, kind: "Column", props: { x: i * 4, y: j * 4 + 2, base: ref("L0"), top: ref("L1"), section_b: 0.3, section_h: 0.3, material: ref("M") } });
}

const leafState = (page) => page.locator(tid("presence")).first().evaluate((el) => ({
  leaves: [...el.querySelectorAll("*")].filter((e) => e.children.length === 0).length, text: el.innerText,
}));

const { chromium } = await oracle("playwright-core");
const { AxeBuilder } = await oracle("@axe-core/playwright");
const srv = await serve(path.join(ROOT, "apps/studio/dist"), { spa: true });
const browser = await chromium.launch({ executablePath: CHROME, headless: true, args: ["--enable-unsafe-webgpu"] });
const errors = [];
async function openStudio(query, label) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
  const page = await ctx.newPage();
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (e) => errors.push(`${label}: ${e}`));
  await page.goto(`http://localhost:${srv.port}/${query}`);
  await ready(page, label);
  return page;
}

let code = 0;
try {
  const main = project("srv", FIX);
  const expectRules = new Set();
  for (const pack of ["ibc", "eurocode"]) {
    const v = JSON.parse(bim(["validate", main, "--standard", pack, "--json"], { ok: [0, 1] }));
    for (const x of v.violations || []) if (x.entity === "S1") expectRules.add(x.rule);
  }
  check(expectRules.size > 0, "fixture stair S1 (riser 0.233 m) has no ibc/eurocode violation from the CLI");
  const big = project("big", BIG);
  const port = await syncServe(main);
  const bigPort = await syncServe(big);
  const q = `?sync=ws://127.0.0.1:${port}`;

  // ------------------------------------------------------------- 1. presence of two peers
  const A = await openStudio(q, "A");
  await sleep(800);
  const presA0 = await leafState(A);
  const B = await openStudio(q, "B");
  const joined = await until(async () => { const s = await leafState(A); return s.leaves > presA0.leaves || s.text !== presA0.text; }, 8000, 250);
  check(joined, "presence in tab A did not change when tab B joined the session");
  const presB = await leafState(B);
  check(presB.leaves > presA0.leaves || presB.text !== presA0.text, "presence in tab B does not show tab A");

  // ---------------------------------------- 2. concurrent drawing converges in both tabs
  const pb = await (await reveal(A, "sheet-2d")).boundingBox();
  await reveal(B, "sheet-2d");
  const seg = (fy) => [[pb.width * 0.35, pb.height * fy], [pb.width * 0.65, pb.height * fy]];
  const SA = seg(0.4), SB = seg(0.6);
  const band = (px, [a, b]) => px.filter((p) => segDist(p, a, b) <= 15);
  const len = Math.hypot(SA[1][0] - SA[0][0], 0);
  await park(A); await park(B);
  const PA0 = await shot(A, "sheet-2d"), PB0 = await shot(B, "sheet-2d");
  const draw = async (page, S) => {
    await page.locator(tid("tool-wall")).first().click();
    await clickIn(page, "sheet-2d", S[0]);
    await clickIn(page, "sheet-2d", S[1]);
    await page.keyboard.press("Escape");
    await park(page);
  };
  await Promise.all([draw(A, SA), draw(B, SB)]);
  let state = await until(() => { const d = peer(port); return walls(d).length === 2 ? d : null; }, 15_000, 500);
  check(state, "the server did not receive both concurrently drawn walls");
  for (const [page, P0, mine, other, n] of [[A, PA0, SA, SB, "A"], [B, PB0, SB, SA, "B"]]) {
    const ok = await until(async () => {
      const d = diff(P0, await shot(page, "sheet-2d"));
      return band(d, mine).length >= 0.25 * len && band(d, other).length >= 0.25 * len;
    }, 10_000, 400);
    check(ok, `tab ${n} does not show both walls after concurrent drawing`);
  }
  const W1PT = centralPixel(band(diff(PA0, await shot(A, "sheet-2d")), SA));

  // ----------------------- 3. concurrent edits of the same wall converge to one value everywhere
  const select = async (page) => {
    await page.keyboard.press("Escape");
    await page.locator(tid("tool-select")).first().click();
    await clickIn(page, "sheet-2d", W1PT);
    const v = await until(() => propValue(page, "height"), 4000);
    check(v !== null, "clicking wall 1 on the plan did not fill props-panel");
    return v;
  };
  await select(A); await select(B);
  await Promise.all([setProp(A, "height", 4), setProp(B, "height", 5)]);
  let wall1 = null, h = null;
  state = await until(() => {
    const d = peer(port);
    for (const w of walls(d)) { const v = show(d, w.id).props.height; if (v === 4 || v === 5) { wall1 = w.id; h = v; return d; } }
    return null;
  }, 15_000, 500);
  check(state, "neither concurrent height edit reached the server");
  await sleep(1500);
  h = show(peer(port), wall1).props.height;
  for (const [page, n] of [[A, "A"], [B, "B"]]) {
    const ok = await until(async () => { await select(page); return Math.abs(parseFloat(await propValue(page, "height")) - h) < 1e-9; }, 10_000, 500);
    check(ok, `tab ${n} shows wall height ${await propValue(page, "height")} but the converged document has ${h}`);
  }

  // --------------------------- 4. Issues panel lists a BCF topic linked to the selected wall
  const issuePeer = peer(port);
  const TITLE = "Judge issue T84 clearance";
  bim(["bcf", issuePeer, "create", "--title", TITLE, "--entity", wall1]);
  bim(["sync", issuePeer, `ws://127.0.0.1:${port}`], { timeout: 30_000 });
  await select(A);
  const iss = await until(async () => (await A.locator(tid("issues")).first().innerText()).includes(TITLE), 10_000, 400);
  check(iss, `issues panel does not list the BCF topic '${TITLE}' linked to the selected wall`);

  // ----------------------------------------------- 5. Validate panel lists code violations
  await A.keyboard.press("Escape");
  await A.locator(tid("validate")).first().click();
  const vtext = await until(async () => {
    const t = await A.locator(tid("violations")).first().innerText();
    return t.includes("S1") && [...expectRules].some((r) => t.includes(r)) ? t : null;
  }, 10_000, 400);
  check(vtext, `violations panel does not list stair S1 with one of ${[...expectRules].join(", ")}`);

  // ----------------------------------------------------------------- 6. section-box tool
  await park(A);
  const V0 = await shot(A, "viewport-3d");
  const noise = diff(V0, await shot(A, "viewport-3d")).length;
  await A.locator(tid("tool-section-box")).first().click();
  await park(A);
  const sbox = await until(async () => diff(V0, await shot(A, "viewport-3d")).length > noise + 20, 3000, 300);
  check(sbox, "activating tool-section-box does not change the 3D view");
  await A.keyboard.press("Escape");

  // ------------------------------------------------- 7. responsive layout and accessibility
  for (const [w, hgt] of [[1440, 900], [390, 844]]) {
    await A.setViewportSize({ width: w, height: hgt });
    await sleep(500);
    const ov = await A.evaluate(() => ({ doc: document.documentElement.scrollWidth, body: document.body.scrollWidth, vw: window.innerWidth }));
    check(ov.doc <= ov.vw && ov.body <= ov.vw, `horizontal overflow at ${w} px: scrollWidth ${Math.max(ov.doc, ov.body)} > ${ov.vw}`);
    const res = await new AxeBuilder({ page: A }).analyze();
    const bad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    check(bad.length === 0, `axe-core at ${w} px: ${bad.map((v) => `${v.id} (${v.impact}, ${v.nodes.length} nodes)`).join("; ")}`);
  }

  // ------------------------------------- 8. frame-time harness on a 10,000-element model
  const C = await openStudio(`?sync=ws://127.0.0.1:${bigPort}&bench=1`, "bench");
  const p95 = await until(async () => {
    const m = (await C.locator(tid("status-bar")).first().innerText()).match(/p95\D{0,12}?(\d+(?:\.\d+)?)\s*ms/i);
    return m ? parseFloat(m[1]) : null;
  }, 60_000, 500);
  check(p95 !== null, "status-bar never reported 'p95 <n> ms' for the 10,000-element model (?bench=1)");
  check(p95 < 16.6, `frame time p95 ${p95} ms >= 16.6 ms on the 10,000-element model`);
  console.log(`T8.4 judge: all checks passed (p95 ${p95} ms)`);
} catch (e) {
  code = 1;
  console.error(e instanceof JudgeFail ? `JUDGE FAIL: ${e.message}` : `JUDGE FAIL: unexpected error: ${e && e.stack || e}`);
  if (errors.length) console.error(`page errors: ${errors.slice(0, 5).join(" | ")}`);
} finally {
  await browser.close().catch(() => {});
  await srv.close();
}
process.exit(code);
