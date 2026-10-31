"""Generate the example floor plan SVG (model units: millimetres) for the openbim project page.

One-bedroom, two-person flat, 10.2 x 6.0 m external, 300 mm external walls, 100 mm partitions.
Checked against the UK Nationally Described Space Standard (1b2p): GIA >= 50 m2, double bedroom
>= 11.5 m2 and >= 2.75 m wide, built-in storage >= 1.5 m2.
Run: python docs/floor_plan.py > plan.svg (needs shapely), then paste the <svg> into docs/index.html.
"""
from shapely.geometry import box
from shapely.ops import unary_union

def segs(lo, hi, gaps):
    out, cur = [], lo
    for a, b in sorted(gaps):
        out.append((cur, a)); cur = b
    out.append((cur, hi))
    return out

walls = []
H = lambda y0, y1, x0, x1, gaps=(): walls.extend(box(a, y0, b, y1) for a, b in segs(x0, x1, gaps))
V = lambda x0, x1, y0, y1, gaps=(): walls.extend(box(x0, a, x1, b) for a, b in segs(y0, y1, gaps))
# external walls
H(0, 300, 0, 10200, [(1500, 3900), (6400, 8400)])                     # north: living, bedroom windows
H(5700, 6000, 0, 10200, [(1500, 2500), (5450, 6350), (7200, 8200)])   # south: kitchen window, entrance, bathroom window
V(0, 300, 300, 5700, [(1500, 3900)])                                  # west: living window
V(9900, 10200, 300, 5700)                                             # east
# partitions
V(5100, 5200, 300, 5700, [(3500, 4300)])                              # living | east zone, door to hall
H(3100, 3200, 5200, 9900, [(5350, 6150), (9000, 9700)])               # bedroom | hall, bathroom, store
V(6700, 6800, 3200, 5700, [(3350, 4150)])                             # hall | bathroom
V(8700, 8800, 3200, 5700)                                             # bathroom | store

u = unary_union(walls)
polys = list(u.geoms) if u.geom_type == "MultiPolygon" else [u]
def ring(c): return "M" + " L".join(f"{x:g} {y:g}" for x, y in list(c)[:-1]) + " Z"
wall_d = " ".join(ring(p.exterior.coords) + "".join(" " + ring(i.coords) for i in p.interiors) for p in polys)

rooms = {  # name: (x0, y0, x1, y1), label position
    "Living and kitchen": ((300, 300, 5100, 5700), (2000, 1650)),
    "Bedroom": ((5200, 300, 9900, 3100), (6700, 1500)),
    "Hall": ((5200, 3200, 6700, 5700), (5950, 3850)),
    "Bathroom": ((6800, 3200, 8700, 5700), (7650, 4500)),
    "Store": ((8800, 3200, 9900, 5700), (9350, 4200)),
}
area = {n: (r[2] - r[0]) * (r[3] - r[1]) / 1e6 for n, (r, _) in rooms.items()}
gia = (10200 - 600) * (6000 - 600) / 1e6
assert gia >= 50 and area["Bedroom"] >= 11.5 and min(4700, 2800) >= 2750 and area["Store"] >= 1.5, (gia, area)

def window(x0, y0, x1, y1):
    """Opening across a wall: both wall faces seen below the cut, and a double glazing line."""
    if x1 - x0 > y1 - y0:   # horizontal wall
        ym = (y0 + y1) / 2
        ls = [(x0, y0, x1, y0, "p"), (x0, y1, x1, y1, "p"), (x0, ym - 25, x1, ym - 25, "t"), (x0, ym + 25, x1, ym + 25, "t")]
    else:
        xm = (x0 + x1) / 2
        ls = [(x0, y0, x0, y1, "p"), (x1, y0, x1, y1, "p"), (xm - 25, y0, xm - 25, y1, "t"), (xm + 25, y0, xm + 25, y1, "t")]
    return "".join(f'<line x1="{a}" y1="{b}" x2="{c}" y2="{d}" class="{k}"/>' for a, b, c, d, k in ls)

windows = "".join(window(*w) for w in [(1500, 0, 3900, 300), (6400, 0, 8400, 300), (0, 1500, 300, 3900),
                                       (1500, 5700, 2500, 6000), (7200, 5700, 8200, 6000)])
doors = [  # leaf rectangle (open at 90 degrees) and swing arc
    ((5450, 4800, 40, 900), "M5450 4800 A900 900 0 0 1 6350 5700"),   # entrance, into hall
    ((4300, 3500, 800, 40), "M4300 3500 A800 800 0 0 0 5100 4300"),   # hall -> living
    ((5350, 2300, 40, 800), "M5350 2300 A800 800 0 0 1 6150 3100"),   # hall -> bedroom
    ((6800, 3350, 800, 40), "M7600 3350 A800 800 0 0 1 6800 4150"),   # hall -> bathroom
    ((9660, 3200, 40, 700), "M9700 3900 A700 700 0 0 1 9000 3200"),   # bedroom -> store
]
door_svg = "".join(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" class="p"/><path d="{d}" class="t"/>' for (x, y, w, h), d in doors)

fixtures = "".join([
    '<rect x="300" y="5100" width="3000" height="600" class="t"/>',                 # worktop
    '<rect x="1700" y="5200" width="600" height="400" rx="60" class="t"/>',         # sink
    *[f'<circle cx="{cx}" cy="{cy}" r="110" class="t"/>' for cx in (650, 950) for cy in (5250, 5550)],  # hob
    '<rect x="1200" y="3200" width="1400" height="800" class="t"/>',                # dining table
    *[f'<rect x="{x}" y="{y}" width="400" height="400" class="t"/>' for x in (1450, 1950) for y in (2750, 4050)],
    '<rect x="4250" y="600" width="800" height="2000" rx="60" class="t"/><line x1="4850" y1="700" x2="4850" y2="2500" class="t"/>',  # sofa
    '<rect x="3500" y="1100" width="500" height="1000" class="t"/>',                # coffee table
    '<rect x="7900" y="950" width="2000" height="1500" class="t"/><line x1="9100" y1="950" x2="9100" y2="2450" class="t"/>',  # bed
    '<rect x="9500" y="1050" width="350" height="600" rx="60" class="t"/><rect x="9500" y="1750" width="350" height="600" rx="60" class="t"/>',
    '<rect x="9450" y="450" width="450" height="450" class="t"/><rect x="9450" y="2500" width="450" height="450" class="t"/>',  # bedside
    '<rect x="6800" y="5000" width="1700" height="700" class="t"/><rect x="6900" y="5080" width="1500" height="540" rx="220" class="t"/>',  # bath
    '<rect x="8500" y="3700" width="200" height="500" class="t"/><ellipse cx="8230" cy="3950" rx="270" ry="190" class="t"/>',  # WC
    '<rect x="7800" y="3200" width="500" height="450" class="t"/><ellipse cx="8050" cy="3430" rx="180" ry="140" class="t"/>',  # basin
    '<rect x="9500" y="4600" width="400" height="1100" class="t"/><line x1="9500" y1="5150" x2="9900" y2="5150" class="t"/>',  # shelving
])

labels = "".join(
    f'<text x="{x}" y="{y}" class="room">{n}</text><text x="{x}" y="{y + 330}" class="area">{area[n]:.1f} m²</text>'
    for n, (_, (x, y)) in rooms.items())

GX = {"A": 150, "B": 5150, "C": 10050}; GY = {"1": 150, "2": 5850}
grid = "".join(f'<line x1="{x}" y1="-550" x2="{x}" y2="6350" class="g"/>' for x in GX.values())
grid += "".join(f'<line x1="-450" y1="{y}" x2="10750" y2="{y}" class="g"/>' for y in GY.values())
grid += "".join(f'<circle cx="{x}" cy="-800" r="250" class="b"/><text x="{x}" y="-710" class="bt">{k}</text>' for k, x in GX.items())
grid += "".join(f'<circle cx="11000" cy="{y}" r="250" class="b"/><text x="11000" y="{y + 90}" class="bt">{k}</text>' for k, y in GY.items())

def dim(x0, x1, y, ext_from):
    t = lambda x: f'<line x1="{x - 70}" y1="{y + 70}" x2="{x + 70}" y2="{y - 70}" class="dk"/>'
    return (f'<line x1="{x0 - 150}" y1="{y}" x2="{x1 + 150}" y2="{y}" class="d"/>'
            f'<line x1="{x0}" y1="{ext_from}" x2="{x0}" y2="{y + 150}" class="d"/><line x1="{x1}" y1="{ext_from}" x2="{x1}" y2="{y + 150}" class="d"/>'
            + t(x0) + t(x1) + f'<text x="{(x0 + x1) / 2}" y="{y - 90}" class="dt">{x1 - x0:g}</text>')
dims = dim(GX["A"], GX["B"], 6700, 6450) + dim(GX["B"], GX["C"], 6700, 6450) + dim(0, 10200, 7300, 6150)

north = ('<g transform="translate(11000 -800)"><circle r="250" class="b"/>'
         '<path d="M0 -230 L120 160 L0 80 L-120 160 Z" class="na"/></g>'
         '<text x="11000" y="-1120" class="bt">N</text>')

svg = f'''<svg class="plan" viewBox="-700 -1400 12200 9050" role="img" aria-labelledby="plan-t">
          <title id="plan-t">Floor plan of a one-bedroom flat for 2 people, 51.8 square metres: living and kitchen {area["Living and kitchen"]:.1f}, bedroom {area["Bedroom"]:.1f}, bathroom {area["Bathroom"]:.1f}, hall {area["Hall"]:.1f} and store {area["Store"]:.1f} square metres</title>
          <defs><pattern id="hatch" width="160" height="160" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><line x1="0" y1="0" x2="0" y2="160" stroke="var(--hatch)" stroke-width="18"/></pattern></defs>
          <g class="grid">{grid}</g>
          <g class="fix">{fixtures}</g>
          <path class="cut" fill-rule="evenodd" d="{wall_d}"/>
          <g class="win">{windows}</g>
          <g class="door">{door_svg}</g>
          <g>{labels}</g>
          <g>{dims}</g>
          {north}
        </svg>'''
print(svg)
import sys; print({k: round(v, 2) for k, v in area.items()}, "GIA", gia, file=sys.stderr)
