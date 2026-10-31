// T8.6 part 1 (Node): bim-wasm `Model.sketch` against `bim sketch` (same resulting hash, atomic on
// error) and `Model.infer` against closed-form expectations on a constructed scene (spec/web.md,
// spec/sketch.md Inference). Usage: node wasm.mjs <bim> <tmp-dir>
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT } from "../lib/oracle.mjs";
import { JudgeFail, fail, check } from "./studio.mjs";
import { cli } from "./peer.mjs";
import { camera, add, sub, mulv, dot, cross, norm, dist, lineDist, edgeClearance, vertexClearance } from "./geom.mjs";

const [BIM, tmp] = process.argv.slice(2);
setTimeout(() => { console.error("JUDGE FAIL: T8.6 wasm checks did not finish within 60s"); process.exit(1); }, 60_000).unref();

const BASE = [{ op: "create", id: "L0", kind: "Level", props: { elevation: 0 } }];
const STEP1 = [
  { op: "rect", shape: "box", corner: [0, 0, 0], u: [4, 0, 0], v: [0, 3, 0] },
  { op: "pushpull", shape: "box", face: 1, distance: 2.5 },
  { op: "line", shape: "box", from: [2, 0, 2.5], to: [2, 3, 2.5] },
  { op: "circle", shape: "disc", centre: [8, 0, 0], normal: [0, 0, 1], radius: 1 },
  { op: "arc", shape: "bow", centre: [0, -4, 0], start: [1, -4, 0], normal: [0, 0, 1], angle: Math.PI / 2, segments: 6 },
  { op: "rect", shape: "wall", corner: [6, 4, 0], u: [2, 0, 0], v: [0, 0, 2] },
  { op: "pushpull", shape: "wall", face: 1, distance: -0.3, new: true },
];

let code = 0;
try {
  const C = cli(BIM, tmp);
  const pkgDir = path.join(ROOT, "packages/bim-wasm/pkg-node");
  check(fs.existsSync(path.join(pkgDir, "package.json")), "packages/bim-wasm/pkg-node/package.json missing (build.sh must produce it)");
  const meta = JSON.parse(fs.readFileSync(path.join(pkgDir, "package.json"), "utf8"));
  let wasm;
  try { wasm = await import(pathToFileURL(path.join(pkgDir, meta.module ?? meta.main)).href); } catch (e) { fail(`cannot import pkg-node: ${e.message}`); }
  const Model = wasm.Model ?? wasm.default?.Model;
  check(typeof Model?.fromOps === "function", "pkg-node does not export Model.fromOps");
  const m = Model.fromOps(JSON.stringify({ schema: 1, ops: BASE }));
  check(typeof m.sketch === "function" && typeof m.infer === "function", "wasm Model lacks sketch(json) / infer(json) (spec/web.md, added by T8.6)");

  // ---------------------------------------------------------------- 1. Model.sketch == bim sketch
  const d = C.project("cli", BASE);
  const hashCli = () => C.out(["hash", d]).trim();
  const sk = (cmds) => JSON.stringify({ schema: 1, commands: cmds });
  const step = (cmds, label) => {
    C.out(["sketch", d, C.file("step", sk(cmds))]);
    try { m.sketch(sk(cmds)); } catch (e) { fail(`Model.sketch threw on ${label}: ${e}`); }
    check(m.hash() === hashCli(), `${label}: Model.hash() ${m.hash()} != bim hash ${hashCli()} after the same sketch commands`);
  };
  step(STEP1, "rect/pushpull/line/circle/arc/pushpull new");
  // erase the line that split the top face (indices read from the CLI's own Shape)
  const box = C.show(d, "box");
  const V = box.props.vertices;
  const idx = (p) => { for (let i = 0; i < V.length; i += 3) if (Math.hypot(V[i] - p[0], V[i + 1] - p[1], V[i + 2] - p[2]) < 1e-9) return i / 3; fail(`bim sketch Shape box has no vertex at ${p}`); };
  step([{ op: "erase", shape: "box", edges: [[idx([2, 0, 2.5]), idx([2, 3, 2.5])]] }], "erase");
  // atomicity: a valid command followed by an invalid one changes nothing, and throws
  const h0 = m.hash();
  const bad = [{ op: "rect", shape: "x", corner: [20, 0, 0], u: [1, 0, 0], v: [0, 1, 0] }, { op: "pushpull", shape: "box", face: 999, distance: 1 }];
  let threw = false;
  try { m.sketch(sk(bad)); } catch { threw = true; }
  check(threw, "Model.sketch did not throw on a pushpull of a missing face");
  check(m.hash() === h0, "Model.sketch changed the model although a command was invalid (must be atomic)");
  const r = C.bim(["sketch", d, C.file("bad", sk(bad))], { ok: [0, 1, 2, 3] });
  check(r.status === 2 && hashCli() === h0, `bim sketch on the invalid file exited ${r.status} (want 2) or changed the document`);

  // ---------------------------------------------------------------- 2. Model.infer on a known scene
  const mi = Model.fromOps(JSON.stringify({ schema: 1, ops: [...BASE, { op: "create", id: "g1", kind: "Guide", props: { point: [10.5, 0, 0], direction: [0, 1, 0] } }] }));
  mi.sketch(sk(STEP1.slice(0, 2)));
  const top = C.show(C.project("top", BASE, [STEP1.slice(0, 2)]), "box");
  const frontId = top.props.face_ids[top.props.faces.findIndex((f) => f[0].every((i) => Math.abs(top.props.vertices[3 * i + 1]) < 1e-9))];
  check(frontId !== undefined, "bim sketch rect + pushpull 2.5 has no face at y = 0");
  const CAM = { eye: [12, -10, 4], target: [4, 0, 0.8], fov: 0.8, width: 1000, height: 700 };
  const cam = camera(CAM, CAM.width, CAM.height);
  const P = (p, dx = 0, dy = 0) => { const q = cam.project(p); return [q[0] + dx, q[1] + dy]; };
  const corners = []; for (const x of [0, 4]) for (const y of [0, 3]) for (const z of [0, 2.5]) corners.push([x, y, z]);
  const boxEdges = [];
  for (const a of corners) for (const b of corners) if (a < b && [0, 1, 2].filter((k) => a[k] !== b[k]).length === 1) boxEdges.push([a, b]);
  const guide = [[10.5, -3, 0], [10.5, 40, 0]];
  const infer = (q, label) => {
    let out;
    try { out = JSON.parse(mi.infer(JSON.stringify({ camera: CAM, ...q }))); } catch (e) { fail(`Model.infer threw or returned non-JSON (${label}): ${e}`); }
    check(out && typeof out.kind === "string", `Model.infer (${label}) returned ${JSON.stringify(out)}, want {point, kind, ref}`);
    return out;
  };
  const expect = (label, q, kind, point, extra = () => null) => {
    const o = infer(q, label);
    check(o.kind === kind, `Model.infer ${label}: kind '${o.kind}', want '${kind}' (${JSON.stringify(o)})`);
    if (point === null) check(o.point === null, `Model.infer ${label}: point ${JSON.stringify(o.point)}, want null`);
    else check(Array.isArray(o.point) && o.point.length === 3 && dist(o.point, point) < 1e-6, `Model.infer ${label}: point ${JSON.stringify(o.point)}, want ${JSON.stringify(point)}`);
    const why = extra(o);
    if (why) fail(`Model.infer ${label}: ${why}`);
  };
  const entityIs = (id) => (o) => (o.ref && o.ref.entity === id ? null : `ref ${JSON.stringify(o.ref)}, want entity '${id}'`);
  // Preconditions of the constructed scene (judge self-check): the target is the only candidate.
  const clear = (px, label, { lines = [], noGuide = false } = {}) => {
    const ec = edgeClearance(cam, px, noGuide ? boxEdges : [...boxEdges, guide]);
    const vc = vertexClearance(cam, px, corners);
    const lc = Math.min(Infinity, ...lines.map(([p, dir]) => lineDist(cam, px, p, dir)));
    if (Math.min(ec, vc, lc) < 14) throw new Error(`judge scene precondition failed for ${label}: clearance ${Math.min(ec, vc, lc).toFixed(1)} px`);
  };
  const FROM = [8, -3, 0];
  const X = [1, 0, 0], Y = [0, 1, 0], Z = [0, 0, 1];
  const at = (p) => add(FROM, p);

  expect("at a corner (3 px off)", { cursor: P([4, 0, 0], 3, -2) }, "endpoint", [4, 0, 0], entityIs("box"));
  expect("at an edge midpoint", { cursor: P([2, 0, 0], 2, 2) }, "midpoint", [2, 0, 0], entityIs("box"));
  check(vertexClearance(cam, P([1, 0, 0]), corners) > 25, "judge precondition: on_edge point too close to a corner");
  expect("on an edge", { cursor: P([1, 0, 0]) }, "on_edge", [1, 0, 0], entityIs("box"));
  clear(P([2, 0, 1.25]), "on_face");
  expect("on the front face", { cursor: P([2, 0, 1.25]) }, "on_face", [2, 0, 1.25],
    (o) => (o.ref && o.ref.entity === "box" && o.ref.id === frontId ? null : `ref ${JSON.stringify(o.ref)}, want {entity: 'box', id: ${frontId}}`));
  clear(P([8, -3, 0]), "ground");
  expect("on the ground", { cursor: P([8, -3, 0]) }, "ground", [8, -3, 0]);
  check(cam.ray([500, 3]).d[2] > 0, "judge precondition: top row must look above the horizon");
  expect("in the sky", { cursor: [500, 3] }, "none", null);
  clear(P(at(mulv(X, 1.5))), "axis_x", { lines: [[FROM, Y], [FROM, Z]] });
  expect("along x from a point", { cursor: P(at(mulv(X, 1.5))), from: FROM }, "axis_x", at(mulv(X, 1.5)));
  clear(P(at(mulv(Y, -1.5))), "axis_y", { lines: [[FROM, X], [FROM, Z]] });
  expect("along y from a point", { cursor: P(at(mulv(Y, -1.5))), from: FROM }, "axis_y", at(mulv(Y, -1.5)));
  clear(P(at(mulv(Z, 1.5))), "axis_z", { lines: [[FROM, X], [FROM, Y]] });
  expect("along z from a point", { cursor: P(at(mulv(Z, 1.5))), from: FROM }, "axis_z", at(mulv(Z, 1.5)));
  clear(P([10.5, -2, 0]), "guide", { noGuide: true });
  expect("on a guide line", { cursor: P([10.5, -2, 0]) }, "guide", [10.5, -2, 0], entityIs("g1"));
  const REF = [[0, 0, 0], [3, 4, 0]], PAR = norm(sub(REF[1], REF[0]));
  // spec/sketch.md: the perpendicular line through `from` lies in the plane containing `from` and the reference
  // edge (here z = 0, so it is z x d); n = d x (from - a) is that plane's normal, the direction is n x d.
  const PERP = norm(cross(cross(PAR, sub(FROM, REF[0])), PAR));
  check(Math.abs(PERP[2]) < 1e-12 && Math.abs(dot(PERP, PAR)) < 1e-12, "judge: perpendicular direction is not horizontal");
  clear(P(at(mulv(PAR, 1.5))), "parallel", { lines: [[FROM, X], [FROM, Y], [FROM, Z]] });
  expect("parallel to the reference edge", { cursor: P(at(mulv(PAR, 1.5))), from: FROM, reference_edge: REF }, "parallel", at(mulv(PAR, 1.5)));
  clear(P(at(mulv(PERP, 1.5))), "perpendicular", { lines: [[FROM, X], [FROM, Y], [FROM, Z]] });
  expect("perpendicular to the reference edge", { cursor: P(at(mulv(PERP, 1.5))), from: FROM, reference_edge: REF }, "perpendicular", at(mulv(PERP, 1.5)));
  // the engine follows model changes: geometry sketched after earlier queries is inferred
  mi.sketch(sk([{ op: "rect", shape: "late", corner: [6, 4, 0], u: [1, 0, 0], v: [0, 1, 0] }]));
  expect("at a corner sketched after earlier queries", { cursor: P([7, 5, 0], -2, 1) }, "endpoint", [7, 5, 0], entityIs("late"));
  console.log("T8.6 wasm checks passed");
} catch (e) {
  code = 1;
  console.error(e instanceof JudgeFail ? `JUDGE FAIL: ${e.message}` : `JUDGE FAIL: unexpected error: ${e && e.stack || e}`);
}
process.exit(code);
