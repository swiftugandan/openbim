//! Tributary areas: the nearest-beam partition of a slab's net plan region.
//!
//! Every point of the slab's net region (outer boundary minus openings) belongs to the
//! supporting segment nearest to it (plan distance), and its load travels to the nearest point
//! of that segment. For a segment `A → B` of length `L` this splits the slab into
//!
//! * a **strip**: points whose nearest point is interior to the segment, i.e. that lie on the
//!   perpendicular through some station `s ∈ [0, L]`. `width(s)` is the length of the part of
//!   that perpendicular (both sides) that is in the net region and nearer to this segment than to
//!   every other one. The strip's area is `∫ width(s) ds` and a uniform pressure `q` reaches the
//!   segment as the line load `q · width(s)`;
//! * **caps**: points whose nearest point is an end node (a slab overhanging a beam end). Their
//!   load goes to that node; the area is credited to the lowest-index segment ending there.
//!
//! Along any line `P(t) = p0 + t·d` (|d| = 1) the set of points in the net region is a union of
//! intervals (exact edge crossings), and the condition "no nearer than another segment",
//! `dist²(P(t), seg) − t² ≥ 0`, is piecewise linear (where the foot on the other segment is
//! clamped to an end) or quadratic (where it is interior) in `t`, so it is also an exact union
//! of intervals. Widths are therefore exact; only the integration over `s` (or, for caps, over
//! the angle around the node) is numerical: adaptive bisection seeded with every station where
//! a vertex or another segment's end projects, refined until the trapezoid rule's local error
//! is below 1e-10 of the slab size. The width samples *are* the reported profile, so the line
//! loads integrate exactly to `q × area`.
//!
//! Ties (points equidistant from two segments over an area, e.g. duplicated segments) go to the
//! segment with the lower index.

pub(crate) type P2 = [f64; 2];

/// A supporting segment in plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segment {
    pub a: P2,
    pub b: P2,
}

/// The nearest-segment partition of one slab.
#[derive(Debug, Clone, Default)]
pub(crate) struct Partition {
    /// Per segment: `(station, width)` samples, stations ascending from 0 to the segment's
    /// length; the width is linear between samples. Empty when the strip has no area.
    pub profiles: Vec<Vec<(f64, f64)>>,
    /// Regions nearest to a segment end: `(segment credited, end (0 = a, 1 = b), area)`.
    pub caps: Vec<(usize, usize, f64)>,
}

impl Partition {
    /// Area of the strip of segment `k` (trapezoid rule over its samples, exact for the profile).
    pub fn strip_area(&self, k: usize) -> f64 {
        profile_integral(&self.profiles[k])
    }

    /// Strip area plus the cap areas credited to segment `k`.
    pub fn area(&self, k: usize) -> f64 {
        self.strip_area(k) + self.caps.iter().filter(|c| c.0 == k).map(|c| c.2).sum::<f64>()
    }
}

/// `∫ width ds` of a piecewise-linear profile.
pub(crate) fn profile_integral(p: &[(f64, f64)]) -> f64 {
    p.windows(2).map(|w| 0.5 * (w[1].0 - w[0].0) * (w[0].1 + w[1].1)).sum()
}

/// A sorted union of disjoint closed intervals.
type Set = Vec<(f64, f64)>;

fn sub(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: P2, b: P2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: P2, b: P2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn norm(a: P2) -> f64 {
    dot(a, a).sqrt()
}

fn intersect(a: &Set, b: &Set) -> Set {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo < hi {
            out.push((lo, hi));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn subtract(a: &Set, b: &Set) -> Set {
    let mut out = Vec::new();
    for &(lo, hi) in a {
        let mut cur = lo;
        for &(blo, bhi) in b {
            if bhi <= cur || blo >= hi {
                continue;
            }
            if blo > cur {
                out.push((cur, blo));
            }
            cur = cur.max(bhi);
            if cur >= hi {
                break;
            }
        }
        if cur < hi {
            out.push((cur, hi));
        }
    }
    out
}

fn measure(s: &Set) -> f64 {
    s.iter().map(|(a, b)| b - a).sum()
}

/// The parameters `t` where the line `p0 + t·d` is inside `poly` (even–odd crossings, a vertex
/// on the line counted on the non-positive side, so grazing lines stay consistent).
fn polygon_set(poly: &[P2], p0: P2, d: P2) -> Set {
    let n = poly.len();
    let mut ts = Vec::new();
    for k in 0..n {
        let (u, v) = (sub(poly[k], p0), sub(poly[(k + 1) % n], p0));
        let (w1, w2) = (cross(d, u), cross(d, v));
        if (w1 > 0.0) != (w2 > 0.0) {
            let (t1, t2) = (dot(d, u), dot(d, v));
            ts.push(t1 + (t2 - t1) * w1 / (w1 - w2));
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.chunks_exact(2).filter(|c| c[0] < c[1]).map(|c| (c[0], c[1])).collect()
}

/// The slab's net region along a line.
struct Region<'a> {
    outer: &'a [P2],
    holes: &'a [Vec<P2>],
}

impl Region<'_> {
    fn along(&self, p0: P2, d: P2) -> Set {
        let mut s = polygon_set(self.outer, p0, d);
        for h in self.holes {
            if s.is_empty() {
                break;
            }
            s = subtract(&s, &polygon_set(h, p0, d));
        }
        s
    }
}

/// Roots of `qa t² + qb t + qc` (any that exist), unordered.
fn roots(qa: f64, qb: f64, qc: f64) -> Vec<f64> {
    if qa == 0.0 {
        return if qb != 0.0 { vec![-qc / qb] } else { Vec::new() };
    }
    let disc = qb * qb - 4.0 * qa * qc;
    if disc < 0.0 {
        return Vec::new();
    }
    let q = -0.5 * (qb + qb.signum() * disc.sqrt());
    if q == 0.0 {
        return vec![0.0];
    }
    vec![q / qa, qc / q]
}

/// The parameters `t ∈ [lo, hi]` where the point `p0 + t·d` is not nearer to segment `seg`
/// than `|t|` (strictly farther when `strict`). `scale` is the problem's length scale, used
/// to recognise an identically zero difference (a duplicated segment).
fn not_nearer(seg: &Segment, p0: P2, d: P2, lo: f64, hi: f64, strict: bool, scale: f64) -> Set {
    let ab = sub(seg.b, seg.a);
    let len = norm(ab);
    let mut out: Set = Vec::new();
    let mut push = |a: f64, b: f64| {
        if b <= a {
            return;
        }
        if let Some(last) = out.last_mut() {
            if a <= last.1 {
                last.1 = last.1.max(b);
                return;
            }
        }
        out.push((a, b));
    };
    if len == 0.0 {
        // A point "segment": distance to its end.
        let e = sub(p0, seg.a);
        let (qa, qb, qc) = (0.0, 2.0 * dot(d, e), dot(e, e));
        sign_pieces(qa, qb, qc, lo, hi, strict, scale, &mut push);
        return out;
    }
    let v = [ab[0] / len, ab[1] / len];
    let m = [-v[1], v[0]];
    let pa = sub(p0, seg.a);
    let (s0, s1) = (dot(pa, v), dot(d, v));
    // Regime breaks where the foot crosses the segment's ends.
    let mut cuts = vec![lo, hi];
    if s1 != 0.0 {
        for t in [-s0 / s1, (len - s0) / s1] {
            if t > lo && t < hi {
                cuts.push(t);
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b <= a {
            continue;
        }
        let sigma = s0 + s1 * 0.5 * (a + b);
        let (qa, qb, qc) = if sigma < 0.0 || sigma > len {
            // Foot clamped at an end E: |p0 − E + t d|² − t².
            let e = if sigma < 0.0 { pa } else { sub(p0, seg.b) };
            (0.0, 2.0 * dot(d, e), dot(e, e))
        } else {
            // Interior foot: (h0 + t h1)² − t².
            let (h0, h1) = (dot(pa, m), dot(d, m));
            (h1 * h1 - 1.0, 2.0 * h0 * h1, h0 * h0)
        };
        sign_pieces(qa, qb, qc, a, b, strict, scale, &mut push);
    }
    out
}

/// Calls `push` with the sub-intervals of `[a, b]` where `f = qa t² + qb t + qc` is positive
/// (non-negative unless `strict`).
#[allow(clippy::too_many_arguments)]
fn sign_pieces(qa: f64, qb: f64, qc: f64, a: f64, b: f64, strict: bool, scale: f64, push: &mut impl FnMut(f64, f64)) {
    let tiny = 1e-12;
    // Identically zero (up to round-off) only for coincident segments: `qc` is a squared
    // distance, so it is compared with the square of the length tolerance.
    if qa.abs() <= tiny && qb.abs() <= tiny * scale && qc.abs() <= (tiny * scale) * (tiny * scale) {
        if !strict {
            push(a, b);
        }
        return;
    }
    let mut cuts = vec![a, b];
    cuts.extend(roots(qa, qb, qc).into_iter().filter(|&t| t > a && t < b));
    cuts.sort_by(f64::total_cmp);
    for w in cuts.windows(2) {
        let t = 0.5 * (w[0] + w[1]);
        let f = (qa * t + qb) * t + qc;
        if f > 0.0 || (!strict && f == 0.0) {
            push(w[0], w[1]);
        }
    }
}

/// Adaptive sampling of `f` on `[a, b]` seeded with `knots` (sorted, including both ends):
/// returns `(x, f(x))` samples such that linear interpolation between them integrates `f` to
/// within `tol_len` per unit length (plus `floor` per interval at a discontinuity).
fn sample(f: &impl Fn(f64) -> f64, knots: &[f64], tol_len: f64, floor: f64) -> Vec<(f64, f64)> {
    fn refine(
        f: &impl Fn(f64) -> f64,
        a: (f64, f64),
        b: (f64, f64),
        depth: u32,
        tol_len: f64,
        floor: f64,
        out: &mut Vec<(f64, f64)>,
    ) {
        let h = b.0 - a.0;
        let m = 0.5 * (a.0 + b.0);
        if depth == 0 || m <= a.0 || m >= b.0 {
            return;
        }
        let fm = f(m);
        let err = 0.25 * h * (2.0 * fm - a.1 - b.1).abs();
        if err <= (tol_len * h).max(floor) && depth <= 58 {
            // Accept, but keep the midpoint: it is exact information.
            out.push((m, fm));
            return;
        }
        refine(f, a, (m, fm), depth - 1, tol_len, floor, out);
        out.push((m, fm));
        refine(f, (m, fm), b, depth - 1, tol_len, floor, out);
    }
    let mut out = Vec::new();
    let mut prev = (knots[0], f(knots[0]));
    out.push(prev);
    for &x in &knots[1..] {
        let cur = (x, f(x));
        refine(f, prev, cur, 60, tol_len, floor, &mut out);
        out.push(cur);
        prev = cur;
    }
    out
}

/// Sorted, de-duplicated knots in `[0, end]`: both ends, `seeds` uniform ones and `extra`.
fn knots(end: f64, seeds: usize, extra: impl IntoIterator<Item = f64>) -> Vec<f64> {
    let mut k: Vec<f64> = (0..=seeds).map(|i| end * i as f64 / seeds as f64).collect();
    k.extend(extra.into_iter().filter(|&x| x > 0.0 && x < end));
    k.sort_by(f64::total_cmp);
    let eps = end * 1e-12;
    let mut out: Vec<f64> = Vec::with_capacity(k.len());
    for x in k {
        if out.last().map_or(true, |&l| x - l > eps) {
            out.push(x);
        }
    }
    if let Some(l) = out.last_mut() {
        *l = end;
    }
    out
}

/// Width of segment `i`'s strip on the perpendicular through station `s` (evaluated just
/// inside the segment at its ends, so the value there is the limit from inside).
fn strip_width(region: &Region<'_>, segs: &[Segment], i: usize, s: f64, scale: f64) -> f64 {
    let seg = &segs[i];
    let ab = sub(seg.b, seg.a);
    let len = norm(ab);
    let u = [ab[0] / len, ab[1] / len];
    let n = [-u[1], u[0]];
    let delta = 1e-9 * len;
    let s = s.clamp(delta, len - delta);
    let p0 = [seg.a[0] + s * u[0], seg.a[1] + s * u[1]];
    let set = region.along(p0, n);
    measure(&nearest_only(set, p0, n, segs, |j| j == i, |j| j < i, scale))
}

/// Distance from `p` to a segment.
fn seg_dist(p: P2, s: &Segment) -> f64 {
    let ab = sub(s.b, s.a);
    let l2 = dot(ab, ab);
    let t = if l2 > 0.0 { (dot(sub(p, s.a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    norm(sub(p, [s.a[0] + t * ab[0], s.a[1] + t * ab[1]]))
}

/// The part of `set` (parameters on the line `p0 + t·d`) whose points are not nearer to any
/// segment `j` (except those `skip`ped) than `|t|`, strictly farther for the `strict` ones.
/// Competitors are visited nearest first; once one is farther from `p0` than twice the largest
/// `|t|` left, it and all farther ones cannot win any point.
fn nearest_only(
    mut set: Set,
    p0: P2,
    d: P2,
    segs: &[Segment],
    skip: impl Fn(usize) -> bool,
    strict: impl Fn(usize) -> bool,
    scale: f64,
) -> Set {
    if set.is_empty() {
        return set;
    }
    let mut order: Vec<(f64, usize)> =
        (0..segs.len()).filter(|&j| !skip(j)).map(|j| (seg_dist(p0, &segs[j]), j)).collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    for (dj, j) in order {
        let Some((lo, hi)) = set.first().map(|f| f.0).zip(set.last().map(|l| l.1)) else { break };
        if dj > 2.0 * lo.abs().max(hi.abs()) * (1.0 + 1e-9) {
            break;
        }
        set = intersect(&set, &not_nearer(&segs[j], p0, d, lo, hi, strict(j), scale));
    }
    set
}

/// The nearest-segment partition of the region `outer` minus `holes` among `segs`.
pub(crate) fn partition(outer: &[P2], holes: &[Vec<P2>], segs: &[Segment]) -> Partition {
    let region = Region { outer, holes };
    let mut pts: Vec<P2> = outer.to_vec();
    for h in holes {
        pts.extend(h.iter().copied());
    }
    for s in segs {
        pts.push(s.a);
        pts.push(s.b);
    }
    let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &pts {
        for k in 0..2 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let scale = norm(sub(max, min)).max(1e-300);
    let (tol_len, floor) = (1e-10 * scale, 1e-14 * scale * scale);

    // Strips.
    let mut profiles = Vec::with_capacity(segs.len());
    for (i, seg) in segs.iter().enumerate() {
        let ab = sub(seg.b, seg.a);
        let len = norm(ab);
        if len == 0.0 {
            profiles.push(Vec::new());
            continue;
        }
        let u = [ab[0] / len, ab[1] / len];
        let width = |s: f64| strip_width(&region, segs, i, s, scale);
        let ks = knots(len, 16, pts.iter().map(|p| dot(sub(*p, seg.a), u)));
        let prof = sample(&width, &ks, tol_len, floor);
        if prof.iter().all(|&(_, w)| w == 0.0) {
            profiles.push(Vec::new());
        } else {
            profiles.push(prof);
        }
    }

    // Caps around every distinct segment end.
    let mut caps = Vec::new();
    let mut ends: Vec<P2> = Vec::new();
    for s in segs {
        for p in [s.a, s.b] {
            if !ends.contains(&p) {
                ends.push(p);
            }
        }
    }
    let two_pi = 2.0 * std::f64::consts::PI;
    for a in ends {
        // Incident segments with the direction from the node into them.
        let mut incident: Vec<(usize, usize, P2)> = Vec::new();
        for (k, s) in segs.iter().enumerate() {
            for (end, (p, q)) in [(s.a, s.b), (s.b, s.a)].into_iter().enumerate() {
                if p == a {
                    let d = sub(q, p);
                    let l = norm(d);
                    if l > 0.0 {
                        incident.push((k, end, [d[0] / l, d[1] / l]));
                    }
                }
            }
        }
        let Some(&(credit, credit_end, _)) = incident.first() else { continue };
        let admissible = |th: f64| {
            let e = [th.cos(), th.sin()];
            incident.iter().all(|&(_, _, u)| dot(e, u) <= 0.0)
        };
        let g = |th: f64| -> f64 {
            let e = [th.cos(), th.sin()];
            let set = intersect(&region.along(a, e), &vec![(0.0, f64::INFINITY)]);
            let set = nearest_only(set, a, e, segs, |j| incident.iter().any(|c| c.0 == j), |_| true, scale);
            set.iter().map(|(r1, r2)| 0.5 * (r2 * r2 - r1 * r1)).sum()
        };
        let angle = |v: P2| v[1].atan2(v[0]).rem_euclid(two_pi);
        let mut extra: Vec<f64> = Vec::new();
        for &(_, _, u) in &incident {
            let phi = angle(u);
            extra.push((phi + 0.5 * std::f64::consts::PI).rem_euclid(two_pi));
            extra.push((phi + 1.5 * std::f64::consts::PI).rem_euclid(two_pi));
        }
        extra.extend(pts.iter().filter(|p| **p != a).map(|p| angle(sub(*p, a))));
        let ks = knots(two_pi, 32, extra);
        let mut area = 0.0;
        for w in ks.windows(2) {
            if !admissible(0.5 * (w[0] + w[1])) {
                continue;
            }
            // Evaluate just inside the arc so the admissibility edges are not sampled.
            let (lo, hi) = (w[0], w[1]);
            let pad = 1e-12 * (hi - lo);
            let gi = |th: f64| g(th.clamp(lo + pad, hi - pad));
            let s = sample(&gi, &[lo, hi], tol_len * scale, floor);
            area += profile_integral(&s);
        }
        if area > 0.0 {
            caps.push((credit, credit_end, area));
        }
    }
    Partition { profiles, caps }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(a: P2, b: P2) -> Segment {
        Segment { a, b }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<P2> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    fn shoelace(p: &[P2]) -> f64 {
        let n = p.len();
        (0..n).map(|k| cross(p[k], p[(k + 1) % n])).sum::<f64>().abs() * 0.5
    }

    fn total(p: &Partition) -> f64 {
        (0..p.profiles.len()).map(|k| p.area(k)).sum()
    }

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "got {a}, expected {b}");
    }

    #[test]
    fn rectangular_bay_with_opening() {
        let segs = [
            seg([0.0, 0.0], [6.0, 0.0]),
            seg([6.0, 0.0], [6.0, 4.0]),
            seg([6.0, 4.0], [0.0, 4.0]),
            seg([0.0, 4.0], [0.0, 0.0]),
        ];
        let p = partition(&rect(0.0, 0.0, 6.0, 4.0), &[rect(2.5, 1.5, 3.5, 2.5)], &segs);
        for (k, e) in [7.5, 4.0, 7.5, 4.0].into_iter().enumerate() {
            close(p.area(k), e, 1e-8);
        }
        assert!(p.caps.is_empty());
        // The south beam's width profile: a trapezoid peaking at 2 with the opening's notch.
        let w = |s: f64| {
            let pr = &p.profiles[0];
            let k = pr.windows(2).position(|w| w[0].0 <= s && s <= w[1].0).unwrap();
            let (a, b) = (pr[k], pr[k + 1]);
            a.1 + (b.1 - a.1) * (s - a.0) / (b.0 - a.0)
        };
        close(w(1.0), 1.0, 1e-9);
        close(w(3.0), 1.5, 1e-9);
        close(w(5.0), 1.0, 1e-9);
    }

    #[test]
    fn split_segments_and_a_middle_beam() {
        let segs = [
            seg([0.0, 0.0], [4.0, 0.0]),
            seg([4.0, 0.0], [8.0, 0.0]),
            seg([8.0, 4.0], [4.0, 4.0]),
            seg([4.0, 4.0], [0.0, 4.0]),
            seg([8.0, 0.0], [8.0, 4.0]),
            seg([0.0, 4.0], [0.0, 0.0]),
            seg([4.0, 0.0], [4.0, 4.0]),
        ];
        let p = partition(&rect(0.0, 0.0, 8.0, 4.0), &[], &segs);
        for (k, e) in [4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 8.0].into_iter().enumerate() {
            close(p.area(k), e, 1e-8);
        }
    }

    #[test]
    fn overhanging_slab_is_fully_accounted_for() {
        // A single beam in the middle of an irregular slab with a hole: everything is nearest
        // to it, partly through the caps at its ends.
        let outer = vec![[-2.0, -1.5], [5.0, -2.0], [6.5, 2.0], [1.0, 3.0], [-1.0, 1.5]];
        let hole = vec![[0.5, 0.5], [1.5, 0.5], [1.2, 1.4]];
        let segs = [seg([0.0, 0.0], [4.0, 0.5])];
        let p = partition(&outer, &[hole.clone()], &segs);
        close(total(&p), shoelace(&outer) - shoelace(&hole), 1e-7);
        assert_eq!(p.caps.len(), 2);

        // Several beams, a cantilever tip and slab outside the beam grid.
        let segs = [
            seg([0.0, 0.0], [4.0, 0.0]),
            seg([4.0, 0.0], [4.0, 2.0]),
            seg([0.0, 0.0], [0.0, 2.5]),
            seg([2.0, 0.0], [2.0, 1.0]),
        ];
        let p = partition(&outer, &[hole.clone()], &segs);
        close(total(&p), shoelace(&outer) - shoelace(&hole), 1e-7);
    }

    #[test]
    fn duplicated_segment_is_not_counted_twice() {
        let segs = [seg([0.0, 0.0], [4.0, 0.0]), seg([0.0, 0.0], [4.0, 0.0])];
        let p = partition(&rect(0.0, -1.0, 4.0, 1.0), &[], &segs);
        close(p.area(0) + p.area(1), 8.0, 1e-8);
    }
}
