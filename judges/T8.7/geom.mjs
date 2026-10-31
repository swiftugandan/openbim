// Independent geometry for the direct-modelling judges: the spec/sketch.md camera model (perspective,
// up +z, vertical fov, viewport in CSS px), Shape props read back from `bim show`, closed-form volume,
// the closed 2-manifold rule, ray casting against planar faces with holes, screen-space clearances
// used to choose pointer positions, and an order-tolerant comparison of two Shapes.

export const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
export const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
export const mulv = (a, s) => [a[0] * s, a[1] * s, a[2] * s];
export const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
export const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
export const len = (a) => Math.hypot(a[0], a[1], a[2]);
export const norm = (a) => mulv(a, 1 / len(a));
export const dist = (a, b) => len(sub(a, b));
export const I4 = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
export const translation = (x, y, z) => [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, z, 1];
export const xf = (m, p) => [0, 1, 2].map((i) => m[i] * p[0] + m[4 + i] * p[1] + m[8 + i] * p[2] + m[12 + i]);

/** Camera {eye, target, fov} over a W x H CSS-px viewport (spec/sketch.md Inference, spec/web.md setCamera). */
export function camera({ eye, target, fov }, W, H) {
  const f = norm(sub(target, eye)), r = norm(cross(f, [0, 0, 1])), u = cross(r, f), t = Math.tan(fov / 2), asp = W / H;
  return {
    eye, target, fov, W, H, f, r, u,
    /** World point -> [x, y] CSS px from the viewport's top-left (null when behind the eye). */
    project(p) {
      const d = sub(p, eye), zc = dot(d, f);
      if (zc <= 1e-9) return null;
      return [W / 2 + (dot(d, r) / zc / (t * asp)) * W / 2, H / 2 - (dot(d, u) / zc / t) * H / 2];
    },
    /** CSS px -> world ray {o, d} (d unit). */
    ray([x, y]) {
      const nx = (x - W / 2) / (W / 2) * t * asp, ny = (H / 2 - y) / (H / 2) * t;
      return { o: eye, d: norm(add(f, add(mulv(r, nx), mulv(u, ny)))) };
    },
  };
}

/** Angle in radians between the view directions of two cameras. */
export const viewAngle = (a, b) => Math.acos(Math.max(-1, Math.min(1, dot(norm(sub(a.target, a.eye)), norm(sub(b.target, b.eye))))));

// ------------------------------------------------------------------------------------- shapes
const refId = (v) => (v && typeof v === "object" ? v.ref : v);
const nums = (v, what) => {
  if (!Array.isArray(v) || v.some((x) => typeof x !== "number" || !Number.isFinite(x))) throw new Error(`${what} is not a list of numbers`);
  return v;
};

/** Parse a Shape from `bim show --json` into world-space faces (spec/sketch.md Shape props). */
export function shape(show) {
  const p = show.props || {};
  const flat = nums(p.vertices, `${show.id}.vertices`);
  if (flat.length % 3) throw new Error(`${show.id}.vertices length ${flat.length} is not a multiple of 3`);
  const T = p.transform === undefined ? I4 : nums(p.transform, `${show.id}.transform`);
  if (T.length !== 16) throw new Error(`${show.id}.transform has ${T.length} numbers, want 16`);
  const L = [];
  for (let i = 0; i < flat.length; i += 3) L.push([flat[i], flat[i + 1], flat[i + 2]]);
  const V = L.map((q) => xf(T, q));
  const faces = p.faces ?? [], ids = p.face_ids ?? [], prods = p.face_products;
  if (!Array.isArray(faces) || !Array.isArray(ids) || faces.length !== ids.length)
    throw new Error(`${show.id}: faces (${faces?.length}) and face_ids (${ids?.length}) differ in length`);
  const at = (i) => { if (!Number.isInteger(i) || i < 0 || i >= V.length) throw new Error(`${show.id}: vertex index ${i} out of range`); return i; };
  const F = faces.map((loops, k) => ({
    id: ids[k],
    idx: loops.map((loop) => loop.map(at)),
    loops: loops.map((loop) => loop.map((i) => V[at(i)])),
    local: loops.map((loop) => loop.map((i) => L[at(i)])),
    product: prods ? (refId(prods[k]) ?? "") : "",
  }));
  const e = p.edges ?? [];
  const E = [];
  for (let i = 0; i + 1 < e.length; i += 2) E.push([V[at(e[i])], V[at(e[i + 1])]]);
  const EL = [];
  for (let i = 0; i + 1 < e.length; i += 2) EL.push([L[at(e[i])], L[at(e[i + 1])]]);
  return { id: show.id, kind: show.kind, props: p, T, V, L, faces: F, loose: E, looseLocal: EL };
}

/** Area vector of a face (sum over all loops of the fan from the first vertex; holes run clockwise). */
export function areaVec(f) {
  const c = f.loops[0][0];
  let a = [0, 0, 0];
  for (const loop of f.loops) for (let k = 0; k < loop.length; k++) a = add(a, mulv(cross(sub(loop[k], c), sub(loop[(k + 1) % loop.length], c)), 0.5));
  return a;
}
export const faceArea = (f) => len(areaVec(f));
export const faceNormal = (f) => norm(areaVec(f));

/** Signed volume by the divergence theorem (positive for outward normals). */
export function volume(s) {
  let v = 0;
  for (const f of s.faces) {
    const c = f.loops[0][0];
    for (const loop of f.loops) for (let k = 0; k < loop.length; k++) v += dot(c, cross(loop[k], loop[(k + 1) % loop.length])) / 6;
  }
  return v;
}

const key = (p) => p.map((x) => Math.round(x * 1e6)).join(",");
/** spec/sketch.md: every edge on exactly two faces, used in opposite directions. */
export function closedManifold(s) {
  const use = new Map();
  for (const f of s.faces) for (const loop of f.loops) for (let k = 0; k < loop.length; k++) {
    const d = `${key(loop[k])}>${key(loop[(k + 1) % loop.length])}`;
    use.set(d, (use.get(d) || 0) + 1);
  }
  if (!use.size) return false;
  for (const [d, n] of use) {
    if (n !== 1) return false;
    const [a, b] = d.split(">");
    if (use.get(`${b}>${a}`) !== 1) return false;
  }
  return true;
}

export function bbox(points) {
  const lo = [Infinity, Infinity, Infinity], hi = [-Infinity, -Infinity, -Infinity];
  for (const p of points) for (let k = 0; k < 3; k++) { lo[k] = Math.min(lo[k], p[k]); hi[k] = Math.max(hi[k], p[k]); }
  return { lo, hi };
}
export const boxCorners = ({ lo, hi }) => [0, 1, 2, 3, 4, 5, 6, 7].map((i) => [i & 1 ? hi[0] : lo[0], i & 2 ? hi[1] : lo[1], i & 4 ? hi[2] : lo[2]]);
export const faceCentroid = (f) => mulv(f.loops[0].reduce(add, [0, 0, 0]), 1 / f.loops[0].length);

/** World-space edges of a set of shapes (face boundaries and loose edges). */
export function edgesOf(shapes) {
  const out = [];
  for (const s of shapes) {
    for (const f of s.faces) for (const loop of f.loops) for (let k = 0; k < loop.length; k++) out.push([loop[k], loop[(k + 1) % loop.length]]);
    for (const e of s.loose) out.push(e);
  }
  return out;
}

// ------------------------------------------------------------------------------- ray casting
function insideFace(f, p) {
  const n = areaVec(f), ax = [0, 1, 2].reduce((m, k) => (Math.abs(n[k]) > Math.abs(n[m]) ? k : m), 0);
  const [i, j] = [0, 1, 2].filter((k) => k !== ax);
  let inside = false;
  for (const loop of f.loops) for (let k = 0, m = loop.length - 1; k < loop.length; m = k++) {
    const a = loop[k], b = loop[m];
    if ((a[j] > p[j]) !== (b[j] > p[j]) && p[i] < ((b[i] - a[i]) * (p[j] - a[j])) / (b[j] - a[j]) + a[i]) inside = !inside;
  }
  return inside;
}
/** Nearest face hit by a ray: {t, shape, face} or null. */
export function raycast(shapes, { o, d }) {
  let best = null;
  for (const s of shapes) for (const f of s.faces) {
    const a = areaVec(f);
    if (len(a) < 1e-12) continue;
    const n = norm(a), den = dot(n, d);
    if (Math.abs(den) < 1e-12) continue;
    const t = dot(n, sub(f.loops[0][0], o)) / den;
    if (t <= 1e-9 || (best && t >= best.t)) continue;
    if (insideFace(f, add(o, mulv(d, t)))) best = { t, shape: s.id, face: f.id };
  }
  return best;
}

// --------------------------------------------------------------------------- screen clearance
export function segDist2([px, py], a, b) {
  const dx = b[0] - a[0], dy = b[1] - a[1], L = dx * dx + dy * dy;
  const t = L ? Math.max(0, Math.min(1, ((px - a[0]) * dx + (py - a[1]) * dy) / L)) : 0;
  return Math.hypot(px - a[0] - t * dx, py - a[1] - t * dy);
}
/** Screen distance from a pixel to the projection of the infinite 3D line through p along dir. */
export function lineDist(cam, px, p, dir) {
  const a = cam.project(p), b = cam.project(add(p, mulv(norm(dir), 0.5)));
  if (!a || !b) return Infinity;
  const dx = b[0] - a[0], dy = b[1] - a[1], L = Math.hypot(dx, dy);
  if (L < 1e-9) return Math.hypot(px[0] - a[0], px[1] - a[1]);
  return Math.abs((px[0] - a[0]) * dy - (px[1] - a[1]) * dx) / L;
}
/** Smallest screen distance from a pixel to any projected world edge (edges behind the eye ignored). */
export function edgeClearance(cam, px, edges) {
  let m = Infinity;
  for (const [a, b] of edges) {
    const pa = cam.project(a), pb = cam.project(b);
    if (pa && pb) m = Math.min(m, segDist2(px, pa, pb));
  }
  return m;
}
export const onScreen = (cam, px, margin) => !!px && px[0] >= margin && px[1] >= margin && px[0] <= cam.W - margin && px[1] <= cam.H - margin;

/** A point inside face (shapeId, faceId) whose pixel sees that face first and is clear of every edge
 * (so `on_face` is the winning inference there); the clearest one, or the best by `score(p)`. Returns {p, px} or null. */
export function facePoint(cam, shapes, shapeId, faceId, { clear = 16, extraEdges = [], score = null } = {}) {
  const s = shapes.find((x) => x.id === shapeId);
  const f = s && s.faces.find((x) => x.id === faceId);
  if (!f) return null;
  const edges = [...edgesOf(shapes), ...extraEdges];
  const n = faceNormal(f), o = f.loops[0][0];
  const e1 = norm(sub(f.loops[0][1], o)), e2 = cross(n, e1);
  const pts = f.loops[0].map((q) => [dot(sub(q, o), e1), dot(sub(q, o), e2)]);
  const lo = [Math.min(...pts.map((q) => q[0])), Math.min(...pts.map((q) => q[1]))];
  const hi = [Math.max(...pts.map((q) => q[0])), Math.max(...pts.map((q) => q[1]))];
  let best = null;
  const N = 12;
  for (let i = 1; i < N; i++) for (let j = 1; j < N; j++) {
    const p = add(o, add(mulv(e1, lo[0] + (hi[0] - lo[0]) * i / N), mulv(e2, lo[1] + (hi[1] - lo[1]) * j / N)));
    if (!insideFace(f, p)) continue;
    const px = cam.project(p);
    if (!onScreen(cam, px, 12)) continue;
    const hit = raycast(shapes, cam.ray(px));
    if (!hit || hit.shape !== shapeId || hit.face !== faceId) continue;
    const c = edgeClearance(cam, px, edges);
    const sc = score ? score(p) : c;
    if (c >= clear && (!best || sc > best.sc)) best = { p: add(cam.eye, mulv(cam.ray(px).d, hit.t)), px, c, sc };
  }
  return best;
}

/** Nearest projected vertex distance (other than `except`). */
export function vertexClearance(cam, px, points, except = null) {
  let m = Infinity;
  for (const q of points) {
    if (except && dist(q, except) < 1e-9) continue;
    const pq = cam.project(q);
    if (pq) m = Math.min(m, Math.hypot(pq[0] - px[0], pq[1] - px[1]));
  }
  return m;
}

// --------------------------------------------------------------------------- shape comparison
const TOL = 1e-7;
const samePt = (a, b) => Math.abs(a[0] - b[0]) <= TOL && Math.abs(a[1] - b[1]) <= TOL && Math.abs(a[2] - b[2]) <= TOL;
function sameLoop(a, b) {
  if (a.length !== b.length) return false;
  for (let r = 0; r < a.length; r++) if (a.every((p, k) => samePt(p, b[(k + r) % b.length]))) return true;
  return false;
}
function matchAll(as, bs, eq) {
  if (as.length !== bs.length) return false;
  const used = new Array(bs.length).fill(false);
  for (const a of as) {
    const j = bs.findIndex((b, k) => !used[k] && eq(a, b));
    if (j < 0) return false;
    used[j] = true;
  }
  return true;
}
const sameFace = (a, b) => a.product === b.product && sameLoop(a.local[0], b.local[0]) && matchAll(a.local.slice(1), b.local.slice(1), sameLoop);

/** Compare two parsed Shapes (local geometry, transform, face ids, face products, loose edges).
 * Faces whose id is in `fixedIds` must match by id; the other faces match as a multiset (copies may be
 * created in either order). Returns null when equal, else a reason. */
export function compareShapes(got, want, fixedIds = null) {
  for (let k = 0; k < 16; k++) if (Math.abs(got.T[k] - want.T[k]) > 1e-9) return `transform ${JSON.stringify(got.T)} != ${JSON.stringify(want.T)}`;
  if (got.faces.length !== want.faces.length) return `${got.faces.length} faces, want ${want.faces.length}`;
  const gi = got.faces.map((f) => f.id).sort((a, b) => a - b), wi = want.faces.map((f) => f.id).sort((a, b) => a - b);
  if (JSON.stringify(gi) !== JSON.stringify(wi)) return `face ids ${JSON.stringify(gi)}, want ${JSON.stringify(wi)}`;
  const fixed = fixedIds ? new Set(fixedIds) : new Set(wi);
  for (const w of want.faces.filter((f) => fixed.has(f.id))) {
    const g = got.faces.find((f) => f.id === w.id);
    if (!sameFace(g, w)) return `face ${w.id} is ${JSON.stringify(g.local)} (product '${g.product}'), want ${JSON.stringify(w.local)} (product '${w.product}')`;
  }
  if (!matchAll(got.faces.filter((f) => !fixed.has(f.id)), want.faces.filter((f) => !fixed.has(f.id)), sameFace)) return "new faces differ";
  const se = (a, b) => (samePt(a[0], b[0]) && samePt(a[1], b[1])) || (samePt(a[0], b[1]) && samePt(a[1], b[0]));
  if (!matchAll(got.looseLocal, want.looseLocal, se)) return `loose edges ${JSON.stringify(got.looseLocal)}, want ${JSON.stringify(want.looseLocal)}`;
  return null;
}
