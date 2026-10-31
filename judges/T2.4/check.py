"""T2.4 oracle + comparison.

  python -I check.py <cases.json> <results.json>   compare bim-solver's results with the oracle
  python -I check.py <cases.json> --selftest        oracle self-test (feeds the oracle's own answers back,
                                                    then a corrupted copy that must be rejected)

Oracle per connected component of the constraint graph:
  * python-solvespace (SolveSpace's solver, datums as locked work planes in a separate group) solves the
    component from the same initial guesses and reports its DOF;
  * an independent numerical rank analysis of the constraint equations (central differences, SVD)
    supplies dof = 3n - rank(J) and redundancy (a constraint whose removal leaves rank(J) unchanged);
  * hand-built cases also carry closed-form positions/statuses, cross-checked against both.
Parallel3 is checked against closed forms only (python-solvespace aborts on 3D parallel).
"""
import json
import math
import sys

import numpy as np
import python_solvespace as ps
from python_solvespace import Entity, SolverSystem, make_quaternion

FREE = Entity.FREE_IN_3D
POS_TOL = 1e-6
RES_TOL = 1e-7


class OracleError(Exception):
    pass


# ---------- constraint equations (independent of the code under test) ----------

def equations(case, cons, x):
    """Residual vector of `cons` at flat coordinates x (3 per point)."""
    P = x.reshape(-1, 3)
    D = case["datums"]
    out = []
    for c in cons:
        t = c["t"]
        if t == "coincident":
            out.extend(P[c["a"]] - P[c["b"]])
        elif t == "distance":
            out.append(np.linalg.norm(P[c["a"]] - P[c["b"]]) - c["v"])
        elif t == "on":
            d = D[c["d"]]
            out.append(np.dot(d["normal"], P[c["a"]]) - d["offset"])
        else:
            a, b, cc, dd = c["p"]
            u, v = P[b] - P[a], P[dd] - P[cc]
            nu, nv = np.linalg.norm(u), np.linalg.norm(v)
            if t == "parallel":
                out.extend(np.cross(u, v) / (nu * nv))
            elif t == "perpendicular":
                out.append(np.dot(u, v) / (nu * nv))
            elif t == "angle":
                out.append(np.dot(u, v) / (nu * nv) - math.cos(c["v"]))
            else:
                raise OracleError(f"unknown constraint {t}")
    return np.array(out, float)


def jac_rank(case, cons, x):
    if not cons:
        return 0
    h = 1e-6
    cols = []
    for i in range(len(x)):
        e = np.zeros_like(x)
        e[i] = h
        cols.append((equations(case, cons, x + e) - equations(case, cons, x - e)) / (2 * h))
    J = np.array(cols).T
    s = np.linalg.svd(J, compute_uv=False)
    return int(np.sum(s > 1e-6 * max(s[0], 1.0)))


def cons_points(c):
    if c["t"] == "on":
        return [c["a"]]
    if c["t"] in ("coincident", "distance"):
        return [c["a"], c["b"]]
    return list(c["p"])


def components(case):
    n = len(case["points"])
    parent = list(range(n))

    def find(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for c in case["constraints"]:
        p = cons_points(c)
        for q in p[1:]:
            parent[find(q)] = find(p[0])
    groups = {}
    for i in range(n):
        groups.setdefault(find(i), []).append(i)
    return sorted(sorted(g) for g in groups.values())


# ---------- python-solvespace ----------

def quaternion_for(normal):
    n = np.asarray(normal, float)
    e = np.eye(3)[int(np.argmin(np.abs(n)))]
    u = np.cross(n, e)
    u /= np.linalg.norm(u)
    v = np.cross(n, u)
    return make_quaternion(*u, *v)


def solvespace(case, pts, cons):
    s = SolverSystem()
    s.set_group(1)
    wps = {}
    for c in cons:
        if c["t"] == "on" and c["d"] not in wps:
            d = case["datums"][c["d"]]
            n = np.asarray(d["normal"], float)
            origin = s.add_point_3d(*(n * d["offset"]))
            wps[c["d"]] = s.add_work_plane(origin, s.add_normal_3d(*quaternion_for(n)))
    s.set_group(2)
    h = {i: s.add_point_3d(*case["points"][i]) for i in pts}
    for c in cons:
        t = c["t"]
        if t == "on":
            s.coincident(h[c["a"]], wps[c["d"]], FREE)
        elif t == "coincident":
            s.coincident(h[c["a"]], h[c["b"]], FREE)
        elif t == "distance":
            s.distance(h[c["a"]], h[c["b"]], c["v"], FREE)
        else:
            a, b, cc, dd = c["p"]
            l1 = s.add_line_3d(h[a], h[b])
            l2 = s.add_line_3d(h[cc], h[dd])
            kind = {"perpendicular": ps.Constraint.PERPENDICULAR, "angle": ps.Constraint.ANGLE}[t]
            val = math.degrees(c["v"]) if t == "angle" else 0.0
            s.add_constraint(kind, FREE, val, Entity.NONE, Entity.NONE, l1, l2)
    flag = s.solve()
    pos = {i: list(s.params(h[i].params)) for i in pts}
    return int(flag), (s.dof() if int(flag) == 0 else None), pos


# ---------- expected answers ----------

def expected(case):
    comps = components(case)
    hand = {tuple(e["points"]): e for e in case["expect"]}
    out = []
    for pts in comps:
        cons = [c for c in case["constraints"] if cons_points(c)[0] in pts]
        e = hand.get(tuple(pts))
        has_par = any(c["t"] == "parallel" for c in cons)
        x0 = np.array(case["points"], float)
        if has_par:
            if e is None or "pos" not in e or len(e["pos"]) == 0:
                raise OracleError(f"{case['name']}: parallel component without closed form")
            sol = x0.copy()
            for k, v in e["pos"].items():
                sol[int(k)] = v
            flag, ss_dof = None, None
        else:
            flag, ss_dof, ss_pos = solvespace(case, pts, cons)
            sol = x0.copy()
            for i, p in ss_pos.items():
                sol[i] = p
        x = sol.flatten()
        res = float(np.linalg.norm(equations(case, cons, x))) if cons else 0.0
        if res > 1e-6:
            status, dof = "Inconsistent", None
        else:
            rank = jac_rank(case, cons, x)
            dof = 3 * len(pts) - rank
            if ss_dof is not None and ss_dof != dof:
                raise OracleError(f"{case['name']} {pts}: solvespace dof {ss_dof} != rank dof {dof}")
            redundant = any(jac_rank(case, [d for d in cons if d is not c], x) == rank for c in cons)
            status = "UnderConstrained" if dof > 0 else ("OverConstrained" if redundant else "WellConstrained")
        pos = None
        if status in ("WellConstrained", "OverConstrained"):
            pos = {i: sol[i].tolist() for i in pts}
        # Cross-check against the closed form / generator intent.
        if e is None:
            raise OracleError(f"{case['name']}: component {pts} has no generator entry (got {[h for h in hand]})")
        if e["status"] != status:
            raise OracleError(f"{case['name']} {pts}: oracle status {status}, closed form {e['status']}")
        if "dof" in e and dof is not None and e["dof"] != dof:
            raise OracleError(f"{case['name']} {pts}: oracle dof {dof}, closed form {e['dof']}")
        for k, v in e.get("pos", {}).items():
            if pos is not None and np.max(np.abs(np.array(pos[int(k)]) - v)) > POS_TOL:
                raise OracleError(f"{case['name']} point {k}: oracle {pos[int(k)]}, closed form {v}")
        out.append({"points": pts, "status": status, "dof": dof, "pos": pos, "cons": cons})
    return out


# ---------- comparison ----------

def compare(cases, oracle, results):
    fails = []
    by_name = {r["name"]: r for r in results["cases"]}
    for case in cases:
        name = case["name"]
        r = by_name.get(name)
        if r is None:
            fails.append(f"{name}: no result")
            continue
        if "error" in r:
            fails.append(f"{name}: runner error: {r['error']}")
            continue
        want_parts = sorted(o["points"] for o in oracle[name])
        for key in ("subsystems", "decompose"):
            got = sorted(sorted(s["points"] if isinstance(s, dict) else s) for s in r[key])
            if got != want_parts:
                fails.append(f"{name}: {key} partition {got}, want connected components {want_parts}")
        if len(r["positions"]) != len(case["points"]):
            fails.append(f"{name}: {len(r['positions'])} positions for {len(case['points'])} points")
            continue
        x = np.array(r["positions"], float).flatten()
        subs = {tuple(sorted(s["points"])): s for s in r["subsystems"]}
        for o in oracle[name]:
            s = subs.get(tuple(o["points"]))
            if s is None:
                continue
            tag = f"{name} subsystem {o['points']}"
            if s["status"] != o["status"]:
                fails.append(f"{tag}: status {s['status']}, oracle {o['status']}")
            if o["dof"] is not None and s["dof"] != o["dof"]:
                fails.append(f"{tag}: dof {s['dof']}, oracle {o['dof']}")
            if o["status"] != "Inconsistent" and o["cons"]:
                res = float(np.linalg.norm(equations(case, o["cons"], x)))
                if not res < RES_TOL:
                    fails.append(f"{tag}: solved positions leave residual {res:.3e}")
            if o["pos"] is not None:
                for i, p in o["pos"].items():
                    got = r["positions"][i]
                    if np.max(np.abs(np.array(got) - p)) > POS_TOL:
                        fails.append(f"{tag}: point {i} at {got}, oracle {p}")
    return fails


def main():
    cases = json.load(open(sys.argv[1]))["cases"]
    try:
        oracle = {c["name"]: expected(c) for c in cases}
    except OracleError as e:
        print(f"JUDGE FAIL: oracle disagreement (judge bug): {e}")
        sys.exit(1)
    if sys.argv[2] == "--selftest":
        perfect = {"cases": []}
        for c in cases:
            pos = np.array(c["points"], float)
            subs = []
            for o in oracle[c["name"]]:
                if o["pos"] is not None:
                    for i, p in o["pos"].items():
                        pos[i] = p
                else:  # under/inconsistent: take solvespace/closed-form placement where satisfiable
                    if o["status"] == "UnderConstrained":
                        _, _, sp = solvespace(c, o["points"], o["cons"]) if o["cons"] else (0, 0, {})
                        for i, p in sp.items():
                            pos[i] = p
                subs.append({"points": o["points"], "status": o["status"], "dof": o["dof"] if o["dof"] is not None else 0})
            perfect["cases"].append({"name": c["name"], "positions": pos.tolist(), "subsystems": subs,
                                     "decompose": [o["points"] for o in oracle[c["name"]]]})
        f = compare(cases, oracle, perfect)
        assert not f, f
        # Corruptions that must be caught.
        bad = json.loads(json.dumps(perfect))
        bad["cases"][0]["positions"][1][0] += 1e-4
        bad["cases"][3]["subsystems"][0]["status"] = "WellConstrained"
        bad["cases"][4]["subsystems"] = [{"points": sorted(sum((s["points"] for s in bad["cases"][4]["subsystems"]), [])),
                                          "status": "Inconsistent", "dof": 0}]
        f = compare(cases, oracle, bad)
        assert len(f) >= 3, f
        print(f"selftest ok: {len(cases)} cases, {sum(len(v) for v in oracle.values())} components; corruptions caught: {len(f)}")
        return
    results = json.load(open(sys.argv[2]))
    fails = compare(cases, oracle, results)
    if fails:
        for f in fails[:40]:
            print("  " + f)
        print(f"JUDGE FAIL: {len(fails)} mismatches against python-solvespace / closed forms (first: {fails[0]})")
        sys.exit(1)
    print(f"T2.4 oracle comparison: {len(cases)} cases, {sum(len(v) for v in oracle.values())} subsystems agree")


if __name__ == "__main__":
    main()
