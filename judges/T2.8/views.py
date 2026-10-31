"""Oracle side of the drawing views of spec/cli.md, computed from the manifold3d reconstruction.

View frames (paper: u to the right, v up, in model metres before the 1:100 scale):
- plan:<level>@<h>: horizontal cut plane z = level.elevation + h, looking down -z; u = x, v = y.
- section:x0,y0,x1,y1: vertical plane through the line, looking to the left of the direction
  d = (x1-x0, y1-y0); the viewer's right is d, so u = (p - p0).d, v = z.
- elevation:s looks north (+y), u = x; n looks south, u = -x; e looks west, u = y; w looks east,
  u = -y; v = z. No cut.
"""
import math

import manifold3d as mf
import numpy as np
import shapely
import trimesh
from shapely.geometry import Polygon
from shapely.ops import unary_union

import model


def parse_view(view, ents):
    kind, _, arg = view.partition(":")
    if kind == "plan":
        lvl, _, h = arg.partition("@")
        z = float(ents[lvl]["props"]["elevation"]) + float(h)
        return {"kind": "plan", "f": np.array([0, 0, -1.0]), "z": z,
                "uv": lambda p: (p[..., 0], p[..., 1])}
    if kind == "section":
        x0, y0, x1, y1 = (float(t) for t in arg.split(","))
        p0 = np.array([x0, y0])
        d = np.array([x1 - x0, y1 - y0]) / math.hypot(x1 - x0, y1 - y0)
        left = np.array([-d[1], d[0]])
        return {"kind": "section", "f": np.array([left[0], left[1], 0.0]), "p0": p0, "d": d,
                "uv": lambda p: ((p[..., 0] - p0[0]) * d[0] + (p[..., 1] - p0[1]) * d[1], p[..., 2])}
    if kind == "elevation":
        f, r = {"s": ((0, 1, 0), (1, 0, 0)), "n": ((0, -1, 0), (-1, 0, 0)),
                "e": ((-1, 0, 0), (0, 1, 0)), "w": ((1, 0, 0), (0, -1, 0))}[arg]
        r = np.array(r, float)
        return {"kind": "elevation", "f": np.array(f, float),
                "uv": lambda p: (p[..., 0] * r[0] + p[..., 1] * r[1], p[..., 2])}
    raise ValueError(view)


def _polys(cs):
    """CrossSection -> shapely geometry (even-odd over its contours, which manifold orients)."""
    g = Polygon()
    for c in cs.to_polygons():
        if len(c) >= 3:
            g = g.symmetric_difference(Polygon(c))
    return g


def cut_regions(solids, v):
    """{entity: shapely geometry in (u, v) metres} of the solids cut by the view's plane."""
    out = {}
    for eid, s in solids.items():
        if v["kind"] == "plan":
            cs = s.slice(v["z"])
            g = _polys(cs)
        elif v["kind"] == "section":
            d, p0 = v["d"], v["p0"]
            c = np.array([d[1], -d[0]])  # d x ez, so (d, ez, c) is right-handed
            m = np.array([[d[0], d[1], 0, -(d @ p0)],
                          [0, 0, 1, 0],
                          [c[0], c[1], 0, -(c @ p0)]], float)
            g = _polys(s.transform(m).slice(0.0))
        else:
            continue
        if g.area > 1e-12:
            out[eid] = g
    return out


def projected_bbox(solids, v):
    lo, hi = np.array([np.inf, np.inf]), np.array([-np.inf, -np.inf])
    for s in solids.values():
        m = s.to_mesh()
        p = np.asarray(m.vert_properties)[:, :3].astype(float)
        u, w = v["uv"](p)
        lo = np.minimum(lo, [u.min(), w.min()])
        hi = np.maximum(hi, [u.max(), w.max()])
    return lo, hi


class Scene:
    """Feature edges and a ray caster over all solids, for exact visibility of sample points."""

    def __init__(self, solids, v):
        self.v = v
        meshes, self.edges = [], []
        for eid, s in solids.items():
            m = s.to_mesh()
            tm = trimesh.Trimesh(np.asarray(m.vert_properties)[:, :3].astype(float),
                                 np.asarray(m.tri_verts).astype(np.int64), process=True)
            meshes.append(tm)
            ang = tm.face_adjacency_angles
            for (a, b), t in zip(tm.face_adjacency_edges, ang):
                if t > 1e-6:
                    self.edges.append((eid, tm.vertices[a].copy(), tm.vertices[b].copy()))
        self.union = mf.Manifold.batch_boolean(list(solids.values()), mf.OpType.Add)
        f = v["f"]
        e1 = np.cross(f, [0.0, 0.0, 1.0]) if abs(f[2]) < 0.9 else np.array([1.0, 0.0, 0.0])
        e1 /= np.linalg.norm(e1)
        self.offsets = [np.zeros(3)] + [s_ * self.DELTA * e for e in (e1, np.cross(f, e1)) for s_ in (1, -1)]
        P = np.array([[e[1], e[2]] for e in self.edges])
        u, w = v["uv"](P)
        self.e2 = np.stack([u, w], axis=-1)  # (n, 2 endpoints, 2)
        self.depth = P @ v["f"]               # (n, 2)

    def edges_at(self, q, tol, direction=None):
        """Edges whose projection passes within tol of q (optionally parallel to direction):
        list of (index, t along edge)."""
        a, b = self.e2[:, 0], self.e2[:, 1]
        ab = b - a
        L2 = (ab ** 2).sum(1)
        ok = L2 > 1e-12
        t = np.where(ok, ((q - a) * ab).sum(1) / np.where(ok, L2, 1), 0)
        t = np.clip(t, 0, 1)
        dist = np.linalg.norm(a + ab * t[:, None] - q, axis=1)
        idx = np.nonzero(ok & (dist <= tol))[0]
        if direction is not None:
            dn = direction / np.linalg.norm(direction)
            keep = []
            for i in idx:
                e = ab[i] / math.sqrt(L2[i])
                if abs(e[0] * dn[1] - e[1] * dn[0]) < 1e-3:
                    keep.append(i)
            idx = np.array(keep, dtype=int)
        return [(int(i), float(t[i])) for i in idx]

    DELTA = 1e-4   # m: lateral probe offset around an edge point
    EPS = 2e-3     # m: an occluder must be this far in front of the edge point

    def visible(self, i, t):
        """Visibility of the point at parameter t on edge i along the view direction: True or
        False when the centre ray and four rays offset by DELTA agree, None when they disagree
        (the point lies on an occluder's outline, where exact HLR may go either way)."""
        _, a, b = self.edges[i]
        p = a + (b - a) * t
        f = self.v["f"]
        depth = float(p @ f)
        res = set()
        for o in self.offsets:
            q = p + o
            hits = self.union.ray_cast(tuple(q - f * 1000.0), tuple(q + f * 1000.0))
            first = min((float(np.asarray(h.position) @ f) for h in hits), default=np.inf)
            res.add(first >= depth - self.EPS)
        return res.pop() if len(res) == 1 else None

    def point_depths(self, i, t):
        _, a, b = self.edges[i]
        return float((a + (b - a) * t) @ self.v["f"])
