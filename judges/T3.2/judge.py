"""External judge for T3.2 (bim-store): SQLite store, atomic applies, revisions, commit/log/diff.

Usage: python -I judges/T3.2/judge.py <path-to-bim>
Expected values are closed-form from the ops this script authors; SQLite integrity comes from the
sqlite3 library itself (PRAGMA integrity_check), never from the code under test.
"""
import json
import os
import re
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

BIM = sys.argv[1]
TMP = tempfile.mkdtemp(prefix="judge-t32-")
HEX16 = re.compile(r"^[0-9a-f]{16}$")


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    shutil.rmtree(TMP, ignore_errors=True)
    sys.exit(1)


def bim(*args, timeout=60):
    try:
        p = subprocess.run([BIM, *map(str, args)], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        fail(f"`bim {' '.join(map(str, args))}` did not finish within {timeout}s")
    if p.returncode < 0 or p.returncode == 3:
        fail(f"`bim {' '.join(map(str, args))}` crashed (exit {p.returncode}): {p.stderr.strip()}")
    return p


def ok(*args):
    p = bim(*args)
    if p.returncode != 0:
        fail(f"`bim {' '.join(map(str, args))}` exited {p.returncode}: {p.stderr.strip()}")
    return p.stdout


def expect(code, *args):
    p = bim(*args)
    if p.returncode != code:
        fail(f"`bim {' '.join(map(str, args))}` exited {p.returncode}, want {code}: {p.stderr.strip()}")
    return p


def hash_of(d):
    h = ok("hash", d).strip()
    if not HEX16.match(h):
        fail(f"hash format: {h!r}")
    return h


def integrity(d):
    db = os.path.join(d, "bim.sqlite")
    if not os.path.isfile(db):
        fail(f"{db} does not exist")
    try:
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=10)
        rows = con.execute("PRAGMA integrity_check").fetchall()
        con.close()
    except sqlite3.Error as e:
        fail(f"{db} is not a readable SQLite database: {e}")
    if rows != [("ok",)]:
        fail(f"PRAGMA integrity_check on {db}: {rows}")


def write_ops(name, ops):
    path = os.path.join(TMP, name)
    with open(path, "w") as f:
        json.dump({"schema": 1, "ops": ops}, f)
    return path


def ref(s):
    return {"ref": s}


def wall(i, level, y, h=3.0):
    return {"op": "create", "id": i, "kind": "Wall", "props": {
        "level": ref(level), "x1": 0, "y1": y, "x2": 5, "y2": y, "height": h, "type": ref("WT")}}


BASE = [
    {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 0}},
    {"op": "create", "id": "L2", "kind": "Level", "props": {"elevation": 3.0}},
    {"op": "create", "id": "WT", "kind": "WallType", "props": {
        "layer_thickness": [0.2], "layer_material": ["concrete"], "layer_conductivity": [1.6]}},
    wall("W1", "L1", 0.0),
    wall("W3", "L1", 4.0),
]


def diff_json(d, a, b, want_exit):
    p = expect(want_exit, "diff", d, a, b, "--json")
    try:
        out = json.loads(p.stdout)
    except ValueError:
        fail(f"diff {a} {b} --json did not print JSON: {p.stdout!r}")
    for k in ("added", "removed", "changed"):
        if not isinstance(out.get(k), list):
            fail(f"diff --json lacks list {k!r}: {out}")
    for k in ("added", "removed"):
        if out[k] != sorted(out[k]):
            fail(f"diff --json {k} not sorted: {out[k]}")
    ids = [c.get("id") for c in out["changed"]]
    if ids != sorted(ids):
        fail(f"diff --json changed ids not sorted: {ids}")
    changed = {}
    for c in out["changed"]:
        keys = c.get("keys")
        if not isinstance(keys, list):
            fail(f"diff --json changed entry lacks keys list: {c}")
        user = sorted(k for k in keys if not k.startswith("_"))  # reserved props (_stamp…) may also change
        if user:
            changed[c["id"]] = user
    return {"added": out["added"], "removed": out["removed"], "changed": changed}


def check_revisions():
    a = os.path.join(TMP, "A")
    ok("init", a)
    if not os.path.isfile(os.path.join(a, "doc.oplog")):
        fail("init did not create doc.oplog")
    integrity(a)
    h0 = hash_of(a)

    ok("apply", a, write_ops("r1.json", BASE))
    h1 = hash_of(a)
    rev1 = ok("commit", a, "-m", "first").strip()
    if not HEX16.match(rev1) or rev1 != h1:
        fail(f"commit printed {rev1!r}, want the content hash {h1}")

    r2 = [
        {"op": "set", "id": "W1", "key": "height", "value": 3.5},
        {"op": "create", "id": "W2", "kind": "Wall", "props": wall("W2", "L1", 8.0)["props"]},
        {"op": "delete", "id": "W3"},
    ]
    ok("apply", a, write_ops("r2.json", r2))
    with open(os.path.join(TMP, "r2b.ops"), "w") as f:
        f.write("set L2 elevation num 3.2\n\nset W1 name text north\n")
    ok("apply", a, os.path.join(TMP, "r2b.ops"))
    h2 = hash_of(a)
    rev2 = ok("commit", a, "-m", "second revision, with spaces").strip()
    if rev2 != h2:
        fail(f"second commit printed {rev2!r}, want {h2}")

    r3 = [{"op": "set", "id": "W2", "key": "height", "value": 2.7},
          {"op": "set", "id": "W2", "key": "x2", "value": 6.0}]
    ok("apply", a, write_ops("r3.json", r3))
    rev3 = ok("commit", a, "-m", "third").strip()
    if rev3 != hash_of(a) or len({h0, rev1, rev2, rev3}) != 4:
        fail(f"revision ids not the distinct content hashes: {h0} {rev1} {rev2} {rev3}")

    # An invalid apply (bad op in the middle of a JSON file) changes nothing, log included.
    log_before = ok("log", a, "--json")
    bad = write_ops("bad.json", [{"op": "set", "id": "W1", "key": "height", "value": 9.0},
                                 {"op": "set", "id": "ghost", "key": "height", "value": 1.0}])
    expect(2, "apply", a, bad)
    if hash_of(a) != rev3:
        fail("a rejected apply changed the document")
    if ok("log", a, "--json") != log_before:
        fail("a rejected apply changed `bim log`")

    # log --json: oldest first, rev/message/ops.
    try:
        log = json.loads(ok("log", a, "--json"))
    except ValueError:
        fail("log --json did not print JSON")
    if not isinstance(log, list) or [e.get("rev") for e in log] != [rev1, rev2, rev3]:
        fail(f"log --json revs: {log!r}, want [{rev1}, {rev2}, {rev3}] oldest first")
    if [e.get("message") for e in log] != ["first", "second revision, with spaces", "third"]:
        fail(f"log --json messages: {[e.get('message') for e in log]}")
    # ops: ids (16 hex) of the ops recorded in that revision; content-addressed ops are distinct here
    # because no op is repeated. Each revision holds at least as many ops as its files contained.
    seen = set()
    for e, n_min in zip(log, (len(BASE), len(r2) + 2, len(r3))):
        ops = e.get("ops")
        if not isinstance(ops, list) or not all(isinstance(o, str) and HEX16.match(o) for o in ops):
            fail(f"log entry ops must be a list of 16-hex op ids: {e!r}")
        if len(ops) < n_min:
            fail(f"revision {e['rev']} lists {len(ops)} ops, fewer than the {n_min} applied")
        if seen & set(ops) or len(set(ops)) != len(ops):
            fail(f"op ids repeat across the log: {e!r}")
        seen |= set(ops)

    # Revision diffs: closed form from the ops above.
    d12 = diff_json(a, rev1, rev2, 1)
    want = {"added": ["W2"], "removed": ["W3"], "changed": {"L2": ["elevation"], "W1": ["height", "name"]}}
    if d12 != want:
        fail(f"diff rev1 rev2: {d12}, want {want}")
    d21 = diff_json(a, rev2, rev1, 1)
    want = {"added": ["W3"], "removed": ["W2"], "changed": {"L2": ["elevation"], "W1": ["height", "name"]}}
    if d21 != want:
        fail(f"diff rev2 rev1: {d21}, want {want}")
    d23 = diff_json(a, rev2, rev3, 1)
    if d23 != {"added": [], "removed": [], "changed": {"W2": ["height", "x2"]}}:
        fail(f"diff rev2 rev3: {d23}")
    d13 = diff_json(a, rev1, rev3, 1)
    want = {"added": ["W2"], "removed": ["W3"], "changed": {"L2": ["elevation"], "W1": ["height", "name"]}}
    if d13 != want:
        fail(f"diff rev1 rev3 (W2 was added in between, so it is only added): {d13}, want {want}")
    d22 = diff_json(a, rev2, rev2, 0)
    if d22 != {"added": [], "removed": [], "changed": {}}:
        fail(f"diff of a revision with itself: {d22}")
    expect(2, "diff", a, rev1, "0123456789abcdef", "--json")

    # The two-directory form of T1.5 still works next to the revision form.
    b = os.path.join(TMP, "B")
    ok("init", b)
    for f in ("r1.json", "r2.json", "r2b.ops", "r3.json"):
        ok("apply", b, os.path.join(TMP, f))
    p = expect(0, "diff", a, b)
    if p.stdout.strip() != "same":
        fail(f"diff <dirA> <dirB> printed {p.stdout!r}, want same")
    integrity(a)
    integrity(b)


def big_ops(n):
    ops = [{"op": "create", "id": f"K{i}", "kind": "Wall", "props": wall(f"K{i}", "L1", float(i))["props"]}
           for i in range(n)]
    return write_ops(f"big{n}.json", ops)


def check_kill_mid_apply():
    base = os.path.join(TMP, "K")
    ok("init", base)
    ok("apply", base, write_ops("k0.json", BASE))
    before = hash_of(base)

    # Size the batch so a full apply takes a measurable time on this machine.
    n, dur, after, big = 2000, 0.0, None, None
    while True:
        big = big_ops(n)
        ref_dir = os.path.join(TMP, f"Kref{n}")
        shutil.copytree(base, ref_dir)
        t = time.monotonic()
        ok("apply", ref_dir, big)
        dur = time.monotonic() - t
        after = hash_of(ref_dir)
        if dur >= 0.4 or n >= 256000:
            break
        n *= 4
    if after == before:
        fail("applying the large batch did not change the hash")

    mid_runs = 0
    for frac in (0.1, 0.25, 0.4, 0.55, 0.7, 0.85):
        d = os.path.join(TMP, f"Kkill{frac}")
        shutil.copytree(base, d)
        proc = subprocess.Popen([BIM, "apply", d, big], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(dur * frac)
        if proc.poll() is None:
            mid_runs += 1
        proc.send_signal(signal.SIGKILL)
        proc.wait(timeout=30)
        integrity(d)
        h = hash_of(d)
        if h not in (before, after):
            fail(f"apply killed at {frac:.0%} of its runtime left a partial state (hash {h}, "
                 f"want {before} or {after})")
        p = bim("validate", d)
        if p.returncode not in (0, 1):
            fail(f"validate after a killed apply exited {p.returncode}")
        if h == before:
            ok("apply", d, big)  # the store is still usable and reaches the same state
            if hash_of(d) != after:
                fail("re-running the killed apply did not reach the reference state")
            integrity(d)
    if mid_runs == 0:
        fail(f"could not kill an apply mid-run (full apply took {dur:.3f}s); the atomicity check would be vacuous")


try:
    check_revisions()
    check_kill_mid_apply()
finally:
    shutil.rmtree(TMP, ignore_errors=True)
print("T3.2 judge: all checks passed")
