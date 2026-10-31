"""T2.9 oracle side: check the DXF (ezdxf) and PDF (pypdf) exports of one view against the
drawing contract of spec/cli.md and against the SVG export of the same view (T2.8).

Usage: python -I check_cad.py <view.svg> <view.dxf> <view.pdf> <view> <ops.json> [<ops.json> ...]
"""
import json
import math
import os
import re
import sys

import ezdxf
import numpy as np
import pypdf
from pypdf.generic import ContentStream
from shapely.geometry import LineString, Point, Polygon
from shapely.ops import unary_union

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import svgdraw  # noqa: E402

LAYERS = svgdraw.LAYERS
LINE_LAYERS = ("CUT", "PROJECTION", "HIDDEN")
TOL = 0.02  # paper mm: coordinate agreement between formats written with finite decimals
PT = 72 / 25.4


def fail(msg):
    print(f"JUDGE FAIL: {msg}")
    sys.exit(1)


def load_ents(paths):
    ents = {}
    for p in paths:
        for op in json.load(open(p))["ops"]:
            if op["op"] == "create":
                ents[op["id"]] = {"kind": op["kind"], "props": dict(op.get("props", {}))}
            elif op["op"] == "set":
                ents[op["id"]]["props"][op["key"]] = op["value"]
            elif op["op"] == "delete":
                del ents[op["id"]]
    return ents


def expected_dimensions(ents, view):
    out = {}
    for i, e in ents.items():
        if e["kind"] == "Dimension" and e["props"].get("view") == view:
            pa = ents[e["props"]["a"]["ref"]]["props"]
            pb = ents[e["props"]["b"]["ref"]]["props"]
            d = math.dist((pa["x"], pa["y"], pa.get("z", 0.0)), (pb["x"], pb["y"], pb.get("z", 0.0)))
            out[i] = str(int(math.floor(d * 1000.0 + 0.5)))
    return out


# ------------------------------------------------------------------ segment-set comparison

def svg_segments(items):
    """{(layer, entity): [((x,y),(x,y))]} in paper mm with y up (SVG y is down)."""
    out = {}
    for it in items:
        if it.layer in LINE_LAYERS and it.tag != "text":
            for a, b in it.segments():
                out.setdefault((it.layer, it.entity), []).append(((a[0], -a[1]), (b[0], -b[1])))
    return out


def bbox_min(groups):
    pts = [p for segs in groups.values() for s in segs for p in s]
    if not pts:
        return np.zeros(2)
    return np.array(pts).min(0)


def compare(name, want, got, tag):
    """Both {(layer, entity): segments}; equal up to a translation (paper mm)."""
    if sorted(want) != sorted(got):
        miss = sorted(set(want) - set(got))[:5]
        extra = sorted(set(got) - set(want))[:5]
        fail(f"{tag} {name} (layer, entity) groups differ from the SVG: missing {miss}, extra {extra}")
    shift = bbox_min(want) - bbox_min(got)
    for key in want:
        lw = unary_union([LineString(s) for s in want[key]])
        lg = unary_union([LineString([tuple(np.add(p, shift)) for p in s]) for s in got[key]])
        for src, dst, what in ((want[key], lg, "SVG stroke missing from"), (got[key], lw, "stroke not in the SVG on")):
            for s in src:
                a, b = np.array(s[0], float), np.array(s[1], float)
                if dst is lw:
                    a, b = a + shift, b + shift
                for t in np.linspace(0.0, 1.0, 9):
                    q = a + (b - a) * t
                    if dst.distance(Point(q)) > TOL:
                        fail(f"{tag} {name}: {what} {key} near {np.round(q, 3)}")
    return shift


# ------------------------------------------------------------------ DXF

LW = {k: int(round(v * 100)) for k, v in LAYERS.items()}


def check_dxf(path, items, ents, dims, tag):
    try:
        doc = ezdxf.readfile(path)
    except Exception as e:  # noqa: BLE001
        fail(f"{tag} ezdxf cannot read the DXF: {e}")
    if doc.dxfversion != "AC1027":
        fail(f"{tag} DXF version {doc.dxfversion}, want AC1027 (AutoCAD 2013)")
    auditor = doc.audit()
    if auditor.errors:
        fail(f"{tag} ezdxf audit errors: {[str(e.message) for e in auditor.errors[:5]]}")
    used = {it.layer for it in items}
    for L in used:
        if L not in doc.layers:
            fail(f"{tag} DXF has no layer {L}")
    for L in LAYERS:
        if L in doc.layers:
            lay = doc.layers.get(L)
            if lay.dxf.lineweight != LW[L]:
                fail(f"{tag} DXF layer {L} lineweight {lay.dxf.lineweight}, want {LW[L]} (1/100 mm)")
    hidden_layer_lt = doc.layers.get("HIDDEN").dxf.linetype.upper() if "HIDDEN" in doc.layers else "CONTINUOUS"
    if "HIDDEN" in used and "DASHED" not in doc.linetypes:
        fail(f"{tag} DXF has no DASHED linetype")
    msp = doc.modelspace()
    groups, hatches, texts = {}, {}, []
    for e in msp:
        L = e.dxf.layer
        if L not in LAYERS:
            fail(f"{tag} DXF {e.dxftype()} on undeclared layer {L!r}")
        lw = e.dxf.get("lineweight", -1)
        if lw not in (-1, LW[L]):
            fail(f"{tag} DXF {e.dxftype()} on {L} overrides lineweight to {lw}")
        if L == "HIDDEN":
            # TASKS.md T2.9 Details: HIDDEN uses BYLAYER or DASHED; BYLAYER must resolve to DASHED.
            lt = e.dxf.get("linetype", "BYLAYER").upper()
            effective = hidden_layer_lt if lt == "BYLAYER" else lt
            if effective != "DASHED":
                fail(f"{tag} DXF HIDDEN {e.dxftype()} has effective linetype {effective} "
                     f"(entity {lt}, layer {hidden_layer_lt}), want DASHED")
        eid = None
        if e.has_xdata("OPENBIM"):
            vals = [v for c, v in e.get_xdata("OPENBIM") if c == 1000]
            eid = vals[0] if vals else None
        if L != "ANNOTATION":
            if eid is None:
                fail(f"{tag} DXF {e.dxftype()} on {L} has no XDATA OPENBIM entity id")
            if eid not in ents:
                fail(f"{tag} DXF XDATA id {eid!r} is not an entity of the document")
        t = e.dxftype()
        if L in LINE_LAYERS:
            segs = groups.setdefault((L, eid), [])
            if t == "LINE":
                segs.append((tuple(e.dxf.start)[:2], tuple(e.dxf.end)[:2]))
            elif t in ("LWPOLYLINE", "POLYLINE"):
                pts = [tuple(p)[:2] for p in (e.get_points("xy") if t == "LWPOLYLINE" else e.points())]
                if e.closed:
                    pts.append(pts[0])
                segs += [(a, b) for a, b in zip(pts, pts[1:]) if a != b]
            else:
                fail(f"{tag} DXF {t} on {L}: expected LINE/LWPOLYLINE/POLYLINE")
        elif L == "HATCH" and t == "HATCH":
            g = Polygon()
            for path in e.paths:
                if hasattr(path, "vertices"):
                    pts = [tuple(v)[:2] for v in path.vertices]
                else:
                    pts = [tuple(ed.start)[:2] for ed in path.edges]
                if len(pts) >= 3:
                    g = g.symmetric_difference(Polygon(pts).buffer(0))
            hatches[eid] = hatches.get(eid, Polygon()).union(g)
        elif L == "DIMENSION":
            if t == "DIMENSION":
                texts.append((eid, e.dxf.get("text", "")))
            elif t == "MTEXT":
                texts.append((eid, e.plain_text()))
            elif t == "TEXT":
                texts.append((eid, e.dxf.text))
    want = svg_segments(items)
    shift = compare("DXF", want, groups, tag)
    # HATCH entities fill exactly the SVG cut regions.
    cut = {}
    for it in items:
        if it.layer == "CUT":
            for pts, closed in it.rings:
                if closed and len(pts) >= 3:
                    cut[it.entity] = cut.get(it.entity, Polygon()).symmetric_difference(
                        Polygon([(x, -y) for x, y in pts]).buffer(0))
    if sorted(cut) != sorted(k for k in hatches if k is not None):
        fail(f"{tag} DXF HATCH entities for {sorted(map(str, hatches))}, want one per cut entity {sorted(cut)}")
    for eid, g in cut.items():
        h = Polygon()
        from shapely.affinity import translate
        h = translate(hatches[eid], shift[0], shift[1])
        if g.symmetric_difference(h).area > 0.01 * g.area + TOL * g.length:
            fail(f"{tag} DXF HATCH of {eid} does not fill its cut region")
    for did, txt in dims.items():
        got = [s.replace(" ", "") for i, s in texts if i == did]
        if txt not in got:
            fail(f"{tag} DXF dimension {did} text {got}, want {txt}")


# ------------------------------------------------------------------ PDF

def mat(a, b, c, d, e, f):
    return np.array([[a, c, e], [b, d, f], [0, 0, 1]], float)


def check_pdf(path, items, ents, dims, tag):
    with open(path, "rb") as fh:
        head = fh.read(16)
    if not head.startswith(b"%PDF-1.7"):
        fail(f"{tag} PDF header {head[:8]!r}, want %PDF-1.7")
    try:
        r = pypdf.PdfReader(path)
        npages = len(r.pages)
        page = r.pages[0]
    except Exception as e:  # noqa: BLE001
        fail(f"{tag} pypdf cannot read the PDF: {e}")
    if npages != 1:
        fail(f"{tag} PDF has {npages} pages, want one page")
    root = r.trailer["/Root"]
    if "/OCProperties" not in root:
        fail(f"{tag} PDF has no /OCProperties (optional content groups)")
    ocgs = [o.get_object() for o in root["/OCProperties"]["/OCGs"]]
    names = [str(o["/Name"]) for o in ocgs]
    used = {it.layer for it in items}
    if len(set(names)) != len(names) or not used <= set(names) or not set(names) <= set(LAYERS):
        fail(f"{tag} PDF OCG names {names}: want one group per layer, covering {sorted(used)}")
    props, ext_lw = {}, {}
    res = page["/Resources"]
    if "/Properties" in res:
        for k, v in res["/Properties"].get_object().items():
            o = v.get_object()
            if "/Name" in o:
                props[str(k)] = str(o["/Name"])
    if "/ExtGState" in res:  # a line width may also be set through `gs` (ExtGState /LW)
        for k, v in res["/ExtGState"].get_object().items():
            o = v.get_object()
            if "/LW" in o:
                ext_lw[str(k)] = float(o["/LW"])
    text = page.extract_text() or ""
    for did, txt in dims.items():
        if txt not in text.replace(" ", ""):
            fail(f"{tag} PDF text does not contain dimension {did} = {txt}")
    ops = ContentStream(page.get_contents(), r).operations
    ctm, stack, lw, oc, path, cur, start = np.eye(3), [], 1.0, [], [], None, None
    groups, widths = {}, {}

    def tp(x, y):
        p = ctm @ np.array([float(x), float(y), 1.0])
        return (p[0] / PT, p[1] / PT)  # paper mm

    for operands, op in ops:
        op = op.decode() if isinstance(op, bytes) else op
        if op == "q":
            stack.append((ctm.copy(), lw))
        elif op == "Q":
            ctm, lw = stack.pop()
        elif op == "cm":
            ctm = ctm @ mat(*map(float, operands))
        elif op == "w":
            lw = float(operands[0])
        elif op == "gs":
            lw = ext_lw.get(str(operands[0]), lw)
        elif op == "BDC":
            name = None
            if str(operands[0]) == "/OC":
                key = str(operands[1])
                name = props.get(key)
                if name is None:
                    fail(f"{tag} PDF marked content /OC {key} is not an OCG in /Resources/Properties")
            oc.append(name)
        elif op == "BMC":
            oc.append(None)
        elif op == "EMC":
            oc.pop()
        elif op == "m":
            cur = start = tp(*operands)
            path.append([cur])
        elif op == "l":
            cur = tp(*operands)
            path[-1].append(cur)
        elif op == "h":
            if path and path[-1]:
                path[-1].append(path[-1][0])
        elif op == "re":
            x, y, w_, h_ = map(float, operands)
            path.append([tp(x, y), tp(x + w_, y), tp(x + w_, y + h_), tp(x, y + h_), tp(x, y)])
        elif op in ("S", "s", "B", "B*", "b", "b*", "f", "F", "f*", "n"):
            layer = next((n for n in reversed(oc) if n), None)
            if op in ("S", "s", "B", "B*", "b", "b*") and path:
                if layer is None:
                    fail(f"{tag} PDF stroked path outside any layer OCG")
                scale = math.sqrt(abs(np.linalg.det(ctm[:2, :2])))
                widths.setdefault(layer, set()).add(round(lw * scale, 4))
                if layer in LINE_LAYERS:
                    for sub in path:
                        if op in ("s", "b", "b*") and sub[0] != sub[-1]:
                            sub = sub + [sub[0]]
                        groups.setdefault(layer, []).extend((a, b) for a, b in zip(sub, sub[1:]) if a != b)
            path = []
    for L, ws in widths.items():
        want = LAYERS[L] * PT
        bad = [w for w in ws if abs(w - want) > 0.01]
        if bad:
            fail(f"{tag} PDF layer {L} line widths {sorted(ws)} pt, want {want:.3f} pt ({LAYERS[L]} mm)")
    # The PDF carries no entity ids, so compare per layer.
    want = {}
    for (L, _), segs in svg_segments(items).items():
        want.setdefault((L, None), []).extend(segs)
    compare("PDF", want, {(L, None): s for L, s in groups.items()}, tag)


def main():
    svg, dxf, pdf, view, ops = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5:]
    ents = load_ents(ops)
    tag = f"[{view}]"
    items, _, _ = svgdraw.load(svg)
    dims = expected_dimensions(ents, view)
    check_dxf(dxf, items, ents, dims, tag)
    check_pdf(pdf, items, ents, dims, tag)
    print(f"T2.9 check {view}: ok")


if __name__ == "__main__":
    main()
