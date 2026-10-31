// Resolve oracle npm packages installed by loop/setup.sh into .oracles/node.
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const require = createRequire(path.join(ROOT, ".oracles/node/package.json"));

/** Import an oracle package (ESM or CJS) by name, e.g. await oracle("playwright-core"). */
export async function oracle(name) {
  const mod = await import(pathToFileURL(require.resolve(name)).href);
  // CommonJS packages arrive wrapped in `default`; expose their exports directly.
  return mod.default && typeof mod.default === "object" ? { ...mod.default, ...mod } : mod;
}

/** Path to the system Chrome that Playwright drives (no browser download). */
export const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
