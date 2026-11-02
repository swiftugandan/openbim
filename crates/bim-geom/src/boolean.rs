//! Boolean set operations (union, difference, intersection) on closed polyhedral solids.
//!
//! The algorithm is the classical "split, classify, select, stitch" scheme for B-Reps, with
//! every combinatorial decision taken exactly:
//!
//! 1. **Split.** Every face of `a` is split by the faces of `b` (and vice versa) inside its own
//!    plane: the intersection segments with the non-coplanar faces of the other solid and the
//!    edges of its coplanar faces are overlaid on the face as a 2D arrangement whose cells are
//!    the *fragments* of the face. All points of the arrangement are exact rational points
//!    ([`crate::exact::HPoint`]): input vertices, an edge piercing a face plane, three planes
//!    meeting, or two coplanar segments crossing. Points are interned in one table keyed by
//!    their exact value, so a point seen from several faces (or produced by different
//!    formulas) is one node, and a constructed point that coincides with an input vertex *is*
//!    that vertex. The arrangement itself (which segments cross or touch, the angular order at
//!    a node, the orientation of a cycle) is decided with exact predicates on those points.
//! 2. **Classify.** Each fragment is classified against the other solid from the local
//!    structure of the other solid along one of the fragment's boundary pieces: a piece in the
//!    interior of a face `g` of the other solid puts the fragment on one exact side of `g`'s
//!    plane; a piece along an edge of the other solid uses the convex/reflex wedge at that
//!    edge; a piece along the edge of a coplanar face decides whether the fragment lies on
//!    that face (`On`). A fragment whose boundary never touches the other solid is classified
//!    by an exact ray cast (with symbolic perturbation) from a point of its boundary.
//! 3. **Select.** Fragments are selected per operation (union keeps what is outside the other
//!    solid, intersection what is inside, difference the outside of `a` and the reversed inside
//!    of `b`; coplanar fragments are kept once, from `a`).
//! 4. **Stitch.** The kept fragments share nodes by identity; coplanar edge-connected
//!    fragments are merged into maximal faces (with holes), vertices that became collinear are
//!    dropped, half-edges are paired (edges with four half-edges are paired by the angular
//!    order of their faces) and vertices are split into one copy per umbrella, so that the
//!    result is a closed 2-manifold even where the set-theoretic result pinches at an edge or
//!    a vertex. Output coordinates are the correctly rounded exact coordinates. A final
//!    validation (planarity, no T-junctions, closed manifold) turns any inconsistency into an
//!    error rather than a malformed solid.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::exact::{cmp_coord, orient2d_h, plane_sign, same_point, triple_sign, HPoint, Plane};
use crate::polygon::Polygon;
use crate::predicates::{orient2d, orient3d};
use crate::solid::{Face, HalfEdge, Solid, Vertex};
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

/// An undirected edge of an input solid: `a < b`; `faces[0]` holds the half-edge `a -> b`,
/// `faces[1]` the half-edge `b -> a`.
struct Edge {
    a: usize,
    b: usize,
    faces: [usize; 2],
}

/// One input solid with the derived data the splitting needs. The solid is handled as a list
/// of *sub-faces*: an exactly planar input face is one sub-face; an input face whose vertices
/// are not exactly coplanar (mitred sweeps and the rounded results of earlier booleans) is
/// split into its triangles, each exactly planar, which are merged back when the result is
/// assembled. Every "face" below is a sub-face.
struct Prep<'a> {
    s: &'a Solid,
    /// Input face of every sub-face.
    parent: Vec<usize>,
    /// Reference triangle of every sub-face (defines its plane exactly, wound outward).
    refs: Vec<[usize; 3]>,
    /// Exact plane of every sub-face.
    planes: Vec<Plane>,
    /// Node id of every input vertex.
    vnode: Vec<usize>,
    /// Vertex loops of every sub-face (outer first).
    loops: Vec<Vec<Vec<usize>>>,
    edges: Vec<Edge>,
    /// Directed vertex pair -> edge id.
    edge_ids: HashMap<(usize, usize), usize>,
    /// Edge ids around every face.
    face_edges: Vec<Vec<usize>>,
    /// All face triangles (face, vertex indices), for ray casting.
    tris: Vec<(usize, [usize; 3])>,
    volume: f64,
}

impl<'a> Prep<'a> {
    fn new(s: &'a Solid, name: &str, nodes: &mut Nodes) -> Result<Self, GeomError> {
        let mut parent = Vec::new();
        let mut refs = Vec::new();
        let mut planes = Vec::new();
        let mut loops: Vec<Vec<Vec<usize>>> = Vec::new();
        let mut tris: Vec<(usize, [usize; 3])> = Vec::new();
        for f in 0..s.faces.len() {
            let r = s
                .reference_triangle(f)
                .ok_or_else(|| GeomError::Degenerate(format!("face {f} of `{name}` has no area")))?;
            let face_loops: Vec<Vec<usize>> = s.faces[f].loops.iter().map(|&h| s.loop_vertices(h)).collect();
            let planar = face_loops.iter().flatten().all(|&v| orient3d(s.vertices[r[0]].point, s.vertices[r[1]].point, s.vertices[r[2]].point, s.vertices[v].point) == 0.0);
            let subs: Vec<([usize; 3], Vec<Vec<usize>>)> = if planar {
                vec![(r, face_loops)]
            } else {
                triangulate_canonical(s, f, &face_loops, r).into_iter().map(|t| (t, vec![t.to_vec()])).collect()
            };
            for (sr, sl) in subs {
                let pl = Plane::through(s.vertices[sr[0]].point, s.vertices[sr[1]].point, s.vertices[sr[2]].point);
                if !pl.is_finite() {
                    return Err(GeomError::Degenerate(format!("face {f} of `{name}` has non-finite coordinates")));
                }
                let id = refs.len();
                if planar {
                    tris.extend(s.faces[f].triangles.iter().map(|&t| (id, t)));
                } else {
                    tris.push((id, sr));
                }
                parent.push(f);
                refs.push(sr);
                planes.push(pl);
                loops.push(sl);
            }
        }
        let mut vnode = Vec::with_capacity(s.vertices.len());
        for v in &s.vertices {
            if !(v.point.x.is_finite() && v.point.y.is_finite() && v.point.z.is_finite()) {
                return Err(GeomError::Degenerate(format!("`{name}` has non-finite coordinates")));
            }
            vnode.push(nodes.intern(HPoint::input(v.point)));
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
        Ok(Prep { s, parent, refs, planes, vnode, loops, edges, edge_ids, face_edges, tris, volume: s.volume() })
    }

    /// Number of sub-faces.
    fn nf(&self) -> usize {
        self.loops.len()
    }

    fn pt(&self, v: usize) -> Point3 {
        self.s.vertices[v].point
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

/// Triangles of an input face that is not exactly planar. A convex single loop is fanned from
/// its lexicographically smallest vertex, which depends only on the vertex set, so that the
/// same face occurring in both inputs (with opposite orientation) is split identically and the
/// pieces are recognised as exactly coplanar; other faces use the solid's own triangulation.
fn triangulate_canonical(s: &Solid, f: usize, face_loops: &[Vec<usize>], r: [usize; 3]) -> Vec<[usize; 3]> {
    let pt = |v: usize| s.vertices[v].point;
    if face_loops.len() == 1 && face_loops[0].len() >= 4 {
        let l = &face_loops[0];
        let n = l.len();
        let normal = (pt(r[1]) - pt(r[0])).cross(pt(r[2]) - pt(r[0]));
        let ax = axes_of([normal.x, normal.y, normal.z]);
        let convex = (0..n).all(|i| orient2d(project(pt(l[(i + n - 1) % n]), ax), project(pt(l[i]), ax), project(pt(l[(i + 1) % n]), ax)) > 0.0);
        if convex {
            let lex = |v: usize| {
                let p = pt(v);
                (p.x, p.y, p.z)
            };
            let mut m = 0;
            for i in 1..n {
                if lex(l[i]).partial_cmp(&lex(l[m])) == Some(std::cmp::Ordering::Less) {
                    m = i;
                }
            }
            return (1..n - 1).map(|k| [l[m], l[(m + k) % n], l[(m + k + 1) % n]]).collect();
        }
    }
    let area2 = |t: [usize; 3]| (pt(t[1]) - pt(t[0])).cross(pt(t[2]) - pt(t[0])).norm2();
    s.faces[f].triangles.iter().copied().filter(|&t| area2(t) > 0.0).collect()
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
struct Ctx<'a> {
    p: [Prep<'a>; 2],
    rel: [Rel; 2],
    /// `cop[f][g]` for face `f` of `a` and `g` of `b`: `Some(same_orientation)` when the faces
    /// are exactly coplanar.
    cop: Vec<Vec<Option<bool>>>,
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

impl<'a> Ctx<'a> {
    fn new(a: &'a Solid, b: &'a Solid) -> Result<Self, GeomError> {
        let mut nodes = Nodes::default();
        let pa = Prep::new(a, "a", &mut nodes)?;
        let pb = Prep::new(b, "b", &mut nodes)?;
        let rel_of = |p: &Prep, q: &Prep| {
            let vside: Vec<Vec<i8>> = (0..p.s.vertices.len())
                .map(|v| {
                    (0..q.nf())
                        .map(|g| {
                            let r = q.refs[g];
                            let o = orient3d(q.pt(r[0]), q.pt(r[1]), q.pt(r[2]), p.pt(v));
                            if o > 0.0 {
                                1
                            } else if o < 0.0 {
                                -1
                            } else {
                                0
                            }
                        })
                        .collect()
                })
                .collect();
            Rel { vside }
        };
        let rel = [rel_of(&pa, &pb), rel_of(&pb, &pa)];
        let mut cop = vec![vec![None; pb.nf()]; pa.nf()];
        for (f, row) in cop.iter_mut().enumerate() {
            for (g, cell) in row.iter_mut().enumerate() {
                let on_f = pb.refs[g].iter().all(|&v| rel[1].vside[v][f] == 0);
                let on_g = pa.refs[f].iter().all(|&v| rel[0].vside[v][g] == 0);
                if on_f && on_g {
                    let (na, nb) = (pa.planes[f].n_approx, pb.planes[g].n_approx);
                    *cell = Some(na[0] * nb[0] + na[1] * nb[1] + na[2] * nb[2] > 0.0);
                }
            }
        }
        Ok(Ctx { p: [pa, pb], rel, cop, nodes: RefCell::new(nodes), edge_nodes: RefCell::new(HashMap::new()) })
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
        let n = HPoint::pierce(self.p[s].pt(ed.a), self.p[s].pt(ed.b), &self.p[t].planes[g]).map(|p| self.nodes.borrow_mut().intern(p));
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

    /// Sign of the direction of `p -> q` along the input segment `a -> b` it is collinear with
    /// (positive when they point the same way).
    fn dir_along(&self, p: usize, q: usize, a: Point3, b: Point3) -> i32 {
        let d = b - a;
        let c = [d.x, d.y, d.z];
        let mut k = 0;
        for i in 1..3 {
            if c[i].abs() > c[k].abs() {
                k = i;
            }
        }
        if c[k] == 0.0 {
            return 0;
        }
        self.cmp(q, p, k) * if c[k] > 0.0 { 1 } else { -1 }
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
        let (pa, pb) = (p.pt(ed.a), p.pt(ed.b));
        if let Some(k) = self.sep_axis(na, nb, None) {
            for v in 0..q.s.vertices.len() {
                let n = q.vnode[v];
                if n == na || n == nb || !collinear(pa, pb, q.pt(v)) {
                    continue;
                }
                if self.cmp(n, na, k) * self.cmp(n, nb, k) < 0 {
                    out.push(n);
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
struct Arrangement<'c, 'a> {
    ctx: &'c Ctx<'a>,
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

impl<'c, 'a> Arrangement<'c, 'a> {
    fn new(ctx: &'c Ctx<'a>, x: usize, f: usize) -> Self {
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
        arr
    }

    /// Overlay the edges of a coplanar face `g` of the other solid.
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
                self.segs.push(Seg { a: q.vnode[u], b: q.vnode[v], line: Line::Edge { s: y, e }, src: Src::Edge(e), clipped: false, splits: Vec::new() });
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
        let mags = [dir[0].leading().abs(), dir[1].leading().abs(), dir[2].leading().abs()];
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
        // Closed intervals of the line inside `g`, with what they lie on.
        let mut iv_g: Vec<(usize, usize, Src, Line)> = Vec::new();
        for pair in ev_g.chunks(2) {
            if pair.len() == 2 && ctx.cmp(pair[0], pair[1], axis) < 0 {
                iv_g.push((pair[0], pair[1], Src::Face(g), Line::Plane { g }));
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
                    let d = ctx.dir_along(pc.a, pc.b, p.pt(u), p.pt(v));
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

impl<'a> Ctx<'a> {
    /// Sign of the plane of face `g` of solid `t` on the left side of the directed piece
    /// `p -> q` of a fragment whose nodes are `nodes` (the piece lies on that plane).
    fn left_side_of_plane(&self, t: usize, g: usize, p: usize, q: usize, frag: &Fragment, ax: (usize, usize)) -> Option<i32> {
        for l in &frag.loops {
            for &r in l {
                if r == p || r == q {
                    continue;
                }
                let s = self.plane_side(t, g, r);
                if s != 0 {
                    let o = self.orient(p, q, r, ax);
                    if o != 0 {
                        return Some(s * o);
                    }
                }
            }
        }
        None
    }

    /// Whether edge `e` of solid `t` is convex (positive), reflex (negative) or flat (zero).
    fn convexity(&self, t: usize, e: usize) -> i32 {
        let p = &self.p[t];
        let ed = &p.edges[e];
        let (g1, g2) = (ed.faces[0], ed.faces[1]);
        let (u, v) = p.edge_dir_in(g1, e);
        let d = crate::exact::diff3(p.pt(v), p.pt(u));
        // Outward in-plane direction of g1 across the edge: d × n1; convex when it points to
        // the outer side of g2.
        triple_sign(&p.planes[g2].n, &d, &p.planes[g1].n)
    }

    fn classify(&self, x: usize, f: usize, frag: &Fragment) -> Result<Class, GeomError> {
        let y = 1 - x;
        let p = &self.p[x];
        let q = &self.p[y];
        let ax = p.axes(f);
        // 1. On a coplanar face of the other solid?
        for g in 0..q.nf() {
            if let Some(same) = self.coplanar(x, f, g) {
                if self.on_coplanar(x, frag, g, same) {
                    return Ok(Class::On(same));
                }
            }
        }
        // 2. Local rules along pieces lying on the other solid.
        for pc in &frag.pieces {
            for src in &pc.srcs {
                if let Src::Edge(e) = *src {
                    let ed = &q.edges[e];
                    let mut sides = Vec::new();
                    for &g in &ed.faces {
                        if self.coplanar(x, f, g).is_none() {
                            sides.push(self.left_side_of_plane(y, g, pc.a, pc.b, frag, ax));
                        }
                    }
                    match sides.len() {
                        0 => return Ok(Class::Outside),
                        1 => {
                            if let Some(s) = sides[0] {
                                return Ok(if s > 0 { Class::Inside } else { Class::Outside });
                            }
                        }
                        _ => {
                            if let (Some(s1), Some(s2)) = (sides[0], sides[1]) {
                                let inside = if self.convexity(y, e) < 0 { s1 > 0 || s2 > 0 } else { s1 > 0 && s2 > 0 };
                                return Ok(if inside { Class::Inside } else { Class::Outside });
                            }
                        }
                    }
                }
            }
        }
        for pc in &frag.pieces {
            for src in &pc.srcs {
                if let Src::Face(g) = *src {
                    if let Some(s) = self.left_side_of_plane(y, g, pc.a, pc.b, frag, ax) {
                        return Ok(if s > 0 { Class::Inside } else { Class::Outside });
                    }
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
                        let d = self.dir_along(pc.a, pc.b, q.pt(u), q.pt(v));
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
    /// triangles crossed by the ray towards `+x`, with the ray's projection perturbed
    /// symbolically by `(ε, ε²)` in `(y, z)` so that edges and vertices on it are counted
    /// consistently. `None` when the point turns out to lie on a triangle.
    fn winding(&self, t: usize, m: &HPoint) -> Option<i32> {
        let q = &self.p[t];
        let nodes = self.nodes.borrow();
        let ax = (1, 2);
        let mut total = 0;
        for &(g, tri) in &q.tris {
            let pts = [q.pt(tri[0]), q.pt(tri[1]), q.pt(tri[2])];
            let (miny, maxy) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)));
            let (minz, maxz) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.z), hi.max(p.z)));
            if m.iv[1].hi < miny || m.iv[1].lo > maxy || m.iv[2].hi < minz || m.iv[2].lo > maxz {
                continue;
            }
            let hp = [&nodes.pts[q.vnode[tri[0]]], &nodes.pts[q.vnode[tri[1]]], &nodes.pts[q.vnode[tri[2]]]];
            let o = orient2d_h(hp[0], hp[1], hp[2], ax);
            if o == 0 {
                continue; // edge-on triangle
            }
            let mut inside = true;
            for i in 0..3 {
                let (u, v) = (hp[i], hp[(i + 1) % 3]);
                let mut s = orient2d_h(u, v, m, ax);
                if s == 0 {
                    let (dy, dz) = (v.r.y - u.r.y, v.r.z - u.r.z);
                    s = if dz != 0.0 {
                        if dz > 0.0 {
                            -1
                        } else {
                            1
                        }
                    } else if dy > 0.0 {
                        1
                    } else if dy < 0.0 {
                        -1
                    } else {
                        0
                    };
                }
                if s != o {
                    inside = false;
                    break;
                }
            }
            if !inside {
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
    // ---- plane classes over all input faces (exactly coplanar, either orientation).
    let (na, nb) = (ctx.p[0].nf(), ctx.p[1].nf());
    let gid = |x: usize, f: usize| if x == 0 { f } else { na + f };
    let total = na + nb;
    let mut parent: Vec<usize> = (0..total).collect();
    let same_plane = |x: usize, f: usize, y: usize, g: usize| -> bool {
        if x != y {
            return ctx.coplanar(x.min(y), if x == 0 { f } else { g }, if x == 0 { g } else { f }).is_some();
        }
        let p = &ctx.p[x];
        if p.parent[f] == p.parent[g] {
            return true; // pieces of one (possibly non-planar) input face
        }
        let on = |t: [usize; 3], q: Point3| orient3d(p.pt(t[0]), p.pt(t[1]), p.pt(t[2]), q) == 0.0;
        p.refs[g].iter().all(|&v| on(p.refs[f], p.pt(v))) && p.refs[f].iter().all(|&v| on(p.refs[g], p.pt(v)))
    };
    for i in 0..total {
        for j in i + 1..total {
            let (x, f) = if i < na { (0, i) } else { (1, i - na) };
            let (y, g) = if j < na { (0, j) } else { (1, j - na) };
            if same_plane(x, f, y, g) {
                unite(&mut parent, i, j);
            }
        }
    }
    let normal_of = |x: usize, f: usize| {
        let n = ctx.p[x].planes[f].n_approx;
        Vec3::new(n[0], n[1], n[2])
    };
    let rep_normal = |root: usize| if root < na { normal_of(0, root) } else { normal_of(1, root - na) };

    // ---- vertices: the rounded coordinates of the shared nodes.
    let points: Vec<Point3> = ctx.nodes.borrow().pts.iter().map(|p| p.r).collect();
    struct Frag {
        loops: Vec<Vec<usize>>,
        class: usize,
        orient: i8,
    }
    let mut frags: Vec<Frag> = Vec::new();
    for k in &kept {
        let root = find(&mut parent, gid(k.solid, k.face));
        let mut orient: i8 = if normal_of(k.solid, k.face).dot(rep_normal(root)) > 0.0 { 1 } else { -1 };
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
            orient = -orient;
            for l in loops.iter_mut() {
                l.reverse();
            }
        }
        frags.push(Frag { loops, class: root, orient });
    }

    // ---- merge coplanar edge-connected fragments.
    let mut dir_edges: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (i, fr) in frags.iter().enumerate() {
        for l in &fr.loops {
            for j in 0..l.len() {
                dir_edges.entry((l[j], l[(j + 1) % l.len()])).or_default().push(i);
            }
        }
    }
    let mut fparent: Vec<usize> = (0..frags.len()).collect();
    for (&(u, v), owners) in &dir_edges {
        if let Some(rev) = dir_edges.get(&(v, u)) {
            for &i in owners {
                for &j in rev {
                    if frags[i].class == frags[j].class && frags[i].orient == frags[j].orient {
                        unite(&mut fparent, i, j);
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..frags.len() {
        let r = find(&mut fparent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut group_ids: Vec<usize> = groups.keys().copied().collect();
    group_ids.sort();

    let mut faces: Vec<Vec<Vec<usize>>> = Vec::new(); // loops of vertex ids, outer first
    for r in group_ids {
        let members = &groups[&r];
        let fr0 = &frags[members[0]];
        let n = rep_normal(fr0.class) * fr0.orient as f64;
        let ax = axes_of([n.x, n.y, n.z]);
        if members.len() == 1 {
            faces.push(fr0.loops.clone());
            continue;
        }
        // Directed edges of the group minus those cancelled by their reverse.
        let mut count: BTreeMap<(usize, usize), i32> = BTreeMap::new();
        for &i in members {
            for l in &frags[i].loops {
                for j in 0..l.len() {
                    *count.entry((l[j], l[(j + 1) % l.len()])).or_insert(0) += 1;
                }
            }
        }
        let mut out_edges: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let mut boundary: Vec<(usize, usize)> = Vec::new();
        for (&(u, v), &c) in &count {
            let back = count.get(&(v, u)).copied().unwrap_or(0);
            if c > back {
                for _ in 0..(c - back) {
                    boundary.push((u, v));
                    out_edges.entry(u).or_default().push(v);
                }
            }
        }
        boundary.sort();
        let mut used: HashSet<(usize, usize)> = HashSet::new();
        let mut loops: Vec<Vec<usize>> = Vec::new();
        for &start in &boundary {
            if used.contains(&start) {
                continue;
            }
            let mut lp = vec![start.0];
            let (mut u, mut v) = start;
            used.insert(start);
            let mut guard = 0;
            while v != start.0 {
                lp.push(v);
                let cands: Vec<usize> = out_edges[&v].iter().copied().filter(|&w| !used.contains(&(v, w))).collect();
                if cands.is_empty() {
                    break;
                }
                let w = if cands.len() == 1 {
                    cands[0]
                } else {
                    // Sharpest left turn: the candidate first clockwise from the way back.
                    let pv = project(points[v], ax);
                    let back = project(points[u], ax);
                    let ang = |q: Point2| {
                        let a = (q.y - pv.y).atan2(q.x - pv.x) - (back.y - pv.y).atan2(back.x - pv.x);
                        let a = a.rem_euclid(2.0 * std::f64::consts::PI);
                        if a == 0.0 {
                            2.0 * std::f64::consts::PI
                        } else {
                            a
                        }
                    };
                    cands.into_iter().max_by(|&p, &q| ang(project(points[p], ax)).partial_cmp(&ang(project(points[q], ax))).unwrap_or(std::cmp::Ordering::Equal)).unwrap()
                };
                used.insert((v, w));
                u = v;
                v = w;
                guard += 1;
                if guard > boundary.len() + 1 {
                    break;
                }
            }
            if lp.len() >= 3 {
                loops.push(lp);
            }
        }
        let area = |l: &Vec<usize>| crate::polygon::signed_area(&l.iter().map(|&i| project(points[i], ax)).collect::<Vec<_>>());
        let mut outers: Vec<(Vec<usize>, Vec<Vec<usize>>)> = Vec::new();
        let mut holes: Vec<Vec<usize>> = Vec::new();
        for l in loops {
            if area(&l) > 0.0 {
                outers.push((l, Vec::new()));
            } else {
                holes.push(l);
            }
        }
        for h in holes {
            if outers.len() == 1 {
                outers[0].1.push(h);
                continue;
            }
            let mut best: Option<(usize, f64)> = None;
            for (k, (o, _)) in outers.iter().enumerate() {
                let poly = Polygon::new(o.iter().map(|&i| project(points[i], ax)).collect());
                let inside = h.iter().find(|v| !o.contains(v)).map(|&v| poly.contains(project(points[v], ax)));
                let a = area(o);
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
        }
    }

    // ---- half-edge structure with angular twin pairing and vertex splitting.
    let mut vertices: Vec<Vertex> = points.iter().map(|&p| Vertex { point: p, half_edge: None }).collect();
    let mut half_edges: Vec<HalfEdge> = Vec::new();
    let mut out_faces: Vec<Face> = Vec::new();
    let mut face_normals: Vec<Vec3> = Vec::new();
    for ls in &faces {
        let face_id = out_faces.len();
        let mut fl = Vec::new();
        for l in ls {
            let base = half_edges.len();
            let n = l.len();
            for (k, &v) in l.iter().enumerate() {
                half_edges.push(HalfEdge { origin: v, twin: None, next: base + (k + 1) % n, prev: base + (k + n - 1) % n, face: face_id });
            }
            fl.push(base);
        }
        face_normals.push(newell(&ls[0].iter().map(|&v| points[v]).collect::<Vec<_>>()));
        out_faces.push(Face { loops: fl, triangles: Vec::new() });
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
    let mut rounds = 0;
    loop {
        half_edges = base_half_edges.clone();
        vertices = base_vertices.clone();
        pair_half_edges(&mut half_edges, &by_edge, &points, &face_normals, &empty_pairing)?;
        split_umbrellas(&mut half_edges, &mut vertices, &points);
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
    let solid = Solid { vertices, half_edges, faces: out_faces };
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
fn split_umbrellas(half_edges: &mut [HalfEdge], vertices: &mut Vec<Vertex>, points: &[Point3]) {
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
                if (s.vertices[v].point - o).dot(n).abs() > 1e-9 * diag {
                    return Err(GeomError::Degenerate("result face is not planar".into()));
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
