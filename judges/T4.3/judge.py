"""External judge for T4.3 (curtain walls, stairs, ramps).

Drives the real `bim` binary (argv[1]): `solve --trade arch --json` must report the closed-form
panel counts, mullion lengths, risers and treads; `export --format gltf` must contain, for each
CurtainWall/Stair/Ramp entity (node extras.id), a closed solid equal to the reference solid that
this judge builds independently with manifold3d from the conventions listed in the report
(volume via trimesh and via the closed form; shape and placement via manifold3d XOR volume),
read back through the y-up glTF frame of spec/cli.md: glTF (x, y, z) = model (x, z, −y).
Run as: .oracles/py/bin/python -I judges/T4.3/judge.py <path-to-bim>
"""
import base64
import json
import math
import os
import subprocess
import sys
import tempfile

import manifold3d as mf
import numpy as np
import trimesh

BIM = sys.argv[1]
REL_EXACT = 1e-9   # counts / lengths / risers in JSON (f64)
REL_MESH = 1e-4    # glTF positions are float32
failures = []


def fail(msg):
    failures.append(msg)


def run(*args):
    p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=60)
    if p.returncode != 0:
        print(f"JUDGE FAIL: bim {' '.join(args)} exited {p.returncode}: {p.stderr.strip()[:500]}")
        sys.exit(1)
    return p.stdout


def close(a, b, rel):
    try:
        return abs(a - b) <= rel * max(abs(b), 1e-12)
    except TypeError:
        return False


# ---------------------------------------------------------------- reference solids (manifold3d)

def box(x0, x1, y0, y1, z0, z1):
    return mf.Manifold.cube([x1 - x0, y1 - y0, z1 - z0]).translate([x0, y0, z0])


def place(local, origin, angle, z):
    """Local frame: x along the run/wall, y to its left, z up from the level → world."""
    return local.rotate([0, 0, math.degrees(angle)]).translate([origin[0], origin[1], z])


def union(parts):
    out = parts[0]
    for p in parts[1:]:
        out = out + p
    return out


def grid(total, spacing):
    n = max(1, math.ceil(total / spacing - 1e-9))
    return [min(k * spacing, total) for k in range(n)] + [total]


def curtain_wall(cw, elev):
    L = math.hypot(cw["x2"] - cw["x1"], cw["y2"] - cw["y1"])
    H, w, d, pt = cw["height"], cw["mullion_width"], cw["mullion_depth"], cw["panel_thickness"]
    us, vs = grid(L, cw["u_spacing"]), grid(H, cw["v_spacing"])
    nu, nv = len(us) - 1, len(vs) - 1
    parts = [box(s - w / 2, s + w / 2, -d / 2, d / 2, 0, H) for s in us]
    parts += [box(0, L, -d / 2, d / 2, t - w / 2, t + w / 2) for t in vs]
    for a, b in zip(us, us[1:]):
        for c, e in zip(vs, vs[1:]):
            parts.append(box(a + w / 2, b - w / 2, -pt / 2, pt / 2, c + w / 2, e - w / 2))
    solid = place(union(parts), (cw["x1"], cw["y1"]), math.atan2(cw["y2"] - cw["y1"], cw["x2"] - cw["x1"]), elev)
    # closed form of the same union: mullions minus their crossings, plus the panels
    mull = w * d * ((nu + 1) * H + (nv + 1) * L) - d * (nu * w) * (nv * w)
    pan = pt * sum(b - a - w for a, b in zip(us, us[1:])) * sum(e - c - w for c, e in zip(vs, vs[1:]))
    return solid, mull + pan, nu * nv, (nu + 1) * H + (nv + 1) * L


def stair(st, zb, zt):
    n, t, W = st["riser_count"], st["tread"], st["width"]
    r = (zt - zb) / n
    parts = [box((k - 1) * t, k * t, -W / 2, W / 2, 0, k * r) for k in range(1, n + 1)]
    solid = place(union(parts), (st["x"], st["y"]), st["angle"], zb)
    return solid, W * t * r * n * (n + 1) / 2, r


def ramp(rp, zb, zt):
    L, W, rise = rp["length"], rp["width"], zt - zb
    # wedge: triangle (0,0)-(L,0)-(L,rise) in the x–z plane, extruded across the width
    tri = mf.CrossSection([[(0.0, 0.0), (L, 0.0), (L, rise)]])
    wedge = mf.Manifold.extrude(tri, W).rotate([90, 0, 0]).translate([0, W / 2, 0])
    return place(wedge, (rp["x"], rp["y"]), rp["angle"], zb), W * L * rise / 2


# ---------------------------------------------------------------- glTF decoding (independent of trimesh's loader)

COMP = {5121: np.uint8, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}
NCOMP = {"SCALAR": 1, "VEC3": 3}


def node_matrix(n):
    if "matrix" in n:
        return np.array(n["matrix"], dtype=float).reshape(4, 4).T
    T = trimesh.transformations.translation_matrix(n.get("translation", [0, 0, 0]))
    x, y, z, w = n.get("rotation", [0, 0, 0, 1])
    R = trimesh.transformations.quaternion_matrix([w, x, y, z])
    S = np.diag(list(n.get("scale", [1, 1, 1])) + [1])
    return T @ R @ S


def load_gltf(path):
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
        stride = v.get("byteStride", 0) or np.dtype(dt).itemsize * nc
        raw = bufs[v["buffer"]]
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
            M = world(i)
            v = np.vstack(verts)
            v = (np.c_[v, np.ones(len(v))] @ M.T)[:, :3]
            meshes[eid] = trimesh.Trimesh(v, np.vstack(faces), process=False)
    return meshes


# spec/cli.md: glTF (x, y, z) = model (x, z, −y), so model = (gx, −gz, gy); as a row-vector
# right-multiplication, vertices @ Y_UP recovers model coordinates. Only this frame is accepted.
Y_UP = np.array([[1, 0, 0], [0, 0, 1], [0, -1, 0]], dtype=float)


def check_solid(eid, mesh, ref, ref_vol):
    if mesh is None:
        fail(f"glTF has no mesh node with extras.id {eid!r}")
        return
    if not close(ref.volume(), ref_vol, 1e-7):
        print(f"JUDGE FAIL: oracle disagreement for {eid}: manifold3d {ref.volume()} vs closed form {ref_vol}")
        sys.exit(1)
    m = mesh.copy()
    m.merge_vertices()
    if not m.is_watertight:
        fail(f"{eid}: glTF mesh is not closed (watertight)")
        return
    if not close(abs(m.volume), ref_vol, REL_MESH):
        fail(f"{eid}: solid volume {abs(m.volume)} expected {ref_vol}")
        return
    v = m.vertices @ Y_UP  # model coordinates
    try:
        got = mf.Manifold(mf.Mesh(vert_properties=np.asarray(v, dtype=np.float32),
                                  tri_verts=np.asarray(m.faces, dtype=np.uint32)))
    except Exception as e:  # noqa: BLE001 - any rejection by manifold3d is a failed mesh
        fail(f"{eid}: glTF mesh is not a valid manifold solid ({e})")
        return
    if got.is_empty() or got.volume() < 0:
        fail(f"{eid}: glTF mesh is not a valid, outward-oriented manifold solid")
        return
    xor = (got - ref).volume() + (ref - got).volume()
    if xor > REL_MESH * ref_vol:
        fail(f"{eid}: solid differs from the reference shape/placement (in the y-up glTF frame "
             f"of spec/cli.md) by volume {xor}")


# ---------------------------------------------------------------- scenario

def scenario():
    levels = {"B": 0.0, "U": 3.15, "R": 4.35}
    cws = {
        # remainder bays in both directions, non-axis-aligned
        "cw_rem": dict(level="B", x1=1.0, y1=2.0, x2=5.0, y2=5.0, height=3.0, u_spacing=1.5,
                       v_spacing=1.25, mullion_width=0.05, mullion_depth=0.15, panel_thickness=0.024),
        # exact multiples: no zero-width last bay
        "cw_exact": dict(level="B", x1=10.0, y1=0.0, x2=16.0, y2=0.0, height=3.0, u_spacing=1.5,
                         v_spacing=1.0, mullion_width=0.06, mullion_depth=0.2, panel_thickness=0.03),
        # spacing larger than the wall: one bay each way
        "cw_one": dict(level="U", x1=0.0, y1=10.0, x2=0.0, y2=8.8, height=1.0, u_spacing=2.0,
                       v_spacing=2.0, mullion_width=0.05, mullion_depth=0.1, panel_thickness=0.02),
    }
    stairs = {
        "st_axis": dict(base="B", top="U", x=20.0, y=5.0, angle=0.0, width=1.2, riser_count=18, tread=0.28),
        "st_skew": dict(base="U", top="R", x=-4.0, y=3.0, angle=math.pi / 6, width=1.0, riser_count=7, tread=0.3),
    }
    ramps = {"rp": dict(base="U", top="R", x=30.0, y=0.0, angle=math.pi / 2, width=1.5, length=14.4)}
    ops = [{"op": "create", "id": k, "kind": "Level", "props": {"elevation": v}} for k, v in levels.items()]
    for k, c in cws.items():
        props = dict(c, level={"ref": c["level"]})
        ops.append({"op": "create", "id": k, "kind": "CurtainWall", "props": props})
    for k, s in stairs.items():
        ops.append({"op": "create", "id": k, "kind": "Stair",
                    "props": dict(s, base={"ref": s["base"]}, top={"ref": s["top"]})})
    for k, r in ramps.items():
        ops.append({"op": "create", "id": k, "kind": "Ramp",
                    "props": dict(r, base={"ref": r["base"]}, top={"ref": r["top"]})})
    return levels, cws, stairs, ramps, ops


def main():
    levels, cws, stairs, ramps, ops = scenario()
    with tempfile.TemporaryDirectory() as tmp:
        proj = os.path.join(tmp, "p")
        run("init", proj)
        with open(os.path.join(tmp, "ops.json"), "w") as f:
            json.dump({"schema": 1, "ops": ops}, f)
        run("apply", proj, os.path.join(tmp, "ops.json"))
        try:
            out = json.loads(run("solve", proj, "--trade", "arch", "--json"))
        except json.JSONDecodeError as e:
            print(f"JUDGE FAIL: solve --json printed invalid JSON: {e}")
            sys.exit(1)
        gltf = os.path.join(tmp, "m.gltf")
        run("export", proj, "--format", "gltf", "--out", gltf)
        meshes = load_gltf(gltf)

    got_cw = {c.get("id"): c for c in out.get("curtain_walls") or []}
    if set(got_cw) != set(cws):
        fail(f"curtain_walls ids {sorted(got_cw)} expected {sorted(cws)}")
    for k, c in cws.items():
        solid, vol, panels, mlen = curtain_wall(c, levels[c["level"]])
        g = got_cw.get(k, {})
        if g.get("panels") != panels:
            fail(f"{k}: panels {g.get('panels')} expected {panels}")
        if not close(g.get("mullion_length"), mlen, REL_EXACT):
            fail(f"{k}: mullion_length {g.get('mullion_length')} expected {mlen}")
        check_solid(k, meshes.get(k), solid, vol)

    got_st = {s.get("id"): s for s in out.get("stairs") or []}
    if set(got_st) != set(stairs):
        fail(f"stairs ids {sorted(got_st)} expected {sorted(stairs)}")
    for k, s in stairs.items():
        solid, vol, riser = stair(s, levels[s["base"]], levels[s["top"]])
        g = got_st.get(k, {})
        if not close(g.get("riser"), riser, REL_EXACT):
            fail(f"{k}: riser {g.get('riser')} expected {riser}")
        if not close(g.get("tread"), s["tread"], REL_EXACT):
            fail(f"{k}: tread {g.get('tread')} expected {s['tread']}")
        if g.get("risers") != s["riser_count"]:
            fail(f"{k}: risers {g.get('risers')} expected {s['riser_count']}")
        check_solid(k, meshes.get(k), solid, vol)

    for k, r in ramps.items():
        solid, vol = ramp(r, levels[r["base"]], levels[r["top"]])
        check_solid(k, meshes.get(k), solid, vol)

    if failures:
        for f in failures:
            print(f"JUDGE FAIL: {f}")
        sys.exit(1)
    print("T4.3 judge: all checks passed")


if __name__ == "__main__":
    main()
