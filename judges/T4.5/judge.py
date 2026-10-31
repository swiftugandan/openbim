"""External judge for T4.5: `bim solve --trade struct --json` on documents built through `bim apply`.

Usage: .oracles/py/bin/python -I judges/T4.5/judge.py <path-to-bim>

Checks
  A. single 6 x 4 bay, slab with a 1 x 1 opening, AreaLoad: tributary areas by closed form (nearest-beam
     rule: trapezoids/triangles, opening removed), reactions per combination equal the factored load,
     each corner column takes a quarter (double symmetry).
  B. 8 x 4 slab over two square bays: the edge beams are split where the middle beam frames in
     (`<id>#1`, `<id>#2`), every tributary is 4 m^2 except the middle beam (8 m^2).
  C. two-storey 2 x 1 bay space frame with a secondary beam (splitting both primaries), a pinned
     Support, near-node PointLoads in three cases and three LoadCombinations, nodes authored 4e-7 apart:
     reactions, member envelopes and utilization against PyNite (0.1 %), PyNite first checked against a
     closed-form cantilever.

Model conventions (TASKS.md T4.5 Details, spec/cli.md `solve --trade struct`): member local x from the start
((x1,y1) for a Beam, base for a Column); reference vector +z for Beams, +y for Columns (local z = unit
component of the reference vector orthogonal to x, local y = z x x); section_b along local y, section_h
along local z; A = b h, Iy = b h^3/12, Iz = h b^3/12, J = a c^3 (1/3 - 0.21 (c/a)(1 - c^4/(12 a^4)))
with a >= c the sides, Wy = b h^2/6, Wz = h b^2/6; Euler-Bernoulli members; no self-weight; envelopes are
[min, max] over stations and combinations of the internal actions exerted on the part from the start to the
station by the rest (tension positive, the T4.4 convention); utilization is the maximum over stations and
combinations; a Beam's axis is at level elevation - z_offset - section_h/2; a split element is reported as
<id>#1..<id>#k from its start.
"""
import json
import math
import os
import subprocess
import sys
import tempfile

import numpy as np
from Pynite import FEModel3D

BIM = sys.argv[1]
FAILS = []


def fail(msg):
    FAILS.append(msg)


def close(got, exp, floor, what, rel=1e-3):
    try:
        g = float(got)
    except (TypeError, ValueError):
        fail(f"{what}: not a number: {got!r}")
        return
    if not abs(g - exp) <= rel * abs(exp) + floor:
        fail(f"{what}: got {g}, expected {exp}")


def run(*args, timeout=60):
    return subprocess.run([BIM, *args], capture_output=True, text=True, timeout=timeout)


def ref(i):
    return {"ref": i}


def solve(entities):
    """entities: list of (id, kind, props). Returns the parsed `solve --trade struct --json` output."""
    with tempfile.TemporaryDirectory() as d:
        proj = os.path.join(d, "p")
        r = run("init", proj)
        if r.returncode != 0:
            raise SystemExit(f"JUDGE FAIL: bim init exited {r.returncode}: {r.stderr.strip()}")
        ops = {"schema": 1, "ops": [{"op": "create", "id": i, "kind": k, "props": p} for i, k, p in entities]}
        f = os.path.join(d, "ops.json")
        with open(f, "w") as fh:
            json.dump(ops, fh)
        r = run("apply", proj, f)
        if r.returncode != 0:
            raise SystemExit(f"JUDGE FAIL: bim apply exited {r.returncode}: {r.stderr.strip()}")
        r = run("solve", proj, "--trade", "struct", "--json")
        if r.returncode != 0:
            raise SystemExit(f"JUDGE FAIL: solve --trade struct exited {r.returncode}: {r.stderr.strip()}")
        try:
            return json.loads(r.stdout)
        except json.JSONDecodeError as e:
            raise SystemExit(f"JUDGE FAIL: solve --trade struct did not print one JSON document: {e}")


# ---------------------------------------------------------------- document builders
E_, G_, FY = 210e9, 81e9, 355e6
MAT = ("S355", "Material", {"name": "S355", "E": E_, "G": G_, "density": 7850.0, "fy": FY})


def level(i, z):
    return (i, "Level", {"elevation": z})


def column(i, x, y, base, top, b):
    return (i, "Column", {"x": x, "y": y, "base": ref(base), "top": ref(top), "section_b": b, "section_h": b,
                          "material": ref("S355")})


def beam(i, lvl, x1, y1, x2, y2, b, h):
    # z_offset = -h/2 puts the beam's centroidal axis exactly at the level elevation, where columns end.
    return (i, "Beam", {"level": ref(lvl), "x1": x1, "y1": y1, "x2": x2, "y2": y2, "section_b": b, "section_h": h,
                        "material": ref("S355"), "z_offset": -h / 2})


def combo(name, cases, factors):
    return (name, "LoadCombination", {"name": name, "cases": cases, "factors": factors})


# ---------------------------------------------------------------- output access
def by_id(rows, key, what):
    out = {}
    for r in rows:
        if not isinstance(r, dict) or key not in r:
            fail(f"{what}: row without '{key}': {r!r}")
            continue
        out[str(r[key])] = r
    return out


def node_coords(out):
    nodes = {}
    for n in out.get("nodes", []):
        if not (isinstance(n, dict) and isinstance(n.get("id"), str) and n["id"][:1] == "n" and n["id"][1:].isdigit()):
            fail(f"nodes[]: id must be n<k>: {n!r}")
        try:
            nodes[str(n["id"])] = (float(n["x"]), float(n["y"]), float(n["z"]))
        except (KeyError, TypeError, ValueError):
            fail(f"nodes[]: malformed row {n!r}")
    return nodes


def find_node(nodes, p, what):
    best = [k for k, q in nodes.items() if math.dist(p, q) < 1e-5]
    if len(best) != 1:
        fail(f"{what}: expected exactly one analytical node at {p}, found {len(best)}")
        return None
    return best[0]


def reactions_by(out, nodes):
    """{(combination, node coords rounded): row}"""
    res = {}
    for r in out.get("reactions", []):
        try:
            p = nodes[str(r["node"])]
        except (KeyError, TypeError):
            fail(f"reactions[]: node {r.get('node') if isinstance(r, dict) else r!r} not in nodes[]")
            continue
        res[(str(r.get("combination")), tuple(round(c, 4) for c in p))] = r
    return res


def check_shape(out):
    for k in ("nodes", "members", "reactions", "tributary"):
        if not isinstance(out.get(k), list):
            raise SystemExit(f"JUDGE FAIL: output lacks list '{k}': keys {sorted(out) if isinstance(out, dict) else out}")


def check_statics(out, nodes, combos, total, centroid, what):
    """Reactions per combination balance the factored vertical load W applied at `centroid`."""
    for name, w in combos.items():
        rows = [r for r in out["reactions"] if str(r.get("combination")) == name]
        if not rows:
            fail(f"{what}: no reactions for combination {name}")
            continue
        fx = sum(float(r["fx"]) for r in rows)
        fy = sum(float(r["fy"]) for r in rows)
        fz = sum(float(r["fz"]) for r in rows)
        close(fz, w * total, 1e-6 * w * total, f"{what} {name}: sum of reaction fz")
        close(fx, 0.0, 1e-6 * w * total, f"{what} {name}: sum of reaction fx")
        close(fy, 0.0, 1e-6 * w * total, f"{what} {name}: sum of reaction fy")
        # Moments about the global origin: reactions balance the load resultant -W at the centroid.
        mx = sum(nodes[str(r["node"])][1] * float(r["fz"]) - nodes[str(r["node"])][2] * float(r["fy"]) + float(r["mx"])
                 for r in rows)
        my = sum(nodes[str(r["node"])][2] * float(r["fx"]) - nodes[str(r["node"])][0] * float(r["fz"]) + float(r["my"])
                 for r in rows)
        close(mx, centroid[1] * w * total, 1e-6 * w * total, f"{what} {name}: reaction moment about x")
        close(my, -centroid[0] * w * total, 1e-6 * w * total, f"{what} {name}: reaction moment about y")


def check_tributary(out, expected, what):
    trib = {}
    for t in out["tributary"]:
        try:
            trib[str(t["member"])] = trib.get(str(t["member"]), 0.0) + float(t["area"])
        except (KeyError, TypeError, ValueError):
            fail(f"{what}: malformed tributary row {t!r}")
    for m, a in expected.items():
        if m not in trib:
            fail(f"{what}: no tributary area for member {m} (got {sorted(trib)})")
        else:
            close(trib[m], a, 1e-9, f"{what}: tributary area of {m}")
    for m, a in trib.items():
        if m not in expected and abs(a) > 1e-9:
            fail(f"{what}: unexpected tributary area {a} on {m}")


# ---------------------------------------------------------------- scenario A
def scenario_a():
    q, f = 5e3, 1.35
    ents = [MAT, level("L0", 0.0), level("L1", 3.5)]
    for k, (x, y) in enumerate([(0, 0), (6, 0), (6, 4), (0, 4)]):
        ents.append(column(f"C{k}", x, y, "L0", "L1", 0.3))
    ents += [beam("BS", "L1", 0, 0, 6, 0, 0.2, 0.4), beam("BE", "L1", 6, 0, 6, 4, 0.2, 0.4),
             beam("BN", "L1", 6, 4, 0, 4, 0.2, 0.4), beam("BW", "L1", 0, 4, 0, 0, 0.2, 0.4)]
    ents += [("S1", "Slab", {"level": ref("L1"), "boundary": [0, 0, 6, 0, 6, 4, 0, 4], "thickness": 0.2}),
             ("H1", "SlabOpening", {"host": ref("S1"), "boundary": [2.5, 1.5, 3.5, 1.5, 3.5, 2.5, 2.5, 2.5]}),
             ("Q1", "AreaLoad", {"host": ref("S1"), "pressure": q, "case": "D"}),
             combo("ULS", ["D"], [f]), combo("SLS", ["D"], [1.0])]
    out = solve(ents)
    check_shape(out)
    nodes = node_coords(out)
    # Nearest-beam regions: long beams get trapezoids (2*6-4)*4/4 = 8 less half the opening each,
    # short beams triangles 4^2/4 = 4 (the opening lies wholly in the long beams' regions).
    check_tributary(out, {"BS": 7.5, "BN": 7.5, "BE": 4.0, "BW": 4.0}, "A")
    area = 24.0 - 1.0
    check_statics(out, nodes, {"ULS": f, "SLS": 1.0}, q * area, (3.0, 2.0), "A")
    reac = reactions_by(out, nodes)
    for x, y in [(0, 0), (6, 0), (6, 4), (0, 4)]:
        r = reac.get(("ULS", (float(x), float(y), 0.0)))
        if r is None:
            fail(f"A: no ULS reaction at column base ({x},{y},0)")
        else:
            close(r["fz"], f * q * area / 4, 1e-6 * q, f"A: column base ({x},{y}) fz")
    if any(abs(p[2] - 3.5) < 1e-9 and (p[0], p[1]) not in [(0, 0), (6, 0), (6, 4), (0, 4)] for p in nodes.values()):
        fail("A: unexpected analytical node on L1 (no member frames into a beam span)")


# ---------------------------------------------------------------- scenario B
def scenario_b():
    q = 4e3
    ents = [MAT, level("L0", 0.0), level("L1", 3.0)]
    for k, (x, y) in enumerate([(0, 0), (4, 0), (8, 0), (8, 4), (4, 4), (0, 4)]):
        ents.append(column(f"C{k}", x, y, "L0", "L1", 0.3))
    ents += [beam("BS", "L1", 0, 0, 8, 0, 0.2, 0.4), beam("BN", "L1", 8, 4, 0, 4, 0.2, 0.4),
             beam("BE", "L1", 8, 0, 8, 4, 0.2, 0.4), beam("BW", "L1", 0, 4, 0, 0, 0.2, 0.4),
             beam("BM", "L1", 4, 0, 4, 4, 0.2, 0.4),
             ("S1", "Slab", {"level": ref("L1"), "boundary": [0, 0, 8, 0, 8, 4, 0, 4], "thickness": 0.2}),
             ("Q1", "AreaLoad", {"host": ref("S1"), "pressure": q, "case": "D"}),
             combo("ULS", ["D"], [1.5])]
    out = solve(ents)
    check_shape(out)
    nodes = node_coords(out)
    # Two square 4 x 4 bays: every beam side takes a 4 m^2 triangle; the middle beam takes two.
    check_tributary(out, {"BS#1": 4.0, "BS#2": 4.0, "BN#1": 4.0, "BN#2": 4.0, "BE": 4.0, "BW": 4.0, "BM": 8.0}, "B")
    mids = {str(m.get("id")) for m in out["members"]}
    for m in ("BS#1", "BS#2", "BN#1", "BN#2", "BE", "BW", "BM", "C0", "C5"):
        if m not in mids:
            fail(f"B: member {m} missing from members[] (got {sorted(mids)})")
    for m in ("BS", "BN"):
        if m in mids:
            fail(f"B: split beam {m} still reported unsplit")
    check_statics(out, nodes, {"ULS": 1.5}, q * 32.0, (4.0, 2.0), "B")
    find_node(nodes, (4.0, 0.0, 3.0), "B: node where BM frames into BS")


# ---------------------------------------------------------------- scenario C (PyNite)
def rect_props(b, h):
    a, c = max(b, h), min(b, h)
    j = a * c ** 3 * (1 / 3 - 0.21 * (c / a) * (1 - c ** 4 / (12 * a ** 4)))
    return {"A": b * h, "Iy": b * h ** 3 / 12, "Iz": h * b ** 3 / 12, "J": j, "Wy": b * h ** 2 / 6, "Wz": h * b ** 2 / 6}


def unit(v):
    v = np.asarray(v, float)
    return v / np.linalg.norm(v)


def local_axes(pi, pj, refv):
    x = unit(np.subtract(pj, pi))
    r = np.asarray(refv, float)
    z = unit(r - r.dot(x) * x)
    return np.array([x, np.cross(z, x), z])


def pynite_rotation(pi, pj, refv):
    m = FEModel3D()
    m.add_node("a", *pi)
    m.add_node("b", *pj)
    m.add_material("m", 1.0, 1.0, 0.3, 1.0)
    m.add_section("s", 1.0, 1.0, 1.0, 1.0)
    m.add_member("e", "a", "b", "m", "s")
    d = m.members["e"].T()[:3, :3]
    y = local_axes(pi, pj, refv)[1]
    return math.degrees(math.atan2(y.dot(d[2]), y.dot(d[1])))


def pynite_self_check():
    """PyNite must reproduce the closed-form cantilever before it is trusted as the oracle."""
    m = FEModel3D()
    m.add_node("a", 0, 0, 0)
    m.add_node("b", 3, 0, 0)
    m.add_material("s", 2e11, 8e10, 0.3, 1.0)
    m.add_section("x", 1e-2, 2e-5, 5e-5, 1e-5)
    m.add_member("m", "a", "b", "s", "x")
    m.def_support("a", True, True, True, True, True, True)
    m.add_node_load("b", "FY", -1e4, "c")
    m.add_node_load("b", "MX", 1e3, "c")
    m.add_load_combo("C", {"c": 1.0})
    m.analyze_linear()
    dy, rx = m.nodes["b"].DY["C"], m.nodes["b"].RX["C"]
    if abs(dy - (-1e4 * 27 / (3 * 2e11 * 5e-5))) > 1e-9 or abs(rx - 1e3 * 3 / (8e10 * 1e-5)) > 1e-9:
        raise SystemExit("JUDGE FAIL: oracle self-check: PyNite disagrees with the closed-form cantilever")


def build_c():
    """Scenario C document and its PyNite expectations."""
    pynite_self_check()
    eps = 4e-7  # authored coordinates within the 1e-6 merge tolerance
    xs, ys, zs = [0.0, 5.0, 10.0], [0.0, 4.0], [0.0, 3.5, 7.0]
    lv = ["L0", "L1", "L2"]
    ents = [MAT] + [level(lv[k], z) for k, z in enumerate(zs)]
    sec_c, sec_b, sec_s = 0.35, (0.25, 0.5), (0.2, 0.35)
    cols = []  # (id, base point, top point, b, h)
    for i, x in enumerate(xs):
        for j, y in enumerate(ys):
            for s in range(2):
                cid = f"C{i}{j}{s}"
                xa = x + (eps if (i + j + s) % 2 else 0.0)
                ents.append(column(cid, xa, y, lv[s], lv[s + 1], sec_c))
                cols.append((cid, (x, y, zs[s]), (x, y, zs[s + 1]), sec_c, sec_c))
    beams = []  # (id, start, end, b, h)
    for s in (1, 2):
        z = zs[s]
        for j, y in enumerate(ys):
            for i in range(2):
                bid = f"BX{s}{j}{i}"
                ents.append(beam(bid, lv[s], xs[i], y, xs[i + 1], y, *sec_b))
                beams.append((bid, (xs[i], y, z), (xs[i + 1], y, z), *sec_b))
        for i, x in enumerate(xs):
            bid = f"BY{s}{i}"
            ents.append(beam(bid, lv[s], x, ys[1], x, ys[0], *sec_b))  # authored north to south
            beams.append((bid, (x, ys[1], z), (x, ys[0], z), *sec_b))
    # Secondary beam on L1 framing into the midspans of BX100 and BX110: splits both.
    ents.append(beam("SB", "L1", 2.5, 0.0, 2.5, 4.0, *sec_s))
    beams.append(("SB", (2.5, 0.0, 3.5), (2.5, 4.0, 3.5), *sec_s))
    # Pinned support under column C200's base at (10, 0, 0) (overrides the default fixed base).
    ents.append(("SUP", "Support", {"x": 10.0, "y": 0.0, "z": 0.0, "ux": True, "uy": True, "uz": True,
                                     "rx": False, "ry": False, "rz": False}))
    loads = [  # (id, point, [fx fy fz mx my mz], case)
        ("PD1", (2.6, 0.1, 3.5), [0, 0, -40e3, 0, 0, 0], "D"),
        ("PD2", (5.0, 4.0, 7.0), [0, 0, -60e3, 2e3, 0, 0], "D"),
        ("PD3", (10.0, 0.05, 3.48), [0, 0, -30e3, 0, 0, 0], "D"),
        ("PL1", (2.5, 4.0, 3.5), [0, 0, -25e3, 0, 0, 0], "L"),
        ("PL2", (0.0, 3.9, 7.0), [0, 0, -20e3, 0, -3e3, 0], "L"),
        ("PW1", (0.0, 0.0, 3.5), [15e3, 4e3, 0, 0, 0, 0], "W"),
        ("PW2", (0.0, 4.0, 7.0), [20e3, -6e3, 0, 0, 0, 1e3], "W"),
    ]
    for lid, p, f, case in loads:
        ents.append((lid, "PointLoad", {"x": p[0], "y": p[1], "z": p[2], "fx": f[0], "fy": f[1], "fz": f[2],
                                         "mx": f[3], "my": f[4], "mz": f[5], "case": case}))
    combos = {"ULS1": {"D": 1.35, "L": 1.5}, "ULS2": {"D": 1.0, "W": 1.5}, "SLS": {"D": 1.0, "L": 1.0, "W": -0.6}}
    for name, fac in combos.items():
        ents.append(combo(name, list(fac), list(fac.values())))
    ents.append(("L-unused", "PointLoad", {"x": 5, "y": 0, "z": 7, "fx": 0, "fy": 0, "fz": -1e9, "mx": 0, "my": 0,
                                            "mz": 0, "case": "X"}))  # a case in no combination

    # Expected analytical model: nodes at member ends, members split at interior member ends.
    elems = [(cid, a, b, bb, hh, (0.0, 1.0, 0.0)) for cid, a, b, bb, hh in cols]
    elems += [(bid, a, b, bb, hh, (0.0, 0.0, 1.0)) for bid, a, b, bb, hh in beams]
    pts = []

    def node_of(p):
        for k, q in enumerate(pts):
            if math.dist(p, q) < 1e-6:
                return k
        pts.append(tuple(p))
        return len(pts) - 1

    for _, a, b, *_ in elems:
        node_of(a)
        node_of(b)
    members = []  # (id, i, j, b, h, ref)
    for eid, a, b, bb, hh, rv in elems:
        a_, b_ = np.array(a), np.array(b)
        L = np.linalg.norm(b_ - a_)
        inner = []
        for k, p in enumerate(pts):
            t = np.dot(np.array(p) - a_, b_ - a_) / L ** 2
            if 1e-9 < t < 1 - 1e-9 and np.linalg.norm(a_ + t * (b_ - a_) - p) < 1e-6:
                inner.append((t, k))
        chain = [node_of(a)] + [k for _, k in sorted(inner)] + [node_of(b)]
        for s in range(len(chain) - 1):
            mid = eid if len(chain) == 2 else f"{eid}#{s + 1}"
            members.append((mid, chain[s], chain[s + 1], bb, hh, rv))
    split = {m[0] for m in members if "#" in m[0]}
    assert split == {"BX100#1", "BX100#2", "BX110#1", "BX110#2"}, split

    m = FEModel3D()
    for k, p in enumerate(pts):
        m.add_node(f"N{k}", *p)
    m.add_material("S355", E_, G_, 0.3, 7850.0)
    props = {}
    for mid, i, j, bb, hh, rv in members:
        pr = rect_props(bb, hh)
        props[mid] = pr
        m.add_section(f"S_{mid}", pr["A"], pr["Iy"], pr["Iz"], pr["J"])
        m.add_member(mid, f"N{i}", f"N{j}", "S355", f"S_{mid}", rotation=pynite_rotation(pts[i], pts[j], rv))
    supports = {}
    for k, p in enumerate(pts):
        if abs(p[2]) < 1e-9:
            supports[k] = [True] * 3 + ([False] * 3 if math.dist(p, (10.0, 0.0, 0.0)) < 1e-9 else [True] * 3)
            m.def_support(f"N{k}", *supports[k])
    for lid, p, f, case in loads:
        k = min(range(len(pts)), key=lambda n: math.dist(p, pts[n]))
        for d, v in zip(["FX", "FY", "FZ", "MX", "MY", "MZ"], f):
            if v:
                m.add_node_load(f"N{k}", d, v, case)
    for name, fac in combos.items():
        m.add_load_combo(name, fac)
    m.analyze_linear()

    exp_env, exp_util = {}, {}
    for mid, i, j, bb, hh, rv in members:
        pm = m.members[mid]
        R = local_axes(pts[i], pts[j], rv)
        if not np.allclose(pm.T()[:3, :3], R, atol=1e-9):
            raise SystemExit(f"JUDGE FAIL: oracle setup: PyNite axes of {mid} differ from the contract's")
        T = np.kron(np.eye(4), R)
        vals = {q: [] for q in range(6)}
        util = 0.0
        pr = props[mid]
        for name in combos:
            subs = list(pm.sub_members.values())
            if len(subs) != 1:
                raise SystemExit(f"JUDGE FAIL: oracle setup: PyNite subdivided {mid}")
            sm = subs[0]
            f = T @ (sm.T().T @ sm.f(name).ravel())
            for end in (-f[:6], f[6:]):
                for q in range(6):
                    vals[q].append(end[q])
                util = max(util, abs(end[0]) / (pr["A"] * FY) + abs(end[4]) / (pr["Wy"] * FY)
                           + abs(end[5]) / (pr["Wz"] * FY))
        exp_env[mid] = [[min(v), max(v)] for v in vals.values()]
        exp_util[mid] = util
    exp_reac = {}
    for name in combos:
        for k in supports:
            nd = m.nodes[f"N{k}"]
            exp_reac[(name, tuple(round(c, 4) for c in pts[k]))] = [
                getattr(nd, a)[name] for a in ("RxnFX", "RxnFY", "RxnFZ", "RxnMX", "RxnMY", "RxnMZ")]

    return ents, pts, exp_env, exp_util, exp_reac


def scenario_c():
    ents, pts, exp_env, exp_util, exp_reac = build_c()
    out = solve(ents)
    check_shape(out)
    nodes = node_coords(out)
    if len(nodes) != len(pts):
        fail(f"C: {len(nodes)} analytical nodes, expected {len(pts)} (merge within 1e-6, split at framing members)")
    for p in pts:
        find_node(nodes, p, "C")
    reac = reactions_by(out, nodes)
    fmax = max(abs(v) for r in exp_reac.values() for v in r[:3])
    mmax = max(abs(v) for r in exp_reac.values() for v in r[3:])
    for key, r in exp_reac.items():
        got = reac.get(key)
        if got is None:
            fail(f"C: no reaction for combination {key[0]} at {key[1]}")
            continue
        for a, e in zip(("fx", "fy", "fz", "mx", "my", "mz"), r):
            close(got.get(a), e, 1e-6 * (fmax if a.startswith("f") else mmax), f"C {key}: reaction {a}")
    extra = [k for k, r in reac.items() if k not in exp_reac
             and any(abs(float(r.get(a, 0))) > 1e-6 * fmax for a in ("fx", "fy", "fz", "mx", "my", "mz"))]
    if extra:
        fail(f"C: non-zero reactions reported at unsupported nodes: {sorted(extra)[:4]}")
    outm = by_id(out["members"], "id", "C members[]")
    if set(outm) != set(exp_env):
        fail(f"C: member ids {sorted(outm)} != expected {sorted(exp_env)}")
    names = ["N", "Vy", "Vz", "T", "My", "Mz"]
    scale = {q: max(max(abs(v) for v in e[q]) for e in exp_env.values()) for q in range(6)}
    for mid, env in exp_env.items():
        row = outm.get(mid)
        if row is None:
            continue
        for q, nm in enumerate(names):
            pair = row.get(nm)
            if not (isinstance(pair, list) and len(pair) == 2):
                fail(f"C {mid}: '{nm}' is not [min, max]: {pair!r}")
                continue
            close(pair[0], env[q][0], 1e-6 * scale[q], f"C {mid}: {nm} min")
            close(pair[1], env[q][1], 1e-6 * scale[q], f"C {mid}: {nm} max")
        close(row.get("utilization"), exp_util[mid], 1e-9, f"C {mid}: utilization")
        d = row.get("deflection")
        if not (isinstance(d, (int, float)) and d >= 0 and math.isfinite(d)):
            fail(f"C {mid}: deflection must be a finite non-negative number, got {d!r}")


def main():
    for sc in (scenario_a, scenario_b, scenario_c):
        try:
            sc()
        except SystemExit:
            raise
        except Exception as e:  # malformed output (missing keys, wrong types)
            fail(f"{sc.__name__}: {type(e).__name__}: {e}")
    if FAILS:
        for f in FAILS[:40]:
            print(f"JUDGE FAIL: {f}", file=sys.stderr)
        if len(FAILS) > 40:
            print(f"JUDGE FAIL: ... and {len(FAILS) - 40} more", file=sys.stderr)
        sys.exit(1)
    print("T4.5 judge: all checks passed")


if __name__ == "__main__":
    main()
