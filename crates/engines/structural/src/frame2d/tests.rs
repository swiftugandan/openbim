//! Unit tests for the 2D frame solver.

use super::*;

const E: f64 = 200e9;
const A: f64 = 1e-2;
const I: f64 = 1e-4;
const L: f64 = 3.0;
const P: f64 = 10e3;

/// `|actual - expected| <= rel * |expected| + abs`. Expected values that are
/// exactly zero must be given an absolute tolerance scaled to the problem.
#[track_caller]
fn near(actual: f64, expected: f64, rel: f64, abs: f64) {
    let tol = rel * expected.abs() + abs;
    assert!(
        (actual - expected).abs() <= tol,
        "got {actual}, expected {expected} (tolerance {tol})"
    );
}

#[track_caller]
fn near_all(actual: &[f64], expected: &[f64], rel: f64, abs: f64) {
    assert_eq!(actual.len(), expected.len());
    for (a, e) in actual.iter().zip(expected) {
        near(*a, *e, rel, abs);
    }
}

/// Root fixed at (0, 0), member along +x, downward tip load `P` at (L, 0).
fn cantilever() -> Frame {
    let mut f = Frame::new();
    let root = f.add_node(0.0, 0.0);
    let tip = f.add_node(L, 0.0);
    f.add_member(root, tip, E, A, I);
    f.fix(root, true, true, true);
    f.load(tip, 0.0, -P, 0.0);
    f
}

/// Two members of length L/2, pin at the left, roller at the right, `P` down
/// at midspan.
fn simply_supported() -> Frame {
    let mut f = Frame::new();
    let a = f.add_node(0.0, 0.0);
    let mid = f.add_node(L / 2.0, 0.0);
    let b = f.add_node(L, 0.0);
    f.add_member(a, mid, E, A, I);
    f.add_member(mid, b, E, A, I);
    f.fix(a, true, true, false);
    f.fix(b, false, true, false);
    f.load(mid, 0.0, -P, 0.0);
    f
}

/// Fixed-base portal, columns of height 3 and span 5, beam stiffer than the
/// columns by `beam_i` in bending, sway load `P` at the left beam-column joint.
fn portal(beam_i: f64) -> Frame {
    let mut f = Frame::new();
    let b1 = f.add_node(0.0, 0.0);
    let t1 = f.add_node(0.0, 3.0);
    let t2 = f.add_node(5.0, 3.0);
    let b2 = f.add_node(5.0, 0.0);
    f.add_member(b1, t1, E, 10.0, I);
    f.add_member(t1, t2, E, 10.0, I * beam_i);
    f.add_member(b2, t2, E, 10.0, I);
    f.fix(b1, true, true, true);
    f.fix(b2, true, true, true);
    f.load(t1, P, 0.0, 0.0);
    f
}

/// The sloped frame of the PyNite cross-check: members N0-N1 (vertical),
/// N1-N2 (rising) and N3-N2 (running right to left); fixed at N0, pinned at N3.
fn sloped_frame() -> Frame {
    let mut f = Frame::new();
    let n0 = f.add_node(0.0, 0.0);
    let n1 = f.add_node(0.0, 3.0);
    let n2 = f.add_node(4.0, 5.0);
    let n3 = f.add_node(6.0, 0.0);
    f.add_member(n0, n1, E, A, I);
    f.add_member(n1, n2, E, A, I);
    f.add_member(n3, n2, E, A, I);
    f.fix(n0, true, true, true);
    f.fix(n3, true, true, false);
    f.load(n1, 12e3, 0.0, 0.0);
    f.load(n2, 0.0, -30e3, 5e3);
    f
}

/// Largest magnitude among loads, reactions and end forces (a force scale for
/// absolute tolerances).
fn force_scale(f: &Frame, r: &Results) -> f64 {
    let loads = f.loads.iter().flatten().copied();
    let reactions = (0..f.nodes.len()).flat_map(|n| {
        let (x, y, z) = r.reaction(n);
        [x, y, z]
    });
    let ends = (0..f.members.len()).flat_map(|m| r.member_end_forces(m));
    loads
        .chain(reactions)
        .chain(ends)
        .fold(0.0, |s, v| s.max(v.abs()))
}

/// Global statics: loads plus reactions sum to zero in x, y and in moment
/// about the origin.
#[track_caller]
fn assert_global_equilibrium(f: &Frame, r: &Results) {
    let (mut sx, mut sy, mut sm) = (0.0, 0.0, 0.0);
    for (n, p) in f.nodes.iter().enumerate() {
        let [fx, fy, mz] = f.loads[n];
        let (rx, ry, rm) = r.reaction(n);
        let (tx, ty) = (fx + rx, fy + ry);
        sx += tx;
        sy += ty;
        sm += p[0] * ty - p[1] * tx + mz + rm;
    }
    let scale = force_scale(f, r);
    let extent = f
        .nodes
        .iter()
        .flatten()
        .fold(1.0, |s: f64, v| s.max(v.abs()));
    near(sx, 0.0, 0.0, 1e-9 * scale);
    near(sy, 0.0, 0.0, 1e-9 * scale);
    near(sm, 0.0, 0.0, 1e-9 * scale * extent * f.nodes.len() as f64);
}

/// Every member is in equilibrium on its own: `N1 + N2 = 0`, `V1 + V2 = 0`,
/// `M1 + M2 + V2 L = 0`.
#[track_caller]
fn assert_member_equilibrium(f: &Frame, r: &Results) {
    // A member that carries (almost) nothing has round-off-sized end forces, so
    // the tolerance is relative to the largest force anywhere in the structure.
    let scale = force_scale(f, r);
    let geoms = f.validate().unwrap();
    for (m, g) in geoms.iter().enumerate() {
        let ef = r.member_end_forces(m);
        near(ef[0] + ef[3], 0.0, 0.0, 1e-9 * scale);
        near(ef[1] + ef[4], 0.0, 0.0, 1e-9 * scale);
        near(
            ef[2] + ef[5] + ef[4] * g.length,
            0.0,
            0.0,
            1e-9 * scale * g.length.max(1.0),
        );
    }
}

/// Equilibrium of every node, using the end forces: the forces the nodes
/// exert on the members, rotated to global axes and summed over the members
/// meeting at a node, equal the applied load plus the reaction.
#[track_caller]
fn assert_nodal_equilibrium(f: &Frame, r: &Results) {
    let geoms = f.validate().unwrap();
    let mut sum = vec![[0.0; NDOF]; f.nodes.len()];
    for (m, (mem, g)) in f.members.iter().zip(&geoms).enumerate() {
        let ef = r.member_end_forces(m);
        for (node, off) in [(mem.i, 0), (mem.j, 3)] {
            let (n, v, mz) = (ef[off], ef[off + 1], ef[off + 2]);
            sum[node][0] += g.c * n - g.s * v;
            sum[node][1] += g.s * n + g.c * v;
            sum[node][2] += mz;
        }
    }
    let scale = force_scale(f, r);
    for (node, got) in sum.iter().enumerate() {
        let (rx, ry, rm) = r.reaction(node);
        let want = [
            f.loads[node][0] + rx,
            f.loads[node][1] + ry,
            f.loads[node][2] + rm,
        ];
        near_all(got, &want, 0.0, 1e-9 * scale);
    }
}

#[track_caller]
fn assert_statics(f: &Frame, r: &Results) {
    assert_global_equilibrium(f, r);
    assert_member_equilibrium(f, r);
    assert_nodal_equilibrium(f, r);
}

// ---- textbook cases, with signs -------------------------------------------------

#[test]
fn cantilever_tip_load() {
    let f = cantilever();
    let r = f.analyze().unwrap();
    let (ux, uy, rz) = r.displacement(1);
    near(ux, 0.0, 0.0, 1e-15);
    near(uy, -P * L.powi(3) / (3.0 * E * I), 1e-9, 0.0);
    near(rz, -P * L * L / (2.0 * E * I), 1e-9, 0.0);
    assert_eq!(r.displacement(0), (0.0, 0.0, 0.0));

    // The support pushes up and resists the clockwise load moment counter-clockwise.
    let (fx, fy, mz) = r.reaction(0);
    near(fx, 0.0, 0.0, 1e-9 * P);
    near(fy, P, 1e-9, 0.0);
    near(mz, P * L, 1e-9, 0.0);
    assert_eq!(
        r.reaction(1),
        (0.0, 0.0, 0.0),
        "the free tip has no reaction"
    );

    // Forces the nodes exert on the member: the root pushes up (V1 > 0) with a
    // counter-clockwise moment, the tip load pushes down (V2 < 0), no tip moment.
    near_all(
        &r.member_end_forces(0),
        &[0.0, P, P * L, 0.0, -P, 0.0],
        1e-9,
        1e-9 * P * L,
    );
    assert_statics(&f, &r);
}

#[test]
fn subdivided_cantilever_matches_the_single_member() {
    let mut f = Frame::new();
    let nodes: Vec<usize> = (0..=4)
        .map(|k| f.add_node(L * k as f64 / 4.0, 0.0))
        .collect();
    for w in nodes.windows(2) {
        f.add_member(w[0], w[1], E, A, I);
    }
    f.fix(nodes[0], true, true, true);
    f.load(nodes[4], 0.0, -P, 0.0);
    let r = f.analyze().unwrap();
    near(
        r.displacement(nodes[4]).1,
        -P * L.powi(3) / (3.0 * E * I),
        1e-9,
        0.0,
    );
    let a = L / 2.0;
    near(
        r.displacement(nodes[2]).1,
        -P * a * a * (3.0 * L - a) / (6.0 * E * I),
        1e-9,
        0.0,
    );
    // The root member carries the full shear and the full moment.
    let ef = r.member_end_forces(0);
    near(ef[1], P, 1e-9, 0.0);
    near(ef[2], P * L, 1e-9, 0.0);
    assert_statics(&f, &r);
}

#[test]
fn simply_supported_beam_midspan_load() {
    let f = simply_supported();
    let r = f.analyze().unwrap();
    near(
        r.displacement(1).1,
        -P * L.powi(3) / (48.0 * E * I),
        1e-9,
        0.0,
    );
    // Rotations at the supports: clockwise at the left, counter-clockwise at the right.
    let theta = P * L * L / (16.0 * E * I);
    near(r.displacement(0).2, -theta, 1e-9, 0.0);
    near(r.displacement(2).2, theta, 1e-9, 0.0);
    near(r.displacement(1).2, 0.0, 0.0, 1e-15);
    near(r.reaction(0).0, 0.0, 0.0, 1e-9 * P);
    near(r.reaction(0).1, P / 2.0, 1e-9, 0.0);
    near(r.reaction(2).1, P / 2.0, 1e-9, 0.0);
    // The pin and the roller are free to rotate, and the roller to move along x:
    // those reaction components are exactly zero, as is everything at the free midspan.
    assert_eq!(r.reaction(0).2, 0.0);
    assert_eq!((r.reaction(2).0, r.reaction(2).2), (0.0, 0.0));
    assert_eq!(r.reaction(1), (0.0, 0.0, 0.0));
    // Sagging: the left half has V1 = P/2 and a counter-clockwise end moment PL/4 at midspan.
    let half = P * L / 4.0;
    near_all(
        &r.member_end_forces(0),
        &[0.0, P / 2.0, 0.0, 0.0, -P / 2.0, half],
        1e-9,
        1e-9 * P,
    );
    near_all(
        &r.member_end_forces(1),
        &[0.0, -P / 2.0, -half, 0.0, P / 2.0, 0.0],
        1e-9,
        1e-9 * P,
    );
    assert_statics(&f, &r);
}

#[test]
fn vertical_bar_in_tension_and_compression() {
    for sign in [1.0, -1.0] {
        let mut f = Frame::new();
        let a = f.add_node(0.0, 0.0);
        let b = f.add_node(0.0, L);
        f.add_member(a, b, E, A, I);
        f.fix(a, true, true, true);
        f.load(b, 0.0, sign * P, 0.0);
        let r = f.analyze().unwrap();
        near(r.displacement(b).1, sign * P * L / (E * A), 1e-9, 0.0);
        // Local x runs up the bar: in tension the bottom node pulls the member
        // down (N1 < 0) and the top node pulls it up (N2 > 0).
        near_all(
            &r.member_end_forces(0),
            &[-sign * P, 0.0, 0.0, sign * P, 0.0, 0.0],
            1e-9,
            1e-9 * P,
        );
        near_all(
            &[r.reaction(a).0, r.reaction(a).1, r.reaction(a).2],
            &[0.0, -sign * P, 0.0],
            1e-9,
            1e-9 * P,
        );
        assert_statics(&f, &r);
    }
}

#[test]
fn portal_with_a_near_rigid_beam_sways_by_the_closed_form() {
    let f = portal(1e7);
    let r = f.analyze().unwrap();
    let sway = P * 27.0 / (24.0 * E * I);
    // The closed form assumes an infinitely stiff beam and rigid columns in
    // axial strain; the finite model sits about 2e-5 away, as it should.
    near(r.displacement(1).0, sway, 1e-4, 0.0);
    near(r.displacement(2).0, sway, 1e-4, 0.0);
    near(r.reaction(0).0 + r.reaction(3).0, -P, 1e-9, 0.0);
    near(r.reaction(0).1 + r.reaction(3).1, 0.0, 0.0, 1e-9 * P);
    // Each column carries half the shear and bends in double curvature.
    near(r.reaction(0).0, -P / 2.0, 1e-4, 0.0);
    near(r.reaction(0).2, P * 3.0 / 4.0, 1e-3, 0.0);
    near(r.member_end_forces(0)[1], P / 2.0, 1e-4, 0.0);
    assert_statics(&f, &r);
}

#[test]
fn beam_far_stiffer_in_bending_is_not_mistaken_for_a_mechanism() {
    for beam_i in [1.0, 1e9, 1e12] {
        let f = portal(beam_i);
        let r = f
            .analyze()
            .unwrap_or_else(|e| panic!("I x {beam_i:e}: {e}"));
        assert_statics(&f, &r);
    }
}

#[test]
fn rotated_cantilever_end_forces_and_displacements() {
    // Member (0,0) -> (3,4): length 5, c = 0.6, s = 0.8.
    let mut f = Frame::new();
    let a = f.add_node(0.0, 0.0);
    let b = f.add_node(3.0, 4.0);
    f.add_member(a, b, E, A, I);
    f.fix(a, true, true, true);
    f.load(b, 0.0, -P, 0.0);
    let r = f.analyze().unwrap();
    near_all(
        &r.member_end_forces(0),
        &[0.8 * P, 0.6 * P, 3.0 * P, -0.8 * P, -0.6 * P, 0.0],
        1e-9,
        1e-9 * P,
    );
    let (fx, fy, mz) = r.reaction(a);
    near_all(&[fx, fy, mz], &[0.0, P, 3.0 * P], 1e-9, 1e-9 * P);

    // Tip displacement from the local cantilever formulas, rotated to global axes.
    let (c, s, len) = (0.6, 0.8, 5.0);
    let (fl_x, fl_y) = (-s * P, -c * P);
    let ul = fl_x * len / (E * A);
    let vl = fl_y * len.powi(3) / (3.0 * E * I);
    let theta = fl_y * len * len / (2.0 * E * I);
    let (ux, uy, rz) = r.displacement(b);
    near(ux, c * ul - s * vl, 1e-9, 0.0);
    near(uy, s * ul + c * vl, 1e-9, 0.0);
    near(rz, theta, 1e-9, 0.0);
    assert_statics(&f, &r);
}

#[test]
fn parallel_members_add_their_stiffness() {
    let single = cantilever().analyze().unwrap();
    let mut f = cantilever();
    f.add_member(0, 1, E, A, I); // a second, identical member between the same nodes
    let double = f.analyze().unwrap();
    near(
        double.displacement(1).1,
        single.displacement(1).1 / 2.0,
        1e-9,
        0.0,
    );
    for m in 0..2 {
        near_all(
            &double.member_end_forces(m),
            &[0.0, P / 2.0, P * L / 2.0, 0.0, -P / 2.0, 0.0],
            1e-9,
            1e-9 * P * L,
        );
    }
    assert_statics(&f, &double);
}

#[test]
fn member_direction_changes_only_the_local_description() {
    let build = |reversed: bool| {
        let mut f = Frame::new();
        let root = f.add_node(0.0, 0.0);
        let tip = f.add_node(L, 0.0);
        if reversed {
            f.add_member(tip, root, E, A, I);
        } else {
            f.add_member(root, tip, E, A, I);
        }
        f.fix(root, true, true, true);
        f.load(tip, 0.0, -P, 0.0);
        f
    };
    let (fwd, rev) = (build(false), build(true));
    let (rf, rr) = (fwd.analyze().unwrap(), rev.analyze().unwrap());
    for node in 0..2 {
        near_all(
            &[
                rr.displacement(node).0,
                rr.displacement(node).1,
                rr.displacement(node).2,
            ],
            &[
                rf.displacement(node).0,
                rf.displacement(node).1,
                rf.displacement(node).2,
            ],
            1e-9,
            1e-15,
        );
        near_all(
            &[
                rr.reaction(node).0,
                rr.reaction(node).1,
                rr.reaction(node).2,
            ],
            &[
                rf.reaction(node).0,
                rf.reaction(node).1,
                rf.reaction(node).2,
            ],
            1e-9,
            1e-9 * P,
        );
    }
    // Local x now points from the tip to the root and local y is rotated with it
    // (global down), so shears change sign and the two ends swap roles.
    near_all(
        &rr.member_end_forces(0),
        &[0.0, P, 0.0, 0.0, -P, P * L],
        1e-9,
        1e-9 * P * L,
    );
    assert_statics(&rev, &rr);
}

/// Compares a whole set of results with reference values: displacements and
/// reactions of every node, end forces of every member.
#[track_caller]
fn assert_matches_reference(
    r: &Results,
    disp: &[[f64; 3]],
    react: &[[f64; 3]],
    end_forces: &[[f64; 6]],
) {
    let (rel, abs_u, abs_f) = (1e-9, 1e-15, 1e-6);
    for (n, want) in disp.iter().enumerate() {
        let d = r.displacement(n);
        near_all(&[d.0, d.1, d.2], want, rel, abs_u);
    }
    for (n, want) in react.iter().enumerate() {
        let d = r.reaction(n);
        near_all(&[d.0, d.1, d.2], want, rel, abs_f);
    }
    for (m, want) in end_forces.iter().enumerate() {
        near_all(&r.member_end_forces(m), want, rel, abs_f);
    }
}

#[test]
fn sloped_frame_matches_pynite() {
    // Reference values from PyNite 3.2.1. Member 2 runs right to left, where PyNite
    // turns the local y and z axes around; its shear and moments are given here in
    // this crate's local axes (local z = global z).
    let f = sloped_frame();
    let r = f.analyze().unwrap();
    assert_matches_reference(
        &r,
        &[
            [0.0, 0.0, 0.0],
            [
                3.99368337046907e-4,
                -8.079313927687373e-6,
                -1.5057415874068948e-4,
            ],
            [
                3.3724294642694313e-4,
                5.735921539943245e-5,
                2.1263129193038649e-4,
            ],
            [0.0, 0.0, -1.9946736128934395e-4],
        ],
        &[
            [-1542.285323874424, 5386.209285124915, 3317.2557107495663],
            [0.0; 3],
            [0.0; 3],
            [-10457.7146761256, 24613.790714875096, 0.0],
        ],
        &[
            [
                5386.209285124915,
                1542.285323874424,
                3317.2557107495663,
                -5386.209285124915,
                -1542.285323874424,
                1309.6002608737067,
            ],
            [
                11762.450382561568,
                140.73986000913783,
                -1309.6002608737085,
                -11762.450382561568,
                -140.73986000913783,
                1939.008049122181,
            ],
            [
                26737.22867977407,
                568.4119354754905,
                4.547473508864641e-13,
                -26737.22867977407,
                -568.4119354754905,
                3060.9919508778194,
            ],
        ],
    );
    // Free degrees of freedom carry no reaction at all, not merely a small one.
    assert_eq!(r.reaction(1), (0.0, 0.0, 0.0));
    assert_eq!(r.reaction(2), (0.0, 0.0, 0.0));
    assert_eq!(r.reaction(3).2, 0.0, "the pin at N3 is free to rotate");
    assert_statics(&f, &r);
}

#[test]
fn frame_with_rollers_and_right_to_left_members_matches_pynite() {
    // Four nodes, five members (four of them run right to left), a fixed node and two
    // rollers that restrain only y, loads with moments on the roller nodes.
    // Reference values from PyNite 3.2.1 (right-to-left members converted as above).
    let mut f = Frame::new();
    for (x, y) in [
        (6.693327793521672, 3.2045524683706104),
        (3.9469186925042754, 5.263554156706467),
        (7.821356307437433, 5.29415725661609),
        (9.752016812821674, 3.7006190259596767),
    ] {
        f.add_node(x, y);
    }
    for (i, j, e, a, iz) in [
        (0, 1, 200e9, 1e-3, 1e-5),
        (2, 0, 70e9, 5e-2, 1e-3),
        (0, 3, 30e9, 5e-2, 1e-4),
        (2, 1, 70e9, 5e-2, 1e-3),
        (3, 2, 30e9, 5e-2, 1e-5),
    ] {
        f.add_member(i, j, e, a, iz);
    }
    f.fix(1, true, true, true);
    f.fix(2, false, true, false);
    f.fix(0, false, true, false);
    f.load(
        2,
        40121.86202868512,
        -38369.05506280705,
        -12234.938440478769,
    );
    f.load(0, 24601.191945740415, 7858.80015599465, 14980.431320290169);
    let r = f.analyze().unwrap();
    assert_matches_reference(
        &r,
        &[
            [0.00013606356883751132, 0.0, 0.0002100115756950194],
            [0.0, 0.0, 0.0],
            [6.617069333399307e-05, 0.0, -0.00010496012331117752],
            [
                0.00012467750034407266,
                7.147107764529546e-05,
                -5.038837141724484e-05,
            ],
        ],
        &[
            [0.0, 33343.171450836446, 0.0],
            [-64723.053974425566, 536.3936828851265, -3616.31261550579],
            [0.0, -3369.3102269091614, 0.0],
            [0.0; 3],
        ],
        &[
            [
                -6343.206011441258,
                165.45849620386738,
                406.3360198595581,
                6343.206011441258,
                -165.45849620386738,
                161.60495981156294,
            ],
            [
                48935.82418870638,
                3966.2815076213865,
                -4575.561527268033,
                -48935.82418870638,
                -3966.2815076213865,
                13994.036687701911,
            ],
            [
                -98.10315274734603,
                211.67202539597238,
                580.058612728705,
                98.10315274734603,
                -211.67202539597238,
                75.83988984174107,
            ],
            [
                -59772.02403878021,
                -2928.9466222568644,
                -7570.457384864074,
                59772.02403878021,
                2928.9466222568644,
                -3777.9175753173527,
            ],
            [
                223.82497987326133,
                -65.81529684607045,
                -75.839889841741,
                -223.82497987326133,
                65.81529684607045,
                -88.91952834666287,
            ],
        ],
    );
    // The rollers carry no x force or moment, exactly.
    for roller in [0, 2] {
        let (fx, _, mz) = r.reaction(roller);
        assert_eq!((fx, mz), (0.0, 0.0));
    }
    assert_eq!(r.reaction(3), (0.0, 0.0, 0.0));
    assert_statics(&f, &r);
}

// ---- element matrix ------------------------------------------------------------

#[test]
fn member_stiffness_has_the_rigid_body_null_space_in_any_orientation() {
    // A beam strains under no rigid-body motion: k_g maps translations and
    // a rotation about any point to zero force. A wrong T (T instead of Tᵀ in
    // Tᵀ k T) fails this for every inclined member.
    for (x1, y1, x2, y2) in [
        (0.0, 0.0, 3.0, 4.0),
        (1.0, 2.0, -2.0, 5.0),
        (0.5, 0.0, 0.5, -3.0),
    ] {
        let mem = Member {
            i: 0,
            j: 1,
            e: E,
            a: A,
            iz: I,
        };
        let (dx, dy) = (x2 - x1, y2 - y1);
        let length = f64::hypot(dx, dy);
        let g = Geometry {
            length,
            c: dx / length,
            s: dy / length,
        };
        let kg = global_stiffness(&mem, &g);
        let scale = kg.iter().flatten().fold(0.0f64, |s, v| s.max(v.abs()));
        let modes: [Vec6; 3] = [
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0, 1.0, 0.0],
            [-y1, x1, 1.0, -y2, x2, 1.0],
        ];
        for mode in &modes {
            for row in mat_vec(&kg, mode) {
                near(row, 0.0, 0.0, 1e-12 * scale * 10.0);
            }
        }
        assert_eq!(kg, transpose(&kg), "k_g must be exactly symmetric");
    }
}

// ---- unstable structures and bad data ------------------------------------------

#[test]
fn mechanisms_are_errors_not_results() {
    // No supports at all.
    let mut f = Frame::new();
    let (a, b) = (f.add_node(0.0, 0.0), f.add_node(L, 0.0));
    f.add_member(a, b, E, A, I);
    f.load(b, 0.0, -P, 0.0);
    let err = f.analyze().unwrap_err();
    assert!(err.contains("unstable or singular"), "{err}");

    // Rollers at both ends leave a rigid-body motion along x.
    f.fix(a, false, true, false);
    f.fix(b, false, true, false);
    assert!(f.analyze().is_err());

    // Only a rotation held: both translations remain rigid-body modes.
    f.fix(a, false, false, true);
    f.fix(b, false, false, false);
    assert!(f.analyze().is_err());

    // A pinned root leaves the cantilever free to rotate about it.
    let mut pinned = cantilever();
    pinned.fix(0, true, true, false);
    assert!(pinned.analyze().is_err());

    // A supported part and an unsupported part joined to nothing.
    let mut split = cantilever();
    let (c, d) = (split.add_node(10.0, 0.0), split.add_node(13.0, 0.0));
    split.add_member(c, d, E, A, I);
    assert!(split.analyze().is_err());
}

#[test]
fn supports_that_leave_rotations_free_are_still_stable() {
    // Pinned at both ends, axially restrained at one: not a mechanism.
    let mut f = simply_supported();
    f.fix(0, true, true, false);
    f.fix(2, false, true, false);
    assert!(f.analyze().is_ok());
    // Remove the axial restraint and it is one.
    f.fix(0, false, true, false);
    assert!(f.analyze().is_err());
}

#[test]
fn a_free_node_without_members_is_unstable_and_a_fixed_one_is_fine() {
    let mut f = cantilever();
    let stray = f.add_node(10.0, 10.0);
    let err = f.analyze().unwrap_err();
    assert!(
        err.contains("unstable or singular") && err.contains(&format!("node {stray}")),
        "{err}"
    );
    f.fix(stray, true, true, false); // rz still free and unconnected
    assert!(f.analyze().is_err());
    f.fix(stray, true, true, true);
    let r = f.analyze().unwrap();
    assert_eq!(r.reaction(stray), (0.0, 0.0, 0.0));
}

#[test]
fn invalid_member_data_is_an_error_naming_the_member() {
    let cases = [
        (0.0, A, I, "e"),
        (-E, A, I, "e"),
        (f64::NAN, A, I, "e"),
        (f64::INFINITY, A, I, "e"),
        (E, 0.0, I, "a"),
        (E, -1.0, I, "a"),
        (E, f64::NAN, I, "a"),
        (E, A, 0.0, "iz"),
        (E, A, -1e-4, "iz"),
        (E, A, f64::NAN, "iz"),
    ];
    for (e, a, iz, what) in cases {
        let mut f = cantilever();
        let far = f.add_node(2.0 * L, 0.0);
        f.add_member(1, far, e, a, iz);
        let err = f.analyze().unwrap_err();
        assert!(
            err.contains("member 1")
                && err.contains(&format!("{what} must be finite and positive")),
            "e={e} a={a} iz={iz}: {err}"
        );
    }
}

#[test]
fn coincident_nodes_make_a_zero_length_member_error() {
    let mut f = Frame::new();
    let (a, b) = (f.add_node(1.0, 2.0), f.add_node(1.0, 2.0));
    f.add_member(a, b, E, A, I);
    f.fix(a, true, true, true);
    let err = f.analyze().unwrap_err();
    assert!(
        err.contains("member 0") && err.contains("zero length"),
        "{err}"
    );
}

#[test]
fn non_finite_coordinates_and_loads_are_errors_never_nan_results() {
    let mut f = cantilever();
    f.add_node(f64::NAN, 0.0);
    let err = f.analyze().unwrap_err();
    assert!(
        err.contains("node 2") && err.contains("non-finite"),
        "{err}"
    );

    let mut f = cantilever();
    f.add_node(f64::INFINITY, 0.0);
    assert!(f.analyze().is_err());

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for slot in 0..3 {
            let mut f = cantilever();
            let mut l = [0.0; 3];
            l[slot] = bad;
            f.load(1, l[0], l[1], l[2]);
            let err = f.analyze().unwrap_err();
            assert!(
                err.contains("node 1") && err.contains("not finite"),
                "{err}"
            );
        }
    }

    // +inf and -inf accumulate to NaN, which is still rejected.
    let mut f = cantilever();
    f.load(1, f64::INFINITY, 0.0, 0.0);
    f.load(1, f64::NEG_INFINITY, 0.0, 0.0);
    assert!(f.analyze().is_err());
}

#[test]
fn stiffness_overflow_is_an_error() {
    let mut f = Frame::new();
    let (a, b) = (f.add_node(0.0, 0.0), f.add_node(L, 0.0));
    f.add_member(a, b, 1e308, 1e10, I);
    f.fix(a, true, true, true);
    f.load(b, 0.0, -P, 0.0);
    let err = f.analyze().unwrap_err();
    assert!(err.contains("overflow"), "{err}");
}

// ---- contract: supports, loads, accumulation -----------------------------------

#[test]
fn fully_fixed_structure_carries_the_load_straight_into_the_supports() {
    let mut f = Frame::new();
    let (a, b) = (f.add_node(0.0, 0.0), f.add_node(L, 0.0));
    f.add_member(a, b, E, A, I);
    f.fix(a, true, true, true);
    f.fix(b, true, true, true);
    f.load(a, 5.0, 6.0, 7.0);
    f.load(b, 100.0, -P, 30.0);
    let r = f.analyze().unwrap();
    for node in [a, b] {
        assert_eq!(r.displacement(node), (0.0, 0.0, 0.0));
    }
    assert_eq!(r.reaction(a), (-5.0, -6.0, -7.0));
    assert_eq!(r.reaction(b), (-100.0, P, -30.0));
    assert_eq!(r.member_end_forces(0), [0.0; 6]);
    assert_global_equilibrium(&f, &r);
}

#[test]
fn empty_frame_analyses_to_empty_results() {
    let r = Frame::new().analyze().unwrap();
    assert_eq!((r.node_count(), r.member_count()), (0, 0));
}

#[test]
fn a_fully_fixed_node_without_members_is_fine() {
    let mut f = Frame::new();
    let n = f.add_node(1.0, 1.0);
    f.fix(n, true, true, true);
    f.load(n, 1.0, 2.0, 3.0);
    let r = f.analyze().unwrap();
    assert_eq!(r.reaction(n), (-1.0, -2.0, -3.0));
}

#[test]
fn load_on_a_restrained_degree_of_freedom_is_absorbed_by_the_support() {
    let base = simply_supported().analyze().unwrap();
    let mut f = simply_supported();
    f.load(0, 400.0, -3000.0, 0.0); // ux and uy are both held at the pin
    f.load(2, 0.0, -2000.0, 0.0); // uy is held at the roller
    let r = f.analyze().unwrap();
    // Nothing moves and no member force changes: the supports take the extra loads.
    for node in 0..3 {
        let (got, want) = (r.displacement(node), base.displacement(node));
        near_all(
            &[got.0, got.1, got.2],
            &[want.0, want.1, want.2],
            1e-9,
            1e-15,
        );
    }
    for m in 0..2 {
        near_all(
            &r.member_end_forces(m),
            &base.member_end_forces(m),
            1e-9,
            1e-9 * P,
        );
    }
    // The reaction is reduced by the load, i.e. it grows by the negated load.
    near(r.reaction(0).0, -400.0, 1e-9, 1e-9 * P);
    near(r.reaction(0).1, P / 2.0 + 3000.0, 1e-9, 0.0);
    near(r.reaction(2).1, P / 2.0 + 2000.0, 1e-9, 0.0);
    assert_global_equilibrium(&f, &r);
}

#[test]
fn load_along_a_free_roller_direction_travels_through_the_member_to_the_pin() {
    let mut f = simply_supported();
    f.load(2, -400.0, 0.0, 0.0); // the roller cannot take it
    let r = f.analyze().unwrap();
    near(r.reaction(0).0, 400.0, 1e-9, 0.0);
    assert_eq!(r.reaction(2).0, 0.0);
    assert_statics(&f, &r);
}

#[test]
fn load_accumulates() {
    let mut once = cantilever();
    let mut twice = cantilever();
    // `cantilever` already carries (0, -P, 0); add the same extra load in one
    // call to one frame and in two calls to the other.
    once.load(1, 1000.0, -2000.0, 500.0);
    twice.load(1, 400.0, -500.0, 100.0);
    twice.load(1, 600.0, -1500.0, 400.0);
    let (a, b) = (once.analyze().unwrap(), twice.analyze().unwrap());
    assert_eq!(a, b);
    // Opposite loads cancel.
    let mut f = cantilever();
    f.load(1, 0.0, P, 0.0);
    let r = f.analyze().unwrap();
    assert_eq!(r.displacement(1), (0.0, 0.0, 0.0));
}

#[test]
fn fix_replaces_all_three_flags() {
    let mut f = cantilever();
    assert!(f.analyze().is_ok());
    f.fix(0, true, true, false); // rz released: the root is a pin, the beam turns freely
    assert!(f.analyze().is_err());
    f.fix(0, true, true, true);
    assert!(f.analyze().is_ok());
    f.fix(0, false, false, false); // fully released again
    assert!(f.analyze().is_err());

    // Two single-flag calls leave only the last one in force.
    let mut g = simply_supported();
    g.fix(0, true, false, false);
    g.fix(0, false, true, false);
    assert!(
        g.analyze().is_err(),
        "ux must have been released by the second call"
    );
    g.fix(0, true, true, false);
    assert!(g.analyze().is_ok());
}

#[test]
fn analyze_is_pure_and_repeatable() {
    let f = sloped_frame();
    let first = f.analyze().unwrap();
    assert_eq!(first, f.analyze().unwrap());
    assert_eq!(first, f.clone().analyze().unwrap());
}

#[test]
fn results_do_not_depend_on_the_unit_system() {
    // The same cantilever in N and mm (E in N/mm², A in mm², I in mm⁴).
    let (e, a, i, l) = (200e3, 1e4, 1e8, 3000.0);
    let mut f = Frame::new();
    let (root, tip) = (f.add_node(0.0, 0.0), f.add_node(l, 0.0));
    let m = f.add_member(root, tip, e, a, i);
    f.fix(root, true, true, true);
    f.load(tip, 0.0, -P, 0.0);
    let r = f.analyze().unwrap();
    near(
        r.displacement(tip).1,
        -P * l.powi(3) / (3.0 * e * i),
        1e-9,
        0.0,
    );
    near(r.displacement(tip).1, -4.5, 1e-9, 0.0);
    near(r.reaction(root).2, P * l, 1e-9, 0.0);
    near(r.member_end_forces(m)[2], P * l, 1e-9, 0.0);

    // And with unit properties.
    let mut f = Frame::new();
    let (root, tip) = (f.add_node(0.0, 0.0), f.add_node(1.0, 0.0));
    f.add_member(root, tip, 1.0, 1.0, 1.0);
    f.fix(root, true, true, true);
    f.load(tip, 0.0, -1.0, 0.0);
    near(
        f.analyze().unwrap().displacement(tip).1,
        -1.0 / 3.0,
        1e-12,
        0.0,
    );
}

#[test]
fn indices_are_sequential_and_counts_track_them() {
    let mut f = Frame::new();
    assert_eq!((f.node_count(), f.member_count()), (0, 0));
    let n: Vec<usize> = (0..3).map(|k| f.add_node(k as f64, 0.0)).collect();
    assert_eq!(n, [0, 1, 2]);
    assert_eq!(f.add_member(0, 1, E, A, I), 0);
    assert_eq!(f.add_member(1, 2, E, A, I), 1);
    assert_eq!((f.node_count(), f.member_count()), (3, 2));
    f.fix(0, true, true, true);
    let r = f.analyze().unwrap();
    assert_eq!((r.node_count(), r.member_count()), (3, 2));
}

// ---- programmer errors panic ---------------------------------------------------

#[test]
#[should_panic(expected = "node index 5 out of range")]
fn member_to_a_missing_node_panics() {
    let mut f = Frame::new();
    let a = f.add_node(0.0, 0.0);
    f.add_member(a, 5, E, A, I);
}

#[test]
#[should_panic(expected = "node index 1 out of range")]
fn member_from_a_missing_node_panics() {
    let mut f = Frame::new();
    let a = f.add_node(0.0, 0.0);
    f.add_member(1, a, E, A, I);
}

#[test]
#[should_panic(expected = "two different nodes")]
fn member_from_a_node_to_itself_panics() {
    let mut f = Frame::new();
    let a = f.add_node(0.0, 0.0);
    f.add_member(a, a, E, A, I);
}

#[test]
#[should_panic(expected = "node index 2 out of range")]
fn fix_on_a_missing_node_panics() {
    let mut f = cantilever();
    f.fix(2, true, true, true);
}

#[test]
#[should_panic(expected = "node index 7 out of range")]
fn load_on_a_missing_node_panics() {
    let mut f = cantilever();
    f.load(7, 1.0, 1.0, 1.0);
}

#[test]
#[should_panic(expected = "node index 2 out of range")]
fn displacement_of_a_missing_node_panics() {
    cantilever().analyze().unwrap().displacement(2);
}

#[test]
#[should_panic(expected = "node index 2 out of range")]
fn reaction_of_a_missing_node_panics() {
    cantilever().analyze().unwrap().reaction(2);
}

#[test]
#[should_panic(expected = "member index 1 out of range")]
fn end_forces_of_a_missing_member_panic() {
    cantilever().analyze().unwrap().member_end_forces(1);
}

// ---- properties on random frames -----------------------------------------------

/// Small deterministic generator, so failures reproduce.
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn below(&mut self, n: usize) -> usize {
        ((self.unit() * n as f64) as usize).min(n - 1)
    }
}

/// A connected random frame (spanning tree plus chords, members running in
/// random directions), one fixed node, one pin, random loads.
fn random_frame(rng: &mut Lcg) -> Frame {
    let mut f = Frame::new();
    let n = 3 + rng.below(6);
    for _ in 0..n {
        f.add_node(rng.range(0.0, 12.0), rng.range(0.0, 8.0));
    }
    let member = |f: &mut Frame, rng: &mut Lcg, a: usize, b: usize| {
        let (i, j) = if rng.unit() < 0.5 { (a, b) } else { (b, a) };
        let e = [30e9, 70e9, 200e9][rng.below(3)];
        let area = [1e-3, 1e-2, 5e-2][rng.below(3)];
        let iz = [1e-5, 1e-4, 1e-3][rng.below(3)];
        f.add_member(i, j, e, area, iz);
    };
    for k in 1..n {
        let parent = rng.below(k);
        member(&mut f, rng, parent, k);
    }
    for _ in 0..rng.below(n) {
        let a = rng.below(n);
        let b = (a + 1 + rng.below(n - 1)) % n;
        member(&mut f, rng, a, b);
    }
    f.fix(0, true, true, true);
    if rng.unit() < 0.7 {
        let k = 1 + rng.below(n - 1);
        f.fix(k, true, true, false);
    }
    for _ in 0..1 + rng.below(n) {
        let k = rng.below(n);
        f.load(
            k,
            rng.range(-5e4, 5e4),
            rng.range(-5e4, 5e4),
            rng.range(-2e4, 2e4),
        );
    }
    f
}

#[test]
fn random_frames_satisfy_global_member_and_nodal_equilibrium() {
    let mut rng = Lcg(0x5EED);
    for case in 0..60 {
        let f = random_frame(&mut rng);
        let r = f.analyze().unwrap_or_else(|e| panic!("case {case}: {e}"));
        assert_statics(&f, &r);
    }
}

#[test]
fn random_frames_obey_maxwell_betti_reciprocity() {
    // The displacement of node q in direction d under a unit load at node p in
    // direction c equals the displacement of p in c under a unit load at q in d.
    let mut rng = Lcg(0xB377);
    for _ in 0..20 {
        let base = random_frame(&mut rng);
        let n = base.node_count();
        let (p, q) = (rng.below(n), rng.below(n));
        let (c, d) = (rng.below(3), rng.below(3));
        let response = |load_node: usize, load_dof: usize, read_node: usize, read_dof: usize| {
            let mut f = base.clone();
            f.loads.iter_mut().for_each(|l| *l = [0.0; NDOF]);
            f.loads[load_node][load_dof] = 1.0;
            let (ux, uy, rz) = f.analyze().unwrap().displacement(read_node);
            [ux, uy, rz][read_dof]
        };
        let (a, b) = (response(p, c, q, d), response(q, d, p, c));
        near(a, b, 1e-9, 1e-9 * (a.abs() + b.abs()) + 1e-30);
    }
}

#[test]
fn rotating_the_model_rotates_displacements_and_reactions_and_keeps_end_forces() {
    // Supports here (fixed, pinned) are rotation invariant, so turning every node
    // and load by an angle must turn the global results by the same angle and leave
    // the member-local end forces untouched. A wrong transformation breaks this.
    let mut rng = Lcg(0xA17E);
    for _ in 0..25 {
        let f = random_frame(&mut rng);
        let phi = rng.range(0.0, std::f64::consts::TAU);
        let (c, s) = (phi.cos(), phi.sin());
        let rot = |x: f64, y: f64| (c * x - s * y, s * x + c * y);
        let mut g = f.clone();
        for p in &mut g.nodes {
            let (x, y) = rot(p[0], p[1]);
            *p = [x, y];
        }
        for l in &mut g.loads {
            let (x, y) = rot(l[0], l[1]);
            (l[0], l[1]) = (x, y);
        }
        let (ra, rb) = (f.analyze().unwrap(), g.analyze().unwrap());
        let fscale = force_scale(&f, &ra);
        let uscale = (0..f.node_count())
            .map(|n| ra.displacement(n))
            .fold(0.0f64, |m, (x, y, z)| {
                m.max(x.abs()).max(y.abs()).max(z.abs())
            });
        for m in 0..f.member_count() {
            near_all(
                &rb.member_end_forces(m),
                &ra.member_end_forces(m),
                0.0,
                1e-8 * fscale,
            );
        }
        for n in 0..f.node_count() {
            let (ux, uy, rz) = ra.displacement(n);
            let (vx, vy) = rot(ux, uy);
            let got = rb.displacement(n);
            near_all(&[got.0, got.1, got.2], &[vx, vy, rz], 0.0, 1e-8 * uscale);
            let (fx, fy, mz) = ra.reaction(n);
            let (gx, gy) = rot(fx, fy);
            let got = rb.reaction(n);
            near_all(&[got.0, got.1, got.2], &[gx, gy, mz], 0.0, 1e-8 * fscale);
        }
    }
}
