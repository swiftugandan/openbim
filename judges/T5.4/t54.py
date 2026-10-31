"""T5.4 reference-corpus round trip, driven from Python (subprocess timeouts; no `timeout` on macOS).

Usage: python -I -B t54.py <bim> <corpus-dir> <tmpdir>

Per corpus/ifc/*.ifc file, in a fresh project:
  bim init; bim import; bim solve --trade arch|struct|mep --json (no error, JSON stdout);
  bim export --format ifc; per storey bim export --format svg|dxf --view plan:<level>@1.2;
then with ifcopenshell on the original vs the round-tripped IFC: per-element volume of every
IfcWall/IfcSlab/IfcColumn/IfcBeam with a body (paired by GlobalId) and per-storey total slab
footprint area agree within 1e-4 relative; every DXF passes ezdxf audit with zero errors; every SVG
parses. All of a file's commands together must take < 60 s.

Id rule (T5.3 Details): an imported entity's id is the 26-char ULID spelling of the 128 bits of its
GlobalId, so a storey imports as the Level `ulid(GlobalId)` and export (T5.2 Details) maps every id
back to the same GlobalId; elements and storeys are therefore paired by GlobalId. The round-tripped
element may carry any IfcElement class (T5.3 keeps non-extrusion geometry on an imported-geometry
entity); storey slab areas are summed over the original's IfcSlabs, paired by GlobalId on both sides.
"""
import glob
import json
import multiprocessing
import os
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

import ezdxf
import ifcopenshell
import ifcopenshell.geom
import ifcopenshell.guid
import ifcopenshell.util.element as uel
import ifcopenshell.util.shape as ush

REL = 1e-4
BUDGET = 60.0
CUT = 1.2
CLASSES = ("IfcWall", "IfcSlab", "IfcColumn", "IfcBeam")
CROCK = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
SETTINGS = ifcopenshell.geom.settings()
SETTINGS.set("use-world-coords", True)


class Fail(Exception):
    pass


def ulid_of_guid(g):
    n = int(ifcopenshell.guid.expand(g), 16)
    return "".join(CROCK[(n >> (5 * (25 - i))) & 31] for i in range(26))


def close(a, b):
    return abs(a - b) <= max(REL * max(abs(a), abs(b)), 1e-9)


def has_body(el):
    rep = el.Representation
    return rep is not None and any(r.RepresentationIdentifier == "Body" for r in rep.Representations)


def storey_of(el):
    c = uel.get_container(el)
    while c is not None and not c.is_a("IfcBuildingStorey"):
        c = uel.get_aggregate(c)
    if c is None:  # parts of an aggregate (e.g. slabs in a roof) take their whole's storey
        agg = uel.get_aggregate(el)
        return storey_of(agg) if agg is not None and agg.is_a("IfcElement") else None
    return c


def comparable(f):
    """Original-side elements whose volume must survive the round trip (T5.4 Details)."""
    return [el for el in f.by_type("IfcElement") if any(el.is_a(c) for c in CLASSES) and has_body(el)]


def measure(f, elements, slab_guids):
    """{guid: volume} for `elements` (world coordinates, multi-threaded iterator) and
    {storey guid: total slab footprint area} over those whose GlobalId is in `slab_guids` (the
    original's IfcSlabs, whatever class they come back as); third result: elements ifcopenshell
    could not mesh."""
    vols, areas = {}, {}
    if elements:
        it = ifcopenshell.geom.iterator(SETTINGS, f, multiprocessing.cpu_count(), include=elements)
        if it.initialize():
            while True:
                sh = it.get()
                g = sh.geometry
                vols[sh.guid] = ush.get_volume(g)
                if sh.guid in slab_guids:
                    st = storey_of(f.by_guid(sh.guid))
                    if st is not None:
                        areas[st.GlobalId] = areas.get(st.GlobalId, 0.0) + ush.get_footprint_area(g)
                if not it.next():
                    break
    unbuildable = [el.GlobalId for el in elements if el.GlobalId not in vols]
    return vols, areas, unbuildable


def run(cmd, deadline, what):
    left = deadline - time.monotonic()
    if left <= 0:
        raise Fail(f"over the {BUDGET:.0f} s per-file budget before {what}")
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=left)
    except subprocess.TimeoutExpired:
        raise Fail(f"{what} still running when the {BUDGET:.0f} s per-file budget ran out")
    return r


def one(bim, path, tmp):
    name = os.path.basename(path)
    proj = os.path.join(tmp, "proj")
    os.makedirs(tmp, exist_ok=True)
    src = ifcopenshell.open(path)
    t0 = time.monotonic()
    deadline = t0 + BUDGET
    r = run([bim, "init", proj], deadline, "init")
    if r.returncode != 0:
        raise Fail(f"init exited {r.returncode}: {r.stderr[:300]}")
    r = run([bim, "import", proj, path], deadline, "import")
    if r.returncode != 0:
        raise Fail(f"import exited {r.returncode}: {r.stderr[:300]}")
    for trade in ("arch", "struct", "mep"):
        r = run([bim, "solve", proj, "--trade", trade, "--json"], deadline, f"solve --trade {trade}")
        if r.returncode not in (0, 1):
            raise Fail(f"solve --trade {trade} exited {r.returncode}: {r.stderr[:300]}")
        try:
            json.loads(r.stdout)
        except ValueError:
            raise Fail(f"solve --trade {trade} did not print one JSON document")
    rt = os.path.join(tmp, "roundtrip.ifc")
    r = run([bim, "export", proj, "--format", "ifc", "--out", rt], deadline, "export --format ifc")
    if r.returncode != 0 or not os.path.exists(rt):
        raise Fail(f"export --format ifc exited {r.returncode}: {r.stderr[:300]}")
    storeys = src.by_type("IfcBuildingStorey")
    if not storeys:
        raise Fail("corpus file has no IfcBuildingStorey; cannot produce per-storey sheets")
    for st in storeys:
        view = f"plan:{ulid_of_guid(st.GlobalId)}@{CUT}"
        for fmt in ("svg", "dxf"):
            out = os.path.join(tmp, f"{st.GlobalId}.{fmt}")
            r = run([bim, "export", proj, "--format", fmt, "--view", view, "--out", out], deadline, f"export {fmt} {view}")
            if r.returncode != 0 or not os.path.exists(out):
                raise Fail(f"export --format {fmt} --view {view} (storey {st.Name!r}) exited {r.returncode}: {r.stderr[:300]}")
            if fmt == "svg":
                try:
                    root = ET.parse(out).getroot()
                except ET.ParseError as e:
                    raise Fail(f"SVG for storey {st.Name!r} is not well-formed XML: {e}")
                if not root.tag.endswith("svg"):
                    raise Fail(f"SVG for storey {st.Name!r} has root <{root.tag}>")
            else:
                try:
                    doc = ezdxf.readfile(out)
                except Exception as e:  # noqa: BLE001
                    raise Fail(f"ezdxf cannot read DXF for storey {st.Name!r}: {e}")
                aud = doc.audit()
                if aud.has_errors:
                    raise Fail(f"ezdxf audit: {len(aud.errors)} errors in DXF for storey {st.Name!r}, first: {aud.errors[0]}")
    elapsed = time.monotonic() - t0
    if elapsed >= BUDGET:
        raise Fail(f"took {elapsed:.1f} s, budget {BUDGET:.0f} s")

    try:
        dst = ifcopenshell.open(rt)
    except Exception as e:  # noqa: BLE001
        raise Fail(f"ifcopenshell cannot open the round-tripped IFC: {e}")
    if dst.schema != "IFC4":
        raise Fail(f"round-tripped file schema {dst.schema}, want IFC4")
    src_els = comparable(src)
    slabs = {el.GlobalId for el in src_els if el.is_a("IfcSlab")}
    v0, a0, bad0 = measure(src, src_els, slabs)
    if not v0:
        raise Fail("no IfcWall/IfcSlab/IfcColumn/IfcBeam with buildable body in the original: nothing to compare")
    # Round-tripped side: the same GlobalIds under any IfcElement class (slab areas paired by the original's class).
    v1, a1, _ = measure(dst, [el for el in dst.by_type("IfcElement") if has_body(el) and el.GlobalId in v0], slabs)
    for g, v in v0.items():
        if g not in v1:
            raise Fail(f"element {g} ({src.by_guid(g).is_a()} {src.by_guid(g).Name!r}) missing or without geometry after round trip")
        if not close(v, v1[g]):
            raise Fail(f"element {g} ({src.by_guid(g).is_a()} {src.by_guid(g).Name!r}): volume {v1[g]} after round trip, original {v}")
    for g, a in a0.items():
        b = a1.get(g, 0.0)
        if not close(a, b):
            raise Fail(f"storey {g} ({src.by_guid(g).Name!r}): slab area {b} after round trip, original {a}")
    return f"{name}: {len(v0)} elements, {len(a0)} storeys with slabs, {elapsed:.1f} s" + \
        (f" ({len(bad0)} original elements ifcopenshell cannot mesh were skipped)" if bad0 else "")


def main():
    bim, corpus, tmp = sys.argv[1], sys.argv[2], sys.argv[3]
    files = sorted(glob.glob(os.path.join(corpus, "*.ifc")))
    if not files:
        print(f"no *.ifc files in {corpus}")
        return 1
    for i, path in enumerate(files):
        try:
            print(one(bim, path, os.path.join(tmp, f"f{i}")), flush=True)
        except Fail as e:
            print(f"{os.path.basename(path)}: {e}")
            return 1
    return 0


sys.exit(main())
