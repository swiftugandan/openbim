//! `bim-struct`: the structural analysis engine.
//!
//! The first slice is a 2D frame direct-stiffness solver: a [`Frame`] of nodes
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
//!
//! # 3D space frames
//!
//! [`Frame3`] extends the solver to space frames: six degrees of freedom per node
//! (`ux uy uz rx ry rz`), twelve-DOF Timoshenko members described by a
//! [`Section3`] (`E, G, A, Iy, Iz, J` and optional shear areas `Asy, Asz`) and
//! oriented by a reference vector, nodal loads per named load case, and
//! [`LoadCombination`]s. [`Frame3::analyze`] returns [`Results3`] with
//! displacements, reactions and local end forces per combination, plus a
//! per-member [`Envelope`] of internal actions and deflection over all
//! combinations. Local x runs from node `i` to node `j`, local z is the unit
//! component of the reference vector orthogonal to x, and local y = z × x; end
//! forces are ordered `[N1, Vy1, Vz1, T1, My1, Mz1, N2, …, Mz2]`. See
//! [`Frame3`] and [`Results3`] for the full contract.
//!
//! ```
//! use bim_struct::{Frame3, LoadCombination, Section3};
//!
//! let (e, g, l, p) = (200e9, 80e9, 2.0, 5e3);
//! let s = Section3 { e, g, a: 1e-2, iy: 2e-5, iz: 5e-5, j: 1e-5, asy: None, asz: None };
//! let mut frame = Frame3::new();
//! let root = frame.add_node(0.0, 0.0, 0.0);
//! let tip = frame.add_node(l, 0.0, 0.0);
//! let m = frame.add_member(root, tip, s, [0.0, 0.0, 1.0]); // local z = global z
//! frame.fix(root, [true; 6]);
//! frame.load(tip, "dead", [0.0, 0.0, -p, 0.0, 0.0, 0.0]);
//! frame.add_combination(LoadCombination {
//!     name: "ULS".to_string(),
//!     factors: vec![("dead".to_string(), 1.35)],
//! });
//!
//! let r = frame.analyze().unwrap();
//! let uz = r.displacement("ULS", tip)[2];
//! assert!((uz + 1.35 * p * l.powi(3) / (3.0 * e * s.iy)).abs() < 1e-12);
//! // The support pushes up; the hogging root moment is the envelope extreme.
//! assert!((r.reaction("ULS", root)[2] - 1.35 * p).abs() < 1e-6);
//! let env = r.envelope(m);
//! assert!((env.my[1] - 1.35 * p * l).abs() < 1e-6);
//! // Deflection is measured from the chord between the displaced ends.
//! assert!(env.deflection > 0.0 && env.deflection < uz.abs());
//! ```
//!
//! # The `struct` trade engine
//!
//! With the `engine` feature, `StructEngine` implements `bim_plugin::TradeEngine` under the
//! name `struct` (`bim solve <dir> --trade struct`, spec/cli.md): it extracts the analytical
//! stick model from the document's Columns and Beams, distributes AreaLoads to the supporting
//! beams by nearest-beam tributary regions, applies PointLoads, Supports and
//! LoadCombinations, solves with [`Frame3`] and reports nodes, member envelopes with
//! utilization, reactions and tributary areas. The extraction and output rules are documented
//! in the (private) `engine` module of the source.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "engine")]
mod engine;
mod frame2d;
mod frame3d;
mod linalg;
#[cfg(feature = "engine")]
mod span;
#[cfg(feature = "engine")]
mod tributary;

pub use frame2d::{Frame, Results};
#[cfg(feature = "engine")]
pub use engine::StructEngine;
pub use frame3d::{Envelope, Frame3, LoadCombination, Results3, Section3, ENVELOPE_STATIONS};
