// Minimal static file server for the T8.x judges (localhost, ephemeral port, correct MIME types).
import http from "node:http";
import fs from "node:fs";
import path from "node:path";

const MIME = {
  ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".mjs": "text/javascript",
  ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".svg": "image/svg+xml",
  ".png": "image/png", ".ico": "image/x-icon", ".map": "application/json", ".woff2": "font/woff2",
  ".woff": "font/woff", ".ttf": "font/ttf", ".txt": "text/plain",
};

/** Serve `root` on 127.0.0.1:<random>. `spa` falls back to /index.html for unknown paths;
 * `overrides` maps URL paths to generated bodies ({ type, body }). */
export async function serve(root, { spa = false, overrides = {} } = {}) {
  root = path.resolve(root);
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, "http://localhost");
    if (overrides[url.pathname]) {
      res.setHeader("content-type", overrides[url.pathname].type);
      res.setHeader("cache-control", "no-store");
      return res.end(overrides[url.pathname].body);
    }
    let p = path.normalize(path.join(root, decodeURIComponent(url.pathname)));
    if (!p.startsWith(root)) { res.statusCode = 403; return res.end(); }
    if (fs.existsSync(p) && fs.statSync(p).isDirectory()) p = path.join(p, "index.html");
    if (!fs.existsSync(p)) {
      if (url.pathname === "/favicon.ico") { res.statusCode = 204; return res.end(); }
      if (spa && !path.extname(url.pathname)) p = path.join(root, "index.html");
      else { res.statusCode = 404; return res.end("not found"); }
    }
    res.setHeader("content-type", MIME[path.extname(p)] || "application/octet-stream");
    res.setHeader("cache-control", "no-store");
    fs.createReadStream(p).pipe(res);
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  return { server, port: server.address().port, close: () => new Promise((r) => server.close(r)) };
}

/** Exit the process with a JUDGE FAIL message if `ms` elapses (judges never hang). */
export function deadline(ms, what) {
  const t = setTimeout(() => {
    console.error(`JUDGE FAIL: ${what} did not finish within ${ms / 1000}s`);
    process.exit(1);
  }, ms);
  t.unref();
  return t;
}

/** Import map for judge pages: the bim-wasm web package (entry from its package.json, as wasm-pack
 * writes it) and three, both under their npm names. */
export function importMap(rootDir) {
  const pj = JSON.parse(fs.readFileSync(path.join(rootDir, "packages/bim-wasm/pkg-web/package.json"), "utf8"));
  const entry = `/packages/bim-wasm/pkg-web/${pj.module ?? pj.main}`;
  const imports = { "bim-wasm": entry, three: "/node_modules/three/build/three.module.js" };
  if (pj.name) imports[pj.name] = entry;
  return JSON.stringify({ imports });
}
