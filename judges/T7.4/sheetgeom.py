# Format-neutral readers for T7.4 sheets (also used by the T8.2 judge).
# Each reader returns sheet geometry in paper millimetres, origin at the sheet's lower-left corner,
# y up, as a list of primitives:
#   {"layer": "CUT"|... or None, "entity": id or None, "bbox": (x0, y0, x1, y1) visible part after
#    clipping or None if fully clipped, "raw": unclipped bbox, "width": stroke width mm or None,
#    "dash": [mm, ...] or None, "kind": "shape"|"text", "text": str, "segs": [((x0,y0),(x1,y1)), ...]}
import math
import re
import xml.etree.ElementTree as ET

import numpy as np

LAYERS = ("CUT", "PROJECTION", "HIDDEN", "HATCH", "DIMENSION", "ANNOTATION")
MM_PER = {"mm": 1.0, "cm": 10.0, "in": 25.4, "pt": 25.4 / 72, "pc": 25.4 / 6, "px": 25.4 / 96, "": 25.4 / 96}
NUM = r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?"


class SheetError(Exception):
    pass


def layer_of_token(s):
    for tok in re.split(r"[^A-Za-z0-9]+", s or ""):
        if tok in LAYERS:
            return tok
    return None


def bbox_of(pts):
    a = np.asarray(pts, dtype=float).reshape(-1, 2)
    return (a[:, 0].min(), a[:, 1].min(), a[:, 0].max(), a[:, 1].max())


EMPTY = (math.inf, math.inf, -math.inf, -math.inf)


def inter(a, b):
    """Intersection of two boxes where None means unbounded (no clip). Returns None only when both
    are None; an empty intersection is EMPTY."""
    if a is None:
        return b
    if b is None:
        return a
    r = (max(a[0], b[0]), max(a[1], b[1]), min(a[2], b[2]), min(a[3], b[3]))
    return r if r[0] <= r[2] + 1e-9 and r[1] <= r[3] + 1e-9 else EMPTY


def visible(raw, clip):
    """Visible part of a raw bbox under a clip box (None = no clip); None when fully clipped."""
    r = inter(raw, clip)
    return None if r is EMPTY else r


# ------------------------------------------------------------------------------------------- SVG
def _len_mm(s):
    m = re.fullmatch(rf"\s*({NUM})\s*([a-z%]*)\s*", s or "")
    if not m or m.group(2) not in MM_PER:
        raise SheetError(f"SVG length '{s}' has no absolute unit")
    return float(m.group(1)) * MM_PER[m.group(2)]


def _nums(s):
    return [float(v) for v in re.findall(NUM, s or "")]


def _mat(a, b, c, d, e, f):
    return np.array([[a, c, e], [b, d, f], [0, 0, 1.0]])


def _transform(s):
    m = np.eye(3)
    for name, args in re.findall(r"(\w+)\s*\(([^)]*)\)", s or ""):
        v = _nums(args)
        if name == "matrix":
            t = _mat(*v[:6])
        elif name == "translate":
            t = _mat(1, 0, 0, 1, v[0], v[1] if len(v) > 1 else 0)
        elif name == "scale":
            t = _mat(v[0], 0, 0, v[1] if len(v) > 1 else v[0], 0, 0)
        elif name == "rotate":
            a = math.radians(v[0])
            t = _mat(math.cos(a), math.sin(a), -math.sin(a), math.cos(a), 0, 0)
            if len(v) == 3:
                t = _mat(1, 0, 0, 1, v[1], v[2]) @ t @ _mat(1, 0, 0, 1, -v[1], -v[2])
        elif name == "skewX":
            t = _mat(1, 0, math.tan(math.radians(v[0])), 1, 0, 0)
        elif name == "skewY":
            t = _mat(1, math.tan(math.radians(v[0])), 0, 1, 0, 0)
        else:
            raise SheetError(f"unknown SVG transform {name}")
        m = m @ t
    return m


def _apply(m, pts):
    a = np.asarray(pts, dtype=float).reshape(-1, 2)
    return (m[:2, :2] @ a.T).T + m[:2, 2]


def _arc_points(x0, y0, rx, ry, phi, large, sweep, x1, y1):
    # SVG implementation notes F.6.5: endpoint to centre parameterisation, then sample.
    if rx == 0 or ry == 0:
        return [(x1, y1)]
    rx, ry = abs(rx), abs(ry)
    c, s = math.cos(math.radians(phi)), math.sin(math.radians(phi))
    dx, dy = (x0 - x1) / 2, (y0 - y1) / 2
    xp, yp = c * dx + s * dy, -s * dx + c * dy
    lam = xp * xp / (rx * rx) + yp * yp / (ry * ry)
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = max(rx * rx * ry * ry - rx * rx * yp * yp - ry * ry * xp * xp, 0.0)
    den = rx * rx * yp * yp + ry * ry * xp * xp
    k = math.sqrt(num / den) if den else 0.0
    if large == sweep:
        k = -k
    cxp, cyp = k * rx * yp / ry, -k * ry * xp / rx
    cx, cy = c * cxp - s * cyp + (x0 + x1) / 2, s * cxp + c * cyp + (y0 + y1) / 2

    def ang(ux, uy, vx, vy):
        return math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)

    t1 = ang(1, 0, (xp - cxp) / rx, (yp - cyp) / ry)
    dt = ang((xp - cxp) / rx, (yp - cyp) / ry, (-xp - cxp) / rx, (-yp - cyp) / ry)
    if not sweep and dt > 0:
        dt -= 2 * math.pi
    elif sweep and dt < 0:
        dt += 2 * math.pi
    out = []
    for i in range(1, 33):
        t = t1 + dt * i / 32
        out.append((cx + rx * math.cos(t) * c - ry * math.sin(t) * s, cy + rx * math.cos(t) * s + ry * math.sin(t) * c))
    return out


def _path_points(d):
    """Return (all points incl. control points, straight segments) of an SVG path in local units."""
    toks = re.findall(rf"[MmLlHhVvCcSsQqTtAaZz]|{NUM}", d or "")
    pts, segs = [], []
    i, cmd = 0, None
    cur = start = (0.0, 0.0)
    last_ctrl = None

    def num():
        nonlocal i
        v = float(toks[i])
        i += 1
        return v

    while i < len(toks):
        if re.fullmatch(r"[A-Za-z]", toks[i]):
            cmd = toks[i]
            i += 1
            if cmd in "Zz":
                segs.append((cur, start))
                cur = start
                continue
        if cmd is None:
            raise SheetError("SVG path data does not start with a command")
        rel = cmd.islower()
        C = cmd.upper()
        ox, oy = cur if rel else (0.0, 0.0)
        if C == "M":
            cur = (ox + num(), oy + num())
            start = cur
            pts.append(cur)
            cmd = "l" if rel else "L"
            continue
        if C in "LT":
            p = (ox + num(), oy + num())
            segs.append((cur, p))
        elif C == "H":
            p = ((ox if rel else 0) + num(), cur[1])
            segs.append((cur, p))
        elif C == "V":
            p = (cur[0], (oy if rel else 0) + num())
            segs.append((cur, p))
        elif C == "C":
            c1 = (ox + num(), oy + num())
            c2 = (ox + num(), oy + num())
            p = (ox + num(), oy + num())
            pts += [c1, c2]
        elif C in "SQ":
            c1 = (ox + num(), oy + num())
            p = (ox + num(), oy + num())
            pts.append(c1)
        elif C == "A":
            rx, ry, phi, la, sw = num(), num(), num(), num(), num()
            p = (ox + num(), oy + num())
            pts += _arc_points(cur[0], cur[1], rx, ry, phi, int(la), int(sw), p[0], p[1])
        else:
            raise SheetError(f"unsupported SVG path command {cmd}")
        pts.append(p)
        cur = p
    return pts, segs


def _parse_css(text):
    rules = []
    for sel, body in re.findall(r"([^{}]+)\{([^}]*)\}", re.sub(r"/\*.*?\*/", "", text or "", flags=re.S)):
        decl = _style_decl(body)
        for s in sel.split(","):
            parts = s.split()
            if parts:
                rules.append((parts, decl))
    return rules


def _style_decl(s):
    out = {}
    for item in (s or "").split(";"):
        if ":" in item:
            k, v = item.split(":", 1)
            out[k.strip()] = v.strip()
    return out


def _simple_match(sel, el, tag):
    m = re.fullmatch(r"([A-Za-z][\w-]*|\*)?((?:[#.][\w-]+)*)", sel)
    if not m:
        return False
    if m.group(1) and m.group(1) not in ("*", tag):
        return False
    classes = (el.get("class") or "").split()
    for kind, name in re.findall(r"([#.])([\w-]+)", m.group(2)):
        if kind == "#" and el.get("id") != name:
            return False
        if kind == "." and name not in classes:
            return False
    return True


INHERITED = ("stroke", "stroke-width", "stroke-dasharray", "fill", "visibility")
STYLE_PROPS = INHERITED + ("display", "vector-effect", "clip-path", "overflow")


def parse_svg(path):
    tree = ET.parse(path)
    root = tree.getroot()

    def tag(el):
        return el.tag.split("}")[-1]

    if tag(root) != "svg":
        raise SheetError("root element is not <svg>")
    ids = {el.get("id"): el for el in root.iter() if el.get("id")}
    css = []
    for el in root.iter():
        if tag(el) == "style":
            css += _parse_css(el.text)
    W = _len_mm(root.get("width"))
    H = _len_mm(root.get("height"))

    def viewport_map(el, w, h):
        vb = _nums(el.get("viewBox"))
        if len(vb) != 4:
            return np.eye(3)
        sx, sy = w / vb[2], h / vb[3]
        par = (el.get("preserveAspectRatio") or "xMidYMid meet").split()
        if par[0] == "none":
            return _mat(sx, 0, 0, sy, -vb[0] * sx, -vb[1] * sy)
        s = max(sx, sy) if len(par) > 1 and par[1] == "slice" else min(sx, sy)
        al = par[0]
        tx = {"xMin": 0, "xMid": 0.5, "xMax": 1}[al[:4]] * (w - vb[2] * s)
        ty = {"YMin": 0, "YMid": 0.5, "YMax": 1}[al[4:]] * (h - vb[3] * s)
        return _mat(s, 0, 0, s, tx - vb[0] * s, ty - vb[1] * s)

    # paper mm with y down -> sheet mm with y up
    flip = _mat(1, 0, 0, -1, 0, H)
    prims = []
    NONRENDER = {"defs", "clipPath", "pattern", "symbol", "marker", "mask", "style", "title", "desc",
                 "metadata", "linearGradient", "radialGradient", "filter"}

    def props_of(el, ancestors, inherited):
        p = {k: v for k, v in inherited.items() if k in INHERITED}
        for k in STYLE_PROPS:
            if el.get(k) is not None:
                p[k] = el.get(k)
        t = tag(el)
        chain = ancestors + [el]
        for parts, decl in css:
            if not _simple_match(parts[-1], el, t):
                continue
            j, ok = len(chain) - 2, True
            for sel in reversed(parts[:-1]):
                while j >= 0 and not _simple_match(sel, chain[j], tag(chain[j])):
                    j -= 1
                if j < 0:
                    ok = False
                    break
                j -= 1
            if ok:
                p.update({k: v for k, v in decl.items() if k in STYLE_PROPS})
        p.update({k: v for k, v in _style_decl(el.get("style")).items() if k in STYLE_PROPS})
        return p

    def clip_box(ref, ctm):
        m = re.fullmatch(r"\s*url\(\s*['\"]?#([^)'\"]+)['\"]?\s*\)\s*", ref or "")
        if not m or m.group(1) not in ids:
            return None
        cp = ids[m.group(1)]
        if cp.get("clipPathUnits") == "objectBoundingBox":
            raise SheetError("clipPathUnits=objectBoundingBox is not supported by the judge")
        cm = ctm @ _transform(cp.get("transform"))
        boxes = []
        for ch in cp.iter():
            if ch is cp:
                continue
            g = shape_points(ch)
            if g:
                boxes.append(bbox_of(_apply(cm @ _transform(ch.get("transform")), g[0])))
        if not boxes:
            return EMPTY
        a = np.array(boxes)
        return (a[:, 0].min(), a[:, 1].min(), a[:, 2].max(), a[:, 3].max())

    def fnum(el, k, d=0.0):
        v = el.get(k)
        return float(re.match(NUM, v.strip()).group(0)) if v else d

    def shape_points(el):
        t = tag(el)
        if t == "line":
            p = [(fnum(el, "x1"), fnum(el, "y1")), (fnum(el, "x2"), fnum(el, "y2"))]
            return p, [(p[0], p[1])]
        if t == "rect":
            x, y, w, h = fnum(el, "x"), fnum(el, "y"), fnum(el, "width"), fnum(el, "height")
            p = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
            return p, [(p[i], p[(i + 1) % 4]) for i in range(4)]
        if t in ("polyline", "polygon"):
            v = _nums(el.get("points"))
            p = list(zip(v[0::2], v[1::2]))
            segs = [(p[i], p[i + 1]) for i in range(len(p) - 1)]
            if t == "polygon" and p:
                segs.append((p[-1], p[0]))
            return (p, segs) if p else None
        if t == "circle":
            cx, cy, r = fnum(el, "cx"), fnum(el, "cy"), fnum(el, "r")
            return [(cx + r * math.cos(a), cy + r * math.sin(a)) for a in np.linspace(0, 2 * math.pi, 33)], []
        if t == "ellipse":
            cx, cy, rx, ry = fnum(el, "cx"), fnum(el, "cy"), fnum(el, "rx"), fnum(el, "ry")
            return [(cx + rx * math.cos(a), cy + ry * math.sin(a)) for a in np.linspace(0, 2 * math.pi, 33)], []
        if t == "path":
            pts, segs = _path_points(el.get("d"))
            return (pts, segs) if pts else None
        return None

    def walk(el, ctm, clip, inherited, ancestors, layer, entity):
        t = tag(el)
        if t in NONRENDER:
            return
        p = props_of(el, ancestors, inherited)
        if p.get("display") == "none":
            return
        layer = layer_of_token(el.get("id")) or layer_of_token(el.get("data-layer")) or \
            layer_of_token(el.get("class")) or layer
        entity = el.get("data-entity") or entity
        local = ctm @ _transform(el.get("transform"))
        if el is not root and t == "svg":
            x, y = fnum(el, "x"), fnum(el, "y")
            w, h = _len_mm_or_user(el.get("width")), _len_mm_or_user(el.get("height"))
            if p.get("overflow") not in ("visible", "auto"):
                clip = inter(clip, bbox_of(_apply(local, [(x, y), (x + w, y + h)])))
            local = local @ _mat(1, 0, 0, 1, x, y) @ viewport_map(el, w, h)
        if p.get("clip-path") and p["clip-path"] != "none":
            cb = clip_box(p["clip-path"], local)
            if cb is not None:
                clip = inter(clip, cb)
        if t == "use":
            href = el.get("href") or el.get("{http://www.w3.org/1999/xlink}href") or ""
            target = ids.get(href.lstrip("#"))
            if target is not None:
                walk(target, local @ _mat(1, 0, 0, 1, fnum(el, "x"), fnum(el, "y")), clip, p,
                     ancestors + [el], layer, entity)
            return
        if t == "text":
            txt = "".join(el.itertext()).strip()
            xs, ys = _nums(el.get("x")) or [0.0], _nums(el.get("y")) or [0.0]
            pt = _apply(local, [(xs[0], ys[0])])
            b = bbox_of(pt)
            prims.append({"layer": layer, "entity": entity, "bbox": visible(b, clip), "raw": b, "width": None,
                          "dash": None, "kind": "text", "text": txt, "segs": []})
            return
        g = shape_points(el)
        if g is not None:
            pts, segs = g
            raw = bbox_of(_apply(local, pts))
            stroke = p.get("stroke", "none")
            width = dash = None
            scale = math.sqrt(abs(np.linalg.det(local[:2, :2])))
            if stroke not in ("none", "transparent") and p.get("visibility") != "hidden":
                sw = float(re.match(NUM, p.get("stroke-width", "1").strip()).group(0))
                unit = (25.4 / 96) if p.get("vector-effect") == "non-scaling-stroke" else scale
                width = sw * unit
                da = p.get("stroke-dasharray")
                if da and da.strip() != "none":
                    v = _nums(da)
                    if len(v) % 2:
                        v = v * 2
                    dash = [x * unit for x in v]
            fill = p.get("fill", "black")
            if width is None and fill in ("none", "transparent"):
                return
            segs_sheet = [tuple(map(tuple, _apply(local, [a, b]))) for a, b in segs]
            prims.append({"layer": layer, "entity": entity, "bbox": visible(raw, clip), "raw": raw,
                          "width": width, "dash": dash, "kind": "shape", "text": "", "segs": segs_sheet})
        for ch in el:
            walk(ch, local, clip, p, ancestors + [el], layer, entity)

    def _len_mm_or_user(s):
        return float(re.match(NUM, (s or "0").strip()).group(0))

    base = flip @ viewport_map(root, W, H)
    walk(root, base, None, {}, [], None, None)
    return (W, H), prims


# ------------------------------------------------------------------------------------------- PDF
def parse_pdf(path):
    from pypdf import PdfReader
    from pypdf.generic import ContentStream

    rd = PdfReader(path)
    if len(rd.pages) < 1:
        raise SheetError("PDF has no pages")
    page = rd.pages[0]
    mb = [float(v) for v in page.mediabox]
    k = 25.4 / 72
    size = ((mb[2] - mb[0]) * k, (mb[3] - mb[1]) * k)
    ocg_names = set()
    try:
        for o in rd.trailer["/Root"]["/OCProperties"]["/OCGs"]:
            ocg_names.add(str(o.get_object()["/Name"]))
    except KeyError:
        pass
    prims = []
    base = np.array([[k, 0, -mb[0] * k], [0, k, -mb[1] * k], [0, 0, 1.0]])

    def deref(x):
        return x.get_object() if hasattr(x, "get_object") else x

    def run(ops_src, resources, ctm0, clip0, oc_stack0):
        resources = deref(resources) or {}
        props = deref(resources.get("/Properties", {}))
        xobjs = deref(resources.get("/XObject", {}))
        extg = deref(resources.get("/ExtGState", {}))
        gs = {"ctm": ctm0, "lw": 1.0, "dash": [], "clip": clip0}
        stack, oc = [], list(oc_stack0)
        path_pts, path_segs, cur, start, pending_clip = [], [], None, None, False
        tm = np.eye(3)
        for operands, op in ops_src.operations:
            op = op.decode() if isinstance(op, bytes) else op
            if op == "q":
                stack.append(dict(gs))
            elif op == "Q":
                gs = stack.pop() if stack else gs
            elif op == "cm":
                a = [float(x) for x in operands]
                gs["ctm"] = gs["ctm"] @ _mat(*a)
            elif op == "w":
                gs["lw"] = float(operands[0])
            elif op == "d":
                gs["dash"] = [float(x) for x in operands[0]]
            elif op == "gs":
                d = extg[operands[0]].get_object() if operands[0] in extg else {}
                if "/LW" in d:
                    gs["lw"] = float(d["/LW"])
                if "/D" in d:
                    gs["dash"] = [float(x) for x in d["/D"][0]]
            elif op in ("m", "l"):
                p = (float(operands[0]), float(operands[1]))
                if op == "l" and cur is not None:
                    path_segs.append((cur, p))
                if op == "m":
                    start = p
                cur = p
                path_pts.append(p)
            elif op in ("c", "v", "y"):
                v = [float(x) for x in operands]
                path_pts += list(zip(v[0::2], v[1::2]))
                cur = (v[-2], v[-1])
            elif op == "re":
                x, y, w, h = [float(v) for v in operands]
                q = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
                path_pts += q
                path_segs += [(q[i], q[(i + 1) % 4]) for i in range(4)]
                cur = start = (x, y)
            elif op == "h":
                if cur is not None and start is not None:
                    path_segs.append((cur, start))
                    cur = start
            elif op in ("W", "W*"):
                pending_clip = True
            elif op in ("S", "s", "f", "F", "f*", "B", "B*", "b", "b*", "n"):
                if path_pts:
                    m = base @ gs["ctm"]
                    raw = bbox_of(_apply(m, path_pts))
                    if op != "n":
                        stroked = op in ("S", "s", "B", "B*", "b", "b*")
                        sc = math.sqrt(abs(np.linalg.det(m[:2, :2])))
                        prims.append({"layer": oc[-1] if oc else None, "entity": None,
                                      "bbox": visible(raw, gs["clip"]), "raw": raw,
                                      "width": gs["lw"] * sc if stroked else None,
                                      "dash": [d * sc for d in gs["dash"]] if stroked and gs["dash"] else None,
                                      "kind": "shape", "text": "",
                                      "segs": [tuple(map(tuple, _apply(m, [a, b]))) for a, b in path_segs]})
                    if pending_clip:
                        gs["clip"] = inter(gs["clip"], raw)
                path_pts, path_segs, cur, start, pending_clip = [], [], None, None, False
            elif op == "BDC":
                name = None
                if str(operands[0]) == "/OC":
                    pr = operands[1]
                    d = props[pr].get_object() if not hasattr(pr, "keys") else pr
                    name = layer_of_token(str(d.get("/Name", "")))
                oc.append(name if name else (oc[-1] if oc else None))
            elif op == "BMC":
                oc.append(oc[-1] if oc else None)
            elif op == "EMC":
                if oc:
                    oc.pop()
            elif op == "BT":
                tm = np.eye(3)
            elif op == "Tm":
                tm = _mat(*[float(x) for x in operands])
            elif op in ("Td", "TD"):
                tm = tm @ _mat(1, 0, 0, 1, float(operands[0]), float(operands[1]))
            elif op in ("Tj", "TJ", "'", '"'):
                pt = _apply(base @ gs["ctm"] @ tm, [(0, 0)])
                b = bbox_of(pt)
                prims.append({"layer": oc[-1] if oc else None, "entity": None, "bbox": visible(b, gs["clip"]),
                              "raw": b, "width": None, "dash": None, "kind": "text", "text": "", "segs": []})
            elif op == "Do":
                xo = xobjs[operands[0]].get_object()
                if xo.get("/Subtype") == "/Form":
                    m = gs["ctm"] @ (_mat(*[float(v) for v in xo["/Matrix"]]) if "/Matrix" in xo else np.eye(3))
                    clip = gs["clip"]
                    if "/BBox" in xo:
                        bb = [float(v) for v in xo["/BBox"]]
                        fb = bbox_of(_apply(base @ m, [(bb[0], bb[1]), (bb[2], bb[3]), (bb[0], bb[3]), (bb[2], bb[1])]))
                        clip = inter(clip, fb)
                    run(ContentStream(xo, rd), xo.get("/Resources", resources), m, clip, oc)

    contents = page.get_contents()
    if contents is not None:
        run(contents if hasattr(contents, "operations") else ContentStream(contents, rd),
            page.get("/Resources", {}), np.eye(3), None, [])
    text = page.extract_text() or ""
    return size, prims, text, ocg_names
