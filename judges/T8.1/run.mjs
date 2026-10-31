// External judge for T8.1 (packages/web-viewport): Playwright + system Chrome, headless.
// Loads judges/T8.1/page.html over http://localhost (WebGPU needs a secure context), builds a Model
// from JSON ops with bim-wasm, and drives createViewport / mountThree per spec/web.md.
// Expected pixel positions come from the closed-form pinhole projection of the scene below.
import { oracle, CHROME, ROOT } from "../lib/oracle.mjs";
import fs from "node:fs";
import { serve, deadline, importMap } from "./serve.mjs";

deadline(80_000, "T8.1 browser checks");
class JudgeFail extends Error {}
const fail = (m) => { throw new JudgeFail(m); };
const check = (cond, m) => { if (!cond) fail(m); };

// ---------------------------------------------------------------- scene (spec/schema.md JSON ops)
const ref = (id) => ({ ref: id });
const wall = (id, x1, y1, x2, y2, h = 3) => ({ op: "create", id, kind: "Wall", props: {
  level: ref("L0"), x1, y1, x2, y2, height: h, type: ref("WT") } });
const ops = [
  { op: "create", id: "L0", kind: "Level", props: { elevation: 0 } },
  { op: "create", id: "WT", kind: "WallType", props: { layer_thickness: [0.2], layer_material: ["concrete"], layer_conductivity: [1.7] } },
  wall("A", -6, 0, -0.5, 0), wall("B", 0.5, 0, 6, 0), wall("C", -1, 5, 1, 5),
];
// 120 walls of distinct lengths far behind the camera: lots of distinct (non-instanced) geometry,
// so a copied upload of positions/normals is easy to tell from a wasm-memory view.
for (let i = 0; i < 120; i++) ops.push(wall(`X${i}`, 0, -80 - i, 1 + 0.05 * i, -80 - i));
const OPS = JSON.stringify({ schema: 1, ops });

// Camera: perspective, up = +z, fovY in radians, aspect = canvas aspect (spec/schema.md: SI, radians).
const W = 400, H = 300, EYE = [0, -20, 1.5], TARGET = [0, 0, 1.5], FOV = 0.8;
function project([x, y, z]) {
  const sub = (a, b) => a.map((v, i) => v - b[i]);
  const dot = (a, b) => a.reduce((s, v, i) => s + v * b[i], 0);
  const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  const norm = (a) => a.map((v) => v / Math.hypot(...a));
  const f = norm(sub(TARGET, EYE)), r = norm(cross(f, [0, 0, 1])), u = cross(r, f);
  const d = sub([x, y, z], EYE), zc = dot(d, f), t = Math.tan(FOV / 2);
  return [Math.round(W / 2 + (dot(d, r) / zc / (t * W / H)) * W / 2), Math.round(H / 2 - (dot(d, u) / zc / t) * H / 2)];
}
const PA = project([-3.25, -0.1, 1.5]);   // centre of A's front face
const PB = project([3.25, -0.1, 1.5]);    // centre of B's front face
const PC = project([0, 4.9, 1.5]);        // C, seen through the 1 m gap between A and B
const PSKY = project([0, 0, 9]);          // above everything
const PA_LOW = project([-3.25, -0.1, 0.25]);

// ------------------------------------------------------------------------------- browser
const { chromium } = await oracle("playwright-core");
const PAGE = fs.readFileSync(new URL("./page.html", import.meta.url), "utf8").replace("__IMPORTMAP__", importMap(ROOT));
const srv = await serve(ROOT, { overrides: { "/judges/T8.1/page.html": { type: "text/html; charset=utf-8", body: PAGE } } });
const base = `http://localhost:${srv.port}/judges/T8.1/page.html`;
const browser = await chromium.launch({ executablePath: CHROME, headless: true, args: ["--enable-unsafe-webgpu"] });
const errors = [];

async function openPage(ctxOpts = {}, init = null) {
  const ctx = await browser.newContext({ viewport: { width: 900, height: 1400 }, deviceScaleFactor: 1, ...ctxOpts });
  if (init) await ctx.addInitScript(init);
  const page = await ctx.newPage();
  page.setDefaultTimeout(15_000);
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(base);
  await page.waitForFunction(() => window.__pageReady === true);
  const r = await page.evaluate(async (opsJson) => {
    try {
      const w = await import("bim-wasm");
      const exp = await w.default();
      window.__mem = w.memory instanceof WebAssembly.Memory ? w.memory : exp.memory;
      window.__vpmod = await import("/packages/web-viewport/dist/index.js");
      window.__models = {};
      for (const k of ["a", "b", "c", "d"]) window.__models[k] = w.Model.fromOps(opsJson);
      window.__vps = {};
      return { ok: true, mem: window.__mem instanceof WebAssembly.Memory,
               fns: ["createViewport", "mountThree", "createSheetView"].filter((n) => typeof window.__vpmod[n] !== "function") };
    } catch (e) { return { ok: false, err: String(e && e.stack || e) }; }
  }, OPS);
  check(r.ok, `loading bim-wasm / web-viewport in the page failed: ${r.err}`);
  check(r.mem, "bim-wasm memory is not a WebAssembly.Memory");
  check(r.fns.length === 0, `web-viewport does not export ${r.fns.join(", ")}`);
  return page;
}

async function create(page, key, backend) {
  const r = await page.evaluate(async ([key, backend, eye, target, fov]) => {
    try {
      const canvas = document.getElementById(key);
      const vp = await window.__vpmod.createViewport(canvas, window.__models[key], { backend });
      window.__vps[key] = vp;
      vp.setCamera(eye, target, fov);
      await window.__frames();
      return { ok: true, backend: vp.backend };
    } catch (e) { return { ok: false, err: String(e && e.stack || e) }; }
  }, [key, backend, EYE, TARGET, FOV]);
  check(r.ok, `createViewport(${backend}) threw: ${r.err}`);
  return r.backend;
}

async function shot(page, key) {
  await page.evaluate(() => window.__frames());
  const png = await page.locator(`#${key}`).screenshot();
  return page.evaluate(async (b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const bmp = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const c = new OffscreenCanvas(bmp.width, bmp.height);
    const x = c.getContext("2d");
    x.drawImage(bmp, 0, 0);
    const d = x.getImageData(0, 0, bmp.width, bmp.height);
    return { w: d.width, h: d.height, data: Array.from(d.data) };
  }, png.toString("base64"));
}
const px = (img, [x, y]) => { const i = (y * img.w + x) * 4; return img.data.slice(i, i + 3); };
const dist = (a, b) => Math.max(...a.map((v, i) => Math.abs(v - b[i])));
const pick = (page, key, [x, y]) => page.evaluate(async ([k, x, y]) => {
  try { return { v: await window.__vps[k].pick(x, y) }; } catch (e) { return { err: String(e) }; }
}, [key, x, y]).then((r) => { check(!("err" in r), `pick(${x},${y}) threw: ${r.err}`); return r.v; });
const call = (page, key, fn, args) => page.evaluate(async ([k, fn, args]) => {
  try { await window.__vps[k][fn](...args); await window.__frames(); return null; } catch (e) { return String(e && e.stack || e); }
}, [key, fn, args]).then((e) => check(e === null, `${fn}(${JSON.stringify(args)}) threw: ${e}`));

async function expectScene(page, key, label) {
  const img = await shot(page, key);
  check(img.w === W && img.h === H, `${label}: canvas screenshot is ${img.w}x${img.h}, want ${W}x${H}`);
  const bg = px(img, [3, 3]);
  check(dist(bg, px(img, [W - 4, 3])) <= 6, `${label}: background is not uniform`);
  for (const [p, n] of [[PA, "A"], [PB, "B"], [PC, "C"]])
    check(dist(px(img, p), bg) > 20, `${label}: wall ${n} not drawn at pixel ${p}`);
  for (const [p, want] of [[PA, "A"], [PB, "B"], [PC, "C"], [PSKY, null]]) {
    const got = await pick(page, key, p);
    check(got === want, `${label}: pick(${p}) returned ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
  }
  return bg;
}

let exitCode = 0;
try {
  // ---------------- 1. explicit webgl2: renders, picks, zero-copy uploads, section box, frameStats
  const page = await openPage();
  check(await create(page, "a", "webgl2") === "webgl2", "createViewport({backend:'webgl2'}).backend is not 'webgl2'");
  check(await page.evaluate(() => !!document.getElementById("a").getContext("webgl2")),
    "the 'webgl2' viewport's canvas does not hold a WebGL2 context");
  const bg = await expectScene(page, "a", "webgl2");

  const up = await page.evaluate(() => ({ uploads: window.__uploads, P: window.__models.a.positions().byteLength,
    posShared: window.__models.a.positions().buffer === window.__mem.buffer }));
  check(up.posShared, "model.positions() is not a view on wasm memory (bim-wasm)");
  const wasmBytes = up.uploads.filter((u) => u.wasm).reduce((s, u) => s + u.bytes, 0);
  const copies = up.uploads.filter((u) => !u.wasm && u.bytes >= 0.9 * up.P);
  check(wasmBytes >= up.P, `WebGL2 uploads read only ${wasmBytes} bytes from wasm memory, less than positions() (${up.P} bytes)`);
  check(copies.length === 0, `WebGL2 uploaded ${copies.length} non-wasm buffer(s) of >= 90% of positions() size (geometry copies)`);

  // Section box: everything with x < 0 (wall A, half of C) is clipped away.
  await call(page, "a", "setSectionBox", [[0, -10, -1], [10, 10, 10]]);
  let img = await shot(page, "a");
  check(dist(px(img, PA), bg) <= 12, "section box: wall A (outside the box) is still drawn");
  check(await pick(page, "a", PA) === null, "section box: geometry outside the box can still be picked");
  check(dist(px(img, PB), bg) > 20, "section box: wall B (inside the box) disappeared");
  check(await pick(page, "a", PB) === "B", "section box: pick on wall B no longer returns 'B'");
  // Box plane y = 0 cuts through A and B (thickness -0.1..0.1): the cut faces are capped.
  await call(page, "a", "setSectionBox", [[-10, 0, -1], [10, 10, 10]]);
  img = await shot(page, "a");
  check(dist(px(img, PA), bg) > 20 && dist(px(img, PB), bg) > 20, "section box cutting A and B shows no capped cut face");
  await call(page, "a", "setSectionBox", [[-10, 0, -1], null]);
  img = await shot(page, "a");
  check(dist(px(img, PA), bg) > 20 && await pick(page, "a", PA) === "A", "setSectionBox(min, null) did not clear the box");

  const fs = await page.evaluate(() => window.__vps.a.frameStats());
  check(fs && fs.frames > 0, `frameStats().frames is ${fs && fs.frames} after rendering`);
  check(Number.isFinite(fs.p50) && Number.isFinite(fs.p95) && fs.p50 >= 0 && fs.p95 >= fs.p50,
    `frameStats() p50/p95 invalid: ${JSON.stringify(fs)}`);

  // ---------------- 2. auto picks a working backend; explicit webgpu works when the GPU offers it
  const autoB = await create(page, "b", "auto");
  check(autoB === "webgpu" || autoB === "webgl2", `auto backend is '${autoB}'`);
  await expectScene(page, "b", `auto (${autoB})`);
  const hasGpu = await page.evaluate(async () => !!(navigator.gpu && await navigator.gpu.requestAdapter()));
  if (hasGpu) {
    check(await create(page, "c", "webgpu") === "webgpu", "createViewport({backend:'webgpu'}).backend is not 'webgpu'");
    await expectScene(page, "c", "webgpu");
  }

  // ---------------- 3. mountThree: every BufferAttribute array is a view on wasm memory
  const mt = await page.evaluate(async () => {
    try {
      const THREE = await import("three");
      const g = window.__vpmod.mountThree(window.__models.d, THREE);
      const mem = window.__mem.buffer;
      let meshes = 0, positions = 0;
      const bad = [];
      const arr = (a) => (a.isInterleavedBufferAttribute ? a.data.array : a.array);
      g.traverse((o) => {
        if (!o.isMesh) return;
        meshes++;
        const geo = o.geometry;
        for (const [k, a] of Object.entries(geo.attributes)) {
          if (k === "position") positions += a.count;
          if (arr(a).buffer !== mem) bad.push(k);
        }
        if (geo.index && geo.index.array.buffer !== mem) bad.push("index");
        if (o.isInstancedMesh && o.instanceMatrix.array.buffer !== mem) bad.push("instanceMatrix");
      });
      return { ok: true, group: !!g && g.isGroup === true, meshes, positions, bad: [...new Set(bad)] };
    } catch (e) { return { ok: false, err: String(e && e.stack || e) }; }
  });
  check(mt.ok, `mountThree threw: ${mt.err}`);
  check(mt.group, "mountThree did not return a THREE.Group");
  check(mt.meshes > 0 && mt.positions > 0, "mountThree group has no meshes with positions");
  check(mt.bad.length === 0, `mountThree attributes not backed by wasm memory: ${mt.bad.join(", ")}`);

  // ---------------- 4. refresh() after model.apply: views are re-fetched
  const ap = await page.evaluate(() => {
    try { window.__models.a.apply(JSON.stringify({ schema: 1, ops: [{ op: "set", id: "A", key: "height", value: 0.5 }] }));
          window.__vps.a.refresh(); return null; } catch (e) { return String(e && e.stack || e); }
  });
  check(ap === null, `model.apply + refresh threw: ${ap}`);
  await page.evaluate(() => window.__frames());
  check(await pick(page, "a", PA) === null, "after lowering A to 0.5 m and refresh(), A is still picked at 1.5 m");
  check(await pick(page, "a", PA_LOW) === "A", "after refresh(), the lowered wall A is not picked at 0.25 m");

  // ---------------- 5. auto falls back to WebGL2 when WebGPU is unavailable
  const page2 = await openPage({}, () => {
    Object.defineProperty(Navigator.prototype, "gpu", { get: () => undefined, configurable: true });
  });
  check(await create(page2, "b", "auto") === "webgl2", "auto without WebGPU did not choose webgl2");
  await expectScene(page2, "b", "auto fallback");
  console.log(`T8.1 judge: all checks passed (webgpu adapter: ${hasGpu}, auto chose ${autoB})`);
} catch (e) {
  exitCode = 1;
  if (e instanceof JudgeFail) console.error(`JUDGE FAIL: ${e.message}`);
  else console.error(`JUDGE FAIL: unexpected error: ${e && e.stack || e}`);
  if (errors.length) console.error(`page errors: ${errors.slice(0, 5).join(" | ")}`);
} finally {
  await browser.close().catch(() => {});
  await srv.close();
}
process.exit(exitCode);
