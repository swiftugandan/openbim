// Shared helpers for the Studio judges: screenshots of test-id elements, pixel diffs, waiting.
import { decodePng } from "./png.mjs";

export class JudgeFail extends Error {}
export const fail = (m) => { throw new JudgeFail(m); };
export const check = (c, m) => { if (!c) fail(m); };
export const tid = (id) => `[data-testid="${id}"]`;
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Screenshot of a test-id element, decoded (CSS px, dpr 1 contexts). */
export async function shot(page, id) {
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  return decodePng(await page.locator(tid(id)).first().screenshot({ animations: "disabled" }));
}

/** Pixels whose colour changed by more than `thr` in any channel: [[x, y], ...]. */
export function diff(a, b, thr = 30) {
  const out = [];
  if (a.w !== b.w || a.h !== b.h) fail(`element size changed between screenshots (${a.w}x${a.h} vs ${b.w}x${b.h})`);
  for (let y = 0; y < a.h; y++) for (let x = 0; x < a.w; x++) {
    const o = (y * a.w + x) * 4;
    if (Math.abs(a.data[o] - b.data[o]) > thr || Math.abs(a.data[o + 1] - b.data[o + 1]) > thr ||
        Math.abs(a.data[o + 2] - b.data[o + 2]) > thr) out.push([x, y]);
  }
  return out;
}

/** Distance from point p to segment ab. */
export function segDist([px, py], [ax, ay], [bx, by]) {
  const dx = bx - ax, dy = by - ay, L = dx * dx + dy * dy;
  const t = L ? Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / L)) : 0;
  return Math.hypot(px - ax - t * dx, py - ay - t * dy);
}

/** The changed pixel closest to the centroid of a pixel set (a point that is on the new geometry). */
export function centralPixel(px) {
  const cx = px.reduce((s, p) => s + p[0], 0) / px.length, cy = px.reduce((s, p) => s + p[1], 0) / px.length;
  let best = px[0], bd = Infinity;
  for (const p of px) { const d = Math.hypot(p[0] - cx, p[1] - cy); if (d < bd) { bd = d; best = p; } }
  return best;
}

/** Poll `fn` until it returns a truthy value or `ms` elapses; returns the last value. */
export async function until(fn, ms, step = 150) {
  const end = Date.now() + ms;
  let v;
  while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(step); }
  return v;
}

/** Wait until the app is up: the status bar names the rendering backend. */
export async function ready(page, label) {
  const ok = await until(async () => {
    const t = await page.locator(tid("status-bar")).first().innerText().catch(() => "");
    return /webgpu|webgl2/i.test(t) ? t : null;
  }, 25_000, 200);
  check(ok, `${label}: status-bar never showed the rendering backend (webgpu/webgl2)`);
  await sleep(300);
  return ok;
}

/** Make a test-id element visible, clicking tabs if the layout is tabbed. */
export async function reveal(page, id) {
  const loc = page.locator(tid(id)).first();
  if (await loc.isVisible().catch(() => false)) return loc;
  for (const tab of await page.locator('[role="tab"]').all()) {
    await tab.click().catch(() => {});
    if (await loc.isVisible().catch(() => false)) return loc;
  }
  fail(`${id} is not visible (and no tab reveals it)`);
}

/** Park the mouse somewhere neutral (over the status bar) so hover effects do not pollute diffs. */
export async function park(page) {
  const bb = await page.locator(tid("status-bar")).first().boundingBox();
  if (bb) await page.mouse.move(bb.x + 2, bb.y + bb.height / 2);
}

export async function blurActive(page) {
  await page.evaluate(() => { if (document.activeElement && document.activeElement !== document.body) document.activeElement.blur(); });
}

/** Read the selected entity's prop input, or null if the props panel does not show it. */
export async function propValue(page, key) {
  const loc = page.locator(`${tid("props-panel")} ${tid(`prop-${key}`)}`).first();
  if (!(await loc.count()) || !(await loc.isVisible().catch(() => false))) return null;
  return loc.inputValue();
}

/** Click a point inside a test-id element (element-relative CSS px). */
export async function clickIn(page, id, [x, y]) {
  const bb = await page.locator(tid(id)).first().boundingBox();
  await page.mouse.click(bb.x + x, bb.y + y);
}

/** Commit a prop edit: type the value and press Enter (also blurs, which fires change). */
export async function setProp(page, key, value) {
  const loc = page.locator(`${tid("props-panel")} ${tid(`prop-${key}`)}`).first();
  check(await loc.count(), `props-panel has no prop-${key} input`);
  await loc.fill(String(value));
  await loc.press("Enter");
  await blurActive(page);
}
