# External judge for T4.6 (bim-mep hydraulics). Builders must not edit anything under judges/.
#
# Drives the real `bim` binary (init / apply JSON ops / solve --trade mep --json) and compares with:
#   * closed forms: single pipes (Darcy-Weisbach + Colebrook-White / 64/Re + minor losses), series and
#     branched trees, a textbook two-loop Hardy-Cross network with constant friction factors
#     (fully rough regime, solved here by Hardy-Cross itself) -> 0.1 %
#   * EPANET 2.2 through WNTR (D-W headloss) on random looped networks -> 1 % (EPANET uses Swamee-Jain)
# Usage: python -I judge.py <bim-binary> <scratch-dir>
import json
import math
import os
import random
import subprocess
import sys
import warnings

warnings.filterwarnings("ignore")

BIM, TMP = sys.argv[1], sys.argv[2]
NU_EPANET = 1.1e-5 * 0.3048**2  # EPANET reference kinematic viscosity (1.1e-5 ft^2/s), m^2/s
G_EPANET = 32.2 * 0.3048        # EPANET's g in its D-W head loss, m/s^2


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def run(args, want=0):
    try:
        p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        fail(f"bim {' '.join(args)} timed out")
    if p.returncode != want:
        fail(f"bim {' '.join(args)} exited {p.returncode}, want {want}: {p.stderr.strip()[-400:]}")
    return p.stdout


def project(name, ops):
    d = os.path.join(TMP, name)
    run(["init", d])
    f = os.path.join(TMP, name + ".json")
    with open(f, "w") as fh:
        json.dump({"schema": 1, "ops": ops}, fh)
    run(["apply", d, f])
    return d


def solve(d, extra=()):
    out = run(["solve", d, "--trade", "mep", *extra, "--json"])
    try:
        doc = json.loads(out)
    except json.JSONDecodeError as e:
        fail(f"solve --trade mep did not print one JSON document: {e}: {out[:200]!r}")
    for key in ("segments", "ports", "continuity_residual"):
        if key not in doc:
            fail(f"solve output lacks '{key}'")
    segs = {s["id"]: s for s in doc["segments"]}
    ports = {p["id"]: p for p in doc["ports"]}
    return doc, segs, ports


def close(actual, expected, rel, what, abs_tol=0.0):
    if not isinstance(actual, (int, float)) or not math.isfinite(actual):
        fail(f"{what}: got non-number {actual!r}")
    if abs(actual - expected) > rel * abs(expected) + abs_tol:
        fail(f"{what}: got {actual:.9g}, expected {expected:.9g} (rel err {abs(actual-expected)/max(abs(expected),1e-300):.2e})")


def colebrook(re, rel_rough):
    f = 0.02
    for _ in range(200):
        nf = (-2.0 * math.log10(rel_rough / 3.7 + 2.51 / (re * math.sqrt(f)))) ** -2
        if abs(nf - f) < 1e-16:
            break
        f = nf
    return nf


def friction(re, rel_rough):
    return 64.0 / re if re < 2000.0 else colebrook(re, rel_rough)


# ---------------------------------------------------------------- document builder
class Net:
    def __init__(self):
        self.ops = []
        self.port_xyz = {}
        self.seg = {}  # id -> dict(from,to,D,eps,K)
        self.systems = {}

    def system(self, sid, medium, rho, mu):
        self.systems[sid] = (rho, mu)
        self.ops.append({"op": "create", "id": sid, "kind": "MepSystem",
                         "props": {"name": sid, "medium": medium, "density": rho, "viscosity": mu}})

    def port(self, pid, sys_id, xyz, kind, flow=None, pressure=None):
        props = {"system": {"ref": sys_id}, "x": xyz[0], "y": xyz[1], "z": xyz[2], "kind": kind}
        if flow is not None:
            props["flow"] = flow
        if pressure is not None:
            props["pressure"] = pressure
        self.port_xyz[pid] = xyz
        self.ops.append({"op": "create", "id": pid, "kind": "Port", "props": props})

    def pipe(self, sid, a, b, D, eps, K=None):
        props = {"from": {"ref": a}, "to": {"ref": b}, "shape": "round", "diameter": D, "roughness": eps}
        if K is not None:
            props["minor_k"] = K
        self.seg[sid] = dict(a=a, b=b, D=D, eps=eps, K=K or 0.0)
        self.ops.append({"op": "create", "id": sid, "kind": "Segment", "props": props})

    def length(self, sid):
        s = self.seg[sid]
        return math.dist(self.port_xyz[s["a"]], self.port_xyz[s["b"]])


def expected_dp(net, sid, q, rho, mu, f_override=None):
    """Signed pressure drop from `from` to `to` for signed flow q (closed form)."""
    s = net.seg[sid]
    D, L = s["D"], net.length(sid)
    A = math.pi * D * D / 4
    v = abs(q) / A
    re = rho * v * D / mu
    f = f_override if f_override is not None else friction(re, s["eps"] / D)
    dp = (f * L / D + s["K"]) * rho * v * v / 2
    return math.copysign(dp, q), v, re, f


def check_segment(net, segs, sid, q, rho, mu, tag, rel=1e-3):
    if sid not in segs:
        fail(f"{tag}: segment {sid} missing from output")
    s = segs[sid]
    dp, v, re, f = expected_dp(net, sid, q, rho, mu)
    close(s["flow"], q, rel, f"{tag} {sid} flow", abs_tol=1e-12)
    close(s["velocity"], v, rel, f"{tag} {sid} velocity")
    close(s["reynolds"], re, rel, f"{tag} {sid} reynolds")
    close(s["friction_factor"], f, rel, f"{tag} {sid} friction_factor")
    close(s["dp"], dp, rel, f"{tag} {sid} dp")


def check_self_consistency(net, segs, ports, tag, sources):
    """Every segment: velocity = |Q|/A, Re = rho v D / mu, f = Colebrook(Re) to 1e-9 (or 64/Re),
    dp = (fL/D + K) rho v^2/2 with the reported f, and p(from) - p(to) = dp. Nodal continuity."""
    for sid, s in net.seg.items():
        r = segs.get(sid) or fail(f"{tag}: segment {sid} missing")
        rho, mu = net.systems[net.port_sys[s["a"]]]
        A = math.pi * s["D"] ** 2 / 4
        close(r["velocity"], abs(r["flow"]) / A, 1e-6, f"{tag} {sid} velocity vs |flow|/A", abs_tol=1e-12)
        close(r["reynolds"], rho * r["velocity"] * s["D"] / mu, 1e-6, f"{tag} {sid} Re vs rho v D/mu", abs_tol=1e-9)
        if r["reynolds"] > 0:
            want_f = friction(r["reynolds"], s["eps"] / s["D"])
            close(r["friction_factor"], want_f, 1e-9, f"{tag} {sid} friction factor vs Colebrook/64-Re at reported Re (solved to 1e-12)")
            want_dp = (r["friction_factor"] * net.length(sid) / s["D"] + s["K"]) * rho * r["velocity"] ** 2 / 2
            close(abs(r["dp"]), want_dp, 1e-6, f"{tag} {sid} |dp| vs (fL/D+K)rho v^2/2", abs_tol=1e-9)
        if r["flow"] * r["dp"] < 0:
            fail(f"{tag} {sid}: flow {r['flow']} and dp {r['dp']} have opposite signs")
        pa, pb = ports[s["a"]]["pressure"], ports[s["b"]]["pressure"]
        close(pa - pb, r["dp"], 1e-6, f"{tag} {sid} p(from)-p(to) vs dp", abs_tol=1e-6 * max(abs(pa), abs(pb), 1.0))
    # continuity at every non-source port
    imbalance = {p: 0.0 for p in net.port_xyz}
    for sid, s in net.seg.items():
        q = segs[sid]["flow"]
        imbalance[s["a"]] -= q
        imbalance[s["b"]] += q
    total = sum(d for d in net.demand.values())
    for p, net_in in imbalance.items():
        if p in sources:
            continue
        if abs(net_in - net.demand.get(p, 0.0)) > 1e-6 * total:
            fail(f"{tag}: continuity violated at port {p}: inflow {net_in}, demand {net.demand.get(p, 0.0)}")


def finish_net(net):
    net.port_sys = {}
    net.demand = {}
    for op in net.ops:
        if op["kind"] == "Port":
            net.port_sys[op["id"]] = op["props"]["system"]["ref"]
            if op["props"]["kind"] == "sink":
                net.demand[op["id"]] = op["props"].get("flow", 0.0)


# ---------------------------------------------------------------- 1. single pipes, series, tree
def single_pipes():
    net = Net()
    cases = []  # (system, src, src_p, [(seg, q)], extra pressure checks)
    # (a) rough turbulent water, diagonal pipe (length 100 m from coordinates, includes z)
    net.system("W1", "water", 998.2, 1.002e-3)
    net.port("w1s", "W1", (0, 0, 1.5), "source", pressure=4e5)
    net.port("w1k", "W1", (60, 80, 1.5), "sink", flow=0.02)
    net.pipe("w1p", "w1s", "w1k", 0.1, 4.5e-5)
    # (b) hydraulically smooth (roughness 0) turbulent
    net.system("W2", "water", 998.2, 1.002e-3)
    net.port("w2s", "W2", (0, 10, 0), "source", pressure=3e5)
    net.port("w2k", "W2", (30, 10, 0), "sink", flow=0.003)
    net.pipe("w2p", "w2s", "w2k", 0.05, 0.0)
    # (c) laminar oil, Re ~ 46: dp = 32 mu L v / D^2
    net.system("O1", "water", 900.0, 0.5)
    net.port("o1s", "O1", (0, 20, 0), "source", pressure=2e5)
    net.port("o1k", "O1", (25, 20, 0), "sink", flow=0.001)
    net.pipe("o1p", "o1s", "o1k", 0.05, 1e-5)
    # (d) Re ~ 1900 -> laminar 64/Re ; (e) Re ~ 2600 -> Colebrook (laminar only below 2000)
    for tag, re in (("lo", 1900.0), ("hi", 2600.0)):
        rho, mu, D = 1000.0, 1e-3, 0.02
        q = re * mu / (rho * D) * math.pi * D * D / 4
        net.system("T" + tag, "water", rho, mu)
        net.port(tag + "s", "T" + tag, (0, 30 + (tag == "hi"), 0), "source", pressure=1e5)
        net.port(tag + "k", "T" + tag, (10, 30 + (tag == "hi"), 0), "sink", flow=q)
        net.pipe(tag + "p", tag + "s", tag + "k", D, 1e-5)
    # (f) minor loss K = 2.5 on top of friction
    net.system("W3", "water", 998.2, 1.002e-3)
    net.port("w3s", "W3", (0, 40, 0), "source", pressure=4e5)
    net.port("w3k", "W3", (0, 140, 0), "sink", flow=0.02)
    net.pipe("w3p", "w3s", "w3k", 0.1, 4.5e-5, K=2.5)
    # (g) air duct (round)
    net.system("A1", "air", 1.2, 1.81e-5)
    net.port("a1s", "A1", (50, 0, 3), "source", pressure=500.0)
    net.port("a1k", "A1", (50, 20, 3), "sink", flow=0.4)
    net.pipe("a1p", "a1s", "a1k", 0.3, 1.5e-4)
    # (h) series: source -> junction -> sink, two diameters
    net.system("S1", "water", 998.2, 1.002e-3)
    net.port("s1s", "S1", (100, 0, 0), "source", pressure=5e5)
    net.port("s1j", "S1", (140, 0, 0), "junction")
    net.port("s1k", "S1", (140, 30, 0), "sink", flow=0.008)
    net.pipe("s1a", "s1s", "s1j", 0.08, 4.5e-5)
    net.pipe("s1b", "s1j", "s1k", 0.05, 1.5e-6, K=1.0)
    # (i) branched tree, one pipe authored against the flow (to -> from), so its flow is negative
    net.system("B1", "water", 998.2, 1.002e-3)
    net.port("b1s", "B1", (200, 0, 0), "source", pressure=5e5)
    net.port("b1j", "B1", (250, 0, 0), "junction")
    net.port("b1k", "B1", (250, 40, 0), "sink", flow=0.006)
    net.port("b1m", "B1", (300, 0, 0), "sink", flow=0.004)
    net.pipe("b1a", "b1s", "b1j", 0.1, 4.5e-5)
    net.pipe("b1b", "b1j", "b1k", 0.065, 4.5e-5)
    net.pipe("b1c", "b1m", "b1j", 0.05, 4.5e-5)  # reversed authoring
    finish_net(net)

    d = project("single", net.ops)
    doc, segs, ports = solve(d)

    def chain(src, src_p, path, tag):
        """path: list of (segment, signed flow, downstream port)."""
        p = src_p
        sysid = net.port_sys[src]
        rho, mu = net.systems[sysid]
        close(ports[src]["pressure"], src_p, 1e-9, f"{tag}: source pressure")
        for sid, q, nxt in path:
            check_segment(net, segs, sid, q, rho, mu, tag)
            dp = expected_dp(net, sid, q, rho, mu)[0]
            p -= abs(dp)
            close(ports[nxt]["pressure"], p, 1e-3, f"{tag}: pressure at {nxt}", abs_tol=1e-6 * src_p)

    chain("w1s", 4e5, [("w1p", 0.02, "w1k")], "rough pipe")
    chain("w2s", 3e5, [("w2p", 0.003, "w2k")], "smooth pipe")
    chain("o1s", 2e5, [("o1p", 0.001, "o1k")], "laminar pipe")
    # explicit Hagen-Poiseuille cross-check of the closed form used above
    v = 0.001 / (math.pi * 0.05**2 / 4)
    close(segs["o1p"]["dp"], 32 * 0.5 * 25.0 * v / 0.05**2, 1e-3, "laminar dp vs Hagen-Poiseuille")
    close(segs["lop"]["friction_factor"], 64 / 1900.0, 1e-3, "Re 1900 friction factor = 64/Re")
    close(segs["hip"]["friction_factor"], colebrook(2600.0, 1e-5 / 0.02), 1e-3, "Re 2600 friction factor = Colebrook")
    chain("los", 1e5, [("lop", segs["lop"]["flow"], "lok")], "Re 1900")
    chain("w3s", 4e5, [("w3p", 0.02, "w3k")], "minor loss")
    no_k = expected_dp(net, "w3p", 0.02, 998.2, 1.002e-3)[0] - 2.5 * 998.2 * (0.02 / (math.pi * 0.01 / 4)) ** 2 / 2
    if abs(segs["w3p"]["dp"] - no_k) < 1e-3 * no_k:
        fail("minor_k ignored")
    chain("a1s", 500.0, [("a1p", 0.4, "a1k")], "air duct")
    chain("s1s", 5e5, [("s1a", 0.008, "s1j"), ("s1b", 0.008, "s1k")], "series")
    chain("b1s", 5e5, [("b1a", 0.010, "b1j"), ("b1b", 0.006, "b1k")], "tree")
    check_segment(net, segs, "b1c", -0.004, 998.2, 1.002e-3, "tree reversed pipe")
    p_m = ports["b1j"]["pressure"] - abs(expected_dp(net, "b1c", -0.004, 998.2, 1.002e-3)[0])
    close(ports["b1m"]["pressure"], p_m, 1e-3, "tree: pressure at b1m")
    sources = {"w1s", "w2s", "o1s", "los", "his", "w3s", "a1s", "s1s", "b1s"}
    check_self_consistency(net, segs, ports, "single/series/tree", sources)
    res = doc["continuity_residual"]
    if not isinstance(res, (int, float)) or not (0 <= res <= 1e-9):
        fail(f"continuity_residual should be a small non-negative number, got {res!r}")


def sloped_pipe():
    """Length comes from the 3D port coordinates (30, 0, 40) -> 50 m. Only the segment's loss is checked:
    how port pressures treat elevation is not part of the contract."""
    net = Net()
    net.system("V1", "water", 998.2, 1.002e-3)
    net.port("v1s", "V1", (5, 5, 0), "source", pressure=8e5)
    net.port("v1k", "V1", (35, 5, 40), "sink", flow=0.01)
    net.pipe("v1p", "v1s", "v1k", 0.08, 4.5e-5)
    finish_net(net)
    assert abs(net.length("v1p") - 50.0) < 1e-12
    doc, segs, ports = solve(project("sloped", net.ops))
    check_segment(net, segs, "v1p", 0.01, 998.2, 1.002e-3, "sloped pipe (3D length)")


# ---------------------------------------------------------------- 2. textbook two-loop Hardy-Cross
def hardy_cross(pipes, demand, src, tol=1e-15):
    """Classic Hardy-Cross: pipes {id: (a, b, r)} with h = r q|q|; returns signed flows a->b."""
    # initial flows: spanning-tree assignment satisfying continuity (tree from the source)
    import networkx as nx
    g = nx.Graph()
    for pid, (a, b, _) in pipes.items():
        g.add_edge(a, b, pid=pid)
    tree = nx.bfs_tree(g, src)
    q = {pid: 0.0 for pid in pipes}
    order = list(nx.dfs_postorder_nodes(tree, src))
    sub = {n: demand.get(n, 0.0) for n in g.nodes}
    for n in order:
        if n == src:
            continue
        parent = next(tree.predecessors(n))
        pid = g.edges[parent, n]["pid"]
        a, b, _ = pipes[pid]
        q[pid] = sub[n] if (a, b) == (parent, n) else -sub[n]
        sub[parent] += sub[n]
    loops = []
    for cyc in nx.cycle_basis(g):
        loop = []
        for i in range(len(cyc)):
            u, w = cyc[i], cyc[(i + 1) % len(cyc)]
            pid = g.edges[u, w]["pid"]
            loop.append((pid, 1.0 if pipes[pid][:2] == (u, w) else -1.0))
        loops.append(loop)
    for _ in range(10000):
        biggest = 0.0
        for loop in loops:
            num = sum(s * pipes[p][2] * q[p] * abs(q[p]) for p, s in loop)
            den = sum(2 * pipes[p][2] * abs(q[p]) for p, s in loop)
            dq = -num / den
            for p, s in loop:
                q[p] += s * dq
            biggest = max(biggest, abs(dq))
        if biggest < tol:
            break
    return q


def textbook_loop():
    net = Net()
    rho, mu = 1000.0, 1e-9  # Re ~ 1e11: Colebrook is fully rough, f = (-2 log10(eps/3.7D))^-2, constant
    net.system("HC", "water", rho, mu)
    xy = {"A": (0, 300), "B": (400, 300), "C": (900, 300), "D": (0, 0), "E": (400, 0), "F": (900, 0)}
    dem = {"B": 0.020, "C": 0.030, "D": 0.025, "E": 0.040, "F": 0.035}
    P0 = 6e5
    for n, (x, y) in xy.items():
        if n == "A":
            net.port(n, "HC", (x, y, 0), "source", pressure=P0)
        else:
            net.port(n, "HC", (x, y, 0), "sink", flow=dem[n])
    layout = [("AB", "A", "B", 0.25), ("BC", "B", "C", 0.15), ("AD", "A", "D", 0.2), ("EB", "E", "B", 0.1),
              ("CF", "C", "F", 0.1), ("DE", "D", "E", 0.2), ("FE", "F", "E", 0.125)]
    for pid, a, b, D in layout:
        net.pipe(pid, a, b, D, 0.001 * D * (1 + layout.index((pid, a, b, D)) % 3))  # eps/D in {1,2,3}e-3
    finish_net(net)
    fconst, r = {}, {}
    for pid, a, b, D in layout:
        s = net.seg[pid]
        fconst[pid] = (-2 * math.log10(s["eps"] / D / 3.7)) ** -2
        r[pid] = fconst[pid] * net.length(pid) / D * rho / 2 / (math.pi * D * D / 4) ** 2  # dp = r q|q|
    q = hardy_cross({pid: (a, b, r[pid]) for pid, a, b, D in layout}, dem, "A")
    # sanity: loop sums vanish and every pipe is far in the turbulent rough regime
    pres = {"A": P0}
    for pid in ("AB", "BC", "AD", "CF", "DE"):
        a, b = net.seg[pid]["a"], net.seg[pid]["b"]
        pres[b] = pres[a] - r[pid] * q[pid] * abs(q[pid])
    pres["E"] = pres["D"] - r["DE"] * q["DE"] * abs(q["DE"])
    assert abs((pres["E"] - r["EB"] * q["EB"] * abs(q["EB"])) - pres["B"]) < 1e-6
    assert abs((pres["F"] - r["FE"] * q["FE"] * abs(q["FE"])) - pres["E"]) < 1e-6
    # sanity: constant-f is the Colebrook solution at this Re to < 1e-5
    for pid, *_ in layout:
        D = net.seg[pid]["D"]
        re = rho * abs(q[pid]) / (math.pi * D * D / 4) * D / mu
        assert abs(colebrook(re, net.seg[pid]["eps"] / D) / fconst[pid] - 1) < 1e-5

    d = project("hardycross", net.ops)
    for extra, tag in (((), "newton"), (("--method", "hardy-cross"), "hardy-cross mode")):
        doc, segs, ports = solve(d, extra)
        for pid, *_ in layout:
            close(segs[pid]["flow"], q[pid], 1e-3, f"textbook loop ({tag}) flow {pid}", abs_tol=1e-7)
            close(segs[pid]["friction_factor"], fconst[pid], 1e-3, f"textbook loop ({tag}) f {pid}")
        for n in xy:
            close(ports[n]["pressure"], pres[n], 1e-3, f"textbook loop ({tag}) pressure {n}",
                  abs_tol=1e-3 * (P0 - min(pres.values())))
        check_self_consistency(net, segs, ports, f"textbook loop ({tag})", {"A"})
        if not (0 <= doc["continuity_residual"] <= 1e-9):
            fail(f"textbook loop ({tag}) continuity_residual {doc['continuity_residual']}")


# ---------------------------------------------------------------- 3. random networks vs EPANET
def random_network(rng):
    import networkx as nx
    nx_, ny_ = rng.randint(3, 4), rng.randint(2, 4)
    sp = rng.uniform(80, 200)
    nodes = {}
    for i in range(nx_):
        for j in range(ny_):
            nodes[f"n{i}_{j}"] = (i * sp + rng.uniform(-20, 20), j * sp + rng.uniform(-20, 20), 0.0)
    edges = []
    for i in range(nx_):
        for j in range(ny_):
            if i + 1 < nx_:
                edges.append((f"n{i}_{j}", f"n{i+1}_{j}"))
            if j + 1 < ny_:
                edges.append((f"n{i}_{j}", f"n{i}_{j+1}"))
    rng.shuffle(edges)
    g = nx.Graph(edges)
    for e in list(edges):  # drop some edges while staying connected, keeping loops
        if len(g.edges) > len(g.nodes) + 1 and rng.random() < 0.3:
            g.remove_edge(*e)
            if not nx.is_connected(g):
                g.add_edge(*e)
    names = sorted(nodes)
    sources = [names[0]] + ([names[-1]] if rng.random() < 0.4 else [])
    pressures = {s: rng.uniform(3e5, 6e5) for s in sources}
    demand = {n: rng.uniform(0.002, 0.012) for n in names if n not in sources and rng.random() < 0.8}
    pipes = []
    for k, (a, b) in enumerate(sorted(tuple(sorted(e)) for e in g.edges)):
        if rng.random() < 0.5:
            a, b = b, a
        D = rng.choice([0.1, 0.125, 0.15, 0.2, 0.25])
        eps = rng.choice([1.5e-6, 4.5e-5, 1.5e-4, 5e-4])
        K = rng.choice([0.0, 0.0, 0.5, 2.0])
        pipes.append((f"p{k}", a, b, D, eps, K))
    return nodes, sources, pressures, demand, pipes


def epanet(nodes, sources, pressures, demand, pipes, rho):
    import tempfile
    import wntr
    wn = wntr.network.WaterNetworkModel()
    wn.options.hydraulic.inpfile_units = "LPS"
    wn.options.hydraulic.headloss = "D-W"
    wn.options.hydraulic.accuracy = 1e-7
    wn.options.hydraulic.trials = 500
    for n, xyz in nodes.items():
        if n in sources:
            wn.add_reservoir(n, base_head=pressures[n] / (rho * G_EPANET), coordinates=xyz[:2])
        else:
            wn.add_junction(n, base_demand=demand.get(n, 0.0), elevation=0.0, coordinates=xyz[:2])
    for pid, a, b, D, eps, K in pipes:
        wn.add_pipe(pid, a, b, length=math.dist(nodes[a], nodes[b]), diameter=D, roughness=eps, minor_loss=K)
    d = tempfile.mkdtemp(dir=TMP)
    r = wntr.sim.EpanetSimulator(wn).run_sim(file_prefix=os.path.join(d, "net"))
    head = r.node["head"].iloc[0].to_dict()
    flow = r.link["flowrate"].iloc[0].to_dict()
    return {n: float(h) * rho * G_EPANET for n, h in head.items()}, {k: float(v) for k, v in flow.items()}


def random_networks():
    rng = random.Random(46_2024)
    rho = 998.2
    mu = rho * NU_EPANET  # same kinematic viscosity as EPANET's reference water
    done, tries = 0, 0
    while done < 10:
        tries += 1
        if tries > 200:
            fail("oracle: could not generate 10 turbulent random networks (judge bug)")
        nodes, sources, pressures, demand, pipes = random_network(rng)
        p_epa, q_epa = epanet(nodes, sources, pressures, demand, pipes, rho)
        # EPANET switches to a transition interpolation below Re 4000; keep fully turbulent networks only
        if any(rho * abs(q_epa[pid]) / (math.pi * D * D / 4) * D / mu < 5000 for pid, a, b, D, eps, K in pipes):
            continue
        done += 1
        net = Net()
        net.system("R", "water", rho, mu)
        for n, xyz in nodes.items():
            if n in sources:
                net.port(n, "R", xyz, "source", pressure=pressures[n])
            elif n in demand:
                net.port(n, "R", xyz, "sink", flow=demand[n])
            else:
                net.port(n, "R", xyz, "junction")
        for pid, a, b, D, eps, K in pipes:
            net.pipe(pid, a, b, D, eps, K)
        finish_net(net)
        d = project(f"random{done}", net.ops)
        doc, segs, ports = solve(d)
        tag = f"random network {done} (vs EPANET)"
        qmax = max(abs(v) for v in q_epa.values())
        # EPANET's Swamee-Jain friction factor differs from Colebrook by up to ~0.7 %, which shifts small
        # loop-correction flows by a few % of their own value; 1 % is therefore taken of the largest flow.
        # (Exact agreement with the Colebrook solution is enforced by check_self_consistency below.)
        for pid, *_ in pipes:
            close(segs[pid]["flow"], q_epa[pid], 0.0, f"{tag} flow {pid}", abs_tol=1e-2 * qmax)
        pmax = max(pressures.values())
        for n in nodes:
            close(ports[n]["pressure"], p_epa[n], 1e-2, f"{tag} pressure {n}", abs_tol=1e-3 * pmax)
        check_self_consistency(net, segs, ports, tag, set(sources))
        if not (0 <= doc["continuity_residual"] <= 1e-9):
            fail(f"{tag}: continuity_residual {doc['continuity_residual']}")


def oracle_selftest():
    # EPANET on one pipe agrees with our Colebrook closed form within its Swamee-Jain error (< 1 %)
    nodes = {"s": (0, 0, 0), "k": (100, 0, 0)}
    rho = 998.2
    p, q = epanet(nodes, ["s"], {"s": 5e5}, {"k": 0.02}, [("p", "s", "k", 0.1, 4.5e-5, 1.0)], rho)
    net = Net()
    net.port_xyz = nodes
    net.seg["p"] = dict(a="s", b="k", D=0.1, eps=4.5e-5, K=1.0)
    want = expected_dp(net, "p", 0.02, rho, rho * NU_EPANET)[0]
    if abs((5e5 - p["k"]) / want - 1) > 0.01:
        fail(f"oracle self-test: EPANET single-pipe drop {5e5 - p['k']} vs closed form {want}")


if __name__ == "__main__":
    try:
        oracle_selftest()
        single_pipes()
        sloped_pipe()
        textbook_loop()
        random_networks()
    except (KeyError, TypeError, ValueError, IndexError, AttributeError) as e:
        fail(f"malformed bim output or missing entity: {type(e).__name__}: {e}")
    print("T4.6 judge: all checks passed")
