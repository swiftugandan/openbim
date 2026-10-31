// External judge for T1.3 (bim-solver): closed-form sketches with known answers.
use bim_solver::{Constraint::*, Sketch, Status};

fn close(p: (f64, f64), q: (f64, f64)) -> bool {
    (p.0 - q.0).abs() < 1e-7 && (p.1 - q.1).abs() < 1e-7
}

fn dist(p: (f64, f64), q: (f64, f64)) -> f64 {
    ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt()
}

/// A 4x3 rectangle anchored at the origin, started from a skewed guess.
fn rectangle() -> (Sketch, [bim_solver::PointId; 4]) {
    let mut s = Sketch::new();
    let p = [s.add_point(0.1, -0.2), s.add_point(4.3, 0.4), s.add_point(3.8, 2.7), s.add_point(-0.3, 3.2)];
    s.add(Fix(p[0], 0.0, 0.0));
    s.add(Horizontal(p[0], p[1]));
    s.add(Distance(p[0], p[1], 4.0));
    s.add(Vertical(p[1], p[2]));
    s.add(Distance(p[1], p[2], 3.0));
    s.add(Horizontal(p[2], p[3]));
    s.add(Vertical(p[3], p[0]));
    (s, p)
}

#[test]
fn rectangle_is_well_constrained() {
    let (mut s, p) = rectangle();
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert_eq!(r.dof, 0);
    assert!(r.residual < 1e-9, "residual {}", r.residual);
    assert!(close(s.point(p[1]), (4.0, 0.0)));
    assert!(close(s.point(p[2]), (4.0, 3.0)));
    assert!(close(s.point(p[3]), (0.0, 3.0)));
}

#[test]
fn redundant_diagonal_is_over_constrained_but_solved() {
    let (mut s, p) = rectangle();
    s.add(Distance(p[0], p[2], 5.0));
    let r = s.solve();
    assert_eq!(r.status, Status::OverConstrained);
    assert!(r.residual < 1e-9);
    assert!(close(s.point(p[2]), (4.0, 3.0)));
}

#[test]
fn free_rotation_is_under_constrained() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(1.0, 1.0);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Distance(a, b, 5.0));
    let r = s.solve();
    assert_eq!(r.status, Status::UnderConstrained);
    assert_eq!(r.dof, 1);
    assert!((dist(s.point(a), s.point(b)) - 5.0).abs() < 1e-9);
}

#[test]
fn contradiction_is_inconsistent() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(3.0, 0.0);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Fix(b, 3.0, 0.0));
    s.add(Distance(a, b, 5.0));
    assert_eq!(s.solve().status, Status::Inconsistent);
}

#[test]
fn angle_places_point_at_sixty_degrees() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(1.0, 0.0);
    let c = s.add_point(0.8, 1.5);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Fix(b, 1.0, 0.0));
    s.add(Distance(a, c, 2.0));
    s.add(Angle(a, b, a, c, std::f64::consts::FRAC_PI_3));
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert!(close(s.point(c), (1.0, 3f64.sqrt())), "{:?}", s.point(c));
}

#[test]
fn parallel_and_perpendicular() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(2.0, 0.0);
    let c = s.add_point(0.0, 1.0);
    let d = s.add_point(1.7, 1.3);
    let e = s.add_point(0.3, 2.6);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Fix(b, 2.0, 0.0));
    s.add(Fix(c, 0.0, 1.0));
    s.add(Parallel(a, b, c, d));
    s.add(Distance(c, d, 2.0));
    s.add(Perpendicular(c, d, c, e));
    s.add(Distance(c, e, 1.5));
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert!(close(s.point(d), (2.0, 1.0)), "{:?}", s.point(d));
    assert!(close(s.point(e), (0.0, 2.5)), "{:?}", s.point(e));
}

#[test]
fn coincident_merges_points() {
    let mut s = Sketch::new();
    let a = s.add_point(1.0, 1.0);
    let b = s.add_point(1.4, 0.7);
    s.add(Fix(a, 1.0, 1.0));
    s.add(Coincident(a, b));
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert!(close(s.point(b), (1.0, 1.0)));
}
