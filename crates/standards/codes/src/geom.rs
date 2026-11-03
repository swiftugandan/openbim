//! Plan geometry of the checks: the minimum-area bounding rectangle of a room polygon and the
//! longest shortest path inside a room polygon to a set of door points.
//!
//! Nothing here panics: every ordering of floats is `total_cmp`, and non-finite input gives `None`
//! or a path that is simply not visible.

use bim_arch::rooms::contains;

pub(crate) type P = [f64; 2];

/// How far a sample point of a sight line may lie outside the polygon and still count as on its
/// boundary (the sight line may run along an edge): 1e-9 m.
const ON_BOUNDARY: f64 = 1e-9;

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: P, b: P) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dist(a: P, b: P) -> f64 {
    let d = sub(a, b);
    d[0].hypot(d[1])
}

/// The convex hull of `points`, counter-clockwise from the lexicographically smallest point, with
/// collinear points dropped (Andrew's monotone chain). Fewer than three points come back as they
/// are (deduplicated); points that are all collinear give a hull of fewer than three points.
pub(crate) fn convex_hull(points: &[P]) -> Vec<P> {
    // `+ 0.0` turns -0.0 into 0.0, so equal points are equal for the deduplication.
    let mut pts: Vec<P> =
        points.iter().filter(|p| p[0].is_finite() && p[1].is_finite()).map(|p| [p[0] + 0.0, p[1] + 0.0]).collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let turns_right = |hull: &[P], p: P| {
        let (a, b) = (hull[hull.len() - 2], hull[hull.len() - 1]);
        cross(sub(b, a), sub(p, a)) <= 0.0
    };
    let mut hull: Vec<P> = Vec::with_capacity(pts.len() + 1);
    for &p in &pts {
        while hull.len() >= 2 && turns_right(&hull, p) {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in pts.iter().rev().skip(1) {
        while hull.len() >= lower && turns_right(&hull, p) {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop(); // the start point again
    hull
}

/// The shorter side of the minimum-area bounding rectangle of the polygon `poly` (any simple or
/// non-simple vertex ring; only its convex hull matters): the rectangle of least area is aligned
/// with an edge of the hull, so each hull edge is tried and the first of least area wins. `None`
/// if the hull is degenerate (fewer than three non-collinear points).
pub(crate) fn min_rect_short_side(poly: &[P]) -> Option<f64> {
    let hull = convex_hull(poly);
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f64, f64)> = None; // (area, short side)
    for (i, &origin) in hull.iter().enumerate() {
        let d = sub(hull[(i + 1) % hull.len()], origin);
        let len = d[0].hypot(d[1]);
        if len == 0.0 {
            continue;
        }
        let u = [d[0] / len, d[1] / len];
        let v = [-u[1], u[0]];
        let (mut lo_u, mut hi_u, mut lo_v, mut hi_v) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for &p in &hull {
            let r = sub(p, origin);
            let (pu, pv) = (dot(r, u), dot(r, v));
            lo_u = lo_u.min(pu);
            hi_u = hi_u.max(pu);
            lo_v = lo_v.min(pv);
            hi_v = hi_v.max(pv);
        }
        let (w, h) = (hi_u - lo_u, hi_v - lo_v);
        let area = w * h;
        if area.is_finite() && best.is_none_or(|(least, _)| area < least) {
            best = Some((area, w.min(h)));
        }
    }
    best.map(|(_, side)| side)
}

/// The point of the closed ring `poly` nearest to `p` and its distance; among equally near points
/// the one on the first edge. `None` for an empty ring.
pub(crate) fn nearest_on_boundary(poly: &[P], p: P) -> Option<(P, f64)> {
    let mut best: Option<(P, f64)> = None;
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let d = sub(b, a);
        let l2 = dot(d, d);
        let t = if l2 == 0.0 { 0.0 } else { (dot(sub(p, a), d) / l2).clamp(0.0, 1.0) };
        let q = [a[0] + t * d[0], a[1] + t * d[1]];
        let dq = dist(p, q);
        if best.is_none_or(|(_, least)| dq < least) {
            best = Some((q, dq));
        }
    }
    best
}

/// Whether the segment `a`–`b` lies inside `poly` (or on its boundary, within 1e-9). The segment is
/// cut at every parameter where it crosses an edge or passes a vertex, and the midpoint of every
/// piece must be inside or on the boundary: each piece lies wholly inside, wholly outside or along
/// the boundary, so this is exact, and a sight line that grazes a reflex vertex or runs along an
/// edge is visible. A segment of zero length is visible.
pub(crate) fn visible(poly: &[P], a: P, b: P) -> bool {
    let d = sub(b, a);
    let l2 = dot(d, d);
    if l2 == 0.0 {
        return true;
    }
    let n = poly.len();
    let mut ts = vec![0.0, 1.0];
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let e = sub(q, p);
        let den = cross(d, e);
        if den != 0.0 {
            // a + t d = p + u e
            let t = cross(sub(p, a), e) / den;
            let u = cross(sub(p, a), d) / den;
            if (-1e-12..=1.0 + 1e-12).contains(&u) && t > 0.0 && t < 1.0 {
                ts.push(t);
            }
        }
        for v in [p, q] {
            let t = dot(sub(v, a), d) / l2;
            if t > 0.0 && t < 1.0 {
                ts.push(t);
            }
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.windows(2).filter(|w| w[1] > w[0]).all(|w| {
        let tm = (w[0] + w[1]) / 2.0;
        let m = [a[0] + tm * d[0], a[1] + tm * d[1]];
        contains(poly, m) || nearest_on_boundary(poly, m).is_some_and(|(_, off)| off <= ON_BOUNDARY)
    })
}

/// The longest, over the vertices of the ring `poly`, of the shortest path inside the polygon to
/// the nearest of the points `doors` (taken to lie on the boundary): the geodesic distance. The
/// shortest path bends only at polygon vertices, so it is found by Dijkstra over the visibility
/// graph of the vertices and the door points, with every door point at distance 0. A vertex that
/// reaches no door is left out; `None` if there are no doors or no vertex reaches one.
pub(crate) fn egress_distance(poly: &[P], doors: &[P]) -> Option<f64> {
    let nodes: Vec<P> = poly.iter().chain(doors).copied().collect();
    let count = nodes.len();
    let mut dist_to_door = vec![f64::INFINITY; count];
    for d in dist_to_door.iter_mut().skip(poly.len()) {
        *d = 0.0;
    }
    let mut done = vec![false; count];
    while let Some(u) =
        (0..count).filter(|&i| !done[i] && dist_to_door[i].is_finite()).min_by(|&i, &j| dist_to_door[i].total_cmp(&dist_to_door[j]))
    {
        done[u] = true;
        for v in 0..count {
            if done[v] {
                continue;
            }
            let through = dist_to_door[u] + dist(nodes[u], nodes[v]);
            if through < dist_to_door[v] && visible(poly, nodes[u], nodes[v]) {
                dist_to_door[v] = through;
            }
        }
    }
    dist_to_door[..poly.len()].iter().copied().filter(|d| d.is_finite()).max_by(f64::total_cmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotated_rectangle(length: f64, width: f64, angle: f64) -> Vec<P> {
        let (c, s) = (angle.cos(), angle.sin());
        [[0.0, 0.0], [length, 0.0], [length, width], [0.0, width]].iter().map(|p| [3.0 + c * p[0] - s * p[1], -2.0 + s * p[0] + c * p[1]]).collect()
    }

    #[test]
    fn the_hull_drops_collinear_and_interior_points() {
        let pts = [[0.0, 0.0], [2.0, 0.0], [4.0, 0.0], [4.0, 4.0], [2.0, 2.0], [0.0, 4.0], [0.0, 2.0], [0.0, 0.0]];
        assert_eq!(convex_hull(&pts), vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]);
        assert_eq!(convex_hull(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]]).len(), 2);
        assert_eq!(convex_hull(&[[1.0, 1.0]]), vec![[1.0, 1.0]]);
        assert!(convex_hull(&[]).is_empty());
        assert_eq!(convex_hull(&[[0.0, f64::NAN], [1.0, 1.0]]), vec![[1.0, 1.0]]);
        // -0.0 and 0.0 are one point
        assert_eq!(convex_hull(&[[-0.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]]).len(), 3);
    }

    #[test]
    fn the_short_side_of_a_rectangle_at_any_angle() {
        assert!((min_rect_short_side(&rotated_rectangle(8.0, 1.318, 0.0)).unwrap() - 1.318).abs() <= 1e-12);
        assert!((min_rect_short_side(&rotated_rectangle(1.318, 8.0, 0.0)).unwrap() - 1.318).abs() <= 1e-12);
        for degrees in [30.0_f64, -15.0, 45.0, 90.0, 123.0] {
            let poly = rotated_rectangle(8.0, 1.318, degrees.to_radians());
            let side = min_rect_short_side(&poly).unwrap();
            assert!((side - 1.318).abs() <= 1e-12, "{degrees}: {side}");
        }
        // clockwise input and a repeated closing vertex change nothing
        let mut poly = rotated_rectangle(8.0, 1.318, 0.5);
        poly.reverse();
        poly.push(poly[0]);
        assert!((min_rect_short_side(&poly).unwrap() - 1.318).abs() <= 1e-12);
    }

    #[test]
    fn the_rectangle_is_that_of_the_hull_not_of_the_polygon_edges() {
        // L: its hull is the pentagon (0,0) (6,0) (6,3) (3,6) (0,6); the least area, 36, is at the
        // axis directions (the 45 degree edge gives 54).
        let l = [[0.0, 0.0], [6.0, 0.0], [6.0, 3.0], [3.0, 3.0], [3.0, 6.0], [0.0, 6.0]];
        assert_eq!(convex_hull(&l), vec![[0.0, 0.0], [6.0, 0.0], [6.0, 3.0], [3.0, 6.0], [0.0, 6.0]]);
        assert!((min_rect_short_side(&l).unwrap() - 6.0).abs() <= 1e-12);
    }

    #[test]
    fn degenerate_polygons_have_no_rectangle() {
        assert_eq!(min_rect_short_side(&[]), None);
        assert_eq!(min_rect_short_side(&[[0.0, 0.0], [1.0, 1.0]]), None);
        assert_eq!(min_rect_short_side(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]]), None);
        assert_eq!(min_rect_short_side(&[[0.0, 0.0], [f64::INFINITY, 1.0], [2.0, 5.0]]), None);
    }

    #[test]
    fn nearest_boundary_point_first_edge_wins() {
        let sq = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        assert_eq!(nearest_on_boundary(&sq, [1.0, 0.5]), Some(([1.0, 0.0], 0.5)));
        assert_eq!(nearest_on_boundary(&sq, [3.0, 3.0]), Some(([2.0, 2.0], 2.0_f64.sqrt())));
        // equidistant from the bottom and the left edge: the first edge (bottom)
        assert_eq!(nearest_on_boundary(&sq, [0.5, 0.5]).unwrap().0, [0.5, 0.0]);
        assert_eq!(nearest_on_boundary(&[], [0.0, 0.0]), None);
    }

    /// The inner polygon of the judge's L-shaped room (walls 0.2 thick on the centreline
    /// (50,0) (56,0) (56,3) (53,3) (53,6) (50,6)), as `bim solve --trade arch` reports it.
    fn judge_l() -> Vec<P> {
        vec![[50.1, 0.09999999999999876], [55.9, 0.1], [55.9, 2.8999999999999995], [52.9, 2.9], [52.9, 5.9], [50.1, 5.9]]
    }

    #[test]
    fn sight_lines_in_an_l_shaped_room() {
        let l = judge_l();
        // across the notch: outside
        assert!(!visible(&l, [50.1, 5.9], [55.9, 1.5]));
        // grazing the reflex corner (52.9, 2.9) exactly: visible
        assert!(visible(&l, [50.1, 5.9], [52.9, 2.9]));
        assert!(visible(&l, [52.9, 2.9], [55.9, 1.5]));
        // along an edge, over a vertex, and a zero-length segment
        assert!(visible(&l, [50.1, 0.1], [55.9, 0.1]));
        assert!(visible(&l, [50.1, 5.9], [50.1, 0.1]));
        assert!(visible(&l, [51.0, 1.0], [51.0, 1.0]));
        // inside-to-inside across the room, and a line leaving through a wall
        assert!(visible(&l, [51.0, 1.0], [55.0, 2.0]));
        assert!(!visible(&l, [51.0, 1.0], [57.0, 1.0]));
    }

    #[test]
    fn geodesic_distance_bends_around_the_reflex_corner() {
        let l = judge_l();
        let (target, offset) = nearest_on_boundary(&l, [56.0, 1.5]).unwrap();
        assert!((target[0] - 55.9).abs() <= 1e-12 && (target[1] - 1.5).abs() <= 1e-12, "{target:?}");
        assert!((offset - 0.1).abs() <= 1e-12);
        let farthest = egress_distance(&l, &[target]).unwrap();
        // (50.1, 5.9) -> reflex corner (52.9, 2.9) -> door point (55.9, 1.5)
        let closed_form = dist([50.1, 5.9], [52.9, 2.9]) + dist([52.9, 2.9], [55.9, 1.5]);
        assert!((farthest - closed_form).abs() <= 1e-12, "{farthest} vs {closed_form}");
        assert!((farthest - 7.414245977186006).abs() <= 1e-12, "{farthest}");
    }

    #[test]
    fn geodesic_distance_in_convex_rooms_and_with_several_doors() {
        let rect = [[0.0, 0.0], [6.0, 0.0], [6.0, 4.0], [0.0, 4.0]];
        // one door in the middle of the bottom edge: the farthest corners are at hypot(3, 4)
        assert!((egress_distance(&rect, &[[3.0, 0.0]]).unwrap() - 5.0).abs() <= 1e-12);
        // a door in the middle of each short side: every corner is 2 from the nearer door
        let two = egress_distance(&rect, &[[0.0, 2.0], [6.0, 2.0]]).unwrap();
        assert!((two - 2.0).abs() <= 1e-12, "{two}");
        assert_eq!(egress_distance(&rect, &[]), None);
    }
}
