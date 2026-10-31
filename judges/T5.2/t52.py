"""T5.2 judge helper (oracle side). Builders must not edit judges/.

  python -I -B t52.py fixture <outdir>
      writes ops.json (the document, JSON ops per spec/cli.md), ops-permuted.json (same document,
      different op order), edit.json (one wall height change) and expected.json (closed forms).
  python -I -B t52.py check <outdir> <a.ifc> <solve.json> <b.ifc> <edited.ifc> <edited-solve.json>
      a.ifc: export of ops.json; b.ifc: export of ops-permuted.json; edited.*: after edit.json.

GlobalId rule (TASKS.md T5.2 Details): an entity's IfcRoot.GlobalId is
ifcopenshell.guid.compress(<its 128-bit EntityId as 32 lowercase hex digits>); storeys carry their
Level's GlobalId and an IfcOpeningElement its Opening's. Fixture ids are ULIDs, so the judge knows
each EntityId.
"""
import json
import math
import random
import sys

import numpy as np
import ifcopenshell
import ifcopenshell.geom
import ifcopenshell.guid
import ifcopenshell.util.element as uel
import ifcopenshell.util.placement as upl
import ifcopenshell.util.shape as ush
import ifcopenshell.util.unit as uunit
import ifcopenshell.validate

REL = 1e-4
CROCK = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def ulid(n):
    return "".join(CROCK[(n >> (5 * (25 - i))) & 31] for i in range(26))


def ulid_value(s):
    n = 0
    for ch in s:
        n = n * 32 + CROCK.index(ch)
    return n


def guid_of(ulid_str):
    return ifcopenshell.guid.compress(format(ulid_value(ulid_str), "032x"))


def shoelace(pts):
    return 0.5 * sum(pts[i][0] * pts[(i + 1) % len(pts)][1] - pts[(i + 1) % len(pts)][0] * pts[i][1]
                     for i in range(len(pts)))


def fixture(out):
    rng = random.Random(52)
    names = ["L0", "L1", "WT3", "WT1", "M1", "W1", "W2", "W3", "W4", "D1", "N1", "N2",
             "S0", "S1", "SO1", "C1", "B1", "SP1"]
    ids = {n: ulid(rng.getrandbits(128)) for n in names}
    R = lambda n: {"ref": ids[n]}  # noqa: E731
    lv = {"L0": 0.0, "L1": 3.2}
    ents = [
        ("L0", "Level", {"elevation": lv["L0"]}),
        ("L1", "Level", {"elevation": lv["L1"]}),
        ("WT3", "WallType", {"layer_thickness": [0.1, 0.15, 0.05], "layer_material": ["Brick", "Insulation", "Plaster"],
                             "layer_conductivity": [0.8, 0.035, 0.5]}),
        ("WT1", "WallType", {"layer_thickness": [0.2], "layer_material": ["Concrete"], "layer_conductivity": [2.0]}),
        ("M1", "Material", {"name": "Steel S355", "E": 2.1e11, "G": 8.1e10, "density": 7850.0, "fy": 3.55e8}),
        # L-join W1/W2 at (8,0)
        ("W1", "Wall", {"level": R("L0"), "x1": 0.0, "y1": 0.0, "x2": 8.0, "y2": 0.0, "height": 3.0, "type": R("WT3")}),
        ("W2", "Wall", {"level": R("L0"), "x1": 8.0, "y1": 0.0, "x2": 8.0, "y2": 6.0, "height": 3.0, "type": R("WT3")}),
        # free, non-orthogonal wall with a base offset
        ("W3", "Wall", {"level": R("L0"), "x1": 12.0, "y1": 0.0, "x2": 16.0, "y2": 3.0, "height": 2.7, "type": R("WT1"),
                        "base_offset": 0.1}),
        ("W4", "Wall", {"level": R("L1"), "x1": 0.0, "y1": 10.0, "x2": 5.0, "y2": 10.0, "height": 2.5, "type": R("WT1")}),
        ("D1", "Opening", {"host": R("W1"), "kind": "door", "offset": 1.0, "sill": 0.0, "width": 0.9, "height": 2.1}),
        ("N1", "Opening", {"host": R("W2"), "kind": "window", "offset": 2.0, "sill": 0.9, "width": 1.2, "height": 1.0}),
        ("N2", "Opening", {"host": R("W3"), "kind": "window", "offset": 1.0, "sill": 1.0, "width": 0.8, "height": 1.2}),
        ("S0", "Slab", {"level": R("L0"), "boundary": [20.0, 0.0, 26.0, 0.0, 26.0, 2.0, 22.0, 2.0, 22.0, 5.0, 20.0, 5.0],
                        "thickness": 0.25}),
        ("S1", "Slab", {"level": R("L1"), "boundary": [0.0, 0.0, 8.0, 0.0, 8.0, 6.0, 0.0, 6.0], "thickness": 0.2}),
        ("SO1", "SlabOpening", {"host": R("S1"), "boundary": [2.0, 2.0, 3.0, 2.0, 3.0, 3.5, 2.0, 3.5]}),
        ("C1", "Column", {"x": 4.0, "y": 1.5, "base": R("L0"), "top": R("L1"), "section_b": 0.3, "section_h": 0.4,
                          "material": R("M1")}),
        ("B1", "Beam", {"level": R("L1"), "x1": 0.5, "y1": 4.0, "x2": 7.5, "y2": 4.0, "section_b": 0.2,
                        "section_h": 0.4, "material": R("M1"), "z_offset": 0.2}),
        ("SP1", "Space", {"level": R("L0"), "x": 4.0, "y": 3.0, "name": "Living", "use": "office"}),
    ]
    ops = [{"op": "create", "id": ids[n], "kind": k, "props": p} for n, k, p in ents]
    json.dump({"schema": 1, "ops": ops}, open(f"{out}/ops.json", "w"), indent=1)
    # Same document, different order: creates without refs first, then refs set afterwards.
    perm = []
    for n, k, p in reversed(ents):
        perm.append({"op": "create", "id": ids[n], "kind": k, "props": {}})
    for n, k, p in ents[::2] + ents[1::2]:
        for key, val in sorted(p.items(), reverse=True):
            perm.append({"op": "set", "id": ids[n], "key": key, "value": val})
    json.dump({"schema": 1, "ops": perm}, open(f"{out}/ops-permuted.json", "w"), indent=1)
    json.dump({"schema": 1, "ops": [{"op": "set", "id": ids["W3"], "key": "height", "value": 2.2}]},
              open(f"{out}/edit.json", "w"))

    def slab_closed(boundary, holes, t):
        pts = list(zip(boundary[::2], boundary[1::2]))
        a = shoelace(pts) - sum(abs(shoelace(list(zip(h[::2], h[1::2])))) for h in holes)
        xs, ys = boundary[::2], boundary[1::2]
        return a * t, (min(xs), min(ys)), (max(xs), max(ys))

    p = dict((n, pp) for n, _, pp in ents)
    exp = {"ids": ids, "levels": {n: lv[n] for n in ("L0", "L1")}, "elements": {}}
    v, lo, hi = slab_closed(p["S0"]["boundary"], [], 0.25)
    exp["elements"]["S0"] = {"class": "IfcSlab", "level": "L0", "volume": v,
                             "bbox": [[lo[0], lo[1], -0.25], [hi[0], hi[1], 0.0]], "pset": "Pset_SlabCommon"}
    v, lo, hi = slab_closed(p["S1"]["boundary"], [p["SO1"]["boundary"]], 0.2)
    exp["elements"]["S1"] = {"class": "IfcSlab", "level": "L1", "volume": v,
                             "bbox": [[lo[0], lo[1], 3.0], [hi[0], hi[1], 3.2]], "pset": "Pset_SlabCommon"}
    exp["elements"]["C1"] = {"class": "IfcColumn", "level": "L0", "volume": 0.3 * 0.4 * 3.2,
                             "bbox_xy_either": [[4.0, 1.5], [0.3, 0.4]], "z": [0.0, 3.2], "pset": "Pset_ColumnCommon"}
    exp["elements"]["B1"] = {"class": "IfcBeam", "level": "L1", "volume": 0.2 * 0.4 * 7.0,
                             "bbox": [[0.5, 3.9, 2.6], [7.5, 4.1, 3.0]], "pset": "Pset_BeamCommon"}
    for w in ("W1", "W2", "W3", "W4"):
        wt = "WT3" if p[w]["type"]["ref"] == ids["WT3"] else "WT1"
        exp["elements"][w] = {"class": "IfcWall", "level": "L1" if w == "W4" else "L0", "pset": "Pset_WallCommon",
                              "type": wt, "thickness": sum(p[wt]["layer_thickness"])}
    # free walls: closed-form bboxes (W3 rotated: bbox of the offset rectangle; W4 axis-aligned)
    exp["elements"]["W4"]["bbox"] = [[0.0, 9.9, 3.2], [5.0, 10.1, 5.7]]
    d = np.array([4.0, 3.0]) / 5.0
    nrm = np.array([-d[1], d[0]])
    corners = [np.array(c) + s * 0.1 * nrm for c in ((12.0, 0.0), (16.0, 3.0)) for s in (-1, 1)]
    exp["elements"]["W3"]["bbox"] = [[min(c[0] for c in corners), min(c[1] for c in corners), 0.1],
                                     [max(c[0] for c in corners), max(c[1] for c in corners), 2.8]]
    exp["openings"] = {"D1": ("W1", "IfcDoor", "Pset_DoorCommon"), "N1": ("W2", "IfcWindow", "Pset_WindowCommon"),
                       "N2": ("W3", "IfcWindow", "Pset_WindowCommon")}
    exp["wall_types"] = {n: {"t": p[n]["layer_thickness"], "m": p[n]["layer_material"]} for n in ("WT3", "WT1")}
    exp["space"] = {"SP1": "L0"}
    json.dump(exp, open(f"{out}/expected.json", "w"), indent=1)


class Fail(Exception):
    pass


def need(cond, msg):
    if not cond:
        raise Fail(msg)


def close(a, b, rel=REL, absol=1e-9):
    return abs(a - b) <= max(rel * max(abs(a), abs(b)), absol)


SETTINGS = ifcopenshell.geom.settings()
SETTINGS.set("use-world-coords", True)


def shape(el):
    try:
        sh = ifcopenshell.geom.create_shape(SETTINGS, el)
    except Exception as e:  # noqa: BLE001
        raise Fail(f"ifcopenshell.geom cannot build {el.is_a()} {el.GlobalId}: {e}")
    verts = np.array(sh.geometry.verts).reshape(-1, 3)
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
    return items


def storey_of(el):
    c = uel.get_container(el)
    if c is None:
        c = uel.get_aggregate(el)
    return c


def load(path):
    try:
        return ifcopenshell.open(path)
    except Exception as e:  # noqa: BLE001
        raise Fail(f"ifcopenshell cannot open {path}: {e}")


def by_guid(f, guid, what):
    try:
        return f.by_guid(guid)
    except Exception:  # noqa: BLE001
        raise Fail(f"no IFC object with GlobalId {guid} (expected for {what})")


def check(out, a_path, solve_path, b_path, e_path, esolve_path):
    exp = json.load(open(f"{out}/expected.json"))
    ids = exp["ids"]
    G = {n: guid_of(u) for n, u in ids.items()}
    f = load(a_path)
    need(f.schema == "IFC4", f"schema is {f.schema}, want IFC4")
    log = ifcopenshell.validate.json_logger()
    ifcopenshell.validate.validate(f, log)
    errs = [s for s in log.statements if str(s.get("level", "")).lower() in ("error", "critical")]
    need(not errs, f"ifcopenshell.validate reports {len(errs)} errors, first: {errs[:1]}")
    scale = uunit.calculate_unit_scale(f)

    gids = [e.GlobalId for e in f.by_type("IfcRoot")]
    need(len(gids) == len(set(gids)), "duplicate GlobalIds")

    # Spatial hierarchy
    for cls in ("IfcProject", "IfcSite", "IfcBuilding"):
        need(len(f.by_type(cls)) == 1, f"want exactly one {cls}, got {len(f.by_type(cls))}")
    proj, site, bld = f.by_type("IfcProject")[0], f.by_type("IfcSite")[0], f.by_type("IfcBuilding")[0]
    need(uel.get_aggregate(site) == proj, "IfcSite is not aggregated into the IfcProject")
    need(uel.get_aggregate(bld) == site, "IfcBuilding is not aggregated into the IfcSite")
    storeys = f.by_type("IfcBuildingStorey")
    need(len(storeys) == len(exp["levels"]), f"{len(storeys)} storeys for {len(exp['levels'])} Levels")
    for lvl, elev in exp["levels"].items():
        st = by_guid(f, G[lvl], f"Level {lvl}")
        need(st.is_a("IfcBuildingStorey"), f"Level {lvl} exported as {st.is_a()}")
        need(uel.get_aggregate(st) == bld, f"storey {lvl} is not aggregated into the IfcBuilding")
        z = upl.get_local_placement(st.ObjectPlacement)[2, 3] * scale if st.ObjectPlacement else None
        attr = st.Elevation * scale if st.Elevation is not None else None
        need((z is not None and close(z, elev, 0, 1e-9)) or (attr is not None and close(attr, elev, 0, 1e-9)),
             f"storey {lvl}: placement z {z} / Elevation {attr}, want {elev}")

    solve = json.load(open(solve_path))
    svol = {o["id"]: o["volume"] for o in solve.get("walls", []) + solve.get("slabs", [])}
    for name, e in exp["elements"].items():
        el = by_guid(f, G[name], name)
        need(el.is_a() == e["class"], f"{name}: exported as {el.is_a()}, want {e['class']}")
        st = storey_of(el)
        need(st is not None and st.GlobalId == G[e["level"]], f"{name}: not contained in storey {e['level']}")
        for it in body_items(el):
            need(it.is_a("IfcExtrudedAreaSolid") or it.is_a("IfcFacetedBrep"),
                 f"{name}: body item {it.is_a()} (cli.md: IfcExtrudedAreaSolid / IfcFacetedBrep)")
        psets = uel.get_psets(el)
        need(e["pset"] in psets, f"{name}: missing {e['pset']} (has {sorted(psets)})")
        vol, lo, hi = shape(el)
        if "volume" in e:
            need(close(vol, e["volume"]), f"{name}: ifcopenshell volume {vol}, closed form {e['volume']}")
        if name in ids and ids[name] in svol:
            need(close(vol, svol[ids[name]]), f"{name}: ifcopenshell volume {vol}, bim solve volume {svol[ids[name]]}")
        elif e["class"] in ("IfcWall", "IfcSlab"):
            raise Fail(f"{name}: bim solve --trade arch reports no volume for id {ids[name]}")
        if "bbox" in e:
            need(np.allclose(lo, e["bbox"][0], atol=1e-6) and np.allclose(hi, e["bbox"][1], atol=1e-6),
                 f"{name}: world bbox {lo.tolist()}..{hi.tolist()}, want {e['bbox']}")
        if "bbox_xy_either" in e:
            (cx, cy), (b, h) = e["bbox_xy_either"]
            ok = any(np.allclose([lo[0], lo[1], hi[0], hi[1]], [cx - u / 2, cy - v / 2, cx + u / 2, cy + v / 2], atol=1e-6)
                     for u, v in ((b, h), (h, b)))
            need(ok and close(lo[2], e["z"][0], 0, 1e-6) and close(hi[2], e["z"][1], 0, 1e-6),
                 f"{name}: world bbox {lo.tolist()}..{hi.tolist()}, want {b}x{h} centred on ({cx},{cy}), z {e['z']}")
        if e["class"] == "IfcWall":
            mat = uel.get_material(el, should_skip_usage=True)
            need(mat is not None and mat.is_a("IfcMaterialLayerSet"), f"{name}: no IfcMaterialLayerSet (got {mat})")
            t = [l.LayerThickness * scale for l in mat.MaterialLayers]
            m = [l.Material.Name if l.Material else None for l in mat.MaterialLayers]
            wt = exp["wall_types"][e["type"]]
            fwd = np.allclose(t, wt["t"], atol=1e-9) and m == wt["m"] if len(t) == len(wt["t"]) else False
            rev = np.allclose(t[::-1], wt["t"], atol=1e-9) and m[::-1] == wt["m"] if len(t) == len(wt["t"]) else False
            need(fwd or rev, f"{name}: layers {list(zip(t, m))}, want {list(zip(wt['t'], wt['m']))}")

    for name, (host, fill_cls, pset) in exp["openings"].items():
        op = by_guid(f, G[name], f"Opening {name}")
        need(op.is_a("IfcOpeningElement"), f"{name}: exported as {op.is_a()}")
        voided = [r.RelatingBuildingElement for r in op.VoidsElements]
        need(len(voided) == 1 and voided[0].GlobalId == G[host],
             f"{name}: IfcRelVoidsElement should void wall {host}, got {[v.GlobalId for v in voided]}")
        fills = [r.RelatedBuildingElement for r in op.HasFillings]
        need(len(fills) == 1 and fills[0].is_a(fill_cls), f"{name}: filling {[x.is_a() for x in fills]}, want one {fill_cls}")
        need(pset in uel.get_psets(fills[0]), f"{name}: {fill_cls} lacks {pset}")

    for name, lvl in exp["space"].items():
        sp = by_guid(f, G[name], f"Space {name}")
        need(sp.is_a("IfcSpace"), f"{name}: exported as {sp.is_a()}")
        st = storey_of(sp)
        need(st is not None and st.GlobalId == G[lvl], f"{name}: IfcSpace not in storey {lvl}")
        need("Pset_SpaceCommon" in uel.get_psets(sp), f"{name}: missing Pset_SpaceCommon")

    # Stability: the permuted-order document exports the same entity GlobalIds and geometry.
    fb = load(b_path)
    for name in list(exp["elements"]) + list(exp["openings"]) + list(exp["levels"]) + list(exp["space"]):
        el = by_guid(fb, G[name], f"{name} in the permuted-order export")
        if name in exp["elements"]:
            va = shape(f.by_guid(G[name]))[0]
            vb = shape(el)[0]
            need(close(va, vb, 1e-9), f"{name}: volume differs between equal documents ({va} vs {vb})")
    # Editing W3's height keeps every GlobalId and changes only W3's geometry.
    fe = load(e_path)
    esolve = json.load(open(esolve_path))
    evol = {o["id"]: o["volume"] for o in esolve.get("walls", [])}
    for name in exp["elements"]:
        el = by_guid(fe, G[name], f"{name} after editing W3")
        if name == "W3":
            vol, lo, hi = shape(el)
            need(close(hi[2], 0.1 + 2.2, 0, 1e-6), f"W3 after edit: top z {hi[2]}, want 2.3")
            need(close(vol, evol.get(ids["W3"], float("nan"))), f"W3 after edit: volume {vol} vs bim solve {evol.get(ids['W3'])}")
    print("ok")


def main():
    if sys.argv[1] == "fixture":
        fixture(sys.argv[2])
        return 0
    try:
        check(*sys.argv[2:8])
    except Fail as e:
        print(str(e))
        return 1
    return 0


sys.exit(main())
