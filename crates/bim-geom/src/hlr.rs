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
//!   (direction `-d`) passes through the *interior* of some solid. Touching a solid in a face,
//!   edge or vertex, or running along a face, is not occlusion; a solid's own faces through `p`
//!   never occlude it; a face seen edge-on never occludes anything.
//!
//! # Algorithm
//!
//! The classification of a piece tests its midpoint `p` against every face whose projection
//! contains the projection of `p` (a uniform grid over the projected face boxes finds the
//! candidates). After splitting, the projection of a midpoint can only lie on the projection of
//! another edge when the two edges' projections are collinear (axis-aligned views of boxes are
//! the usual case), so each candidate face is located exactly, with a tolerance band of width
//! `eps` around its boundary (`eps` is `1e-9` of the scene's bounding-box diagonal):
//!
//! * strictly inside the face polygon (even-odd rule with the exact [`orient2d`] predicate): the
//!   ray crosses the face transversally, which enters or leaves the solid's interior; the point is
//!   hidden if the crossing lies strictly in front of `p`;
//! * on a non-feature edge or on a vertex all of whose edges are non-feature: inside the planar
//!   region of coplanar faces, same as above;
//! * on a feature edge: the ray passes through that edge, and it enters the interior iff the
//!   viewing direction or its opposite points into the open wedge of the solid at the edge
//!   (`n₁·x < 0 ∧ n₂·x < 0` for a convex edge, `n₁·x < 0 ∨ n₂·x < 0` for a reflex one);
//! * on a vertex with feature edges: impossible for a midpoint, ignored.
//!
//! This is exact for the geometry (no sampling along the edges, no depth sorting of faces) up to
//! the tolerance `eps` used to decide incidences.

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
            if nw.abs() <= 1e-12 {
                continue; // seen edge-on: no projected area
            }
            let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            let loops: Vec<Vec<(Point2, usize)>> = face
                .loops
                .iter()
                .map(|&start| {
                    let mut lp = Vec::new();
                    let mut h = start;
                    loop {
                        let q = proj(s.vertices[s.half_edges[h].origin].point);
                        bb = [bb[0].min(q.x), bb[1].min(q.y), bb[2].max(q.x), bb[3].max(q.y)];
                        lp.push((q, h));
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
            faces.push(FaceRec { solid: k, n, nw, p0, loops, bb });
        }
    }
    let grid = Grid::new(&faces, eps);
    let ctx = Ctx { solids, infos: &infos, faces: &faces, grid: &grid, w, r, v, eps };

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
    /// Whether each vertex has at least one feature edge.
    feature_vertex: Vec<bool>,
}

fn analyse(s: &Solid) -> SolidInfo {
    let normals: Vec<Option<Vec3>> = (0..s.faces.len()).map(|f| s.face_normal(f).normalized()).collect();
    let mut feature = vec![false; s.half_edges.len()];
    let mut feature_vertex = vec![false; s.vertices.len()];
    for (h, he) in s.half_edges.iter().enumerate() {
        let is_feature = match he.twin {
            None => true,
            Some(t) => match (normals[he.face], normals.get(s.half_edges[t].face).copied().flatten()) {
                (Some(n1), Some(n2)) => n1.cross(n2).norm().atan2(n1.dot(n2)) > 1e-9,
                _ => true,
            },
        };
        feature[h] = is_feature;
        if is_feature {
            feature_vertex[he.origin] = true;
            feature_vertex[s.half_edges[he.next].origin] = true;
        }
    }
    SolidInfo { normals, feature, feature_vertex }
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

struct FaceRec {
    solid: usize,
    /// Unit outward normal.
    n: Vec3,
    /// `n · w` (`w` towards the eye); non-zero.
    nw: f64,
    /// A point of the face plane.
    p0: Point3,
    /// Projected loops as `(point, half-edge)`.
    loops: Vec<Vec<(Point2, usize)>>,
    bb: [f64; 4],
}

/// Uniform grid over the projected face boxes.
struct Grid {
    x0: f64,
    y0: f64,
    inv: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(faces: &[FaceRec], eps: f64) -> Grid {
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
        let cell = (w.max(h) / k).max(mean).max(eps).max(f64::MIN_POSITIVE);
        let nx = ((w / cell).ceil() as usize).clamp(1, 256);
        let ny = ((h / cell).ceil() as usize).clamp(1, 256);
        let mut g = Grid { x0: bb[0], y0: bb[1], inv: 1.0 / cell, nx, ny, cells: vec![Vec::new(); nx * ny] };
        for (i, f) in faces.iter().enumerate() {
            let (cx0, cy0) = g.cell(f.bb[0] - eps, f.bb[1] - eps);
            let (cx1, cy1) = g.cell(f.bb[2] + eps, f.bb[3] + eps);
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

enum Loc {
    Outside,
    Inside,
    OnEdge(usize),
    OnVertex(usize),
}

/// Locates `q` relative to the projected face: on a vertex or an edge within `eps`, else strictly
/// inside or outside by the even-odd rule.
fn locate(f: &FaceRec, q: Point2, eps: f64, s: &Solid) -> Loc {
    for lp in &f.loops {
        for &(a, h) in lp {
            let e = q - a;
            if e.x * e.x + e.y * e.y <= eps * eps {
                return Loc::OnVertex(s.half_edges[h].origin);
            }
        }
    }
    for lp in &f.loops {
        let n = lp.len();
        for k in 0..n {
            let (a, h) = lp[k];
            let b = lp[(k + 1) % n].0;
            let e = b - a;
            let l2 = dot2(e, e);
            if l2 <= 0.0 {
                continue;
            }
            let qa = q - a;
            let tau = dot2(qa, e) / l2;
            if tau > 0.0 && tau < 1.0 && cross2(e, qa).abs() <= eps * l2.sqrt() {
                return Loc::OnEdge(h);
            }
        }
    }
    let mut crossings = 0;
    for lp in &f.loops {
        let n = lp.len();
        for k in 0..n {
            let a = lp[k].0;
            let b = lp[(k + 1) % n].0;
            if (a.y > q.y) != (b.y > q.y) {
                let o = orient2d(a, b, q);
                if (b.y > a.y && o > 0.0) || (b.y < a.y && o < 0.0) {
                    crossings += 1;
                }
            }
        }
    }
    if crossings % 2 == 1 {
        Loc::Inside
    } else {
        Loc::Outside
    }
}

struct Ctx<'a> {
    solids: &'a [Solid],
    infos: &'a [SolidInfo],
    faces: &'a [FaceRec],
    grid: &'a Grid,
    /// Unit direction towards the eye.
    w: Vec3,
    /// View-plane axes.
    r: Vec3,
    v: Vec3,
    eps: f64,
}

impl Ctx<'_> {
    fn proj(&self, p: Point3) -> Point2 {
        Point2::new(p.to_vec().dot(self.r), p.to_vec().dot(self.v))
    }

    /// Whether the ray from `p` (projecting to `q`) towards the eye passes through the interior
    /// of some solid.
    fn hidden(&self, p: Point3, q: Point2) -> bool {
        let eps = self.eps;
        for &fi in self.grid.query(q) {
            let f = &self.faces[fi as usize];
            if q.x < f.bb[0] - eps || q.x > f.bb[2] + eps || q.y < f.bb[1] - eps || q.y > f.bb[3] + eps {
                continue;
            }
            let s = &self.solids[f.solid];
            let info = &self.infos[f.solid];
            let through_face = match locate(f, q, eps, s) {
                Loc::Outside => false,
                Loc::Inside => true,
                Loc::OnVertex(vtx) => !info.feature_vertex[vtx],
                Loc::OnEdge(h) => {
                    if info.feature[h] {
                        if self.through_edge(f.solid, h, p, q) {
                            return true;
                        }
                        false
                    } else {
                        true
                    }
                }
            };
            if through_face {
                // Depth of the transversal crossing of the face plane along the ray.
                let depth = f.n.dot(f.p0 - p) / f.nw;
                if depth > eps {
                    return true;
                }
            }
        }
        false
    }

    /// The ray from `p` passes through feature edge `h` of solid `k` in front of `p`: does it
    /// enter the solid's wedge there?
    fn through_edge(&self, k: usize, h: usize, p: Point3, q: Point2) -> bool {
        let s = &self.solids[k];
        let info = &self.infos[k];
        let he = &s.half_edges[h];
        let a = s.vertices[he.origin].point;
        let b = s.vertices[s.half_edges[he.next].origin].point;
        // Depth of the edge point on the ray: the point of the 3D edge projecting onto `q`.
        let (a2, b2) = (self.proj(a), self.proj(b));
        let e = b2 - a2;
        let l2 = dot2(e, e);
        if !(l2 > 0.0) {
            return false;
        }
        let tau = (dot2(q - a2, e) / l2).clamp(0.0, 1.0);
        let qp = a + (b - a) * tau;
        let depth = (qp - p).dot(self.w);
        if !(depth > self.eps) {
            return false;
        }
        let (Some(t), Some(n1)) = ((b - a).normalized(), info.normals[he.face]) else {
            return true;
        };
        let Some(tw) = he.twin else {
            return true; // open mesh: treat the face as the boundary of a half-space
        };
        let Some(n2) = info.normals[s.half_edges[tw].face] else {
            return true;
        };
        // Inward direction of the neighbouring face, perpendicular to the edge. Outer loops are
        // counter-clockwise and inner loops clockwise about the outward normal, so the face's
        // material lies to the left of every half-edge, whichever loop it belongs to: for the
        // twin (direction `-t` on a face with normal `n2`) that is `n2 × (-t)`.
        let m2 = n2.cross(-t);
        let convex = n1.dot(m2) < 0.0;
        let inside = |x: Vec3| {
            let (c1, c2) = (n1.dot(x) < -1e-12, n2.dot(x) < -1e-12);
            if convex {
                c1 && c2
            } else {
                c1 || c2
            }
        };
        inside(self.w) || inside(-self.w)
    }
}
