# External judge for T7.4: `bim sheet` composition in SVG, PDF and DXF.
# Usage: python -I check.py <path-to-bim>
# Oracles: ISO 216 sizes by closed form (A0 = 1189 x 841 mm, halve the long side, floor) cross-checked
# with ezdxf's page table; ISO 128-20 dashed line (type 02) from ezdxf's ACAD_ISO02W100 definition
# (12 d dash, 3 d gap); geometry measured with ezdxf (DXF), pypdf content streams (PDF) and an
# SVG flattener (sheetgeom.py); expected placements from the model by closed form.
import atexit
import json
import os
import shutil
import subprocess
import sys
import tempfile

import ezdxf
from ezdxf import bbox as dxfbbox
from ezdxf import path as dxfpath
from ezdxf.tools._iso_pattern import ISO_PATTERN
from ezdxf.tools.standards import PAGE_SIZES
from shapely.geometry import Polygon

sys.dont_write_bytecode = True  # keep judges/ free of __pycache__
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sheetgeom import LAYERS, SheetError, parse_pdf, parse_svg  # noqa: E402

BIM = sys.argv[1]
TMP = tempfile.mkdtemp(prefix="judge-t74-")
atexit.register(shutil.rmtree, TMP, True)
PEN = {"CUT": 0.50, "PROJECTION": 0.25, "HIDDEN": 0.18, "HATCH": 0.13, "DIMENSION": 0.18, "ANNOTATION": 0.18}
DRAWING = ("CUT", "PROJECTION", "HIDDEN", "HATCH", "DIMENSION")


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def must(args, timeout=60):
    try:
        p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        fail(f"'bim {' '.join(args)}' timed out")
    if p.returncode != 0:
        fail(f"'bim {' '.join(args)}' exited {p.returncode}: {p.stderr.strip()[:400]}")
    return p.stdout


# ------------------------------------------------------------------------------- oracles
def iso_a(n):
    long_, short = 1189, 841  # A0 per ISO 216
    for _ in range(n):
        long_, short = short, long_ // 2
    return long_, short


for n, name in [(1, "ISO A1"), (3, "ISO A3")]:
    _, w, h = PAGE_SIZES[name]
    if iso_a(n) != (w, h):
        fail(f"oracle disagreement on {name}: closed form {iso_a(n)} vs ezdxf {(w, h)}")
iso02 = ISO_PATTERN["ACAD_ISO02W100"][0][3]  # [12.0, -3.0] for a 1 mm pen
DASH = [abs(v) * PEN["HIDDEN"] for v in iso02]  # paper mm for the HIDDEN pen
if abs(DASH[0] - 12 * 0.18) > 1e-12 or abs(DASH[1] - 3 * 0.18) > 1e-12:
    fail("oracle disagreement on the ISO 128-20 dashed line")

# ------------------------------------------------------------------------------- fixture
ops = [
    {"op": "create", "id": "L0", "kind": "Level", "props": {"elevation": 0.0}},
    {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 3.0}},
    {"op": "create", "id": "WT1", "kind": "WallType", "props": {
        "layer_thickness": [0.2, 0.1], "layer_material": ["concrete", "insulation"],
        "layer_conductivity": [1.7, 0.04]}},
    {"op": "create", "id": "WT2", "kind": "WallType", "props": {
        "layer_thickness": [0.3], "layer_material": ["brick"], "layer_conductivity": [0.8]}},
    # W1: 10 m long, 0.3 m thick, 3 m high. W2 sits 6 m behind it, shorter and narrower, so in a
    # north/south elevation one of them hides part of the other (HIDDEN lines).
    {"op": "create", "id": "W1", "kind": "Wall", "props": {
        "level": {"ref": "L0"}, "x1": 0.0, "y1": 0.0, "x2": 10.0, "y2": 0.0, "height": 3.0, "type": {"ref": "WT1"}}},
    {"op": "create", "id": "W2", "kind": "Wall", "props": {
        "level": {"ref": "L0"}, "x1": 2.0, "y1": 6.0, "x2": 8.0, "y2": 6.0, "height": 2.0, "type": {"ref": "WT2"}}},
]
proj = os.path.join(TMP, "p")
must(["init", proj])
with open(os.path.join(TMP, "ops.json"), "w") as fh:
    json.dump({"schema": 1, "ops": ops}, fh)
must(["apply", proj, os.path.join(TMP, "ops.json")])

TITLE = {"project": "Judge Hall T74", "drawing": "Plan and south elevation", "number": "A-174"}
PLAN = "plan:L0@1.2"
# Frames in sheet mm (origin lower-left, y up); all fit inside 297 x 297 so either orientation works.
F1 = (15.0, 150.0, 180.0, 120.0)   # plan at 1:100 -> content 100 x 63 mm, fits
F2 = (15.0, 15.0, 150.0, 100.0)    # south elevation at 1:50 -> content 200 x 60 mm, must be clipped
SHEETS = {
    "a3": {"size": "A3", "viewports": [
        {"view": PLAN, "scale": 100, "x": F1[0], "y": F1[1], "w": F1[2], "h": F1[3]},
        {"view": "elevation:s", "scale": 50, "x": F2[0], "y": F2[1], "w": F2[2], "h": F2[3]}],
        "title": TITLE},
    "a1": {"size": "A1", "viewports": [
        {"view": PLAN, "scale": 100, "x": 50.0, "y": 50.0, "w": 200.0, "h": 150.0}], "title": TITLE},
    "custom": {"size": [500, 350], "viewports": [
        {"view": PLAN, "scale": 100, "x": 50.0, "y": 50.0, "w": 200.0, "h": 150.0}], "title": TITLE},
}
EXPECT_SIZE = {"a3": [iso_a(3)], "a1": [iso_a(1)], "custom": [(500.0, 350.0)]}


def frames(sheet):
    return [(v["x"], v["y"], v["x"] + v["w"], v["y"] + v["h"]) for v in SHEETS[sheet]["viewports"]]


def within(b, f, tol=0.05):
    return b[0] >= f[0] - tol and b[1] >= f[1] - tol and b[2] <= f[2] + tol and b[3] <= f[3] + tol


def union(prims, key="bbox"):
    bs = [p[key] for p in prims if p[key] is not None]
    if not bs:
        return None
    return (min(b[0] for b in bs), min(b[1] for b in bs), max(b[2] for b in bs), max(b[3] for b in bs))


def near(a, b, tol):
    return abs(a - b) <= tol


# ------------------------------------------------------------------------------- DXF reader
def parse_dxf(path):
    try:
        doc = ezdxf.readfile(path)
    except Exception as e:  # noqa: BLE001
        fail(f"ezdxf cannot read {os.path.basename(path)}: {e}")
    aud = doc.audit()
    if aud.has_errors:
        fail(f"ezdxf audit reports {len(aud.errors)} errors in {os.path.basename(path)}")
    if doc.header.get("$INSUNITS") != 4:
        fail(f"DXF $INSUNITS is {doc.header.get('$INSUNITS')}, want 4 (millimetres)")
    lo, hi = doc.header.get("$LIMMIN", (0, 0)), doc.header.get("$LIMMAX", (0, 0))
    size = (hi[0] - lo[0], hi[1] - lo[1])
    if abs(lo[0]) > 1e-6 or abs(lo[1]) > 1e-6:
        fail("DXF $LIMMIN must be (0, 0) (sheet lower-left corner)")
    ltscale = float(doc.header.get("$LTSCALE", 1.0))
    prims, texts = [], []
    for e in doc.modelspace():
        layer = e.dxf.get("layer", "0")
        ent = None
        if e.has_xdata("OPENBIM"):
            for code, val in e.get_xdata("OPENBIM"):
                if code == 1000:
                    ent = val
                    break
        t = e.dxftype()
        if t in ("TEXT", "MTEXT", "ATTRIB"):
            texts.append(e.plain_text() if t == "MTEXT" else e.dxf.get("text", ""))
        if t == "INSERT":
            for a in e.attribs:
                texts.append(a.dxf.get("text", ""))
        try:
            ext = dxfbbox.extents([e])
        except Exception:  # noqa: BLE001
            ext = None
        b = None
        if ext is not None and ext.has_data:
            b = (ext.extmin.x, ext.extmin.y, ext.extmax.x, ext.extmax.y)
        lw = e.dxf.get("lineweight", -1)
        lay = doc.layers.get(layer) if doc.layers.has_entry(layer) else None
        if lw == -1 and lay is not None:
            lw = lay.dxf.get("lineweight", -3)
        ltname = e.dxf.get("linetype", "BYLAYER")
        if ltname.upper() == "BYLAYER" and lay is not None:
            ltname = lay.dxf.get("linetype", "Continuous")
        dash = None
        if doc.linetypes.has_entry(ltname):
            pat = doc.linetypes.get(ltname).simplified_line_pattern()
            if pat:
                s = ltscale * float(e.dxf.get("ltscale", 1.0))
                dash = [abs(v) * s for v in pat]
        area = None
        if t == "HATCH":
            polys = [Polygon([(v.x, v.y) for v in p.flattening(0.01)]) for p in dxfpath.from_hatch(e)]
            area = sum(abs(p.area) for p in polys if p.is_valid)
        prims.append({"layer": layer if layer in LAYERS else None, "entity": ent, "bbox": b, "raw": b,
                      "width": lw / 100.0 if lw >= 0 else None, "lw_code": lw, "dash": dash,
                      "kind": "text" if t in ("TEXT", "MTEXT") else "shape", "dxftype": t,
                      "pattern": e.dxf.get("pattern_name", "") if t == "HATCH" else None,
                      "solid": e.dxf.get("solid_fill", 0) if t == "HATCH" else None, "area": area})
    return size, prims, "\n".join(texts)


# ------------------------------------------------------------------------------- checks
def check_size(sheet, fmt, size):
    for w, h in EXPECT_SIZE[sheet]:
        named = isinstance(SHEETS[sheet]["size"], str)
        options = [(w, h), (h, w)] if named else [(w, h)]
        if any(near(size[0], a, 0.2) and near(size[1], b, 0.2) for a, b in options):
            return
    fail(f"{sheet}.{fmt}: sheet is {size[0]:.2f} x {size[1]:.2f} mm, want {EXPECT_SIZE[sheet][0]} (ISO 216)")


def check_common(sheet, fmt, prims, text, clipped_ok):
    tag = f"{sheet}.{fmt}"
    for v in TITLE.values():
        if v not in text:
            fail(f"{tag}: title block text '{v}' not found")
    fr = frames(sheet)
    draw = [p for p in prims if p["layer"] in DRAWING]
    if not draw:
        fail(f"{tag}: no geometry on the drawing layers {DRAWING}")
    for p in draw:
        b = p["bbox"] if clipped_ok else p["raw"]
        if b is None:
            continue
        if not any(within(b, f) for f in fr):
            fail(f"{tag}: {p['layer']} content at {tuple(round(c, 2) for c in b)} lies outside every "
                 f"viewport frame {fr} (origin lower-left, y up)")
    # Lineweights are paper widths per spec/cli.md, the same at 1:100 and 1:50.
    for p in draw:
        if p["width"] is not None and p["layer"] in PEN and not near(p["width"], PEN[p["layer"]], 0.011):
            fail(f"{tag}: {p['layer']} stroke is {p['width']:.4f} mm on paper, want {PEN[p['layer']]}")
    # Viewport 1 (plan at 1:100): cut walls span x 0..10 m and y -0.15..6.15 m -> 100 x 63 mm.
    v1 = [p for p in draw if p["layer"] == "CUT" and p["bbox"] and within(p["bbox"], fr[0])]
    u = union(v1, "bbox" if clipped_ok else "raw")
    if u is None:
        fail(f"{tag}: no CUT geometry inside viewport 1")
    if not (near(u[2] - u[0], 100.0, 0.3) and near(u[3] - u[1], 63.0, 0.3)):
        fail(f"{tag}: plan CUT extent is {u[2] - u[0]:.2f} x {u[3] - u[1]:.2f} mm, want 100 x 63 at 1:100")
    if not within(union(v1, "raw"), fr[0]):
        fail(f"{tag}: plan content at 1:100 fits its frame but was cut off")
    stroked = {L: [p for p in draw if p["layer"] == L and p["width"] is not None] for L in PEN}
    if not stroked["CUT"]:
        fail(f"{tag}: no stroked CUT lines")
    if len(fr) > 1:
        # Viewport 2 (south elevation at 1:50): walls 3 m high -> 60 mm; 10 m long -> 200 mm,
        # clipped to the 150 mm frame width.
        v2 = [p for p in draw if p["layer"] in ("CUT", "PROJECTION", "HIDDEN") and p["bbox"]
              and within(p["bbox"], fr[1])]
        u2 = union(v2)
        if u2 is None:
            fail(f"{tag}: no elevation geometry inside viewport 2")
        if not near(u2[3] - u2[1], 60.0, 0.3):
            fail(f"{tag}: elevation height is {u2[3] - u2[1]:.2f} mm, want 60 (3 m at 1:50)")
        if not near(u2[2] - u2[0], F2[2], 0.3):
            fail(f"{tag}: elevation spans {u2[2] - u2[0]:.2f} mm wide, want it clipped to the "
                 f"{F2[2]} mm frame (200 mm at 1:50)")
        if not stroked["HIDDEN"]:
            fail(f"{tag}: no stroked HIDDEN lines in the elevation (one wall hides part of the other)")
        if not stroked["PROJECTION"]:
            fail(f"{tag}: no stroked PROJECTION lines in the elevation")
        for p in stroked["HIDDEN"]:
            d = p["dash"]
            if not d or len(d) % 2 or any(not near(x, DASH[i % 2], 0.011) for i, x in enumerate(d)):
                fail(f"{tag}: HIDDEN dash pattern {d} mm, want {DASH} (ISO 128-20 type 02 for a 0.18 mm pen)")


def entity_checks(tag, prims, fr, clipped_ok):
    key = "bbox" if clipped_ok else "raw"
    plan = {}
    for e in ("W1", "W2"):
        ps = [p for p in prims if p["entity"] == e and p["layer"] in ("CUT", "HATCH") and p[key]
              and within(p[key], fr[0])]
        if not ps:
            fail(f"{tag}: no CUT/HATCH geometry tagged with entity {e} in the plan viewport")
        plan[e] = union(ps, key)
    w1, w2 = plan["W1"], plan["W2"]
    for e, b, ln in [("W1", w1, 100.0), ("W2", w2, 60.0)]:
        if not (near(b[2] - b[0], ln, 0.3) and near(b[3] - b[1], 3.0, 0.3)):
            fail(f"{tag}: {e} plan extent {b[2] - b[0]:.2f} x {b[3] - b[1]:.2f} mm, want {ln} x 3 at 1:100")
    if not near(abs((w2[1] + w2[3]) - (w1[1] + w1[3])) / 2, 60.0, 0.3):
        fail(f"{tag}: W1 and W2 centrelines are not 60 mm apart (6 m at 1:100)")
    if not near(abs(w2[0] - w1[0]), 20.0, 0.3):
        fail(f"{tag}: W2 does not start 20 mm along from W1 (2 m at 1:100)")


try:
    for sheet, spec in SHEETS.items():
        sj = os.path.join(TMP, f"{sheet}.json")
        with open(sj, "w") as fh:
            json.dump(spec, fh)
        for fmt in ("svg", "pdf", "dxf"):
            out = os.path.join(TMP, f"{sheet}.{fmt}")
            must(["sheet", proj, sj, "--format", fmt, "--out", out])
            if not os.path.isfile(out) or os.path.getsize(out) == 0:
                fail(f"{sheet}.{fmt} was not written")
            tag = f"{sheet}.{fmt}"
            if fmt == "svg":
                size, prims = parse_svg(out)
                text = "\n".join(p["text"] for p in prims if p["kind"] == "text")
                check_size(sheet, fmt, size)
                check_common(sheet, fmt, prims, text, clipped_ok=True)
                entity_checks(tag, prims, frames(sheet), clipped_ok=True)
            elif fmt == "pdf":
                size, prims, text, ocgs = parse_pdf(out)
                check_size(sheet, fmt, size)
                missing = [L for L in ("CUT", "PROJECTION", "HIDDEN", "HATCH") if L not in ocgs]
                if missing:
                    fail(f"{tag}: optional-content groups missing for layers {missing}")
                check_common(sheet, fmt, prims, text, clipped_ok=True)
            else:
                size, prims, text = parse_dxf(out)
                check_size(sheet, fmt, size)
                # DXF has no clip paths: viewport content must be clipped geometrically.
                check_common(sheet, fmt, prims, text, clipped_ok=False)
                entity_checks(tag, prims, frames(sheet), clipped_ok=False)
                if sheet == "a3":
                    # Hatch per material: W1 = concrete 0.2 m + insulation 0.1 m, W2 = brick 0.3 m.
                    # Cut areas on paper at 1:100: 10 m x 0.2 m -> 200 mm2, 10 x 0.1 -> 100 mm2, 6 x 0.3 -> 180 mm2.
                    hat = [p for p in prims if p["dxftype"] == "HATCH" and p["layer"] == "HATCH"
                           and p["bbox"] and within(p["bbox"], frames(sheet)[0])]
                    for e, want in [("W1", {"CONC": 200.0, "INSUL": 100.0}), ("W2", {"ANSI31": 180.0})]:
                        hs = [p for p in hat if p["entity"] == e]
                        if any(p["solid"] for p in hs):
                            fail(f"{tag}: {e} cut hatch is a solid fill, want a pattern")
                        for key, area in want.items():
                            got = sum(p["area"] or 0 for p in hs if key in (p["pattern"] or "").upper())
                            if not near(got, area, 0.02 * area):
                                pats = sorted({p["pattern"] for p in hs})
                                fail(f"{tag}: {e} hatch with a '{key}' pattern covers {got:.1f} mm2, want "
                                     f"{area} mm2 (patterns found: {pats})")
except SheetError as e:
    fail(str(e))

print("T7.4 judge: all checks passed")
