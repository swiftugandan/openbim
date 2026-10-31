"""External judge for T3.3 (bim-sync): WebSocket state sync between peers through one server.

Usage: python -I judges/T3.3/judge.py <path-to-bim>

Three project dirs share a base, are edited offline (concurrent sets of one key, a delete under a
reference, a new reference to the deleted entity, disjoint creates), then synced through one server
in several orders, concurrently, and across a killed-and-restarted server. Every run starts from a
byte copy of the same offline state, so CRDT peer ids are identical across runs and the final hash
must be the same whatever the order. Every process is bounded by a timeout; servers are always killed.
"""
import json
import os
import re
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

BIM = sys.argv[1]
TMP = tempfile.mkdtemp(prefix="judge-t33-")
CMD_TIMEOUT = 30
SERVERS = []


class JudgeFail(Exception):
    pass


def fail(msg):
    raise JudgeFail(msg)


def run(*args, timeout=CMD_TIMEOUT):
    try:
        return subprocess.run([BIM, *map(str, args)], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        fail(f"`bim {' '.join(map(str, args))}` did not finish within {timeout}s")


def ok(*args):
    p = run(*args)
    if p.returncode != 0:
        fail(f"`bim {' '.join(map(str, args))}` exited {p.returncode}: {p.stderr.strip()[:500]}")
    return p.stdout


def hash_of(d):
    hx = ok("hash", d).strip()
    if not re.fullmatch(r"[0-9a-f]{16}", hx):
        fail(f"hash format {hx!r}")
    return hx


def validate(d):
    p = run("validate", d)
    if p.returncode not in (0, 1):
        fail(f"validate {d} exited {p.returncode}: {p.stderr.strip()[:300]}")
    return p.returncode, sorted(l for l in p.stdout.splitlines() if l.strip())


class Server:
    def __init__(self, d):
        self.d = d
        self.proc = subprocess.Popen([BIM, "sync", "serve", d, "--port", "0"],
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        SERVERS.append(self)
        line = self._first_line(20)
        m = re.fullmatch(r"listening (\d+)\s*", line)
        if not m:
            self.kill()
            fail(f"`bim sync serve` did not print `listening <port>` first within 20s (got {line!r})")
        self.port = int(m.group(1))
        if self.port == 0:
            fail("server announced port 0")
        # Keep draining stdout so a chatty server never blocks on a full pipe.
        threading.Thread(target=lambda: [None for _ in iter(lambda: self.proc.stdout.read(4096), b"")],
                         daemon=True).start()

    def _first_line(self, secs):
        """First stdout line, read without ever blocking past the deadline."""
        fd, buf = self.proc.stdout.fileno(), b""
        deadline = time.monotonic() + secs
        while b"\n" not in buf:
            left = deadline - time.monotonic()
            if left <= 0:
                break
            r, _, _ = select.select([fd], [], [], left)
            if not r:
                break
            chunk = os.read(fd, 1)  # byte-wise: nothing after the newline is consumed here
            if not chunk:
                break
            buf += chunk
        return buf.decode(errors="replace")

    @property
    def url(self):
        return f"ws://127.0.0.1:{self.port}"

    def alive(self):
        return self.proc.poll() is None

    def kill(self):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGKILL)
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            pass


def sync(d, srv):
    ok("sync", d, srv.url)
    if not srv.alive():
        fail(f"server died during sync of {os.path.basename(d)}")


def sync_concurrently(dirs, srv):
    procs = [subprocess.Popen([BIM, "sync", d, srv.url], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
             for d in dirs]
    for d, p in zip(dirs, procs):
        try:
            _, err = p.communicate(timeout=CMD_TIMEOUT)
        except subprocess.TimeoutExpired:
            for q in procs:
                q.kill()
            fail(f"concurrent `bim sync {os.path.basename(d)}` did not finish within {CMD_TIMEOUT}s")
        if p.returncode != 0:
            fail(f"concurrent `bim sync {os.path.basename(d)}` exited {p.returncode}: {err.decode()[:300]}")
    if not srv.alive():
        fail("server died during concurrent syncs")


def write(name, text):
    path = os.path.join(TMP, name)
    with open(path, "w") as f:
        f.write(text)
    return path


def ops_json(name, ops):
    return write(name, json.dumps({"schema": 1, "ops": ops}))


def show(d, i):
    p = run("show", d, i, "--json")
    if p.returncode != 0:
        return None
    try:
        return json.loads(p.stdout)
    except ValueError:
        fail(f"show {i} --json printed non-JSON: {p.stdout[:200]!r}")


PEERS = ("A", "B", "C")


def setup():
    """Base state on A, distributed through a server; then offline edits on A, B, C. Returns snapshot dir."""
    snap = os.path.join(TMP, "snap")
    os.makedirs(snap)  # `bim init <dir>` is only promised to create <dir>, not its parents
    for n in ("S",) + PEERS:
        ok("init", os.path.join(snap, n))
    S, A, B, C = (os.path.join(snap, n) for n in ("S",) + PEERS)
    ok("apply", A, ops_json("base.json", [
        {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 0}},
        {"op": "create", "id": "L2", "kind": "Level", "props": {"elevation": 3.0}},
        {"op": "create", "id": "WT", "kind": "WallType", "props": {
            "layer_thickness": [0.2], "layer_material": ["brick"], "layer_conductivity": [0.8]}},
        {"op": "create", "id": "W1", "kind": "Wall", "props": {
            "level": {"ref": "L1"}, "x1": 0, "y1": 0, "x2": 5, "y2": 0, "height": 3.0, "type": {"ref": "WT"}}},
    ]))
    srv = Server(S)
    try:
        # The server speaks WebSocket and survives a client that connects and leaves without a word.
        from websockets.sync.client import connect
        try:
            with connect(srv.url, open_timeout=10, close_timeout=5):
                pass
        except Exception as e:  # noqa: BLE001 - any handshake failure is a contract failure
            fail(f"server at {srv.url} did not accept a WebSocket handshake: {e}")
        for d in (A, B, C):
            sync(d, srv)
        h = hash_of(A)
        if hash_of(B) != h or hash_of(C) != h:
            fail("after distributing the base, peers' hashes differ")
        sync(A, srv)
        if hash_of(A) != h:
            fail("re-syncing an up-to-date peer changed its hash")
    finally:
        srv.kill()
    if hash_of(S) != h:
        fail("server dir does not hold the synced state")

    # Offline edits (servers down). A, B and C all set W1.height (same entity, same key);
    # A deletes L1 while W1 refers to it and B adds a new reference to it; disjoint creates.
    ok("apply", A, write("a.ops", "set W1 height num 3.5\ndelete L1\ncreate P1 Space\nset P1 name text lobby\n"))
    ok("apply", B, write("b.ops", "set W1 height num 4\ncreate W2 Wall\nset W2 level ref L1\n"
                                  "set W2 height num 2.5\nset L2 elevation num 3.2\n"))
    ok("apply", C, write("c.ops", "set W1 height num 5\nset W1 name text north\nset L1 elevation num 0.5\n"
                                  "create D1 Opening\nset D1 host ref W1\nset D1 width num 0.9\n"))
    if len({hash_of(A), hash_of(B), hash_of(C)}) != 3:
        fail("offline edits did not diverge the peers")
    return snap


def fresh_copy(snap, name):
    d = os.path.join(TMP, name)
    shutil.copytree(snap, d)
    return {n: os.path.join(d, n) for n in ("S",) + PEERS}


def final_state(dirs, label):
    hashes = {n: hash_of(d) for n, d in dirs.items()}
    if len(set(hashes.values())) != 1:
        fail(f"{label}: peers did not converge: {hashes}")
    vals = {n: validate(d) for n, d in dirs.items()}
    if len({json.dumps(v) for v in vals.values()}) != 1:
        fail(f"{label}: diagnostics differ between peers: {vals}")
    return hashes["A"], vals["A"]


def converge(dirs, srv, order, label):
    """Two passes in `order`, then a third idempotent pass that must not change anything."""
    for _ in range(2):
        for n in order:
            sync(dirs[n], srv)
    before = {n: hash_of(dirs[n]) for n in PEERS}
    for n in order:
        sync(dirs[n], srv)
    if {n: hash_of(dirs[n]) for n in PEERS} != before:
        fail(f"{label}: an extra sync round changed a converged peer")


def scenario_order(snap, order):
    label = "order " + ",".join(order)
    dirs = fresh_copy(snap, "run-" + "".join(order))
    srv = Server(dirs["S"])
    try:
        converge(dirs, srv, order, label)
    finally:
        srv.kill()
    return final_state(dirs, label), dirs


def scenario_concurrent(snap):
    dirs = fresh_copy(snap, "run-concurrent")
    srv = Server(dirs["S"])
    try:
        sync_concurrently([dirs[n] for n in PEERS], srv)
        sync_concurrently([dirs[n] for n in PEERS], srv)
        for n in PEERS:  # a final sequential pass reaches every peer regardless of interleaving
            sync(dirs[n], srv)
        for n in PEERS:
            sync(dirs[n], srv)
    finally:
        srv.kill()
    return final_state(dirs, "concurrent syncs")


def scenario_restart(snap):
    dirs = fresh_copy(snap, "run-restart")
    srv = Server(dirs["S"])
    try:
        sync(dirs["A"], srv)
        sync(dirs["B"], srv)
    finally:
        srv.kill()  # SIGKILL: whatever a completed sync delivered must already be durable
    srv = Server(dirs["S"])
    try:
        sync(dirs["C"], srv)
        for n in PEERS:
            sync(dirs[n], srv)
        for n in PEERS:
            sync(dirs[n], srv)
    finally:
        srv.kill()
    return final_state(dirs, "server restart mid-session")


def check_semantics(dirs):
    """Closed-form CRDT outcomes that hold for any peer ids."""
    for n in ("S",) + PEERS:
        d = dirs[n]
        w1 = show(d, "W1")
        if w1 is None:
            fail(f"{n}: W1 is missing after sync")
        props = w1.get("props", {})
        if props.get("height") not in (3.5, 4, 4.0, 5, 5.0):
            fail(f"{n}: W1.height is {props.get('height')!r}, want one of the concurrent values 3.5/4/5")
        if props.get("name") != "north":
            fail(f"{n}: W1.name is {props.get('name')!r}, want 'north' (C's set is the only write to that key)")
        if show(d, "L1") is not None:
            fail(f"{n}: L1 is live; A's delete (stamp above L1's create) must win over C's concurrent set")
        for i, key, want in (("W2", "height", 2.5), ("D1", "width", 0.9), ("L2", "elevation", 3.2)):
            e = show(d, i)
            if e is None or e.get("props", {}).get(key) != want:
                fail(f"{n}: {i}.{key} should be {want} after sync, got {e!r}")
        p1 = show(d, "P1")
        if p1 is None or p1.get("props", {}).get("name") != "lobby":
            fail(f"{n}: P1 missing or wrong after sync: {p1!r}")


def check_unreachable_server(snap):
    d = fresh_copy(snap, "run-dead")["A"]
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()  # nothing listens on this port now
    h = hash_of(d)
    p = run("sync", d, f"ws://127.0.0.1:{port}")
    if p.returncode == 0:
        fail("`bim sync` to a port with no server exited 0")
    if hash_of(d) != h:
        fail("a failed sync changed the local document")


def main():
    snap = setup()
    (h1, v1), dirs1 = scenario_order(snap, ("A", "B", "C"))
    (h2, v2), _ = scenario_order(snap, ("C", "A", "B"))
    (h3, v3), _ = scenario_order(snap, ("B", "C", "A"))
    h4, v4 = scenario_concurrent(snap)
    h5, v5 = scenario_restart(snap)
    results = {"A,B,C": h1, "C,A,B": h2, "B,C,A": h3, "concurrent": h4, "restart": h5}
    if len(set(results.values())) != 1:
        fail(f"final state depends on sync order / restart: {results}")
    if len({json.dumps(v) for v in (v1, v2, v3, v4, v5)}) != 1:
        fail("final diagnostics depend on sync order / restart")
    code, lines = v1
    if code != 1 or not lines:
        fail(f"W1 and W2 refer to the deleted L1, so validate must report dangling refs and exit 1, got {code} {lines}")
    check_semantics(dirs1)
    check_unreachable_server(snap)


try:
    main()
    print("T3.3 judge: all checks passed")
    code = 0
except JudgeFail as e:
    print(f"JUDGE FAIL: {e}", file=sys.stderr)
    code = 1
finally:
    for s in SERVERS:
        s.kill()
    shutil.rmtree(TMP, ignore_errors=True)
sys.exit(code)
