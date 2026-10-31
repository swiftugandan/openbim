"""T2.10 oracle side: drive `bim sketch` / `bim show --json` / `bim export --format gltf` and compare every
Shape with closed forms recomputed here (spec/sketch.md, TASKS.md T2.10).

Usage: python -I check.py <path to bim>
Prints `JUDGE FAIL: <group>: <reason>` and exits 1 on the first failed check.
"""
import json
import math
import os
import sys

import numpy as np
import trimesh

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sk import (Bim, EPS, close, fail, main, norm, same_points, rot, mat, colmajor,  # noqa: E402
                transform_points, check_gltf_matches, pts_equal, GLTF_REL)

SPEC_EXAMPLE = [
    {"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]},
    {"op": "pushpull", "shape": "box", "face": 1, "distance": 2.5},
]


def renamed(cmds, name):
    return [dict(c, shape=name) for c in cmds]


def check_split_id(before, after, fid, what):
    """spec/sketch.md: when a face is split, the part containing its first outer-loop vertex keeps the id."""
    p = before.V[before.face(fid)[0][0]]
    keeper = [f for f in after.ids if any(np.linalg.norm(after.V[i] - p) <= 10 * EPS
                                          for lp in after.face(f) for i in lp)]
    if fid not in keeper:
        fail(f"{what}: face {fid} should be the part containing its first outer-loop vertex {p.tolist()}")
    others = [f for f in keeper if f != fid and f not in before.ids]
    if others:
        fail(f"{what}: the new part {others} contains face {fid}'s first outer-loop vertex {p.tolist()}")


def rect_pts(c, u, v):
    c, u, v = (np.asarray(x, float) for x in (c, u, v))
    return np.array([c, c + u, c + u + v, c + v])


def box(b, name):
    """The spec example under another name; returns (shape, n1, z_face1, z_other) with the face-1 normal
    read back from the rectangle (its orientation is the implementation's choice; the push follows it)."""
    b.sketch(renamed(SPEC_EXAMPLE[:1], name), "rect")
    s0 = b.shape(name)
    if s0.ids != [1]:
        fail(f"{name}: a rect on a new shape should make exactly one face with id 1, got ids {s0.ids}")
    n1 = s0.normals[1]
    if abs(abs(n1[2]) - 1) > 1e-12:
        fail(f"{name}: the rect face in z = 0 has normal {n1}, want ±z")
    b.sketch(renamed(SPEC_EXAMPLE[1:], name), "pushpull")
    s = b.shape(name)
    return s, n1, 2.5 * n1[2], 0.0


def check_box(s, n1, what, h=2.5):
    s.require_closed(what)
    if sorted(s.ids) != [1, 2, 3, 4, 5, 6]:
        fail(f"{what}: face ids {sorted(s.ids)}, want 1..6 (face 1 kept, five new faces 2..6)")
    close(s.volume(), 4 * 3 * h, f"{what}: signed volume (outward faces)")
    n, d = s.plane(1)
    if np.abs(n - n1).max() > 1e-12:
        fail(f"{what}: face 1 normal changed from {n1} to {n}")
    close(d, h, f"{what}: face 1 moved along its normal by the distance", 0, 1e-12)
    z1 = h * n1[2]
    same_points(s.V, [[x, y, z] for x in (0, 4) for y in (0, 3) for z in (0, z1)], f"{what}: box vertices")


# ------------------------------------------------------------------ groups

def g_spec_example(exe, root):
    """TASKS.md T2.10: after rect 4 x 3 and pushpull 2.5 the Shape is a closed solid of 30 m3 with 6 faces,
    applied as the single spec example file; then the glTF node."""
    b = Bim(exe, root, "spec")
    b.sketch(SPEC_EXAMPLE, "spec example")
    s = b.shape("box")
    s.require_closed("spec example")
    if len(s.faces) != 6:
        fail(f"spec example: {len(s.faces)} faces, want 6")
    close(s.volume(), 30.0, "spec example: volume")
    if sorted(s.ids) != [1, 2, 3, 4, 5, 6]:
        fail(f"spec example: face ids {sorted(s.ids)}, want 1..6")
    if s.euler() != 2:
        fail(f"spec example: V - E + F = {s.euler()}, want 2")
    meshes = b.gltf("spec")
    check_gltf_matches(meshes, "box", s.V, 30.0, "spec example", euler=2, bodies=1)


def g_rect_and_pushpull(exe, root):
    b = Bim(exe, root, "rects")
    s, n1, _, _ = box(b, "b")
    check_box(s, n1, "rect+pushpull")
    # Tilted planes: u and v are exactly perpendicular in f64 (integer directions, scaled), so a strict u ⟂ v
    # check in the implementation cannot reject the judge's own input.
    cases = [
        ([1, 2, 3], [3, 0, 0], [0, 4, 3], 2.0),
        ([-2.5, 1.25, 0.5], [1, 2, 2], [3, -1.5, 0], 1.75),          # (1,2,2)·(3,-1.5,0) = 0
        ([4, -3, 2.5], [1, -1.5, 3], [1.5, 1, 0], 0.4),            # (1,-1.5,3)·(1.5,1,0) = 0
        ([0.5, 0.5, -1], [-1, 1, 2], [4, 0, 2], 2.8),              # (-1,1,2)·(4,0,2) = 0
    ]
    for k, (c, u, v, d) in enumerate(cases):
        name = f"tilt{k}"
        b.sketch([{"op": "rect", "shape": name, "corner": c, "u": u, "v": v}], name)
        s = b.shape(name)
        what = f"rect in a tilted plane ({name})"
        if s.ids != [1] or s.loose:
            fail(f"{what}: want one face (id 1) and no loose edges, got ids {s.ids}, {len(s.loose)} loose edges")
        A = np.linalg.norm(u) * np.linalg.norm(v)
        close(s.area(1), A, f"{what}: face area |u||v|")
        same_points(s.points(1), rect_pts(c, u, v), f"{what}: face vertices")
        if s.is_closed():
            fail(f"{what}: a single face is an open surface, not a solid")
        n1 = s.normals[1]
        if abs(abs(n1 @ norm(np.cross(u, v))) - 1) > 1e-9:
            fail(f"{what}: face normal {n1} is not perpendicular to the rectangle's plane")
        b.sketch([{"op": "pushpull", "shape": name, "face": 1, "distance": d}], name)
        s = b.shape(name)
        s.require_closed(what + " after pushpull")
        close(s.volume(), A * d, f"{what}: pushpull volume")
        if sorted(s.ids) != [1, 2, 3, 4, 5, 6]:
            fail(f"{what}: face ids {sorted(s.ids)} after pushpull, want 1..6")
        same_points(s.points(1), rect_pts(c, u, v) + d * n1, f"{what}: face 1 after moving d along its normal")
        same_points(s.V, np.vstack([rect_pts(c, u, v), rect_pts(c, u, v) + d * n1]), f"{what}: prism vertices")
    meshes = b.gltf("rects")
    s = b.shape("tilt1")
    check_gltf_matches(meshes, "tilt1", s.V, s.volume(), "tilted prism", euler=2, bodies=1)


def polygon_pts(c, first, n, r, k):
    c, first, n = (np.asarray(x, float) for x in (c, first, n))
    t = np.cross(n, first)
    return np.array([c + r * (math.cos(2 * math.pi * i / k) * first + math.sin(2 * math.pi * i / k) * t)
                     for i in range(k)])


def g_circle(exe, root):
    b = Bim(exe, root, "circles")
    cases = [  # centre, normal, radius, segments (None = default 24), expected first direction
        ([1, 1, 0.5], [0, 0, 1], 1.5, None, [1, 0, 0]),
        ([2, -1, 3], [1, 0, 0], 0.8, 7, [0, 1, 0]),          # x̂ ∥ normal -> ŷ
        ([0, 0, 0], [1 / 3, 2 / 3, 2 / 3], 2.0, 12, None),   # x̂ projected onto a tilted plane (unit normal)
    ]
    for k, (c, n, r, seg, first) in enumerate(cases):
        name = f"circ{k}"
        cmd = {"op": "circle", "shape": name, "centre": c, "normal": n, "radius": r}
        if seg:
            cmd["segments"] = seg
        nn = norm(n)
        if first is None:
            x = np.array([1.0, 0, 0])
            first = norm(x - (x @ nn) * nn)
        kk = seg or 24
        b.sketch([cmd], name)
        s = b.shape(name)
        what = f"circle {name} ({kk} segments)"
        if s.ids != [1]:
            fail(f"{what}: want one face with id 1, got {s.ids}")
        want = polygon_pts(c, first, nn, r, kk)
        same_points(s.points(1), want, f"{what}: polygon vertices (first along x̂ projected)")
        close(s.area(1), kk / 2 * r * r * math.sin(2 * math.pi / kk), f"{what}: polygon area")
        b.sketch([{"op": "pushpull", "shape": name, "face": 1, "distance": 1.25}], name)
        s = b.shape(name)
        s.require_closed(what + " after pushpull")
        if len(s.faces) != kk + 2:
            fail(f"{what}: prism has {len(s.faces)} faces, want {kk + 2}")
        close(s.volume(), 1.25 * kk / 2 * r * r * math.sin(2 * math.pi / kk), f"{what}: prism volume")
    meshes = b.gltf("circles")
    s = b.shape("circ0")
    check_gltf_matches(meshes, "circ0", s.V, s.volume(), "24-gon prism", euler=2, bodies=1)


def g_arc(exe, root):
    b = Bim(exe, root, "arcs")
    c, r = np.array([5.0, 5, 0]), 2.0
    b.sketch([{"op": "arc", "shape": "a", "centre": c.tolist(), "start": [7, 5, 0], "normal": [0, 0, 1],
               "angle": math.pi}], "arc")
    s = b.shape("a")
    want = [c + r * np.array([math.cos(k * math.pi / 12), math.sin(k * math.pi / 12), 0]) for k in range(13)]
    if s.faces:
        fail(f"arc: an open polyline makes no face, got {len(s.faces)}")
    if len(s.loose) != 12:
        fail(f"arc: want 12 loose edges (default segments), got {len(s.loose)}")
    same_points(s.V, want, "arc: polyline vertices (right-hand about normal)")
    b.sketch([{"op": "line", "shape": "a", "from": [3, 5, 0], "to": [7, 5, 0]}], "closing line")
    s = b.shape("a")
    if s.ids != [1] or s.loose:
        fail(f"arc closed by a chord: want one face id 1 and no loose edges, got ids {s.ids}, "
             f"{len(s.loose)} loose edges")
    close(s.area(1), 6 * r * r * math.sin(math.pi / 12), "half-disc polygon area")
    # Negative angle with 3 segments in a vertical plane: clockwise about the normal.
    b.sketch([{"op": "arc", "shape": "a2", "centre": [0, 0, 1], "start": [0, 1, 1], "normal": [1, 0, 0],
               "angle": -math.pi / 2, "segments": 3}], "arc 2")
    s = b.shape("a2")
    want = [[0, math.cos(-k * math.pi / 6), 1 + math.sin(-k * math.pi / 6)] for k in range(4)]
    if len(s.loose) != 3 or s.faces:
        fail(f"arc 2: want 3 loose edges and no face, got {len(s.loose)} / {len(s.faces)}")
    same_points(s.V, want, "arc 2 vertices (negative angle)")


def g_lines(exe, root):
    b = Bim(exe, root, "lines")
    # Triangle in a tilted plane; the closing point differs by 4e-10 m (equal within 1e-9 m).
    P = [np.array(p) for p in ([1.0, 0, 0], [0, 2.0, 0], [0, 0, 3.0])]
    b.sketch([
        {"op": "line", "shape": "tri", "from": P[0].tolist(), "to": P[1].tolist()},
        {"op": "line", "shape": "tri", "from": P[1].tolist(), "to": P[2].tolist()},
        {"op": "line", "shape": "tri", "from": P[2].tolist(), "to": (P[0] + [4e-10, 0, 0]).tolist()},
    ], "triangle")
    s = b.shape("tri")
    if s.ids != [1] or s.loose or len(s.V) != 3:
        fail(f"triangle from 3 lines: want 1 face, 3 vertices, no loose edges; got ids {s.ids}, "
             f"{len(s.V)} vertices, {len(s.loose)} loose")
    close(s.area(1), 0.5 * np.linalg.norm(np.cross(P[1] - P[0], P[2] - P[0])), "triangle area", 1e-8)

    # Two unit squares sharing an edge in the plane z = 1, drawn in two orders: smallest regions only.
    sq = [([0, 0], [2, 0]), ([2, 0], [2, 1]), ([2, 1], [0, 1]), ([0, 1], [0, 0]), ([1, 0], [1, 1])]
    for name, order in (("grid", sq), ("grid2", sq[4:] + sq[:4])):
        b.sketch([{"op": "line", "shape": name, "from": a + [1.0], "to": c + [1.0]} for a, c in order], name)
        s = b.shape(name)
        if sorted(s.ids) != [1, 2] or s.loose:
            fail(f"{name}: two adjacent unit squares should be 2 faces (ids 1, 2) and no loose edges, got "
                 f"ids {sorted(s.ids)}, {len(s.loose)} loose edges")
        for f in s.ids:
            close(s.area(f), 1.0, f"{name}: face {f} area")
        same_points(s.V, [[x, y, 1] for x in (0, 1, 2) for y in (0, 1)], f"{name}: vertices (edges split)")

    # A non-planar closed loop of 4 lines makes no face.
    Q = [[0, 0, 0], [1, 0, 0], [1, 1, 0.5], [0, 1, 0]]
    b.sketch([{"op": "line", "shape": "skew", "from": Q[i], "to": Q[(i + 1) % 4]} for i in range(4)], "skew")
    s = b.shape("skew")
    if s.faces or len(s.loose) != 4:
        fail(f"non-planar loop: want no face and 4 loose edges, got {len(s.faces)} faces, {len(s.loose)} loose")

    # A line across a rectangle splits the edges it crosses and the face it joins; the outside parts stay loose.
    b.sketch([{"op": "rect", "shape": "cross", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]}], "cross rect")
    s0 = b.shape("cross")
    b.sketch([{"op": "line", "shape": "cross", "from": [2, -1, 0], "to": [2, 4, 0]}], "cross")
    s = b.shape("cross")
    check_split_id(s0, s, 1, "line across a face")
    if sorted(s.ids) != [1, 2]:
        fail(f"line across a face: face ids {sorted(s.ids)}, want [1, 2] (one new face)")
    for f in s.ids:
        close(s.area(f), 6.0, f"line across a face: face {f} area")
    if len(s.loose) != 2:
        fail(f"line across a face: want the 2 outside parts as loose edges, got {len(s.loose)}")
    same_points(s.V, [[0, 0, 0], [4, 0, 0], [4, 3, 0], [0, 3, 0], [2, -1, 0], [2, 0, 0], [2, 3, 0], [2, 4, 0]],
                "line across a face: vertices")
    # Erasing a loose edge removes just that edge.
    i, j = s.find([2, -1, 0]), s.find([2, 0, 0])
    b.sketch([{"op": "erase", "shape": "cross", "edges": [[i, j]]}], "erase loose edge")
    s = b.shape("cross")
    if len(s.loose) != 1 or sorted(s.ids) != [1, 2]:
        fail(f"erase of a loose edge: want 1 loose edge left and faces [1, 2], got {len(s.loose)} / {sorted(s.ids)}")
    # A line between two boundary points of a tilted face splits it.
    c, u, v = np.array([1.0, 1, 1]), np.array([0, 3.0, 0]), np.array([2.0, 0, 2.0])
    b.sketch([{"op": "rect", "shape": "tsplit", "corner": c.tolist(), "u": u.tolist(), "v": v.tolist()}], "tilted rect")
    s0 = b.shape("tsplit")
    b.sketch([{"op": "line", "shape": "tsplit", "from": (c + u / 3).tolist(), "to": (c + u / 3 + v).tolist()}],
             "tilted split")
    s = b.shape("tsplit")
    check_split_id(s0, s, 1, "tilted split")
    A = 3 * math.sqrt(8)
    if sorted(s.ids) != [1, 2]:
        fail(f"tilted split: face ids {sorted(s.ids)}, want [1, 2]")
    areas = sorted(s.area(f) for f in s.ids)
    close(areas[0], A / 3, "tilted split: smaller face area")
    close(areas[1], 2 * A / 3, "tilted split: larger face area")


def top_rect(s, n1, x0, y0, w, h):
    """A rectangle on face 1's plane (z = top) given in plan."""
    z = s.plane(1)[1] * n1[2]
    return {"corner": [x0, y0, z], "u": [w, 0, 0], "v": [0, h, 0]}, z


def g_pushpull_solids(exe, root):
    b = Bim(exe, root, "pp")
    # Push in: -1 on face 1 of the box.
    s, n1, _, _ = box(b, "neg")
    b.sketch([{"op": "pushpull", "shape": "neg", "face": 1, "distance": -1}], "push in")
    s = b.shape("neg")
    check_box(s, n1, "push in by 1", h=1.5)

    # Split the top with a line and pull one half up by 1: 30 + 2*3*1.
    s0, n1, _, _ = box(b, "half")
    z = s0.plane(1)[1] * n1[2]
    b.sketch([{"op": "line", "shape": "half", "from": [2, 0, z], "to": [2, 3, z]}], "split top")
    s = b.shape("half")
    check_split_id(s0, s, 1, "split top")
    s.require_closed("box with split top")
    close(s.volume(), 30.0, "box with split top: volume")
    tops = [f for f in s.ids if abs(s.plane(f)[1] - 2.5) < 1e-9 and np.abs(s.normals[f] - n1).max() < 1e-9]
    if len(tops) != 2 or 1 not in tops or len(s.ids) != 7 or sorted(s.ids) != list(range(1, 8)):
        fail(f"split top: want 7 faces with ids 1..7 and two top faces incl. 1, got ids {sorted(s.ids)}, tops {tops}")
    for f in tops:
        close(s.area(f), 6.0, f"split top: face {f} area")
    right = [f for f in tops if s.centroid(f)[0] > 2][0]
    b.sketch([{"op": "pushpull", "shape": "half", "face": right, "distance": 1}], "pull half")
    s = b.shape("half")
    s.require_closed("pulled half")
    close(s.volume(), 36.0, "pulled half: volume")
    close(s.plane(right)[1], 3.5, "pulled half: the moved face keeps its id and moves 1 along its normal", 0, 1e-12)
    # The three perpendicular neighbours are stretched (ids kept); only the edge on the coplanar half gets a new
    # side face, and coplanar faces never merge on their own.
    if sorted(s.ids) != list(range(1, 9)):
        fail(f"pulled half: face ids {sorted(s.ids)}, want 1..8 (perpendicular neighbours stretched, one new side)")
    if abs(s.normals[8] @ n1) > 1e-9 or abs(abs(s.normals[8][0]) - 1) > 1e-9 or abs(s.plane(8)[1] * s.normals[8][0] - 2) > 1e-9:
        fail("pulled half: the new face 8 should be the side at x = 2")
    close(s.area(8), 3.0, "pulled half: new side face area 3 x 1")

    # Pulling the whole top of a box stretches all four sides: no new faces.
    s, n1, _, _ = box(b, "grow")
    b.sketch([{"op": "pushpull", "shape": "grow", "face": 1, "distance": 1}], "pull top")
    check_box(b.shape("grow"), n1, "pull the box top by 1 (sides stretched)", h=3.5)

    # Pocket: a rect drawn on the top makes an inner face; push it in by 1 (genus 0) ...
    s, n1, _, _ = box(b, "pocket")
    r, z = top_rect(s, n1, 1, 1, 2, 1)
    b.sketch([dict(op="rect", shape="pocket", **r)], "inner rect")
    s = b.shape("pocket")
    if sorted(s.ids) != list(range(1, 8)):
        fail(f"rect inside the top face: face ids {sorted(s.ids)}, want 1..7 (one new face, id 7)")
    s.require_closed("box with an inner rect on top")
    close(s.volume(), 30.0, "box with an inner rect on top: volume")
    close(s.area(7), 2.0, "inner face 7 area")
    if np.abs(s.normals[7] - n1).max() > 1e-9:
        fail(f"inner face 7 normal {s.normals[7]} should face like the top face {n1} (consistent orientation)")
    close(s.area(1), 10.0, "top face 1 area after the inner rect (12 - 2)")
    b.sketch([{"op": "pushpull", "shape": "pocket", "face": 7, "distance": -1}], "pocket")
    s = b.shape("pocket")
    s.require_closed("pocket")
    close(s.volume(), 28.0, "pocket: volume")
    if s.euler() != 2:
        fail(f"pocket: V - E + F - H = {s.euler()}, want 2 (genus 0)")
    if len(s.ids) != 11:
        fail(f"pocket: {len(s.ids)} faces, want 11 (6 + moved face + 4 new sides)")
    close(s.plane(7)[1], 1.5, "pocket: face 7 moved 1 into the solid", 0, 1e-12)

    # ... and punch it through (distance = height): both faces go, leaving a hole (genus 1).
    s, n1, _, _ = box(b, "punch")
    r, z = top_rect(s, n1, 1, 1, 2, 1)
    b.sketch([dict(op="rect", shape="punch", **r), {"op": "pushpull", "shape": "punch", "face": 7, "distance": -2.5}],
             "punch-through")
    s = b.shape("punch")
    s.require_closed("punch-through")
    close(s.volume(), 25.0, "punch-through: volume (30 - 2 x 1 x 2.5)")
    if s.euler() != 0:
        fail(f"punch-through: V - E + F - H = {s.euler()}, want 0 (a hole through the solid, genus 1)")
    if len(s.ids) != 10 or not set(range(1, 7)) <= set(s.ids):
        fail(f"punch-through: face ids {sorted(s.ids)}, want faces 1..6 kept (shrunk to rings) plus 4 new sides")
    for f in s.ids:
        if f not in range(1, 7) and abs(s.normals[f] @ n1) > 1e-9:
            fail(f"punch-through: face {f} is not a side of the hole (normal {s.normals[f]})")
    for f in (1,):
        if len(s.face(f)) != 2:
            fail(f"punch-through: face {f} should be a ring (2 loops), has {len(s.face(f))}")
        close(s.area(f), 10.0, f"punch-through: ring face {f} area")
    bottom = [f for f in range(1, 7) if np.abs(s.normals[f] + n1).max() < 1e-9]
    if len(bottom) != 1 or len(s.face(bottom[0])) != 2:
        fail("punch-through: the opposite face should remain as a ring with the hole")
    close(s.area(bottom[0]), 10.0, "punch-through: opposite ring face area")

    # new: true keeps the original face and adds the moved copy as a new face.
    b.sketch([{"op": "rect", "shape": "lone", "corner": [0, 5, 0], "u": [2, 0, 0], "v": [0, 1, 0]}], "lone rect")
    s0 = b.shape("lone")
    n = s0.normals[1]
    b.sketch([{"op": "pushpull", "shape": "lone", "face": 1, "distance": 1.5, "new": True}], "pushpull new")
    s = b.shape("lone")
    if sorted(s.ids) != list(range(1, 7)):
        fail(f"pushpull new: face ids {sorted(s.ids)}, want 1..6 (original 1 stays, 5 new faces)")
    same_points(s.points(1), s0.points(1), "pushpull new: face 1 stays where it was")
    moved = [f for f in s.ids if f != 1 and pts_equal(s.points(f), s0.points(1) + 1.5 * n)]
    if len(moved) != 1:
        fail("pushpull new: no new face equal to face 1 moved 1.5 along its normal")
    if len(s.V) != 8:
        fail(f"pushpull new: want the 8 prism vertices, got {len(s.V)}")


def g_erase(exe, root):
    b = Bim(exe, root, "erase")
    s, n1, _, _ = box(b, "m")
    z = s.plane(1)[1] * n1[2]
    b.sketch([{"op": "line", "shape": "m", "from": [2, 0, z], "to": [2, 3, z]}], "split top")
    s = b.shape("m")
    if sorted(s.ids) != list(range(1, 8)):
        fail(f"split top: face ids {sorted(s.ids)}, want 1..7")
    i, j = s.find([2, 0, z]), s.find([2, 3, z])
    b.sketch([{"op": "erase", "shape": "m", "edges": [[i, j]]}], "erase split")
    s = b.shape("m")
    if sorted(s.ids) != list(range(1, 7)):
        fail(f"erase between coplanar faces 1 and 7: face ids {sorted(s.ids)}, want 1..6 (merged face keeps 1)")
    close(s.area(1), 12.0, "merged face 1 area")
    s.require_closed("box after the merge")
    close(s.volume(), 30.0, "box after the merge: volume")

    # Erasing a box edge removes the two faces it bounds.
    s, n1, _, _ = box(b, "e2")
    z = 2.5 * n1[2]
    i, j = s.find([0, 0, 0]), s.find([0, 0, z])
    bounded = [f for f in s.ids if any(
        {i, j} == {lp[k], lp[(k + 1) % len(lp)]} for lp in s.face(f) for k in range(len(lp)))]
    if len(bounded) != 2:
        fail(f"box edge should bound exactly 2 faces, found {bounded}")
    b.sketch([{"op": "erase", "shape": "e2", "edges": [[j, i]]}], "erase box edge")
    s2 = b.shape("e2")
    if sorted(s2.ids) != sorted(set(range(1, 7)) - set(bounded)):
        fail(f"erase box edge: face ids {sorted(s2.ids)}, want {sorted(set(range(1, 7)) - set(bounded))}")
    if s2.is_closed():
        fail("erase box edge: the remaining 4 faces are an open surface, not a solid")


def g_atomic(exe, root):
    b = Bim(exe, root, "atomic")
    b.sketch(SPEC_EXAMPLE, "spec example")
    h0 = b.hash()
    s0 = b.shape("box")
    bad = [
        ("pushpull on a missing face", [
            {"op": "rect", "shape": "zz", "corner": [0, 0, 0], "u": [1, 0, 0], "v": [0, 1, 0]},
            {"op": "pushpull", "shape": "box", "face": 1, "distance": 1},
            {"op": "pushpull", "shape": "box", "face": 99, "distance": 1}]),
        ("unknown op", [
            {"op": "pushpull", "shape": "box", "face": 1, "distance": 1},
            {"op": "extrude-everything", "shape": "box"}]),
        ("rect with u not perpendicular to v", [
            {"op": "pushpull", "shape": "box", "face": 1, "distance": 1},
            {"op": "rect", "shape": "box", "corner": [9, 9, 9], "u": [1, 0, 0], "v": [1, 1, 0]}]),
        ("erase of an edge that does not exist", [
            {"op": "pushpull", "shape": "box", "face": 1, "distance": 1},
            {"op": "erase", "shape": "box", "edges": [[0, 9999]]}]),
    ]
    for what, cmds in bad:
        r = b.sketch_rc(cmds)
        if r.returncode != 2:
            fail(f"{what}: bim sketch exited {r.returncode}, want 2 (invalid input)")
        if b.hash() != h0:
            fail(f"{what}: the document changed although the file was rejected (not atomic)")
    for what, raw in (("malformed JSON", '{"schema": 1, "commands": [ {"op": "rect"'),
                      ("no commands list", '{"schema": 1}')):
        r = b.sketch_rc(None, raw=raw)
        if r.returncode != 2:
            fail(f"{what}: bim sketch exited {r.returncode}, want 2")
        if b.hash() != h0:
            fail(f"{what}: the document changed")
    s1 = b.shape("box")
    if s1.props != s0.props:
        fail("rejected files changed the Shape")


SEQ = SPEC_EXAMPLE + [
    {"op": "line", "shape": "box", "from": [2, 0, 2.5], "to": [2, 3, 2.5]},
    {"op": "circle", "shape": "c", "centre": [10, 0, 0], "normal": [0, 0.6, 0.8], "radius": 1},
    {"op": "pushpull", "shape": "c", "face": 1, "distance": 2},
    {"op": "arc", "shape": "a", "centre": [0, 10, 0], "start": [1, 10, 0], "normal": [0, 0, 1], "angle": 2},
]


def g_determinism(exe, root):
    hs, shows = [], []
    for k in range(2):
        b = Bim(exe, root, f"det{k}")
        b.sketch(SEQ, "sequence")
        hs.append(b.hash())
        shows.append([b.show(e)["props"] for e in ("box", "c", "a")])
    if hs[0] != hs[1]:
        fail(f"the same command file in two fresh projects gives hashes {hs[0]} and {hs[1]}")
    if shows[0] != shows[1]:
        fail("the same command file in two fresh projects gives different Shapes")


def g_gltf(exe, root):
    """glTF: one node per Shape with extras.id; closed Shapes as closed meshes with exact volume, placed by the
    Shape's transform (local frame -> model frame)."""
    b = Bim(exe, root, "gltf")
    b.sketch(renamed(SPEC_EXAMPLE, "placed") + [
        {"op": "rect", "shape": "open", "corner": [0, 0, 5], "u": [1, 0, 0], "v": [0, 2, 0]},
        {"op": "rect", "shape": "punched", "corner": [20, 0, 0], "u": [3, 0, 0], "v": [0, 3, 0]},
        {"op": "pushpull", "shape": "punched", "face": 1, "distance": 1}], "gltf scene")
    s = b.shape("punched")
    n1 = s.normals[1]
    z = s.plane(1)[1] * n1[2]
    b.sketch([{"op": "rect", "shape": "punched", "corner": [21, 1, z], "u": [1, 0, 0], "v": [0, 1, 0]},
              {"op": "pushpull", "shape": "punched", "face": 7, "distance": -1}], "punch")
    M = np.eye(4)
    M[:3, :3] = rot([0, 0, 1], math.radians(30)) @ rot([1, 0, 0], 0.2)
    M[:3, 3] = [10, 20, 1]
    b.apply([{"op": "set", "id": "placed", "key": "transform", "value": colmajor(M)}], "set transform")
    meshes = b.gltf("scene")
    for eid in ("placed", "open", "punched"):
        if eid not in meshes:
            fail(f"glTF has no mesh node with extras.id {eid!r} (one node per Shape)")
    sp = b.shape("placed")
    check_gltf_matches(meshes, "placed", transform_points(M, sp.V), 30.0, "Shape under its transform", 2, 1)
    s = b.shape("punched")
    check_gltf_matches(meshes, "punched", s.V, 8.0, "punched Shape", euler=0, bodies=1)
    close(s.volume(), 8.0, "punched Shape volume")
    # An open surface: one node of its triangulated faces, not closed.
    v, f = meshes["open"]
    tm = trimesh.Trimesh(vertices=v, faces=f, process=True)
    tm.merge_vertices(digits_vertex=6)
    close(float(tm.area), 2.0, "open-surface Shape: glTF area = its face area (triangulated faces, single-sided)",
          GLTF_REL, 1e-7)
    if tm.is_watertight:
        fail("open-surface Shape: its glTF mesh should not be closed")
    same_points(np.unique(np.round(tm.vertices, 6), axis=0), [[0, 0, 5], [1, 0, 5], [1, 2, 5], [0, 2, 5]],
                "open-surface Shape: glTF vertices", tol=1e-5)


TESTS = [g_spec_example, g_rect_and_pushpull, g_circle, g_arc, g_lines, g_pushpull_solids, g_erase,
         g_atomic, g_determinism, g_gltf]

if __name__ == "__main__":
    main(TESTS, "T2.10", sys.argv[1])
