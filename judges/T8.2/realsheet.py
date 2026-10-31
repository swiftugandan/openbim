# Helper for the T8.2 judge: make a real T7.4 sheet SVG with `bim sheet` and report where its
# visible HATCH geometry lies (sheet mm, SVG orientation: origin top-left, y down).
# usage: python -I realsheet.py <bim> <out.svg> <out-hatch.json>
import json
import os
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True  # keep judges/ free of __pycache__
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "T7.4"))
from sheetgeom import parse_svg  # noqa: E402

bim, out_svg, out_json = sys.argv[1:4]


def fail(m):
    print(f"JUDGE FAIL: {m}", file=sys.stderr)
    sys.exit(1)


def must(*args):
    try:
        p = subprocess.run([bim, *args], capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        fail(f"'bim {' '.join(args)}' timed out")
    if p.returncode != 0:
        fail(f"'bim {' '.join(args)}' exited {p.returncode}: {p.stderr.strip()[:300]}")


with tempfile.TemporaryDirectory() as tmp:
    d = os.path.join(tmp, "p")
    ops = [
        {"op": "create", "id": "L0", "kind": "Level", "props": {"elevation": 0.0}},
        {"op": "create", "id": "WT1", "kind": "WallType", "props": {
            "layer_thickness": [0.2, 0.1], "layer_material": ["concrete", "insulation"],
            "layer_conductivity": [1.7, 0.04]}},
        {"op": "create", "id": "W1", "kind": "Wall", "props": {
            "level": {"ref": "L0"}, "x1": 0.0, "y1": 0.0, "x2": 2.0, "y2": 0.0, "height": 3.0,
            "type": {"ref": "WT1"}}},
    ]
    with open(os.path.join(tmp, "ops.json"), "w") as fh:
        json.dump({"schema": 1, "ops": ops}, fh)
    sheet = {"size": [100, 60], "title": {"project": "T82"},
             "viewports": [{"view": "plan:L0@1.2", "scale": 20, "x": 5, "y": 5, "w": 90, "h": 50}]}
    with open(os.path.join(tmp, "sheet.json"), "w") as fh:
        json.dump(sheet, fh)
    must("init", d)
    must("apply", d, os.path.join(tmp, "ops.json"))
    must("sheet", d, os.path.join(tmp, "sheet.json"), "--format", "svg", "--out", out_svg)
(W, H), prims = parse_svg(out_svg)
hs = [p["bbox"] for p in prims if p["layer"] == "HATCH" and p["bbox"] is not None]
if not hs:
    fail("the bim sheet SVG has no visible HATCH geometry for a cut layered wall")
u = (min(b[0] for b in hs), min(b[1] for b in hs), max(b[2] for b in hs), max(b[3] for b in hs))
with open(out_json, "w") as fh:
    json.dump([[u[0], H - u[3], u[2], H - u[1]]], fh)
