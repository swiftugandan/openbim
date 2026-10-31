// External judge for T8.7 (Studio direct modelling II): move/copy/array, rotate, scale, offset,
// follow-me, tape, eraser, paint, make-component, component editing and scenes, driven in headless system
// Chrome through the spec/web.md hooks, the mouse and the keyboard. The document lives in a real
// `bim sync serve`; a CLI peer reads what the Studio wrote, and every result is compared with the
// corresponding `bim sketch` command (or `bim catalog use`) replayed on the Studio's pre-state.
// Pointer positions come from the camera the Studio reports (viewport-3d data-camera).
// Usage: node run.mjs <bim>
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { oracle, CHROME, ROOT } from "../lib/oracle.mjs";
import { serve, deadline } from "./serve.mjs";
import { JudgeFail, fail, check, tid, sleep, until, ready, park, shot, diff, reveal } from "./studio.mjs";
import { cli } from "./peer.mjs";
import {
  shape, bbox, edgesOf, facePoint, faceCentroid, raycast, edgeClearance, vertexClearance, onScreen, compareShapes,
  viewAngle, add, mulv, sub, dist, translation,
} from "./geom.mjs";
import { view, hover, clickAt, expectTip, typeValue, key, expectTool, zoomExtents } from "./sketch.mjs";

deadline(140_000, "T8.7 browser checks");
const BIM = process.argv[2];
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "judge-t87-"));
const C = cli(BIM, tmp);
process.on("exit", () => { C.kill(); fs.rmSync(tmp, { recursive: true, force: true }); });
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(sig, () => process.exit(1));

const HOOKS = ["viewport-3d", "status-bar", "tool-select", "tool-pushpull", "tool-line", "tool-move", "tool-rotate", "tool-scale",
  "tool-offset", "tool-followme", "tool-tape", "tool-eraser", "tool-paint", "make-component", "scenes", "measurements",
  "inference-tip", "catalog-import", "catalog-products"];
const ref = (id) => ({ ref: id });
const box = (id, corner, [a, b, h]) => [
  { op: "rect", shape: id, corner, u: [a, 0, 0], v: [0, b, 0] },
  { op: "pushpull", shape: id, face: 1, distance: h },
];
const CATALOG = {
  catalog: "acme", version: "2026.1", manufacturer: "Acme Building Products Ltd",
  products: [
    { sku: "EM-WHT", category: "wall_finish", name: "Matt emulsion, white", props: { colour: "white", coats: 2 } },
    { sku: "OAK-14", category: "floor_finish", name: "Engineered oak 14 mm", props: { thickness: 0.014 } },
  ],
};
const OPS0 = [{ op: "create", id: "L0", kind: "Level", props: { elevation: 0 } }];
const SKETCH1 = [
  ...box("mv", [0, -10, 0], [2, 2, 0.5]),
  ...box("rt", [0, 0, 0], [2, 2, 0.5]),
  ...box("sc", [5, 0, 0], [2, 2, 0.5]),
  ...box("of", [12, 0, 0], [3, 3, 0.5]),
  // Follow-me profile and path: the first segment (3 m) is longer than the profile's 2 m extent in the turn
  // direction, so the 45-degree mitre plane at the corner clears the start cap (no degenerate sweep).
  { op: "rect", shape: "fm", corner: [18, 0, 0], u: [2, 0, 0], v: [0, 2, 0] },
  { op: "line", shape: "fmpath", from: [18, 0, 0], to: [18, 0, 3] },
  { op: "line", shape: "fmpath", from: [18, 0, 3], to: [21, 0, 3] },
  { op: "rect", shape: "er", corner: [0, 6, 0], u: [3, 0, 0], v: [0, 2, 0] },
  { op: "line", shape: "er", from: [1.5, 6, 0], to: [1.5, 8, 0] },
  ...box("pt", [6, 6, 0], [2, 2, 0.5]),
  ...box("mk", [0, 0, 0], [2, 2, 0.5]),
  ...box("cs", [0, 0, 0], [2, 2, 0.5]),
];
const OPS2 = [
  { op: "set", id: "mk", key: "transform", value: translation(11, 6, 0) },
  { op: "set", id: "cs", key: "transform", value: translation(15, 6, 0) },
];
const SKETCH3 = [{ op: "make_component", shape: "cs", definition: "cd", instance: "ci1", name: "Unit" }];
const OPS4 = [{ op: "create", id: "ci2", kind: "ComponentInstance", props: { definition: ref("cd"), transform: translation(18.5, 6, 0) } }];

const { chromium } = await oracle("playwright-core");
const { AxeBuilder } = await oracle("@axe-core/playwright");
const browser = await chromium.launch({ executablePath: CHROME, headless: true, args: ["--enable-unsafe-webgpu"] });
const errors = [];
let srv = null;
let code = 0;
try {
  // The fixture is built with the CLI itself (ops, sketch commands, make_component), then served.
  const proj = C.project("studio", OPS0, [SKETCH1]);
  C.out(["apply", proj, C.file("ops2", { schema: 1, ops: OPS2 })]);
  C.out(["sketch", proj, C.file("sk3", { schema: 1, commands: SKETCH3 })]);
  C.out(["apply", proj, C.file("ops4", { schema: 1, ops: OPS4 })]);
  const port = await C.syncServe(proj);
  const peer = C.peer(port);
  // Placed geometry: Shapes and ComponentInstances (an instance = its definition's Shape under its transform).
  const placed = ["mv", "rt", "sc", "of", "fm", "fmpath", "er", "pt", "mk", "ci1", "ci2"];
  const world = () => placed.map((id) => {
    const e = peer.show(id);
    if (e.kind !== "ComponentInstance") return shape(e);
    const s = peer.show(peer.show(e.props.definition.ref).props.shape.ref);
    return shape({ id, kind: e.kind, props: { ...s.props, transform: e.props.transform } });
  });
  let known = world();
  const W = (id) => known.find((s) => s.id === id);
  const pts = () => known.flatMap((s) => s.V);
  const topFace = (s) => s.faces.reduce((m, f) => (faceCentroid(f)[2] > faceCentroid(m)[2] ? f : m));
  const vIndex = (s, p) => { const i = s.V.findIndex((q) => dist(q, p) < 1e-9); check(i >= 0, `fixture Shape ${s.id} has no vertex at ${p}`); return i; };

  srv = await serve(path.join(ROOT, "apps/studio/dist"), { spa: true });
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1 });
  const page = await ctx.newPage();
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(`http://localhost:${srv.port}/?sync=ws://127.0.0.1:${port}`);
  await ready(page, "studio");
  for (const id of HOOKS) check(await page.locator(tid(id)).count() > 0, `missing data-testid="${id}"`);
  let v = await zoomExtents(page, pts(), "key", "Shift+Z after the synced document loaded");

  // ------------------------------------------------------------------------- 1. tool keys
  for (const [k, id] of [["m", "tool-move"], ["q", "tool-rotate"], ["Shift+S", "tool-scale"], ["f", "tool-offset"], ["t", "tool-tape"],
    ["e", "tool-eraser"], ["b", "tool-paint"], ["s", "tool-select"]]) {
    await key(page, k);
    await expectTool(page, id, `key ${k}`);
  }
  await key(page, "Escape");

  // Helpers --------------------------------------------------------------------------------------
  const centre = () => [v.box.x + v.box.width / 2, v.box.y + v.box.height / 2];
  const orbit = async (dx) => {
    const [cx, cy] = centre();
    await page.mouse.move(cx, cy);
    await page.mouse.down({ button: "middle" });
    for (let i = 1; i <= 8; i++) { await page.mouse.move(cx + dx * i / 8, cy + 10 * i / 8); await sleep(16); }
    await page.mouse.up({ button: "middle" });
    await sleep(250);
  };
  /** Reframe (and orbit if needed) until `plan(view)` finds pointer positions; returns its result. */
  let panSign = 1, zoomSign = -1;
  /** Pan (Shift + middle drag) and wheel-zoom so world point `p` sits near the centre at a larger scale,
   * checking `plan` after every step; the directions are learnt from data-camera. Returns plan's result or null. */
  const focus = async (p, plan) => {
    const scale = (vv) => { const a = vv.cam.project(p), b = vv.cam.project(add(p, [0.5, 0, 0])), c = vv.cam.project(add(p, [0, 0.5, 0])); return a && b && c ? Math.max(Math.hypot(b[0] - a[0], b[1] - a[1]), Math.hypot(c[0] - a[0], c[1] - a[1])) : 0; };
    for (let i = 0; i < 12; i++) {
      const r = plan(v);
      if (r) return r;
      const q = v.cam.project(p), [cx, cy] = centre();
      if (!q) return null;
      const off = Math.hypot(q[0] + v.box.x - cx, q[1] + v.box.y - cy);
      if (off > 0.15 * v.box.width) {
        const dx = (cx - v.box.x - q[0]) * panSign, dy = (cy - v.box.y - q[1]) * panSign;
        await page.mouse.move(cx, cy);
        await page.keyboard.down("Shift");
        await page.mouse.down({ button: "middle" });
        for (let k = 1; k <= 6; k++) { await page.mouse.move(cx + dx * k / 6, cy + dy * k / 6); await sleep(16); }
        await page.mouse.up({ button: "middle" });
        await page.keyboard.up("Shift");
        await sleep(200);
        const nv = await view(page), nq = nv.cam.project(p);
        if (nq && Math.hypot(nq[0] + nv.box.x - cx, nq[1] + nv.box.y - cy) > off) panSign = -panSign;
        v = nv;
      } else {
        const s0 = scale(v);
        await page.mouse.move(q[0] + v.box.x, q[1] + v.box.y);
        await page.mouse.wheel(0, 240 * zoomSign);
        await sleep(200);
        const nv = await view(page);
        if (scale(nv) < s0) zoomSign = -zoomSign;
        v = nv;
      }
    }
    return plan(v);
  };
  /** Reframe, then pan/zoom towards `at` and orbit if needed, until `plan(view)` finds pointer positions. */
  const arrange = async (plan, what, at = null) => {
    v = await zoomExtents(page, pts(), "key", "Shift+Z");
    let r = plan(v);
    for (let i = 0; i < 4 && !r; i++) {
      if (at) r = await focus(at, plan);
      if (r) break;
      await orbit(i % 2 ? -90 : 130);
      v = await zoomExtents(page, pts(), "key", "Shift+Z");
      r = plan(v);
    }
    check(r, `judge: no clear pointer positions for ${what} on screen (after panning, zooming and orbiting)`);
    return r;
  };
  const freeAt = (vv, p, c = 20, except = null) => {
    const q = vv.cam.project(p);
    return onScreen(vv.cam, q, 30) && edgeClearance(vv.cam, q, edgesOf(known).filter((e) => !except || !e.some((x) => dist(x, except) < 1e-9))) >= c &&
      vertexClearance(vv.cam, q, pts(), except) >= c;
  };
  const cornerAt = (vv, p, c = 10) => { const q = vv.cam.project(p); return onScreen(vv.cam, q, 30) && vertexClearance(vv.cam, q, pts(), p) >= c; };
  const groundFree = (vv, p) => freeAt(vv, p) && !raycast(known, vv.cam.ray(vv.cam.project(p)));
  const FP = { clear: 12 };
  /** Arrange a view where the top of `id` has a clear visible point, then click it with tool-select. */
  const select = async (id, also = () => true) => {
    await key(page, "Escape");
    const r = await arrange((vv) => { const fp = facePoint(vv.cam, known, id, topFace(W(id)).id, FP); return fp && also(vv) ? fp : null; }, `selecting ${id}`, faceCentroid(topFace(W(id))));
    await key(page, "Space");
    await clickAt(page, v, r.p);
    await sleep(150);
  };
  /** Wait until entity `id` differs from `prev` (and `ok(show)` holds); returns the new `bim show`. */
  const changed = async (id, prev, what, ok = () => true) => {
    let cur = null;
    await until(() => { peer.mark(); const e = peer.show(id); if (JSON.stringify(e.props) !== JSON.stringify(prev.props) && ok(e)) { cur = e; return true; } return false; }, 10_000, 400);
    check(cur, `${what} did not change ${id} as expected (read through bim sync)`);
    return cur;
  };
  const expectCli = (got, pre, cmds, what, opts = {}) => {
    const want = shape(C.replay(pre.extra ? [...pre.extra, pre.show] : [pre.show], cmds, [got.id], opts)[got.id]);
    const r = compareShapes(shape(got), want, pre.show.props.face_ids);
    if (r) fail(`${what}: the Studio's ${got.id} differs from 'bim sketch' ${JSON.stringify(cmds[0]).slice(0, 160)}: ${r}`);
  };
  peer.mark();

  // ------------------------------------------------------- 2. tape measure creates a Guide point
  const tapePlan = await arrange((vv) => {
    for (const c of [[6, 6, 0], [8, 6, 0], [6, 8, 0], [8, 8, 0]]) for (const d of [[-1.5, 0, 0], [1.5, 0, 0], [0, -1.5, 0], [0, 1.5, 0]]) {
      const e = add(c, d);
      if (cornerAt(vv, c, 10) && groundFree(vv, e) && freeAt(vv, add(c, mulv(d, 0.6)), 14)) return { c, e };
    }
    return null;
  }, "the tape measure", [7, 7, 0]);
  await key(page, "t");
  await clickAt(page, v, tapePlan.c);
  await hover(page, v, tapePlan.e);
  await expectTip(page, /on (red|green) axis/i, "tape measure pointer along an axis");
  await typeValue(page, "1500", "tape measure");
  let guide = null;
  await until(() => { const ch = peer.mark(); for (const id of ch.added) { const e = peer.show(id); if (e.kind === "Guide") { guide = e; return true; } } return false; }, 10_000, 400);
  check(guide, "the tape measure created no Guide entity (read through bim sync)");
  const gp = guide.props.point;
  check(Array.isArray(gp) && gp.length === 3 && dist(gp, tapePlan.e) < 1e-9, `the tape Guide is at ${JSON.stringify(gp)}, want ${JSON.stringify(tapePlan.e)} (1500 mm from the corner along the pointer)`);
  await key(page, "Escape");
  await key(page, "l");
  await hover(page, v, tapePlan.e);
  await expectTip(page, /guide/i, "hovering the new Guide point with the line tool");
  await key(page, "Escape");

  // --------------------------------------- 3. paint applies the selected catalogue product to a face
  const catFile = path.join(tmp, "catalog.json");
  fs.writeFileSync(catFile, JSON.stringify(CATALOG));
  await reveal(page, "catalog-import").catch(() => {});
  await page.locator(tid("catalog-import")).first().setInputFiles(catFile);
  const prod = page.locator(`${tid("catalog-products")} ${tid("product-acme:EM-WHT")}`).first();
  check(await until(() => prod.count(), 5000), "importing catalog.json did not list product-acme:EM-WHT in catalog-products");
  await reveal(page, "catalog-products").catch(() => {});
  await prod.scrollIntoViewIfNeeded().catch(() => {});
  check(await until(() => prod.isVisible().catch(() => false), 5000), "product-acme:EM-WHT in catalog-products is not visible (and no tab reveals it)");
  await prod.click();
  const ptPre = peer.show("pt"), ptTop = topFace(W("pt")).id;
  const fpP = await arrange((vv) => facePoint(vv.cam, known, "pt", ptTop, FP), "painting the top of pt", [7, 7, 0.5]);
  await key(page, "b");
  await clickAt(page, v, fpP.p);
  const ptNow = await changed("pt", ptPre, "paint");
  const scratchCatalog = (d) => {
    fs.mkdirSync(path.join(d, "catalogs", "acme"), { recursive: true });
    fs.writeFileSync(path.join(d, "catalogs", "acme", "catalog.json"), JSON.stringify(CATALOG));
    C.out(["catalog", "use", d, "acme:EM-WHT"]);
  };
  const prodRef = C.project("catref");
  scratchCatalog(prodRef);
  const strip = (props) => Object.fromEntries(Object.entries(props).filter(([k]) => !k.startsWith("_")));
  const wantProd = strip(C.show(prodRef, "acme:EM-WHT").props);
  let gotProd = null;
  try { gotProd = strip(peer.show("acme:EM-WHT").props); } catch { gotProd = null; }
  check(gotProd && JSON.stringify(Object.entries(gotProd).sort()) === JSON.stringify(Object.entries(wantProd).sort()),
    `painting did not create Product acme:EM-WHT exactly as 'bim catalog use' does (got ${JSON.stringify(gotProd)}, want ${JSON.stringify(wantProd)})`);
  expectCli(ptNow, { show: ptPre }, [{ op: "paint", shape: "pt", faces: [ptTop], product: "acme:EM-WHT" }], "paint", { setup: scratchCatalog });
  known = world();

  // ------------------------------------------------- 4. eraser merges two coplanar faces
  const erPre = peer.show("er"), erS = W("er");
  check(erS.faces.length === 2, `fixture er should have two faces after the splitting line, has ${erS.faces.length}`);
  const mid = [1.5, 7, 0];
  await arrange((vv) => (vertexClearance(vv.cam, vv.cam.project(mid), pts()) >= 14 && onScreen(vv.cam, vv.cam.project(mid), 30) ? true : null), "the eraser", mid);
  await key(page, "e");
  await clickAt(page, v, mid);
  const erNow = await changed("er", erPre, "eraser on the splitting edge");
  const erGot = shape(erNow);
  check(erGot.faces.length === 1 && erGot.faces[0].id === Math.min(...erS.faces.map((f) => f.id)),
    `erasing the edge between two coplanar faces should leave one face with the lower id (faces ${JSON.stringify(erGot.faces.map((f) => f.id))})`);
  expectCli(erNow, { show: erPre }, [{ op: "erase", shape: "er", edges: [[vIndex(erS, [1.5, 6, 0]), vIndex(erS, [1.5, 8, 0])]] }], "eraser");
  known = world();

  // ------------------------------------------------------------- 5. offset with typed 300 (inward)
  const ofPre = peer.show("of"), ofTop = topFace(W("of"));
  const ofC = faceCentroid(ofTop);
  const fpO = await arrange((vv) => { const f = facePoint(vv.cam, known, "of", ofTop.id, { ...FP, score: (p) => dist(p, ofC) }); return f && dist(f.p, ofC) > 0.6 ? f : null; }, "offset", ofC);
  await key(page, "f");
  await clickAt(page, v, fpO.p);
  await hover(page, v, add(fpO.p, mulv(sub(ofC, fpO.p), 0.6)));
  await typeValue(page, "300", "offset");
  const ofNow = await changed("of", ofPre, "offset + typed 300");
  expectCli(ofNow, { show: ofPre }, [{ op: "offset", shape: "of", face: ofTop.id, distance: 0.3 }], "offset 300 towards the face centre");
  known = world();

  // ---------------------------------------------------- 6. rotate the selected Shape by typed 90
  const rtPlan = await arrange((vv) => {
    const c = [0, 0, 0], r = [0, -1.5, 0], p = [1.5 * Math.sin(0.7), -1.5 * Math.cos(0.7), 0];
    return cornerAt(vv, c, 10) && groundFree(vv, r) && groundFree(vv, p) ? { c, r, p } : null;
  }, "rotate", [0, 0, 0]);
  const rtPre = peer.show("rt");
  await select("rt", (vv) => cornerAt(vv, rtPlan.c, 10) && groundFree(vv, rtPlan.r) && groundFree(vv, rtPlan.p));
  await key(page, "q");
  await clickAt(page, v, rtPlan.c);
  await clickAt(page, v, rtPlan.r);
  await hover(page, v, rtPlan.p);
  await typeValue(page, "90", "rotate");
  const rtNow = await changed("rt", rtPre, "rotate + typed 90");
  expectCli(rtNow, { show: rtPre }, [{ op: "rotate", shape: "rt", faces: rtPre.props.face_ids, centre: [0, 0, 0], axis: [0, 0, 1], angle: Math.PI / 2 }],
    "rotate 90 degrees (counter-clockwise from above, towards the pointer)");
  known = world();

  // ------------------------------------------------------------ 7. Shift+S scale by typed 2
  const scPre = peer.show("sc");
  await select("sc", (vv) => cornerAt(vv, [5, 0, 0], 10));
  await key(page, "Shift+S");
  await clickAt(page, v, [5, 0, 0]);
  await typeValue(page, "2", "scale");
  const scNow = await changed("sc", scPre, "scale + typed 2");
  expectCli(scNow, { show: scPre }, [{ op: "scale", shape: "sc", faces: scPre.props.face_ids, origin: [5, 0, 0], factors: [2, 2, 2] }], "scale by 2");
  known = world();

  // -------------------------------------- 8. move with Ctrl/Cmd copy, then 'x5' for an array of 5
  const mvPre = peer.show("mv"), nf = mvPre.props.face_ids.length;
  await select("mv", (vv) => cornerAt(vv, [0, -10, 0], 10) && groundFree(vv, [3.5, -10, 0]));
  await key(page, "m");
  await clickAt(page, v, [0, -10, 0]);
  await page.keyboard.press("ControlOrMeta");
  await hover(page, v, [3.5, -10, 0]);
  await expectTip(page, /on red axis/i, "move pointer along x");
  await typeValue(page, "3000", "move copy");
  await changed("mv", mvPre, "move with Ctrl/Cmd + typed 3000", (e) => e.props.face_ids.length === 2 * nf);
  await typeValue(page, "x5", "array after a copy");
  const mvNow = await changed("mv", mvPre, "typing x5 after the copy", (e) => e.props.face_ids.length === 6 * nf);
  expectCli(mvNow, { show: mvPre }, [{ op: "move", shape: "mv", faces: mvPre.props.face_ids, vector: [3, 0, 0], copy: true, array: 5 }], "move copy x5");
  known = world();

  // --------------------------------------------------------------------------- 9. follow-me
  const path3 = [[18, 0, 0], [18, 0, 3], [21, 0, 3]];
  const fmPre = peer.show("fm"), fmFace = W("fm").faces[0].id;
  const fpM = await arrange((vv) => (path3.every((p) => cornerAt(vv, p, 10)) ? facePoint(vv.cam, known, "fm", fmFace, FP) : null), "follow-me", [19, 0, 1.5]);
  await key(page, "Escape");
  await page.locator(tid("tool-followme")).first().click();
  await clickAt(page, v, fpM.p);
  for (const p of path3) await clickAt(page, v, p);
  await page.keyboard.press("Enter");
  const fmNow = await changed("fm", fmPre, "follow-me (face, three path points, Enter)");
  expectCli(fmNow, { show: fmPre }, [{ op: "followme", shape: "fm", face: fmFace, path: path3 }], "follow-me");
  known = world();

  // ------------------------------------------------------- 10. make-component from the selection
  const mkPre = peer.show("mk");
  await select("mk");
  await page.locator(tid("make-component")).first().click();
  let def = null, inst = null;
  await until(() => {
    const ch = peer.mark();
    for (const id of ch.added) { const e = peer.show(id); if (e.kind === "ComponentDefinition") def = e; if (e.kind === "ComponentInstance") inst = e; }
    return def && inst;
  }, 10_000, 400);
  check(def && inst, "make-component did not create a ComponentDefinition and a ComponentInstance (read through bim sync)");
  check(typeof def.props.name === "string", `the ComponentDefinition has no Text name: ${JSON.stringify(def.props)}`);
  {
    const r = C.replay([mkPre], [{ op: "make_component", shape: "mk", definition: def.id, instance: inst.id, name: def.props.name }], ["mk", def.id, inst.id]);
    const r1 = compareShapes(shape(peer.show("mk")), shape(r.mk), mkPre.props.face_ids);
    check(!r1, `after make-component Shape mk differs from 'bim sketch' make_component: ${r1}`);
    // `level` is an optional ComponentInstance prop the Studio may set (spec/sketch.md); the replay project has
    // no Level, so it is left out of the comparison like the reserved props.
    const strip = (props) => Object.fromEntries(Object.entries(props).filter(([k]) => !k.startsWith("_") && k !== "level"));
    for (const [g, w] of [[def, r[def.id]], [inst, r[inst.id]]]) {
      const a = JSON.stringify(strip(g.props)), b = JSON.stringify(strip(w.props));
      const close = (x, y) => JSON.stringify(Object.keys(x).sort()) === JSON.stringify(Object.keys(y).sort()) && Object.keys(x).every((k) =>
        Array.isArray(x[k]) && Array.isArray(y[k]) ? x[k].length === y[k].length && x[k].every((n, i) => Math.abs(n - y[k][i]) < 1e-9) : JSON.stringify(x[k]) === JSON.stringify(y[k]));
      check(close(strip(g.props), strip(w.props)), `make-component ${g.kind} ${g.id} is ${a}, 'bim sketch' make_component gives ${b}`);
    }
  }
  placed[placed.indexOf("mk")] = inst.id;
  known = world();

  // -------------------------- 11. push/pull on one instance edits the definition: every instance changes
  const csPre = peer.show("cs"), ci2 = W("ci2"), ciTop = topFace(ci2).id;
  // The other instance ci1 grows by 1 m above its top; that region must be on screen in the arranged view.
  const ci1 = W("ci1"), grow = bbox(ci1.V);
  const growCorners = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => [i & 1 ? grow.hi[0] : grow.lo[0], i & 2 ? grow.hi[1] : grow.lo[1], i & 4 ? grow.hi[2] + 1 : grow.hi[2]]);
  const fpI = await arrange((vv) => (growCorners.every((p) => onScreen(vv.cam, vv.cam.project(p), 10)) ? facePoint(vv.cam, known, "ci2", ciTop, FP) : null),
    "push/pull on instance ci2 (with ci1 on screen)", faceCentroid(topFace(ci2)));
  const region = growCorners.map((p) => v.cam.project(p));
  await key(page, "Escape");
  await park(page);
  const before = await shot(page, "viewport-3d");
  const noise = diff(before, await shot(page, "viewport-3d"));
  await key(page, "p");
  await clickAt(page, v, fpI.p);
  await hover(page, v, add(fpI.p, [0, 0, 1]));
  await typeValue(page, "1000", "push/pull on a component instance");
  const csNow = await changed("cs", csPre, "push/pull on instance ci2 (the definition's Shape cs)");
  expectCli(csNow, { show: csPre }, [{ op: "pushpull", shape: "cs", face: ciTop, distance: 1 }], "push/pull 1000 on instance ci2");
  await key(page, "Escape");
  await park(page);
  await sleep(300);
  const after = await shot(page, "viewport-3d");
  const hull = (() => { // convex hull of the projected growth region of ci1
    const P = region.filter(Boolean).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    const cz = (o, a, b) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    const lo = [], up = [];
    for (const p of P) { while (lo.length >= 2 && cz(lo[lo.length - 2], lo[lo.length - 1], p) <= 0) lo.pop(); lo.push(p); }
    for (const p of [...P].reverse()) { while (up.length >= 2 && cz(up[up.length - 2], up[up.length - 1], p) <= 0) up.pop(); up.push(p); }
    return { H: [...lo.slice(0, -1), ...up.slice(0, -1)], cz };
  })();
  const inHull = (x, y) => {
    if (hull.H.length < 3) return false;
    const s = hull.H.map((a, i) => hull.cz(a, hull.H[(i + 1) % hull.H.length], [x, y]));
    return s.every((c) => c >= 0) || s.every((c) => c <= 0);
  };
  const changedPx = diff(before, after).filter(([x, y]) => inHull(x, y)).length;
  const noisePx = noise.filter(([x, y]) => inHull(x, y)).length;
  let hullPx = 0;
  for (let y = 0; y < after.h; y++) for (let x = 0; x < after.w; x++) if (inHull(x, y)) hullPx++;
  // As in T8.3's "appears in viewport-3d" checks: a clear absolute change above the frame-to-frame noise, in
  // the region where ci1's new faces and edges must appear (not a share of the region, which depends on shading).
  check(hullPx >= 100 && changedPx >= noisePx + 40,
    `instance ci1 did not visibly grow in viewport-3d after editing ci2 (${changedPx} px changed in its ${hullPx} px growth region, noise ${noisePx})`);
  known = world();

  // --------------------------------------------- 12. scenes save and restore the camera as Scene entities
  await reveal(page, "scenes");
  const save = page.locator(`${tid("scenes")} ${tid("scene-save")}`).first();
  check(await save.count(), "scenes has no scene-save button");
  v = await zoomExtents(page, pts(), "key", "Shift+Z");
  await orbit(70);
  const c1 = (await view(page)).raw;
  await save.click();
  let sceneE = null;
  await until(() => { const ch = peer.mark(); for (const id of ch.added) { const e = peer.show(id); if (e.kind === "Scene") { sceneE = e; return true; } } return false; }, 10_000, 400);
  check(sceneE, "scene-save created no Scene entity (read through bim sync)");
  const sp = sceneE.props;
  const near = (a, b) => Array.isArray(a) && a.length === 3 && dist(a, b) < 1e-6 * Math.max(1, Math.hypot(...b));
  check(typeof sp.name === "string" && near(sp.eye, c1.eye) && near(sp.target, c1.target) && Math.abs(sp.fov - c1.fov) < 1e-9,
    `the saved Scene ${JSON.stringify(sp)} does not hold the camera ${JSON.stringify(c1)}`);
  await orbit(-150);
  const c2 = (await view(page)).raw;
  check(viewAngle(c1, c2) > 0.05, "judge: orbiting did not move the camera away from the saved scene");
  const item = page.locator(`${tid("scenes")} ${tid(`scene-${sceneE.id}`)}`).first();
  check(await item.count(), `scenes lists no scene-${sceneE.id} item for the saved Scene`);
  await item.click();
  const back = await until(async () => { const c = (await view(page)).raw; return near(c.eye, sp.eye) && near(c.target, sp.target) && Math.abs(c.fov - sp.fov) < 1e-9 ? c : null; }, 3000, 150);
  check(back, `activating scene-${sceneE.id} did not restore its camera (camera ${JSON.stringify((await view(page)).raw)})`);

  // ------------------------------------------------------- 13. responsive layout and accessibility
  for (const [w, h] of [[1440, 900], [390, 844]]) {
    await page.setViewportSize({ width: w, height: h });
    await sleep(500);
    const ov = await page.evaluate(() => ({ doc: document.documentElement.scrollWidth, body: document.body.scrollWidth, vw: window.innerWidth }));
    check(ov.doc <= ov.vw && ov.body <= ov.vw, `horizontal overflow at ${w} px: scrollWidth ${Math.max(ov.doc, ov.body)} > ${ov.vw}`);
    const res = await new AxeBuilder({ page }).analyze();
    const bad = res.violations.filter((x) => x.impact === "serious" || x.impact === "critical");
    check(bad.length === 0, `axe-core at ${w} px: ${bad.map((x) => `${x.id} (${x.impact}, ${x.nodes.length} nodes)`).join("; ")}`);
  }
  check(errors.length === 0, `page errors: ${errors.slice(0, 3).join(" | ")}`);
  console.log("T8.7 judge: all checks passed");
} catch (e) {
  code = 1;
  console.error(e instanceof JudgeFail ? `JUDGE FAIL: ${e.message}` : `JUDGE FAIL: unexpected error: ${e && e.stack || e}`);
  if (errors.length) console.error(`page errors: ${errors.slice(0, 5).join(" | ")}`);
} finally {
  await browser.close().catch(() => {});
  if (srv) await srv.close();
}
process.exit(code);
