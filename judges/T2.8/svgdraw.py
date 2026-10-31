"""Minimal SVG reader for the drawing contract of spec/cli.md: layer groups, paper-mm geometry
(with viewBox/width and nested transforms applied), effective stroke widths, data-entity, text.
"""
import math
import re
import xml.etree.ElementTree as ET

import numpy as np

LAYERS = {"CUT": 0.50, "PROJECTION": 0.25, "HIDDEN": 0.18, "HATCH": 0.13, "DIMENSION": 0.18, "ANNOTATION": 0.18}
SHAPES = {"line", "polyline", "polygon", "path", "rect"}


def local(tag):
    return tag.rsplit("}", 1)[-1]


def parse_style(el):
    st = {}
    for part in (el.get("style") or "").split(";"):
        if ":" in part:
            k, v = part.split(":", 1)
            st[k.strip()] = v.strip()
    return st


def attr(el, name):
    st = parse_style(el)
    return st.get(name, el.get(name))


def parse_len(s):
    m = re.fullmatch(r"\s*([-+0-9.eE]+)\s*(mm|px|pt|cm|in)?\s*", s or "")
    if not m:
        return None
    v = float(m.group(1))
    return v * {"mm": 1, None: 1, "px": 1, "pt": 25.4 / 72, "cm": 10, "in": 25.4}[m.group(2)]


def parse_transform(s):
    m = np.eye(3)
    for name, args in re.findall(r"(\w+)\s*\(([^)]*)\)", s or ""):
        a = [float(x) for x in re.split(r"[\s,]+", args.strip()) if x]
        t = np.eye(3)
        if name == "matrix":
            t = np.array([[a[0], a[2], a[4]], [a[1], a[3], a[5]], [0, 0, 1]])
        elif name == "translate":
            t[0, 2] = a[0]
            t[1, 2] = a[1] if len(a) > 1 else 0
        elif name == "scale":
            t[0, 0] = a[0]
            t[1, 1] = a[1] if len(a) > 1 else a[0]
        elif name == "rotate":
            c, s_ = math.cos(math.radians(a[0])), math.sin(math.radians(a[0]))
            r = np.array([[c, -s_, 0], [s_, c, 0], [0, 0, 1]])
            if len(a) == 3:
                tr = np.array([[1, 0, a[1]], [0, 1, a[2]], [0, 0, 1]])
                r = tr @ r @ np.linalg.inv(tr)
            t = r
        else:
            raise ValueError(f"unsupported transform {name}")
        m = m @ t
    return m


def nums(s):
    return [float(x) for x in re.findall(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?", s or "")]


def path_rings(d):
    """Subpaths of an SVG path with only straight commands -> list of (points, closed)."""
    toks = re.findall(r"[MmLlHhVvZzCcSsQqTtAa]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?", d or "")
    out, cur, pos, start, cmd, i = [], [], (0.0, 0.0), (0.0, 0.0), None, 0
    while i < len(toks):
        t = toks[i]
        if re.fullmatch(r"[A-Za-z]", t):
            cmd = t
            i += 1
            if cmd in "Zz":
                if cur:
                    out.append((cur, True))
                cur, pos = [], start
                continue
            if cmd in "CcSsQqTtAa":
                raise ValueError(f"curved path command {cmd} in a drawing path")
            continue
        def take(n):
            nonlocal i
            v = [float(x) for x in toks[i:i + n]]
            i += n
            return v
        if cmd in "Mm":
            x, y = take(2)
            if cmd == "m":
                x, y = pos[0] + x, pos[1] + y
            if cur:
                out.append((cur, False))
            cur, pos, start = [(x, y)], (x, y), (x, y)
            cmd = "L" if cmd == "M" else "l"
        elif cmd in "Ll":
            x, y = take(2)
            if cmd == "l":
                x, y = pos[0] + x, pos[1] + y
            pos = (x, y)
            cur.append(pos)
        elif cmd in "Hh":
            (x,) = take(1)
            pos = (x if cmd == "H" else pos[0] + x, pos[1])
            cur.append(pos)
        elif cmd in "Vv":
            (y,) = take(1)
            pos = (pos[0], y if cmd == "V" else pos[1] + y)
            cur.append(pos)
        else:
            raise ValueError(f"bad path data near {t}")
    if cur:
        out.append((cur, False))
    return out


class Item:
    """One drawable element in paper millimetres."""

    def __init__(self, layer, tag, rings, entity, width, dashed, text=None):
        self.layer, self.tag, self.rings, self.entity = layer, tag, rings, entity
        self.width, self.dashed, self.text = width, dashed, text

    def segments(self):
        for pts, closed in self.rings:
            seq = list(pts) + ([pts[0]] if closed and len(pts) > 1 else [])
            for a, b in zip(seq, seq[1:]):
                if a != b:
                    yield (a, b)


def load(path):
    tree = ET.parse(path)
    root = tree.getroot()
    if local(root.tag) != "svg":
        raise ValueError("root element is not <svg>")
    k = 1.0
    vb = nums(root.get("viewBox"))
    w = root.get("width")
    if vb and w and parse_len(w) is not None and len(vb) == 4 and vb[2] > 0:
        k = parse_len(w) / vb[2]
    base = np.diag([k, k, 1.0])
    if vb:
        base = base @ np.array([[1, 0, -vb[0]], [0, 1, -vb[1]], [0, 0, 1]])
    items, layer_groups = [], {}

    def walk(el, m, layer, entity, width, dashed):
        tag = local(el.tag)
        if el.get("transform"):
            m = m @ parse_transform(el.get("transform"))
        if tag == "g" and el.get("id") in LAYERS:
            layer = el.get("id")
            layer_groups.setdefault(layer, 0)
            layer_groups[layer] += 1
        entity = el.get("data-entity", entity)
        sw = attr(el, "stroke-width")
        if sw is not None:
            width = (parse_len(sw), m)
        da = attr(el, "stroke-dasharray")
        if da is not None:
            dashed = da.strip() not in ("none", "")
        if tag in SHAPES or tag == "text":
            ew = None
            if width is not None and width[0] is not None:
                wm = width[1]
                ew = width[0] * math.sqrt(abs(np.linalg.det(wm[:2, :2])))
            def tp(pts):
                return [tuple((m @ np.array([x, y, 1.0]))[:2]) for x, y in pts]
            rings = []
            if tag == "line":
                rings = [(tp([(float(el.get("x1", 0)), float(el.get("y1", 0))),
                              (float(el.get("x2", 0)), float(el.get("y2", 0)))]), False)]
            elif tag in ("polyline", "polygon"):
                v = nums(el.get("points"))
                pts = list(zip(v[0::2], v[1::2]))
                closed = tag == "polygon" or (len(pts) > 2 and pts[0] == pts[-1])
                if closed and pts and pts[0] == pts[-1]:
                    pts = pts[:-1]
                rings = [(tp(pts), closed)]
            elif tag == "rect":
                x, y = float(el.get("x", 0)), float(el.get("y", 0))
                ww, hh = float(el.get("width", 0)), float(el.get("height", 0))
                rings = [(tp([(x, y), (x + ww, y), (x + ww, y + hh), (x, y + hh)]), True)]
            elif tag == "path":
                try:
                    subpaths = path_rings(el.get("d"))
                except ValueError:
                    # Curves (e.g. door swings) are allowed off the straight-line layers.
                    if layer in ("CUT", "PROJECTION", "HIDDEN", "HATCH"):
                        raise
                    subpaths = []
                for pts, closed in subpaths:
                    if closed and len(pts) > 1 and pts[0] == pts[-1]:
                        pts = pts[:-1]
                    rings.append((tp(pts), closed))
            text = None
            if tag == "text":
                text = "".join(el.itertext()).strip()
                rings = [(tp([(float(nums(el.get("x"))[0] if el.get("x") else 0),
                              float(nums(el.get("y"))[0] if el.get("y") else 0))]), False)]
            if layer is not None:
                items.append(Item(layer, tag, rings, entity, ew, dashed, text))
        for c in el:
            walk(c, m, layer, entity, width, dashed)

    walk(root, base, None, None, None, False)
    return items, layer_groups, k
