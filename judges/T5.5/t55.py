"""T5.5 judge helper (oracle side). Builders must not edit judges/.

  python -I -B t55.py fixture <outdir>       ops.json (building document, ULID ids) + expected.json
  python -I -B t55.py gen-ifc <outdir>       source.ifc authored with ifcopenshell.api
  python -I -B t55.py to-ifcjson <in.ifc> <out.json>
        The judge's own IFC -> ifcJSON 0.0.1 converter, written from the buildingSMART ifcJSON
        specification (same conventions as its reference converter ifc2json4 with inverses off):
        {"type":"ifcJSON","version":"0.0.1","schemaIdentifier":"IFC4","data":[...]}; one top-level
        object per IfcRoot instance (relationships last) with "type" = IFC entity name, "globalId" =
        the GlobalId in UUID form, attribute keys lowerCamelCase; references to IfcRoot instances
        {"type":…, "ref": <uuid>}; every other entity nested inline; typed values
        {"type":"IfcLabel","value":…}; enums as strings; booleans as JSON booleans.
  python -I -B t55.py check-export <outdir> <export.json>
  python -I -B t55.py compare <solveX.json> <solveY.json> <x.ifc> <y.ifc>
"""
import json
import random
import re
import sys
import uuid

import ifcopenshell
import ifcopenshell.api.aggregate
import ifcopenshell.api.context
import ifcopenshell.api.feature
import ifcopenshell.api.geometry
import ifcopenshell.api.material
import ifcopenshell.api.pset
import ifcopenshell.api.root
import ifcopenshell.api.spatial
import ifcopenshell.api.unit
import ifcopenshell.geom
import ifcopenshell.guid
import ifcopenshell.util.shape as ush
import numpy as np
from ifcopenshell import api

CROCK = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
REL = 1e-4


def ulid(n):
    return "".join(CROCK[(n >> (5 * (25 - i))) & 31] for i in range(26))


def ulid_value(s):
    n = 0
    for ch in s:
        n = n * 32 + CROCK.index(ch)
    return n


class Fail(Exception):
    pass


def need(c, msg):
    if not c:
        raise Fail(msg)


def close(a, b):
    return abs(a - b) <= max(REL * max(abs(a), abs(b)), 1e-9)


# ---------------------------------------------------------------- document fixture (doc -> ifcJSON -> doc)
def fixture(out):
    rng = random.Random(55)
    names = ["L0", "L1", "WT", "M1", "W1", "W2", "W3", "W4", "D1", "N1", "S0", "S1", "SO1", "C1", "B1", "SP1"]
    ids = {n: ulid(rng.getrandbits(128)) for n in names}
    R = lambda n: {"ref": ids[n]}  # noqa: E731
    ents = [
        ("L0", "Level", {"elevation": 0.0}),
        ("L1", "Level", {"elevation": 3.1}),
        ("WT", "WallType", {"layer_thickness": [0.1, 0.15], "layer_material": ["Brick", "Mineral wool"],
                            "layer_conductivity": [0.77, 0.04]}),
        ("M1", "Material", {"name": "C30/37", "E": 3.3e10, "G": 1.375e10, "density": 2500.0, "fy": 3.0e7}),
        ("W1", "Wall", {"level": R("L0"), "x1": 0.0, "y1": 0.0, "x2": 7.0, "y2": 0.0, "height": 3.0, "type": R("WT")}),
        ("W2", "Wall", {"level": R("L0"), "x1": 7.0, "y1": 0.0, "x2": 7.0, "y2": 5.0, "height": 3.0, "type": R("WT")}),
        ("W3", "Wall", {"level": R("L0"), "x1": 10.0, "y1": 1.0, "x2": 13.0, "y2": 5.0, "height": 2.6, "type": R("WT"),
                        "base_offset": 0.05}),
        ("W4", "Wall", {"level": R("L1"), "x1": 0.0, "y1": 9.0, "x2": 4.0, "y2": 9.0, "height": 2.7, "type": R("WT")}),
        ("D1", "Opening", {"host": R("W1"), "kind": "door", "offset": 2.0, "sill": 0.0, "width": 1.0, "height": 2.2}),
        ("N1", "Opening", {"host": R("W2"), "kind": "window", "offset": 1.5, "sill": 1.0, "width": 1.5, "height": 1.1}),
        ("S0", "Slab", {"level": R("L0"), "boundary": [20.0, 0.0, 25.0, 0.0, 25.0, 4.0, 20.0, 4.0], "thickness": 0.25}),
        ("S1", "Slab", {"level": R("L1"), "boundary": [0.0, 0.0, 7.0, 0.0, 7.0, 5.0, 0.0, 5.0], "thickness": 0.2}),
        ("SO1", "SlabOpening", {"host": R("S1"), "boundary": [1.0, 1.0, 2.0, 1.0, 2.0, 2.0, 1.0, 2.0]}),
        ("C1", "Column", {"x": 3.5, "y": 2.5, "base": R("L0"), "top": R("L1"), "section_b": 0.3, "section_h": 0.3,
                          "material": R("M1")}),
        ("B1", "Beam", {"level": R("L1"), "x1": 0.5, "y1": 2.5, "x2": 6.5, "y2": 2.5, "section_b": 0.25,
                        "section_h": 0.5, "material": R("M1"), "z_offset": 0.2}),
        ("SP1", "Space", {"level": R("L0"), "x": 3.5, "y": 2.0, "name": "Hall", "use": "corridor"}),
    ]
    ops = [{"op": "create", "id": ids[n], "kind": k, "props": p} for n, k, p in ents]
    json.dump({"schema": 1, "ops": ops}, open(f"{out}/ops.json", "w"), indent=1)
    counts = {"IfcBuildingStorey": 2, "IfcWall": 4, "IfcSlab": 2, "IfcColumn": 1, "IfcBeam": 1, "IfcSpace": 1,
              "IfcDoor": 1, "IfcWindow": 1}
    json.dump({"ids": ids, "counts": counts,
               "elements": {n: ids[n] for n in ("L0", "L1", "W1", "W2", "W3", "W4", "D1", "N1", "S0", "S1", "C1", "B1", "SP1")}},
              open(f"{out}/expected.json", "w"), indent=1)


# ---------------------------------------------------------------- independent IFC producer
def gen_ifc(out):
    import math
    m = ifcopenshell.file(schema="IFC4")
    proj = api.root.create_entity(m, ifc_class="IfcProject", name="T5.5")
    api.unit.assign_unit(m)
    ctx = api.context.add_context(m, context_type="Model")
    body = api.context.add_context(m, context_type="Model", context_identifier="Body", target_view="MODEL_VIEW", parent=ctx)
    site = api.root.create_entity(m, ifc_class="IfcSite")
    bld = api.root.create_entity(m, ifc_class="IfcBuilding")
    api.aggregate.assign_object(m, relating_object=proj, products=[site])
    api.aggregate.assign_object(m, relating_object=site, products=[bld])

    def place(p, x, y, z, ang=0.0, zax=None, xax=None):
        M = np.eye(4)
        if zax is None:
            c, s = math.cos(ang), math.sin(ang)
            M[:3, :3] = [[c, -s, 0], [s, c, 0], [0, 0, 1]]
        else:
            xa, za = np.array(xax, float), np.array(zax, float)
            M[:3, 0], M[:3, 2], M[:3, 1] = xa, za, np.cross(za, xa)
        M[:3, 3] = [x, y, z]
        api.geometry.edit_object_placement(m, product=p, matrix=M)

    st = {}
    for n, z in (("G", 0.0), ("F1", 3.2)):
        s = api.root.create_entity(m, ifc_class="IfcBuildingStorey", name=n)
        s.Elevation = z * 1000.0
        api.aggregate.assign_object(m, relating_object=bld, products=[s])
        place(s, 0, 0, z)
        st[n] = s
    walls = {}
    for n, s, x, y, z, ang, L, h, t in (("WA", "G", 0, 0, 0, 0.0, 6.0, 3.0, 0.24),
                                         ("WB", "G", 9, 0, 0, math.radians(60), 4.0, 2.8, 0.2),
                                         ("WC", "F1", 0, 8, 3.2, 0.0, 5.0, 2.7, 0.2)):
        w = api.root.create_entity(m, ifc_class="IfcWall", name=n)
        api.spatial.assign_container(m, relating_structure=st[s], products=[w])
        api.geometry.assign_representation(m, product=w, representation=api.geometry.add_wall_representation(
            m, context=body, length=L, height=h, thickness=t, offset=-t / 2))
        place(w, x, y, z, ang)
        ls = api.material.add_material_set(m, name="ls", set_type="IfcMaterialLayerSet")
        lay = api.material.add_layer(m, layer_set=ls, material=api.material.add_material(m, name="Concrete " + n))
        api.material.edit_layer(m, layer=lay, attributes={"LayerThickness": t * 1000.0})
        api.material.assign_material(m, products=[w], type="IfcMaterialLayerSetUsage", material=ls)
        ps = api.pset.add_pset(m, product=w, name="Pset_WallCommon")
        api.pset.edit_pset(m, pset=ps, properties={"IsExternal": n != "WB", "FireRating": "EI30"})
        walls[n] = (w, ang, x, y, z, t)
    for n, host, fill, off, sill, wd, ht in (("OA", "WA", "IfcDoor", 1.0, 0.0, 0.9, 2.1),
                                             ("OB", "WB", "IfcWindow", 1.0, 0.8, 1.2, 1.0)):
        w, ang, x, y, z, t = walls[host]
        o = api.root.create_entity(m, ifc_class="IfcOpeningElement", name=n)
        api.geometry.assign_representation(m, product=o, representation=api.geometry.add_wall_representation(
            m, context=body, length=wd, height=ht, thickness=t, offset=-t / 2))
        place(o, x + off * math.cos(ang), y + off * math.sin(ang), z + sill, ang)
        api.feature.add_feature(m, feature=o, element=w)
        api.feature.add_filling(m, opening=o, element=api.root.create_entity(m, ifc_class=fill))
    for n, s, poly, d, z in (("SA", "G", [(20, 0), (26, 0), (26, 3), (20, 3)], 0.25, 0.0),
                             ("SB", "F1", [(0, 0), (6, 0), (6, 5), (3, 6.5), (0, 5)], 0.2, 3.2)):
        sl = api.root.create_entity(m, ifc_class="IfcSlab", name=n)
        api.spatial.assign_container(m, relating_structure=st[s], products=[sl])
        api.geometry.assign_representation(m, product=sl, representation=api.geometry.add_slab_representation(
            m, context=body, depth=d, polyline=poly))
        place(sl, 0, 0, z - d)
    c = api.root.create_entity(m, ifc_class="IfcColumn", name="CA")
    api.spatial.assign_container(m, relating_structure=st["G"], products=[c])
    api.geometry.assign_representation(m, product=c, representation=api.geometry.add_profile_representation(
        m, context=body, profile=m.createIfcRectangleProfileDef("AREA", None, None, 300.0, 500.0), depth=3.2))
    place(c, 3, 2, 0)
    b = api.root.create_entity(m, ifc_class="IfcBeam", name="BA")
    api.spatial.assign_container(m, relating_structure=st["F1"], products=[b])
    api.geometry.assign_representation(m, product=b, representation=api.geometry.add_profile_representation(
        m, context=body, profile=m.createIfcRectangleProfileDef("AREA", None, None, 250.0, 450.0), depth=5.0))
    place(b, 0.5, 4.0, 3.2 - 0.2 - 0.225, zax=(1, 0, 0), xax=(0, 1, 0))
    sp = api.root.create_entity(m, ifc_class="IfcSpace", name="G01")
    api.aggregate.assign_object(m, relating_object=st["G"], products=[sp])
    api.geometry.assign_representation(m, product=sp, representation=api.geometry.add_slab_representation(
        m, context=body, depth=2.75, polyline=[(0.5, 0.5), (5.5, 0.5), (5.5, 4.5), (0.5, 4.5)]))
    place(sp, 0, 0, 0)
    rng = random.Random(550)
    for r in m.by_type("IfcRoot"):
        r.GlobalId = ifcopenshell.guid.compress(format(rng.getrandbits(128), "032x"))
    m.write(f"{out}/source.ifc")


# ---------------------------------------------------------------- IFC -> ifcJSON (judge's converter)
def lower_camel(s):
    return s[0].lower() + s[1:]


def uuid_of(g):
    return str(uuid.UUID(ifcopenshell.guid.expand(g)))


def to_json_value(v):
    if v is None:
        return None
    if isinstance(v, ifcopenshell.entity_instance):
        if v.id() == 0:  # typed value in a select
            return {"type": v.is_a(), "value": to_json_value(v.wrappedValue)}
        if v.is_a("IfcRoot"):
            return {"type": v.is_a(), "ref": uuid_of(v.GlobalId)}
        return full_object(v)
    if isinstance(v, (tuple, list)):
        return [x for x in (to_json_value(i) for i in v) if x is not None]
    return v


def full_object(e):
    obj = {"type": e.is_a()}
    info = e.get_info(recursive=False, include_identifier=False)
    for k, v in info.items():
        if k == "type":
            continue
        if k == "GlobalId":
            obj["globalId"] = uuid_of(v)
            continue
        jv = to_json_value(v)
        if jv is not None and jv != "":
            obj[lower_camel(k)] = jv
    return obj


def to_ifcjson(src, dst):
    f = ifcopenshell.open(src)
    roots = [e for e in f.by_type("IfcRoot") if not e.is_a("IfcRelationship")]
    rels = [e for e in f.by_type("IfcRoot") if e.is_a("IfcRelationship")]
    data = [full_object(e) for e in roots + rels]
    doc = {"type": "ifcJSON", "version": "0.0.1", "schemaIdentifier": f.schema,
           "originatingSystem": "openbim judge T5.5", "data": data}
    json.dump(doc, open(dst, "w"), indent=1)
    # self-check: every IfcRoot appears once with its GlobalId, references resolve
    ids = {o["globalId"] for o in data}
    need(len(ids) == len(data) == len(f.by_type("IfcRoot")), "converter self-check: object count")

    def walk(o):
        if isinstance(o, dict):
            if "ref" in o:
                need(o["ref"] in ids, f"converter self-check: dangling ref {o}")
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)
    walk(data)


# ---------------------------------------------------------------- checks
def guid_forms(ulid_str):
    hexs = format(ulid_value(ulid_str), "032x")
    return {ifcopenshell.guid.compress(hexs), str(uuid.UUID(hexs))}


def check_export(out, path):
    exp = json.load(open(f"{out}/expected.json"))
    try:
        doc = json.load(open(path))
    except ValueError as e:
        raise Fail(f"ifcJSON export is not JSON: {e}")
    # T5.5 Details: {"type":"ifcJSON","version":"0.0.1","schemaIdentifier":"IFC4","data":[…]} (spec/cli.md: 0.0.x)
    need(isinstance(doc, dict), "ifcJSON export must be a JSON object with type/version/schemaIdentifier/data")
    need(doc.get("type") == "ifcJSON", f"ifcJSON export top-level 'type' is {doc.get('type')!r}, want 'ifcJSON'")
    need(isinstance(doc.get("version"), str) and re.fullmatch(r"0\.0\.\d+", doc["version"]),
         f"ifcJSON export 'version' is {doc.get('version')!r}, want 0.0.x")
    need(doc.get("schemaIdentifier") == "IFC4", f"ifcJSON export 'schemaIdentifier' is {doc.get('schemaIdentifier')!r}, want 'IFC4'")
    data = doc.get("data")
    need(isinstance(data, list) and data, "ifcJSON export has no non-empty 'data' object list")
    need(all(isinstance(o, dict) and isinstance(o.get("type"), str) for o in data), "every ifcJSON object needs a 'type'")
    by_type = {}
    for o in data:
        t = o["type"]
        t = {"IfcWallStandardCase": "IfcWall", "IfcSlabStandardCase": "IfcSlab"}.get(t, t)  # IFC4 subtypes count as their base
        by_type.setdefault(t, []).append(o)
    for cls, n in exp["counts"].items():
        got = len(by_type.get(cls, []))
        need(got == n, f"ifcJSON export has {got} {cls} objects, want {n}")
    gids = {o.get("globalId") for o in data if "globalId" in o}
    for name, u in exp["elements"].items():
        need(guid_forms(u) & gids, f"no ifcJSON object carries the GlobalId of {name} (T5.2 rule)")
    print("ok")


SETTINGS = ifcopenshell.geom.settings()
SETTINGS.set("use-world-coords", True)


def compare(sx, sy, ix, iy):
    a, b = json.load(open(sx)), json.load(open(sy))
    for key in ("walls", "slabs"):
        va = {o["id"]: o["volume"] for o in a.get(key, [])}
        vb = {o["id"]: o["volume"] for o in b.get(key, [])}
        need(va, f"importing the .ifc produced no {key}")
        need(set(va) == set(vb), f"{key}: ids from .ifc import {sorted(va)} vs ifcJSON import {sorted(vb)}")
        for k in va:
            need(close(va[k], vb[k]), f"{key} {k}: volume {va[k]} (.ifc) vs {vb[k]} (ifcJSON)")
    fx, fy = ifcopenshell.open(ix), ifcopenshell.open(iy)
    for cls in ("IfcBuildingStorey", "IfcWall", "IfcSlab", "IfcColumn", "IfcBeam", "IfcSpace",
                "IfcOpeningElement", "IfcDoor", "IfcWindow"):
        need(len(fx.by_type(cls)) == len(fy.by_type(cls)),
             f"{cls}: {len(fx.by_type(cls))} after .ifc import vs {len(fy.by_type(cls))} after ifcJSON import")
    for cls in ("IfcWall", "IfcSlab", "IfcColumn", "IfcBeam"):
        need(fx.by_type(cls), f"no {cls} after importing the .ifc")
        for el in fx.by_type(cls):
            try:
                other = fy.by_guid(el.GlobalId)
            except Exception:  # noqa: BLE001
                raise Fail(f"{cls} {el.GlobalId} from the .ifc import is missing from the ifcJSON import")
            v1 = ush.get_volume(ifcopenshell.geom.create_shape(SETTINGS, el).geometry)
            v2 = ush.get_volume(ifcopenshell.geom.create_shape(SETTINGS, other).geometry)
            need(close(v1, v2), f"{cls} {el.GlobalId}: volume {v1} (.ifc) vs {v2} (ifcJSON)")
    print("ok")


def main():
    cmd = sys.argv[1]
    try:
        if cmd == "fixture":
            fixture(sys.argv[2])
        elif cmd == "gen-ifc":
            gen_ifc(sys.argv[2])
        elif cmd == "to-ifcjson":
            to_ifcjson(sys.argv[2], sys.argv[3])
        elif cmd == "check-export":
            check_export(sys.argv[2], sys.argv[3])
        elif cmd == "compare":
            compare(*sys.argv[2:6])
        else:
            print(f"unknown command {cmd}")
            return 2
    except Fail as e:
        print(str(e))
        return 1
    return 0


sys.exit(main())
