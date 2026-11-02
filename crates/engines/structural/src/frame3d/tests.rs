use super::*;

const E: f64 = 200e9;
const G: f64 = 80e9;
const P: f64 = 10e3;

fn sec() -> Section3 {
    Section3 { e: E, g: G, a: 1e-2, iy: 2e-5, iz: 5e-5, j: 1e-5, asy: None, asz: None }
}

fn combo(name: &str, factors: &[(&str, f64)]) -> LoadCombination {
    LoadCombination { name: name.to_string(), factors: factors.iter().map(|(c, f)| (c.to_string(), *f)).collect() }
}

fn close(actual: f64, expected: f64, tol: f64, what: &str) {
    assert!((actual - expected).abs() <= tol, "{what}: got {actual}, expected {expected}");
}

fn cantilever(s: Section3) -> (Frame3, usize, usize, usize) {
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(2.0, 0.0, 0.0);
    let m = f.add_member(a, b, s, [0.0, 0.0, 1.0]);
    f.fix(a, [true; 6]);
    (f, a, b, m)
}

/// An irregular space frame with inclined members, a pinned base and mixed loads.
fn space_frame() -> (Frame3, Vec<[f64; 3]>) {
    let pts = vec![
        [0.0, 0.0, 0.0],
        [4.0, 0.3, 0.0],
        [0.2, 3.5, 0.0],
        [0.1, -0.2, 3.1],
        [4.2, 0.1, 3.3],
        [0.3, 3.6, 2.9],
    ];
    let mut f = Frame3::new();
    for p in &pts {
        f.add_node(p[0], p[1], p[2]);
    }
    let s = sec();
    let t = Section3 { asy: Some(4e-3), asz: Some(6e-3), ..s };
    f.add_member(0, 3, s, [1.0, 0.2, 0.0]);
    f.add_member(4, 1, t, [0.0, 1.0, 0.3]);
    f.add_member(2, 5, s, [1.0, 1.0, 0.0]);
    f.add_member(3, 4, t, [0.0, 0.0, 1.0]);
    f.add_member(5, 3, s, [0.3, 0.0, 1.0]);
    f.add_member(4, 5, t, [0.0, 0.5, 1.0]);
    f.add_member(0, 4, s, [0.0, 1.0, 0.0]);
    f.fix(0, [true; 6]);
    f.fix(1, [true; 6]);
    f.fix(2, [true, true, true, false, false, false]);
    f.load(3, "D", [1e3, -2e3, -8e3, 500.0, 0.0, -300.0]);
    f.load(4, "D", [0.0, 0.0, -9e3, 0.0, 200.0, 0.0]);
    f.load(5, "L", [-3e3, 4e3, -5e3, 0.0, -400.0, 100.0]);
    f.load(3, "L", [0.0, 2e3, 0.0, 0.0, 0.0, 0.0]);
    f.load(0, "D", [5e3, 0.0, 0.0, 0.0, 0.0, 0.0]); // on a fully fixed node
    f.add_combination(combo("C1", &[("D", 1.2), ("L", 1.6)]));
    f.add_combination(combo("C2", &[("D", 0.9), ("L", -1.0)]));
    (f, pts)
}

#[test]
fn members_and_structure_are_in_equilibrium() {
    let (f, pts) = space_frame();
    let r = f.analyze().unwrap();
    let geoms = f.validate().unwrap();
    for c in ["C1", "C2"] {
        for (m, g) in geoms.iter().enumerate() {
            let e = r.member_end_forces(c, m);
            let scale = e.iter().fold(1.0f64, |a, v| a.max(v.abs()));
            for q in 0..3 {
                close(e[q] + e[6 + q], 0.0, 1e-9 * scale, "member force balance");
            }
            // Moments about node i: M1 + M2 + (L, 0, 0) × F2.
            let l = g.length;
            close(e[3] + e[9], 0.0, 1e-9 * scale, "torsion balance");
            close(e[4] + e[10] - l * e[8], 0.0, 1e-9 * scale, "My balance");
            close(e[5] + e[11] + l * e[7], 0.0, 1e-9 * scale, "Mz balance");
        }
        // Global: Σ reactions + Σ loads = 0 for forces and for moments about the origin.
        let factors: &[(&str, f64)] = if c == "C1" { &[("D", 1.2), ("L", 1.6)] } else { &[("D", 0.9), ("L", -1.0)] };
        let mut sum = [0.0; 6];
        let mut add = |p: [f64; 3], v: [f64; 6]| {
            let m = cross(p, [v[0], v[1], v[2]]);
            for q in 0..3 {
                sum[q] += v[q];
                sum[3 + q] += v[3 + q] + m[q];
            }
        };
        for (case, node, l) in &f.loads {
            let k = factors.iter().find(|(n, _)| n == case).map_or(0.0, |x| x.1);
            add(pts[*node], l.map(|v| k * v));
        }
        for (n, p) in pts.iter().enumerate() {
            add(*p, r.reaction(c, n));
        }
        for v in sum {
            close(v, 0.0, 1e-6, "global equilibrium");
        }
        // Free DOFs carry no reaction.
        assert_eq!(r.reaction(c, 3), [0.0; 6]);
        assert_eq!(&r.reaction(c, 2)[3..], &[0.0; 3]);
    }
}

#[test]
fn combinations_are_linear_and_unused_cases_ignored() {
    let (mut f, _, b, _) = cantilever(sec());
    f.load(b, "A", [0.0, -P, 0.0, 0.0, 0.0, 0.0]);
    f.load(b, "A", [0.0, -P, 0.0, 0.0, 0.0, 0.0]); // accumulates
    f.load(b, "X", [1e9; 6]);
    f.add_combination(combo("one", &[("A", 1.0)]));
    f.add_combination(combo("split", &[("A", 0.5), ("A", 1.0), ("missing", 7.0)]));
    let r = f.analyze().unwrap();
    let d1 = r.displacement("one", b);
    let d2 = r.displacement("split", b);
    close(d1[1], -2.0 * P * 8.0 / (3.0 * E * 5e-5), 1e-15, "accumulated load");
    for q in 0..6 {
        close(d2[q], 1.5 * d1[q], 1e-15, "linearity");
    }
    assert_eq!(r.combinations(), vec!["one", "split"]);
}

#[test]
fn load_on_a_support_reduces_the_reaction() {
    let (mut f, a, b, _) = cantilever(sec());
    f.load(a, "A", [0.0, 0.0, 3e3, 0.0, 0.0, 0.0]);
    f.load(b, "A", [0.0, 0.0, -P, 0.0, 0.0, 0.0]);
    f.add_combination(combo("U", &[("A", 1.0)]));
    let r = f.analyze().unwrap();
    close(r.reaction("U", a)[2], P - 3e3, 1e-6, "reaction net of the support load");
}

#[test]
fn timoshenko_field_matches_closed_form_cantilever() {
    let (l, ei, gas, p): (f64, f64, f64, f64) = (1.7, 3e6, 2e8, 1e4);
    let phi = 12.0 * ei / (gas * l * l);
    let tip = p * l.powi(3) / (3.0 * ei) + p * l / gas;
    let rot = p * l * l / (2.0 * ei);
    for xi in [0.1, 0.25, 0.5, 0.8] {
        let x = xi * l;
        let exact = p * x * x * (3.0 * l - x) / (6.0 * ei) + p * x / gas;
        let chord = tip * x / l;
        let got = transverse_field(0.0, 0.0, tip, rot, phi, l, x);
        close(got, exact - chord, 1e-12 * tip, "Timoshenko field");
    }
}

#[test]
fn deflection_envelope_of_a_cantilever_both_planes() {
    // Tip loads in both planes; relative to the chord the E-B field is
    // v(x) − v(L) x/L with v = P x² (3L − x) / 6EI. Check against dense evaluation.
    let (mut f, _, b, m) = cantilever(sec());
    f.load(b, "A", [0.0, -P, 2.0 * P, 0.0, 0.0, 0.0]);
    f.add_combination(combo("U", &[("A", 1.0)]));
    let r = f.analyze().unwrap();
    let l = 2.0;
    let rel = |p: f64, i: f64, x: f64| {
        let v = |x: f64| p * x * x * (3.0 * l - x) / (6.0 * E * i);
        v(x) - v(l) * x / l
    };
    let mut best = 0.0f64;
    for k in 0..=10_000 {
        let x = l * k as f64 / 10_000.0;
        best = best.max(rel(P, 5e-5, x).hypot(rel(2.0 * P, 2e-5, x)));
    }
    let env = r.envelope(m);
    assert!(env.deflection <= best * (1.0 + 1e-9) && env.deflection >= best * 0.9995, "{} vs {best}", env.deflection);
}

#[test]
fn shear_does_not_change_deflection_relative_to_chord_under_end_moments() {
    // Simply supported, end moment: constant shear only adds a rigid-body-like
    // linear field, which the chord removes.
    let run = |s: Section3| {
        let mut f = Frame3::new();
        let a = f.add_node(0.0, 0.0, 0.0);
        let b = f.add_node(3.0, 0.0, 0.0);
        let m = f.add_member(a, b, s, [0.0, 0.0, 1.0]);
        f.fix(a, [true, true, true, true, false, false]);
        f.fix(b, [false, true, true, false, false, false]);
        f.load(b, "M", [0.0, 0.0, 0.0, 0.0, 0.0, 5e3]);
        f.add_combination(combo("U", &[("M", 1.0)]));
        f.analyze().unwrap().envelope(m).deflection
    };
    let eb = run(sec());
    let ti = run(Section3 { asy: Some(1e-4), asz: Some(1e-4), ..sec() });
    close(ti, eb, 1e-9 * eb, "shear-flexible deflection");
}

#[test]
fn envelope_takes_extremes_over_combinations_and_stations() {
    let (mut f, _, b, m) = cantilever(sec());
    f.load(b, "A", [P, -P, 0.0, 2e3, 0.0, 0.0]);
    f.add_combination(combo("pos", &[("A", 1.0)]));
    f.add_combination(combo("neg", &[("A", -0.5)]));
    let r = f.analyze().unwrap();
    let env = r.envelope(m);
    let tol = 1e-6;
    close(env.n[0], -0.5 * P, tol, "N min");
    close(env.n[1], P, tol, "N max");
    close(env.t[0], -1e3, tol, "T min");
    close(env.t[1], 2e3, tol, "T max");
    close(env.vy[0], -P, tol, "Vy min");
    close(env.vy[1], 0.5 * P, tol, "Vy max");
    close(env.mz[0], -2.0 * P, tol, "Mz min (root, pos)");
    close(env.mz[1], P, tol, "Mz max (root, neg)");
}

#[test]
fn no_combinations_gives_zero_envelopes_but_still_checks_stability() {
    let (f, _, _, m) = cantilever(sec());
    let r = f.analyze().unwrap();
    assert_eq!(r.envelope(m), Envelope::ZERO);
    assert!(r.combinations().is_empty());
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(1.0, 0.0, 0.0);
    f.add_member(a, b, sec(), [0.0, 0.0, 1.0]);
    assert!(f.analyze().is_err());
}

#[test]
fn invalid_inputs_are_errors() {
    let bad = |edit: &dyn Fn(&mut Frame3)| {
        let (mut f, _, b, _) = cantilever(sec());
        f.load(b, "A", [0.0, -P, 0.0, 0.0, 0.0, 0.0]);
        f.add_combination(combo("U", &[("A", 1.0)]));
        edit(&mut f);
        f.analyze().unwrap_err()
    };
    assert!(bad(&|f| f.members[0].reference = [3.0, 0.0, 0.0]).contains("parallel"));
    assert!(bad(&|f| f.members[0].reference = [0.0; 3]).contains("reference"));
    assert!(bad(&|f| f.members[0].reference = [f64::NAN, 0.0, 1.0]).contains("reference"));
    assert!(bad(&|f| f.members[0].s.g = 0.0).contains("g must be"));
    assert!(bad(&|f| f.members[0].s.j = -1.0).contains("j must be"));
    assert!(bad(&|f| f.members[0].s.asz = Some(0.0)).contains("asz"));
    assert!(bad(&|f| f.nodes[1] = [0.0; 3]).contains("zero length"));
    assert!(bad(&|f| f.nodes[1][2] = f64::INFINITY).contains("non-finite"));
    assert!(bad(&|f| f.load(1, "A", [f64::NAN, 0.0, 0.0, 0.0, 0.0, 0.0])).contains("not finite"));
    assert!(bad(&|f| f.add_combination(combo("U", &[]))).contains("named"));
    assert!(bad(&|f| f.add_combination(combo("V", &[("A", f64::NAN)]))).contains("factor"));
    assert!(bad(&|f| {
        f.add_node(5.0, 5.0, 5.0);
    })
    .contains("no stiffness"));
}

#[test]
fn fully_fixed_frame_returns_negated_loads() {
    let (mut f, a, b, m) = cantilever(sec());
    f.fix(b, [true; 6]);
    f.load(b, "A", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    f.add_combination(combo("U", &[("A", 2.0)]));
    let r = f.analyze().unwrap();
    assert_eq!(r.displacement("U", b), [0.0; 6]);
    assert_eq!(r.reaction("U", b), [-2.0, -4.0, -6.0, -8.0, -10.0, -12.0]);
    assert_eq!(r.reaction("U", a), [0.0; 6]);
    assert_eq!(r.member_end_forces("U", m), [0.0; 12]);
}

#[test]
#[should_panic(expected = "unknown load combination")]
fn unknown_combination_panics() {
    let (f, _, b, _) = cantilever(sec());
    f.analyze().unwrap().displacement("nope", b);
}

#[test]
#[should_panic(expected = "out of range")]
fn bad_node_index_panics() {
    let mut f = Frame3::new();
    f.add_node(0.0, 0.0, 0.0);
    f.fix(3, [true; 6]);
}
