//! Spatial indexing for BIM geometry.
//!
//! * [`Aabb`]: closed axis-aligned bounding boxes (touching boxes intersect).
//! * [`RStarTree`]: a dynamic R*-tree (ChooseSubtree with overlap enlargement, forced
//!   reinsert once per level per insertion, margin/overlap driven split), STR bulk
//!   loading, removal with tree condensation, box queries and exact k-nearest search.
//! * [`Bvh`]: a bounding volume hierarchy over triangles built with a binned surface
//!   area heuristic, with closest-hit raycasting (Möller–Trumbore, two-sided) and
//!   simultaneous-traversal overlap queries between two hierarchies.

mod aabb;
mod bvh;
mod rstar;

pub use aabb::Aabb;
pub use bvh::{Bvh, Triangle};
pub use rstar::RStarTree;
