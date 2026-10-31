# External judge for T7.3: drives `bim render` (native wgpu, offscreen) through the TASKS.md Spec.
# Usage: python -I check.py <path-to-bim>
# Expected values are derived from the scenes' own geometry: every pixel location is found
# from the image itself (ID buffer runs, image centre = --target), never from an assumed FOV.
import atexit
import json
import os
import shutil
import subprocess
import sys
import tempfile

import numpy as np

sys.dont_write_bytecode = True  # keep judges/ free of __pycache__
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pngread import read_png  # noqa: E402

BIM = sys.argv[1]
TMP = tempfile.mkdtemp(prefix="judge-t73-")
atexit.register(shutil.rmtree, TMP, True)


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def run(args, timeout=60):
    try:
        p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        fail(f"'bim {' '.join(args)}' timed out after {timeout}s")
    return p


def must(args):
    p = run(args)
    if p.returncode != 0:
        fail(f"'bim {' '.join(args)}' exited {p.returncode}: {p.stderr.strip()[:400]}")
    return p.stdout


# ---------------------------------------------------------------- scenes (JSON ops, spec/schema.md)
def ref(i):
    return {"ref": i}


COMMON = [
    {"op": "create", "id": "L0", "kind": "Level", "props": {"elevation": 0.0}},
    {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 3.0}},
    {"op": "create", "id": "WT", "kind": "WallType", "props": {
        "layer_thickness": [0.2], "layer_material": ["concrete"], "layer_conductivity": [1.7]}},
    {"op": "create", "id": "M", "kind": "Material", "props": {
        "name": "steel", "E": 2.1e11, "G": 8.1e10, "density": 7850.0, "fy": 3.55e8}},
]


def wall(i, x1, y1, x2, y2):
    return {"op": "create", "id": i, "kind": "Wall", "props": {
        "level": ref("L0"), "x1": x1, "y1": y1, "x2": x2, "y2": y2, "height": 3.0, "type": ref("WT")}}


def column(i, x, y, b, h):
    return {"op": "create", "id": i, "kind": "Column", "props": {
        "x": x, "y": y, "base": ref("L0"), "top": ref("L1"), "section_b": b, "section_h": h,
        "material": ref("M")}}


def project(name, ops):
    d = os.path.join(TMP, name)
    must(["init", d])
    f = os.path.join(TMP, name + ".json")
    with open(f, "w") as fh:
        json.dump({"schema": 1, "ops": COMMON + ops}, fh)
    must(["apply", d, f])
    return d


def vec(v):
    return ",".join(repr(float(c)) for c in v)


def render(d, eye, target, size, out, extra=()):
    w, h = size
    must(["render", d, "--eye", vec(eye), "--target", vec(target), "--size", f"{w}x{h}",
          "--out", out, *extra])
    img = read_png(out)
    if img.shape[:2] != (h, w):
        fail(f"{os.path.basename(out)} is {img.shape[1]}x{img.shape[0]}, want {w}x{h}")
    return img.astype(np.int32)


def pick(d, eye, target, size, px, py):
    w, h = size
    out = os.path.join(TMP, "pick.png")
    p = run(["render", d, "--eye", vec(eye), "--target", vec(target), "--size", f"{w}x{h}",
             "--out", out, "--pick", f"{px},{py}"])
    if p.returncode != 0:
        fail(f"--pick {px},{py} exited {p.returncode}: {p.stderr.strip()[:400]}")
    return p.stdout.strip()


def dist(a, b):
    return np.abs(np.asarray(a, dtype=np.int32) - np.asarray(b, dtype=np.int32)).max(axis=-1)


def runs(row):
    """Split a row of colours into maximal runs of identical colour: [(start, end_excl, colour)]."""
    out, s = [], 0
    for x in range(1, len(row) + 1):
        if x == len(row) or (row[x] != row[s]).any():
            out.append((s, x, tuple(row[s])))
            s = x
    return out


def lum(img):
    return img[..., 0] * 0.2126 + img[..., 1] * 0.7152 + img[..., 2] * 0.0722


# ------------------------------------------------------------------ 1. picking (L-shaped walls)
# Walls A (along +x) and B (along +y) meet in an L at the origin. --target puts the aimed point at
# the image centre for any FOV, so picking the centre pixel must return the wall that was aimed at.
SIZE = (320, 240)
CX, CY = SIZE[0] // 2, SIZE[1] // 2
lw = project("L", [wall("A", 0, 0, 6, 0), wall("B", 0, 0, 0, 6)])
EYE_IN = (5.0, 5.0, 1.5)
got = pick(lw, EYE_IN, (3.0, 0.0, 1.5), SIZE, CX, CY)
if got != "A":
    fail(f"pick at the image centre aimed at wall A's face returned '{got}', want 'A'")
got = pick(lw, EYE_IN, (0.0, 3.0, 1.5), SIZE, CX, CY)
if got != "B":
    fail(f"pick at the image centre aimed at wall B's face returned '{got}', want 'B'")

# ---------------------------------------- 2. instanced identical columns: ID buffer and picking
# Three identical columns share one family mesh (T7.2); each instance must keep its own id.
cols = project("C", [column("c1", -3, 0, 0.6, 0.6), column("c2", 0, 0, 0.6, 0.6),
                     column("c3", 3, 0, 0.6, 0.6)])
# 15 m away the 0.6 m columns are >= 4 px wide and 20 px apart for any vertical FOV up to 90 degrees.
EYE_C, TGT_C = (0.0, -15.0, 1.5), (0.0, 0.0, 1.5)
colour = render(cols, EYE_C, TGT_C, SIZE, os.path.join(TMP, "c.png"),
                ["--ids", os.path.join(TMP, "c_ids.png")])
ids = read_png(os.path.join(TMP, "c_ids.png")).astype(np.int32)
if ids.shape[:2] != (SIZE[1], SIZE[0]):
    fail(f"--ids image is {ids.shape[1]}x{ids.shape[0]}, want {SIZE[0]}x{SIZE[1]}")
bg_id = tuple(ids[0, 0])
corners = [tuple(ids[0, -1]), tuple(ids[-1, 0]), tuple(ids[-1, -1])]
if any(c != bg_id for c in corners):
    fail("--ids background (empty image corners) is not one uniform value")
row = runs(ids[CY])
fg = [r for r in row if r[2] != bg_id]
if len(fg) != 3:
    fail(f"ID buffer centre row should cross exactly 3 column instances, found {len(fg)} id runs")
if len({r[2] for r in fg}) != 3:
    fail("the three column instances share an ID-buffer value; per-instance ids are required")
for (s, e, _), want in zip(fg, ["c1", "c2", "c3"]):
    got = pick(cols, EYE_C, TGT_C, SIZE, (s + e - 1) // 2, CY)
    if got != want:
        fail(f"pick on instance run {s}..{e} returned '{got}', want '{want}' (left to right = -x to +x)")
# The colour image must show geometry exactly where the ID buffer does (same camera).
cbg = colour[0, 0]
geo_c = dist(colour[CY], cbg) > 12
geo_i = np.array([tuple(p) != bg_id for p in ids[CY]])
if (geo_c != geo_i).mean() > 0.03:
    fail("colour image and ID buffer disagree about where geometry is on the centre row")

# --------------------------------------------- 3. SSAO darkens the inside corner of the L walls
# Seen from inside the L, the seam between A and B (found in the ID buffer) is an inside corner.
# Luminance near the seam must drop (SSAO on vs --no-ssao) clearly more than on the open faces.
ssao = render(lw, EYE_IN, (0.0, 0.0, 1.5), SIZE, os.path.join(TMP, "s1.png"),
              ["--ids", os.path.join(TMP, "s_ids.png")])
flat = render(lw, EYE_IN, (0.0, 0.0, 1.5), SIZE, os.path.join(TMP, "s0.png"), ["--no-ssao"])
sids = read_png(os.path.join(TMP, "s_ids.png")).astype(np.int32)
sbg = tuple(sids[0, 0])
r = [x for x in runs(sids[CY]) if x[2] != sbg]
seams = [i for i in range(len(r) - 1) if r[i][1] == r[i + 1][0] and r[i][2] != r[i + 1][2]]
if len(seams) != 1:
    fail(f"expected one A|B seam in the ID buffer centre row, found {len(seams)}")
left, right = r[seams[0]], r[seams[0] + 1]
seam = left[1]
if seam - left[0] < 40 or right[1] - seam < 40:
    fail("walls too narrow in the ID buffer to measure SSAO (check camera framing)")
rows = slice(CY - 10, CY + 11)
L_s, L_f = lum(ssao), lum(flat)


def band(img, x0, x1):
    return float(img[rows, x0:x1].mean())


near = [(seam - 7, seam - 2), (seam + 2, seam + 7)]
far = [((left[0] + seam) // 2 - 3, (left[0] + seam) // 2 + 3),
       ((seam + right[1]) // 2 - 3, (seam + right[1]) // 2 + 3)]
for a, b in near + far:
    if band(L_f, a, b) < 15:
        fail("wall faces render (nearly) black with --no-ssao; cannot measure SSAO")
ratio_near = np.mean([band(L_s, a, b) / band(L_f, a, b) for a, b in near])
ratio_far = np.mean([band(L_s, a, b) / band(L_f, a, b) for a, b in far])
if not ratio_near < ratio_far - 0.05:
    fail(f"SSAO does not darken the inside corner: near-seam lum ratio {ratio_near:.3f} vs "
         f"open-face ratio {ratio_far:.3f} (want at least 0.05 lower)")

# -------------------------------------------------------- 4. section box with capped cut faces
# A 4 x 4 x 3 m column centred at the origin, cut by the box [0,10]x[0,10]x[-1,2]: what remains is
# the cube [0,2]^3 whose three faces visible from EYE_S (planes x=0, y=0, z=2) are all cut faces.
# So with the box every geometry pixel must be the one cap colour, and everything else background.
blk = project("S", [column("K", 0, 0, 4, 4)])
# Camera 15 m from the corner: the uncut column stays inside the image at a 30 degree vertical FOV
# and the remaining cube covers >= 0.6 % of the image at 90 degrees (the FOV is not in the contract).
EYE_S, TGT_S, SZ = (-8.0, -11.2, 7.2), (0.0, 0.0, 2.0), (384, 384)
cx, cy = SZ[0] // 2, SZ[1] // 2
nobox = render(blk, EYE_S, TGT_S, SZ, os.path.join(TMP, "n.png"), ["--no-ssao"])
boxed = render(blk, EYE_S, TGT_S, SZ, os.path.join(TMP, "b.png"),
               ["--no-ssao", "--section-box", "0,0,-1,10,10,2"])
empty = render(blk, EYE_S, TGT_S, SZ, os.path.join(TMP, "e.png"),
               ["--no-ssao", "--section-box", "50,50,50,60,60,60"])
bg = nobox[0, 0]
for img, what in [(nobox, "no-box"), (boxed, "section-box")]:
    if any(dist(img[y, x], bg) > 6 for y, x in [(0, -1), (-1, 0), (-1, -1)]):
        fail(f"{what} render: image corners are not one uniform background")
# The three cut faces meet at (0,0,2), which --target places at the image centre: the top cap lies
# straight up, the x=0 cap down-left and the y=0 cap down-right of it (from the view geometry).
d = 8
samples = [boxed[cy - d - 1:cy - d + 2, cx - 1:cx + 2], boxed[cy + d - 1:cy + d + 2, cx - d - 1:cx - d + 2],
           boxed[cy + d - 1:cy + d + 2, cx + d - 1:cx + d + 2]]
cap = np.median(np.concatenate([s.reshape(-1, 3) for s in samples]), axis=0)
if dist(cap, bg) <= 40:
    fail("section box: cut faces show background, not a cap (no stencil caps?)")
for s in samples:
    if dist(s.reshape(-1, 3), cap).max() > 8:
        fail("section box: the three cut faces are not one uniform cap colour")
is_bg = dist(boxed, bg) <= 12
is_cap = dist(boxed, cap) <= 12
other = ~(is_bg | is_cap)
if is_cap.sum() < 0.004 * is_cap.size:
    fail("section box: cap region is too small")
if other.sum() > 0.25 * is_cap.sum():
    fail(f"section box: {int(other.sum())} pixels are neither cap colour nor background "
         "(geometry outside the box still drawn, or caps shaded)")
was_geo = dist(nobox, bg) > 12
if (was_geo & is_bg).sum() < 0.05 * was_geo.sum():
    fail("section box did not remove the geometry outside the box")
uncut = [nobox[cy - d, cx], nobox[cy + d, cx - d], nobox[cy + d, cx + d]]
if sum(int(dist(u, cap)) > 30 for u in uncut) < 2:
    fail("cap colour is indistinguishable from the uncut solid's faces")
if (dist(empty, bg) > 6).mean() > 0.001:
    fail("a section box excluding all geometry still shows geometry")

print("T7.3 judge: all checks passed")
