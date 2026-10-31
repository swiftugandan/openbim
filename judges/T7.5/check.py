"""T7.5 judge: run `bim bench` (release build) and enforce the spec/cli.md performance gate honestly.

Usage: python -I check.py <bim>
"""
import json
import math
import subprocess
import sys
import time

FIELDS = ["elements", "triangles", "build_ms", "incremental_ms", "frame_ms_p50", "frame_ms_p95", "raycast_us"]
FRAMES = 300  # TASKS.md T7.5: orbit + section-box moves, 300 frames


def fail(msg):
    print(f"JUDGE FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def bench(bim, n, timeout):
    t0 = time.perf_counter()
    try:
        r = subprocess.run([bim, "bench", "--elements", str(n), "--json"], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        fail(f"bim bench --elements {n} did not finish within {timeout} s")
    wall_ms = (time.perf_counter() - t0) * 1000.0
    if r.returncode not in (0, 1):
        fail(f"bim bench --elements {n} exited {r.returncode}: {r.stderr.strip()[-500:]}")
    try:
        out = json.loads(r.stdout)
    except json.JSONDecodeError:
        fail(f"bim bench --elements {n} --json did not print one JSON document: {r.stdout[:300]!r}")
    for f in FIELDS:
        v = out.get(f)
        if not isinstance(v, (int, float)) or isinstance(v, bool) or not math.isfinite(v):
            fail(f"bench --elements {n}: field {f!r} missing or not a finite number: {v!r}")
    if out["elements"] < n:
        fail(f"bench --elements {n} reported only elements={out['elements']}")
    for f in ["build_ms", "incremental_ms", "frame_ms_p50", "frame_ms_p95", "raycast_us", "triangles"]:
        if out[f] <= 0:
            fail(f"bench --elements {n}: {f} must be positive (measured), got {out[f]}")
    if out["frame_ms_p50"] > out["frame_ms_p95"]:
        fail(f"bench --elements {n}: frame_ms_p50 {out['frame_ms_p50']} > frame_ms_p95 {out['frame_ms_p95']}")
    # Exit code contract: 1 iff the budget is missed.
    missed = out["incremental_ms"] >= 50 or out["frame_ms_p95"] >= 16.6
    if r.returncode != (1 if missed else 0):
        fail(f"bench --elements {n} exited {r.returncode} but budget missed={missed}: {out}")
    # Honesty: the process must have actually spent the time it reports. Of 300 frames at least 150 take
    # >= p50 and at least 15 take >= p95, so the frames alone took >= 135*p50 + 15*p95.
    floor = out["build_ms"] + (FRAMES // 2 - FRAMES // 20) * out["frame_ms_p50"] + (FRAMES // 20) * out["frame_ms_p95"]
    if wall_ms < 0.95 * floor:
        fail(f"bench --elements {n} reports build_ms + 135*p50 + 15*p95 = {floor:.1f} ms but ran only {wall_ms:.1f} ms wall time")
    return out, wall_ms


def main():
    bim = sys.argv[1]
    # Deterministic generator: same element count -> same corpus.
    a, _ = bench(bim, 2000, 120)
    b, _ = bench(bim, 2000, 120)
    if (a["elements"], a["triangles"]) != (b["elements"], b["triangles"]):
        fail(f"generator not deterministic: {a['triangles']} vs {b['triangles']} triangles for 2000 elements")
    out, wall = bench(bim, 100_000, 300)
    print(f"bim bench --elements 100000: {json.dumps(out)} (wall {wall:.0f} ms)")
    if out["triangles"] < 2_000_000:
        fail(f"triangles {out['triangles']} < 2,000,000")
    if b["triangles"] >= out["triangles"]:
        fail("triangle count does not grow with --elements")
    if not out["incremental_ms"] < 50:
        fail(f"incremental_ms {out['incremental_ms']} >= 50")
    if not out["frame_ms_p95"] < 16.6:
        fail(f"frame_ms_p95 {out['frame_ms_p95']} >= 16.6")


if __name__ == "__main__":
    main()
