"""Oracle for the T2.5 judge (bim_geom::section). Builders must not edit anything under judges/.

  oracle.py gen <cases.txt> <expected.json>     write the case file and the expected sections
  oracle.py check <expected.json> <out.jsonl>   compare the driver's output with the expectation
  oracle.py selftest <expected.json>            feed the checker oracle-made answers (must pass) and
                                                corrupted ones (must fail)

Expected values: closed forms for the degenerate planes (through faces, edges, vertices, coplanar
faces), and for random solids/booleans an independent rebuild in manifold3d (rotated so the plane is
z = offset, then Manifold.slice) cross-checked against trimesh.section; shapes compared with shapely.
"""
import json
import math
import sys

import numpy as np
import shapely
from shapely.geometry import MultiPoint, Polygon
from shapely.ops import unary_union

TOL = 1e-6


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def basis(n):
    n = np.asarray(n, float)
    a = np.array([1.0, 0, 0]) if abs(n[0]) < 0.9 else np.array([0, 1.0, 0])
    u = np.cross(a, n)
    u /= np.linalg.norm(u)
    v = np.cross(n, u)
    return u, v


def fmt(x):
    return repr(float(x))


# ---------------------------------------------------------------- case construction
class Cases:
    def __init__(self):
        self.lines = []
        self.prims = {}  # id -> ("prism", z0, h, outer, holes) | ("bool", op, a, b)
        self.expected = []

    def prism(self, sid, z0, h, outer, holes=()):
        outer = [(round(x, 6), round(y, 6)) for x, y in outer]
        holes = [[(round(x, 6), round(y, 6)) for x, y in hl] for hl in holes]
        assert Polygon(outer).exterior.is_ccw
        for hl in holes:
            assert not Polygon(hl).exterior.is_ccw
        toks = ["prism", sid, fmt(z0), fmt(h), str(len(outer))] + [fmt(c) for p in outer for c in p]
        for hl in holes:
            toks += ["hole", str(len(hl))] + [fmt(c) for p in hl for c in p]
        self.lines.append(" ".join(toks))
        self.prims[sid] = ("prism", z0, h, outer, holes)

    def boolean(self, sid, op, a, b):
        self.lines.append(f"bool {sid} {op} {a} {b}")
        self.prims[sid] = ("bool", op, a, b)

    def manifold(self, sid):
        import manifold3d as m3

        p = self.prims[sid]
        if p[0] == "prism":
            _, z0, h, outer, holes = p
            cs = m3.CrossSection([outer] + list(holes))
            return cs.extrude(h).translate([0, 0, z0])
        _, op, a, b = p
        A, B = self.manifold(a), self.manifold(b)
        return {"union": A + B, "difference": A - B, "intersection": A ^ B}[op]

    def section(self, name, sid, n, offset, contours=None, convex=None, scale=1.0):
        """Expected region = XOR of `contours` (2D, plane basis) or of convex hulls of 3D point sets.
        `scale` multiplies the normal and offset written to the case file (same plane, non-unit
        normal: TASKS.md T2.5 Details say the normal need not be unit); expectations use the unit normal."""
        n = np.asarray(n, float)
        n = n / np.linalg.norm(n)
        u, v = basis(n)
        if convex is not None:
            contours = []
            for pts in convex:
                P = np.asarray(pts, float)
                hull = MultiPoint([(p @ u, p @ v) for p in P]).convex_hull
                contours.append(list(hull.exterior.coords)[:-1])
        if contours is None:  # oracle: manifold3d slice in the plane frame, checked against trimesh
            M = self.manifold(sid)
            R = np.zeros((3, 4))
            R[0, :3], R[1, :3], R[2, :3] = u, v, n
            sl = M.transform(R).slice(offset)
            contours = [np.asarray(c).tolist() for c in sl.to_polygons()]
            import trimesh

            mesh = M.to_mesh()
            tm = trimesh.Trimesh(np.asarray(mesh.vert_properties)[:, :3], np.asarray(mesh.tri_verts))
            sec = tm.section(plane_normal=n, plane_origin=n * offset)
            ta = 0.0 if sec is None else sec.to_2D()[0].area
            assert abs(ta - sl.area()) <= TOL * max(1, ta), (name, ta, sl.area())
            assert sl.area() > 1e-3, (name, "random plane misses the solid")
        geom = Polygon()
        for c in contours:
            geom = geom.symmetric_difference(Polygon(c))
        self.lines.append(f"section {name} {sid} {fmt(scale * n[0])} {fmt(scale * n[1])} {fmt(scale * n[2])} "
                          f"{fmt(scale * offset)}")
        self.expected.append({
            "case": name, "n": n.tolist(), "offset": float(offset), "u": u.tolist(), "v": v.tolist(),
            "area": geom.area, "wkt": geom.wkt, "regions": len(getattr(geom, "geoms", [geom])) if geom.area > 0 else 0,
        })


def box(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def cube_pts(x0, y0, z0, x1, y1, z1):
    return [(x, y, z) for x in (x0, x1) for y in (y0, y1) for z in (z0, z1)]


def star(rng, cx, cy, r):
    k = int(rng.integers(5, 10))
    ang = np.sort(rng.uniform(0, 2 * math.pi, k))
    rad = rng.uniform(0.4 * r, r, k)
    return [(cx + rr * math.cos(a), cy + rr * math.sin(a)) for a, rr in zip(ang, rad)]


def build():
    C = Cases()
    s3 = math.sqrt(3)
    # Unit cube: planes through its interior, faces, edges and vertices (closed forms).
    C.prism("cube", 0, 1, box(0, 0, 1, 1))
    C.section("cube_mid_z", "cube", (0, 0, 1), 0.5, convex=[[(x, y, .5) for x in (0, 1) for y in (0, 1)]])
    C.section("cube_top_face", "cube", (0, 0, 1), 1.0, convex=[[(x, y, 1) for x in (0, 1) for y in (0, 1)]])
    C.section("cube_bottom_face_flipped", "cube", (0, 0, -1), 0.0, convex=[[(x, y, 0) for x in (0, 1) for y in (0, 1)]])
    C.section("cube_side_face", "cube", (1, 0, 0), 1.0, convex=[[(1, y, z) for z in (0, 1) for y in (0, 1)]])
    C.section("cube_edge_touch", "cube", (1, 1, 0), 0.0, contours=[])
    C.section("cube_vertex_touch", "cube", (1, 1, 1), 0.0, contours=[])
    C.section("cube_outside", "cube", (0, 0, 1), 1.5, contours=[])
    C.section("cube_three_vertices", "cube", (1, 1, 1), 1 / s3, convex=[[(1, 0, 0), (0, 1, 0), (0, 0, 1)]])
    C.section("cube_hexagon", "cube", (1, 1, 1), 1.5 / s3,
              convex=[[(1, .5, 0), (1, 0, .5), (.5, 0, 1), (0, .5, 1), (0, 1, .5), (.5, 1, 0)]])
    # The same hexagon with a non-unit normal written to the case file (scaled normal and offset).
    C.section("cube_hexagon_scaled_normal", "cube", (1, 1, 1), 1.5 / s3, scale=2.5,
              convex=[[(1, .5, 0), (1, 0, .5), (.5, 0, 1), (0, .5, 1), (0, 1, .5), (.5, 1, 0)]])
    C.section("cube_two_vertical_edges", "cube", (1, -1, 0), 0.0, convex=[[(0, 0, 0), (0, 0, 1), (1, 1, 0), (1, 1, 1)]])
    C.section("cube_tilted_through_edges", "cube", (1, 0, -1), 0.0, convex=[[(0, 0, 0), (0, 1, 0), (1, 0, 1), (1, 1, 1)]])
    # Tube (prism with a hole): hole loops, two regions, a coplanar inner face.
    C.prism("tube", 0, 2, box(0, 0, 4, 4), [box(1, 1, 3, 3)[::-1]])
    C.section("tube_mid", "tube", (0, 0, 1), 1.0,
              convex=[cube_pts(0, 0, 1, 4, 4, 1), cube_pts(1, 1, 1, 3, 3, 1)])
    C.section("tube_mid_small_normal", "tube", (0, 0, 1), 1.0, scale=0.25,
              convex=[cube_pts(0, 0, 1, 4, 4, 1), cube_pts(1, 1, 1, 3, 3, 1)])
    C.section("tube_vertical_two_regions", "tube", (1, 0, 0), 2.0,
              convex=[cube_pts(2, 0, 0, 2, 1, 2), cube_pts(2, 3, 0, 2, 4, 2)])
    C.section("tube_inner_coplanar_face", "tube", (1, 0, 0), 1.0, convex=[cube_pts(1, 0, 0, 1, 4, 2)])
    # Island inside the hole (disjoint union): nesting outer / hole / separate island.
    C.prism("islet", 0, 2, box(1.5, 1.5, 2.5, 2.5))
    C.boolean("island", "union", "tube", "islet")
    C.section("island_mid", "island", (0, 0, 1), 0.5,
              convex=[cube_pts(0, 0, .5, 4, 4, .5), cube_pts(1, 1, .5, 3, 3, .5), cube_pts(1.5, 1.5, .5, 2.5, 2.5, .5)])
    # Non-convex L prism: plane through the reflex vertical edge and its coplanar face.
    C.prism("ell", 0, 1, [(0, 0), (2, 0), (2, 1), (1, 1), (1, 2), (0, 2)])
    C.section("ell_reflex_face", "ell", (1, 0, 0), 1.0, convex=[cube_pts(1, 0, 0, 1, 2, 1)])
    C.section("ell_mid", "ell", (0, 0, 1), 0.5, convex=[cube_pts(0, 0, .5, 2, 1, .5), cube_pts(0, 1, .5, 1, 2, .5)])
    # Stepped union: plane through the union's interior interface plus a coplanar top face.
    C.prism("stepA", 0, 1, box(0, 0, 2, 1))
    C.prism("stepB", 1, 1, box(0, 0, 1, 1))
    C.boolean("step", "union", "stepA", "stepB")
    C.section("step_interface_and_face", "step", (0, 0, 1), 1.0, convex=[cube_pts(0, 0, 1, 2, 1, 1)])
    C.section("step_vertical", "step", (0, 1, 0), 0.5,
              convex=[cube_pts(0, .5, 0, 2, .5, 1), cube_pts(0, .5, 1, 1, .5, 2)])
    # Pocket (difference): plane coplanar with the pocket floor, and through the pocket.
    C.prism("slab", 0, 1, box(0, 0, 3, 3))
    C.prism("cutter", 0.5, 1.5, box(1, 1, 2, 2))
    C.boolean("pocket", "difference", "slab", "cutter")
    C.section("pocket_floor_coplanar", "pocket", (0, 0, 1), 0.5, convex=[cube_pts(0, 0, .5, 3, 3, .5)])
    C.section("pocket_mid", "pocket", (0, 0, 1), 0.75,
              convex=[cube_pts(0, 0, .75, 3, 3, .75), cube_pts(1, 1, .75, 2, 2, .75)])

    # Random star prisms and their booleans, sectioned by random planes (oracle: manifold3d + trimesh).
    rng = np.random.default_rng(20250205)
    for i in range(8):
        a, b = f"ra{i}", f"rb{i}"
        C.prism(a, round(rng.uniform(-1, 0), 6), round(rng.uniform(1.5, 3), 6), star(rng, 0, 0, 2.0))
        C.prism(b, round(rng.uniform(-0.5, 0.5), 6), round(rng.uniform(1.5, 3), 6),
                star(rng, rng.uniform(-1, 1), rng.uniform(-1, 1), 1.8))
        op = ["union", "difference", "intersection", None][i % 4]
        sid = a
        if op:
            sid = f"rc{i}"
            C.boolean(sid, op, a, b)
        for j in range(3):
            n = rng.normal(size=3)
            n /= np.linalg.norm(n)
            c = np.array([rng.uniform(-0.4, 0.4), rng.uniform(-0.4, 0.4), rng.uniform(0.2, 0.9)])
            C.section(f"rand{i}_{j}", sid, n, float(n @ c))
    return C


# ---------------------------------------------------------------- checking
def project(ring, u, v):
    return [(p[0] * u[0] + p[1] * u[1] + p[2] * u[2], p[0] * v[0] + p[1] * v[1] + p[2] * v[2]) for p in ring]


def signed_area(r):
    a = 0.0
    for (x0, y0), (x1, y1) in zip(r, r[1:] + r[:1]):
        a += x0 * y1 - x1 * y0
    return a / 2


def check_case(exp, got):
    name = exp["case"]
    n, off, u, v = np.array(exp["n"]), exp["offset"], exp["u"], exp["v"]
    A = exp["area"]
    tol = TOL * max(1.0, A)
    if not isinstance(got.get("area"), (int, float)) or not math.isfinite(got["area"]):
        return f"{name}: section_area is not a finite number"
    if abs(got["area"] - A) > tol:
        return f"{name}: section_area {got['area']} != expected {A}"
    if A == 0:
        if got["polys"]:
            return f"{name}: plane touches the solid only in an edge/vertex (or misses it); expected no loops, got {len(got['polys'])}"
        return None
    regions = []
    for k, p in enumerate(got["polys"]):
        rings = [("outer", p["outer"])] + [("hole", h) for h in p["holes"]]
        proj = []
        for kind, r in rings:
            if len(r) >= 2 and r[0] == r[-1]:
                r = r[:-1]
            if len(r) < 3:
                return f"{name}: polygon {k} has a {kind} loop with {len(r)} vertices"
            for pt in r:
                if abs(float(np.dot(n, pt)) - off) > 1e-7 * max(1.0, abs(off)):
                    return f"{name}: polygon {k} {kind} vertex {pt} is off the plane"
            q = project(r, u, v)
            sa = signed_area(q)
            if kind == "outer" and not sa > 0:
                return f"{name}: polygon {k} outer loop is not CCW about the plane normal (signed area {sa})"
            if kind == "hole" and not sa < 0:
                return f"{name}: polygon {k} hole loop is not CW about the plane normal (signed area {sa})"
            proj.append(q)
        poly = Polygon(proj[0], proj[1:])
        if not poly.is_valid:
            return f"{name}: polygon {k} is invalid (holes not nested inside its outer loop, or self-intersecting): {shapely.is_valid_reason(poly)}"
        regions.append(poly)
    total = sum(r.area for r in regions)
    if abs(total - A) > tol:
        return f"{name}: loops enclose area {total}, expected {A}"
    for i in range(len(regions)):
        for j in range(i + 1, len(regions)):
            if regions[i].intersection(regions[j]).area > tol:
                return f"{name}: polygons {i} and {j} overlap (wrong nesting)"
    union = unary_union(regions) if regions else Polygon()
    diff = union.symmetric_difference(shapely.from_wkt(exp["wkt"])).area
    if diff > tol:
        return f"{name}: section shape differs from the oracle by area {diff}"
    return None


def check(expected, outputs):
    got = {}
    errs = []
    for line in outputs:
        line = line.strip()
        if line:
            try:  # NaN/inf areas or malformed lines are failures, not crashes
                o = json.loads(line, parse_constant=lambda c: float("nan"))
                got[o["case"]] = o
            except (ValueError, KeyError, TypeError) as e:
                errs.append(f"driver output line is not a section record ({e}): {line[:80]}")
    for exp in expected:
        if exp["case"] not in got:
            errs.append(f"{exp['case']}: no output")
            continue
        e = check_case(exp, got[exp["case"]])
        if e:
            errs.append(e)
    return errs


def fabricate(exp, corrupt=None):
    """An answer made from the oracle's own geometry, lifted back to 3D."""
    n, off, u, v = map(np.array, (exp["n"], exp["offset"], exp["u"], exp["v"]))
    g = shapely.from_wkt(exp["wkt"])
    polys = [] if g.is_empty else list(getattr(g, "geoms", [g]))
    lift = lambda r: [list(x * u + y * v + off * n) for x, y in r]
    out = []
    for p in polys:
        p = shapely.geometry.polygon.orient(p, 1.0)
        out.append({"outer": lift(list(p.exterior.coords)[:-1]),
                    "holes": [lift(list(h.coords)[:-1]) for h in p.interiors]})
    if corrupt == "hole_ccw":
        for p in out:
            if p["holes"]:
                p["holes"][0] = p["holes"][0][::-1]
    if corrupt == "hole_as_outer":
        for p in out:
            if p["holes"]:
                out.append({"outer": p["holes"].pop(0)[::-1], "holes": []})
                break
    if corrupt == "shift" and out:
        out[0]["outer"][0] = [c + 0.01 * w for c, w in zip(out[0]["outer"][0], u)]
    area = exp["area"] + (0.01 if corrupt == "area" else 0)
    return {"case": exp["case"], "area": area, "polys": out}


def main():
    mode = sys.argv[1]
    if mode == "gen":
        C = build()
        open(sys.argv[2], "w").write("\n".join(C.lines) + "\n")
        json.dump(C.expected, open(sys.argv[3], "w"))
    elif mode == "check":
        expected = json.load(open(sys.argv[2]))
        errs = check(expected, open(sys.argv[3]).read().splitlines())
        if errs:
            fail(f"{len(errs)} of {len(expected)} section cases wrong; first: " + " | ".join(errs[:5]))
        print(f"T2.5 judge: {len(expected)} section cases agree with the oracle")
    elif mode == "selftest":
        expected = json.load(open(sys.argv[2]))
        good = [json.dumps(fabricate(e)) for e in expected]
        assert check(expected, good) == [], check(expected, good)
        for corrupt in ["hole_ccw", "hole_as_outer", "shift", "area"]:
            bad = [json.dumps(fabricate(e, corrupt)) for e in expected]
            errs = check(expected, bad)
            assert errs, corrupt
            print(f"corruption {corrupt}: {len(errs)} cases rejected, e.g. {errs[0]}")
        print("selftest ok")


if __name__ == "__main__":
    main()
