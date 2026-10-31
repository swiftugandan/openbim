"""Oracle for the T2.6 judge (bim_geom::hlr). Builders must not edit anything under judges/.

  oracle.py gen <scenes.txt> <expected.json>    write the scene file and the oracle's scene data
  oracle.py check <expected.json> <out.jsonl>   check the driver's visible/hidden segments
  oracle.py selftest <expected.json>            feed the checker an oracle-made answer (must pass) and
                                                corrupted ones (must fail)

Scenes are pairwise-disjoint convex prisms (touching allowed), so every edge is a feature edge and the oracle knows the exact edge set.
Checks per scene, all independent of the code under test:
  * every output segment lies on the projection of a scene edge and is attributed to a solid owning
    that edge; per 3D edge line the segments do not overlap (collinear overlaps merged) and cover
    the projected edge exactly (nothing missing);
  * visibility: rays are cast towards the viewer from 7 points inside every segment and intersected
    exactly with each convex prism's half-spaces (trimesh ray casting is used as a cross-check in
    selftest); a segment must be uniformly visible or hidden, so splits happen at crossings;
  * closed forms: for an isolated cube the visible length is the projected length of the edges
    adjacent to a front-facing face and the hidden length is the rest; for a box completely behind
    another, all of its edges are hidden.
Projection (the T2.6 spec gap): right = normalize(dir x up), v = right x normalize(dir),
2D point = (p . right, p . v).
"""
import json
import math
import sys

import numpy as np
from shapely.geometry import MultiPoint, Polygon
from shapely.geometry.polygon import orient

FRACS = [0.02, 0.1, 0.3, 0.5, 0.7, 0.9, 0.98]


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def fmt(x):
    return repr(float(x))


def frame(d, up):
    d = np.asarray(d, float) / np.linalg.norm(d)
    r = np.cross(d, up)
    r /= np.linalg.norm(r)
    v = np.cross(r, d)
    return d, r, v


# ---------------------------------------------------------------- scenes
def prism_data(z0, h, poly):
    """Half-spaces n.x <= c and 3D edges of a convex CCW prism."""
    P = np.asarray(poly, float)
    k = len(P)
    hs = [(np.array([0, 0, -1.0]), -z0), (np.array([0, 0, 1.0]), z0 + h)]
    edges = []
    for i in range(k):
        a, b = P[i], P[(i + 1) % k]
        e = b - a
        n = np.array([e[1], -e[0], 0.0])
        n /= np.linalg.norm(n)
        hs.append((n, float(n[:2] @ a)))
        edges.append(((a[0], a[1], z0), (b[0], b[1], z0)))
        edges.append(((a[0], a[1], z0 + h), (b[0], b[1], z0 + h)))
        edges.append(((a[0], a[1], z0), (a[0], a[1], z0 + h)))
    faces = [(np.array([0, 0, -1.0]), "bottom", None), (np.array([0, 0, 1.0]), "top", None)]
    return hs, edges, faces


def box(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def convex_poly(rng, cx, cy, r):
    while True:
        pts = [(round(cx + r * math.cos(a) * s, 6), round(cy + r * math.sin(a) * s, 6))
               for a, s in zip(rng.uniform(0, 2 * math.pi, 8), rng.uniform(0.5, 1, 8))]
        hull = MultiPoint(pts).convex_hull
        if hull.geom_type != "Polygon":
            continue
        hull = list(orient(hull, 1.0).exterior.coords)[:-1]
        if len(hull) < 3:
            continue
        ok = True
        for i in range(len(hull)):
            a, b, c = map(np.array, (hull[i - 1], hull[i], hull[(i + 1) % len(hull)]))
            u, w = b - a, c - b
            if (u[0] * w[1] - u[1] * w[0]) / (np.linalg.norm(u) * np.linalg.norm(w)) < 0.05 or np.linalg.norm(u) < 0.05:
                ok = False
        if ok:
            return hull


def build():
    scenes = []
    d0 = (-0.31, -0.52, -0.79)
    up = (0, 0, 1)
    scenes.append(("cube", d0, up, [(0, 1, box(0, 0, 1, 1))]))
    scenes.append(("stacked_boxes", d0, up, [(0, 1, box(0, 0, 1, 1)), (1, 1, box(0, 0, 1, 1))]))
    d, _, _ = frame(d0, up)
    c = np.array([0.5, 0.5, 0.5]) - 6 * d  # a big box towards the viewer hides the small one
    scenes.append(("fully_occluded", d0, up, [
        (0, 1, box(0, 0, 1, 1)),
        (round(c[2] - 2, 6), 4, box(round(c[0] - 2, 6), round(c[1] - 2, 6), round(c[0] + 2, 6), round(c[1] + 2, 6)))]))
    scenes.append(("partial_occlusion", (-0.6, -0.45, -0.66), up, [
        (0, 2, box(0, 0, 1, 1)), (0, 1, [(2.2, 1.6), (3.4, 1.9), (2.6, 2.9)]), (0.5, 0.7, box(1.6, -1.5, 4.5, -0.8))]))
    scenes.append(("floating_over", (-0.47, -0.33, -0.82), up, [
        (0, 1, box(0, 0, 2, 2)), (1.5, 1.2, [(1.4, 1.3), (2.9, 1.1), (3.2, 2.6), (1.7, 2.4)])]))
    rng = np.random.default_rng(20250206)
    for i in range(6):
        n = int(rng.integers(4, 7))
        solids = []
        while len(solids) < n:  # pairwise disjoint (footprints or z ranges apart): no piercing edges
            cand = (round(rng.uniform(0, 2.5), 6), round(rng.uniform(0.5, 2), 6),
                    convex_poly(rng, rng.uniform(-3, 3), rng.uniform(-3, 3), rng.uniform(0.6, 1.6)))
            if all(Polygon(cand[2]).distance(Polygon(o[2])) > 0.05
                   or cand[0] > o[0] + o[1] + 0.05 or o[0] > cand[0] + cand[1] + 0.05 for o in solids):
                solids.append(cand)
        az, el = rng.uniform(0, 2 * math.pi), rng.uniform(0.15, 1.2)
        dv = (-math.cos(el) * math.cos(az), -math.cos(el) * math.sin(az), -math.sin(el))
        scenes.append((f"random{i}", tuple(round(x, 6) for x in dv), up, solids))
    return scenes


# ---------------------------------------------------------------- geometry of a scene
class Scene:
    def __init__(self, name, d, up, solids):
        self.name = name
        self.d, self.r, self.v = frame(d, up)
        self.solids = solids
        self.hs, self.edges = [], []  # edges: (solid, p0, p1)
        for k, (z0, h, poly) in enumerate(solids):
            hs, edges, _ = prism_data(z0, h, poly)
            self.hs.append(hs)
            self.edges += [(k, np.array(a, float), np.array(b, float)) for a, b in edges]
        self.scale = max(1.0, max(np.abs(np.r_[e[1], e[2]]).max() for e in self.edges))
        # Group edges by their 3D supporting line.
        self.groups = []  # {o, t, members: [(solid, s0, s1)], union: [(s0, s1)]}
        for k, a, b in self.edges:
            t = (b - a) / np.linalg.norm(b - a)
            for g in self.groups:
                if abs(abs(g["t"] @ t) - 1) < 1e-12 and np.linalg.norm(np.cross(a - g["o"], g["t"])) < 1e-9:
                    break
            else:
                g = {"o": a, "t": t, "members": []}
                self.groups.append(g)
            s0, s1 = sorted(((a - g["o"]) @ g["t"], (b - g["o"]) @ g["t"]))
            g["members"].append((k, s0, s1))
        for g in self.groups:
            iv = sorted((s0, s1) for _, s0, s1 in g["members"])
            u = [list(iv[0])]
            for s0, s1 in iv[1:]:
                if s0 <= u[-1][1] + 1e-12:
                    u[-1][1] = max(u[-1][1], s1)
                else:
                    u.append([s0, s1])
            g["union"] = u
            g["o2"] = self.proj(g["o"])
            g["t2"] = self.proj(g["t"])

    def proj(self, p):
        return np.array([p @ self.r, p @ self.v])

    def generic(self):
        """The view must not make distinct edge lines project onto each other or to points."""
        for g in self.groups:
            assert np.linalg.norm(g["t2"]) > 1e-3, (self.name, "edge seen end-on")
        for i, g in enumerate(self.groups):
            for h in self.groups[i + 1:]:
                n2 = np.array([-g["t2"][1], g["t2"][0]]) / np.linalg.norm(g["t2"])
                par = abs(n2 @ h["t2"]) / np.linalg.norm(h["t2"]) < 1e-6
                if par and abs(n2 @ (h["o2"] - g["o2"])) < 1e-6:
                    raise AssertionError((self.name, "two edge lines project onto one line"))

    def occluded(self, p):
        w = -self.d
        for hs in self.hs:
            lo, hi = 0.0, math.inf
            for n, c in hs:
                a, b = n @ w, c - n @ p
                if abs(a) < 1e-14:
                    if b <= 1e-12:
                        lo, hi = 1.0, 0.0
                        break
                elif a > 0:
                    hi = min(hi, b / a)
                else:
                    lo = max(lo, b / a)
            if hi - lo > 1e-9:
                return True
        return False

    def locate(self, q0, q1):
        """Group and 3D-line parameters of a 2D output segment, or None."""
        for gi, g in enumerate(self.groups):
            t2 = g["t2"]
            nt = np.linalg.norm(t2)
            n2 = np.array([-t2[1], t2[0]]) / nt
            if abs(n2 @ (q0 - g["o2"])) < 1e-7 * self.scale and abs(n2 @ (q1 - g["o2"])) < 1e-7 * self.scale:
                s = sorted(((q0 - g["o2"]) @ t2 / nt ** 2, (q1 - g["o2"]) @ t2 / nt ** 2))
                return gi, s[0], s[1]
        return None


def check_scene(sc, got):
    name, tol = sc.name, 1e-7 * sc.scale
    per_group = [[] for _ in sc.groups]
    for label in ("visible", "hidden"):
        for seg in got[label]:
            ax, ay, bx, by, solid = seg
            q0, q1 = np.array([ax, ay], float), np.array([bx, by], float)
            loc = sc.locate(q0, q1)
            if loc is None:
                return f"{name}: {label} segment {seg} is not on the projection of any edge of the scene"
            gi, s0, s1 = loc
            g = sc.groups[gi]
            if not any(s0 >= u0 - tol and s1 <= u1 + tol for u0, u1 in g["union"]):
                return f"{name}: {label} segment {seg} extends beyond its edge"
            if not any(k == solid and min(s1, m1) - max(s0, m0) > tol for k, m0, m1 in g["members"]):
                return f"{name}: {label} segment {seg} is attributed to solid {solid}, which has no edge there"
            for f in FRACS if s1 - s0 > tol else []:  # slivers below tolerance: coverage only
                s = s0 + f * (s1 - s0)
                occ = sc.occluded(g["o"] + s * g["t"])
                if occ != (label == "hidden"):
                    return (f"{name}: {label} segment {seg} is wrong at {f:.0%} of its length "
                            f"(oracle ray says {'hidden' if occ else 'visible'})")
            per_group[gi].append((s0, s1, label))
    for gi, g in enumerate(sc.groups):
        segs = sorted(per_group[gi])
        for (a0, a1, la), (b0, b1, lb) in zip(segs, segs[1:]):
            if b0 < a1 - tol:
                return f"{name}: overlapping {la}/{lb} segments on one edge line (collinear overlaps must be merged)"
        want = sum(u1 - u0 for u0, u1 in g["union"])
        have = sum(s1 - s0 for s0, s1, _ in segs)
        if abs(want - have) > 10 * tol:
            k = g["members"][0][0]
            return f"{name}: an edge of solid {k} is covered for {have:.9f} of its {want:.9f} length (missing segments)"
    return None


def closed_form(sc, got):
    """Totals for the closed-form scenes, from face orientation alone (no ray casting)."""
    L = lambda segs: sum(math.hypot(s[2] - s[0], s[3] - s[1]) for s in segs)
    plen = lambda a, b: np.linalg.norm(sc.proj(b) - sc.proj(a))
    if sc.name == "cube":
        vis = hid = 0.0
        for _, a, b in sc.edges:
            m = (a + b) / 2 - 0.5
            normals = [np.sign(m[i]) * np.eye(3)[i] for i in range(3) if abs(m[i]) > 0.25]
            front = any(n @ sc.d < 0 for n in normals)
            vis, hid = (vis + plen(a, b), hid) if front else (vis, hid + plen(a, b))
        if abs(L(got["visible"]) - vis) > 1e-9 or abs(L(got["hidden"]) - hid) > 1e-9:
            return f"cube: visible/hidden lengths {L(got['visible'])}/{L(got['hidden'])}, closed form {vis}/{hid}"
    if sc.name == "fully_occluded":
        small = [s for s in got["visible"] if s[4] == 0]
        if small:
            return f"fully_occluded: solid 0 is entirely behind solid 1 but has visible segments {small[:3]}"
        want = sum(plen(a, b) for k, a, b in sc.edges if k == 0)
        have = L([s for s in got["hidden"] if s[4] == 0])
        if abs(want - have) > 1e-9:
            return f"fully_occluded: hidden length of solid 0 is {have}, closed form {want}"
    return None


def check(expected, outputs):
    got = {}
    errs = []
    for line in outputs:
        if line.strip():
            try:  # NaN/inf coordinates or malformed lines are failures, not crashes
                o = json.loads(line, parse_constant=lambda c: float("nan"))
                got[o["case"]] = o
            except (ValueError, KeyError, TypeError) as e:
                errs.append(f"driver output line is not a scene record ({e}): {line[:80]}")
    for name, d, up, solids in expected:
        if name not in got:
            errs.append(f"{name}: no output")
            continue
        sc = Scene(name, d, up, solids)
        e = check_scene(sc, got[name]) or closed_form(sc, got[name])
        if e:
            errs.append(e)
    return errs


# ---------------------------------------------------------------- self-test
def fabricate(sc, split=True):
    """An exact answer: split every edge line at all projected crossings, classify midpoints."""
    out = {"case": sc.name, "visible": [], "hidden": []}
    for g in sc.groups:
        for u0, u1 in g["union"]:
            cuts = {u0, u1}
            for h in sc.groups:
                if h is g:
                    continue
                A = np.array([g["t2"], -h["t2"]]).T
                if abs(np.linalg.det(A)) < 1e-12:
                    continue
                s, t = np.linalg.solve(A, h["o2"] - g["o2"])
                if u0 < s < u1 and any(m0 <= t <= m1 for m0, m1 in h["union"]):
                    cuts.add(s)
            cuts = sorted(cuts)
            cuts = [c for i, c in enumerate(cuts) if i == 0 or c - cuts[i - 1] > 1e-9]
            if not split:  # corruption: one piece per edge, labelled by its first piece
                cuts = [cuts[0], cuts[-1]]
            for s0, s1 in zip(cuts, cuts[1:]):
                sm = (s0 + s1) / 2 if split else s0 + 1e-6
                label = "hidden" if sc.occluded(g["o"] + sm * g["t"]) else "visible"
                owner = next(k for k, m0, m1 in g["members"] if m0 - 1e-12 <= sm <= m1 + 1e-12)
                a, b = sc.proj(g["o"] + s0 * g["t"]), sc.proj(g["o"] + s1 * g["t"])
                out[label].append([a[0], a[1], b[0], b[1], owner])
    return out


def trimesh_crosscheck(sc, rng):
    import trimesh

    meshes = []
    for z0, h, poly in sc.solids:
        P = np.asarray(poly)
        k = len(P)
        V = np.r_[np.c_[P, np.full(k, z0)], np.c_[P, np.full(k, z0 + h)]]
        F = [[0, i + 1, i] for i in range(1, k - 1)] + [[k, k + i, k + i + 1] for i in range(1, k - 1)]
        F += [[i, (i + 1) % k, k + (i + 1) % k] for i in range(k)] + [[i, k + (i + 1) % k, k + i] for i in range(k)]
        meshes.append(trimesh.Trimesh(V, F))
    m = trimesh.util.concatenate(meshes)
    n = agree = 0
    for g in sc.groups:
        for u0, u1 in g["union"]:
            for f in rng.uniform(0.05, 0.95, 4):
                p = g["o"] + (u0 + f * (u1 - u0)) * g["t"]
                locs, _, _ = m.ray.intersects_location([p], [-sc.d])
                hit = any((q - p) @ -sc.d > 1e-5 for q in locs)
                n += 1
                agree += hit == sc.occluded(p)
    return n, agree


def main():
    mode = sys.argv[1]
    if mode == "gen":
        scenes = build()
        lines = []
        for name, d, up, solids in scenes:
            Scene(name, d, up, solids).generic()
            lines.append(f"scene {name} " + " ".join(fmt(x) for x in (*d, *up)))
            for z0, h, poly in solids:
                lines.append(f"prism {fmt(z0)} {fmt(h)} {len(poly)} " + " ".join(fmt(c) for p in poly for c in p))
            lines.append("end")
        open(sys.argv[2], "w").write("\n".join(lines) + "\n")
        json.dump(scenes, open(sys.argv[3], "w"))
    elif mode == "check":
        expected = json.load(open(sys.argv[2]))
        errs = check(expected, open(sys.argv[3]).read().splitlines())
        if errs:
            fail(f"{len(errs)} of {len(expected)} HLR scenes wrong; first: " + " | ".join(errs[:4]))
        print(f"T2.6 judge: {len(expected)} HLR scenes agree with the oracle")
    elif mode == "selftest":
        expected = json.load(open(sys.argv[2]))
        rng = np.random.default_rng(1)
        good = []
        for e in expected:
            sc = Scene(*e)
            good.append(fabricate(sc))
            n, agree = trimesh_crosscheck(sc, rng)
            print(f"{sc.name}: {len(sc.groups)} edge lines, trimesh agrees on {agree}/{n} rays")
        errs = check(expected, [json.dumps(o) for o in good])
        assert errs == [], errs
        corruptions = {
            "flip_label": lambda o: o["hidden"].append(o["visible"].pop(0)) if o["visible"] else None,
            "drop": lambda o: o["visible"].pop() if o["visible"] else None,
            "duplicate": lambda o: o["visible"].append(list(o["visible"][0])) if o["visible"] else None,
            "wrong_solid": lambda o: o["visible"][0].__setitem__(4, 99) if o["visible"] else None,
            "shift": lambda o: o["visible"][0].__setitem__(0, o["visible"][0][0] + 1e-4) if o["visible"] else None,
        }
        errs = check(expected, [json.dumps(fabricate(Scene(*e), split=False)) for e in expected])
        print(f"corruption unsplit: {len(errs)} scenes rejected, e.g. {errs[0] if errs else None}")
        assert errs, "unsplit"
        for cname, f in corruptions.items():
            bad = []
            for e in expected:
                o = json.loads(json.dumps(fabricate(Scene(*e))))
                f(o)
                bad.append(json.dumps(o))
            errs = check(expected, bad)
            print(f"corruption {cname}: {len(errs)} scenes rejected, e.g. {errs[0] if errs else None}")
            assert errs, cname
        print("selftest ok")


if __name__ == "__main__":
    main()
