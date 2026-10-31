// External judge for T1.4 (bim-struct): textbook closed forms within 0.1%.
use bim_struct::Frame;

const E: f64 = 200e9;
const A: f64 = 1e-2;
const I: f64 = 1e-4;
const L: f64 = 3.0;
const P: f64 = 10e3;

fn within(actual: f64, expected: f64) {
    let rel = ((actual - expected) / expected).abs();
    assert!(rel < 1e-3, "got {actual}, expected {expected} (rel err {rel})");
}

#[test]
fn cantilever_tip_load() {
    let mut f = Frame::new();
    let n0 = f.add_node(0.0, 0.0);
    let n1 = f.add_node(L, 0.0);
    let m = f.add_member(n0, n1, E, A, I);
    f.fix(n0, true, true, true);
    f.load(n1, 0.0, -P, 0.0);
    let r = f.analyze().unwrap();
    let (_, uy, rz) = r.displacement(n1);
    within(uy, -P * L.powi(3) / (3.0 * E * I));
    within(rz, -P * L.powi(2) / (2.0 * E * I));
    let (_, fy, mz) = r.reaction(n0);
    within(fy, P);
    within(mz, P * L);
    let ef = r.member_end_forces(m);
    within(ef[1].abs(), P);
    within(ef[2].abs(), P * L);
    assert!(ef[5].abs() < 1e-6 * P * L, "tip moment should vanish: {}", ef[5]);
}

#[test]
fn subdivided_cantilever_matches() {
    let mut f = Frame::new();
    let nodes: Vec<usize> = (0..=4).map(|k| f.add_node(L * k as f64 / 4.0, 0.0)).collect();
    for w in nodes.windows(2) {
        f.add_member(w[0], w[1], E, A, I);
    }
    f.fix(nodes[0], true, true, true);
    f.load(nodes[4], 0.0, -P, 0.0);
    let r = f.analyze().unwrap();
    within(r.displacement(nodes[4]).1, -P * L.powi(3) / (3.0 * E * I));
    within(r.displacement(nodes[2]).1, -P * (L / 2.0).powi(2) * (3.0 * L - L / 2.0) / (6.0 * E * I));
}

#[test]
fn simply_supported_midspan_load() {
    let mut f = Frame::new();
    let a = f.add_node(0.0, 0.0);
    let mid = f.add_node(L / 2.0, 0.0);
    let b = f.add_node(L, 0.0);
    f.add_member(a, mid, E, A, I);
    f.add_member(mid, b, E, A, I);
    f.fix(a, true, true, false);
    f.fix(b, false, true, false);
    f.load(mid, 0.0, -P, 0.0);
    let r = f.analyze().unwrap();
    within(r.displacement(mid).1, -P * L.powi(3) / (48.0 * E * I));
    within(r.reaction(a).1, P / 2.0);
    within(r.reaction(b).1, P / 2.0);
}

#[test]
fn axial_bar() {
    let mut f = Frame::new();
    let a = f.add_node(0.0, 0.0);
    let b = f.add_node(0.0, L);
    let m = f.add_member(a, b, E, A, I);
    f.fix(a, true, true, true);
    f.load(b, 0.0, P, 0.0);
    let r = f.analyze().unwrap();
    within(r.displacement(b).1, P * L / (E * A));
    within(r.member_end_forces(m)[3].abs(), P);
}

#[test]
fn fixed_base_portal_with_rigid_beam_sways_by_closed_form() {
    let h = 3.0;
    let mut f = Frame::new();
    let b1 = f.add_node(0.0, 0.0);
    let t1 = f.add_node(0.0, h);
    let t2 = f.add_node(5.0, h);
    let b2 = f.add_node(5.0, 0.0);
    f.add_member(b1, t1, E, 10.0, I);
    f.add_member(t1, t2, E, 10.0, I * 1e7);
    f.add_member(b2, t2, E, 10.0, I);
    f.fix(b1, true, true, true);
    f.fix(b2, true, true, true);
    f.load(t1, P, 0.0, 0.0);
    let r = f.analyze().unwrap();
    within(r.displacement(t1).0, P * h.powi(3) / (24.0 * E * I));
    within(r.displacement(t2).0, P * h.powi(3) / (24.0 * E * I));
    within(r.reaction(b1).0 + r.reaction(b2).0, -P);
}

#[test]
fn unsupported_structure_is_an_error() {
    let mut f = Frame::new();
    let a = f.add_node(0.0, 0.0);
    let b = f.add_node(L, 0.0);
    f.add_member(a, b, E, A, I);
    f.load(b, 0.0, -P, 0.0);
    assert!(f.analyze().is_err());
}
