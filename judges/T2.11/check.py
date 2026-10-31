"""T2.11 oracle side: offset, follow-me, move/rotate/scale (stretching, triangle fan, copy/array), paint and
components, driven through the real `bim sketch` and checked against closed forms and independent
constructions recomputed here (spec/sketch.md, TASKS.md T2.11).

Usage: python -I check.py <path to bim>
Prints `JUDGE FAIL: <group>: <reason>` and exits 1 on the first failed check.
"""
import math
import os
import sys

import numpy as np
from scipy.spatial import ConvexHull
from shapely.geometry import Polygon

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sk import (Bim, EPS, close, fail, main, norm, same_points, pts_equal, rot, mat, colmajor,  # noqa: E402
                transform_points, check_gltf_matches, poly_volume, IDENTITY)

RECT = {"op": "rect", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]}


def box(b, name):
    """rect 4 x 3 + pushpull 2.5 (the spec example; T2.10). Returns (shape, n1, zf): face 1's normal and
    the z of face 1 (the box spans z between 0 and zf)."""
    b.sketch([dict(RECT, shape=name), {"op": "pushpull", "shape": name, "face": 1, "distance": 2.5}], "box")
    s = b.shape(name)
    s.require_closed(f"box {name}")
    close(s.volume(), 30.0, f"box {name}: volume")
    if sorted(s.ids) != list(range(1, 7)):
        fail(f"box {name}: face ids {sorted(s.ids)}, want 1..6")
    n1 = s.normals[1]
    return s, n1, 2.5 * n1[2]


def predict(s, moved, f):
    """The judge's own stretched Shape: same loops, vertices in `moved` mapped by f."""
    V = np.array([f(v) if i in moved else v for i, v in enumerate(s.V)])
    faces = [[V[lp] for lp in s.faces[fid]] for fid in s.order]
    for fid, loops in zip(s.order, faces):  # sanity of the judge's own scenario: every face stays planar
        n = norm(np.cross(loops[0][1] - loops[0][0], loops[0][2] - loops[0][0]))
        if np.max(np.abs((np.vstack(loops) - loops[0][0]) @ n)) > 1e-9:
            raise AssertionError(f"judge scenario error: face {fid} would not stay planar")
    return V, poly_volume(V, faces)


def check_stretch(b, name, s0, sel_vertices, f, cmd, what):
    V, vol = predict(s0, set(sel_vertices), f)
    b.sketch([dict(cmd, shape=name)], what)
    s = b.shape(name)
    s.require_closed(what)
    if sorted(s.ids) != sorted(s0.ids):
        fail(f"{what}: face ids {sorted(s.ids)}, want {sorted(s0.ids)} unchanged (faces only moved or stretched)")
    same_points(s.V, V, f"{what}: vertices")
    close(s.volume(), vol, f"{what}: volume")
    return s


# ------------------------------------------------------------------ groups

def g_offset(exe, root):
    b = Bim(exe, root, "offset")
    b.sketch([dict(RECT, shape="o1"), {"op": "offset", "shape": "o1", "face": 1, "distance": 0.5}], "offset rect")
    s = b.shape("o1")
    if sorted(s.ids) != [1, 2]:
        fail(f"offset 0.5 of a rect: face ids {sorted(s.ids)}, want [1, 2] (inner face gets the new id 2)")
    inner = [[0.5, 0.5, 0], [3.5, 0.5, 0], [3.5, 2.5, 0], [0.5, 2.5, 0]]
    close(s.area(2), (4 - 1) * (3 - 1), "offset: inner face area (w - 2d)(h - 2d)")
    same_points(s.points(2), inner, "offset: inner face vertices (mitred)")
    if len(s.face(1)) != 2:
        fail(f"offset: face 1 should become a ring (2 loops), has {len(s.face(1))}")
    close(s.area(1), 12 - 6, "offset: ring face 1 area")
    same_points(s.points(1, loop=1), inner, "offset: ring hole vertices")

    # Outward: the face is unchanged and a new ring face (id 2) surrounds it, mitred.
    b.sketch([dict(RECT, shape="oo"), {"op": "offset", "shape": "oo", "face": 1, "distance": -0.5}], "offset out")
    s = b.shape("oo")
    if sorted(s.ids) != [1, 2]:
        fail(f"outward offset: face ids {sorted(s.ids)}, want [1, 2] (new ring face 2)")
    if len(s.face(1)) != 1:
        fail("outward offset: face 1 should be unchanged (one loop)")
    same_points(s.points(1), [[0, 0, 0], [4, 0, 0], [4, 3, 0], [0, 3, 0]], "outward offset: face 1 unchanged")
    if len(s.face(2)) != 2:
        fail(f"outward offset: face 2 should be a ring (2 loops), has {len(s.face(2))}")
    same_points(s.points(2, loop=0), [[-0.5, -0.5, 0], [4.5, -0.5, 0], [4.5, 3.5, 0], [-0.5, 3.5, 0]],
                "outward offset: ring outer loop (mitred)")
    same_points(s.points(2, loop=1), [[0, 0, 0], [4, 0, 0], [4, 3, 0], [0, 3, 0]], "outward offset: ring hole")
    close(s.area(2), 5 * 4 - 12, "outward offset: ring area (w + 2d)(h + 2d) - wh")

    # L-shaped face in z = 2 drawn with lines; mitred inward offset vs shapely's mitre buffer.
    L = [(0, 0), (4, 0), (4, 1), (1, 1), (1, 3), (0, 3)]
    b.sketch([{"op": "line", "shape": "oL", "from": [*L[k], 2.0], "to": [*L[(k + 1) % 6], 2.0]} for k in range(6)]
             + [{"op": "offset", "shape": "oL", "face": 1, "distance": 0.25}], "offset L")
    s = b.shape("oL")
    want = Polygon(L).buffer(-0.25, join_style="mitre", mitre_limit=100.0)
    wpts = [[x, y, 2.0] for x, y in list(want.exterior.coords)[:-1]]
    if sorted(s.ids) != [1, 2]:
        fail(f"offset L: face ids {sorted(s.ids)}, want [1, 2]")
    close(s.area(2), want.area, "offset L: inner face area (shapely mitre buffer)")
    same_points(s.points(2), wpts, "offset L: inner face vertices")
    close(s.area(1), 6 - want.area, "offset L: ring area")

    # Offset on a solid's face, then push the inner face in.
    s, n1, zf = box(b, "ob")
    b.sketch([{"op": "offset", "shape": "ob", "face": 1, "distance": 0.5}], "offset box top")
    s = b.shape("ob")
    if sorted(s.ids) != list(range(1, 8)):
        fail(f"offset box top: face ids {sorted(s.ids)}, want 1..7")
    s.require_closed("offset box top")
    close(s.volume(), 30.0, "offset box top: volume")
    close(s.area(7), 6.0, "offset box top: inner face 7 area")
    b.sketch([{"op": "pushpull", "shape": "ob", "face": 7, "distance": -1}], "push inner face")
    s = b.shape("ob")
    s.require_closed("offset + push in")
    close(s.volume(), 24.0, "offset + push in: volume 30 - 3 x 2 x 1")


def mitred_sweep(profile, path):
    """Independent mitred sweep of a convex planar profile (spec/sketch.md followme): the profile is carried
    along each segment and cut by the bisecting plane at each joint (perpendicular plane at the end).
    Returns (volume, all section vertices): each segment piece is the convex hull of its two sections."""
    P = [np.asarray(p, float) for p in path]
    d = [norm(P[k + 1] - P[k]) for k in range(len(P) - 1)]
    sec = [np.asarray(profile, float)]
    vol = 0.0
    for k in range(len(d)):
        m = norm(d[k] + d[k + 1]) if k + 1 < len(d) else d[k]
        cur = sec[-1]
        t = ((P[k + 1] - cur) @ m) / (d[k] @ m)
        nxt = cur + np.outer(t, d[k])
        vol += ConvexHull(np.vstack([cur, nxt])).volume
        sec.append(nxt)
    return vol, np.vstack(sec)


def g_followme(exe, root):
    b = Bim(exe, root, "followme")
    cases = [
        ("fsq", {"op": "rect", "corner": [0, 0, 0], "u": [0, 1, 0], "v": [0, 0, 1]},
         [[0, 0, 0], [4, 0, 0], [4, 3, 0]], 6.0),                          # off-centre square, L path: 7 - 2*0.5
        ("fcl", {"op": "rect", "corner": [0, -0.5, -0.5], "u": [0, 1, 0], "v": [0, 0, 1]},
         [[0, 0, 0], [4, 0, 0], [4, 3, 0]], 7.0),                          # centred square: A (L1 + L2)
        ("fci", {"op": "circle", "centre": [0, 0, 0], "normal": [1, 0, 0], "radius": 0.5, "segments": 12},
         [[0, 0, 0], [3, 0, 0], [3, 2, 0], [0, 2, 0]], None),              # 12-gon along a U path
        ("fob", {"op": "rect", "corner": [0, -0.5, -0.25], "u": [0, 1, 0], "v": [0, 0, 0.5]},
         [[0, 0, 0], [3, 0, 0], [5, 2, 1]], None),                         # oblique, non-planar joint
    ]
    shapes = {}
    for name, prof, path, closed_form in cases:
        b.sketch([dict(prof, shape=name)], f"{name} profile")
        s0 = b.shape(name)
        if s0.ids != [1]:
            fail(f"{name}: profile should be one face, got {s0.ids}")
        vol, secs = mitred_sweep(s0.points(1), path)
        if closed_form is not None and abs(vol - closed_form) > 1e-9:
            raise AssertionError(f"judge oracle disagrees with the closed form for {name}: {vol} vs {closed_form}")
        b.sketch([{"op": "followme", "shape": name, "face": 1, "path": path}], f"{name} followme")
        s = b.shape(name)
        what = f"followme {name}"
        s.require_closed(what)
        close(s.volume(), vol, f"{what}: volume of the mitred sweep")
        same_points(s.V, secs, f"{what}: vertices (the profile at the start, at each mitre plane and at the end)")
        nprof, nseg = len(s0.face(1)[0]), len(path) - 1
        if len(s.ids) != nprof * nseg + 2:
            fail(f"{what}: {len(s.ids)} faces, want {nprof * nseg + 2} ({nprof} sides x {nseg} segments + 2 caps; "
                 f"coplanar faces never merge on their own)")
        shapes[name] = (s, vol)
    meshes = b.gltf("followme")
    for name in ("fsq", "fob"):
        s, vol = shapes[name]
        check_gltf_matches(meshes, name, s.V, vol, f"followme {name}", euler=2, bodies=1)


def fan_case(b, name, want_first_in):
    """Move one box corner along a face normal so that face fans; want_first_in = 0 or -1 prefers a choice where
    the face's first outer-loop vertex lies only in the first or only in the last fan triangle (and where the
    highest-index vertex is adjacent to the lowest, so a fan from the wrong vertex differs)."""
    s0, n1, zf = box(b, name)
    choices = []
    for v in range(len(s0.V)):
        for fid in s0.ids:
            if v in s0.face(fid)[0]:
                lp = s0.face(fid)[0]
                lo, hi = min(lp), max(lp)
                k = lp.index(lo)
                rl = lp[k:] + lp[:k]
                tris = [(rl[0], rl[j], rl[j + 1]) for j in range(1, len(rl) - 1)]
                first = lp[0]
                only = [t for t in tris if first in t]
                rank = (0 if len(only) == 1 and only[0] == tris[want_first_in] else 1 if len(only) == 1 else 2,
                        0 if hi in (rl[1], rl[-1]) else 1)
                choices.append((rank, v, fid, tris, first))
    choices.sort(key=lambda c: (c[0], c[1], c[2]))
    _, v, fid, tris, first = choices[0]
    vec = 0.5 * s0.normals[fid]
    V = s0.V.copy()
    V[v] = V[v] + vec
    faces = [[V[lp] for lp in s0.face(f)] for f in s0.ids if f != fid] + [[V[list(t)]] for t in tris]
    want_vol = poly_volume(V, faces)
    b.sketch([{"op": "move", "shape": name, "vertices": [v], "vector": vec.tolist()}], "move a corner")
    s = b.shape(name)
    what = f"{name}: move vertex {v} along face {fid}'s normal"
    s.require_closed(what)
    same_points(s.V, V, f"{what}: vertices (indices kept)")
    if np.abs(s.V[v] - V[v]).max() > 1e-9:
        fail(f"{what}: move should keep vertex indices (vertex {v} is not the moved one)")
    if sorted(s.ids) != list(range(1, 8)):
        fail(f"{what}: face ids {sorted(s.ids)}, want 1..7 (face {fid} split into 2 triangles, the other is 7)")
    got = {f: tuple(sorted(s.face(f)[0])) for f in (fid, 7)}
    want = [tuple(sorted(t)) for t in tris]
    if sorted(got.values()) != sorted(want) or any(len(s.face(f)) != 1 for f in (fid, 7)):
        fail(f"{what}: triangles {got}, want the fan from the lowest-index vertex {min(s0.face(fid)[0])}: {want}")
    keep = [t for t in tris if first in t]
    if len(keep) == 1 and got[fid] != tuple(sorted(keep[0])):
        fail(f"{what}: the triangle containing face {fid}'s first outer-loop vertex {first} should keep id {fid}")
    for f in s0.ids:
        if f != fid and sorted(s.face(f)[0]) != sorted(s0.face(f)[0]):
            fail(f"{what}: face {f} should keep its loop (only stretched)")
    close(s.volume(), want_vol, f"{what}: volume")


def g_move(exe, root):
    b = Bim(exe, root, "move")
    s0, n1, zf = box(b, "m1")
    check_stretch(b, "m1", s0, [i for lp in s0.face(1) for i in lp], lambda v: v + n1,
                  {"op": "move", "faces": [1], "vector": n1.tolist()}, "move face 1 out by 1")
    s = b.shape("m1")
    close(s.volume(), 42.0, "move face 1: volume 4 x 3 x 3.5")

    s0, n1, zf = box(b, "m2")
    i, j = s0.find([0, 3, zf]), s0.find([4, 3, zf])
    s = check_stretch(b, "m2", s0, [i, j], lambda v: v + [0, 1, 0],
                      {"op": "move", "edges": [[i, j]], "vector": [0, 1, 0]}, "move a top edge sideways")
    close(abs(s.volume()), 35.0, "move edge: volume 4 x 2.5 x (3 + 4) / 2")

    # One box corner moved along one face's normal: only that face becomes non-planar and is split into triangles
    # fanned from its lowest-index vertex; the triangle containing its first outer-loop vertex keeps the id and
    # the other takes the next new id (7). Vertex indices are kept, so the expected fan follows from the loops
    # before the move. fan_case picks (corner, face) choices that make the id rule and the fan vertex observable.
    fan_case(b, "m3a", 0)
    fan_case(b, "m3b", -1)

    # copy + array: the nth copy transformed n times; copies are new faces with new ids.
    s0, n1, zf = box(b, "cp")
    b.sketch([{"op": "move", "shape": "cp", "faces": list(range(1, 7)), "vector": [6, 0, 0], "copy": True,
               "array": 3}], "array of box copies")
    s = b.shape("cp")
    s.require_closed("array of box copies")
    if sorted(s.ids) != list(range(1, 25)):
        fail(f"array of 3 copies: face ids {sorted(s.ids)}, want 1..24")
    close(s.volume(), 120.0, "array of 3 copies: volume 4 x 30")
    same_points(s.V, np.vstack([s0.V + [6 * n, 0, 0] for n in range(4)]), "array of 3 copies: vertices")
    b.sketch([dict(RECT, shape="cq", u=[1, 0, 0], v=[0, 1, 0]),
              {"op": "move", "shape": "cq", "faces": [1], "vector": [2, 0, 0], "copy": True, "array": 4},
              dict(RECT, shape="c1", corner=[0, 5, 0], u=[1, 0, 0], v=[0, 1, 0]),
              {"op": "move", "shape": "c1", "faces": [1], "vector": [0, 2, 0], "copy": True}], "face copies")
    s = b.shape("cq")
    if sorted(s.ids) != [1, 2, 3, 4, 5]:
        fail(f"array of 4 face copies: face ids {sorted(s.ids)}, want 1..5")
    cx = sorted(round(float(s.centroid(f)[0]), 9) for f in s.ids)
    if cx != [0.5, 2.5, 4.5, 6.5, 8.5] or any(abs(s.area(f) - 1) > 1e-9 for f in s.ids):
        fail(f"array of 4 face copies: centroids x {cx}, want 0.5, 2.5, ..., 8.5 with unit areas")
    if 1 not in s.ids or abs(s.centroid(1)[0] - 0.5) > 1e-9:
        fail("array of face copies: the original face 1 should stay in place")
    s = b.shape("c1")
    if sorted(s.ids) != [1, 2]:
        fail(f"copy without array: face ids {sorted(s.ids)}, want [1, 2] (one copy)")
    same_points(s.points(2), np.array([[0, 7, 0], [1, 7, 0], [1, 8, 0], [0, 8, 0]]), "copy: face 2 vertices")
    meshes = b.gltf("copies")
    s = b.shape("cp")
    check_gltf_matches(meshes, "cp", s.V, 120.0, "array of box copies", bodies=4)


def g_rotate_scale(exe, root):
    b = Bim(exe, root, "rotscale")
    s0, n1, zf = box(b, "r1")
    R = rot([0, 0, 1], math.pi / 3)
    c = np.array([1.0, 1, 0])
    check_stretch(b, "r1", s0, range(len(s0.V)), lambda v: c + R @ (v - c),
                  {"op": "rotate", "vertices": list(range(len(s0.V))), "centre": c.tolist(), "axis": [0, 0, 1],
                   "angle": math.pi / 3}, "rotate the whole box")

    s0, n1, zf = box(b, "r2")
    R = rot([1, 0, 0], 0.3)
    c = np.array([0, 0, zf])
    s = check_stretch(b, "r2", s0, [i for lp in s0.face(1) for i in lp], lambda v: c + R @ (v - c),
                      {"op": "rotate", "faces": [1], "centre": c.tolist(), "axis": [1, 0, 0], "angle": 0.3},
                      "rotate face 1 about its edge (hinge)")
    # Closed form: a prism along x whose (y, z) section is (0, 0), (3, 0), R(3, zf) - c, (0, zf).
    q = R @ np.array([0, 3, 0.0]) + c
    sec = np.array([[0, 0], [3, 0], [q[1], q[2]], [0, zf]])
    A = abs(0.5 * np.sum(sec[:, 0] * np.roll(sec[:, 1], -1) - np.roll(sec[:, 0], -1) * sec[:, 1]))
    close(s.volume(), 4 * A, "hinged face: volume = 4 x section area")

    s0, n1, zf = box(b, "s1")
    o = np.array([0.0, 0, 0])
    fct = np.array([2, 0.5, 3])
    s = check_stretch(b, "s1", s0, range(len(s0.V)), lambda v: o + fct * (v - o),
                      {"op": "scale", "vertices": list(range(len(s0.V))), "origin": o.tolist(),
                       "factors": fct.tolist()}, "non-uniform scale of the box")
    close(s.volume(), 90.0, "non-uniform scale: volume x 3")

    s0, n1, zf = box(b, "s2")
    o = np.array([2.0, 1.5, zf])
    fct = np.array([0.5, 0.5, 1])
    s = check_stretch(b, "s2", s0, [i for lp in s0.face(1) for i in lp], lambda v: o + fct * (v - o),
                      {"op": "scale", "faces": [1], "origin": o.tolist(), "factors": fct.tolist()},
                      "scale face 1 about its centre")
    close(s.volume(), 2.5 / 6 * (12 + 4 * 6.75 + 3), "scaled top: prismatoid volume h/6 (A1 + 4 Am + A2)")

    # rotate + copy + array: four quarter turns of a unit square.
    b.sketch([{"op": "rect", "shape": "rc", "corner": [2, 0, 0], "u": [1, 0, 0], "v": [0, 1, 0]},
              {"op": "rotate", "shape": "rc", "faces": [1], "centre": [0, 0, 0], "axis": [0, 0, 1],
               "angle": math.pi / 2, "copy": True, "array": 3}], "rotated copies")
    s = b.shape("rc")
    if sorted(s.ids) != [1, 2, 3, 4]:
        fail(f"rotated copies: face ids {sorted(s.ids)}, want 1..4")
    sq = np.array([[2, 0, 0], [3, 0, 0], [3, 1, 0], [2, 1, 0.0]])
    same_points(s.points(1), sq, "rotated copies: face 1 stays")
    for n in range(4):
        Rn = rot([0, 0, 1], n * math.pi / 2)
        if not any(pts_equal(s.points(f), (Rn @ sq.T).T) for f in s.ids):
            fail(f"rotated copies: no face equal to the square turned {n} x 90 degrees")


def g_paint(exe, root):
    b = Bim(exe, root, "paint")
    b.apply([{"op": "create", "id": "P1", "kind": "Product", "props": {
        "catalog": "acme", "catalog_version": "2026.1", "sku": "PNT-1", "category": "finish", "name": "Paint",
        "manufacturer": "Acme"}}], "product")
    box(b, "pt")
    b.sketch([{"op": "paint", "shape": "pt", "faces": [1, 3], "product": "P1"}], "paint")
    s = b.shape("pt")
    fp = s.props.get("face_products")
    want = [{"ref": "P1"} if f in (1, 3) else "" for f in s.order]
    if fp != want:
        fail(f"paint faces 1 and 3: face_products {fp}, want {want} (per face, in face order)")
    b.sketch([{"op": "paint", "shape": "pt", "faces": [3], "product": None}], "unpaint")
    s = b.shape("pt")
    want = [{"ref": "P1"} if f == 1 else "" for f in s.order]
    if s.props.get("face_products") != want:
        fail(f"paint null on face 3: face_products {s.props.get('face_products')}, want {want}")


def g_components(exe, root):
    b = Bim(exe, root, "comp")
    box(b, "kb")
    T0 = np.eye(4)
    T0[:3, :3] = rot([0, 0, 1], math.radians(30))
    T0[:3, 3] = [10, 0, 0]
    b.apply([{"op": "set", "id": "kb", "key": "transform", "value": colmajor(T0)}], "shape transform")
    b.sketch([{"op": "make_component", "shape": "kb", "definition": "KD", "instance": "KI", "name": "Crate"}],
             "make_component")
    d = b.show("KD")
    if d.get("kind") != "ComponentDefinition" or d["props"].get("name") != "Crate" \
            or d["props"].get("shape") != {"ref": "kb"}:
        fail(f"make_component: KD should be a ComponentDefinition named Crate with shape kb, got {d}")
    i = b.show("KI")
    if i.get("kind") != "ComponentInstance" or i["props"].get("definition") != {"ref": "KD"}:
        fail(f"make_component: KI should be a ComponentInstance of KD, got {i}")
    tr = i["props"].get("transform")
    if not isinstance(tr, list) or len(tr) != 16 or np.abs(np.array(tr, float) - colmajor(T0)).max() > 1e-12:
        fail(f"make_component: KI transform {tr} should be the shape's former transform {colmajor(T0)}")
    s = b.shape("kb")
    st = s.props.get("transform", IDENTITY)
    if not isinstance(st, list) or len(st) != 16 or np.abs(np.array(st, float) - IDENTITY).max() > 1e-12:
        fail(f"make_component: the shape's transform should become identity, got {st}")
    close(s.volume(), 30.0, "make_component: the definition's shape is unchanged")
    T2 = np.eye(4)
    T2[:3, :3] = rot([1, 1, 0], 0.7)
    T2[:3, 3] = [0, 20, 3]
    b.apply([{"op": "create", "id": "KI2", "kind": "ComponentInstance",
              "props": {"definition": {"ref": "KD"}, "transform": colmajor(T2)}}], "second instance")
    for tag, vol in (("before", 30.0), ("after", 42.0)):
        if tag == "after":
            b.sketch([{"op": "pushpull", "shape": "kb", "face": 1, "distance": 1}], "edit the definition's shape")
            s = b.shape("kb")
            close(s.volume(), 42.0, "edited definition shape volume")
        meshes = b.gltf(tag)
        if "kb" in meshes:
            fail(f"({tag}) the definition's Shape kb is placed itself (a glTF node with extras.id kb)")
        for eid, T in (("KI", T0), ("KI2", T2)):
            check_gltf_matches(meshes, eid, transform_points(T, s.V), vol, f"({tag}) instance {eid}",
                               euler=2, bodies=1)


SEQ = [dict(RECT, shape="d"), {"op": "pushpull", "shape": "d", "face": 1, "distance": 2.5},
       {"op": "offset", "shape": "d", "face": 1, "distance": 0.4},
       {"op": "pushpull", "shape": "d", "face": 7, "distance": -0.5},
       {"op": "move", "shape": "d", "faces": [1], "vector": [0, 0, 0.25]},
       {"op": "rotate", "shape": "d", "faces": list(range(1, 12)), "centre": [0, 0, 0], "axis": [0, 0, 1],
        "angle": 0.5, "copy": True, "array": 2},
       {"op": "rect", "shape": "f", "corner": [0, 0, 0], "u": [0, 1, 0], "v": [0, 0, 1]},
       {"op": "followme", "shape": "f", "face": 1, "path": [[0, 0, 0], [3, 0, 0], [3, 3, 0]]},
       {"op": "make_component", "shape": "f", "definition": "FD", "instance": "FI", "name": "Frame"}]


def g_atomic_determinism(exe, root):
    hs = []
    for k in range(2):
        b = Bim(exe, root, f"det{k}")
        b.sketch(SEQ, "sequence")
        hs.append(b.hash())
    if hs[0] != hs[1]:
        fail(f"the same command file in two fresh projects gives hashes {hs[0]} and {hs[1]}")
    h0 = hs[1]
    for what, cmds in (
            ("offset of a missing face", [{"op": "offset", "shape": "d", "face": 99, "distance": 0.1}]),
            ("followme of a missing face", [{"op": "followme", "shape": "f", "face": 42, "path": [[0, 0, 0], [1, 0, 0]]}]),
            ("move after a valid command, then a bad face id", [
                {"op": "move", "shape": "d", "faces": [1], "vector": [0, 0, 1]},
                {"op": "move", "shape": "d", "faces": [999], "vector": [0, 0, 1]}]),
            ("make_component of a missing shape", [
                {"op": "make_component", "shape": "nope", "definition": "ND", "instance": "NI", "name": "x"}])):
        r = b.sketch_rc(cmds)
        if r.returncode != 2:
            fail(f"{what}: bim sketch exited {r.returncode}, want 2")
        if b.hash() != h0:
            fail(f"{what}: the document changed although the file was rejected (not atomic)")


TESTS = [g_offset, g_followme, g_move, g_rotate_scale, g_paint, g_components, g_atomic_determinism]

if __name__ == "__main__":
    main(TESTS, "T2.11", sys.argv[1])
