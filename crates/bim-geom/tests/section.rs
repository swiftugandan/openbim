use bim_geom::{boolean, section, section_area, BoolOp, Plane3, Point2, Point3, Polygon, Polygon3, Solid, Vec3};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point2> {
    vec![Point2::new(x0, y0), Point2::new(x1, y0), Point2::new(x1, y1), Point2::new(x0, y1)]
}

fn prism(outer: Vec<Point2>, holes: Vec<Vec<Point2>>, z0: f64, h: f64) -> Solid {
    Solid::extrude(&Polygon { outer, holes }, z0, h)
}

fn cube() -> Solid {
    prism(rect(0.0, 0.0, 1.0, 1.0), vec![], 0.0, 1.0)
}

fn plane(x: f64, y: f64, z: f64, offset: f64) -> Plane3 {
    Plane3 { normal: Vec3::new(x, y, z), offset }
}

/// Signed area of a ring about the unit normal `n` (relative to its first vertex, so it stays
/// accurate far from the origin).
fn ring_area(r: &[Point3], n: Vec3) -> f64 {
    let o = r[0];
    let mut s = Vec3::ZERO;
    for i in 0..r.len() {
        s += (r[i] - o).cross(r[(i + 1) % r.len()] - o);
    }
    0.5 * s.dot(n)
}

/// Checks the invariants of a section and returns its area.
fn check(polys: &[Polygon3], p: &Plane3) -> f64 {
    let n = p.normal.normalized().unwrap();
    let off = p.offset / p.normal.norm();
    let mut total = 0.0;
    for poly in polys {
        for r in std::iter::once(&poly.outer).chain(poly.holes.iter()) {
            assert!(r.len() >= 3);
            for q in r {
                // The T2.5 judge's bound, and a few ulps of the coordinates when that is tighter.
                let tol = (1e-7 * off.abs().max(1.0)).min(1e-9 * q.to_vec().norm().max(off.abs()).max(1.0));
                assert!((q.to_vec().dot(n) - off).abs() <= tol, "vertex off the plane");
            }
        }
        let a = ring_area(&poly.outer, n);
        assert!(a > 0.0, "outer loop not CCW");
        total += a;
        for h in &poly.holes {
            let ha = ring_area(h, n);
            assert!(ha < 0.0, "hole not CW");
            total += ha;
        }
    }
    total
}

fn area_of(s: &Solid, p: Plane3) -> (usize, f64) {
    let polys = section(s, &p);
    let a = check(&polys, &p);
    let sa = section_area(s, &p);
    assert!((a - sa).abs() <= 1e-12 * a.abs().max(sa.abs()).max(1e-300), "section_area {sa} != loops {a}");
    (polys.len(), a)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Relative closeness, for areas of solids far from the origin or much smaller than 1.
fn rel_close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs()
}

#[test]
fn cube_interior_and_faces() {
    let c = cube();
    let (n, a) = area_of(&c, plane(0.0, 0.0, 1.0, 0.5));
    assert_eq!(n, 1);
    assert!(close(a, 1.0));
    // Coplanar faces are part of the section, whichever way the normal points.
    for p in [plane(0.0, 0.0, 1.0, 1.0), plane(0.0, 0.0, -1.0, 0.0), plane(0.0, 0.0, 1.0, 0.0), plane(1.0, 0.0, 0.0, 1.0)] {
        let (n, a) = area_of(&c, p);
        assert_eq!(n, 1);
        assert!(close(a, 1.0));
    }
}

#[test]
fn cube_edge_and_vertex_contact_is_empty() {
    let c = cube();
    for p in [
        plane(1.0, 1.0, 0.0, 0.0),
        plane(-1.0, -1.0, 0.0, 0.0),
        plane(1.0, 1.0, 1.0, 0.0),
        plane(-1.0, -1.0, -1.0, 0.0),
        plane(1.0, 1.0, 1.0, 3.0),
        plane(0.0, 0.0, 1.0, 1.5),
    ] {
        assert!(section(&c, &p).is_empty());
        assert_eq!(section_area(&c, &p), 0.0);
    }
}

#[test]
fn cube_through_vertices_and_scaled_normal() {
    let c = cube();
    let s3 = 3f64.sqrt();
    // Triangle through three vertices: area sqrt(3)/2.
    let (n, a) = area_of(&c, plane(1.0 / s3, 1.0 / s3, 1.0 / s3, 1.0 / s3));
    assert_eq!(n, 1);
    assert!(close(a, s3 / 2.0));
    // Regular hexagon of side sqrt(2)/2 with a non-unit normal.
    let (n, a) = area_of(&c, plane(2.5, 2.5, 2.5, 3.75));
    assert_eq!(n, 1);
    assert!(close(a, 3.0 * s3 / 4.0));
    // Diagonal plane through two vertical edges: rectangle sqrt(2) x 1.
    let (_, a) = area_of(&c, plane(1.0, -1.0, 0.0, 0.0));
    assert!(close(a, 2f64.sqrt()));
}

#[test]
fn tube_hole_and_island() {
    let tube = prism(rect(0.0, 0.0, 4.0, 4.0), vec![rect(1.0, 1.0, 3.0, 3.0).into_iter().rev().collect()], 0.0, 2.0);
    let polys = section(&tube, &plane(0.0, 0.0, 1.0, 1.0));
    assert_eq!(polys.len(), 1);
    assert_eq!(polys[0].holes.len(), 1);
    assert!(close(check(&polys, &plane(0.0, 0.0, 1.0, 1.0)), 12.0));
    // Vertical plane through the hole: two separate rectangles.
    let (n, a) = area_of(&tube, plane(1.0, 0.0, 0.0, 2.0));
    assert_eq!(n, 2);
    assert!(close(a, 4.0));
    // Plane on the hole's wall: the whole 4 x 2 strip.
    let (n, a) = area_of(&tube, plane(1.0, 0.0, 0.0, 1.0));
    assert_eq!(n, 1);
    assert!(close(a, 8.0));

    let islet = prism(rect(1.5, 1.5, 2.5, 2.5), vec![], 0.0, 2.0);
    let island = boolean(&tube, &islet, BoolOp::Union).unwrap();
    let polys = section(&island, &plane(0.0, 0.0, 1.0, 0.5));
    assert_eq!(polys.len(), 2);
    let holes: usize = polys.iter().map(|p| p.holes.len()).sum();
    assert_eq!(holes, 1);
    assert!(close(check(&polys, &plane(0.0, 0.0, 1.0, 0.5)), 13.0));
}

#[test]
fn coplanar_faces_of_booleans() {
    let a = prism(rect(0.0, 0.0, 2.0, 1.0), vec![], 0.0, 1.0);
    let b = prism(rect(0.0, 0.0, 1.0, 1.0), vec![], 1.0, 1.0);
    let step = boolean(&a, &b, BoolOp::Union).unwrap();
    for p in [plane(0.0, 0.0, 1.0, 1.0), plane(0.0, 0.0, -1.0, -1.0)] {
        let (n, ar) = area_of(&step, p);
        assert_eq!(n, 1);
        assert!(close(ar, 2.0));
    }
    let slab = prism(rect(0.0, 0.0, 3.0, 3.0), vec![], 0.0, 1.0);
    let cutter = prism(rect(1.0, 1.0, 2.0, 2.0), vec![], 0.5, 1.5);
    let pocket = boolean(&slab, &cutter, BoolOp::Difference).unwrap();
    let (n, ar) = area_of(&pocket, plane(0.0, 0.0, 1.0, 0.5));
    assert_eq!(n, 1);
    assert!(close(ar, 9.0));
    let (_, ar) = area_of(&pocket, plane(0.0, 0.0, 1.0, 0.75));
    assert!(close(ar, 8.0));
}

#[test]
fn regions_touching_at_a_point_stay_separate() {
    // Two cubes sharing only a vertical edge: the section is two squares touching at a corner.
    let a = cube();
    let b = prism(rect(1.0, 1.0, 2.0, 2.0), vec![], 0.0, 1.0);
    let both = boolean(&a, &b, BoolOp::Union).unwrap();
    let polys = section(&both, &plane(0.0, 0.0, 1.0, 0.5));
    assert_eq!(polys.len(), 2);
    assert!(polys.iter().all(|p| p.outer.len() == 4 && p.holes.is_empty()));
    assert!(close(check(&polys, &plane(0.0, 0.0, 1.0, 0.5)), 2.0));
}

#[test]
fn self_intersecting_profile_uses_positive_fill() {
    // A bow-tie: one lobe counter-clockwise (kept), the other clockwise (dropped).
    let bow = vec![Point2::new(0.0, 0.0), Point2::new(2.0, 2.0), Point2::new(2.0, 0.0), Point2::new(0.0, 2.0)];
    let s = Solid::extrude(&Polygon { outer: bow, holes: vec![] }, 0.0, 1.0);
    assert!(s.is_closed_manifold());
    assert!(close(s.volume(), 1.0));
    let (n, a) = area_of(&s, plane(0.0, 0.0, 1.0, 0.5));
    assert_eq!(n, 1);
    assert!(close(a, 1.0));
}

/// Axis-aligned box `[x, x + sx] × [y, y + sy] × [z, z + sz]`.
fn boxed(x: f64, y: f64, z: f64, sx: f64, sy: f64, sz: f64) -> Solid {
    prism(rect(x, y, x + sx, y + sy), vec![], z, sz)
}

#[test]
fn unit_cube_far_from_origin() {
    for x in [1e5, 1e6] {
        let c = boxed(x, 0.0, 0.0, 1.0, 1.0, 1.0);
        for p in [plane(0.0, 0.0, 1.0, 0.5), plane(1.0, 0.0, 0.0, x + 0.5), plane(0.0, -2.0, 0.0, -1.0)] {
            let (n, a) = area_of(&c, p);
            assert_eq!(n, 1, "x = {x}, {p:?}");
            assert!(rel_close(a, 1.0), "x = {x}, {p:?}: area {a}");
        }
        // Diagonal plane through two opposite vertical edges: rectangle sqrt(2) x 1.
        let (n, a) = area_of(&c, plane(1.0, -1.0, 0.0, x));
        assert_eq!(n, 1);
        assert!(rel_close(a, 2f64.sqrt()), "x = {x}: diagonal area {a}");
        // Plane exactly on a face: the coplanar face is the section, for either normal direction.
        for p in [plane(0.0, 0.0, 1.0, 1.0), plane(0.0, 0.0, -1.0, -1.0), plane(1.0, 0.0, 0.0, x), plane(-1.0, 0.0, 0.0, -(x + 1.0))] {
            let (n, a) = area_of(&c, p);
            assert_eq!(n, 1, "x = {x}, {p:?}");
            assert!(rel_close(a, 1.0), "x = {x}, {p:?}: area {a}");
        }
        // Contact in an edge or a vertex only: empty.
        for p in [
            plane(1.0, -1.0, 0.0, x + 1.0),
            plane(1.0, 1.0, 0.0, x + 2.0),
            plane(-1.0, -1.0, 0.0, -x),
            plane(1.0, 0.0, 1.0, x + 2.0),
            plane(1.0, 1.0, 1.0, x + 3.0),
            plane(-1.0, -1.0, -1.0, -x),
        ] {
            assert!(section(&c, &p).is_empty(), "x = {x}, {p:?}");
            assert_eq!(section_area(&c, &p).to_bits(), 0.0f64.to_bits(), "x = {x}, {p:?}");
        }
    }
    // A larger cube at 1e5.
    let c3 = boxed(1e5, 0.0, 0.0, 3.0, 3.0, 3.0);
    let (n, a) = area_of(&c3, plane(0.0, 0.0, 1.0, 1.5));
    assert_eq!(n, 1);
    assert!(rel_close(a, 9.0));
}

#[test]
fn box_in_projected_coordinates() {
    // A 10 x 10 x 3 box at UTM-like coordinates.
    let b = boxed(5e5, 5e6, 0.0, 10.0, 10.0, 3.0);
    let (n, a) = area_of(&b, plane(0.0, 0.0, 1.0, 1.5));
    assert_eq!(n, 1);
    assert!(rel_close(a, 100.0), "horizontal area {a}");
    for p in [plane(1.0, 0.0, 0.0, 5e5 + 5.0), plane(0.0, 1.0, 0.0, 5e6 + 5.0)] {
        let (n, a) = area_of(&b, p);
        assert_eq!(n, 1);
        assert!(rel_close(a, 30.0), "{p:?}: vertical area {a}");
    }
    // Top face, and a vertical edge only.
    let (n, a) = area_of(&b, plane(0.0, 0.0, 1.0, 3.0));
    assert_eq!(n, 1);
    assert!(rel_close(a, 100.0));
    assert!(section(&b, &plane(1.0, 1.0, 0.0, 5e5 + 5e6)).is_empty());

    // A tube with a hole, far away: the hole is still nested in its outer loop.
    let (x, y) = (5e5, 5e6);
    let tube = prism(rect(x, y, x + 4.0, y + 4.0), vec![rect(x + 1.0, y + 1.0, x + 3.0, y + 3.0).into_iter().rev().collect()], 0.0, 2.0);
    let p = plane(0.0, 0.0, 1.0, 1.0);
    let polys = section(&tube, &p);
    assert_eq!(polys.len(), 1);
    assert_eq!(polys[0].holes.len(), 1);
    assert!(rel_close(check(&polys, &p), 12.0));
}

#[test]
fn thin_column_far_from_origin() {
    let (x, y) = (1e4, 5e3);
    let col = prism(rect(x, y, x + 0.3, y + 0.3), vec![], 0.0, 3.0);
    // The profile's representable width is (x + 0.3) - x, not exactly 0.3.
    let (w, h) = ((x + 0.3) - x, (y + 0.3) - y);
    let (n, a) = area_of(&col, plane(0.0, 0.0, 1.0, 1.5));
    assert_eq!(n, 1);
    assert!(rel_close(a, w * h), "area {a}");
    assert!((a - 0.09).abs() < 1e-11);
}

#[test]
fn tiny_cube_near_origin() {
    let s = 1e-5;
    let c = boxed(0.0, 0.0, 0.0, s, s, s);
    for p in [plane(0.0, 0.0, 1.0, s / 2.0), plane(1.0, 0.0, 0.0, s / 2.0), plane(0.0, 0.0, 1.0, s), plane(0.0, 0.0, -1.0, 0.0)] {
        let (n, a) = area_of(&c, p);
        assert_eq!(n, 1, "{p:?}");
        assert!(rel_close(a, s * s), "{p:?}: area {a}");
    }
    let (n, a) = area_of(&c, plane(1.0, -1.0, 0.0, 0.0));
    assert_eq!(n, 1);
    assert!(rel_close(a, 2f64.sqrt() * s * s));
    for p in [plane(1.0, 1.0, 0.0, 0.0), plane(1.0, -1.0, 0.0, s), plane(1.0, 1.0, 1.0, 3.0 * s), plane(0.0, 0.0, 1.0, 1.5 * s)] {
        assert!(section(&c, &p).is_empty(), "{p:?}");
        assert_eq!(section_area(&c, &p).to_bits(), 0.0f64.to_bits());
    }
}

#[test]
fn polygon_area_matches_section_area_far_away() {
    let c = boxed(1e6, 1e6, 0.0, 1.0, 1.0, 1.0);
    let p = plane(1.0, 1.0, 1.0, 2e6 + 1.5);
    let polys = section(&c, &p);
    assert_eq!(polys.len(), 1);
    let a: f64 = polys.iter().map(|q| q.area(p.normal)).sum();
    // Regular hexagon of side sqrt(2)/2.
    assert!(rel_close(a, 3.0 * 3f64.sqrt() / 4.0), "area {a}");
    assert!(rel_close(section_area(&c, &p), 3.0 * 3f64.sqrt() / 4.0));
}

#[test]
fn small_hole_in_large_far_slab_is_kept() {
    // A 1 km slab with a 5 cm square hole, near the origin and at projected coordinates.
    for (x, y) in [(0.0, 0.0), (2e5, 3e5)] {
        let hole: Vec<Point2> = rect(x + 500.0, y + 500.0, x + 500.05, y + 500.05).into_iter().rev().collect();
        let (hw, hh) = ((x + 500.05) - (x + 500.0), (y + 500.05) - (y + 500.0));
        let slab = prism(rect(x, y, x + 1000.0, y + 1000.0), vec![hole], 0.0, 10.0);
        let p = plane(0.0, 0.0, 1.0, 5.0);
        let polys = section(&slab, &p);
        assert_eq!(polys.len(), 1);
        assert_eq!(polys[0].holes.len(), 1, "hole dropped at ({x}, {y})");
        let a = check(&polys, &p);
        assert!((a - (1e6 - hw * hh)).abs() < 1e-6, "area {a}");
        // Vertical plane through the hole: two strips of total area (1000 - 0.05) x 10.
        let (n, a) = area_of(&slab, plane(1.0, 0.0, 0.0, x + 500.025));
        assert_eq!(n, 2);
        assert!((a - (1000.0 - hh) * 10.0).abs() < 1e-6, "vertical area {a}");
    }
}
