//! `bim-geom`: the geometric kernel of OpenBIM.
//!
//! * [`Point2`], [`Point3`], [`Vec3`] — plain `f64` value types.
//! * [`orient2d`], [`orient3d`], [`incircle`] — robust geometric predicates (Shewchuk
//!   conventions). A floating-point filter answers the easy cases; everything else is decided
//!   with exact floating-point expansion arithmetic, so the *sign* of the result is always
//!   correct for coordinates in `{0} ∪ [2^-100, 2^100]` (see [`predicates`] for the argument).
//! * [`Polygon`] — a simple polygon with holes: area, point containment and ear-clipping
//!   triangulation (holes are bridged into the outer ring, no points are added).
//! * [`Solid`] — a closed half-edge boundary representation with planar polygonal faces,
//!   built by [`Solid::extrude`] and [`Solid::sweep`], with volume / area / centroid,
//!   manifold and Euler checks and ASCII STL export.

//! * [`boolean`] — union, difference and intersection of closed solids with exact-predicate
//!   classification; results are closed 2-manifolds with merged coplanar faces, or empty.

pub(crate) mod big;
pub mod boolean;
pub(crate) mod exact;
pub mod polygon;
pub mod predicates;
pub mod solid;
pub mod types;

pub use boolean::{boolean, BoolOp, GeomError};
pub use polygon::Polygon;
pub use predicates::{incircle, orient2d, orient3d};
pub use solid::{Face, HalfEdge, Solid, Vertex};
pub use types::{Point2, Point3, Vec3};
