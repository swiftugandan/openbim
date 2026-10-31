"""CLI half of the T1.6 judge: JSON ops files and `bim show` per spec/cli.md and spec/schema.md.

Usage: python -I judges/T1.6/cli.py <path-to-bim>
"""
import atexit
import json
import shutil
import math
import subprocess
import sys
import tempfile
from pathlib import Path

BIM = sys.argv[1]
ULID = "01ARZ3NDEKTSV4RRFFQ69G5FAV"


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def run(*args):
    try:
        p = subprocess.run([BIM, *map(str, args)], capture_output=True, text=True, timeout=30)
    except subprocess.TimeoutExpired:
        fail(f"bim {' '.join(map(str, args))} timed out")
    return p


def ok(*args):
    p = run(*args)
    if p.returncode != 0:
        fail(f"bim {' '.join(map(str, args))} exited {p.returncode}: {p.stderr.strip()}")
    return p.stdout


def expect_code(code, *args):
    p = run(*args)
    if p.returncode != code:
        fail(f"bim {' '.join(map(str, args))} exited {p.returncode}, want {code}: {p.stderr.strip()}")
    return p


def write(path, content):
    Path(path).write_text(content if isinstance(content, str) else json.dumps(content))
    return path


def show(d, ident):
    out = ok("show", d, ident, "--json")
    try:
        return json.loads(out)
    except json.JSONDecodeError as e:
        fail(f"bim show {ident} --json is not one JSON document ({e}): {out!r}")


def same_value(got, want):
    """Exact JSON-encoding comparison: numbers by value (never bools), bools, strings, lists, refs."""
    if isinstance(want, bool):
        return got is want
    if isinstance(want, (int, float)):
        return isinstance(got, (int, float)) and not isinstance(got, bool) and math.isclose(got, want, rel_tol=0, abs_tol=0)
    if isinstance(want, str):
        return got == want
    if isinstance(want, list):
        return isinstance(got, list) and len(got) == len(want) and all(same_value(g, w) for g, w in zip(got, want))
    if isinstance(want, dict):
        return isinstance(got, dict) and set(got) == set(want) and all(same_value(got[k], want[k]) for k in want)
    raise AssertionError(want)


def check_entity(d, ident, kind, props):
    e = show(d, ident)
    if not isinstance(e, dict) or set(e) != {"id", "kind", "props"}:
        fail(f"show {ident}: want keys id, kind, props; got {e!r}")
    if e["id"] != ident:
        fail(f"show {ident}: id is {e['id']!r}; JSON outputs report the string the entity was created with")
    if e["kind"] != kind:
        fail(f"show {ident}: kind {e['kind']!r}, want {kind!r}")
    got = e["props"]
    if not same_value(got.get("_name"), ident):
        fail(f"show {ident}: reserved prop _name is {got.get('_name')!r}, want {ident!r}")
    if not same_value(got.get("_schema"), 1):
        fail(f"show {ident}: reserved prop _schema is {got.get('_schema')!r}, want 1")
    user = {k: v for k, v in got.items() if not k.startswith("_")}
    if not same_value(user, props):
        fail(f"show {ident}: props {user!r}, want {props!r}")


tmp = Path(tempfile.mkdtemp(prefix="t16-"))
atexit.register(shutil.rmtree, tmp, True)
A, B, C = tmp / "A", tmp / "B", tmp / "C"
for d in (A, B, C):
    ok("init", d)

ops = {"schema": 1, "ops": [
    {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 0}},
    {"op": "create", "id": "WT", "kind": "WallType",
     "props": {"layer_thickness": [0.1, 0.25], "layer_material": ["brick", "mineral wool"]}},
    {"op": "create", "id": ULID, "kind": "Wall", "props": {
        "level": {"ref": "L1"}, "type": {"ref": "WT"}, "x1": 0, "y1": 0, "x2": 5.5, "y2": -1e-3,
        "height": 3.0, "name": "north wall, façade", "exterior": True, "loadbearing": False,
        "tags": [{"ref": "L1"}, "a", [1, False], []]}},
    {"op": "set", "id": "L1", "key": "elevation", "value": 3.25},
    {"op": "set", "id": "L1", "key": "label", "value": "ground"},
    {"op": "create", "id": "tmp", "kind": "Level", "props": {}},
    {"op": "delete", "id": "tmp"},
    {"op": "create", "id": "door", "kind": "Opening"},
    {"op": "set", "id": "door", "key": "host", "value": {"ref": ULID}},
]}
ok("apply", A, write(tmp / "a.json", ops))
check_entity(A, "L1", "Level", {"elevation": 3.25, "label": "ground"})
check_entity(A, "WT", "WallType", {"layer_thickness": [0.1, 0.25], "layer_material": ["brick", "mineral wool"]})
check_entity(A, ULID, "Wall", {
    "level": {"ref": "L1"}, "type": {"ref": "WT"}, "x1": 0, "y1": 0, "x2": 5.5, "y2": -1e-3, "height": 3.0,
    "name": "north wall, façade", "exterior": True, "loadbearing": False, "tags": [{"ref": "L1"}, "a", [1, False], []]})
check_entity(A, "door", "Opening", {"host": {"ref": ULID}})
if run("show", A, "tmp", "--json").returncode == 0:
    fail("show of a deleted entity must fail")
if run("show", A, "nobody", "--json").returncode == 0:
    fail("show of an unknown id must fail")
expect_code(0, "validate", A)

# The same model through the T1.5 line format (seeds only) hashes like its JSON twin.
write(tmp / "b.ops", "create L1 Level\nset L1 elevation num 3.25\n\ncreate w Wall\nset w host ref L1\nset w name text north\n")
ok("apply", B, tmp / "b.ops")
ok("apply", C, write(tmp / "c.json", {"schema": 1, "ops": [
    {"op": "create", "id": "w", "kind": "Wall", "props": {"name": "north"}},
    {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 1}},
    {"op": "set", "id": "w", "key": "host", "value": {"ref": "L1"}},
    {"op": "set", "id": "L1", "key": "elevation", "value": 3.25},
]}))
check_entity(B, "w", "Wall", {"host": {"ref": "L1"}, "name": "north"})
check_entity(C, "w", "Wall", {"host": {"ref": "L1"}, "name": "north"})
hb, hc = ok("hash", B).strip(), ok("hash", C).strip()
if hb != hc:
    fail(f"line-format and JSON ops building the same model hash differently ({hb} vs {hc})")
expect_code(0, "diff", B, C)

# Atomicity: any invalid op, or an invalid file, rejects the whole file with exit 2.
before = ok("hash", A).strip()
bad_files = {
    "set on missing id": {"schema": 1, "ops": [
        {"op": "create", "id": "fresh", "kind": "Level", "props": {"elevation": 1}},
        {"op": "set", "id": "L1", "key": "elevation", "value": 9},
        {"op": "set", "id": "ghost", "key": "x", "value": 1}]},
    "create of an existing id": {"schema": 1, "ops": [
        {"op": "create", "id": "fresh", "kind": "Level"}, {"op": "create", "id": "L1", "kind": "Level"}]},
    "delete of a missing id": {"schema": 1, "ops": [{"op": "create", "id": "fresh", "kind": "Level"}, {"op": "delete", "id": "ghost"}]},
    "unknown op": {"schema": 1, "ops": [{"op": "create", "id": "fresh", "kind": "Level"}, {"op": "rename", "id": "L1"}]},
    "null value": {"schema": 1, "ops": [{"op": "create", "id": "fresh", "kind": "Level"}, {"op": "set", "id": "L1", "key": "x", "value": None}]},
    "user write to a reserved prop": {"schema": 1, "ops": [{"op": "create", "id": "fresh", "kind": "Level"}, {"op": "set", "id": "L1", "key": "_name", "value": "evil"}]},
    "unsupported schema version": {"schema": 2, "ops": [{"op": "create", "id": "fresh", "kind": "Level"}]},
    "malformed JSON": '{"schema": 1, "ops": [{"op": "create", "id": "fresh", "kind": "Level"},',
}
for why, content in bad_files.items():
    expect_code(2, "apply", A, write(tmp / "bad.json", content))
    if ok("hash", A).strip() != before:
        fail(f"rejected JSON ops file ({why}) changed the document")
    if run("show", A, "fresh", "--json").returncode == 0:
        fail(f"rejected JSON ops file ({why}) left its first op applied")
check_entity(A, "L1", "Level", {"elevation": 3.25, "label": "ground"})

# Deleting the wall leaves the door's host dangling (T1.5 validate semantics on JSON-built docs).
ok("apply", A, write(tmp / "del.json", {"schema": 1, "ops": [{"op": "delete", "id": ULID}]}))
expect_code(1, "validate", A)
print("T1.6 CLI checks passed")
