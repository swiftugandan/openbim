// Studio-side helpers for the direct-modelling judges: the 3D camera the Studio reports on
// `viewport-3d` (data-camera), pointer positions computed from it, the inference tip, the
// measurements box, tool state (aria-pressed) and coloured-pixel counts along a screen segment.
import { check, fail, tid, sleep, until, blurActive, shot } from "./studio.mjs";
import { camera, add, mulv, sub, len, boxCorners, bbox, segDist2 } from "./geom.mjs";

/** Current camera of viewport-3d: {cam, box} (cam over the element's CSS box). */
export async function view(page) {
  const loc = page.locator(tid("viewport-3d")).first();
  const [raw, box] = await Promise.all([loc.getAttribute("data-camera"), loc.boundingBox()]);
  check(raw, "viewport-3d has no data-camera attribute (spec/web.md direct-modelling conventions)");
  let c;
  try { c = JSON.parse(raw); } catch { fail(`viewport-3d data-camera is not JSON: '${raw.slice(0, 80)}'`); }
  const ok = (v) => Array.isArray(v) && v.length === 3 && v.every(Number.isFinite);
  check(ok(c.eye) && ok(c.target) && Number.isFinite(c.fov) && c.fov > 0 && c.fov < Math.PI, `viewport-3d data-camera ${raw} is not {eye, target, fov}`);
  check(box && box.width > 100 && box.height > 100, "viewport-3d is not visible or too small");
  return { cam: camera(c, box.width, box.height), box, raw: c };
}
/** Page coordinates of a world point in the given view. */
export function pagePt(v, p) {
  const q = v.cam.project(p);
  check(q, `judge: point ${p} is behind the camera`);
  return [v.box.x + q[0], v.box.y + q[1]];
}
export async function hover(page, v, p, [dx, dy] = [0, 0]) {
  const [x, y] = pagePt(v, p);
  await page.mouse.move(x + dx, y + dy, { steps: 5 });
}
export async function clickAt(page, v, p, off = [0, 0]) {
  await hover(page, v, p, off);
  await sleep(60);
  const [x, y] = pagePt(v, p);
  await page.mouse.click(x + off[0], y + off[1]);
}
export async function dblclickAt(page, v, p) {
  await hover(page, v, p);
  await sleep(60);
  const [x, y] = pagePt(v, p);
  await page.mouse.dblclick(x, y);
}
export const tipText = (page) => page.locator(tid("inference-tip")).first().innerText().catch(() => "");
/** Wait until inference-tip matches `re`. */
export async function expectTip(page, re, what, ms = 3000) {
  const t = await until(async () => { const s = await tipText(page); return re.test(s) ? s : null; }, ms, 80);
  check(t, `${what}: inference-tip shows '${(await tipText(page)).trim().slice(0, 60)}', want ${re}`);
}
export const measureText = (page) => page.locator(tid("measurements")).first()
  .evaluate((el) => (typeof el.value === "string" && el.value) || el.innerText || el.textContent || "").catch(() => "");
/** Type a value while a tool runs; measurements must show it; then Enter. */
export async function typeValue(page, text, what) {
  await page.keyboard.type(text, { delay: 25 });
  const ok = await until(async () => (await measureText(page)).replace(/\s/g, "").includes(text), 2000, 80);
  check(ok, `${what}: typing '${text}' did not fill measurements (shows '${(await measureText(page)).trim().slice(0, 40)}')`);
  await page.keyboard.press("Enter");
}
/** Press a key with nothing focused (tool shortcuts). */
export async function key(page, k) {
  await blurActive(page);
  await page.keyboard.press(k);
  await sleep(120);
}
/** The active tool is the only `tool-*` button with aria-pressed="true". */
export async function activeTools(page) {
  return page.locator('[data-testid^="tool-"]').evaluateAll((els) =>
    els.filter((e) => e.getAttribute("aria-pressed") === "true").map((e) => e.getAttribute("data-testid")));
}
export async function expectTool(page, id, what) {
  const ok = await until(async () => { const a = await activeTools(page); return a.length === 1 && a[0] === id ? a : null; }, 1500, 80);
  check(ok, `${what}: active tool buttons (aria-pressed="true") are [${(await activeTools(page)).join(", ")}], want [${id}]`);
}

/** Press Shift+Z (or click zoom-extents) and wait until the camera frames `pts`: every point projects
 * inside the viewport and their screen extent covers at least 30% of its width or height. */
export async function frames(v, pts) {
  const px = pts.map((p) => v.cam.project(p));
  if (px.some((q) => !q)) return null;
  const xs = px.map((q) => q[0]), ys = px.map((q) => q[1]);
  const inside = px.every((q) => q[0] >= -1 && q[1] >= -1 && q[0] <= v.cam.W + 1 && q[1] <= v.cam.H + 1);
  const span = Math.max((Math.max(...xs) - Math.min(...xs)) / v.cam.W, (Math.max(...ys) - Math.min(...ys)) / v.cam.H);
  return { inside, span, ok: inside && span >= 0.3 };
}
export async function zoomExtents(page, pts, how = "key", what = "zoom-extents") {
  let last = null;
  const ok = await until(async () => {
    if (how === "key") await key(page, "Shift+Z"); else await page.locator(tid("zoom-extents")).first().click();
    await sleep(250);
    const v = await view(page);
    last = await frames(v, pts);
    return last && last.ok ? v : null;
  }, 15_000, 400);
  check(ok, `${what} does not frame the model (${last ? `inside ${last.inside}, extent ${(last.span * 100).toFixed(0)}% of the viewport` : "model behind the camera"})`);
  return ok;
}

/** Count pixels within `w` px of the screen segment a-b (element px) whose colour satisfies `pred`. */
export function countAlong(img, a, b, pred, w = 4) {
  let n = 0;
  const x0 = Math.max(0, Math.floor(Math.min(a[0], b[0]) - w)), x1 = Math.min(img.w - 1, Math.ceil(Math.max(a[0], b[0]) + w));
  const y0 = Math.max(0, Math.floor(Math.min(a[1], b[1]) - w)), y1 = Math.min(img.h - 1, Math.ceil(Math.max(a[1], b[1]) + w));
  for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) {
    if (segDist2([x, y], a, b) > w) continue;
    const o = (y * img.w + x) * 4;
    if (pred(img.data[o], img.data[o + 1], img.data[o + 2])) n++;
  }
  return n;
}
// Dominant-channel tests loose enough for dark greens/navies and for 1 px antialiased lines blended with a
// light or dark background, yet never true for greys, whites, blacks or the usual face shading.
export const RED = (r, g, b) => r >= 120 && r - Math.max(g, b) >= 60;
export const GREEN = (r, g, b) => g >= 95 && g - Math.max(r, b) >= 45;
export const BLUE = (r, g, b) => b >= 120 && b - Math.max(r, g) >= 50;

export { shot, bbox, boxCorners, add, mulv, sub, len };
