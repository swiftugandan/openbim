// T8.6 part 2 (browser): the Studio's direct-modelling tools driven through spec/web.md hooks, the
// mouse and the keyboard, in headless system Chrome. The document lives in a real `bim sync serve`;
// a CLI peer reads what the Studio wrote and every result is compared with the same `bim sketch`
// command replayed on the Studio's pre-state. Pointer positions are computed from the camera the
// Studio reports (viewport-3d data-camera). Finally `Model.infer` latency is measured in a Chrome page
// on a 100,000-Shape model. Usage: node run.mjs <bim>
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { oracle, CHROME, ROOT } from "../lib/oracle.mjs";
import { serve, deadline, importMap } from "./serve.mjs";
import { JudgeFail, fail, check, tid, sleep, until, ready, park, shot } from "./studio.mjs";
import { cli } from "./peer.mjs";
import {
  shape, volume, closedManifold, bbox, boxCorners, edgesOf, facePoint, faceCentroid, faceArea, raycast, edgeClearance,
  vertexClearance, lineDist, onScreen, compareShapes, viewAngle, add, mulv, sub, dist, len, dot,
} from "./geom.mjs";
import {
  view, pagePt, hover, clickAt, dblclickAt, expectTip, tipText, typeValue, key, expectTool, zoomExtents, frames,
  countAlong, RED, GREEN, BLUE,
} from "./sketch.mjs";

deadline(135_000, "T8.6 browser checks");
const BIM = process.argv[2];
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "judge-t86-"));
const C = cli(BIM, tmp);
process.on("exit", () => { C.kill(); fs.rmSync(tmp, { recursive: true, force: true }); });
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(sig, () => process.exit(1));

const HOOKS = ["viewport-3d", "status-bar", "tool-select", "tool-line", "tool-rect", "tool-circle", "tool-arc", "tool-pushpull",
  "tool-orbit", "tool-pan", "zoom-extents", "measurements", "inference-tip"];
const X = [1, 0, 0], Y = [0, 1, 0], Z = [0, 0, 1];
const L0 = { op: "create", id: "L0", kind: "Level", props: { elevation: 0 } };
const FIXTURE = [
  { op: "rect", shape: "fx", corner: [0, 0, 0], u: [2, 0, 0], v: [0, 2, 0] },
  { op: "pushpull", shape: "fx", face: 1, distance: 2 },
  { op: "rect", shape: "fy", corner: [12, 8, 0], u: [1, 0, 0], v: [0, 1, 0] },
  { op: "pushpull", shape: "fy", face: 1, distance: 1 },
];

const { chromium } = await oracle("playwright-core");
const browser = await chromium.launch({ executablePath: CHROME, headless: true, args: ["--enable-unsafe-webgpu"] });
const errors = [];
let srv = null, srv2 = null;
let code = 0;

/** The latency model loads in its own page while the Studio checks run; the timed queries run after them. */
async function startStress() {
  const N = 100_000, COLS = 320;
  const ops = [L0];
  const FACES = [[[0, 3, 2, 1]], [[4, 5, 6, 7]], [[0, 1, 5, 4]], [[1, 2, 6, 5]], [[2, 3, 7, 6]], [[3, 0, 4, 7]]];
  for (let i = 0; i < N; i++) {
    const x = (i % COLS) * 2, y = Math.floor(i / COLS) * 2, vv = [];
    for (const z of [0, 1]) for (const [a, b] of [[0, 0], [1, 0], [1, 1], [0, 1]]) vv.push(x + a, y + b, z);
    ops.push({ op: "create", id: `s${i}`, kind: "Shape", props: { vertices: vv, faces: FACES, face_ids: [1, 2, 3, 4, 5, 6], edges: [] } });
  }
  const PAGE = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>T8.6 inference latency</title>
<script type="importmap">${importMap(ROOT)}</script></head><body><script type="module">window.__ready = true;</script></body></html>`;
  srv2 = await serve(ROOT, { overrides: {
    "/judges/T8.6/latency.html": { type: "text/html; charset=utf-8", body: PAGE },
    "/judges/T8.6/stress.json": { type: "application/json", body: JSON.stringify({ schema: 1, ops }) },
  } });
  const p2 = await (await browser.newContext({ viewport: { width: 800, height: 600 } })).newPage();
  p2.setDefaultTimeout(90_000);
  p2.on("pageerror", (e) => errors.push(String(e)));
  await p2.goto(`http://localhost:${srv2.port}/judges/T8.6/latency.html`);
  await p2.waitForFunction(() => window.__ready === true);
  const loaded = p2.evaluate(async () => {
    try {
      const w = await import("bim-wasm");
      await w.default();
      const text = await (await fetch("/judges/T8.6/stress.json")).text();
      window.__m = w.Model.fromOps(text);
      window.__q = (i) => {
        const camera = { eye: [150, 120, 25], target: [160, 150, 0], fov: 0.8, width: 1000, height: 700 };
        const o = { camera, cursor: [500 + 470 * Math.sin(i * 0.37), 350 + 330 * Math.sin(i * 0.23 + 1)] };
        if (i % 2) { o.from = [150 + (i % 7) * 2, 140 + (i % 5) * 2, 1]; o.reference_edge = [[0, 0, 0], [1, 1, 0]]; }
        return JSON.stringify(o);
      };
      window.__m.infer(window.__q(0)); // warm-up: a lazily built index is not counted
      return { ok: true };
    } catch (e) { return { ok: false, err: String(e && e.stack || e) }; }
  }).catch((e) => ({ ok: false, err: String(e) }));
  return {
    async timed() {
      const l = await loaded;
      if (!l.ok) return l;
      return p2.evaluate(() => {
        const times = [], kinds = {};
        for (let i = 1; i <= 400; i++) {
          const s = window.__q(i), t = performance.now();
          const out = window.__m.infer(s);
          times.push(performance.now() - t);
          const k = JSON.parse(out).kind;
          kinds[k] = (kinds[k] || 0) + 1;
        }
        times.sort((a, b) => a - b);
        return { ok: true, p95: times[Math.ceil(0.95 * times.length) - 1], p50: times[times.length / 2], kinds };
      }).catch((e) => ({ ok: false, err: String(e) }));
    },
  };
}

try {
  const stress = await startStress();
  const proj = C.project("studio", [L0], [FIXTURE]);
  const port = await C.syncServe(proj);
  const peer = C.peer(port);
  const shapeIds = ["fx", "fy"];
  const shapes = () => shapeIds.map((id) => shape(peer.show(id)));
  let known = shapes();
  const pts = () => known.flatMap((s) => s.V);

  srv = await serve(path.join(ROOT, "apps/studio/dist"), { spa: true });
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1 });
  const page = await ctx.newPage();
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(`http://localhost:${srv.port}/?sync=ws://127.0.0.1:${port}`);
  await ready(page, "studio");
  for (const id of HOOKS) check(await page.locator(tid(id)).count() > 0, `missing data-testid="${id}"`);

  // --------------------------------------------- 1. zoom-extents frames the synced fixture; navigation
  let v = await zoomExtents(page, pts(), "key", "Shift+Z after the synced document loaded");
  const centre = [v.box.x + v.box.width / 2, v.box.y + v.box.height / 2];
  await page.mouse.move(...centre);
  for (let i = 0; i < 6; i++) { await page.mouse.wheel(0, 240); await sleep(40); }
  await sleep(300);
  const vz = await view(page);
  const fz = await frames(vz, pts()), f0 = await frames(v, pts());
  // fz is null when the zoom brought a fixture corner behind the eye: that is a (large) zoom too.
  check(dist(vz.raw.eye, v.raw.eye) > 1e-3 * dist(v.raw.eye, v.raw.target) && (!fz || Math.abs(fz.span - f0.span) > 0.1 * f0.span),
    "the mouse wheel over viewport-3d did not zoom (camera unchanged or model's screen size unchanged)");
  check(viewAngle(vz.raw, v.raw) < 0.01, "the mouse wheel changed the view direction (zoom must not rotate)");
  v = await zoomExtents(page, pts(), "key", "Shift+Z after zooming");
  for (let i = 0; i < 6; i++) { await page.mouse.wheel(0, -240); await sleep(40); }
  await sleep(200);
  v = await zoomExtents(page, pts(), "button", "the zoom-extents button");
  const drag = async (dx, dy, shift) => {
    await page.mouse.move(...centre);
    if (shift) await page.keyboard.down("Shift");
    await page.mouse.down({ button: "middle" });
    for (let i = 1; i <= 8; i++) { await page.mouse.move(centre[0] + dx * i / 8, centre[1] + dy * i / 8); await sleep(16); }
    await page.mouse.up({ button: "middle" });
    if (shift) await page.keyboard.up("Shift");
    await sleep(250);
    return view(page);
  };
  const vo = await drag(160, 50, false);
  check(viewAngle(vo.raw, v.raw) > 0.05, `middle-button drag did not orbit (view direction changed by ${(viewAngle(vo.raw, v.raw) * 180 / Math.PI).toFixed(2)} deg)`);
  const vp = await drag(120, 70, true);
  check(viewAngle(vp.raw, vo.raw) < 0.01 && dist(vp.raw.eye, vo.raw.eye) > 1e-3 * dist(vo.raw.eye, vo.raw.target),
    "Shift + middle-button drag did not pan (the eye must move and the view direction stay)");
  v = await zoomExtents(page, pts(), "key", "Shift+Z after orbit and pan");

  // ------------------------------------------------------------------------- 2. tool keys
  for (const [k, id] of [["l", "tool-line"], ["r", "tool-rect"], ["c", "tool-circle"], ["a", "tool-arc"], ["p", "tool-pushpull"],
    ["Space", "tool-select"], ["l", "tool-line"], ["s", "tool-select"]]) {
    await key(page, k);
    await expectTool(page, id, `key ${k}`);
  }
  await key(page, "Escape");

  // Ground points whose pixel sees no face and is clear of every edge and vertex (ground inference wins).
  const free = (vv, p, c = 22) => {
    const px = vv.cam.project(p);
    return onScreen(vv.cam, px, 40) && !raycast(known, vv.cam.ray(px)) &&
      edgeClearance(vv.cam, px, edgesOf(known)) >= c && vertexClearance(vv.cam, px, pts()) >= c;
  };
  // Ground points stay at least 0.5 m off the world x and y axes: a Studio may draw those axes permanently in
  // red and green, which must not be mistaken for (or hide) the axis inference guides checked below.
  const groundSearch = (vv, ok) => {
    const lo = bbox(pts()).lo, hi = bbox(pts()).hi;
    let best = null;
    for (let x = lo[0] - 3; x <= hi[0] + 3; x += 0.5) for (let y = lo[1] - 3; y <= hi[1] + 3; y += 0.5) {
      const p = [x, y, 0];
      if (Math.abs(x) < 0.5 || Math.abs(y) < 0.5 || !free(vv, p) || !ok(p)) continue;
      const q = vv.cam.project(p), dc = Math.hypot(q[0] - vv.cam.W / 2, q[1] - vv.cam.H / 2);
      if (!best || dc < best.dc) best = { p, dc };
    }
    return best && best.p;
  };
  const footprints = () => known.map((s) => bbox(s.V));
  const clearOf = (lo, hi, m = 0.6) => footprints().every((b) => hi[0] + m <= b.lo[0] || lo[0] - m >= b.hi[0] || hi[1] + m <= b.lo[1] || lo[1] - m >= b.hi[1]);

  // ----------------------------------------------------- 3. inference tip: Endpoint at a corner
  await key(page, "l");
  const corner = known[0].V.map((p) => ({ p, c: vertexClearance(v.cam, v.cam.project(p), pts(), p) }))
    .filter((o) => onScreen(v.cam, v.cam.project(o.p), 20)).sort((a, b) => b.c - a.c)[0];
  check(corner && corner.c >= 22, "judge: no fixture corner is clear of the others on screen");
  await hover(page, v, corner.p, [2, -2]);
  await expectTip(page, /endpoint/i, "hovering a box corner with the line tool");
  const G0 = groundSearch(v, () => true);
  check(G0, "judge: no free ground point on screen");
  await hover(page, v, G0);
  const t0 = await until(async () => (/endpoint/i.test(await tipText(page)) ? null : true), 2000, 80);
  check(t0, "inference-tip still says Endpoint on empty ground (the tip does not follow the pointer)");

  // ---------------------------- 4. axis inference from a point: tip, guide colours, snapped geometry
  const pxd = (vv, a, b) => { const p = vv.cam.project(a), q = vv.cam.project(b); return Math.hypot(p[0] - q[0], p[1] - q[1]); };
  const pick = (vv) => {
    for (const s of [3, 2.5, 2, 1.5]) {
      const G = groundSearch(vv, (g) => {
        const hx = add(g, mulv(X, s)), hy = add(g, mulv(Y, s)), hz = add(g, mulv(Z, s));
        const okp = (p) => { const q = vv.cam.project(p); return onScreen(vv.cam, q, 30) && edgeClearance(vv.cam, q, edgesOf(known)) >= 20 && vertexClearance(vv.cam, q, pts()) >= 20; };
        return okp(hx) && okp(hy) && okp(hz) &&
          lineDist(vv.cam, vv.cam.project(hy), g, X) >= 20 &&
          lineDist(vv.cam, vv.cam.project(hz), g, X) >= 20 && lineDist(vv.cam, vv.cam.project(hz), g, Y) >= 20 &&
          pxd(vv, hx, g) >= 50 && pxd(vv, hy, g) >= 50 && pxd(vv, hz, g) >= 50;
      });
      if (G) return { G, s };
    }
    return null;
  };
  // The x, y and z axes must look distinct on screen; orbit (middle drag) and reframe until they do.
  let ax = pick(v);
  for (let i = 0; i < 5 && !ax; i++) { await drag(110, i % 2 ? -25 : 25, false); v = await zoomExtents(page, pts(), "key", "Shift+Z"); ax = pick(v); }
  check(ax, "judge: no ground point with room for the axis checks on screen (after orbiting)");
  const { G, s } = ax;
  await park(page);
  const base = await shot(page, "viewport-3d");
  await clickAt(page, v, G);
  for (const [dir, re, pred, name] of [[X, /on red axis/i, RED, "red"], [Y, /on green axis/i, GREEN, "green"], [Z, /on blue axis/i, BLUE, "blue"]]) {
    const H = add(G, mulv(dir, s));
    await hover(page, v, H);
    await expectTip(page, re, `moving along ${name === "red" ? "x" : name === "green" ? "y" : "z"} from the line's first point`);
    const img = await shot(page, "viewport-3d");
    const a = v.cam.project(G), b = v.cam.project(H), L = Math.hypot(b[0] - a[0], b[1] - a[1]);
    // Count coloured pixels along the guide's line from 0.3 L before G to 0.3 L past H: a guide drawn as an
    // infinite axis line, a from-to-cursor segment, dotted, or partly covered by the rubber-band line all pass
    // (0.2 L coloured pixels in a 4 px band), while no guide fails.
    const e = [(b[0] - a[0]) / L, (b[1] - a[1]) / L];
    const a2 = [a[0] - 0.3 * L * e[0], a[1] - 0.3 * L * e[1]], b2 = [b[0] + 0.3 * L * e[0], b[1] + 0.3 * L * e[1]];
    const n = countAlong(img, a2, b2, pred) - countAlong(base, a2, b2, pred);
    check(n >= 0.2 * L, `the ${name} axis guide is not drawn in ${name} (${n} ${name} pixels along a ${L.toFixed(0)} px guide)`);
  }
  const HX = add(G, mulv(X, s));
  await hover(page, v, HX);
  await expectTip(page, /on red axis/i, "back on the x axis");
  peer.mark();
  await clickAt(page, v, HX);
  await key(page, "Escape");
  let line = null;
  await until(() => {
    const ch = peer.mark();
    for (const id of ch.added) {
      const e = peer.show(id);
      if (e.kind === "Shape" && (e.props.edges || []).length >= 2) { line = shape(e); shapeIds.push(id); return true; }
    }
    return false;
  }, 10_000, 400);
  check(line, "clicking two points with the line tool created no Shape with a loose edge (read through bim sync)");
  known = shapes();
  const seg = line.loose.find(([a, b]) => Math.abs(a[1] - b[1]) < 1e-9 && Math.abs(a[2] - b[2]) < 1e-9 && Math.abs(Math.abs(a[0] - b[0]) - s) < 0.15 * s);
  check(seg, `the line drawn on the red axis is not exactly parallel to x with length ~${s} m: ${JSON.stringify(line.loose)}`);
  const pg = v.cam.project(G);
  const near = seg.map((e) => { const q = v.cam.project(e); return Math.hypot(q[0] - pg[0], q[1] - pg[1]); });
  check(Math.min(...near) < 3 && Math.abs(seg[near[0] < near[1] ? 0 : 1][2]) < 1e-9, `the line does not start at the clicked ground point (${JSON.stringify(seg)})`);

  // ------------------------------------------------- 5. Esc cancels mid-tool; invalid input is refused
  v = await view(page);
  const h0 = peer.mark() && peer.hash();
  const H1 = groundSearch(v, () => true);
  await key(page, "r");
  await clickAt(page, v, H1);
  await hover(page, v, add(H1, [1.5, 1, 0]));
  await page.keyboard.press("Escape");
  await sleep(200);
  await clickAt(page, v, add(H1, [2, 1.5, 0])); // after Esc this click may only start a new rectangle
  await page.keyboard.press("Escape");
  await key(page, "p");
  const fpF = facePoint(v.cam, known, "fx", known[0].faces.reduce((m, f) => (faceCentroid(f)[2] > faceCentroid(m)[2] ? f : m)).id);
  check(fpF, "judge: no clear visible point on the fixture's top face");
  await clickAt(page, v, fpF.p);
  await hover(page, v, add(fpF.p, [0, 0, 1]));
  await page.keyboard.press("Escape");
  await sleep(200);
  await clickAt(page, v, add(fpF.p, [0, 0, 1])); // after Esc this click finishes nothing
  await page.keyboard.press("Escape");
  await key(page, "r");
  await clickAt(page, v, H1);
  await hover(page, v, add(H1, [1.5, 1, 0]));
  await page.keyboard.type("abc", { delay: 25 });
  await page.keyboard.press("Enter");
  await sleep(300);
  await page.keyboard.press("Escape");
  await key(page, "Escape");
  await sleep(1500);
  const ch5 = peer.mark();
  check(peer.hash() === h0 && ch5.added.length === 0 && ch5.changed.length === 0,
    `Esc mid-tool or an invalid typed value changed the document (added ${JSON.stringify(ch5.added)}, changed ${JSON.stringify(ch5.changed.map((c) => c.id))})`);

  // ----------------------------------------------- 6. rectangle by click + typed '4000,3000' Enter
  const newShape = async (what) => {
    let got = null;
    await until(() => {
      const ch = peer.mark();
      for (const id of ch.added) { const e = peer.show(id); if (e.kind === "Shape" && (e.props.faces || []).length) { got = id; return true; } }
      return false;
    }, 10_000, 400);
    check(got, `${what} created no Shape with a face (read through bim sync)`);
    shapeIds.push(got);
    known = shapes();
    return known.find((x) => x.id === got);
  };
  const changed = async (id, prev, what) => {
    let cur = null;
    await until(() => { peer.mark(); const e = peer.show(id); if (JSON.stringify(e.props) !== JSON.stringify(prev.props)) { cur = shape(e); return true; } return false; }, 10_000, 400);
    check(cur, `${what} did not change Shape ${id} (read through bim sync)`);
    known = shapes();
    return cur;
  };
  /** The Studio's result equals `bim sketch` of one of `cands` (command lists) replayed on `pre`. */
  const equalsCli = (got, pre, cands, what) => {
    const why = [];
    for (const cmds of cands) {
      const want = shape(C.replay(pre ? [pre] : [], cmds, [got.id])[got.id]);
      const r = compareShapes(got, want, pre ? pre.props.face_ids : []);
      if (!r) return cmds;
      why.push(`${JSON.stringify(cmds)}: ${r}`);
    }
    fail(`${what}: the Studio's Shape ${got.id} differs from the bim sketch result (${why[0]})`);
  };
  v = await view(page);
  // Either side may be the 4000 one (spec/sketch.md fixes no order), so the clear region is 4 x 4 m.
  const RH = groundSearch(v, (p) => clearOf(p, add(p, [4, 4, 0])) && onScreen(v.cam, v.cam.project(add(p, [4, 4, 0])), 30));
  check(RH, "judge: no free 4 x 4 m ground region on screen");
  await key(page, "r");
  await clickAt(page, v, RH);
  await hover(page, v, add(RH, [2, 1.5, 0]));
  await typeValue(page, "4000,3000", "rectangle");
  const R1 = await newShape("rectangle + typed 4000,3000");
  check(R1.faces.length === 1 && R1.faces[0].loops.length === 1 && R1.faces[0].loops[0].length === 4, `the rectangle Shape has ${R1.faces.length} faces / not one 4-vertex loop`);
  const q = R1.faces[0].loops[0];
  const sides = q.map((p, k) => dist(p, q[(k + 1) % 4]));
  check([0, 1, 2, 3].every((k) => Math.abs(dot(sub(q[(k + 1) % 4], q[k]), sub(q[(k + 2) % 4], q[(k + 1) % 4]))) < 1e-9) &&
    Math.abs(sides[0] - sides[2]) < 1e-9 && Math.abs(sides[1] - sides[3]) < 1e-9 &&
    Math.abs(Math.min(sides[0], sides[1]) - 3) < 1e-9 && Math.abs(Math.max(sides[0], sides[1]) - 4) < 1e-9,
  `typed 4000,3000 did not give a 4 x 3 m rectangle (sides ${sides.map((x) => x.toFixed(6)).join(", ")})`);
  const rectCands = [];
  for (let k = 0; k < 4; k++) for (const o of [1, -1]) {
    const c0 = q[k], a = q[(k + o + 4) % 4], b = q[(k - o + 4) % 4];
    rectCands.push([{ op: "rect", shape: R1.id, corner: c0, u: sub(a, c0), v: sub(b, c0) }]);
  }
  equalsCli(R1, null, rectCands, "rectangle");

  // ------------------------------------------------------- 7. push/pull with typed 2500 -> 30 m3
  const pushpull = async (target, faceId, lift, typed, what, { ctrl = false, dbl = false } = {}) => {
    v = await view(page);
    const fp = facePoint(v.cam, known, target.id, faceId);
    check(fp, `judge: no clear visible point on face ${faceId} of ${target.id}`);
    await key(page, "p");
    if (dbl) { await dblclickAt(page, v, fp.p); return; }
    await clickAt(page, v, fp.p);
    if (ctrl) await page.keyboard.press("ControlOrMeta");
    await hover(page, v, add(fp.p, [0, 0, lift]));
    await typeValue(page, typed, what);
  };
  await pushpull(R1, R1.faces[0].id, 1.2, "2500", "push/pull");
  const R1b = await changed(R1.id, R1, "push/pull + typed 2500");
  const z0 = q[0][2], zb = bbox(R1b.V);
  check(closedManifold(R1b), "push/pull of the 4 x 3 m face by 2500 did not give a closed shape");
  check(Math.abs(Math.abs(volume(R1b)) - 30) < 1e-6, `push/pull 2500 of the 4 x 3 m face gives ${Math.abs(volume(R1b)).toFixed(6)} m3, want 30`);
  check(Math.abs(zb.lo[2] - z0) < 1e-9 && Math.abs(zb.hi[2] - z0 - 2.5) < 1e-9, `push/pull did not extrude 2.5 m towards the pointer (z ${zb.lo[2]}..${zb.hi[2]})`);
  equalsCli(R1b, R1, [2.5, -2.5].map((d) => [{ op: "pushpull", shape: R1.id, face: R1.faces[0].id, distance: d }]), "push/pull 2500");

  // ------------------------------------------------- 8. double-click with push/pull repeats 2.5 m
  v = await view(page);
  const RH2 = groundSearch(v, (p) => clearOf(p, add(p, [2, 2, 0])) && onScreen(v.cam, v.cam.project(add(p, [2, 2, 0])), 30));
  check(RH2, "judge: no free 2 x 2 m ground region on screen");
  await key(page, "r");
  await clickAt(page, v, RH2);
  await hover(page, v, add(RH2, [1, 0.5, 0]));
  await typeValue(page, "2000,1000", "second rectangle");
  const R2 = await newShape("second rectangle + typed 2000,1000");
  check(Math.abs(faceArea(R2.faces[0]) - 2) < 1e-9, `typed 2000,1000 gave a face of ${faceArea(R2.faces[0])} m2, want 2`);
  await pushpull(R2, R2.faces[0].id, 0, "", "double-click", { dbl: true });
  const R2b = await changed(R2.id, R2, "double-click with push/pull");
  const zb2 = bbox(R2b.V);
  check(closedManifold(R2b) && Math.abs(Math.abs(volume(R2b)) - 5) < 1e-6 && Math.abs(zb2.hi[2] - zb2.lo[2] - 2.5) < 1e-9,
    `double-click with push/pull did not repeat 2.5 m on the 2 x 1 m face (closed ${closedManifold(R2b)}, volume ${Math.abs(volume(R2b)).toFixed(6)})`);
  equalsCli(R2b, R2, [2.5, -2.5].map((d) => [{ op: "pushpull", shape: R2.id, face: R2.faces[0].id, distance: d }]), "double-click push/pull");
  await key(page, "Escape");

  // --------------------------------------- 9. Ctrl/Cmd during push/pull keeps the face (new: true)
  const topF = R1b.faces.reduce((m, f) => (faceCentroid(f)[2] > faceCentroid(m)[2] ? f : m));
  await pushpull(R1b, topF.id, 1, "1000", "push/pull with Ctrl/Cmd", { ctrl: true });
  const R1c = await changed(R1.id, R1b, "push/pull with Ctrl/Cmd + typed 1000");
  check(R1c.faces.length === R1b.faces.length + 5, `push/pull with Ctrl/Cmd should keep the start face and add 5 faces (${R1b.faces.length} -> ${R1c.faces.length})`);
  check(Math.abs(bbox(R1c.V).hi[2] - zb.hi[2] - 1) < 1e-9, "push/pull with Ctrl/Cmd did not pull the new face 1 m towards the pointer");
  equalsCli(R1c, R1b, [1, -1].map((d) => [{ op: "pushpull", shape: R1.id, face: topF.id, distance: d, new: true }]), "push/pull with Ctrl/Cmd");

  check(errors.length === 0, `page errors: ${errors.slice(0, 3).join(" | ")}`);
  await ctx.close();

  // ------------------------------------- 10. Model.infer latency in Chrome on a 100,000-Shape model
  const r = await stress.timed();
  check(r.ok, `Model.infer latency page failed: ${r.err}`);
  const hits = (r.kinds.endpoint || 0) + (r.kinds.midpoint || 0) + (r.kinds.on_edge || 0) + (r.kinds.on_face || 0);
  check(hits >= 120, `Model.infer found almost no geometry on the 100,000-Shape model (kinds ${JSON.stringify(r.kinds)})`);
  check(r.p95 < 8, `Model.infer p95 ${r.p95.toFixed(2)} ms >= 8 ms per query on the 100,000-Shape model (p50 ${r.p50.toFixed(2)} ms)`);
  console.log(`T8.6 judge: all checks passed (inference p95 ${r.p95.toFixed(2)} ms, kinds ${JSON.stringify(r.kinds)})`);
} catch (e) {
  code = 1;
  console.error(e instanceof JudgeFail ? `JUDGE FAIL: ${e.message}` : `JUDGE FAIL: unexpected error: ${e && e.stack || e}`);
  if (errors.length) console.error(`page errors: ${errors.slice(0, 5).join(" | ")}`);
} finally {
  await browser.close().catch(() => {});
  if (srv) await srv.close();
  if (srv2) await srv2.close();
}
process.exit(code);
