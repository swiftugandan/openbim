"""T5.7 judge: declarative code-rule packs, driven from Python. Builders must not edit judges/.

Usage: python -I -B t57.py <bim> <repo-root> <tmpdir>

Fixtures sit 1e-6 inside and 1e-6 outside every limit of the three shipped packs (spec/plugins.md):
ibc (corridor 1.118, door 0.813, riser 0.1778, tread 0.2794), eurocode (blondel 0.60/0.65, wall U 0.35),
accessibility (door 0.85, corridor 1.5, ramp slope 1/12). Measured values come from closed forms
(room widths from centreline rectangles minus wall thickness, rotated as well as axis-aligned;
riser = rise / count; U = 1/(0.13 + sum t/lambda + 0.04); slope = rise / length) and, for egress
distance, from an independent shapely + networkx visibility-graph geodesic. A judge-authored pack
at a path covers the rest of the vocabulary (min_area, min_clear_height, riser_min,
egress_distance_max), `where` filtering, hot-swapping (edit between runs, same binary) and
unknown-check rejection (exit 2).
"""
import glob
import json
import math
import os
import subprocess
import sys

import networkx as nx
import shapely.geometry as sg

EPS = 1e-6     # how far inside/outside each limit the fixtures sit
TOL = 1e-9     # measured values vs closed forms
T = 0.2        # wall thickness of room walls


class Fail(Exception):
    pass


def need(c, msg):
    if not c:
        raise Fail(msg)


# ------------------------------------------------------------------------------------------ fixture builder
class Doc:
    def __init__(self):
        self.ops = []
        self.expect = {}  # (check-key, entity) -> measured, check-key like "Space.min_width"
        self.add("L0", "Level", {"elevation": 0.0})
        self.add("WT", "WallType", {"layer_thickness": [T], "layer_material": ["Block"], "layer_conductivity": [1.0]})
        self.levels = {}

    def add(self, i, kind, props):
        self.ops.append({"op": "create", "id": i, "kind": kind, "props": props})

    def level(self, elev):
        key = repr(elev)
        if key not in self.levels:
            lid = f"L{len(self.levels) + 1}"
            self.levels[key] = lid
            self.add(lid, "Level", {"elevation": elev})
        return self.levels[key]

    def room(self, name, ox, oy, length, width, angle=0.0, use="office", wall_type="WT", level="L0"):
        """Closed rectangle of 4 walls (centreline length x width, L-joined), Space at its centre.
        Returns (wall ids, inner polygon, transform)."""
        c, s = math.cos(angle), math.sin(angle)
        tr = lambda x, y: (ox + c * x - s * y, oy + s * x + c * y)  # noqa: E731
        pts = [tr(0, 0), tr(length, 0), tr(length, width), tr(0, width)]
        walls = []
        for k in range(4):
            (x1, y1), (x2, y2) = pts[k], pts[(k + 1) % 4]
            wid = f"{name}-w{k}"
            self.add(wid, "Wall", {"level": {"ref": level}, "x1": x1, "y1": y1, "x2": x2, "y2": y2, "height": 2.8,
                                    "type": {"ref": wall_type}})
            walls.append(wid)
        cx, cy = tr(length / 2, width / 2)
        self.add(name, "Space", {"level": {"ref": level}, "x": cx, "y": cy, "name": name, "use": use})
        inner = sg.Polygon([tr(T / 2, T / 2), tr(length - T / 2, T / 2), tr(length - T / 2, width - T / 2),
                            tr(T / 2, width - T / 2)])
        return walls, inner

    def opening(self, name, host, kind, width, offset=1.0):
        self.add(name, "Opening", {"host": {"ref": host}, "kind": kind, "offset": offset, "sill": 0.0 if kind == "door" else 0.9,
                                   "width": width, "height": 2.1 if kind == "door" else 1.0})

    def stair(self, name, x, rise, count, tread):
        top = self.level(rise)
        self.add(name, "Stair", {"base": {"ref": "L0"}, "top": {"ref": top}, "x": x, "y": -30.0, "angle": 0.0,
                                 "width": 1.2, "riser_count": count, "tread": tread})
        return (rise - 0.0) / count

    def ramp(self, name, x, rise, length):
        top = self.level(rise)
        self.add(name, "Ramp", {"base": {"ref": "L0"}, "top": {"ref": top}, "x": x, "y": -60.0, "angle": 0.0,
                                "width": 1.5, "length": length})
        return (rise - 0.0) / length

    def write(self, path):
        json.dump({"schema": 1, "ops": self.ops}, open(path, "w"), indent=1)


def corridor_width(inner_width):
    """centreline width giving the requested clear (inner-face) width"""
    return inner_width + T


def u_value(layers):
    return 1.0 / (0.13 + sum(t / lam for t, lam in layers) + 0.04)


# ------------------------------------------------------------------------------------------ shipped packs
SPEC_PACKS = {
    "ibc": {("Space", "min_width"): 1.118, ("Opening", "min_width"): 0.813, ("Stair", "riser_max"): 0.1778,
            ("Stair", "tread_min"): 0.2794},
    "eurocode": {("Stair", "blondel"): [0.60, 0.65], ("Wall", "u_value_max"): 0.35},
    "accessibility": {("Opening", "min_width"): 0.85, ("Space", "min_width"): 1.5, ("Ramp", "slope_max"): 1.0 / 12.0},
}


def shipped_rules(root):
    packs = {}
    for p in glob.glob(os.path.join(root, "crates/standards/codes/packs/*.json")):
        try:
            d = json.load(open(p))
        except ValueError as e:
            raise Fail(f"shipped pack {p} is not JSON: {e}")
        packs[d.get("pack")] = d
    out = {}
    for name, want in SPEC_PACKS.items():
        need(name in packs, f"no shipped pack with \"pack\": \"{name}\" in crates/standards/codes/packs/")
        rules = packs[name].get("rules", [])
        got = {}
        for r in rules:
            key = (r.get("applies_to"), r.get("check"))
            need(key not in got, f"pack {name}: two rules for {key}")
            got[key] = r
        need(set(got) == set(want), f"pack {name}: rules {sorted(got)}, spec/plugins.md lists {sorted(want)}")
        for key, lim in want.items():
            gl = got[key].get("limit")
            ok = (isinstance(gl, list) and isinstance(lim, list) and len(gl) == 2 and
                  all(abs(a - b) <= 1e-12 for a, b in zip(gl, lim))) or \
                 (isinstance(gl, (int, float)) and not isinstance(lim, list) and abs(gl - lim) <= 1e-12)
            need(ok, f"pack {name}: {key} limit {gl}, spec {lim}")
        out[name] = {key: (r["id"], r["limit"]) for key, r in got.items()}
    return out


# ------------------------------------------------------------------------------------------ fixtures per pack
def ibc_doc(outside=True):
    d = Doc()
    exp = {}
    lim = SPEC_PACKS["ibc"]
    _, inner = d.room("ibc-corr-in", 0, 0, 8.0, corridor_width(lim[("Space", "min_width")] + EPS), use="corridor")
    if outside:  # rotated 30 degrees: measured on the minimum-area bounding rectangle, not the axis box
        d.room("ibc-corr-out", 20, 0, 8.0, corridor_width(lim[("Space", "min_width")] - EPS), angle=math.radians(30),
               use="corridor")
        exp[(("Space", "min_width"), "ibc-corr-out")] = lim[("Space", "min_width")] - EPS
    walls, _ = d.room("ibc-office", 40, 0, 6.0, 0.9 + T, use="office")  # narrow, but not a corridor
    d.opening("ibc-door-in", walls[0], "door", lim[("Opening", "min_width")] + EPS, offset=0.5)
    d.opening("ibc-window", walls[2], "window", 0.5, offset=1.0)        # narrow window: not a door
    if outside:
        d.opening("ibc-door-out", walls[0], "door", lim[("Opening", "min_width")] - EPS, offset=3.0)
        exp[(("Opening", "min_width"), "ibc-door-out")] = lim[("Opening", "min_width")] - EPS
    n = 17
    d.stair("ibc-stair-in", 0.0, n * (lim[("Stair", "riser_max")] - EPS), n, lim[("Stair", "tread_min")] + EPS)
    if outside:
        r = d.stair("ibc-stair-riser-out", 10.0, n * (lim[("Stair", "riser_max")] + EPS), n, lim[("Stair", "tread_min")] + EPS)
        exp[(("Stair", "riser_max"), "ibc-stair-riser-out")] = r
        d.stair("ibc-stair-tread-out", 20.0, n * (lim[("Stair", "riser_max")] - EPS), n, lim[("Stair", "tread_min")] - EPS)
        exp[(("Stair", "tread_min"), "ibc-stair-tread-out")] = lim[("Stair", "tread_min")] - EPS
    return d, exp


def eurocode_doc(outside=True):
    d = Doc()
    exp = {}
    lo, hi = SPEC_PACKS["eurocode"][("Stair", "blondel")]
    n, rise = 16, 2.72
    r = rise / n
    cases = [("eu-blondel-lo-in", lo + EPS), ("eu-blondel-hi-in", hi - EPS)]
    if outside:
        cases += [("eu-blondel-lo-out", lo - EPS), ("eu-blondel-hi-out", hi + EPS)]
    for k, (name, target) in enumerate(cases):
        tread = target - 2 * r
        d.stair(name, 10.0 * k, rise, n, tread)
        if name.endswith("-out"):
            exp[(("Stair", "blondel"), name)] = (2 * r + tread, lo if name.startswith("eu-blondel-lo") else hi)
    ulim = SPEC_PACKS["eurocode"][("Wall", "u_value_max")]
    for name, u in (("eu-wall-in", ulim - EPS),) + ((("eu-wall-out", ulim + EPS),) if outside else ()):
        # concrete 0.2 m (lambda 2.0) + insulation sized to hit U exactly
        t_ins = (1.0 / u - 0.17 - 0.2 / 2.0) * 0.035
        layers = [(0.2, 2.0), (t_ins, 0.035)]
        d.add(f"{name}-type", "WallType", {"layer_thickness": [0.2, t_ins], "layer_material": ["Concrete", "Mineral wool"],
                                           "layer_conductivity": [2.0, 0.035]})
        y = 0.0 if name.endswith("in") else 10.0
        d.add(name, "Wall", {"level": {"ref": "L0"}, "x1": 100.0, "y1": y, "x2": 106.0, "y2": y, "height": 3.0,
                             "type": {"ref": f"{name}-type"}})
        if name.endswith("out"):
            exp[(("Wall", "u_value_max"), name)] = u_value(layers)
    return d, exp


def access_doc(outside=True):
    d = Doc()
    exp = {}
    lim = SPEC_PACKS["accessibility"]
    d.room("acc-corr-in", 0, 0, 9.0, corridor_width(lim[("Space", "min_width")] + EPS), angle=math.radians(-15), use="corridor")
    if outside:
        d.room("acc-corr-out", 20, 0, 9.0, corridor_width(lim[("Space", "min_width")] - EPS), use="corridor")
        exp[(("Space", "min_width"), "acc-corr-out")] = lim[("Space", "min_width")] - EPS
    walls, _ = d.room("acc-office", 40, 0, 6.0, 1.0 + T, use="office")
    d.opening("acc-door-in", walls[0], "door", lim[("Opening", "min_width")] + EPS, offset=0.5)
    d.opening("acc-window", walls[2], "window", 0.6, offset=1.0)
    if outside:
        d.opening("acc-door-out", walls[0], "door", lim[("Opening", "min_width")] - EPS, offset=3.0)
        exp[(("Opening", "min_width"), "acc-door-out")] = lim[("Opening", "min_width")] - EPS
    rise = 0.5
    d.ramp("acc-ramp-in", 0.0, rise, rise / (lim[("Ramp", "slope_max")] - EPS))
    if outside:
        s = d.ramp("acc-ramp-out", 10.0, rise, rise / (lim[("Ramp", "slope_max")] + EPS))
        exp[(("Ramp", "slope_max"), "acc-ramp-out")] = s
    return d, exp


# ------------------------------------------------------------------------------------------ custom pack fixture
def geodesic_max(poly, target):
    """Longest over polygon vertices of the shortest path inside poly to target (visibility graph)."""
    verts = list(poly.exterior.coords)[:-1]
    nodes = verts + [target]
    g = nx.Graph()
    inside = poly.buffer(1e-9)
    for i, a in enumerate(nodes):
        for j in range(i + 1, len(nodes)):
            b = nodes[j]
            if inside.covers(sg.LineString([a, b])):
                g.add_edge(i, j, weight=math.dist(a, b))
    dist = nx.single_source_dijkstra_path_length(g, len(nodes) - 1)
    return max(dist[i] for i in range(len(verts)))


def custom_doc():
    d = Doc()
    exp = {}
    up = d.level(3.0)
    # min_area: inner 3.0 x 4.0 = 12.0 and 3.0 x 3.9 = 11.7 (limit 11.8)
    d.room("c-area-big", 0, 0, 4.0 + T, 3.0 + T, use="store")
    d.room("c-area-small", 10, 0, 3.9 + T, 3.0 + T, use="store")
    exp[("min_area", "c-area-small")] = 3.0 * 3.9
    # min_clear_height: rooms used as labs under slabs on the 3.0 m level (thickness 0.25 / 0.35, limit 2.7)
    for name, x0, th in (("c-lab-ok", 20.0, 0.25), ("c-lab-low", 30.0, 0.35)):
        d.room(name, x0, 0, 5.0, 4.0, use="lab")
        d.add(f"{name}-slab", "Slab", {"level": {"ref": up}, "thickness": th,
                                       "boundary": [x0 - 1, -1.0, x0 + 6, -1.0, x0 + 6, 5.0, x0 - 1, 5.0]})
    exp[("min_clear_height", "c-lab-low")] = 3.0 - 0.35
    # riser_min 0.16: risers 0.15 (violates) and 0.17; their top levels sit above the 3.0 m slab level so
    # that level stays the one directly above the lab rooms
    exp[("riser_min", "c-stair-low")] = d.stair("c-stair-low", 0.0, 22 * 0.15, 22, 0.30)
    d.stair("c-stair-ok", 10.0, 20 * 0.17, 20, 0.28)
    # egress_distance_max: L-shaped room (6 walls, centreline below), door on the first wall
    cl = [(50.0, 0.0), (56.0, 0.0), (56.0, 3.0), (53.0, 3.0), (53.0, 6.0), (50.0, 6.0)]
    for k in range(6):
        (x1, y1), (x2, y2) = cl[k], cl[(k + 1) % 6]
        d.add(f"c-L-w{k}", "Wall", {"level": {"ref": "L0"}, "x1": x1, "y1": y1, "x2": x2, "y2": y2, "height": 2.8,
                                    "type": {"ref": "WT"}})
    d.add("c-L", "Space", {"level": {"ref": "L0"}, "x": 51.0, "y": 1.0, "name": "L", "use": "hall"})
    d.opening("c-L-door", "c-L-w1", "door", 1.0, offset=1.0)  # midpoint on the centreline at (56, 1.5)
    inner = sg.Polygon(cl).buffer(-T / 2, join_style="mitre")
    mid = sg.Point(56.0, 1.5)
    target = inner.exterior.interpolate(inner.exterior.project(mid))
    egress = geodesic_max(inner, (target.x, target.y))
    # closed-form cross-check of the oracle: the farthest vertex (50.1, 5.9) cannot see the door point
    # (55.9, 1.5) across the notch and bends around the reflex corner (52.9, 2.9)
    cf = math.dist((50.1, 5.9), (52.9, 2.9)) + math.dist((52.9, 2.9), (55.9, 1.5))
    need(abs(egress - cf) < 1e-9, f"oracle self-check: geodesic {egress} vs closed form {cf}")
    exp[("egress", "c-L")] = egress
    return d, exp


def custom_pack(egress_limit, door_limit):
    return {"pack": "judge-custom", "version": "1", "rules": [
        {"id": "J-area", "applies_to": "Space", "where": {"use": "store"}, "check": "min_area", "limit": 11.8},
        {"id": "J-clear", "applies_to": "Space", "where": {"use": "lab"}, "check": "min_clear_height", "limit": 2.7},
        {"id": "J-riser-min", "applies_to": "Stair", "check": "riser_min", "limit": 0.16},
        {"id": "J-egress", "applies_to": "Space", "where": {"use": "hall"}, "check": "egress_distance_max", "limit": egress_limit},
        {"id": "J-door", "applies_to": "Opening", "where": {"kind": "door"}, "check": "min_width", "limit": door_limit},
    ]}


# ------------------------------------------------------------------------------------------ running bim
class Bim:
    def __init__(self, exe, tmp):
        self.exe, self.tmp = exe, tmp

    def run(self, *args, timeout=60):
        try:
            return subprocess.run([self.exe, *args], capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            raise Fail(f"bim {' '.join(args)} timed out")

    def project(self, name, doc):
        d = os.path.join(self.tmp, name)
        r = self.run("init", d)
        need(r.returncode == 0, f"bim init {name}: exit {r.returncode} {r.stderr[:300]}")
        ops = d + ".ops.json"
        doc.write(ops)
        r = self.run("apply", d, ops)
        need(r.returncode == 0, f"bim apply {name}: exit {r.returncode} {r.stderr[:300]}")
        return d

    def validate(self, d, standard, want_exit):
        r = self.run("validate", d, "--standard", standard, "--json")
        need(r.returncode == want_exit,
             f"validate --standard {standard} on {os.path.basename(d)}: exit {r.returncode}, want {want_exit}; "
             f"stdout {r.stdout[:600]} stderr {r.stderr[:300]}")
        try:
            out = json.loads(r.stdout)
        except ValueError:
            raise Fail(f"validate --standard {standard} --json did not print one JSON document: {r.stdout[:300]}")
        need(isinstance(out.get("violations"), list), f"validate JSON lacks a violations list: {out}")
        need(out.get("dangling") == [] and out.get("conflicts") == [],
             f"fixture {os.path.basename(d)} should have no dangling refs/conflicts: {out.get('dangling')} {out.get('conflicts')}")
        for v in out["violations"]:
            for k in ("rule", "entity", "message", "measured", "limit"):
                need(k in v, f"violation lacks {k}: {v}")
        return out["violations"]


def compare(violations, expected, what):
    """expected: {(rule id, entity): (measured, limit)}"""
    got = {}
    for v in violations:
        key = (v["rule"], v["entity"])
        need(key not in got, f"{what}: violation {key} reported twice")
        got[key] = v
    need(set(got) == set(expected),
         f"{what}: violations {sorted(got)}, want {sorted(expected)}")
    for key, (measured, limit) in expected.items():
        v = got[key]
        need(isinstance(v["measured"], (int, float)) and abs(v["measured"] - measured) <= TOL,
             f"{what}: {key} measured {v['measured']!r}, closed form {measured!r}")
        need(isinstance(v["limit"], (int, float)) and abs(v["limit"] - limit) <= 1e-12,
             f"{what}: {key} limit {v['limit']!r}, want {limit!r}")
        need(isinstance(v["message"], str) and v["message"].strip(), f"{what}: {key} has an empty message")


def main():
    exe, root, tmp = sys.argv[1], sys.argv[2], sys.argv[3]
    bim = Bim(exe, tmp)
    rules = shipped_rules(root)

    for pack, build in (("ibc", ibc_doc), ("eurocode", eurocode_doc), ("accessibility", access_doc)):
        doc, exp = build(True)
        want = {}
        for (key, entity), m in exp.items():
            rid, lim = rules[pack][key]
            if isinstance(m, tuple):  # blondel: (measured, violated bound)
                m, lim = m
            want[(rid, entity)] = (m, lim)
        d = bim.project(pack, doc)
        compare(bim.validate(d, pack, 1), want, f"--standard {pack}")
        clean, _ = build(False)
        d = bim.project(pack + "-clean", clean)
        compare(bim.validate(d, pack, 0), {}, f"--standard {pack} (all just inside)")

    # Judge-authored pack at a path: the rest of the vocabulary, `where`, hot swap.
    doc, exp = custom_doc()
    d = bim.project("custom", doc)
    pack_path = os.path.join(tmp, "judge-pack.json")
    json.dump(custom_pack(egress_limit=round(exp[("egress", "c-L")] - 0.01, 6), door_limit=1.05), open(pack_path, "w"))
    want = {("J-area", "c-area-small"): (exp[("min_area", "c-area-small")], 11.8),
            ("J-clear", "c-lab-low"): (exp[("min_clear_height", "c-lab-low")], 2.7),
            ("J-riser-min", "c-stair-low"): (exp[("riser_min", "c-stair-low")], 0.16),
            ("J-egress", "c-L"): (exp[("egress", "c-L")], round(exp[("egress", "c-L")] - 0.01, 6)),
            ("J-door", "c-L-door"): (1.0, 1.05)}
    st0 = os.stat(exe)
    compare(bim.validate(d, pack_path, 1), want, "judge pack (run 1)")
    # Edit the pack file between runs: no rebuild, different result.
    json.dump(custom_pack(egress_limit=round(exp[("egress", "c-L")] + 0.01, 6), door_limit=0.95), open(pack_path, "w"))
    del want[("J-egress", "c-L")], want[("J-door", "c-L-door")]
    compare(bim.validate(d, pack_path, 1), want, "judge pack after editing it (run 2)")
    st1 = os.stat(exe)
    need((st0.st_mtime_ns, st0.st_size) == (st1.st_mtime_ns, st1.st_size), "the bim binary changed between runs")

    # Unknown check names, unknown packs and malformed packs are invalid input: exit 2.
    bad = custom_pack(1.0, 1.0)
    bad["rules"].append({"id": "J-bad", "applies_to": "Space", "check": "max_banana", "limit": 1.0})
    json.dump(bad, open(pack_path, "w"))
    r = bim.run("validate", d, "--standard", pack_path, "--json")
    need(r.returncode == 2, f"pack with unknown check 'max_banana' exited {r.returncode}, want 2")
    r = bim.run("validate", d, "--standard", "no-such-pack", "--json")
    need(r.returncode == 2, f"--standard no-such-pack exited {r.returncode}, want 2")
    open(pack_path, "w").write('{"pack": "broken", "rules": [')
    r = bim.run("validate", d, "--standard", pack_path, "--json")
    need(r.returncode == 2, f"malformed pack JSON exited {r.returncode}, want 2")
    print("ok")


if __name__ == "__main__":
    try:
        main()
    except Fail as e:
        print(str(e))
        sys.exit(1)
