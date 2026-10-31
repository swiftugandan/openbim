// External judge for T4.4 (bim-struct Frame3): textbook closed forms within 0.1%, and seeded random
// space frames against PyNite (judges/T4.4/random_frames.json, regenerate with gen_fixture.py).
//
// Conventions under test (T4.4 contract): local x from node i to node j; local z = unit component of the
// member's reference vector orthogonal to x; local y = z × x. Iz governs bending in the local x–y plane
// (shear area Asy), Iy bending in the local x–z plane (shear area Asz). End forces are the forces the
// nodes exert on the member in local axes [N1,Vy1,Vz1,T1,My1,Mz1,N2,Vy2,Vz2,T2,My2,Mz2]; a reaction is the
// force the support exerts on the structure. Internal actions at a station are those exerted by the part
// beyond the station on the part from node i to it (tension positive); envelopes are [min, max] over
// stations and combinations.
use bim_struct::{Frame3, LoadCombination, Section3};

const E: f64 = 200e9;
const G: f64 = 80e9;
const P: f64 = 10e3;

fn sec(a: f64, iy: f64, iz: f64, j: f64) -> Section3 {
    Section3 { e: E, g: G, a, iy, iz, j, asy: None, asz: None }
}

fn combo(name: &str, factors: &[(&str, f64)]) -> LoadCombination {
    LoadCombination { name: name.to_string(), factors: factors.iter().map(|(c, f)| (c.to_string(), *f)).collect() }
}

/// |actual − expected| ≤ 0.1 % of |expected| + `floor` (floor covers values that should be zero).
fn close(actual: f64, expected: f64, floor: f64, what: &str) {
    let err = (actual - expected).abs();
    assert!(
        err <= 1e-3 * expected.abs() + floor,
        "{what}: got {actual}, expected {expected} (err {err})"
    );
}

fn close_v(actual: &[f64], expected: &[f64], floor: f64, what: &str) {
    assert_eq!(actual.len(), expected.len());
    for (k, (a, e)) in actual.iter().zip(expected).enumerate() {
        close(*a, *e, floor, &format!("{what}[{k}]"));
    }
}

fn close_pair(actual: [f64; 2], expected: [f64; 2], floor: f64, what: &str) {
    close(actual[0], expected[0], floor, &format!("{what} min"));
    close(actual[1], expected[1], floor, &format!("{what} max"));
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / n, v[1] / n, v[2] / n]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn scale(v: [f64; 3], s: f64) -> [f64; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

const FIXED: [bool; 6] = [true; 6];
const FREE: [bool; 6] = [false; 6];

#[test]
fn euler_bernoulli_cantilever_biaxial_bending_torsion_and_end_forces() {
    let (l, iy, iz, j, t0) = (3.0, 2e-5, 5e-5, 1e-5, 4e3);
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(l, 0.0, 0.0);
    let m = f.add_member(a, b, sec(1e-2, iy, iz, j), [0.0, 0.0, 1.0]);
    f.fix(a, FIXED);
    f.fix(b, FREE);
    f.load(b, "P", [0.0, -P, -2.0 * P, t0, 0.0, 0.0]);
    f.add_combination(combo("U", &[("P", 1.0)]));
    let r = f.analyze().expect("stable cantilever");
    let d = r.displacement("U", b);
    let dfloor = 1e-9;
    close(d[0], 0.0, dfloor, "ux");
    close(d[1], -P * l.powi(3) / (3.0 * E * iz), dfloor, "uy (Iz bending)");
    close(d[2], -2.0 * P * l.powi(3) / (3.0 * E * iy), dfloor, "uz (Iy bending)");
    close(d[3], t0 * l / (G * j), dfloor, "rx (torsion)");
    close(d[4], 2.0 * P * l * l / (2.0 * E * iy), dfloor, "ry");
    close(d[5], -P * l * l / (2.0 * E * iz), dfloor, "rz");
    let floor = 1e-6 * P * l;
    close_v(&r.reaction("U", a), &[0.0, P, 2.0 * P, -t0, -2.0 * P * l, P * l], floor, "root reaction");
    close_v(&r.reaction("U", b), &[0.0; 6], floor, "free node has no reaction");
    let ef = r.member_end_forces("U", m);
    close_v(
        &ef,
        &[0.0, P, 2.0 * P, -t0, -2.0 * P * l, P * l, 0.0, -P, -2.0 * P, t0, 0.0, 0.0],
        floor,
        "end forces",
    );
    let env = r.envelope(m);
    close_pair(env.n, [0.0, 0.0], floor, "N");
    close_pair(env.vy, [-P, -P], floor, "Vy");
    close_pair(env.vz, [-2.0 * P, -2.0 * P], floor, "Vz");
    close_pair(env.t, [t0, t0], floor, "T");
    close_pair(env.my, [0.0, 2.0 * P * l], floor, "My");
    close_pair(env.mz, [-P * l, 0.0], floor, "Mz");
}

#[test]
fn reference_vector_orients_the_section() {
    // Member along global y with reference vector global x: local z = x, local y = z × x = global z.
    let (l, iy, iz) = (2.5, 3e-5, 8e-5);
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(0.0, l, 0.0);
    f.add_member(a, b, sec(1e-2, iy, iz, 1e-5), [1.0, 0.0, 0.0]);
    f.fix(a, FIXED);
    f.load(b, "X", [-P, 0.0, 0.0, 0.0, 0.0, 0.0]);
    f.load(b, "Z", [0.0, 0.0, -P, 0.0, 0.0, 0.0]);
    f.add_combination(combo("X", &[("X", 1.0)]));
    f.add_combination(combo("Z", &[("Z", 1.0)]));
    let r = f.analyze().unwrap();
    close(r.displacement("X", b)[0], -P * l.powi(3) / (3.0 * E * iy), 1e-9, "global x load bends about local y");
    close(r.displacement("Z", b)[2], -P * l.powi(3) / (3.0 * E * iz), 1e-9, "global z load bends about local z");

    // Inclined member, reference vector neither unit nor orthogonal to the axis.
    let d = unit([3.0, 4.0, 0.0]);
    let refv = [2.0, 2.0, 2.0];
    let x = d;
    let z = unit({
        let s = dot(refv, x);
        [refv[0] - s * x[0], refv[1] - s * x[1], refv[2] - s * x[2]]
    });
    let y = cross(z, x);
    let l = 5.0;
    let mut f = Frame3::new();
    let a = f.add_node(1.0, -2.0, 0.5);
    let b = f.add_node(1.0 + l * d[0], -2.0 + l * d[1], 0.5 + l * d[2]);
    f.add_member(a, b, sec(1e-2, iy, iz, 1e-5), refv);
    f.fix(a, FIXED);
    let fy = scale(y, -P);
    let fz = scale(z, P);
    f.load(b, "Y", [fy[0], fy[1], fy[2], 0.0, 0.0, 0.0]);
    f.load(b, "Z", [fz[0], fz[1], fz[2], 0.0, 0.0, 0.0]);
    f.add_combination(combo("Y", &[("Y", 1.0)]));
    f.add_combination(combo("Z", &[("Z", 1.0)]));
    let r = f.analyze().unwrap();
    let u = r.displacement("Y", b);
    let u = [u[0], u[1], u[2]];
    close(dot(u, y), -P * l.powi(3) / (3.0 * E * iz), 1e-9, "inclined: local-y deflection");
    close(dot(u, z), 0.0, 1e-9, "inclined: no local-z deflection under local-y load");
    let u = r.displacement("Z", b);
    let u = [u[0], u[1], u[2]];
    close(dot(u, z), P * l.powi(3) / (3.0 * E * iy), 1e-9, "inclined: local-z deflection");
    close(dot(u, x), 0.0, 1e-9, "inclined: no axial displacement");
}

#[test]
fn inclined_member_torsion_and_axial() {
    let (l, a_, j, t0) = (3.0, 4e-3, 2e-6, 2e3);
    let x = unit([1.0, 2.0, 2.0]);
    let mut f = Frame3::new();
    let n0 = f.add_node(0.0, 0.0, 0.0);
    let n1 = f.add_node(l * x[0], l * x[1], l * x[2]);
    let m = f.add_member(n0, n1, sec(a_, 1e-5, 2e-5, j), [0.0, 0.0, 1.0]);
    f.fix(n0, FIXED);
    let t = scale(x, t0);
    let n = scale(x, P);
    f.load(n1, "T", [0.0, 0.0, 0.0, t[0], t[1], t[2]]);
    f.load(n1, "N", [n[0], n[1], n[2], 0.0, 0.0, 0.0]);
    f.add_combination(combo("T", &[("T", 1.0)]));
    f.add_combination(combo("N", &[("N", 1.0)]));
    let r = f.analyze().unwrap();
    let d = r.displacement("T", n1);
    let th = t0 * l / (G * j);
    close_v(&d[3..6], &scale(x, th), 1e-9 * th, "twist vector");
    close_v(&d[0..3], &[0.0; 3], 1e-12, "torsion moves nothing");
    let d = r.displacement("N", n1);
    close_v(&d[0..3], &scale(x, P * l / (E * a_)), 1e-12, "axial elongation");
    let ef = r.member_end_forces("T", m);
    close(ef[3], -t0, 1e-6 * t0, "T1");
    close(ef[9], t0, 1e-6 * t0, "T2");
    let ef = r.member_end_forces("N", m);
    close(ef[0], -P, 1e-6 * P, "N1");
    close(ef[6], P, 1e-6 * P, "N2");
    let env = r.envelope(m);
    close_pair(env.n, [0.0, P], 1e-6 * P, "N envelope (tension positive)");
    close_pair(env.t, [0.0, t0], 1e-6 * t0, "T envelope");
}

fn timoshenko_section(iy: f64, iz: f64, asy: f64, asz: f64) -> Section3 {
    Section3 { e: E, g: G, a: 0.02, iy, iz, j: 1e-4, asy: Some(asy), asz: Some(asz) }
}

#[test]
fn timoshenko_cantilever_adds_shear_deflection_both_planes() {
    let (l, iy, iz, asy, asz) = (1.0, 2e-4, 4e-4, 0.01, 0.015);
    let build = |s: Section3| {
        let mut f = Frame3::new();
        let a = f.add_node(0.0, 0.0, 0.0);
        let b = f.add_node(l, 0.0, 0.0);
        f.add_member(a, b, s, [0.0, 0.0, 1.0]);
        f.fix(a, FIXED);
        f.load(b, "P", [0.0, -P, 2.0 * P, 0.0, 0.0, 0.0]);
        f.add_combination(combo("U", &[("P", 1.0)]));
        f.analyze().unwrap().displacement("U", b)
    };
    let d = build(timoshenko_section(iy, iz, asy, asz));
    close(d[1], -(P * l.powi(3) / (3.0 * E * iz) + P * l / (G * asy)), 1e-12, "Timoshenko uy");
    close(d[2], 2.0 * P * l.powi(3) / (3.0 * E * iy) + 2.0 * P * l / (G * asz), 1e-12, "Timoshenko uz");
    close(d[5], -P * l * l / (2.0 * E * iz), 1e-12, "Timoshenko tip rz (shear adds no rotation)");
    close(d[4], -2.0 * P * l * l / (2.0 * E * iy), 1e-12, "Timoshenko tip ry");
    let d = build(Section3 { asy: None, asz: None, ..timoshenko_section(iy, iz, asy, asz) });
    close(d[1], -P * l.powi(3) / (3.0 * E * iz), 1e-12, "no shear areas → Euler-Bernoulli uy");
    close(d[2], 2.0 * P * l.powi(3) / (3.0 * E * iy), 1e-12, "no shear areas → Euler-Bernoulli uz");
}

#[test]
fn timoshenko_guided_cantilever() {
    // Far end restrained except uy: δ = PL³/(12EI) + PL/(G·As); end moments PL/2.
    let (l, iz, asy) = (1.2, 4e-4, 0.01);
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(l, 0.0, 0.0);
    f.add_member(a, b, timoshenko_section(2e-4, iz, asy, 0.01), [0.0, 0.0, 1.0]);
    f.fix(a, FIXED);
    f.fix(b, [true, false, true, true, true, true]);
    f.load(b, "P", [0.0, -P, 0.0, 0.0, 0.0, 0.0]);
    f.add_combination(combo("U", &[("P", 1.0)]));
    let r = f.analyze().unwrap();
    close(r.displacement("U", b)[1], -(P * l.powi(3) / (12.0 * E * iz) + P * l / (G * asy)), 1e-12, "guided uy");
    close(r.reaction("U", a)[5], P * l / 2.0, 1e-6 * P, "guided root moment");
    close(r.reaction("U", b)[5], P * l / 2.0, 1e-6 * P, "guided far-end moment");
    close(r.reaction("U", b)[1], 0.0, 1e-6 * P, "no reaction in the released dof");
}

#[test]
fn timoshenko_propped_cantilever_end_moment() {
    // θ_b = M L (1+Φ) / (E I (4+Φ)); carry-over moment at the fixed end M (2−Φ)/(4+Φ).
    let (l, iz, asy, m0) = (1.5, 3e-4, 0.03, 5e3);
    let phi = 12.0 * E * iz / (G * asy * l * l);
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(l, 0.0, 0.0);
    f.add_member(a, b, timoshenko_section(2e-4, iz, asy, 0.01), [0.0, 0.0, 1.0]);
    f.fix(a, FIXED);
    f.fix(b, [true, true, true, true, false, false]);
    f.load(b, "M", [0.0, 0.0, 0.0, 0.0, 0.0, m0]);
    f.add_combination(combo("U", &[("M", 1.0)]));
    let r = f.analyze().unwrap();
    close(r.displacement("U", b)[5], m0 * l * (1.0 + phi) / (E * iz * (4.0 + phi)), 1e-15, "θ_b");
    close(r.reaction("U", a)[5], m0 * (2.0 - phi) / (4.0 + phi), 1e-6 * m0, "carry-over moment");
}

#[test]
fn simply_supported_end_moment_deflection_envelope() {
    // v(x) = M x (L² − x²) / (6 E I L), max M L² / (9√3 E I) at x = L/√3; the envelope samples
    // ≥ 11 equally spaced stations, so some station lies within 0.05 L of the maximum.
    let (l, iz, m0) = (4.0, 6e-5, 8e3);
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(l, 0.0, 0.0);
    let m = f.add_member(a, b, sec(1e-2, 1e-4, iz, 1e-5), [0.0, 0.0, 1.0]);
    f.fix(a, [true, true, true, true, false, false]);
    f.fix(b, [false, true, true, false, false, false]);
    f.load(b, "M", [0.0, 0.0, 0.0, 0.0, 0.0, m0]);
    f.add_combination(combo("U", &[("M", 1.0)]));
    let r = f.analyze().unwrap();
    close(r.displacement("U", b)[5], m0 * l / (3.0 * E * iz), 1e-12, "θ_b");
    close(r.displacement("U", a)[5], -m0 * l / (6.0 * E * iz), 1e-12, "θ_a");
    let v = |xi: f64| m0 * l * l * xi * (1.0 - xi * xi) / (6.0 * E * iz);
    let xs = 1.0 / 3f64.sqrt();
    let vmax = v(xs);
    let vlow = v(xs - 0.05).min(v(xs + 0.05));
    let env = r.envelope(m);
    assert!(
        env.deflection <= vmax * 1.001 && env.deflection >= vlow * 0.999,
        "deflection envelope {} outside [{vlow}, {vmax}]",
        env.deflection
    );
    close_pair(env.mz, [0.0, m0], 1e-6 * m0, "Mz envelope");
    close_pair(env.vy, [-m0 / l, -m0 / l], 1e-6 * m0, "Vy envelope");
}

/// Fixed-base portal (columns along z at x = 0 and x = w, rigid beam on top, axially rigid columns).
#[test]
fn portal_frame_sway_combinations_and_envelopes() {
    let (h, w, iy, iz, q) = (3.0, 5.0, 1e-4, 4e-5, 30e3);
    let col = Section3 { e: E, g: G, a: 10.0, iy, iz, j: 1e-5, asy: None, asz: None };
    let beam = Section3 { e: E, g: G, a: 10.0, iy: 1e3, iz: 1e3, j: 1e3, asy: None, asz: None };
    let mut f = Frame3::new();
    let b1 = f.add_node(0.0, 0.0, 0.0);
    let t1 = f.add_node(0.0, 0.0, h);
    let t2 = f.add_node(w, 0.0, h);
    let b2 = f.add_node(w, 0.0, 0.0);
    let c1 = f.add_member(b1, t1, col, [1.0, 0.0, 0.0]);
    f.add_member(t1, t2, beam, [0.0, 0.0, 1.0]);
    f.add_member(b2, t2, col, [1.0, 0.0, 0.0]);
    f.fix(b1, FIXED);
    f.fix(b2, FIXED);
    f.load(t1, "W", [P, 0.0, 0.0, 0.0, 0.0, 0.0]);
    f.load(t1, "G", [0.0, 0.0, -q, 0.0, 0.0, 0.0]);
    f.load(t2, "G", [0.0, 0.0, -q, 0.0, 0.0, 0.0]);
    f.load(t1, "Y", [0.0, P / 2.0, 0.0, 0.0, 0.0, 0.0]);
    f.load(t2, "Y", [0.0, P / 2.0, 0.0, 0.0, 0.0, 0.0]);
    f.load(t1, "UNUSED", [1e9, 1e9, 1e9, 1e9, 1e9, 1e9]); // a case in no combination
    f.add_combination(combo("C1", &[("G", 1.0), ("W", 1.5)]));
    f.add_combination(combo("C2", &[("G", 1.0), ("W", -1.5)]));
    f.add_combination(combo("CY", &[("Y", 1.0)]));
    let r = f.analyze().unwrap();

    // In-plane sway: each column fixed-fixed, Δ = P_total H³ / (24 E Iy) (ref x → x–z bending uses Iy).
    let sway = 1.5 * P * h.powi(3) / (24.0 * E * iy);
    close(r.displacement("C1", t1)[0], sway, 1e-12, "C1 sway t1");
    close(r.displacement("C1", t2)[0], sway, 1e-12, "C1 sway t2");
    close(r.displacement("C2", t1)[0], -sway, 1e-12, "C2 sway");
    // Out of plane: two cantilevers tied by the rigid beam, Δ = (P/2) H³ / (3 E Iz).
    close(r.displacement("CY", t1)[1], (P / 2.0) * h.powi(3) / (3.0 * E * iz), 1e-12, "out-of-plane sway");

    // Reactions: shears split equally, overturning shared by base moments PH/4 and an axial couple PH/(2w).
    let fz1 = q - 1.5 * P * h / (2.0 * w);
    let rr = r.reaction("C1", b1);
    close(rr[0], -0.75 * P, 1e-6 * P, "C1 base shear b1");
    close(rr[2], fz1, 1e-6 * P, "C1 vertical reaction b1");
    close(rr[4], -1.5 * P * h / 4.0, 1e-6 * P, "C1 base moment b1");
    let rr2 = r.reaction("C1", b2);
    close(rr[0] + rr2[0], -1.5 * P, 1e-6 * P, "C1 Σfx");
    close(rr[2] + rr2[2], 2.0 * q, 1e-6 * P, "C1 Σfz");

    // Windward column envelope over C1, C2 and CY (CY: cantilever bending in the local x–y plane).
    let env = r.envelope(c1);
    let nc = 1.5 * P * h / (2.0 * w);
    let fl = 1e-6 * P * h;
    close_pair(env.n, [-q - nc, 0.0f64.max(-q + nc)], fl, "column N");
    close_pair(env.vz, [-0.75 * P, 0.75 * P], fl, "column Vz");
    close_pair(env.my, [-1.5 * P * h / 4.0, 1.5 * P * h / 4.0], fl, "column My");
    close_pair(env.vy, [-P / 2.0, 0.0], fl, "column Vy (CY): local y = −global y");
    close_pair(env.mz, [-P / 2.0 * h, 0.0], fl, "column Mz (CY)");
    close_pair(env.t, [0.0, 0.0], fl, "column T");
}

#[test]
fn mechanism_is_an_error() {
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(3.0, 0.0, 0.0);
    f.add_member(a, b, sec(1e-2, 1e-4, 1e-4, 1e-5), [0.0, 0.0, 1.0]);
    f.fix(a, [true, true, true, false, false, false]); // pinned: free to spin about the support
    f.load(b, "P", [0.0, -P, 0.0, 0.0, 0.0, 0.0]);
    f.add_combination(combo("U", &[("P", 1.0)]));
    assert!(f.analyze().is_err(), "a pinned single member is a mechanism");
    let mut f = Frame3::new();
    let a = f.add_node(0.0, 0.0, 0.0);
    let b = f.add_node(3.0, 0.0, 0.0);
    f.add_member(a, b, sec(1e-2, 1e-4, 1e-4, 1e-5), [0.0, 0.0, 1.0]);
    f.load(b, "P", [0.0, -P, 0.0, 0.0, 0.0, 0.0]);
    f.add_combination(combo("U", &[("P", 1.0)]));
    assert!(f.analyze().is_err(), "an unsupported structure is an error");
}

fn arr(v: &serde_json::Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
}

#[test]
fn random_space_frames_match_pynite() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/random_frames.json");
    let data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let frames = data["frames"].as_array().unwrap();
    assert!(frames.len() >= 6);
    for (fi, fr) in frames.iter().enumerate() {
        let mut f = Frame3::new();
        let nodes: Vec<usize> = fr["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let p = arr(p);
                f.add_node(p[0], p[1], p[2])
            })
            .collect();
        let mut members = Vec::new();
        for mb in fr["members"].as_array().unwrap() {
            let g = |k: &str| mb[k].as_f64().unwrap();
            let r = arr(&mb["ref"]);
            let s = Section3 { e: g("e"), g: g("g"), a: g("a"), iy: g("iy"), iz: g("iz"), j: g("j_t"), asy: None, asz: None };
            let i = nodes[mb["i"].as_u64().unwrap() as usize];
            let j = nodes[mb["j"].as_u64().unwrap() as usize];
            members.push(f.add_member(i, j, s, [r[0], r[1], r[2]]));
        }
        for s in fr["supports"].as_array().unwrap() {
            let fx: Vec<bool> = s["fix"].as_array().unwrap().iter().map(|b| b.as_bool().unwrap()).collect();
            f.fix(nodes[s["node"].as_u64().unwrap() as usize], [fx[0], fx[1], fx[2], fx[3], fx[4], fx[5]]);
        }
        for ld in fr["loads"].as_array().unwrap() {
            let v = arr(&ld["f"]);
            f.load(nodes[ld["node"].as_u64().unwrap() as usize], ld["case"].as_str().unwrap(), [v[0], v[1], v[2], v[3], v[4], v[5]]);
        }
        let combos: Vec<String> = fr["combos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                let name = c["name"].as_str().unwrap().to_string();
                let factors = c["factors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| (p[0].as_str().unwrap().to_string(), p[1].as_f64().unwrap()))
                    .collect();
                f.add_combination(LoadCombination { name: name.clone(), factors });
                name
            })
            .collect();
        let r = f.analyze().unwrap_or_else(|e| panic!("frame {fi}: analyze failed: {e}"));
        let exp = &fr["expected"];
        // Per-frame magnitude scales for the zero-value floor.
        let maxabs = |vals: &mut dyn Iterator<Item = f64>| vals.fold(0.0f64, |m, v| m.max(v.abs()));
        for c in &combos {
            let disp = exp["displacement"][c].as_array().unwrap();
            let all: Vec<Vec<f64>> = disp.iter().map(arr).collect();
            let tmax = maxabs(&mut all.iter().flat_map(|d| d[0..3].to_vec()));
            let rmax = maxabs(&mut all.iter().flat_map(|d| d[3..6].to_vec()));
            for (k, d) in all.iter().enumerate() {
                let got = r.displacement(c, nodes[k]);
                close_v(&got[0..3], &d[0..3], 1e-6 * tmax, &format!("frame {fi} {c} node {k} translation"));
                close_v(&got[3..6], &d[3..6], 1e-6 * rmax, &format!("frame {fi} {c} node {k} rotation"));
            }
            let efs: Vec<Vec<f64>> = exp["end_forces"][c].as_array().unwrap().iter().map(arr).collect();
            let fmax = maxabs(&mut efs.iter().flat_map(|e| [0, 1, 2, 6, 7, 8].map(|q| e[q])));
            let mmax = maxabs(&mut efs.iter().flat_map(|e| [3, 4, 5, 9, 10, 11].map(|q| e[q])));
            for (k, e) in efs.iter().enumerate() {
                let got = r.member_end_forces(c, members[k]);
                for q in 0..12 {
                    let floor = 1e-6 * if q % 6 < 3 { fmax } else { mmax };
                    close(got[q], e[q], floor, &format!("frame {fi} {c} member {k} end force {q}"));
                }
            }
            for (node, rx) in exp["reaction"][c].as_object().unwrap() {
                let n: usize = node.parse().unwrap();
                let rx = arr(rx);
                let got = r.reaction(c, nodes[n]);
                close_v(&got[0..3], &rx[0..3], 1e-6 * fmax, &format!("frame {fi} {c} reaction force node {n}"));
                close_v(&got[3..6], &rx[3..6], 1e-6 * mmax, &format!("frame {fi} {c} reaction moment node {n}"));
            }
        }
        let envs = exp["envelope"].as_array().unwrap();
        let emax: Vec<f64> = (0..6)
            .map(|q| maxabs(&mut envs.iter().flat_map(|e| arr(&e[q]))))
            .collect();
        for (k, e) in envs.iter().enumerate() {
            let got = r.envelope(members[k]);
            let names = ["N", "Vy", "Vz", "T", "My", "Mz"];
            let vals = [got.n, got.vy, got.vz, got.t, got.my, got.mz];
            for q in 0..6 {
                let ev = arr(&e[q]);
                close_pair(vals[q], [ev[0], ev[1]], 1e-6 * emax[q], &format!("frame {fi} member {k} envelope {}", names[q]));
            }
        }
    }
}
