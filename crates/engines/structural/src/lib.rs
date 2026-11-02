//! `bim-struct`: the structural analysis engine.
//!
//! This first slice is a 2D frame direct-stiffness solver: a [`Frame`] of nodes
//! joined by rigidly connected Euler–Bernoulli beam-column members, supports and
//! nodal loads, solved by [`Frame::analyze`] into [`Results`] (displacements,
//! support reactions and member end forces). The crate has no dependencies.
//!
//! # Conventions
//!
//! * Global x points right, y up; rotation is counter-clockwise positive.
//! * Units are the caller's but must be consistent (N and m, or kN and m, …).
//! * A **reaction** is the force the support exerts on the structure.
//! * **Member end forces** are the forces the nodes exert on the member, in the
//!   member's local axes: x from node `i` to node `j`, y 90° counter-clockwise from
//!   x, so local z is global z. They are ordered `[N1, V1, M1, N2, V2, M2]`.
//!
//! # Contract
//!
//! | Situation | Behaviour |
//! |---|---|
//! | node index out of range in `add_member`, `fix`, `load`, or in a `Results` accessor | panic |
//! | `add_member(i, i, ..)` | panic |
//! | `fix` called again on a node | replaces all three flags; `fix(n, false, false, false)` frees it |
//! | `load` called again on a node | accumulates |
//! | coincident member nodes, `e`/`a`/`iz` not finite and positive, non-finite coordinate or load | `analyze()` returns `Err` naming the member or node |
//! | unstable or singular structure (rigid-body motion, mechanism, free node with no member) | `analyze()` returns `Err` |
//! | load on a restrained degree of freedom | the support absorbs it: the reaction there is reduced by it |
//! | reaction at a degree of freedom that is not restrained | exactly `0.0` |
//! | every degree of freedom fixed (or an empty frame) | `Ok`: zero displacements, reactions equal to the negated loads |
//!
//! Not modelled: member loads, releases (hinges), prescribed displacements,
//! springs, P-Δ effects and shear deformation.
//!
//! # Example
//!
//! A 3 m cantilever with a 10 kN downward tip load:
//!
//! ```
//! use bim_struct::Frame;
//!
//! let (e, a, iz, l, p) = (200e9, 1e-2, 1e-4, 3.0, 10e3);
//! let mut frame = Frame::new();
//! let root = frame.add_node(0.0, 0.0);
//! let tip = frame.add_node(l, 0.0);
//! let beam = frame.add_member(root, tip, e, a, iz);
//! frame.fix(root, true, true, true);
//! frame.load(tip, 0.0, -p, 0.0);
//!
//! let results = frame.analyze().unwrap();
//!
//! // Tip deflection P L³ / 3EI downwards, rotation P L² / 2EI clockwise.
//! let (_, uy, rz) = results.displacement(tip);
//! assert!((uy + p * l.powi(3) / (3.0 * e * iz)).abs() < 1e-12);
//! assert!((rz + p * l.powi(2) / (2.0 * e * iz)).abs() < 1e-12);
//!
//! // The support pushes up with P and resists with a counter-clockwise moment P L.
//! let (_, fy, mz) = results.reaction(root);
//! assert!((fy - p).abs() < 1e-6 && (mz - p * l).abs() < 1e-6);
//!
//! // End forces on the member, local axes: [N1, V1, M1, N2, V2, M2].
//! let ef = results.member_end_forces(beam);
//! assert!((ef[1] - p).abs() < 1e-6 && (ef[2] - p * l).abs() < 1e-6);
//! assert!((ef[4] + p).abs() < 1e-6 && ef[5].abs() < 1e-6);
//!
//! // An unsupported frame is a mechanism, not a result.
//! let mut loose = Frame::new();
//! let (n0, n1) = (loose.add_node(0.0, 0.0), loose.add_node(l, 0.0));
//! loose.add_member(n0, n1, e, a, iz);
//! assert!(loose.analyze().is_err());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod frame2d;
mod linalg;

pub use frame2d::{Frame, Results};
