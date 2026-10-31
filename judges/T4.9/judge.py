"""External judge for T4.9 (product catalogues, Product/Furniture/Roof, effective props, finish take-off).

Drives the real `bim` binary (argv[1]) in a scratch dir (argv[2]) through spec/catalogs.md, spec/schema.md and
spec/cli.md:
  * catalogue validation: malformed JSON, a missing required field per category, a duplicate sku and a directory
    name that differs from `catalog` make `bim catalog list` and `bim catalog use` exit 2 naming the file;
  * `bim catalog list --json` content and sorting; `bim catalog use` creates the exact `Product`, prints its id, is
    idempotent (`bim hash` unchanged) and, after the catalogue file changes, updates the Product (removed props
    removed); engines read the Product entity, never the catalogue file;
  * effective props: Openings without width/height take the product's (own value wins) in the wall's cut
    (`solve --trade arch` walls[] and the glTF wall solid); Furniture takes the product's `size` (own wins);
  * Furniture and Roof solids in the glTF export (closed meshes equal to reference solids built here with
    manifold3d from spec/schema.md, read back in the y-up frame glTF (x, y, z) = model (x, z, -y)) and in
    `bim clash` (closed-form intersection volumes, cross-checked with manifold3d on the exported meshes);
  * `solve --trade arch` `products` (count / area rules of spec/catalogs.md Take-off) and `roofs`
    (volume = plan area x thickness, sloped area = plan area x sqrt(1 + slope^2)).
Usage: .oracles/py/bin/python -I -B judges/T4.9/judge.py <bim> <scratch-dir>
"""
import base64
import copy
import json
import math
import os
import subprocess
import sys

import manifold3d as mf
import numpy as np
import trimesh

BIM, TMP = sys.argv[1], sys.argv[2]
HERE = os.path.dirname(os.path.abspath(__file__))
REL = 1e-6         # f64 quantities in JSON (areas, volumes) against closed forms
REL_MESH = 1e-4    # glTF positions are float32


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def run(args, want=0):
    try:
        p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        fail(f"bim {' '.join(args)} timed out (60 s)")
    if p.returncode != want:
        fail(f"bim {' '.join(args)} exited {p.returncode}, want {want}: {p.stderr.strip()[-400:]}")
    return p


def as_json(text, what):
    try:
        return json.loads(text)
    except json.JSONDecodeError as e:
        fail(f"{what} did not print one JSON document: {e}: {text[:200]!r}")


def close(a, b, rel=REL, absol=1e-9):
    if isinstance(a, bool) or not isinstance(a, (int, float)):
        return False
    return abs(a - b) <= max(rel * abs(b), absol)


def need(cond, msg):
    if not cond:
        fail(msg)


# ---------------------------------------------------------------- catalogues (fixture data)

ACME = {"catalog": "acme", "version": "2026.1", "manufacturer": "Acme Building Products Ltd", "products": [
    # written out of sku order on purpose
    {"sku": "SOFA-3", "category": "furniture", "name": "Three-seat sofa",
     "props": {"size": [2.1, 0.9, 0.85], "colour": "slate"}},
    {"sku": "FD30-926", "category": "door", "name": "Oak fire door 926 x 2040",
     "props": {"width": 0.926, "height": 2.04, "fire_rating": "FD30", "u_value": 1.8, "self_closing": True}},
    {"sku": "CW-1210", "category": "window", "name": "Casement window 1210 x 1050",
     "props": {"width": 1.21, "height": 1.05, "u_value": 1.2, "glazing": "double"}},
    {"sku": "SL-NAT", "category": "roof_covering", "name": "Natural slate", "props": {"mass_per_area": 25}},
    {"sku": "EM-WHT", "category": "wall_finish", "name": "Matt emulsion, white", "props": {"colour": "white", "coats": 2}},
    {"sku": "OAK-14", "category": "floor_finish", "name": "Engineered oak 14 mm", "props": {"thickness": 0.014}},
    {"sku": "CL-PLN", "category": "ceiling_finish", "name": "Plain plaster ceiling", "manufacturer": "Plasterco",
     "props": {"colour": "white"}},
    {"sku": "WC-01", "category": "sanitary", "name": "Wall-hung WC", "props": {"flush_volume": 4.5}},
]}
ZETA = {"catalog": "zeta", "version": "7", "manufacturer": "Zeta Interiors", "products": [
    {"sku": "TBL-2", "category": "furniture", "name": "Dining table", "props": {"size": [1.6, 0.8, 0.75]}},
    {"sku": "ARM-1", "category": "furniture", "name": "Armchair", "props": {"size": [0.8, 0.85, 0.9], "colour": "red"}},
]}


def acme_v2():
    """The catalogue after the manufacturer's update: new version, changed, removed and added props."""
    c = copy.deepcopy(ACME)
    c["version"] = "2026.2"
    p = {q["sku"]: q for q in c["products"]}
    p["FD30-926"]["props"] = {"width": 1.026, "height": 2.1, "fire_rating": "FD30", "self_closing": True,
                              "acoustic_db": 32}            # u_value removed, acoustic_db added
    p["CW-1210"]["props"]["height"] = 1.1
    p["SOFA-3"]["props"]["size"] = [2.2, 0.95, 0.8]
    return c


def write_catalog(proj, dirname, content):
    d = os.path.join(proj, "catalogs", dirname)
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "catalog.json"), "w") as fh:
        if isinstance(content, str):
            fh.write(content)
        else:
            json.dump(content, fh, indent=1)


def new_project(name):
    d = os.path.join(TMP, name)
    run(["init", d])
    return d


def apply(d, ops, name):
    f = os.path.join(TMP, name + ".ops.json")
    with open(f, "w") as fh:
        json.dump({"schema": 1, "ops": ops}, fh)
    run(["apply", d, f])


def bim_hash(d):
    h = run(["hash", d]).stdout.strip()
    need(len(h) == 16 and all(c in "0123456789abcdef" for c in h), f"bim hash printed {h!r}")
    return h


def use(d, pid):
    out = run(["catalog", "use", d, pid]).stdout
    lines = [l.strip() for l in out.splitlines() if l.strip()]
    need(lines and lines[-1] == pid, f"bim catalog use {pid} printed {out.strip()!r}, want the Product id {pid!r} (last line)")


def product_props(cat, sku):
    p = [q for q in cat["products"] if q["sku"] == sku][0]
    want = {"catalog": cat["catalog"], "catalog_version": cat["version"], "sku": sku, "category": p["category"],
            "name": p["name"], "manufacturer": p.get("manufacturer", cat["manufacturer"])}
    want.update(p["props"])
    return want


def same_value(got, want):
    if isinstance(want, bool):
        return got is want
    if isinstance(want, (int, float)):
        return isinstance(got, (int, float)) and not isinstance(got, bool) and got == want
    if isinstance(want, list):
        return isinstance(got, list) and len(got) == len(want) and all(same_value(g, w) for g, w in zip(got, want))
    return got == want


def check_product(d, cat, sku):
    pid = f"{cat['catalog']}:{sku}"
    doc = as_json(run(["show", d, pid, "--json"]).stdout, f"bim show {pid}")
    need(doc.get("id") == pid, f"bim show {pid}: id {doc.get('id')!r}")
    need(doc.get("kind") == "Product", f"{pid}: kind {doc.get('kind')!r}, want Product")
    props = {k: v for k, v in (doc.get("props") or {}).items() if not k.startswith("_")}
    want = product_props(cat, sku)
    need(set(props) == set(want), f"{pid}: props {sorted(props)}, want exactly {sorted(want)}")
    for k, v in want.items():
        need(same_value(props[k], v), f"{pid}: prop {k} = {props[k]!r}, want {v!r}")


# ---------------------------------------------------------------- 1. catalogue validation

def broken_cases():
    door = {"sku": "D1", "category": "door", "name": "Door", "props": {"width": 0.9, "height": 2.1}}
    ok = {"sku": "OK-1", "category": "material", "name": "Brick", "props": {"density": 1800}}

    def cat(*products):
        return {"catalog": "broken", "version": "1", "manufacturer": "B", "products": [ok, *products]}

    def without(p, key, where=None):
        q = copy.deepcopy(p)
        (q["props"] if where == "props" else q).pop(key)
        return q
    win = {"sku": "W1", "category": "window", "name": "Window", "props": {"width": 1.0, "height": 1.2}}
    sofa = {"sku": "F1", "category": "furniture", "name": "Sofa", "props": {"size": [2.0, 0.9, 0.8]}}
    return [  # (label, dir name, content, sku used with `catalog use`)
        ("malformed JSON", "broken", '{"catalog": "broken", "version": "1", "products": [{"sku": "D1",', "D1"),
        ("product without sku", "broken", cat(without(door, "sku")), "OK-1"),
        ("product without category", "broken", cat(without(door, "category")), "D1"),
        ("product without name", "broken", cat(without(door, "name")), "D1"),
        ("door without height", "broken", cat(without(door, "height", "props")), "D1"),
        ("door without width", "broken", cat(without(door, "width", "props")), "D1"),
        ("window without width", "broken", cat(without(win, "width", "props")), "W1"),
        ("window without height", "broken", cat(without(win, "height", "props")), "W1"),
        ("furniture without size", "broken", cat(without(sofa, "size", "props")), "F1"),
        ("duplicate sku", "broken", cat(door, dict(win, sku="D1")), "D1"),
        ("directory name differs from catalog", "broken", dict(cat(door), catalog="other"), "D1"),
    ]


def validation():
    base = new_project("valid")
    write_catalog(base, "zeta", ZETA)
    write_catalog(base, "acme", ACME)
    got = as_json(run(["catalog", "list", base, "--json"]).stdout, "catalog list --json")
    cats = got.get("catalogs")
    need(isinstance(cats, list), f"catalog list --json has no catalogs[] array: {str(got)[:200]}")
    need([c.get("catalog") for c in cats] == ["acme", "zeta"],
         f"catalog list: catalogs {[c.get('catalog') for c in cats]}, want ['acme', 'zeta'] (sorted by name)")
    for c, want in zip(cats, (ACME, ZETA)):
        for k in ("version", "manufacturer"):
            need(c.get(k) == want[k], f"catalog list {want['catalog']}: {k} {c.get(k)!r}, want {want[k]!r}")
        exp = sorted(({"sku": p["sku"], "category": p["category"], "name": p["name"]} for p in want["products"]),
                     key=lambda p: p["sku"])
        have = [{k: p.get(k) for k in ("sku", "category", "name")} for p in c.get("products") or []]
        need(have == exp, f"catalog list {want['catalog']}: products {have}, want {exp} (sorted by sku)")

    for i, (label, dirname, content, sku) in enumerate(broken_cases()):
        d = new_project(f"broken{i}")
        write_catalog(d, "acme", ACME)
        write_catalog(d, dirname, content)
        h0 = bim_hash(d)
        p = run(["catalog", "list", d, "--json"], want=2)
        need("catalog.json" in p.stderr and dirname in p.stderr,
             f"{label}: catalog list exit 2 must name the file catalogs/{dirname}/catalog.json; stderr: {p.stderr[-300:]!r}")
        p = run(["catalog", "use", d, f"{dirname}:{sku}"], want=2)
        need("catalog.json" in p.stderr and dirname in p.stderr,
             f"{label}: catalog use exit 2 must name the file catalogs/{dirname}/catalog.json; stderr: {p.stderr[-300:]!r}")
        need(bim_hash(d) == h0, f"{label}: a failed catalog use changed the document")


# ---------------------------------------------------------------- geometry references (manifold3d)

def box(x0, x1, y0, y1, z0, z1):
    return mf.Manifold.cube([x1 - x0, y1 - y0, z1 - z0]).translate([x0, y0, z0])


def shoelace(pts):
    return 0.5 * sum(pts[i][0] * pts[(i + 1) % len(pts)][1] - pts[(i + 1) % len(pts)][0] * pts[i][1]
                     for i in range(len(pts)))


def wall_ref(w, elev, openings):
    """Isolated wall: box along the centreline, openings cut through the full thickness (sill from the base)."""
    (x1, y1), (x2, y2) = w["a"], w["b"]
    L = math.hypot(x2 - x1, y2 - y1)
    t, b, H = w["t"], w.get("base_offset", 0.0), w["height"]
    s = box(0, L, -t / 2, t / 2, b, b + H)
    for o in openings:
        z0 = b + o["sill"]
        s = s - box(o["offset"], o["offset"] + o["w"], -t, t, z0 - (1.0 if o["sill"] == 0 else 0.0), z0 + o["h"])
    s = s.rotate([0, 0, math.degrees(math.atan2(y2 - y1, x2 - x1))]).translate([x1, y1, elev])
    vol = t * (L * H - sum(o["w"] * o["h"] for o in openings))
    return s, vol, L * H - sum(o["w"] * o["h"] for o in openings)


def furniture_ref(f, elev, size):
    w, dpt, h = size
    s = box(-w / 2, w / 2, -dpt / 2, dpt / 2, 0, h).rotate([0, 0, math.degrees(f["angle"])])
    return s.translate([f["x"], f["y"], elev + f.get("z_offset", 0.0)]), w * dpt * h


def roof_ref(r, elev):
    """Footprint prism intersected with the slab between the underside plane and the plane `thickness` above."""
    pts = r["boundary"]
    A = shoelace(pts)
    if A <= 0:
        raise ValueError("fixture roof boundary must be counter-clockwise")
    x0, y0 = pts[0]
    s, a, t, zb = r["slope"], r["angle"], r["thickness"], elev + r.get("base", 0.0)
    M = 100.0
    prism = mf.Manifold.extrude(mf.CrossSection([pts]), 2 * M).translate([0, 0, -M])
    cx, cy = sum(p[0] for p in pts) / len(pts), sum(p[1] for p in pts) / len(pts)
    slab = box(cx - 50, cx + 50, cy - 50, cy + 50, 0, t)
    shear = np.array([[1, 0, 0, 0], [0, 1, 0, 0],
                      [s * math.cos(a), s * math.sin(a), 1, zb - s * (x0 * math.cos(a) + y0 * math.sin(a))]], dtype=float)
    return prism ^ slab.transform(shear), A * t, A, A * math.sqrt(1 + s * s)


# ---------------------------------------------------------------- glTF decoding (independent of trimesh's loader)

COMP = {5121: np.uint8, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}
NCOMP = {"SCALAR": 1, "VEC3": 3}
# spec/cli.md: glTF (x, y, z) = model (x, z, -y); vertices @ Y_UP recovers model coordinates.
Y_UP = np.array([[1, 0, 0], [0, 0, 1], [0, -1, 0]], dtype=float)


def node_matrix(n):
    if "matrix" in n:
        return np.array(n["matrix"], dtype=float).reshape(4, 4).T
    T = trimesh.transformations.translation_matrix(n.get("translation", [0, 0, 0]))
    x, y, z, w = n.get("rotation", [0, 0, 0, 1])
    R = trimesh.transformations.quaternion_matrix([w, x, y, z])
    S = np.diag(list(n.get("scale", [1, 1, 1])) + [1])
    return T @ R @ S


def load_gltf(path):
    """{extras.id: trimesh in MODEL coordinates (z up)}."""
    g = json.load(open(path))
    bufs = []
    for b in g.get("buffers", []):
        uri = b["uri"]
        if uri.startswith("data:"):
            bufs.append(base64.b64decode(uri.split(",", 1)[1]))
        else:
            bufs.append(open(os.path.join(os.path.dirname(path), uri), "rb").read())

    def accessor(i):
        a = g["accessors"][i]
        v = g["bufferViews"][a["bufferView"]]
        dt, nc = COMP[a["componentType"]], NCOMP[a["type"]]
        start = v.get("byteOffset", 0) + a.get("byteOffset", 0)
        item = np.dtype(dt).itemsize * nc
        stride = v.get("byteStride", 0) or item
        raw = bufs[v["buffer"]]
        if stride == item:
            return np.frombuffer(raw, dtype=dt, count=a["count"] * nc, offset=start).reshape(-1, nc)
        out = np.empty((a["count"], nc), dtype=dt)
        for k in range(a["count"]):
            out[k] = np.frombuffer(raw, dtype=dt, count=nc, offset=start + k * stride)
        return out

    parent = {}
    for i, n in enumerate(g.get("nodes", [])):
        for c in n.get("children", []):
            parent[c] = i

    def world(i):
        M = node_matrix(g["nodes"][i])
        while i in parent:
            i = parent[i]
            M = node_matrix(g["nodes"][i]) @ M
        return M

    meshes = {}
    for i, n in enumerate(g.get("nodes", [])):
        eid = (n.get("extras") or {}).get("id")
        if eid is None or "mesh" not in n:
            continue
        verts, faces, base = [], [], 0
        for prim in g["meshes"][n["mesh"]]["primitives"]:
            if prim.get("mode", 4) != 4:
                continue
            p = accessor(prim["attributes"]["POSITION"]).astype(float)
            idx = accessor(prim["indices"]).reshape(-1) if "indices" in prim else np.arange(len(p))
            verts.append(p)
            faces.append(idx.reshape(-1, 3).astype(np.int64) + base)
            base += len(p)
        if verts:
            v = np.vstack(verts)
            v = (np.c_[v, np.ones(len(v))] @ world(i).T)[:, :3] @ Y_UP
            m = trimesh.Trimesh(v, np.vstack(faces), process=False)
            m.merge_vertices()
            meshes[eid] = m
    return meshes


def as_manifold(eid, mesh):
    try:
        got = mf.Manifold(mf.Mesh(vert_properties=np.asarray(mesh.vertices, dtype=np.float32),
                                  tri_verts=np.asarray(mesh.faces, dtype=np.uint32)))
    except Exception as e:  # noqa: BLE001 - any rejection by manifold3d is a failed mesh
        fail(f"{eid}: glTF mesh is not a valid manifold solid ({e})")
    if got.is_empty() or got.volume() <= 0:
        fail(f"{eid}: glTF mesh is not a valid, outward-oriented manifold solid")
    return got


def check_solid(meshes, eid, ref, ref_vol, what):
    if not close(ref.volume(), ref_vol, 1e-7):
        fail(f"oracle disagreement for {eid}: manifold3d {ref.volume()} vs closed form {ref_vol}")
    mesh = meshes.get(eid)
    need(mesh is not None, f"glTF has no mesh node with extras.id {eid!r}")
    need(mesh.is_watertight, f"{eid}: glTF mesh is not closed (watertight)")
    need(close(abs(mesh.volume), ref_vol, REL_MESH), f"{eid}: glTF solid volume {abs(mesh.volume)}, expected {ref_vol} ({what})")
    got = as_manifold(eid, mesh)
    xor = (got - ref).volume() + (ref - got).volume()
    need(xor <= REL_MESH * ref_vol, f"{eid}: glTF solid differs from the reference ({what}) by volume {xor} "
                                    "(model frame read back as glTF (x, y, z) = model (x, z, -y))")


def export_gltf(d, name):
    path = os.path.join(TMP, name + ".gltf")
    run(["export", d, "--format", "gltf", "--out", path])
    need(os.path.getsize(path) > 0, f"export wrote an empty {name}.gltf")
    try:
        p = subprocess.run(["node", os.path.join(HERE, "validate_gltf.mjs"), path], capture_output=True, text=True,
                           timeout=120)
    except subprocess.TimeoutExpired:
        fail("glTF-Validator timed out")
    need(p.returncode == 0, f"{name}.gltf: {p.stdout.strip()} {p.stderr.strip()[-300:]}")
    return load_gltf(path)


# ---------------------------------------------------------------- 2. main model

T = 0.2   # single-layer wall type thickness
LEV = {"L0": 0.0, "L1": 3.0}
SLAB = [(0, 0), (8, 0), (8, 6), (0, 6)]
SLAB_HOLE = [(2, 2), (3, 2), (3, 3.5), (2, 3.5)]
ROOFS = {
    "R1": dict(level="L1", boundary=[(10, 0), (16, 0), (17, 3), (13, 5), (10, 3)], slope=0.35, angle=0.6,
               thickness=0.22, base=0.4, covering="acme:SL-NAT"),
    "R2": dict(level="L1", boundary=[(20, 0), (24, 0), (22, 3)], slope=0.0, angle=0.0, thickness=0.15,
               covering="acme:SL-NAT"),
    "R3": dict(level="L0", boundary=[(30, 0), (33, 0), (33, 2), (30, 2)], slope=1.0, angle=math.pi, thickness=0.1),
}
FURN = {
    "F1": dict(level="L1", x=5.0, y=10.0, angle=math.pi / 6, product="acme:SOFA-3", z_offset=0.4),
    "F2": dict(level="L0", x=10.0, y=10.0, angle=0.0, product="acme:SOFA-3", size=[1.0, 0.5, 0.4]),
    "F3": dict(level="L0", x=14.0, y=10.0, angle=-1.0, product="zeta:TBL-2"),
}


def R(i):
    return {"ref": i}


def main_ops():
    ops = [{"op": "create", "id": k, "kind": "Level", "props": {"elevation": v}} for k, v in LEV.items()]
    ops.append({"op": "create", "id": "T20", "kind": "WallType",
                "props": {"layer_thickness": [T], "layer_material": ["Block"], "layer_conductivity": [1.0]}})
    ops.append({"op": "create", "id": "W1", "kind": "Wall", "props": {
        "level": R("L0"), "x1": 0.0, "y1": 0.0, "x2": 6.0, "y2": 0.0, "height": 3.0, "type": R("T20"),
        "finish_left": R("acme:EM-WHT"), "finish_right": R("acme:EM-WHT")}})
    ops.append({"op": "create", "id": "W2", "kind": "Wall", "props": {
        "level": R("L0"), "x1": 0.0, "y1": 4.0, "x2": 5.0, "y2": 7.0, "height": 2.7, "type": R("T20"),
        "base_offset": 0.1, "finish_left": R("acme:EM-WHT")}})
    # O1: width and height from the door product; O2: own width, height from the product; O3: both own.
    ops.append({"op": "create", "id": "O1", "kind": "Opening", "props": {
        "host": R("W1"), "kind": "door", "offset": 0.5, "sill": 0.0, "product": R("acme:FD30-926")}})
    ops.append({"op": "create", "id": "O2", "kind": "Opening", "props": {
        "host": R("W1"), "kind": "window", "offset": 3.0, "sill": 0.9, "width": 0.8, "product": R("acme:CW-1210")}})
    ops.append({"op": "create", "id": "O3", "kind": "Opening", "props": {
        "host": R("W2"), "kind": "window", "offset": 1.0, "sill": 0.8, "width": 1.0, "height": 1.2,
        "product": R("acme:CW-1210")}})
    ops.append({"op": "create", "id": "S1", "kind": "Slab", "props": {
        "level": R("L1"), "boundary": [c for p in SLAB for c in p], "thickness": 0.2,
        "finish_top": R("acme:OAK-14"), "finish_bottom": R("acme:CL-PLN")}})
    ops.append({"op": "create", "id": "SO1", "kind": "SlabOpening", "props": {
        "host": R("S1"), "boundary": [c for p in SLAB_HOLE for c in p]}})
    for k, r in ROOFS.items():
        props = {"level": R(r["level"]), "boundary": [c for p in r["boundary"] for c in p], "slope": r["slope"],
                 "angle": r["angle"], "thickness": r["thickness"]}
        if "base" in r:
            props["base"] = r["base"]
        if "covering" in r:
            props["covering"] = R(r["covering"])
        ops.append({"op": "create", "id": k, "kind": "Roof", "props": props})
    for k, f in FURN.items():
        props = {"level": R(f["level"]), "x": f["x"], "y": f["y"], "angle": f["angle"], "product": R(f["product"])}
        for opt in ("z_offset", "size"):
            if opt in f:
                props[opt] = f[opt]
        ops.append({"op": "create", "id": k, "kind": "Furniture", "props": props})
    return ops


def expected(acme, zeta):
    """Closed-form expectations for the main model under the given catalogue versions."""
    prod = {f"acme:{p['sku']}": p["props"] for p in acme["products"]}
    prod.update({f"zeta:{p['sku']}": p["props"] for p in zeta["products"]})
    door, win = prod["acme:FD30-926"], prod["acme:CW-1210"]
    w1 = dict(a=(0.0, 0.0), b=(6.0, 0.0), t=T, height=3.0)
    w1_open = [dict(offset=0.5, sill=0.0, w=door["width"], h=door["height"]),
               dict(offset=3.0, sill=0.9, w=0.8, h=win["height"])]
    w2 = dict(a=(0.0, 4.0), b=(5.0, 7.0), t=T, height=2.7, base_offset=0.1)
    w2_open = [dict(offset=1.0, sill=0.8, w=1.0, h=1.2)]
    walls = {"W1": wall_ref(w1, 0.0, w1_open), "W2": wall_ref(w2, 0.0, w2_open)}
    furn = {k: furniture_ref(f, LEV[f["level"]], f.get("size") or prod[f["product"]]["size"]) for k, f in FURN.items()}
    roofs = {k: roof_ref(r, LEV[r["level"]]) for k, r in ROOFS.items()}
    net = shoelace(SLAB) - shoelace(SLAB_HOLE)
    sa = {k: v[2] for k, v in walls.items()}
    sloped = {k: v[3] for k, v in roofs.items()}
    products = {  # id -> (category, count, area)
        "acme:CL-PLN": ("ceiling_finish", 1, net),
        "acme:CW-1210": ("window", 2, 0.0),
        "acme:EM-WHT": ("wall_finish", 3, 2 * sa["W1"] + sa["W2"]),
        "acme:FD30-926": ("door", 1, 0.0),
        "acme:OAK-14": ("floor_finish", 1, net),
        "acme:SL-NAT": ("roof_covering", 2, sloped["R1"] + sloped["R2"]),
        "acme:SOFA-3": ("furniture", 2, 0.0),
        "zeta:TBL-2": ("furniture", 1, 0.0),
    }
    return dict(walls=walls, furn=furn, roofs=roofs, net=net, products=products)


def check_solve(d, exp, tag):
    out = as_json(run(["solve", d, "--trade", "arch", "--json"]).stdout, f"solve --trade arch ({tag})")
    walls = {w.get("id"): w for w in out.get("walls") or []}
    for k, (_, vol, sa) in exp["walls"].items():
        w = walls.get(k)
        need(w is not None, f"({tag}) walls[] has no entry for {k}")
        need(close(w.get("volume"), vol), f"({tag}) {k}: volume {w.get('volume')}, expected {vol} "
                                          "(opening cut with effective width/height; finishes have no thickness)")
        need(close(w.get("side_area"), sa), f"({tag}) {k}: side_area {w.get('side_area')}, expected {sa}")
        lay = w.get("layers") or []
        need(len(lay) == 1 and close(lay[0].get("volume"), vol), f"({tag}) {k}: layers {lay}, expected one of volume {vol}")
    slabs = {s.get("id"): s for s in out.get("slabs") or []}
    s1 = slabs.get("S1") or {}
    need(close(s1.get("area"), exp["net"]) and close(s1.get("volume"), exp["net"] * 0.2),
         f"({tag}) S1: {s1}, expected area {exp['net']} volume {exp['net'] * 0.2} (finishes have no thickness)")

    prods = out.get("products")
    need(isinstance(prods, list), f"({tag}) solve output has no products[] although the model references products")
    ids = [p.get("id") for p in prods]
    need(ids == sorted(exp["products"]), f"({tag}) products ids {ids}, expected {sorted(exp['products'])} "
         "(one per referenced Product, sorted by id; unreferenced acme:WC-01 absent)")
    for p in prods:
        cat, cnt, area = exp["products"][p["id"]]
        need(p.get("category") == cat, f"({tag}) products {p['id']}: category {p.get('category')!r}, expected {cat!r}")
        need(p.get("count") == cnt, f"({tag}) products {p['id']}: count {p.get('count')}, expected {cnt}")
        need(close(p.get("area"), area), f"({tag}) products {p['id']}: area {p.get('area')}, expected {area}")

    roofs = out.get("roofs")
    need(isinstance(roofs, list), f"({tag}) solve output has no roofs[] although the model has Roofs")
    need([r.get("id") for r in roofs] == sorted(ROOFS), f"({tag}) roofs ids {[r.get('id') for r in roofs]}, expected {sorted(ROOFS)}")
    for r in roofs:
        _, vol, _, sloped = exp["roofs"][r["id"]]
        need(close(r.get("volume"), vol), f"({tag}) roof {r['id']}: volume {r.get('volume')}, expected plan area x thickness = {vol}")
        need(close(r.get("area"), sloped), f"({tag}) roof {r['id']}: area {r.get('area')}, expected sloped area {sloped}")
    return out


def check_gltf(d, exp, tag):
    meshes = export_gltf(d, f"main-{tag}")
    for k, (ref, vol, _) in exp["walls"].items():
        check_solid(meshes, k, ref, vol, f"{tag}: wall with openings cut at their effective width/height")
    for k, (ref, vol) in exp["furn"].items():
        check_solid(meshes, k, ref, vol, f"{tag}: Furniture cuboid of effective size, rotated by angle, base at level + z_offset")
    for k, (ref, vol, _, _) in exp["roofs"].items():
        check_solid(meshes, k, ref, vol, f"{tag}: Roof between the sloped underside and thickness above it")


def main_model():
    d = new_project("main")
    write_catalog(d, "acme", ACME)
    write_catalog(d, "zeta", ZETA)
    h0 = bim_hash(d)
    use(d, "acme:FD30-926")
    h1 = bim_hash(d)
    need(h1 != h0, "bim catalog use did not change the document")
    use(d, "acme:FD30-926")
    need(bim_hash(d) == h1, "a second bim catalog use with an unchanged catalogue changed bim hash")
    for sku in ("CW-1210", "SOFA-3", "SL-NAT", "EM-WHT", "OAK-14", "CL-PLN", "WC-01"):
        use(d, f"acme:{sku}")
    use(d, "zeta:TBL-2")
    for p in ACME["products"]:
        check_product(d, ACME, p["sku"])
    check_product(d, ZETA, "TBL-2")
    h = bim_hash(d)
    p = run(["catalog", "use", d, "acme:NO-SUCH"], want=2)
    need(bim_hash(d) == h, "catalog use of an unknown sku changed the document")

    apply(d, main_ops(), "main")
    exp1 = expected(ACME, ZETA)
    check_solve(d, exp1, "v1")
    check_gltf(d, exp1, "v1")
    h = bim_hash(d)
    for sku in ("FD30-926", "CW-1210", "SOFA-3"):
        use(d, f"acme:{sku}")
    need(bim_hash(d) == h, "re-running bim catalog use with an unchanged catalogue changed bim hash")

    # The manufacturer updates the catalogue. Engines read Product entities, never the file, so nothing
    # changes until `catalog use` runs again.
    v2 = acme_v2()
    write_catalog(d, "acme", v2)
    check_solve(d, exp1, "catalogue edited, not re-used")
    need(bim_hash(d) == h, "editing a catalogue file alone changed bim hash")
    for sku in ("FD30-926", "CW-1210", "SOFA-3"):
        use(d, f"acme:{sku}")
        check_product(d, v2, sku)   # FD30-926: u_value removed, acoustic_db added, catalog_version 2026.2
    need(bim_hash(d) != h, "catalog use after a catalogue change did not change the document")
    h2 = bim_hash(d)
    use(d, "acme:FD30-926")
    need(bim_hash(d) == h2, "catalog use is not idempotent after a catalogue change")
    exp2 = expected(v2, ZETA)
    check_solve(d, exp2, "v2")
    check_gltf(d, exp2, "v2")


# ---------------------------------------------------------------- 3. clash

def clash_model():
    d = new_project("clash")
    write_catalog(d, "acme", ACME)
    use(d, "acme:SOFA-3")
    ops = [{"op": "create", "id": "G", "kind": "Level", "props": {"elevation": 0.0}},
           {"op": "create", "id": "T20", "kind": "WallType",
            "props": {"layer_thickness": [T], "layer_material": ["Block"], "layer_conductivity": [1.0]}}]

    def wall(i, a, b, h):
        ops.append({"op": "create", "id": i, "kind": "Wall", "props": {
            "level": R("G"), "x1": a[0], "y1": a[1], "x2": b[0], "y2": b[1], "height": h, "type": R("T20")}})
    wall("CW1", (20, 0), (26, 0), 3.0)
    wall("CW2", (28, 0.5), (32, 0.5), 1.0)
    wall("CW3", (41, 0), (45, 0), 3.0)
    ops.append({"op": "create", "id": "CF1", "kind": "Furniture", "props": {
        "level": R("G"), "x": 23.0, "y": 0.3, "angle": 0.0, "product": R("acme:SOFA-3")}})
    ops.append({"op": "create", "id": "CF2", "kind": "Furniture", "props": {
        "level": R("G"), "x": 30.0, "y": 0.0, "angle": math.pi / 2, "product": R("acme:SOFA-3"), "z_offset": 0.3}})
    roof = dict(level="G", boundary=[(40, -2), (46, -2), (46, 2), (40, 2)], slope=0.2, angle=0.0, thickness=0.2, base=2.5)
    ops.append({"op": "create", "id": "CR1", "kind": "Roof", "props": {
        "level": R("G"), "boundary": [c for p in roof["boundary"] for c in p], "slope": 0.2, "angle": 0.0,
        "thickness": 0.2, "base": 2.5}})
    apply(d, ops, "clash")

    # Closed forms (sofa 2.1 x 0.9 x 0.85, wall 0.2 thick):
    #  CF1/CW1: 2.1 (x) x 0.2 (y) x 0.85 (z)
    #  CF2/CW2: rotated 90 deg -> 0.9 (x) x 0.2 (y) x (1.0 - 0.3) (z, wall top 1.0, base at z_offset 0.3)
    #  CR1/CW3: underside 2.5 + 0.2 (x - 40) crosses the wall top (3.0); section area 0.1 + 0.1 = 0.2, x 0.2 thick
    want = {frozenset(("CF1", "CW1")): 2.1 * 0.2 * 0.85, frozenset(("CF2", "CW2")): 0.9 * 0.2 * 0.7,
            frozenset(("CR1", "CW3")): 0.2 * 0.2}
    rr = roof_ref(roof, 0.0)[0]
    oracle = {frozenset(("CR1", "CW3")): (rr ^ box(41, 45, -0.1, 0.1, 0, 3)).volume(),
              frozenset(("CF1", "CW1")): (furniture_ref(dict(x=23.0, y=0.3, angle=0.0), 0.0, [2.1, 0.9, 0.85])[0]
                                          ^ box(20, 26, -0.1, 0.1, 0, 3)).volume(),
              frozenset(("CF2", "CW2")): (furniture_ref(dict(x=30.0, y=0.0, angle=math.pi / 2, z_offset=0.3), 0.0,
                                                        [2.1, 0.9, 0.85])[0] ^ box(28, 32, 0.4, 0.6, 0, 1)).volume()}
    for k, v in want.items():
        if not close(oracle[k], v, 1e-7):
            fail(f"oracle disagreement: manifold3d {sorted(k)} {oracle[k]} vs closed form {v}")

    p = run(["clash", d, "--clearance", "0.01", "--json"], want=1)
    doc = as_json(p.stdout, "clash --json")
    hard = {}
    for c in doc.get("hard") or []:
        key = frozenset((c.get("a"), c.get("b")))
        need(len(key) == 2 and key not in hard, f"clash: malformed or duplicated hard pair {c}")
        hard[key] = c.get("volume")
    need(set(hard) == set(want), f"hard clashes {sorted(sorted(k) for k in hard)}, expected {sorted(sorted(k) for k in want)}")
    meshes = export_gltf(d, "clash")
    for k, v in want.items():
        need(close(hard[k], v), f"hard clash {sorted(k)}: volume {hard[k]}, expected {v}")
        a, b = sorted(k)
        need(a in meshes and b in meshes, f"glTF lacks a mesh for {a} or {b}")
        mv = (as_manifold(a, meshes[a]) ^ as_manifold(b, meshes[b])).volume()
        need(close(mv, v, REL_MESH), f"hard clash {a}/{b}: manifold3d on the exported solids gives {mv}, expected {v}")
    mat = doc.get("matrix") or {}

    def count(ka, kb):
        get = lambda x, y: (mat.get(x) or {}).get(y, 0)  # noqa: E731
        return get(ka, kb) + get(kb, ka)
    need(count("Furniture", "Wall") == 2 and count("Roof", "Wall") == 1,
         f"clash matrix {mat}: expected Furniture/Wall 2 and Roof/Wall 1")


def main():
    validation()
    main_model()
    clash_model()
    print("T4.9 judge: all checks passed")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, TypeError, ValueError, IndexError, AttributeError) as e:
        fail(f"malformed bim output or missing entity: {type(e).__name__}: {e}")
