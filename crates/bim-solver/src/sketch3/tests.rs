use super::Constraint3::*;
use super::*;
use std::f64::consts::PI;

const EX: [f64; 3] = [1.0, 0.0, 0.0];
const EY: [f64; 3] = [0.0, 1.0, 0.0];
const EZ: [f64; 3] = [0.0, 0.0, 1.0];

fn close(p: (f64, f64, f64), q: (f64, f64, f64), tol: f64) -> bool {
    (p.0 - q.0).abs() < tol && (p.1 - q.1).abs() < tol && (p.2 - q.2).abs() < tol
}

/// Pins the given axes of `p` to `xyz` with axis-aligned datums.
fn pin(s: &mut Sketch3, p: Point3Id, xyz: [f64; 3], axes: &str) {
    for (k, (ax, n)) in ["x", "y", "z"].iter().zip([EX, EY, EZ]).enumerate() {
        if axes.contains(ax) {
            let d = s.add_datum(Plane { normal: n, offset: xyz[k] });
            s.add(OnDatum(p, d));
        }
    }
}

/// Right-angle tetrahedron A(0,0,0) B(3,0,0) C(0,4,0) D(0,0,5): 12 equations.
fn tetra(s: &mut Sketch3) -> [Point3Id; 4] {
    let a = s.add_point3(0.1, -0.1, 0.05);
    let b = s.add_point3(2.8, 0.2, 0.1);
    let c = s.add_point3(0.2, 3.7, -0.1);
    let d = s.add_point3(-0.1, 0.2, 4.6);
    pin(s, a, [0.0; 3], "xyz");
    pin(s, b, [0.0; 3], "yz");
    s.add(Distance3(a, b, 3.0));
    pin(s, c, [0.0; 3], "z");
    s.add(Distance3(a, c, 4.0));
    s.add(Perpendicular3(a, b, a, c));
    s.add(Distance3(a, d, 5.0));
    s.add(Perpendicular3(a, b, a, d));
    s.add(Perpendicular3(a, c, a, d));
    [a, b, c, d]
}

fn only(r: &SolveReport3) -> &SubReport {
    assert_eq!(r.subsystems.len(), 1, "{r:?}");
    &r.subsystems[0]
}

/// The symbolic Jacobian agrees with central finite differences.
#[test]
fn symbolic_jacobian_matches_finite_differences() {
    let mut s = Sketch3::new();
    let p: Vec<_> = (0..4)
        .map(|i| s.add_point3(0.3 * i as f64 + 0.1, 1.0 - 0.7 * i as f64, (i * i) as f64 * 0.4 - 0.2))
        .collect();
    let d = s.add_datum(Plane { normal: [1.0, 2.0, -2.0], offset: 0.5 });
    s.add(Coincident3(p[0], p[1]));
    s.add(Distance3(p[1], p[2], 2.0));
    s.add(Parallel3(p[0], p[1], p[2], p[3]));
    s.add(Perpendicular3(p[0], p[2], p[1], p[3]));
    s.add(Angle3(p[3], p[0], p[3], p[2], 1.0));
    s.add(OnDatum(p[3], d));
    let sub = &s.decompose()[0];
    let sys = System::build(&s, sub);
    let x = s.gather(&sub.points);
    let (m, n) = (sys.m, sys.n);
    assert_eq!(m, 3 + 1 + 3 + 1 + 1 + 1);
    let mut f = vec![0.0; m];
    let mut j = vec![0.0; m * n];
    sys.eval(&x, &mut f, Some(&mut j));
    let h = 1e-6;
    for col in 0..n {
        let (mut xp, mut xm) = (x.clone(), x.clone());
        xp[col] += h;
        xm[col] -= h;
        let (mut fp, mut fm) = (vec![0.0; m], vec![0.0; m]);
        sys.eval(&xp, &mut fp, None);
        sys.eval(&xm, &mut fm, None);
        for r in 0..m {
            let fd = (fp[r] - fm[r]) / (2.0 * h);
            assert!((fd - j[r * n + col]).abs() < 1e-6 * (1.0 + fd.abs()), "J[{r}][{col}]");
        }
    }
}

#[test]
fn tetrahedron_is_well_constrained() {
    let mut s = Sketch3::new();
    let [a, b, c, d] = tetra(&mut s);
    let r = s.solve();
    let sub = only(&r);
    assert_eq!((sub.status, sub.dof), (Status::WellConstrained, 0));
    assert!(sub.residual < 1e-12);
    assert!(close(s.point3(a), (0.0, 0.0, 0.0), 1e-9));
    assert!(close(s.point3(b), (3.0, 0.0, 0.0), 1e-9));
    assert!(close(s.point3(c), (0.0, 4.0, 0.0), 1e-9));
    assert!(close(s.point3(d), (0.0, 0.0, 5.0), 1e-9));
    // Re-solving is a no-op apart from reporting.
    let before = s.point3(d);
    assert_eq!(s.solve().subsystems[0].status, Status::WellConstrained);
    assert!(close(s.point3(d), before, 1e-12));
}

#[test]
fn implied_diagonal_is_redundant_and_wrong_one_inconsistent() {
    let mut s = Sketch3::new();
    let [_, b, c, _] = tetra(&mut s);
    s.add(Distance3(b, c, 5.0));
    let r = s.solve();
    assert_eq!((only(&r).status, only(&r).dof), (Status::OverConstrained, 0));

    let mut s = Sketch3::new();
    let [_, b, c, _] = tetra(&mut s);
    s.add(Distance3(b, c, 6.0));
    let r = s.solve();
    assert_eq!(only(&r).status, Status::Inconsistent);
    assert!(only(&r).residual > INCONSISTENT_RESIDUAL);
}

#[test]
fn decomposition_is_connected_components() {
    let mut s = Sketch3::new();
    let a = s.add_point3(0.0, 0.0, 0.0);
    let e = s.add_point3(7.0, 0.0, 0.0);
    let b = s.add_point3(1.0, 0.0, 0.0);
    let g = s.add_point3(-3.0, -3.0, -3.0);
    let f = s.add_point3(7.5, 1.5, 0.3);
    let c = s.add_point3(0.0, 1.0, 0.0);
    s.add(Distance3(e, f, 2.0));
    s.add(Perpendicular3(a, b, b, c));
    let subs = s.decompose();
    let parts: Vec<Vec<Point3Id>> = subs.iter().map(|x| x.points.clone()).collect();
    assert_eq!(parts, vec![vec![a, b, c], vec![e, f], vec![g]]);
    assert_eq!(subs[0].constraints, vec![1]);
    assert_eq!(subs[1].constraints, vec![0]);
    assert!(subs[2].constraints.is_empty());
    let r = s.solve();
    let rp: Vec<Vec<Point3Id>> = r.subsystems.iter().map(|x| x.points.clone()).collect();
    assert_eq!(rp, parts);
}

#[test]
fn mixed_clusters_are_reported_individually() {
    let mut s = Sketch3::new();
    let [a, b, c, d] = tetra(&mut s);
    let e = s.add_point3(7.0, 0.0, 0.0);
    let f = s.add_point3(7.5, 1.5, 0.3);
    s.add(Distance3(e, f, 2.0));
    let g = s.add_point3(-3.0, -3.0, -3.0);
    let h = s.add_point3(1.2, 0.9, 1.1);
    pin(&mut s, h, [1.0; 3], "xyz");
    let dz = s.add_datum(Plane { normal: EZ, offset: 1.0 });
    s.add(OnDatum(h, dz));
    let r = s.solve();
    let got: Vec<_> = r.subsystems.iter().map(|x| (x.points.clone(), x.status, x.dof)).collect();
    assert_eq!(
        got,
        vec![
            (vec![a, b, c, d], Status::WellConstrained, 0),
            (vec![e, f], Status::UnderConstrained, 5),
            (vec![g], Status::UnderConstrained, 3),
            (vec![h], Status::OverConstrained, 0),
        ]
    );
    assert_eq!(s.point3(g), (-3.0, -3.0, -3.0), "an unconstrained point never moves");
    assert!(close(s.point3(h), (1.0, 1.0, 1.0), 1e-12));
}

#[test]
fn inconsistent_subsystem_does_not_disturb_others() {
    let mut s = Sketch3::new();
    let p = s.add_point3(5.1, 0.1, 0.0);
    let [_, _, _, d] = tetra(&mut s);
    let q = s.add_point3(8.2, 0.0, 0.1);
    pin(&mut s, p, [5.0, 0.0, 0.0], "xyz");
    pin(&mut s, q, [8.0, 0.0, 0.0], "xyz");
    s.add(Distance3(p, q, 4.0));
    let r = s.solve();
    assert_eq!(r.subsystems.len(), 2);
    assert_eq!(r.subsystems[0].points, vec![p, q]);
    assert_eq!(r.subsystems[0].status, Status::Inconsistent);
    assert_eq!(r.subsystems[1].status, Status::WellConstrained);
    assert!(close(s.point3(d), (0.0, 0.0, 5.0), 1e-9));
}

#[test]
fn parallel_alone_is_not_redundant_but_implied_plane_is() {
    for with_plane in [false, true] {
        let mut s = Sketch3::new();
        let a = s.add_point3(0.0, 0.0, 0.0);
        let b = s.add_point3(2.0, 0.0, 0.0);
        let c = s.add_point3(0.0, 1.0, 0.0);
        let d = s.add_point3(1.7, 1.3, 0.2);
        pin(&mut s, a, [0.0, 0.0, 0.0], "xyz");
        pin(&mut s, b, [2.0, 0.0, 0.0], "xyz");
        pin(&mut s, c, [0.0, 1.0, 0.0], "xyz");
        s.add(Parallel3(a, b, c, d));
        s.add(Distance3(c, d, 2.0));
        if with_plane {
            let dz = s.add_datum(Plane { normal: EZ, offset: 0.0 });
            s.add(OnDatum(d, dz));
        }
        let r = s.solve();
        let want = if with_plane { Status::OverConstrained } else { Status::WellConstrained };
        assert_eq!((only(&r).status, only(&r).dof), (want, 0));
        assert!(close(s.point3(d), (2.0, 1.0, 0.0), 1e-9));
    }
}

#[test]
fn unsigned_angle_and_coincidence() {
    let mut s = Sketch3::new();
    let a = s.add_point3(0.0, 0.0, 0.0);
    let b = s.add_point3(1.0, 0.0, 0.0);
    let c = s.add_point3(0.8, 1.5, 0.1);
    pin(&mut s, a, [0.0; 3], "xyz");
    pin(&mut s, b, [1.0, 0.0, 0.0], "xyz");
    pin(&mut s, c, [0.0; 3], "z");
    s.add(Distance3(a, c, 2.0));
    s.add(Angle3(a, b, a, c, PI / 3.0));
    let e = s.add_point3(1.1, 2.2, 2.9);
    let f = s.add_point3(0.4, 2.7, 3.6);
    pin(&mut s, e, [1.0, 2.0, 3.0], "xyz");
    s.add(Coincident3(e, f));
    let r = s.solve();
    for sub in &r.subsystems {
        assert_eq!((sub.status, sub.dof), (Status::WellConstrained, 0));
    }
    assert!(close(s.point3(c), (1.0, 3f64.sqrt(), 0.0), 1e-9));
    assert!(close(s.point3(f), (1.0, 2.0, 3.0), 1e-9));
}

#[test]
fn oblique_datums_meet_in_a_point_and_normals_are_normalised() {
    let mut s = Sketch3::new();
    let a = s.add_point3(0.0, 0.0, 0.0);
    for n in [[1.0, 1.0, 1.0], [2.0, -2.0, 0.0], [0.0, 1.0, -2.0]] {
        let off = n[0] * 1.0 + n[1] * 2.0 + n[2] * 3.0;
        let d = s.add_datum(Plane { normal: n, offset: off });
        let stored = s.datum(d);
        let len: f64 = stored.normal.iter().map(|v| v * v).sum::<f64>().sqrt();
        assert!((len - 1.0).abs() < 1e-15);
        s.add(OnDatum(a, d));
    }
    let r = s.solve();
    assert_eq!((only(&r).status, only(&r).dof), (Status::WellConstrained, 0));
    assert!(close(s.point3(a), (1.0, 2.0, 3.0), 1e-9));
}

#[test]
fn free_rotation_has_two_dof() {
    let mut s = Sketch3::new();
    let a = s.add_point3(0.0, 0.0, 0.0);
    let b = s.add_point3(1.0, 1.0, 1.0);
    pin(&mut s, a, [0.0; 3], "xyz");
    s.add(Distance3(a, b, 5.0));
    let r = s.solve();
    assert_eq!((only(&r).status, only(&r).dof), (Status::UnderConstrained, 2));
    let (x, y, z) = s.point3(b);
    assert!(((x * x + y * y + z * z).sqrt() - 5.0).abs() < 1e-9);
}

#[test]
fn report_does_not_move_points() {
    let mut s = Sketch3::new();
    let [a, ..] = tetra(&mut s);
    let r = s.report();
    assert_eq!(only(&r).status, Status::Inconsistent);
    assert_eq!(s.point3(a), (0.1, -0.1, 0.05));
}

#[test]
fn degenerate_start_does_not_panic_or_produce_nan() {
    let mut s = Sketch3::new();
    let a = s.add_point3(1.0, 1.0, 1.0);
    let b = s.add_point3(1.0, 1.0, 1.0);
    let c = s.add_point3(1.0, 1.0, 1.0);
    s.add(Distance3(a, b, 2.0));
    s.add(Perpendicular3(a, b, a, c));
    let r = s.solve();
    assert_eq!(r.subsystems.len(), 1);
    // Documented limitation: a zero-length direction at the start has no
    // usable gradient, so the subsystem stays put and is reported Inconsistent.
    assert_eq!(r.subsystems[0].status, Status::Inconsistent);
    assert_eq!(s.point3(b), (1.0, 1.0, 1.0));
    for p in [a, b, c] {
        let (x, y, z) = s.point3(p);
        assert!(x.is_finite() && y.is_finite() && z.is_finite());
    }
}

#[test]
#[should_panic(expected = "non-zero normal")]
fn zero_normal_datum_panics() {
    Sketch3::new().add_datum(Plane { normal: [0.0; 3], offset: 1.0 });
}

#[test]
#[should_panic(expected = "does not belong")]
fn foreign_point_panics() {
    let mut other = Sketch3::new();
    let p = other.add_point3(0.0, 0.0, 0.0);
    let mut s = Sketch3::new();
    let q = s.add_point3(0.0, 0.0, 0.0);
    s.add(Distance3(p, q, 1.0));
}

#[test]
#[should_panic(expected = "does not belong")]
fn foreign_datum_panics() {
    let mut other = Sketch3::new();
    let d = other.add_datum(Plane { normal: EX, offset: 0.0 });
    let mut s = Sketch3::new();
    let q = s.add_point3(0.0, 0.0, 0.0);
    s.add(OnDatum(q, d));
}
