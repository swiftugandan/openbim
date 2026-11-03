//! Planar sections of solids.
//!
//! [`section`] intersects a [`Solid`] with a plane and returns the cut as polygons with holes
//! lying on that plane; [`section_area`] is the area they enclose.
//!
//! # Semantics
//!
//! The section is the closure of the set of points of the plane that lie inside the solid, plus
//! every face of the solid that is coplanar with the plane. Equivalently it is the union of the
//! two one-sided limits `lim ε→0 section(offset − ε) ∪ section(offset + ε)`. A plane that meets the
//! solid only in edges or vertices yields no polygons.
//!
//! # Algorithm
//!
//! With `d(p) = n̂·p − offset/|n|`, every vertex is classified once as below, above or on the
//! plane (within a tolerance relative to the solid's size). The cut is assembled as
//!
//! * `R⁻`, the limit of the section by the plane moved by `−ε` along the normal. This is a
//!   symbolic perturbation: on-plane vertices are treated as lying above, so no face has a vertex
//!   exactly on the perturbed plane. Each non-coplanar face then crosses it in proper intervals:
//!   the crossing points of all its loops are sorted along `n̂ × m` (`m` the outward face normal)
//!   and paired, each interval being directed along `n̂ × m` so that the material lies on its left.
//!   A crossing on an edge with an on-plane endpoint is that vertex itself; other crossings are
//!   interpolated once per (undirected) edge so both adjacent faces share bit-identical points.
//! * the coplanar faces whose outward normal opposes `n̂` (material on the `+n̂` side, so they are
//!   not part of `R⁻`), with their loops reversed. Coplanar faces facing `+n̂` are already in `R⁻`.
//!
//! For a closed manifold these two parts have disjoint interiors, so the winding number of the
//! directed segments is 0 or 1 everywhere. In the plane's 2D frame the segments are snapped
//! together, split at T-junctions and crossings, and opposite copies of an edge cancel; the edges
//! left over are exactly the region boundary with the region on the left. They are walked into
//! simple loops (a loop that revisits a vertex is split there, which separates regions that touch
//! at a point and holes that touch their outer loop), counter-clockwise loops become outer
//! boundaries and clockwise loops become holes of the smallest outer loop containing them.

use std::collections::HashMap;

use crate::region2d::{positive_regions, PointSet, Region2, P2};
use crate::solid::Solid;
use crate::types::{Point3, Vec3};

/// The plane `normal · p = offset`. The normal need not be a unit vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane3 {
    pub normal: Vec3,
    pub offset: f64,
}

/// One region of a section: an outer loop counter-clockwise about the plane normal and holes
/// clockwise about it. Loops are not closed (the first vertex is not repeated).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polygon3 {
    pub outer: Vec<Point3>,
    pub holes: Vec<Vec<Point3>>,
}

impl Polygon3 {
    /// Area enclosed by the outer loop minus the holes.
    pub fn area(&self, normal: Vec3) -> f64 {
        let n = match normal.normalized() {
            Some(n) => n,
            None => return 0.0,
        };
        // Relative to a vertex of the outer loop, so far-from-origin polygons keep their precision.
        let Some(&o) = self.outer.first() else {
            return 0.0;
        };
        let a = |r: &[Point3]| -> f64 {
            let mut s = Vec3::ZERO;
            for i in 0..r.len() {
                s += (r[i] - o).cross(r[(i + 1) % r.len()] - o);
            }
            0.5 * s.dot(n)
        };
        self.holes.iter().fold(a(&self.outer), |s, h| s + a(h))
    }
}

/// The section of `solid` by `plane`: closed regions on the plane, outer loops counter-clockwise
/// about the plane normal, holes clockwise and nested inside their outer loop. Coplanar faces are
/// part of the section; contact in edges or vertices only gives an empty result.
pub fn section(solid: &Solid, plane: &Plane3) -> Vec<Polygon3> {
    let Some(sec) = Section2::compute(solid, plane) else {
        return Vec::new();
    };
    sec.regions
        .iter()
        .map(|r| Polygon3 {
            outer: r.outer.iter().map(|&p| sec.lift(p)).collect(),
            holes: r.holes.iter().map(|h| h.iter().map(|&p| sec.lift(p)).collect()).collect(),
        })
        .collect()
}

/// Total area of [`section`]`(solid, plane)`.
pub fn section_area(solid: &Solid, plane: &Plane3) -> f64 {
    match Section2::compute(solid, plane) {
        Some(sec) => sec.regions.iter().fold(0.0, |s, r| s + r.area),
        None => 0.0,
    }
}


/// A section expressed in an orthonormal frame `(u, v)` of the plane with `u × v = n̂`.
struct Section2 {
    u: Vec3,
    v: Vec3,
    origin: Vec3,
    regions: Vec<Region2>,
}

impl Section2 {
    fn lift(&self, p: P2) -> Point3 {
        (self.origin + self.u * p[0] + self.v * p[1]).to_point()
    }

    fn compute(solid: &Solid, plane: &Plane3) -> Option<Section2> {
        let len = plane.normal.norm();
        if !(len > 0.0 && len.is_finite() && plane.offset.is_finite()) {
            return None;
        }
        let n = plane.normal / len;
        let off = plane.offset / len;
        let a = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let u = a.cross(n).normalized()?;
        let v = n.cross(u);

        // Tolerances follow the solid's own extent, not the magnitude of its coordinates: the
        // length tolerance is a fixed fraction of the bounding-box diagonal `extent`, floored at a
        // few ulps of the largest coordinate (the rounding level of every computed point), and
        // the 2D work is done in a frame centred near the solid so coordinates there are O(extent).
        let inf = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let (mut lo, mut hi) = (inf, -inf);
        for vx in &solid.vertices {
            let p = vx.point.to_vec();
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        let extent = (hi - lo).norm();
        if !(extent > 0.0 && extent.is_finite()) {
            return None;
        }
        let mag = lo.x.abs().max(lo.y.abs()).max(lo.z.abs()).max(hi.x.abs()).max(hi.y.abs()).max(hi.z.abs());
        let eps = (1e-9 * extent).max(64.0 * f64::EPSILON * mag);
        // Frame origin: the bounding-box centre projected onto the plane.
        let centre = (lo + hi) * 0.5;
        let origin = centre - n * (centre.dot(n) - off);
        let mut sec = Section2 { u, v, origin, regions: Vec::new() };

        // Signed distances and classification (on-plane vertices count as above: the `−ε` limit).
        let dist: Vec<f64> = solid.vertices.iter().map(|vx| (vx.point.to_vec() - origin).dot(n)).collect();
        let on: Vec<bool> = dist.iter().map(|d| d.abs() <= eps).collect();
        let above = |i: usize| on[i] || dist[i] > 0.0;

        let mut pts = PointSet::new(eps);
        let project = |p: Point3| -> P2 {
            let d = p.to_vec() - origin;
            [d.dot(u), d.dot(v)]
        };
        let mut segs: Vec<(usize, usize)> = Vec::new();
        let mut edge_pts: HashMap<(usize, usize), (Point3, usize)> = HashMap::new();

        for f in 0..solid.faces.len() {
            let loops: Vec<Vec<usize>> = solid.faces[f].loops.iter().map(|&h| solid.loop_vertices(h)).collect();
            if loops.iter().all(|l| l.iter().all(|&i| on[i])) {
                // Coplanar face: only the ones with material on the +n side are added here.
                if solid.face_normal(f).dot(n) < 0.0 {
                    for l in &loops {
                        for k in 0..l.len() {
                            let a = pts.insert(project(solid.vertices[l[k]].point));
                            let b = pts.insert(project(solid.vertices[l[(k + 1) % l.len()]].point));
                            segs.push((b, a));
                        }
                    }
                }
                continue;
            }
            let t = n.cross(solid.face_normal(f));
            let mut cross: Vec<(f64, usize)> = Vec::new();
            for l in &loops {
                for k in 0..l.len() {
                    let (ia, ib) = (l[k], l[(k + 1) % l.len()]);
                    if above(ia) == above(ib) {
                        continue;
                    }
                    let key = (ia.min(ib), ia.max(ib));
                    let (p, id) = *edge_pts.entry(key).or_insert_with(|| {
                        let (lo, hi) = key;
                        let p = if on[lo] {
                            solid.vertices[lo].point
                        } else if on[hi] {
                            solid.vertices[hi].point
                        } else {
                            let (pa, pb) = (solid.vertices[lo].point, solid.vertices[hi].point);
                            let s = dist[lo] / (dist[lo] - dist[hi]);
                            pa + (pb - pa) * s
                        };
                        (p, pts.insert(project(p)))
                    });
                    cross.push(((p.to_vec() - origin).dot(t), id));
                }
            }
            cross.sort_by(|a, b| a.0.total_cmp(&b.0));
            for pair in cross.chunks_exact(2) {
                segs.push((pair[0].1, pair[1].1));
            }
        }

        sec.regions = positive_regions(pts, segs);
        Some(sec)
    }
}
