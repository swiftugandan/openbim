use super::Constraint::*;
use super::*;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_3, FRAC_PI_4};

fn close(p: (f64, f64), q: (f64, f64), tol: f64) -> bool {
    (p.0 - q.0).abs() < tol && (p.1 - q.1).abs() < tol
}

fn dist(p: (f64, f64), q: (f64, f64)) -> f64 {
    (p.0 - q.0).hypot(p.1 - q.1)
}

/// Signed CCW angle from a→b to c→d.
fn angle(s: &Sketch, a: PointId, b: PointId, c: PointId, d: PointId) -> f64 {
    let (pa, pb, pc, pd) = (s.point(a), s.point(b), s.point(c), s.point(d));
    let u = (pb.0 - pa.0, pb.1 - pa.1);
    let v = (pd.0 - pc.0, pd.1 - pc.1);
    (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1)
}

// ---------------------------------------------------------------- Jacobians

/// Compares the analytic Jacobian with central finite differences.
fn check_jacobian(s: &Sketch) {
    let n = s.coords.len();
    let m = s.equation_count();
    let mut f = vec![0.0; m];
    let mut j = vec![0.0; m * n];
    evaluate(&s.constraints, &s.coords, &mut f, Some(&mut j));
    let h = 1e-6;
    for col in 0..n {
        let mut xp = s.coords.clone();
        let mut xm = s.coords.clone();
        xp[col] += h;
        xm[col] -= h;
        let mut fp = vec![0.0; m];
        let mut fm = vec![0.0; m];
        evaluate(&s.constraints, &xp, &mut fp, None);
        evaluate(&s.constraints, &xm, &mut fm, None);
        for r in 0..m {
            let fd = (fp[r] - fm[r]) / (2.0 * h);
            let an = j[r * n + col];
            assert!((fd - an).abs() < 1e-6, "row {r} col {col}: analytic {an} vs fd {fd} ({:?})", s.constraints);
        }
    }
}

fn four_points() -> (Sketch, [PointId; 4]) {
    let mut s = Sketch::new();
    let p = [s.add_point(0.3, -0.7), s.add_point(2.1, 0.4), s.add_point(-1.2, 0.9), s.add_point(0.5, 2.3)];
    (s, p)
}

#[test]
fn analytic_jacobians_match_finite_differences() {
    let all = |p: [PointId; 4]| {
        vec![
            Fix(p[0], 1.0, 2.0),
            Coincident(p[0], p[1]),
            Distance(p[0], p[1], 1.5),
            Horizontal(p[1], p[2]),
            Vertical(p[2], p[3]),
            Parallel(p[0], p[1], p[2], p[3]),
            Perpendicular(p[0], p[1], p[2], p[3]),
            Angle(p[0], p[1], p[2], p[3], 0.7),
            // Shared points: line a→b vs a→c.
            Parallel(p[0], p[1], p[0], p[2]),
            Perpendicular(p[0], p[1], p[0], p[2]),
            Angle(p[0], p[1], p[0], p[2], -2.0),
        ]
    };
    let (s0, p) = four_points();
    for c in all(p) {
        let mut s = s0.clone();
        s.add(c);
        check_jacobian(&s);
    }
}

#[test]
fn degenerate_directions_produce_no_nan() {
    let mut s = Sketch::new();
    let a = s.add_point(1.0, 1.0);
    let b = s.add_point(1.0, 1.0);
    let c = s.add_point(0.0, 0.0);
    let d = s.add_point(0.0, 0.0);
    s.add(Distance(a, b, 0.0));
    s.add(Parallel(a, b, c, d));
    s.add(Perpendicular(a, b, c, d));
    s.add(Angle(a, b, c, d, 0.0));
    let r = s.report();
    assert!(r.residual.is_finite());
    let r = s.solve();
    assert!(r.residual.is_finite());
    for p in [a, b, c, d] {
        let q = s.point(p);
        assert!(q.0.is_finite() && q.1.is_finite());
    }
}

#[test]
fn coincident_start_for_distance_separates_points() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(0.0, 0.0);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Distance(a, b, 2.0));
    let r = s.solve();
    assert_eq!(r.status, Status::UnderConstrained);
    assert_eq!(r.dof, 1);
    assert!((dist(s.point(a), s.point(b)) - 2.0).abs() < 1e-9);
}

// ---------------------------------------------------------- per constraint

#[test]
fn fix_moves_point() {
    let mut s = Sketch::new();
    let a = s.add_point(5.0, -3.0);
    s.add(Fix(a, 1.5, 2.5));
    let r = s.solve();
    assert_eq!(r, SolveReport { status: Status::WellConstrained, residual: r.residual, dof: 0 });
    assert!(r.residual < 1e-12);
    assert!(close(s.point(a), (1.5, 2.5), 1e-12));
}

#[test]
fn coincident_points() {
    let mut s = Sketch::new();
    let a = s.add_point(1.0, 1.0);
    let b = s.add_point(-2.0, 4.0);
    s.add(Fix(b, -2.0, 4.0));
    s.add(Coincident(a, b));
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert!(close(s.point(a), (-2.0, 4.0), 1e-10));
}

#[test]
fn distance_horizontal_vertical() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(2.5, 0.3);
    let c = s.add_point(2.2, 1.1);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Horizontal(a, b));
    s.add(Distance(a, b, 3.0));
    s.add(Vertical(b, c));
    s.add(Distance(b, c, 2.0));
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert!(r.residual < 1e-10);
    assert!(close(s.point(b), (3.0, 0.0), 1e-9));
    assert!(close(s.point(c), (3.0, 2.0), 1e-9));
}

#[test]
fn parallel_accepts_antiparallel() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(1.0, 1.0);
    let c = s.add_point(0.0, 0.0);
    let d = s.add_point(-1.0, -0.8);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Fix(b, 1.0, 1.0));
    s.add(Fix(c, 0.0, 0.0));
    s.add(Parallel(a, b, c, d));
    let r = s.solve();
    assert_eq!(r.status, Status::UnderConstrained);
    assert_eq!(r.dof, 1);
    let q = s.point(d);
    assert!((q.0 - q.1).abs() < 1e-9 && q.0 < 0.0, "{q:?}");
}

#[test]
fn perpendicular_lines() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(2.0, 1.0);
    let c = s.add_point(0.4, 1.7);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Fix(b, 2.0, 1.0));
    s.add(Perpendicular(a, b, a, c));
    s.add(Distance(a, c, 5f64.sqrt()));
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert!(close(s.point(c), (-1.0, 2.0), 1e-9), "{:?}", s.point(c));
}

/// Fixed a=(0,0), b=(1,0), |ac| = 2, angle(a→b, a→c) = θ; start near target.
fn angle_sketch(theta: f64, start: (f64, f64)) -> (Sketch, [PointId; 3]) {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(1.0, 0.0);
    let c = s.add_point(start.0, start.1);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Fix(b, 1.0, 0.0));
    s.add(Distance(a, c, 2.0));
    s.add(Angle(a, b, a, c, theta));
    (s, [a, b, c])
}

#[test]
fn angle_is_signed_counter_clockwise() {
    for (theta, start) in [
        (FRAC_PI_3, (0.8, 1.5)),
        (-FRAC_PI_3, (0.8, -1.5)),
        (2.0 * FRAC_PI_3, (-0.8, 1.5)),
        (-2.0 * FRAC_PI_3, (-1.2, -1.4)),
        (PI, (-1.8, 0.3)),
        (0.0, (1.5, 0.4)),
    ] {
        let (mut s, [a, b, c]) = angle_sketch(theta, start);
        let r = s.solve();
        assert_eq!(r.status, Status::WellConstrained, "theta {theta}");
        let expect = (2.0 * theta.cos(), 2.0 * theta.sin());
        assert!(close(s.point(c), expect, 1e-9), "theta {theta}: {:?}", s.point(c));
        assert!(wrap_angle(angle(&s, a, b, a, c) - theta).abs() < 1e-9);
    }
}

#[test]
fn angle_rejects_the_opposite_branch() {
    // Start exactly on the θ+π ray: the sin/cos-only form would be satisfied
    // there, the atan2 form is not, so the solver must leave this branch.
    let (mut s, [_, _, c]) = angle_sketch(FRAC_PI_4, (-2f64.sqrt() + 0.01, -2f64.sqrt()));
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert!(close(s.point(c), (2f64.sqrt(), 2f64.sqrt()), 1e-9), "{:?}", s.point(c));
}

#[test]
fn angle_is_taken_modulo_two_pi() {
    let (mut s, [_, _, c]) = angle_sketch(FRAC_PI_2 + 4.0 * PI, (0.3, 1.8));
    assert_eq!(s.solve().status, Status::WellConstrained);
    assert!(close(s.point(c), (0.0, 2.0), 1e-9));
}

#[test]
fn wrap_angle_range() {
    assert_eq!(wrap_angle(PI), PI);
    assert_eq!(wrap_angle(-PI), PI);
    assert!((wrap_angle(3.0 * PI / 2.0) + PI / 2.0).abs() < 1e-15);
    assert!((wrap_angle(0.1 - 6.0 * PI) - 0.1).abs() < 1e-12);
}

// ------------------------------------------------------------ rank and dof

#[test]
fn empty_sketches() {
    let mut s = Sketch::new();
    assert_eq!(s.solve(), SolveReport { status: Status::WellConstrained, residual: 0.0, dof: 0 });
    s.add_point(1.0, 2.0);
    s.add_point(3.0, 4.0);
    let r = s.solve();
    assert_eq!(r.status, Status::UnderConstrained);
    assert_eq!(r.dof, 4);
    assert_eq!(s.point_count(), 2);
}

#[test]
fn free_segment_has_three_dof() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(1.0, 1.0);
    s.add(Distance(a, b, 3.0));
    let r = s.solve();
    assert_eq!(r.status, Status::UnderConstrained);
    assert_eq!(r.dof, 3);
    assert!((dist(s.point(a), s.point(b)) - 3.0).abs() < 1e-10);
}

#[test]
fn rectangle_dof_counts_down() {
    let mut s = Sketch::new();
    let p = [s.add_point(0.1, -0.2), s.add_point(4.3, 0.4), s.add_point(3.8, 2.7), s.add_point(-0.3, 3.2)];
    let cs = [
        Fix(p[0], 0.0, 0.0),
        Horizontal(p[0], p[1]),
        Distance(p[0], p[1], 4.0),
        Vertical(p[1], p[2]),
        Distance(p[1], p[2], 3.0),
        Horizontal(p[2], p[3]),
        Vertical(p[3], p[0]),
    ];
    let mut expected_dof = 8;
    for c in cs {
        s.add(c);
        expected_dof -= c.equation_count();
        let r = s.solve();
        assert_eq!(r.dof, expected_dof, "after {c:?}");
        let want = if expected_dof == 0 { Status::WellConstrained } else { Status::UnderConstrained };
        assert_eq!(r.status, want);
    }
    assert!(close(s.point(p[2]), (4.0, 3.0), 1e-9));
    // Redundant but consistent: the diagonal of a 3-4-5 rectangle.
    s.add(Distance(p[0], p[2], 5.0));
    let r = s.solve();
    assert_eq!(r.status, Status::OverConstrained);
    assert_eq!(r.dof, 0);
    assert!(r.residual < 1e-9);
    // Same diagonal with a wrong length: unsatisfiable.
    let mut bad = s.clone();
    bad.add(Distance(p[0], p[2], 6.0));
    let r = bad.solve();
    assert_eq!(r.status, Status::Inconsistent);
    assert!(r.residual > 1e-6 && r.residual.is_finite());
}

#[test]
fn redundant_but_under_constrained_reports_under() {
    // Horizontal stated twice: redundant, yet the system still has freedom.
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(1.0, 0.5);
    s.add(Horizontal(a, b));
    s.add(Horizontal(b, a));
    let r = s.solve();
    assert_eq!(r.status, Status::UnderConstrained);
    assert_eq!(r.dof, 3);
}

#[test]
fn duplicate_fix_is_over_constrained() {
    let mut s = Sketch::new();
    let a = s.add_point(0.2, 0.1);
    s.add(Fix(a, 1.0, 1.0));
    s.add(Fix(a, 1.0, 1.0));
    let r = s.solve();
    assert_eq!(r.status, Status::OverConstrained);
    assert_eq!(r.dof, 0);
}

#[test]
fn contradictory_fixes_are_inconsistent() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    s.add(Fix(a, 0.0, 0.0));
    s.add(Fix(a, 2.0, 0.0));
    let r = s.solve();
    assert_eq!(r.status, Status::Inconsistent);
    // Least-squares compromise: midway, residual sqrt(2).
    assert!(close(s.point(a), (1.0, 0.0), 1e-9));
    assert!((r.residual - 2f64.sqrt()).abs() < 1e-9);
}

#[test]
fn negative_distance_is_inconsistent() {
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(1.0, 0.0);
    s.add(Distance(a, b, -1.0));
    assert_eq!(s.solve().status, Status::Inconsistent);
}

#[test]
fn solving_twice_is_stable() {
    let (mut s, [_, _, c]) = angle_sketch(FRAC_PI_3, (0.8, 1.5));
    let r1 = s.solve();
    let p1 = s.point(c);
    let r2 = s.solve();
    assert_eq!(r1.status, r2.status);
    assert_eq!(r1.dof, r2.dof);
    assert!(close(p1, s.point(c), 1e-14));
}

#[test]
fn under_constrained_points_stay_near_guess() {
    // Only a horizontal constraint: minimal-change solution averages the ys.
    let mut s = Sketch::new();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(3.0, 1.0);
    s.add(Horizontal(a, b));
    s.solve();
    assert!(close(s.point(a), (0.0, 0.5), 1e-9), "{:?}", s.point(a));
    assert!(close(s.point(b), (3.0, 0.5), 1e-9), "{:?}", s.point(b));
}

#[test]
#[should_panic(expected = "does not belong to this sketch")]
fn foreign_point_id_panics() {
    let mut s1 = Sketch::new();
    let mut s2 = Sketch::new();
    let a = s1.add_point(0.0, 0.0);
    s2.add_point(0.0, 0.0);
    s2.add(Fix(a, 0.0, 0.0));
}

#[test]
fn clone_keeps_point_ids_valid() {
    let mut s = Sketch::new();
    let a = s.add_point(1.0, 2.0);
    let t = s.clone();
    assert_eq!(t.point(a), (1.0, 2.0));
    assert_eq!(a.index(), 0);
}
