# External judge for T4.7 (sizing, circuits, segment solids, clash). Builders must not edit judges/.
#
# Drives the real `bim` binary and compares with
#   * closed forms: nominal-size selection (velocity / Colebrook pressure-gradient limits), circuit
#     current and voltage drop, 16-gon (vertices at D/2) / rectangular prism volumes, clash volumes of
#     pipes and ducts passing through walls, columns and ducts, clearance distances between parallel faces;
#   * manifold3d: intersection volumes of the solids `bim export --format gltf` writes (read back in the
#     y-up frame of spec/cli.md: glTF (x, y, z) = model (x, z, -y));
#   * trimesh proximity: clearance distances between those exported solids.
# Usage: python -I judge.py <bim-binary> <scratch-dir>
import base64
import json
import math
import os
import re
import subprocess
import sys

import numpy as np

BIM, TMP = sys.argv[1], sys.argv[2]
DN = [15, 20, 25, 32, 40, 50, 65, 80, 100, 125, 150, 200, 250, 300]
LIMITS = {"water": (2.0, 400.0), "air": (6.0, 1.5)}  # (max velocity m/s, max gradient Pa/m)


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def run(args, want=0):
    try:
        p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        fail(f"bim {' '.join(args)} timed out")
    if p.returncode != want:
        fail(f"bim {' '.join(args)} exited {p.returncode}, want {want}: {p.stderr.strip()[-400:]}")
    return p.stdout


def as_json(out, what):
    try:
        return json.loads(out)
    except json.JSONDecodeError as e:
        fail(f"{what} did not print one JSON document: {e}: {out[:200]!r}")


def apply(d, ops, name):
    f = os.path.join(TMP, name + ".json")
    with open(f, "w") as fh:
        json.dump({"schema": 1, "ops": ops}, fh)
    run(["apply", d, f])


def project(name, ops):
    d = os.path.join(TMP, name)
    run(["init", d])
    apply(d, ops, name)
    return d


def close(actual, expected, rel, what, abs_tol=0.0):
    if not isinstance(actual, (int, float)) or not math.isfinite(actual):
        fail(f"{what}: got non-number {actual!r}")
    if abs(actual - expected) > rel * abs(expected) + abs_tol:
        fail(f"{what}: got {actual:.9g}, expected {expected:.9g}")


def colebrook(re_, rr):
    if re_ < 2000:
        return 64 / re_
    f = 0.02
    for _ in range(200):
        f = (-2 * math.log10(rr / 3.7 + 2.51 / (re_ * math.sqrt(f)))) ** -2
    return f


def create(i, entity_kind, /, **props):
    return {"op": "create", "id": i, "kind": entity_kind, "props": props}


def ref(i):
    return {"ref": i}


# ---------------------------------------------------------------- 1. sizing
def grad_and_v(q, dh, area, rho, mu, eps):
    v = q / area
    re_ = rho * v * dh / mu
    return colebrook(re_, eps / dh) / dh * rho * v * v / 2, v


def expected_size(q, shape, medium, rho, mu, eps, height=None):
    vmax, gmax = LIMITS[medium]
    if shape == "round":
        cands = [(n, n / 1000, math.pi * (n / 1000) ** 2 / 4) for n in DN]
    else:
        h = height
        cands = [(k * 50, 2 * (k * 0.05) * h / (k * 0.05 + h), k * 0.05 * h) for k in range(1, 400)]
    prev = None
    for n, dh, area in cands:
        g, v = grad_and_v(q, dh, area, rho, mu, eps)
        if v <= vmax and g <= gmax:
            # judge self-check: the decision is not within 0.5 % of a limit (robust to rounding)
            for gg, vv in ((g, v), prev or (0, 0)):
                for val, lim in ((vv, vmax), (gg, gmax)):
                    if lim and abs(val / lim - 1) < 5e-3:
                        raise AssertionError(f"sizing case q={q} too close to a limit")
            binding = None
            if prev:
                binding = "gradient" if prev[1] <= vmax else "velocity"
            return n, binding
        prev = (g, v)
    raise AssertionError("no size fits")


def parse_size(s, shape):
    if shape == "round":
        if isinstance(s, (int, float)):
            return int(round(s))
        m = re.fullmatch(r"\s*DN\s*(\d+)\s*", str(s))
        return int(m.group(1)) if m else None
    m = re.fullmatch(r"\s*(\d+)\s*[x×]\s*(\d+)\s*", str(s))
    return (int(m.group(1)), int(m.group(2))) if m else None


def sizing():
    ops, cases = [], []
    media = {"water": (998.2, 1.002e-3, 4.5e-5), "air": (1.2, 1.81e-5, 1.5e-4)}
    plan = [("water", "round", q) for q in (0.00015, 0.0004, 0.001, 0.003, 0.008, 0.02, 0.05, 0.1)]
    plan += [("air", "round", q) for q in (0.02, 0.1, 0.3)]
    plan += [("air", "rect", q) for q in (0.1, 0.5, 1.1)]
    bindings = set()
    for k, (medium, shape, q) in enumerate(plan):
        rho, mu, eps = media[medium]
        sid, a, b, s = f"Z{k}", f"z{k}s", f"z{k}k", f"zs{k}"
        ops += [create(sid, "MepSystem", name=sid, medium=medium, density=rho, viscosity=mu),
                create(a, "Port", system=ref(sid), x=0.0, y=2.0 * k, z=0.0, kind="source", pressure=3e5),
                create(b, "Port", system=ref(sid), x=10.0, y=2.0 * k, z=0.0, kind="sink", flow=q)]
        props = {"from": ref(a), "to": ref(b), "roughness": eps}
        if shape == "round":
            props.update(shape="round", diameter=0.05)
            want, binding = expected_size(q, shape, medium, rho, mu, eps)
        else:
            props.update(shape="rect", width=0.3, height=0.2)
            want = (expected_size(q, shape, medium, rho, mu, eps, height=0.2)[0], 200)
            binding = None
        bindings.add(binding)
        ops.append({"op": "create", "id": s, "kind": "Segment", "props": props})
        cases.append((s, shape, want, f"{medium} {shape} q={q}"))
    assert {"gradient", "velocity"} <= bindings, bindings  # both limits decide some case
    d = project("sizing", ops)
    doc = as_json(run(["solve", d, "--trade", "mep", "--json"]), "solve --trade mep")
    segs = {s["id"]: s for s in doc.get("segments", [])}
    for s, shape, want, what in cases:
        if s not in segs:
            fail(f"sizing: segment {s} missing from solve output")
        got = parse_size(segs[s].get("size"), shape)
        if got != want:
            fail(f"sizing {what}: size {segs[s].get('size')!r}, expected "
                 f"{'DN%d' % want if shape == 'round' else '%dx%d' % want}")


# ---------------------------------------------------------------- 2. circuits
def circuits():
    ops = [create("PWR", "MepSystem", name="power", medium="water", density=1000.0, viscosity=1e-3)]
    for i, (x, y) in enumerate([(0, 0), (5, 0), (10, 0), (15, 0), (0, 8), (6, 8)]):
        ops.append(create(f"e{i}", "Port", system=ref("PWR"), x=float(x), y=float(y), z=0.0, kind="junction"))
    spec = {
        "C1": dict(panel="e0", loads=["e1", "e2", "e3"], watts=[1000.0, 2000.0, 1500.0], V=230.0, A=2.5e-6,
                   rho=1.72e-8, L=30.0),
        "C2": dict(panel="e4", loads=["e5"], watts=[3680.0], V=120.0, A=6e-6, rho=2.82e-8, L=18.5),
    }
    for cid, c in spec.items():
        ops.append(create(cid, "Circuit", panel=ref(c["panel"]), voltage=c["V"], loads=[ref(p) for p in c["loads"]],
                          load_watts=c["watts"], conductor_area=c["A"], resistivity=c["rho"], length=c["L"]))
    d = project("circuits", ops)
    doc = as_json(run(["solve", d, "--trade", "mep", "--json"]), "solve --trade mep")
    got = {c["id"]: c for c in doc.get("circuits", [])}
    for cid, c in spec.items():
        if cid not in got:
            fail(f"circuit {cid} missing from solve output")
        current = sum(c["watts"]) / c["V"]
        drop = current * 2 * c["rho"] * c["L"] / c["A"]
        close(got[cid]["current"], current, 1e-9, f"circuit {cid} current")
        close(got[cid]["voltage_drop"], drop, 1e-9, f"circuit {cid} voltage_drop")
        close(got[cid]["voltage_drop_pct"], 100 * drop / c["V"], 1e-9, f"circuit {cid} voltage_drop_pct")


# ---------------------------------------------------------------- 3. glTF reader (extras.id -> world mesh)
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
    out = {}
    for eid, parts in meshes.items():
        vs, fs, off = [], [], 0
        for v, f in parts:
            vs.append(v)
            fs.append(f + off)
            off += len(v)
        tm = trimesh.Trimesh(np.vstack(vs), np.vstack(fs), process=False)
        tm.merge_vertices(digits_vertex=6)
        out[eid] = tm
    return out


# ---------------------------------------------------------------- 4. the clash scene
P16 = 8 * math.sin(math.pi / 8)  # 16-gon area = P16 * R^2 (R = vertex radius)
SEGS = {  # id: (shape, dims, from xyz, to xyz)
    "P1": ("round", 0.1, (2, -1, 1.5), (2, 1, 1.5)),        # through wall W1 (0.2 thick)
    "D1": ("rect", (0.4, 0.3), (4, -1, 1.0), (4, 1, 1.0)),  # through wall W1
    "P2": ("round", 0.1, (5, -1, 1.2), (5, 1, 1.2)),        # through the window opening of W1: no clash
    "D2": ("rect", (0.2, 0.2), (1, 0.23, 2.5), (3, 0.23, 2.5)),  # 0.03 off W1's face
    "P5": ("round", 0.1, (1, 0.18, 0.5), (3, 0.18, 0.5)),   # ~0.03 off W1's face (depends on the 16-gon's rotation)
    "P3": ("round", 0.1, (7, 2, 1), (9, 2, 1)),             # through column C1 (0.3)
    "D3": ("rect", (0.4, 0.4), (10, 0, 1), (10, 4, 1)),
    "P4": ("round", 0.1, (9.5, 2, 1), (10.5, 2, 1)),        # through duct D3 (0.4)
    "D4": ("rect", (0.2, 0.2), (12, 0, 1), (12, 4, 1)),
    "D5": ("rect", (0.2, 0.2), (12.24, 0, 1), (12.24, 4, 1)),  # 0.04 off D4
}
KIND = {"W1": "Wall", "W2": "Wall", "C1": "Column", "O1": "Opening", **{s: "Segment" for s in SEGS}}


def scene_ops():
    ops = [create("L0", "Level", elevation=0.0), create("L1", "Level", elevation=3.0),
           create("WT", "WallType", layer_thickness=[0.1, 0.1], layer_material=["brick", "block"],
                  layer_conductivity=[0.8, 0.5]),
           create("W1", "Wall", level=ref("L0"), x1=0.0, y1=0.0, x2=6.0, y2=0.0, height=3.0, type=ref("WT")),
           create("W2", "Wall", level=ref("L0"), x1=6.0, y1=0.0, x2=6.0, y2=4.0, height=3.0, type=ref("WT")),
           create("O1", "Opening", host=ref("W1"), kind="window", offset=4.6, sill=0.5, width=0.8, height=1.5),
           create("M1", "Material", name="concrete", E=30e9, G=12.5e9, density=2400.0, fy=30e6),
           create("C1", "Column", x=8.0, y=2.0, base=ref("L0"), top=ref("L1"), section_b=0.3, section_h=0.3,
                  material=ref("M1")),
           create("WS", "MepSystem", name="water", medium="water", density=998.2, viscosity=1.002e-3),
           create("AS", "MepSystem", name="air", medium="air", density=1.2, viscosity=1.81e-5)]
    for s, (shape, dims, a, b) in SEGS.items():
        sysid = "WS" if shape == "round" else "AS"
        ops.append(create(s + "a", "Port", system=ref(sysid), x=float(a[0]), y=float(a[1]), z=float(a[2]),
                          kind="source", pressure=2e5 if sysid == "WS" else 300.0))
        ops.append(create(s + "b", "Port", system=ref(sysid), x=float(b[0]), y=float(b[1]), z=float(b[2]),
                          kind="sink", flow=0.001 if sysid == "WS" else 0.05))
        props = {"from": ref(s + "a"), "to": ref(s + "b"), "shape": shape, "roughness": 1e-4}
        if shape == "round":
            props["diameter"] = dims
        else:
            props["width"], props["height"] = dims
        ops.append({"op": "create", "id": s, "kind": "Segment", "props": props})
    return ops


def world_meshes(d):
    f = os.path.join(TMP, "scene.gltf")
    run(["export", d, "--format", "gltf", "--out", f])
    raw = read_gltf(f)
    if "C1" not in raw:
        fail("glTF export has no mesh for column C1")
    want_lo, want_hi = np.array([7.85, 1.85, 0.0]), np.array([8.15, 2.15, 3.0])
    # spec/cli.md: glTF (x, y, z) = model (x, z, -y), so model = (gx, -gz, gy). Checked on the column's known box.
    t = np.array([[1, 0, 0], [0, 0, -1], [0, 1, 0]], float)
    v = raw["C1"].vertices @ t.T
    if not (np.allclose(v.min(0), want_lo, atol=1e-4) and np.allclose(v.max(0), want_hi, atol=1e-4)):
        fail(f"column C1 in glTF has bounds {raw['C1'].bounds.tolist()}; in the y-up frame of spec/cli.md "
             f"(glTF (x, y, z) = model (x, z, -y)) it must map to {want_lo.tolist()}..{want_hi.tolist()}")
    out = {}
    for k, m in raw.items():
        m2 = m.copy()
        m2.vertices = m.vertices @ t.T
        out[k] = m2
    return out


def check_segment_solids(meshes):
    """spec/schema.md: round = 16-gon prism with every vertex at radius D/2; rect = width horizontal and
    perpendicular to the axis, height vertical; both run from the `from` port to the `to` port."""
    for s, (shape, dims, a, b) in SEGS.items():
        if s not in meshes:
            fail(f"glTF export has no mesh for segment {s} (extras.id)")
        m = meshes[s]
        if not m.is_watertight:
            fail(f"segment {s} solid is not watertight")
        a, b = np.array(a, float), np.array(b, float)
        L = np.linalg.norm(b - a)
        ax = (b - a) / L
        rel = m.vertices - a
        t = rel @ ax
        radial = rel - np.outer(t, ax)
        if not (np.all((np.abs(t) < 1e-4) | (np.abs(t - L) < 1e-4))):
            fail(f"segment {s} solid does not run exactly between its port coordinates")
        if shape == "round":
            r = np.linalg.norm(radial, axis=1)
            for cap in (np.abs(t) < 1e-4, np.abs(t - L) < 1e-4):
                if cap.sum() != 16:
                    fail(f"round segment {s}: cap has {cap.sum()} vertices, expected a 16-gon")
            if r.max() - r.min() > 1e-5:
                fail(f"round segment {s}: 16-gon vertices are not on one circle")
            R = dims / 2
            if abs(r.mean() / R - 1) > 1e-4:
                fail(f"round segment {s}: 16-gon vertex radius {r.mean()}, spec/schema.md requires D/2 = {R}")
            close(abs(m.volume), P16 * R * R * L, 1e-4, f"segment {s} 16-gon prism volume")
        else:
            w, h = dims
            if len(m.vertices) != 8:
                fail(f"rect segment {s}: {len(m.vertices)} vertices, expected a box")
            close(abs(m.volume), w * h * L, 1e-4, f"segment {s} box volume")
            horiz = np.cross(ax, [0, 0, 1.0])
            horiz /= np.linalg.norm(horiz)
            close(np.ptp(radial @ horiz), w, 1e-4, f"rect segment {s} horizontal size (width)")
            close(np.ptp(m.vertices[:, 2]), h, 1e-4, f"rect segment {s} vertical size (height)")


def manifold_volume(ma, mb):
    import manifold3d as m3
    c = (ma.centroid + mb.centroid) / 2  # recentre near the origin

    def man(m):
        return m3.Manifold(m3.Mesh64(vert_properties=np.array(m.vertices - c, dtype=np.float64, order="C"),
                                     tri_verts=np.array(m.faces, dtype=np.uint64, order="C")))
    return (man(ma) ^ man(mb)).volume()


def mesh_distance(ma, mb):
    import trimesh

    def samples(m):
        pts = [m.vertices]
        for e in m.edges_unique:
            p, q = m.vertices[e[0]], m.vertices[e[1]]
            pts.append(p + np.linspace(0, 1, 41)[:, None] * (q - p))
        return np.vstack(pts)
    d1 = trimesh.proximity.closest_point(mb, samples(ma))[1].min()
    d2 = trimesh.proximity.closest_point(ma, samples(mb))[1].min()
    return float(min(d1, d2))


def clash_json(d, clearance, want_exit):
    doc = as_json(run(["clash", d, "--clearance", str(clearance), "--json"], want=want_exit), "clash --json")
    for k in ("hard", "soft", "matrix"):
        if k not in doc:
            fail(f"clash output lacks '{k}'")
    return doc


def pairs(lst, field, what):
    out = {}
    for c in lst:
        key = frozenset((c["a"], c["b"]))
        if len(key) != 2:
            fail(f"{what}: self-pair {c}")
        if {KIND.get(c["a"]), KIND.get(c["b"])} == {"Opening", "Segment"}:
            continue  # a pipe through the opening's void: opening solids are not part of the contract
        if key in out:
            fail(f"{what}: pair {sorted(key)} reported twice")
        out[key] = c[field]
    return out


def clash():
    d = project("clash", scene_ops())
    meshes = world_meshes(d)
    check_segment_solids(meshes)
    a16 = P16 * (SEGS["P1"][1] / 2) ** 2  # 16-gon with vertices at radius D/2 (schema.md)

    doc = clash_json(d, 0.05, 1)
    hard = pairs(doc["hard"], "volume", "hard clashes")
    want_hard = {("P1", "W1"): a16 * 0.2, ("D1", "W1"): 0.4 * 0.3 * 0.2, ("P3", "C1"): a16 * 0.3,
                 ("P4", "D3"): a16 * 0.4}
    if set(hard) != {frozenset(k) for k in want_hard}:
        fail(f"hard clashes {sorted(sorted(k) for k in hard)}, expected {sorted(sorted(k) for k in want_hard)} "
             "(joined walls, hosted openings and a pipe through an opening are not clashes)")
    for (a, b), v in want_hard.items():
        close(hard[frozenset((a, b))], v, 1e-6, f"hard clash {a}/{b} volume (closed form)")
        close(manifold_volume(meshes[a], meshes[b]), v, 1e-4, f"oracle self-check: manifold3d {a}/{b} volume")
        close(hard[frozenset((a, b))], manifold_volume(meshes[a], meshes[b]), 1e-4,
              f"hard clash {a}/{b} volume vs manifold3d on the exported solids")

    soft = pairs(doc["soft"], "distance", "soft clashes")
    d_p5 = mesh_distance(meshes["P5"], meshes["W1"])
    if not 0.025 < d_p5 < 0.035:
        fail(f"P5 is {d_p5} from W1 in the exported solids (expected ~0.03)")
    want_soft = {("D2", "W1"): 0.03, ("D4", "D5"): 0.04, ("P5", "W1"): d_p5}
    if set(soft) != {frozenset(k) for k in want_soft}:
        fail(f"soft clashes {sorted(sorted(k) for k in soft)}, expected {sorted(sorted(k) for k in want_soft)}")
    for (a, b), v in want_soft.items():
        tm = mesh_distance(meshes[a], meshes[b])
        close(tm, v, 0, f"oracle self-check: trimesh distance {a}/{b}", abs_tol=1e-5)
        close(soft[frozenset((a, b))], tm, 0, f"soft clash {a}/{b} distance vs trimesh proximity", abs_tol=1e-5)
    close(soft[frozenset(("D2", "W1"))], 0.03, 1e-6, "soft clash D2/W1 distance (closed form)")
    close(soft[frozenset(("D4", "D5"))], 0.04, 1e-6, "soft clash D4/D5 distance (closed form)")

    # matrix: hard clashes per kind pair (either orientation of the pair is accepted)
    mat = doc["matrix"]

    def count(ka, kb):
        get = lambda x, y: (mat.get(x) or {}).get(y, 0)
        return get(ka, kb) if ka == kb else get(ka, kb) + get(kb, ka)
    for (ka, kb), n in {("Segment", "Wall"): 2, ("Column", "Segment"): 1, ("Segment", "Segment"): 1,
                        ("Wall", "Wall"): 0, ("Opening", "Wall"): 0, ("Column", "Wall"): 0}.items():
        if count(ka, kb) != n:
            fail(f"clash matrix {ka}/{kb} = {count(ka, kb)}, expected {n}: {mat}")

    # a tighter clearance drops the 0.04 pair and keeps the ~0.03 ones
    soft2 = pairs(clash_json(d, 0.035, 1)["soft"], "distance", "soft clashes (clearance 0.035)")
    if set(soft2) != {frozenset(("D2", "W1")), frozenset(("P5", "W1"))}:
        fail(f"clearance 0.035: soft clashes {sorted(sorted(k) for k in soft2)}")

    # remove the clashing segments: no hard clash -> exit 0 even with soft clashes present
    apply(d, [{"op": "delete", "id": s} for s in ("P1", "D1", "P3", "P4")], "clash-fix")
    doc3 = clash_json(d, 0.05, 0)
    if pairs(doc3["hard"], "volume", "hard clashes after fix"):
        fail(f"hard clashes remain after deleting the clashing segments: {doc3['hard']}")
    if set(pairs(doc3["soft"], "distance", "soft after fix")) != {frozenset(k) for k in want_soft}:
        fail(f"soft clashes after fix: {doc3['soft']}")


if __name__ == "__main__":
    try:
        sizing()
        circuits()
        clash()
    except (KeyError, TypeError, ValueError, IndexError, AttributeError) as e:
        fail(f"malformed bim output or missing entity: {type(e).__name__}: {e}")
    print("T4.7 judge: all checks passed")
