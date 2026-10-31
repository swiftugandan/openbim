# Writes `bim solve --trade arch|struct|mep --json` outputs of three small models for the built-in metrics test
# (tests/metrics.rs). Builders must not edit judges/.
# Usage: python -I fixtures.py <bim-binary> <scratch-dir>   -> <scratch-dir>/{arch,struct,mep}.json
import json
import os
import subprocess
import sys

BIM, TMP = sys.argv[1], sys.argv[2]


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def bim(*args):
    try:
        p = subprocess.run([BIM, *map(str, args)], capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        fail(f"bim {' '.join(map(str, args))} did not finish within 60s")
    if p.returncode != 0:
        fail(f"bim {' '.join(map(str, args))}: exit {p.returncode}: {p.stderr.strip()[-300:]}")
    return p.stdout


def c(i, kind, /, **props):
    return {"op": "create", "id": i, "kind": kind, "props": props}


def ref(i):
    return {"ref": i}


def model(name, ops, trade, key, field):
    d = os.path.join(TMP, name)
    bim("init", d)
    f = os.path.join(TMP, name + ".ops.json")
    json.dump({"schema": 1, "ops": ops}, open(f, "w"))
    bim("apply", d, f)
    out = bim("solve", d, "--trade", trade, "--json")
    try:
        doc = json.loads(out)
    except json.JSONDecodeError as e:
        fail(f"solve --trade {trade} --json is not JSON: {e}")
    items = doc.get(key)
    if not isinstance(items, list) or len(items) < 2 or not all(isinstance(x, dict) and field in x for x in items):
        fail(f"solve --trade {trade}: expected at least two {key}[] entries with `{field}`, got {str(items)[:300]}")
    open(os.path.join(TMP, trade + ".json"), "w").write(out)
    return doc


# arch: two closed rectangular rooms
ops = [c("L1", "Level", elevation=0.0), c("L2", "Level", elevation=3.0),
       c("WT", "WallType", layer_thickness=[0.2], layer_material=["concrete"], layer_conductivity=[1.6])]
for tag, (x0, y0, x1, y1) in {"A": (0, 0, 4, 3), "B": (6, 0, 10, 5)}.items():
    pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    for k in range(4):
        (ax, ay), (bx, by) = pts[k], pts[(k + 1) % 4]
        ops.append(c(f"{tag}{k + 1}", "Wall", level=ref("L1"), type=ref("WT"), x1=ax, y1=ay, x2=bx, y2=by, height=3.0))
model("arch", ops, "arch", "rooms", "area")

# struct: a loaded portal frame (column bases at the lowest level are fixed)
ops = [c("L0", "Level", elevation=0.0), c("L1", "Level", elevation=3.0),
       c("S355", "Material", name="S355", E=210e9, G=81e9, density=7850.0, fy=355e6),
       c("C1", "Column", x=0.0, y=0.0, base=ref("L0"), top=ref("L1"), section_b=0.2, section_h=0.2, material=ref("S355")),
       c("C2", "Column", x=5.0, y=0.0, base=ref("L0"), top=ref("L1"), section_b=0.2, section_h=0.2, material=ref("S355")),
       c("B1", "Beam", level=ref("L1"), x1=0.0, y1=0.0, x2=5.0, y2=0.0, section_b=0.2, section_h=0.4, material=ref("S355")),
       c("P1", "PointLoad", x=2.5, y=0.0, z=2.8, fx=5e3, fy=0.0, fz=-40e3, mx=0.0, my=0.0, mz=0.0, case="D"),
       c("ULS", "LoadCombination", name="ULS", cases=["D"], factors=[1.35])]
model("struct", ops, "struct", "members", "utilization")

# mep: source -> junction <- sink; S2 is drawn against the flow, so its dp is negative and the largest in magnitude
ops = [c("W", "MepSystem", name="water", medium="water", density=998.2, viscosity=1.002e-3),
       c("a", "Port", system=ref("W"), x=0.0, y=0.0, z=0.0, kind="source", pressure=3e5),
       c("b", "Port", system=ref("W"), x=10.0, y=0.0, z=0.0, kind="junction"),
       c("s", "Port", system=ref("W"), x=40.0, y=0.0, z=0.0, kind="sink", flow=0.002),
       c("S1", "Segment", **{"from": ref("a"), "to": ref("b")}, shape="round", diameter=0.05, roughness=4.5e-5),
       c("S2", "Segment", **{"from": ref("s"), "to": ref("b")}, shape="round", diameter=0.05, roughness=4.5e-5)]
doc = model("mep", ops, "mep", "segments", "dp")
dps = {s.get("id"): s.get("dp") for s in doc["segments"]}
if not (isinstance(dps.get("S2"), (int, float)) and dps["S2"] < 0 and abs(dps["S2"]) > abs(dps.get("S1", 0))):
    fail(f"solve --trade mep: segment S2 runs against the flow, so its dp must be negative and the largest in magnitude; got {dps}")
