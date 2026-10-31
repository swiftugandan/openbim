# External judge for T4.8 (trade packages). Builders must not edit judges/.
#
# Drives the real `bim` binary with a trade package `fire` whose engine is the judge's own component
# (judges/T4.8/plugins/fire, built from source by T4.8.sh). That component echoes what the host passed, so
# the judge checks the host's `entities`, `rooms` and `solids` against what it authored, against closed
# forms (mitred wall prisms, column and box cuboids, 16-gon prisms with vertices at D/2) and against
# `bim solve --trade arch`. Also: `validate --standard fire`, err/invalid-JSON results, glTF export and
# `bim clash` of extension solids, `plugin list` trades, trap/spin/misnamed/missing engines.
# Usage: python -I judge.py <bim-binary> <scratch-dir> <dir with fire.wasm misnamed.wasm trap.wasm spin.wasm>
import base64
import json
import math
import os
import shutil
import subprocess
import sys
import time

import numpy as np

BIM, TMP, WASM = sys.argv[1], sys.argv[2], sys.argv[3]
VALIDATOR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "validate_gltf.mjs")
P16 = 8 * math.sin(math.pi / 8)  # area of a 16-gon with vertex radius 1


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def bim(args, timeout=60):
    t0 = time.monotonic()
    try:
        p = subprocess.run([BIM, *map(str, args)], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        fail(f"bim {' '.join(map(str, args))} did not finish within {timeout}s")
    p.elapsed = time.monotonic() - t0
    return p


def run(args, want=0, what=None):
    p = bim(args)
    if p.returncode != want:
        fail(f"{what or 'bim ' + ' '.join(map(str, args))}: exit {p.returncode}, want {want}; "
             f"stdout: {p.stdout.strip()[:300]!r}; stderr: {p.stderr.strip()[-400:]!r}")
    return p


def as_json(out, what):
    try:
        return json.loads(out)
    except json.JSONDecodeError as e:
        fail(f"{what} did not print one JSON document: {e}: {out[:300]!r}")


def close(actual, expected, what, rel=1e-9, abs_tol=1e-9):
    if not isinstance(actual, (int, float)) or isinstance(actual, bool) or not math.isfinite(actual):
        fail(f"{what}: got non-number {actual!r}")
    if abs(actual - expected) > rel * abs(expected) + abs_tol:
        fail(f"{what}: got {actual:.12g}, expected {expected:.12g}")


def create(i, kind, /, **props):
    return {"op": "create", "id": i, "kind": kind, "props": props}


def sett(i, key, value):
    return {"op": "set", "id": i, "key": key, "value": value}


def ref(i):
    return {"ref": i}


def apply(d, ops, name):
    f = os.path.join(TMP, name + ".json")
    with open(f, "w") as fh:
        json.dump({"schema": 1, "ops": ops}, fh)
    run(["apply", d, f])


def hash_of(d):
    return run(["hash", d]).stdout.strip()


# ---------------------------------------------------------------- the package and the model
FIRE = {"package": "fire", "version": "0.3.1", "abi": "openbim:trade@0.1.0",
        "kinds": {
            "fire:Sprinkler": {"props": {"level": "ref", "x": "num", "y": "num", "z": "num", "w": "num",
                                         "coverage_radius": "num", "label": "text"},
                               "geometry": {"box": {"centre": ["x", "y", "z"], "size": [0.3, "w", 0.2]}}},
            "fire:Main": {"props": {"x1": "num", "y1": "num", "z1": "num", "x2": "num", "y2": "num", "z2": "num",
                                    "diameter": "num"},
                          "geometry": {"segment": {"from": ["x1", "y1", "z1"], "to": ["x2", "y2", "z2"],
                                                   "diameter": "diameter"}}},
            "fire:Hydrant": {"props": {"x": "num", "y": "num", "h": "num"},
                             "geometry": {"segment": {"from": ["x", "y", 0], "to": ["x", "y", "h"], "diameter": 0.15}}},
            "fire:Zone": {"props": {"name": "text", "active": "bool", "rooms": "list", "level": "ref"}},
        },
        "engine": "fire.wasm"}
ACOUSTIC = {"package": "acoustic", "version": "1.0.0", "abi": "openbim:trade@0.1.0",
            "kinds": {"acoustic:Panel": {"props": {"nrc": "num"}}}}  # no engine: not a trade

T, H = 0.2, 3.0  # wall thickness and height
RECTS = {"A": (0.0, 0.0, 4.0, 3.0), "B": (6.0, 0.0, 10.0, 5.0)}  # two closed rooms, walls counter-clockwise
WALLS = {}
for tag, (x0, y0, x1, y1) in RECTS.items():
    pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    for k in range(4):
        WALLS[f"{tag}{k + 1}"] = (*pts[k], *pts[(k + 1) % 4])
SPRINKLERS = {  # id: (x, y, z, w, coverage_radius)
    "S1": (2.0, 1.5, 2.7, 0.3, 3.0),   # radius set to 2.0 below (violation)
    "S2": (11.9, 2.0, 1.5, 0.4, 3.0),  # overlaps column C1
    "S3": (8.0, 2.0, 2.7, 0.3, 3.0),   # overlaps S4
    "S4": (8.1, 2.1, 2.75, 0.3, 1.5),  # violation
    "S5": (7.0, 0.23, 1.5, 0.2, 3.0),  # 0.03 off the inner face of wall B1 (y = 0.1)
}
MAIN = ((1.0, 1.0, 2.0), (1.0, 5.0, 2.0), 0.1)  # crosses wall A3 (y = 3) at right angles
HYDRANT = (14.0, 4.0, 1.2)
COLUMN = (12.0, 2.0, 0.3)


def model_ops():
    ops = [create("L1", "Level", elevation=0.0), create("L2", "Level", elevation=3.0),
           create("WT", "WallType", layer_thickness=[T], layer_material=["concrete"], layer_conductivity=[1.6]),
           create("MAT", "Material", name="concrete", E=30e9, G=12.5e9, density=2400.0, fy=30e6)]
    for w, (x1, y1, x2, y2) in WALLS.items():
        ops.append(create(w, "Wall", level=ref("L1"), type=ref("WT"), x1=x1, y1=y1, x2=x2, y2=y2, height=H))
    ops += [create("C1", "Column", x=COLUMN[0], y=COLUMN[1], base=ref("L1"), top=ref("L2"), section_b=COLUMN[2],
                   section_h=COLUMN[2], material=ref("MAT")),
            create("SP1", "Space", level=ref("L1"), x=1.0, y=1.0, name="office", use="office")]
    for s, (x, y, z, w, r) in SPRINKLERS.items():
        ops.append(create(s, "fire:Sprinkler", level=ref("L1"), x=x, y=y, z=z, w=w, coverage_radius=r, label=s.lower()))
    (a, b, dia) = MAIN
    ops += [create("S6", "fire:Sprinkler", level=ref("L1"), x=3.0, y=2.0, z=2.7, coverage_radius=3.0),  # no w: no solid
            create("S7", "fire:Sprinkler", x=1.0, y=1.0, z=1.0, w=0.1),
            create("FM1", "fire:Main", x1=a[0], y1=a[1], z1=a[2], x2=b[0], y2=b[1], z2=0.0, diameter=dia),
            sett("FM1", "z2", b[2]),
            create("H1", "fire:Hydrant", x=HYDRANT[0], y=HYDRANT[1], h=HYDRANT[2]),
            create("Z1", "fire:Zone", name="north", active=True, rooms=["L1/1", 2, [ref("SP1")]], level=ref("L1")),
            sett("S1", "coverage_radius", 2.0),
            {"op": "delete", "id": "S7"}]
    return ops


def expected_props():
    """Non-reserved props of every live entity after model_ops (spec/schema.md JSON encoding)."""
    props = {}
    for o in model_ops():
        if o["op"] == "create":
            props[o["id"]] = (o["kind"], dict(o["props"]))
        elif o["op"] == "set":
            props[o["id"]][1][o["key"]] = o["value"]
        else:
            del props[o["id"]]
    return props


def box(lo, hi):
    lo, hi = np.array(lo, float), np.array(hi, float)
    return ("box", float(np.prod(hi - lo)), lo, hi)


def seg(a, b, d):
    a, b = np.array(a, float), np.array(b, float)
    return ("seg", P16 * (d / 2) ** 2 * float(np.linalg.norm(b - a)), a, b, d)


def expected_solids():
    out = {}
    for w, (x1, y1, x2, y2) in WALLS.items():  # mitred at both ends: L·t·h, bounds = centreline box ± t/2
        lo = np.array([min(x1, x2) - T / 2, min(y1, y2) - T / 2, 0.0])
        hi = np.array([max(x1, x2) + T / 2, max(y1, y2) + T / 2, H])
        out[w] = ("wall", math.hypot(x2 - x1, y2 - y1) * T * H, lo, hi)
    cx, cy, b = COLUMN
    out["C1"] = box([cx - b / 2, cy - b / 2, 0], [cx + b / 2, cy + b / 2, 3.0])
    for s, (x, y, z, w, _) in SPRINKLERS.items():
        out[s] = box([x - 0.15, y - w / 2, z - 0.1], [x + 0.15, y + w / 2, z + 0.1])
    out["FM1"] = seg(*MAIN)
    hx, hy, hh = HYDRANT
    out["H1"] = seg((hx, hy, 0.0), (hx, hy, hh), 0.15)
    return out


ROOMS = {"L1/1": (0.1, 0.1, 3.9, 2.9), "L1/2": (6.1, 0.1, 9.9, 4.9)}  # inner faces (closed form)


# ---------------------------------------------------------------- glTF reader (extras.id -> model-frame mesh)
COMP = {5120: np.int8, 5121: np.uint8, 5122: np.int16, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}
NCOMP = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


def read_gltf(path):
    g = json.load(open(path))
    bufs = []
    for b in g.get("buffers", []):
        uri = b.get("uri", "")
        if not uri.startswith("data:"):
            fail("glTF buffers must be embedded data URIs")
        bufs.append(base64.b64decode(uri.split(",", 1)[1]))

    def accessor(i):
        a = g["accessors"][i]
        bv = g["bufferViews"][a["bufferView"]]
        dt, n = np.dtype(COMP[a["componentType"]]), NCOMP[a["type"]]
        start = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        stride = bv.get("byteStride", dt.itemsize * n)
        raw = bufs[bv["buffer"]]
        out = np.empty((a["count"], n), dtype=np.float64)
        for k in range(a["count"]):
            out[k] = np.frombuffer(raw, dtype=dt, count=n, offset=start + k * stride)
        return out

    def local(node):
        if "matrix" in node:
            return np.array(node["matrix"], dtype=float).reshape(4, 4).T
        t = np.eye(4)
        x, y, z, w = node.get("rotation", [0, 0, 0, 1])
        r = np.array([[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
                      [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
                      [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]])
        t[:3, :3] = r * np.array(node.get("scale", [1, 1, 1]))
        t[:3, 3] = node.get("translation", [0, 0, 0])
        return t

    meshes = {}

    def walk(i, parent, pid):
        node = g["nodes"][i]
        m = parent @ local(node)
        eid = (node.get("extras") or {}).get("id", pid)
        if "mesh" in node:
            if eid is None:
                fail(f"glTF node {i} has a mesh but no extras.id")
            for prim in g["meshes"][node["mesh"]]["primitives"]:
                if prim.get("mode", 4) != 4:
                    fail("glTF primitive is not a triangle list")
                v = accessor(prim["attributes"]["POSITION"])
                v = (m @ np.c_[v, np.ones(len(v))].T).T[:, :3]
                idx = accessor(prim["indices"]).astype(np.int64).reshape(-1, 3) if "indices" in prim \
                    else np.arange(len(v)).reshape(-1, 3)
                meshes.setdefault(eid, []).append((v, idx))
        for c in node.get("children", []):
            walk(c, m, eid)

    scene = g["scenes"][g.get("scene", 0)]
    for r in scene["nodes"]:
        walk(r, np.eye(4), None)
    import trimesh
    to_model = np.array([[1, 0, 0], [0, 0, -1], [0, 1, 0]], float)  # glTF (x, y, z) = model (x, z, -y)
    out = {}
    for eid, parts in meshes.items():
        vs, fs, off = [], [], 0
        for v, f in parts:
            vs.append(v @ to_model.T)
            fs.append(f + off)
            off += len(v)
        tm = trimesh.Trimesh(np.vstack(vs), np.vstack(fs), process=False)
        tm.merge_vertices(digits_vertex=6)
        out[eid] = tm
    return out


def check_bounds(lo, hi, want, what, tol):
    """Bounds vs the closed form; for a 16-gon prism only the axial extent is exact, a radial extent lies
    between R·cos(π/16) and R (the polygon's rotation about its axis is not part of the contract)."""
    lo, hi = np.asarray(lo, float), np.asarray(hi, float)
    if want[0] != "seg":
        if not (np.allclose(lo, want[2], atol=tol, rtol=0) and np.allclose(hi, want[3], atol=tol, rtol=0)):
            fail(f"{what}: bounds {lo.tolist()}..{hi.tolist()}, expected {want[2].tolist()}..{want[3].tolist()}")
        return
    _, _, a, b, d = want
    r = d / 2
    for k in range(3):
        if abs(a[k] - b[k]) > 1e-12:  # axial (the judge's segments are axis-parallel)
            if abs(lo[k] - min(a[k], b[k])) > tol or abs(hi[k] - max(a[k], b[k])) > tol:
                fail(f"{what}: axial extent along axis {k} is {lo[k]}..{hi[k]}, expected {min(a[k], b[k])}..{max(a[k], b[k])}")
        else:
            for ext in (a[k] - lo[k], hi[k] - a[k]):
                if not (r * math.cos(math.pi / 16) - tol <= ext <= r + tol):
                    fail(f"{what}: radial extent {ext} along axis {k} is outside [R·cos(π/16), R] for R = {r}")


def check_mesh(eid, m, want):
    if not m.is_watertight or not m.is_winding_consistent:
        fail(f"glTF mesh of {eid} is not closed and consistently oriented")
    close(abs(m.volume), want[1], f"glTF mesh volume of {eid}", rel=1e-5, abs_tol=0)
    check_bounds(m.bounds[0], m.bounds[1], want, f"glTF mesh of {eid}", 1e-5)
    if want[0] == "seg":  # spec/schema.md: 16-gon prism with every vertex at radius D/2, exactly between the ends
        _, _, a, b, d = want
        L = np.linalg.norm(b - a)
        ax = (b - a) / L
        rel = m.vertices - a
        t = rel @ ax
        rad = np.linalg.norm(rel - np.outer(t, ax), axis=1)
        caps = (np.abs(t) < 1e-5, np.abs(t - L) < 1e-5)
        if not np.all(caps[0] | caps[1]) or any(c.sum() != 16 for c in caps):
            fail(f"segment solid {eid}: expected two 16-vertex caps exactly at its end points")
        if np.abs(rad - d / 2).max() > 1e-5:
            fail(f"segment solid {eid}: vertices at radius {rad.min()}..{rad.max()}, spec/schema.md requires D/2 = {d / 2}")


# ---------------------------------------------------------------- the checks
def check_echo(doc, d, arch_rooms):
    if not isinstance(doc, dict) or set(doc) != {"engine", "entities", "rooms", "solids"} or doc["engine"] != "fire":
        fail(f"solve --trade fire must print the engine's output.json unchanged; got keys "
             f"{sorted(doc) if isinstance(doc, dict) else type(doc).__name__}")
    # entities: one per live entity, sorted by id, props in the spec/schema.md JSON encoding
    want = expected_props()
    ents = doc["entities"]
    ids = [e.get("id") for e in ents]
    if ids != sorted(want):
        fail(f"engine input entities: ids {ids}, expected every live entity sorted by id: {sorted(want)}")
    for e in ents:
        kind, props = want[e["id"]]
        if e.get("kind") != kind:
            fail(f"engine input entity {e['id']}: kind {e.get('kind')!r}, expected {kind!r}")
        got = e.get("props")
        if not isinstance(got, dict):
            fail(f"engine input entity {e['id']}: props-json is not a JSON object: {got!r}")
        user = {k: v for k, v in got.items() if not k.startswith("_")}
        if user != props or any((type(user[k]) is bool) != (type(v) is bool) for k, v in props.items()):
            fail(f"engine input entity {e['id']}: props {user}, expected {props}")
    # rooms: exactly those of `bim solve --trade arch`, boundary flattened, sorted by id
    rooms = doc["rooms"]
    if [r.get("id") for r in rooms] != sorted(ROOMS):
        fail(f"engine input rooms: ids {[r.get('id') for r in rooms]}, expected {sorted(ROOMS)} (sorted by id)")
    arch = {r["id"]: r for r in arch_rooms}
    if set(arch) != set(ROOMS):
        fail(f"solve --trade arch reports rooms {sorted(arch)}, expected {sorted(ROOMS)}")
    for r in rooms:
        a = arch[r["id"]]
        flat = [float(c) for pt in a["boundary"] for c in pt]
        if r.get("level") != a["level"] or r.get("level") != "L1":
            fail(f"engine input room {r['id']}: level {r.get('level')!r}, arch reports {a['level']!r} (want L1)")
        close(r.get("area"), a["area"], f"engine input room {r['id']} area vs solve --trade arch", rel=1e-12, abs_tol=0)
        x0, y0, x1, y1 = ROOMS[r["id"]]
        close(r["area"], (x1 - x0) * (y1 - y0), f"engine input room {r['id']} area (closed form, inner faces)")
        b = r.get("boundary")
        if not isinstance(b, list) or len(b) != len(flat) or any(abs(p - q) > 1e-9 for p, q in zip(b, flat)):
            fail(f"engine input room {r['id']}: boundary {b}, expected arch's boundary flattened x0, y0, x1, y1, …: {flat}")
        xs, ys = b[0::2], b[1::2]
        if abs(min(xs) - x0) > 1e-9 or abs(max(xs) - x1) > 1e-9 or abs(min(ys) - y0) > 1e-9 or abs(max(ys) - y1) > 1e-9:
            fail(f"engine input room {r['id']}: boundary {b} does not span the inner faces {ROOMS[r['id']]}")
    # solids: one per entity with a derived solid, sorted by id; volume and [x, y, z] bounds
    sol = expected_solids()
    got_ids = [s.get("id") for s in doc["solids"]]
    if got_ids != sorted(sol):
        fail(f"engine input solids: ids {got_ids}, expected {sorted(sol)} (every entity with a solid — S6 lacks "
             f"the prop `w` its box needs, Z1 has no geometry — sorted by id)")
    for s in doc["solids"]:
        want_s = sol[s["id"]]
        close(s.get("volume"), want_s[1], f"engine input solid {s['id']} volume", rel=1e-6, abs_tol=0)
        lo, hi = s.get("min"), s.get("max")
        if not (isinstance(lo, list) and isinstance(hi, list) and len(lo) == 3 and len(hi) == 3):
            fail(f"engine input solid {s['id']}: min/max must be [x, y, z], got {lo!r} {hi!r}")
        check_bounds(lo, hi, want_s, f"engine input solid {s['id']}", 1e-7)
    return {s["id"]: s for s in doc["solids"]}


def check_violations(out, want):
    doc = as_json(out, "validate --standard fire --json")
    for k in ("violations", "dangling", "conflicts"):
        if not isinstance(doc.get(k), list):
            fail(f"validate --standard fire: missing list '{k}'")
    if doc["dangling"] or doc["conflicts"]:
        fail(f"validate --standard fire: unexpected dangling/conflicts {doc['dangling']} {doc['conflicts']}")
    got = sorted(((v.get("rule"), v.get("entity"), v.get("message"), v.get("measured"), v.get("limit"))
                  for v in doc["violations"]), key=lambda v: (str(v[0]), str(v[1])))
    if got != sorted(want):
        fail(f"validate --standard fire must report the engine's violations unchanged: got {got}, want {sorted(want)}")


def clash_checks(d, sol):
    p = run(["clash", d, "--clearance", "0.05", "--json"], want=1, what="bim clash (hard clashes present)")
    doc = as_json(p.stdout, "clash --json")
    for k in ("hard", "soft", "matrix"):
        if k not in doc:
            fail(f"clash output lacks '{k}'")
    a16 = P16 * (MAIN[2] / 2) ** 2
    want_hard = {frozenset(("FM1", "A3")): a16 * T,
                 frozenset(("S2", "C1")): 0.2 * 0.3 * 0.2,
                 frozenset(("S3", "S4")): 0.2 * 0.2 * 0.15}
    hard = {}
    for c in doc["hard"]:
        key = frozenset((c["a"], c["b"]))
        if len(key) != 2 or key in hard:
            fail(f"clash: bad or repeated hard pair {c}")
        hard[key] = c["volume"]
    if set(hard) != set(want_hard):
        fail(f"hard clashes {sorted(sorted(k) for k in hard)}, expected {sorted(sorted(k) for k in want_hard)}")
    for k, v in want_hard.items():
        close(hard[k], v, f"hard clash {sorted(k)} volume (closed form)", rel=1e-6, abs_tol=0)
    soft = {frozenset((c["a"], c["b"])): c["distance"] for c in doc["soft"]}
    if set(soft) != {frozenset(("S5", "B1"))}:
        fail(f"soft clashes {sorted(sorted(k) for k in soft)}, expected [['B1', 'S5']] (0.03 m at clearance 0.05)")
    close(soft[frozenset(("S5", "B1"))], 0.03, "soft clash S5/B1 distance (closed form)", rel=1e-6, abs_tol=1e-9)
    mat = doc["matrix"]
    if not isinstance(mat, dict) or not all(isinstance(v, dict) for v in mat.values()):
        fail(f"clash matrix is not an object of objects: {mat!r}")
    for ka, row in mat.items():
        for kb in [ka, *row]:
            if kb in ("Sprinkler", "Main", "Hydrant", "Zone") or (kb.startswith("fire") and kb not in FIRE["kinds"]):
                fail(f"clash matrix key {kb!r}: extension kinds use their full name (e.g. fire:Sprinkler)")

    def count(x, y):
        g = lambda p, q: (mat.get(p) or {}).get(q, 0)
        return g(x, y) if x == y else g(x, y) + g(y, x)
    for (x, y), n in {("fire:Main", "Wall"): 1, ("Column", "fire:Sprinkler"): 1,
                      ("fire:Sprinkler", "fire:Sprinkler"): 1, ("Wall", "Wall"): 0}.items():
        if count(x, y) != n:
            fail(f"clash matrix {x}/{y} = {count(x, y)}, expected {n}: {mat}")


def main():
    d = os.path.join(TMP, "P")
    run(["init", d])
    plug = os.path.join(d, "plugins", "fire")
    os.makedirs(plug)
    json.dump(FIRE, open(os.path.join(plug, "trade.json"), "w"))
    shutil.copy(os.path.join(WASM, "fire.wasm"), os.path.join(plug, "fire.wasm"))
    os.makedirs(os.path.join(d, "plugins", "acoustic"))
    json.dump(ACOUSTIC, open(os.path.join(d, "plugins", "acoustic", "trade.json"), "w"))
    apply(d, model_ops(), "model")
    h0 = hash_of(d)

    # 1. plugin list: trades = Registry::engine_names(), sorted, with the package engine
    lst = as_json(run(["plugin", "list", d, "--json"]).stdout, "plugin list --json")
    trades = lst.get("trades")
    if not isinstance(trades, list) or trades != sorted(set(trades)) or not {"arch", "mep", "struct", "fire"} <= set(trades):
        fail(f"plugin list trades: {trades!r}, expected a sorted list of engine names including arch, fire, mep, struct")
    if "acoustic" in trades:
        fail("plugin list trades lists acoustic, a package without an engine")
    pk = {p.get("name"): p for p in lst.get("packages", [])}
    if pk.get("fire", {}).get("engine") != "plugins/fire/fire.wasm" or pk.get("acoustic", {}).get("engine", 0) is not None:
        fail(f"plugin list packages: {lst.get('packages')}")

    # 2. solve --trade fire prints output.json; the host's inputs are what the judge authored
    arch = as_json(run(["solve", d, "--trade", "arch", "--json"]).stdout, "solve --trade arch")
    p = run(["solve", d, "--trade", "fire", "--json"])
    sol = check_echo(as_json(p.stdout, "solve --trade fire --json"), d, arch.get("rooms", []))

    # 3. validate --standard fire reports output.violations unchanged; exit 1 iff any
    msg = f"n={len(expected_props())} rooms={len(ROOMS)} solids={len(expected_solids())}"
    p = run(["validate", d, "--standard", "fire", "--json"], want=1, what="validate --standard fire (2 violations)")
    check_violations(p.stdout, [("FIRE-COVER", "S1", msg, 2.0, 2.5), ("FIRE-COVER", "S4", msg, 1.5, 2.5)])

    # 4. glTF: one closed mesh node per solid, extension solids included, exact volumes
    f = os.path.join(TMP, "p.gltf")
    run(["export", d, "--format", "gltf", "--out", f])
    try:
        v = subprocess.run(["node", VALIDATOR, f], capture_output=True, text=True, timeout=120)
    except subprocess.TimeoutExpired:
        fail("glTF-Validator timed out")
    if v.returncode != 0:
        fail(f"glTF export: {v.stdout.strip()} {v.stderr.strip()[-300:]}")
    meshes = read_gltf(f)
    want_sol = expected_solids()
    if set(meshes) != set(want_sol):
        fail(f"glTF mesh nodes {sorted(meshes)}, expected one per solid: {sorted(want_sol)}")
    for eid, want in want_sol.items():
        check_mesh(eid, meshes[eid], want)
        s = sol[eid]
        if not (np.allclose(s["min"], meshes[eid].bounds[0], atol=1e-5) and np.allclose(s["max"], meshes[eid].bounds[1], atol=1e-5)):
            fail(f"solid {eid}: engine input bounds {s['min']}..{s['max']} differ from its glTF mesh {meshes[eid].bounds.tolist()}")

    # 5. clash: extension solids clash like core ones; matrix uses full kind names
    clash_checks(d, sol)
    if hash_of(d) != h0:
        fail("solve/validate/export/clash changed the document hash")

    # 6. compliant model: no violations, exit 0; err(..) exits 1 with the message; invalid output.json exits 3
    apply(d, [sett("S1", "coverage_radius", 2.5), sett("S4", "coverage_radius", 4.0)], "fix")
    p = run(["validate", d, "--standard", "fire", "--json"], what="validate --standard fire on a compliant model")
    check_violations(p.stdout, [])
    apply(d, [sett("Z1", "name", "ERR")], "err")
    for cmd in (["solve", d, "--trade", "fire", "--json"], ["validate", d, "--standard", "fire", "--json"]):
        p = run(cmd, want=1, what=f"{cmd[0]} when the engine returns err(..)")
        if "fire engine refused zone Z1" not in p.stderr:
            fail(f"{cmd[0]}: err(message) must print the message on stderr; stderr: {p.stderr.strip()[-300:]!r}")
    apply(d, [sett("Z1", "name", "BADJSON")], "badjson")
    run(["solve", d, "--trade", "fire", "--json"], want=3, what="solve when output.json is not valid JSON")
    apply(d, [sett("Z1", "name", "north")], "restore")

    # 7. engines that break the host rules, each installed as the fire package's engine in a copy
    def variant(tag, wasm):
        v = os.path.join(TMP, tag)
        shutil.copytree(d, v)
        target = os.path.join(v, "plugins", "fire", "fire.wasm")
        os.remove(target)
        if wasm is not None:
            if isinstance(wasm, bytes):
                open(target, "wb").write(wasm)
            else:
                shutil.copy(os.path.join(WASM, wasm), target)
        return v

    for tag, wasm in (("missing", None), ("garbage", b"\0asm not a component"), ("misnamed", "misnamed.wasm")):
        v = variant(tag, wasm)
        # only invoking the engine fails
        run(["hash", v], what=f"bim hash with a {tag} engine")
        run(["plugin", "list", v, "--json"], what=f"plugin list with a {tag} engine")
        run(["solve", v, "--trade", "arch", "--json"], what=f"solve --trade arch with a {tag} fire engine")
        run(["validate", v, "--json"], what=f"plain validate with a {tag} fire engine")
        apply(v, [sett("S1", "label", tag)], f"apply-{tag}")
        run(["solve", v, "--trade", "fire", "--json"], want=2, what=f"solve --trade fire with a {tag} engine")
        run(["validate", v, "--standard", "fire", "--json"], want=2, what=f"validate --standard fire with a {tag} engine")

    elapsed = {}
    for tag in ("trap", "spin"):
        v = variant(tag, f"{tag}.wasm")
        for cmd in (["validate", v, "--standard", "fire", "--json"], ["solve", v, "--trade", "fire", "--json"]):
            p = run(cmd, want=3, what=f"{cmd[0]} with a {tag} engine")
            if not p.stderr.strip():
                fail(f"{cmd[0]} with a {tag} engine exited 3 without a message on stderr")
            elapsed[(tag, cmd[0])] = p.elapsed
    for c in ("validate", "solve"):
        extra = elapsed[("spin", c)] - elapsed[("trap", c)]
        if extra > 12.0:
            fail(f"{c} with a never-returning engine took {elapsed[('spin', c)]:.1f}s ({extra:.1f}s more than a "
                 "trapping one); fuel must stop it within 10 s")
    if hash_of(d) == h0:
        fail("judge self-check: the edits of step 6 did not change the hash")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, TypeError, ValueError, IndexError, AttributeError) as e:
        fail(f"malformed bim output or missing entity: {type(e).__name__}: {e}")
    print("T4.8 trade package checks passed")
