"""T2.8 oracle side: check one `bim export --format svg --view <view>` drawing against the
manifold3d reconstruction (model.py) and the drawing contract of spec/cli.md.

Usage: python -I check_svg.py <drawing.svg> <view> <ops.json> [<ops.json> ...]
"""
import math
import os
import sys

import numpy as np
from shapely.geometry import LineString, Point, Polygon
from shapely.ops import unary_union

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import model  # noqa: E402
import svgdraw  # noqa: E402
import views  # noqa: E402

SCALE = 10.0       # paper mm per model metre at the default 1:100
TOL = 0.003        # model metres (0.03 mm on paper): coordinate precision allowance
AREA_REL = 2e-3    # cut areas, after the coordinate rounding of a text format
GEOM_LAYERS = ("CUT", "PROJECTION", "HIDDEN", "HATCH", "DIMENSION")


def fail(msg):
    print(f"JUDGE FAIL: {msg}")
    sys.exit(1)


def expected_dimensions(ents, view):
    out = {}
    for i, e in ents.items():
        if e["kind"] == "Dimension" and e["props"].get("view") == view:
            pa = ents[model.ref(e["props"]["a"])]["props"]
            pb = ents[model.ref(e["props"]["b"])]["props"]
            d = math.dist((pa["x"], pa["y"], pa.get("z", 0.0)), (pb["x"], pb["y"], pb.get("z", 0.0)))
            out[i] = str(int(math.floor(d * 1000.0 + 0.5)))
    return out


def region(items):
    """Even-odd region of the closed rings of some items (paper coordinates)."""
    g = Polygon()
    for it in items:
        for pts, closed in it.rings:
            if closed and len(pts) >= 3:
                g = g.symmetric_difference(Polygon(pts).buffer(0))
    return g


def main():
    svg, view, ops = sys.argv[1], sys.argv[2], sys.argv[3:]
    ents = model.load_ops(*ops)
    rec = model.build(ents)
    solids = model.all_solids(rec)
    v = views.parse_view(view, ents)
    try:
        items, groups, _ = svgdraw.load(svg)
    except Exception as e:  # noqa: BLE001
        fail(f"{view}: SVG not readable as a straight-line drawing: {e}")
    tag = f"[{view}]"

    # ---- layers, stroke widths, dashes, entity references
    for it in items:
        if it.layer in GEOM_LAYERS and it.tag != "text":
            if it.entity is None:
                fail(f"{tag} a <{it.tag}> on layer {it.layer} has no data-entity")
            if it.entity not in ents:
                fail(f"{tag} data-entity {it.entity!r} is not an entity of the document")
        if it.tag != "text":
            want = svgdraw.LAYERS[it.layer]
            if it.width is None or abs(it.width - want) > 0.005:
                fail(f"{tag} <{it.tag}> on {it.layer} has stroke width {it.width} mm, want {want} mm")
            if it.layer == "HIDDEN" and not it.dashed:
                fail(f"{tag} HIDDEN <{it.tag}> is not dashed (stroke-dasharray)")
    by_layer = {L: [it for it in items if it.layer == L] for L in svgdraw.LAYERS}

    def to_model(pts):
        return [((x - ox) / SCALE, -(y - oy) / SCALE) for x, y in pts]

    # ---- cut geometry: per-entity regions vs manifold3d sections (shapely areas)
    want_cut = views.cut_regions(solids, v)
    cut_items = [it for it in by_layer["CUT"] if it.tag != "text"]
    if v["kind"] == "elevation":
        if cut_items:
            fail(f"{tag} an elevation has nothing on CUT, found {len(cut_items)} elements")
        lo, hi = views.projected_bbox(solids, v)
        pts = [p for L in ("PROJECTION", "HIDDEN") for it in by_layer[L] for r in it.rings for p in r[0]]
        if not pts:
            fail(f"{tag} elevation has no PROJECTION/HIDDEN geometry")
        P = np.array(pts)
        # Paper x right = u, paper (SVG) y down = -v.
        dlo = np.array([P[:, 0].min(), -P[:, 1].max()]) / SCALE
        dhi = np.array([P[:, 0].max(), -P[:, 1].min()]) / SCALE
        if np.abs((dhi - dlo) - (hi - lo)).max() > TOL:
            fail(f"{tag} drawn extents {(dhi - dlo).round(4)} m != model extents {(hi - lo).round(4)} m at 1:100")
        oy = P[:, 1].max() + lo[1] * SCALE  # svg y of v = lo[1]
        ox = P[:, 0].min() - lo[0] * SCALE
    else:
        if not want_cut:
            fail(f"{tag} judge fixture error: the view cuts nothing")
        drawn = {}
        for it in cut_items:
            drawn.setdefault(it.entity, []).append(it)
        if sorted(drawn) != sorted(want_cut):
            fail(f"{tag} CUT entities {sorted(drawn)} != cut solids {sorted(want_cut)}")
        dall = unary_union([region(x) for x in drawn.values()])
        wall_ = unary_union(list(want_cut.values()))
        if abs(dall.area / SCALE ** 2 - wall_.area) > AREA_REL * wall_.area:
            fail(f"{tag} total CUT area {dall.area / SCALE ** 2:.6f} m2 != {wall_.area:.6f} m2 (scale 1:100)")
        c, cw = dall.centroid, wall_.centroid
        ox, oy = c.x - cw.x * SCALE, c.y + cw.y * SCALE
        for eid, items_e in drawn.items():
            g = region(items_e)
            gm = Polygon() if g.is_empty else unary_union([
                Polygon(to_model(list(p.exterior.coords)), [to_model(list(h.coords)) for h in p.interiors])
                for p in getattr(g, "geoms", [g]) if p.geom_type == "Polygon"])
            w = want_cut[eid]
            if abs(gm.area - w.area) > AREA_REL * w.area:
                fail(f"{tag} CUT area of {eid}: {gm.area:.6f} m2, want {w.area:.6f} m2")
            x = gm.symmetric_difference(w).area
            if x > AREA_REL * w.area + TOL * w.length:
                fail(f"{tag} CUT outline of {eid} misplaced/misshapen: symmetric difference {x:.3g} m2")
        # Hatches: every cut entity hatched, every hatch inside its entity's cut region.
        hatch = {}
        for it in by_layer["HATCH"]:
            hatch.setdefault(it.entity, []).append(it)
        for eid in want_cut:
            if eid not in hatch:
                fail(f"{tag} cut entity {eid} has no HATCH")
        for eid, hs in hatch.items():
            if eid not in want_cut:
                fail(f"{tag} HATCH for {eid}, which is not cut")
            reg = want_cut[eid].buffer(TOL)
            for it in hs:
                for pts, _ in it.rings:
                    for p in to_model(pts):
                        if not reg.contains(Point(p)):
                            fail(f"{tag} HATCH point {np.round(p, 4)} of {eid} lies outside its cut region")

    # ---- beyond geometry: every PROJECTION/HIDDEN stroke lies on a model feature edge
    scene = views.Scene(solids, v)
    proj = [it for it in by_layer["PROJECTION"] if it.tag != "text"]
    if not proj:
        fail(f"{tag} nothing on PROJECTION")
    exact = v["kind"] == "elevation"
    for it in proj + [x for x in by_layer["HIDDEN"] if x.tag != "text"]:
        for a, b in it.segments():
            (a, b) = to_model([a, b])
            a, b = np.array(a), np.array(b)
            if np.linalg.norm(b - a) < TOL:
                continue
            for t in (0.29, 0.5, 0.71):
                q = a + (b - a) * t
                m = scene.edges_at(q, TOL, b - a)
                if not m:
                    fail(f"{tag} {it.layer} stroke of {it.entity} at {q.round(4)} is not on any model edge")
                if exact:
                    vis = [scene.visible(i, tt) for i, tt in m]
                    if it.layer == "PROJECTION" and not any(x is not False for x in vis):
                        fail(f"{tag} PROJECTION stroke at {q.round(4)} draws a hidden edge")
                    if it.layer == "HIDDEN" and not any(x is not True for x in vis):
                        fail(f"{tag} HIDDEN stroke at {q.round(4)} draws a visible edge")
    if exact:
        # Completeness: every unambiguous edge point is drawn on the layer of its visibility.
        segs = {L: [] for L in ("PROJECTION", "HIDDEN")}
        for L in segs:
            for it in by_layer[L]:
                for a, b in it.segments():
                    segs[L].append(LineString(to_model([a, b])))
        lines = {L: unary_union(s) if s else LineString() for L, s in segs.items()}
        if lines["HIDDEN"].is_empty:
            fail(f"{tag} elevation has no HIDDEN lines although the model has hidden edges")
        for i, (eid, a, b) in enumerate(scene.edges):
            e2 = scene.e2[i]
            if np.linalg.norm(e2[1] - e2[0]) < 10 * TOL:
                continue
            for t in (0.31, 0.67):
                q = e2[0] + (e2[1] - e2[0]) * t
                m = scene.edges_at(q, TOL)
                vis = {scene.visible(j, tt) for j, tt in m}
                if len(vis) != 1 or None in vis:
                    continue
                L = "PROJECTION" if vis.pop() else "HIDDEN"
                if lines[L].distance(Point(q)) > TOL:
                    fail(f"{tag} edge of {eid} at {q.round(4)} should be drawn on {L}")

    # ---- dimensions
    want_dims = expected_dimensions(ents, view)
    texts = [it for it in by_layer["DIMENSION"] if it.tag == "text"]
    for did, txt in want_dims.items():
        got = [it.text for it in texts if it.entity == did]
        if not got:
            fail(f"{tag} no DIMENSION <text> with data-entity={did}")
        if txt not in [g.replace(" ", "") for g in got]:
            fail(f"{tag} dimension {did} reads {got}, want {txt} (measured distance in mm)")
    print(f"T2.8 check {view}: ok ({len(items)} elements)")


if __name__ == "__main__":
    main()
