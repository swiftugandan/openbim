"""T5.1: compare bim-ifc's parsed instances (driver `dump` JSON lines) with ifcopenshell's parse.

Usage: python -I -B dump_compare.py <file.ifc> <dump.jsonl>
Every instance id, entity name and parameter value must match the oracle: reals bit-exact, strings
decoded (\\X2\\, \\X4\\, '' and \\\\ escapes), binaries as bit strings, enums/booleans/logicals, typed
selects, nested lists and references. '$' and '*' both read as "no value" here; the round-trip check
in compare.py distinguishes them.
"""
import json
import struct
import sys

import ifcopenshell


def oracle_norm(v):
    if isinstance(v, ifcopenshell.entity_instance):
        if v.id() > 0:
            return ("ref", v.id())
        return ("typed", v.is_a().upper(), oracle_norm(v.wrappedValue))
    if isinstance(v, (tuple, list)):
        return ("list", tuple(oracle_norm(x) for x in v))
    if v is None:
        return ("none",)
    if isinstance(v, bool):
        return ("bool", v)
    if isinstance(v, int):
        return ("int", v)
    if isinstance(v, float):
        return ("float", struct.pack(">d", v).hex())
    if isinstance(v, str):
        return ("str", v)
    raise TypeError(f"unexpected oracle value {v!r}")


def bim_norm(v):
    if v is None:
        return ("none",)
    if isinstance(v, list):
        return ("list", tuple(bim_norm(x) for x in v))
    if isinstance(v, dict) and "unknown" in v:
        return ("unsupported Param variant",)
    if not isinstance(v, dict) or len(v) not in (1, 2):
        raise ValueError(f"malformed dump value {v!r}")
    if "d" in v:
        return ("none",)
    if "i" in v:
        return ("int", int(v["i"]))
    if "r" in v:
        return ("float", struct.pack(">Q", int(v["r"], 16)).hex())
    if "s" in v:
        return ("str", v["s"])
    if "b" in v:
        return ("str", v["b"])
    if "e" in v:
        e = v["e"].strip(".").upper()
        return {"T": ("bool", True), "F": ("bool", False), "U": ("str", "UNKNOWN")}.get(e, ("str", e))
    if "ref" in v:
        return ("ref", int(v["ref"]))
    if "t" in v:
        return ("typed", v["t"].upper(), bim_norm(v["v"]))
    raise ValueError(f"malformed dump value {v!r}")


def main():
    f = ifcopenshell.open(sys.argv[1])
    bim = {}
    with open(sys.argv[2], encoding="utf-8") as fh:
        for n, line in enumerate(fh, 1):
            if not line.strip():
                continue
            rec = json.loads(line)
            if rec["id"] in bim:
                print(f"instance #{rec['id']} reported twice")
                return 1
            bim[rec["id"]] = rec
    oracle_ids = sorted(e.id() for e in f)
    if sorted(bim) != oracle_ids:
        missing = sorted(set(oracle_ids) - set(bim))[:5]
        extra = sorted(set(bim) - set(oracle_ids))[:5]
        print(f"parsed {len(bim)} instances, oracle {len(oracle_ids)}; missing {missing} extra {extra}")
        return 1
    for i in oracle_ids:
        e, rec = f.by_id(i), bim[i]
        if rec["entity"].upper() != e.is_a().upper():
            print(f"#{i}: entity {rec['entity']!r}, oracle {e.is_a()}")
            return 1
        if len(rec["params"]) != len(e):
            print(f"#{i} {e.is_a()}: {len(rec['params'])} parameters, oracle {len(e)}")
            return 1
        for k in range(len(e)):
            want, got = oracle_norm(e[k]), bim_norm(rec["params"][k])
            if want != got:
                print(f"#{i} {e.is_a()}.{e.attribute_name(k)}: parsed {got}, oracle {want}")
                return 1
    print(f"parse matches oracle: {len(oracle_ids)} instances")
    return 0


sys.exit(main())
