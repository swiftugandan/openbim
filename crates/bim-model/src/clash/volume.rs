//! The exact volume of the intersection of two closed, outward-oriented triangle meshes.
//!
//! **Method.** Put the origin at a point `o` inside both meshes' bounding boxes. For a closed,
//! outward-oriented mesh `A` and the cone `tet_i` from `o` to its triangle `t_i`,
//! `1_A = Σ_i s_i · 1_{tet_i}` almost everywhere, with `s_i` the sign of the tetrahedron's
//! orientation `det(a_i, b_i, c_i)` (the signed-volume decomposition of the divergence theorem), so
//! for any solid `B`: `vol(A ∩ B) = Σ_i s_i · vol(tet_i ∩ B)`. When `B` is convex, `tet_i ∩ B` is
//! the tetrahedron clipped by the half-spaces of `B`'s face planes ([`clip`]), a convex polyhedron.
//! When neither mesh is convex, both are fanned: `Σ_i Σ_j s_i s_j · vol(tet_i ∩ tet_j)`, each
//! tetrahedron being convex too. There is no meshing and no topology of the intersection to get
//! wrong: the result is exact up to rounding for any pair of closed meshes, including one lying
//! entirely inside the other, coincident faces and touching solids (volume 0). The only tolerances
//! are relative: a vertex closer than `1e-12` times the largest coordinate (measured from `o`) to a
//! clipping plane counts as on it, and a mesh counts as convex ([`is_convex`]) when every vertex is
//! behind every face plane to within `1e-9` times its bounding box diagonal (a dent that shallow
//! perturbs a volume by at most its depth times a face area).

use crate::Mesh;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn lerp(p: V3, q: V3, t: f64) -> V3 {
    [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1]), p[2] + t * (q[2] - p[2])]
}

/// The half-space `n · x ≤ d`, `n` a unit vector pointing out of it.
#[derive(Debug, Clone, Copy)]
struct Plane {
    n: V3,
    d: f64,
}

impl Plane {
    /// The plane of the triangle `a b c`, facing the way its winding does; `None` for a triangle
    /// with no area to speak of.
    fn of_triangle(a: V3, b: V3, c: V3) -> Option<Plane> {
        let n = cross(sub(b, a), sub(c, a));
        let len = dot(n, n).sqrt();
        if !(len > 1e-300) || !len.is_finite() {
            return None;
        }
        let n = [n[0] / len, n[1] / len, n[2] / len];
        Some(Plane { n, d: dot(n, a) })
    }

    /// Signed distance of `p` outside the half-space (negative inside).
    fn side(&self, p: V3) -> f64 {
        dot(self.n, p) - self.d
    }

    /// Whether `other` is this plane, facing the same way, to within `tol` in offset.
    fn same_as(&self, other: &Plane, tol: f64) -> bool {
        (0..3).all(|k| (self.n[k] - other.n[k]).abs() <= 1e-10) && (self.d - other.d).abs() <= tol
    }
}

/// A closed convex polyhedron: its faces, each a convex polygon counter-clockwise seen from outside.
#[derive(Debug, Clone, Default)]
struct Poly {
    faces: Vec<Vec<V3>>,
}

impl Poly {
    fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// The enclosed volume: every face fanned into triangles and coned from the first vertex.
    fn volume(&self) -> f64 {
        let Some(r) = self.faces.first().and_then(|f| f.first()).copied() else { return 0.0 };
        let mut six = 0.0;
        for f in &self.faces {
            let a = sub(f[0], r);
            for k in 1..f.len() - 1 {
                six += dot(a, cross(sub(f[k], r), sub(f[k + 1], r)));
            }
        }
        six / 6.0
    }

    /// The tetrahedron from the origin to the triangle `a b c`, and the sign of its orientation
    /// (the triangle's winding seen from the origin); `None` if it is flat.
    fn cone(a: V3, b: V3, c: V3) -> Option<(Poly, f64)> {
        let det = dot(a, cross(b, c));
        if !(det.abs() > 1e-300) || !det.is_finite() {
            return None;
        }
        let (sign, b, c) = if det > 0.0 { (1.0, b, c) } else { (-1.0, c, b) };
        let o = [0.0; 3];
        Some((Poly { faces: vec![vec![a, b, c], vec![o, c, b], vec![o, a, c], vec![o, b, a]] }, sign))
    }

    /// The outward planes of the faces (flat faces left out).
    fn planes(&self) -> Vec<Plane> {
        self.faces.iter().filter_map(|f| Plane::of_triangle(f[0], f[1], f[2])).collect()
    }
}

/// `poly ∩ {x : plane.side(x) ≤ 0}`, for a convex `poly`. Vertices within `eps` of the plane count
/// as on it. A polyhedron that does not reach beyond the plane is returned as it is (so a face
/// lying in the plane is not doubled by a cap), one that lies wholly beyond it (or touches it at
/// most) gives the empty polyhedron.
fn clip(poly: Poly, plane: &Plane, eps: f64) -> Poly {
    let (mut lowest, mut highest) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in poly.faces.iter().flatten() {
        let s = plane.side(*p);
        lowest = lowest.min(s);
        highest = highest.max(s);
    }
    if highest <= eps {
        return poly;
    }
    if lowest >= -eps {
        return Poly::default();
    }
    let mut faces: Vec<Vec<V3>> = Vec::with_capacity(poly.faces.len() + 1);
    let mut cap: Vec<V3> = Vec::new();
    for face in &poly.faces {
        let m = face.len();
        let s: Vec<f64> = face.iter().map(|&p| plane.side(p)).collect();
        let mut kept: Vec<V3> = Vec::with_capacity(m + 2);
        for i in 0..m {
            let (p, q, sp, sq) = (face[i], face[(i + 1) % m], s[i], s[(i + 1) % m]);
            if sp <= eps {
                kept.push(p);
                if sp >= -eps {
                    cap.push(p);
                }
            }
            if (sp < -eps && sq > eps) || (sp > eps && sq < -eps) {
                let x = lerp(p, q, sp / (sp - sq));
                kept.push(x);
                cap.push(x);
            }
        }
        if kept.len() >= 3 {
            faces.push(kept);
        }
    }
    // The cut is a convex polygon in the plane, the hull of the points where the faces met it.
    let hull = hull_ccw(&dedup(cap, 10.0 * eps), plane.n);
    if hull.len() >= 3 {
        faces.push(hull);
    }
    Poly { faces }
}

/// `points` without those within `tol` of an earlier one (per axis).
fn dedup(points: Vec<V3>, tol: f64) -> Vec<V3> {
    let mut out: Vec<V3> = Vec::with_capacity(points.len());
    for p in points {
        if !out.iter().any(|q| (0..3).all(|k| (p[k] - q[k]).abs() <= tol)) {
            out.push(p);
        }
    }
    out
}

/// The convex hull of coplanar `points` (on a plane with unit normal `n`), counter-clockwise seen
/// from the side `n` points to; collinear points are dropped. Fewer than three points result when
/// the points are collinear.
fn hull_ccw(points: &[V3], n: V3) -> Vec<V3> {
    if points.len() < 3 {
        return Vec::new();
    }
    let axis = if n[0].abs() <= n[1].abs() && n[0].abs() <= n[2].abs() {
        [1.0, 0.0, 0.0]
    } else if n[1].abs() <= n[2].abs() {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let e1 = cross(n, axis);
    let len = dot(e1, e1).sqrt();
    let e1 = [e1[0] / len, e1[1] / len, e1[2] / len];
    let e2 = cross(n, e1); // e1 × e2 = n
    let mut pts: Vec<(f64, f64, V3)> = points.iter().map(|&p| (dot(p, e1), dot(p, e2), p)).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let turn = |o: &(f64, f64, V3), a: &(f64, f64, V3), b: &(f64, f64, V3)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let chain = |order: &mut dyn Iterator<Item = &(f64, f64, V3)>| {
        let mut out: Vec<(f64, f64, V3)> = Vec::new();
        for p in order {
            while out.len() >= 2 && turn(&out[out.len() - 2], &out[out.len() - 1], p) <= 0.0 {
                out.pop();
            }
            out.push(*p);
        }
        out.pop();
        out
    };
    let mut hull = chain(&mut pts.iter());
    hull.extend(chain(&mut pts.iter().rev()));
    hull.into_iter().map(|p| p.2).collect()
}

/// The bounding box diagonal of `points`.
fn diagonal(points: &[V3]) -> f64 {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let d = sub(hi, lo);
    dot(d, d).sqrt()
}

/// The distinct face planes of the closed mesh `triangles` over `points` if it bounds a convex
/// solid, i.e. every vertex lies behind (to within `1e-9` of the bounding box diagonal) the plane
/// of every non-degenerate triangle. `None` for a mesh that is not convex.
fn convex_planes(points: &[V3], triangles: &[[u32; 3]]) -> Option<Vec<Plane>> {
    let diag = diagonal(points);
    let mut planes: Vec<Plane> = Vec::new();
    for t in triangles {
        let [a, b, c] = t.map(|i| points[i as usize]);
        let Some(plane) = Plane::of_triangle(a, b, c) else { continue };
        if planes.iter().any(|q| q.same_as(&plane, 1e-10 * diag)) {
            continue;
        }
        if points.iter().any(|&p| plane.side(p) > 1e-9 * diag) {
            return None;
        }
        planes.push(plane);
    }
    Some(planes)
}

fn to_v3(m: &Mesh) -> Vec<V3> {
    m.positions.iter().map(|p| [p.x, p.y, p.z]).collect()
}

/// Whether the closed mesh `m` bounds a convex solid (a box, a prism over a convex polygon, …):
/// every vertex is behind the plane of every non-degenerate triangle, to within `1e-9` of the
/// bounding box diagonal. An open mesh is not convex.
pub fn is_convex(m: &Mesh) -> bool {
    m.is_closed() && convex_planes(&to_v3(m), &m.triangles).is_some()
}

/// The cones from the origin to the triangles of `m` (see the module docs).
fn cones(points: &[V3], triangles: &[[u32; 3]]) -> Vec<(Poly, f64)> {
    triangles
        .iter()
        .filter_map(|t| {
            let [a, b, c] = t.map(|i| points[i as usize]);
            Poly::cone(a, b, c)
        })
        .collect()
}

/// `Σ_i s_i · vol(cone_i ∩ ⋂ planes)`.
fn fan_vs_planes(fan: &[(Poly, f64)], planes: &[Plane], eps: f64) -> f64 {
    let mut total = 0.0;
    for (cone, sign) in fan {
        let mut piece = cone.clone();
        for plane in planes {
            piece = clip(piece, plane, eps);
            if piece.is_empty() {
                break;
            }
        }
        total += sign * piece.volume();
    }
    total
}

/// `Σ_i Σ_j s_i s_j · vol(cone_i ∩ cone_j)`.
fn fan_vs_fan(a: &[(Poly, f64)], b: &[(Poly, f64)], eps: f64) -> f64 {
    let planes: Vec<Vec<Plane>> = b.iter().map(|(cone, _)| cone.planes()).collect();
    let mut total = 0.0;
    for (cone, sa) in a {
        for ((_, sb), planes) in b.iter().zip(&planes) {
            let mut piece = cone.clone();
            for plane in planes {
                piece = clip(piece, plane, eps);
                if piece.is_empty() {
                    break;
                }
            }
            total += sa * sb * piece.volume();
        }
    }
    total
}

/// The exact volume of `a ∩ b` for two closed, consistently outward-oriented meshes (see
/// [`Mesh::is_closed`]; an open mesh gives a meaningless number). Exact up to rounding for any
/// such pair, nested or touching ones included; 0 for disjoint ones and for solids that only touch.
/// Cost: the triangle count of one mesh times the plane count of the other when one is convex
/// (the cheaper way round is taken), the product of the triangle counts otherwise.
pub fn intersection_volume(a: &Mesh, b: &Mesh) -> f64 {
    let (Some((alo, ahi)), Some((blo, bhi))) = (a.bounds(), b.bounds()) else { return 0.0 };
    let lo = [alo.x.max(blo.x), alo.y.max(blo.y), alo.z.max(blo.z)];
    let hi = [ahi.x.min(bhi.x), ahi.y.min(bhi.y), ahi.z.min(bhi.z)];
    if (0..3).any(|k| !(hi[k] > lo[k])) {
        return 0.0;
    }
    // The origin at the centre of the boxes' overlap keeps every number small and relevant.
    let o = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let shifted = |m: &Mesh| -> Vec<V3> { to_v3(m).into_iter().map(|p| sub(p, o)).collect() };
    let (pa, pb) = (shifted(a), shifted(b));
    let extent = pa.iter().chain(&pb).flatten().fold(0.0f64, |m, x| m.max(x.abs()));
    if !extent.is_finite() || extent == 0.0 {
        return 0.0;
    }
    let eps = 1e-12 * extent;
    let (convex_a, convex_b) = (convex_planes(&pa, &a.triangles), convex_planes(&pb, &b.triangles));
    let volume = match (convex_a, convex_b) {
        (Some(planes_a), Some(planes_b)) => {
            // Fan the mesh that costs less against the other's planes.
            if a.triangles.len() * planes_b.len() <= b.triangles.len() * planes_a.len() {
                fan_vs_planes(&cones(&pa, &a.triangles), &planes_b, eps)
            } else {
                fan_vs_planes(&cones(&pb, &b.triangles), &planes_a, eps)
            }
        }
        (_, Some(planes_b)) => fan_vs_planes(&cones(&pa, &a.triangles), &planes_b, eps),
        (Some(planes_a), None) => fan_vs_planes(&cones(&pb, &b.triangles), &planes_a, eps),
        (None, None) => fan_vs_fan(&cones(&pa, &a.triangles), &cones(&pb, &b.triangles), eps),
    };
    volume.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{box_union, Aabb};
    use crate::segment::{segment_mesh, Section};

    fn aabb(lo: [f64; 3], hi: [f64; 3]) -> Aabb {
        Aabb { lo, hi }
    }

    /// The closed outward mesh of the union of boxes.
    fn boxes(bs: &[Aabb]) -> Mesh {
        let (v, m) = box_union(bs);
        assert!(v > 0.0 && m.is_closed());
        m
    }

    fn overlap(a: &Aabb, b: &Aabb) -> f64 {
        (0..3).map(|k| (a.hi[k].min(b.hi[k]) - a.lo[k].max(b.lo[k])).max(0.0)).product()
    }

    #[test]
    fn clipping_a_tetrahedron_by_a_plane() {
        let (tet, sign) = Poly::cone([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]).unwrap();
        assert_eq!(sign, 1.0);
        assert!((tet.volume() - 1.0 / 6.0).abs() < 1e-15);
        let eps = 1e-12;
        // x ≤ 0.5 keeps 1 − (1/2)³ of it, x ≤ 0 nothing, x ≤ 1 everything.
        let half = clip(tet.clone(), &Plane { n: [1.0, 0.0, 0.0], d: 0.5 }, eps);
        assert!((half.volume() - (1.0 - 0.125) / 6.0).abs() < 1e-15, "{}", half.volume());
        let other = clip(tet.clone(), &Plane { n: [-1.0, 0.0, 0.0], d: -0.5 }, eps);
        assert!((half.volume() + other.volume() - 1.0 / 6.0).abs() < 1e-15);
        assert!(clip(tet.clone(), &Plane { n: [1.0, 0.0, 0.0], d: 0.0 }, eps).is_empty());
        assert!((clip(tet.clone(), &Plane { n: [1.0, 0.0, 0.0], d: 1.0 }, eps).volume() - 1.0 / 6.0).abs() < 1e-15);
        // A plane through the apex and one edge halves the volume exactly.
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let halved = clip(tet.clone(), &Plane { n: [s, -s, 0.0], d: 0.0 }, eps);
        assert!((halved.volume() - 1.0 / 12.0).abs() < 1e-15, "{}", halved.volume());
        // A plane through a vertex (and cutting the opposite face), and one through an edge.
        let through_vertex = clip(tet.clone(), &Plane { n: [0.0, 1.0, 1.0], d: 0.0 }, eps);
        assert!(through_vertex.is_empty());
        let skew = Plane { n: [0.6, 0.0, 0.8], d: 0.6 * 0.4 };
        let v = clip(tet, &skew, eps).volume();
        assert!(v > 0.0 && v < 1.0 / 6.0);
    }

    #[test]
    fn a_face_lying_in_the_plane_is_not_doubled() {
        // The cube [0,1]³ clipped by x ≤ 1 (a face lies in the plane) is unchanged; by x ≤ 0 it vanishes.
        let c = boxes(&[aabb([0.0; 3], [1.0; 3])]);
        let pts: Vec<V3> = to_v3(&c);
        let fan = cones(&pts.iter().map(|p| sub(*p, [0.5, 0.5, 0.5])).collect::<Vec<_>>(), &c.triangles);
        let half = |d: f64| fan_vs_planes(&fan, &[Plane { n: [1.0, 0.0, 0.0], d }], 1e-12);
        assert!((half(0.5) - 1.0).abs() < 1e-14, "{}", half(0.5));
        assert!((half(0.0) - 0.5).abs() < 1e-14, "{}", half(0.0));
        assert!((half(-0.5)).abs() < 1e-14);
        assert!((half(1.0) - 1.0).abs() < 1e-14);
    }

    #[test]
    fn overlapping_boxes_give_the_product_of_the_overlaps() {
        let a = aabb([0.0, 0.0, 0.0], [2.0, 3.0, 1.0]);
        for b in [
            aabb([1.0, 1.0, 0.5], [4.0, 2.0, 5.0]),
            aabb([-1.0, -1.0, -1.0], [1.0, 1.5, 0.25]),
            aabb([0.5, 0.5, 0.25], [1.5, 2.5, 0.75]), // inside
            aabb([-5.0, -5.0, -5.0], [5.0, 5.0, 5.0]), // around
            aabb([0.0, 0.0, 0.0], [2.0, 3.0, 1.0]),   // the same
            aabb([1.0, 0.0, 0.0], [2.5, 3.0, 1.0]),    // shared faces
        ] {
            let v = intersection_volume(&boxes(&[a]), &boxes(&[b]));
            assert!((v - overlap(&a, &b)).abs() < 1e-13, "{b:?}: {v} vs {}", overlap(&a, &b));
            let w = intersection_volume(&boxes(&[b]), &boxes(&[a]));
            assert!((w - v).abs() < 1e-13);
        }
    }

    #[test]
    fn touching_and_disjoint_solids_have_no_common_volume() {
        let a = boxes(&[aabb([0.0; 3], [1.0; 3])]);
        for b in [
            aabb([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]), // face
            aabb([1.0, 1.0, 0.0], [2.0, 2.0, 1.0]), // edge
            aabb([1.0, 1.0, 1.0], [2.0, 2.0, 2.0]), // corner
            aabb([0.0, 0.0, 1.0], [0.5, 0.5, 2.0]), // part of a face
            aabb([5.0, 5.0, 5.0], [6.0, 6.0, 6.0]),
        ] {
            assert_eq!(intersection_volume(&a, &boxes(&[b])), 0.0, "{b:?}");
        }
        assert_eq!(intersection_volume(&a, &Mesh::default()), 0.0);
    }

    #[test]
    fn convexity_and_plane_counts() {
        let cube = boxes(&[aabb([0.0; 3], [1.0; 3])]);
        assert!(is_convex(&cube));
        let pts = to_v3(&cube);
        assert_eq!(convex_planes(&pts, &cube.triangles).unwrap().len(), 6);
        let pipe = segment_mesh([0.0, 0.0, 0.0], [1.0, 2.0, 0.5], Section::Round { diameter: 0.1 }).unwrap();
        assert!(is_convex(&pipe));
        assert_eq!(convex_planes(&to_v3(&pipe), &pipe.triangles).unwrap().len(), 18);
        let duct = segment_mesh([0.0, 0.0, 0.0], [3.0, 1.0, 1.0], Section::Rect { width: 0.3, height: 0.2 }).unwrap();
        assert_eq!(convex_planes(&to_v3(&duct), &duct.triangles).unwrap().len(), 6);
        // An L, a ring and an open surface are not convex.
        let l = boxes(&[aabb([0.0; 3], [2.0, 1.0, 1.0]), aabb([0.0; 3], [1.0, 2.0, 1.0])]);
        assert!(!is_convex(&l));
        let mut open = cube.clone();
        open.triangles.pop();
        assert!(!is_convex(&open));
        assert!(!is_convex(&Mesh::default()));
    }

    #[test]
    fn an_l_shape_is_not_convex_and_clips_like_its_two_boxes() {
        let l = boxes(&[aabb([0.0; 3], [4.0, 1.0, 1.0]), aabb([0.0; 3], [1.0, 3.0, 1.0])]);
        let cutter = boxes(&[aabb([0.5, 0.5, 0.25], [3.0, 2.0, 0.75])]);
        // The cutter overlaps the long arm 2.5 × 0.5 × 0.5 and the short arm's part above y = 1.
        let want = 2.5 * 0.5 * 0.5 + 0.5 * 1.0 * 0.5;
        assert!((intersection_volume(&l, &cutter) - want).abs() < 1e-13);
        assert!((intersection_volume(&cutter, &l) - want).abs() < 1e-13);
    }

    /// A tiny deterministic generator.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.next()
        }
        fn boxy(&mut self, size: f64) -> Aabb {
            let lo = [self.range(0.0, 4.0), self.range(0.0, 4.0), self.range(0.0, 4.0)];
            aabb(lo, [lo[0] + self.range(0.2, size), lo[1] + self.range(0.2, size), lo[2] + self.range(0.2, size)])
        }
    }

    /// Unions of boxes are closed, non-convex meshes; their intersection is the union of the
    /// pairwise intersections of the boxes, which `box_union` computes independently.
    #[test]
    fn random_unions_of_boxes_match_the_union_of_the_pairwise_intersections() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let (mut nonconvex, mut positive) = (0, 0);
        for case in 0..60 {
            let xs: Vec<Aabb> = (0..1 + case % 4).map(|_| rng.boxy(3.0)).collect();
            let ys: Vec<Aabb> = (0..1 + (case / 4) % 3).map(|_| rng.boxy(3.0)).collect();
            let (va, ma) = box_union(&xs);
            let (vb, mb) = box_union(&ys);
            if va == 0.0 || vb == 0.0 {
                continue; // boxes that touch only along an edge give no mesh
            }
            let common: Vec<Aabb> = xs
                .iter()
                .flat_map(|x| ys.iter().map(move |y| aabb(std::array::from_fn(|k| x.lo[k].max(y.lo[k])), std::array::from_fn(|k| x.hi[k].min(y.hi[k])))))
                .filter(|b| (0..3).all(|k| b.hi[k] - b.lo[k] > 1e-6))
                .collect();
            let want = if common.is_empty() { 0.0 } else { box_union(&common).0 };
            let got = intersection_volume(&ma, &mb);
            assert!((got - want).abs() < 1e-11, "case {case}: {got} vs {want}");
            nonconvex += (!is_convex(&ma) && !is_convex(&mb)) as u32;
            positive += (want > 0.0) as u32;
        }
        assert!(nonconvex > 5 && positive > 20, "{nonconvex} {positive}");
    }

    #[test]
    fn a_round_pipe_through_a_wall_and_through_a_hole() {
        let a16 = 8.0 * (std::f64::consts::PI / 8.0).sin() * 0.05 * 0.05;
        let wall = boxes(&[aabb([0.0, -0.1, 0.0], [6.0, 0.1, 3.0])]);
        let pipe = segment_mesh([2.0, -1.0, 1.5], [2.0, 1.0, 1.5], Section::Round { diameter: 0.1 }).unwrap();
        assert!((intersection_volume(&wall, &pipe) - a16 * 0.2).abs() < 1e-14);
        // A wall with a window (a union of boxes around the hole): the pipe through the hole misses it.
        let framed = boxes(&[
            aabb([0.0, -0.1, 0.0], [4.6, 0.1, 3.0]),
            aabb([5.4, -0.1, 0.0], [6.0, 0.1, 3.0]),
            aabb([4.6, -0.1, 0.0], [5.4, 0.1, 0.5]),
            aabb([4.6, -0.1, 2.0], [5.4, 0.1, 3.0]),
        ]);
        assert!(!is_convex(&framed));
        let through_hole = segment_mesh([5.0, -1.0, 1.2], [5.0, 1.0, 1.2], Section::Round { diameter: 0.1 }).unwrap();
        assert!(intersection_volume(&framed, &through_hole).abs() < 1e-14);
        let through_wall = segment_mesh([2.0, -1.0, 1.5], [2.0, 1.0, 1.5], Section::Round { diameter: 0.1 }).unwrap();
        assert!((intersection_volume(&framed, &through_wall) - a16 * 0.2).abs() < 1e-14);
        // A pipe lying along the sill, half in the frame.
        let on_sill = segment_mesh([4.0, 0.0, 0.5], [6.0, 0.0, 0.5], Section::Round { diameter: 0.1 }).unwrap();
        let v = intersection_volume(&framed, &on_sill);
        assert!(v > 0.0 && v < a16 * 2.0);
    }

    #[test]
    fn a_pipe_inside_a_box_and_a_box_inside_a_pipe() {
        let big = boxes(&[aabb([-1.0; 3], [1.0; 3])]);
        let pipe = segment_mesh([-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], Section::Round { diameter: 0.2 }).unwrap();
        assert!((intersection_volume(&big, &pipe) - pipe.volume()).abs() < 1e-14);
        let small = boxes(&[aabb([-0.01, -0.01, -0.01], [0.01, 0.01, 0.01])]);
        let fat = segment_mesh([-5.0, 0.0, 0.0], [5.0, 0.0, 0.0], Section::Round { diameter: 2.0 }).unwrap();
        assert!((intersection_volume(&fat, &small) - 8e-6).abs() < 1e-15);
        // One component of a two-part mesh inside the other solid.
        let two = boxes(&[aabb([-0.2; 3], [0.2; 3]), aabb([7.0; 3], [8.0; 3])]);
        assert!((intersection_volume(&big, &two) - 0.4f64.powi(3)).abs() < 1e-14);
    }

    #[test]
    fn rotated_prisms_match_the_exact_boolean() {
        use bim_geom::{boolean, BoolOp};
        let cases = [
            ([0.0, 0.0, 0.0], [3.0, 1.0, 0.5], [1.0, -1.0, 0.0], [1.5, 2.0, 1.0]),
            ([0.0, 0.0, 0.0], [1.0, 0.0, 2.0], [0.3, 0.2, -0.5], [0.4, 0.9, 1.7]),
            ([0.0, 0.0, 0.0], [2.0, 2.0, 0.0], [1.0, 3.0, 0.1], [1.0, -1.0, 0.3]),
        ];
        for (a0, a1, b0, b1) in cases {
            let round = segment_mesh(a0, a1, Section::Round { diameter: 0.6 }).unwrap();
            let duct = segment_mesh(b0, b1, Section::Rect { width: 0.5, height: 0.4 }).unwrap();
            let got = intersection_volume(&round, &duct);
            let exact = boolean(&round.to_solid(), &duct.to_solid(), BoolOp::Intersection).unwrap().volume();
            assert!(exact > 1e-4, "the case should overlap: {exact}");
            assert!((got - exact).abs() < 1e-9 * exact.max(1.0), "{got} vs {exact}");
        }
    }
}
