// External judge for T8.2 (createSheetView): Playwright + system Chrome, headless.
// usage: node run.mjs <real-sheet.svg> <real-sheet-hatch.json>
// The fixture sheet below follows the T7.4 SVG conventions (mm units, layer groups, dasharray in mm).
// Expected values: pen widths per spec/cli.md, ISO 128-20 dash (12 d / 3 d, d = 0.18 mm), and the
// sheet->canvas scale measured from the drawing's own extent (no assumption about the initial fit).
import fs from "node:fs";
import { oracle, CHROME, ROOT } from "../lib/oracle.mjs";
import { serve, deadline, importMap } from "./serve.mjs";
import { decodePng } from "./png.mjs";

deadline(80_000, "T8.2 browser checks");
class JudgeFail extends Error {}
const fail = (m) => { throw new JudgeFail(m); };
const check = (c, m) => { if (!c) fail(m); };
const [realSvgPath, realHatchPath] = process.argv.slice(2);

// ------------------------------------------------------------------ fixture sheet (100 x 60 mm)
// CUT: A (10,10)-(90,10) and B (10,10)-(10,52); HIDDEN: (10,25)-(90,25) dashed;
// PROJECTION: rect (55,35)-(85,50); HATCH: 45 degree lines 2 mm apart inside that rect.
const hatch = [];
for (let c = 55 + 35 + 2; c < 85 + 50; c += 2) {
  // segment of x + y = c inside [55,85] x [35,50]
  const x0 = Math.max(55, c - 50), x1 = Math.min(85, c - 35);
  if (x1 > x0) hatch.push(`M${x0} ${c - x0} L${x1} ${c - x1}`);
}
const FIXTURE = `<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="60mm" viewBox="0 0 100 60">
<g id="CUT" stroke="#000000" stroke-width="0.5" fill="none"><path data-entity="A" d="M10 10 L90 10"/><path data-entity="B" d="M10 10 L10 52"/></g>
<g id="HIDDEN" stroke="#000000" stroke-width="0.18" stroke-dasharray="2.16 0.54" fill="none"><path data-entity="H" d="M10 25 L90 25"/></g>
<g id="PROJECTION" stroke="#000000" stroke-width="0.25" fill="none"><path data-entity="R" d="M55 35 L85 35 L85 50 L55 50 Z"/></g>
<g id="HATCH" stroke="#000000" stroke-width="0.13" fill="none"><path data-entity="R" d="${hatch.join(" ")}"/></g>
</svg>`;
const INK_X = [9.75, 90], INK_Y = [9.75, 52]; // extent of the dark strokes (B is 0.5 mm wide, butt caps)
const CSS_W = 1000, CSS_H = 600;
const DASH = 12 * 0.18, GAP = 3 * 0.18;

// ------------------------------------------------------------------------------ image analysis
const lum = (img, x, y) => { const o = (y * img.w + x) * 4; return 0.2126 * img.data[o] + 0.7152 * img.data[o + 1] + 0.0722 * img.data[o + 2]; };
function calibrate(img, label) {
  let x0 = Infinity, y0 = Infinity, x1 = -1, y1 = -1;
  for (let y = 0; y < img.h; y++) for (let x = 0; x < img.w; x++) if (lum(img, x, y) < 128) {
    if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y;
  }
  check(x1 >= 0, `${label}: nothing drawn on the canvas`);
  check(x0 > 0 && y0 > 0 && x1 < img.w - 1 && y1 < img.h - 1, `${label}: drawing touches the canvas edge; cannot measure`);
  const sx = (x1 - x0 + 1) / (INK_X[1] - INK_X[0]), sy = (y1 - y0 + 1) / (INK_Y[1] - INK_Y[0]);
  check(Math.abs(sx / sy - 1) < 0.03, `${label}: sheet is not drawn with a uniform scale (x ${sx.toFixed(3)} vs y ${sy.toFixed(3)} px/mm)`);
  const s = (sx + sy) / 2;
  return { s, ox: x0 - s * INK_X[0], oy: y0 - s * INK_Y[0], map: (x, y) => [x0 - s * INK_X[0] + s * x, y0 - s * INK_Y[0] + s * y] };
}
// Ink coverage (in pixels) across a stroke: sum of darkness along a perpendicular profile.
function strokeWidth(img, cal, [xm, ym], horizontal) {
  const [cx, cy] = cal.map(xm, ym).map(Math.round);
  const half = Math.ceil(0.5 * cal.s) + 6;
  const prof = [];
  for (let k = -half; k <= half; k++) prof.push(horizontal ? lum(img, cx, cy + k) : lum(img, cx + k, cy));
  const edge = [...prof.slice(0, 3), ...prof.slice(-3)].sort((a, b) => a - b);
  const bg = edge[Math.floor(edge.length / 2)];
  return prof.reduce((s, v) => s + Math.max(0, bg - v), 0) / bg;
}
function dashRuns(img, cal) {
  const [xa, y] = cal.map(12, 25).map(Math.round), [xb] = cal.map(88, 25).map(Math.round);
  const dark = [];
  for (let x = xa; x <= xb; x++) dark.push(255 - Math.min(lum(img, x, y - 1), lum(img, x, y), lum(img, x, y + 1)));
  const thr = Math.max(...dark) / 2;
  const runs = [];
  let cur = dark[0] > thr, len = 0;
  for (const d of dark) { const on = d > thr; if (on === cur) len++; else { runs.push([cur, len]); cur = on; len = 1; } }
  runs.push([cur, len]);
  return runs.slice(1, -1); // drop partial runs at both ends
}
function touchesEdge(img) {
  for (let x = 0; x < img.w; x++) if (lum(img, x, 0) < 128 || lum(img, x, img.h - 1) < 128) return true;
  for (let y = 0; y < img.h; y++) if (lum(img, 0, y) < 128 || lum(img, img.w - 1, y) < 128) return true;
  return false;
}
const median = (a) => { const s = [...a].sort((x, y) => x - y); return s[Math.floor(s.length / 2)]; };
const near = (got, want, tol) => Math.abs(got - want) <= tol;

function measure(img, label, { full = true } = {}) {
  const cal = calibrate(img, label);
  const wA = strokeWidth(img, cal, [50, 10], true), wB = strokeWidth(img, cal, [10, 40], false);
  for (const [w, n] of [[wA, "horizontal"], [wB, "vertical"]])
    check(near(w, 0.5 * cal.s, Math.max(0.6, 0.12 * 0.5 * cal.s)),
      `${label}: ${n} CUT stroke is ${w.toFixed(2)} device px, want 0.50 mm x ${cal.s.toFixed(2)} px/mm = ${(0.5 * cal.s).toFixed(2)}`);
  if (full) {
    // Widest profile over one dash period = a profile through the middle of a dash.
    let wH = 0;
    for (let x = 50; x <= 50 + DASH + GAP; x += 0.1) wH = Math.max(wH, strokeWidth(img, cal, [x, 25], true));
    // Chrome's Canvas2D draws dashed strokes without anti-aliasing (whole pixels), hence +-1 px.
    check(near(wH, 0.18 * cal.s, Math.max(1.0, 0.2 * 0.18 * cal.s)),
      `${label}: HIDDEN stroke is ${wH.toFixed(2)} device px, want ${(0.18 * cal.s).toFixed(2)}`);
    const runs = dashRuns(img, cal);
    const on = runs.filter((r) => r[0]).map((r) => r[1]), off = runs.filter((r) => !r[0]).map((r) => r[1]);
    check(on.length >= 5 && off.length >= 5, `${label}: HIDDEN line is not dashed (${on.length} dashes seen)`);
    const period = median(on) + median(off), want = (DASH + GAP) * cal.s;
    check(near(period, want, Math.max(2, 0.06 * want)),
      `${label}: HIDDEN dash period ${period} px, want ${want.toFixed(1)} (2.16 + 0.54 mm at ${cal.s.toFixed(2)} px/mm)`);
    check(near(median(on) / period, DASH / (DASH + GAP), 0.12), `${label}: HIDDEN dash/gap ratio is not 12:3`);
    // Hatch: the region holds hatch lines (some ink, far from solid).
    const [hx0, hy0] = cal.map(56, 36).map(Math.round), [hx1, hy1] = cal.map(84, 49).map(Math.round);
    let inked = 0, n = 0;
    for (let y = hy0; y <= hy1; y++) for (let x = hx0; x <= hx1; x++) { n++; if (lum(img, x, y) < 200) inked++; }
    check(inked / n > 0.02 && inked / n < 0.6, `${label}: hatch region coverage ${(inked / n).toFixed(3)} (want hatch lines)`);
  }
  return cal;
}

// ------------------------------------------------------------------------------------ browser
const { chromium } = await oracle("playwright-core");
const PAGE = fs.readFileSync(new URL("./page.html", import.meta.url), "utf8").replace("__IMPORTMAP__", importMap(ROOT));
const srv = await serve(ROOT, { overrides: { "/judges/T8.2/page.html": { type: "text/html; charset=utf-8", body: PAGE } } });
const browser = await chromium.launch({ executablePath: CHROME, headless: true });
const errors = [];
async function openPage(dpr) {
  const ctx = await browser.newContext({ viewport: { width: 1100, height: 2000 }, deviceScaleFactor: dpr });
  const page = await ctx.newPage();
  page.setDefaultTimeout(15_000);
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(`http://localhost:${srv.port}/judges/T8.2/page.html`);
  await page.waitForFunction(() => window.__pageReady === true);
  const r = await page.evaluate(async () => {
    try { window.__vpmod = await import("/packages/web-viewport/dist/index.js"); window.__sv = {};
          return typeof window.__vpmod.createSheetView === "function" ? null : "createSheetView is not exported"; }
    catch (e) { return String(e && e.stack || e); }
  });
  check(r === null, `loading web-viewport failed: ${r}`);
  return page;
}
async function sheet(page, id, svg, opts) {
  const r = await page.evaluate(async ([id, svg, opts]) => {
    try {
      const c = document.getElementById(id);
      window.__sv[id] = opts ? window.__vpmod.createSheetView(c, svg, opts) : window.__vpmod.createSheetView(c, svg);
      await window.__frames();
      return { w: c.width, h: c.height };
    } catch (e) { return { err: String(e && e.stack || e) }; }
  }, [id, svg, opts]);
  check(!r.err, `createSheetView threw: ${r.err}`);
  return r;
}
async function shot(page, id) {
  await page.evaluate(() => window.__frames());
  return decodePng(await page.locator(`#${id}`).screenshot({ scale: "device" }));
}
const box = (page, id) => page.locator(`#${id}`).boundingBox();

let code = 0;
try {
  // --------------------------------------------------------- 1. device pixel ratio 1 and 2
  const cals = {};
  const pages = {};
  for (const dpr of [1, 2]) {
    const page = (pages[dpr] = await openPage(dpr));
    const size = await sheet(page, "a", FIXTURE);
    check(size.w === CSS_W * dpr && size.h === CSS_H * dpr,
      `dpr ${dpr}: canvas backing store is ${size.w}x${size.h}, want ${CSS_W * dpr}x${CSS_H * dpr} (CSS size x dpr)`);
    const img = await shot(page, "a");
    check(img.w === CSS_W * dpr, `dpr ${dpr}: unexpected screenshot size ${img.w}`);
    cals[dpr] = measure(img, `dpr ${dpr}`);
  }
  check(near(cals[2].s / cals[1].s, 2, 0.06), `device px/mm at dpr 2 is ${(cals[2].s / cals[1].s).toFixed(3)}x dpr 1, want 2x`);

  // opts.dpr overrides window.devicePixelRatio for the backing store
  const o = await sheet(pages[1], "b", FIXTURE, { dpr: 2 });
  check(o.w === CSS_W * 2 && o.h === CSS_H * 2, `opts.dpr 2 on a dpr-1 page gave a ${o.w}x${o.h} backing store`);

  // ---------------------------------------------------- 2. snap glyph at an endpoint (dpr 2)
  const page = pages[2], dpr = 2, cal = cals[2];
  const bb = await box(page, "a");
  const toCss = (xm, ym) => { const [x, y] = cal.map(xm, ym); return [bb.x + x / dpr, bb.y + y / dpr]; };
  const far = toCss(30, 45);                 // empty paper
  const E = cal.map(90, 10);                 // free end of CUT line A, device px
  await page.mouse.move(...far);
  const base = await shot(page, "a");
  const [ex, ey] = toCss(90, 10);
  await page.mouse.move(ex + 3, ey + 3);     // 4.2 CSS px from the endpoint: inside the 5 px snap radius
                                             // even if the viewer rounds event coordinates to whole px
  await page.waitForTimeout(100);
  const hover = await shot(page, "a");
  let n = 0, sx = 0, sy = 0;
  const R = 30 * dpr;
  for (let y = Math.round(E[1] - R); y <= E[1] + R; y++) for (let x = Math.round(E[0] - R); x <= E[0] + R; x++) {
    const o2 = (y * base.w + x) * 4;
    if (Math.max(...[0, 1, 2].map((k) => Math.abs(base.data[o2 + k] - hover.data[o2 + k]))) > 40) { n++; sx += x; sy += y; }
  }
  check(n >= 6 * dpr * dpr, "no snap glyph appears when the pointer is 5 px from a line endpoint");
  const gx = sx / n, gy = sy / n;
  check(Math.hypot(gx - E[0], gy - E[1]) <= 3 * dpr,
    `snap glyph is centred ${(Math.hypot(gx - E[0], gy - E[1]) / dpr).toFixed(1)} CSS px from the endpoint (want <= 3)`);
  await page.mouse.move(...far);
  await page.waitForTimeout(100);
  const after = await shot(page, "a");
  let left = 0;
  for (let i = 0; i < base.data.length; i += 4) if (Math.abs(base.data[i] - after.data[i]) > 40) left++;
  check(left < 4 * dpr * dpr, "snap glyph stays after the pointer moves away from the endpoint");

  // ------------------------------------------------------------------ 3. wheel zoom, drag pan
  await page.mouse.move(bb.x + bb.width / 2, bb.y + bb.height / 2);
  await page.mouse.wheel(0, 200);
  await page.waitForTimeout(150);
  let zimg = await shot(page, "a");
  if (touchesEdge(zimg) || calibrate(zimg, "after wheel zoom").s > cal.s) {
    // This viewer zooms in on positive deltaY: zoom out the other way, to leave room for panning.
    for (let k = 0; k < 2; k++) { await page.mouse.wheel(0, -200); await page.waitForTimeout(100); }
    await page.waitForTimeout(100);
    zimg = await shot(page, "a");
  }
  const zc = measure(zimg, "after wheel zoom", { full: false });
  const ratio = zc.s / cal.s;
  check(ratio < 0.95 || ratio > 1.05, `wheel did not zoom (scale ratio ${ratio.toFixed(3)})`);
  const start = [bb.x + bb.width / 2, bb.y + bb.height / 2];
  const drag = async (button) => {
    await page.mouse.move(...start);
    await page.mouse.down({ button });
    for (let k = 1; k <= 8; k++) await page.mouse.move(start[0] + 5 * k, start[1] + 3 * k);
    await page.mouse.up({ button });
    await page.waitForTimeout(150);
    return measure(await shot(page, "a"), "after drag pan", { full: false });
  };
  let pc = await drag("left");
  if (Math.abs(pc.ox - zc.ox) < 1 && Math.abs(pc.oy - zc.oy) < 1) pc = await drag("middle"); // pan on middle button
  check(near(pc.s / zc.s, 1, 0.02), "dragging changed the zoom");
  check(near(pc.ox - zc.ox, 40 * dpr, 3) && near(pc.oy - zc.oy, 24 * dpr, 3),
    `dragging by (40, 24) CSS px moved the sheet by (${((pc.ox - zc.ox) / dpr).toFixed(1)}, ${((pc.oy - zc.oy) / dpr).toFixed(1)})`);

  // ------------------------------------------- 4. a real `bim sheet` SVG (T7.4) with hatches
  const real = fs.readFileSync(realSvgPath, "utf8");
  const hatches = JSON.parse(fs.readFileSync(realHatchPath, "utf8"));
  await sheet(pages[1], "c", real);
  const rimg = await shot(pages[1], "c");
  // Same physical sheet size (100 x 60 mm) and canvas as the fixture: same initial mapping.
  const c1 = cals[1];
  let ink = 0;
  for (let i = 0; i < rimg.data.length; i += 4) if (rimg.data[i] < 128) ink++;
  check(ink > 200, "the real bim sheet SVG rendered (almost) nothing");
  check(hatches.length > 0, "the real bim sheet has no visible HATCH geometry");
  let hin = 0, hn = 0;
  for (const [x0, y0, x1, y1] of hatches) {
    const [ax, ay] = c1.map(x0 + 0.4, y0 + 0.4).map(Math.round), [bx, by] = c1.map(x1 - 0.4, y1 - 0.4).map(Math.round);
    for (let y = ay; y <= by; y++) for (let x = ax; x <= bx; x++) { hn++; if (lum(rimg, x, y) < 200) hin++; }
  }
  check(hn > 0 && hin / hn > 0.02 && hin / hn < 0.97, `hatched cut regions of the real sheet have ink coverage ${(hin / Math.max(hn, 1)).toFixed(3)} ` +
    "(assumes the zoom-1 view depends only on the physical sheet size, which equals the fixture's 100 x 60 mm)");
  console.log(`T8.2 judge: all checks passed (${cal.s.toFixed(2)} device px/mm at dpr 2)`);
} catch (e) {
  code = 1;
  console.error(e instanceof JudgeFail ? `JUDGE FAIL: ${e.message}` : `JUDGE FAIL: unexpected error: ${e && e.stack || e}`);
  if (errors.length) console.error(`page errors: ${errors.slice(0, 5).join(" | ")}`);
} finally {
  await browser.close().catch(() => {});
  await srv.close();
}
process.exit(code);
