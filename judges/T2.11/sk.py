"""Shared helpers for the direct-modelling judges (spec/sketch.md): drive the real `bim`, read Shapes
back with `bim show --json`, and recompute everything from the Shape's own vertices and loops
(planarity, areas, manifoldness, volume, Euler characteristic) plus the glTF export.

Nothing here depends on vertex order: vertices are found by coordinates (spec: within 1e-9 m points are
equal) and faces by their stable ids.
"""
import base64
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile

import numpy as np
import trimesh

EPS = 1e-9            # spec/sketch.md + TASKS.md: points within 1e-9 m are equal; faces planar within 1e-9 m
PLANAR = 1e-9 + 1e-12 # the same bound with room for the judge's own f64 plane fit
VOL_REL = 1e-9        # exact f64 volumes recomputed from the Shape (closed forms are exact in f64 to ~1e-15)
GLTF_REL = 2e-5       # glTF POSITION accessors are float32
HERE = os.path.dirname(os.path.abspath(__file__))
TAG = "T2.x"


class JudgeFail(Exception):
    pass


def fail(msg):
    raise JudgeFail(msg)


def close(got, want, what, rel=VOL_REL, abs_tol=1e-12):
    if not isinstance(got, (int, float)) or isinstance(got, bool) or not math.isfinite(got):
        fail(f"{what}: expected a finite number, got {got!r}")
    if abs(got - want) > rel * abs(want) + abs_tol:
        fail(f"{what}: got {got!r}, want {want!r}")


def norm(v):
    v = np.asarray(v, float)
    n = np.linalg.norm(v)
    if n == 0:
        fail("internal: zero vector")
    return v / n


# ------------------------------------------------------------------ the bim CLI

class Bim:
    """One project directory driven through the real `bim` binary; every call has a wall-clock limit."""

    def __init__(self, exe, root, name):
        self.exe = exe
        self.dir = os.path.join(root, name)
        self.root = root
        self.n = 0
        r = self.run(["init", self.dir])
        if r.returncode != 0:
            fail(f"bim init exited {r.returncode}: {r.stderr[:300]}")

    def run(self, args, timeout=60):
        try:
            return subprocess.run([self.exe] + args, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            fail(f"`bim {' '.join(args[:1])}` timed out after {timeout} s")

    def _file(self, obj, stem):
        self.n += 1
        p = os.path.join(self.root, f"{stem}-{os.path.basename(self.dir)}-{self.n}.json")
        with open(p, "w") as f:
            f.write(obj if isinstance(obj, str) else json.dumps(obj))
        return p

    def sketch_rc(self, commands, raw=None):
        p = self._file(raw if raw is not None else {"schema": 1, "commands": commands}, "sketch")
        return self.run(["sketch", self.dir, p])

    def sketch(self, commands, what=None):
        r = self.sketch_rc(commands)
        if r.returncode != 0:
            fail(f"bim sketch ({what or commands[0].get('op')}) exited {r.returncode} on valid commands "
                 f"{json.dumps(commands)[:300]}: {r.stderr[:300]}")

    def apply(self, ops, what="ops"):
        p = self._file({"schema": 1, "ops": ops}, "ops")
        r = self.run(["apply", self.dir, p])
        if r.returncode != 0:
            fail(f"bim apply ({what}) exited {r.returncode}: {r.stderr[:300]}")

    def hash(self):
        r = self.run(["hash", self.dir])
        h = r.stdout.strip()
        if r.returncode != 0 or len(h) != 16 or any(c not in "0123456789abcdef" for c in h):
            fail(f"bim hash should print 16 lowercase hex digits, got rc={r.returncode} {r.stdout!r}")
        return h

    def show(self, eid):
        r = self.run(["show", self.dir, eid, "--json"])
        if r.returncode != 0:
            fail(f"bim show {eid} --json exited {r.returncode}: {r.stderr[:300]}")
        try:
            doc = json.loads(r.stdout)
        except json.JSONDecodeError as e:
            fail(f"bim show {eid} --json is not one JSON document: {e}")
        if not isinstance(doc, dict) or not isinstance(doc.get("props"), dict):
            fail(f"bim show {eid} --json has no props object: {r.stdout[:200]}")
        return doc

    def shape(self, eid):
        doc = self.show(eid)
        if doc.get("kind") != "Shape":
            fail(f"{eid}: kind {doc.get('kind')!r}, want Shape")
        return Shape(eid, doc["props"])

    def gltf(self, tag):
        out = os.path.join(self.root, f"{os.path.basename(self.dir)}-{tag}.gltf")
        r = self.run(["export", self.dir, "--format", "gltf", "--out", out], timeout=120)
        if r.returncode != 0 or not os.path.isfile(out) or os.path.getsize(out) == 0:
            fail(f"bim export --format gltf ({tag}) exited {r.returncode}: {r.stderr[:300]}")
        try:
            v = subprocess.run(["node", os.path.join(HERE, "validate_gltf.mjs"), out],
                               capture_output=True, text=True, timeout=120)
        except subprocess.TimeoutExpired:
            fail("glTF-Validator timed out")
        if v.returncode != 0:
            fail(f"({tag}) {v.stdout.strip()[:400]} {v.stderr.strip()[:400]}")
        return load_gltf(out)


# ------------------------------------------------------------------ Shapes

def as_int(x, what):
    if isinstance(x, bool) or not isinstance(x, (int, float)) or not math.isfinite(x) or x != int(x):
        fail(f"{what}: expected an integer Num, got {x!r}")
    return int(x)


IDENTITY = [1.0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]


class Shape:
    """A Shape read from `bim show`, validated against spec/sketch.md's structural rules on load."""

    def __init__(self, eid, props):
        self.id = eid
        self.props = props
        vs = props.get("vertices")
        if not isinstance(vs, list) or len(vs) % 3 or not all(
                isinstance(x, (int, float)) and not isinstance(x, bool) and math.isfinite(x) for x in vs):
            fail(f"{eid}: vertices must be a flat List[Num] of x, y, z triples, got {str(vs)[:120]}")
        self.V = np.array(vs, float).reshape(-1, 3)
        nv = len(self.V)
        fs = props.get("faces")
        ids = props.get("face_ids")
        if not isinstance(fs, list) or not isinstance(ids, list) or len(fs) != len(ids):
            fail(f"{eid}: faces and face_ids must be Lists of equal length")
        self.ids = [as_int(i, f"{eid}: face_ids") for i in ids]
        if len(set(self.ids)) != len(self.ids):
            fail(f"{eid}: face_ids {self.ids} are not distinct")
        self.faces = {}
        self.order = list(self.ids)
        for fid, f in zip(self.ids, fs):
            if not isinstance(f, list) or not f:
                fail(f"{eid}: face {fid} must be a non-empty List of loops")
            loops = []
            for lp in f:
                if not isinstance(lp, list) or len(lp) < 3:
                    fail(f"{eid}: face {fid} has a loop with fewer than 3 vertices: {lp!r}")
                ix = [as_int(i, f"{eid}: face {fid} loop index") for i in lp]
                if any(i < 0 or i >= nv for i in ix):
                    fail(f"{eid}: face {fid} indexes a vertex outside 0..{nv - 1}")
                if len(set(ix)) != len(ix):
                    fail(f"{eid}: face {fid} has a loop repeating a vertex: {ix}")
                loops.append(ix)
            self.faces[fid] = loops
        es = props.get("edges", [])
        if not isinstance(es, list) or len(es) % 2:
            fail(f"{eid}: edges must be a flat List[Num] of index pairs")
        es = [as_int(i, f"{eid}: edges") for i in es]
        if any(i < 0 or i >= nv for i in es):
            fail(f"{eid}: loose edge indexes a vertex outside 0..{nv - 1}")
        self.loose = [tuple(sorted(es[k:k + 2])) for k in range(0, len(es), 2)]
        if len(set(self.loose)) != len(self.loose) or any(a == b for a, b in self.loose):
            fail(f"{eid}: loose edges repeat or are degenerate: {self.loose}")
        # Distinct vertices (spec: within 1e-9 m points are equal, so two vertices closer than that are one).
        if nv > 1:
            from scipy.spatial import cKDTree
            pairs = cKDTree(self.V).query_pairs(EPS)
            if pairs:
                a, b = next(iter(pairs))
                fail(f"{eid}: vertices {a} and {b} are within 1e-9 m ({self.V[a]}) and should be one vertex")
        self.normals = {}
        for fid, loops in self.faces.items():
            n = newell(self.V[loops[0]])
            if np.linalg.norm(n) < 1e-12:
                fail(f"{eid}: face {fid} outer loop is degenerate (zero area)")
            n = n / np.linalg.norm(n)
            d = float(n @ self.V[loops[0][0]])
            for lp in loops:
                dev = np.max(np.abs(self.V[lp] @ n - d))
                if dev > PLANAR:
                    fail(f"{eid}: face {fid} is not planar (a vertex is {dev:.3g} m off its plane)")
            for h in loops[1:]:
                if newell(self.V[h]) @ n >= 0:
                    fail(f"{eid}: face {fid} has a hole loop that is not clockwise seen from the front")
            self.normals[fid] = n
        # Loose edges are edges on no face.
        fe = self.face_edges()
        for e in self.loose:
            if e in fe:
                fail(f"{eid}: edges lists {e}, which lies on a face (edges holds loose edges only)")
        fp = props.get("face_products")
        if fp is not None and (not isinstance(fp, list) or len(fp) != len(fs)):
            fail(f"{eid}: face_products must have one entry per face")
        tr = props.get("transform")
        if tr is not None and (not isinstance(tr, list) or len(tr) != 16):
            fail(f"{eid}: transform must be 16 numbers")

    # --- topology
    def face_edges(self):
        out = set()
        for loops in self.faces.values():
            for lp in loops:
                for k in range(len(lp)):
                    out.add(tuple(sorted((lp[k], lp[(k + 1) % len(lp)]))))
        return out

    def directed(self):
        d = {}
        for fid, loops in self.faces.items():
            for lp in loops:
                for k in range(len(lp)):
                    e = (lp[k], lp[(k + 1) % len(lp)])
                    d[e] = d.get(e, 0) + 1
        return d

    def is_closed(self):
        """spec/sketch.md: every edge on exactly two faces, used in opposite directions."""
        if not self.faces:
            return False
        d = self.directed()
        return all(c == 1 and d.get((b, a), 0) == 1 for (a, b), c in d.items())

    def require_closed(self, what):
        d = self.directed()
        if not self.faces:
            fail(f"{what}: {self.id} has no faces, expected a closed solid")
        for (a, b), c in d.items():
            if c != 1 or d.get((b, a), 0) != 1:
                fail(f"{what}: {self.id} is not a closed, consistently oriented 2-manifold "
                     f"(edge {self.V[a]}->{self.V[b]} used {c} time(s), reverse {d.get((b, a), 0)})")
        if self.loose:
            fail(f"{what}: {self.id} should have no loose edges, has {len(self.loose)}")

    def euler(self):
        """V - E + F - H (H = hole loops) over the vertices used by faces: = 2 - 2g for a closed connected solid."""
        used = {i for loops in self.faces.values() for lp in loops for i in lp}
        holes = sum(len(l) - 1 for l in self.faces.values())
        return len(used) - len(self.face_edges()) + len(self.faces) - holes

    # --- geometry
    def volume(self):
        v = 0.0
        for loops in self.faces.values():
            for lp in loops:
                p = self.V[lp]
                for k in range(1, len(p) - 1):
                    v += float(np.dot(p[0], np.cross(p[k], p[k + 1])))
        return v / 6.0

    def area(self, fid):
        loops = self.face(fid)
        n = self.normals[fid]
        return float(sum(newell(self.V[lp]) @ n for lp in loops))

    def total_area(self):
        return sum(self.area(f) for f in self.faces)

    def face(self, fid):
        if fid not in self.faces:
            fail(f"{self.id}: no face with id {fid} (face ids {sorted(self.faces)})")
        return self.faces[fid]

    def plane(self, fid):
        n = self.normals[fid]
        return n, float(n @ self.V[self.face(fid)[0][0]])

    def centroid(self, fid):
        """Area centroid of the face (outer minus holes) by a fan of signed triangles."""
        n = self.normals[fid]
        acc, tot = np.zeros(3), 0.0
        for lp in self.face(fid):
            p = self.V[lp]
            for k in range(1, len(p) - 1):
                a = float(np.cross(p[k] - p[0], p[k + 1] - p[0]) @ n) / 2
                acc += a * (p[0] + p[k] + p[k + 1]) / 3
                tot += a
        return acc / tot

    def points(self, fid, loop=None):
        loops = self.face(fid)
        idx = [i for lp in (loops if loop is None else [loops[loop]]) for i in lp]
        return self.V[idx]

    def find(self, p, what=""):
        d = np.linalg.norm(self.V - np.asarray(p, float), axis=1) if len(self.V) else np.array([])
        hits = np.nonzero(d <= EPS * 10)[0]
        if len(hits) != 1:
            fail(f"{self.id}: expected exactly one vertex at {list(p)} {what}, found {len(hits)}")
        return int(hits[0])

    def faces_where(self, pred):
        return [f for f in self.order if pred(f)]


def newell(p):
    p = np.asarray(p, float)
    q = np.roll(p, -1, axis=0)
    return 0.5 * np.cross(p - p[0], q - p[0]).sum(axis=0)


def same_points(got, want, what, tol=EPS * 10):
    got = np.asarray(got, float).reshape(-1, 3)
    want = np.asarray(want, float).reshape(-1, 3)
    if len(got) != len(want):
        fail(f"{what}: {len(got)} points, want {len(want)}")
    used = set()
    for w in want:
        d = np.linalg.norm(got - w, axis=1)
        k = int(np.argmin(d))
        if d[k] > tol or k in used:
            fail(f"{what}: no point at {np.round(w, 9).tolist()} (nearest {np.round(got[k], 9).tolist()})")
        used.add(k)


def pts_equal(got, want, tol=EPS * 10):
    try:
        same_points(got, want, "")
        return True
    except JudgeFail:
        return False


def poly_volume(V, faces):
    """Signed volume of faces (lists of loops of points) by fans: the judge's own construction."""
    v = 0.0
    for loops in faces:
        for p in loops:
            p = np.asarray(p, float)
            for k in range(1, len(p) - 1):
                v += float(np.dot(p[0], np.cross(p[k], p[k + 1])))
    return v / 6.0


def mat(t):
    """16 numbers, column-major -> 4x4."""
    return np.array(t, float).reshape(4, 4).T


def colmajor(m):
    return [float(x) for x in np.asarray(m, float).T.reshape(-1)]


def rot(axis, ang):
    a = norm(axis)
    K = np.array([[0, -a[2], a[1]], [a[2], 0, -a[0]], [-a[1], a[0], 0]])
    return np.eye(3) + math.sin(ang) * K + (1 - math.cos(ang)) * K @ K


def transform_points(m, P):
    P = np.asarray(P, float).reshape(-1, 3)
    return (m @ np.c_[P, np.ones(len(P))].T).T[:, :3]


# ------------------------------------------------------------------ glTF

COMP = {5120: np.int8, 5121: np.uint8, 5122: np.int16, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}
NCOMP = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}
# spec/cli.md: glTF (x, y, z) = model (x, z, -y), so model = (gx, -gz, gy).
Y_UP = np.array([[1, 0, 0], [0, 0, -1], [0, 1, 0]], float)


def read_accessor(g, bufs, idx):
    acc = g["accessors"][idx]
    if "bufferView" not in acc:
        fail(f"glTF accessor {idx} has no bufferView")
    bv = g["bufferViews"][acc["bufferView"]]
    dt = np.dtype(COMP[acc["componentType"]])
    n = NCOMP[acc["type"]]
    buf = bufs[bv["buffer"]]
    start = bv.get("byteOffset", 0) + acc.get("byteOffset", 0)
    stride = bv.get("byteStride", 0) or dt.itemsize * n
    out = np.empty((acc["count"], n), dtype=np.float64)
    for i in range(acc["count"]):
        out[i] = np.frombuffer(buf, dtype=dt, count=n, offset=start + i * stride)
    return out


def trs_matrix(node):
    if "matrix" in node:
        return np.array(node["matrix"], dtype=float).reshape(4, 4).T
    t = np.array(node.get("translation", [0, 0, 0]), float)
    x, y, z, w = node.get("rotation", [0, 0, 0, 1])
    r = np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]])
    m = np.eye(4)
    m[:3, :3] = r @ np.diag(node.get("scale", [1, 1, 1]))
    m[:3, 3] = t
    return m


def load_gltf(path):
    """{extras.id: (world vertices in the model's z-up frame (n,3), triangles (m,3))}."""
    with open(path) as f:
        g = json.load(f)
    if g.get("asset", {}).get("version") != "2.0":
        fail("glTF asset.version is not 2.0")
    bufs = []
    for b in g.get("buffers", []):
        uri = b.get("uri", "")
        if not uri.startswith("data:"):
            fail("glTF buffers must be embedded (data: URIs) per spec/cli.md")
        bufs.append(base64.b64decode(uri.split(",", 1)[1]))
    meshes = {}

    def visit(ni, parent):
        node = g["nodes"][ni]
        m = parent @ trs_matrix(node)
        if "mesh" in node:
            eid = (node.get("extras") or {}).get("id")
            if eid is not None:
                if eid in meshes:
                    fail(f"glTF: entity {eid} has more than one mesh node")
                vs, fs, base = [], [], 0
                for prim in g["meshes"][node["mesh"]]["primitives"]:
                    if prim.get("mode", 4) != 4:
                        fail(f"glTF {eid}: primitive mode {prim.get('mode')} is not TRIANGLES")
                    pos = read_accessor(g, bufs, prim["attributes"]["POSITION"])
                    if "indices" in prim:
                        idx = read_accessor(g, bufs, prim["indices"]).astype(np.int64).reshape(-1, 3)
                    else:
                        idx = np.arange(len(pos)).reshape(-1, 3)
                    vs.append(pos)
                    fs.append(idx + base)
                    base += len(pos)
                v = np.vstack(vs) if vs else np.zeros((0, 3))
                v = (m @ np.c_[v, np.ones(len(v))].T).T[:, :3]
                meshes[eid] = ((Y_UP @ v.T).T, np.vstack(fs) if fs else np.zeros((0, 3), int))
        for c in node.get("children", []):
            visit(c, m)

    scene = g["scenes"][g.get("scene", 0)]
    for r in scene["nodes"]:
        visit(r, np.eye(4))
    return meshes


def gltf_solid(meshes, eid, what):
    if eid not in meshes:
        fail(f"{what}: glTF has no mesh node with extras.id {eid!r} (ids: {sorted(meshes)[:20]})")
    v, f = meshes[eid]
    tm = trimesh.Trimesh(vertices=v, faces=f, process=True)
    tm.merge_vertices(digits_vertex=6)
    tm.remove_unreferenced_vertices()
    if not tm.is_watertight:
        fail(f"{what}: glTF mesh of {eid} is not watertight")
    if not tm.is_winding_consistent:
        fail(f"{what}: glTF mesh of {eid} is not consistently wound")
    return tm


def check_gltf_matches(meshes, eid, shape_world_vertices, volume, what, euler=None, bodies=None):
    tm = gltf_solid(meshes, eid, what)
    close(float(tm.volume), volume, f"{what}: glTF {eid} volume (positive = outward winding)", GLTF_REL, 1e-7)
    if euler is not None and tm.euler_number != euler:
        fail(f"{what}: glTF {eid} Euler number {tm.euler_number}, want {euler}")
    if bodies is not None and tm.body_count != bodies:
        fail(f"{what}: glTF {eid} has {tm.body_count} bodies, want {bodies}")
    # Every mesh vertex lies on a Shape vertex or face; compare bounding boxes and vertex coverage.
    sw = np.asarray(shape_world_vertices, float)
    for w in sw:
        if np.min(np.linalg.norm(tm.vertices - w, axis=1)) > 2e-5 * max(1.0, np.abs(w).max()):
            fail(f"{what}: glTF {eid} has no vertex at the Shape's world vertex {np.round(w, 6).tolist()}")
    lo, hi = tm.vertices.min(axis=0), tm.vertices.max(axis=0)
    if np.abs(lo - sw.min(axis=0)).max() > 1e-4 or np.abs(hi - sw.max(axis=0)).max() > 1e-4:
        fail(f"{what}: glTF {eid} bounds {lo.round(6)}..{hi.round(6)} differ from the Shape's "
             f"{sw.min(axis=0).round(6)}..{sw.max(axis=0).round(6)}")
    return tm


# ------------------------------------------------------------------ test running

def main(tests, tag, exe):
    global TAG
    TAG = tag
    root = tempfile.mkdtemp(prefix=f"judge-{tag}-")
    try:
        for t in tests:
            try:
                t(exe, root)
            except JudgeFail as e:
                print(f"JUDGE FAIL: {t.__name__}: {e}")
                sys.exit(1)
        print(f"{tag} check: {len(tests)} groups passed")
    finally:
        shutil.rmtree(root, ignore_errors=True)
