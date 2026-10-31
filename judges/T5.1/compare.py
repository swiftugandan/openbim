"""T5.1 oracle comparison: ifcopenshell must parse <a> and <b> identically.

Usage: python -I -B compare.py <a.ifc> <b.ifc> [--same-ids]
Compares the header and, per instance id, the entity type and every attribute (entity references by
instance id and type, floats bit-exact, strings exact, '*' vs '$' distinguished via ifcopenshell's own
re-serialisation). Exit 0 when identical, 1 with a reason otherwise.
"""
import math
import struct
import sys

import ifcopenshell


def norm(v):
    if isinstance(v, ifcopenshell.entity_instance):
        if v.id() > 0:
            return ("ref", v.id(), v.is_a())
        return ("typed", v.is_a(), norm(v.wrappedValue))
    if isinstance(v, (tuple, list)):
        return ("list", tuple(norm(x) for x in v))
    if isinstance(v, float):
        # bit-exact, so -0.0 != 0.0 and the shortest-round-trip requirement is enforced
        return ("float", struct.pack(">d", v).hex())
    if isinstance(v, bool):
        return ("bool", v)
    if isinstance(v, int):
        return ("int", v)
    return (type(v).__name__, v)


def header(f):
    h = f.header
    return (
        tuple(h.file_description.description), h.file_description.implementation_level,
        h.file_name.name, h.file_name.time_stamp, tuple(h.file_name.author), tuple(h.file_name.organization),
        h.file_name.preprocessor_version, h.file_name.originating_system, h.file_name.authorization,
        tuple(h.file_schema.schema_identifiers),
    )


def main():
    a_path, b_path = sys.argv[1], sys.argv[2]
    try:
        a = ifcopenshell.open(a_path)
    except Exception as e:  # noqa: BLE001
        print(f"oracle cannot parse reference {a_path}: {e}")
        return 1
    try:
        b = ifcopenshell.open(b_path)
    except Exception as e:  # noqa: BLE001
        print(f"ifcopenshell cannot parse {b_path}: {e}")
        return 1
    if header(a) != header(b):
        print(f"header differs:\n  {header(a)}\n  {header(b)}")
        return 1
    ids_a = sorted(e.id() for e in a)
    ids_b = sorted(e.id() for e in b)
    if ids_a != ids_b:
        missing = sorted(set(ids_a) - set(ids_b))[:5]
        extra = sorted(set(ids_b) - set(ids_a))[:5]
        print(f"instance ids differ ({len(ids_a)} vs {len(ids_b)}); missing {missing} extra {extra}")
        return 1
    for i in ids_a:
        ea, eb = a.by_id(i), b.by_id(i)
        if ea.is_a() != eb.is_a():
            print(f"#{i}: type {ea.is_a()} became {eb.is_a()}")
            return 1
        for k in range(len(ea)):
            va, vb = norm(ea[k]), norm(eb[k])
            if va != vb:
                name = ea.attribute_name(k)
                print(f"#{i} {ea.is_a()}.{name}: {ea[k]!r} became {eb[k]!r}")
                return 1
        if str(ea) != str(eb):  # catches '*' (derived) vs '$' (unset)
            print(f"#{i}: ifcopenshell serialises differently:\n  {ea}\n  {eb}")
            return 1
    print(f"identical: {len(ids_a)} instances")
    return 0


sys.exit(main())
