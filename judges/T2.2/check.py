"""T2.2 judge: boolean CSG on closed polyhedral B-Reps.

For random and hand-picked pairs (boxes on coarse grids so coplanar faces, shared edges and
touching vertices are common, star prisms, mitred sweeps) the driver runs union, A-B, B-A and
intersection. Oracles: manifold3d (double precision, Mesh64) for volumes and for the number of
maximal connected coplanar face regions; trimesh for watertightness and winding of the exported STL.

Usage: python -I judges/T2.2/check.py <driver-binary>
       python -I judges/T2.2/check.py --self-test   (feeds the oracle's own answers through the checks)
"""
import atexit
import warnings
import json
import math
import random
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

import manifold3d as m3
import numpy as np
import shapely
import trimesh
from shapely.geometry import Polygon as SPoly

warnings.filterwarnings("ignore", category=RuntimeWarning)
SELF_TEST = sys.argv[1] == "--self-test"
rng = random.Random(0xC5_6_2022)
failures = []


def fail(msg):
    failures.append(msg)


def bits(x):
    return struct.unpack("<Q", struct.pack("<d", float(x)))[0]


def unbits(b):
    return struct.unpack("<d", struct.pack("<Q", b))[0]


# ---------------------------------------------------------------- oracle construction

def prism_mesh(outer, holes, z0, h):
    """Closed triangle mesh of a polygon-with-holes extrusion, built independently of bim-geom."""
    sp = SPoly(outer, holes)
    rings = [list(outer)] + [list(r) for r in holes]
    verts2 = [p for r in rings for p in r]
    index = {(float(x), float(y)): i for i, (x, y) in enumerate(verts2)}
    n = len(verts2)
    z1 = z0 + h
    V = [(x, y, z0) for x, y in verts2] + [(x, y, z1) for x, y in verts2]
    F = []
    for tri in shapely.constrained_delaunay_triangles(sp).geoms:
        ids = [index[(float(x), float(y))] for x, y in list(tri.exterior.coords)[:3]]
        if SPoly([verts2[i] for i in ids]).exterior.is_ccw:
            ids = ids[::-1]
        F.append(ids)  # bottom faces down (clockwise seen from above)
        F.append([i + n for i in ids[::-1]])
    off = 0
    for r in rings:
        k = len(r)
        for i in range(k):
            a, b = off + i, off + (i + 1) % k
            F.append([a, b, b + n])
            F.append([a, b + n, a + n])
        off += k
    return np.array(V, float), np.array(F, np.int64)


def to_manifold(V, F):
    M = m3.Manifold(m3.Mesh64(vert_properties=np.asarray(V, float), tri_verts=np.asarray(F, np.uint32)))
    assert M.status() == m3.Error.NoError, M.status()
    return M


def oracle_solid(desc, stl_path):
    if desc["kind"] == "sweep":
        # The sweep's own geometry is judged by T2.1 (closed-form volume); reuse the exported mesh.
        t = trimesh.load(stl_path, file_type="stl", force="mesh")
        if not t.is_watertight:
            return None
        return to_manifold(t.vertices, t.faces)
    return to_manifold(*prism_mesh(desc["outer"], desc["holes"], desc["z0"], desc["h"]))


def mesh_of(M):
    me = M.to_mesh64()
    return trimesh.Trimesh(np.asarray(me.vert_properties)[:, :3], np.asarray(me.tri_verts), process=False)


def regions(t):
    """Maximal edge-connected coplanar regions = faces of a B-Rep with merged coplanar faces."""
    f = t.facets
    return len(f) + len(t.faces) - sum(len(x) for x in f)


# ---------------------------------------------------------------- cases

def box(mn, mx):
    outer = [(mn[0], mn[1]), (mx[0], mn[1]), (mx[0], mx[1]), (mn[0], mx[1])]
    return {"kind": "extrude", "outer": outer, "holes": [], "z0": mn[2], "h": mx[2] - mn[2]}


def grid_box(step, span):
    mn = [rng.randint(0, int(span / step) - 1) * step for _ in range(3)]
    mx = [lo + rng.randint(1, int(span / step)) * step for lo in mn]
    return box(mn, mx)


def star(cx, cy, R, n):
    while True:
        angles = sorted(rng.uniform(0, 2 * math.pi) for _ in range(n))
        gaps = [(angles[(k + 1) % n] - angles[k]) % (2 * math.pi) for k in range(n)]
        if max(gaps) < math.pi / 3 and min(gaps) > 0.05:
            break
    pts = []
    for a in angles:
        r = R * rng.uniform(0.55, 1)
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    # Reject nearly collinear consecutive edges (they would be "almost coplanar" side faces).
    for i in range(n):
        p, q, s = np.array(pts[i - 1]), np.array(pts[i]), np.array(pts[(i + 1) % n])
        u, v = q - p, s - q
        if abs(u[0] * v[1] - u[1] * v[0]) < 0.05 * np.linalg.norm(u) * np.linalg.norm(v):
            return star(cx, cy, R, n)
    return pts


def prism(z_step=0.5):
    z0 = rng.randint(0, 3) * z_step
    return {"kind": "extrude", "outer": star(rng.uniform(0.6, 1.4), rng.uniform(0.6, 1.4), rng.uniform(0.6, 1.2), rng.randint(7, 14)),
            "holes": [], "z0": z0, "h": rng.randint(1, 4) * z_step}


def rect(w, h, cw=False):
    pts = [(-w / 2, -h / 2), (w / 2, -h / 2), (w / 2, h / 2), (-w / 2, h / 2)]
    return pts[::-1] if cw else pts


def sweep(path):
    return {"kind": "sweep", "outer": rect(0.5, 0.3), "holes": [], "path": path}


fixed = [
    ("identical", box([0, 0, 0], [1, 1, 1]), box([0, 0, 0], [1, 1, 1])),
    ("stacked: shared face", box([0, 0, 0], [1, 1, 1]), box([0, 0, 1], [1, 1, 2])),
    ("side by side: partial shared face", box([0, 0, 0], [1, 1, 1]), box([1, 0.25, 0], [2, 0.75, 1])),
    ("overlap along x: coplanar sides", box([0, 0, 0], [1, 1, 1]), box([0.5, 0, 0], [1.5, 1, 1])),
    ("shared edge only", box([0, 0, 0], [1, 1, 1]), box([1, 1, 0], [2, 2, 1])),
    ("touching vertex only", box([0, 0, 0], [1, 1, 1]), box([1, 1, 1], [2, 2, 2])),
    ("contained", box([0, 0, 0], [3, 3, 3]), box([1, 1, 1], [2, 2, 2])),
    ("tunnel", box([0, 0, 0], [1, 1, 1]), box([0.25, 0.25, -1], [0.75, 0.75, 2])),
    ("corner notch", box([0, 0, 0], [1, 1, 1]), box([0.5, 0.5, 0.5], [2, 2, 2])),
    ("flush top, offset", box([0, 0, 0], [1, 1, 1]), box([0.5, 0.5, 0.5], [1.5, 1.5, 1])),
    ("block on block", box([0, 0, 0], [3, 3, 1]), box([1, 1, 1], [2, 2, 2])),
    ("disjoint", box([0, 0, 0], [1, 1, 1]), box([3, 3, 3], [4, 4, 4])),
    ("holed prism vs bar", {"kind": "extrude", "outer": [(0, 0), (4, 0), (4, 4), (0, 4)], "holes": [[(1, 1), (1, 3), (3, 3), (3, 1)]], "z0": 0.0, "h": 1.0},
     box([-1, 1.5, 0.25], [5, 2.5, 0.75])),
]
cases = [(name, a, b) for name, a, b in fixed]
for k in range(60):
    cases.append((f"grid boxes 0.5 #{k}", grid_box(0.5, 2.0), grid_box(0.5, 2.0)))
for k in range(30):
    cases.append((f"grid boxes 0.25 #{k}", grid_box(0.25, 2.0), grid_box(0.25, 2.0)))
for k in range(25):
    cases.append((f"prism vs box #{k}", prism(), grid_box(0.5, 2.0)))
for k in range(15):
    cases.append((f"prism vs prism #{k}", prism(), prism()))
for k in range(12):
    path = [(0.0, 0.5, 0.5), (rng.choice([2.0, 2.5, 3.0]), 0.5, 0.5), (2.0 + rng.choice([0.0, 0.5, 1.0]), 3.0, rng.choice([0.5, 1.0]))]
    cases.append((f"sweep vs box #{k}", sweep(path), grid_box(0.5, 3.0)))
if SELF_TEST:
    cases = [c for c in cases if c[1]["kind"] != "sweep"]


def enc(desc):
    poly = {"outer": [[bits(x), bits(y)] for x, y in desc["outer"]],
            "holes": [[[bits(x), bits(y)] for x, y in h] for h in desc["holes"]]}
    if desc["kind"] == "sweep":
        return {"type": "sweep", "polygon": poly, "path": [[bits(x) for x in p] for p in desc["path"]]}
    return {"type": "extrude", "polygon": poly, "z0": bits(desc["z0"]), "height": bits(desc["h"])}


tmp = Path(tempfile.mkdtemp(prefix="t22-"))
atexit.register(shutil.rmtree, tmp, True)
OPS = ("union", "difference", "difference_ba", "intersection")


def run_oracle_as_driver():
    """Self-test: produce driver-shaped results from the oracle itself."""
    out = []
    for k, (_, a, b) in enumerate(cases):
        Ma, Mb = oracle_solid(a, None), oracle_solid(b, None)
        ops = []
        for name, R in zip(OPS, (Ma + Mb, Ma - Mb, Mb - Ma, Ma ^ Mb)):
            stl = f"c{k}_{name}.stl"
            if R.is_empty() or abs(R.volume()) <= 1e-12 * max(1.0, Ma.volume(), Mb.volume()):
                (tmp / stl).write_text("solid empty\nendsolid empty\n")
                ops.append({"op": name, "ok": True, "volume": bits(0.0), "faces": 0, "closed": True, "stl": stl})
            else:
                t = mesh_of(R)
                (tmp / stl).write_bytes(trimesh.exchange.stl.export_stl_ascii(t).encode())
                ops.append({"op": name, "ok": True, "volume": bits(R.volume()), "faces": regions(t), "closed": True, "stl": stl})
        out.append({"a_volume": bits(Ma.volume()), "b_volume": bits(Mb.volume()), "ops": ops})
    return out


if SELF_TEST:
    results = run_oracle_as_driver()
else:
    (tmp / "cases.json").write_text(json.dumps([{"a": enc(a), "b": enc(b)} for _, a, b in cases]))
    try:
        p = subprocess.run([sys.argv[1], tmp / "cases.json", tmp / "results.json", tmp], capture_output=True, text=True, timeout=150)
    except subprocess.TimeoutExpired as e:
        err = e.stderr or ""
        last = (err.decode(errors="replace") if isinstance(err, bytes) else err).strip().splitlines()[-1:]
        print(f"JUDGE FAIL: boolean driver timed out after 150 s (last progress: {last})", file=sys.stderr)
        sys.exit(1)
    if p.returncode != 0:
        print(f"JUDGE FAIL: boolean driver crashed (exit {p.returncode}):\n{p.stderr[-3000:]}", file=sys.stderr)
        sys.exit(1)
    results = json.loads((tmp / "results.json").read_text())

checked = 0
for k, ((name, a, b), r) in enumerate(zip(cases, results)):
    Ma = oracle_solid(a, tmp / f"c{k}_a.stl")
    Mb = oracle_solid(b, tmp / f"c{k}_b.stl")
    if Ma is None or Mb is None:
        fail(f"{name}: exported input sweep is not watertight, cannot be used as boolean input")
        continue
    for M, key in ((Ma, "a_volume"), (Mb, "b_volume")):
        if abs(unbits(r[key]) - M.volume()) > 1e-9 * M.volume():
            fail(f"{name}: input volume {unbits(r[key])} vs oracle {M.volume()} (inputs must be exact)")
    for op, R in zip(OPS, (Ma + Mb, Ma - Mb, Mb - Ma, Ma ^ Mb)):
        got = next(o for o in r["ops"] if o["op"] == op)
        label = f"{name}: {op}"
        if not got["ok"]:
            fail(f"{label}: returned Err({got['error']})")
            continue
        # manifold3d may return zero-volume slivers for touching inputs; those results are empty.
        empty = R.is_empty() or abs(R.volume()) <= 1e-12 * max(1.0, Ma.volume(), Mb.volume())
        want_v = 0.0 if empty else R.volume()
        vol = unbits(got["volume"])
        if abs(vol - want_v) > max(1e-9 * want_v, 1e-12):
            fail(f"{label}: volume {vol}, manifold3d {want_v}")
        if not empty and not got["closed"]:
            fail(f"{label}: is_closed_manifold() is false")
        if empty:
            if got["faces"] != 0:
                fail(f"{label}: empty result should have no faces, has {got['faces']}")
            path = tmp / got["stl"]
            if not path.exists() or b"facet" in path.read_bytes():
                fail(f"{label}: to_stl of an empty result must write a file with no facets")
            checked += 1
            continue
        oracle_mesh = mesh_of(R)
        want_f = regions(oracle_mesh)
        if got["faces"] != want_f:
            fail(f"{label}: face_count {got['faces']}, want {want_f} maximal coplanar regions (coplanar faces must be merged)")
        path = tmp / got["stl"]
        t = trimesh.load(path, file_type="stl", force="mesh") if path.exists() else None
        if t is None or len(t.faces) == 0:
            fail(f"{label}: missing or empty STL for a non-empty result")
            continue
        merged_oracle = trimesh.Trimesh(oracle_mesh.vertices, oracle_mesh.faces, process=True)
        if merged_oracle.is_watertight and not (t.is_watertight and t.is_winding_consistent):
            fail(f"{label}: result STL is not watertight / consistently wound")
        if abs(t.volume - want_v) > 1e-9 * want_v:
            fail(f"{label}: STL volume {t.volume} (trimesh) vs manifold3d {want_v} (negative: inward normals)")
        checked += 1

if failures:
    for f in failures[:25]:
        print(f"JUDGE FAIL: {f}", file=sys.stderr)
    if len(failures) > 25:
        print(f"JUDGE FAIL: ... and {len(failures) - 25} more", file=sys.stderr)
    sys.exit(1)
print(f"T2.2 checks passed: {checked} boolean results over {len(cases)} input pairs")
