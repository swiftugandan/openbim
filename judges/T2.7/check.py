"""T2.7 oracle side: compare `bim solve --trade arch --json` and `bim export --format gltf` against
an independent manifold3d reconstruction (model.py) of the same ops.

Usage: python -I check.py <solve.json> <model.gltf> <ops.json> [<ops.json> ...]
Prints `JUDGE FAIL: <reason>` and exits 1 on the first failed check.
"""
import base64
import json
import os
import sys

import manifold3d as mf
import numpy as np
import trimesh

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import model  # noqa: E402

REL = 1e-6          # TASKS.md T2.7: volumes within 1e-6 relative (f64 outputs of `bim solve`)
GLTF_REL = 2e-5     # glTF POSITION accessors are float32 (spec: glTF 2.0), ~1e-7 m per coordinate
XOR_REL = 1e-4      # symmetric-difference volume of a float32 mesh vs the exact solid


def fail(msg):
    print(f"JUDGE FAIL: {msg}")
    sys.exit(1)


def close(got, want, rel, what, abs_tol=1e-12):
    if not isinstance(got, (int, float)) or isinstance(got, bool):
        fail(f"{what}: expected a number, got {got!r}")
    if abs(got - want) > rel * abs(want) + abs_tol:
        fail(f"{what}: got {got!r}, want {want!r} (rel tol {rel})")


# ---------------------------------------------------------------- glTF reading

COMP = {5120: np.int8, 5121: np.uint8, 5122: np.int16, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}
NCOMP = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


def read_accessor(g, bufs, idx):
    acc = g["accessors"][idx]
    if "bufferView" not in acc:
        fail(f"accessor {idx} has no bufferView")
    bv = g["bufferViews"][acc["bufferView"]]
    dt = np.dtype(COMP[acc["componentType"]])
    n = NCOMP[acc["type"]]
    buf = bufs[bv["buffer"]]
    start = bv.get("byteOffset", 0) + acc.get("byteOffset", 0)
    stride = bv.get("byteStride", 0) or dt.itemsize * n
    out = np.empty((acc["count"], n), dtype=np.float64)
    for i in range(acc["count"]):
        o = start + i * stride
        out[i] = np.frombuffer(buf, dtype=dt, count=n, offset=o)
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
    s = np.diag(node.get("scale", [1, 1, 1]))
    m = np.eye(4)
    m[:3, :3] = r @ s
    m[:3, 3] = t
    return m


def load_gltf(path):
    """{extras.id: (vertices (n,3) world, faces (m,3))} for every mesh node carrying extras.id."""
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
    scene = g["scenes"][g.get("scene", 0)]

    def visit(ni, parent):
        node = g["nodes"][ni]
        m = parent @ trs_matrix(node)
        if "mesh" in node:
            eid = (node.get("extras") or {}).get("id")
            if eid is not None:
                if not isinstance(eid, str):
                    fail(f"node {ni} extras.id is not a string: {eid!r}")
                if eid in meshes:
                    fail(f"entity {eid} has more than one mesh node")
                vs, fs, base = [], [], 0
                for prim in g["meshes"][node["mesh"]]["primitives"]:
                    if prim.get("mode", 4) != 4:
                        fail(f"{eid}: primitive mode {prim.get('mode')} is not TRIANGLES")
                    pos = read_accessor(g, bufs, prim["attributes"]["POSITION"])
                    if "indices" in prim:
                        idx = read_accessor(g, bufs, prim["indices"]).astype(np.int64).reshape(-1, 3)
                    else:
                        idx = np.arange(len(pos)).reshape(-1, 3)
                    vs.append(pos)
                    fs.append(idx + base)
                    base += len(pos)
                v = np.vstack(vs)
                v = (m @ np.c_[v, np.ones(len(v))].T).T[:, :3]
                meshes[eid] = (v, np.vstack(fs))
        for c in node.get("children", []):
            visit(c, m)

    for r in scene["nodes"]:
        visit(r, np.eye(4))
    return meshes


# glTF is y-up; the model is z-up (spec/schema.md). Accept the identity frame or the standard
# z-up -> y-up mapping glTF(x, y, z) = model(x, z, -y).
FRAMES = {"z-up (identity)": np.eye(3), "y-up": np.array([[1, 0, 0], [0, 0, -1], [0, 1, 0]], float)}


def to_manifold(eid, v, f):
    tm = trimesh.Trimesh(vertices=v, faces=f, process=True)
    tm.merge_vertices(digits_vertex=6)
    tm.remove_unreferenced_vertices()
    if not tm.is_watertight:
        fail(f"glTF mesh of {eid} is not watertight (not a closed solid)")
    man = mf.Manifold(mf.Mesh64(vert_properties=np.ascontiguousarray(tm.vertices, dtype=np.float64),
                                tri_verts=np.ascontiguousarray(tm.faces, dtype=np.uint64)))
    if man.status() != mf.Error.NoError or man.is_empty():
        fail(f"glTF mesh of {eid} is not an oriented closed 2-manifold ({man.status()})")
    return man


def xor_vol(a, b):
    return (a - b).volume() + (b - a).volume()


# ---------------------------------------------------------------- checks

def main():
    solve_path, gltf_path, ops = sys.argv[1], sys.argv[2], sys.argv[3:]
    ents = model.load_ops(*ops)
    rec = model.build(ents)
    with open(solve_path) as f:
        try:
            solve = json.load(f)
        except json.JSONDecodeError as e:
            fail(f"bim solve --json output is not one JSON document: {e}")

    # --- walls
    walls = solve.get("walls")
    if not isinstance(walls, list):
        fail("solve JSON has no walls[] array")
    got_ids = [w.get("id") for w in walls]
    if sorted(got_ids) != sorted(rec["walls"]) or len(set(got_ids)) != len(got_ids):
        fail(f"walls[] ids {sorted(map(str, got_ids))} != wall entities {sorted(rec['walls'])}")
    by_id = {w["id"]: w for w in walls}
    for wid, r in rec["walls"].items():
        w = by_id[wid]
        layers = w.get("layers")
        if not isinstance(layers, list) or len(layers) != len(r["layers"]):
            fail(f"{wid}: layers {layers!r} should have {len(r['layers'])} entries in type order")
        for i, (lg, (mat, sol)) in enumerate(zip(layers, r["layers"])):
            if lg.get("material") != mat:
                fail(f"{wid}: layer {i} material {lg.get('material')!r}, want {mat!r} (left side first)")
        close(sum(l.get("volume", float("nan")) for l in layers), w.get("volume"), REL,
              f"{wid}: sum of layer volumes vs wall volume", 1e-9)
        if r["determinate"]:
            close(w.get("volume"), r["solid"].volume(), REL, f"{wid}: volume", 1e-9)
            for i, (lg, (mat, sol)) in enumerate(zip(layers, r["layers"])):
                close(lg.get("volume"), sol.volume(), REL, f"{wid}: layer {i} ({mat}) volume", 1e-9)
            # Spec gap (proposed): side_area = area of the wall solid's section by the vertical
            # plane through its centreline, i.e. volume / thickness under the join rules.
            close(w.get("side_area"), r["solid"].volume() / r["t"], REL, f"{wid}: side_area", 1e-9)
        else:
            if not (w.get("volume", 0) > 0):
                fail(f"{wid}: volume must be positive")

    # L-joins between walls of unequal thickness: the split is not pinned, the filled region is.
    undet = sorted(i for i, r in rec["walls"].items() if not r["determinate"])
    groups = []
    for wid in undet:
        for k in (0, 1):
            j = rec["joins"][wid][k]
            if j and j[0] == "L" and not rec["walls"][j[1].id]["determinate"]:
                pair = tuple(sorted((wid, j[1].id)))
                if pair not in groups:
                    groups.append(pair)
    for pair in groups:
        union = model.run_through_union(rec, list(pair))
        # Each wall's other end is free in the fixtures, so the pair's volume is the union.
        close(sum(by_id[i]["volume"] for i in pair), union.volume(), REL,
              f"L-join {pair}: sum of wall volumes vs the joined region (gap or overlap)", 1e-9)

    # --- slabs
    slabs = solve.get("slabs")
    if not isinstance(slabs, list) or sorted(s.get("id") for s in slabs) != sorted(rec["slabs"]):
        fail(f"slabs[] ids should be {sorted(rec['slabs'])}, got {slabs!r}")
    for s in slabs:
        r = rec["slabs"][s["id"]]
        close(s.get("volume"), r["solid"].volume(), REL, f"{s['id']}: slab volume", 1e-9)
        # Spec gap (proposed): slab area = net plan area (boundary minus SlabOpenings).
        close(s.get("area"), r["area"], REL, f"{s['id']}: slab area", 1e-9)

    # --- glTF
    meshes = load_gltf(gltf_path)
    want = model.all_solids(rec)
    missing = sorted(set(want) - set(meshes))
    if missing:
        fail(f"glTF has no mesh node with extras.id for {missing}")
    mans = {}
    for eid in want:
        v, f = meshes[eid]
        mans[eid] = (v, f)
    best = None
    for name, R in FRAMES.items():
        built = {}
        err = 0.0
        for eid, (v, f) in mans.items():
            vv = (R @ v.T).T
            lo, hi = vv.min(0), vv.max(0)
            bb = want[eid].bounding_box()
            err += float(np.abs(lo - np.array(bb[:3])).sum() + np.abs(hi - np.array(bb[3:])).sum())
        if best is None or err < best[1]:
            best = (name, err, R)
    frame, _, R = best
    solids = {eid: to_manifold(eid, (R @ v.T).T, f) for eid, (v, f) in mans.items()}
    for eid, sol in solids.items():
        wrec = rec["walls"].get(eid)
        if wrec is not None and not wrec["determinate"]:
            continue
        close(sol.volume(), want[eid].volume(), GLTF_REL, f"glTF {eid}: mesh volume ({frame} frame)", 1e-9)
        x = xor_vol(sol, want[eid])
        if x > XOR_REL * want[eid].volume() + 1e-7:
            fail(f"glTF {eid}: mesh differs from the schema solid by {x:.3g} m3 (wrong placement/shape)")
    for pair in groups:
        u = solids[pair[0]] + solids[pair[1]]
        ref_u = model.run_through_union(rec, list(pair))
        x = xor_vol(u, ref_u)
        if x > XOR_REL * ref_u.volume() + 1e-7:
            fail(f"glTF L-join {pair}: joined region differs from the schema region by {x:.3g} m3")
    # Joined (and all other) solids never overlap.
    ids = sorted(solids)
    for i, a in enumerate(ids):
        for b in ids[i + 1:]:
            ov = (solids[a] ^ solids[b]).volume()
            if ov > 1e-6 * min(solids[a].volume(), solids[b].volume()) + 1e-8:
                fail(f"glTF solids {a} and {b} overlap by {ov:.3g} m3")
    print(f"T2.7 check: {len(walls)} walls, {len(slabs)} slabs, {len(solids)} glTF solids ok ({frame})")


if __name__ == "__main__":
    main()
