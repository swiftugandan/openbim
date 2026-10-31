"""T5.9 judge helper (oracle side). Builders must not edit judges/.

  python -I -B t59.py drive <bim> <tmpdir>
      builds the fixture project through the real `bim` (init, apply, sketch, show, export) and writes
      <tmpdir>/state.json (expected values + the shapes as `bim show` reports them) and <tmpdir>/out.ifc.
  python -I -B t59.py check <tmpdir>
      checks <tmpdir>/out.ifc against <tmpdir>/state.json with ifcopenshell.

Contract (TASKS.md T5.9, spec/sketch.md): each placed closed Shape -> an occurrence of its `classification`
class (default IfcBuildingElementProxy) with an IfcFacetedBrep of its faces; each ComponentDefinition -> a type
object of the occurrence class's type (IfcBuildingElementProxyType by default) with an IfcRepresentationMap; each
ComponentInstance -> an occurrence whose Body is an IfcMappedItem of that map under the instance transform, linked
by IfcRelDefinesByType; an open surface -> IfcBuildingElementProxy with an IfcShellBasedSurfaceModel; painted faces
carry IfcStyledItem colours from the product's `colour`
(IfcColourRgb = sRGB/255 on the IfcFace itself); MappingOrigin is the identity placement. Volumes by ifcopenshell within 1e-6 relative; instance
placements within 1e-9. GlobalIds follow the T5.2 rule (compress of the 128-bit EntityId); fixture ids are ULIDs
(spec/schema.md: used as-is), so the judge knows every EntityId.
"""
import json
import math
import random
import subprocess
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

CROCK = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
VOL_REL = 1e-6
PLACE_TOL = 1e-9


class Fail(Exception):
    pass


def need(cond, msg):
    if not cond:
        raise Fail(msg)


def ulid(n):
    return "".join(CROCK[(n >> (5 * (25 - i))) & 31] for i in range(26))


def ulid_value(s):
    n = 0
    for ch in s:
        n = n * 32 + CROCK.index(ch)
    return n


def guid_of(u):
    return ifcopenshell.guid.compress(format(ulid_value(u), "032x"))


def close(a, b, rel, absol=1e-12):
    return abs(a - b) <= max(rel * max(abs(a), abs(b)), absol)


# ----------------------------------------------------------------------------- fixture definition

NAMES = ["L0", "BOX1", "BOX2", "CSH", "D1", "I1", "I2", "CSH2", "D2", "I3", "SURF"]
RED, BLUE = "paints:EM-RED", "paints:EM-BLU"


def rz(deg, t):
    """Column-major 4x4 of a rotation about +z by `deg` then a translation t (16 numbers)."""
    c, s = {90: (0.0, 1.0), -90: (0.0, -1.0)}.get(deg, (math.cos(math.radians(deg)), math.sin(math.radians(deg))))
    m = np.array([[c, -s, 0, t[0]], [s, c, 0, t[1]], [0, 0, 1, t[2]], [0, 0, 0, 1.0]])
    return [float(x) for x in m.reshape(16, order="F")]


T_BOX1 = rz(20, (3.0, 4.0, 0.5))
T_I1 = rz(90, (10.0, 0.0, 0.0))
T_I2 = rz(35, (12.5, -3.25, 1.0))
T_I3 = rz(-60, (-4.0, 6.0, 0.0))
IDENTITY = [1.0, 0, 0, 0, 0, 1.0, 0, 0, 0, 0, 1.0, 0, 0, 0, 0, 1.0]
# L footprint (unequal arms: no proper symmetry, so a matching point set pins the transform).
L_PTS = [(0, 0), (2, 0), (2, 0.5), (0.6, 0.5), (0.6, 1.5), (0, 1.5)]
L_AREA = 2 * 0.5 + 0.6 * 1.0
VOLUMES = {"BOX1": 4 * 3 * 2.5, "BOX2": 0.4 * 0.3 * 3.0, "CSH": L_AREA * 1.2, "CSH2": 0.8 * 0.6 * 0.75}
HEIGHTS = {"BOX1": 2.5, "BOX2": 3.0, "CSH": 1.2, "CSH2": 0.75}
SURF_AREA = 2 * 1.5 + 2 * 1.5
SURF_PTS = [(20, 0, 0), (22, 0, 0), (22, 0, 1.5), (20, 0, 1.5), (23, 0, 0), (23, 2, 0), (23, 2, 1.5), (23, 0, 1.5)]


def ids_():
    rng = random.Random(59)
    return {n: ulid(rng.getrandbits(128)) for n in NAMES}


# ----------------------------------------------------------------------------- shape geometry (from bim show)

def verts_of(props):
    v = props["vertices"]
    need(isinstance(v, list) and len(v) % 3 == 0 and len(v) > 0, f"Shape vertices malformed: {str(v)[:80]}")
    return np.array(v, dtype=float).reshape(-1, 3)


def faces_of(props):
    f = props["faces"]
    need(isinstance(f, list), "Shape faces malformed")
    return f


def shape_volume(props):
    """Divergence theorem over the loops (holes are clockwise, so they subtract)."""
    V = verts_of(props)
    vol = 0.0
    for face in faces_of(props):
        for loop in face:
            p0 = V[loop[0]]
            for i in range(1, len(loop) - 1):
                vol += np.dot(p0, np.cross(V[loop[i]], V[loop[i + 1]])) / 6.0
    return vol


def face_points(props, k):
    V = verts_of(props)
    return V[sorted({i for loop in faces_of(props)[k] for i in loop})]


def apply(T, P):
    M = np.array(T, dtype=float).reshape(4, 4, order="F")
    P = np.asarray(P, dtype=float)
    return (M[:3, :3] @ P.T).T + M[:3, 3]


def hausdorff(A, B):
    A, B = np.asarray(A, float), np.asarray(B, float)
    if len(A) == 0 or len(B) == 0:
        return math.inf
    d = np.linalg.norm(A[:, None, :] - B[None, :, :], axis=2)
    return max(d.min(1).max(), d.min(0).max())


def ref_name(v):
    return v.get("ref") if isinstance(v, dict) else None


# ----------------------------------------------------------------------------- driving bim

def drive(bim, tmp):
    ids = ids_()
    P = f"{tmp}/P"

    def run(*args):
        what = "bim " + " ".join(a if len(a) < 40 else "…" + a[-30:] for a in args)
        try:
            r = subprocess.run([bim, *args], capture_output=True, text=True, timeout=60)
        except subprocess.TimeoutExpired:
            raise Fail(f"'{what}' timed out after 60 s")
        need(r.returncode == 0, f"'{what}' exited {r.returncode}: {r.stderr.strip()[:300]}")
        return r.stdout

    def write(name, doc):
        path = f"{tmp}/{name}"
        json.dump(doc, open(path, "w"), indent=1)
        return path

    def show(n):
        out = run("show", P, ids[n] if n in ids else n, "--json")
        try:
            d = json.loads(out)
        except ValueError:
            raise Fail(f"bim show {n} --json printed non-JSON: {out[:200]}")
        need(isinstance(d.get("props"), dict), f"bim show {n}: no props object")
        return d

    def sketch(name, cmds):
        run("sketch", P, write(name, {"schema": 1, "commands": cmds}))

    run("init", P)
    prod = lambda pid, sku, name, colour: {  # noqa: E731
        "op": "create", "id": pid, "kind": "Product",
        "props": {"catalog": "paints", "catalog_version": "2026.1", "sku": sku, "category": "wall_finish",
                  "name": name, "manufacturer": "Judge Paints Ltd", "colour": colour, "coats": 2}}
    run("apply", P, write("ops0.json", {"schema": 1, "ops": [
        {"op": "create", "id": ids["L0"], "kind": "Level", "props": {"elevation": 0.0}},
        prod(RED, "EM-RED", "Matt emulsion, red", "red"),
        prod(BLUE, "EM-BLU", "Matt emulsion, blue", "blue")]}))

    # Phase 1: draw (faces appear).
    lines = [{"op": "line", "shape": ids["CSH"], "from": [*L_PTS[i], 0], "to": [*L_PTS[(i + 1) % len(L_PTS)], 0]}
             for i in range(len(L_PTS))]
    sketch("s1.json", [
        {"op": "rect", "shape": ids["BOX1"], "corner": [1, 2, 0], "u": [4, 0, 0], "v": [0, 3, 0]},
        {"op": "rect", "shape": ids["BOX2"], "corner": [0, 0, 0], "u": [0.4, 0, 0], "v": [0, 0.3, 0]},
        *lines,
        {"op": "rect", "shape": ids["CSH2"], "corner": [0, 0, 0], "u": [0.8, 0, 0], "v": [0, 0.6, 0]},
        {"op": "rect", "shape": ids["SURF"], "corner": [20, 0, 0], "u": [2, 0, 0], "v": [0, 0, 1.5]},
        {"op": "rect", "shape": ids["SURF"], "corner": [23, 0, 0], "u": [0, 2, 0], "v": [0, 0, 1.5]},
    ])
    first = {}
    for n in ("BOX1", "BOX2", "CSH", "CSH2"):
        fids = show(n)["props"].get("face_ids")
        need(isinstance(fids, list) and len(fids) == 1, f"after drawing, Shape {n} has face_ids {fids}, want one face")
        first[n] = fids[0]
    sfids = show("SURF")["props"].get("face_ids")
    need(isinstance(sfids, list) and len(sfids) == 2, f"after drawing, the open surface has face_ids {sfids}, want 2")

    # Phase 2: push/pull into solids.
    sketch("s2.json", [{"op": "pushpull", "shape": ids[n], "face": first[n], "distance": HEIGHTS[n]}
                       for n in ("BOX1", "BOX2", "CSH", "CSH2")])

    # Phase 3: paint two faces of BOX1 (the moved face and the side at local x = 1), make components.
    b1 = show("BOX1")["props"]
    fids = b1.get("face_ids")
    need(isinstance(fids, list) and len(fids) == 6 and first["BOX1"] in fids,
         f"after push/pull BOX1 has face_ids {fids} (want 6 faces keeping id {first['BOX1']})")
    side = [fids[k] for k in range(len(fids)) if np.allclose(face_points(b1, k)[:, 0], 1.0, atol=1e-9)]
    need(len(side) == 1, f"BOX1 has {len(side)} faces in the plane x = 1, want 1")
    sketch("s3.json", [
        {"op": "paint", "shape": ids["BOX1"], "faces": [first["BOX1"]], "product": RED},
        {"op": "paint", "shape": ids["BOX1"], "faces": [side[0]], "product": BLUE},
        {"op": "make_component", "shape": ids["CSH"], "definition": ids["D1"], "instance": ids["I1"], "name": "Crate"},
        {"op": "make_component", "shape": ids["CSH2"], "definition": ids["D2"], "instance": ids["I3"], "name": "Chair"},
    ])

    # Phase 4: classifications, transforms and a second instance of D1 (core kinds: plain ops).
    run("apply", P, write("ops4.json", {"schema": 1, "ops": [
        {"op": "set", "id": ids["BOX2"], "key": "classification", "value": "IfcColumn"},
        {"op": "set", "id": ids["CSH2"], "key": "classification", "value": "IfcFurniture"},
        {"op": "set", "id": ids["BOX1"], "key": "transform", "value": T_BOX1},
        {"op": "set", "id": ids["I1"], "key": "transform", "value": T_I1},
        {"op": "create", "id": ids["I2"], "kind": "ComponentInstance",
         "props": {"definition": {"ref": ids["D1"]}, "transform": T_I2}},
        {"op": "set", "id": ids["I3"], "key": "transform", "value": T_I3},
    ]}))

    shapes = {n: show(n) for n in ("BOX1", "BOX2", "CSH", "CSH2", "SURF", "D1", "D2", "I1", "I2", "I3")}
    # Fixture sanity: the document is what the judge built (else the earlier tasks regressed).
    for n in ("BOX1", "BOX2", "CSH", "CSH2", "SURF"):
        need(shapes[n]["kind"] == "Shape", f"{n} is a {shapes[n]['kind']}, want Shape")
    for n, (shape, name) in {"D1": ("CSH", "Crate"), "D2": ("CSH2", "Chair")}.items():
        p = shapes[n]["props"]
        need(shapes[n]["kind"] == "ComponentDefinition" and ref_name(p.get("shape")) == ids[shape],
             f"make_component did not create ComponentDefinition {n} -> {shape}: {shapes[n]}")
    for n, d in {"I1": "D1", "I2": "D1", "I3": "D2"}.items():
        p = shapes[n]["props"]
        need(shapes[n]["kind"] == "ComponentInstance" and ref_name(p.get("definition")) == ids[d],
             f"{n} is not a ComponentInstance of {d}: {shapes[n]}")
    for n in ("CSH", "CSH2"):
        t = shapes[n]["props"].get("transform", IDENTITY)
        need(np.allclose(t, IDENTITY, atol=1e-12), f"definition Shape {n} transform {t}, want identity after make_component")
    for n, v in VOLUMES.items():
        got = shape_volume(shapes[n]["props"])
        need(close(abs(got), v, 1e-9), f"Shape {n}: volume from its faces {got}, closed form {v} (fixture)")
    fp = shapes["BOX1"]["props"].get("face_products")
    need(isinstance(fp, list) and sorted(ref_name(x) for x in fp if ref_name(x)) == sorted([RED, BLUE]),
         f"BOX1 face_products {fp}: want one face painted {RED} and one {BLUE}")

    run("export", P, "--format", "ifc", "--out", f"{tmp}/out.ifc")
    json.dump({"ids": ids, "shapes": shapes}, open(f"{tmp}/state.json", "w"))


# ----------------------------------------------------------------------------- IFC side

SETTINGS = ifcopenshell.geom.settings()
SETTINGS.set("use-world-coords", True)


def geom(el):
    try:
        sh = ifcopenshell.geom.create_shape(SETTINGS, el)
    except Exception as e:  # noqa: BLE001
        raise Fail(f"ifcopenshell.geom cannot build {el.is_a()} {el.GlobalId}: {e}")
    return sh.geometry, np.array(sh.geometry.verts, dtype=float).reshape(-1, 3)


def body_items(el, name):
    """[(item, mapped_items_path, 4x4 matrix item->placement-relative)] for the Body representation."""
    need(el.Representation is not None, f"{name}: {el.is_a()} has no representation")
    bodies = [r for r in el.Representation.Representations if r.RepresentationIdentifier == "Body"]
    need(len(bodies) == 1, f"{name}: {len(bodies)} Body representations, want 1")
    out = []

    def walk(items, M, path):
        for it in items:
            if it.is_a("IfcMappedItem"):
                T = upl.get_mappeditem_transformation(it)
                need(T is not None, f"{name}: IfcMappedItem target is not an IfcCartesianTransformationOperator3D")
                walk(it.MappingSource.MappedRepresentation.Items, M @ T, path + [it])
            else:
                out.append((it, path, M))
    walk(bodies[0].Items, np.eye(4), [])
    need(out, f"{name}: empty Body representation")
    return bodies[0], out


def loops_of_face(face):
    loops = []
    for b in face.Bounds:
        need(b.Bound.is_a("IfcPolyLoop"), f"face bound {b.Bound.is_a()} (want IfcPolyLoop)")
        pts = [p.Coordinates for p in b.Bound.Polygon]
        loops.append((pts, b.is_a("IfcFaceOuterBound") or len(face.Bounds) == 1))
    return loops


def world(M, pts, placement, scale):
    P = np.array([list(p) + [0.0] * (3 - len(p)) for p in pts], dtype=float)
    W = placement @ M
    return ((W[:3, :3] @ P.T).T + W[:3, 3]) * scale


def colour_of(item):
    for si in getattr(item, "StyledByItem", None) or []:
        for st in si.Styles or []:
            styles = st.Styles if st.is_a("IfcPresentationStyleAssignment") else [st]
            for s in styles:
                if s.is_a("IfcSurfaceStyle"):
                    for e in s.Styles:
                        if e.is_a("IfcSurfaceStyleShading") and e.SurfaceColour is not None:
                            c = e.SurfaceColour
                            if c.is_a("IfcColourRgb"):
                                return (float(c.Red), float(c.Green), float(c.Blue))
    return None


def check(tmp):
    st = json.load(open(f"{tmp}/state.json"))
    ids, S = st["ids"], st["shapes"]
    G = {n: guid_of(u) for n, u in ids.items()}
    try:
        f = ifcopenshell.open(f"{tmp}/out.ifc")
    except Exception as e:  # noqa: BLE001
        raise Fail(f"ifcopenshell cannot open the export: {e}")
    need(f.schema == "IFC4", f"schema is {f.schema}, want IFC4")
    log = ifcopenshell.validate.json_logger()
    ifcopenshell.validate.validate(f, log)
    errs = [s for s in log.statements if str(s.get("level", "")).lower() in ("error", "critical")]
    need(not errs, f"ifcopenshell.validate reports {len(errs)} errors, first: {errs[:1]}")
    scale = uunit.calculate_unit_scale(f)
    gids = [e.GlobalId for e in f.by_type("IfcRoot")]
    need(len(gids) == len(set(gids)), "duplicate GlobalIds")

    def by_guid(n, what):
        try:
            return f.by_guid(G[n])
        except Exception:  # noqa: BLE001
            raise Fail(f"no IFC object with GlobalId {G[n]} (the T5.2 GlobalId of {what})")

    # Exactly the placed shapes and instances are exported as occurrences of these classes.
    occ = sorted(e.GlobalId for c in ("IfcBuildingElementProxy", "IfcColumn", "IfcFurniture") for e in f.by_type(c))
    want_occ = sorted(G[n] for n in ("BOX1", "BOX2", "I1", "I2", "I3", "SURF"))
    need(occ == want_occ, f"IfcBuildingElementProxy/IfcColumn/IfcFurniture occurrences {occ}, want exactly the placed "
         f"Shapes and ComponentInstances {want_occ}")
    # The definitions' Shapes are not placed themselves (spec/sketch.md).
    for n in ("CSH", "CSH2"):
        hit = [e for e in f.by_type("IfcProduct") if e.GlobalId == G[n]]
        need(not hit, f"the Shape of a ComponentDefinition ({n}) is exported as a placed {hit[0].is_a() if hit else ''}")

    def shape_world(n, T):
        return apply(T, verts_of(S[n]["props"]))

    # ------------------------------------------------------------- placed closed Shapes
    occ_spec = {"BOX1": ("IfcBuildingElementProxy", T_BOX1), "BOX2": ("IfcColumn", IDENTITY)}
    breps = {}
    for n, (cls, T) in occ_spec.items():
        el = by_guid(n, f"Shape {n}")
        need(el.is_a() == cls, f"Shape {n}: exported as {el.is_a()}, want {cls}")
        _, items = body_items(el, n)
        need(all(it.is_a("IfcFacetedBrep") for it, _, _ in items) and len(items) == 1,
             f"Shape {n}: Body items {[it.is_a() for it, _, _ in items]}, want one IfcFacetedBrep")
        brep, _, M = items[0]
        faces = list(brep.Outer.CfsFaces)
        nf = len(faces_of(S[n]["props"]))
        need(len(faces) == nf, f"Shape {n}: IfcFacetedBrep has {len(faces)} faces, the Shape has {nf}")
        Pl = upl.get_local_placement(el.ObjectPlacement)
        pts = world(M, [p for fc in faces for lp, _ in loops_of_face(fc) for p in lp], Pl, scale)
        h = hausdorff(pts, shape_world(n, T))
        need(h <= PLACE_TOL, f"Shape {n}: brep vertices are {h:.3g} m from the Shape's vertices under its transform")
        g, gv = geom(el)
        vol = ush.get_volume(g)
        need(close(vol, VOLUMES[n], VOL_REL), f"Shape {n}: ifcopenshell volume {vol}, shape volume {VOLUMES[n]}")
        breps[n] = (brep, faces, M, Pl)

    # ------------------------------------------------------------- components and instances
    inst_spec = {"I1": ("D1", "CSH", T_I1, "IfcBuildingElementProxy"), "I2": ("D1", "CSH", T_I2, "IfcBuildingElementProxy"),
                 "I3": ("D2", "CSH2", T_I3, "IfcFurniture")}
    maps = {}
    for n, (d, shp, T, cls) in inst_spec.items():
        el = by_guid(n, f"ComponentInstance {n}")
        need(el.is_a() == cls, f"ComponentInstance {n}: exported as {el.is_a()}, want {cls} (its definition Shape's class)")
        _, items = body_items(el, n)
        mapped = {p[0].id(): p[0] for _, p, _ in items if p}
        need(all(p for _, p, _ in items) and len(mapped) == 1,
             f"ComponentInstance {n}: Body must be one IfcMappedItem (got items {[it.is_a() for it, p, _ in items if not p]} "
             f"outside mapped items, {len(mapped)} mapped items)")
        mi = next(iter(mapped.values()))
        need(all(it.is_a("IfcFacetedBrep") for it, _, _ in items),
             f"ComponentInstance {n}: mapped representation items {[it.is_a() for it, _, _ in items]}, want IfcFacetedBrep")
        maps.setdefault(d, set()).add(mi.MappingSource.id())
        O = upl.get_axis2placement(mi.MappingSource.MappingOrigin)
        need(np.allclose(O, np.eye(4), rtol=0, atol=1e-12),
             f"ComponentDefinition {d}: IfcRepresentationMap.MappingOrigin is not the identity placement (T5.9 Details): {O.tolist()}")
        Pl = upl.get_local_placement(el.ObjectPlacement)
        pts = []
        for it, _, M in items:
            pts += list(world(M, [p for fc in it.Outer.CfsFaces for lp, _ in loops_of_face(fc) for p in lp], Pl, scale))
        nf = sum(len(it.Outer.CfsFaces) for it, _, _ in items)
        need(nf == len(faces_of(S[shp]["props"])), f"ComponentInstance {n}: {nf} brep faces, the definition Shape has "
             f"{len(faces_of(S[shp]['props']))}")
        want = shape_world(shp, T)
        h = hausdorff(pts, want)
        need(h <= PLACE_TOL, f"ComponentInstance {n}: placed vertices are {h:.3g} m from the definition Shape under the "
             f"instance transform (tolerance {PLACE_TOL})")
        g, gv = geom(el)
        hg = hausdorff(gv, want)
        need(hg <= 1e-6, f"ComponentInstance {n}: ifcopenshell.geom places it {hg:.3g} m from the instance transform")
        vol = ush.get_volume(g)
        need(close(vol, VOLUMES[shp], VOL_REL), f"ComponentInstance {n}: ifcopenshell volume {vol}, shape volume {VOLUMES[shp]}")
        # Type object, representation map, IfcRelDefinesByType.
        ty = uel.get_type(el)
        need(ty is not None, f"ComponentInstance {n}: no type object (IfcRelDefinesByType)")
        need(ty.GlobalId == G[d], f"ComponentInstance {n}: type GlobalId {ty.GlobalId}, want ComponentDefinition {d}'s {G[d]}")
        need(ty.is_a() == cls + "Type", f"ComponentDefinition {d}: type object is {ty.is_a()}, want {cls}Type")
        rmaps = list(ty.RepresentationMaps or [])
        need(len(rmaps) == 1 and rmaps[0].id() == mi.MappingSource.id(),
             f"ComponentDefinition {d}: type's RepresentationMaps {[m.id() for m in rmaps]} should be exactly the "
             f"IfcRepresentationMap #{mi.MappingSource.id()} the instance maps")
    for d in ("D1", "D2"):
        need(len(maps[d]) == 1, f"instances of {d} use {len(maps[d])} different IfcRepresentationMaps, want one shared map")
        ty = by_guid(d, f"ComponentDefinition {d}")
        rel = [o.GlobalId for r in f.by_type("IfcRelDefinesByType") if r.RelatingType == ty for o in r.RelatedObjects]
        want = sorted(G[i] for i, v in inst_spec.items() if v[0] == d)
        need(sorted(rel) == want, f"IfcRelDefinesByType of {d} relates {sorted(rel)}, want exactly its instances {want}")

    # ------------------------------------------------------------- open surface
    el = by_guid("SURF", "the open-surface Shape")
    need(el.is_a() == "IfcBuildingElementProxy", f"open surface exported as {el.is_a()}, want IfcBuildingElementProxy")
    _, items = body_items(el, "SURF")
    need(len(items) == 1 and items[0][0].is_a("IfcShellBasedSurfaceModel"),
         f"open surface Body items {[it.is_a() for it, _, _ in items]}, want one IfcShellBasedSurfaceModel")
    sbsm, _, M = items[0]
    Pl = upl.get_local_placement(el.ObjectPlacement)
    sfaces = [fc for sh in sbsm.SbsmBoundary for fc in sh.CfsFaces]
    need(len(sfaces) == 2, f"open surface: {len(sfaces)} faces in the IfcShellBasedSurfaceModel, want 2")
    area, allp = 0.0, []
    for fc in sfaces:
        for lp, ori in loops_of_face(fc):  # ori: outer boundary
            W = world(M, lp, Pl, scale)
            allp += list(W)
            a = 0.5 * np.linalg.norm(sum(np.cross(W[i], W[(i + 1) % len(W)]) for i in range(len(W))))
            area += a if ori else -a
    need(close(area, SURF_AREA, 1e-9), f"open surface: face area {area}, want {SURF_AREA}")
    h = hausdorff(allp, np.array(SURF_PTS, float))
    need(h <= PLACE_TOL, f"open surface: vertices are {h:.3g} m from the drawn surface")

    # ------------------------------------------------------------- painted faces
    b1 = S["BOX1"]["props"]
    brep, faces, M, Pl = breps["BOX1"]
    fp = b1["face_products"]
    expect = {}
    for k in range(len(faces_of(b1))):
        expect[k] = ref_name(fp[k]) if k < len(fp) else None
    face_col = {}
    for k in expect:
        want = apply(T_BOX1, face_points(b1, k))
        hits = [fc for fc in faces
                if hausdorff(world(M, [p for lp, _ in loops_of_face(fc) for p in lp], Pl, scale), want) <= 1e-7]
        need(len(hits) == 1, f"BOX1 face {b1['face_ids'][k]}: {len(hits)} matching IfcFaces in the brep, want 1")
        face_col[k] = (colour_of(hits[0]), colour_of(hits[0]) or colour_of(brep))
    # T5.9 Details: IfcColourRgb = sRGB / 255 of the product's colour, in an IfcSurfaceStyle attached by an
    # IfcStyledItem to that IfcFace. CSS red = #ff0000, blue = #0000ff.
    same = lambda a, b: a is not None and all(abs(x - y) <= 1e-9 for x, y in zip(a, b))  # noqa: E731
    want_rgb = {RED: (1.0, 0.0, 0.0), BLUE: (0.0, 0.0, 1.0)}
    for k, p in expect.items():
        if p in want_rgb:
            got = face_col[k][0]
            need(got is not None, f"BOX1 face {b1['face_ids'][k]} painted {p}: no IfcStyledItem with an IfcColourRgb on the IfcFace itself")
            need(same(got, want_rgb[p]), f"BOX1 face {b1['face_ids'][k]} painted {p} (colour '{ {RED: "red", BLUE: "blue"}[p] }'): "
                 f"IfcColourRgb {got}, want {want_rgb[p]}")
    need(sorted(p for p in expect.values() if p) == sorted(want_rgb), f"BOX1 painted faces {expect} (fixture)")
    for k, p in expect.items():
        if p is None:
            need(not any(same(face_col[k][1], c) for c in want_rgb.values()),
                 f"unpainted BOX1 face {b1['face_ids'][k]} carries a painted colour {face_col[k][1]}")
    print("ok")


def main():
    try:
        if sys.argv[1] == "drive":
            drive(sys.argv[2], sys.argv[3])
        elif sys.argv[1] == "check":
            check(sys.argv[2])
        else:
            raise Fail(f"unknown mode {sys.argv[1]}")
    except Fail as e:
        print(str(e))
        return 1
    return 0


sys.exit(main())
