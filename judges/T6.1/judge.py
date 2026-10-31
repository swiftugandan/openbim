"""External judge for T6.1 (bim-query): drives `bim query` black-box and compares with networkx.

Usage: python -I judge.py <path-to-bim> <tmpdir>
Every expected value comes from the generator below (closed form) or from networkx.
"""
import json
import os
import random
import signal
import subprocess
import sys

import networkx as nx

BIM, TMP = sys.argv[1], sys.argv[2]
CALL_TIMEOUT = 30


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def _overall_timeout(*_):
    fail("judge exceeded its overall time budget (bim query hung or is far too slow)")


signal.signal(signal.SIGALRM, _overall_timeout)
signal.alarm(100)  # hard backstop; the judge is sized to finish well under 60 s


def run(args):
    try:
        p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=CALL_TIMEOUT)
    except subprocess.TimeoutExpired:
        fail(f"'bim {' '.join(args)}' did not finish within {CALL_TIMEOUT}s")
    return p


def project(name, entities):
    """entities: list of (id, kind, props) in the JSON value encoding of spec/schema.md."""
    d = os.path.join(TMP, name)
    p = run(["init", d])
    if p.returncode != 0:
        fail(f"bim init {name} exited {p.returncode}: {p.stderr}")
    ops = [{"op": "create", "id": i, "kind": k, "props": props} for i, k, props in entities]
    if not ops:  # an empty project needs no apply (whether an empty ops list is accepted is not pinned)
        return d
    path = os.path.join(TMP, f"{name}.json")
    with open(path, "w") as f:
        json.dump({"schema": 1, "ops": ops}, f)
    p = run(["apply", d, path])
    if p.returncode != 0:
        fail(f"bim apply {name} exited {p.returncode}: {p.stderr}")
    return d


def apply_ops(d, ops, tag):
    path = os.path.join(TMP, f"{tag}.json")
    with open(path, "w") as f:
        json.dump({"schema": 1, "ops": ops}, f)
    p = run(["apply", d, path])
    if p.returncode != 0:
        fail(f"bim apply {tag} exited {p.returncode}: {p.stderr}")


_prog_n = [0]


def write_program(text):
    _prog_n[0] += 1
    path = os.path.join(TMP, f"prog{_prog_n[0]}.dl")
    with open(path, "w") as f:
        f.write(text)
    return path


def query(d, program, atom, what, naive=True):
    """Run the query semi-naive and (for recursive programs) naive; both must agree. Returns rows."""
    prog = write_program(program)
    results = []
    for extra in ([], ["--naive"]) if naive else ([],):
        args = ["query", d, prog, atom, "--json", *extra]
        p = run(args)
        mode = "naive" if extra else "semi-naive"
        if p.returncode != 0:
            fail(f"{what}: {mode} 'bim query … {atom}' exited {p.returncode}: {p.stderr.strip()[:500]}")
        try:
            doc = json.loads(p.stdout)
        except json.JSONDecodeError as e:
            fail(f"{what}: {mode} stdout is not one JSON document ({e}): {p.stdout[:300]!r}")
        if not isinstance(doc, dict) or not isinstance(doc.get("rows"), list):
            fail(f'{what}: {mode} output must be {{"rows": [...]}}, got {p.stdout[:300]!r}')
        rows = doc["rows"]
        for r in rows:
            if not isinstance(r, list):
                fail(f"{what}: {mode} row is not a list: {r!r}")
        if rows != sorted(rows):
            fail(f"{what}: {mode} rows are not sorted lexicographically")
        if any(rows[i] == rows[i + 1] for i in range(len(rows) - 1)):
            fail(f"{what}: {mode} rows contain duplicates")
        results.append(rows)
    if len(results) == 2 and results[0] != results[1]:
        a, b = set(map(tuple, results[0])), set(map(tuple, results[1]))
        fail(f"{what}: semi-naive and naive evaluation disagree "
             f"(only semi-naive: {sorted(a - b)[:5]}, only naive: {sorted(b - a)[:5]})")
    return results[0]


def expect_rows(d, program, atom, expected, what, naive=True):
    rows = query(d, program, atom, what, naive)
    exp = sorted(set(map(tuple, expected)))
    got = [tuple(r) for r in rows]
    if got != exp:
        g, e = set(got), set(exp)
        fail(f"{what}: {len(got)} rows vs {len(exp)} expected; missing {sorted(e - g)[:5]}, "
             f"unexpected {sorted(g - e)[:5]}")


def expect_error(d, program, atom, what):
    prog = write_program(program)
    p = run(["query", d, prog, atom, "--json"])
    if p.returncode != 2:
        fail(f"{what}: expected exit 2 (rejected program), got {p.returncode}; stdout {p.stdout[:200]!r}")
    if not p.stderr.strip():
        fail(f"{what}: rejected program printed no error message on stderr")


def closure_pairs(edges, nodes=()):
    """Pairs (u, v) with a path of length >= 1 from u to v (networkx, reflexive=False)."""
    g = nx.DiGraph()
    g.add_nodes_from(nodes)
    g.add_edges_from(edges)
    return list(nx.transitive_closure(g, reflexive=False).edges())


PATH_RULES = """
path(X, Y) :- edge(X, Y).
path(X, Z) :- path(X, Y), edge(Y, Z).
"""

# ---------------------------------------------------------------- 1. pure Datalog over facts
rng = random.Random(6101)
empty = project("empty", [])
nodes = [f"n{i}" for i in range(80)]
edges = set()
while len(edges) < 170:
    a, b = rng.choice(nodes), rng.choice(nodes)
    edges.add((a, b))
edges = sorted(edges)
fact_lines = [f'edge("{a}", "{b}").' for a, b in edges]
fact_lines += fact_lines[:20]  # duplicate facts must not duplicate rows
facts = "\n".join(fact_lines) + "\n"
closure = closure_pairs(edges)
expect_rows(empty, facts + PATH_RULES, "path(X, Y)", closure, "transitive closure of facts")
# Right-recursive and doubly-recursive formulations derive the same closure.
expect_rows(empty, facts + "path(X, Y) :- edge(X, Y).\npath(X, Z) :- edge(X, Y), path(Y, Z).\n",
            "path(X, Y)", closure, "right-recursive closure")
expect_rows(empty, facts + "path(X, Y) :- edge(X, Y).\npath(X, Z) :- path(X, Y), path(Y, Z).\n",
            "path(X, Y)", closure, "non-linear closure")
# Repeated variable in a body atom.
on_cycle = [(u,) for u, v in closure if u == v]
expect_rows(empty, facts + PATH_RULES + "cyc(X) :- path(X, X).\n", "cyc(X)", on_cycle, "nodes on cycles")
# Constant in a body atom.
expect_rows(empty, facts + PATH_RULES + 'from0(Y) :- path("n0", Y).\n', "from0(Y)",
            [(v,) for u, v in closure if u == "n0"], "reachability from a constant")
# Stratified negation: complement of the closure over the node set.
used = sorted({x for e in edges for x in e})
neg_prog = facts + PATH_RULES + """
node(X) :- edge(X, Y).
node(Y) :- edge(X, Y).
unreach(X, Y) :- node(X), node(Y), not path(X, Y).
"""
cl = set(closure)
expect_rows(empty, neg_prog, "unreach(X, Y)",
            [(a, b) for a in used for b in used if (a, b) not in cl], "stratified negation (complement)")
# Two-level negation chain.
expect_rows(empty, neg_prog + "isolated(X) :- node(X), not reached(X).\nreached(Y) :- path(X, Y).\n",
            "isolated(X)", [(n,) for n in used if not any(v == n for _, v in closure)],
            "negation over a derived relation")
# Numbers: facts carry numbers, numeric constants match by value, rows report JSON numbers.
num_prog = 'w("a", 3.0).\nw("b", 3.5).\nw("c", 3).\nw("d", -2).\nthree(X) :- w(X, 3).\nwv(X, V) :- w(X, V).\n'
expect_rows(empty, num_prog, "three(X)", [("a",), ("c",)], "numeric constant matching", naive=False)
expect_rows(empty, num_prog, "wv(X, V)", [("a", 3.0), ("b", 3.5), ("c", 3.0), ("d", -2.0)], "numeric rows", naive=False)
# Constant in a rule head; two rules deriving the same tuple.
expect_rows(empty, 'e("x").\ne("y").\nf("x").\nt(X, "tag") :- e(X).\nt(X, "tag") :- f(X).\n', "t(X, Y)",
            [("x", "tag"), ("y", "tag")], "head constants and union dedup", naive=False)
# Unstratifiable negation and syntax errors are rejected with exit 2.
expect_error(empty, facts + 'p(X) :- edge(X, Y), not q(X).\nq(X) :- edge(X, Y), not p(X).\n', "p(X)",
             "mutual negation through recursion")
expect_error(empty, facts + "p(X) :- edge(X, Y), not p(Y).\n", "p(X)", "self-negation")
expect_error(empty, facts + "p(X) :- edge(X, Y.\n", "p(X)", "unbalanced parenthesis (syntax error)")

# ---------------------------------------------------------------- 2. document relations: entity/prop/ref
rng = random.Random(6102)
N = 40
issues = []
iedges = set()
for i in range(N):
    k = rng.randint(0, 4)
    targets = sorted({f"I{rng.randrange(N - 1)}" for _ in range(k)})  # never targets I39
    for t in targets:
        iedges.add((f"I{i}", t))
    issues.append((f"I{i}", "Issue", {
        "title": f"issue {i}", "status": rng.choice(["open", "closed"]),
        "priority": float(rng.randint(1, 3)), "entities": [{"ref": t} for t in targets]}))
status = {i: p["status"] for i, _, p in issues}
prio = {i: p["priority"] for i, _, p in issues}
issues.append(("Lv", "Level", {"elevation": 0.0}))
docd = project("issues", issues)
apply_ops(docd, [{"op": "delete", "id": "I39"}], "issues-del")
live = [f"I{i}" for i in range(N - 1)]
iedges = sorted((a, b) for a, b in iedges if a != "I39")

expect_rows(docd, "", "entity(X, K)", [(i, "Issue") for i in live] + [("Lv", "Level")], "entity relation", naive=False)
expect_rows(docd, 'r(X, Y) :- ref(X, "entities", Y).\n', "r(X, Y)", iedges, "ref over List[Ref] props", naive=False)
expect_rows(docd, 'st(X, S) :- prop(X, "status", S).\n', "st(X, S)", [(i, status[i]) for i in live],
            "prop relation (Text)", naive=False)
expect_rows(docd, 'pr(X, P) :- prop(X, "priority", P).\n', "pr(X, P)", [(i, prio[i]) for i in live],
            "prop relation (Num)", naive=False)
expect_rows(docd, 'p2(X) :- prop(X, "priority", 2).\n', "p2(X)", [(i,) for i in live if prio[i] == 2.0],
            "numeric constant against a Num prop", naive=False)
ref_reach = 'edge(X, Y) :- ref(X, "entities", Y).\n' + PATH_RULES
icl = closure_pairs(iedges)
expect_rows(docd, ref_reach, "path(X, Y)", icl, "transitive closure of document refs")
expect_rows(docd, ref_reach + 'cold(X) :- entity(X, "Issue"), prop(X, "status", "open"), not path("I0", X).\n',
            "cold(X)", [(i,) for i in live if status[i] == "open" and ("I0", i) not in set(icl)],
            "negation over document-derived closure")

# ---------------------------------------------------------------- 3. rooms, adjacency, bounds
WT = ("WT", "WallType", {"layer_thickness": [0.2], "layer_material": ["concrete"], "layer_conductivity": [1.0]})
ents = [("L1", "Level", {"elevation": 0.0}), ("L2", "Level", {"elevation": 3.0}),
        ("L3", "Level", {"elevation": 6.0}), WT]


def wall(wid, lvl, x1, y1, x2, y2):
    return (wid, "Wall", {"level": {"ref": lvl}, "x1": float(x1), "y1": float(y1), "x2": float(x2),
                          "y2": float(y2), "height": 2.8, "type": {"ref": "WT"}})


XS, YS = [0, 4, 8, 12], [0, 4, 8]
for j, y in enumerate(YS):  # full-length horizontal walls
    ents.append(wall(f"H{j}", "L1", 0, y, 12, y))
for i, x in enumerate(XS):  # vertical walls split at every horizontal (T and L joins only)
    for j in range(len(YS) - 1):
        ents.append(wall(f"V{i}{j}", "L1", x, YS[j], x, YS[j + 1]))
ents.append(wall("STUB", "L1", 1, 0, 1, 1.5))  # dangling wall: bounds no room
ents += [wall("U0", "L2", 0, 0, 4, 0), wall("U1", "L2", 4, 0, 4, 4),
         wall("U2", "L2", 4, 4, 0, 4), wall("U3", "L2", 0, 4, 0, 0)]
roomd = project("rooms", ents)
# Room (i, j) occupies [XS[i], XS[i+1]] x [YS[j], YS[j+1]]; its bounding walls identify it.
cells = {}
for i in range(3):
    for j in range(2):
        cells[frozenset({f"H{j}", f"H{j + 1}", f"V{i}{j}", f"V{i + 1}{j}"})] = ("L1", (i, j))
cells[frozenset({"U0", "U1", "U2", "U3"})] = ("L2", (0, 0))

bounds_rows = query(roomd, "", "bounds(W, R)", "bounds relation", naive=False)
room_rows = query(roomd, "", "room(R, L)", "room relation", naive=False)
walls_of = {}
for w, r in bounds_rows:
    walls_of.setdefault(r, set()).add(w)
if "STUB" in {w for w, _ in bounds_rows}:
    fail("bounds: the dangling wall STUB must not bound any room")
room_level = {r: l for r, l in room_rows}
if len(room_rows) != len(room_level):
    fail(f"room relation lists a room twice: {room_rows}")
if set(room_level) != set(walls_of):
    fail(f"rooms from room() {sorted(room_level)} differ from rooms in bounds() {sorted(walls_of)}")
cell_of = {}
for r, ws in walls_of.items():
    key = frozenset(ws)
    if key not in cells:
        fail(f"bounds: room {r} is bounded by {sorted(ws)}, which is not one grid cell")
    cell_of[r] = cells[key]
if sorted(cell_of.values()) != sorted(cells.values()):
    fail(f"expected 7 rooms (6 on L1, 1 on L2), got {sorted(cell_of.items())}")
for r, (lvl, _) in cell_of.items():
    if room_level[r] != lvl:
        fail(f"room {r} reported on level {room_level[r]}, expected {lvl}")
    if not r.startswith(lvl + "/") or not r[len(lvl) + 1:].isdigit():
        fail(f"room id {r!r} is not '<level id>/<n>'")
# Numbering follows ascending centroid (x, then y), consecutively; the start index is not pinned.
l1 = sorted((r for r in cell_of if cell_of[r][0] == "L1"), key=lambda r: int(r.split("/")[1]))
centroid = lambda r: (XS[cell_of[r][1][0]] + 2, YS[cell_of[r][1][1]] + 2)
if [centroid(r) for r in l1] != sorted(centroid(r) for r in l1):
    fail(f"rooms on L1 are not numbered by ascending centroid (x, then y): {l1}")
ns = [int(r.split("/")[1]) for r in l1]
if ns != list(range(ns[0], ns[0] + len(ns))):
    fail(f"room numbers on L1 are not consecutive: {ns}")

name_of = {cell: r for r, cell in cell_of.items()}
grid = nx.grid_2d_graph(3, 2)
adj = [(name_of[("L1", a)], name_of[("L1", b)]) for a, b in grid.edges()]
adj += [(b, a) for a, b in adj]
expect_rows(roomd, "", "adjacent(A, B)", adj, "adjacent relation (symmetric, grid neighbours only)", naive=False)
expect_rows(roomd, "conn(X, Y) :- adjacent(X, Y).\nconn(X, Z) :- conn(X, Y), adjacent(Y, Z).\n", "conn(X, Y)",
            closure_pairs(adj, list(cell_of)), "closure of adjacency")
expect_rows(roomd, 'lone(R) :- room(R, L), not hasnb(R).\nhasnb(R) :- adjacent(R, S).\n', "lone(R)",
            [(name_of[("L2", (0, 0))],)], "rooms with no adjacent room", naive=False)

# ---------------------------------------------------------------- 4. feeds on an MEP tree
rng = random.Random(6104)
ents = [("S", "MepSystem", {"name": "cw", "medium": "water", "density": 998.0, "viscosity": 1.0e-3})]
pos = {"P0": (0.0, 0.0)}
parent = {}
NP = 25
for k in range(1, NP):
    par = f"P{rng.randrange(k)}"
    px, py = pos[par]
    pos[f"P{k}"] = (px + rng.uniform(1.0, 4.0), py + rng.uniform(-3.0, 3.0))
    parent[f"P{k}"] = par
children = {p for p in parent.values()}
for p, (x, y) in pos.items():
    if p == "P0":
        props = {"kind": "source", "pressure": 3.0e5}
    elif p in children:
        props = {"kind": "junction"}
    else:
        props = {"kind": "sink", "flow": 0.0005}
    ents.append((p, "Port", {"system": {"ref": "S"}, "x": x, "y": y, "z": 0.0, **props}))
tree_edges = []
reversed_segments = 0
for c, par in sorted(parent.items(), key=lambda t: int(t[0][1:])):
    a, b = (c, par) if rng.random() < 0.4 else (par, c)  # some segments authored against the flow
    reversed_segments += a == c
    ents.append((f"G{c}", "Segment", {"from": {"ref": a}, "to": {"ref": b}, "shape": "round",
                                      "diameter": 0.1, "roughness": 1.5e-5}))
    tree_edges.append((par, c))
mepd = project("mep", ents)
if reversed_segments == 0:
    fail("generator produced no reversed segment (fix seed)")
expect_rows(mepd, "f(A, B) :- feeds(A, B).\n", "f(A, B)", tree_edges,
            "feeds follows solved flow direction (source towards sinks)", naive=False)
tree = nx.DiGraph(tree_edges)
expect_rows(mepd, 'down(X, Y) :- feeds(X, Y).\ndown(X, Z) :- down(X, Y), feeds(Y, Z).\nsrc(Y) :- down("P0", Y).\n',
            "src(Y)", [(v,) for v in nx.descendants(tree, "P0")], "reachability from the source")
expect_rows(mepd, "leaf(X) :- entity(X, \"Port\"), not up(X).\nup(X) :- feeds(X, Y).\n", "leaf(X)",
            [(p,) for p in pos if tree.out_degree(p) == 0], "ports feeding nothing (negation)", naive=False)

# ---------------------------------------------------------------- 5. bears_on on a regular frame
ents = [("G", "Level", {"elevation": 0.0}), ("F1", "Level", {"elevation": 3.0}),
        ("F2", "Level", {"elevation": 6.0}),
        ("M", "Material", {"name": "S355", "E": 2.1e11, "G": 8.1e10, "density": 7850.0, "fy": 3.55e8}),
        ("ULS", "LoadCombination", {"name": "ULS", "cases": ["D"], "factors": [1.35]})]
GX, GY = [0.0, 5.0, 10.0], [0.0, 5.0]
storeys = [("G", "F1"), ("F1", "F2")]
bears = []
for i, x in enumerate(GX):
    for j, y in enumerate(GY):
        ents.append((f"SUP{i}{j}", "Support", {"x": x, "y": y, "z": 0.0, **{k: True for k in
                                                ["ux", "uy", "uz", "rx", "ry", "rz"]}}))
        for s, (base, top) in enumerate(storeys):
            cid = f"C{s}{i}{j}"
            ents.append((cid, "Column", {"x": x, "y": y, "base": {"ref": base}, "top": {"ref": top},
                                         "section_b": 0.3, "section_h": 0.3, "material": {"ref": "M"}}))
            bears.append((cid, f"C{s - 1}{i}{j}" if s else f"SUP{i}{j}"))
for s, lvl in enumerate(["F1", "F2"]):
    beams_at = {}
    def beam(bid, a, b):
        (i1, j1), (i2, j2) = a, b
        ents.append((bid, "Beam", {"level": {"ref": lvl}, "x1": GX[i1], "y1": GY[j1], "x2": GX[i2], "y2": GY[j2],
                                   "section_b": 0.2, "section_h": 0.4, "material": {"ref": "M"}}))
        bears.append((bid, f"C{s}{i1}{j1}"))
        bears.append((bid, f"C{s}{i2}{j2}"))
    for j in range(2):
        for i in range(2):
            beam(f"BX{s}{i}{j}", (i, j), (i + 1, j))
    for i in range(3):
        beam(f"BY{s}{i}", (i, 0), (i, 1))
    for b in range(2):  # one slab per bay, carried by the four beams around it
        x0, x1 = GX[b], GX[b + 1]
        sid = f"SL{s}{b}"
        ents.append((sid, "Slab", {"level": {"ref": lvl}, "boundary": [x0, 0.0, x1, 0.0, x1, 5.0, x0, 5.0],
                                   "thickness": 0.2}))
        ents.append((f"AL{s}{b}", "AreaLoad", {"host": {"ref": sid}, "pressure": 5000.0, "case": "D"}))
        bears += [(sid, f"BX{s}{b}0"), (sid, f"BX{s}{b}1"), (sid, f"BY{s}{b}"), (sid, f"BY{s}{b + 1}")]
framed = project("frame", ents)
expect_rows(framed, "b(X, Y) :- bears_on(X, Y).\n", "b(X, Y)", bears, "bears_on load-path edges", naive=False)
lp = closure_pairs(bears)
expect_rows(framed, "lp(X, Y) :- bears_on(X, Y).\nlp(X, Z) :- lp(X, Y), bears_on(Y, Z).\n"
            'grounded(X) :- lp(X, S), entity(S, "Support").\n', "grounded(X)",
            sorted({(u,) for u, v in lp if v.startswith("SUP")}), "every element reaches a support")
expect_rows(framed, "lp(X, Y) :- bears_on(X, Y).\nlp(X, Z) :- lp(X, Y), bears_on(Y, Z).\n", "lp(X, Y)", lp,
            "closure of the load path")

print("T6.1 judge: all checks passed")
