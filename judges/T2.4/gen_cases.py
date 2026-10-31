"""Generate the T2.4 judge's sketch cases (hand-built closed forms + seeded random clusters).

Usage: python -I gen_cases.py <out.json>

Case format (shared with the Rust runner and check.py):
  points:      [[x, y, z], ...]                initial guesses, index = order of add_point3
  datums:      [{"normal": [nx, ny, nz], "offset": d}]   plane {p : normal . p = offset}, unit normals
  constraints: {"t": "coincident", "a", "b"} | {"t": "distance", "a", "b", "v"}
               | {"t": "parallel", "p": [a, b, c, d]} | {"t": "perpendicular", "p": [a, b, c, d]}
               | {"t": "angle", "p": [a, b, c, d], "v": radians} | {"t": "on", "a", "d": datum index}
  expect:      closed-form or generator intent per component: {"points": [...], "status", "dof"?, "pos"?}
"""
import json
import math
import sys

import os

import numpy as np

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # our own check.py (equations)
import check  # noqa: E402

EX, EY, EZ = [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]


class Case:
    def __init__(self, name):
        self.name = name
        self.points, self.datums, self.cons, self.expect = [], [], [], []
        self.ids = {}

    def pt(self, key, guess):
        self.ids[key] = len(self.points)
        self.points.append([float(v) for v in guess])
        return self.ids[key]

    def datum(self, normal, offset):
        n = np.asarray(normal, float)
        n = n / np.linalg.norm(n)
        self.datums.append({"normal": n.tolist(), "offset": float(offset)})
        return len(self.datums) - 1

    def pin(self, p, xyz, axes="xyz"):
        for ax, n in zip("xyz", (EX, EY, EZ)):
            if ax in axes:
                self.on(p, self.datum(n, xyz["xyz".index(ax)]))

    def on(self, p, d):
        self.cons.append({"t": "on", "a": p, "d": d})

    def dist(self, a, b, v):
        self.cons.append({"t": "distance", "a": a, "b": b, "v": float(v)})

    def coinc(self, a, b):
        self.cons.append({"t": "coincident", "a": a, "b": b})

    def perp(self, a, b, c, d):
        self.cons.append({"t": "perpendicular", "p": [a, b, c, d]})

    def par(self, a, b, c, d):
        self.cons.append({"t": "parallel", "p": [a, b, c, d]})

    def angle(self, a, b, c, d, v):
        self.cons.append({"t": "angle", "p": [a, b, c, d], "v": float(v)})

    def exp(self, pts, status, dof=None, pos=None):
        e = {"points": sorted(pts), "status": status}
        if dof is not None:
            e["dof"] = dof
        if pos is not None:
            e["pos"] = {str(k): [float(x) for x in v] for k, v in pos.items()}
        self.expect.append(e)

    def json(self):
        return {"name": self.name, "points": self.points, "datums": self.datums,
                "constraints": self.cons, "expect": self.expect}


def tetra(c, tag=""):
    """Right-angle tetrahedron A(0,0,0) B(3,0,0) C(0,4,0) D(0,0,5); 12 equations on 12 unknowns."""
    a = c.pt("A" + tag, (0.1, -0.1, 0.05))
    b = c.pt("B" + tag, (2.8, 0.2, 0.1))
    cc = c.pt("C" + tag, (0.2, 3.7, -0.1))
    d = c.pt("D" + tag, (-0.1, 0.2, 4.6))
    c.pin(a, (0, 0, 0))
    c.pin(b, (0, 0, 0), axes="yz")
    c.dist(a, b, 3)
    c.pin(cc, (0, 0, 0), axes="z")
    c.dist(a, cc, 4)
    c.perp(a, b, a, cc)
    c.dist(a, d, 5)
    c.perp(a, b, a, d)
    c.perp(a, cc, a, d)
    pos = {a: (0, 0, 0), b: (3, 0, 0), cc: (0, 4, 0), d: (0, 0, 5)}
    return [a, b, cc, d], pos


def hand_cases():
    out = []

    c = Case("tetra_well")
    pts, pos = tetra(c)
    c.exp(pts, "WellConstrained", 0, pos)
    out.append(c)

    c = Case("tetra_redundant_diagonal")
    pts, pos = tetra(c)
    c.dist(pts[1], pts[2], 5.0)  # |BC| = 5 already implied: consistent but redundant
    c.exp(pts, "OverConstrained", 0, pos)
    out.append(c)

    c = Case("tetra_contradiction")
    pts, pos = tetra(c)
    c.dist(pts[1], pts[2], 6.0)  # |BC| must be 5
    c.exp(pts, "Inconsistent")
    out.append(c)

    # Four disjoint clusters, point indices interleaved, each with its own status.
    c = Case("mixed_clusters")
    a = c.pt("A", (0.1, -0.1, 0.05))
    e = c.pt("E", (7.0, 0.0, 0.0))
    b = c.pt("B", (2.8, 0.2, 0.1))
    g = c.pt("G", (-3.0, -3.0, -3.0))
    cc = c.pt("C", (0.2, 3.7, -0.1))
    f = c.pt("F", (7.5, 1.5, 0.3))
    d = c.pt("D", (-0.1, 0.2, 4.6))
    h = c.pt("H", (1.2, 0.9, 1.1))
    c.pin(a, (0, 0, 0))
    c.pin(b, (0, 0, 0), axes="yz")
    c.dist(a, b, 3)
    c.pin(cc, (0, 0, 0), axes="z")
    c.dist(a, cc, 4)
    c.perp(a, b, a, cc)
    c.dist(e, f, 2.0)
    c.dist(a, d, 5)
    c.perp(a, b, a, d)
    c.perp(a, cc, a, d)
    c.pin(h, (1, 1, 1))
    c.on(h, c.datum(EZ, 1.0))  # a second z = 1 datum: redundant
    c.exp([a, b, cc, d], "WellConstrained", 0, {a: (0, 0, 0), b: (3, 0, 0), cc: (0, 4, 0), d: (0, 0, 5)})
    c.exp([e, f], "UnderConstrained", 5)
    c.exp([g], "UnderConstrained", 3)  # an unconstrained point is its own subsystem
    c.exp([h], "OverConstrained", 0, {h: (1, 1, 1)})
    out.append(c)

    # An unsatisfiable cluster must not disturb a healthy one.
    c = Case("healthy_beside_inconsistent")
    p = c.pt("P", (5.1, 0.1, 0.0))
    pts, pos = tetra(c)
    q = c.pt("Q", (8.2, 0.0, 0.1))
    c.pin(p, (5, 0, 0))
    c.pin(q, (8, 0, 0))
    c.dist(p, q, 4.0)
    c.exp(pts, "WellConstrained", 0, pos)
    c.exp([p, q], "Inconsistent")
    out.append(c)

    c = Case("parallel3")
    a = c.pt("A", (0, 0, 0))
    b = c.pt("B", (2, 0, 0))
    cc = c.pt("C", (0, 1, 0))
    d = c.pt("D", (1.7, 1.3, 0.2))
    c.pin(a, (0, 0, 0)); c.pin(b, (2, 0, 0)); c.pin(cc, (0, 1, 0))
    c.par(a, b, cc, d)
    c.dist(cc, d, 2.0)
    c.exp([a, b, cc, d], "WellConstrained", 0, {a: (0, 0, 0), b: (2, 0, 0), cc: (0, 1, 0), d: (2, 1, 0)})
    out.append(c)

    c = Case("parallel3_redundant_plane")
    a = c.pt("A", (0, 0, 0))
    b = c.pt("B", (2, 0, 0))
    cc = c.pt("C", (0, 1, 0))
    d = c.pt("D", (1.7, 1.3, 0.2))
    c.pin(a, (0, 0, 0)); c.pin(b, (2, 0, 0)); c.pin(cc, (0, 1, 0))
    c.par(a, b, cc, d)
    c.dist(cc, d, 2.0)
    c.on(d, c.datum(EZ, 0.0))  # implied by the parallel
    c.exp([a, b, cc, d], "OverConstrained", 0, {d: (2, 1, 0)})
    out.append(c)

    c = Case("angle3_sixty")
    a = c.pt("A", (0, 0, 0))
    b = c.pt("B", (1, 0, 0))
    cc = c.pt("C", (0.8, 1.5, 0.1))
    c.pin(a, (0, 0, 0)); c.pin(b, (1, 0, 0))
    c.pin(cc, (0, 0, 0), axes="z")
    c.dist(a, cc, 2.0)
    c.angle(a, b, a, cc, math.pi / 3)
    c.exp([a, b, cc], "WellConstrained", 0, {cc: (1, math.sqrt(3), 0)})
    out.append(c)

    c = Case("coincident3")
    a = c.pt("A", (1.1, 2.2, 2.9))
    b = c.pt("B", (0.4, 2.7, 3.6))
    c.pin(a, (1, 2, 3))
    c.coinc(a, b)
    c.exp([a, b], "WellConstrained", 0, {a: (1, 2, 3), b: (1, 2, 3)})
    out.append(c)

    # Three oblique datums meet in a single point: (1, 2, 3).
    c = Case("oblique_datums")
    a = c.pt("A", (0.0, 0.0, 0.0))
    target = np.array([1.0, 2.0, 3.0])
    for n in ((1, 1, 1), (1, -1, 0), (0, 1, -2)):
        nn = np.asarray(n, float) / np.linalg.norm(n)
        c.on(a, c.datum(nn, float(nn @ target)))
    c.exp([a], "WellConstrained", 0, {a: target})
    out.append(c)

    c = Case("free_rotation")
    a = c.pt("A", (0, 0, 0))
    b = c.pt("B", (1, 1, 1))
    c.pin(a, (0, 0, 0))
    c.dist(a, b, 5.0)
    c.exp([a, b], "UnderConstrained", 2)
    out.append(c)

    return out


def unit(v):
    return v / np.linalg.norm(v)


def well_conditioned(n, target, cons):
    """Smallest singular value of the base (square) Jacobian at the target, relative to the largest."""
    case = {"datums": [], "points": target.tolist()}
    eqs = []
    for kind, args, val in cons:
        if kind == "on":
            case["datums"].append({"normal": list(val[0]), "offset": val[1]})
            eqs.append({"t": "on", "a": args, "d": len(case["datums"]) - 1})
        elif kind == "distance":
            eqs.append({"t": "distance", "a": args[0], "b": args[1], "v": val})
        else:
            eqs.append({"t": "angle", "p": list(args), "v": val})
    x = target.flatten()
    h = 1e-6
    J = np.array([(check.equations(case, eqs, x + h * e) - check.equations(case, eqs, x - h * e)) / (2 * h)
                  for e in np.eye(len(x))]).T
    s = np.linalg.svd(J, compute_uv=False)
    return J.shape == (3 * n, 3 * n) and s[-1] > 0.05 * s[0]


def random_cluster(c, rng, centre, intent):
    """A generically rigid, well-conditioned cluster of n points anchored by datums, then perturbed by `intent`."""
    while True:
        n = int(rng.integers(3, 7))
        target = centre + rng.uniform(-2.0, 2.0, size=(n, 3))
        cons = []
        # 6 datum equations fix the rigid motion: 3 on p0, 2 on p1, 1 on p2 (planes through the targets).
        for i, k in ((0, 3), (1, 2), (2, 1)):
            for _ in range(k):
                nrm = unit(rng.normal(size=3))
                cons.append(("on", i, (nrm, float(nrm @ target[i]))))
        # 3n - 6 metric equations: p1 -> p0; p2 -> p0, p1; p_i -> 3 earlier points (angles for some).
        for i in range(1, n):
            prev = list(range(i)) if i < 3 else [int(v) for v in rng.choice(i, 3, replace=False)]
            for j in prev:
                if i >= 3 and j != prev[0] and rng.random() < 0.25:
                    # angle at p_j between p_j->p_prev0 and p_j->p_i, value read off the target
                    u = target[prev[0]] - target[j]
                    v = target[i] - target[j]
                    ang = math.acos(float(u @ v / np.linalg.norm(u) / np.linalg.norm(v)))
                    if 0.35 < ang < math.pi - 0.35:
                        cons.append(("angle", (j, prev[0], j, i), ang))
                        continue
                cons.append(("distance", (j, i), float(np.linalg.norm(target[i] - target[j]))))
        if well_conditioned(n, target - centre, cons):
            break
    base = len(c.points)
    for i in range(n):
        c.pt(f"r{base + i}", target[i] + rng.normal(0, 0.03, 3))
    dof = None
    if intent == "under":
        k = int(rng.integers(1, 3))
        for t in range(k):
            # drop datum equations of p1/p2 (indices 3..5): never disconnects the cluster
            cons.pop(int(rng.integers(3, 6 - t)))
        dof = k
    elif intent in ("over", "inconsistent"):
        dval = float(np.linalg.norm(target[0] - target[n - 1]))
        if intent == "inconsistent":
            dval += 0.5
        cons.append(("distance", (0, n - 1), dval))
    else:
        dof = 0
    for kind, args, val in cons:
        if kind == "on":
            c.on(base + args, c.datum(*val))
        elif kind == "distance":
            c.dist(base + args[0], base + args[1], val)
        else:
            c.angle(*[base + a for a in args], val)
    status = {"well": "WellConstrained", "under": "UnderConstrained", "over": "OverConstrained",
              "inconsistent": "Inconsistent"}[intent]
    pos = {base + i: target[i] for i in range(n)} if intent in ("well", "over") else None
    c.exp(list(range(base, base + n)), status, 0 if intent in ("well", "over") else dof, pos)


def random_cases(seed=24, count=24):
    rng = np.random.default_rng(seed)
    out = []
    intents = ["well", "under", "over", "inconsistent"]
    for k in range(count):
        c = Case(f"random_{k}")
        ncl = int(rng.integers(2, 4))
        for m in range(ncl):
            intent = intents[(k + m) % 4] if m else ("well" if k % 2 == 0 else "under")
            random_cluster(c, rng, np.array([20.0 * m, 0.0, 0.0]), intent)
        out.append(c)
    return out


if __name__ == "__main__":
    cases = [c.json() for c in hand_cases() + random_cases()]
    with open(sys.argv[1], "w") as fh:
        json.dump({"cases": cases}, fh, indent=1)
