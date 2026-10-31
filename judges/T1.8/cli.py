"""CLI half of the T1.8 judge: kind validation in `bim apply` (JSON ops and the line format), package
loading on every command, and `bim plugin list --json`, per spec/schema.md (Extension kinds),
spec/plugins.md (Trade packages) and spec/cli.md.

Usage: python -I judges/T1.8/cli.py <path-to-bim>
"""
import atexit
import json
import math
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

BIM = sys.argv[1]

# spec/schema.md "Entity kinds" table, ConflictDiagnostic included (sorted).
CORE = sorted([
    "Level", "WallType", "Wall", "Opening", "Slab", "SlabOpening", "CurtainWall", "Stair", "Ramp", "Roof", "Furniture",
    "Product", "Space", "Point", "Material", "Column", "Beam", "Support", "PointLoad", "AreaLoad", "LoadCombination",
    "MepSystem", "Port", "Segment", "Circuit", "Constraint", "Dimension", "Issue", "Shape", "ComponentDefinition",
    "ComponentInstance", "Guide", "Scene", "ConflictDiagnostic",
])
AUTHORED = [k for k in CORE if k != "ConflictDiagnostic"]  # "never authored": not created here

# The spec/schema.md example package (engine file deliberately absent: only invoking it may fail).
FIRE = {"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0",
        "kinds": {
            "fire:Sprinkler": {"props": {"level": "ref", "x": "num", "y": "num", "z": "num", "coverage_radius": "num"},
                               "geometry": {"box": {"centre": ["x", "y", "z"], "size": [0.1, 0.1, 0.1]}}},
            "fire:Main": {"props": {"x1": "num", "y1": "num", "z1": "num", "x2": "num", "y2": "num", "z2": "num",
                                    "diameter": "num"},
                          "geometry": {"segment": {"from": ["x1", "y1", "z1"], "to": ["x2", "y2", "z2"],
                                                   "diameter": "diameter"}}},
            "fire:Zone": {"props": {"name": "text", "active": "bool", "rooms": "list", "level": "ref"}},
        },
        "engine": "fire.wasm"}
ACOUSTIC = {"package": "acoustic", "version": "2.1.0", "abi": "openbim:trade@0.1.0",
            "kinds": {"acoustic:Panel": {"props": {"nrc": "num"}}, "acoustic:Baffle": {"props": {}}}}


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def run(*args):
    try:
        return subprocess.run([BIM, *map(str, args)], capture_output=True, text=True, timeout=30)
    except subprocess.TimeoutExpired:
        fail(f"bim {' '.join(map(str, args))} timed out")


def ok(*args):
    p = run(*args)
    if p.returncode != 0:
        fail(f"bim {' '.join(map(str, args))} exited {p.returncode}, want 0: {p.stderr.strip()[-400:]}")
    return p.stdout


def expect_code(code, *args):
    p = run(*args)
    if p.returncode != code:
        fail(f"bim {' '.join(map(str, args))} exited {p.returncode}, want {code}: {p.stderr.strip()[-400:]}")
    return p


def write(path, content):
    Path(path).write_text(content if isinstance(content, str) else json.dumps(content))
    return path


def ops(*o):
    return {"schema": 1, "ops": list(o)}


def create(i, kind, **props):
    return {"op": "create", "id": i, "kind": kind, "props": props}


def sett(i, key, value):
    return {"op": "set", "id": i, "key": key, "value": value}


def ref(i):
    return {"ref": i}


def install(d, dirname, package):
    p = Path(d) / "plugins" / dirname
    p.mkdir(parents=True, exist_ok=True)
    write(p / "trade.json", package)


def hash_of(d):
    return ok("hash", d).strip()


def show(d, ident):
    out = ok("show", d, ident, "--json")
    try:
        return json.loads(out)
    except json.JSONDecodeError as e:
        fail(f"bim show {ident} --json is not one JSON document ({e}): {out!r}")


def rejects(d, path, why):
    """An apply that must exit 2 and change nothing."""
    before = hash_of(d)
    expect_code(2, "apply", d, path)
    if hash_of(d) != before:
        fail(f"rejected apply ({why}) changed the document hash")


tmp = Path(tempfile.mkdtemp(prefix="t18-"))
atexit.register(shutil.rmtree, tmp, True)

# ---------------------------------------------------------------- 1. core kinds, no packages
A = tmp / "A"
ok("init", A)
ok("apply", A, write(tmp / "core.json", ops(
    create("L1", "Level", elevation=0),
    create("WT", "WallType", layer_thickness=[0.2], layer_material=["concrete"], layer_conductivity=[1.6]),
    create("W1", "Wall", level=ref("L1"), type=ref("WT"), x1=0, y1=0, x2=4, y2=0, height=3,
           exterior=True, note="free-form props on core kinds are not type-checked"),
    create("P1", "Product", catalog="acme", catalog_version="1", sku="T-1", category="roof_covering", name="tile",
           manufacturer="Acme", thickness=0.02),
    create("R1", "Roof", level=ref("L1"), boundary=[0, 0, 4, 0, 4, 3, 0, 3], slope=0.5, angle=0, thickness=0.2,
           covering=ref("P1")),
    create("F1", "Furniture", level=ref("L1"), x=1, y=1, angle=0, product=ref("P1"), size=[1, 0.5, 0.8]),
    create("SH", "Shape", vertices=[0, 0, 0, 1, 0, 0, 0, 1, 0], faces=[[[0, 1, 2]]], face_ids=[1], edges=[]),
    create("CD", "ComponentDefinition", name="tri", shape=ref("SH")),
    create("CI", "ComponentInstance", definition=ref("CD"),
           transform=[1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 2, 0, 0, 1]),
    create("G1", "Guide", point=[0, 0, 0], direction=[1, 0, 0]),
    create("SC", "Scene", name="front", eye=[5, -5, 2], target=[0, 0, 0], fov=0.8),
)))
ok("apply", A, write(tmp / "core.ops", "".join(f"create k{i} {k}\n" for i, k in enumerate(AUTHORED))))
ok("apply", A, write(tmp / "core2.ops", "set W1 height text tall\nset R1 slope num 0.25\nset F1 product ref P1\n"))
if show(A, "R1").get("kind") != "Roof" or show(A, f"k{AUTHORED.index('Scene')}").get("kind") != "Scene":
    fail("core kinds were not created")

for why, path in [
    ("JSON create of the undeclared kind Bogus", write(tmp / "u1.json", ops(create("n1", "Level"), create("b1", "Bogus")))),
    ("JSON create of fire:Sprinkler with no package", write(tmp / "u2.json", ops(create("s1", "fire:Sprinkler", x=1)))),
    ("JSON create of the lower-case kind wall", write(tmp / "u3.json", ops(create("w9", "wall")))),
    ("line create of the undeclared kind Bogus", write(tmp / "u4.ops", "create n1 Level\ncreate b1 Bogus\n")),
    ("line create of fire:Sprinkler with no package", write(tmp / "u5.ops", "create s1 fire:Sprinkler\n")),
]:
    rejects(A, path, why)
if run("show", A, "n1", "--json").returncode == 0:
    fail("a rejected file left its valid first op applied")
p = ok("plugin", "list", A, "--json")
try:
    lst = json.loads(p)
except json.JSONDecodeError as e:
    fail(f"plugin list --json is not one JSON document ({e}): {p[:300]!r}")
if lst.get("packages") != [] or lst.get("core_kinds") != CORE:
    fail(f"plugin list without packages: want packages [] and core_kinds {CORE}, got {json.dumps(lst)[:600]}")

# ---------------------------------------------------------------- 2. with packages
h_core = hash_of(A)
install(A, "fire", FIRE)
install(A, "acoustic", ACOUSTIC)
if hash_of(A) != h_core:
    fail("installing packages changed the document hash")
ok("apply", A, write(tmp / "ext.json", ops(
    create("S1", "fire:Sprinkler", level=ref("L1"), x=1.5, y=2, z=2.8, coverage_radius=2.5),
    create("S2", "fire:Sprinkler"),
    create("M1", "fire:Main", x1=0, y1=0, z1=3, x2=4, y2=0, z2=3, diameter=0.1),
    create("Z1", "fire:Zone", name="north", active=True, rooms=["a", 1, [ref("L1")]], level=ref("L1")),
    sett("S2", "x", 3.25), sett("S2", "level", ref("L1")), sett("Z1", "active", False), sett("Z1", "rooms", []),
    create("AP", "acoustic:Panel", nrc=0.85),
    create("W2", "Wall", level=ref("L1"), anything="goes", height=False),
)))
ok("apply", A, write(tmp / "ext.ops", "create S3 fire:Sprinkler\nset S3 x num 1\nset S3 level ref L1\n"
                                       "set Z1 name text south\ncreate AB acoustic:Baffle\ndelete AB\n"))
s1 = show(A, "S1")
want = {"level": ref("L1"), "x": 1.5, "y": 2, "z": 2.8, "coverage_radius": 2.5}
if s1.get("kind") != "fire:Sprinkler" or {k: v for k, v in s1.get("props", {}).items() if not k.startswith("_")} != want:
    fail(f"show S1: want kind fire:Sprinkler and props {want}, got {s1}")
z1 = show(A, "Z1")
if z1.get("kind") != "fire:Zone" or z1.get("props", {}).get("name") != "south" or z1["props"].get("active") is not False \
        or z1["props"].get("rooms") != []:
    fail(f"show Z1: extension props not applied: {z1}")
s3 = show(A, "S3")
if s3.get("props", {}).get("x") != 1 or s3["props"].get("level") != ref("L1"):
    fail(f"show S3 (line format on an extension kind): {s3}")
if run("show", A, "AB", "--json").returncode == 0:
    fail("a deleted extension entity is still shown")

bad = {
    "create of fire:Hose (not declared by the fire package)": ops(create("H1", "fire:Hose")),
    "create of a namespaced kind whose package is absent": ops(create("H2", "struct:Brace")),
    "create with a num prop given as text": ops(create("S9", "fire:Sprinkler", x="1.5")),
    "create with a num prop given as a bool": ops(create("S9", "fire:Sprinkler", z=True)),
    "create with a ref prop given as text": ops(create("S9", "fire:Sprinkler", level="L1")),
    "create with a num prop given as a list": ops(create("S9", "fire:Sprinkler", coverage_radius=[2.5])),
    "create with a text prop given as a number": ops(create("Z9", "fire:Zone", name=3)),
    "create with a bool prop given as text": ops(create("Z9", "fire:Zone", active="true")),
    "create with a list prop given as text": ops(create("Z9", "fire:Zone", rooms="a,b")),
    "create with a ref prop given as a number": ops(create("Z9", "fire:Zone", level=0)),
    "create with an undeclared prop": ops(create("S9", "fire:Sprinkler", x=1, colour="red")),
    "create with a prop of another kind": ops(create("M9", "fire:Main", x=1)),
    "set of a num prop to text": ops(sett("S1", "x", "2")),
    "set of a ref prop to a number": ops(sett("S1", "level", 1)),
    "set of a bool prop to a number": ops(sett("Z1", "active", 0)),
    "set of a list prop to a ref": ops(sett("Z1", "rooms", ref("L1"))),
    "set of an undeclared prop": ops(sett("M1", "colour", "red")),
    "set of a reserved prop on an extension entity": ops(sett("S1", "_name", "evil")),
    "valid ops followed by an invalid set (atomicity)": ops(create("S8", "fire:Sprinkler", x=1),
                                                             sett("S1", "x", 9.0), sett("S1", "y", "north")),
}
for i, (why, content) in enumerate(bad.items()):
    rejects(A, write(tmp / f"bad{i}.json", content), why)
for i, (why, text) in enumerate({
    "line create of fire:Hose": "create H1 fire:Hose\n",
    "line set of a num prop to text": "set S1 x text two\n",
    "line set of a text prop to a number": "set Z1 name num 4\n",
    "line set of a ref prop to text": "set S1 level text L1\n",
    "line set of a num prop to a ref": "set S1 z ref L1\n",
    "line set of an undeclared prop": "set S1 colour text red\n",
    "line create then an invalid set (atomicity)": "create S8 fire:Sprinkler\nset S8 x num 1\nset S8 y text a\n",
}.items()):
    rejects(A, write(tmp / f"badl{i}.ops", text), why)
for gone in ("S8", "S9", "H1", "Z9", "M9"):
    if run("show", A, gone, "--json").returncode == 0:
        fail(f"entity {gone} exists although every file creating it was rejected")
if show(A, "S1")["props"].get("x") != 1.5:
    fail("a rejected file changed S1.x")
# Core kinds stay unchecked while packages are loaded.
ok("apply", A, write(tmp / "core3.json", ops(sett("W1", "height", [1, 2]), create("k99", "Furniture", whatever="x"))))
ok("apply", A, write(tmp / "core3.ops", "create k98 Shape\nset k98 colour text red\n"))

lst = json.loads(ok("plugin", "list", A, "--json"))
want_pk = [
    {"name": "acoustic", "version": "2.1.0", "abi": "openbim:trade@0.1.0", "kinds": ["acoustic:Baffle", "acoustic:Panel"],
     "engine": None},
    {"name": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0",
     "kinds": ["fire:Main", "fire:Sprinkler", "fire:Zone"], "engine": "plugins/fire/fire.wasm"},
]
got_pk = lst.get("packages")
if not isinstance(got_pk, list) or len(got_pk) != 2 or not all(isinstance(x, dict) for x in got_pk):
    fail(f"plugin list: want 2 packages, got {json.dumps(lst)[:600]}")
for g, w in zip(got_pk, want_pk):
    if {k: g.get(k, "<missing>") for k in w} != w:
        fail(f"plugin list package: want {w} (packages sorted by name, kinds sorted, engine relative to <dir> "
             f"or null), got {g}")
if lst.get("core_kinds") != CORE:
    fail(f"plugin list core_kinds: want {CORE}, got {lst.get('core_kinds')}")

# ---------------------------------------------------------------- 3. broken packages: every command exits 2
B = tmp / "B"
ok("init", B)
ok("apply", B, write(tmp / "b.json", ops(create("L1", "Level", elevation=0))))
hb = hash_of(B)
C = tmp / "C"
ok("init", C)
good_kinds = {"fire:Sprinkler": {"props": {"x": "num"}}}


def pkg(name, abi="openbim:trade@0.1.0", kinds=None):
    return {"package": name, "version": "0.1.0", "abi": abi, "kinds": good_kinds if kinds is None else kinds}


broken = [
    ("malformed JSON", "fire", '{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0", "kinds": {'),
    ("directory name differs from package", "fire2", pkg("fire")),
    ("package is the built-in trade mep", "mep", pkg("mep", kinds={"mep:Hanger": {"props": {"x": "num"}}})),
    ("package is the built-in trade struct", "struct", pkg("struct", kinds={"struct:Brace": {"props": {}}})),
    ("kind outside the package namespace", "fire", pkg("fire", kinds={"fire:Sprinkler": {"props": {}},
                                                                     "acoustic:Panel": {"props": {}}})),
    ("kind without a namespace", "fire", pkg("fire", kinds={"Sprinkler": {"props": {}}})),
    ("wrong abi", "fire", pkg("fire", abi="openbim:plugin@0.1.0")),
    ("unknown prop type", "fire", pkg("fire", kinds={"fire:Sprinkler": {"props": {"x": "float"}}})),
    ("unknown geometry template", "fire", pkg("fire", kinds={
        "fire:Sprinkler": {"props": {"x": "num"}, "geometry": {"cylinder": {"centre": ["x", 0, 0], "radius": 1}}}})),
]
for why, dirname, content in broken:
    install(B, dirname, content)
    install(B, "acoustic", ACOUSTIC)  # a valid sibling must not mask the broken one
    name = f"{dirname}/trade.json"
    for cmd in (["hash", B], ["apply", B, write(tmp / "ok.json", ops(create("L2", "Level", elevation=3)))],
                ["show", B, "L1", "--json"], ["validate", B], ["diff", B, C], ["plugin", "list", B, "--json"]):
        p = expect_code(2, *cmd)
        if name not in p.stderr:
            fail(f"package with a defect ({why}): 'bim {cmd[0]}' exited 2 but stderr does not name the file "
                 f"{name}: {p.stderr.strip()[-300:]!r}")
    shutil.rmtree(B / "plugins")
    if hash_of(B) != hb:
        fail(f"package with a defect ({why}): an apply changed the document")
print("T1.8 CLI checks passed")
