//! Tests of `bim_geom::hlr`: closed-form scenes, non-generic views (edges seen end-on, coincident
//! projections, coplanar face splits, touching solids) and random scenes checked against an
//! independent ray-casting oracle that works for any closed solid.

use bim_geom::{boolean, hlr, BoolOp, HlrResult, OrthoView, Point2, Point3, Polygon, Seg2, Solid, Vec3};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point2> {
    vec![Point2::new(x0, y0), Point2::new(x1, y0), Point2::new(x1, y1), Point2::new(x0, y1)]
}

fn prism(outer: Vec<Point2>, holes: Vec<Vec<Point2>>, z0: f64, h: f64) -> Solid {
    Solid::extrude(&Polygon { outer, holes }, z0, h)
}

fn cube() -> Solid {
    prism(rect(0.0, 0.0, 1.0, 1.0), vec![], 0.0, 1.0)
}

fn view(dx: f64, dy: f64, dz: f64) -> OrthoView {
    OrthoView::new(Vec3::new(dx, dy, dz), Vec3::Z)
}

fn p3(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

fn len(s: &Seg2) -> f64 {
    ((s.b.x - s.a.x).powi(2) + (s.b.y - s.a.y).powi(2)).sqrt()
}

fn total(v: &[Seg2]) -> f64 {
    v.iter().map(len).sum()
}

fn plen(v: &OrthoView, a: Point3, b: Point3) -> f64 {
    let (pa, pb) = (v.project(a).unwrap(), v.project(b).unwrap());
    (pb.x - pa.x).hypot(pb.y - pa.y)
}

fn all_finite(r: &HlrResult) {
    for s in r.visible.iter().chain(&r.hidden) {
        assert!([s.a.x, s.a.y, s.b.x, s.b.y].iter().all(|x| x.is_finite()), "non-finite segment {s:?}");
    }
}

/// Feature edges `(a, b)` of a closed solid (dihedral angle above 1e-9), each once.
fn feature_edges(s: &Solid) -> Vec<(Point3, Point3)> {
    let normal = |f: usize| s.face_normal(f).normalized().unwrap();
    let mut out = Vec::new();
    for (h, he) in s.half_edges.iter().enumerate() {
        let t = he.twin.expect("closed solid");
        if t < h {
            continue;
        }
        let (n1, n2) = (normal(he.face), normal(s.half_edges[t].face));
        if n1.cross(n2).norm().atan2(n1.dot(n2)) > 1e-9 {
            out.push((s.vertices[he.origin].point, s.vertices[s.half_edges[he.next].origin].point));
        }
    }
    out
}

// -------------------------------------------------------------------------------------------
// an independent oracle: ray casting against triangles plus winding numbers

/// Generalised winding number of `p` with respect to the solid's triangles (1 inside, 0 outside,
/// fractions on the boundary).
fn winding(s: &Solid, p: Point3) -> f64 {
    let mut w = 0.0;
    for t in s.triangles() {
        let a = t[0] - p;
        let b = t[1] - p;
        let c = t[2] - p;
        let (la, lb, lc) = (a.norm(), b.norm(), c.norm());
        let num = a.dot(b.cross(c));
        let den = la * lb * lc + a.dot(b) * lc + b.dot(c) * la + c.dot(a) * lb;
        w += 2.0 * num.atan2(den);
    }
    w / (4.0 * std::f64::consts::PI)
}

/// Whether `m` lies on the surface of the solid (within 1e-9 of a closed triangle).
fn on_boundary(s: &Solid, m: Point3) -> bool {
    s.triangles().iter().any(|t| {
        let Some(n) = (t[1] - t[0]).cross(t[2] - t[0]).normalized() else {
            return false;
        };
        (m - t[0]).dot(n).abs() <= 1e-9 && (0..3).all(|i| (t[(i + 1) % 3] - t[i]).cross(m - t[i]).dot(n) >= -1e-9)
    })
}

/// Whether the open ray `p + s w`, `s > 0`, passes through the interior of some solid: the ray is
/// cut at every (closed) triangle it meets and the midpoint of every gap is tested by its winding
/// number (a midpoint on the surface, i.e. a ray running along a face, is not interior). Passing
/// through edges and vertices is not occlusion either.
fn occluded(solids: &[Solid], p: Point3, w: Vec3) -> bool {
    for s in solids {
        let mut hits = vec![0.0];
        for t in s.triangles() {
            let e1 = t[1] - t[0];
            let e2 = t[2] - t[0];
            let pv = w.cross(e2);
            let det = e1.dot(pv);
            if det.abs() < 1e-12 {
                continue;
            }
            let tv = p - t[0];
            let u = tv.dot(pv) / det;
            let qv = tv.cross(e1);
            let v = w.dot(qv) / det;
            let dist = e2.dot(qv) / det;
            if u >= -1e-9 && v >= -1e-9 && u + v <= 1.0 + 1e-9 && dist > 0.0 {
                hits.push(dist);
            }
        }
        hits.sort_by(|a, b| a.total_cmp(b));
        for pair in hits.windows(2) {
            let m = p + w * (0.5 * (pair[0] + pair[1]));
            if pair[1] - pair[0] > 1e-9 && !on_boundary(s, m) && winding(s, m) > 0.5 {
                return true;
            }
        }
    }
    false
}

/// A 3D supporting line with the union of the feature-edge intervals on it.
struct LineGroup {
    o: Point3,
    t: Vec3,
    union: Vec<(f64, f64)>,
}

fn line_groups(solids: &[Solid]) -> Vec<LineGroup> {
    let mut groups: Vec<(Point3, Vec3, Vec<(f64, f64)>)> = Vec::new();
    for s in solids {
        for (a, b) in feature_edges(s) {
            let t = (b - a).normalized().unwrap();
            let gi = groups
                .iter()
                .position(|g| g.1.cross(t).norm() < 1e-9 && (a - g.0).cross(g.1).norm() < 1e-9)
                .unwrap_or_else(|| {
                    groups.push((a, t, Vec::new()));
                    groups.len() - 1
                });
            let g = &mut groups[gi];
            let (s0, s1) = ((a - g.0).dot(g.1), (b - g.0).dot(g.1));
            g.2.push((s0.min(s1), s0.max(s1)));
        }
    }
    groups
        .into_iter()
        .map(|(o, t, mut iv)| {
            iv.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut union: Vec<(f64, f64)> = Vec::new();
            for (s0, s1) in iv {
                match union.last_mut() {
                    Some(l) if s0 <= l.1 + 1e-12 => l.1 = l.1.max(s1),
                    _ => union.push((s0, s1)),
                }
            }
            LineGroup { o, t, union }
        })
        .collect()
}

/// Parameters `(s0, s1)` of a 2D segment on the projected line through `o2` with direction `t2`
/// (per unit of the 3D parameter), or `None` if it is not on that line.
fn on_line(seg: &Seg2, o2: Point2, t2: Point2) -> Option<(f64, f64)> {
    let nt = (t2.x * t2.x + t2.y * t2.y).sqrt();
    let off = |q: Point2| ((q.x - o2.x) * t2.y - (q.y - o2.y) * t2.x).abs() / nt;
    if nt < 1e-12 || off(seg.a) > 1e-7 || off(seg.b) > 1e-7 {
        return None;
    }
    let par = |q: Point2| ((q.x - o2.x) * t2.x + (q.y - o2.y) * t2.y) / (nt * nt);
    let (s0, s1) = (par(seg.a), par(seg.b));
    Some((s0.min(s1), s0.max(s1)))
}

/// Checks a result the way the T2.6 judge does, for scenes in general position (no two edge
/// lines project onto one line): every segment lies on a projected feature edge of the solid it
/// is attributed to, the segments on each line do not overlap and cover the union of the edges
/// exactly, and the visibility at seven points of every segment matches the ray-casting oracle.
/// Returns the number of segments.
fn verify(solids: &[Solid], view: &OrthoView, res: &HlrResult) -> usize {
    all_finite(res);
    let [d, _, _] = view.frame().unwrap();
    let groups = line_groups(solids);
    let g2: Vec<(Point2, Point2)> = groups
        .iter()
        .map(|g| {
            let o2 = view.project(g.o).unwrap();
            let p2 = view.project(g.o + g.t).unwrap();
            (o2, Point2::new(p2.x - o2.x, p2.y - o2.y))
        })
        .collect();
    let mut per: Vec<Vec<(f64, f64)>> = vec![Vec::new(); groups.len()];
    let mut count = 0;
    for (hidden, segs) in [(false, &res.visible), (true, &res.hidden)] {
        for seg in segs {
            count += 1;
            let mut located = None;
            for (gi, &(o2, t2)) in g2.iter().enumerate() {
                if let Some((s0, s1)) = on_line(seg, o2, t2) {
                    assert!(located.is_none(), "segment {seg:?} lies on two edge lines (scene not generic)");
                    located = Some((gi, s0, s1));
                }
            }
            let (gi, s0, s1) = located.unwrap_or_else(|| panic!("segment {seg:?} is not on any edge line"));
            let g = &groups[gi];
            assert!(g.union.iter().any(|&(u0, u1)| s0 >= u0 - 1e-7 && s1 <= u1 + 1e-7), "{seg:?} beyond its edge");
            assert!(
                feature_edges(&solids[seg.solid]).iter().any(|&(a, b)| {
                    let (m0, m1) = ((a - g.o).dot(g.t), (b - g.o).dot(g.t));
                    (a - g.o).cross(g.t).norm() < 1e-9
                        && g.t.cross((b - a).normalized().unwrap()).norm() < 1e-9
                        && m1.max(m0).min(s1) - m0.min(m1).max(s0) > 1e-7
                }),
                "{seg:?} attributed to a solid without an edge there"
            );
            for f in [0.02, 0.1, 0.3, 0.5, 0.7, 0.9, 0.98] {
                let p = g.o + g.t * (s0 + f * (s1 - s0));
                let occ = occluded(solids, p, -d);
                assert_eq!(occ, hidden, "{seg:?} (reported {}) is wrong at {f}", if hidden { "hidden" } else { "visible" });
            }
            per[gi].push((s0, s1));
        }
    }
    for (gi, g) in groups.iter().enumerate() {
        let segs = &mut per[gi];
        segs.sort_by(|a, b| a.0.total_cmp(&b.0));
        for pair in segs.windows(2) {
            assert!(pair[1].0 >= pair[0].1 - 1e-7, "overlapping segments on one edge line");
        }
        let want: f64 = g.union.iter().map(|(u0, u1)| u1 - u0).sum();
        let have: f64 = segs.iter().map(|(s0, s1)| s1 - s0).sum();
        assert!((want - have).abs() < 1e-6, "edge line covered for {have} of {want}");
    }
    count
}

/// The pieces reported on the projection of the 3D edge `a`-`b`, which must not share its
/// projected line with any other edge: checks the attribution, full coverage, no overlaps and
/// the oracle at three points of every piece. Returns the visible and hidden lengths.
fn check_edge(solids: &[Solid], view: &OrthoView, res: &HlrResult, a: Point3, b: Point3, solid: usize) -> (f64, f64) {
    let [d, _, _] = view.frame().unwrap();
    let o2 = view.project(a).unwrap();
    let p2 = view.project(b).unwrap();
    let t2 = Point2::new(p2.x - o2.x, p2.y - o2.y);
    let mut pieces: Vec<(f64, f64, bool)> = Vec::new();
    for (hidden, segs) in [(false, &res.visible), (true, &res.hidden)] {
        for seg in segs {
            if let Some((s0, s1)) = on_line(seg, o2, t2) {
                if s1 < -1e-7 || s0 > 1.0 + 1e-7 {
                    continue; // on the line, beyond the edge (another collinear edge)
                }
                assert_eq!(seg.solid, solid, "{seg:?}");
                assert!(s0 >= -1e-7 && s1 <= 1.0 + 1e-7, "{seg:?} beyond the edge");
                for f in [0.1, 0.5, 0.9] {
                    let p = a + (b - a) * (s0 + f * (s1 - s0));
                    assert_eq!(occluded(solids, p, -d), hidden, "{seg:?} at {f}");
                }
                pieces.push((s0, s1, hidden));
            }
        }
    }
    pieces.sort_by(|x, y| x.0.total_cmp(&y.0));
    for pair in pieces.windows(2) {
        assert!(pair[1].0 >= pair[0].1 - 1e-7, "overlap on edge {a:?}-{b:?}");
    }
    let covered: f64 = pieces.iter().map(|(s0, s1, _)| s1 - s0).sum();
    assert!((covered - 1.0).abs() < 1e-7, "edge {a:?}-{b:?} covered for {covered}");
    let l = plen(view, a, b);
    let hid: f64 = pieces.iter().filter(|p| p.2).map(|(s0, s1, _)| (s1 - s0) * l).sum();
    (l - hid, hid)
}

/// Checks a result for scenes that need not be generic (axis-aligned elevations, where several
/// 3D edges project onto one line): the feature-edge lines are clustered by their projected
/// line; every piece must lie within the union of the edges of some 3D line projecting onto its
/// line, come from a solid with an edge there, and agree with the ray-casting oracle at seven
/// points when lifted onto that 3D line; and the total reported length on each projected line
/// must equal the sum of the projected union lengths of all 3D lines projecting onto it
/// (coincident projections are reported separately: nothing missing, nothing duplicated).
/// Returns the total visible and hidden lengths.
fn verify_lines(solids: &[Solid], view: &OrthoView, res: &HlrResult) -> (f64, f64) {
    all_finite(res);
    let [d, _, _] = view.frame().unwrap();
    let groups = line_groups(solids);
    let g2: Vec<(Point2, Point2, f64)> = groups
        .iter()
        .map(|g| {
            let o2 = view.project(g.o).unwrap();
            let p2 = view.project(g.o + g.t).unwrap();
            let t2 = Point2::new(p2.x - o2.x, p2.y - o2.y);
            (o2, t2, t2.x.hypot(t2.y))
        })
        .collect();
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for gi in 0..groups.len() {
        let (o2, t2, nt) = g2[gi];
        if nt < 1e-9 {
            continue; // seen end-on: not reported
        }
        let found = clusters.iter().position(|c| {
            let (p2, u2, nu) = g2[c[0]];
            (u2.x * t2.y - u2.y * t2.x).abs() / (nu * nt) < 1e-9 && ((o2.x - p2.x) * u2.y - (o2.y - p2.y) * u2.x).abs() / nu < 1e-7
        });
        match found {
            Some(ci) => clusters[ci].push(gi),
            None => clusters.push(vec![gi]),
        }
    }
    let mut have = vec![0.0; clusters.len()];
    let (mut vis, mut hid) = (0.0, 0.0);
    for (hidden, segs) in [(false, &res.visible), (true, &res.hidden)] {
        for seg in segs {
            let ci = clusters
                .iter()
                .position(|c| on_line(seg, g2[c[0]].0, g2[c[0]].1).is_some())
                .unwrap_or_else(|| panic!("segment {seg:?} is not on any edge line"));
            let edges = feature_edges(&solids[seg.solid]);
            let lift = clusters[ci].iter().find(|&&gi| {
                let g = &groups[gi];
                let Some((s0, s1)) = on_line(seg, g2[gi].0, g2[gi].1) else {
                    return false;
                };
                g.union.iter().any(|&(u0, u1)| s0 >= u0 - 1e-7 && s1 <= u1 + 1e-7)
                    && edges.iter().any(|&(a, b)| {
                        let (m0, m1) = ((a - g.o).dot(g.t), (b - g.o).dot(g.t));
                        (a - g.o).cross(g.t).norm() < 1e-9
                            && g.t.cross((b - a).normalized().unwrap()).norm() < 1e-9
                            && m1.max(m0).min(s1) - m0.min(m1).max(s0) > 1e-7
                    })
                    && [0.02, 0.1, 0.3, 0.5, 0.7, 0.9, 0.98].iter().all(|f| occluded(solids, g.o + g.t * (s0 + f * (s1 - s0)), -d) == hidden)
            });
            assert!(
                lift.is_some(),
                "{seg:?} (reported {}) agrees with the oracle on no edge projecting onto its line",
                if hidden { "hidden" } else { "visible" }
            );
            have[ci] += len(seg);
            if hidden {
                hid += len(seg);
            } else {
                vis += len(seg);
            }
        }
    }
    for (ci, c) in clusters.iter().enumerate() {
        let want: f64 = c.iter().map(|&gi| g2[gi].2 * groups[gi].union.iter().map(|(u0, u1)| u1 - u0).sum::<f64>()).sum();
        assert!((want - have[ci]).abs() < 1e-6, "projected line {ci}: reported length {} of {want}", have[ci]);
    }
    (vis, hid)
}

/// The visible and hidden lengths reported on the projected line through the images of `a` and
/// `b` (all pieces on that 2D line, whichever 3D edges they come from).
fn on_projected_line(view: &OrthoView, res: &HlrResult, a: Point3, b: Point3) -> (f64, f64) {
    let o2 = view.project(a).unwrap();
    let p2 = view.project(b).unwrap();
    let t2 = Point2::new(p2.x - o2.x, p2.y - o2.y);
    let sum = |segs: &[Seg2]| segs.iter().filter(|s| on_line(s, o2, t2).is_some()).map(len).sum::<f64>();
    (sum(&res.visible), sum(&res.hidden))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-7
}

// -------------------------------------------------------------------------------------------
// deterministic pseudo-random scenes

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
}

/// Convex hull (counter-clockwise) of random points around a centre.
fn convex_polygon(rng: &mut Rng, cx: f64, cy: f64, r: f64) -> Vec<Point2> {
    loop {
        let mut pts: Vec<Point2> = (0..8)
            .map(|_| {
                let a = rng.range(0.0, std::f64::consts::TAU);
                let s = rng.range(0.5, 1.0) * r;
                Point2::new((cx + s * a.cos() * 1e6).round() / 1e6, (cy + s * a.sin() * 1e6).round() / 1e6)
            })
            .collect();
        pts.sort_by(|p, q| p.x.total_cmp(&q.x).then(p.y.total_cmp(&q.y)));
        let cross = |o: Point2, a: Point2, b: Point2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
        let mut hull: Vec<Point2> = Vec::new();
        for pass in 0..2 {
            let start = hull.len();
            let it: Vec<Point2> = if pass == 0 { pts.clone() } else { pts.iter().rev().copied().collect() };
            for p in it {
                while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 1e-9 {
                    hull.pop();
                }
                hull.push(p);
            }
            hull.pop();
        }
        if hull.len() >= 3 && hull.windows(2).all(|w| (w[1] - w[0]).x.hypot((w[1] - w[0]).y) > 0.05) {
            return hull;
        }
    }
}

/// Distance between two convex counter-clockwise footprints; zero when they overlap.
fn footprint_distance(a: &[Point2], b: &[Point2]) -> f64 {
    let cross = |o: Point2, p: Point2, q: Point2| (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    let inside = |poly: &[Point2], p: Point2| (0..poly.len()).all(|i| cross(poly[i], poly[(i + 1) % poly.len()], p) >= 0.0);
    if a.iter().any(|&p| inside(b, p)) || b.iter().any(|&p| inside(a, p)) {
        return 0.0;
    }
    for i in 0..a.len() {
        for j in 0..b.len() {
            let (p, q) = (a[i], a[(i + 1) % a.len()]);
            let (r, s) = (b[j], b[(j + 1) % b.len()]);
            if cross(p, q, r) * cross(p, q, s) < 0.0 && cross(r, s, p) * cross(r, s, q) < 0.0 {
                return 0.0;
            }
        }
    }
    let seg_dist = |p: Point2, a: Point2, b: Point2| {
        let e = b - a;
        let l2 = e.x * e.x + e.y * e.y;
        let t = (((p.x - a.x) * e.x + (p.y - a.y) * e.y) / l2).clamp(0.0, 1.0);
        (p.x - a.x - t * e.x).hypot(p.y - a.y - t * e.y)
    };
    let mut d = f64::INFINITY;
    for i in 0..a.len() {
        for j in 0..b.len() {
            d = d.min(seg_dist(a[i], b[j], b[(j + 1) % b.len()]));
            d = d.min(seg_dist(b[j], a[i], a[(i + 1) % a.len()]));
        }
    }
    d
}

/// Random pairwise disjoint convex prisms (apart in plan or in height), as in the judge.
fn random_scene(rng: &mut Rng, n: usize) -> Vec<Solid> {
    let mut solids: Vec<(f64, f64, Vec<Point2>)> = Vec::new();
    while solids.len() < n {
        let z0 = (rng.range(0.0, 2.5) * 1e6).round() / 1e6;
        let h = (rng.range(0.5, 2.0) * 1e6).round() / 1e6;
        let (cx, cy, r) = (rng.range(-3.0, 3.0), rng.range(-3.0, 3.0), rng.range(0.6, 1.6));
        let poly = convex_polygon(rng, cx, cy, r);
        let ok = solids.iter().all(|o| {
            footprint_distance(&poly, &o.2) > 0.05 || z0 > o.0 + o.1 + 0.05 || o.0 > z0 + h + 0.05
        });
        if ok {
            solids.push((z0, h, poly));
        }
    }
    solids.into_iter().map(|(z0, h, p)| prism(p, vec![], z0, h)).collect()
}

fn random_view(rng: &mut Rng) -> OrthoView {
    let az = rng.range(0.0, std::f64::consts::TAU);
    let el = rng.range(0.15, 1.2);
    view(-el.cos() * az.cos(), -el.cos() * az.sin(), -el.sin())
}

// -------------------------------------------------------------------------------------------
// closed-form scenes

#[test]
fn lone_cube_from_a_corner() {
    let v = view(-0.31, -0.52, -0.79);
    let res = hlr(&[cube()], &v);
    verify(&[cube()], &v, &res);
    // Edges adjacent to a front-facing face (+x, +y, +z) are visible, the other three hidden.
    let o = p3(0.0, 0.0, 0.0);
    let hid = plen(&v, o, p3(1.0, 0.0, 0.0)) + plen(&v, o, p3(0.0, 1.0, 0.0)) + plen(&v, o, p3(0.0, 0.0, 1.0));
    let all: f64 = feature_edges(&cube()).iter().map(|&(a, b)| plen(&v, a, b)).sum();
    assert!((total(&res.hidden) - hid).abs() < 1e-9, "hidden length {}", total(&res.hidden));
    assert!((total(&res.visible) - (all - hid)).abs() < 1e-9, "visible length {}", total(&res.visible));
    assert!(res.visible.iter().chain(&res.hidden).all(|s| s.solid == 0));
}

#[test]
fn stacked_boxes_share_edges_reported_once() {
    let solids = vec![cube(), prism(rect(0.0, 0.0, 1.0, 1.0), vec![], 1.0, 1.0)];
    let v = view(-0.31, -0.52, -0.79);
    let res = hlr(&solids, &v);
    verify(&solids, &v, &res);
    // The four edges at z = 1 belong to both boxes: reported once, attributed to box 0, and the
    // vertical edges of the two boxes are merged into one line each.
    let ring = [p3(0.0, 0.0, 1.0), p3(1.0, 0.0, 1.0), p3(1.0, 1.0, 1.0), p3(0.0, 1.0, 1.0)];
    for i in 0..4 {
        check_edge(&solids, &v, &res, ring[i], ring[(i + 1) % 4], 0);
    }
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        let (a, b) = (p3(x, y, 0.0), p3(x, y, 2.0));
        let o2 = v.project(a).unwrap();
        let q2 = v.project(b).unwrap();
        let t2 = Point2::new(q2.x - o2.x, q2.y - o2.y);
        let pieces: Vec<(f64, f64)> =
            res.visible.iter().chain(&res.hidden).filter_map(|s| on_line(s, o2, t2)).collect();
        let covered: f64 = pieces.iter().map(|(s0, s1)| s1 - s0).sum();
        assert!((covered - 1.0).abs() < 1e-9, "vertical edge at ({x},{y}) covered for {covered} of its length");
    }
}

#[test]
fn box_fully_behind_another_is_entirely_hidden() {
    let v = view(-0.31, -0.52, -0.79);
    let [d, _, _] = v.frame().unwrap();
    let c = p3(0.5, 0.5, 0.5) - d * 6.0;
    let big = prism(rect(c.x - 2.0, c.y - 2.0, c.x + 2.0, c.y + 2.0), vec![], c.z - 2.0, 4.0);
    let solids = vec![cube(), big];
    let res = hlr(&solids, &v);
    verify(&solids, &v, &res);
    assert!(res.visible.iter().all(|s| s.solid == 1), "small box has visible pieces");
    let want: f64 = feature_edges(&cube()).iter().map(|&(a, b)| plen(&v, a, b)).sum();
    let have: f64 = res.hidden.iter().filter(|s| s.solid == 0).map(len).sum();
    assert!((want - have).abs() < 1e-9);
    // Of the big box only its own three back edges (at its corner furthest from the eye, which
    // lies towards +x +y +z) are hidden.
    let k = p3(c.x - 2.0, c.y - 2.0, c.z - 2.0);
    let back = plen(&v, k, p3(c.x + 2.0, k.y, k.z)) + plen(&v, k, p3(k.x, c.y + 2.0, k.z)) + plen(&v, k, p3(k.x, k.y, c.z + 2.0));
    let big_hidden: f64 = res.hidden.iter().filter(|s| s.solid == 1).map(len).sum();
    let big_all: f64 = feature_edges(&solids[1]).iter().map(|&(a, b)| plen(&v, a, b)).sum();
    let big_visible: f64 = res.visible.iter().filter(|s| s.solid == 1).map(len).sum();
    assert!((big_hidden - back).abs() < 1e-9, "{big_hidden} {back}");
    assert!((big_visible - (big_all - back)).abs() < 1e-9);
}

#[test]
fn visibility_changes_only_at_crossings() {
    // A tall box in front of a wide one (the eye lies towards +x +y +z): the wide box's edges
    // are cut where the tall box's silhouette crosses them, and nowhere else.
    let solids = vec![prism(rect(0.0, 4.0, 1.0, 5.0), vec![], 0.0, 4.0), prism(rect(-2.0, 2.0, 3.0, 3.0), vec![], 0.0, 1.0)];
    let v = view(-0.6, -0.45, -0.66);
    let res = hlr(&solids, &v);
    let n = verify(&solids, &v, &res);
    assert!(n > 24, "expected pieces beyond the 24 edges, got {n}");
    assert!(!res.hidden.is_empty() && !res.visible.is_empty());
    // The wide box's near top edge (y = 3, z = 1) is partly hidden by the tall box.
    let (vis, hid) = check_edge(&solids, &v, &res, p3(-2.0, 3.0, 1.0), p3(3.0, 3.0, 1.0), 1);
    assert!(vis > 0.0 && hid > 0.0, "{vis} {hid}");
}

// -------------------------------------------------------------------------------------------
// non-generic views

#[test]
fn axis_aligned_view_drops_end_on_edges() {
    // Looking along -x at a cube: the four x-parallel edges project to points and are dropped;
    // the front and back squares coincide. Nothing is occluded by an interior: rays from the
    // back edges run along the side faces, so both squares are visible.
    let v = view(-1.0, 0.0, 0.0);
    let res = hlr(&[cube()], &v);
    all_finite(&res);
    assert!(res.hidden.is_empty(), "{:?}", res.hidden);
    assert_eq!(res.visible.len(), 8);
    assert!((total(&res.visible) - 8.0).abs() < 1e-12);
    for s in &res.visible {
        for q in [s.a, s.b] {
            let on_square = ((q.x.abs() < 1e-12 || (q.x - 1.0).abs() < 1e-12) && (-1e-12..=1.0 + 1e-12).contains(&q.y))
                || ((q.y.abs() < 1e-12 || (q.y - 1.0).abs() < 1e-12) && (-1e-12..=1.0 + 1e-12).contains(&q.x));
            assert!(on_square, "{q:?}");
        }
    }
    // Straight down (with an up hint that is not parallel to the view direction): top and
    // bottom squares coincide, vertical edges vanish.
    let res = hlr(&[cube()], &OrthoView::new(Vec3::new(0.0, 0.0, -1.0), Vec3::Y));
    all_finite(&res);
    assert_eq!(res.visible.len() + res.hidden.len(), 8);
    assert!(res.hidden.is_empty());
}

#[test]
fn elevation_of_boxes_beside_and_behind() {
    // Elevation along +y (the eye at y = -inf). Box 2 stands behind box 0 and is hidden where
    // box 0 covers it; box 1 stands beside box 0, sharing part of a side face.
    let solids = vec![
        cube(),
        prism(rect(1.0, 0.0, 2.0, 1.0), vec![], 0.0, 0.5),
        prism(rect(0.5, 2.0, 1.5, 3.0), vec![], 0.0, 2.0),
    ];
    let v = view(0.0, 1.0, 0.0);
    let res = hlr(&solids, &v);
    all_finite(&res);
    let [d, _, _] = v.frame().unwrap();
    // Box 2's vertical edges at x = 0.5 (one at y = 2, one at y = 3; coincident in projection,
    // reported separately): hidden below z = 1, visible above.
    let line_x = v.project(p3(0.5, 2.0, 0.0)).unwrap().x;
    let at_half: Vec<(&Seg2, bool)> = res
        .visible
        .iter()
        .map(|s| (s, false))
        .chain(res.hidden.iter().map(|s| (s, true)))
        .filter(|(s, _)| s.solid == 2 && (s.a.x - line_x).abs() < 1e-12 && (s.b.x - line_x).abs() < 1e-12)
        .collect();
    let hidden_len: f64 = at_half.iter().filter(|(_, h)| *h).map(|(s, _)| len(s)).sum();
    let visible_len: f64 = at_half.iter().filter(|(_, h)| !*h).map(|(s, _)| len(s)).sum();
    assert!((hidden_len - 2.0).abs() < 1e-9, "hidden {hidden_len}");
    assert!((visible_len - 2.0).abs() < 1e-9, "visible {visible_len}");
    for (s, hidden) in &at_half {
        let (lo, hi) = (s.a.y.min(s.b.y), s.a.y.max(s.b.y));
        assert!(if *hidden { hi <= 1.0 + 1e-12 } else { lo >= 1.0 - 1e-12 }, "{s:?}");
    }
    // Every piece agrees with the ray oracle when lifted onto each edge of its solid projecting
    // onto it (coincident projections included).
    for (hidden, segs) in [(false, &res.visible), (true, &res.hidden)] {
        for s in segs {
            let mut lifted = 0;
            for (a, b) in feature_edges(&solids[s.solid]) {
                let o2 = v.project(a).unwrap();
                let q2 = v.project(b).unwrap();
                let Some((s0, s1)) = on_line(s, o2, Point2::new(q2.x - o2.x, q2.y - o2.y)) else { continue };
                if s0 < -1e-9 || s1 > 1.0 + 1e-9 {
                    continue;
                }
                lifted += 1;
                for f in [0.1, 0.5, 0.9] {
                    let p = a + (b - a) * (s0 + f * (s1 - s0));
                    assert_eq!(occluded(&solids, p, -d), hidden, "{s:?} at {f}");
                }
            }
            assert!(lifted > 0, "{s:?} is on no edge of solid {}", s.solid);
        }
    }
    // The shared vertical edge x = 1, y = 0 of boxes 0 and 1 is one merged line.
    let o2 = v.project(p3(1.0, 0.0, 0.0)).unwrap();
    let pieces: Vec<&Seg2> = res
        .visible
        .iter()
        .chain(&res.hidden)
        .filter(|s| s.solid < 2 && (s.a.x - o2.x).abs() < 1e-12 && (s.b.x - o2.x).abs() < 1e-12)
        .collect();
    // Two 3D lines project there (y = 0: boxes 0 and 1 merged; y = 1: boxes 0 and 1 merged).
    assert!((total(&pieces.iter().map(|s| **s).collect::<Vec<_>>()) - 2.0).abs() < 1e-9, "{pieces:?}");
}

#[test]
fn coplanar_face_split_is_not_an_edge_but_still_occludes() {
    // A cube whose top face is split into two coplanar quads along x = 0.5.
    let pts = vec![
        p3(0.0, 0.0, 0.0), p3(1.0, 0.0, 0.0), p3(1.0, 1.0, 0.0), p3(0.0, 1.0, 0.0), // 0..3 bottom
        p3(0.0, 0.0, 1.0), p3(1.0, 0.0, 1.0), p3(1.0, 1.0, 1.0), p3(0.0, 1.0, 1.0), // 4..7 top
        p3(0.5, 0.0, 1.0), p3(0.5, 1.0, 1.0),                                       // 8, 9 split
    ];
    let faces = vec![
        vec![vec![0, 3, 2, 1]],    // bottom
        vec![vec![4, 8, 9, 7]],    // top, left half
        vec![vec![8, 5, 6, 9]],    // top, right half
        vec![vec![0, 1, 5, 8, 4]], // y = 0
        vec![vec![2, 3, 7, 9, 6]], // y = 1
        vec![vec![1, 2, 6, 5]],    // x = 1
        vec![vec![3, 0, 4, 7]],    // x = 0
    ];
    let split = Solid::from_faces(pts, faces);
    assert!(split.is_closed_manifold());
    assert!((split.volume() - 1.0).abs() < 1e-12);
    // A tetrahedron hanging under the cube, touching its bottom face along the edge AB at
    // x = 0.5. In this view AB projects exactly onto the split line and the rays from AB leave
    // the cube through its top face right at the split, i.e. after passing through the
    // interior: AB is hidden.
    let (a, b, c, d) = (p3(0.5, 0.2, 0.0), p3(0.5, 0.8, 0.0), p3(0.9, 0.5, 0.0), p3(0.7, 0.5, -1.0));
    let tetra = Solid::from_faces(vec![a, b, c, d], vec![vec![vec![0, 2, 1]], vec![vec![0, 1, 3]], vec![vec![1, 2, 3]], vec![vec![2, 0, 3]]]);
    assert!(tetra.is_closed_manifold() && tetra.volume() > 0.0);
    let solids = vec![split, tetra];
    let v = view(0.0, 0.3, -1.0);
    let res = hlr(&solids, &v);
    all_finite(&res);
    let (vis, hid) = check_edge(&solids, &v, &res, a, b, 1);
    assert!(vis == 0.0 && (hid - plen(&v, a, b)).abs() < 1e-9, "{vis} {hid}");
    // Nothing else is reported on the split line: the split segment is not a feature edge.
    let x05 = v.project(a).unwrap().x;
    let on_split = res.visible.iter().chain(&res.hidden).filter(|s| (s.a.x - x05).abs() < 1e-12 && (s.b.x - x05).abs() < 1e-12);
    assert!(on_split.clone().all(|s| s.solid == 1), "{:?}", on_split.collect::<Vec<_>>());
    // The cube's x-parallel edges: the two collinear halves of each split top edge are merged
    // into one line; the far bottom edge's rays leave through the top at the split (hidden).
    for (y, z) in [(0.0, 1.0), (1.0, 1.0), (0.0, 0.0), (1.0, 0.0)] {
        check_edge(&solids, &v, &res, p3(0.0, y, z), p3(1.0, y, z), 0);
    }
    let [dv, _, _] = v.frame().unwrap();
    assert!(occluded(&solids, p3(0.5, 1.0, 0.0), -dv));
    let (vis, _) = check_edge(&solids, &v, &res, p3(0.0, 1.0, 0.0), p3(1.0, 1.0, 0.0), 0);
    assert_eq!(vis, 0.0);
}

#[test]
fn hole_and_non_convex_faces() {
    let mut rng = Rng(7);
    // A plate with a square hole, an L-shaped block and a peg through the hole (not touching).
    let plate = prism(rect(-2.0, -2.0, 2.0, 2.0), vec![rect(-0.5, -0.5, 0.5, 0.5)], 0.0, 0.4);
    let ell = prism(
        vec![Point2::new(3.0, 0.0), Point2::new(5.0, 0.0), Point2::new(5.0, 2.0), Point2::new(4.0, 2.0), Point2::new(4.0, 1.0), Point2::new(3.0, 1.0)],
        vec![],
        0.0,
        1.5,
    );
    let peg = prism(rect(-0.3, -0.3, 0.3, 0.3), vec![], -1.0, 3.0);
    let solids = vec![plate, ell, peg];
    for _ in 0..6 {
        let v = random_view(&mut rng);
        let res = hlr(&solids, &v);
        let n = verify(&solids, &v, &res);
        assert!(n >= 24 + 18 + 12, "{n}");
    }
}

#[test]
fn plate_with_hole_in_axis_aligned_views_hides_nothing() {
    // A lone 4x4x0.4 plate with a 1x1 through-hole. In every axis-aligned elevation and in the
    // plan views, the rays from the far edges (outer and hole) run along a cap face and through
    // the hole's air, never through the plate's open interior: nothing is hidden. The hole-loop
    // edges are convex edges of the solid, like the outer ones, even though the inner loop is
    // wound the other way.
    let plate = prism(rect(-2.0, -2.0, 2.0, 2.0), vec![rect(-0.5, -0.5, 0.5, 0.5)], 0.0, 0.4);
    assert!(plate.faces.iter().any(|f| f.loops.len() > 1));
    let solids = vec![plate];
    // Elevations: the caps give two lines of 4 + 4 + 1 + 1, entirely visible; the four pairs of
    // vertical edge lines give 2 x 0.4 each, of which the hole's four vertical edges are hidden
    // (their rays enter the plate's material beside the hole).
    for v in [view(-1.0, 0.0, 0.0), view(1.0, 0.0, 0.0), view(0.0, -1.0, 0.0), view(0.0, 1.0, 0.0)] {
        let res = hlr(&solids, &v);
        let (vis, hid) = verify_lines(&solids, &v, &res);
        assert!(close(vis, 21.6) && close(hid, 1.6), "{vis} {hid}");
        for z in [0.0, 0.4] {
            let (vis, hid) = on_projected_line(&v, &res, p3(0.0, 0.0, z), p3(0.0, 0.0, z) + v.frame().unwrap()[1]);
            assert!(close(vis, 10.0) && hid == 0.0, "{vis} {hid}");
        }
        for s in &res.hidden {
            assert!(s.a.x == s.b.x && s.a.x.abs() == 0.5, "{s:?}");
        }
    }
    // Plan views: every horizontal edge line carries two edges (z = 0 and z = 0.4).
    for v in [OrthoView::new(Vec3::new(0.0, 0.0, -1.0), Vec3::Y), OrthoView::new(Vec3::new(0.0, 0.0, 1.0), Vec3::X)] {
        let res = hlr(&solids, &v);
        let (vis, hid) = verify_lines(&solids, &v, &res);
        assert!(res.hidden.is_empty(), "{:?}", res.hidden);
        assert!(close(vis, 2.0 * (16.0 + 4.0)) && hid == 0.0, "{vis} {hid}");
    }
}

#[test]
fn box_behind_a_hole_edge_is_seen_through_the_hole() {
    // A box below the plate, seen along (0, -1, -1): the rays from its near-top edge pass
    // exactly through the bottom edge of the hole and then through the hole's air (visible);
    // its far-bottom edge projects onto the same line and is hidden by the box itself, and so
    // does that hole edge (visible). The view projects a point to (-x, (z - y) / sqrt 2).
    let plate = prism(rect(-2.0, -2.0, 2.0, 2.0), vec![rect(-0.5, -0.5, 0.5, 0.5)], 0.0, 0.4);
    let under = prism(rect(-0.3, -2.5, 0.3, -1.5), vec![], -2.0, 1.0);
    let solids = vec![plate, under];
    let v = view(0.0, -1.0, -1.0);
    let res = hlr(&solids, &v);
    verify_lines(&solids, &v, &res);
    let (vis, hid) = on_projected_line(&v, &res, p3(-0.3, -1.5, -1.0), p3(0.3, -1.5, -1.0));
    assert!(close(vis, 0.6 + 1.0) && close(hid, 0.6), "{vis} {hid}");
    // Shifted so that the rays meet the solid part of the plate instead: the near-top edge is
    // hidden too (the line now also carries the hole's top edge at y = -0.5, visible).
    let under2 = prism(rect(-0.3, -2.9, 0.3, -1.9), vec![], -2.0, 1.0);
    let solids = vec![solids[0].clone(), under2];
    let res = hlr(&solids, &v);
    verify_lines(&solids, &v, &res);
    let (vis, hid) = on_projected_line(&v, &res, p3(-0.3, -1.9, -1.0), p3(0.3, -1.9, -1.0));
    assert!(close(vis, 1.0) && close(hid, 1.2), "{vis} {hid}");
}

#[test]
fn boolean_pocket_in_elevation() {
    // A block with a pocket cut into its top by a boolean difference: the top face gets an inner
    // loop. In elevation the rim edges of the pocket and the far top edge are grazed along the top
    // face and through the pocket's air (visible); the pocket floor edges are behind the block's
    // near wall (hidden).
    let block = prism(rect(0.0, 0.0, 2.0, 1.0), vec![], 0.0, 1.0);
    let cutter = prism(rect(0.5, 0.3, 1.5, 0.7), vec![], 0.5, 1.0);
    let pocket = boolean(&block, &cutter, BoolOp::Difference).unwrap();
    assert!(pocket.is_closed_manifold());
    assert!(pocket.faces.iter().any(|f| f.loops.len() > 1), "the top face should carry the pocket as an inner loop");
    assert!((pocket.volume() - 1.8).abs() < 1e-9);
    let solids = vec![pocket];

    let v = view(-1.0, 0.0, 0.0);
    let res = hlr(&solids, &v);
    verify_lines(&solids, &v, &res);
    // Top plane z = 1: far edge (1) + near edge (1) + two rim edges (0.4 each), all visible.
    let (vis, hid) = on_projected_line(&v, &res, p3(0.0, 0.0, 1.0), p3(0.0, 1.0, 1.0));
    assert!(close(vis, 2.8) && hid == 0.0, "{vis} {hid}");
    // Pocket floor z = 0.5: two edges of 0.4, both hidden.
    let (vis, hid) = on_projected_line(&v, &res, p3(0.5, 0.3, 0.5), p3(0.5, 0.7, 0.5));
    assert!(vis == 0.0 && close(hid, 0.8), "{vis} {hid}");

    let v = view(0.0, -1.0, 0.0);
    let res = hlr(&solids, &v);
    verify_lines(&solids, &v, &res);
    let (vis, hid) = on_projected_line(&v, &res, p3(0.0, 0.0, 1.0), p3(2.0, 0.0, 1.0));
    assert!(close(vis, 6.0) && hid == 0.0, "{vis} {hid}");
    let (vis, hid) = on_projected_line(&v, &res, p3(0.5, 0.3, 0.5), p3(1.5, 0.3, 0.5));
    assert!(vis == 0.0 && close(hid, 2.0), "{vis} {hid}");

    let mut rng = Rng(5);
    for _ in 0..4 {
        let v = random_view(&mut rng);
        let res = hlr(&solids, &v);
        verify(&solids, &v, &res);
    }
}

/// A 4x4x0.4 plate with a 1x1x1 boss on top, as one solid whose top face has an inner loop around
/// the boss: the edges of that inner loop are genuinely reflex.
fn plate_with_boss() -> Solid {
    let pts = vec![
        p3(-2.0, -2.0, 0.0), p3(2.0, -2.0, 0.0), p3(2.0, 2.0, 0.0), p3(-2.0, 2.0, 0.0),
        p3(-2.0, -2.0, 0.4), p3(2.0, -2.0, 0.4), p3(2.0, 2.0, 0.4), p3(-2.0, 2.0, 0.4),
        p3(-0.5, -0.5, 0.4), p3(0.5, -0.5, 0.4), p3(0.5, 0.5, 0.4), p3(-0.5, 0.5, 0.4),
        p3(-0.5, -0.5, 1.4), p3(0.5, -0.5, 1.4), p3(0.5, 0.5, 1.4), p3(-0.5, 0.5, 1.4),
    ];
    let faces = vec![
        vec![vec![0, 3, 2, 1]],
        vec![vec![4, 5, 6, 7], vec![8, 11, 10, 9]], // top: outer CCW, boss footprint CW
        vec![vec![0, 1, 5, 4]],
        vec![vec![1, 2, 6, 5]],
        vec![vec![2, 3, 7, 6]],
        vec![vec![3, 0, 4, 7]],
        vec![vec![8, 9, 13, 12]],
        vec![vec![9, 10, 14, 13]],
        vec![vec![10, 11, 15, 14]],
        vec![vec![11, 8, 12, 15]],
        vec![vec![12, 13, 14, 15]],
    ];
    let s = Solid::from_faces(pts, faces);
    assert!(s.is_closed_manifold());
    assert!((s.volume() - 7.4).abs() < 1e-9, "{}", s.volume());
    s
}

#[test]
fn reflex_inner_loop_edges_of_a_boss_on_a_plate() {
    // Elevation along -x, on the projected line z = 0.4: the near plate edge (4) and the near
    // boss-base edge (1) are visible; the far boss-base edge (1) is hidden, since its rays enter
    // the solid under the boss at once, and the far plate edge is hidden exactly behind the boss
    // (1 of 4). With the inner loop mistaken for an outer one, both far edges come out visible.
    let mut variants = vec![plate_with_boss()];
    let plate = prism(rect(-2.0, -2.0, 2.0, 2.0), vec![], 0.0, 0.4);
    let boss = prism(rect(-0.5, -0.5, 0.5, 0.5), vec![], 0.4, 1.0);
    if let Ok(u) = boolean(&plate, &boss, BoolOp::Union) {
        if u.is_closed_manifold() && (u.volume() - 7.4).abs() < 1e-9 {
            variants.push(u);
        }
    }
    assert!(variants[0].faces.iter().any(|f| f.loops.len() > 1));
    for (i, s) in variants.into_iter().enumerate() {
        let solids = vec![s];
        for v in [view(-1.0, 0.0, 0.0), view(1.0, 0.0, 0.0), view(0.0, -1.0, 0.0), view(0.0, 1.0, 0.0)] {
            let res = hlr(&solids, &v);
            verify_lines(&solids, &v, &res);
            let (vis, hid) = on_projected_line(&v, &res, p3(0.0, 0.0, 0.4), p3(0.0, 0.0, 0.4) + v.frame().unwrap()[1]);
            assert!(close(vis, 8.0) && close(hid, 2.0), "variant {i}, view {:?}: {vis} {hid}", v.dir);
        }
        // Plan view from above: the rays from the plate's bottom edges and from the boss base
        // edges run along the plate's and the boss's walls, so everything is visible (every
        // horizontal edge line carries two coincident edges).
        let v = OrthoView::new(Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
        let res = hlr(&solids, &v);
        let (vis, hid) = verify_lines(&solids, &v, &res);
        assert!(close(vis, 32.0 + 8.0) && hid == 0.0, "variant {i}: {vis} {hid}");
        let mut rng = Rng(13);
        for _ in 0..4 {
            let v = random_view(&mut rng);
            let res = hlr(&solids, &v);
            verify(&solids, &v, &res);
        }
    }
}

#[test]
fn boolean_union_with_merged_coplanar_faces() {
    // Two overlapping boxes united: a non-convex solid whose coplanar faces are merged.
    let a = prism(rect(0.0, 0.0, 2.0, 1.0), vec![], 0.0, 1.0);
    let b = prism(rect(1.0, 0.0, 3.0, 2.0), vec![], 0.0, 1.0);
    let u = boolean(&a, &b, BoolOp::Union).unwrap();
    assert!(u.is_closed_manifold());
    let other = prism(rect(0.5, 3.0, 2.5, 4.0), vec![], 0.0, 2.0);
    let solids = vec![u, other];
    let mut rng = Rng(11);
    for _ in 0..6 {
        let v = random_view(&mut rng);
        let res = hlr(&solids, &v);
        verify(&solids, &v, &res);
    }
}

#[test]
fn touching_solids_do_not_occlude_each_other_at_the_contact() {
    // Boxes on a slab, one pair sharing part of a side face.
    let slab = prism(rect(-2.0, -2.0, 2.0, 2.0), vec![], 0.0, 0.5);
    let box_a = prism(rect(-1.0, -1.0, 0.0, 0.0), vec![], 0.5, 1.0);
    let box_b = prism(rect(0.0, -1.0, 1.0, 0.5), vec![], 0.5, 0.8);
    let solids = vec![slab, box_a, box_b];
    let mut rng = Rng(3);
    for _ in 0..8 {
        let v = random_view(&mut rng);
        let res = hlr(&solids, &v);
        verify(&solids, &v, &res);
    }
}

#[test]
fn random_prism_scenes_agree_with_ray_casting() {
    let mut rng = Rng(20250206);
    for i in 0..10 {
        let n = 3 + (rng.next() * 4.0) as usize;
        let solids = random_scene(&mut rng, n);
        let v = random_view(&mut rng);
        let res = hlr(&solids, &v);
        let count = verify(&solids, &v, &res);
        assert!(count > 0, "scene {i} produced nothing");
    }
}

#[test]
fn degenerate_inputs() {
    assert_eq!(hlr(&[], &view(0.0, 0.0, -1.0)), HlrResult::default());
    assert_eq!(hlr(&[cube()], &OrthoView::new(Vec3::ZERO, Vec3::Z)), HlrResult::default());
    assert_eq!(hlr(&[cube()], &OrthoView::new(Vec3::Z, Vec3::Z)), HlrResult::default());
    assert_eq!(hlr(&[Solid::new()], &view(0.0, 0.0, -1.0)), HlrResult::default());
    assert!(OrthoView::new(Vec3::new(f64::NAN, 0.0, 0.0), Vec3::Z).frame().is_none());
    // The projection convention: r = normalize(dir × up), v = r × normalize(dir).
    let v = view(0.0, 1.0, 0.0);
    let q = v.project(p3(2.0, 5.0, 3.0)).unwrap();
    assert!((q.x - 2.0).abs() < 1e-15 && (q.y - 3.0).abs() < 1e-15, "{q:?}");
}
