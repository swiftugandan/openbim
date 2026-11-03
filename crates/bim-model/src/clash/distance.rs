//! Exact Euclidean distances between triangles and between triangle meshes: the soft-clash
//! measure. Everything is plain `f64` geometry (closest points on triangles and segments after
//! Ericson, *Real-Time Collision Detection*, §5.1), with no tolerances: two triangles are at
//! distance 0 exactly when one passes through the other, and otherwise the distance is attained
//! at a vertex of one and a point of the other, or between two edges.

use bim_spatial::{Aabb, Bvh};

use crate::Mesh;

pub(super) type V3 = [f64; 3];
/// A triangle as three points.
pub(super) type Tri = [V3; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dist2(a: V3, b: V3) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

/// The point of the segment `a b` closest to `p`.
fn closest_on_segment(p: V3, a: V3, b: V3) -> V3 {
    let ab = sub(b, a);
    let len2 = dot(ab, ab);
    if !(len2 > 0.0) {
        return a;
    }
    add(a, scale(ab, (dot(sub(p, a), ab) / len2).clamp(0.0, 1.0)))
}

/// The point of the triangle `a b c` closest to `p`.
fn closest_on_triangle(p: V3, a: V3, b: V3, c: V3) -> V3 {
    let (ab, ac) = (sub(b, a), sub(c, a));
    let n = cross(ab, ac);
    if !(dot(n, n) > 1e-300) {
        // A flat triangle is its longest edge: the closest of its three edges.
        let on = [closest_on_segment(p, a, b), closest_on_segment(p, b, c), closest_on_segment(p, c, a)];
        return on.into_iter().min_by(|x, y| dist2(p, *x).total_cmp(&dist2(p, *y))).unwrap_or(a);
    }
    let ap = sub(p, a);
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
}

/// The squared distance between the segments `p1 q1` and `p2 q2`.
fn segment_segment_dist2(p1: V3, q1: V3, p2: V3, q2: V3) -> f64 {
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let (s, t);
    if a <= 0.0 && e <= 0.0 {
        return dot(r, r);
    }
    if a <= 0.0 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, r);
        if e <= 0.0 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s_ = if denom > 0.0 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t_ = (b * s_ + f) / e;
            if t_ < 0.0 {
                t_ = 0.0;
                s_ = (-c / a).clamp(0.0, 1.0);
            } else if t_ > 1.0 {
                t_ = 1.0;
                s_ = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s_;
            t = t_;
        }
    }
    dist2(add(p1, scale(d1, s)), add(p2, scale(d2, t)))
}

/// Whether the segment `p q` meets the interior or boundary of the triangle `t` at a single
/// point (a segment lying in the triangle's plane is left to the distance tests).
fn segment_meets_triangle(p: V3, q: V3, t: &Tri) -> bool {
    let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    let (sp, sq) = (dot(n, sub(p, t[0])), dot(n, sub(q, t[0])));
    if (sp > 0.0 && sq > 0.0) || (sp < 0.0 && sq < 0.0) || sp == sq {
        return false;
    }
    let x = add(p, scale(sub(q, p), sp / (sp - sq)));
    (0..3).all(|k| dot(cross(sub(t[(k + 1) % 3], t[k]), sub(x, t[k])), n) >= 0.0)
}

/// The distance between the triangles `t` and `u`: 0 when they touch or cross.
pub(super) fn triangle_distance(t: &Tri, u: &Tri) -> f64 {
    for k in 0..3 {
        if segment_meets_triangle(t[k], t[(k + 1) % 3], u) || segment_meets_triangle(u[k], u[(k + 1) % 3], t) {
            return 0.0;
        }
    }
    let mut best = f64::INFINITY;
    for &p in t {
        let c = closest_on_triangle(p, u[0], u[1], u[2]);
        best = best.min(dist2(p, c));
    }
    for &p in u {
        let c = closest_on_triangle(p, t[0], t[1], t[2]);
        best = best.min(dist2(p, c));
    }
    for i in 0..3 {
        for j in 0..3 {
            best = best.min(segment_segment_dist2(t[i], t[(i + 1) % 3], u[j], u[(j + 1) % 3]));
        }
    }
    best.sqrt()
}

/// The triangles of `m` as point triples.
pub(super) fn triangles(m: &Mesh) -> Vec<Tri> {
    let p = |i: u32| {
        let q = m.positions[i as usize];
        [q.x, q.y, q.z]
    };
    m.triangles.iter().map(|t| [p(t[0]), p(t[1]), p(t[2])]).collect()
}

/// A hierarchy over the triangles of `tris` whose boxes are each grown by `margin` on every side:
/// two such hierarchies overlap in exactly the triangle pairs whose boxes are no further apart than
/// `2 · margin` along every axis, a superset of the pairs closer than `2 · margin`. (`Bvh::build`
/// looks only at the box of each triangle, so a degenerate triangle spanning the grown box is
/// enough to describe it.)
pub(super) fn grown_bvh(tris: &[Tri], margin: f64) -> Bvh {
    let boxes: Vec<Tri> = tris
        .iter()
        .map(|t| {
            let b = Aabb::from_points(t);
            let (lo, hi) = (b.min.map(|x| x - margin), b.max.map(|x| x + margin));
            [lo, hi, lo]
        })
        .collect();
    Bvh::build(&boxes)
}

/// Distances below this are rounding noise: touching surfaces are at distance exactly 0.
const NOISE: f64 = 1e-12;

/// The smallest distance between a triangle of `a` and one of `b` over the candidate index pairs
/// `pairs` (into `a`'s and `b`'s triangle lists), stopping early at 0.
pub(super) fn min_distance(a: &[Tri], b: &[Tri], pairs: &[(usize, usize)]) -> f64 {
    let mut best = f64::INFINITY;
    for &(i, j) in pairs {
        best = best.min(triangle_distance(&a[i], &b[j]));
        if best == 0.0 {
            break;
        }
    }
    if best < NOISE {
        0.0
    } else {
        best
    }
}

/// The smallest Euclidean distance between the surfaces of two triangle meshes if it is less than
/// `within`, else `None`: 0 for meshes that touch or cross. (Not 0 for one solid lying wholly
/// inside another: their surfaces do not meet.) `within` must be a positive finite number.
pub fn mesh_distance(a: &Mesh, b: &Mesh, within: f64) -> Option<f64> {
    if !(within > 0.0) || !within.is_finite() {
        return None;
    }
    let (ta, tb) = (triangles(a), triangles(b));
    // Boxes grown by `within / 2` meet exactly when the boxes are at most `within` apart.
    let pairs = grown_bvh(&ta, within / 2.0).overlaps(&grown_bvh(&tb, within / 2.0));
    let d = min_distance(&ta, &tb, &pairs);
    (d < within).then_some(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{box_union, Aabb as Box3};
    use crate::segment::{segment_mesh, Section};

    fn cuboid(lo: [f64; 3], hi: [f64; 3]) -> Mesh {
        box_union(&[Box3 { lo, hi }]).1
    }

    #[test]
    fn point_triangle_and_segment_segment_distances() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let d = |p: V3| dist2(p, closest_on_triangle(p, a, b, c)).sqrt();
        assert_eq!(d([0.2, 0.2, 3.0]), 3.0); // above the interior
        assert!(d([0.2, 0.2, 0.0]) < 1e-15);
        assert_eq!(d([-3.0, 4.0, 0.0]), 3.0f64.hypot(3.0)); // nearest the vertex (0, 1, 0)? (−3, 4) to (0, 1): √18
        assert!((d([1.0, 1.0, 0.0]) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-15); // across the hypotenuse
        assert_eq!(d([0.5, -2.0, 1.0]), 5.0f64.sqrt()); // below the edge a-b
        // A flat triangle behaves as its edges.
        let flat = |p: V3| dist2(p, closest_on_triangle(p, [0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0])).sqrt();
        assert_eq!(flat([1.5, 3.0, 0.0]), 3.0);
        assert_eq!(flat([-1.0, 0.0, 0.0]), 1.0);
        // Skew segments, parallel segments, a crossing and degenerate ones.
        let s = |p1, q1, p2, q2| segment_segment_dist2(p1, q1, p2, q2).sqrt();
        assert_eq!(s([0.0; 3], [1.0, 0.0, 0.0], [0.5, -1.0, 2.0], [0.5, 1.0, 2.0]), 2.0);
        assert_eq!(s([0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]), 1.0);
        assert_eq!(s([0.0; 3], [1.0, 0.0, 0.0], [0.0, 3.0, 0.0], [1.0, 3.0, 0.0]), 3.0);
        assert!(s([0.0; 3], [1.0, 0.0, 0.0], [0.5, -1.0, 0.0], [0.5, 1.0, 0.0]) < 1e-15);
        assert_eq!(s([0.0; 3], [0.0; 3], [3.0, 4.0, 0.0], [3.0, 4.0, 0.0]), 5.0);
        assert_eq!(s([0.0; 3], [0.0; 3], [3.0, 4.0, 0.0], [6.0, 8.0, 0.0]), 5.0);
    }

    #[test]
    fn triangle_distances() {
        let t: Tri = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        let up = |z: f64, dx: f64| -> Tri { [[dx, 0.0, z], [dx + 2.0, 0.0, z], [dx, 2.0, z]] };
        assert_eq!(triangle_distance(&t, &up(0.5, 0.0)), 0.5); // parallel and above
        assert_eq!(triangle_distance(&t, &up(0.0, 0.0)), 0.0); // the same
        assert_eq!(triangle_distance(&t, &up(0.0, 0.5)), 0.0); // coplanar and overlapping
        assert_eq!(triangle_distance(&t, &up(0.0, 3.0)), 1.0); // coplanar and apart
        // A triangle standing through the middle of t pierces it: no vertex is near, distance 0.
        let through: Tri = [[0.5, 0.5, -1.0], [0.5, 0.5, 1.0], [1.0, 0.0, 1.0]];
        assert_eq!(triangle_distance(&t, &through), 0.0);
        assert_eq!(triangle_distance(&through, &t), 0.0);
        // Touching at a vertex or along an edge.
        let touch: Tri = [[2.0, 0.0, 0.0], [3.0, 0.0, 1.0], [3.0, 1.0, 1.0]];
        assert_eq!(triangle_distance(&t, &touch), 0.0);
        // Vertex (3, 3, 1) to the hypotenuse x + y = 2: 4/√2 in the plane, 1 above it.
        let skew: Tri = [[3.0, 3.0, 1.0], [4.0, 3.0, 1.0], [3.0, 4.0, 1.0]];
        assert!((triangle_distance(&t, &skew) - 3.0).abs() < 1e-14, "{}", triangle_distance(&t, &skew));
    }

    #[test]
    fn mesh_distances_of_boxes_and_pipes() {
        let a = cuboid([0.0; 3], [1.0; 3]);
        let near = |dx: f64| cuboid([1.0 + dx, 0.0, 0.0], [2.0 + dx, 1.0, 1.0]);
        let d = mesh_distance(&a, &near(0.04), 0.05).unwrap();
        assert!((d - 0.04).abs() < 1e-15, "{d}");
        assert_eq!(mesh_distance(&a, &near(0.04), 0.035), None);
        assert_eq!(mesh_distance(&a, &near(0.04), 0.04), None, "strictly less than `within`");
        assert_eq!(mesh_distance(&a, &near(0.0), 0.05), Some(0.0), "touching");
        assert_eq!(mesh_distance(&a, &cuboid([0.5; 3], [3.0; 3]), 0.05), Some(0.0), "crossing");
        // Diagonal corner to corner: √3 · 0.01.
        let corner = cuboid([1.01; 3], [2.0; 3]);
        let d = mesh_distance(&a, &corner, 0.05).unwrap();
        assert!((d - 3.0f64.sqrt() * 0.01).abs() < 1e-15);
        // A box strictly inside another is not at distance 0 from it (the surfaces do not meet).
        let outer = cuboid([-1.0; 3], [2.0; 3]);
        assert_eq!(mesh_distance(&a, &outer, 0.5), None);
        let d = mesh_distance(&a, &outer, 1.5).unwrap();
        assert!((d - 1.0).abs() < 1e-15);
        // A 16-gon pipe and a wall face: its nearest vertex is at the radius.
        let wall = cuboid([0.0, -0.1, 0.0], [6.0, 0.1, 3.0]);
        let pipe = segment_mesh([1.0, 0.18, 0.5], [3.0, 0.18, 0.5], Section::Round { diameter: 0.1 }).unwrap();
        let d = mesh_distance(&wall, &pipe, 0.05).unwrap();
        assert!((d - 0.03).abs() < 1e-15, "{d}");
        assert_eq!(mesh_distance(&wall, &pipe, 0.0), None);
        assert_eq!(mesh_distance(&wall, &pipe, f64::NAN), None);
        assert_eq!(mesh_distance(&wall, &Mesh::default(), 1.0), None);
    }
}
