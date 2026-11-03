//! Hidden-line removal for orthographic views of polyhedral scenes.
//!
//! [`hlr`] takes solids with pairwise disjoint interiors (they may touch) and an [`OrthoView`]
//! and returns every feature edge of the scene, projected onto the view plane, cut into the
//! pieces that are uniformly visible or uniformly hidden.
//!
//! # Semantics
//!
//! * **Edges.** An edge of a solid is a *feature edge* when the dihedral angle between its two
//!   faces exceeds `1e-9` radians (within that solid; a boundary edge of an open mesh also
//!   counts). Silhouette edges of a polyhedron are always feature edges, so they need no separate
//!   treatment. Edges of coplanar neighbouring faces are neither drawn nor used as split points,
//!   but the faces still occlude as one planar region.
//! * **Merging.** Feature edges of all solids are grouped by their 3D supporting line and the
//!   overlapping parameter intervals on each line are united, so an edge shared by two touching
//!   solids (or two collinear edges that overlap) is reported once. Each output piece carries the
//!   index of a solid whose edge covers it.
//! * **Projection.** With `d = normalize(dir)`, `r = normalize(dir × up)` and `v = r × d`, a
//!   point projects to `(p·r, p·v)`; `p·d` is its depth (`dir` points from the eye into the
//!   scene). Edges parallel to `dir` project to points and are not reported, but the points still
//!   split the edges they land on.
//! * **Splitting.** The projected edges are cut at every proper crossing of two projections, at
//!   every endpoint of a projection lying on another one (T-junctions, overlapping collinear
//!   projections) and at the endpoints of the merged edges. Visibility can only change at such
//!   points, so each piece is classified at a single interior point and cut points closer than
//!   about `3e-7` of the scene size are merged, so no sliver pieces are produced.
//! * **Visibility.** A point `p` of an edge is *hidden* iff the open ray from `p` towards the eye
//!   (direction `-d`) passes through the *interior* of some solid for a positive length. Touching
//!   a solid in a face, edge or vertex, or running along a face or an edge, is not occlusion; a
//!   solid's own faces through `p` never occlude it, but a solid does hide its own back edges.
//!
//! # Algorithm
//!
//! Each piece is classified at its midpoint `p` by examining the ray `p + t·w` (`w = -d`, `t >
//! 0`) solid by solid, as a sequence of open intervals between the depths at which the ray can
//! meet the solid's boundary. No incidence case analysis is involved: a ray passing exactly
//! through a vertex (even a straight, 180 degree one left by a boolean operation or by a
//! collinear split), along an edge or inside a face plane is handled like any other.
//!
//! 1. *Candidates.* A uniform grid over the projected bounding boxes of all faces (edge-on ones
//!    included) gives the faces whose box, padded by `3·eps`, contains the projection `q` of `p`
//!    (`eps` is `1e-9` of the scene's bounding-box diagonal). Every point of a solid on the ray
//!    lies on such a face, so the candidate solids are the distinct solids of those faces.
//! 2. *Events.* For a candidate solid, the depth of every point where the ray can touch its
//!    boundary: the plane crossing of each face not seen edge-on whose closed projected polygon
//!    contains `q` (inside, on an edge or on a vertex, with the exact [`orient2d`] predicate and
//!    an `eps` band around the boundary), the point over `q` of each edge whose projection passes
//!    within `eps` of `q`, and each vertex projecting within `eps` of `q`. Extra events are
//!    harmless: they only split an interval into two with the same answer.
//! 3. *Intervals.* The events are sorted and merged with their predecessor (starting from depth
//!    `0`) when closer than `eps`; between two consecutive merged events the ray is uniformly
//!    inside, outside or on the boundary of the solid, so each such interval is decided at its
//!    midpoint `m`. Beyond the last event the ray is outside.
//! 4. *Boundary.* If `m` lies within `3·eps` of the plane of a candidate face and inside that
//!    closed face (within `3·eps` of a loop edge, or inside by the even-odd rule in the face
//!    plane), the ray runs along the boundary there: not occlusion.
//! 5. *Interior.* Otherwise the interval is interior iff the generalised winding number of the
//!    solid at `m` exceeds `1/2`: the signed solid angle of every face loop (outer loops
//!    counter-clockwise and inner loops clockwise about the outward normal, so holes subtract),
//!    summed over a triangle fan of each loop. Faces whose plane passes within `eps` of `m` are
//!    left out: by step 4, `m` is then at least `2.8·eps` from the face polygon in its plane, where
//!    the face's exact contribution is at most `0.06`, and the fan formula is ill-conditioned for
//!    (nearly) coplanar points.
//!
//! The piece is hidden iff some interval of some candidate solid is interior. The decision is
//! exact for the geometry (no sampling along the edges, no depth sorting of faces) up to the
//! tolerance `eps` used to decide incidences; intervals shorter than `eps` are never interior.

use crate::predicates::orient2d;
use crate::solid::Solid;
use crate::types::{Point2, Point3, Vec3};

/// An orthographic view. `dir` points from the eye into the scene and `up` is the hint for the
/// vertical axis of the view plane; neither needs to be a unit vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrthoView {
    pub dir: Vec3,
    pub up: Vec3,
}

/// A projected edge piece, attributed to the solid whose edge it comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg2 {
    pub a: Point2,
    pub b: Point2,
    pub solid: usize,
}

/// The result of [`hlr`]: the visible and the hidden pieces of the scene's feature edges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HlrResult {
    pub visible: Vec<Seg2>,
    pub hidden: Vec<Seg2>,
}

impl OrthoView {
    pub const fn new(dir: Vec3, up: Vec3) -> Self {
        OrthoView { dir, up }
    }

    /// The orthonormal view frame `[d, r, v]` with `d = normalize(dir)`, `r = normalize(dir × up)`
    /// and `v = r × d`, or `None` when `dir` is zero, parallel to `up`, or not finite.
    pub fn frame(&self) -> Option<[Vec3; 3]> {
        let d = self.dir.normalized()?;
        let r = self.dir.cross(self.up).normalized()?;
        let v = r.cross(d);
        if !(d.x.is_finite() && v.x.is_finite() && v.y.is_finite() && v.z.is_finite()) {
            return None;
        }
        Some([d, r, v])
    }

    /// The view-plane coordinates `(p·r, p·v)` of a point, or `None` for a degenerate view.
    pub fn project(&self, p: Point3) -> Option<Point2> {
        let [_, r, v] = self.frame()?;
        Some(Point2::new(p.to_vec().dot(r), p.to_vec().dot(v)))
    }
}

/// Hidden-line removal of `solids` seen along `view` (see the [module documentation](self)).
/// The solids must have pairwise disjoint interiors. A degenerate view or a scene without
/// extent gives an empty result.
pub fn hlr(solids: &[Solid], view: &OrthoView) -> HlrResult {
    let mut out = HlrResult::default();
    let Some([d, r, v]) = view.frame() else {
        return out;
    };
    let w = -d;

    // ---------------------------------------------------------------- scene tolerances
    let inf = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let (mut lo, mut hi) = (inf, -inf);
    for s in solids {
        for vx in &s.vertices {
            let p = vx.point.to_vec();
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
    }
    let extent = (hi - lo).norm();
    if !(extent > 0.0 && extent.is_finite()) {
        return out;
    }
    let mag = lo.x.abs().max(lo.y.abs()).max(lo.z.abs()).max(hi.x.abs()).max(hi.y.abs()).max(hi.z.abs());
    let eps = (1e-9 * extent).max(64.0 * f64::EPSILON * mag);
    // Faces whose projected box lies within this band of a midpoint's projection are candidates
    // for touching its ray; it is also the thickness of the boundary for the interval test.
    let band = 3.0 * eps;
    // Cut points closer than this (in the view plane) are merged: no sliver pieces.
    let sliver = 3e-7 * extent;
    let centre = ((lo + hi) * 0.5).to_point();
    let proj = |p: Point3| Point2::new(p.to_vec().dot(r), p.to_vec().dot(v));

    // ---------------------------------------------------------------- feature edges
    let infos: Vec<SolidInfo> = solids.iter().map(analyse).collect();
    let mut edges: Vec<(usize, Point3, Point3)> = Vec::new();
    for (k, s) in solids.iter().enumerate() {
        for (h, he) in s.half_edges.iter().enumerate() {
            if matches!(he.twin, Some(t) if t < h) || !infos[k].feature[h] {
                continue;
            }
            let a = s.vertices[he.origin].point;
            let b = s.vertices[s.half_edges[he.next].origin].point;
            let len = (b - a).norm();
            if len > eps && len.is_finite() {
                edges.push((k, a, b));
            }
        }
    }

    // ---------------------------------------------------------------- lines
    let lines = group_lines(&edges, centre, eps, extent);

    // 2D pieces of every line for the crossing search.
    let mut items: Vec<Item> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let p2 = proj(line.o);
        let d2 = Point2::new(line.t.dot(r), line.t.dot(v));
        let nt = (d2.x * d2.x + d2.y * d2.y).sqrt();
        for &(u0, u1) in &line.pieces {
            let a = Point2::new(p2.x + u0 * d2.x, p2.y + u0 * d2.y);
            let b = Point2::new(p2.x + u1 * d2.x, p2.y + u1 * d2.y);
            let len2 = nt * (u1 - u0);
            let point = !(len2 > eps);
            let dir = if point { Point2::new(0.0, 0.0) } else { Point2::new(d2.x / nt, d2.y / nt) };
            items.push(Item {
                line: li,
                a,
                b,
                dir,
                nt,
                u0,
                u1,
                len2,
                point,
                bb: [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)],
            });
        }
    }

    // ---------------------------------------------------------------- crossings
    let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); lines.len()];
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&i, &j| items[i].bb[0].total_cmp(&items[j].bb[0]));
    for (oi, &i) in order.iter().enumerate() {
        let a = &items[i];
        for &j in &order[oi + 1..] {
            let b = &items[j];
            if b.bb[0] > a.bb[2] + eps {
                break;
            }
            if a.line == b.line || b.bb[1] > a.bb[3] + eps || a.bb[1] > b.bb[3] + eps {
                continue;
            }
            split_pair(a, b, eps, &mut cuts);
        }
    }

    // ---------------------------------------------------------------- faces
    let mut faces: Vec<FaceRec> = Vec::new();
    for (k, s) in solids.iter().enumerate() {
        for (f, face) in s.faces.iter().enumerate() {
            let Some(n) = infos[k].normals[f] else {
                continue;
            };
            let nw = n.dot(w);
            let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            let loops: Vec<Vec<LoopPt>> = face
                .loops
                .iter()
                .map(|&start| {
                    let mut lp = Vec::new();
                    let mut h = start;
                    loop {
                        let p = s.vertices[s.half_edges[h].origin].point;
                        let q = proj(p);
                        bb = [bb[0].min(q.x), bb[1].min(q.y), bb[2].max(q.x), bb[3].max(q.y)];
                        lp.push(LoopPt { p, q });
                        h = s.half_edges[h].next;
                        if h == start || lp.len() > s.half_edges.len() {
                            break;
                        }
                    }
                    lp
                })
                .collect();
            if loops.iter().all(|l| l.len() < 3) || !bb.iter().all(|x| x.is_finite()) {
                continue;
            }
            let p0 = s.vertices[s.half_edges[face.loops[0]].origin].point;
            // A face seen edge-on has no projected area: it yields no plane-crossing events, but
            // its edges and vertices do, and the ray can run along it.
            faces.push(FaceRec { solid: k, n, nw, edge_on: nw.abs() <= 1e-12, p0, loops, bb });
        }
    }
    let grid = Grid::new(&faces, band);
    let ctx = Ctx { solids, infos: &infos, faces: &faces, grid: &grid, w, eps, band };

    // ---------------------------------------------------------------- pieces
    for item in &items {
        if item.point {
            continue;
        }
        let line = &lines[item.line];
        let (u0, u1) = (item.u0, item.u1);
        let tol = sliver / item.nt;
        let mut cs: Vec<f64> = cuts[item.line]
            .iter()
            .copied()
            .chain(line.members.iter().flat_map(|m| [m.1, m.2]))
            .filter(|c| *c > u0 && *c < u1)
            .collect();
        cs.sort_by(|a, b| a.total_cmp(b));
        let mut kept = vec![u0];
        for c in cs {
            if c - kept[kept.len() - 1] > tol && u1 - c > tol {
                kept.push(c);
            }
        }
        kept.push(u1);
        for pair in kept.windows(2) {
            let (c0, c1) = (pair[0], pair[1]);
            let mid = 0.5 * (c0 + c1);
            let p = line.o + line.t * mid;
            let owner = line
                .members
                .iter()
                .map(|m| (m.2.min(c1) - m.1.max(c0), m.0))
                .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))
                .map(|m| m.1)
                .unwrap_or(0);
            let seg = Seg2 { a: proj(line.o + line.t * c0), b: proj(line.o + line.t * c1), solid: owner };
            if ctx.hidden(p, proj(p)) {
                out.hidden.push(seg);
            } else {
                out.visible.push(seg);
            }
        }
    }
    out
}

// -------------------------------------------------------------------------------------------
// per-solid analysis

struct SolidInfo {
    /// Unit outward normal of each face (`None` for a degenerate face).
    normals: Vec<Option<Vec3>>,
    /// Whether each half-edge lies on a feature edge.
    feature: Vec<bool>,
}

fn analyse(s: &Solid) -> SolidInfo {
    let normals: Vec<Option<Vec3>> = (0..s.faces.len()).map(|f| s.face_normal(f).normalized()).collect();
    let feature = s
        .half_edges
        .iter()
        .map(|he| match he.twin {
            None => true,
            Some(t) => match (normals[he.face], normals.get(s.half_edges[t].face).copied().flatten()) {
                (Some(n1), Some(n2)) => n1.cross(n2).norm().atan2(n1.dot(n2)) > 1e-9,
                _ => true,
            },
        })
        .collect();
    SolidInfo { normals, feature }
}

// -------------------------------------------------------------------------------------------
// lines

/// A supporting line of one or more collinear feature edges.
struct Line {
    /// The point of the line closest to the scene centre.
    o: Point3,
    /// Unit direction.
    t: Vec3,
    /// Sort key for the grouping scan.
    key: f64,
    /// `(solid, s0, s1)` parameter intervals of the member edges.
    members: Vec<(usize, f64, f64)>,
    /// The union of the member intervals.
    pieces: Vec<(f64, f64)>,
}

fn group_lines(edges: &[(usize, Point3, Point3)], centre: Point3, eps: f64, extent: f64) -> Vec<Line> {
    const ANGLE: f64 = 1e-7;
    struct Cand {
        o: Point3,
        t: Vec3,
        key: f64,
    }
    let cands: Vec<Cand> = edges
        .iter()
        .map(|&(_, a, b)| {
            let t = (b - a).normalized().unwrap_or(Vec3::X);
            let o = a + t * (centre - a).dot(t);
            Cand { o, t, key: o.x + o.y + o.z }
        })
        .collect();
    let mut order: Vec<usize> = (0..edges.len()).collect();
    order.sort_by(|&i, &j| cands[i].key.total_cmp(&cands[j].key));
    let window = 3.0 * (eps + ANGLE * extent) + eps;
    let mut lines: Vec<Line> = Vec::new();
    for &i in &order {
        let (k, a, b) = edges[i];
        let c = &cands[i];
        let dist = |line: &Line, p: Point3| (p - line.o).cross(line.t).norm();
        let mut found = None;
        for j in (0..lines.len()).rev() {
            if lines[j].key < c.key - window {
                break;
            }
            let l = &lines[j];
            if c.t.cross(l.t).norm() <= ANGLE && dist(l, a) <= eps && dist(l, b) <= eps {
                found = Some(j);
                break;
            }
        }
        let j = match found {
            Some(j) => j,
            None => {
                lines.push(Line { o: c.o, t: c.t, key: c.key, members: Vec::new(), pieces: Vec::new() });
                lines.len() - 1
            }
        };
        let l = &mut lines[j];
        let s0 = (a - l.o).dot(l.t);
        let s1 = (b - l.o).dot(l.t);
        l.members.push((k, s0.min(s1), s0.max(s1)));
    }
    for l in &mut lines {
        l.members.sort_by(|a, b| a.1.total_cmp(&b.1));
        for &(_, s0, s1) in &l.members {
            match l.pieces.last_mut() {
                Some(last) if s0 <= last.1 + eps => last.1 = last.1.max(s1),
                _ => l.pieces.push((s0, s1)),
            }
        }
    }
    lines
}

// -------------------------------------------------------------------------------------------
// crossings

/// One union piece of a line, projected.
struct Item {
    line: usize,
    a: Point2,
    b: Point2,
    /// Unit 2D direction (zero for a point).
    dir: Point2,
    /// Projected length of the unit 3D direction.
    nt: f64,
    u0: f64,
    u1: f64,
    len2: f64,
    /// Projects to a point (an edge seen end-on).
    point: bool,
    bb: [f64; 4],
}

fn cross2(a: Point2, b: Point2) -> f64 {
    a.x * b.y - a.y * b.x
}

fn dot2(a: Point2, b: Point2) -> f64 {
    a.x * b.x + a.y * b.y
}

/// Adds a cut to `a`'s line where the point `q` lies on `a` (within `eps`).
fn endpoint_on(a: &Item, q: Point2, eps: f64, cuts: &mut [Vec<f64>]) {
    let e = q - a.a;
    if cross2(a.dir, e).abs() <= eps {
        let tau = dot2(e, a.dir);
        if tau >= -eps && tau <= a.len2 + eps {
            cuts[a.line].push(a.u0 + tau / a.nt);
        }
    }
}

fn split_pair(a: &Item, b: &Item, eps: f64, cuts: &mut [Vec<f64>]) {
    match (a.point, b.point) {
        (true, true) => {}
        (true, false) => endpoint_on(b, a.a, eps, cuts),
        (false, true) => endpoint_on(a, b.a, eps, cuts),
        (false, false) => {
            endpoint_on(a, b.a, eps, cuts);
            endpoint_on(a, b.b, eps, cuts);
            endpoint_on(b, a.a, eps, cuts);
            endpoint_on(b, a.b, eps, cuts);
            let o1 = orient2d(a.a, a.b, b.a);
            let o2 = orient2d(a.a, a.b, b.b);
            let o3 = orient2d(b.a, b.b, a.a);
            let o4 = orient2d(b.a, b.b, a.b);
            let proper = |x: f64, y: f64| (x > 0.0 && y < 0.0) || (x < 0.0 && y > 0.0);
            if proper(o1, o2) && proper(o3, o4) {
                let da = a.b - a.a;
                let db = b.b - b.a;
                let e = b.a - a.a;
                let den = cross2(da, db);
                if den != 0.0 && den.is_finite() {
                    let alpha = (cross2(e, db) / den).clamp(0.0, 1.0);
                    let beta = (cross2(e, da) / den).clamp(0.0, 1.0);
                    cuts[a.line].push(a.u0 + alpha * (a.u1 - a.u0));
                    cuts[b.line].push(b.u0 + beta * (b.u1 - b.u0));
                }
            }
        }
    }
}

// -------------------------------------------------------------------------------------------
// classification

/// A loop vertex of a face, in 3D and projected.
#[derive(Clone, Copy)]
struct LoopPt {
    p: Point3,
    q: Point2,
}

struct FaceRec {
    solid: usize,
    /// Unit outward normal.
    n: Vec3,
    /// `n · w` (`w` towards the eye).
    nw: f64,
    /// Seen edge-on (`nw` is zero): no projected area.
    edge_on: bool,
    /// A point of the face plane.
    p0: Point3,
    /// The loops: the outer one first, then the holes.
    loops: Vec<Vec<LoopPt>>,
    /// Projected bounding box.
    bb: [f64; 4],
}

/// Uniform grid over the projected face boxes, each padded by `band`.
struct Grid {
    x0: f64,
    y0: f64,
    inv: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(faces: &[FaceRec], band: f64) -> Grid {
        let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let mut mean = 0.0;
        for f in faces {
            bb = [bb[0].min(f.bb[0]), bb[1].min(f.bb[1]), bb[2].max(f.bb[2]), bb[3].max(f.bb[3])];
            mean += 0.5 * ((f.bb[2] - f.bb[0]) + (f.bb[3] - f.bb[1]));
        }
        if faces.is_empty() {
            return Grid { x0: 0.0, y0: 0.0, inv: 0.0, nx: 1, ny: 1, cells: vec![Vec::new()] };
        }
        mean /= faces.len() as f64;
        let (w, h) = (bb[2] - bb[0], bb[3] - bb[1]);
        let k = ((faces.len() as f64).sqrt().ceil() as usize).clamp(1, 128) as f64;
        let cell = (w.max(h) / k).max(mean).max(band).max(f64::MIN_POSITIVE);
        let nx = ((w / cell).ceil() as usize).clamp(1, 256);
        let ny = ((h / cell).ceil() as usize).clamp(1, 256);
        let mut g = Grid { x0: bb[0], y0: bb[1], inv: 1.0 / cell, nx, ny, cells: vec![Vec::new(); nx * ny] };
        for (i, f) in faces.iter().enumerate() {
            let (cx0, cy0) = g.cell(f.bb[0] - band, f.bb[1] - band);
            let (cx1, cy1) = g.cell(f.bb[2] + band, f.bb[3] + band);
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    g.cells[cy * nx + cx].push(i as u32);
                }
            }
        }
        g
    }

    fn cell(&self, x: f64, y: f64) -> (usize, usize) {
        let cx = (((x - self.x0) * self.inv).floor().max(0.0) as usize).min(self.nx - 1);
        let cy = (((y - self.y0) * self.inv).floor().max(0.0) as usize).min(self.ny - 1);
        (cx, cy)
    }

    fn query(&self, q: Point2) -> &[u32] {
        let (cx, cy) = self.cell(q.x, q.y);
        &self.cells[cy * self.nx + cx]
    }
}

/// Whether the closed projected polygon of the face contains `q`: within `eps` of a loop vertex
/// or edge, or strictly inside by the even-odd rule with the exact [`orient2d`] predicate.
fn contains(f: &FaceRec, q: Point2, eps: f64) -> bool {
    let mut crossings = 0;
    for lp in &f.loops {
        let n = lp.len();
        for k in 0..n {
            let a = lp[k].q;
            let b = lp[(k + 1) % n].q;
            let qa = q - a;
            if qa.x * qa.x + qa.y * qa.y <= eps * eps {
                return true;
            }
            let e = b - a;
            let l2 = dot2(e, e);
            if l2 > 0.0 {
                let tau = dot2(qa, e) / l2;
                if tau > 0.0 && tau < 1.0 && cross2(e, qa).abs() <= eps * l2.sqrt() {
                    return true;
                }
            }
            if (a.y > q.y) != (b.y > q.y) {
                let o = orient2d(a, b, q);
                if (b.y > a.y && o > 0.0) || (b.y < a.y && o < 0.0) {
                    crossings += 1;
                }
            }
        }
    }
    crossings % 2 == 1
}

/// Distance from `m` to the closed segment `a`-`b`.
fn segment_distance(m: Point3, a: Point3, b: Point3) -> f64 {
    let e = b - a;
    let l2 = e.norm2();
    let tau = if l2 > 0.0 { ((m - a).dot(e) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (m - (a + e * tau)).norm()
}

/// Signed solid angle of the triangle `abc` (vectors from the viewpoint), by Van Oosterom and
/// Strackee's formula; positive when the triangle is wound counter-clockwise as seen from the
/// viewpoint's side of the outward normal, i.e. when the viewpoint lies behind the triangle.
fn solid_angle(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    let (la, lb, lc) = (a.norm(), b.norm(), c.norm());
    let num = a.dot(b.cross(c));
    let den = la * lb * lc + a.dot(b) * lc + b.dot(c) * la + c.dot(a) * lb;
    2.0 * num.atan2(den)
}

struct Ctx<'a> {
    solids: &'a [Solid],
    infos: &'a [SolidInfo],
    faces: &'a [FaceRec],
    grid: &'a Grid,
    /// Unit direction towards the eye.
    w: Vec3,
    /// Incidence tolerance.
    eps: f64,
    /// Candidate padding and boundary thickness (`3 * eps`).
    band: f64,
}

impl Ctx<'_> {
    /// Whether the open ray from `p` (projecting to `q`) towards the eye passes through the
    /// interior of some solid (see the [module documentation](self)).
    fn hidden(&self, p: Point3, q: Point2) -> bool {
        let band = self.band;
        let mut cands: Vec<u32> = self
            .grid
            .query(q)
            .iter()
            .copied()
            .filter(|&fi| {
                let bb = self.faces[fi as usize].bb;
                q.x >= bb[0] - band && q.x <= bb[2] + band && q.y >= bb[1] - band && q.y <= bb[3] + band
            })
            .collect();
        cands.sort_unstable_by_key(|&fi| self.faces[fi as usize].solid);
        let mut i = 0;
        while i < cands.len() {
            let k = self.faces[cands[i] as usize].solid;
            let j = cands[i..].iter().position(|&fi| self.faces[fi as usize].solid != k).map_or(cands.len(), |n| i + n);
            if self.through_solid(k, &cands[i..j], p, q) {
                return true;
            }
            i = j;
        }
        false
    }

    /// Whether the ray from `p` passes through the interior of solid `k`, whose candidate faces
    /// are `cands`.
    fn through_solid(&self, k: usize, cands: &[u32], p: Point3, q: Point2) -> bool {
        let (eps, w) = (self.eps, self.w);
        let depth = |x: Point3| (x - p).dot(w);
        // Depths at which the ray can touch the boundary.
        let mut events: Vec<f64> = Vec::new();
        for &fi in cands {
            let f = &self.faces[fi as usize];
            if !f.edge_on && contains(f, q, eps) {
                events.push(f.n.dot(f.p0 - p) / f.nw);
            }
            for lp in &f.loops {
                let n = lp.len();
                for i in 0..n {
                    let (a, b) = (lp[i], lp[(i + 1) % n]);
                    let qa = q - a.q;
                    let e = b.q - a.q;
                    let l2 = dot2(e, e);
                    if qa.x * qa.x + qa.y * qa.y <= eps * eps {
                        events.push(depth(a.p));
                        if l2 <= eps * eps {
                            events.push(depth(b.p)); // seen end-on: the whole edge is over `q`
                        }
                    }
                    if l2 > eps * eps {
                        let tau = dot2(qa, e) / l2;
                        if tau > 0.0 && tau < 1.0 && cross2(e, qa).abs() <= eps * l2.sqrt() {
                            events.push(depth(a.p + (b.p - a.p) * tau));
                        }
                    }
                }
            }
        }
        events.retain(|t| t.is_finite());
        events.sort_by(|a, b| a.total_cmp(b));
        let mut kept = vec![0.0];
        for t in events {
            if t > kept[kept.len() - 1] + eps {
                kept.push(t);
            }
        }
        kept.windows(2).any(|iv| {
            let m = p + w * (0.5 * (iv[0] + iv[1]));
            !self.on_boundary(cands, m) && self.winding(k, m) > 0.5
        })
    }

    /// Whether `m` lies on one of the candidate faces (within `band` of its plane and inside the
    /// closed face polygon).
    fn on_boundary(&self, cands: &[u32], m: Point3) -> bool {
        cands.iter().any(|&fi| {
            let f = &self.faces[fi as usize];
            f.n.dot(m - f.p0).abs() <= self.band && self.in_face(f, m)
        })
    }

    /// Whether the point `m` of the face plane lies in the closed face: within `band` of a loop
    /// edge, or inside by the even-odd rule in the plane's dominant projection.
    fn in_face(&self, f: &FaceRec, m: Point3) -> bool {
        let n = f.n;
        let flat: fn(Point3) -> Point2 = if n.x.abs() >= n.y.abs() && n.x.abs() >= n.z.abs() {
            |p| Point2::new(p.y, p.z)
        } else if n.y.abs() >= n.z.abs() {
            |p| Point2::new(p.z, p.x)
        } else {
            |p| Point2::new(p.x, p.y)
        };
        let q = flat(m);
        let mut crossings = 0;
        for lp in &f.loops {
            let len = lp.len();
            for i in 0..len {
                let (a3, b3) = (lp[i].p, lp[(i + 1) % len].p);
                if segment_distance(m, a3, b3) <= self.band {
                    return true;
                }
                let (a, b) = (flat(a3), flat(b3));
                if (a.y > q.y) != (b.y > q.y) {
                    let o = orient2d(a, b, q);
                    if (b.y > a.y && o > 0.0) || (b.y < a.y && o < 0.0) {
                        crossings += 1;
                    }
                }
            }
        }
        crossings % 2 == 1
    }

    /// Generalised winding number of solid `k` at `m` (1 inside, 0 outside), leaving out the
    /// faces whose plane passes within `eps` of `m`.
    fn winding(&self, k: usize, m: Point3) -> f64 {
        let s = &self.solids[k];
        let info = &self.infos[k];
        let mut total = 0.0;
        for (fi, face) in s.faces.iter().enumerate() {
            let Some(n) = info.normals[fi] else {
                continue;
            };
            let p0 = s.vertices[s.half_edges[face.loops[0]].origin].point;
            if n.dot(m - p0).abs() <= self.eps {
                continue;
            }
            for &start in &face.loops {
                let a = s.vertices[s.half_edges[start].origin].point - m;
                let mut h = s.half_edges[start].next;
                let mut b = s.vertices[s.half_edges[h].origin].point - m;
                h = s.half_edges[h].next;
                let mut guard = 0;
                while h != start && guard < s.half_edges.len() {
                    let c = s.vertices[s.half_edges[h].origin].point - m;
                    total += solid_angle(a, b, c);
                    b = c;
                    h = s.half_edges[h].next;
                    guard += 1;
                }
            }
        }
        total / (4.0 * std::f64::consts::PI)
    }
}
