use super::*;
use crate::hydraulics::{colebrook, friction_factor};

const WATER: Fluid = Fluid { density: 998.2, viscosity: 1.002e-3 };

fn pipe(a: usize, b: usize, d: f64, length: f64, roughness: f64, k: f64) -> Pipe {
    Pipe { a, b, area: std::f64::consts::PI * d * d / 4.0, dh: d, length, roughness, k }
}

fn source(p: f64) -> Node {
    Node { fixed: Some(p), demand: 0.0 }
}

fn sink(q: f64) -> Node {
    Node { fixed: None, demand: q }
}

#[test]
fn colebrook_satisfies_its_equation_and_matches_a_known_value() {
    for &(re, rr) in &[(2000.0, 0.0), (2600.0, 5e-4), (1e5, 4.5e-4), (1e7, 1e-6), (1e11, 3e-3), (5e4, 0.05)] {
        let (f, _) = colebrook(re, rr);
        let lhs = 1.0 / f.sqrt();
        let rhs = -2.0 * (rr / 3.7 + 2.51 / (re * f.sqrt())).log10();
        assert!((lhs - rhs).abs() <= 1e-13 * lhs, "Re {re} rr {rr}: {lhs} vs {rhs}");
    }
    // Moody chart: smooth pipe at Re 1e5 has f ≈ 0.01799.
    let (f, _) = colebrook(1e5, 0.0);
    assert!((f - 0.017_99).abs() < 5e-5, "{f}");
    // Re·df/dRe against a central difference.
    let (re, rr) = (3e4, 1e-4);
    let h = re * 1e-6;
    let fd = (colebrook(re + h, rr).0 - colebrook(re - h, rr).0) / (2.0 * h) * re;
    assert!((colebrook(re, rr).1 - fd).abs() < 1e-6 * fd.abs(), "{} vs {fd}", colebrook(re, rr).1);
}

#[test]
fn laminar_below_re_2000_and_zero_at_rest() {
    assert_eq!(friction_factor(1999.0, 1e-3).0, 64.0 / 1999.0);
    assert_eq!(friction_factor(2000.0, 1e-3).0, colebrook(2000.0, 1e-3).0);
    assert_eq!(friction_factor(0.0, 1e-3), (0.0, 0.0));
    let st = pipe(0, 1, 0.1, 10.0, 0.0, 1.0).state(&WATER, 0.0);
    assert_eq!((st.velocity, st.reynolds, st.friction_factor, st.dp), (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn single_pipe_matches_the_closed_form() {
    let (d, l, eps, k, q, p0) = (0.1, 100.0, 4.5e-5, 2.5, 0.02, 4e5);
    let net = Network { fluid: WATER, nodes: vec![source(p0), sink(q)], pipes: vec![pipe(0, 1, d, l, eps, k)] };
    for method in [Method::Newton, Method::HardyCross] {
        let sol = solver::solve(&net, method).unwrap();
        let a = std::f64::consts::PI * d * d / 4.0;
        let v = q / a;
        let re = WATER.density * v * d / WATER.viscosity;
        let f = colebrook(re, eps / d).0;
        let dp = (f * l / d + k) * WATER.density * v * v / 2.0;
        assert!((sol.flows[0] - q).abs() < 1e-15, "{method:?}");
        assert!((p0 - sol.pressures[1] - dp).abs() < 1e-9 * dp, "{method:?}: {} vs {dp}", p0 - sol.pressures[1]);
    }
    // Laminar: Hagen-Poiseuille.
    let oil = Fluid { density: 900.0, viscosity: 0.5 };
    let net = Network { fluid: oil, nodes: vec![source(2e5), sink(0.001)], pipes: vec![pipe(1, 0, 0.05, 25.0, 1e-5, 0.0)] };
    let sol = solver::solve(&net, Method::Newton).unwrap();
    let v = 0.001 / (std::f64::consts::PI * 0.05 * 0.05 / 4.0);
    assert!((sol.flows[0] + 0.001).abs() < 1e-15);
    let hp = 32.0 * 0.5 * 25.0 * v / (0.05 * 0.05);
    assert!(((2e5 - sol.pressures[1]) - hp).abs() < 1e-9 * hp);
}

/// A looped network with two sources: Newton and Hardy-Cross agree, continuity and energy hold.
#[test]
fn newton_and_hardy_cross_agree_on_a_looped_two_source_network() {
    // 3x3 grid, sources at opposite corners.
    let mut nodes = Vec::new();
    for i in 0..9 {
        nodes.push(match i {
            0 => source(5e5),
            8 => source(4.2e5),
            _ => sink(0.002 + 0.001 * i as f64),
        });
    }
    let mut pipes = Vec::new();
    let ds = [0.1, 0.15, 0.125, 0.2];
    for r in 0..3 {
        for c in 0..3 {
            let i = r * 3 + c;
            if c < 2 {
                let n = pipes.len();
                pipes.push(if n % 2 == 0 { pipe(i, i + 1, ds[n % 4], 120.0, 4.5e-5, 0.5) } else { pipe(i + 1, i, ds[n % 4], 150.0, 1.5e-4, 0.0) });
            }
            if r < 2 {
                let n = pipes.len();
                pipes.push(pipe(i, i + 3, ds[(n + 1) % 4], 100.0 + 10.0 * n as f64, 1.5e-6, 2.0));
            }
        }
    }
    let net = Network { fluid: WATER, nodes, pipes };
    let a = solver::solve(&net, Method::Newton).unwrap();
    let b = solver::solve(&net, Method::HardyCross).unwrap();
    let qmax = a.flows.iter().fold(0.0f64, |m, q| m.max(q.abs()));
    for (qa, qb) in a.flows.iter().zip(&b.flows) {
        assert!((qa - qb).abs() < 1e-9 * qmax, "{qa} vs {qb}");
    }
    for sol in [&a, &b] {
        for (p, pa) in net.pipes.iter().zip(&sol.flows).map(|(p, q)| (p, *q)) {
            let dp = p.state(&WATER, pa).dp;
            let drop = sol.pressures[p.a] - sol.pressures[p.b];
            assert!((drop - dp).abs() < 1e-7 * 5e5, "{drop} vs {dp}");
        }
        for (i, node) in net.nodes.iter().enumerate() {
            if node.fixed.is_some() {
                assert_eq!(sol.pressures[i], node.fixed.unwrap());
                continue;
            }
            let inflow: f64 = net.pipes.iter().zip(&sol.flows).map(|(p, q)| if p.b == i { *q } else if p.a == i { -*q } else { 0.0 }).sum();
            assert!((inflow - node.demand).abs() < 1e-14, "{inflow} vs {}", node.demand);
        }
    }
}

#[test]
fn a_part_without_a_source_is_an_error() {
    let net = Network {
        fluid: WATER,
        nodes: vec![source(1e5), sink(0.01), sink(0.0), sink(0.01)],
        pipes: vec![pipe(0, 1, 0.1, 10.0, 0.0, 0.0), pipe(2, 3, 0.1, 10.0, 0.0, 0.0)],
    };
    assert_eq!(solver::solve(&net, Method::Newton), Err(SolveError::NoSource(2)));
    // A lone port without demand is fine.
    let net = Network { fluid: WATER, nodes: vec![source(1e5), sink(0.0)], pipes: vec![] };
    assert!(solver::solve(&net, Method::HardyCross).is_ok());
}

#[test]
fn sizing_picks_the_smallest_size_meeting_the_limits() {
    // 0.02 m³/s of water: DN100 has v = 2.55 m/s, DN125 1.63 m/s and a gradient well below 400 Pa/m.
    assert_eq!(recommend_size(Shape::Round { diameter: 0.1 }, 0.02, 4.5e-5, &WATER, Medium::Water), "DN125");
    assert_eq!(recommend_size(Shape::Round { diameter: 0.1 }, 0.0, 4.5e-5, &WATER, Medium::Water), "DN15");
    assert_eq!(recommend_size(Shape::Round { diameter: 0.1 }, 10.0, 4.5e-5, &WATER, Medium::Water), "DN300");
    let air = Fluid { density: 1.2, viscosity: 1.81e-5 };
    let s = recommend_size(Shape::Rect { width: 0.4, height: 0.3 }, 0.4, 1.5e-4, &air, Medium::Air);
    assert!(s.ends_with("x300"), "{s}");
}

#[test]
fn engine_identity_and_method_names() {
    assert_eq!(MepEngine::new().name(), "mep");
    assert_eq!(MepEngine::new().method(), Method::Newton);
    assert_eq!(MepEngine::with_method(Method::HardyCross).method(), Method::HardyCross);
    assert_eq!(Method::parse("hardy-cross"), Some(Method::HardyCross));
    assert_eq!(Method::parse("newton"), Some(Method::Newton));
    assert_eq!(Method::parse("gauss"), None);
}

/// Two sources whose pressure difference lies inside the jump of the friction law at Re 2000: no
/// flow balances it, and both methods say so, naming the pipe.
#[test]
fn a_pipe_pinned_at_the_laminar_turbulent_jump_is_reported() {
    let p = pipe(0, 1, 0.05, 50.0, 4.5e-5, 0.0);
    let q_c = crate::hydraulics::RE_LAMINAR * WATER.viscosity / (WATER.density * p.dh) * p.area;
    let below = p.state(&WATER, q_c * (1.0 - 1e-12)).dp;
    let above = p.state(&WATER, q_c * (1.0 + 1e-12)).dp;
    assert!(above > below * 1.3, "{below} {above}");
    let net = Network { fluid: WATER, nodes: vec![source(1e5 + (below + above) / 2.0), source(1e5)], pipes: vec![p] };
    for method in [Method::Newton, Method::HardyCross] {
        assert_eq!(solver::solve(&net, method), Err(SolveError::Transition(vec![0])), "{method:?}");
    }
    // Just outside the gap both solve.
    for (dp, laminar) in [(below * 0.99, true), (above * 1.01, false)] {
        let net = Network { fluid: WATER, nodes: vec![source(1e5 + dp), source(1e5)], pipes: vec![p] };
        for method in [Method::Newton, Method::HardyCross] {
            let sol = solver::solve(&net, method).unwrap();
            let st = p.state(&WATER, sol.flows[0]);
            assert_eq!(st.reynolds < 2000.0, laminar, "{method:?}");
            assert!((st.dp - dp).abs() < 1e-9 * dp, "{method:?}: {} vs {dp}", st.dp);
        }
    }
}
