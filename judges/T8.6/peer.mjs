// The real `bim` CLI as the independent reader of the Studio's document: the Studio edits a project
// served by `bim sync serve` (spec/web.md `?sync=`), a CLI peer pulls it (`bim sync`) and reads entities
// (`bim show`); new entities are found by `bim commit` + `bim diff <dir> <revA> <revB> --json`
// (spec/cli.md). Expected results replay the corresponding `bim sketch` command on the Studio's
// pre-state in a scratch project.
import fs from "node:fs";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fail, check, until } from "./studio.mjs";

export function cli(BIM, tmp) {
  const children = [];
  let n = 0;
  function bim(args, { ok = [0], timeout = 60_000 } = {}) {
    const r = spawnSync(BIM, args, { encoding: "utf8", timeout });
    if (r.error) fail(`'bim ${args.join(" ")}' failed: ${r.error.message}`);
    if (!ok.includes(r.status)) fail(`'bim ${args.join(" ")}' exited ${r.status}: ${(r.stderr || "").trim().slice(0, 300)}`);
    return r;
  }
  const out = (args, o) => bim(args, o).stdout;
  const show = (d, id) => JSON.parse(out(["show", d, id, "--json"]));
  function file(name, obj) {
    const f = path.join(tmp, `${name}-${n++}.json`);
    fs.writeFileSync(f, typeof obj === "string" ? obj : JSON.stringify(obj));
    return f;
  }
  /** New project with ops then sketch command lists (each list one `bim sketch` call). */
  function project(name, ops = [], sketches = [], { catalogs = {} } = {}) {
    const d = path.join(tmp, `${name}-${n++}`);
    out(["init", d]);
    for (const [c, json] of Object.entries(catalogs)) {
      fs.mkdirSync(path.join(d, "catalogs", c), { recursive: true });
      fs.writeFileSync(path.join(d, "catalogs", c, "catalog.json"), JSON.stringify(json));
    }
    if (ops.length) out(["apply", d, file(`${name}-ops`, { schema: 1, ops })]);
    for (const cmds of sketches) out(["sketch", d, file(`${name}-sk`, { schema: 1, commands: cmds })]);
    return d;
  }
  async function syncServe(dir) {
    const p = spawn(BIM, ["sync", "serve", dir, "--port", "0"], { stdio: ["ignore", "pipe", "pipe"] });
    children.push(p);
    let o = "";
    p.stdout.on("data", (b) => { o += b; });
    const port = await until(() => { const m = o.match(/^listening (\d+)\s*$/m); return m ? +m[1] : null; }, 10_000, 50);
    check(port, `'bim sync serve' did not print 'listening <port>' (got '${o.slice(0, 80)}')`);
    return port;
  }
  const kill = () => { for (const c of children) c.kill("SIGKILL"); };

  /** A persistent CLI peer of the served document. */
  function peer(port) {
    const d = path.join(tmp, `peer-${n++}`);
    out(["init", d]);
    const url = `ws://127.0.0.1:${port}`;
    const P = {
      dir: d,
      sync() { bim(["sync", d, url], { timeout: 30_000 }); return P; },
      show: (id) => show(d, id),
      hash: () => out(["hash", d]).trim(),
      rev: null,
      /** Pull, record a revision and return what changed since the previous mark: {added, removed, changed}. */
      mark() {
        P.sync();
        const r = out(["commit", d, "-m", "judge"]).trim();
        check(/^[0-9a-f]{16}$/.test(r), `bim commit printed '${r}', want a 16-hex revision id`);
        let ch = { added: [], removed: [], changed: [] };
        if (P.rev && P.rev !== r) ch = JSON.parse(out(["diff", d, P.rev, r, "--json"], { ok: [0, 1] }));
        P.rev = r;
        return ch;
      },
    };
    P.mark();
    return P;
  }

  /** Strip reserved props and level refs (absent from scratch projects) for re-creating an entity. */
  const createOp = (s) => {
    const props = {};
    for (const [k, v] of Object.entries(s.props || {})) if (!k.startsWith("_") && k !== "level") props[k] = v;
    return { op: "create", id: s.id, kind: s.kind, props };
  };
  /** Replay: re-create `pre` entities (as shown by the peer) in a scratch project, apply `cmds` with
   * `bim sketch`, return `bim show` of `ids`. `setup(dir)` may prepare the project first. */
  function replay(pre, cmds, ids, { setup = null } = {}) {
    const d = project("replay");
    if (setup) setup(d);
    if (pre.length) out(["apply", d, file("replay-ops", { schema: 1, ops: pre.map(createOp) })]);
    out(["sketch", d, file("replay-sk", { schema: 1, commands: cmds })]);
    return Object.fromEntries(ids.map((id) => [id, show(d, id)]));
  }
  return { bim, out, show, file, project, syncServe, peer, replay, kill };
}
