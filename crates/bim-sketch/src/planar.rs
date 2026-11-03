//! Planar graphs: the vertices and edges of a Shape lying in one plane, projected to 2D, with the
//! face walk that finds the smallest regions bounded by edges (spec/sketch.md, `line`), and the
//! assembly of regions from directed boundary edges (face unions and differences).
//!
//! Every vertex here is a *local* index into the graph; [`PlanarGraph::verts`] maps it back to the
//! shape's vertex index. All orientation is measured in the plane's 2D frame, so a loop with
//! positive signed area is counter-clockwise seen from the plane normal's side.

use std::collections::{BTreeMap, BTreeSet};

use bim_geom::{orient2d, Point2, Polygon};

use crate::geom::{Plane, V3};

/// A bounded region of a plane: an outer loop (positive area) and hole loops (negative area), as
/// local vertex indices of the graph that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub outer: Vec<usize>,
    pub holes: Vec<Vec<usize>>,
}

impl Region {
    /// Whether the undirected edge `{a, b}` is on the outer loop or a hole of the region.
    pub fn has_edge(&self, a: usize, b: usize) -> bool {
        self.loops().any(|lp| loop_has_edge(lp, a, b))
    }

    pub fn loops(&self) -> impl Iterator<Item = &Vec<usize>> {
        std::iter::once(&self.outer).chain(self.holes.iter())
    }
}

pub fn loop_has_edge(lp: &[usize], a: usize, b: usize) -> bool {
    let n = lp.len();
    (0..n).any(|i| {
        let (x, y) = (lp[i], lp[(i + 1) % n]);
        (x == a && y == b) || (x == b && y == a)
    })
}

/// Signed area of a ring of 2D points (shoelace relative to the first point).
pub fn signed_area(pts: &[Point2]) -> f64 {
    bim_geom::polygon::signed_area(pts)
}

/// The edges of a Shape lying in one plane, projected onto it.
pub struct PlanarGraph {
    /// Shape vertex index of each local vertex.
    pub verts: Vec<usize>,
    /// Projected position of each local vertex.
    pub pos: Vec<Point2>,
    local: BTreeMap<usize, usize>,
    /// Neighbours of each local vertex, sorted counter-clockwise by angle.
    adj: Vec<Vec<usize>>,
    /// Connected component of each local vertex.
    comp: Vec<usize>,
    ncomp: usize,
}

impl PlanarGraph {
    /// The graph of every edge of `edges` whose endpoints both lie on `plane` (within the
    /// equality tolerance).
    pub fn new(plane: &Plane, vertices: &[V3], edges: &[(usize, usize)]) -> PlanarGraph {
        let mut verts = Vec::new();
        let mut local = BTreeMap::new();
        for (i, &p) in vertices.iter().enumerate() {
            if plane.contains(p) {
                local.insert(i, verts.len());
                verts.push(i);
            }
        }
        let pos: Vec<Point2> = verts.iter().map(|&i| plane.project(vertices[i])).collect();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); verts.len()];
        for &(a, b) in edges {
            if let (Some(&la), Some(&lb)) = (local.get(&a), local.get(&b)) {
                if la != lb && !adj[la].contains(&lb) {
                    adj[la].push(lb);
                    adj[lb].push(la);
                }
            }
        }
        for (i, nb) in adj.iter_mut().enumerate() {
            let o = pos[i];
            nb.sort_by(|&x, &y| {
                let ax = (pos[x].y - o.y).atan2(pos[x].x - o.x);
                let ay = (pos[y].y - o.y).atan2(pos[y].x - o.x);
                ax.total_cmp(&ay).then(x.cmp(&y))
            });
        }
        // Components by a deterministic flood fill.
        let mut comp = vec![usize::MAX; verts.len()];
        let mut ncomp = 0;
        for start in 0..verts.len() {
            if comp[start] != usize::MAX {
                continue;
            }
            let mut stack = vec![start];
            comp[start] = ncomp;
            while let Some(v) = stack.pop() {
                for &w in &adj[v] {
                    if comp[w] == usize::MAX {
                        comp[w] = ncomp;
                        stack.push(w);
                    }
                }
            }
            ncomp += 1;
        }
        PlanarGraph { verts, pos, local, adj, comp, ncomp }
    }

    /// The local index of shape vertex `v`, if it lies on the plane.
    pub fn local(&self, v: usize) -> Option<usize> {
        self.local.get(&v).copied()
    }

    /// The signed area of a loop of local vertices.
    pub fn area(&self, lp: &[usize]) -> f64 {
        let pts: Vec<Point2> = lp.iter().map(|&i| self.pos[i]).collect();
        signed_area(&pts)
    }

    /// Whether a loop's area is negligible: collinear or repeated points only.
    pub fn is_flat(&self, lp: &[usize]) -> bool {
        let (mut lo, mut hi) = (Point2::new(f64::INFINITY, f64::INFINITY), Point2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
        for &i in lp {
            let p = self.pos[i];
            lo = Point2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let l2 = (hi.x - lo.x).powi(2) + (hi.y - lo.y).powi(2);
        self.area(lp).abs() <= 1e-12 * l2.max(1e-18)
    }

    /// The step of the left-face walk: arriving at `to` from `from`, the next vertex is the
    /// neighbour of `to` just clockwise of `from` (the sharpest left turn); a dead end turns back.
    fn next(&self, from: usize, to: usize) -> usize {
        let nb = &self.adj[to];
        let i = nb.iter().position(|&x| x == from).expect("from is a neighbour of to");
        nb[(i + nb.len() - 1) % nb.len()]
    }

    /// The closed walk of the face on the left of the directed edge `s → t`, as the sequence of
    /// vertices visited starting at `s` (the last vertex links back to `s`).
    pub fn walk(&self, s: usize, t: usize) -> Vec<usize> {
        let mut out = vec![s];
        let (mut a, mut b) = (s, t);
        let limit = 4 * self.adj.iter().map(Vec::len).sum::<usize>() + 4;
        loop {
            out.push(b);
            let c = self.next(a, b);
            a = b;
            b = c;
            if (a, b) == (s, t) || out.len() > limit {
                out.pop();
                return out;
            }
        }
    }

    /// Every directed edge's walk, each walk once, keyed by its smallest directed edge.
    fn all_walks(&self) -> Vec<Vec<usize>> {
        let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
        let mut out = Vec::new();
        for a in 0..self.adj.len() {
            for &b in &self.adj[a] {
                if seen.contains(&(a, b)) {
                    continue;
                }
                let w = self.walk(a, b);
                let n = w.len();
                for i in 0..n {
                    seen.insert((w[i], w[(i + 1) % n]));
                }
                out.push(w);
            }
        }
        out
    }

    /// The outer boundary loops (negative area, i.e. clockwise) of a connected component: the
    /// simple loops of its unbounded face's walk. A tree has none.
    pub fn component_boundary(&self, comp: usize) -> Vec<Vec<usize>> {
        for w in self.all_walks() {
            if self.comp[w[0]] != comp {
                continue;
            }
            if self.area(&w) < 0.0 && !self.is_flat(&w) {
                return decompose(&w).into_iter().filter(|lp| self.area(lp) < 0.0 && !self.is_flat(lp)).collect();
            }
        }
        Vec::new()
    }

    /// Whether the point `p` is inside the ring (even-odd rule, exact predicate).
    pub fn inside(&self, ring: &[usize], p: Point2) -> bool {
        let pts: Vec<Point2> = ring.iter().map(|&i| self.pos[i]).collect();
        Polygon::new(pts).contains(p)
    }

    /// A point strictly inside a region: the centroid of the largest triangle of its triangulation.
    pub fn interior_point(&self, r: &Region) -> Option<Point2> {
        let to_pts = |lp: &Vec<usize>| -> Vec<Point2> { lp.iter().map(|&i| self.pos[i]).collect() };
        let poly = Polygon::with_holes(to_pts(&r.outer), r.holes.iter().map(to_pts).collect());
        let pts = poly.vertices();
        let mut best: Option<(f64, Point2)> = None;
        for t in poly.triangulate() {
            let [a, b, c] = t.map(|i| pts[i]);
            let area = orient2d(a, b, c).abs();
            let centroid = Point2::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
            if best.map_or(true, |(ba, _)| area > ba) {
                best = Some((area, centroid));
            }
        }
        best.filter(|(a, _)| *a > 0.0).map(|(_, c)| c)
    }

    /// Whether `p` is inside the region (inside the outer ring and outside every hole).
    pub fn region_contains(&self, r: &Region, p: Point2) -> bool {
        self.inside(&r.outer, p) && !r.holes.iter().any(|h| self.inside(h, p))
    }

    /// The regions bounded by the closed walk `w` (a left-face walk): each simple positive loop of
    /// its decomposition, with the negative loops it contains and the outer boundaries of the other
    /// components inside it (maximal ones only) as holes.
    pub fn regions_of_walk(&self, w: &[usize]) -> Vec<Region> {
        let loops = decompose(w);
        let positives: Vec<&Vec<usize>> = loops.iter().filter(|lp| self.area(lp) > 0.0 && !self.is_flat(lp)).collect();
        let negatives: Vec<&Vec<usize>> = loops.iter().filter(|lp| self.area(lp) < 0.0 && !self.is_flat(lp)).collect();
        let comp = self.comp[w[0]];
        let mut out = Vec::new();
        for outer in positives {
            let on_outer: BTreeSet<usize> = outer.iter().copied().collect();
            let mut candidates: Vec<Vec<usize>> = Vec::new();
            for neg in &negatives {
                if let Some(&v) = neg.iter().find(|v| !on_outer.contains(v)) {
                    if self.inside(outer, self.pos[v]) {
                        candidates.push((*neg).clone());
                    }
                }
            }
            for c in 0..self.ncomp {
                if c == comp {
                    continue;
                }
                for lp in self.component_boundary(c) {
                    if self.inside(outer, self.pos[lp[0]]) {
                        candidates.push(lp);
                    }
                }
            }
            // Only the maximal candidates are holes; the others lie inside a hole.
            let holes: Vec<Vec<usize>> = candidates
                .iter()
                .filter(|h| {
                    !candidates.iter().any(|o| {
                        o != *h && {
                            let os: BTreeSet<usize> = o.iter().copied().collect();
                            h.iter().find(|v| !os.contains(v)).map_or(false, |&v| self.inside(o, self.pos[v]))
                        }
                    })
                })
                .cloned()
                .collect();
            out.push(Region { outer: outer.clone(), holes });
        }
        out
    }

    /// Links a multiset of directed edges (local indices) into loops: opposite pairs cancel first,
    /// then every remaining edge is walked, taking the sharpest left turn at vertices with several
    /// ways out. Returns the simple loops found, each with non-negligible area.
    pub fn link(&self, dir_edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
        let mut count: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for &(a, b) in dir_edges {
            if a == b {
                continue;
            }
            match count.get_mut(&(b, a)) {
                Some(n) if *n > 0 => *n -= 1,
                _ => *count.entry((a, b)).or_insert(0) += 1,
            }
        }
        let mut out_edges: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (&(a, b), &n) in &count {
            for _ in 0..n {
                out_edges.entry(a).or_default().push(b);
            }
        }
        let mut loops = Vec::new();
        loop {
            let Some((&start, _)) = out_edges.iter().find(|(_, v)| !v.is_empty()) else { break };
            let mut walk = vec![start];
            let mut a = start;
            let mut b = {
                let v = out_edges.get_mut(&start).expect("has edges");
                v.remove(0)
            };
            let limit = dir_edges.len() + 2;
            while b != start && walk.len() <= limit {
                walk.push(b);
                let v = out_edges.get_mut(&b).map(std::mem::take).unwrap_or_default();
                if v.is_empty() {
                    break;
                }
                let c = self.pick_left(a, b, &v);
                let mut rest = v;
                let k = rest.iter().position(|&x| x == c).expect("c is a candidate");
                rest.remove(k);
                out_edges.insert(b, rest);
                a = b;
                b = c;
            }
            if b == start {
                for lp in decompose(&walk) {
                    if !self.is_flat(&lp) {
                        loops.push(lp);
                    }
                }
            }
        }
        loops
    }

    /// Among `candidates` (neighbours of `at`), the one that is the sharpest left turn when
    /// arriving from `from`: the first found going clockwise from the direction back to `from`.
    fn pick_left(&self, from: usize, at: usize, candidates: &[usize]) -> usize {
        let o = self.pos[at];
        let back = (self.pos[from].y - o.y).atan2(self.pos[from].x - o.x);
        let mut best: Option<(f64, usize)> = None;
        for &c in candidates {
            let ang = (self.pos[c].y - o.y).atan2(self.pos[c].x - o.x);
            let mut cw = back - ang;
            while cw <= 0.0 {
                cw += std::f64::consts::TAU;
            }
            while cw > std::f64::consts::TAU {
                cw -= std::f64::consts::TAU;
            }
            if c == from {
                cw = std::f64::consts::TAU;
            }
            if best.map_or(true, |(b, bc)| cw < b || (cw == b && c < bc)) {
                best = Some((cw, c));
            }
        }
        best.expect("at least one candidate").1
    }

    /// Groups loops into regions: positive loops are outers, each negative loop is a hole of the
    /// smallest positive loop containing it (negative loops inside no positive loop are dropped).
    pub fn assemble(&self, loops: Vec<Vec<usize>>) -> Vec<Region> {
        let mut regions: Vec<Region> = Vec::new();
        let mut areas: Vec<f64> = Vec::new();
        for lp in &loops {
            let a = self.area(lp);
            if a > 0.0 {
                regions.push(Region { outer: lp.clone(), holes: Vec::new() });
                areas.push(a);
            }
        }
        for lp in &loops {
            if self.area(lp) >= 0.0 {
                continue;
            }
            let mut best: Option<usize> = None;
            for (k, r) in regions.iter().enumerate() {
                let os: BTreeSet<usize> = r.outer.iter().copied().collect();
                let Some(&v) = lp.iter().find(|v| !os.contains(v)) else { continue };
                if self.inside(&r.outer, self.pos[v]) && best.map_or(true, |b| areas[k] < areas[b]) {
                    best = Some(k);
                }
            }
            if let Some(k) = best {
                regions[k].holes.push(lp.clone());
            }
        }
        regions
    }

    /// The directed edges of a loop of shape vertices, as local indices; `None` if a vertex is off
    /// the plane.
    pub fn directed(&self, lp: &[usize]) -> Option<Vec<(usize, usize)>> {
        let l: Option<Vec<usize>> = lp.iter().map(|&v| self.local(v)).collect();
        let l = l?;
        let n = l.len();
        Some((0..n).map(|i| (l[i], l[(i + 1) % n])).collect())
    }

    /// Shape vertex indices of a local loop.
    pub fn to_shape(&self, lp: &[usize]) -> Vec<usize> {
        lp.iter().map(|&i| self.verts[i]).collect()
    }
}

/// Splits a closed walk (which may revisit vertices) into simple loops: whenever a vertex repeats,
/// the stretch since its previous visit is one loop. Stretches of fewer than three vertices (an
/// edge walked there and back) are dropped.
pub fn decompose(walk: &[usize]) -> Vec<Vec<usize>> {
    let n = walk.len();
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut at: BTreeMap<usize, usize> = BTreeMap::new();
    for i in 0..=n {
        let v = walk[i % n];
        if let Some(&p) = at.get(&v) {
            let lp: Vec<usize> = stack[p..].to_vec();
            for &x in &stack[p + 1..] {
                at.remove(&x);
            }
            stack.truncate(p + 1);
            if lp.len() >= 3 {
                out.push(lp);
            }
        } else {
            at.insert(v, stack.len());
            stack.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xy(pts: &[[f64; 2]]) -> Vec<V3> {
        pts.iter().map(|p| [p[0], p[1], 0.0]).collect()
    }

    fn graph(pts: &[[f64; 2]], edges: &[(usize, usize)]) -> PlanarGraph {
        let plane = Plane::new([0.0, 0.0, 1.0], [0.0, 0.0, 0.0]);
        PlanarGraph::new(&plane, &xy(pts), edges)
    }

    #[test]
    fn decompose_splits_pinched_walks() {
        assert_eq!(decompose(&[0, 1, 2, 3]), vec![vec![0, 1, 2, 3]]);
        assert_eq!(decompose(&[0, 1, 5, 1, 2, 3]), vec![vec![0, 1, 2, 3]], "spur dropped");
        assert_eq!(decompose(&[0, 1, 2, 7, 8, 2, 3]), vec![vec![2, 7, 8], vec![0, 1, 2, 3]]);
    }

    #[test]
    fn two_squares_sharing_an_edge() {
        // (0,0)=0 (1,0)=1 (2,0)=2 (2,1)=3 (1,1)=4 (0,1)=5
        let g = graph(&[[0., 0.], [1., 0.], [2., 0.], [2., 1.], [1., 1.], [0., 1.]], &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0), (1, 4)]);
        let left = g.regions_of_walk(&g.walk(5, 0));
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].outer, vec![5, 0, 1, 4]);
        assert!(left[0].holes.is_empty());
        let outer = g.walk(0, 5);
        assert!(g.area(&outer) < 0.0);
        assert!(g.regions_of_walk(&outer).is_empty());
        let right = g.regions_of_walk(&g.walk(4, 1));
        assert_eq!(right[0].outer, vec![4, 1, 2, 3]);
        assert!(right[0].has_edge(1, 4) && !right[0].has_edge(0, 1));
        assert!((g.area(&right[0].outer) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn island_becomes_a_hole() {
        // Outer 4x3 rect (0..3) and an inner unit square (4..7), disconnected.
        let g = graph(
            &[[0., 0.], [4., 0.], [4., 3.], [0., 3.], [1., 1.], [2., 1.], [2., 2.], [1., 2.]],
            &[(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4)],
        );
        let r = g.regions_of_walk(&g.walk(0, 1));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].holes.len(), 1);
        assert!(g.area(&r[0].holes[0]) < 0.0, "holes are clockwise");
        let p = g.interior_point(&r[0]).unwrap();
        assert!(g.region_contains(&r[0], p));
        // The inner square alone: its other side is the unbounded face of its component.
        let inner = g.regions_of_walk(&g.walk(4, 5));
        assert_eq!(inner.len(), 1);
        assert!(inner[0].holes.is_empty());
        assert!(g.regions_of_walk(&g.walk(5, 4)).is_empty());
    }

    #[test]
    fn link_cancels_shared_edges_and_builds_the_union() {
        let g = graph(&[[0., 0.], [1., 0.], [2., 0.], [2., 1.], [1., 1.], [0., 1.]], &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0), (1, 4)]);
        let a = [(0, 1), (1, 4), (4, 5), (5, 0)];
        let b = [(1, 2), (2, 3), (3, 4), (4, 1)];
        let loops = g.link(&[&a[..], &b[..]].concat());
        assert_eq!(loops.len(), 1);
        let regions = g.assemble(loops);
        assert_eq!(regions[0].outer.len(), 6);
        assert!((g.area(&regions[0].outer) - 2.0).abs() < 1e-12);
        // A difference: the big rect minus the island, opposite orientations as stored.
        let g = graph(
            &[[0., 0.], [4., 0.], [4., 3.], [0., 3.], [1., 1.], [2., 1.], [2., 2.], [1., 2.]],
            &[(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4)],
        );
        let outer = [(0, 1), (1, 2), (2, 3), (3, 0)];
        let hole = [(4, 7), (7, 6), (6, 5), (5, 4)];
        let regions = g.assemble(g.link(&[&outer[..], &hole[..]].concat()));
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].holes, vec![vec![4, 7, 6, 5]]);
    }
}
