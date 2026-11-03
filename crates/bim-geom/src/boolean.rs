//! Boolean set operations (union, difference, intersection) on closed polyhedral solids.
//!
//! The algorithm is the classical "split, classify, select, stitch" scheme for B-Reps, with
//! every combinatorial decision taken exactly:
//!
//! 0. **Prepare.** The faces of both inputs are clustered into planes: faces whose vertices
//!    lie within a small fraction of the inputs' size of one another's plane share one exact
//!    plane (a face of an earlier result is planar only to rounding, and the same plane is
//!    rounded differently in the two inputs). Every vertex is then snapped onto the exact
//!    planes of its faces: a vertex already exactly on them is left alone, one with three
//!    planes becomes their exact common point, and one with more planes (the mitre vertex
//!    of a sweep, a vertex where edges crossed in an earlier boolean) is resolved into
//!    three-plane points joined by edges of rounding length (see [`Prep`]). After that every
//!    face is exactly planar with one plane, and the same geometry in both inputs is the
//!    same exact geometry.
//! 1. **Split.** Every face of `a` is split by the faces of `b` (and vice versa) inside its own
//!    plane: the intersection segments with the non-coplanar faces of the other solid and the
//!    edges of its coplanar faces are overlaid on the face as a 2D arrangement whose cells are
//!    the *fragments* of the face. All points of the arrangement are exact rational points
//!    ([`crate::exact::HPoint`], with arbitrary-precision dyadic coordinates): input or
//!    snapped vertices, an edge piercing a face plane, three planes meeting, or two coplanar
//!    segments crossing. Points are interned in one table keyed by their exact value, so a
//!    point seen from several faces (or produced by different formulas) is one node, and a
//!    constructed point that coincides with a vertex *is* that vertex. The arrangement itself
//!    (which segments cross or touch, the angular order at a node, the orientation of a
//!    cycle) is decided with exact predicates on those points. Edges of the solid itself that
//!    lie in the face's plane without bounding the face (the creases of a pinched input,
//!    where another sheet of the same solid touches the face) are overlaid too, so that both
//!    sheets are split at the same points.
//! 2. **Classify.** Each fragment is classified against the other solid from the local
//!    structure of the other solid along one of the fragment's boundary pieces that lies on
//!    its surface: an exact angular sweep around the piece's line over every face of the other
//!    solid through that line (faces bounded by an edge along the line and faces passing
//!    straight through it) tells which wedge the fragment is in and whether that wedge is
//!    material, whatever the number of sheets through the line (see [`Ctx::wedge_class`]); a
//!    piece along the edge of a coplanar face decides whether the fragment lies on that face
//!    (`On`). A fragment whose boundary never touches the other solid is classified by an
//!    exact ray cast (with symbolic perturbation) from a point of its boundary.
//! 3. **Select.** Fragments are selected per operation (union keeps what is outside the other
//!    solid, intersection what is inside, difference the outside of `a` and the reversed inside
//!    of `b`; coplanar fragments are kept once, from `a`).
//! 4. **Stitch.** The kept fragments share nodes by identity. Their half-edges are paired
//!    first: a manifold edge pairs its two half-edges; an edge with four or more (sheets
//!    touching along the edge) is paired by the angular order of their faces, each sheet
//!    bounding one material wedge, or one empty wedge where that is the only pairing that
//!    keeps every vertex a single fan; vertices are then split into one copy per umbrella.
//!    Only after that are coplanar fragments that are *twins* across an edge merged into
//!    maximal faces (with holes), so a face is never glued across a crease where two sheets
//!    merely touch, and `Solid::face_count` (maximal edge-connected coplanar regions) equals
//!    the number of faces of every result. Output coordinates are the correctly rounded exact
//!    coordinates; edges of rounding length (from resolved vertices) are collapsed, contacts
//!    of rounding size are welded, and vertices that became collinear are dropped. A final
//!    validation (planarity, no T-junctions, closed manifold) turns any inconsistency into an
//!    error rather than a malformed solid.
//!
//! Results are therefore valid inputs to further booleans, including results that pinch along
//! an edge (two sheets through one edge, with distinct vertex records at the same
//! coordinates) or at a vertex, results whose face contains a crease, and results whose faces
//! are planar only to rounding.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::exact::{cmp_coord, orient2d_h, plane_sign, same_point, HPoint, Plane};
use crate::predicates::orient2d;
use crate::solid::{HalfEdge, Solid, Vertex};
use crate::types::{Point2, Point3, Vec3};

/// A boolean set operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoolOp {
    /// `a ∪ b`.
    Union,
    /// `a − b`.
    Difference,
    /// `a ∩ b`.
    Intersection,
}

/// Errors of the geometric kernel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeomError {
    /// An input solid is not a closed 2-manifold.
    NotClosed(&'static str),
    /// The operation could not be completed consistently (a numerical degeneracy the kernel
    /// does not handle, or an input with degenerate faces).
    Degenerate(String),
}

impl std::fmt::Display for GeomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeomError::NotClosed(which) => write!(f, "input solid `{which}` is not a closed manifold"),
            GeomError::Degenerate(msg) => write!(f, "degenerate boolean: {msg}"),
        }
    }
}

impl std::error::Error for GeomError {}

/// Boolean operation `a op b` on two closed solids.
///
/// Both inputs must be closed 2-manifolds (or empty). The result is a closed 2-manifold with
/// planar faces, coplanar adjacent faces merged (faces may have inner loops), or the empty
/// solid when the result has no volume. Inputs touching along faces, edges or vertices are
/// handled: coplanar faces are overlaid exactly, and where the result only touches itself at an
/// edge or a vertex the sheets are kept separate in the half-edge structure (vertices are
/// duplicated), so `is_closed_manifold()` holds for every non-empty result.
pub fn boolean(a: &Solid, b: &Solid, op: BoolOp) -> Result<Solid, GeomError> {
    let (empty_a, empty_b) = (a.faces.is_empty(), b.faces.is_empty());
    if empty_a || empty_b {
        return Ok(match op {
            BoolOp::Union => {
                if empty_a {
                    b.clone()
                } else {
                    a.clone()
                }
            }
            BoolOp::Intersection => Solid::new(),
            BoolOp::Difference => {
                if empty_a {
                    Solid::new()
                } else {
                    a.clone()
                }
            }
        });
    }
    if !a.is_closed_manifold() {
        return Err(GeomError::NotClosed("a"));
    }
    if !b.is_closed_manifold() {
        return Err(GeomError::NotClosed("b"));
    }
    let ctx = Ctx::new(a, b)?;
    let mut kept: Vec<Kept> = Vec::new();
    for x in 0..2 {
        for f in 0..ctx.p[x].nf() {
            for frag in ctx.split_face(x, f)? {
                let class = ctx.classify(x, f, &frag)?;
                if let Some(flip) = select(op, x, class) {
                    kept.push(Kept { solid: x, face: f, loops: frag.loops, flip });
                }
            }
        }
    }
    let scale = ctx.p[0].volume.abs().max(ctx.p[1].volume.abs());
    assemble(&ctx, kept, scale)
}

// ---------------------------------------------------------------------------------------------
// Input preparation

/// Faces whose vertices all lie within this fraction of the inputs' size of one another's plane
/// are one plane: the faces of an earlier result (planar only to rounding) and the same plane
/// rounded differently in the two inputs. Distinct planes of a well-formed model are far
/// further apart than rounding noise (`~1e-16`).
const PLANE_TOL: f64 = 1e-11;
/// A vertex snapped onto its planes may not move further than this fraction of the inputs'
/// size; otherwise its planes are too ill-conditioned and the vertex is left in place.
const SNAP_TOL: f64 = 1e-9;
/// Edges of the result shorter than this fraction of the inputs' size are collapsed: the
/// infinitesimal edges left by resolving vertices with more than three planes.
const COLLAPSE_TOL: f64 = 1e-12;

/// An undirected edge of an input solid: `a < b`; `faces[0]` holds the half-edge `a -> b`,
/// `faces[1]` the half-edge `b -> a`.
struct Edge {
    a: usize,
    b: usize,
    faces: [usize; 2],
}

/// The planes of both inputs, clustered: faces whose vertices lie within [`PLANE_TOL`] of a
/// cluster's plane share that one exact plane, so that a face which is planar only to rounding
/// has one plane (not a fan of triangle planes), and the same plane in both inputs is one
/// plane. The representative of a cluster is the exact plane through the reference triangle of
/// its largest face.
struct Clusters {
    planes: Vec<Plane>,
    /// Size of the inputs: the larger of their joint bounding diagonal and their largest
    /// coordinate magnitude (rounding errors scale with the magnitude).
    scale: f64,
    /// Per solid, per face: the cluster and whether the face's outward normal agrees with the
    /// cluster plane's orientation.
    of_face: Vec<Vec<(usize, bool)>>,
}

impl Clusters {
    fn build(solids: &[&Solid], names: &[&str]) -> Result<Clusters, GeomError> {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        let mut maxabs = 0.0f64;
        for (x, s) in solids.iter().enumerate() {
            for v in s.vertices.iter().filter(|v| v.half_edge.is_some()) {
                if !(v.point.x.is_finite() && v.point.y.is_finite() && v.point.z.is_finite()) {
                    return Err(GeomError::Degenerate(format!("`{}` has non-finite coordinates", names[x])));
                }
                for k in 0..3 {
                    let c = coord(v.point, k);
                    lo[k] = lo[k].min(c);
                    hi[k] = hi[k].max(c);
                    maxabs = maxabs.max(c.abs());
                }
            }
        }
        let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
        let scale = diag.max(maxabs);
        if !scale.is_finite() || scale == 0.0 {
            return Err(GeomError::Degenerate("inputs have no extent".into()));
        }
        let tol = PLANE_TOL * scale;
        // Faces by decreasing reference-triangle area, so that representatives are well
        // conditioned; ties by position for determinism.
        let mut order: Vec<(f64, usize, usize, [usize; 3])> = Vec::new();
        for (x, s) in solids.iter().enumerate() {
            for f in 0..s.faces.len() {
                let r = s.reference_triangle(f).ok_or_else(|| GeomError::Degenerate(format!("face {f} of `{}` has no area", names[x])))?;
                let (a, b, c) = (s.vertices[r[0]].point, s.vertices[r[1]].point, s.vertices[r[2]].point);
                order.push(((b - a).cross(c - a).norm2(), x, f, r));
            }
        }
        order.sort_by(|p, q| q.0.partial_cmp(&p.0).unwrap_or(std::cmp::Ordering::Equal).then(p.1.cmp(&q.1)).then(p.2.cmp(&q.2)));
        // Clusters by decreasing face area: a face joins the first cluster whose plane holds
        // all its vertices within the tolerance, else founds one. The cluster's exact plane is
        // chosen afterwards among its faces' reference triangles as the one through the most
        // of the cluster's vertices (ties to the largest triangle), so that faces which are
        // exactly planar and exactly share a plane keep their vertices exactly on it (box
        // faces, caps and sides of extrusions with dyadic coordinates, edges of an earlier
        // result lying in such a plane) and no snapping is needed there.
        let mut fit: Vec<(Point3, Vec3)> = Vec::new();
        let mut members: Vec<Vec<(usize, usize, [usize; 3])>> = Vec::new();
        let mut of_face: Vec<Vec<(usize, bool)>> = solids.iter().map(|s| vec![(usize::MAX, true); s.faces.len()]).collect();
        for (_, x, f, r) in order {
            let s = solids[x];
            let verts: Vec<Point3> = s.faces[f].loops.iter().flat_map(|&h| s.loop_vertices(h)).map(|v| s.vertices[v].point).collect();
            let newell = s.face_normal(f);
            let mut found = None;
            for (c, &(o, n)) in fit.iter().enumerate() {
                if verts.iter().all(|&v| (v - o).dot(n).abs() <= tol) {
                    let d = newell.dot(n);
                    if d.abs() > 0.5 * newell.norm() {
                        found = Some((c, d > 0.0));
                        break;
                    }
                }
            }
            let (c, same) = match found {
                Some(cf) => cf,
                None => {
                    let (a, b, c) = (s.vertices[r[0]].point, s.vertices[r[1]].point, s.vertices[r[2]].point);
                    let n = (b - a).cross(c - a).normalized().ok_or_else(|| GeomError::Degenerate(format!("face {f} of `{}` has no area", names[x])))?;
                    fit.push((a, n));
                    members.push(Vec::new());
                    (fit.len() - 1, true)
                }
            };
            of_face[x][f] = (c, same);
            members[c].push((x, f, r));
        }
        // All vertex records of both solids, as exact points, for scoring the candidates.
        let all: Vec<(HPoint, usize)> = solids.iter().flat_map(|s| s.vertices.iter().filter(|v| v.half_edge.is_some()).map(|v| (HPoint::input(v.point), 1usize))).collect();
        let mut planes: Vec<Plane> = Vec::with_capacity(fit.len());
        for (c, ms) in members.iter().enumerate() {
            // Candidates: every triangle of every member face's triangulation; the best is the
            // one through the most vertex records of both solids (so that the exact contacts
            // between the solids, such as an edge of one lying in a face of the other, stay
            // exact), ties to the largest triangle.
            let mut best: Option<(usize, f64, Plane)> = None;
            for &(x, f, _) in ms {
                let s = solids[x];
                for &t in &s.faces[f].triangles {
                    let (a, b, cc) = (s.vertices[t[0]].point, s.vertices[t[1]].point, s.vertices[t[2]].point);
                    let area2 = (b - a).cross(cc - a).norm2();
                    if area2 == 0.0 {
                        continue;
                    }
                    let pl = Plane::through(a, b, cc);
                    if !pl.is_finite() {
                        return Err(GeomError::Degenerate(format!("face {f} of `{}` has non-finite coordinates", names[x])));
                    }
                    let pl = if of_face[x][f].1 { pl } else { pl.negated() };
                    let on: usize = all.iter().filter(|(v, _)| plane_sign(&pl, v) == 0).map(|(_, w)| *w).sum();
                    if best.as_ref().map_or(true, |(bo, ba, _)| on > *bo || (on == *bo && area2 > *ba)) {
                        best = Some((on, area2, pl));
                    }
                }
            }
            let (_, _, pl) = best.ok_or_else(|| GeomError::Degenerate(format!("cluster {c} has no triangle with area")))?;
            planes.push(pl);
        }
        Ok(Clusters { planes, scale, of_face })
    }
}

/// One input solid with the derived data the splitting needs.
///
/// Its vertices are first *snapped* onto the exact cluster planes of their faces: a vertex
/// that already lies exactly on all of them (every vertex of an input with exact coordinates)
/// is kept as it is; a vertex with three distinct planes becomes their exact common point; a
/// vertex with more distinct planes (the mitre vertex of a sweep, a vertex where two edges
/// crossed in an earlier boolean) cannot lie on all of them exactly and is *resolved* into
/// several three-plane vertices joined by infinitesimal edges (the dual of a fan triangulation
/// of its umbrella, a deterministic function of the planes, so the same vertex in both inputs
/// resolves to the same exact points). After that every face is exactly planar, with one plane
/// per face, which keeps every constructed point and every predicate consistent between the
/// faces that share it. A face whose vertices could not be snapped (planes too ill-conditioned,
/// or a genuinely non-planar input face) falls back to being split into exact triangles, which
/// are merged back by tolerance when the result is assembled.
///
/// The solid is then handled as a list of *sub-faces*: a planar face is one sub-face, a
/// fallback face one sub-face per triangle. Every "face" below is a sub-face.
struct Prep {
    /// Node id of every vertex: the input's vertex records (in order), then the vertices added
    /// by the resolution of vertices with more than three planes.
    vnode: Vec<usize>,
    /// Rounded coordinates of every vertex.
    vr: Vec<Point3>,
    /// Whether the vertex is used by a loop.
    used: Vec<bool>,
    /// Loops (outer first) of every input face after snapping, as vertex ids.
    face_loops: Vec<Vec<Vec<usize>>>,
    /// Fitted plane (a point and the unit normal) of every input face, for the tolerance test
    /// that merges fallback faces which are only coplanar to rounding.
    face_plane: Vec<Option<(Point3, Vec3)>>,
    /// Input face of every sub-face.
    parent: Vec<usize>,
    /// Cluster of a planar sub-face and whether its normal agrees with the cluster plane's.
    cluster: Vec<Option<(usize, bool)>>,
    /// Three non-collinear vertices of every sub-face (on its plane, wound outward).
    refs: Vec<[usize; 3]>,
    /// Exact outward-oriented plane of every sub-face.
    planes: Vec<Plane>,
    /// Vertex loops of every sub-face (outer first).
    loops: Vec<Vec<Vec<usize>>>,
    edges: Vec<Edge>,
    /// Directed vertex pair -> edge id.
    edge_ids: HashMap<(usize, usize), usize>,
    /// Edge ids around every face.
    face_edges: Vec<Vec<usize>>,
    /// Creases of every face: edges of the solid lying exactly in the face's plane without
    /// bounding the face. A closed manifold built by `extrude` / `sweep` has none; the
    /// result of a boolean that pinches along a line has the pinch edge as a crease of the
    /// face it lies in (when the sheets were paired so that the face is whole). Also the
    /// edges of an exactly coplanar neighbouring face, which lie outside the face.
    creases: Vec<Vec<usize>>,
    volume: f64,
}

impl Prep {
    fn new(s: &Solid, x: usize, name: &str, nodes: &mut Nodes, cl: &Clusters) -> Result<Self, GeomError> {
        let nhe = s.half_edges.len();
        let mut vnode: Vec<usize> = Vec::with_capacity(s.vertices.len());
        let mut vr: Vec<Point3> = Vec::with_capacity(s.vertices.len());
        for v in &s.vertices {
            if !(v.point.x.is_finite() && v.point.y.is_finite() && v.point.z.is_finite()) {
                return Err(GeomError::Degenerate(format!("`{name}` has non-finite coordinates")));
            }
            vnode.push(nodes.intern(HPoint::input(v.point)));
            vr.push(v.point);
        }
        // ---- snap every vertex onto its planes; `subst[h]` replaces the origin of half-edge `h`.
        let mut subst: Vec<Vec<usize>> = vec![Vec::new(); nhe];
        let snap_tol = SNAP_TOL * cl.scale;
        for v in 0..s.vertices.len() {
            let Some(h0) = s.vertices[v].half_edge else { continue };
            // The umbrella: half-edges leaving `v` in order around it.
            let mut hs = Vec::new();
            let mut h = h0;
            loop {
                hs.push(h);
                let t = s.half_edges[h].twin.ok_or(GeomError::NotClosed("input"))?;
                h = s.half_edges[t].next;
                if h == h0 {
                    break;
                }
                if hs.len() > nhe {
                    return Err(GeomError::NotClosed("input"));
                }
            }
            let planes_at: Vec<usize> = hs.iter().map(|&h| cl.of_face[x][s.half_edges[h].face].0).collect();
            let p = s.vertices[v].point;
            let hp = HPoint::input(p);
            let mut distinct = planes_at.clone();
            distinct.sort_unstable();
            distinct.dedup();
            if distinct.iter().all(|&c| plane_sign(&cl.planes[c], &hp) == 0) {
                for &h in &hs {
                    subst[h] = vec![v];
                }
                continue;
            }
            let far: Vec<Point3> = hs.iter().map(|&h| s.vertices[s.half_edges[s.half_edges[h].next].origin].point).collect();
            let resolved = resolve_vertex(p, &planes_at, &far, cl, snap_tol);
            let Some((apex, tri_pts)) = resolved else {
                // Not snappable: the vertex stays, and its faces are handled as triangles.
                for &h in &hs {
                    subst[h] = vec![v];
                }
                continue;
            };
            // Runs of consecutive faces in one plane (creases) form one corner of the umbrella.
            let runs = cyclic_runs(&planes_at);
            let m = runs.len();
            // Vertex id of every exact point (the first reuses `v`).
            let mut ids: Vec<(usize, usize)> = Vec::new(); // (node, vertex id)
            let mut id_of = |pt: &HPoint, nodes: &mut Nodes, vnode: &mut Vec<usize>, vr: &mut Vec<Point3>| -> usize {
                let n = nodes.intern(pt.clone());
                if let Some(&(_, id)) = ids.iter().find(|&&(nn, _)| nn == n) {
                    return id;
                }
                let id = if ids.is_empty() {
                    vnode[v] = n;
                    vr[v] = nodes.pts[n].r;
                    v
                } else {
                    vnode.push(n);
                    vr.push(nodes.pts[n].r);
                    vnode.len() - 1
                };
                ids.push((n, id));
                id
            };
            if m <= 2 {
                let id = id_of(&tri_pts[0], nodes, &mut vnode, &mut vr);
                for &h in &hs {
                    subst[h] = vec![id];
                }
                continue;
            }
            // `tri_pts[j]` for `j = 1 .. m-2` is the point of the fan triangle
            // `(apex, apex+j, apex+j+1)`; the side between runs `r` and `r+1` belongs to one
            // triangle, the apex run carries the whole chain.
            let tri_id: Vec<usize> = tri_pts.iter().enumerate().map(|(j, pt)| if j == 0 { usize::MAX } else { id_of(pt, nodes, &mut vnode, &mut vr) }).collect();
            let tri_of_side = |r: usize| -> usize {
                // Side (r, r+1), as a fan offset from the apex.
                let off = (r + m - apex) % m; // 0 for the apex's own leaving side
                if off == 0 {
                    1
                } else if off == m - 1 {
                    m - 2
                } else {
                    off
                }
            };
            for (r, (_, members)) in runs.iter().enumerate() {
                let arrive = tri_id[tri_of_side((r + m - 1) % m)];
                let leave = tri_id[tri_of_side(r)];
                let mut first: Vec<usize> = if r == apex { (1..=m - 2).rev().map(|j| tri_id[j]).collect() } else { vec![arrive, leave] };
                first.dedup();
                for (i, &j) in members.iter().enumerate() {
                    subst[hs[j]] = if i == 0 { first.clone() } else { vec![leave] };
                }
            }
        }
        // ---- loops of every input face after snapping.
        let mut used = vec![false; vnode.len()];
        let mut face_loops: Vec<Vec<Vec<usize>>> = Vec::with_capacity(s.faces.len());
        let mut face_plane: Vec<Option<(Point3, Vec3)>> = Vec::with_capacity(s.faces.len());
        for f in 0..s.faces.len() {
            let mut ls: Vec<Vec<usize>> = Vec::new();
            for (li, &h0) in s.faces[f].loops.iter().enumerate() {
                let mut l: Vec<usize> = Vec::new();
                let mut h = h0;
                loop {
                    l.extend_from_slice(&subst[h]);
                    h = s.half_edges[h].next;
                    if h == h0 || l.len() > 4 * nhe + 4 {
                        break;
                    }
                }
                l.dedup_by(|b, a| vnode[*b] == vnode[*a]);
                while l.len() > 1 && vnode[l[0]] == vnode[*l.last().unwrap()] {
                    l.pop();
                }
                if l.len() < 3 {
                    if li == 0 {
                        return Err(GeomError::Degenerate(format!("face {f} of `{name}` has no area")));
                    }
                    continue;
                }
                for &v in &l {
                    used[v] = true;
                }
                ls.push(l);
            }
            face_plane.push({
                let pts: Vec<Point3> = ls[0].iter().map(|&v| vr[v]).collect();
                newell(&pts).normalized().map(|n| (pts[0], n))
            });
            face_loops.push(ls);
        }
        // ---- sub-faces.
        let mut parent = Vec::new();
        let mut cluster = Vec::new();
        let mut refs = Vec::new();
        let mut planes = Vec::new();
        let mut loops: Vec<Vec<Vec<usize>>> = Vec::new();
        for f in 0..s.faces.len() {
            let (c, same) = cl.of_face[x][f];
            let pl = if same { cl.planes[c].clone() } else { cl.planes[c].negated() };
            let on_plane = face_loops[f].iter().flatten().all(|&v| plane_sign(&pl, &nodes.pts[vnode[v]]) == 0);
            if on_plane {
                let r = non_collinear_triple(nodes, &vnode, &face_loops[f][0], axes_of(pl.n_approx)).ok_or_else(|| GeomError::Degenerate(format!("face {f} of `{name}` has no area")))?;
                parent.push(f);
                cluster.push(Some((c, same)));
                refs.push(r);
                planes.push(pl);
                loops.push(face_loops[f].clone());
                continue;
            }
            for t in fallback_triangles(nodes, &vnode, &vr, &face_loops[f]) {
                let pl = Plane::through_h(&nodes.pts[vnode[t[0]]], &nodes.pts[vnode[t[1]]], &nodes.pts[vnode[t[2]]]);
                if !pl.is_finite() {
                    return Err(GeomError::Degenerate(format!("face {f} of `{name}` has non-finite coordinates")));
                }
                if pl.n.iter().all(|c| crate::exact::sign(c) == 0) {
                    continue; // degenerate triangle
                }
                parent.push(f);
                cluster.push(None);
                refs.push(t);
                planes.push(pl);
                loops.push(vec![t.to_vec()]);
            }
        }
        // Edges from the sub-face loops: every directed edge once, paired with its reverse.
        let mut directed: HashMap<(usize, usize), usize> = HashMap::new();
        for (f, ls) in loops.iter().enumerate() {
            for l in ls {
                for i in 0..l.len() {
                    if directed.insert((l[i], l[(i + 1) % l.len()]), f).is_some() {
                        return Err(GeomError::NotClosed("input"));
                    }
                }
            }
        }
        let mut edges = Vec::new();
        let mut edge_ids = HashMap::new();
        let mut keys: Vec<(usize, usize)> = directed.keys().copied().filter(|&(a, b)| a < b).collect();
        keys.sort();
        for (a, b) in keys {
            let fab = directed[&(a, b)];
            let fba = *directed.get(&(b, a)).ok_or(GeomError::NotClosed("input"))?;
            let id = edges.len();
            edges.push(Edge { a, b, faces: [fab, fba] });
            edge_ids.insert((a, b), id);
            edge_ids.insert((b, a), id);
        }
        if directed.len() != 2 * edges.len() {
            return Err(GeomError::NotClosed("input"));
        }
        let mut face_edges: Vec<Vec<usize>> = Vec::with_capacity(loops.len());
        for ls in &loops {
            let mut fe = Vec::new();
            for l in ls {
                for i in 0..l.len() {
                    fe.push(edge_ids[&(l[i], l[(i + 1) % l.len()])]);
                }
            }
            face_edges.push(fe);
        }
        let mut creases: Vec<Vec<usize>> = vec![Vec::new(); loops.len()];
        for (f, cr) in creases.iter_mut().enumerate() {
            let on = |v: usize| plane_sign(&planes[f], &nodes.pts[vnode[v]]) == 0;
            for (e, ed) in edges.iter().enumerate() {
                if on(ed.a) && on(ed.b) && !face_edges[f].contains(&e) {
                    cr.push(e);
                }
            }
        }
        Ok(Prep { vnode, vr, used, face_loops, face_plane, parent, cluster, refs, planes, loops, edges, edge_ids, face_edges, creases, volume: s.volume() })
    }

    /// Number of sub-faces.
    fn nf(&self) -> usize {
        self.loops.len()
    }

    /// Number of vertices (input records and added ones).
    fn nv(&self) -> usize {
        self.vnode.len()
    }

    /// The direction `(u, v)` in which edge `e` is traversed by the loop of face `f`.
    fn edge_dir_in(&self, f: usize, e: usize) -> (usize, usize) {
        let ed = &self.edges[e];
        if ed.faces[0] == f {
            (ed.a, ed.b)
        } else {
            (ed.b, ed.a)
        }
    }

    /// Projection axes `(u, v)` of a face so that its outer loop is counter-clockwise in 2D.
    fn axes(&self, f: usize) -> (usize, usize) {
        axes_of(self.planes[f].n_approx)
    }
}

/// Runs of consecutive equal values in a cyclic sequence, as `(value, indices)`, starting at
/// a run boundary (one run when all values are equal).
fn cyclic_runs(seq: &[usize]) -> Vec<(usize, Vec<usize>)> {
    let k = seq.len();
    let mut start = 0;
    while start < k && seq[start] == seq[(start + k - 1) % k] {
        start += 1;
    }
    if start == k {
        return vec![(seq[0], (0..k).collect())];
    }
    let mut runs = Vec::new();
    let mut i = start;
    while i < start + k {
        let c = seq[i % k];
        let mut members = Vec::new();
        while i < start + k && seq[i % k] == c {
            members.push(i % k);
            i += 1;
        }
        runs.push((c, members));
    }
    runs
}

/// The exact points a vertex at `p` with the given cyclic sequence of face planes is snapped
/// to: `(apex, points)` where, for an umbrella with `m` distinct corners (runs of equal
/// planes), `points[j]` for `j = 1 .. m-2` is the common point of the planes of corners
/// `apex`, `apex+j` and `apex+j+1` (one point when `m <= 3`; for `m < 3` the missing planes
/// are coordinate planes through `p`). `far[j]` is the other end of the edge leaving the
/// vertex between faces `j` and `j+1`.
///
/// A fan from an apex replaces the corner of every face other than the apex's neighbours by
/// a chamfer between two of the points; it is valid only when each chamfer cuts the corner
/// off rather than extending it, i.e. when each chamfer point lies, along its edge line, on
/// the same side of the plane that closes the corner as the far end of that edge. The apex
/// is the corner with the smallest plane id whose plane differs from every other corner's
/// and whose fan is valid, so that the resolution depends on the planes only. `None` when no
/// apex works or a point would move further than `tol`.
fn resolve_vertex(p: Point3, planes_at: &[usize], far: &[Point3], cl: &Clusters, tol: f64) -> Option<(usize, Vec<HPoint>)> {
    let runs = cyclic_runs(planes_at);
    let m = runs.len();
    let near = |q: &HPoint| (q.r - p).norm() <= tol;
    let dominant = |c: [f64; 3]| -> usize {
        let mut k = 0;
        for i in 1..3 {
            if c[i].abs() > c[k].abs() {
                k = i;
            }
        }
        k
    };
    let mut distinct: Vec<usize> = runs.iter().map(|(c, _)| *c).collect();
    distinct.sort_unstable();
    distinct.dedup();
    // One or two distinct planes (whatever the number of faces: a vertex where two sheets
    // touch has its planes twice around it): the point of the plane, or of the line, with
    // the vertex's own coordinates along it.
    if distinct.len() == 1 {
        let pl = &cl.planes[distinct[0]];
        let k = dominant(pl.n_approx);
        let (i, j) = ((k + 1) % 3, (k + 2) % 3);
        let q = HPoint::triple(pl, &Plane::axis(i, coord(p, i)), &Plane::axis(j, coord(p, j)))?;
        return near(&q).then_some((0, vec![q]));
    }
    if distinct.len() == 2 {
        let (p0, p1) = (&cl.planes[distinct[0]], &cl.planes[distinct[1]]);
        let (n0, n1) = (p0.n_approx, p1.n_approx);
        let dir = [n0[1] * n1[2] - n0[2] * n1[1], n0[2] * n1[0] - n0[0] * n1[2], n0[0] * n1[1] - n0[1] * n1[0]];
        let k = dominant(dir);
        let q = HPoint::triple(p0, p1, &Plane::axis(k, coord(p, k)))?;
        return near(&q).then_some((0, vec![q]));
    }
    let mut cands: Vec<usize> = (0..m).filter(|&a| runs.iter().enumerate().all(|(r, (c, _))| r == a || *c != runs[a].0)).collect();
    cands.sort_by_key(|&a| (runs[a].0, a));
    'apex: for a in cands {
        let mut pts = vec![HPoint::input(p)]; // index 0 unused
        for j in 1..=m - 2 {
            let (c0, c1, c2) = (runs[a].0, runs[(a + j) % m].0, runs[(a + j + 1) % m].0);
            match HPoint::triple(&cl.planes[c0], &cl.planes[c1], &cl.planes[c2]) {
                Some(q) if near(&q) => pts.push(q),
                _ => continue 'apex,
            }
        }
        // Validity of the chamfers (see above). The corner between runs `r` and `r+1` takes
        // the point of one fan triangle; the edge leaving there is the last half-edge of run `r`.
        let tri_of_side = |r: usize| -> usize {
            let off = (r + m - a) % m;
            if off == 0 {
                1
            } else if off == m - 1 {
                m - 2
            } else {
                off
            }
        };
        let far_of_side = |r: usize| HPoint::input(far[*runs[r].1.last().unwrap()]);
        for u in 0..m {
            let (prev, next) = ((u + m - 1) % m, (u + 1) % m);
            let (ta, tb) = (tri_of_side(prev), tri_of_side(u));
            if ta == tb || same_point(&pts[ta], &pts[tb]) {
                continue; // a single corner point: the natural corner of the face
            }
            // The arriving chamfer point lies on the edge line between `prev` and `u`, which
            // the plane of `next` crosses at the natural corner; it is *beyond* the corner
            // when it is not on the far end's side. Symmetrically for the leaving point. One
            // point beyond leaves the face a simple polygon (the corner grows a bump instead
            // of being cut); both beyond make it self-intersecting.
            let beyond = |pl: &Plane, q: &HPoint, w: &HPoint| -> Option<bool> {
                let (s1, s2) = (plane_sign(pl, q), plane_sign(pl, w));
                (s2 != 0).then_some(s1 != 0 && s1 != s2)
            };
            let arrive = beyond(&cl.planes[runs[next].0], &pts[ta], &far_of_side(prev));
            let leave = beyond(&cl.planes[runs[prev].0], &pts[tb], &far_of_side(u));
            match (arrive, leave) {
                (Some(false), _) | (_, Some(false)) => {}
                _ => continue 'apex,
            }
        }
        return Some((a, pts));
    }
    None
}

/// Three non-collinear vertices of a loop (its first vertex and two others), or `None` when
/// the loop has no area.
fn non_collinear_triple(nodes: &Nodes, vnode: &[usize], l: &[usize], ax: (usize, usize)) -> Option<[usize; 3]> {
    let n = l.len();
    for i in 1..n {
        for j in i + 1..n {
            if orient2d_h(&nodes.pts[vnode[l[0]]], &nodes.pts[vnode[l[i]]], &nodes.pts[vnode[l[j]]], ax) > 0 {
                return Some([l[0], l[i], l[j]]);
            }
        }
    }
    None
}

/// Triangles of a face that is not exactly planar, from the rounded coordinates of its
/// vertices. A convex single loop is fanned from its lexicographically smallest vertex, which
/// depends only on the vertex set, so that the same face occurring in both inputs (with
/// opposite orientation) is split identically and the pieces are recognised as exactly
/// coplanar; other faces are ear-clipped. Triangles without area are left out by the caller.
fn fallback_triangles(nodes: &Nodes, vnode: &[usize], vr: &[Point3], ls: &[Vec<usize>]) -> Vec<[usize; 3]> {
    let pts: Vec<Point3> = ls[0].iter().map(|&v| vr[v]).collect();
    let normal = newell(&pts);
    let ax = axes_of([normal.x, normal.y, normal.z]);
    if ls.len() == 1 && ls[0].len() >= 4 {
        let l = &ls[0];
        let n = l.len();
        let convex = (0..n).all(|i| orient2d_h(&nodes.pts[vnode[l[(i + n - 1) % n]]], &nodes.pts[vnode[l[i]]], &nodes.pts[vnode[l[(i + 1) % n]]], ax) > 0);
        if convex {
            let mut m = 0;
            for i in 1..n {
                if (0..3).map(|k| cmp_coord(&nodes.pts[vnode[l[i]]], &nodes.pts[vnode[l[m]]], k)).find(|&c| c != 0) == Some(-1) {
                    m = i;
                }
            }
            return (1..n - 1).map(|k| [l[m], l[(m + k) % n], l[(m + k + 1) % n]]).collect();
        }
    }
    let flat: Vec<usize> = ls.iter().flatten().copied().collect();
    let poly = crate::polygon::Polygon {
        outer: ls[0].iter().map(|&v| project(vr[v], ax)).collect(),
        holes: ls[1..].iter().map(|l| l.iter().map(|&v| project(vr[v], ax)).collect()).collect(),
    };
    poly.triangulate().into_iter().map(|t| [flat[t[0]], flat[t[1]], flat[t[2]]]).collect()
}

/// Projection axes of a plane with normal `n`: drop the dominant coordinate, ordered so that
/// a loop counter-clockwise about `n` stays counter-clockwise.
fn axes_of(c: [f64; 3]) -> (usize, usize) {
    let mut k = 0;
    for i in 1..3 {
        if c[i].abs() > c[k].abs() {
            k = i;
        }
    }
    if c[k] > 0.0 {
        ((k + 1) % 3, (k + 2) % 3)
    } else {
        ((k + 2) % 3, (k + 1) % 3)
    }
}

fn coord(p: Point3, k: usize) -> f64 {
    match k {
        0 => p.x,
        1 => p.y,
        _ => p.z,
    }
}

fn project(p: Point3, ax: (usize, usize)) -> Point2 {
    Point2::new(coord(p, ax.0), coord(p, ax.1))
}

fn key_of(p: Point3) -> [u64; 3] {
    let b = |x: f64| if x == 0.0 { 0.0f64.to_bits() } else { x.to_bits() };
    [b(p.x), b(p.y), b(p.z)]
}

/// Exact relations of the vertices of one solid to the face planes of the other.
struct Rel {
    /// `vside[v][g]`: side of vertex `v` with respect to face `g` of the other solid.
    vside: Vec<Vec<i8>>,
}

/// The table of exact points shared by every face arrangement. Ids are assigned in order of
/// first construction, which is deterministic.
#[derive(Default)]
struct Nodes {
    pts: Vec<HPoint>,
    by_key: HashMap<[u64; 3], Vec<usize>>,
    /// `(solid of edge, edge, solid of face, face)` -> node of the edge piercing the plane.
    pierce_cache: HashMap<(usize, usize, usize, usize), Option<usize>>,
    /// Sorted `(solid, face)` triple -> node of the three planes meeting.
    triple_cache: HashMap<[(usize, usize); 3], Option<usize>>,
}

impl Nodes {
    fn intern(&mut self, p: HPoint) -> usize {
        let key = key_of(p.r);
        if let Some(ids) = self.by_key.get(&key) {
            for &i in ids {
                if same_point(&self.pts[i], &p) {
                    return i;
                }
            }
        }
        let i = self.pts.len();
        self.pts.push(p);
        self.by_key.entry(key).or_default().push(i);
        i
    }
}

/// Both inputs plus their mutual exact relations.
struct Ctx {
    p: [Prep; 2],
    rel: [Rel; 2],
    /// `cop[f][g]` for face `f` of `a` and `g` of `b`: `Some(same_orientation)` when the faces
    /// are exactly coplanar.
    cop: Vec<Vec<Option<bool>>>,
    /// Size of the inputs (see [`Clusters::scale`]).
    scale: f64,
    nodes: RefCell<Nodes>,
    /// `(solid, edge)` -> nodes in the interior of the edge that lie on the other solid's
    /// surface (see [`Ctx::edge_nodes`]).
    edge_nodes: RefCell<HashMap<(usize, usize), Vec<usize>>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Inside,
    Outside,
    /// On a coplanar face of the other solid; the flag tells whether the normals agree.
    On(bool),
}

impl Ctx {
    fn new(a: &Solid, b: &Solid) -> Result<Self, GeomError> {
        let mut nodes = Nodes::default();
        let clusters = Clusters::build(&[a, b], &["a", "b"])?;
        let pa = Prep::new(a, 0, "a", &mut nodes, &clusters)?;
        let pb = Prep::new(b, 1, "b", &mut nodes, &clusters)?;
        let rel_of = |p: &Prep, q: &Prep| {
            let vside: Vec<Vec<i8>> = (0..p.nv())
                .map(|v| (0..q.nf()).map(|g| if p.used[v] { plane_sign(&q.planes[g], &nodes.pts[p.vnode[v]]) as i8 } else { 0 }).collect())
                .collect();
            Rel { vside }
        };
        let rel = [rel_of(&pa, &pb), rel_of(&pb, &pa)];
        let mut cop = vec![vec![None; pb.nf()]; pa.nf()];
        for (f, row) in cop.iter_mut().enumerate() {
            for (g, cell) in row.iter_mut().enumerate() {
                // Planar faces are coplanar iff they share a cluster plane; a fallback triangle
                // is compared exactly.
                *cell = match (pa.cluster[f], pb.cluster[g]) {
                    (Some((cf, of)), Some((cg, og))) => (cf == cg).then_some(of == og),
                    _ => {
                        let on_f = pb.refs[g].iter().all(|&v| rel[1].vside[v][f] == 0);
                        let on_g = pa.refs[f].iter().all(|&v| rel[0].vside[v][g] == 0);
                        if on_f && on_g {
                            let (na, nb) = (pa.planes[f].n_approx, pb.planes[g].n_approx);
                            Some(na[0] * nb[0] + na[1] * nb[1] + na[2] * nb[2] > 0.0)
                        } else {
                            None
                        }
                    }
                };
            }
        }
        Ok(Ctx { p: [pa, pb], rel, cop, scale: clusters.scale, nodes: RefCell::new(nodes), edge_nodes: RefCell::new(HashMap::new()) })
    }

    fn coplanar(&self, x: usize, f: usize, g: usize) -> Option<bool> {
        if x == 0 {
            self.cop[f][g]
        } else {
            self.cop[g][f]
        }
    }

    fn side(&self, x: usize, v: usize, g: usize) -> i32 {
        self.rel[x].vside[v][g] as i32
    }


    fn rounded(&self, i: usize) -> Point3 {
        self.nodes.borrow().pts[i].r
    }

    // ------------------------------------------------------------------ exact points

    /// Node where edge `e` of solid `s` pierces the plane of face `g` of solid `t` (the edge
    /// must cross the plane strictly).
    fn pierce(&self, s: usize, e: usize, t: usize, g: usize) -> Option<usize> {
        let key = (s, e, t, g);
        if let Some(&n) = self.nodes.borrow().pierce_cache.get(&key) {
            return n;
        }
        let ed = &self.p[s].edges[e];
        let p = {
            let nodes = self.nodes.borrow();
            HPoint::pierce_h(&nodes.pts[self.p[s].vnode[ed.a]], &nodes.pts[self.p[s].vnode[ed.b]], &self.p[t].planes[g])
        };
        let n = p.map(|p| self.nodes.borrow_mut().intern(p));
        self.nodes.borrow_mut().pierce_cache.insert(key, n);
        n
    }

    /// Node where three face planes meet, `None` when they are not independent.
    fn triple(&self, mut faces: [(usize, usize); 3]) -> Option<usize> {
        faces.sort();
        if let Some(&n) = self.nodes.borrow().triple_cache.get(&faces) {
            return n;
        }
        let pl = |(x, f): (usize, usize)| &self.p[x].planes[f];
        let n = HPoint::triple(pl(faces[0]), pl(faces[1]), pl(faces[2])).map(|p| self.nodes.borrow_mut().intern(p));
        self.nodes.borrow_mut().triple_cache.insert(faces, n);
        n
    }

    /// Side of node `n` with respect to the plane of face `g` of solid `t`.
    fn plane_side(&self, t: usize, g: usize, n: usize) -> i32 {
        plane_sign(&self.p[t].planes[g], &self.nodes.borrow().pts[n])
    }

    fn orient(&self, a: usize, b: usize, c: usize, ax: (usize, usize)) -> i32 {
        let n = self.nodes.borrow();
        orient2d_h(&n.pts[a], &n.pts[b], &n.pts[c], ax)
    }

    fn cmp(&self, a: usize, b: usize, k: usize) -> i32 {
        if a == b {
            return 0;
        }
        let n = self.nodes.borrow();
        cmp_coord(&n.pts[a], &n.pts[b], k)
    }

    /// An axis along which the nodes `a` and `b` differ, preferring the projection axes `ax`
    /// (when given) and the largest rounded difference.
    fn sep_axis(&self, a: usize, b: usize, ax: Option<(usize, usize)>) -> Option<usize> {
        let (pa, pb) = (self.rounded(a), self.rounded(b));
        let mut cands: Vec<usize> = match ax {
            Some((u, v)) => vec![u, v],
            None => vec![0, 1, 2],
        };
        cands.sort_by(|&i, &j| (coord(pb, j) - coord(pa, j)).abs().partial_cmp(&(coord(pb, i) - coord(pa, i)).abs()).unwrap_or(std::cmp::Ordering::Equal));
        cands.into_iter().find(|&k| self.cmp(a, b, k) != 0)
    }

    /// Sign of the direction of `p -> q` along the segment of nodes `a -> b` it is collinear
    /// with (positive when they point the same way).
    fn dir_along(&self, p: usize, q: usize, a: usize, b: usize) -> i32 {
        match self.sep_axis(a, b, None) {
            Some(k) => self.cmp(q, p, k) * self.cmp(b, a, k),
            None => 0,
        }
    }

    /// Exact collinearity of three nodes.
    fn collinear(&self, a: usize, b: usize, c: usize) -> bool {
        [(0, 1), (1, 2), (2, 0)].iter().all(|&ax| self.orient(a, b, c, ax) == 0)
    }

    /// The nodes strictly inside edge `e` of solid `x` where it meets the surface of the other
    /// solid: its piercings of the other solid's face planes that fall within those faces
    /// (closed), and the other solid's vertices lying on it. Both faces at `e` split the edge
    /// at exactly these nodes, whether or not an intersection segment ends there, so they
    /// always agree on the edge's vertices.
    fn edge_nodes(&self, x: usize, e: usize) -> Vec<usize> {
        if let Some(v) = self.edge_nodes.borrow().get(&(x, e)) {
            return v.clone();
        }
        let y = 1 - x;
        let p = &self.p[x];
        let q = &self.p[y];
        let ed = &p.edges[e];
        let (na, nb) = (p.vnode[ed.a], p.vnode[ed.b]);
        let mut out = Vec::new();
        for g in 0..q.nf() {
            if self.side(x, ed.a, g) * self.side(x, ed.b, g) < 0 {
                if let Some(n) = self.pierce(x, e, y, g) {
                    let inside = {
                        let nodes = self.nodes.borrow();
                        locate_in_face(self, y, g, &nodes.pts[n]) != Locate::Outside
                    };
                    if inside && n != na && n != nb {
                        out.push(n);
                    }
                }
            }
        }
        // Vertices lying strictly inside the edge: the other solid's, and the solid's own
        // (another sheet of a pinched input touching the edge at a vertex, or a crease
        // ending on it), so that both faces at `e` split there.
        // (The bounding box of the rounded end points contains the rounded coordinates of
        // every node of the segment, rounding being monotone.)
        let (pa, pb) = (p.vr[ed.a], p.vr[ed.b]);
        if let Some(k) = self.sep_axis(na, nb, None) {
            let lo = Point3::new(pa.x.min(pb.x), pa.y.min(pb.y), pa.z.min(pb.z));
            let hi = Point3::new(pa.x.max(pb.x), pa.y.max(pb.y), pa.z.max(pb.z));
            for r in [q, p] {
                for v in 0..r.nv() {
                    let n = r.vnode[v];
                    let c = r.vr[v];
                    if !r.used[v] || n == na || n == nb || c.x < lo.x || c.x > hi.x || c.y < lo.y || c.y > hi.y || c.z < lo.z || c.z > hi.z {
                        continue;
                    }
                    if !self.collinear(na, nb, n) {
                        continue;
                    }
                    if self.cmp(n, na, k) * self.cmp(n, nb, k) < 0 {
                        out.push(n);
                    }
                }
            }
        }
        // Crossings with the creases of the faces at `e` (edges of the same solid lying in a
        // face plane without bounding the face, see `Prep::creases`): the face arrangement
        // splits its boundary there, so the face on the other side of `e` must split too.
        for &f in &ed.faces {
            let ax = p.axes(f);
            let phi = if ed.faces[0] == f { ed.faces[1] } else { ed.faces[0] };
            for &e2 in &p.creases[f] {
                let ed2 = &p.edges[e2];
                let (n2a, n2b) = (p.vnode[ed2.a], p.vnode[ed2.b]);
                if e2 == e || n2a == n2b || [n2a, n2b].iter().any(|&n| n == na || n == nb) {
                    continue;
                }
                if self.orient(na, nb, n2a, ax) * self.orient(na, nb, n2b, ax) >= 0 || self.orient(n2a, n2b, na, ax) * self.orient(n2a, n2b, nb, ax) >= 0 {
                    continue;
                }
                let n = if self.plane_side(x, phi, n2a) * self.plane_side(x, phi, n2b) < 0 {
                    self.pierce(x, e2, x, phi)
                } else {
                    let pt = {
                        let nodes = self.nodes.borrow();
                        HPoint::cross_in_plane(&nodes.pts[na], &nodes.pts[nb], &nodes.pts[n2a], &nodes.pts[n2b], ax)
                    };
                    pt.map(|q| self.nodes.borrow_mut().intern(q))
                };
                if let Some(n) = n {
                    if n != na && n != nb {
                        out.push(n);
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        self.edge_nodes.borrow_mut().insert((x, e), out.clone());
        out
    }

    // ------------------------------------------------------------------ splitting

    /// The fragments of face `f` of solid `x` after splitting by the other solid.
    fn split_face(&self, x: usize, f: usize) -> Result<Vec<Fragment>, GeomError> {
        let mut arr = Arrangement::new(self, x, f);
        let y = 1 - x;
        for g in 0..self.p[y].nf() {
            if self.coplanar(x, f, g).is_some() {
                arr.add_overlay(g);
            } else {
                arr.add_section(g)?;
            }
        }
        if arr.segs.len() == arr.boundary_count && arr.segs.iter().all(|s| s.splits.is_empty()) {
            // (Own-edge segments count as constraints too: a creased face always goes
            // through the arrangement, so that both sheets are split identically.)
            // Untouched face: a single fragment with its own loops.
            let p = &self.p[x];
            let loops: Vec<Vec<usize>> = p.loops[f].iter().map(|l| l.iter().map(|&v| p.vnode[v]).collect()).collect();
            let mut pieces = Vec::new();
            for l in &p.loops[f] {
                for i in 0..l.len() {
                    let (u, v) = (l[i], l[(i + 1) % l.len()]);
                    pieces.push(Piece { a: p.vnode[u], b: p.vnode[v], srcs: vec![Src::Bnd(p.edge_ids[&(u, v)])], clipped: false });
                }
            }
            return Ok(vec![Fragment { loops, pieces }]);
        }
        arr.cells()
    }
}

/// A fragment of a face: its loops (outer first) as node ids, and its directed boundary pieces
/// (the fragment lies to the left of each piece) with what they lie on.
struct Fragment {
    loops: Vec<Vec<usize>>,
    pieces: Vec<Piece>,
}

#[derive(Clone, Debug)]
struct Piece {
    a: usize,
    b: usize,
    srcs: Vec<Src>,
    /// Whether the piece came from a section segment (clipped to the face interior).
    clipped: bool,
}

/// What a piece of the arrangement lies on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Src {
    /// Boundary edge `e` of the face being split.
    Bnd(usize),
    /// Along edge `e` of the other solid.
    Edge(usize),
    /// In the interior of face `g` of the other solid.
    Face(usize),
    /// Along edge `e` of the *same* solid, lying in the plane of the face being split without
    /// bounding it: a crease where another sheet of the solid touches the face (left behind
    /// by an earlier boolean whose result pinches along a line). It says nothing about the
    /// other solid; it is overlaid so that both sheets are split at the same points.
    Own(usize),
}

/// The line a segment lies on, for exact side tests.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Line {
    /// Input edge `e` of solid `s`.
    Edge { s: usize, e: usize },
    /// The intersection of the face being split with the plane of face `g` of the other solid.
    Plane { g: usize },
}

/// A segment of a face arrangement.
struct Seg {
    a: usize,
    b: usize,
    line: Line,
    src: Src,
    clipped: bool,
    splits: Vec<usize>,
}

/// The 2D arrangement of one face with the constraints imposed by the other solid.
struct Arrangement<'c> {
    ctx: &'c Ctx,
    x: usize,
    f: usize,
    ax: (usize, usize),
    segs: Vec<Seg>,
    boundary_count: usize,
}

/// A sub-edge of the arrangement after splitting.
#[derive(Clone, Debug, Default)]
struct SubEdge {
    srcs: Vec<Src>,
    clipped: bool,
}

impl<'c> Arrangement<'c> {
    fn new(ctx: &'c Ctx, x: usize, f: usize) -> Self {
        let p = &ctx.p[x];
        let mut arr = Arrangement { ctx, x, f, ax: p.axes(f), segs: Vec::new(), boundary_count: 0 };
        for l in &p.loops[f] {
            for i in 0..l.len() {
                let (u, v) = (l[i], l[(i + 1) % l.len()]);
                let e = p.edge_ids[&(u, v)];
                let splits = ctx.edge_nodes(x, e);
                arr.segs.push(Seg { a: p.vnode[u], b: p.vnode[v], line: Line::Edge { s: x, e }, src: Src::Bnd(e), clipped: false, splits });
            }
        }
        arr.boundary_count = arr.segs.len();
        // Edges of the same solid lying in the face plane without bounding the face: creases
        // of a pinched input. Their sub-edges that end inside the face are pruned as dangling;
        // the others split the face, and the pieces are merged back when the result is
        // assembled if they end up as twins.
        for &e in &p.creases[f] {
            let ed = &p.edges[e];
            if p.vnode[ed.a] == p.vnode[ed.b] {
                continue;
            }
            let splits = ctx.edge_nodes(x, e);
            arr.segs.push(Seg { a: p.vnode[ed.a], b: p.vnode[ed.b], line: Line::Edge { s: x, e }, src: Src::Own(e), clipped: false, splits });
        }
        arr
    }

    /// Overlay the edges of a coplanar face `g` of the other solid. Each edge is split at the
    /// same nodes as in the arrangement of its own faces, so that the pieces along it are
    /// identical in both solids' fragments.
    fn add_overlay(&mut self, g: usize) {
        let y = 1 - self.x;
        let q = &self.ctx.p[y];
        for l in &q.loops[g] {
            for i in 0..l.len() {
                let (u, v) = (l[i], l[(i + 1) % l.len()]);
                let e = q.edge_ids[&(u, v)];
                if q.vnode[u] == q.vnode[v] {
                    continue;
                }
                let splits = self.ctx.edge_nodes(y, e);
                self.segs.push(Seg { a: q.vnode[u], b: q.vnode[v], line: Line::Edge { s: y, e }, src: Src::Edge(e), clipped: false, splits });
            }
        }
    }

    /// Intersection of face `pf` of solid `px` with the plane of face `q` of the other solid,
    /// along the line of the two planes ordered by coordinate `axis`: the crossing events
    /// (nodes, sorted) and the polygon edges lying on the plane (as `(lo, hi, edge)`).
    fn section(&self, px: usize, pf: usize, q: usize, axis: usize) -> (Vec<usize>, Vec<(usize, usize, usize)>) {
        let ctx = self.ctx;
        let p = &ctx.p[px];
        let py = 1 - px;
        let mut events: Vec<usize> = Vec::new();
        let mut on_plane = Vec::new();
        for l in &p.loops[pf] {
            for i in 0..l.len() {
                let (u, v) = (l[i], l[(i + 1) % l.len()]);
                let (su, sv) = (ctx.side(px, u, q), ctx.side(px, v, q));
                let (nu, nv) = (p.vnode[u], p.vnode[v]);
                if su == 0 && sv == 0 {
                    if nu == nv {
                        continue;
                    }
                    let e = p.edge_ids[&(u, v)];
                    if ctx.cmp(nu, nv, axis) <= 0 {
                        on_plane.push((nu, nv, e));
                    } else {
                        on_plane.push((nv, nu, e));
                    }
                } else if (su >= 0) != (sv >= 0) {
                    let n = if su == 0 {
                        Some(nu)
                    } else if sv == 0 {
                        Some(nv)
                    } else {
                        ctx.pierce(px, p.edge_ids[&(u, v)], py, q)
                    };
                    if let Some(n) = n {
                        events.push(n);
                    }
                }
            }
        }
        events.sort_by(|&a, &b| ctx.cmp(a, b, axis).cmp(&0).then(a.cmp(&b)));
        (events, on_plane)
    }

    /// Add the intersection segments with a non-coplanar face `g` of the other solid.
    fn add_section(&mut self, g: usize) -> Result<(), GeomError> {
        let (x, f) = (self.x, self.f);
        let y = 1 - x;
        let ctx = self.ctx;
        let dir = crate::exact::cross3(&ctx.p[x].planes[f].n, &ctx.p[y].planes[g].n);
        let mags = [dir[0].approx().abs(), dir[1].approx().abs(), dir[2].approx().abs()];
        let mut axis = 0;
        for k in 1..3 {
            if mags[k] > mags[axis] {
                axis = k;
            }
        }
        if mags[axis] == 0.0 {
            return Ok(()); // parallel planes
        }
        let (ev_g, on_g) = self.section(y, g, f, axis);
        if ev_g.is_empty() && on_g.is_empty() {
            return Ok(());
        }
        let (ev_f, on_f) = self.section(x, f, g, axis);
        // Closed intervals of the line inside `f`.
        let mut iv_f: Vec<(usize, usize)> = Vec::new();
        for pair in ev_f.chunks(2) {
            if pair.len() == 2 && ctx.cmp(pair[0], pair[1], axis) < 0 {
                iv_f.push((pair[0], pair[1]));
            }
        }
        for &(lo, hi, _) in &on_f {
            if ctx.cmp(lo, hi, axis) < 0 {
                iv_f.push((lo, hi));
            }
        }
        if iv_f.is_empty() {
            return Ok(());
        }
        // Closed intervals of the line inside `g`, with what they lie on. The crossing events
        // treat vertices on the plane as slightly inside, so an edge of `g` along the line
        // with `g` on the far side shows up as a zero-width "interior" interval as well: those
        // stretches are removed from the interior intervals, they lie on the edge, not in the
        // face.
        let mut iv_g: Vec<(usize, usize, Src, Line)> = Vec::new();
        let mut on_sorted: Vec<(usize, usize)> = on_g.iter().map(|&(lo, hi, _)| (lo, hi)).filter(|&(lo, hi)| ctx.cmp(lo, hi, axis) < 0).collect();
        on_sorted.sort_by(|a, b| ctx.cmp(a.0, b.0, axis).cmp(&0));
        for pair in ev_g.chunks(2) {
            if pair.len() != 2 || ctx.cmp(pair[0], pair[1], axis) >= 0 {
                continue;
            }
            let mut lo = pair[0];
            let hi = pair[1];
            for &(elo, ehi) in &on_sorted {
                if ctx.cmp(ehi, lo, axis) <= 0 || ctx.cmp(elo, hi, axis) >= 0 {
                    continue;
                }
                if ctx.cmp(elo, lo, axis) > 0 {
                    iv_g.push((lo, elo, Src::Face(g), Line::Plane { g }));
                }
                if ctx.cmp(ehi, lo, axis) > 0 {
                    lo = ehi;
                }
            }
            if ctx.cmp(lo, hi, axis) < 0 {
                iv_g.push((lo, hi, Src::Face(g), Line::Plane { g }));
            }
        }
        for &(lo, hi, e) in &on_g {
            if ctx.cmp(lo, hi, axis) < 0 {
                iv_g.push((lo, hi, Src::Edge(e), Line::Edge { s: y, e }));
            }
        }
        for &(ga, gb, src, line) in &iv_g {
            for &(fa, fb) in &iv_f {
                let lo = if ctx.cmp(fa, ga, axis) > 0 { fa } else { ga };
                let hi = if ctx.cmp(fb, gb, axis) < 0 { fb } else { gb };
                if ctx.cmp(lo, hi, axis) < 0 {
                    self.segs.push(Seg { a: lo, b: hi, line, src, clipped: true, splits: Vec::new() });
                }
            }
        }
        Ok(())
    }

    /// Exact side of node `n` with respect to the line of segment `i` (the sign convention is
    /// fixed per segment, which is all the crossing and touching tests need).
    fn side(&self, i: usize, n: usize) -> i32 {
        let ctx = self.ctx;
        match self.segs[i].line {
            Line::Edge { s, e } => {
                let ed = &ctx.p[s].edges[e];
                ctx.orient(ctx.p[s].vnode[ed.a], ctx.p[s].vnode[ed.b], n, self.ax)
            }
            Line::Plane { g } => ctx.plane_side(1 - self.x, g, n),
        }
    }

    /// Whether node `n` (known to be on the line of segment `i`) lies strictly between its
    /// endpoints.
    fn in_range(&self, i: usize, n: usize) -> bool {
        let (a, b) = (self.segs[i].a, self.segs[i].b);
        if n == a || n == b {
            return false;
        }
        match self.ctx.sep_axis(a, b, Some(self.ax)) {
            Some(k) => self.ctx.cmp(n, a, k) * self.ctx.cmp(n, b, k) < 0,
            None => false,
        }
    }

    /// The node where segments `i` and `j` (which cross strictly) meet, from the symbolic
    /// identity of their lines, or from their 2D crossing when that is not available.
    fn cross_node(&self, i: usize, j: usize) -> Option<usize> {
        let ctx = self.ctx;
        let (x, f) = (self.x, self.f);
        let y = 1 - x;
        let edge_pierce = |s: usize, e: usize, t: usize, g: usize| -> Option<usize> {
            let ed = &ctx.p[s].edges[e];
            let (na, nb) = (ctx.p[s].vnode[ed.a], ctx.p[s].vnode[ed.b]);
            if ctx.plane_side(t, g, na) * ctx.plane_side(t, g, nb) < 0 {
                ctx.pierce(s, e, t, g)
            } else {
                None
            }
        };
        let symbolic = match (self.segs[i].line, self.segs[j].line) {
            (Line::Edge { s: s1, e: e1 }, Line::Edge { s: s2, e: e2 }) => {
                let mut r = None;
                for &phi in &ctx.p[s1].edges[e1].faces {
                    r = r.or_else(|| edge_pierce(s2, e2, s1, phi));
                }
                for &phi in &ctx.p[s2].edges[e2].faces {
                    r = r.or_else(|| edge_pierce(s1, e1, s2, phi));
                }
                r
            }
            (Line::Edge { s, e }, Line::Plane { g }) | (Line::Plane { g }, Line::Edge { s, e }) => edge_pierce(s, e, y, g),
            (Line::Plane { g: g1 }, Line::Plane { g: g2 }) => {
                if g1 == g2 {
                    None
                } else {
                    ctx.triple([(x, f), (y, g1), (y, g2)])
                }
            }
        };
        if symbolic.is_some() {
            return symbolic;
        }
        let (a1, b1, a2, b2) = (self.segs[i].a, self.segs[i].b, self.segs[j].a, self.segs[j].b);
        let p = {
            let n = ctx.nodes.borrow();
            HPoint::cross_in_plane(&n.pts[a1], &n.pts[b1], &n.pts[a2], &n.pts[b2], self.ax)
        };
        p.map(|p| ctx.nodes.borrow_mut().intern(p))
    }

    /// Resolve all crossings and touchings, split the segments, and return the cells inside
    /// the face.
    fn cells(mut self) -> Result<Vec<Fragment>, GeomError> {
        let ctx = self.ctx;
        let ax = self.ax;
        let n = self.segs.len();
        for i in 0..n {
            for j in i + 1..n {
                let (a1, b1, a2, b2) = (self.segs[i].a, self.segs[i].b, self.segs[j].a, self.segs[j].b);
                let shared = a1 == a2 || a1 == b2 || b1 == a2 || b1 == b2;
                if !shared {
                    let (s_a2, s_b2) = (self.side(i, a2), self.side(i, b2));
                    if s_a2 * s_b2 < 0 {
                        let (s_a1, s_b1) = (self.side(j, a1), self.side(j, b1));
                        if s_a1 * s_b1 < 0 {
                            let k = self.cross_node(i, j).ok_or_else(|| GeomError::Degenerate("segments cross without a computable point".into()))?;
                            self.segs[i].splits.push(k);
                            self.segs[j].splits.push(k);
                            continue;
                        }
                    }
                }
                // Touchings: an endpoint of one segment on the other.
                for &(seg, pt) in &[(i, a2), (i, b2), (j, a1), (j, b1)] {
                    if self.in_range(seg, pt) && self.side(seg, pt) == 0 {
                        self.segs[seg].splits.push(pt);
                    }
                }
            }
        }
        // Collinear segments that overlap share their split points: a node splitting one of
        // them (a vertex of the other solid on it, a piercing next to a resolved vertex) lies
        // on the other as well, and both must be cut there for their pieces to coincide.
        let splits: Vec<Vec<usize>> = self.segs.iter().map(|s| s.splits.clone()).collect();
        for i in 0..n {
            for j in 0..n {
                if i == j || splits[i].is_empty() {
                    continue;
                }
                let (a2, b2) = (self.segs[j].a, self.segs[j].b);
                if self.side(i, a2) != 0 || self.side(i, b2) != 0 {
                    continue;
                }
                for &k in &splits[i] {
                    if self.in_range(j, k) {
                        self.segs[j].splits.push(k);
                    }
                }
            }
        }

        // Split the segments into sub-edges, deduplicated by node pair.
        let mut edges: BTreeMap<(usize, usize), SubEdge> = BTreeMap::new();
        for s in &self.segs {
            let mut mids: Vec<usize> = s.splits.iter().copied().filter(|&k| k != s.a && k != s.b).collect();
            mids.sort_unstable();
            mids.dedup();
            if mids.len() > 1 {
                let k = ctx.sep_axis(s.a, s.b, Some(ax)).ok_or_else(|| GeomError::Degenerate("zero-length segment".into()))?;
                let forward = ctx.cmp(s.b, s.a, k) > 0;
                mids.sort_by(|&p, &q| {
                    let c = ctx.cmp(p, q, k);
                    let c = if forward { c } else { -c };
                    c.cmp(&0).then(p.cmp(&q))
                });
            }
            let mut chain = vec![s.a];
            chain.extend(mids);
            chain.push(s.b);
            for w in chain.windows(2) {
                if w[0] == w[1] {
                    continue;
                }
                let key = (w[0].min(w[1]), w[0].max(w[1]));
                let se = edges.entry(key).or_default();
                if !se.srcs.contains(&s.src) {
                    se.srcs.push(s.src);
                }
                se.clipped |= s.clipped;
            }
        }

        // Prune dangling non-boundary pieces.
        loop {
            let mut degree: HashMap<usize, usize> = HashMap::new();
            for &(u, v) in edges.keys() {
                *degree.entry(u).or_insert(0) += 1;
                *degree.entry(v).or_insert(0) += 1;
            }
            let dangling: Vec<(usize, usize)> = edges
                .iter()
                .filter(|(&(u, v), se)| !se.srcs.iter().any(|s| matches!(s, Src::Bnd(_))) && (degree[&u] == 1 || degree[&v] == 1))
                .map(|(&k, _)| k)
                .collect();
            if dangling.is_empty() {
                break;
            }
            for k in dangling {
                edges.remove(&k);
            }
        }

        // Adjacency sorted counter-clockwise (exactly).
        let mut adj: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &(u, v) in edges.keys() {
            adj.entry(u).or_default().push(v);
            adj.entry(v).or_default().push(u);
        }
        for (&u, nb) in adj.iter_mut() {
            nb.sort_by(|&a, &b| angular_cmp(ctx, u, a, b, ax));
        }

        // Trace the cycles: after `u -> v`, continue with the neighbour of `v` just clockwise
        // of `u`, which keeps the cell on the left.
        let mut visited: HashSet<(usize, usize)> = HashSet::new();
        let mut cycles: Vec<Vec<usize>> = Vec::new();
        let mut starts: Vec<(usize, usize)> = edges.keys().flat_map(|&(u, v)| [(u, v), (v, u)]).collect();
        starts.sort();
        for start in starts {
            if visited.contains(&start) {
                continue;
            }
            let mut cycle = Vec::new();
            let (mut u, mut v) = start;
            loop {
                visited.insert((u, v));
                cycle.push(u);
                let nb = &adj[&v];
                let k = nb.iter().position(|&w| w == u).unwrap();
                let w = nb[(k + nb.len() - 1) % nb.len()];
                u = v;
                v = w;
                if (u, v) == start || cycle.len() > edges.len() * 2 + 2 {
                    break;
                }
            }
            if cycle.len() >= 2 {
                cycles.push(cycle);
            }
        }

        // Orientation of every cycle, from the turn at its lexicographically smallest node.
        let orientation = |c: &[usize]| -> i32 {
            let n = c.len();
            if n < 3 {
                return 0;
            }
            let mut m = 0;
            for i in 1..n {
                let cu = ctx.cmp(c[i], c[m], ax.0);
                if cu < 0 || (cu == 0 && ctx.cmp(c[i], c[m], ax.1) < 0) {
                    m = i;
                }
            }
            ctx.orient(c[(m + n - 1) % n], c[m], c[(m + 1) % n], ax)
        };
        let mut positive: Vec<(Vec<usize>, Vec<Vec<usize>>)> = Vec::new();
        let mut negative: Vec<Vec<usize>> = Vec::new();
        for c in cycles {
            match orientation(&c) {
                o if o > 0 => positive.push((c, Vec::new())),
                o if o < 0 => negative.push(c),
                _ => {}
            }
        }
        // Assign hole cycles to the innermost positive cycle strictly containing them.
        for hole in negative {
            let mut cands: Vec<usize> = Vec::new();
            for (pi, (outer, _)) in positive.iter().enumerate() {
                if let Some(&v) = hole.iter().find(|v| !outer.contains(v)) {
                    if ring_contains(ctx, outer, v, ax) {
                        cands.push(pi);
                    }
                }
            }
            if cands.is_empty() {
                continue;
            }
            let mut best = cands[0];
            for &c in &cands[1..] {
                // `c` is inner to `best` when a node of `c` lies inside `best`.
                let (oc, ob) = (&positive[c].0, &positive[best].0);
                let inner = match oc.iter().find(|v| !ob.contains(v)) {
                    Some(&v) => ring_contains(ctx, ob, v, ax),
                    None => ring_area(ctx, oc, ax) < ring_area(ctx, ob, ax),
                };
                if inner {
                    best = c;
                }
            }
            positive[best].1.push(hole);
        }

        let p = &ctx.p[self.x];
        let f = self.f;
        let mut out = Vec::new();
        for (outer, holes) in positive {
            let mut pieces = Vec::new();
            for c in std::iter::once(&outer).chain(holes.iter()) {
                for i in 0..c.len() {
                    let (a, b) = (c[i], c[(i + 1) % c.len()]);
                    let se = &edges[&(a.min(b), a.max(b))];
                    pieces.push(Piece { a, b, srcs: se.srcs.clone(), clipped: se.clipped });
                }
            }
            // Is the cell inside the face?
            let mut inside: Option<bool> = None;
            for pc in &pieces {
                if let Some(e) = pc.srcs.iter().find_map(|s| if let Src::Bnd(e) = s { Some(*e) } else { None }) {
                    let (u, v) = p.edge_dir_in(f, e);
                    let d = ctx.dir_along(pc.a, pc.b, p.vnode[u], p.vnode[v]);
                    if d != 0 {
                        inside = Some(d > 0);
                        break;
                    }
                }
            }
            if inside.is_none() && pieces.iter().any(|pc| pc.clipped) {
                inside = Some(true);
            }
            if inside.is_none() {
                let m = {
                    let n = ctx.nodes.borrow();
                    HPoint::along(&n.pts[pieces[0].a], &n.pts[pieces[0].b], 0.5)
                };
                inside = m.map(|m| matches!(locate_in_face(ctx, self.x, f, &m), Locate::Inside));
            }
            if inside != Some(true) {
                continue;
            }
            let mut loops = vec![outer];
            loops.extend(holes);
            out.push(Fragment { loops, pieces });
        }
        Ok(out)
    }
}

/// Exact counter-clockwise angular order of nodes `a` and `b` around `o` (angles in `[0, 2π)`).
fn angular_cmp(ctx: &Ctx, o: usize, a: usize, b: usize, ax: (usize, usize)) -> std::cmp::Ordering {
    let half = |p: usize| {
        let dy = ctx.cmp(p, o, ax.1);
        if dy > 0 || (dy == 0 && ctx.cmp(p, o, ax.0) > 0) {
            0
        } else {
            1
        }
    };
    let (ha, hb) = (half(a), half(b));
    if ha != hb {
        return ha.cmp(&hb);
    }
    match ctx.orient(o, a, b, ax) {
        o2 if o2 > 0 => std::cmp::Ordering::Less,
        o2 if o2 < 0 => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    }
}

/// Whether node `v` (not on the ring) lies inside the ring of nodes, by the exact ray rule.
fn ring_contains(ctx: &Ctx, ring: &[usize], v: usize, ax: (usize, usize)) -> bool {
    let n = ring.len();
    let mut count = 0;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let (ay, by) = (ctx.cmp(a, v, ax.1) > 0, ctx.cmp(b, v, ax.1) > 0);
        if ay != by {
            let o = ctx.orient(a, b, v, ax);
            if (by && o > 0) || (!by && o < 0) {
                count += 1;
            }
        }
    }
    count % 2 == 1
}

fn ring_area(ctx: &Ctx, ring: &[usize], ax: (usize, usize)) -> f64 {
    let pts: Vec<Point2> = ring.iter().map(|&i| project(ctx.rounded(i), ax)).collect();
    crate::polygon::signed_area(&pts)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Locate {
    Inside,
    Outside,
    OnBoundary,
}

/// Exact location of the point `m` (in the plane of face `f` of solid `x`) with respect to the
/// face polygon.
fn locate_in_face(ctx: &Ctx, x: usize, f: usize, m: &HPoint) -> Locate {
    let p = &ctx.p[x];
    let ax = p.axes(f);
    let nodes = ctx.nodes.borrow();
    let mut count = 0;
    for l in &p.loops[f] {
        let n = l.len();
        for i in 0..n {
            let (a, b) = (&nodes.pts[p.vnode[l[i]]], &nodes.pts[p.vnode[l[(i + 1) % n]]]);
            let o = orient2d_h(a, b, m, ax);
            if o == 0 {
                // On the line: on the edge when within its range along a separating axis.
                for k in [ax.0, ax.1] {
                    let c = cmp_coord(a, b, k);
                    if c != 0 {
                        if cmp_coord(m, a, k) * cmp_coord(m, b, k) <= 0 {
                            return Locate::OnBoundary;
                        }
                        break;
                    }
                }
            }
            let (ay, by) = (cmp_coord(a, m, ax.1) > 0, cmp_coord(b, m, ax.1) > 0);
            if ay != by && ((by && o > 0) || (!by && o < 0)) {
                count += 1;
            }
        }
    }
    if count % 2 == 1 {
        Locate::Inside
    } else {
        Locate::Outside
    }
}

// ---------------------------------------------------------------------------------------------
// Classification

impl Ctx {
    /// Classification of a fragment of face `f` of solid `x` from one of its boundary pieces
    /// that lies on the surface of the other solid, by an exact angular sweep around the
    /// piece's line.
    ///
    /// Every face of the other solid through that line is a half-plane leaving the line in
    /// some direction, with the other solid's material on one rotational side of it: a face
    /// bounded by an edge `e` along the line (the piece carries `Src::Edge(e)`) has its
    /// material clockwise of itself (looking along the piece) when its half-edge runs with
    /// the piece and counter-clockwise when it runs against it; a face that passes straight
    /// through the line (the piece lies in its interior, `Src::Face(g)`) contributes two
    /// opposite half-planes with its material on its inner side. Going around the line, the
    /// wedges between consecutive half-planes alternate material / empty, whatever the number
    /// of sheets through the line (a manifold edge is two half-planes, an edge where two sheets
    /// pinch is four, a crease inside a face is two plus the face). The fragment's own
    /// in-plane direction falls into one wedge, whose status is read off the half-planes next
    /// to it; when it coincides with a half-plane the fragment lies on that face. All
    /// comparisons are signs of exact triple products.
    ///
    /// `None` when the piece carries no half-plane, or when the neighbouring half-planes
    /// disagree (an inconsistent input), in which case the caller falls back to a ray cast.
    fn wedge_class(&self, x: usize, f: usize, pc: &Piece) -> Option<Class> {
        use crate::exact::{cross3, dot3, hdiff, sign, Ex};
        let y = 1 - x;
        let p = &self.p[x];
        let q = &self.p[y];
        let nodes = self.nodes.borrow();
        // Direction of the piece, from its exact end points (positive denominators dropped).
        let w: [Ex; 3] = hdiff(&nodes.pts[pc.b], &nodes.pts[pc.a]);
        // Half-planes: (in-plane direction, material counter-clockwise of it, face).
        let mut planes: Vec<([Ex; 3], bool, usize)> = Vec::new();
        for src in &pc.srcs {
            match *src {
                Src::Edge(e) => {
                    let ed = &q.edges[e];
                    for &g in &ed.faces {
                        let (u, v) = q.edge_dir_in(g, e);
                        let t = hdiff(&nodes.pts[q.vnode[v]], &nodes.pts[q.vnode[u]]);
                        let along = sign(&dot3(&w, &t));
                        if along == 0 {
                            return None;
                        }
                        planes.push((cross3(&q.planes[g].n, &t), along < 0, g));
                    }
                }
                Src::Face(g) => {
                    let d = cross3(&q.planes[g].n, &w);
                    planes.push(([d[0].neg(), d[1].neg(), d[2].neg()], true, g));
                    planes.push((d, false, g));
                }
                Src::Bnd(_) | Src::Own(_) => {}
            }
        }
        if planes.is_empty() {
            return None;
        }
        // The fragment lies to the left of the piece: its direction is n_f × w.
        let dp = cross3(&p.planes[f].n, &w);
        // Angular position of a direction relative to `dp`, counter-clockwise around `w`:
        // 0 = same direction, 1 = in (0, π), 2 = opposite, 3 = in (π, 2π).
        let pos = |d: &[Ex; 3]| -> u8 {
            match sign(&dot3(&w, &cross3(&dp, d))) {
                s if s > 0 => 1,
                s if s < 0 => 3,
                _ => {
                    if sign(&dot3(&dp, d)) > 0 {
                        0
                    } else {
                        2
                    }
                }
            }
        };
        let positions: Vec<u8> = planes.iter().map(|(d, _, _)| pos(d)).collect();
        if let Some(i) = positions.iter().position(|&c| c == 0) {
            // The fragment continues face `g` of the other solid: on it.
            let g = planes[i].2;
            let (nf, ng) = (p.planes[f].n_approx, q.planes[g].n_approx);
            return Some(Class::On(nf[0] * ng[0] + nf[1] * ng[1] + nf[2] * ng[2] > 0.0));
        }
        let before = |i: usize, j: usize| -> bool {
            // Whether half-plane `i` comes before `j` counter-clockwise from `dp`.
            if positions[i] != positions[j] {
                return positions[i] < positions[j];
            }
            positions[i] != 2 && sign(&dot3(&w, &cross3(&planes[i].0, &planes[j].0))) > 0
        };
        let (mut next, mut prev) = (0, 0);
        for i in 1..planes.len() {
            if before(i, next) {
                next = i;
            }
            if before(prev, i) {
                prev = i;
            }
        }
        // `next` is the first half-plane counter-clockwise of the fragment: the fragment is
        // in the material iff that half-plane's material is clockwise of it. `prev` must agree.
        let inside = !planes[next].1;
        if planes[prev].1 != inside {
            return None;
        }
        Some(if inside { Class::Inside } else { Class::Outside })
    }

    fn classify(&self, x: usize, f: usize, frag: &Fragment) -> Result<Class, GeomError> {
        let y = 1 - x;
        let q = &self.p[y];
        // 1. On a coplanar face of the other solid?
        for g in 0..q.nf() {
            if let Some(same) = self.coplanar(x, f, g) {
                if self.on_coplanar(x, frag, g, same) {
                    return Ok(Class::On(same));
                }
            }
        }
        // 2. Local structure of the other solid along a piece lying on its surface.
        for pc in &frag.pieces {
            if pc.srcs.iter().any(|s| matches!(s, Src::Edge(_) | Src::Face(_))) {
                if let Some(c) = self.wedge_class(x, f, pc) {
                    return Ok(c);
                }
            }
        }
        // 3. Ray cast from a point of a boundary piece that is off the other solid's surface.
        for pc in &frag.pieces {
            for t in [0.5, 0.25, 0.75, 0.125, 0.375, 0.625, 0.875] {
                let m = {
                    let n = self.nodes.borrow();
                    HPoint::along(&n.pts[pc.a], &n.pts[pc.b], t)
                };
                let Some(m) = m else { continue };
                if self.on_surface(y, &m) {
                    continue;
                }
                if let Some(w) = self.winding(y, &m) {
                    return Ok(if w != 0 { Class::Inside } else { Class::Outside });
                }
            }
        }
        Err(GeomError::Degenerate("fragment could not be classified".into()))
    }

    /// Whether the fragment lies on the coplanar face `g` of the other solid.
    fn on_coplanar(&self, x: usize, frag: &Fragment, g: usize, same: bool) -> bool {
        let y = 1 - x;
        let q = &self.p[y];
        for pc in &frag.pieces {
            for src in &pc.srcs {
                if let Src::Edge(e) = *src {
                    if q.face_edges[g].contains(&e) {
                        // The fragment is left of `a -> b`; `g` is left of its own edge direction
                        // (in the projection, when the orientations agree).
                        let (u, v) = q.edge_dir_in(g, e);
                        let d = self.dir_along(pc.a, pc.b, q.vnode[u], q.vnode[v]);
                        if d != 0 {
                            return (d > 0) == same;
                        }
                    }
                }
            }
        }
        for pc in &frag.pieces {
            let m = {
                let n = self.nodes.borrow();
                HPoint::along(&n.pts[pc.a], &n.pts[pc.b], 0.5)
            };
            if let Some(m) = m {
                match locate_in_face(self, y, g, &m) {
                    Locate::Inside => return true,
                    Locate::Outside => return false,
                    Locate::OnBoundary => continue,
                }
            }
        }
        false
    }

    /// Whether the point lies on the surface of solid `t`.
    fn on_surface(&self, t: usize, m: &HPoint) -> bool {
        let q = &self.p[t];
        for g in 0..q.nf() {
            if plane_sign(&q.planes[g], m) == 0 && locate_in_face(self, t, g, m) != Locate::Outside {
                return true;
            }
        }
        false
    }

    /// Winding number of solid `t` around a point off its surface: the signed count of the
    /// faces crossed by the ray towards `+x`, with the ray's projection perturbed symbolically
    /// by `(ε, ε²)` in `(y, z)` so that edges and vertices on it are counted consistently.
    /// Whether the ray passes through a face is decided by the parity of the crossings of the
    /// face's projected loops with the ray from the perturbed projection towards `+z`. `None`
    /// when the point turns out to lie on a face.
    fn winding(&self, t: usize, m: &HPoint) -> Option<i32> {
        let q = &self.p[t];
        let nodes = self.nodes.borrow();
        let ax = (1, 2);
        let mut total = 0;
        for g in 0..q.nf() {
            let o = crate::exact::sign(&q.planes[g].n[0]);
            if o == 0 {
                continue; // edge-on face
            }
            let (mut miny, mut maxy, mut minz, mut maxz) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
            for &v in q.loops[g].iter().flatten() {
                let c = q.vr[v];
                miny = miny.min(c.y);
                maxy = maxy.max(c.y);
                minz = minz.min(c.z);
                maxz = maxz.max(c.z);
            }
            if m.iv[1].hi < miny || m.iv[1].lo > maxy || m.iv[2].hi < minz || m.iv[2].lo > maxz {
                continue;
            }
            let mut crossings = 0;
            for l in &q.loops[g] {
                let n = l.len();
                for i in 0..n {
                    let (u, v) = (&nodes.pts[q.vnode[l[i]]], &nodes.pts[q.vnode[l[(i + 1) % n]]]);
                    // The edge straddles the perturbed y (a vertex at m's y counts as below).
                    let (uy, vy) = (cmp_coord(u, m, 1) > 0, cmp_coord(v, m, 1) > 0);
                    if uy == vy {
                        continue;
                    }
                    // Is the edge above the perturbed point at that y? When the edge runs
                    // towards +y, the point is below it iff it is to its left.
                    let mut above = orient2d_h(u, v, m, ax) * if vy { -1 } else { 1 };
                    if above == 0 {
                        // On the edge's line: at y + ε the edge is at z + ε dz/dy, which is
                        // above z + ε² iff dz/dy > 0.
                        above = cmp_coord(v, u, 2) * cmp_coord(v, u, 1);
                    }
                    if above > 0 {
                        crossings += 1;
                    }
                }
            }
            if crossings % 2 == 0 {
                continue;
            }
            let ps = plane_sign(&q.planes[g], m);
            if ps == 0 {
                return None;
            }
            if ps * o > 0 {
                total += o;
            }
        }
        Some(total)
    }
}

// ---------------------------------------------------------------------------------------------
// Selection

/// Whether a fragment of solid `x` with the given classification is part of the result, and
/// whether its orientation must be reversed.
fn select(op: BoolOp, x: usize, class: Class) -> Option<bool> {
    match (op, class) {
        (BoolOp::Union, Class::Outside) => Some(false),
        (BoolOp::Union, Class::On(true)) if x == 0 => Some(false),
        (BoolOp::Intersection, Class::Inside) => Some(false),
        (BoolOp::Intersection, Class::On(true)) if x == 0 => Some(false),
        (BoolOp::Difference, Class::Outside) if x == 0 => Some(false),
        (BoolOp::Difference, Class::On(false)) if x == 0 => Some(false),
        (BoolOp::Difference, Class::Inside) if x == 1 => Some(true),
        _ => None,
    }
}

struct Kept {
    solid: usize,
    face: usize,
    loops: Vec<Vec<usize>>,
    flip: bool,
}

// ---------------------------------------------------------------------------------------------
// Assembly

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn unite(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra] = rb;
    }
}

/// Exact collinearity of three points.
fn collinear(a: Point3, b: Point3, c: Point3) -> bool {
    [(0, 1), (1, 2), (2, 0)].iter().all(|&(u, v)| {
        let ax = (u, v);
        orient2d(project(a, ax), project(b, ax), project(c, ax)) == 0.0
    })
}

fn newell(loop_pts: &[Point3]) -> Vec3 {
    let o = loop_pts[0];
    let mut n = Vec3::ZERO;
    for i in 1..loop_pts.len().saturating_sub(1) {
        n += (loop_pts[i] - o).cross(loop_pts[i + 1] - o);
    }
    n
}

fn assemble(ctx: &Ctx, kept: Vec<Kept>, scale: f64) -> Result<Solid, GeomError> {
    if kept.is_empty() {
        return Ok(Solid::new());
    }
    // Whether two sub-faces belong to one face of the result (either orientation): pieces of
    // one input face; two planar faces sharing a cluster plane; or, when an input face is a
    // fallback face (split into triangles), the two whole input faces lie within `1e-10` of
    // the inputs' size of each other's plane. The triangles of a fallback face are never
    // compared on their own: such a triangle can be a sliver whose exact plane is unrelated
    // to its face's (three nearly collinear vertices), and merging it would drag its whole
    // face into another plane.
    let near_tol = 1e-10 * ctx.scale;
    let within = |p: &Prep, face: usize, origin: Point3, n: Vec3| -> bool {
        p.face_loops[face].iter().flatten().all(|&v| (p.vr[v] - origin).dot(n).abs() <= near_tol)
    };
    let same_plane = |x: usize, f: usize, y: usize, g: usize| -> bool {
        let (p, q) = (&ctx.p[x], &ctx.p[y]);
        if x == y && p.parent[f] == q.parent[g] {
            return true;
        }
        match (p.cluster[f], q.cluster[g]) {
            (Some((cf, _)), Some((cg, _))) => cf == cg,
            _ => match (p.face_plane[p.parent[f]], q.face_plane[q.parent[g]]) {
                (Some((o1, n1)), Some((o2, n2))) => within(q, q.parent[g], o1, n1) && within(p, p.parent[f], o2, n2),
                _ => false,
            },
        }
    };
    let normal_of = |x: usize, f: usize| {
        let n = ctx.p[x].planes[f].n_approx;
        Vec3::new(n[0], n[1], n[2])
    };

    // ---- vertices: the rounded coordinates of the shared nodes.
    let points: Vec<Point3> = ctx.nodes.borrow().pts.iter().map(|p| p.r).collect();
    struct Frag {
        loops: Vec<Vec<usize>>,
        solid: usize,
        face: usize,
        /// Outward normal of the fragment (the sub-face normal, reversed when flipped).
        normal: Vec3,
    }
    let mut frags: Vec<Frag> = Vec::new();
    for k in &kept {
        let mut normal = normal_of(k.solid, k.face);
        let mut loops: Vec<Vec<usize>> = k.loops.clone();
        for l in loops.iter_mut() {
            l.dedup();
            while l.len() > 1 && l[0] == *l.last().unwrap() {
                l.pop();
            }
        }
        loops.retain(|l| l.len() >= 3);
        if loops.is_empty() {
            continue;
        }
        if k.flip {
            normal = -normal;
            for l in loops.iter_mut() {
                l.reverse();
            }
        }
        frags.push(Frag { loops, solid: k.solid, face: k.face, normal });
    }

    // ---- half-edge structure of the (unmerged) fragments, with angular twin pairing and
    // vertex splitting. Fragment `i` is face `i` of this structure.
    let mut vertices: Vec<Vertex> = points.iter().map(|&p| Vertex { point: p, half_edge: None }).collect();
    let mut half_edges: Vec<HalfEdge> = Vec::new();
    let mut face_normals: Vec<Vec3> = Vec::new();
    for (face_id, fr) in frags.iter().enumerate() {
        for l in &fr.loops {
            let base = half_edges.len();
            let n = l.len();
            for (k, &v) in l.iter().enumerate() {
                half_edges.push(HalfEdge { origin: v, twin: None, next: base + (k + 1) % n, prev: base + (k + n - 1) % n, face: face_id });
            }
        }
        // The exact plane's normal (a sliver fragment's rounded loop would give a garbage one).
        face_normals.push(fr.normal);
    }
    let mut by_edge: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (i, h) in half_edges.iter().enumerate() {
        let (a, b) = (h.origin, half_edges[h.next].origin);
        if a == b {
            return Err(GeomError::Degenerate("zero-length edge in result".into()));
        }
        by_edge.entry((a.min(b), a.max(b))).or_default().push(i);
    }
    // Pinch edges (several sheets through one edge) can be paired two ways: each sheet bounds
    // one of the material wedges at the edge, or one of the empty wedges. Which one yields a
    // manifold depends on the edge's end vertices (whether the material or the empty space is
    // connected around them), so start with the material pairing and switch an edge to the
    // other pairing when its end vertices come out with a double edge.
    let mut empty_pairing: HashSet<(usize, usize)> = HashSet::new();
    let base_half_edges = half_edges.clone();
    let base_vertices = vertices.clone();
    let mut node_of: Vec<usize>;
    let mut rounds = 0;
    loop {
        half_edges = base_half_edges.clone();
        vertices = base_vertices.clone();
        node_of = (0..points.len()).collect();
        pair_half_edges(&mut half_edges, &by_edge, &points, &face_normals, &empty_pairing)?;
        split_umbrellas(&mut half_edges, &mut vertices, &mut node_of, &points);
        // Double edges: the same directed vertex pair used twice.
        let mut seen: HashMap<(usize, usize), usize> = HashMap::new();
        let mut doubled: Vec<(usize, usize)> = Vec::new();
        for (i, h) in half_edges.iter().enumerate() {
            let key = (h.origin, half_edges[h.next].origin);
            if seen.insert(key, i).is_some() {
                let (a, b) = (base_half_edges[i].origin, base_half_edges[base_half_edges[i].next].origin);
                doubled.push((a.min(b), a.max(b)));
            }
        }
        if doubled.is_empty() {
            break;
        }
        rounds += 1;
        let mut changed = false;
        for key in doubled {
            if by_edge.get(&key).map_or(false, |hs| hs.len() > 2) && empty_pairing.insert(key) {
                changed = true;
            }
        }
        if !changed || rounds > by_edge.len() + 1 {
            return Err(GeomError::Degenerate("result is not a closed manifold (pinched edge cannot be separated)".into()));
        }
    }

    // ---- merge coplanar fragments that are twins across an edge into maximal faces. Only
    // twins are merged: two coplanar fragments meeting along a pinch edge whose sheets were
    // paired apart stay separate faces, so a face is never glued across a crease, and the
    // face list matches `Solid::face_count` (maximal edge-connected coplanar regions).
    let nfr = frags.len();
    let mut fparent: Vec<usize> = (0..nfr).collect();
    for h in &half_edges {
        let t = h.twin.ok_or_else(|| GeomError::Degenerate("unpaired half-edge".into()))?;
        let (f, g) = (h.face, half_edges[t].face);
        if f != g && frags[f].normal.dot(frags[g].normal) > 0.0 && same_plane(frags[f].solid, frags[f].face, frags[g].solid, frags[g].face) {
            unite(&mut fparent, f, g);
        }
    }
    let group_of: Vec<usize> = (0..nfr).map(|i| find(&mut fparent, i)).collect();
    // Boundary loops of every group, traced through the half-edge structure (the group is on
    // the left, so outer loops come out counter-clockwise and holes clockwise).
    let mut group_loops: BTreeMap<usize, Vec<Vec<usize>>> = BTreeMap::new();
    let mut visited = vec![false; half_edges.len()];
    let across = |h: usize, half_edges: &[HalfEdge]| group_of[half_edges[half_edges[h].twin.unwrap()].face];
    for h0 in 0..half_edges.len() {
        let g = group_of[half_edges[h0].face];
        if visited[h0] || across(h0, &half_edges) == g {
            continue;
        }
        let mut lp = Vec::new();
        let mut h = h0;
        loop {
            visited[h] = true;
            lp.push(half_edges[h].origin);
            // Rotate around the end vertex until the next boundary half-edge of the group.
            let mut n = half_edges[h].next;
            let mut guard = 0;
            while across(n, &half_edges) == g {
                n = half_edges[half_edges[n].twin.unwrap()].next;
                guard += 1;
                if guard > half_edges.len() {
                    return Err(GeomError::Degenerate("face merge did not converge".into()));
                }
            }
            h = n;
            if h == h0 {
                break;
            }
            if lp.len() > half_edges.len() {
                return Err(GeomError::Degenerate("face merge did not converge".into()));
            }
        }
        if lp.len() >= 3 {
            group_loops.entry(g).or_default().push(lp);
        }
    }
    let mut faces: Vec<Vec<Vec<usize>>> = Vec::new(); // loops of vertex ids, outer first
    let mut face_planes: Vec<&Plane> = Vec::new(); // exact plane of every entry of `faces`
    for (&g, loops) in &group_loops {
        let plane = &ctx.p[frags[g].solid].planes[frags[g].face];
        let n = frags[g].normal;
        let ax = axes_of([n.x, n.y, n.z]);
        if loops.len() == 1 {
            faces.push(loops.clone());
            face_planes.push(plane);
            continue;
        }
        // Exact orientation of each loop from the turn at its lexicographically smallest node.
        let orientation = |l: &[usize]| -> i32 {
            let c: Vec<usize> = l.iter().map(|&v| node_of[v]).collect();
            let n = c.len();
            let mut m = 0;
            for i in 1..n {
                let cu = ctx.cmp(c[i], c[m], ax.0);
                if cu < 0 || (cu == 0 && ctx.cmp(c[i], c[m], ax.1) < 0) {
                    m = i;
                }
            }
            let mut o = ctx.orient(c[(m + n - 1) % n], c[m], c[(m + 1) % n], ax);
            if o == 0 {
                let pts: Vec<Point2> = c.iter().map(|&i| project(points[i], ax)).collect();
                o = crate::polygon::signed_area(&pts).partial_cmp(&0.0).map_or(0, |ord| ord as i32);
            }
            o
        };
        let mut outers: Vec<(Vec<usize>, Vec<Vec<usize>>)> = Vec::new();
        let mut holes: Vec<Vec<usize>> = Vec::new();
        for l in loops {
            if orientation(l) > 0 {
                outers.push((l.clone(), Vec::new()));
            } else {
                holes.push(l.clone());
            }
        }
        if outers.is_empty() {
            return Err(GeomError::Degenerate("merged face without an outer loop".into()));
        }
        for h in holes {
            if outers.len() == 1 {
                outers[0].1.push(h);
                continue;
            }
            // Several outer loops in one group (only through a vertex shared by two sheets):
            // put the hole into the innermost outer loop containing it.
            let mut best: Option<(usize, f64)> = None;
            for (k, (o, _)) in outers.iter().enumerate() {
                let ring: Vec<usize> = o.iter().map(|&v| node_of[v]).collect();
                let inside = h.iter().map(|&v| node_of[v]).find(|v| !ring.contains(v)).map(|v| ring_contains(ctx, &ring, v, ax));
                let a = crate::polygon::signed_area(&o.iter().map(|&i| project(points[node_of[i]], ax)).collect::<Vec<_>>());
                if inside == Some(true) && best.map_or(true, |(_, ba)| a < ba) {
                    best = Some((k, a));
                }
            }
            if let Some((k, _)) = best {
                outers[k].1.push(h);
            }
        }
        for (o, hs) in outers {
            let mut ls = vec![o];
            ls.extend(hs);
            faces.push(ls);
            face_planes.push(plane);
        }
    }
    // Resolved vertices (see `Prep`) leave edges of rounding length, which round to coincident
    // or adjacent doubles: contract them. Only edges are contracted, so the vertices of
    // separate sheets touching at one point stay separate.
    let faces = collapse_short_edges(faces, &vertices, &face_planes, COLLAPSE_TOL * ctx.scale);
    // Contacts of rounding size (a vertex next to another vertex or next to an edge of a
    // different sheet, left by resolved vertices) are welded: the same coordinates, with
    // separate records, as for an exact contact, so that the result read back as an input
    // has an exact pinch there rather than a gap or a crossing of rounding size.
    let faces = weld_near_contacts(faces, &mut vertices, &face_planes, COLLAPSE_TOL * ctx.scale);
    let solid = Solid::from_faces(vertices.iter().map(|v| v.point).collect(), faces.clone());
    let solid = drop_collinear_vertices(&solid);
    if !solid.is_closed_manifold() {
        return Err(GeomError::Degenerate("result is not a closed manifold".into()));
    }
    validate(&solid)?;
    let vol = solid.volume();
    if vol.abs() <= 1e-12 * scale {
        return Ok(Solid::new());
    }
    Ok(solid)
}

/// Contract every edge shorter than `tol` (its end vertices become one), then drop the
/// vertices repeated consecutively in a loop, the spikes (`a, b, a`) and the loops left with
/// fewer than three vertices; a face whose outer loop collapsed is dropped. A loop of a
/// closed manifold stays matched: a dropped digon's two edges were twins of two half-edges
/// that now pair with each other.
fn collapse_short_edges(faces: Vec<Vec<Vec<usize>>>, vertices: &[Vertex], face_planes: &[&Plane], tol: f64) -> Vec<Vec<Vec<usize>>> {
    let mut parent: Vec<usize> = (0..vertices.len()).collect();
    let mut any = false;
    for l in faces.iter().flatten() {
        for i in 0..l.len() {
            let (u, v) = (l[i], l[(i + 1) % l.len()]);
            if u != v && (vertices[u].point - vertices[v].point).norm() <= tol {
                unite(&mut parent, u, v);
                any = true;
            }
        }
    }
    if !any {
        return faces;
    }
    // The vertex that survives a contraction is the one whose rounded point lies exactly on
    // the most planes of the faces around it (the point of a resolved vertex with the exact
    // coordinates of its axis-aligned planes rather than one an ulp off), then the one
    // sharing its coordinates with the most other records (where a contracted vertex marks
    // the contact with another sheet, the contact stays exact in coordinates).
    let scores = exactness(&faces, vertices, face_planes);
    let score = |v: usize| scores[&v];
    let mut rep: HashMap<usize, usize> = HashMap::new();
    for l in faces.iter().flatten() {
        for &v in l {
            let r = find(&mut parent, v);
            let e = rep.entry(r).or_insert(v);
            if score(v) > score(*e) || (score(v) == score(*e) && v < *e) {
                *e = v;
            }
        }
    }
    let mut out = Vec::new();
    for ls in faces {
        let mut nl: Vec<Vec<usize>> = Vec::new();
        for (li, l) in ls.into_iter().enumerate() {
            let mut m: Vec<usize> = l.iter().map(|&v| rep[&find(&mut parent, v)]).collect();
            loop {
                let before = m.len();
                m.dedup();
                while m.len() > 1 && m[0] == *m.last().unwrap() {
                    m.pop();
                }
                let n = m.len();
                if n >= 3 {
                    if let Some(i) = (0..n).find(|&i| m[(i + n - 1) % n] == m[(i + 1) % n]) {
                        m.remove(i);
                    }
                }
                if m.len() == before {
                    break;
                }
            }
            if m.len() < 3 {
                if li == 0 {
                    nl.clear();
                    break;
                }
                continue;
            }
            nl.push(m);
        }
        if !nl.is_empty() {
            out.push(nl);
        }
    }
    out
}

/// Exactness of every vertex used by the loops: how many planes of the faces around it its
/// rounded point lies on exactly, and how many records share its coordinates.
fn exactness(faces: &[Vec<Vec<usize>>], vertices: &[Vertex], face_planes: &[&Plane]) -> HashMap<usize, (usize, usize)> {
    let mut at_point: HashMap<[u64; 3], usize> = HashMap::new();
    let mut on_planes: HashMap<usize, usize> = HashMap::new();
    for (ls, &pl) in faces.iter().zip(face_planes) {
        for &v in ls.iter().flatten() {
            *at_point.entry(key_of(vertices[v].point)).or_insert(0) += 1;
            let e = on_planes.entry(v).or_insert(0);
            if plane_sign(pl, &HPoint::input(vertices[v].point)) == 0 {
                *e += 1;
            }
        }
    }
    on_planes.iter().map(|(&v, &on)| (v, (on, at_point[&key_of(vertices[v].point)]))).collect()
}

/// Weld the contacts of rounding size (see `assemble`): vertices within `tol` of each other
/// take the coordinates of the most exact one; a vertex within `tol` of the inside of an
/// edge it does not belong to splits that edge, in both faces at it, with a new record at
/// the vertex's coordinates.
fn weld_near_contacts(faces: Vec<Vec<Vec<usize>>>, vertices: &mut Vec<Vertex>, face_planes: &[&Plane], tol: f64) -> Vec<Vec<Vec<usize>>> {
    // Welding only ever moves points by `tol` and adds records; it is kept only when the
    // welded loops still form a closed manifold, otherwise the loops are left as they are.
    let before = (faces.clone(), vertices.clone());
    let welded = weld_near_contacts_inner(faces, vertices, face_planes, tol);
    let t = Solid::from_faces(vertices.iter().map(|v| v.point).collect(), welded.clone());
    if t.faces.len() == welded.len() && t.is_closed_manifold() {
        welded
    } else {
        *vertices = before.1;
        before.0
    }
}

fn weld_near_contacts_inner(faces: Vec<Vec<Vec<usize>>>, vertices: &mut Vec<Vertex>, face_planes: &[&Plane], tol: f64) -> Vec<Vec<Vec<usize>>> {
    let score = exactness(&faces, vertices, face_planes);
    let mut used: Vec<usize> = score.keys().copied().collect();
    used.sort_unstable();
    // Vertices within `tol` of each other: one point for the whole group.
    let mut parent: Vec<usize> = (0..vertices.len()).collect();
    for (i, &u) in used.iter().enumerate() {
        for &v in &used[i + 1..] {
            if (vertices[u].point - vertices[v].point).norm() <= tol {
                unite(&mut parent, u, v);
            }
        }
    }
    let mut best: HashMap<usize, usize> = HashMap::new();
    for &v in &used {
        let r = find(&mut parent, v);
        let e = best.entry(r).or_insert(v);
        if score[&v] > score[e] || (score[&v] == score[e] && v < *e) {
            *e = v;
        }
    }
    for &v in &used {
        let b = best[&find(&mut parent, v)];
        vertices[v].point = vertices[b].point;
    }
    // Vertices inside an edge of another sheet: split the edge there.
    let mut edges: Vec<(usize, usize)> = faces.iter().flatten().flat_map(|l| (0..l.len()).map(move |i| (l[i].min(l[(i + 1) % l.len()]), l[i].max(l[(i + 1) % l.len()])))).collect();
    edges.sort_unstable();
    edges.dedup();
    let mut splits: HashMap<(usize, usize), Vec<(f64, usize)>> = HashMap::new(); // edge -> (parameter, new record)
    for &(a, b) in &edges {
        let (pa, pb) = (vertices[a].point, vertices[b].point);
        let d = pb - pa;
        let len2 = d.norm2();
        if len2 == 0.0 {
            continue;
        }
        for &v in &used {
            if v == a || v == b {
                continue;
            }
            let p = vertices[v].point;
            if key_of(p) == key_of(pa) || key_of(p) == key_of(pb) {
                continue;
            }
            let t = (p - pa).dot(d) / len2;
            if t <= 0.0 || t >= 1.0 || (p - (pa + d * t)).norm() > tol {
                continue;
            }
            let entry = splits.entry((a, b)).or_default();
            if entry.iter().any(|&(_, w)| key_of(vertices[w].point) == key_of(p)) {
                continue;
            }
            vertices.push(Vertex { point: p, half_edge: None });
            entry.push((t, vertices.len() - 1));
        }
    }
    for e in splits.values_mut() {
        e.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
    }
    faces
        .into_iter()
        .map(|ls| {
            ls.into_iter()
                .map(|l| {
                    let mut out = Vec::with_capacity(l.len());
                    for i in 0..l.len() {
                        let (a, b) = (l[i], l[(i + 1) % l.len()]);
                        out.push(a);
                        if let Some(sp) = splits.get(&(a.min(b), a.max(b))) {
                            if a < b {
                                out.extend(sp.iter().map(|&(_, w)| w));
                            } else {
                                out.extend(sp.iter().rev().map(|&(_, w)| w));
                            }
                        }
                    }
                    out
                })
                .collect()
        })
        .collect()
}

/// Pair every half-edge with its twin. Edges with two half-edges pair directly; edges with
/// more (sheets touching along the edge) are paired by the angular order of their faces around
/// the edge: each forward half-edge takes the next backward one clockwise (the sheet then
/// bounds a material wedge) or counter-clockwise for the edges in `empty_pairing` (the sheet
/// then bounds an empty wedge).
fn pair_half_edges(
    half_edges: &mut [HalfEdge],
    by_edge: &BTreeMap<(usize, usize), Vec<usize>>,
    points: &[Point3],
    face_normals: &[Vec3],
    empty_pairing: &HashSet<(usize, usize)>,
) -> Result<(), GeomError> {
    for (&(a, b), hs) in by_edge {
        if hs.len() == 2 {
            let (h0, h1) = (hs[0], hs[1]);
            if half_edges[h0].origin == half_edges[h1].origin {
                return Err(GeomError::Degenerate("inconsistently oriented result edge".into()));
            }
            half_edges[h0].twin = Some(h1);
            half_edges[h1].twin = Some(h0);
            continue;
        }
        if hs.len() % 2 != 0 {
            return Err(GeomError::Degenerate(format!("result edge with {} half-edges: {:?} -> {:?}", hs.len(), points[a], points[b])));
        }
        let u = (points[b] - points[a]).normalized().ok_or_else(|| GeomError::Degenerate("zero-length edge".into()))?;
        let r1 = {
            let t = if u.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            t.cross(u).normalized().ok_or_else(|| GeomError::Degenerate("degenerate edge frame".into()))?
        };
        let r2 = u.cross(r1);
        let mut ordered: Vec<(f64, usize, bool)> = hs
            .iter()
            .map(|&h| {
                let forward = half_edges[h].origin == a;
                let n = face_normals[half_edges[h].face];
                let d = n.cross(if forward { u } else { -u });
                ((d.dot(r2)).atan2(d.dot(r1)), h, forward)
            })
            .collect();
        ordered.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
        let m = ordered.len();
        let step = if empty_pairing.contains(&(a, b)) { 1 } else { m - 1 };
        for i in 0..m {
            let (_, h, forward) = ordered[i];
            if !forward {
                continue;
            }
            // Material of a forward half-edge lies clockwise (decreasing angle) from its face.
            let mut j = (i + step) % m;
            let mut found = None;
            for _ in 0..m {
                if !ordered[j].2 {
                    found = Some(ordered[j].1);
                    break;
                }
                j = (j + step) % m;
            }
            let t = found.ok_or_else(|| GeomError::Degenerate("unpaired half-edge".into()))?;
            if half_edges[t].twin.is_some() {
                return Err(GeomError::Degenerate("conflicting sheet pairing".into()));
            }
            half_edges[h].twin = Some(t);
            half_edges[t].twin = Some(h);
        }
    }
    if half_edges.iter().any(|h| h.twin.is_none()) {
        return Err(GeomError::Degenerate("result is not closed".into()));
    }
    Ok(())
}

/// Split every vertex into one copy per umbrella (fan of faces connected through twins).
/// `node_of` maps every vertex (copies included) to its node.
fn split_umbrellas(half_edges: &mut [HalfEdge], vertices: &mut Vec<Vertex>, node_of: &mut Vec<usize>, points: &[Point3]) {
    let mut by_origin: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, h) in half_edges.iter().enumerate() {
        by_origin.entry(h.origin).or_default().push(i);
    }
    for (&v, hs) in &by_origin {
        let mut seen: HashSet<usize> = HashSet::new();
        let mut first = true;
        for &h0 in hs {
            if seen.contains(&h0) {
                continue;
            }
            let target = if first {
                first = false;
                v
            } else {
                vertices.push(Vertex { point: points[v], half_edge: None });
                node_of.push(v);
                vertices.len() - 1
            };
            let mut h = h0;
            loop {
                seen.insert(h);
                half_edges[h].origin = target;
                h = half_edges[half_edges[h].twin.unwrap()].next;
                if h == h0 || seen.contains(&h) {
                    break;
                }
            }
            vertices[target].half_edge = Some(h0);
        }
    }
}

/// Geometric validity of an assembled result: faces planar (to rounding), no zero-area face,
/// and watertight once vertices are merged by coordinates (a T-junction would make the STL
/// non-watertight).
fn validate(s: &Solid) -> Result<(), GeomError> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &s.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(coord(v.point, k));
            hi[k] = hi[k].max(coord(v.point, k));
        }
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    if !diag.is_finite() {
        return Err(GeomError::Degenerate("non-finite result coordinates".into()));
    }
    for f in 0..s.faces.len() {
        let n = match s.face_normal(f).normalized() {
            Some(n) => n,
            None => return Err(GeomError::Degenerate("result face without area".into())),
        };
        let o = s.vertices[s.half_edges[s.faces[f].loops[0]].origin].point;
        for &h in &s.faces[f].loops {
            for v in s.loop_vertices(h) {
                let dev = (s.vertices[v].point - o).dot(n).abs();
                if dev > 1e-9 * diag {
                    return Err(GeomError::Degenerate(format!("result face is not planar (deviation {dev:e}, diagonal {diag:e}, normal {n:?})")));
                }
            }
        }
    }
    // Watertightness by coordinates: with the vertices merged by position, every directed edge
    // must still be matched by its reverse. Separate sheets touching along part of an edge pass
    // (both sheets carry their own edge pairs); a T-junction inside one sheet does not.
    let mut directed: HashMap<([u64; 3], [u64; 3]), i64> = HashMap::new();
    for h in &s.half_edges {
        let (a, b) = (key_of(s.vertices[h.origin].point), key_of(s.vertices[s.half_edges[h.next].origin].point));
        *directed.entry((a, b)).or_insert(0) += 1;
    }
    for (&(a, b), &c) in &directed {
        if directed.get(&(b, a)).copied().unwrap_or(0) != c {
            return Err(GeomError::Degenerate(format!(
                "result is not watertight by coordinates (T-junction) at edge {:?} -> {:?}",
                Point3::new(f64::from_bits(a[0]), f64::from_bits(a[1]), f64::from_bits(a[2])),
                Point3::new(f64::from_bits(b[0]), f64::from_bits(b[1]), f64::from_bits(b[2]))
            )));
        }
    }
    Ok(())
}

/// Rebuild a solid without the vertices that have exactly two incident edges and lie on the
/// line through their two neighbours (left behind when fragments merge). Vertices are judged
/// after the umbrella split, so a vertex shared by two sheets that is redundant for one sheet
/// only is dropped from that sheet alone. Collinearity is exact, or within `1e-13` of the
/// solid's diagonal: a chain of nodes along the intersection of two slightly non-planar input
/// faces (split into planar triangles for the computation) bends by rounding only, and
/// keeping it would leave sliver ears in the triangulation of both faces.
fn drop_collinear_vertices(s: &Solid) -> Solid {
    let loops: Vec<Vec<Vec<usize>>> = s.faces.iter().map(|f| f.loops.iter().map(|&h| s.loop_vertices(h)).collect()).collect();
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in s.vertices.iter().filter(|v| v.half_edge.is_some()) {
        for k in 0..3 {
            lo[k] = lo[k].min(coord(v.point, k));
            hi[k] = hi[k].max(coord(v.point, k));
        }
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    let nearly_collinear = |p: Point3, v: Point3, q: Point3| -> bool {
        if collinear(p, v, q) {
            return true;
        }
        let d = q - p;
        let len = d.norm();
        if len == 0.0 {
            return false;
        }
        let w = v - p;
        let t = w.dot(d) / (len * len);
        if !(0.0..=1.0).contains(&t) {
            return false;
        }
        (w - d * t).norm() <= 1e-13 * diag
    };
    let mut neighbours: HashMap<usize, HashSet<usize>> = HashMap::new();
    for ls in &loops {
        for l in ls {
            for j in 0..l.len() {
                let (u, v) = (l[j], l[(j + 1) % l.len()]);
                neighbours.entry(u).or_default().insert(v);
                neighbours.entry(v).or_default().insert(u);
            }
        }
    }
    let removable: HashSet<usize> = neighbours
        .iter()
        .filter(|(&v, nb)| {
            if nb.len() != 2 {
                return false;
            }
            let mut it = nb.iter();
            let (&p, &q) = (it.next().unwrap(), it.next().unwrap());
            nearly_collinear(s.vertices[p].point, s.vertices[v].point, s.vertices[q].point)
        })
        .map(|(&v, _)| v)
        .collect();
    // Compact the vertex list to the ones still used.
    let mut remap: HashMap<usize, usize> = HashMap::new();
    let mut points: Vec<Point3> = Vec::new();
    let mut faces: Vec<Vec<Vec<usize>>> = Vec::new();
    for ls in &loops {
        let mut out = Vec::new();
        for l in ls {
            let kept: Vec<usize> = l
                .iter()
                .filter(|v| !removable.contains(v))
                .map(|&v| {
                    *remap.entry(v).or_insert_with(|| {
                        points.push(s.vertices[v].point);
                        points.len() - 1
                    })
                })
                .collect();
            if kept.len() >= 3 {
                out.push(kept);
            } else if out.is_empty() {
                break; // outer loop collapsed: drop the face
            }
        }
        if !out.is_empty() {
            faces.push(out);
        }
    }
    Solid::from_faces(points, faces)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::polygon::Polygon;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    fn boxed(mn: [f64; 3], mx: [f64; 3]) -> Solid {
        let poly = Polygon::new(vec![
            Point2::new(mn[0], mn[1]),
            Point2::new(mx[0], mn[1]),
            Point2::new(mx[0], mx[1]),
            Point2::new(mn[0], mx[1]),
        ]);
        Solid::extrude(&poly, mn[2], mx[2] - mn[2])
    }

    /// Runs the four operations and checks closedness and the inclusion–exclusion identities
    /// `|a ∪ b| + |a ∩ b| = |a| + |b|`, `|a − b| + |a ∩ b| = |a|`, `|b − a| + |a ∩ b| = |b|`.
    fn check_all(a: &Solid, b: &Solid) -> [Solid; 4] {
        let u = boolean(a, b, BoolOp::Union).unwrap();
        let d = boolean(a, b, BoolOp::Difference).unwrap();
        let e = boolean(b, a, BoolOp::Difference).unwrap();
        let i = boolean(a, b, BoolOp::Intersection).unwrap();
        for s in [&u, &d, &e, &i] {
            assert!(s.faces.is_empty() || s.is_closed_manifold(), "result not closed");
            assert!(s.faces.is_empty() || s.volume() > 0.0, "non-empty result with no volume");
            assert_eq!(s.faces.is_empty(), s.face_count() == 0);
        }
        assert!(close(u.volume() + i.volume(), a.volume() + b.volume()), "union {} inter {}", u.volume(), i.volume());
        assert!(close(d.volume() + i.volume(), a.volume()));
        assert!(close(e.volume() + i.volume(), b.volume()));
        [u, d, e, i]
    }

    #[test]
    fn identical_boxes() {
        let a = boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let [u, d, e, i] = check_all(&a, &a.clone());
        assert!(close(u.volume(), 1.0) && u.face_count() == 6);
        assert!(close(i.volume(), 1.0) && i.face_count() == 6);
        assert!(d.faces.is_empty() && e.faces.is_empty());
        assert_eq!(d.volume(), 0.0);
        let mut buf = Vec::new();
        d.write_stl(&mut buf).unwrap();
        assert!(!String::from_utf8(buf).unwrap().contains("facet"));
    }

    #[test]
    fn stacked_and_coplanar_sides() {
        // Two unit cubes sharing a face: the union is a 1x1x2 box with six faces.
        let a = boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = boxed([0.0, 0.0, 1.0], [1.0, 1.0, 2.0]);
        let [u, d, e, i] = check_all(&a, &b);
        assert!(close(u.volume(), 2.0));
        assert_eq!(u.face_count(), 6);
        assert_eq!(u.vertex_count(), 8);
        assert_eq!(u.euler_characteristic(), 2);
        assert!(close(d.volume(), 1.0) && d.face_count() == 6);
        assert!(close(e.volume(), 1.0) && e.face_count() == 6);
        assert!(i.faces.is_empty());
        // Overlapping along x with four coplanar side faces.
        let c = boxed([0.5, 0.0, 0.0], [1.5, 1.0, 1.0]);
        let [u, d, _, i] = check_all(&a, &c);
        assert!(close(u.volume(), 1.5) && u.face_count() == 6);
        assert!(close(d.volume(), 0.5) && d.face_count() == 6);
        assert!(close(i.volume(), 0.5) && i.face_count() == 6);
    }

    #[test]
    fn touching_edge_and_vertex() {
        let a = boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        for b in [boxed([1.0, 1.0, 0.0], [2.0, 2.0, 1.0]), boxed([1.0, 1.0, 1.0], [2.0, 2.0, 2.0])] {
            let [u, d, e, i] = check_all(&a, &b);
            // Two separate sheets: closed, twelve faces, sixteen vertices (the contact is not welded).
            assert!(close(u.volume(), 2.0));
            assert_eq!(u.face_count(), 12);
            assert_eq!(u.vertex_count(), 16);
            assert!(u.is_closed_manifold());
            assert!(close(d.volume(), 1.0) && close(e.volume(), 1.0));
            assert!(i.faces.is_empty());
        }
        // Boxes meeting only in part of an edge: the union keeps each box's edge whole.
        let b = boxed([0.5, 1.0, 1.0], [1.5, 2.0, 2.0]);
        let [u, ..] = check_all(&a, &b);
        assert_eq!(u.face_count(), 12);
        assert_eq!(u.vertex_count(), 16);
    }

    #[test]
    fn contained_tunnel_and_notch() {
        let big = boxed([0.0, 0.0, 0.0], [3.0, 3.0, 3.0]);
        let small = boxed([1.0, 1.0, 1.0], [2.0, 2.0, 2.0]);
        let [u, d, e, i] = check_all(&big, &small);
        assert!(close(u.volume(), 27.0) && u.face_count() == 6);
        assert!(close(d.volume(), 26.0) && d.face_count() == 12); // a cavity
        assert!(e.faces.is_empty());
        assert!(close(i.volume(), 1.0) && i.face_count() == 6);

        let cube = boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let bar = boxed([0.25, 0.25, -1.0], [0.75, 0.75, 2.0]);
        let [u, d, e, i] = check_all(&cube, &bar);
        assert!(close(u.volume(), 1.5) && u.face_count() == 16);
        // The cube with a square tunnel: caps with inner loops, genus 1.
        assert!(close(d.volume(), 0.75));
        assert_eq!(d.face_count(), 10);
        assert_eq!(d.euler_characteristic(), 2); // V - E + F with two inner rings
        assert!(close(e.volume(), 0.5) && e.face_count() == 12); // two separate stubs
        assert!(close(i.volume(), 0.25) && i.face_count() == 6);

        let corner = boxed([0.5, 0.5, 0.5], [2.0, 2.0, 2.0]);
        let [u, d, e, i] = check_all(&cube, &corner);
        assert!(close(u.volume(), 4.25) && u.face_count() == 12);
        assert!(close(d.volume(), 0.875) && d.face_count() == 9);
        assert!(close(e.volume(), 3.25) && e.face_count() == 9);
        assert!(close(i.volume(), 0.125) && i.face_count() == 6);
    }

    #[test]
    fn holed_prism_against_bar() {
        let frame = Polygon::with_holes(
            vec![Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), Point2::new(4.0, 4.0), Point2::new(0.0, 4.0)],
            vec![vec![Point2::new(1.0, 1.0), Point2::new(1.0, 3.0), Point2::new(3.0, 3.0), Point2::new(3.0, 1.0)]],
        );
        let a = Solid::extrude(&frame, 0.0, 1.0);
        let b = boxed([-1.0, 1.5, 0.25], [5.0, 2.5, 0.75]);
        let [u, d, e, i] = check_all(&a, &b);
        assert!(close(a.volume(), 12.0));
        assert!(close(i.volume(), 2.0 * 0.5));
        assert!(close(u.volume(), 12.0 + 3.0 - 1.0));
        assert!(close(d.volume(), 11.0));
        assert!(close(e.volume(), 2.0));
        assert_eq!(e.face_count(), 18); // three separate pieces: outside, in the hole, outside
    }

    #[test]
    fn prism_and_sweep_against_boxes() {
        let star: Vec<Point2> = (0..9)
            .map(|k| {
                let a = 2.0 * std::f64::consts::PI * k as f64 / 9.0 + 0.3;
                let r = if k % 2 == 0 { 1.1 } else { 0.7 };
                Point2::new(1.0 + r * a.cos(), 1.0 + r * a.sin())
            })
            .collect();
        let prism = Solid::extrude(&Polygon::new(star), 0.5, 1.0);
        let b = boxed([0.5, 0.0, 0.0], [1.5, 2.0, 1.5]);
        let [u, d, e, i] = check_all(&prism, &b);
        for s in [&u, &d, &e, &i] {
            assert!(s.is_closed_manifold());
            assert_eq!(s.face_count(), s.faces.len(), "faces are maximal");
        }
        // Mitred sweep through a box.
        let rect = Polygon::new(vec![Point2::new(-0.25, -0.15), Point2::new(0.25, -0.15), Point2::new(0.25, 0.15), Point2::new(-0.25, 0.15)]);
        let sweep = Solid::sweep(&rect, &[Point3::new(0.0, 0.5, 0.5), Point3::new(2.5, 0.5, 0.5), Point3::new(2.5, 3.0, 1.0)]);
        let b = boxed([1.0, 0.0, 0.0], [3.0, 1.0, 1.0]);
        let [u, d, e, i] = check_all(&sweep, &b);
        assert!(i.volume() > 0.0 && d.volume() > 0.0 && e.volume() > 0.0 && u.volume() > 0.0);
    }

    #[test]
    fn empty_and_invalid_inputs() {
        let a = boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let empty = Solid::new();
        assert_eq!(boolean(&a, &empty, BoolOp::Union).unwrap(), a);
        assert_eq!(boolean(&empty, &a, BoolOp::Union).unwrap(), a);
        assert!(boolean(&a, &empty, BoolOp::Intersection).unwrap().faces.is_empty());
        assert_eq!(boolean(&a, &empty, BoolOp::Difference).unwrap(), a);
        assert!(boolean(&empty, &a, BoolOp::Difference).unwrap().faces.is_empty());
        // Disjoint solids.
        let far = boxed([3.0, 3.0, 3.0], [4.0, 4.0, 4.0]);
        let [u, d, _, i] = check_all(&a, &far);
        assert_eq!(u.face_count(), 12);
        assert!(close(d.volume(), 1.0) && i.faces.is_empty());
        // An open mesh is rejected.
        let pts = vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0), Point3::new(0.0, 0.0, 1.0)];
        let open = Solid::from_faces(pts, vec![vec![vec![0, 2, 1]], vec![vec![0, 1, 3]]]);
        assert!(matches!(boolean(&open, &a, BoolOp::Union), Err(GeomError::NotClosed("a"))));
        assert!(matches!(boolean(&a, &open, BoolOp::Union), Err(GeomError::NotClosed("b"))));
    }

    #[test]
    fn face_count_merges_coplanar_regions() {
        // A unit cube whose top is stored as two coplanar quads counts six faces.
        let p = |x: f64, y: f64, z: f64| Point3::new(x, y, z);
        let pts = vec![
            p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(1.0, 1.0, 0.0), p(0.0, 1.0, 0.0),
            p(0.0, 0.0, 1.0), p(1.0, 0.0, 1.0), p(1.0, 1.0, 1.0), p(0.0, 1.0, 1.0),
            p(0.5, 0.0, 1.0), p(0.5, 1.0, 1.0),
        ];
        let faces = vec![
            vec![vec![0, 3, 2, 1]],
            vec![vec![4, 8, 9, 7]],
            vec![vec![8, 5, 6, 9]],
            vec![vec![0, 1, 5, 8, 4]],
            vec![vec![1, 2, 6, 5]],
            vec![vec![2, 3, 7, 9, 6]],
            vec![vec![3, 0, 4, 7]],
        ];
        let s = Solid::from_faces(pts, faces);
        assert!(s.is_closed_manifold());
        assert_eq!(s.faces.len(), 7);
        assert_eq!(s.face_count(), 6);
        assert_eq!(s.euler_characteristic(), 2);
    }
}


/// Regression cases from the T2.2 audit (minimal reproducers of the failures of the first
/// implementation), with manifold3d's volumes.
#[cfg(test)]
mod audit_cases {
    use super::*;
    use crate::polygon::Polygon;

    fn p2(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn prism(outer: &[(f64, f64)], holes: &[&[(f64, f64)]], z0: f64, h: f64) -> Solid {
        let ring = |r: &[(f64, f64)]| r.iter().map(|&(x, y)| p2(x, y)).collect::<Vec<_>>();
        Solid::extrude(&Polygon::with_holes(ring(outer), holes.iter().map(|h| ring(h)).collect()), z0, h)
    }

    fn boxed(mn: [f64; 3], mx: [f64; 3]) -> Solid {
        prism(&[(mn[0], mn[1]), (mx[0], mn[1]), (mx[0], mx[1]), (mn[0], mx[1])], &[], mn[2], mx[2] - mn[2])
    }

    fn sweep(outer: &[(f64, f64)], path: &[[f64; 3]]) -> Solid {
        let poly = Polygon::new(outer.iter().map(|&(x, y)| p2(x, y)).collect());
        Solid::sweep(&poly, &path.iter().map(|p| Point3::new(p[0], p[1], p[2])).collect::<Vec<_>>())
    }

    /// Runs the four operations; every result must be `Ok`, closed (or empty) and have the
    /// oracle's volume within `1e-9` relative; `faces` are the oracle's region counts (`None`
    /// where the oracle's count is an artefact of tolerance merging).
    fn check(a: &Solid, b: &Solid, vols: [f64; 4], faces: [Option<usize>; 4]) {
        let results = [
            boolean(a, b, BoolOp::Union),
            boolean(a, b, BoolOp::Difference),
            boolean(b, a, BoolOp::Difference),
            boolean(a, b, BoolOp::Intersection),
        ];
        for (k, r) in results.iter().enumerate() {
            let s = r.as_ref().unwrap_or_else(|e| panic!("op {k}: {e:?}"));
            let want = vols[k];
            if want == 0.0 {
                assert!(s.faces.is_empty(), "op {k}: expected empty, got volume {}", s.volume());
                continue;
            }
            assert!(s.is_closed_manifold(), "op {k}: not closed");
            assert!((s.volume() - want).abs() <= 1e-9 * want, "op {k}: volume {} vs {want}", s.volume());
            if let Some(f) = faces[k] {
                assert_eq!(s.face_count(), f, "op {k}: face count");
            }
            let mut buf = Vec::new();
            s.write_stl(&mut buf).unwrap();
        }
    }

    #[test]
    fn exact_l_prism_vs_box_false_edge_crossing() {
        let l = prism(&[(1.5, 1.0), (2.5, 1.0), (2.5, 2.0), (2.0, 2.0), (2.0, 4.0), (1.5, 4.0)], &[], 0.5, 3.0);
        let b = boxed([0.5, 2.0, 2.0], [1.5, 2.5, 4.5]);
        check(&l, &b, [7.25, 6.0, 1.25, 0.0], [Some(14), Some(8), Some(6), None]);
    }

    #[test]
    fn decimal_boxes_edge_through_vertex() {
        let a = boxed([0.6, 1.3, 0.0], [1.9, 3.1, 1.0]);
        let b = boxed([1.9, 1.4, 0.0], [2.6, 3.0, 1.0]);
        check(&a, &b, [3.46, 2.34, 1.12, 0.0], [Some(10), Some(6), Some(6), None]);
    }

    #[test]
    fn sweep_edge_through_mitre_vertex() {
        let s = sweep(&[(-0.25, -0.15), (0.25, -0.15), (0.25, 0.15), (-0.25, 0.15)], &[[0.0, 0.5, 0.5], [2.5, 0.5, 0.5], [3.0, 3.0, 0.5]]);
        let b = boxed([1.0, 0.5, 0.5], [2.5, 1.0, 2.5]);
        check(&s, &b, [2.1934790372962767, 0.6934790372962766, 1.436052573776819, 0.06394742622318103], [Some(14), Some(13), Some(9), Some(8)]);
    }

    #[test]
    fn side_by_side_sweeps_share_a_face() {
        let path = [[0.0, 0.5, 0.5], [2.0, 0.5, 0.5], [2.5, 3.0, 1.0]];
        let a = sweep(&[(-0.25, -0.15), (0.25, -0.15), (0.25, 0.15), (-0.25, 0.15)], &path);
        let b = sweep(&[(0.25, -0.15), (0.75, -0.15), (0.75, 0.15), (0.25, 0.15)], &path);
        // Mitred quads are not exactly planar, so the union keeps the quads of both sweeps as
        // separate faces (the oracle merges them within its tolerance).
        check(&a, &b, [1.4036314350754564, 0.6897114317029974, 0.7139200033724588, 0.0], [None, Some(10), Some(10), None]);
    }

    #[test]
    fn exact_holed_prisms_stacked() {
        let a = prism(&[(2.0, 2.0), (5.0, 2.0), (5.0, 3.5), (2.0, 3.5)], &[&[(2.5, 2.5), (2.5, 3.0), (4.5, 3.0), (4.5, 2.5)]], 3.5, 2.0);
        let b = prism(
            &[(2.0, 0.0), (5.0, 0.0), (5.0, 3.0), (2.0, 3.0)],
            &[&[(3.5, 1.0), (3.5, 1.5), (4.0, 1.5), (4.0, 1.0)], &[(4.0, 2.0), (4.0, 2.5), (4.5, 2.5), (4.5, 2.0)]],
            0.5,
            3.5,
        );
        check(&a, &b, [35.875, 6.125, 28.875, 0.875], [Some(24), Some(15), Some(19), Some(14)]);
    }

    #[test]
    fn decimal_l_vs_box_one_ulp_apart() {
        // The box's top plane y = 1.2000000000000002 and the L's y = 1.2 are distinct planes:
        // the exact union has a sliver face between them (the oracle merges it away).
        let l = prism(&[(0.5, 0.8), (1.0, 0.8), (1.0, 1.2), (0.8, 1.2), (0.8, 1.4), (0.5, 1.4)], &[], 0.7, 0.6);
        let b = boxed([0.3, 0.4, 0.4], [1.2, 1.2000000000000002, 0.7]);
        check(&l, &b, [0.372, 0.156, 0.216, 0.0], [None, Some(8), Some(6), None]);
    }

    #[test]
    fn pinched_chained_result_vs_tunnel() {
        let big = boxed([0.0, 0.0, 0.0], [3.0, 3.0, 3.0]);
        let tunnel = boxed([1.0, 1.0, -1.0], [2.0, 2.0, 4.0]);
        let corner = boxed([2.0, 2.0, 0.0], [3.0, 3.0, 3.0]);
        let tunneled = boolean(&big, &tunnel, BoolOp::Difference).unwrap();
        let pinched = boolean(&tunneled, &corner, BoolOp::Difference).unwrap();
        assert!(pinched.is_closed_manifold());
        check(&pinched, &tunnel, [26.0, 21.0, 5.0, 0.0], [Some(18), Some(12), Some(6), None]);
    }

    #[test]
    fn stacked_holed_prisms_material_connected_around_pinch_vertex() {
        // A's holes are covered by B: the pinch edges where A's top meets B's bottom must be
        // paired by the empty wedges for the result to be a manifold.
        let a = prism(&[(0.5, 2.0), (3.5, 2.0), (3.5, 3.5), (0.5, 3.5)], &[&[(2.5, 2.5), (2.5, 3.0), (3.0, 3.0), (3.0, 2.5)], &[(1.0, 2.5), (1.0, 3.0), (1.5, 3.0), (1.5, 2.5)]], 0.5, 1.5);
        let b = prism(&[(0.5, 0.0), (4.5, 0.0), (4.5, 3.0), (0.5, 3.0)], &[&[(3.0, 0.5), (3.0, 1.0), (4.0, 1.0), (4.0, 0.5)], &[(3.5, 2.0), (3.5, 2.5), (4.0, 2.5), (4.0, 2.0)]], 2.0, 1.5);
        check(&a, &b, [22.875, 6.0, 16.875, 0.0], [Some(28), Some(14), Some(14), None]);
    }

    #[test]
    fn diamond_vertex_touching_box_edge() {
        let d = prism(&[(2.5, 2.5), (1.5, 3.5), (0.5, 2.5), (1.5, 1.5)], &[], 2.0, 2.5);
        let b = boxed([0.0, 2.0, 1.0], [0.5, 3.5, 2.0]);
        check(&d, &b, [5.75, 5.0, 0.75, 0.0], [Some(12), Some(6), Some(6), None]);
    }
}

/// Pinched results (sheets touching along a line or at a point) and their use as inputs to
/// further booleans, with manifold3d's volumes and region counts.
#[cfg(test)]
mod pinch_cases {
    use super::*;
    use crate::polygon::Polygon;

    fn p2(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    fn boxed(mn: [f64; 3], mx: [f64; 3]) -> Solid {
        let poly = Polygon::new(vec![p2(mn[0], mn[1]), p2(mx[0], mn[1]), p2(mx[0], mx[1]), p2(mn[0], mx[1])]);
        Solid::extrude(&poly, mn[2], mx[2] - mn[2])
    }

    /// A square column rotated by 45°, centred at `(cx, cy)` with half-diagonal `r`.
    fn diamond(cx: f64, cy: f64, r: f64, z0: f64, h: f64) -> Solid {
        Solid::extrude(&Polygon::new(vec![p2(cx + r, cy), p2(cx, cy + r), p2(cx - r, cy), p2(cx, cy - r)]), z0, h)
    }

    /// Runs the four operations: every result is `Ok`, closed or empty, has as many faces as
    /// maximal coplanar regions, and the volumes satisfy inclusion–exclusion.
    fn ops(a: &Solid, b: &Solid) -> [Solid; 4] {
        let u = boolean(a, b, BoolOp::Union).unwrap();
        let d = boolean(a, b, BoolOp::Difference).unwrap();
        let e = boolean(b, a, BoolOp::Difference).unwrap();
        let i = boolean(a, b, BoolOp::Intersection).unwrap();
        for s in [&u, &d, &e, &i] {
            assert!(s.faces.is_empty() || s.is_closed_manifold(), "result not closed");
            assert_eq!(s.face_count(), s.faces.len(), "faces are maximal coplanar regions");
        }
        assert!(close(u.volume() + i.volume(), a.volume() + b.volume()));
        assert!(close(d.volume() + i.volume(), a.volume()));
        assert!(close(e.volume() + i.volume(), b.volume()));
        [u, d, e, i]
    }

    fn faces_with_normal(s: &Solid, n: Vec3) -> usize {
        (0..s.faces.len()).filter(|&f| s.face_normal(f).normalized().map_or(false, |m| m.dot(n) > 0.999)).count()
    }

    #[test]
    fn diamond_column_with_an_edge_in_a_box_face() {
        // The diamond's vertical edge at (1, 0.5) lies inside the box face x = 1: the box minus
        // the diamond has that face split in two halves meeting only along the crease (the
        // sheets are paired apart, never glued): 2 + 1 + 1 + 3 + 4 = 11 faces, as manifold3d.
        let unit = boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let d = diamond(0.75, 0.5, 0.25, -1.0, 3.0);
        let [u, d1, e, i] = ops(&unit, &d);
        assert!(close(d1.volume(), 0.875));
        assert_eq!(d1.face_count(), 11);
        assert_eq!(faces_with_normal(&d1, Vec3::X), 2);
        assert_eq!(u.face_count(), 16);
        assert_eq!(e.face_count(), 12);
        assert_eq!(i.face_count(), 6);
        // The same with the diamond exactly as tall as the box (coplanar caps).
        let [_, d2, e2, _] = ops(&unit, &diamond(0.75, 0.5, 0.25, 0.0, 1.0));
        assert_eq!(d2.face_count(), 11);
        assert!(e2.faces.is_empty());
        // Shorter than the box: the crease ends inside the face, which stays one region with
        // the cavity touching it along a slit (a chord).
        let [_, d3, _, _] = ops(&unit, &diamond(0.75, 0.5, 0.25, 0.25, 0.5));
        assert_eq!(d3.face_count(), 12);
        assert!(close(d3.volume(), 0.9375));
        // Touching the face from outside: two separate sheets in the union.
        let [u4, d4, _, i4] = ops(&unit, &diamond(1.25, 0.5, 0.25, -1.0, 3.0));
        assert_eq!(u4.face_count(), 12);
        assert_eq!(d4.face_count(), 6);
        assert!(i4.faces.is_empty());
        // Touching all four faces: the difference is four separate triangular prisms.
        let [u5, d5, _, _] = ops(&unit, &diamond(0.5, 0.5, 0.5, -1.0, 3.0));
        assert_eq!(d5.face_count(), 20);
        assert!(close(d5.volume(), 0.5));
        assert_eq!(u5.face_count(), 22);
    }

    #[test]
    fn pinched_results_are_valid_inputs() {
        let unit = boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let bd = boolean(&unit, &diamond(0.75, 0.5, 0.25, -1.0, 3.0), BoolOp::Difference).unwrap();
        // A box crossing the crease: the intersection is four corner prisms touching along
        // lines (20 faces).
        let [_, _, _, i] = ops(&bd, &boxed([0.5, 0.25, 0.25], [1.5, 0.75, 0.75]));
        assert!(close(i.volume(), 0.0625));
        assert_eq!(i.face_count(), 20);
        // Against the same diamond, the right half of the box, and the box itself.
        let [u, d, e, i] = ops(&bd, &diamond(0.75, 0.5, 0.25, -1.0, 3.0));
        assert!(close(u.volume(), 1.25) && close(d.volume(), 0.875) && close(e.volume(), 0.375) && i.faces.is_empty());
        let [u, _, _, i] = ops(&bd, &boxed([0.5, 0.0, 0.0], [2.0, 1.0, 1.0]));
        assert!(close(u.volume(), 2.0) && u.face_count() == 6);
        assert_eq!(i.face_count(), 14); // two pentagonal prisms touching along two lines
        let [u, d, _, i] = ops(&bd, &unit);
        assert!(close(u.volume(), 1.0) && u.face_count() == 6 && d.faces.is_empty());
        assert_eq!(i.face_count(), 11);
        // A chained pinch: a tunnel and a corner column meeting along a line.
        let big = boxed([0.0, 0.0, 0.0], [3.0, 3.0, 3.0]);
        let tunnel = boxed([1.0, 1.0, -1.0], [2.0, 2.0, 4.0]);
        let t = boolean(&big, &tunnel, BoolOp::Difference).unwrap();
        let pinched = boolean(&t, &boxed([2.0, 2.0, 0.0], [3.0, 3.0, 3.0]), BoolOp::Difference).unwrap();
        assert_eq!(pinched.face_count(), 12);
        let [u, d, e, i] = ops(&pinched, &boxed([1.5, 1.5, 0.5], [2.5, 2.5, 2.5]));
        assert!(close(u.volume(), 22.0) && close(d.volume(), 20.0) && close(e.volume(), 1.0) && close(i.volume(), 1.0));
    }

    #[test]
    fn creased_results_are_valid_inputs() {
        // Faces with a crease (a cavity or an outside sheet touching the face along a line that
        // ends inside the face, or crosses it) cut by another solid's face across the crease.
        let unit = boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let cutter = boxed([0.75, 0.25, 0.4], [1.25, 0.75, 0.6]);
        let slit = boolean(&unit, &diamond(0.75, 0.5, 0.25, 0.25, 0.5), BoolOp::Difference).unwrap();
        let [u, d, e, i] = ops(&slit, &cutter);
        assert!(close(i.volume(), 0.025 - 0.0125) && close(e.volume(), 0.05 - 0.0125));
        assert!(close(u.volume(), 0.9375 + 0.0375) && close(d.volume(), 0.925));
        assert_eq!(i.face_count(), 10); // two triangular prisms touching along a line
        let half_slit = boolean(&unit, &diamond(0.75, 0.5, 0.25, -1.0, 1.5), BoolOp::Difference).unwrap();
        ops(&half_slit, &cutter);
        let outside = boolean(&unit, &diamond(1.25, 0.5, 0.25, -1.0, 3.0), BoolOp::Union).unwrap();
        let [u, _, e, i] = ops(&outside, &cutter);
        assert!(close(i.volume(), 0.025 + 0.0125) && close(e.volume(), 0.0125));
        assert!(close(u.volume(), 1.375 + 0.0125));
        let outside_short = boolean(&unit, &diamond(1.25, 0.5, 0.25, 0.25, 0.5), BoolOp::Union).unwrap();
        let [_, _, e, _] = ops(&outside_short, &cutter);
        assert_eq!(e.face_count(), 10);
        // Two boxes sharing only an edge, then cut by a column through the shared edge.
        let pair = boolean(&unit, &boxed([1.0, 1.0, 0.0], [2.0, 2.0, 1.0]), BoolOp::Union).unwrap();
        assert_eq!(pair.face_count(), 12);
        let [u, d, _, i] = ops(&pair, &boxed([0.5, 0.5, -1.0], [1.5, 1.5, 2.0]));
        assert!(close(u.volume(), 4.5) && close(d.volume(), 1.5) && close(i.volume(), 0.5));
        assert_eq!(i.face_count(), 12);
        let [u, ..] = ops(&pair, &boxed([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]));
        assert!(close(u.volume(), 2.75));
    }

    #[test]
    fn rounded_faces_of_earlier_results_merge_back() {
        // Chained unions of overlapping mitred sweeps and boxes: the faces of intermediate
        // results are planar only to rounding and are handled as exact triangles; the final
        // faces are merged back by tolerance and never mix planes.
        let rect = Polygon::new(vec![p2(-0.25, -0.15), p2(0.25, -0.15), p2(0.25, 0.15), p2(-0.25, 0.15)]);
        let s1 = Solid::sweep(&rect, &[Point3::new(0.0, 0.5, 0.5), Point3::new(2.5, 0.5, 0.5), Point3::new(2.5, 3.0, 0.5)]);
        let s2 = Solid::sweep(&rect, &[Point3::new(0.0, 0.5, 0.5), Point3::new(3.0, 0.5, 0.5), Point3::new(2.5, 3.0, 1.0)]);
        let b1 = boxed([0.75, 0.5, 0.75], [2.75, 2.25, 1.25]);
        let r1 = boolean(&b1, &s1, BoolOp::Union).unwrap();
        let r2 = boolean(&r1, &s2, BoolOp::Union).unwrap();
        assert!(r2.is_closed_manifold());
        let b2 = boxed([0.0, 0.5, 1.0], [1.0, 2.0, 1.5]);
        let [u, d, e, i] = ops(&r2, &b2);
        assert!(u.volume() > r2.volume() && d.volume() < r2.volume() && e.volume() > 0.0 && i.volume() > 0.0);
        let [u, ..] = ops(&r2, &boxed([2.0, 0.5, 0.5], [4.0, 2.0, 2.0]));
        assert!(u.is_closed_manifold());
    }
}
