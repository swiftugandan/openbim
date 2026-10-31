# Checks a `bim validate --json` document against the expected violation set.
# usage: check.py <stdout-file> <expected-json>  where expected is [[rule, entity, message_substring, measured, limit], …]
import json, math, sys

try:
    doc = json.load(open(sys.argv[1]))
except Exception as e:
    sys.exit(f"stdout is not one JSON document: {e}")
for k in ("violations", "dangling", "conflicts"):
    if not isinstance(doc.get(k), list):
        sys.exit(f"missing list '{k}' in {doc!r}")
if doc["dangling"] or doc["conflicts"]:
    sys.exit(f"unexpected dangling/conflicts: {doc['dangling']!r} {doc['conflicts']!r}")
want = sorted(json.loads(sys.argv[2]))
got = []
for v in doc["violations"]:
    if not {"rule", "entity", "message", "measured", "limit"} <= set(v):
        sys.exit(f"violation lacks spec fields: {v!r}")
    got.append(v)
got.sort(key=lambda v: (v["rule"], v["entity"]))
if len(got) != len(want):
    sys.exit(f"want {len(want)} violations {want!r}, got {got!r}")
for g, (rule, ent, msg, measured, limit) in zip(got, want):
    ok = (g["rule"] == rule and g["entity"] == ent and msg in str(g["message"])
          and math.isclose(g["measured"], measured, rel_tol=0, abs_tol=1e-12)
          and math.isclose(g["limit"], limit, rel_tol=0, abs_tol=1e-12))
    if not ok:
        sys.exit(f"violation {g!r} != expected {[rule, ent, msg, measured, limit]!r}")
