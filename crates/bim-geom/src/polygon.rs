//! Simple polygons with holes: area, containment and ear-clipping triangulation.

use crate::predicates::orient2d;
use crate::types::Point2;

/// A polygon: an outer ring and zero or more hole rings. Rings are listed without repeating the
/// first vertex. By convention the outer ring is counter-clockwise and the holes clockwise; all
/// methods normalise the orientation internally, so the opposite orientation is tolerated (the
/// rings must still be simple, nested and pairwise disjoint).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polygon {
    pub outer: Vec<Point2>,
    pub holes: Vec<Vec<Point2>>,
}

/// Signed area of a ring (shoelace formula): positive for counter-clockwise rings. The sum is
/// taken relative to the first vertex for better conditioning.
pub fn signed_area(ring: &[Point2]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let o = ring[0];
    let mut s = 0.0;
    for i in 1..ring.len() - 1 {
        let a = ring[i] - o;
        let b = ring[i + 1] - o;
        s += a.x * b.y - a.y * b.x;
    }
    0.5 * s
}

/// Length of a closed ring.
pub fn ring_length(ring: &[Point2]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let d = ring[(i + 1) % n] - ring[i];
            (d.x * d.x + d.y * d.y).sqrt()
        })
        .sum()
}

/// Number of crossings of the horizontal ray from `p` towards +x with the ring, using the
/// half-open rule (a vertex exactly on the ray counts for the edge going upwards through it) and
/// the exact `orient2d` predicate for the side test.
fn ray_crossings(ring: &[Point2], p: Point2) -> usize {
    let n = ring.len();
    let mut count = 0;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        if (a.y > p.y) != (b.y > p.y) {
            let o = orient2d(a, b, p);
            // Upward edge: the crossing is right of `p` iff `p` is left of a->b.
            // Downward edge: iff `p` is right of a->b.
            if (b.y > a.y && o > 0.0) || (b.y < a.y && o < 0.0) {
                count += 1;
            }
        }
    }
    count
}

/// `r` lies in the closed triangle `a b c` (either orientation), decided exactly.
fn in_closed_triangle(a: Point2, b: Point2, c: Point2, r: Point2) -> bool {
    let o1 = orient2d(a, b, r);
    let o2 = orient2d(b, c, r);
    let o3 = orient2d(c, a, r);
    (o1 >= 0.0 && o2 >= 0.0 && o3 >= 0.0) || (o1 <= 0.0 && o2 <= 0.0 && o3 <= 0.0)
}

impl Polygon {
    /// A polygon without holes.
    pub fn new(outer: Vec<Point2>) -> Self {
        Polygon { outer, holes: Vec::new() }
    }

    /// A polygon with holes.
    pub fn with_holes(outer: Vec<Point2>, holes: Vec<Vec<Point2>>) -> Self {
        Polygon { outer, holes }
    }

    /// All vertices in index order: the outer ring followed by each hole in turn. This is the
    /// numbering used by [`Polygon::triangulate`].
    pub fn vertices(&self) -> Vec<Point2> {
        let mut v = self.outer.clone();
        for h in &self.holes {
            v.extend_from_slice(h);
        }
        v
    }

    /// Net area: the area enclosed by the outer ring minus the areas of the holes. Always
    /// non-negative for a well-formed polygon, whatever the ring orientations.
    pub fn area(&self) -> f64 {
        let outer = signed_area(&self.outer).abs();
        let holes: f64 = self.holes.iter().map(|h| signed_area(h).abs()).sum();
        outer - holes
    }

    /// Total boundary length (outer ring plus all holes).
    pub fn perimeter(&self) -> f64 {
        ring_length(&self.outer) + self.holes.iter().map(|h| ring_length(h)).sum::<f64>()
    }

    /// Whether `p` lies inside the polygon (inside the outer ring and outside every hole),
    /// by the even-odd rule. The result for points exactly on the boundary is unspecified.
    pub fn contains(&self, p: Point2) -> bool {
        let mut crossings = ray_crossings(&self.outer, p);
        for h in &self.holes {
            crossings += ray_crossings(h, p);
        }
        crossings % 2 == 1
    }

    /// Triangulate by ear clipping. Returns index triples into [`Polygon::vertices`] (the outer
    /// ring followed by the holes); every triangle is counter-clockwise. Holes are joined to the
    /// enclosing ring with bridge diagonals (Eberly's method), so the triangulation is
    /// edge-to-edge, adds no points and, for inputs in general position, has exactly
    /// `n + 2h - 2` triangles for `n` vertices and `h` holes.
    ///
    /// Degenerate inputs never drop vertices: if no proper ear exists (self-touching or
    /// collinear input) the least-bad vertex is clipped, which may yield zero-area triangles.
    /// Fewer than three outer vertices give an empty result.
    pub fn triangulate(&self) -> Vec<[usize; 3]> {
        if self.outer.len() < 3 {
            return Vec::new();
        }
        let pts = self.vertices();

        // Rings as global index lists, oriented outer CCW / holes CW.
        let mut outer: Vec<usize> = (0..self.outer.len()).collect();
        if signed_area(&self.outer) < 0.0 {
            outer.reverse();
        }
        let mut holes: Vec<Vec<usize>> = Vec::new();
        let mut offset = self.outer.len();
        for h in &self.holes {
            if h.len() >= 3 {
                let mut idx: Vec<usize> = (offset..offset + h.len()).collect();
                if signed_area(h) > 0.0 {
                    idx.reverse();
                }
                holes.push(idx);
            }
            offset += h.len();
        }

        // Merge holes from the right-most inwards, each into the current merged ring, so that
        // bridges never cross a hole that is yet to be merged.
        let max_x = |ring: &[usize]| ring.iter().map(|&i| pts[i].x).fold(f64::NEG_INFINITY, f64::max);
        holes.sort_by(|a, b| max_x(b).partial_cmp(&max_x(a)).unwrap_or(std::cmp::Ordering::Equal));
        let mut merged = outer;
        for hole in &holes {
            merged = bridge_hole(&merged, hole, &pts);
        }

        ear_clip(&merged, &pts)
    }
}

/// Whether the direction from vertex `cur` (with ring neighbours `prev`, `next`, ring CCW) towards
/// `target` enters the interior wedge at `cur`.
fn locally_inside(prev: Point2, cur: Point2, next: Point2, target: Point2) -> bool {
    let convex = orient2d(prev, cur, next) > 0.0;
    let left_of_out = orient2d(cur, next, target) >= 0.0;
    let left_of_in = orient2d(prev, cur, target) >= 0.0;
    if convex {
        left_of_out && left_of_in
    } else {
        left_of_out || left_of_in
    }
}

/// Splice a hole ring (CW, global indices) into a CCW ring with a bridge diagonal.
///
/// Eberly, "Triangulation by Ear Clipping", §5: from the hole vertex `M` of maximum x cast a ray
/// towards +x; let `I` be the closest point of the ring on it and `P` the vertex of that edge
/// with the larger x (or `I` itself when it is a vertex). If a reflex vertex of the ring lies in
/// the closed triangle `M I P`, `P` is replaced by the one minimising the angle from the ray
/// (closest first on ties), which is then visible from `M`.
fn bridge_hole(ring: &[usize], hole: &[usize], pts: &[Point2]) -> Vec<usize> {
    let n = ring.len();
    // Hole vertex of maximum x (highest y on ties).
    let m_pos = (0..hole.len())
        .max_by(|&i, &j| {
            let (a, b) = (pts[hole[i]], pts[hole[j]]);
            a.x.partial_cmp(&b.x).unwrap().then(a.y.partial_cmp(&b.y).unwrap())
        })
        .unwrap();
    let m = pts[hole[m_pos]];

    // Closest hit of the ray on the ring: either a vertex exactly on the ray or the interior of
    // an edge strictly straddling it.
    let mut best: Option<(f64, Point2, usize)> = None; // (distance, I, ring position of P)
    for i in 0..n {
        let a = pts[ring[i]];
        let b = pts[ring[(i + 1) % n]];
        if a.y == m.y && a.x >= m.x {
            let d = a.x - m.x;
            if best.map_or(true, |(bd, _, _)| d < bd) {
                best = Some((d, a, i));
            }
        }
        if (a.y < m.y && b.y > m.y) || (a.y > m.y && b.y < m.y) {
            let t = (m.y - a.y) / (b.y - a.y);
            let x = a.x + t * (b.x - a.x);
            if x >= m.x {
                let d = x - m.x;
                if best.map_or(true, |(bd, _, _)| d < bd) {
                    let p_pos = if b.x > a.x { (i + 1) % n } else { i };
                    best = Some((d, Point2::new(x, m.y), p_pos));
                }
            }
        }
    }
    let (_, hit, mut p_pos) = match best {
        Some(b) => b,
        // The hole is not enclosed by the ring (invalid input): connect to the nearest vertex.
        None => {
            let p = (0..n)
                .min_by(|&i, &j| {
                    let di = pts[ring[i]] - m;
                    let dj = pts[ring[j]] - m;
                    (di.x * di.x + di.y * di.y).partial_cmp(&(dj.x * dj.x + dj.y * dj.y)).unwrap()
                })
                .unwrap();
            (0.0, pts[ring[p]], p)
        }
    };
    let p = pts[ring[p_pos]];

    // Reflex vertices inside the closed triangle M I P block the view of P; the one with the
    // smallest angle from the ray (closest on ties) is visible from M. When the hit is a vertex
    // itself (I = P) it is visible by construction and the search is skipped.
    let mut cand: Option<(f64, f64, usize)> = None; // (-cos angle, distance, ring position)
    for i in 0..n {
        if hit == p {
            break;
        }
        let r = pts[ring[i]];
        if r == p || r.x < m.x {
            continue;
        }
        let prev = pts[ring[(i + n - 1) % n]];
        let next = pts[ring[(i + 1) % n]];
        if orient2d(prev, r, next) > 0.0 {
            continue; // convex
        }
        if !in_closed_triangle(m, hit, p, r) {
            continue;
        }
        let d = r - m;
        let dist = (d.x * d.x + d.y * d.y).sqrt();
        let key = (if dist > 0.0 { -d.x / dist } else { -1.0 }, dist);
        if cand.map_or(true, |(k0, k1, _)| key.0 < k0 || (key.0 == k0 && key.1 < k1)) {
            cand = Some((key.0, key.1, i));
        }
    }
    if let Some((_, _, i)) = cand {
        p_pos = i;
    }

    // Among copies of P already created by earlier bridges, use the one whose interior wedge
    // contains the direction towards M.
    let p = pts[ring[p_pos]];
    let p_pos = (0..n)
        .filter(|&i| pts[ring[i]] == p)
        .find(|&i| locally_inside(pts[ring[(i + n - 1) % n]], p, pts[ring[(i + 1) % n]], m))
        .unwrap_or(p_pos);

    // ring[..=P], hole from M around back to M, P, ring[P+1..]
    let mut out = Vec::with_capacity(n + hole.len() + 2);
    out.extend_from_slice(&ring[..=p_pos]);
    for k in 0..hole.len() {
        out.push(hole[(m_pos + k) % hole.len()]);
    }
    out.push(hole[m_pos]);
    out.extend_from_slice(&ring[p_pos..]);
    out
}

/// Ear-clip a CCW (weakly simple) ring of global indices.
fn ear_clip(ring: &[usize], pts: &[Point2]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = ring.to_vec();
    let mut tris = Vec::with_capacity(idx.len().saturating_sub(2));
    while idx.len() > 3 {
        let n = idx.len();
        let mut ear = None;
        let mut fallback = (f64::NEG_INFINITY, 0usize);
        for i in 0..n {
            let (ip, ic, inx) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (p, c, nx) = (pts[ip], pts[ic], pts[inx]);
            let o = orient2d(p, c, nx);
            if o > fallback.0 {
                fallback = (o, i);
            }
            if o <= 0.0 {
                continue;
            }
            // With four vertices left, clipping this ear leaves the other three as the last
            // triangle: skip it when they are collinear (a vertex on a straight edge, kept
            // because another sheet touches there) and another ear can take it.
            if n == 4 && orient2d(nx, pts[idx[(i + 2) % n]], p) == 0.0 && (0..n).any(|j| j != i && orient2d(pts[idx[(j + n - 1) % n]], pts[idx[j]], pts[idx[(j + 1) % n]]) > 0.0 && orient2d(pts[idx[(j + 1) % n]], pts[idx[(j + 2) % n]], pts[idx[(j + n - 1) % n]]) != 0.0) {
                continue;
            }
            let blocked = (0..n).any(|j| {
                if j == i || j == (i + n - 1) % n || j == (i + 1) % n {
                    return false;
                }
                let ir = idx[j];
                if ir == ip || ir == ic || ir == inx {
                    return false; // bridge copy of a corner
                }
                let r = pts[ir];
                if r == p || r == c || r == nx {
                    return false;
                }
                in_closed_triangle(p, c, nx, r)
            });
            if !blocked {
                ear = Some(i);
                break;
            }
        }
        let i = ear.unwrap_or(fallback.1);
        let n = idx.len();
        tris.push([idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]]);
        idx.remove(i);
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn tri_area(pts: &[Point2], t: [usize; 3]) -> f64 {
        0.5 * orient2d(pts[t[0]], pts[t[1]], pts[t[2]])
    }

    fn check_tiling(poly: &Polygon) -> Vec<[usize; 3]> {
        let tris = poly.triangulate();
        let pts = poly.vertices();
        let holes = poly.holes.iter().filter(|h| h.len() >= 3).count();
        assert_eq!(tris.len(), pts.len() + 2 * holes - 2, "triangle count");
        let sum: f64 = tris.iter().map(|&t| tri_area(&pts, t)).sum();
        assert!((sum - poly.area()).abs() <= 1e-12 * poly.area().max(1.0), "area {sum} vs {}", poly.area());
        for &t in &tris {
            assert!(tri_area(&pts, t) > 0.0, "triangle {t:?} not CCW/positive");
            // Every triangle centroid must be inside the polygon.
            let c = p(
                (pts[t[0]].x + pts[t[1]].x + pts[t[2]].x) / 3.0,
                (pts[t[0]].y + pts[t[1]].y + pts[t[2]].y) / 3.0,
            );
            assert!(poly.contains(c), "centroid of {t:?} outside");
        }
        // Edge-to-edge: every directed interior edge is matched by its reverse exactly once and
        // boundary edges are polygon edges.
        let mut edges = std::collections::HashMap::new();
        for &t in &tris {
            for k in 0..3 {
                *edges.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
            }
        }
        let mut boundary = std::collections::HashSet::new();
        let rings = std::iter::once(&poly.outer).chain(poly.holes.iter());
        let mut off = 0;
        for r in rings {
            for i in 0..r.len() {
                boundary.insert((off + i, off + (i + 1) % r.len()));
                boundary.insert((off + (i + 1) % r.len(), off + i));
            }
            off += r.len();
        }
        for (&(a, b), &c) in &edges {
            assert_eq!(c, 1, "edge {a}-{b} used {c} times");
            if !boundary.contains(&(a, b)) {
                assert_eq!(edges.get(&(b, a)), Some(&1), "interior edge {a}-{b} has no twin");
            }
        }
        tris
    }

    #[test]
    fn area_and_contains() {
        let sq = Polygon::with_holes(
            vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 4.0), p(0.0, 4.0)],
            vec![vec![p(1.0, 1.0), p(1.0, 2.0), p(2.0, 2.0), p(2.0, 1.0)]],
        );
        assert_eq!(sq.area(), 15.0);
        assert!(sq.contains(p(3.0, 3.0)));
        assert!(!sq.contains(p(1.5, 1.5)));
        assert!(!sq.contains(p(5.0, 1.0)));
        assert!(!sq.contains(p(-1.0, 1.0)));
        assert_eq!(sq.perimeter(), 20.0);
        // Orientation-agnostic.
        let mut rev = sq.clone();
        rev.outer.reverse();
        rev.holes[0].reverse();
        assert_eq!(rev.area(), 15.0);
        assert!(rev.contains(p(3.0, 3.0)));
        assert!(!rev.contains(p(1.5, 1.5)));
    }

    #[test]
    fn triangulate_simple_shapes() {
        check_tiling(&Polygon::new(vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)]));
        check_tiling(&Polygon::new(vec![p(0.0, 0.0), p(3.0, 0.0), p(3.0, 1.0), p(1.0, 1.0), p(1.0, 3.0), p(0.0, 3.0)]));
        check_tiling(&Polygon::new(vec![p(0.0, 0.0), p(5.0, 1.0), p(2.0, 4.0)]));
        // Comb with many reflex vertices.
        let teeth = 7;
        let mut pts = vec![p(0.0, 0.0), p(2.0 * teeth as f64, 0.0), p(2.0 * teeth as f64, 1.0)];
        for k in (0..teeth).rev() {
            let k = k as f64;
            pts.extend([p(2.0 * k + 1.6, 1.0), p(2.0 * k + 1.6, 3.0), p(2.0 * k + 0.4, 3.0), p(2.0 * k + 0.4, 1.0)]);
        }
        pts.push(p(0.0, 1.0));
        check_tiling(&Polygon::new(pts));
    }

    #[test]
    fn triangulate_with_holes() {
        check_tiling(&Polygon::with_holes(
            vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 4.0), p(0.0, 4.0)],
            vec![vec![p(1.0, 1.0), p(1.0, 2.0), p(2.0, 2.0), p(2.0, 1.0)]],
        ));
        // Two holes sharing horizontal levels: the ray from one hole runs along the other.
        check_tiling(&Polygon::with_holes(
            vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 6.0), p(0.0, 6.0)],
            vec![
                vec![p(1.0, 1.0), p(1.0, 5.0), p(4.0, 5.0), p(4.0, 1.0)],
                vec![p(6.0, 1.0), p(6.0, 5.0), p(9.0, 5.0), p(9.0, 1.0)],
            ],
        ));
        // Hollow square profile and a star with three holes.
        let rect = |w: f64, h: f64, cw: bool| {
            let mut r = vec![p(-w / 2.0, -h / 2.0), p(w / 2.0, -h / 2.0), p(w / 2.0, h / 2.0), p(-w / 2.0, h / 2.0)];
            if cw {
                r.reverse();
            }
            r
        };
        check_tiling(&Polygon::with_holes(rect(0.6, 0.6, false), vec![rect(0.2, 0.2, true)]));
        let n = 23;
        let outer: Vec<Point2> = (0..n)
            .map(|k| {
                let a = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                let r = if k % 2 == 0 { 10.0 } else { 6.5 };
                p(3.0 + r * a.cos(), -2.0 + r * a.sin())
            })
            .collect();
        let holes: Vec<Vec<Point2>> = (0..3)
            .map(|k| {
                let ca = 2.0 * std::f64::consts::PI * k as f64 / 3.0;
                let (cx, cy) = (3.0 + 2.2 * ca.cos(), -2.0 + 2.2 * ca.sin());
                let m = 5 + k;
                (0..m)
                    .rev()
                    .map(|j| {
                        let a = 0.3 + 2.0 * std::f64::consts::PI * j as f64 / m as f64;
                        p(cx + 0.9 * a.cos(), cy + 0.9 * a.sin())
                    })
                    .collect()
            })
            .collect();
        check_tiling(&Polygon::with_holes(outer, holes));
    }

    #[test]
    fn degenerate_polygons() {
        assert!(Polygon::new(vec![p(0.0, 0.0), p(1.0, 0.0)]).triangulate().is_empty());
        assert_eq!(Polygon::new(vec![]).area(), 0.0);
        // Collinear vertex: no vertex is dropped.
        let tris = Polygon::new(vec![p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0), p(2.0, 1.0), p(0.0, 1.0)]).triangulate();
        assert_eq!(tris.len(), 3);
    }
}
