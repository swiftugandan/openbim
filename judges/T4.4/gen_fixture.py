"""Generate judges/T4.4/random_frames.json: seeded random space frames solved by PyNite.

Run: .oracles/py/bin/python -I judges/T4.4/gen_fixture.py   (rewrites the fixture byte-identically)

Conventions encoded here (the T4.4 contract): local x from node i to node j; local z is the unit
component of the member's reference vector orthogonal to x; local y = z x x. End forces are the forces
the nodes exert on the member, in local axes [N1,Vy1,Vz1,T1,My1,Mz1, N2,...,Mz2]. Internal actions at
a station are those the part beyond the station exerts on the part from node i to it, so at node i
they are -f_i and at node j +f_j; envelopes are min/max over stations and combinations.
"""
import json, math, os, random
import numpy as np
from Pynite import FEModel3D

HERE = os.path.dirname(os.path.abspath(__file__))


def unit(v):
    v = np.asarray(v, float)
    return v / np.linalg.norm(v)


def local_axes(pi, pj, ref):
    x = unit(np.subtract(pj, pi))
    r = np.asarray(ref, float)
    z = unit(r - r.dot(x) * x)
    y = np.cross(z, x)
    return np.array([x, y, z])


def make_frame(rng):
    nx, ny, ns = rng.randint(2, 3), 2, rng.randint(1, 2)
    bx, by, h = rng.uniform(4, 6), rng.uniform(3.5, 5), rng.uniform(3, 4)
    nodes, idx = [], {}
    for k in range(ns + 1):
        for i in range(nx):
            for j in range(ny):
                jitter = (lambda s: rng.uniform(-s, s)) if k > 0 else (lambda s: 0.0)
                idx[(i, j, k)] = len(nodes)
                nodes.append([i * bx + jitter(0.3), j * by + jitter(0.3), k * h + jitter(0.2)])
    pairs = []
    for k in range(ns):
        for i in range(nx):
            for j in range(ny):
                pairs.append((idx[(i, j, k)], idx[(i, j, k + 1)]))
    for k in range(1, ns + 1):
        for i in range(nx):
            for j in range(ny):
                if i + 1 < nx:
                    pairs.append((idx[(i, j, k)], idx[(i + 1, j, k)]))
                if j + 1 < ny:
                    pairs.append((idx[(i, j, k)], idx[(i, j + 1, k)]))
    pairs.append((idx[(0, 0, 0)], idx[(1, 1, 1)]))  # one inclined brace
    members = []
    for a, b in pairs:
        if rng.random() < 0.5:
            a, b = b, a  # random member direction
        x = unit(np.subtract(nodes[b], nodes[a]))
        while True:
            ref = unit([rng.uniform(-1, 1) for _ in range(3)])
            if abs(ref.dot(x)) < 0.8:
                break
        E = rng.choice([200e9, 30e9])
        members.append({
            "i": a, "j": b, "ref": ref.tolist(),
            "e": E, "g": E / 2.6, "a": rng.uniform(5e-3, 2e-2),
            "iy": rng.uniform(1e-5, 3e-4), "iz": rng.uniform(1e-5, 3e-4), "j_t": rng.uniform(1e-6, 1e-4),
        })
    bases = [idx[(i, j, 0)] for i in range(nx) for j in range(ny)]
    supports = [{"node": n, "fix": [True] * 6} for n in bases]
    supports[-1]["fix"] = [True, True, True, False, False, False]  # one pinned base
    free = [n for n in range(len(nodes)) if n not in bases]
    loads = []
    for case in ["D", "L", "W"]:
        for n in rng.sample(free, 3):
            f = [rng.uniform(-20e3, 20e3) for _ in range(3)] + [rng.uniform(-5e3, 5e3) for _ in range(3)]
            loads.append({"node": n, "case": case, "f": f})
    combos = [
        {"name": "C1", "factors": [["D", 1.2], ["L", 1.6]]},
        {"name": "C2", "factors": [["D", 0.9], ["W", rng.uniform(0.8, 1.5)]]},
        {"name": "C3", "factors": [["D", 1.0], ["L", 0.5], ["W", -rng.uniform(0.8, 1.5)]]},
    ]
    return {"nodes": nodes, "members": members, "supports": supports, "loads": loads, "combos": combos}


def pynite_rotation(fr, k):
    """Degrees PyNite must rotate member k about its axis so its local y/z equal ours."""
    mb = fr["members"][k]
    m = FEModel3D()
    m.add_node("a", *fr["nodes"][mb["i"]])
    m.add_node("b", *fr["nodes"][mb["j"]])
    m.add_material("m", 1.0, 1.0, 0.3, 1.0)
    m.add_section("s", 1.0, 1.0, 1.0, 1.0)
    m.add_member("e", "a", "b", "m", "s")
    d = m.members["e"].T()[:3, :3]  # PhysMember is a Member3D: default local axes
    y = local_axes(fr["nodes"][mb["i"]], fr["nodes"][mb["j"]], mb["ref"])[1]
    return math.degrees(math.atan2(y.dot(d[2]), y.dot(d[1])))


def solve(fr):
    m = FEModel3D()
    for k, p in enumerate(fr["nodes"]):
        m.add_node(f"N{k}", *p)
    for k, mb in enumerate(fr["members"]):
        m.add_material(f"M{k}", mb["e"], mb["g"], 0.3, 1.0)
        m.add_section(f"S{k}", mb["a"], mb["iy"], mb["iz"], mb["j_t"])
        m.add_member(f"E{k}", f"N{mb['i']}", f"N{mb['j']}", f"M{k}", f"S{k}", rotation=pynite_rotation(fr, k))
    for s in fr["supports"]:
        m.def_support(f"N{s['node']}", *s["fix"])
    for ld in fr["loads"]:
        for d, v in zip(["FX", "FY", "FZ", "MX", "MY", "MZ"], ld["f"]):
            m.add_node_load(f"N{ld['node']}", d, v, ld["case"])
    for c in fr["combos"]:
        m.add_load_combo(c["name"], {k: v for k, v in c["factors"]})
    m.analyze_linear()
    exp = {"displacement": {}, "reaction": {}, "end_forces": {}, "envelope": []}
    for c in fr["combos"]:
        cn = c["name"]
        exp["displacement"][cn] = [[getattr(m.nodes[f"N{k}"], d)[cn] for d in ("DX", "DY", "DZ", "RX", "RY", "RZ")]
                                   for k in range(len(fr["nodes"]))]
        exp["reaction"][cn] = {str(s["node"]): [getattr(m.nodes[f"N{s['node']}"], d)[cn] for d in
                                                ("RxnFX", "RxnFY", "RxnFZ", "RxnMX", "RxnMY", "RxnMZ")]
                               for s in fr["supports"]}
        ef = []
        for k, mb in enumerate(fr["members"]):
            pm = m.members[f"E{k}"]
            assert len(pm.sub_members) == 1
            sm = list(pm.sub_members.values())[0]
            f_glob = sm.T().T @ sm.f(cn).ravel()  # PyNite local -> global
            R = local_axes(fr["nodes"][mb["i"]], fr["nodes"][mb["j"]], mb["ref"])
            assert np.allclose(sm.T()[:3, :3], R, atol=1e-9), "PyNite local axes differ from ours"
            T = np.zeros((12, 12))
            for b in range(4):
                T[3 * b:3 * b + 3, 3 * b:3 * b + 3] = R
            ef.append((T @ f_glob).tolist())
        exp["end_forces"][cn] = ef
    for k in range(len(fr["members"])):
        env = []
        for q in range(6):  # N Vy Vz T My Mz
            vals = []
            for c in fr["combos"]:
                f = exp["end_forces"][c["name"]][k]
                vals += [-f[q], f[6 + q]]
                if q < 4:
                    assert abs(-f[q] - f[6 + q]) <= 1e-6 * (1 + abs(f[q]))
            env.append([min(vals), max(vals)])
        exp["envelope"].append(env)
    return exp


def main():
    rng = random.Random(4404)
    frames = []
    for _ in range(6):
        fr = make_frame(rng)
        fr["expected"] = solve(fr)
        frames.append(fr)
    with open(os.path.join(HERE, "random_frames.json"), "w") as fh:
        json.dump({"generator": "judges/T4.4/gen_fixture.py seed 4404, PyNite (PyNiteFEA 3.2.1)",
                   "frames": frames}, fh, indent=1)
        fh.write("\n")


if __name__ == "__main__":
    main()
