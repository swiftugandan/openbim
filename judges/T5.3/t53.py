"""T5.3 judge helper (oracle side). Builders must not edit judges/.

  python -I -B t53.py gen <outdir>
      Authors <outdir>/source.ifc (IFC4) and <outdir>/source-2x3.ifc (IFC2X3, the same building) with
      ifcopenshell.api (independent producer; millimetre units, nested local placements) and writes
      <outdir>/expected.json / expected-2x3.json: per element its ULID id, expected kind/props and its
      ifcopenshell.geom volume.
  python -I -B t53.py check <outdir> <bim> <projectdir> <solve.json> <reexport.ifc> [<tag>]
      Inspects the imported document with `bim show <projectdir> <id> --json`; <tag> ("" or "-2x3")
      selects the source/expected pair.

Id rule (TASKS.md T5.3 Details): an imported entity's id is the 26-character ULID spelling of the
128 bits of its IFC GlobalId, and export maps it back to the same GlobalId (T5.2 rule). Property-set
values become props `<PsetName>.<PropName>`; an Opening's `kind` comes from its filling. Import reads
IFC4 and IFC2X3 (spec/cli.md); exports are IFC4.
"""
import json
import math
import random
import subprocess
import sys

import numpy as np
import shapely.geometry as sg
import ifcopenshell
import ifcopenshell.api.aggregate
import ifcopenshell.api.context
import ifcopenshell.api.feature
import ifcopenshell.api.geometry
import ifcopenshell.api.material
import ifcopenshell.api.owner
import ifcopenshell.api.owner.settings
import ifcopenshell.api.pset
import ifcopenshell.api.root
import ifcopenshell.api.spatial
import ifcopenshell.api.unit
import ifcopenshell.geom
import ifcopenshell.guid
import ifcopenshell.util.placement as upl
import ifcopenshell.util.shape as ush
from ifcopenshell import api

CROCK = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
REL = 1e-4
TOL = 1e-6  # metres, for coordinates resolved from placements
OWNER_DEFAULTS = (ifcopenshell.api.owner.settings.get_user, ifcopenshell.api.owner.settings.get_application)


def ulid_of_guid(g):
    n = int(ifcopenshell.guid.expand(g), 16)
    return "".join(CROCK[(n >> (5 * (25 - i))) & 31] for i in range(26))


SETTINGS = ifcopenshell.geom.settings()
SETTINGS.set("use-world-coords", True)


def volume(el):
    return ush.get_volume(ifcopenshell.geom.create_shape(SETTINGS, el).geometry)


class Model:
    """ifcopenshell.api authoring with explicit, nested IfcLocalPlacements in millimetres."""

    def __init__(self, schema):
        m = self.m = ifcopenshell.file(schema=schema)
        if schema == "IFC2X3":  # IFC2X3 requires an IfcOwnerHistory on every IfcRoot
            person = api.owner.add_person(m, identification="J", family_name="Judge")
            org = api.owner.add_organisation(m, identification="J", name="Judge")
            user = api.owner.add_person_and_organisation(m, person=person, organisation=org)
            app = api.owner.add_application(m)
            ifcopenshell.api.owner.settings.get_user = lambda f: user
            ifcopenshell.api.owner.settings.get_application = lambda f: app
        else:
            ifcopenshell.api.owner.settings.get_user = OWNER_DEFAULTS[0]
            ifcopenshell.api.owner.settings.get_application = OWNER_DEFAULTS[1]
        self.proj = api.root.create_entity(m, ifc_class="IfcProject", name="T5.3 source")
        api.unit.assign_unit(m)  # millimetres
        ctx = api.context.add_context(m, context_type="Model")
        self.body = api.context.add_context(m, context_type="Model", context_identifier="Body",
                                            target_view="MODEL_VIEW", parent=ctx)

    def place(self, product, rel, origin, angle=0.0, z_axis=(0.0, 0.0, 1.0), x_axis=None):
        m = self.m
        if x_axis is None:
            x_axis = (math.cos(angle), math.sin(angle), 0.0)
        a2p = m.createIfcAxis2Placement3D(
            m.createIfcCartesianPoint(tuple(float(c) * 1000.0 for c in origin)),
            m.createIfcDirection(tuple(map(float, z_axis))), m.createIfcDirection(tuple(map(float, x_axis))))
        product.ObjectPlacement = m.createIfcLocalPlacement(rel.ObjectPlacement if rel is not None else None, a2p)

    def world(self, product):
        M = upl.get_local_placement(product.ObjectPlacement)
        M = M.copy()
        M[:3, 3] /= 1000.0
        return M


def gen(out):
    gen_model(out, "IFC4", "", 53)
    gen_model(out, "IFC2X3", "-2x3", 54)


def gen_model(out, schema, tag, seed):
    md = Model(schema)
    m, body = md.m, md.body

    def rect_profile(xdim, ydim):  # explicit Position: required in IFC2X3, optional in IFC4
        return m.createIfcRectangleProfileDef("AREA", None, m.createIfcAxis2Placement2D(
            m.createIfcCartesianPoint((0.0, 0.0))), xdim, ydim)
    site = api.root.create_entity(m, ifc_class="IfcSite", name="Site")
    bld = api.root.create_entity(m, ifc_class="IfcBuilding", name="Building")
    api.aggregate.assign_object(m, relating_object=md.proj, products=[site])
    api.aggregate.assign_object(m, relating_object=site, products=[bld])
    md.place(site, None, (0, 0, 0))
    md.place(bld, site, (100.0, 50.0, 0.0))  # building offset: every coordinate below is resolved through it
    exp = {"elements": {}, "levels": {}}
    storeys = {}
    for name, elev in (("S0", 0.0), ("S1", 3.0)):
        st = api.root.create_entity(m, ifc_class="IfcBuildingStorey", name=name)
        st.Elevation = elev * 1000.0
        api.aggregate.assign_object(m, relating_object=bld, products=[st])
        md.place(st, bld, (0, 0, elev))
        storeys[name] = st
        exp["levels"][name] = {"id": ulid_of_guid(st.GlobalId), "elevation": elev}

    def layered(product, layers):
        ls = api.material.add_material_set(m, name="layers", set_type="IfcMaterialLayerSet")
        for i, t in enumerate(layers):
            lay = api.material.add_layer(m, layer_set=ls, material=api.material.add_material(m, name=f"Mat{i}"))
            api.material.edit_layer(m, layer=lay, attributes={"LayerThickness": t * 1000.0})
        api.material.assign_material(m, products=[product], type="IfcMaterialLayerSetUsage", material=ls)

    walls = {}
    for name, st, origin, angle, length, height, layers in (
        ("WA", "S0", (0.0, 0.0, 0.0), 0.0, 6.0, 3.0, [0.2]),
        ("WB", "S0", (10.0, 0.0, 0.0), math.radians(35.0), 4.0, 2.7, [0.12, 0.08, 0.05]),
        ("WC", "S1", (0.0, 8.0, 0.0), math.radians(-90.0), 5.0, 2.8, [0.1, 0.1]),
    ):
        w = api.root.create_entity(m, ifc_class="IfcWall", name=name)
        api.spatial.assign_container(m, relating_structure=storeys[st], products=[w])
        t = sum(layers)
        rep = api.geometry.add_wall_representation(m, context=body, length=length, height=height,
                                                   thickness=t, offset=-t / 2)
        api.geometry.assign_representation(m, product=w, representation=rep)
        md.place(w, storeys[st], origin, angle)
        layered(w, layers)
        walls[name] = (w, length, t, height)
    pset = api.pset.add_pset(m, product=walls["WA"][0], name="Pset_WallCommon")
    api.pset.edit_pset(m, pset=pset, properties={"FireRating": "REI60", "IsExternal": True, "ThermalTransmittance": 0.3})
    pset = api.pset.add_pset(m, product=walls["WA"][0], name="Judge_Custom")
    api.pset.edit_pset(m, pset=pset, properties={"Count": m.createIfcInteger(7), "Note": "kept"})
    exp["psets"] = {"WA": {"Pset_WallCommon.FireRating": "REI60", "Pset_WallCommon.IsExternal": True,
                           "Pset_WallCommon.ThermalTransmittance": 0.3, "Judge_Custom.Count": 7,
                           "Judge_Custom.Note": "kept"}}

    openings = {}
    for name, host, fill, offset, sill, width, height in (
        ("OA", "WA", "IfcDoor", 1.0, 0.0, 0.9, 2.1),
        ("OB", "WB", "IfcWindow", 1.5, 0.9, 1.0, 1.2),
    ):
        w, length, t, _ = walls[host]
        o = api.root.create_entity(m, ifc_class="IfcOpeningElement", name=name)
        rep = api.geometry.add_wall_representation(m, context=body, length=width, height=height, thickness=t, offset=-t / 2)
        api.geometry.assign_representation(m, product=o, representation=rep)
        md.place(o, w, (offset, 0.0, sill))  # nested: relative to the host wall's placement
        api.feature.add_feature(m, feature=o, element=w)
        f = api.root.create_entity(m, ifc_class=fill, name=name + "-fill")
        api.feature.add_filling(m, opening=o, element=f)
        openings[name] = (o, host, "door" if fill == "IfcDoor" else "window", offset, sill, width, height)

    slabs = {}
    for name, st, poly, depth in (
        ("SL0", "S0", [(20.0, 0.0), (26.0, 0.0), (26.0, 2.0), (22.0, 2.0), (22.0, 5.0), (20.0, 5.0)], 0.25),
        ("SL1", "S1", [(0.0, 0.0), (8.0, 0.0), (8.0, 6.0), (0.0, 6.0)], 0.2),
    ):
        s = api.root.create_entity(m, ifc_class="IfcSlab", name=name)
        api.spatial.assign_container(m, relating_structure=storeys[st], products=[s])
        rep = api.geometry.add_slab_representation(m, context=body, depth=depth, polyline=poly)
        api.geometry.assign_representation(m, product=s, representation=rep)
        md.place(s, storeys[st], (0.0, 0.0, -depth))  # top face at the storey elevation
        slabs[name] = (s, st, poly, depth)

    co = api.root.create_entity(m, ifc_class="IfcColumn", name="CO")
    api.spatial.assign_container(m, relating_structure=storeys["S0"], products=[co])
    prof = rect_profile(300.0, 400.0)
    api.geometry.assign_representation(m, product=co, representation=api.geometry.add_profile_representation(
        m, context=body, profile=prof, depth=3.0))
    md.place(co, storeys["S0"], (3.0, 4.0, 0.0))

    be = api.root.create_entity(m, ifc_class="IfcBeam", name="BE")
    api.spatial.assign_container(m, relating_structure=storeys["S1"], products=[be])
    prof = rect_profile(200.0, 400.0)
    api.geometry.assign_representation(m, product=be, representation=api.geometry.add_profile_representation(
        m, context=body, profile=prof, depth=7.0))
    # extrusion along world +x; profile x along world +y (width 0.2), profile y up (depth 0.4); centred
    md.place(be, storeys["S1"], (0.5, 3.0, -0.2 - 0.2), z_axis=(1.0, 0.0, 0.0), x_axis=(0.0, 1.0, 0.0))

    sp = api.root.create_entity(m, ifc_class="IfcSpace", name="101")
    sp.LongName = "Office"
    api.aggregate.assign_object(m, relating_object=storeys["S0"], products=[sp])
    api.geometry.assign_representation(m, product=sp, representation=api.geometry.add_slab_representation(
        m, context=body, depth=2.75, polyline=[(0.5, 0.5), (5.5, 0.5), (5.5, 5.5), (0.5, 5.5)]))
    md.place(sp, storeys["S0"], (0.0, 0.0, 0.0))

    # Non-extruded element: a wedge as a polygonal face set (must survive as faceted geometry).
    wf = api.root.create_entity(m, ifc_class="IfcWall", name="WF")
    api.spatial.assign_container(m, relating_structure=storeys["S0"], products=[wf])
    v = [(0, 0, 0), (2, 0, 0), (2, 1, 0), (0, 1, 0), (0, 0, 1.5), (0, 1, 1.5)]
    faces = [[0, 3, 2, 1], [0, 1, 4], [3, 5, 2], [1, 2, 5, 4], [0, 4, 5, 3]]
    rep = api.geometry.add_mesh_representation(m, context=body, vertices=[[tuple(c) for c in v]], faces=[faces])
    api.geometry.assign_representation(m, product=wf, representation=rep)
    md.place(wf, storeys["S0"], (14.0, 6.0, 0.0))

    rng = random.Random(seed)  # fixed GlobalIds: deterministic fixtures
    for r in m.by_type("IfcRoot"):
        r.GlobalId = ifcopenshell.guid.compress(format(rng.getrandbits(128), "032x"))
    m.write(f"{out}/source{tag}.ifc")
    # Re-open: expectations come from the written file as ifcopenshell reads it.
    f = ifcopenshell.open(f"{out}/source{tag}.ifc")
    assert f.schema == schema, f.schema
    W = lambda e: Model.world(None, f.by_guid(e.GlobalId))  # noqa: E731
    E = exp["elements"]
    for name, (w, length, t, height) in walls.items():
        M = W(w)
        p1 = M[:3, 3]
        p2 = p1 + M[:3, 0] * length
        lvl = "S1" if name == "WC" else "S0"
        E[name] = {"guid": w.GlobalId, "kind": "Wall", "volume": volume(f.by_guid(w.GlobalId)),
                   "level": lvl, "p1": p1[:2].tolist(), "p2": p2[:2].tolist(), "length": length, "height": height,
                   "thickness": t, "base_z": float(p1[2])}
    for name, (o, host, kind, offset, sill, width, height) in openings.items():
        E[name] = {"guid": o.GlobalId, "kind": "Opening", "host": host, "opening_kind": kind, "offset": offset,
                   "sill": sill, "width": width, "height": height}
    for name, (s, st, poly, depth) in slabs.items():
        M = W(s)
        world = [(M @ np.array([x, y, 0.0, 1.0]))[:2].tolist() for x, y in poly]
        E[name] = {"guid": s.GlobalId, "kind": "Slab", "level": st, "boundary": world, "thickness": depth,
                   "volume": volume(f.by_guid(s.GlobalId))}
    M = W(co)
    E["CO"] = {"guid": co.GlobalId, "kind": "Column", "xy": M[:2, 3].tolist(), "dims": [0.3, 0.4], "base": "S0",
               "top": "S1", "volume": volume(f.by_guid(co.GlobalId))}
    M = W(be)
    a = M[:3, 3]
    b = a + M[:3, 2] * 7.0
    E["BE"] = {"guid": be.GlobalId, "kind": "Beam", "level": "S1", "p1": a[:2].tolist(), "p2": b[:2].tolist(),
               "section_b": 0.2, "section_h": 0.4, "top_z": float(a[2] + 0.2), "volume": volume(f.by_guid(be.GlobalId))}
    M = W(sp)
    fp = [(M @ np.array([x, y, 0.0, 1.0]))[:2].tolist() for x, y in [(0.5, 0.5), (5.5, 0.5), (5.5, 5.5), (0.5, 5.5)]]
    E["SP"] = {"guid": sp.GlobalId, "kind": "Space", "level": "S0", "footprint": fp, "names": ["101", "Office"]}
    E["WF"] = {"guid": wf.GlobalId, "kind": None, "volume": volume(f.by_guid(wf.GlobalId)), "closed_form": 2 * 1 * 1.5 / 2}
    for e in E.values():
        e["id"] = ulid_of_guid(e["guid"])
    # oracle self-checks against closed forms
    assert abs(E["WF"]["volume"] - E["WF"]["closed_form"]) < 1e-9, E["WF"]
    assert abs(E["CO"]["volume"] - 0.3 * 0.4 * 3.0) < 1e-9
    assert abs(E["BE"]["volume"] - 0.2 * 0.4 * 7.0) < 1e-9
    assert abs(E["WA"]["volume"] - (6.0 * 3.0 - 0.9 * 2.1) * 0.2) < 1e-9, E["WA"]["volume"]
    assert abs(E["WB"]["volume"] - (4.0 * 2.7 - 1.0 * 1.2) * 0.25) < 1e-9, E["WB"]["volume"]
    assert abs(E["SL0"]["volume"] - 18.0 * 0.25) < 1e-9
    assert np.allclose(E["WA"]["p1"], [100.0, 50.0]) and abs(E["SL1"]["boundary"][2][1] - 56.0) < 1e-9
    json.dump(exp, open(f"{out}/expected{tag}.json", "w"), indent=1)


class Fail(Exception):
    pass


def need(c, msg):
    if not c:
        raise Fail(msg)


def close(a, b, rel=REL, absol=TOL):
    return abs(a - b) <= max(rel * max(abs(a), abs(b)), absol)


def check(out, bim, pdir, solve_path, reexport, tag=""):
    exp = json.load(open(f"{out}/expected{tag}.json"))
    E, L = exp["elements"], exp["levels"]

    def show(eid, what):
        try:
            r = subprocess.run([bim, "show", pdir, eid, "--json"], capture_output=True, text=True, timeout=30)
        except subprocess.TimeoutExpired:
            raise Fail(f"`bim show {eid}` timed out")
        need(r.returncode == 0, f"`bim show` failed for {what} (id {eid}, exit {r.returncode}): {r.stderr[:300]}")
        try:
            d = json.loads(r.stdout)
        except ValueError as e:
            raise Fail(f"`bim show {eid} --json` printed invalid JSON: {e}")
        need(isinstance(d, dict) and "props" in d and "kind" in d, f"`bim show {eid}` lacks kind/props: {d}")
        return d

    def num(props, key, what):
        v = props.get(key)
        need(isinstance(v, (int, float)) and not isinstance(v, bool), f"{what}.{key} = {v!r}, want a number")
        return float(v)

    def ref(props, key, want_id, what):
        v = props.get(key)
        need(isinstance(v, dict) and v.get("ref") == want_id, f"{what}.{key} = {v!r}, want {{'ref': {want_id!r}}}")

    for lname, lv in L.items():
        d = show(lv["id"], f"storey {lname}")
        need(d["kind"] == "Level", f"storey {lname} imported as {d['kind']}")
        need(close(num(d["props"], "elevation", lname), lv["elevation"], 0), f"Level {lname} elevation wrong: {d['props']}")

    walls_dir = {}
    for name in ("WA", "WB", "WC"):
        e = E[name]
        d = show(e["id"], f"wall {name}")
        p = d["props"]
        need(d["kind"] == "Wall", f"{name}: imported as {d['kind']}, want Wall")
        ref(p, "level", L[e["level"]]["id"], name)
        got1 = (num(p, "x1", name), num(p, "y1", name))
        got2 = (num(p, "x2", name), num(p, "y2", name))
        fwd = np.allclose(got1, e["p1"], atol=TOL) and np.allclose(got2, e["p2"], atol=TOL)
        rev = np.allclose(got1, e["p2"], atol=TOL) and np.allclose(got2, e["p1"], atol=TOL)
        need(fwd or rev, f"{name}: centreline {got1}->{got2}, want {e['p1']}->{e['p2']} (placements resolved)")
        walls_dir[name] = fwd
        need(close(num(p, "height", name), e["height"], 0), f"{name}: height {p.get('height')}, want {e['height']}")
        z0 = L[e["level"]]["elevation"] + float(p.get("base_offset", 0.0) or 0.0)
        need(close(z0, e["base_z"], 0), f"{name}: base at {z0}, want {e['base_z']}")
        need(isinstance(p.get("type"), dict) and "ref" in p["type"], f"{name}: no WallType ref: {p.get('type')}")
        t = show(p["type"]["ref"], f"WallType of {name}")
        need(t["kind"] == "WallType", f"{name}: type ref points at {t['kind']}")
        lt = t["props"].get("layer_thickness")
        need(isinstance(lt, list) and close(sum(lt), e["thickness"], 0), f"{name}: layer_thickness {lt}, sum must be {e['thickness']}")

    for name in ("OA", "OB"):
        e = E[name]
        d = show(e["id"], f"opening {name}")
        p = d["props"]
        need(d["kind"] == "Opening", f"{name}: imported as {d['kind']}, want Opening")
        ref(p, "host", E[e["host"]]["id"], name)
        need(p.get("kind") == e["opening_kind"], f"{name}: kind {p.get('kind')!r}, want {e['opening_kind']!r}")
        for k in ("sill", "width", "height"):
            need(close(num(p, k, name), e[k], 0), f"{name}: {k} {p.get(k)}, want {e[k]}")
        want_off = e["offset"] if walls_dir[e["host"]] else E[e["host"]]["length"] - e["offset"] - e["width"]
        need(close(num(p, "offset", name), want_off, 0), f"{name}: offset {p.get('offset')}, want {want_off}")

    for name in ("SL0", "SL1"):
        e = E[name]
        d = show(e["id"], f"slab {name}")
        p = d["props"]
        need(d["kind"] == "Slab", f"{name}: imported as {d['kind']}, want Slab")
        ref(p, "level", L[e["level"]]["id"], name)
        need(close(num(p, "thickness", name), e["thickness"], 0), f"{name}: thickness {p.get('thickness')}")
        b = p.get("boundary")
        need(isinstance(b, list) and len(b) >= 6 and len(b) % 2 == 0, f"{name}: boundary {b!r}")
        got = sg.Polygon(list(zip(b[::2], b[1::2])))
        want = sg.Polygon(e["boundary"])
        need(got.is_valid and got.exterior.is_ccw, f"{name}: boundary is not a valid counter-clockwise polygon")
        need(got.symmetric_difference(want).area < 1e-9, f"{name}: boundary {b} differs from {e['boundary']}")

    e = E["CO"]
    d = show(e["id"], "column CO")
    p = d["props"]
    need(d["kind"] == "Column", f"CO: imported as {d['kind']}")
    need(np.allclose([num(p, "x", "CO"), num(p, "y", "CO")], e["xy"], atol=TOL), f"CO: x,y {p.get('x')},{p.get('y')}, want {e['xy']}")
    need(np.allclose(sorted([num(p, "section_b", "CO"), num(p, "section_h", "CO")]), sorted(e["dims"]), atol=1e-9),
         f"CO: section {p.get('section_b')}x{p.get('section_h')}, want 0.3x0.4")
    ref(p, "base", L[e["base"]]["id"], "CO")
    ref(p, "top", L[e["top"]]["id"], "CO")

    e = E["BE"]
    d = show(e["id"], "beam BE")
    p = d["props"]
    need(d["kind"] == "Beam", f"BE: imported as {d['kind']}")
    got1 = (num(p, "x1", "BE"), num(p, "y1", "BE"))
    got2 = (num(p, "x2", "BE"), num(p, "y2", "BE"))
    need((np.allclose(got1, e["p1"], atol=TOL) and np.allclose(got2, e["p2"], atol=TOL)) or
         (np.allclose(got1, e["p2"], atol=TOL) and np.allclose(got2, e["p1"], atol=TOL)),
         f"BE: axis {got1}->{got2}, want {e['p1']}->{e['p2']}")
    need(close(num(p, "section_b", "BE"), e["section_b"], 0) and close(num(p, "section_h", "BE"), e["section_h"], 0),
         f"BE: section {p.get('section_b')}x{p.get('section_h')}, want 0.2x0.4")
    ref(p, "level", L[e["level"]]["id"], "BE")
    top = L[e["level"]]["elevation"] - float(p.get("z_offset", 0.0) or 0.0)
    need(close(top, e["top_z"], 0), f"BE: top at {top}, want {e['top_z']}")

    e = E["SP"]
    d = show(e["id"], "space SP")
    p = d["props"]
    need(d["kind"] == "Space", f"SP: imported as {d['kind']}")
    ref(p, "level", L[e["level"]]["id"], "SP")
    need(sg.Polygon(e["footprint"]).contains(sg.Point(num(p, "x", "SP"), num(p, "y", "SP"))),
         f"SP: label point ({p.get('x')},{p.get('y')}) not inside the IfcSpace footprint")
    need(p.get("name") in e["names"], f"SP: name {p.get('name')!r}, want one of {e['names']}")

    e = E["WF"]
    d = show(e["id"], "faceted element WF")
    need(d["kind"] not in ("Wall", "Slab", "Column", "Beam"),
         f"WF (non-extruded IfcWall) became parametric {d['kind']}; must be kept as faceted geometry")

    wp = show(E["WA"]["id"], "wall WA")["props"]
    for k, v in exp["psets"]["WA"].items():
        got = wp.get(k)
        if isinstance(v, bool) or isinstance(v, str):
            ok = got is v if isinstance(v, bool) else got == v
        else:
            ok = isinstance(got, (int, float)) and not isinstance(got, bool) and close(float(got), float(v), 0, 1e-12)
        need(ok, f"WA: prop {k} = {got!r}, want {v!r} (property sets kept as props)")

    # Counts and volumes through the engines: `bim solve --trade arch`.
    solve = json.load(open(solve_path))
    sw = {w["id"]: w for w in solve.get("walls", [])}
    ss = {s["id"]: s for s in solve.get("slabs", [])}
    need(set(sw) == {E[n]["id"] for n in ("WA", "WB", "WC")},
         f"solve walls {sorted(sw)} != imported walls WA, WB, WC")
    need(set(ss) == {E[n]["id"] for n in ("SL0", "SL1")}, f"solve slabs {sorted(ss)} != SL0, SL1")
    for n in ("WA", "WB", "WC"):
        need(close(sw[E[n]["id"]]["volume"], E[n]["volume"]), f"{n}: bim volume {sw[E[n]['id']]['volume']}, ifcopenshell {E[n]['volume']}")
    for n in ("SL0", "SL1"):
        need(close(ss[E[n]["id"]]["volume"], E[n]["volume"]), f"{n}: bim volume {ss[E[n]['id']]['volume']}, ifcopenshell {E[n]['volume']}")

    # import -> export -> ifcopenshell: counts by class and per-element volumes conserved.
    try:
        g = ifcopenshell.open(reexport)
    except Exception as ex:  # noqa: BLE001
        raise Fail(f"re-exported IFC unreadable: {ex}")
    need(g.schema == "IFC4", f"re-export schema is {g.schema}, want IFC4 (spec/cli.md: exports are IFC4)")
    src = ifcopenshell.open(f"{out}/source{tag}.ifc")
    for cls in ("IfcBuildingStorey", "IfcSlab", "IfcColumn", "IfcBeam", "IfcSpace", "IfcDoor", "IfcWindow"):
        need(len(g.by_type(cls)) == len(src.by_type(cls)), f"re-export has {len(g.by_type(cls))} {cls}, source {len(src.by_type(cls))}")
    need(len(g.by_type("IfcRelVoidsElement")) >= 2, "re-export lost the wall openings")
    for name, e in E.items():
        if "volume" not in e:
            continue
        try:
            el = g.by_guid(e["guid"])
        except Exception:  # noqa: BLE001
            raise Fail(f"{name}: GlobalId {e['guid']} not preserved by import -> export")
        try:
            v = volume(el)
        except Exception as ex:  # noqa: BLE001
            raise Fail(f"{name}: re-exported geometry unreadable: {ex}")
        need(close(v, e["volume"]), f"{name}: volume after import->export {v}, source {e['volume']}")
    print("ok")


def main():
    if sys.argv[1] == "gen":
        gen(sys.argv[2])
        return 0
    try:
        check(*sys.argv[2:8])
    except Fail as e:
        print(str(e))
        return 1
    return 0


sys.exit(main())
