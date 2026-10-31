"""T2.1 judge: generate cases, run the bim-geom driver, compare with independent oracles.

Oracles: exact rational arithmetic (fractions.Fraction) for predicate signs; shapely for polygon
area / containment / triangulation coverage; closed forms for extrusions and mitred sweeps;
trimesh on the exported ASCII STL (watertight, winding, volume, area, centre of mass).

Usage: python -I judges/T2.1/check.py <driver-binary>
"""
import atexit
import json
import math
import random
import shutil
import struct
import subprocess
import sys
import tempfile
from fractions import Fraction as Fr
from pathlib import Path

import numpy as np
import shapely
import trimesh
from shapely.geometry import Point, Polygon as SPoly

DRIVER = sys.argv[1]
rng = random.Random(0x7E0_2021)
failures = []


def fail(msg):
    failures.append(msg)


def bits(x):
    return struct.unpack("<Q", struct.pack("<d", float(x)))[0]


def unbits(b):
    return struct.unpack("<d", struct.pack("<Q", b))[0]


def sgn(fr):
    return (fr > 0) - (fr < 0)


def rel_close(got, want, rel=1e-9, abs_=0.0):
    return abs(got - want) <= max(rel * abs(want), abs_)


# ---------------------------------------------------------------- exact predicate cases
# Coordinates stay in {0} U [2^-100, 2^100] in magnitude so no intermediate over/underflow
# occurs; within that range the sign must be exact.

def orient2d_exact(a, b, c):
    ax, ay, bx, by, cx, cy = map(Fr, (*a, *b, *c))
    return sgn((ax - cx) * (by - cy) - (ay - cy) * (bx - cx))


def orient3d_exact(a, b, c, d):
    m = [[Fr(p[i]) - Fr(d[i]) for i in range(3)] for p in (a, b, c)]
    det = (m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
           - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
           + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]))
    return sgn(det)


def incircle_exact(a, b, c, d):
    rows = []
    for p in (a, b, c):
        x, y = Fr(p[0]) - Fr(d[0]), Fr(p[1]) - Fr(d[1])
        rows.append((x, y, x * x + y * y))
    (a0, a1, a2), (b0, b1, b2), (c0, c1, c2) = rows
    return sgn(a0 * (b1 * c2 - b2 * c1) - a1 * (b0 * c2 - b2 * c0) + a2 * (b0 * c1 - b1 * c0))


def ulps(x, k):
    for _ in range(abs(k)):
        x = math.nextafter(x, math.inf if k > 0 else -math.inf)
    return x


def scale_exp():
    return rng.randint(-90, 90)


o2 = []
u = math.ulp(0.5)
for i in range(64):  # Shewchuk's classic near-degenerate grid
    for j in range(64):
        o2.append(((0.5 + i * u, 0.5 + j * u), (12.0, 12.0), (24.0, 24.0)))
for _ in range(600):  # exactly collinear integer points, scaled by powers of two
    e = scale_exp()
    p, q = rng.randint(-2**20, 2**20), rng.randint(-2**20, 2**20)
    d1, d2 = rng.randint(-2**10, 2**10), rng.randint(-2**10, 2**10)
    k = rng.randint(-50, 50)
    pts = [(p, q), (p + d1, q + d2), (p + k * d1, q + k * d2)]
    rng.shuffle(pts)
    o2.append(tuple((math.ldexp(x, e), math.ldexp(y, e)) for x, y in pts))
for _ in range(1500):  # nearly collinear: third point rounded onto the line, then nudged
    e = scale_exp()
    a = (math.ldexp(rng.uniform(-1, 1), e), math.ldexp(rng.uniform(-1, 1), e))
    b = (math.ldexp(rng.uniform(-1, 1), e), math.ldexp(rng.uniform(-1, 1), e))
    t = rng.uniform(-3, 3)
    c = (ulps(a[0] + t * (b[0] - a[0]), rng.randint(-2, 2)), ulps(a[1] + t * (b[1] - a[1]), rng.randint(-2, 2)))
    o2.append((a, b, c))
for _ in range(500):  # generic, mixed scales
    o2.append(tuple((math.ldexp(rng.uniform(-1, 1), scale_exp()), math.ldexp(rng.uniform(-1, 1), scale_exp())) for _ in range(3)))
o2 += [((1.0, 1.0), (1.0, 1.0), (3.0, 7.0)), ((0.0, 0.0), (0.0, 0.0), (0.0, 0.0)), ((0.0, 0.0), (1.0, 0.0), (0.0, 1.0))]

o3 = []
for _ in range(600):  # exactly coplanar integer points
    e = scale_exp()
    a = [rng.randint(-2**15, 2**15) for _ in range(3)]
    b = [rng.randint(-2**15, 2**15) for _ in range(3)]
    c = [rng.randint(-2**15, 2**15) for _ in range(3)]
    s, t = rng.randint(-20, 20), rng.randint(-20, 20)
    d = [a[i] + s * (b[i] - a[i]) + t * (c[i] - a[i]) for i in range(3)]
    o3.append(tuple(tuple(math.ldexp(x, e) for x in p) for p in (a, b, c, d)))
for _ in range(2000):  # nearly coplanar
    e = scale_exp()
    a, b, c = ([math.ldexp(rng.uniform(-1, 1), e) for _ in range(3)] for _ in range(3))
    s, t = rng.uniform(-2, 2), rng.uniform(-2, 2)
    d = [ulps(a[i] + s * (b[i] - a[i]) + t * (c[i] - a[i]), rng.randint(-1, 1)) for i in range(3)]
    o3.append((tuple(a), tuple(b), tuple(c), tuple(d)))
for _ in range(500):
    o3.append(tuple(tuple(math.ldexp(rng.uniform(-1, 1), scale_exp()) for _ in range(3)) for _ in range(4)))
o3.append(((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, -1.0)))  # d below CCW abc: positive

# Integer points on circles of radius 65 (many Pythagorean triples) and 5.
circle65 = [(x, y) for x in range(-65, 66) for y in range(-65, 66) if x * x + y * y == 65 * 65]
ic = []
for _ in range(600):  # exactly cocircular
    e = rng.randint(-60, 60)
    ox, oy = rng.randint(-2**12, 2**12), rng.randint(-2**12, 2**12)
    pts = rng.sample(circle65, 4)
    ic.append(tuple((math.ldexp(x + ox, e), math.ldexp(y + oy, e)) for x, y in pts))
for _ in range(2000):  # nearly cocircular
    e = rng.randint(-60, 60)
    cx, cy, r = rng.uniform(-1, 1), rng.uniform(-1, 1), rng.uniform(0.1, 1)
    pts = []
    for _ in range(4):
        th = rng.uniform(0, 2 * math.pi)
        pts.append((ulps(math.ldexp(cx + r * math.cos(th), e), rng.randint(-1, 1)),
                    ulps(math.ldexp(cy + r * math.sin(th), e), rng.randint(-1, 1))))
    ic.append(tuple(pts))
for _ in range(500):
    ic.append(tuple((math.ldexp(rng.uniform(-1, 1), scale_exp() // 2), math.ldexp(rng.uniform(-1, 1), scale_exp() // 2)) for _ in range(4)))
ic.append(((5.0, 0.0), (0.0, 5.0), (-5.0, 0.0), (0.0, 0.0)))  # centre inside CCW circle: positive

# Exact signs are promised only for |coords| in {0} U [2^-100, 2^100]; drop the few scaled-integer
# cases that land outside (the scale exponent can push 2^21-sized integers past 2^100).
LO, HI = 2.0 ** -100, 2.0 ** 100


def in_range(case):
    return all(x == 0 or LO <= abs(x) <= HI for p in case for x in p)


o2 = [c for c in o2 if in_range(c)]
o3 = [c for c in o3 if in_range(c)]
ic = [c for c in ic if in_range(c)]
exp_o2 = [orient2d_exact(*c) for c in o2]
exp_o3 = [orient3d_exact(*c) for c in o3]
exp_ic = [incircle_exact(*c) for c in ic]
for name, exp in (("orient2d", exp_o2), ("orient3d", exp_o3), ("incircle", exp_ic)):
    zeros = sum(1 for s in exp if s == 0)
    assert zeros >= 500 and len(exp) - zeros >= 1000, f"oracle sanity {name}: {zeros} zeros of {len(exp)}"


# ---------------------------------------------------------------- polygons

def regular(cx, cy, r, n, phase=0.0, cw=False):
    pts = [(cx + r * math.cos(phase + 2 * math.pi * k / n), cy + r * math.sin(phase + 2 * math.pi * k / n)) for k in range(n)]
    return pts[::-1] if cw else pts


def star(n, holes):
    cx, cy, R = rng.uniform(-50, 50), rng.uniform(-50, 50), rng.uniform(1, 10)
    while True:
        angles = sorted(rng.uniform(0, 2 * math.pi) for _ in range(n))
        gaps = [(angles[(k + 1) % n] - angles[k]) % (2 * math.pi) for k in range(n)]
        if max(gaps) < math.pi / 3 and min(gaps) > 0.02:
            break
    radii = [R * rng.uniform(0.55, 1) for _ in angles]
    outer = [(cx + r * math.cos(a), cy + r * math.sin(a)) for r, a in zip(radii, angles)]
    hs = [regular(cx + 0.22 * R * math.cos(2 * math.pi * k / 3), cy + 0.22 * R * math.sin(2 * math.pi * k / 3),
                  R * rng.uniform(0.06, 0.12), rng.randint(3, 8), rng.uniform(0, 1), cw=True) for k in range(holes)]
    return outer, hs


def comb(teeth):
    """Base strip with `teeth` rectangular teeth: many reflex vertices, no collinear ones."""
    pts = [(0.0, 0.0), (2.0 * teeth, 0.0), (2.0 * teeth, 1.0)]
    for k in range(teeth - 1, -1, -1):
        pts += [(2 * k + 1.6, 1.0), (2 * k + 1.6, 3.0), (2 * k + 0.4, 3.0), (2 * k + 0.4, 1.0)]
    return pts + [(0.0, 1.0)]


polys = [
    ([(0, 0), (1, 0), (1, 1), (0, 1)], []),
    ([(0, 0), (4, 0), (4, 4), (0, 4)], [[(1, 1), (1, 2), (2, 2), (2, 1)]]),
    ([(0, 0), (3, 0), (3, 1), (1, 1), (1, 3), (0, 3)], []),
    ([(0, 0), (10, 0), (10, 6), (0, 6)], [[(1, 1), (1, 5), (4, 5), (4, 1)], [(6, 1), (6, 5), (9, 5), (9, 1)]]),
    ([(0, 0), (5, 1), (2, 4)], []),
    (comb(7), []),
]
for k in range(70):
    polys.append(star(rng.randint(7, 40), rng.choice([0, 0, 1, 2, 3])))
poly_cases = []
for outer, holes in polys:
    sp = SPoly(outer, holes)
    assert sp.is_valid and sp.exterior.is_ccw and all(not SPoly(h).exterior.is_ccw for h in holes), (outer, holes)
    minx, miny, maxx, maxy = sp.bounds
    probes = []
    while len(probes) < 60:
        p = (rng.uniform(minx - 1, maxx + 1), rng.uniform(miny - 1, maxy + 1))
        if sp.boundary.distance(Point(p)) > 1e-6:
            probes.append(p)
    poly_cases.append({"outer": outer, "holes": holes, "shapely": sp, "probes": probes})


def enc_poly(c):
    return {"outer": [[bits(x), bits(y)] for x, y in c["outer"]],
            "holes": [[[bits(x), bits(y)] for x, y in h] for h in c["holes"]]}


# ---------------------------------------------------------------- solids

extrusions = []
for k, c in enumerate(poly_cases[:40]):
    extrusions.append({"poly": c, "z0": rng.uniform(-5, 5), "h": rng.uniform(0.1, 6), "stl": f"ex{k}.stl"})


def rect(w, h, cw=False):
    pts = [(-w / 2, -h / 2), (w / 2, -h / 2), (w / 2, h / 2), (-w / 2, h / 2)]
    return pts[::-1] if cw else pts


l_raw = [(0, 0), (0.4, 0), (0.4, 0.1), (0.1, 0.1), (0.1, 0.3), (0, 0.3)]
lc = SPoly(l_raw).centroid
l_profile = [(x - lc.x, y - lc.y) for x, y in l_raw]
profiles = {
    "rect": (rect(0.5, 0.3), [], True),
    "hollow": (rect(0.6, 0.6), [rect(0.2, 0.2, cw=True)], True),
    "hexagon": (regular(0, 0, 0.25, 6), [], True),
    "L": (l_profile, [], False),  # centroid at the origin, not centrally symmetric
}
paths = {
    "vertical": [(1.0, 2.0, -1.0), (1.0, 2.0, 3.5)],
    "oblique": [(0.0, 0.0, 0.0), (3.0, 4.0, 12.0)],
    "plan-L": [(0.0, 0.0, 0.0), (4.0, 0.0, 0.0), (4.0, 3.0, 0.0)],
    "zigzag": [(0.0, 0.0, 1.0), (3.0, 0.0, 1.0), (4.5, 2.598076211353316, 1.0), (7.5, 2.598076211353316, 1.0), (9.0, 0.0, 1.0)],
    "space": [(0.0, 0.0, 0.0), (3.0, 0.0, 0.0), (3.0, 2.5, 1.0), (3.0, 3.0, 4.0), (5.5, 4.0, 5.0)],
}


def path_len(path):
    return sum(math.dist(path[i], path[i + 1]) for i in range(len(path) - 1))


for name, path in paths.items():  # generation sanity: turns <= 90 degrees, long segments
    for i in range(1, len(path) - 1):
        d1 = np.subtract(path[i], path[i - 1])
        d2 = np.subtract(path[i + 1], path[i])
        assert np.dot(d1, d2) >= -1e-12 and min(np.linalg.norm(d1), np.linalg.norm(d2)) >= 2.5, name

sweeps = []
for pname, (outer, holes, symmetric) in profiles.items():
    for qname, path in paths.items():
        sweeps.append({"pname": pname, "qname": qname, "outer": outer, "holes": holes, "symmetric": symmetric,
                       "shapely": SPoly(outer, holes), "path": path, "stl": f"sw_{pname}_{qname}.stl"})

tmp = Path(tempfile.mkdtemp(prefix="t21-"))
atexit.register(shutil.rmtree, tmp, True)
cases = {
    "orient2d": [[[bits(x) for x in p] for p in c] for c in o2],
    "orient3d": [[[bits(x) for x in p] for p in c] for c in o3],
    "incircle": [[[bits(x) for x in p] for p in c] for c in ic],
    "polygons": [{"polygon": enc_poly(c), "probes": [[bits(x), bits(y)] for x, y in c["probes"]]} for c in poly_cases],
    "extrusions": [{"polygon": enc_poly(e["poly"]), "z0": bits(e["z0"]), "height": bits(e["h"]), "stl": e["stl"]} for e in extrusions],
    "sweeps": [{"polygon": enc_poly(s), "path": [[bits(x) for x in p] for p in s["path"]], "stl": s["stl"]} for s in sweeps],
}
(tmp / "cases.json").write_text(json.dumps(cases))
try:
    p = subprocess.run([DRIVER, tmp / "cases.json", tmp / "results.json", tmp], capture_output=True, text=True, timeout=120)
except subprocess.TimeoutExpired:
    print("JUDGE FAIL: bim-geom driver timed out after 120 s", file=sys.stderr)
    sys.exit(1)
if p.returncode != 0:
    print(f"JUDGE FAIL: bim-geom driver crashed (exit {p.returncode}):\n{p.stderr[-3000:]}", file=sys.stderr)
    sys.exit(1)
res = json.loads((tmp / "results.json").read_text())

# ---------------------------------------------------------------- checks: predicates
for name, cs, exp in (("orient2d", o2, exp_o2), ("orient3d", o3, exp_o3), ("incircle", ic, exp_ic)):
    got = res[name]
    bad = [i for i, (g, e) in enumerate(zip(got, exp)) if g != e]
    if len(got) != len(exp) or bad:
        i = bad[0] if bad else 0
        fail(f"{name}: {len(bad)} of {len(exp)} signs wrong, e.g. {cs[i]} -> {got[i] if i < len(got) else None}, exact {exp[i]}")

# ---------------------------------------------------------------- checks: polygons
def general_position(verts):
    v = np.asarray(verts, dtype=float)
    a, b, c = np.meshgrid(np.arange(len(v)), np.arange(len(v)), np.arange(len(v)), indexing="ij")
    m = (a < b) & (b < c)
    a, b, c = a[m], b[m], c[m]
    cr = (v[b, 0] - v[a, 0]) * (v[c, 1] - v[a, 1]) - (v[b, 1] - v[a, 1]) * (v[c, 0] - v[a, 0])
    scale = np.ptp(v, axis=0).max() ** 2
    return bool(np.all(np.abs(cr) > 1e-9 * scale))


for k, (c, r) in enumerate(zip(poly_cases, res["polygons"])):
    sp = c["shapely"]
    area = unbits(r["area"])
    if not rel_close(area, sp.area):
        fail(f"polygon {k}: area {area}, shapely {sp.area}")
    want_in = [sp.contains(Point(q)) for q in c["probes"]]
    wrong = [q for q, g, w in zip(c["probes"], r["contains"], want_in) if g != w]
    if wrong:
        fail(f"polygon {k}: contains() wrong for {len(wrong)} probes, e.g. {wrong[0]}")
    verts = list(c["outer"]) + [v for h in c["holes"] for v in h]
    tris = r["triangles"]
    want_n = len(verts) + 2 * len(c["holes"]) - 2
    # Euler's count holds for every edge-to-edge triangulation; with three collinear vertices a
    # valid ear-clipping tiling may have a T-junction instead, so only general-position inputs are counted.
    if general_position(verts) and len(tris) != want_n:
        fail(f"polygon {k}: {len(tris)} triangles, a triangulation of {len(verts)} vertices with {len(c['holes'])} holes has {want_n}")
    if any(not (0 <= i < len(verts)) for t in tris for i in t):
        fail(f"polygon {k}: triangle index out of range (indices are outer ++ holes)")
        continue
    tri_polys = [SPoly([verts[i] for i in t]) for t in tris]
    tri_sum = sum(t.area for t in tri_polys)
    union = shapely.union_all(tri_polys)
    if not rel_close(tri_sum, sp.area) or not rel_close(union.area, sp.area) or union.symmetric_difference(sp).area > 1e-9 * sp.area:
        fail(f"polygon {k}: triangles do not tile the polygon (sum {tri_sum}, union {union.area}, polygon {sp.area})")

# ---------------------------------------------------------------- checks: solids


def check_stl(label, path, r):
    raw = path.read_bytes() if path.exists() else b""
    if not raw.lstrip().startswith(b"solid") or b"facet normal" not in raw:
        fail(f"{label}: {path.name} is not an ASCII STL")
        return
    m = trimesh.load(path, file_type="stl", force="mesh")
    if not m.is_watertight or not m.is_winding_consistent:
        fail(f"{label}: STL not watertight/consistently wound (watertight={m.is_watertight})")
        return
    vol, area, cm = unbits(r["volume"]), unbits(r["area"]), [unbits(x) for x in r["centroid"]]
    if not rel_close(m.volume, vol):
        fail(f"{label}: STL volume {m.volume} (trimesh) vs volume() {vol}; negative means inward normals")
    if not rel_close(m.area, area):
        fail(f"{label}: STL area {m.area} (trimesh) vs area() {area}")
    scale = max(1.0, float(np.abs(m.bounds).max()))
    if np.abs(np.asarray(m.center_mass) - cm).max() > 1e-9 * scale:
        fail(f"{label}: STL centre of mass {list(m.center_mass)} (trimesh) vs centroid() {cm}")


for k, (e, r) in enumerate(zip(extrusions, res["extrusions"])):
    sp, z0, h = e["poly"]["shapely"], e["z0"], e["h"]
    label = f"extrude #{k}"
    perim = sp.exterior.length + sum(i.length for i in sp.interiors)
    if not rel_close(unbits(r["volume"]), sp.area * h):
        fail(f"{label}: volume {unbits(r['volume'])}, want A*h = {sp.area * h}")
    if not rel_close(unbits(r["area"]), 2 * sp.area + perim * h):
        fail(f"{label}: area {unbits(r['area'])}, want 2A + P*h = {2 * sp.area + perim * h}")
    want_c = (sp.centroid.x, sp.centroid.y, z0 + h / 2)
    got_c = [unbits(x) for x in r["centroid"]]
    if max(abs(g - w) for g, w in zip(got_c, want_c)) > 1e-9 * max(1.0, max(map(abs, want_c))):
        fail(f"{label}: centroid {got_c}, want {want_c}")
    if not r["closed"]:
        fail(f"{label}: is_closed_manifold() is false")
    if not e["poly"]["holes"] and r["euler"] != 2:
        fail(f"{label}: euler_characteristic {r['euler']}, want 2 for a genus-0 solid")
    check_stl(label, tmp / e["stl"], r)

by_key = {}
for s, r in zip(sweeps, res["sweeps"]):
    label = f"sweep {s['pname']} along {s['qname']}"
    sp, L = s["shapely"], path_len(s["path"])
    by_key[(s["pname"], s["qname"])] = r
    if not rel_close(unbits(r["volume"]), sp.area * L):
        fail(f"{label}: volume {unbits(r['volume'])}, want A*L = {sp.area * L} (profile centroid on the path)")
    if s["symmetric"]:
        perim = sp.exterior.length + sum(i.length for i in sp.interiors)
        if not rel_close(unbits(r["area"]), 2 * sp.area + perim * L):
            fail(f"{label}: area {unbits(r['area'])}, want 2A + P*L = {2 * sp.area + perim * L}")
    if not r["closed"]:
        fail(f"{label}: is_closed_manifold() is false")
    if not s["holes"] and r["euler"] != 2:
        fail(f"{label}: euler_characteristic {r['euler']}, want 2")
    check_stl(label, tmp / s["stl"], r)
    if s["qname"] in ("vertical", "oblique"):
        a, b = np.array(s["path"][0]), np.array(s["path"][1])
        want_c = (a + b) / 2
        got_c = np.array([unbits(x) for x in r["centroid"]])
        if np.abs(got_c - want_c).max() > 1e-9 * max(1.0, np.abs(want_c).max()):
            fail(f"{label}: centroid {list(got_c)}, want path midpoint {list(want_c)}")

if failures:
    for f in failures[:25]:
        print(f"JUDGE FAIL: {f}", file=sys.stderr)
    if len(failures) > 25:
        print(f"JUDGE FAIL: ... and {len(failures) - 25} more", file=sys.stderr)
    sys.exit(1)
print(f"T2.1 checks passed: {len(o2)}+{len(o3)}+{len(ic)} predicate signs, {len(poly_cases)} polygons, "
      f"{len(extrusions)} extrusions, {len(sweeps)} sweeps")
