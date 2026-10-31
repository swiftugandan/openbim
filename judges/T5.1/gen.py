"""T5.1 fixture generator: every fixture is produced by ifcopenshell (the independent writer).

Usage: python -I -B gen.py <outdir>
Writes <outdir>/ok/*.ifc (files bim-ifc must round-trip) and <outdir>/bad/*.ifc (malformed files
bim-ifc must reject with an error, not a panic).
"""
import os
import random
import sys

import numpy as np
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
from ifcopenshell import api

out = sys.argv[1]
os.makedirs(f"{out}/ok", exist_ok=True)
os.makedirs(f"{out}/bad", exist_ok=True)
random.seed(51)


def placement(m, product, x, y, z, angle=0.0):
    M = np.eye(4)
    c, s = np.cos(angle), np.sin(angle)
    M[:3, :3] = [[c, -s, 0], [s, c, 0], [0, 0, 1]]
    M[:3, 3] = [x, y, z]
    api.geometry.edit_object_placement(m, product=product, matrix=M)


def building():
    """A small but complete IFC4 building authored through ifcopenshell.api."""
    m = ifcopenshell.file(schema="IFC4")
    proj = api.root.create_entity(m, ifc_class="IfcProject", name="Judge T5.1 Projekt Ä")
    api.unit.assign_unit(m)
    model = api.context.add_context(m, context_type="Model")
    body = api.context.add_context(m, context_type="Model", context_identifier="Body",
                                   target_view="MODEL_VIEW", parent=model)
    site = api.root.create_entity(m, ifc_class="IfcSite", name="Site")
    bld = api.root.create_entity(m, ifc_class="IfcBuilding", name="Building")
    api.aggregate.assign_object(m, relating_object=proj, products=[site])
    api.aggregate.assign_object(m, relating_object=site, products=[bld])
    mat_a = api.material.add_material(m, name="Brick 'red'")
    mat_b = api.material.add_material(m, name="Insulation \\ mineral")
    lset = api.material.add_material_set(m, name="Wall 300", set_type="IfcMaterialLayerSet")
    for mat, t in ((mat_a, 0.1), (mat_b, 0.2)):
        layer = api.material.add_layer(m, layer_set=lset, material=mat)
        api.material.edit_layer(m, layer=layer, attributes={"LayerThickness": t})
    for k, elev in enumerate((0.0, 3.0)):
        st = api.root.create_entity(m, ifc_class="IfcBuildingStorey", name=f"Storey {k} – 中文")
        st.Elevation = elev
        api.aggregate.assign_object(m, relating_object=bld, products=[st])
        placement(m, st, 0, 0, elev)
        for i in range(3):
            w = api.root.create_entity(m, ifc_class="IfcWall", name=f"Wall {k}.{i} \"quoted\" 'single'")
            api.spatial.assign_container(m, relating_structure=st, products=[w])
            rep = api.geometry.add_wall_representation(m, context=body, length=4 + i, height=2.8,
                                                       thickness=0.3, offset=-0.15)
            api.geometry.assign_representation(m, product=w, representation=rep)
            placement(m, w, 10 * i, 1.5 * k, elev, angle=0.3 * i)
            api.material.assign_material(m, products=[w], type="IfcMaterialLayerSetUsage", material=lset)
            o = api.root.create_entity(m, ifc_class="IfcOpeningElement")
            orep = api.geometry.add_wall_representation(m, context=body, length=0.9, height=2.1,
                                                        thickness=0.5, offset=-0.25)
            api.geometry.assign_representation(m, product=o, representation=orep)
            placement(m, o, 10 * i + 1, 1.5 * k, elev, angle=0.3 * i)
            api.feature.add_feature(m, feature=o, element=w)
            d = api.root.create_entity(m, ifc_class="IfcDoor", name=f"Door {k}.{i}")
            d.OverallWidth, d.OverallHeight = 0.9, 2.1
            api.feature.add_filling(m, opening=o, element=d)
            pset = api.pset.add_pset(m, product=w, name="Pset_WallCommon")
            api.pset.edit_pset(m, pset=pset, properties={
                "Reference": f"W-{k}{i} Ä中😀 back\\slash 'q'",
                "IsExternal": i % 2 == 0,
                "LoadBearing": True,
                "ThermalTransmittance": 0.1 + 0.05 * i,
                "FireRating": "",
            })
        s = api.root.create_entity(m, ifc_class="IfcSlab", name=f"Slab {k}")
        api.spatial.assign_container(m, relating_structure=st, products=[s])
        rep = api.geometry.add_slab_representation(m, context=body, depth=0.25,
                                                   polyline=[(0, 0), (12, 0), (12, 8), (5, 9.5), (0, 8)])
        api.geometry.assign_representation(m, product=s, representation=rep)
        placement(m, s, 0, 0, elev - 0.25)
        c = api.root.create_entity(m, ifc_class="IfcColumn", name=f"Column {k}")
        api.spatial.assign_container(m, relating_structure=st, products=[c])
        prof = m.createIfcRectangleProfileDef("AREA", "C300x400", None, 0.3, 0.4)
        rep = api.geometry.add_profile_representation(m, context=body, profile=prof, depth=3.0)
        api.geometry.assign_representation(m, product=c, representation=rep)
        placement(m, c, 2.5, 2.5, elev)
        sp = api.root.create_entity(m, ifc_class="IfcSpace", name=f"{k}01")
        sp.LongName = "Büro / office"
        api.aggregate.assign_object(m, relating_object=st, products=[sp])
    return m


def values():
    """Every Part 21 value form ifcopenshell can write: binary, \\X2\\ and \\X4\\ strings, enums,
    logicals, derived *, unset $, typed selects, nested lists, extreme floats, negative integers."""
    m = ifcopenshell.file(schema="IFC4")
    m.createIfcBlobTexture(True, False, "MODULATE", None, None, "PNG", "0101101100011010")
    m.createIfcBlobTexture(False, True, None, None, None, "JPG", "111100001010")  # ifcopenshell 0.9 misreads non-multiple-of-4 bit counts
    m.createIfcPixelTexture(True, True, None, None, None, 2, 1, 3,
                            ["101100001111000011110000", "111111110000000000000000"])
    floats = [0.0, -0.0, 1e-300, 5e-324, 1.7976931348623157e308, -12345678.123456789, 0.1, 1 / 3,
              2.5e-7, 6.02214076e23, -1.0, 100.0]
    for i in range(0, len(floats), 3):
        m.createIfcCartesianPoint(tuple(floats[i:i + 3]))
    m.createIfcSIUnit(None, "LENGTHUNIT", None, "METRE")
    m.createIfcSIUnit(None, "PLANEANGLEUNIT", None, "RADIAN")
    owner = m.createIfcPerson(None, "Ünïcödé 中文 😀 \U0001F3D7", "", ("a'b", "c\\d", "\t-tab"), None, None, None, None)
    org = m.createIfcOrganization(None, "Org \"q\" 'q' & <x>", None, None, None)
    m.createIfcPersonAndOrganization(owner, org, None)
    pts = m.createIfcCartesianPointList3D([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.5]])
    m.createIfcTriangulatedFaceSet(pts, None, True, [[1, 2, 3], [1, 2, 4], [2, 3, 4], [1, 3, 4]], None)
    m.createIfcPropertySingleValue("logical-u", None, m.createIfcLogical("UNKNOWN"), None)
    m.createIfcPropertySingleValue("logical-f", None, m.createIfcLogical(False), None)
    m.createIfcPropertySingleValue("int-neg", None, m.createIfcInteger(-42), None)
    m.createIfcPropertySingleValue("count", None, m.createIfcCountMeasure(7), None)
    m.createIfcPropertySingleValue("len", None, m.createIfcLengthMeasure(-0.000125), None)
    m.createIfcPropertySingleValue("text", None, m.createIfcText("line1\nline2 é"), None)
    m.createIfcPropertySingleValue("empty", None, m.createIfcLabel(""), None)
    m.createIfcPropertySingleValue("only-quote", None, m.createIfcLabel("'"), None)
    m.createIfcPropertyEnumeratedValue("enum", None, [m.createIfcLabel("A"), m.createIfcLabel("B")], None)
    m.createIfcPropertyListValue("list", None, [m.createIfcReal(1.5), m.createIfcReal(-2.25)], None)
    m.createIfcPropertyBoundedValue("bounded", None, m.createIfcThermodynamicTemperatureMeasure(300.0),
                                    m.createIfcThermodynamicTemperatureMeasure(250.0), None, None)
    m.createIfcPropertySingleValue("long", None, m.createIfcText("x" * 5000 + "Ä" * 300), None)
    return m


def big(n_faces=4000):
    """A larger file (tens of thousands of instances) to exercise streaming over many records."""
    m = ifcopenshell.file(schema="IFC4")
    rng = random.Random(510)
    faces = []
    for _ in range(n_faces):
        poly = [m.createIfcCartesianPoint((rng.uniform(-1e3, 1e3), rng.uniform(-1e3, 1e3), rng.uniform(-1e3, 1e3)))
                for _ in range(3)]
        faces.append(m.createIfcFace([m.createIfcFaceOuterBound(m.createIfcPolyLoop(poly), True)]))
    m.createIfcFacetedBrep(m.createIfcClosedShell(faces))
    return m


def commented(src_text):
    """Same data as src_text with Part 21 comments and extra whitespace/newlines between tokens."""
    head, data = src_text.split("DATA;\n", 1)
    lines = []
    for i, line in enumerate(data.split("\n")):
        if line.startswith("#") and i % 2 == 0:
            # break the record after its '=' and after the first top-level comma outside strings
            eq = line.index("=")
            line = line[:eq] + " =\n  /* split */ " + line[eq + 1:]
        lines.append(line)
        if i % 3 == 0:
            lines.append(f"/* comment {i} with ; and ' and ( */")
    return head.replace("HEADER;", "HEADER;\n/* header comment */") + "DATA;\n" + "\n".join(lines)


bld = building()
bld.write(f"{out}/ok/building.ifc")
values().write(f"{out}/ok/values.ifc")
big().write(f"{out}/ok/big.ifc")
text = open(f"{out}/ok/building.ifc", encoding="ascii").read()
open(f"{out}/ok/commented.ifc", "w", encoding="ascii").write(commented(text))

# Malformed derivatives of the oracle's own output.
data_start = text.index("DATA;")
cut = text.index("IFCWALL(", data_start) + 20
open(f"{out}/bad/truncated.ifc", "w").write(text[:cut])
open(f"{out}/bad/no-endsec.ifc", "w").write(text.replace("ENDSEC;\nEND-ISO-10303-21;", ""))
q = text.index("IFCWALL(", data_start)
open(f"{out}/bad/unterminated-string.ifc", "w").write(text[:q] + "IFCWALL('abc,$);\n" + text[text.index("\n", q) + 1:])
p = text.index("IFCSLAB(", data_start)
open(f"{out}/bad/unbalanced-paren.ifc", "w").write(text[:p] + text[p:text.index(";", p)] + "(;" + text[text.index(";", p) + 1:])
open(f"{out}/bad/not-step.ifc", "w").write("hello, this is not ISO 10303-21\n")
print("generated")
