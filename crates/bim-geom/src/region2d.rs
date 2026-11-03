//! Planar regions from directed segments under the positive fill rule (crate-internal).
//!
//! Directed segments (closed curves, possibly overlapping, touching or crossing each other) are
//! turned into the region `{p : winding(p) ≥ 1}` as simple loops: points are snapped together
//! within a tolerance, segments are split at mutual crossings and at points lying on them,
//! opposite copies of an edge cancel, and an edge is kept only where the winding number is
//! positive on exactly one side (found by casting a ray from the edge's midpoint). The kept edges
//! are walked into simple loops, counter-clockwise loops become outer boundaries and clockwise
//! loops holes of the smallest outer loop containing them.

use std::collections::HashMap;

use crate::polygon::Polygon;
use crate::predicates::orient2d;
use crate::types::Point2;

pub(crate) type P2 = [f64; 2];

/// One region: an outer loop (counter-clockwise) and its holes (clockwise); `area` is net.
pub(crate) struct Region2 {
    pub outer: Vec<P2>,
    pub holes: Vec<Vec<P2>>,
    pub area: f64,
}

/// 2D points merged within a tolerance (grid hashed).
pub(crate) struct PointSet {
    pub pts: Vec<P2>,
    pub eps: f64,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

impl PointSet {
    pub fn new(eps: f64) -> Self {
        PointSet { pts: Vec::new(), eps, grid: HashMap::new() }
    }

    fn cell(&self, p: P2) -> (i64, i64) {
        ((p[0] / self.eps).floor() as i64, (p[1] / self.eps).floor() as i64)
    }

    pub fn insert(&mut self, p: P2) -> usize {
        let (cx, cy) = self.cell(p);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(ids) = self.grid.get(&(cx + dx, cy + dy)) {
                    for &i in ids {
                        let q = self.pts[i];
                        let d = (q[0] - p[0]).hypot(q[1] - p[1]);
                        if d <= self.eps && best.map_or(true, |b| d < b.0) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }
        if let Some((_, i)) = best {
            return i;
        }
        let i = self.pts.len();
        self.pts.push(p);
        self.grid.entry((cx, cy)).or_default().push(i);
        i
    }
}

fn sub(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross2(a: P2, b: P2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot2(a: P2, b: P2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// Turn directed segments into non-overlapping edges: segments are split at mutual crossings and
/// at points lying on them, then opposite copies of an edge cancel. Returns `(a, b, m)` with
/// `a < b` and the net multiplicity `m ≠ 0` of the direction `a → b`.
fn arrange(pts: &mut PointSet, segs: Vec<(usize, usize)>) -> Vec<(usize, usize, i64)> {
    let eps = pts.eps;
    let segs: Vec<(usize, usize)> = segs.into_iter().filter(|s| s.0 != s.1).collect();

    // Proper crossings between segments (rare: only from numerical noise or touching parts).
    let bbox = |s: &(usize, usize), p: &[P2]| {
        let (a, b) = (p[s.0], p[s.1]);
        [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
    };
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by(|&i, &j| bbox(&segs[i], &pts.pts)[0].total_cmp(&bbox(&segs[j], &pts.pts)[0]));
    let mut new_pts: Vec<P2> = Vec::new();
    for oi in 0..order.len() {
        let si = segs[order[oi]];
        let bi = bbox(&si, &pts.pts);
        for &j in &order[oi + 1..] {
            let sj = segs[j];
            let bj = bbox(&sj, &pts.pts);
            if bj[0] > bi[2] + eps {
                break;
            }
            if bj[1] > bi[3] + eps || bj[3] < bi[1] - eps {
                continue;
            }
            if si.0 == sj.0 || si.0 == sj.1 || si.1 == sj.0 || si.1 == sj.1 {
                continue;
            }
            let (a, b, c, d) = (pts.pts[si.0], pts.pts[si.1], pts.pts[sj.0], pts.pts[sj.1]);
            let (ab, cd) = (sub(b, a), sub(d, c));
            let (lab, lcd) = (ab[0].hypot(ab[1]), cd[0].hypot(cd[1]));
            // Signed distances of each endpoint from the other segment's line.
            let dc = cross2(ab, sub(c, a)) / lab;
            let dd = cross2(ab, sub(d, a)) / lab;
            let da = cross2(cd, sub(a, c)) / lcd;
            let db = cross2(cd, sub(b, c)) / lcd;
            let proper = |x: f64, y: f64| (x > eps && y < -eps) || (x < -eps && y > eps);
            if proper(dc, dd) && proper(da, db) {
                let s = da / (da - db);
                new_pts.push([a[0] + ab[0] * s, a[1] + ab[1] * s]);
            }
        }
    }
    for p in new_pts {
        pts.insert(p);
    }

    // Split every segment at the points lying on it.
    let p = &pts.pts;
    let mut by_x: Vec<usize> = (0..p.len()).collect();
    by_x.sort_by(|&i, &j| p[i][0].total_cmp(&p[j][0]));
    let xs: Vec<f64> = by_x.iter().map(|&i| p[i][0]).collect();
    let mut net: HashMap<(usize, usize), i64> = HashMap::new();
    for &(ia, ib) in &segs {
        let (a, b) = (p[ia], p[ib]);
        let ab = sub(b, a);
        let l2 = dot2(ab, ab);
        let len = l2.sqrt();
        let (x0, x1) = (a[0].min(b[0]) - eps, a[0].max(b[0]) + eps);
        let (y0, y1) = (a[1].min(b[1]) - eps, a[1].max(b[1]) + eps);
        let start = xs.partition_point(|&x| x < x0);
        let mut inner: Vec<(f64, usize)> = Vec::new();
        for &k in &by_x[start..] {
            let q = p[k];
            if q[0] > x1 {
                break;
            }
            if k == ia || k == ib || q[1] < y0 || q[1] > y1 {
                continue;
            }
            let aq = sub(q, a);
            let t = dot2(aq, ab) / l2;
            if t * len <= eps || (1.0 - t) * len <= eps {
                continue;
            }
            if (cross2(ab, aq) / len).abs() <= eps {
                inner.push((t, k));
            }
        }
        inner.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut prev = ia;
        for k in inner.into_iter().map(|x| x.1).chain(std::iter::once(ib)) {
            if k != prev {
                let (key, s) = if prev < k { ((prev, k), 1) } else { ((k, prev), -1) };
                *net.entry(key).or_insert(0) += s;
                prev = k;
            }
        }
    }
    let mut edges: Vec<(usize, usize, i64)> = net.into_iter().filter(|e| e.1 != 0).map(|((a, b), m)| (a, b, m)).collect();
    edges.sort();
    edges
}

/// Keep the edges that bound `{winding ≥ 1}`, directed with that region on their left.
fn boundary_edges(pts: &[P2], edges: &[(usize, usize, i64)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (k, &(ia, ib, m)) in edges.iter().enumerate() {
        let (a, b) = (pts[ia], pts[ib]);
        let ab = sub(b, a);
        let len = ab[0].hypot(ab[1]);
        let mid = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
        // Ray from the midpoint towards the left of a -> b, in the frame (d, e) with e = rot90(d).
        let d = [-ab[1] / len, ab[0] / len];
        let e = [-d[1], d[0]];
        // Winding just left of the edge: signed crossings of the ray by every other edge
        // (the edges are non-overlapping and split at every point on them, so no other edge
        // passes through the midpoint).
        let mut w_left: i64 = 0;
        for (j, &(ja, jb, mj)) in edges.iter().enumerate() {
            if j == k {
                continue;
            }
            let (p, q) = (sub(pts[ja], mid), sub(pts[jb], mid));
            let (py, qy) = (dot2(p, e), dot2(q, e));
            if (py > 0.0) == (qy > 0.0) {
                continue;
            }
            let (px, qx) = (dot2(p, d), dot2(q, d));
            let x = px + (qx - px) * (py / (py - qy));
            if x > 0.0 {
                // Upward crossing of a ray to +d (seen in the (d, e) frame) winds positively.
                w_left += if qy > py { mj } else { -mj };
            }
        }
        let w_right = w_left - m;
        if w_left >= 1 && w_right <= 0 {
            out.push((ia, ib));
        } else if w_left <= 0 && w_right >= 1 {
            out.push((ib, ia));
        }
    }
    out
}

/// The region `{winding ≥ 1}` of the directed segments `segs` between points of `pts`. Loops
/// thinner than the snapping tolerance `pts.eps` (area at most `eps × perimeter / 2`, i.e. mean
/// width at most `eps`) are numerical slivers and dropped; the test is per loop, so small but
/// genuine regions of a large or far-away input are kept.
pub(crate) fn positive_regions(mut pts: PointSet, segs: Vec<(usize, usize)>) -> Vec<Region2> {
    let eps = pts.eps;
    let edges = arrange(&mut pts, segs);
    let edges = boundary_edges(&pts.pts, &edges);
    let loops = walk_loops(pts.pts.len(), &edges);
    build_regions(&pts.pts, loops, eps)
}

/// Whether two edges of the polygon's rings (outer and holes together) cross properly, i.e. their
/// interiors meet in a single point that is interior to both (decided exactly).
fn has_proper_crossing(rings: &[&Vec<Point2>]) -> bool {
    let mut segs: Vec<(Point2, Point2)> = Vec::new();
    for r in rings {
        for i in 0..r.len() {
            segs.push((r[i], r[(i + 1) % r.len()]));
        }
    }
    let xmin = |s: &(Point2, Point2)| s.0.x.min(s.1.x);
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by(|&i, &j| xmin(&segs[i]).total_cmp(&xmin(&segs[j])));
    for oi in 0..order.len() {
        let (a, b) = segs[order[oi]];
        let xmax = a.x.max(b.x);
        for &j in &order[oi + 1..] {
            let (c, d) = segs[j];
            if xmin(&segs[j]) > xmax {
                break;
            }
            let (o1, o2) = (orient2d(a, b, c), orient2d(a, b, d));
            let (o3, o4) = (orient2d(c, d, a), orient2d(c, d, b));
            let opposite = |x: f64, y: f64| x != 0.0 && y != 0.0 && (x > 0.0) != (y > 0.0);
            if opposite(o1, o2) && opposite(o3, o4) {
                return true;
            }
        }
    }
    false
}

/// For a polygon whose rings cross each other or themselves, the regions it bounds under the
/// positive fill rule (after orienting the outer ring counter-clockwise and the holes clockwise
/// by their signed area, as the solid constructors do), as simple polygons with holes. `None`
/// when no two edges cross properly (the polygon is used as given).
pub(crate) fn resolve_crossings(polygon: &Polygon) -> Option<Vec<Polygon>> {
    if polygon.outer.len() < 3 {
        return None;
    }
    let rings: Vec<&Vec<Point2>> = std::iter::once(&polygon.outer).chain(polygon.holes.iter().filter(|h| h.len() >= 3)).collect();
    if !has_proper_crossing(&rings) {
        return None;
    }
    // Length tolerance relative to the profile's extent, floored at a few ulps of its coordinates.
    let all = || rings.iter().flat_map(|r| r.iter());
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in all() {
        lo = [lo[0].min(p.x), lo[1].min(p.y)];
        hi = [hi[0].max(p.x), hi[1].max(p.y)];
    }
    let extent = (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
    if !(extent > 0.0 && extent.is_finite()) {
        return None;
    }
    let mag = all().fold(0.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()));
    let eps = (1e-12 * extent).max(64.0 * f64::EPSILON * mag);
    let mut pts = PointSet::new(eps);
    let mut segs = Vec::new();
    for (k, r) in rings.iter().enumerate() {
        let ids: Vec<usize> = r.iter().map(|p| pts.insert([p.x, p.y])).collect();
        let area = crate::polygon::signed_area(r);
        let flip = if k == 0 { area < 0.0 } else { area > 0.0 };
        for i in 0..ids.len() {
            let (a, b) = (ids[i], ids[(i + 1) % ids.len()]);
            segs.push(if flip { (b, a) } else { (a, b) });
        }
    }
    let to_pts = |r: &Vec<P2>| r.iter().map(|p| Point2::new(p[0], p[1])).collect::<Vec<_>>();
    Some(
        positive_regions(pts, segs)
            .iter()
            .map(|r| Polygon { outer: to_pts(&r.outer), holes: r.holes.iter().map(to_pts).collect() })
            .collect(),
    )
}

/// Walk directed edges into simple closed loops. A walk that comes back to a vertex already on
/// its path closes a loop there, so loops never touch themselves. Dangling paths (only possible
/// with an inconsistent input) are dropped.
fn walk_loops(npts: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); npts];
    for (k, &(a, _)) in edges.iter().enumerate().rev() {
        out[a].push(k);
    }
    let mut loops = Vec::new();
    let mut pos: HashMap<usize, usize> = HashMap::new();
    for s in 0..npts {
        while !out[s].is_empty() {
            let mut path = vec![s];
            pos.clear();
            pos.insert(s, 0);
            let mut cur = s;
            loop {
                let Some(e) = out[cur].pop() else {
                    break; // closed back to the start with nothing left, or a dead end
                };
                let w = edges[e].1;
                if let Some(&i) = pos.get(&w) {
                    loops.push(path[i..].to_vec());
                    for v in path.drain(i + 1..) {
                        pos.remove(&v);
                    }
                } else {
                    pos.insert(w, path.len());
                    path.push(w);
                }
                cur = w;
            }
        }
    }
    loops
}

/// Signed area of a ring, computed relative to its first vertex (translation invariant).
fn ring_area(r: &[P2]) -> f64 {
    let Some(&o) = r.first() else {
        return 0.0;
    };
    let mut s = 0.0;
    for i in 1..r.len().saturating_sub(1) {
        s += cross2(sub(r[i], o), sub(r[i + 1], o));
    }
    0.5 * s
}

/// Inside (1), outside (-1) or on the boundary (0) of a ring.
fn classify(r: &[P2], q: P2, eps: f64) -> i32 {
    let mut inside = false;
    for i in 0..r.len() {
        let (a, b) = (r[i], r[(i + 1) % r.len()]);
        let ab = sub(b, a);
        let l2 = dot2(ab, ab);
        let t = (dot2(sub(q, a), ab) / l2).clamp(0.0, 1.0);
        let c = [a[0] + ab[0] * t, a[1] + ab[1] * t];
        if sub(q, c)[0].hypot(sub(q, c)[1]) <= eps {
            return 0;
        }
        if (a[1] > q[1]) != (b[1] > q[1]) {
            let x = a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if q[0] < x {
                inside = !inside;
            }
        }
    }
    if inside {
        1
    } else {
        -1
    }
}

/// Remove vertices lying on the straight line through their neighbours.
fn simplify(mut r: Vec<P2>, eps: f64) -> Vec<P2> {
    let mut changed = true;
    while changed && r.len() >= 3 {
        changed = false;
        let mut i = 0;
        while i < r.len() && r.len() >= 3 {
            let a = r[(i + r.len() - 1) % r.len()];
            let b = r[i];
            let c = r[(i + 1) % r.len()];
            let ac = sub(c, a);
            let lac = ac[0].hypot(ac[1]);
            if lac > 0.0 && (cross2(ac, sub(b, a)) / lac).abs() <= eps && dot2(sub(b, a), sub(c, b)) > 0.0 {
                r.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    r
}

/// Classify loops by orientation and nest each hole in the smallest outer loop containing it.
fn build_regions(pts: &[P2], loops: Vec<Vec<usize>>, eps: f64) -> Vec<Region2> {
    let mut outers: Vec<(f64, Vec<P2>)> = Vec::new();
    let mut holes: Vec<(f64, Vec<P2>)> = Vec::new();
    for l in loops {
        let r = simplify(l.iter().map(|&i| pts[i]).collect(), eps);
        if r.len() < 3 {
            continue;
        }
        let a = ring_area(&r);
        let perimeter: f64 = (0..r.len()).map(|i| {
            let d = sub(r[(i + 1) % r.len()], r[i]);
            d[0].hypot(d[1])
        }).sum();
        let area_eps = 0.5 * eps * perimeter;
        if a > area_eps {
            outers.push((a, r));
        } else if a < -area_eps {
            holes.push((a, r));
        }
    }
    outers.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut regions: Vec<Region2> =
        outers.into_iter().map(|(a, r)| Region2 { outer: r, holes: Vec::new(), area: a }).collect();
    for (ha, h) in holes {
        let owner = regions.iter().position(|reg| {
            if ring_area(&reg.outer) < -ha {
                return false;
            }
            for i in 0..h.len() {
                let (a, b) = (h[i], h[(i + 1) % h.len()]);
                let m = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
                match classify(&reg.outer, m, eps) {
                    0 => continue,
                    c => return c > 0,
                }
            }
            false
        });
        if let Some(k) = owner {
            regions[k].holes.push(h);
            regions[k].area += ha;
        }
    }
    regions
}
