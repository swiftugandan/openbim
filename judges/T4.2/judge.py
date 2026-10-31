"""External judge for T4.2 (architectural engine: rooms, areas, volumes, layer takeoff).

Drives the real `bim` binary (argv[1]) through spec/cli.md and recomputes every room
independently with shapely: the wall centrelines of a level are noded with unary_union and
polygonized; each bounded face is shrunk by offsetting every edge inward by the half thickness
of the wall that carries it (spec/schema.md "Rooms"). Volumes follow the same schema rule.
Run as: .oracles/py/bin/python -I judges/T4.2/judge.py <path-to-bim>
"""
import json
import math
import os
import subprocess
import sys
import tempfile

from shapely.geometry import LineString, Point, Polygon
from shapely.geometry.polygon import orient
from shapely.ops import polygonize, unary_union

BIM = sys.argv[1]
REL = 1e-6        # exact geometry: areas/volumes within 1e-6 relative
TIE = 1e-6        # centroid x values closer than this are ordered by y (spec gap)
failures = []


def fail(msg):
    failures.append(msg)


def run(*args, ok=(0,)):
    p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=60)
    if p.returncode not in ok:
        print(f"JUDGE FAIL: bim {' '.join(args)} exited {p.returncode}: {p.stderr.strip()[:500]}")
        sys.exit(1)
    return p.stdout


# ---------------------------------------------------------------- document builder

class Doc:
    def __init__(self):
        self.ops = []
        self.levels = {}      # id -> elevation
        self.types = {}       # id -> (thicknesses, materials)
        self.walls = []       # dicts
        self.slabs = []       # (level, boundary pts, thickness)
        self.spaces = []      # (id, level, x, y)
        self.openings = []

    def create(self, id_, kind, props):
        self.ops.append({"op": "create", "id": id_, "kind": kind, "props": props})

    def level(self, id_, elev):
        self.levels[id_] = elev
        self.create(id_, "Level", {"elevation": elev})

    def wall_type(self, id_, thicknesses, materials):
        self.types[id_] = (thicknesses, materials)
        self.create(id_, "WallType", {"layer_thickness": thicknesses, "layer_material": materials,
                                      "layer_conductivity": [1.0] * len(thicknesses)})

    def wall(self, id_, level, a, b, type_, height=2.7):
        self.walls.append({"id": id_, "level": level, "a": a, "b": b, "type": type_, "height": height})
        self.create(id_, "Wall", {"level": {"ref": level}, "x1": a[0], "y1": a[1], "x2": b[0], "y2": b[1],
                                  "height": height, "type": {"ref": type_}})

    def slab(self, id_, level, pts, thickness):
        self.slabs.append((level, pts, thickness))
        flat = [c for p in pts for c in p]
        self.create(id_, "Slab", {"level": {"ref": level}, "boundary": flat, "thickness": thickness})

    def space(self, id_, level, x, y, name):
        self.spaces.append((id_, level, x, y))
        self.create(id_, "Space", {"level": {"ref": level}, "x": x, "y": y, "name": name})

    def opening(self, id_, host, offset, sill, width, height):
        self.openings.append({"host": host, "width": width, "height": height})
        self.create(id_, "Opening", {"host": {"ref": host}, "kind": "window", "offset": offset,
                                     "sill": sill, "width": width, "height": height})

    def half(self, wall):
        return sum(self.types[wall["type"]][0]) / 2.0


def polygon_rect(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def loop_walls(doc, prefix, level, pts, type_, reverse_every_other=True):
    """Walls along a closed polyline; some drawn backwards so direction cannot matter."""
    for i, a in enumerate(pts):
        b = pts[(i + 1) % len(pts)]
        if reverse_every_other and i % 2:
            a, b = b, a
        doc.wall(f"{prefix}{i}", level, a, b, type_)


# ---------------------------------------------------------------- independent oracle

def line_intersection(p, d, q, e):
    den = d[0] * e[1] - d[1] * e[0]
    if abs(den) < 1e-12:
        raise ValueError("parallel offset lines")
    t = ((q[0] - p[0]) * e[1] - (q[1] - p[1]) * e[0]) / den
    return (p[0] + t * d[0], p[1] + t * d[1])


def inner_polygon(doc, face, walls):
    """Shrink a polygonized face by each carrying wall's half thickness (mitred corners)."""
    face = orient(face, sign=1.0)  # CCW: inward normal is the left normal
    pts = list(face.exterior.coords)[:-1]
    edges = []  # (start, direction, half thickness)
    for i, p in enumerate(pts):
        q = pts[(i + 1) % len(pts)]
        mid = Point((p[0] + q[0]) / 2, (p[1] + q[1]) / 2)
        owners = [w for w in walls if LineString([w["a"], w["b"]]).distance(mid) < 1e-9]
        if len(owners) != 1:
            raise ValueError(f"face edge {p}->{q} carried by {len(owners)} walls")
        h = doc.half(owners[0])
        d = (q[0] - p[0], q[1] - p[1])
        if edges:
            sp, sd, sh = edges[-1]
            if abs(sd[0] * d[1] - sd[1] * d[0]) < 1e-12 * math.hypot(*sd) * math.hypot(*d):
                if abs(sh - h) > 1e-12:
                    raise ValueError("collinear edges with different thickness")
                edges[-1] = (sp, (q[0] - sp[0], q[1] - sp[1]), sh)
                continue
        edges.append((p, d, h))
    # the first and last edge may also be collinear
    (p0, d0, h0), (pl, dl, hl) = edges[0], edges[-1]
    if abs(d0[0] * dl[1] - d0[1] * dl[0]) < 1e-12 * math.hypot(*d0) * math.hypot(*dl):
        edges[0] = (pl, (p0[0] + d0[0] - pl[0], p0[1] + d0[1] - pl[1]), h0)
        edges.pop()
    off = []
    for p, d, h in edges:
        n = math.hypot(*d)
        nx, ny = -d[1] / n, d[0] / n
        off.append(((p[0] + nx * h, p[1] + ny * h), d))
    verts = [line_intersection(*off[i - 1], *off[i]) for i in range(len(off))]
    poly = Polygon(verts)
    if not poly.is_valid or poly.area <= 0:
        raise ValueError("offset polygon degenerate")
    return poly


def oracle_rooms(doc):
    """{level: [(polygon, space id or None)]} numbered by ascending centroid x, then y."""
    elevs = sorted(doc.levels.values())
    out = {}
    for lvl, elev in doc.levels.items():
        walls = [w for w in doc.walls if w["level"] == lvl]
        if not walls:
            out[lvl] = []
            continue
        noded = unary_union([LineString([w["a"], w["b"]]) for w in walls])
        rooms = []
        for face in polygonize(noded):
            poly = inner_polygon(doc, face, walls)
            higher = [e for e in elevs if e > elev + 1e-9]
            if higher:
                nxt = min(higher)
                nxt_lvl = [k for k, v in doc.levels.items() if v == nxt][0]
                above = [t for (l, pts, t) in doc.slabs if l == nxt_lvl and Polygon(pts).contains(poly.centroid)]
                volume = poly.area * (nxt - elev - (above[0] if above else 0.0))
            else:
                volume = None
            labels = [sid for (sid, l, x, y) in doc.spaces if l == lvl and poly.contains(Point(x, y))]
            rooms.append((poly, volume, labels[0] if labels else None))
        rooms.sort(key=lambda r: (round(r[0].centroid.x / TIE), r[0].centroid.y))
        out[lvl] = rooms
    return out


def self_test():
    """The offset construction agrees with shapely's mitred negative buffer for uniform walls."""
    d = Doc()
    d.level("L", 0.0)
    d.wall_type("T", [0.1, 0.12], ["a", "b"])
    pts = [(0, 0), (6, 0), (6, 3), (3, 3), (3, 6), (0, 6)]
    loop_walls(d, "w", "L", pts, "T")
    face = Polygon(pts)
    mine = inner_polygon(d, face, d.walls)
    ref = face.buffer(-0.11, join_style="mitre", mitre_limit=50)
    assert abs(mine.area - ref.area) < 1e-9 and mine.symmetric_difference(ref).area < 1e-9, "oracle self-test"
    assert abs(mine.area - (5.78 * 2.78 + 2.78 * (6 - 0.11 - 2.89))) < 1e-9


# ---------------------------------------------------------------- scenarios

def build_main():
    d = Doc()
    d.level("L0", 0.0)
    d.level("L1", 3.0)
    d.level("L2", 6.5)
    d.level("L3", 9.5)
    d.wall_type("EXT", [0.1, 0.2], ["brick", "block"])     # 0.30 thick
    d.wall_type("INT", [0.15], ["stud"])                    # 0.15 thick
    d.wall_type("THIN", [0.04, 0.02, 0.04], ["gyp", "air", "gyp"])  # 0.10 thick

    # L0: rectangle with a thinner partition meeting the long walls at T-junctions.
    loop_walls(d, "a", "L0", polygon_rect(0, 0, 8, 5), "EXT")
    d.wall("a_part", "L0", (3, 5), (3, 0), "INT")
    d.space("S_kitchen", "L0", 1.5, 2.5, "kitchen")
    d.space("S_living", "L0", 6.0, 2.0, "living")

    # L1: rectangle split by two crossing walls into four rooms (x-ties broken by y),
    # plus a dangling stub that must not affect anything.
    loop_walls(d, "b", "L1", polygon_rect(0, 0, 10, 6), "EXT")
    d.wall("b_v", "L1", (4, 0), (4, 6), "INT")
    d.wall("b_h", "L1", (10, 2.5), (0, 2.5), "INT")
    d.wall("b_stub", "L1", (7, 6), (7, 4.5), "INT")
    d.space("S_office", "L1", 8.0, 4.0, "office")

    # L2: an L-shaped room and, apart from it, a non-orthogonal quadrilateral split by a
    # skew partition whose ends land mid-wall (non-orthogonal T-junctions).
    loop_walls(d, "c", "L2", [(0, 0), (6, 0), (6, 3), (3, 3), (3, 6), (0, 6)], "THIN")
    quad = [(10, 0), (15, 1), (14, 5), (9.5, 4)]
    loop_walls(d, "q", "L2", quad, "EXT")
    d.wall("q_part", "L2", (12.5, 0.5), (11.75, 4.5), "INT")
    d.space("S_ell", "L2", 1.0, 5.0, "ell")

    # Slabs above L0 (on L1) and above L2 (on L3); none on L2, so L1 rooms use the full storey.
    d.slab("SL1", "L1", polygon_rect(-1, -1, 20, 10), 0.25)
    d.slab("SL3", "L3", polygon_rect(-1, -1, 20, 10), 0.2)
    return d


def build_takeoff():
    """An isolated layered wall with a window: no rooms, exact per-layer volumes."""
    d = Doc()
    d.level("G", 0.0)
    d.level("G1", 3.0)
    d.wall_type("LAY", [0.02, 0.15, 0.08], ["render", "concrete", "insulation"])
    d.wall("solo", "G", (1.0, 1.0), (5.0, 4.0), "LAY", height=2.8)  # length 5
    d.opening("win", "solo", 1.0, 0.9, 1.2, 1.1)
    return d


# ---------------------------------------------------------------- checks

def close(a, b, rel=REL, absol=1e-9):
    return abs(a - b) <= max(rel * abs(b), absol)


def project(doc, tmp, name):
    path = os.path.join(tmp, name)
    run("init", path)
    ops = os.path.join(tmp, name + ".json")
    with open(ops, "w") as f:
        json.dump({"schema": 1, "ops": doc.ops}, f)
    run("apply", path, ops)
    out = run("solve", path, "--trade", "arch", "--json")
    try:
        return json.loads(out)
    except json.JSONDecodeError as e:
        print(f"JUDGE FAIL: solve --json printed invalid JSON ({e}): {out[:300]}")
        sys.exit(1)


def check_rooms(doc, got):
    want = oracle_rooms(doc)
    rooms = got.get("rooms")
    if not isinstance(rooms, list):
        fail("output has no rooms[] array")
        return
    by_level = {}
    for r in rooms:
        by_level.setdefault(r.get("level"), []).append(r)
    for lvl in sorted(set(by_level) - set(want)):
        fail(f"rooms reported for unknown level {lvl!r}")
    for lvl, exp in want.items():
        have = by_level.get(lvl, [])
        if len(have) != len(exp):
            fail(f"level {lvl}: {len(have)} rooms, expected {len(exp)}")
            continue
        for n, (poly, volume, space) in enumerate(exp, start=1):
            rid = f"{lvl}/{n}"
            match = [r for r in have if r.get("id") == rid]
            if len(match) != 1:
                fail(f"room id {rid} missing or duplicated (ids: {[r.get('id') for r in have]})")
                continue
            r = match[0]
            if not close(r.get("area", float("nan")), poly.area):
                fail(f"{rid}: area {r.get('area')} expected {poly.area}")
            try:
                bpoly = Polygon([tuple(p) for p in r["boundary"]])
                diff = bpoly.symmetric_difference(poly).area
            except Exception as e:  # malformed boundary
                fail(f"{rid}: boundary unusable ({e})")
            else:
                if diff > REL * poly.area:
                    fail(f"{rid}: boundary differs from inner-face polygon by area {diff}")
            if volume is not None and not close(r.get("volume", float("nan")), volume):
                fail(f"{rid}: volume {r.get('volume')} expected {volume}")
            if r.get("space", "<missing>") != space:
                fail(f"{rid}: space {r.get('space', '<missing>')!r} expected {space!r}")


def check_takeoff(doc, got):
    walls = {w.get("id"): w for w in got.get("walls", [])}
    w = walls.get("solo")
    if w is None:
        fail("walls[] has no entry for 'solo'")
        return
    th, mats = doc.types["LAY"]
    length, height = 5.0, 2.8
    net = length * height - 1.2 * 1.1
    layers = w.get("layers", [])
    if sorted(l.get("material") for l in layers) != sorted(mats):
        fail(f"solo layers {[l.get('material') for l in layers]} expected {mats}")
        return
    for t, m in zip(th, mats):
        v = [l["volume"] for l in layers if l.get("material") == m][0]
        if not close(v, t * net):
            fail(f"solo layer {m}: volume {v} expected {t * net}")
    if not close(sum(l["volume"] for l in layers), w.get("volume", float("nan"))):
        fail("solo: layer volumes do not sum to the wall volume")
    if got.get("rooms"):
        fail(f"an isolated wall produced rooms: {got['rooms']}")


def check_stable(tmp, name, first):
    path = os.path.join(tmp, name)
    again = json.loads(run("solve", path, "--trade", "arch", "--json"))
    if again.get("rooms") != first.get("rooms"):
        fail("solve --trade arch is not deterministic")


def main():
    self_test()
    with tempfile.TemporaryDirectory() as tmp:
        main_doc = build_main()
        got = project(main_doc, tmp, "main")
        check_rooms(main_doc, got)
        check_stable(tmp, "main", got)

        # Moving a partition moves the derived rooms (geometry is derived, not stored).
        edit = os.path.join(tmp, "edit.json")
        with open(edit, "w") as f:
            json.dump({"schema": 1, "ops": [{"op": "set", "id": "a_part", "key": "x1", "value": 5.0},
                                            {"op": "set", "id": "a_part", "key": "x2", "value": 5.0}]}, f)
        run("apply", os.path.join(tmp, "main"), edit)
        for w in main_doc.walls:
            if w["id"] == "a_part":
                w["a"], w["b"] = (5, 5), (5, 0)
        got2 = json.loads(run("solve", os.path.join(tmp, "main"), "--trade", "arch", "--json"))
        check_rooms(main_doc, got2)

        tdoc = build_takeoff()
        check_takeoff(tdoc, project(tdoc, tmp, "takeoff"))
    if failures:
        for f in failures:
            print(f"JUDGE FAIL: {f}")
        sys.exit(1)
    print("T4.2 judge: all checks passed")


if __name__ == "__main__":
    main()
