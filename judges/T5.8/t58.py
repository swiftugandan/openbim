"""External judge for T5.8 (IFC4 export of catalogue products, furniture, roofs, finishes, extension kinds).

Drives the real `bim` binary (argv[1]) in a scratch dir (argv[2]): builds a project whose Products come from a
catalogue through `bim catalog use` (spec/catalogs.md), with a trade package `plugins/fire/` holding only
trade.json (spec/plugins.md: a missing engine file matters only when the engine is invoked), exports IFC and reads
it back with ifcopenshell:
  * schema IFC4, `ifcopenshell.validate` zero errors, unique GlobalIds (as T5.2);
  * one type object per Product (Tag = Product id, Name = product name) of the class / PredefinedType of TASKS.md
    T5.8, Pset_ManufacturerTypeInformation (Manufacturer, ModelReference = sku, ModelLabel = name), property set
    `openbim_Catalogue` with the product's own props, IfcRelDefinesByType to exactly its occurrences;
  * IfcFurniture / IfcRoof per entity (GlobalId rule of T5.2, Name = `_name`, storey containment), volumes and
    world bounding boxes from ifcopenshell.geom equal to the closed forms of spec/schema.md;
  * one IfcCovering per finish reference, related to its element by IfcRelCoversBldgElements and typed by the
    finish product; the IfcWall voided by a product Opening has the volume of the effective width/height cut;
  * extension entities as IfcBuildingElementProxy (ObjectType = kind, property set `<package>_<Kind>` with its
    Num/Text/Bool props, storey containment when it has a `level`);
  * every property set whose name starts with `Pset_` is a standard IFC4 property set (ifcopenshell's
    buildingSMART templates), so no custom set uses the reserved prefix.
Usage: .oracles/py/bin/python -I -B judges/T5.8/t58.py <bim> <scratch-dir>
"""
import json
import math
import os
import random
import subprocess
import sys

import numpy as np
import ifcopenshell
import ifcopenshell.geom
import ifcopenshell.guid
import ifcopenshell.util.element as uel
import ifcopenshell.util.pset as upset
import ifcopenshell.util.shape as ush
import ifcopenshell.util.unit as uunit
import ifcopenshell.validate

BIM, TMP = sys.argv[1], sys.argv[2]
REL = 1e-4
CROCK = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def need(cond, msg):
    if not cond:
        fail(msg)


def close(a, b, rel=REL, absol=1e-9):
    return abs(a - b) <= max(rel * max(abs(a), abs(b)), absol)


def run(args, want=0):
    try:
        p = subprocess.run([BIM, *args], capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        fail(f"bim {' '.join(args)} timed out (60 s)")
    if p.returncode != want:
        fail(f"bim {' '.join(args)} exited {p.returncode}, want {want}: {p.stderr.strip()[-400:]}")
    return p.stdout


def ulid(n):
    return "".join(CROCK[(n >> (5 * (25 - i))) & 31] for i in range(26))


def guid_of(u):
    n = 0
    for ch in u:
        n = n * 32 + CROCK.index(ch)
    return ifcopenshell.guid.compress(format(n, "032x"))


def shoelace(pts):
    return 0.5 * sum(pts[i][0] * pts[(i + 1) % len(pts)][1] - pts[(i + 1) % len(pts)][0] * pts[i][1]
                     for i in range(len(pts)))


# ---------------------------------------------------------------- fixture

CATALOG = {"catalog": "acme", "version": "2026.1", "manufacturer": "Acme Building Products Ltd", "products": [
    {"sku": "FD30-926", "category": "door", "name": "Oak fire door 926 x 2040",
     "props": {"width": 0.926, "height": 2.04, "fire_rating": "FD30", "u_value": 1.8, "self_closing": True}},
    {"sku": "CW-1210", "category": "window", "name": "Casement window 1210 x 1050",
     "props": {"width": 1.21, "height": 1.05, "u_value": 1.2, "glazing": "double"}},
    {"sku": "SOFA-3", "category": "furniture", "name": "Three-seat sofa",
     "props": {"size": [2.1, 0.9, 0.85], "colour": "slate"}},
    {"sku": "ARM-1", "category": "furniture", "name": "Armchair", "props": {"size": [0.8, 0.85, 0.9], "seats": 1}},
    {"sku": "EM-WHT", "category": "wall_finish", "name": "Matt emulsion, white", "props": {"colour": "white", "coats": 2}},
    {"sku": "EM-GRY", "category": "wall_finish", "name": "Matt emulsion, grey", "props": {"colour": "grey", "coats": 3}},
    {"sku": "OAK-14", "category": "floor_finish", "name": "Engineered oak 14 mm", "props": {"thickness": 0.014}},
    {"sku": "CL-PLN", "category": "ceiling_finish", "name": "Plain plaster ceiling", "manufacturer": "Plasterco",
     "props": {"colour": "white", "fire_rated": False}},
    {"sku": "SL-NAT", "category": "roof_covering", "name": "Natural slate", "props": {"mass_per_area": 25}},
    {"sku": "WC-01", "category": "sanitary", "name": "Wall-hung WC", "props": {"flush_volume": 4.5}},
]}
TYPE_CLASS = {"door": ("IfcDoorType", None), "window": ("IfcWindowType", None), "furniture": ("IfcFurnitureType", None),
              "wall_finish": ("IfcCoveringType", "CLADDING"), "floor_finish": ("IfcCoveringType", "FLOORING"),
              "ceiling_finish": ("IfcCoveringType", "CEILING"), "roof_covering": ("IfcCoveringType", "ROOFING")}
TRADE = {"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0",
         "kinds": {
             "fire:Sprinkler": {"props": {"level": "ref", "x": "num", "y": "num", "z": "num", "coverage_radius": "num",
                                          "head": "text", "concealed": "bool"},
                                "geometry": {"box": {"centre": ["x", "y", "z"], "size": [0.1, 0.1, 0.1]}}},
             "fire:Pump": {"props": {"duty": "num", "model": "text", "standby": "bool"}}},
         "engine": "fire.wasm"}   # the component file is deliberately absent: export never invokes the engine

LEV = {"L0": 0.0, "L1": 3.2}
T = 0.2
ROOFS = {"R1": dict(level="L1", boundary=[(10, 0), (16, 0), (17, 3), (13, 5), (10, 3)], slope=0.35, angle=0.6,
                    thickness=0.22, base=0.4, covering="acme:SL-NAT"),
         "R2": dict(level="L1", boundary=[(20, 0), (24, 0), (22, 3)], slope=0.0, angle=0.0, thickness=0.15)}
FURN = {"F1": dict(level="L1", x=5.0, y=10.0, angle=math.pi / 6, product="acme:SOFA-3", z_offset=0.4),
        "F2": dict(level="L0", x=10.0, y=10.0, angle=0.0, product="acme:SOFA-3", size=[1.0, 0.5, 0.4]),
        "F3": dict(level="L0", x=14.0, y=10.0, angle=-1.0, product="acme:ARM-1")}
SPRINKLERS = {"SPR1": dict(level="L0", x=2.0, y=2.0, z=2.9, coverage_radius=2.5, head="pendant", concealed=True),
              "SPR2": dict(level="L1", x=3.0, y=3.0, z=6.0, coverage_radius=3.0, head="upright", concealed=False)}
PUMP = {"duty": 12.5, "model": "FP-200", "standby": True}
FINISHES = {  # element -> finish products (one IfcCovering each)
    "W1": ["acme:EM-WHT", "acme:EM-WHT"], "W2": ["acme:EM-WHT", "acme:EM-GRY"],
    "S1": ["acme:OAK-14", "acme:CL-PLN"], "R1": ["acme:SL-NAT"]}


def fixture():
    rng = random.Random(58)
    names = ["L0", "L1", "WT", "W1", "W2", "D1", "N1", "S1", "R1", "R2", "F1", "F2", "F3", "SPR1", "SPR2", "PUMP"]
    ids = {n: ulid(rng.getrandbits(128)) for n in names}
    R = lambda n: {"ref": ids[n]}       # noqa: E731
    P = lambda pid: {"ref": pid}        # noqa: E731  (Product ids are the `<catalog>:<sku>` seeds)
    ops = []

    def create(n, kind, props):
        ops.append({"op": "create", "id": ids[n], "kind": kind, "props": props})
    for k, v in LEV.items():
        create(k, "Level", {"elevation": v})
    create("WT", "WallType", {"layer_thickness": [T], "layer_material": ["Concrete"], "layer_conductivity": [2.0]})
    create("W1", "Wall", {"level": R("L0"), "x1": 0.0, "y1": 0.0, "x2": 6.0, "y2": 0.0, "height": 3.0, "type": R("WT"),
                          "finish_left": P("acme:EM-WHT"), "finish_right": P("acme:EM-WHT")})
    create("W2", "Wall", {"level": R("L0"), "x1": 0.0, "y1": 5.0, "x2": 4.0, "y2": 8.0, "height": 2.7, "type": R("WT"),
                          "finish_left": P("acme:EM-WHT"), "finish_right": P("acme:EM-GRY")})
    create("D1", "Opening", {"host": R("W1"), "kind": "door", "offset": 0.5, "sill": 0.0, "product": P("acme:FD30-926")})
    create("N1", "Opening", {"host": R("W1"), "kind": "window", "offset": 3.0, "sill": 0.9, "width": 0.8,
                             "product": P("acme:CW-1210")})
    create("S1", "Slab", {"level": R("L1"), "boundary": [0.0, 0.0, 8.0, 0.0, 8.0, 6.0, 0.0, 6.0], "thickness": 0.2,
                          "finish_top": P("acme:OAK-14"), "finish_bottom": P("acme:CL-PLN")})
    for k, r in ROOFS.items():
        props = {"level": R(r["level"]), "boundary": [c for p in r["boundary"] for c in p], "slope": r["slope"],
                 "angle": r["angle"], "thickness": r["thickness"]}
        if "base" in r:
            props["base"] = r["base"]
        if "covering" in r:
            props["covering"] = P(r["covering"])
        create(k, "Roof", props)
    for k, f in FURN.items():
        props = {"level": R(f["level"]), "x": f["x"], "y": f["y"], "angle": f["angle"], "product": P(f["product"])}
        for opt in ("z_offset", "size"):
            if opt in f:
                props[opt] = f[opt]
        create(k, "Furniture", props)
    for k, s in SPRINKLERS.items():
        create(k, "fire:Sprinkler", dict(s, level=R(s["level"])))
    create("PUMP", "fire:Pump", dict(PUMP))
    return ids, ops


def build():
    d = os.path.join(TMP, "p")
    run(["init", d])
    os.makedirs(os.path.join(d, "catalogs", "acme"))
    json.dump(CATALOG, open(os.path.join(d, "catalogs", "acme", "catalog.json"), "w"), indent=1)
    os.makedirs(os.path.join(d, "plugins", "fire"))
    json.dump(TRADE, open(os.path.join(d, "plugins", "fire", "trade.json"), "w"), indent=1)
    for p in CATALOG["products"]:
        pid = f"acme:{p['sku']}"
        out = (run(["catalog", "use", d, pid]).strip().splitlines() or [""])[-1].strip()
        need(out == pid, f"bim catalog use {pid} printed {out!r}")
    ids, ops = fixture()
    ops_path = os.path.join(TMP, "ops.json")
    json.dump({"schema": 1, "ops": ops}, open(ops_path, "w"))
    run(["apply", d, ops_path])
    ifc = os.path.join(TMP, "out.ifc")
    run(["export", d, "--format", "ifc", "--out", ifc])
    need(os.path.exists(ifc) and os.path.getsize(ifc) > 0, "bim export --format ifc wrote no file")
    return ids, ifc


# ---------------------------------------------------------------- readers

SETTINGS = ifcopenshell.geom.settings()
SETTINGS.set("use-world-coords", True)


def shape(el, scale):
    try:
        sh = ifcopenshell.geom.create_shape(SETTINGS, el)
    except Exception as e:  # noqa: BLE001
        fail(f"ifcopenshell.geom cannot build {el.is_a()} {el.GlobalId}: {e}")
    verts = np.array(sh.geometry.verts).reshape(-1, 3)
    # create_shape returns SI metres; the unit scale is used only for attribute values
    return ush.get_volume(sh.geometry), verts.min(0), verts.max(0)


def body_items(el):
    need(el.Representation is not None, f"{el.is_a()} {el.GlobalId} has no representation")
    items = []
    for rep in el.Representation.Representations:
        if rep.RepresentationIdentifier == "Body":
            stack = list(rep.Items)
            while stack:
                it = stack.pop()
                if it.is_a("IfcMappedItem"):
                    stack.extend(it.MappingSource.MappedRepresentation.Items)
                else:
                    items.append(it)
    need(items, f"{el.is_a()} {el.GlobalId} has no Body representation items")
    for it in items:
        need(it.is_a("IfcExtrudedAreaSolid") or it.is_a("IfcFacetedBrep"),
             f"{el.is_a()} {el.GlobalId}: body item {it.is_a()} (spec/cli.md: IfcExtrudedAreaSolid / IfcFacetedBrep)")


def by_guid(f, guid, what):
    try:
        return f.by_guid(guid)
    except Exception:  # noqa: BLE001
        fail(f"no IFC object with GlobalId {guid} (expected for {what}; GlobalId rule of TASKS.md T5.2)")


def storey_of(el):
    return uel.get_container(el)


def psets(el):
    return {k: {kk: vv for kk, vv in v.items() if kk != "id"} for k, v in uel.get_psets(el).items()}


def same(got, want, what):
    if isinstance(want, bool):
        need(isinstance(got, bool) and got == want,
             f"{what} = {got!r}, want {want!r}")
    elif isinstance(want, (int, float)):
        need(isinstance(got, (int, float)) and not isinstance(got, bool) and close(got, want, 1e-9),
             f"{what} = {got!r}, want {want!r}")
    elif isinstance(want, list):
        need(isinstance(got, (list, tuple)) and len(got) == len(want)
             and all(isinstance(g, (int, float)) and close(g, w, 1e-9) for g, w in zip(got, want)),
             f"{what} = {got!r}, want {want!r}")
    else:
        need(got == want, f"{what} = {got!r}, want {want!r}")


# ---------------------------------------------------------------- checks

def check(ids, path):
    G = {n: guid_of(u) for n, u in ids.items()}
    try:
        f = ifcopenshell.open(path)
    except Exception as e:  # noqa: BLE001
        fail(f"ifcopenshell cannot open the export: {e}")
    need(f.schema == "IFC4", f"schema is {f.schema}, want IFC4")
    log = ifcopenshell.validate.json_logger()
    ifcopenshell.validate.validate(f, log)
    errs = [s for s in log.statements if str(s.get("level", "")).lower() in ("error", "critical")]
    need(not errs, f"ifcopenshell.validate reports {len(errs)} errors, first: {errs[:1]}")
    gids = [e.GlobalId for e in f.by_type("IfcRoot")]
    need(len(gids) == len(set(gids)), "duplicate GlobalIds")
    scale = uunit.calculate_unit_scale(f)

    # --- reserved prefix: every Pset_* set is a standard IFC4 property set
    tmpl = upset.get_template("IFC4")
    for ps in f.by_type("IfcPropertySet"):
        if (ps.Name or "").startswith("Pset_"):
            need(tmpl.get_by_name(ps.Name) is not None,
                 f"property set {ps.Name!r} uses the reserved Pset_ prefix but is not a standard IFC4 property set")

    # --- type objects, one per Product
    types = {}
    for t in f.by_type("IfcTypeProduct"):  # `Tag` is an IfcTypeProduct attribute
        if t.Tag is not None and t.Tag.startswith("acme:"):
            need(t.Tag not in types, f"two type objects carry Tag {t.Tag!r}")
            types[t.Tag] = t
    want_tags = sorted(f"acme:{p['sku']}" for p in CATALOG["products"])
    need(sorted(types) == want_tags, f"type objects with Tags {sorted(types)}, want one per Product {want_tags}")
    for p in CATALOG["products"]:
        pid = f"acme:{p['sku']}"
        t = types[pid]
        cls, pre = TYPE_CLASS.get(p["category"], ("IfcBuildingElementProxyType", None))
        need(t.is_a() == cls, f"{pid} ({p['category']}): type object is {t.is_a()}, want {cls}")
        if pre is not None:
            need(t.PredefinedType == pre, f"{pid}: PredefinedType {t.PredefinedType}, want {pre}")
        need(t.Name == p["name"], f"{pid}: type Name {t.Name!r}, want {p['name']!r}")
        ps = psets(t)
        mti = ps.get("Pset_ManufacturerTypeInformation")
        need(mti is not None, f"{pid}: no Pset_ManufacturerTypeInformation (has {sorted(ps)})")
        for k, v in (("Manufacturer", p.get("manufacturer", CATALOG["manufacturer"])), ("ModelReference", p["sku"]),
                     ("ModelLabel", p["name"])):
            need(mti.get(k) == v, f"{pid}: Pset_ManufacturerTypeInformation.{k} = {mti.get(k)!r}, want {v!r}")
        cat = ps.get("openbim_Catalogue")
        need(cat is not None, f"{pid}: no property set openbim_Catalogue (has {sorted(ps)})")
        for k, v in p["props"].items():
            need(k in cat, f"{pid}: openbim_Catalogue lacks the product prop {k!r} (has {sorted(cat)})")
            same(cat[k], v, f"{pid}: openbim_Catalogue.{k}")

    def occurrences(pid):
        return {o.id() for o in uel.get_types(types[pid])}

    # --- openings: fillings typed by the door/window products; effective width/height cut the wall
    fills = {}
    for name, (host, cls, pset, pid) in {"D1": ("W1", "IfcDoor", "Pset_DoorCommon", "acme:FD30-926"),
                                         "N1": ("W1", "IfcWindow", "Pset_WindowCommon", "acme:CW-1210")}.items():
        op = by_guid(f, G[name], f"Opening {name}")
        need(op.is_a("IfcOpeningElement"), f"{name}: exported as {op.is_a()}")
        fl = [r.RelatedBuildingElement for r in op.HasFillings]
        need(len(fl) == 1 and fl[0].is_a(cls), f"{name}: fillings {[x.is_a() for x in fl]}, want one {cls}")
        need(pset in psets(fl[0]), f"{name}: {cls} lacks {pset} (T5.2 behaviour unchanged)")
        fills[pid] = fl[0]
    for pid, fl in fills.items():
        need(occurrences(pid) == {fl.id()}, f"{pid}: IfcRelDefinesByType relates "
             f"{sorted(o.is_a() + ' ' + str(o.GlobalId) for o in uel.get_types(types[pid]))}, want only the filling of its Opening")
    w1 = by_guid(f, G["W1"], "Wall W1")
    vol = shape(w1, scale)[0]
    want = T * (6.0 * 3.0 - 0.926 * 2.04 - 0.8 * 1.05)
    need(close(vol, want), f"W1: ifcopenshell volume {vol}, want {want} (openings cut at their effective width/height)")

    # --- furniture
    furn_occ = {}
    for name, fu in FURN.items():
        el = by_guid(f, G[name], f"Furniture {name}")
        need(el.is_a() == "IfcFurniture", f"{name}: exported as {el.is_a()}, want IfcFurniture")
        need(el.Name == ids[name], f"{name}: Name {el.Name!r}, want its _name {ids[name]!r}")
        st = storey_of(el)
        need(st is not None and st.GlobalId == G[fu["level"]], f"{name}: not contained in storey {fu['level']}")
        body_items(el)
        size = fu.get("size") or [p for p in CATALOG["products"] if f"acme:{p['sku']}" == fu["product"]][0]["props"]["size"]
        w, dpt, h = size
        v, lo, hi = shape(el, scale)
        need(close(v, w * dpt * h), f"{name}: ifcopenshell volume {v}, want {w * dpt * h} (effective size)")
        c, s = math.cos(fu["angle"]), math.sin(fu["angle"])
        ex, ey = abs(c) * w / 2 + abs(s) * dpt / 2, abs(s) * w / 2 + abs(c) * dpt / 2
        z0 = LEV[fu["level"]] + fu.get("z_offset", 0.0)
        want_lo, want_hi = [fu["x"] - ex, fu["y"] - ey, z0], [fu["x"] + ex, fu["y"] + ey, z0 + h]
        need(np.allclose(lo, want_lo, atol=1e-6) and np.allclose(hi, want_hi, atol=1e-6),
             f"{name}: world bbox {lo.tolist()}..{hi.tolist()}, want {want_lo}..{want_hi}")
        furn_occ.setdefault(fu["product"], set()).add(el.id())
    for pid, occ in furn_occ.items():
        need(occurrences(pid) == occ, f"{pid}: IfcRelDefinesByType relates {len(occurrences(pid))} objects, "
                                      f"want exactly its {len(occ)} IfcFurniture")

    # --- roofs
    for name, r in ROOFS.items():
        el = by_guid(f, G[name], f"Roof {name}")
        need(el.is_a() == "IfcRoof", f"{name}: exported as {el.is_a()}, want IfcRoof")
        need(el.Name == ids[name], f"{name}: Name {el.Name!r}, want its _name {ids[name]!r}")
        st = storey_of(el)
        need(st is not None and st.GlobalId == G[r["level"]], f"{name}: not contained in storey {r['level']}")
        body_items(el)
        pts = r["boundary"]
        A = shoelace(pts)
        v, lo, hi = shape(el, scale)
        need(close(v, A * r["thickness"]), f"{name}: ifcopenshell volume {v}, want plan area x thickness {A * r['thickness']}")
        x0, y0 = pts[0]
        zb = LEV[r["level"]] + r.get("base", 0.0)
        under = [zb + r["slope"] * ((x - x0) * math.cos(r["angle"]) + (y - y0) * math.sin(r["angle"])) for x, y in pts]
        want_lo = [min(p[0] for p in pts), min(p[1] for p in pts), min(under)]
        want_hi = [max(p[0] for p in pts), max(p[1] for p in pts), max(under) + r["thickness"]]
        need(np.allclose(lo, want_lo, atol=1e-6) and np.allclose(hi, want_hi, atol=1e-6),
             f"{name}: world bbox {lo.tolist()}..{hi.tolist()}, want {want_lo}..{want_hi}")

    # --- coverings: one per finish reference, covering its element, typed by the finish product
    seen = set()
    occ_by_type = {}
    for name, prods in FINISHES.items():
        el = by_guid(f, G[name], name)
        cov = [c for rel in getattr(el, "HasCoverings", ()) for c in rel.RelatedCoverings]
        need(len(cov) == len(prods), f"{name}: {len(cov)} IfcCoverings related by IfcRelCoversBldgElements, "
                                     f"want {len(prods)} (one per finish reference)")
        tags = []
        for c in cov:
            need(c.is_a() == "IfcCovering", f"{name}: covering is {c.is_a()}, want IfcCovering")
            t = uel.get_type(c)
            tags.append(t.Tag if t is not None else None)
            seen.add(c.id())
            if t is not None:
                occ_by_type.setdefault(t.Tag, set()).add(c.id())
        need(sorted(map(str, tags)) == sorted(prods), f"{name}: coverings typed {sorted(map(str, tags))}, want {sorted(prods)}")
    need(len(f.by_type("IfcCovering")) == len(seen),
         f"{len(f.by_type('IfcCovering'))} IfcCovering in the file, want {len(seen)} (one per finish reference)")
    for pid, occ in occ_by_type.items():
        need(occurrences(pid) == occ, f"{pid}: IfcRelDefinesByType relates objects other than its coverings")
    need(not uel.get_types(types["acme:WC-01"]), "acme:WC-01 is referenced by nothing but has occurrences")

    # --- extension kinds
    for name, s in SPRINKLERS.items():
        el = by_guid(f, G[name], f"fire:Sprinkler {name}")
        need(el.is_a() == "IfcBuildingElementProxy", f"{name}: exported as {el.is_a()}, want IfcBuildingElementProxy")
        need(el.ObjectType == "fire:Sprinkler", f"{name}: ObjectType {el.ObjectType!r}, want 'fire:Sprinkler'")
        need(el.Name == ids[name], f"{name}: Name {el.Name!r}, want its _name {ids[name]!r}")
        st = storey_of(el)
        need(st is not None and st.GlobalId == G[s["level"]], f"{name}: not contained in storey {s['level']}")
        ps = psets(el).get("fire_Sprinkler")
        need(ps is not None, f"{name}: no property set fire_Sprinkler (has {sorted(psets(el))})")
        for k, v in s.items():
            if k != "level":
                need(k in ps, f"{name}: fire_Sprinkler lacks {k!r} (has {sorted(ps)})")
                same(ps[k], v, f"{name}: fire_Sprinkler.{k}")
    el = by_guid(f, G["PUMP"], "fire:Pump PUMP")
    need(el.is_a() == "IfcBuildingElementProxy" and el.ObjectType == "fire:Pump",
         f"PUMP: exported as {el.is_a()} ObjectType {el.ObjectType!r}, want IfcBuildingElementProxy 'fire:Pump'")
    ps = psets(el).get("fire_Pump")
    need(ps is not None, f"PUMP: no property set fire_Pump (has {sorted(psets(el))})")
    for k, v in PUMP.items():
        need(k in ps, f"PUMP: fire_Pump lacks {k!r}")
        same(ps[k], v, f"PUMP: fire_Pump.{k}")


def main():
    ids, path = build()
    check(ids, path)
    print("T5.8 judge: all checks passed")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, TypeError, ValueError, IndexError, AttributeError) as e:
        fail(f"malformed export or missing entity: {type(e).__name__}: {e}")
