"""Independent reconstruction of building solids from a JSON ops file, following the
"Derived geometry rules" of spec/schema.md, using manifold3d. Shared by the T2.7 judge
(copied verbatim into T2.8/T2.9). Nothing here comes from the code under test.

Rules implemented:
- Wall: centreline offset +-t/2 (t = sum of layer_thickness), extruded from
  level.elevation + base_offset by height; layers ordered from the left side looking from
  (x1,y1) to (x2,y2). A free end is square at the endpoint.
- L-join (endpoints coincide within 1e-6): each wall is cut by the mitre plane through the
  intersection of the two outer faces and the intersection of the two inner faces (for equal
  thicknesses this is the angle bisector through the joint).
- T-join (an endpoint lies on the interior of another wall's centreline): the abutting wall
  stops at the face of the through wall that faces it.
- Openings: box over [offset, offset+width] along the centreline from (x1,y1), full thickness,
  from wall base + sill up by height; subtracted from the wall and from every layer.
- Slab: boundary minus SlabOpenings, from level elevation down by thickness.
- Column: section_b (along x) x section_h (along y) centred on (x,y), base..top elevation.
- Beam: section_b wide (centred on the axis) x section_h deep, top at level elevation - z_offset.
"""
import json
import math

import manifold3d as mf
import numpy as np

JOIN_TOL = 1e-6


def load_ops(*paths):
    """Replay JSON ops files (spec/cli.md), in order, into {id: {"kind":..., "props":{...}}}."""
    ops = []
    for path in paths:
        with open(path) as f:
            ops += json.load(f)["ops"]
    ents = {}
    for op in ops:
        if op["op"] == "create":
            ents[op["id"]] = {"kind": op["kind"], "props": dict(op.get("props", {}))}
        elif op["op"] == "set":
            ents[op["id"]]["props"][op["key"]] = op["value"]
        elif op["op"] == "delete":
            del ents[op["id"]]
    return ents


def ref(v):
    assert isinstance(v, dict) and "ref" in v, v
    return v["ref"]


def _box_cs(pts):
    return mf.CrossSection([np.asarray(pts, dtype=float)])


def prism(poly2d, z0, h):
    """Vertical prism over a CCW polygon (list of (x,y)) from z0 up by h."""
    return mf.Manifold.extrude(_box_cs(poly2d), h).translate((0.0, 0.0, z0))


def half_space_trim(m, n2, origin2):
    """Keep the part of m with n2 . (p - origin2) >= 0 (n2 a horizontal 2D normal)."""
    n = np.array([n2[0], n2[1]], dtype=float)
    n /= np.linalg.norm(n)
    off = float(n @ np.asarray(origin2, dtype=float))
    return m.trim_by_plane((float(n[0]), float(n[1]), 0.0), off)


class Wall:
    def __init__(self, wid, e, ents):
        p = e["props"]
        self.id = wid
        self.a = np.array([p["x1"], p["y1"]], dtype=float)
        self.b = np.array([p["x2"], p["y2"]], dtype=float)
        self.L = float(np.linalg.norm(self.b - self.a))
        self.u = (self.b - self.a) / self.L
        self.left = np.array([-self.u[1], self.u[0]])
        wt = ents[ref(p["type"])]["props"]
        self.layers_t = [float(x) for x in wt["layer_thickness"]]
        self.layers_m = list(wt["layer_material"])
        self.t = sum(self.layers_t)
        lvl = ents[ref(p["level"])]["props"]
        self.z0 = float(lvl["elevation"]) + float(p.get("base_offset", 0.0))
        self.h = float(p["height"])

    def end(self, k):
        return self.a if k == 0 else self.b

    def inward(self, k):
        """Unit direction from end k into the wall body."""
        return self.u if k == 0 else -self.u


def _line_intersect(p, d, q, e):
    """Intersection of lines p + s d and q + r e."""
    m = np.array([[d[0], -e[0]], [d[1], -e[1]]])
    s, _ = np.linalg.solve(m, q - p)
    return p + s * d


def _on_interior(w, pt):
    v = pt - w.a
    s = float(v @ w.u)
    dist = abs(float(v @ w.left))
    return dist <= JOIN_TOL and JOIN_TOL < s < w.L - JOIN_TOL


def wall_end_cuts(walls):
    """For each wall, the list of (n2, origin2) half-planes to keep, from joins at its ends."""
    cuts = {w.id: [] for w in walls}
    joins = {w.id: [None, None] for w in walls}
    for w in walls:
        for k in (0, 1):
            P = w.end(k)
            for o in walls:
                if o is w:
                    continue
                for ko in (0, 1):
                    if np.linalg.norm(o.end(ko) - P) <= JOIN_TOL:
                        joins[w.id][k] = ("L", o, ko)
                if joins[w.id][k] is None and _on_interior(o, P):
                    joins[w.id][k] = ("T", o, None)
    for w in walls:
        for k in (0, 1):
            j = joins[w.id][k]
            if j is None:
                continue
            P = w.end(k)
            d1 = w.inward(k)
            if j[0] == "L":
                o, ko = j[1], j[2]
                d2 = o.inward(ko)
                # Face lines: each wall's two faces, offset +-t/2 from the centreline.
                n1 = np.array([-d1[1], d1[0]])
                n2 = np.array([-d2[1], d2[0]])
                # The side of wall w facing wall o is the "inner" side.
                s1 = 1.0 if n1 @ d2 > 0 else -1.0
                s2 = 1.0 if n2 @ d1 > 0 else -1.0
                inner = _line_intersect(P + s1 * n1 * w.t / 2, d1, P + s2 * n2 * o.t / 2, d2)
                outer = _line_intersect(P - s1 * n1 * w.t / 2, d1, P - s2 * n2 * o.t / 2, d2)
                m = outer - inner
                nm = np.array([-m[1], m[0]])
                if nm @ d1 < 0:
                    nm = -nm
                cuts[w.id].append((nm, inner, k, "L"))
            else:
                o = j[1]
                # Face of the through wall on the side the abutting wall lies.
                side = 1.0 if o.left @ d1 > 0 else -1.0
                face_pt = P + side * o.left * o.t / 2
                n = side * o.left
                cuts[w.id].append((n, face_pt, k, "T"))
    return cuts, joins


def _strip(w, lo, hi, ext0, ext1):
    """Plan rectangle of the wall between lateral offsets lo..hi (left positive), lengthened by
    ext0 before (x1,y1) and ext1 beyond (x2,y2)."""
    a = w.a - w.u * ext0
    b = w.b + w.u * ext1
    pts = [a + w.left * lo, b + w.left * lo, b + w.left * hi, a + w.left * hi]
    # CCW: left is +90deg from u, so (a,lo)->(b,lo)->(b,hi)->(a,hi) is CCW when hi > lo.
    return [tuple(p) for p in pts]


def _opening_prism(w, op):
    p = op["props"]
    s0 = float(p["offset"])
    s1 = s0 + float(p["width"])
    big = w.t * 4 + 1.0
    a = w.a + w.u * s0
    b = w.a + w.u * s1
    pts = [a - w.left * big, b - w.left * big, b + w.left * big, a + w.left * big]
    return prism([tuple(q) for q in pts], w.z0 + float(p["sill"]), float(p["height"]))


def build(ents):
    """Return dict with walls {id: {"solid", "layers": [(material, solid)], "t", "determinate"}},
    slabs {id: {"solid", "area"}}, columns {id: solid}, beams {id: solid}."""
    walls = [Wall(i, e, ents) for i, e in sorted(ents.items()) if e["kind"] == "Wall"]
    cuts, joins = wall_end_cuts(walls)
    openings = {}
    for i, e in ents.items():
        if e["kind"] == "Opening":
            openings.setdefault(ref(e["props"]["host"]), []).append(e)
    out = {"walls": {}, "slabs": {}, "columns": {}, "beams": {}, "joins": joins}
    for w in walls:
        ext = [0.0, 0.0]
        for (_, _, k, _) in cuts[w.id]:
            ext[k] = 4 * max(ww.t for ww in walls) + 1.0
        holes = [_opening_prism(w, op) for op in openings.get(w.id, [])]

        def finish(sol):
            for (n, origin, _, _) in cuts[w.id]:
                sol = half_space_trim(sol, n, origin)
            for hprism in holes:
                sol = sol - hprism
            return sol

        whole = finish(prism(_strip(w, -w.t / 2, w.t / 2, ext[0], ext[1]), w.z0, w.h))
        layers = []
        top = w.t / 2
        for lt, lm in zip(w.layers_t, w.layers_m):
            layers.append((lm, finish(prism(_strip(w, top - lt, top, ext[0], ext[1]), w.z0, w.h))))
            top -= lt
        # A wall's own split is pinned unless it has an L-join with a wall of different thickness.
        det = True
        for k in (0, 1):
            j = joins[w.id][k]
            if j and j[0] == "L" and abs(j[1].t - w.t) > 1e-12:
                det = False
        # Union reference used for undeterminate pairs: each wall run through to the far face.
        out["walls"][w.id] = {"solid": whole, "layers": layers, "t": w.t, "determinate": det,
                              "wall": w}
    for i, e in ents.items():
        p = e["props"]
        if e["kind"] == "Slab":
            elev = float(ents[ref(p["level"])]["props"]["elevation"])
            th = float(p["thickness"])
            cs = _poly_cs(p["boundary"])
            for j, f in ents.items():
                if f["kind"] == "SlabOpening" and ref(f["props"]["host"]) == i:
                    cs = cs - _poly_cs(f["props"]["boundary"])
            out["slabs"][i] = {"solid": mf.Manifold.extrude(cs, th).translate((0, 0, elev - th)),
                               "area": cs.area()}
        elif e["kind"] == "Column":
            zb = float(ents[ref(p["base"])]["props"]["elevation"])
            zt = float(ents[ref(p["top"])]["props"]["elevation"])
            x, y, bx, hy = float(p["x"]), float(p["y"]), float(p["section_b"]), float(p["section_h"])
            out["columns"][i] = prism([(x - bx / 2, y - hy / 2), (x + bx / 2, y - hy / 2),
                                       (x + bx / 2, y + hy / 2), (x - bx / 2, y + hy / 2)], zb, zt - zb)
        elif e["kind"] == "Beam":
            elev = float(ents[ref(p["level"])]["props"]["elevation"])
            top = elev - float(p.get("z_offset", 0.0))
            a = np.array([p["x1"], p["y1"]], float)
            b = np.array([p["x2"], p["y2"]], float)
            u = (b - a) / np.linalg.norm(b - a)
            lf = np.array([-u[1], u[0]]) * float(p["section_b"]) / 2
            out["beams"][i] = prism([tuple(a - lf), tuple(b - lf), tuple(b + lf), tuple(a + lf)],
                                    top - float(p["section_h"]), float(p["section_h"]))
    return out


def _poly_cs(flat):
    pts = np.asarray(flat, dtype=float).reshape(-1, 2)
    return mf.CrossSection([pts])


def run_through_union(model, ids):
    """Union of walls `ids` where each L-joined end between them is run through to the far face
    of the partner: the region an L-join must fill regardless of how the corner is split."""
    parts = []
    for wid in ids:
        rec = model["walls"][wid]
        w = rec["wall"]
        ext = [0.0, 0.0]
        for k in (0, 1):
            j = model["joins"][wid][k]
            if j and j[0] == "L" and j[1].id in ids:
                ext[k] = j[1].t / 2
        parts.append(prism(_strip(w, -w.t / 2, w.t / 2, ext[0], ext[1]), w.z0, w.h))
    u = parts[0]
    for p in parts[1:]:
        u = u + p
    return u


def all_solids(model):
    """{entity id: Manifold} for every element that must appear as a glTF mesh node."""
    s = {}
    for k in ("walls", "slabs"):
        for i, r in model[k].items():
            s[i] = r["solid"]
    s.update(model["columns"])
    s.update(model["beams"])
    return s
