//! Clash detection between the elements of a compiled [`Model`] (`bim clash`, spec/cli.md).
//!
//! Every element with a solid takes part (walls, slabs, columns, beams, curtain walls, stairs,
//! ramps, MEP segments, direct-modelling shapes, …), each pair once. Two kinds of clash:
//!
//! * **hard**: the solids share volume. The volume of `A ∩ B` is computed exactly
//!   ([`intersection_volume`]: no tessellation, no tolerance beyond rounding) and is a clash when it
//!   exceeds [`HARD_MIN_VOLUME`] (1 mm³). Solids that merely touch, or an element that lies
//!   wholly inside another (which is then a clash of its whole volume), are told apart exactly.
//!   Only closed meshes have a volume: an open-surface Shape is never in a hard clash.
//! * **soft**: a pair that is not hard and whose surfaces are closer than the `clearance`
//!   ([`mesh_distance`], exact Euclidean distance, strictly less than the clearance). Touching
//!   solids are at distance 0 and so are soft for any clearance above 0. With no clearance (the
//!   default, [`DEFAULT_CLEARANCE`] = 0) there are no soft clashes.
//!
//! Joined walls (an L-join or a T-join, spec/schema.md) are excluded from both lists, whatever
//! their openings. An opening's host needs no exclusion: openings are not solids of their own, they
//! are already cut out of the wall. Nothing else is excluded: two segments meeting at a port, or a
//! slab resting on a wall, are reported like any other touching or overlapping pair.
//!
//! **Using the BVH.** The element boxes go in an R*-tree; a pair is looked at when the boxes are
//! within the clearance of each other. Each element has a [`bim_spatial::Bvh`] over its triangles'
//! boxes (grown by half the clearance), and two of them overlap in the triangle pairs that can be
//! closer than the clearance: those are the only pairs whose distance is computed, and when none
//! exists (and neither element could lie inside the other) the pair has no volume in common.
//!
//! The result lists each pair once, named by the elements' `_name`s with `a` before `b`, both
//! lists sorted by `(a, b)`, and counts the hard clashes by unordered pair of element kinds.

mod distance;
mod volume;

use std::cell::OnceCell;
use std::collections::BTreeMap;

use bim_doc::Document;
use bim_spatial::{Aabb as Box3, Bvh, RStarTree};

use crate::{joined_wall_pairs, Mesh, Model};

pub use distance::mesh_distance;
pub use volume::{intersection_volume, is_convex};

/// The clearance used when none is given: none, so no soft clashes (spec/cli.md gives no default).
pub const DEFAULT_CLEARANCE: f64 = 0.0;

/// Solids sharing at most this volume (m³, 1 mm³) are not in a hard clash.
pub const HARD_MIN_VOLUME: f64 = 1e-9;

/// Two solids that share volume.
#[derive(Debug, Clone, PartialEq)]
pub struct HardClash {
    /// The element whose name sorts first.
    pub a: String,
    /// The other element.
    pub b: String,
    /// The volume they have in common (m³).
    pub volume: f64,
}

/// Two solids that do not overlap but are closer than the clearance.
#[derive(Debug, Clone, PartialEq)]
pub struct SoftClash {
    /// The element whose name sorts first.
    pub a: String,
    /// The other element.
    pub b: String,
    /// The distance between their surfaces (m).
    pub distance: f64,
}

/// The clashes of a model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClashReport {
    /// Hard clashes, sorted by `(a, b)`.
    pub hard: Vec<HardClash>,
    /// Soft clashes, sorted by `(a, b)`.
    pub soft: Vec<SoftClash>,
    /// Hard clashes counted by unordered pair of element kinds: `matrix[k1][k2]` with `k1 ≤ k2`;
    /// only non-zero counts are present.
    pub matrix: BTreeMap<String, BTreeMap<String, u64>>,
}

/// An element's mesh with what the pair tests need.
struct Prepared<'a> {
    mesh: &'a Mesh,
    bounds: Box3,
    closed: bool,
    tris: Vec<distance::Tri>,
    bvh: Bvh,
}

impl<'a> Prepared<'a> {
    fn new(mesh: &'a Mesh, bounds: Box3, margin: f64) -> Prepared<'a> {
        let tris = distance::triangles(mesh);
        let bvh = distance::grown_bvh(&tris, margin);
        Prepared { mesh, bounds, closed: mesh.is_closed(), tris, bvh }
    }

    /// Whether some vertex lies in `b` (closed).
    fn has_vertex_in(&self, b: &Box3) -> bool {
        self.mesh.positions.iter().any(|p| b.intersects(&Box3::from_point([p.x, p.y, p.z])))
    }
}

/// The box of a mesh with triangles and finite coordinates.
fn mesh_box(mesh: &Mesh) -> Option<Box3> {
    if mesh.triangles.is_empty() {
        return None;
    }
    let (lo, hi) = mesh.bounds()?;
    let b = Box3::new([lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z]);
    (b.min.iter().chain(&b.max).all(|x| x.is_finite())).then_some(b)
}

/// The volume shared by the elements `a` and `b`, 0 unless both are closed and could overlap.
/// `pairs` are the candidate triangle pairs of their hierarchies.
fn shared_volume(a: &Prepared, b: &Prepared, pairs: &[(usize, usize)]) -> f64 {
    if !a.closed || !b.closed || a.bounds.overlap_volume(&b.bounds) <= 0.0 {
        return 0.0;
    }
    // Without any pair of surfaces within reach, solids overlap only if one is inside the other,
    // and then some vertex of it is in the other's box (a part of a multi-part solid included).
    if pairs.is_empty() && !a.has_vertex_in(&b.bounds) && !b.has_vertex_in(&a.bounds) {
        return 0.0;
    }
    intersection_volume(a.mesh, b.mesh)
}

/// The hard and soft clashes of the elements of `model` (compiled from `doc`) at `clearance` metres
/// (see the module docs); a clearance that is not a positive number means none.
pub fn clash(doc: &Document, model: &Model, clearance: f64) -> ClashReport {
    let clearance = if clearance.is_finite() && clearance > 0.0 { clearance } else { 0.0 };
    let elements = &model.elements;
    let joined = joined_wall_pairs(doc);
    let bounds: Vec<Option<Box3>> = elements.iter().map(|e| mesh_box(&e.mesh)).collect();
    let tree = RStarTree::bulk_load(bounds.iter().enumerate().filter_map(|(i, b)| b.map(|b| (b, i))).collect());
    let prepared: Vec<OnceCell<Prepared>> = elements.iter().map(|_| OnceCell::new()).collect();
    let prepare = |i: usize| {
        prepared[i].get_or_init(|| Prepared::new(&elements[i].mesh, bounds[i].expect("boxed"), clearance / 2.0))
    };

    let mut report = ClashReport::default();
    for (i, bi) in bounds.iter().enumerate() {
        let Some(bi) = bi else { continue };
        let reach = Box3::new(bi.min.map(|x| x - clearance), bi.max.map(|x| x + clearance));
        let mut others: Vec<usize> = tree.query_intersects(&reach).into_iter().copied().filter(|&j| j > i).collect();
        others.sort_unstable();
        for j in others {
            let (ea, eb) = (&elements[i], &elements[j]);
            if joined.contains(&(ea.id.min(eb.id), ea.id.max(eb.id))) {
                continue;
            }
            let (pa, pb) = (prepare(i), prepare(j));
            let pairs = pa.bvh.overlaps(&pb.bvh);
            let (first, second) = if ea.name <= eb.name { (ea, eb) } else { (eb, ea) };
            let volume = shared_volume(pa, pb, &pairs);
            if volume > HARD_MIN_VOLUME {
                report.hard.push(HardClash { a: first.name.clone(), b: second.name.clone(), volume });
                let (k1, k2) = if first.kind <= second.kind { (&first.kind, &second.kind) } else { (&second.kind, &first.kind) };
                *report.matrix.entry(k1.clone()).or_default().entry(k2.clone()).or_default() += 1;
            } else if clearance > 0.0 {
                let distance = distance::min_distance(&pa.tris, &pb.tris, &pairs);
                if distance < clearance {
                    report.soft.push(SoftClash { a: first.name.clone(), b: second.name.clone(), distance });
                }
            }
        }
    }
    report.hard.sort_by(|x, y| x.a.cmp(&y.a).then_with(|| x.b.cmp(&y.b)));
    report.soft.sort_by(|x, y| x.a.cmp(&y.a).then_with(|| x.b.cmp(&y.b)));
    report
}
