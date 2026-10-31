// External judge for T8.3 (apps/studio): Playwright + system Chrome, headless, against the built
// app (apps/studio/dist) served from http://localhost. Interacts only through the spec/web.md test
// hooks, the mouse and the keyboard. Geometry is located by pixel diffs of the views themselves.
import path from "node:path";
import { oracle, CHROME, ROOT } from "../lib/oracle.mjs";
import { serve, deadline } from "./serve.mjs";
import {
  JudgeFail, check, tid, sleep, shot, diff, segDist, centralPixel, until, ready, reveal, park,
  blurActive, propValue, clickIn, setProp,
} from "./studio.mjs";

deadline(110_000, "T8.3 browser checks");
const REQUIRED = ["viewport-3d", "sheet-2d", "tool-select", "tool-wall", "tool-slab", "tool-door",
  "props-panel", "history", "undo", "redo", "level-picker", "status-bar"];
const num = (v) => (v === null ? NaN : parseFloat(v));

const { chromium } = await oracle("playwright-core");
const srv = await serve(path.join(ROOT, "apps/studio/dist"), { spa: true });
const URL_ = `http://localhost:${srv.port}/`;
const browser = await chromium.launch({ executablePath: CHROME, headless: true, args: ["--enable-unsafe-webgpu"] });
const errors = [];
const watch = (page, label) => {
  page.on("console", (m) => { if (m.type() === "error") errors.push(`${label}: ${m.text()}`); });
  page.on("pageerror", (e) => errors.push(`${label}: ${e}`));
};
const meanLum = (img) => { let s = 0; for (let i = 0; i < img.data.length; i += 4) s += 0.2126 * img.data[i] + 0.7152 * img.data[i + 1] + 0.0722 * img.data[i + 2]; return s / (img.w * img.h); };
const { decodePng } = await import("./png.mjs");

let code = 0;
try {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1, colorScheme: "light" });
  const page = await ctx.newPage();
  page.setDefaultTimeout(10_000);
  watch(page, "light");
  await page.goto(URL_);
  const status = await ready(page, "initial load");
  check(/fps/i.test(status) && /\d/.test(status), `status-bar should show the backend and fps, shows '${status.trim()}'`);
  const lightLum = meanLum(decodePng(await page.screenshot()));

  // ------------------------------------------------------------------ layout and test hooks
  for (const id of REQUIRED) check(await page.locator(tid(id)).count() > 0, `missing data-testid="${id}"`);
  // (props-panel and history may be empty, hence zero-size, before anything exists)
  for (const id of REQUIRED.filter((i) => !["viewport-3d", "sheet-2d", "props-panel", "history"].includes(i)))
    check(await page.locator(tid(id)).first().isVisible(), `${id} is not visible at 1440 px`);
  await reveal(page, "viewport-3d");
  const plan = await reveal(page, "sheet-2d");
  const pb = await plan.boundingBox();
  check(pb.width > 200 && pb.height > 150, "sheet-2d is too small to draw on");
  const seg = (fy, a = 0.35, b = 0.65) => [[pb.width * a, pb.height * fy], [pb.width * b, pb.height * fy]];
  const band = (px, [a, b]) => px.filter((p) => segDist(p, a, b) <= 15);
  const segLen = (s) => Math.hypot(s[1][0] - s[0][0], s[1][1] - s[0][1]);
  const view = async (id) => { await reveal(page, id); return shot(page, id); };
  const historyText = () => page.locator(tid("history")).first().innerText();

  // baseline and screenshot noise
  await park(page);
  const P0 = await view("sheet-2d"), V0 = await view("viewport-3d");
  const nV = diff(V0, await view("viewport-3d")).length, nP = diff(P0, await view("sheet-2d")).length;
  const H0 = await historyText();

  // ------------------------------------------------ 1. draw a wall with two clicks on the plan
  const S1 = seg(0.45);
  await page.locator(tid("tool-wall")).first().click();
  await reveal(page, "sheet-2d");
  await clickIn(page, "sheet-2d", S1[0]);
  await clickIn(page, "sheet-2d", S1[1]);
  await park(page);
  await sleep(300);
  const P1 = await view("sheet-2d"), V1 = await view("viewport-3d");
  const w1Plan = band(diff(P0, P1), S1);
  check(w1Plan.length >= 0.25 * segLen(S1), "clicking two points with tool-wall drew no wall along the clicked segment in sheet-2d");
  check(diff(V0, V1).length > nV + 50, "the new wall does not appear in viewport-3d");
  check((await historyText()) !== H0, "history did not change after creating a wall");
  const W1PT = centralPixel(w1Plan);
  await page.keyboard.press("Escape");

  // ------------------------------------------ 2. reload: the document comes back from IndexedDB
  await page.reload();
  await ready(page, "reload");
  check((await page.evaluate(async () => (await indexedDB.databases()).length)) > 0, "no IndexedDB database was created");
  await park(page);
  const reloaded = await until(async () => band(diff(P0, await view("sheet-2d")), S1).length >= 0.5 * w1Plan.length, 8000, 300);
  check(reloaded, "after reload the wall is missing from sheet-2d (not restored from IndexedDB)");
  const V1r = await view("viewport-3d");
  const w1View = diff(V0, V1r);
  check(w1View.length > nV + 50, "after reload the wall is missing from viewport-3d");
  check((await propValue(page, "height")) === null, "props-panel shows a selection right after load; cannot verify picking");

  // ---------------------------------------------------- 3. pick in 3D fills the properties panel
  await page.locator(tid("tool-select")).first().click();
  const V3D = centralPixel(w1View);
  await reveal(page, "viewport-3d");
  await clickIn(page, "viewport-3d", V3D);
  const h0s = await until(() => propValue(page, "height"), 4000);
  check(h0s !== null, "clicking the wall in viewport-3d did not fill props-panel (no prop-height input)");
  const h0 = num(h0s);
  check(h0 > 0, `prop-height of the new wall is '${h0s}'`);
  const xs = await Promise.all(["x1", "y1", "x2", "y2"].map((k) => propValue(page, k)));
  check(xs.every((v) => v !== null && Number.isFinite(num(v))), `wall props x1 y1 x2 y2 not shown as numeric inputs: ${xs}`);
  check(Math.hypot(num(xs[2]) - num(xs[0]), num(xs[3]) - num(xs[1])) > 0, "the drawn wall has zero length");

  // ------------------------------------- 4. editing prop-height updates the model and both views
  await park(page);
  const P2 = await view("sheet-2d"), V2 = await view("viewport-3d");
  await setProp(page, "height", 0.3);
  const P3 = await view("sheet-2d"), V3 = await view("viewport-3d");
  check(diff(V2, V3).length > nV + 30, "editing prop-height to 0.3 m did not update viewport-3d");
  check(diff(P2, P3).length > nP + 10, "editing prop-height to 0.3 m (below the plan cut) did not update sheet-2d");
  const selectW1 = async () => {
    if ((await propValue(page, "height")) !== null) return;
    await page.locator(tid("tool-select")).first().click();
    await reveal(page, "sheet-2d");
    await clickIn(page, "sheet-2d", W1PT);
    await until(() => propValue(page, "height"), 3000);
  };
  const expectHeight = async (want, what) => {
    await selectW1();
    const got = await until(async () => { const v = num(await propValue(page, "height")); return Math.abs(v - want) < 1e-6 ? v : null; }, 3000);
    check(got !== null, `${what}: prop-height is ${await propValue(page, "height")}, want ${want}`);
  };
  await expectHeight(0.3, "after editing");

  // --------------------------------------------------------------- 5. undo / redo (buttons, keys)
  await page.locator(tid("undo")).first().click();
  await expectHeight(h0, "after undo");
  await park(page);
  const V4 = await view("viewport-3d");
  check(diff(V3, V4).length > nV + 30, "undo did not update viewport-3d");
  await page.locator(tid("redo")).first().click();
  await expectHeight(0.3, "after redo");
  await blurActive(page);
  await page.keyboard.press("ControlOrMeta+z");
  await expectHeight(h0, "after Ctrl/Cmd+Z");
  await blurActive(page);
  await page.keyboard.press("ControlOrMeta+Shift+z");
  await expectHeight(0.3, "after Ctrl/Cmd+Shift+Z");

  // ------------------------------------------------------------ 6. keyboard: W, Esc, S
  await page.keyboard.press("Escape");
  await blurActive(page);
  await park(page);
  const P5 = await view("sheet-2d");
  const H5 = await historyText();
  const S2 = seg(0.7);
  await page.keyboard.press("w");
  await clickIn(page, "sheet-2d", S2[0]);
  await clickIn(page, "sheet-2d", S2[1]);
  await park(page);
  await sleep(300);
  const P6 = await view("sheet-2d");
  const w2Plan = band(diff(P5, P6), S2);
  check(w2Plan.length >= 0.25 * segLen(S2), "pressing W then clicking two points did not draw a wall");
  const H6 = await historyText();
  check(H6 !== H5, "history did not change after drawing with the W shortcut");
  const S3 = seg(0.2);
  await page.keyboard.press("w");
  await clickIn(page, "sheet-2d", S3[0]);
  await page.keyboard.press("Escape");
  await clickIn(page, "sheet-2d", S3[1]);
  await park(page);
  await sleep(300);
  const P7 = await view("sheet-2d");
  check(band(diff(P6, P7), S3).length < 0.1 * segLen(S3), "Esc did not cancel the wall tool (a wall was drawn)");
  check((await historyText()) === H6, "Esc did not cancel the wall tool (history changed)");
  await page.keyboard.press("Escape");
  await page.keyboard.press("s");
  await clickIn(page, "sheet-2d", centralPixel(w2Plan));
  const sel2 = await until(() => propValue(page, "height"), 3000);
  check(sel2 !== null, "pressing S then clicking a wall on the plan did not fill props-panel (2D selection)");

  // --------------------------------------------- 7. reload again: the edit persisted
  await page.keyboard.press("Escape");
  await page.reload();
  await ready(page, "second reload");
  await page.locator(tid("tool-select")).first().click();
  await reveal(page, "sheet-2d");
  await clickIn(page, "sheet-2d", W1PT);
  const hp = await until(() => propValue(page, "height"), 4000);
  check(hp !== null && Math.abs(num(hp) - 0.3) < 1e-6, `after reload wall 1 has prop-height ${hp}, want 0.3 (edit not persisted)`);

  // --------------------------------------------- 8. light and dark themes (prefers-color-scheme)
  const dctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1, colorScheme: "dark" });
  const dpage = await dctx.newPage();
  watch(dpage, "dark");
  await dpage.goto(URL_);
  await ready(dpage, "dark theme");
  const darkLum = meanLum(decodePng(await dpage.screenshot()));
  check(lightLum - darkLum > 40, `dark theme is not darker than light (mean luminance ${darkLum.toFixed(0)} vs ${lightLum.toFixed(0)})`);

  check(errors.length === 0, `console errors: ${errors.slice(0, 5).join(" | ")}`);
  console.log("T8.3 judge: all checks passed");
} catch (e) {
  code = 1;
  console.error(e instanceof JudgeFail ? `JUDGE FAIL: ${e.message}` : `JUDGE FAIL: unexpected error: ${e && e.stack || e}`);
  if (errors.length) console.error(`console errors: ${errors.slice(0, 5).join(" | ")}`);
} finally {
  await browser.close().catch(() => {});
  await srv.close();
}
process.exit(code);
