// External judge for T2.4: symbolic differentiation (closed forms), and the T1.3 2D API unchanged.
// Solver behaviour on 3D sketches is judged by src/main.rs + check.py against python-solvespace.
use bim_solver::{Constraint, Expr, Sketch, Status};

fn x() -> Expr {
    Expr::var(0)
}
fn y() -> Expr {
    Expr::var(1)
}
fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1.0)
}

#[test]
fn diff_of_norm_is_exact() {
    // r = sqrt(x^2 + y^2); dr/dx = x/r, dr/dy = y/r. A finite difference cannot reach 1e-14.
    let r = (x() * x() + y() * y()).sqrt();
    for &(a, b) in &[(3.0f64, 4.0f64), (1e-3, 2e-3), (1234.5, -987.25)] {
        let n = (a * a + b * b).sqrt();
        assert!(rel(r.diff(0).eval(&[a, b]), a / n) < 1e-14, "d/dx at ({a},{b})");
        assert!(rel(r.diff(1).eval(&[a, b]), b / n) < 1e-14, "d/dy at ({a},{b})");
    }
}

#[test]
fn diff_returns_an_expression_tree() {
    // d²/dx² of x³ = 6x and d³/dx³ = 6, evaluated exactly at a large argument.
    let c = x() * x() * x();
    let d2 = c.diff(0).diff(0);
    assert!(rel(d2.eval(&[1000.0]), 6000.0) < 1e-14);
    assert_eq!(c.diff(0).diff(0).diff(0).eval(&[-7.5]), 6.0);
    // Variables an expression does not use differentiate to exactly zero; constants too.
    assert_eq!(c.diff(1).eval(&[2.0, 3.0]), 0.0);
    assert_eq!(Expr::constant(4.25).diff(0).eval(&[1.0]), 0.0);
}

#[test]
fn quotient_product_and_negation_rules() {
    // f = -(x / y) + x*y - 2; df/dx = -1/y + y; df/dy = x/y² + x.
    let f = -(x() / y()) + x() * y() - Expr::constant(2.0);
    let (a, b) = (1.7, -3.1);
    assert!(rel(f.eval(&[a, b]), -(a / b) + a * b - 2.0) < 1e-15);
    assert!(rel(f.diff(0).eval(&[a, b]), -1.0 / b + b) < 1e-15);
    assert!(rel(f.diff(1).eval(&[a, b]), a / (b * b) + a) < 1e-15);
}

#[test]
fn t1_3_api_is_unchanged() {
    let mut s = Sketch::new();
    let p = [s.add_point(0.1, -0.2), s.add_point(4.3, 0.4), s.add_point(3.8, 2.7)];
    s.add(Constraint::Fix(p[0], 0.0, 0.0));
    s.add(Constraint::Horizontal(p[0], p[1]));
    s.add(Constraint::Distance(p[0], p[1], 4.0));
    s.add(Constraint::Vertical(p[1], p[2]));
    s.add(Constraint::Distance(p[1], p[2], 3.0));
    let r = s.solve();
    assert_eq!(r.status, Status::WellConstrained);
    assert_eq!(r.dof, 0);
    let (cx, cy) = s.point(p[2]);
    assert!((cx - 4.0).abs() < 1e-7 && (cy - 3.0).abs() < 1e-7);
}
