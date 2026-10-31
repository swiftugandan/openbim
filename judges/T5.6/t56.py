"""T5.6 judge helper (oracle side): BCF 3.0 with the official buildingSMART XSDs via xmlschema.

  python -I -B t56.py fixture <outdir>
      ops.json (walls/column with ULID ids) and x.bcfzip: a judge-authored BCF 3.0 archive with topics,
      comments and viewpoints whose components are those entities' IfcGuids. x.bcfzip is validated
      against the XSDs before use. expected.json records what it contains.
  python -I -B t56.py validate <f.bcfzip>
      every XML file in the archive validates against its XSD; structure checks.
  python -I -B t56.py canon <f.bcfzip>
      prints the canonical content (topics, comments, per-viewpoint component lists) as JSON.
  python -I -B t56.py same <a.bcfzip> <b.bcfzip>        canonical content equal
  python -I -B t56.py created <outdir> <f.bcfzip> <guids.txt>
      checks the two topics made with `bim bcf create` (guids.txt: the Guid each create printed).
  python -I -B t56.py issues <outdir> <bim> <dir> <f.bcfzip>
      every topic in f.bcfzip is mirrored as an Issue entity (`bim show <dir> <topic guid> --json`).

IfcGuid rule (T5.2 Details): IfcGuid = ifcopenshell.guid.compress(<EntityId as 32 hex>).
Mirror rule (T5.6 Details): the Issue's id is the topic Guid as a seed, `status` = TopicStatus,
`entities` = Refs to the viewpoint components; applied to created and imported topics alike.
"""
import datetime as dt
import io
import json
import os
import random
import subprocess
import sys
import uuid
import xml.etree.ElementTree as ET
import zipfile

import ifcopenshell.guid
import xmlschema

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
XSD = os.path.join(ROOT, ".oracles", "bcf-xsd")
CROCK = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
_schemas = {}


class Fail(Exception):
    pass


def need(c, msg):
    if not c:
        raise Fail(msg)


def schema(name):
    if name not in _schemas:
        _schemas[name] = xmlschema.XMLSchema(os.path.join(XSD, name))
    return _schemas[name]


def ulid(n):
    return "".join(CROCK[(n >> (5 * (25 - i))) & 31] for i in range(26))


def ulid_value(s):
    n = 0
    for ch in s:
        n = n * 32 + CROCK.index(ch)
    return n


def ifc_guid(ulid_str):
    return ifcopenshell.guid.compress(format(ulid_value(ulid_str), "032x"))


def xsd_for(name):
    base = name.rsplit("/", 1)[-1]
    if base == "bcf.version":
        return "version.xsd"
    if base.endswith(".bcfp"):
        return "project.xsd"
    if base == "extensions.xml":
        return "extensions.xsd"
    if base == "documents.xml":
        return "documents.xsd"
    if base.endswith(".bcf"):
        return "markup.xsd"
    if base.endswith(".bcfv"):
        return "visinfo.xsd"
    return None


def is_xml(data):
    head = data.lstrip()[:100]
    return head.startswith(b"<?xml") or head.startswith(b"<")


# ------------------------------------------------------------------------------------------- fixture
def fixture(out):
    rng = random.Random(56)
    names = ["L0", "L1", "WT", "W1", "W2", "W3", "M1", "C1"]
    ids = {n: ulid(rng.getrandbits(128)) for n in names}
    R = lambda n: {"ref": ids[n]}  # noqa: E731
    ops = [
        ("L0", "Level", {"elevation": 0.0}),
        ("L1", "Level", {"elevation": 3.0}),
        ("WT", "WallType", {"layer_thickness": [0.2], "layer_material": ["Concrete"], "layer_conductivity": [2.0]}),
        ("W1", "Wall", {"level": R("L0"), "x1": 0.0, "y1": 0.0, "x2": 5.0, "y2": 0.0, "height": 3.0, "type": R("WT")}),
        ("W2", "Wall", {"level": R("L0"), "x1": 5.0, "y1": 0.0, "x2": 5.0, "y2": 4.0, "height": 3.0, "type": R("WT")}),
        ("W3", "Wall", {"level": R("L0"), "x1": 9.0, "y1": 0.0, "x2": 12.0, "y2": 0.0, "height": 3.0, "type": R("WT")}),
        ("M1", "Material", {"name": "S355", "E": 2.1e11, "G": 8.1e10, "density": 7850.0, "fy": 3.55e8}),
        ("C1", "Column", {"x": 2.0, "y": 2.0, "base": R("L0"), "top": R("L1"), "section_b": 0.3, "section_h": 0.3,
                          "material": R("M1")}),
    ]
    json.dump({"schema": 1, "ops": [{"op": "create", "id": ids[n], "kind": k, "props": p} for n, k, p in ops]},
              open(f"{out}/ops.json", "w"), indent=1)
    U = lambda: str(uuid.UUID(int=rng.getrandbits(128), version=4))  # noqa: E731
    G = {n: ifc_guid(ids[n]) for n in names}
    topics = [
        {"guid": U(), "title": "Wall W1 clashes with column – prüfen", "type": "Clash", "status": "Open",
         "date": "2026-03-01T09:30:00+00:00", "author": "alice@example.com", "description": "Säule/Wand <&> overlap",
         "labels": ["Structural", "Architecture"],
         "comments": [{"guid": U(), "date": "2026-03-01T10:00:00+00:00", "author": "bob@example.com",
                       "text": "Confirmed on site 'north' – 2 cm", "viewpoint": 0},
                      {"guid": U(), "date": "2026-03-02T08:15:30+01:00", "author": "alice@example.com",
                       "text": "Moved column by 50 mm"}],
         "viewpoints": [{"guid": U(), "components": ["W1", "C1"], "camera": "perspective"},
                        {"guid": U(), "components": ["W2"], "camera": "orthogonal"}]},
        {"guid": U(), "title": "Missing fire rating", "type": "Issue", "status": "Closed",
         "date": "2026-04-11T14:00:00+00:00", "author": "carol@example.com",
         "comments": [{"guid": U(), "date": "2026-04-12T07:00:00+00:00", "author": "dave@example.com",
                       "text": "Rated EI60 now"}],
         "viewpoints": [{"guid": U(), "components": ["W3"], "camera": "perspective"}]},
    ]
    with zipfile.ZipFile(f"{out}/x.bcfzip", "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("bcf.version", '<?xml version="1.0" encoding="UTF-8"?>\n<Version VersionId="3.0"/>\n')
        ext = ET.Element("Extensions")
        for tag, child, vals in (("TopicTypes", "TopicType", ["Clash", "Issue"]),
                                 ("TopicStatuses", "TopicStatus", ["Open", "Closed"]),
                                 ("TopicLabels", "TopicLabel", ["Structural", "Architecture"])):
            e = ET.SubElement(ext, tag)
            for v in vals:
                ET.SubElement(e, child).text = v
        z.writestr("extensions.xml", xml_bytes(ext))
        for t in topics:
            mk = ET.Element("Markup")
            tp = ET.SubElement(mk, "Topic", Guid=t["guid"], TopicType=t["type"], TopicStatus=t["status"])
            ET.SubElement(tp, "Title").text = t["title"]
            if t.get("labels"):
                lab = ET.SubElement(tp, "Labels")
                for label in t["labels"]:
                    ET.SubElement(lab, "Label").text = label
            ET.SubElement(tp, "CreationDate").text = t["date"]
            ET.SubElement(tp, "CreationAuthor").text = t["author"]
            if t.get("description"):
                ET.SubElement(tp, "Description").text = t["description"]
            cs = ET.SubElement(tp, "Comments")
            for c in t["comments"]:
                ce = ET.SubElement(cs, "Comment", Guid=c["guid"])
                ET.SubElement(ce, "Date").text = c["date"]
                ET.SubElement(ce, "Author").text = c["author"]
                ET.SubElement(ce, "Comment").text = c["text"]
                if "viewpoint" in c:
                    ET.SubElement(ce, "Viewpoint", Guid=t["viewpoints"][c["viewpoint"]]["guid"])
            vps = ET.SubElement(tp, "Viewpoints")
            for k, v in enumerate(t["viewpoints"]):
                vp = ET.SubElement(vps, "ViewPoint", Guid=v["guid"])
                ET.SubElement(vp, "Viewpoint").text = f"{v['guid']}.bcfv"
                vi = ET.Element("VisualizationInfo", Guid=v["guid"])
                sel = ET.SubElement(ET.SubElement(vi, "Components"), "Selection")
                for n in v["components"]:
                    ET.SubElement(sel, "Component", IfcGuid=G[n])
                cam = ET.SubElement(vi, "PerspectiveCamera" if v["camera"] == "perspective" else "OrthogonalCamera")
                for tag, xyz in (("CameraViewPoint", (10.5, -4.25, 1.7)), ("CameraDirection", (-0.6, 0.8, 0.0)),
                                 ("CameraUpVector", (0.0, 0.0, 1.0))):
                    e = ET.SubElement(cam, tag)
                    for ax, val in zip("XYZ", xyz):
                        ET.SubElement(e, ax).text = repr(val)
                if v["camera"] == "perspective":
                    ET.SubElement(cam, "FieldOfView").text = "60.0"
                else:
                    ET.SubElement(cam, "ViewToWorldScale").text = "12.5"
                ET.SubElement(cam, "AspectRatio").text = "1.7777777777777777"
                z.writestr(f"{t['guid']}/{v['guid']}.bcfv", xml_bytes(vi))
            z.writestr(f"{t['guid']}/markup.bcf", xml_bytes(mk))
    validate(f"{out}/x.bcfzip")  # oracle self-check: the judge's own archive is valid BCF 3.0
    exp = {"ids": ids, "guids": G, "topics": topics,
           "create": [{"title": "Crack above door", "entities": ["W1"]},
                      {"title": "Column too close to wall W2", "entities": ["C1", "W2"]}]}
    json.dump(exp, open(f"{out}/expected.json", "w"), indent=1, ensure_ascii=False)


def xml_bytes(el):
    buf = io.BytesIO()
    ET.ElementTree(el).write(buf, encoding="UTF-8", xml_declaration=True)
    return buf.getvalue()


# ------------------------------------------------------------------------------------------- reading
def read(path):
    try:
        z = zipfile.ZipFile(path)
    except (zipfile.BadZipFile, FileNotFoundError) as e:
        raise Fail(f"{path} is not a zip archive: {e}")
    return z, {n: z.read(n) for n in z.namelist() if not n.endswith("/")}


def validate(path):
    z, files = read(path)
    need("bcf.version" in files, "bcfzip lacks bcf.version")
    for name, data in files.items():
        x = xsd_for(name)
        if x is None:
            need(not is_xml(data) or name.lower().endswith((".png", ".jpg", ".jpeg")),
                 f"XML file {name} has no BCF 3.0 schema (unexpected file)")
            continue
        try:
            schema(x).validate(io.BytesIO(data))
        except xmlschema.XMLSchemaException as e:
            raise Fail(f"{name} does not validate against {x}: {str(e)[:600]}")
    ver = ET.fromstring(files["bcf.version"])
    need(ver.get("VersionId") == "3.0", f"bcf.version VersionId {ver.get('VersionId')!r}, want 3.0")
    for name in files:
        if name.endswith("markup.bcf"):
            folder = name.rsplit("/", 1)[0] if "/" in name else ""
            topic = ET.fromstring(files[name]).find("Topic")
            need(folder == topic.get("Guid"), f"topic folder {folder!r} does not match Topic Guid {topic.get('Guid')}")
            for vp in topic.findall("Viewpoints/ViewPoint"):
                ref = vp.findtext("Viewpoint")
                if ref:
                    need(f"{folder}/{ref}" in files, f"{name}: viewpoint file {ref} missing from the archive")
    return files


def instant(s):
    d = dt.datetime.fromisoformat(s.strip().replace("Z", "+00:00"))
    if d.tzinfo is None:
        d = d.replace(tzinfo=dt.timezone.utc)
    return d.astimezone(dt.timezone.utc).isoformat()


def canon(path):
    """Canonical content per topic (T5.6 Details): Guid, title, type, status, creation date as an instant,
    author, description, labels; comments (Guid, date, author, text, viewpoint link); each viewpoint's
    component IfcGuids. Viewpoint Guids themselves are not promised stable, so a viewpoint is canonicalized
    as its sorted component list and a comment's viewpoint link as the linked viewpoint's components."""
    files = validate(path)
    topics = {}
    for name, data in files.items():
        if not name.endswith("markup.bcf"):
            continue
        folder = name.rsplit("/", 1)[0]
        tp = ET.fromstring(data).find("Topic")
        vps = {}
        for vp in tp.findall("Viewpoints/ViewPoint"):
            ref = vp.findtext("Viewpoint")
            comps = []
            if ref:
                vi = ET.fromstring(files[f"{folder}/{ref}"])
                comps = sorted(c.get("IfcGuid") for c in vi.findall("Components/Selection/Component"))
            vps[vp.get("Guid")] = comps
        comments = {}
        for c in tp.findall("Comments/Comment"):
            vp = c.find("Viewpoint")
            link = None
            if vp is not None:
                need(vp.get("Guid") in vps, f"{name}: comment {c.get('Guid')} links to unknown viewpoint {vp.get('Guid')}")
                link = vps[vp.get("Guid")]
            comments[c.get("Guid")] = {"date": instant(c.findtext("Date")), "author": c.findtext("Author"),
                                       "text": c.findtext("Comment") or "", "viewpoint": link}
        topics[tp.get("Guid")] = {
            "title": tp.findtext("Title"), "type": tp.get("TopicType"), "status": tp.get("TopicStatus"),
            "date": instant(tp.findtext("CreationDate")), "author": tp.findtext("CreationAuthor"),
            "description": tp.findtext("Description") or None,  # absent and empty are the same
            "labels": sorted(l.text for l in tp.findall("Labels/Label")),
            "comments": comments, "viewpoints": sorted(vps.values())}
    return topics


def same(a, b):
    ca, cb = canon(a), canon(b)
    need(set(ca) == set(cb), f"topic Guids differ: {sorted(ca)} vs {sorted(cb)}")
    for g in ca:
        for key in ("title", "type", "status", "date", "author", "description", "labels"):
            need(ca[g][key] == cb[g][key], f"topic {g}: {key} {ca[g][key]!r} became {cb[g][key]!r}")
        need(ca[g]["comments"] == cb[g]["comments"],
             f"topic {g}: comments changed:\n  {ca[g]['comments']}\n  {cb[g]['comments']}")
        need(ca[g]["viewpoints"] == cb[g]["viewpoints"],
             f"topic {g}: viewpoint components changed: {ca[g]['viewpoints']} vs {cb[g]['viewpoints']}")


def created(out, path, guids_file):
    exp = json.load(open(f"{out}/expected.json"))
    guids = [l.strip() for l in open(guids_file) if l.strip()]
    need(len(guids) == 2, f"bim bcf create printed {len(guids)} topic Guids, want one per create")
    topics = canon(path)
    need(set(topics) == set(guids), f"exported topics {sorted(topics)} != created {guids}")
    for g, c in zip(guids, exp["create"]):
        t = topics[g]
        need(t["title"] == c["title"], f"topic {g}: title {t['title']!r}, want {c['title']!r}")
        comps = sorted({x for v in t["viewpoints"] for x in v})
        want = sorted(exp["guids"][n] for n in c["entities"])
        need(t["viewpoints"], f"topic {g}: no viewpoint")
        need(comps == want, f"topic {g}: components {comps}, want IfcGuids {want} of {c['entities']}")


def issues(out, bim, pdir, path):
    exp = json.load(open(f"{out}/expected.json"))
    ulid_of = {exp["guids"][n]: u for n, u in exp["ids"].items()}
    for g, t in canon(path).items():
        r = subprocess.run([bim, "show", pdir, g, "--json"], capture_output=True, text=True, timeout=30)
        need(r.returncode == 0, f"no Issue entity mirrors topic {g} (`bim show` exit {r.returncode}: {r.stderr[:200]})")
        try:
            d = json.loads(r.stdout)
        except ValueError:
            raise Fail(f"`bim show {g} --json` printed invalid JSON")
        p = d.get("props", {})
        need(d.get("kind") == "Issue", f"topic {g} mirrored as {d.get('kind')}, want Issue")
        need(p.get("title") == t["title"], f"Issue {g}: title {p.get('title')!r}, want {t['title']!r}")
        need(p.get("status") == t["status"], f"Issue {g}: status {p.get('status')!r}, want {t['status']!r}")
        ents = p.get("entities")
        need(isinstance(ents, list) and all(isinstance(e, dict) and "ref" in e for e in ents),
             f"Issue {g}: entities {ents!r} must be a List of Refs")
        want = sorted({ulid_of[c] for v in t["viewpoints"] for c in v})
        need(sorted(e["ref"] for e in ents) == want, f"Issue {g}: entities {sorted(e['ref'] for e in ents)}, want {want}")


def main():
    cmd = sys.argv[1]
    try:
        if cmd == "fixture":
            fixture(sys.argv[2])
        elif cmd == "validate":
            validate(sys.argv[2])
        elif cmd == "canon":
            print(json.dumps(canon(sys.argv[2]), indent=1, ensure_ascii=False))
        elif cmd == "same":
            same(sys.argv[2], sys.argv[3])
        elif cmd == "created":
            created(*sys.argv[2:5])
        elif cmd == "issues":
            issues(*sys.argv[2:6])
        else:
            print(f"unknown command {cmd}")
            return 2
    except Fail as e:
        print(str(e))
        return 1
    return 0


sys.exit(main())
