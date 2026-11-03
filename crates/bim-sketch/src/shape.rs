//! The `Shape` data model of spec/sketch.md: an index-stable vertex table, faces (loops of vertex
//! indices with stable ids) and the edge set, read from and written to the entity's props.
//!
//! Invariants every operation restores through [`Shape::normalize`]: vertices are pairwise
//! further apart than [`EPS`]; no vertex lies in the interior of an edge (edges are split, never
//! left as T-junctions); every face loop has at least three distinct vertices and non-zero area;
//! and every loop edge is in [`Shape::edges`].
//!
//! Vertex indices are stable (spec/sketch.md: "Operations keep vertex indices; new vertices
//! (including copies) are appended"): an operation never removes or renumbers a vertex, so a
//! vertex whose edges are all erased, or that a push leaves behind, stays in `vertices` at its
//! index as an orphan on no edge. Orphans are plain points: an operation reuses one when a new
//! point lands on it ([`Shape::vertex`]), and they are left out of the glTF mesh.

use std::collections::{BTreeMap, BTreeSet};

use bim_doc::{Entity, EntityId, Value};

use crate::geom::{self, dist, newell, on_open_segment, param_on_line, unit, Plane, EPS, V3};

/// One face: a stable id and its loops (the outer loop first, then holes).
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub id: u64,
    pub loops: Vec<Vec<usize>>,
}

impl Face {
    /// The directed edges of every loop.
    pub fn directed_edges(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for lp in &self.loops {
            let n = lp.len();
            for i in 0..n {
                out.push((lp[i], lp[(i + 1) % n]));
            }
        }
        out
    }

    /// Whether the undirected edge `{a, b}` is on one of the loops.
    pub fn has_edge(&self, a: usize, b: usize) -> bool {
        self.loops.iter().any(|lp| crate::planar::loop_has_edge(lp, a, b))
    }

    /// The direction in which the face traverses the edge `{a, b}`: `Some(true)` for `a → b`,
    /// `Some(false)` for `b → a`, `None` when the edge is not on the face.
    pub fn direction(&self, a: usize, b: usize) -> Option<bool> {
        for lp in &self.loops {
            let n = lp.len();
            for i in 0..n {
                let (x, y) = (lp[i], lp[(i + 1) % n]);
                if (x, y) == (a, b) {
                    return Some(true);
                }
                if (x, y) == (b, a) {
                    return Some(false);
                }
            }
        }
        None
    }

    pub fn has_vertex(&self, v: usize) -> bool {
        self.loops.iter().any(|lp| lp.contains(&v))
    }

    /// Every vertex of the face, in loop order, once each.
    pub fn vertices(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for lp in &self.loops {
            for &v in lp {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
        out
    }
}

/// A Shape's geometry (its local frame; `transform` and the other optional placement props are
/// not read) and its paint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub vertices: Vec<V3>,
    pub faces: Vec<Face>,
    /// Every edge (face edges and loose ones), as `(lower, higher)` index pairs, in creation order.
    pub edges: Vec<(usize, usize)>,
    /// `face_products` keyed by face id: `None` while the Shape has no such prop, else the Product
    /// painted on each face that has one. Paint follows the face id, so a face that keeps its id
    /// keeps its paint; [`Shape::prune_products`] drops the entries of faces that are gone.
    pub products: Option<BTreeMap<u64, EntityId>>,
}

fn key(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
}

fn as_index(v: &Value, n: usize, what: &str) -> Result<usize, String> {
    match v {
        Value::Num(x) if x.is_finite() && x.fract() == 0.0 && *x >= 0.0 && (*x as usize) < n => Ok(*x as usize),
        other => Err(format!("{what}: expected a vertex index below {n}, found {other:?}")),
    }
}

fn num_list(v: Option<&Value>, what: &str) -> Result<Vec<f64>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::List(items)) => items
            .iter()
            .map(|x| match x {
                Value::Num(x) if x.is_finite() => Ok(*x),
                other => Err(format!("{what}: expected a finite number, found {other:?}")),
            })
            .collect(),
        Some(other) => Err(format!("{what}: expected a List, found {other:?}")),
    }
}

impl Shape {
    /// Reads the geometry props of a Shape entity (missing props are empty).
    pub fn from_entity(e: &Entity) -> Result<Shape, String> {
        let flat = num_list(e.props.get("vertices"), "vertices")?;
        if flat.len() % 3 != 0 {
            return Err(format!("vertices: {} numbers is not a list of x, y, z triples", flat.len()));
        }
        let vertices: Vec<V3> = flat.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
        let n = vertices.len();
        let ids: Vec<u64> = match e.props.get("face_ids") {
            None => Vec::new(),
            Some(Value::List(items)) => items
                .iter()
                .map(|x| match x {
                    Value::Num(x) if x.is_finite() && x.fract() == 0.0 && *x >= 0.0 => Ok(*x as u64),
                    other => Err(format!("face_ids: expected a non-negative integer, found {other:?}")),
                })
                .collect::<Result<_, _>>()?,
            Some(other) => return Err(format!("face_ids: expected a List, found {other:?}")),
        };
        let mut faces = Vec::new();
        match e.props.get("faces") {
            None => {}
            Some(Value::List(items)) => {
                if items.len() != ids.len() {
                    return Err(format!("faces has {} entries but face_ids has {}", items.len(), ids.len()));
                }
                for (k, f) in items.iter().enumerate() {
                    let Value::List(loops) = f else { return Err(format!("faces[{k}]: expected a List of loops")) };
                    let mut out = Vec::new();
                    for lp in loops {
                        let Value::List(ix) = lp else { return Err(format!("faces[{k}]: expected a loop List")) };
                        let lp: Vec<usize> = ix.iter().map(|i| as_index(i, n, &format!("faces[{k}]"))).collect::<Result<_, _>>()?;
                        if lp.len() < 3 {
                            return Err(format!("faces[{k}]: a loop needs at least 3 vertices"));
                        }
                        out.push(lp);
                    }
                    if out.is_empty() {
                        return Err(format!("faces[{k}]: a face needs an outer loop"));
                    }
                    faces.push(Face { id: ids[k], loops: out });
                }
            }
            Some(other) => return Err(format!("faces: expected a List, found {other:?}")),
        }
        let distinct: BTreeSet<u64> = ids.iter().copied().collect();
        if distinct.len() != ids.len() {
            return Err("face_ids are not distinct".to_string());
        }
        let loose = num_list(e.props.get("edges"), "edges")?;
        if loose.len() % 2 != 0 {
            return Err("edges: an odd number of indices is not a list of pairs".to_string());
        }
        // `face_products` is read leniently (it only colours faces): a Ref paints the face at the
        // same position, anything else leaves it unpainted.
        let products = match e.props.get("face_products") {
            Some(Value::List(items)) => Some(
                faces
                    .iter()
                    .zip(items)
                    .filter_map(|(f, v): (&Face, &Value)| match v {
                        Value::Ref(p) => Some((f.id, *p)),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        };
        let mut shape = Shape { vertices, faces, edges: Vec::new(), products };
        for f in 0..shape.faces.len() {
            for (a, b) in shape.faces[f].directed_edges() {
                shape.add_edge(a, b);
            }
        }
        for pair in loose.chunks(2) {
            let a = as_index(&Value::Num(pair[0]), n, "edges")?;
            let b = as_index(&Value::Num(pair[1]), n, "edges")?;
            if a == b {
                return Err("edges: a degenerate pair".to_string());
            }
            shape.add_edge(a, b);
        }
        Ok(shape)
    }

    /// The geometry props: `vertices`, `faces`, `face_ids` and `edges` (the loose ones), in that
    /// order, then `face_products` (one entry per face: a Ref or `""`) once the Shape has paint.
    pub fn props(&self) -> Vec<(String, Value)> {
        let clean = |x: f64| Value::Num(if x == 0.0 { 0.0 } else { x });
        let vertices = Value::List(self.vertices.iter().flat_map(|v| v.iter().map(|&x| clean(x))).collect());
        let faces = Value::List(
            self.faces
                .iter()
                .map(|f| Value::List(f.loops.iter().map(|lp| Value::List(lp.iter().map(|&i| Value::Num(i as f64)).collect())).collect()))
                .collect(),
        );
        let ids = Value::List(self.faces.iter().map(|f| Value::Num(f.id as f64)).collect());
        let on_faces = self.loop_edges();
        let loose = Value::List(
            self.edges
                .iter()
                .filter(|e| !on_faces.contains(e))
                .flat_map(|&(a, b)| [Value::Num(a as f64), Value::Num(b as f64)])
                .collect(),
        );
        let mut out = vec![("vertices".into(), vertices), ("faces".into(), faces), ("face_ids".into(), ids), ("edges".into(), loose)];
        if let Some(products) = &self.products {
            let list = self.faces.iter().map(|f| products.get(&f.id).map_or_else(|| Value::Text(String::new()), |p| Value::Ref(*p))).collect();
            out.push(("face_products".into(), Value::List(list)));
        }
        out
    }

    /// The Product painted on face `id`, if any.
    pub fn product(&self, id: u64) -> Option<EntityId> {
        self.products.as_ref().and_then(|p| p.get(&id).copied())
    }

    /// Paints face `id` with `product` (`None` clears it).
    pub fn set_product(&mut self, id: u64, product: Option<EntityId>) {
        let map = self.products.get_or_insert_with(BTreeMap::new);
        match product {
            Some(p) => {
                map.insert(id, p);
            }
            None => {
                map.remove(&id);
            }
        }
    }

    /// Drops the paint of faces the Shape no longer has, so a later face given the same id does
    /// not inherit it.
    pub fn prune_products(&mut self) {
        let ids: BTreeSet<u64> = self.faces.iter().map(|f| f.id).collect();
        if let Some(p) = &mut self.products {
            p.retain(|id, _| ids.contains(id));
        }
    }

    // ------------------------------------------------------------------ queries

    /// The index of the vertex within [`EPS`] of `p`, if any (the lowest such index).
    pub fn find_vertex(&self, p: V3) -> Option<usize> {
        self.vertices.iter().position(|&v| dist(v, p) <= EPS)
    }

    /// The vertex at `p`: an existing one within [`EPS`], or a new one appended.
    pub fn vertex(&mut self, p: V3) -> usize {
        self.find_vertex(p).unwrap_or_else(|| {
            self.vertices.push(p);
            self.vertices.len() - 1
        })
    }

    pub fn has_edge(&self, a: usize, b: usize) -> bool {
        self.edges.contains(&key(a, b))
    }

    /// Adds the edge unless it exists; true when it was added.
    pub fn add_edge(&mut self, a: usize, b: usize) -> bool {
        if a == b || self.has_edge(a, b) {
            return false;
        }
        self.edges.push(key(a, b));
        true
    }

    pub fn remove_edge(&mut self, a: usize, b: usize) {
        self.edges.retain(|&e| e != key(a, b));
    }

    /// The edges lying on face loops.
    pub fn loop_edges(&self) -> BTreeSet<(usize, usize)> {
        self.faces.iter().flat_map(|f| f.directed_edges().into_iter().map(|(a, b)| key(a, b))).collect()
    }

    /// The loose edges (on no face) at vertex `v`.
    pub fn loose_edges_at(&self, v: usize) -> Vec<(usize, usize)> {
        let on_faces = self.loop_edges();
        self.edges.iter().copied().filter(|&e| (e.0 == v || e.1 == v) && !on_faces.contains(&e)).collect()
    }

    /// The edges at vertex `v`, as their other endpoints.
    pub fn neighbours(&self, v: usize) -> Vec<usize> {
        self.edges.iter().filter_map(|&(a, b)| if a == v { Some(b) } else if b == v { Some(a) } else { None }).collect()
    }

    pub fn face_by_id(&self, id: u64) -> Option<usize> {
        self.faces.iter().position(|f| f.id == id)
    }

    /// The id for the next new face: the largest id + 1, or 1.
    pub fn next_face_id(&self) -> u64 {
        self.faces.iter().map(|f| f.id).max().map_or(1, |m| m + 1)
    }

    pub fn faces_with_edge(&self, a: usize, b: usize) -> Vec<usize> {
        (0..self.faces.len()).filter(|&f| self.faces[f].has_edge(a, b)).collect()
    }

    pub fn faces_with_vertex(&self, v: usize) -> Vec<usize> {
        (0..self.faces.len()).filter(|&f| self.faces[f].has_vertex(v)).collect()
    }

    pub fn loop_points(&self, lp: &[usize]) -> Vec<V3> {
        lp.iter().map(|&i| self.vertices[i]).collect()
    }

    /// The unit normal of face `f` by its outer loop (right-hand rule); `None` for a degenerate loop.
    pub fn face_normal(&self, f: usize) -> Option<V3> {
        unit(newell(&self.loop_points(&self.faces[f].loops[0])))
    }

    pub fn face_plane(&self, f: usize) -> Option<Plane> {
        Plane::from_loop(&self.loop_points(&self.faces[f].loops[0]))
    }

    /// Whether every vertex of face `f` lies on `plane`.
    pub fn face_on_plane(&self, f: usize, plane: &Plane) -> bool {
        self.faces[f].loops.iter().flatten().all(|&v| plane.contains(self.vertices[v]))
    }

    // ------------------------------------------------------------------ invariants

    /// Restores the invariants after an operation moved, added or merged geometry: merges vertices
    /// closer than [`EPS`] (the lower index survives), splits every edge at the vertices lying on
    /// it (in every loop and loose edge that holds it), drops loops with fewer than three distinct
    /// vertices or no area (a face whose outer loop goes is removed) and makes sure every loop
    /// edge is in [`Shape::edges`]. Vertex indices are kept: nothing is removed or renumbered.
    ///
    /// The operations never create a vertex within [`EPS`] of another (they reuse the existing
    /// one instead), so the merge here is a safety net for geometry that arrives coincident, such
    /// as hand-written props. When it fires, the merged index is left in place as an orphan that
    /// still coincides with the survivor rather than shifting every later index.
    pub fn normalize(&mut self) {
        // 1. Merge coincident vertices.
        let n = self.vertices.len();
        let mut map: Vec<usize> = (0..n).collect();
        for j in 0..n {
            for i in 0..j {
                if map[i] == i && dist(self.vertices[i], self.vertices[j]) <= EPS {
                    map[j] = i;
                    break;
                }
            }
        }
        if map.iter().enumerate().any(|(i, &m)| m != i) {
            self.remap(&map);
        }
        // 2. Split edges at the vertices lying on them.
        let mut chains: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
        for &(a, b) in &self.edges {
            let (pa, pb) = (self.vertices[a], self.vertices[b]);
            let mut on: Vec<(f64, usize)> = (0..n)
                .filter(|&v| v != a && v != b && map[v] == v && on_open_segment(self.vertices[v], pa, pb))
                .map(|v| (param_on_line(self.vertices[v], pa, pb), v))
                .collect();
            if on.is_empty() {
                continue;
            }
            on.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            chains.insert((a, b), on.into_iter().map(|(_, v)| v).collect());
        }
        if !chains.is_empty() {
            for f in &mut self.faces {
                for lp in &mut f.loops {
                    let mut out = Vec::with_capacity(lp.len());
                    let m = lp.len();
                    for i in 0..m {
                        let (x, y) = (lp[i], lp[(i + 1) % m]);
                        out.push(x);
                        if let Some(chain) = chains.get(&key(x, y)) {
                            if x < y {
                                out.extend(chain.iter().copied());
                            } else {
                                out.extend(chain.iter().rev().copied());
                            }
                        }
                    }
                    *lp = out;
                }
            }
            let mut edges = Vec::with_capacity(self.edges.len());
            for &(a, b) in &self.edges {
                match chains.get(&(a, b)) {
                    Some(chain) => {
                        let mut prev = a;
                        for &v in chain.iter().chain(std::iter::once(&b)) {
                            edges.push(key(prev, v));
                            prev = v;
                        }
                    }
                    None => edges.push((a, b)),
                }
            }
            self.edges = edges;
        }
        // 3. Clean loops, split pinched ones into simple loops and drop degenerate ones.
        self.faces.retain_mut(|f| {
            f.loops.retain_mut(|lp| {
                clean_loop(lp);
                lp.len() >= 3
            });
            !f.loops.is_empty()
        });
        self.simplify_loops();
        let vertices = self.vertices.clone();
        self.faces.retain_mut(|f| {
            f.loops.retain(|lp| !flat(&lp.iter().map(|&i| vertices[i]).collect::<Vec<_>>()));
            !f.loops.is_empty()
        });
        // 4. Sync edges.
        let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
        let mut edges = Vec::with_capacity(self.edges.len());
        for &(a, b) in &self.edges {
            if a != b && seen.insert(key(a, b)) {
                edges.push(key(a, b));
            }
        }
        for f in &self.faces {
            for (a, b) in f.directed_edges() {
                if seen.insert(key(a, b)) {
                    edges.push(key(a, b));
                }
            }
        }
        self.edges = edges;
    }

    /// Splits every loop that revisits a vertex (a face stretched or shrunk onto itself) into
    /// simple loops: the piece of an outer loop holding the face's first vertex (else the largest)
    /// stays its outer loop, pieces wound the same way become new faces and pieces wound the
    /// other way its holes; the pieces of a hole are holes. Pieces without area (spurs) go.
    fn simplify_loops(&mut self) {
        let mut extra: Vec<Face> = Vec::new();
        let vertices = self.vertices.clone();
        let pieces_of = |lp: &[usize]| -> Vec<Vec<usize>> {
            let distinct: BTreeSet<usize> = lp.iter().copied().collect();
            if distinct.len() == lp.len() {
                return vec![lp.to_vec()];
            }
            crate::planar::decompose(lp)
                .into_iter()
                .filter(|p| p.len() >= 3 && !flat(&p.iter().map(|&i| vertices[i]).collect::<Vec<_>>()))
                .collect()
        };
        let mut next_id = self.next_face_id();
        for f in &mut self.faces {
            let first = f.loops[0][0];
            let mut loops: Vec<Vec<usize>> = Vec::new();
            for (k, lp) in f.loops.iter().enumerate() {
                let pieces = pieces_of(lp);
                if k == 0 {
                    let Some(main) = pieces
                        .iter()
                        .position(|p| p.contains(&first))
                        .or_else(|| {
                            pieces.iter().enumerate().max_by(|a, b| {
                                let area = |p: &Vec<usize>| geom::norm(newell(&p.iter().map(|&i| vertices[i]).collect::<Vec<_>>()));
                                area(a.1).total_cmp(&area(b.1))
                            }).map(|(i, _)| i)
                        })
                    else {
                        break;
                    };
                    let n = newell(&pieces[main].iter().map(|&i| vertices[i]).collect::<Vec<_>>());
                    loops.push(pieces[main].clone());
                    for (i, p) in pieces.iter().enumerate() {
                        if i == main {
                            continue;
                        }
                        let m = newell(&p.iter().map(|&i| vertices[i]).collect::<Vec<_>>());
                        if geom::dot(m, n) > 0.0 {
                            extra.push(Face { id: next_id, loops: vec![p.clone()] });
                            next_id += 1;
                        } else {
                            loops.push(p.clone());
                        }
                    }
                } else {
                    loops.extend(pieces);
                }
            }
            f.loops = loops;
        }
        self.faces.retain(|f| !f.loops.is_empty());
        self.faces.extend(extra);
    }

    /// Replaces every vertex index `i` by `map[i]` (merging), then cleans loops and edges.
    fn remap(&mut self, map: &[usize]) {
        for f in &mut self.faces {
            for lp in &mut f.loops {
                for v in lp.iter_mut() {
                    *v = map[*v];
                }
                clean_loop(lp);
            }
        }
        let mut seen = BTreeSet::new();
        let mut edges = Vec::with_capacity(self.edges.len());
        for &(a, b) in &self.edges {
            let (a, b) = (map[a], map[b]);
            if a != b && seen.insert(key(a, b)) {
                edges.push(key(a, b));
            }
        }
        self.edges = edges;
    }

    /// Makes every connected group of faces that encloses a volume face outward: where each edge
    /// of the group is on exactly two of its faces, the faces are reoriented to agree across every
    /// edge (starting from the group's first face), and the whole group is turned inside out if its
    /// volume is negative. Groups with non-manifold edges or that cannot be oriented are left alone.
    /// Ids and vertices are untouched; only loop directions change.
    pub fn orient_closed(&mut self) {
        let n = self.faces.len();
        let mut by_edge: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
        for f in 0..n {
            for (a, b) in self.faces[f].directed_edges() {
                by_edge.entry(key(a, b)).or_default().push(f);
            }
        }
        let mut comp = vec![usize::MAX; n];
        let mut ncomp = 0;
        for start in 0..n {
            if comp[start] != usize::MAX {
                continue;
            }
            let mut stack = vec![start];
            comp[start] = ncomp;
            while let Some(f) = stack.pop() {
                for (a, b) in self.faces[f].directed_edges() {
                    for &g in &by_edge[&key(a, b)] {
                        if comp[g] == usize::MAX {
                            comp[g] = ncomp;
                            stack.push(g);
                        }
                    }
                }
            }
            ncomp += 1;
        }
        for c in 0..ncomp {
            let members: Vec<usize> = (0..n).filter(|&f| comp[f] == c).collect();
            let manifold = members.iter().all(|&f| self.faces[f].directed_edges().iter().all(|&(a, b)| by_edge[&key(a, b)].len() == 2));
            if !manifold {
                continue;
            }
            // Propagate orientation: a shared edge must be traversed both ways.
            let mut flip: BTreeMap<usize, bool> = BTreeMap::new();
            flip.insert(members[0], false);
            let mut stack = vec![members[0]];
            let mut orientable = true;
            while let Some(f) = stack.pop() {
                let ff = flip[&f];
                for (a, b) in self.faces[f].directed_edges() {
                    for &g in &by_edge[&key(a, b)] {
                        if g == f {
                            continue;
                        }
                        // g agrees with f when it traverses the edge the other way (before flips).
                        let agrees = self.faces[g].direction(a, b) == Some(false);
                        let want = if agrees { ff } else { !ff };
                        match flip.get(&g) {
                            Some(&have) if have != want => orientable = false,
                            Some(_) => {}
                            None => {
                                flip.insert(g, want);
                                stack.push(g);
                            }
                        }
                    }
                }
            }
            if !orientable || flip.len() != members.len() {
                continue;
            }
            let mut part = Shape { vertices: self.vertices.clone(), faces: members.iter().map(|&f| self.faces[f].clone()).collect(), edges: Vec::new(), products: None };
            for (k, &f) in members.iter().enumerate() {
                if flip[&f] {
                    part.faces[k].loops.iter_mut().for_each(|lp| lp.reverse());
                }
            }
            if !part.is_closed() {
                continue;
            }
            let inside_out = part.volume() < 0.0;
            for &f in &members {
                if flip[&f] != inside_out {
                    for lp in &mut self.faces[f].loops {
                        lp.reverse();
                        if let Some(k) = lp.iter().enumerate().min_by_key(|(_, &v)| v).map(|(k, _)| k) {
                            lp.rotate_left(k);
                        }
                    }
                }
            }
        }
    }

    /// The signed volume enclosed by the faces (exact for a closed, outward-oriented shape).
    pub fn volume(&self) -> f64 {
        let mut v = 0.0;
        for f in &self.faces {
            for lp in &f.loops {
                let p = self.loop_points(lp);
                for k in 1..p.len().saturating_sub(1) {
                    v += geom::dot(p[0], geom::cross(p[k], p[k + 1]));
                }
            }
        }
        v / 6.0
    }

    /// spec/sketch.md: a solid when every edge is on exactly two faces, used in opposite directions.
    pub fn is_closed(&self) -> bool {
        if self.faces.is_empty() {
            return false;
        }
        let mut count: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for f in &self.faces {
            for e in f.directed_edges() {
                *count.entry(e).or_insert(0) += 1;
            }
        }
        count.iter().all(|(&(a, b), &n)| n == 1 && count.get(&(b, a)) == Some(&1))
    }
}

/// Removes consecutive (cyclically) repeated vertices from a loop.
pub fn clean_loop(lp: &mut Vec<usize>) {
    let mut out: Vec<usize> = Vec::with_capacity(lp.len());
    for &v in lp.iter() {
        if out.last() != Some(&v) {
            out.push(v);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    *lp = out;
}

/// Whether a loop of points has negligible area relative to its extent.
pub fn flat(points: &[V3]) -> bool {
    if points.len() < 3 {
        return true;
    }
    let mut lo = points[0];
    let mut hi = points[0];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let l2 = geom::dot(geom::sub(hi, lo), geom::sub(hi, lo));
    geom::norm(newell(points)) <= 2e-12 * l2.max(1e-18)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bim_doc::EntityId;

    fn entity(props: &[(&str, Value)]) -> Entity {
        Entity { id: EntityId::derive("s"), kind: "Shape".into(), props: props.iter().map(|(k, v)| (k.to_string(), v.clone())).collect() }
    }

    fn nums(xs: &[f64]) -> Value {
        Value::List(xs.iter().map(|&x| Value::Num(x)).collect())
    }

    #[test]
    fn round_trips_props() {
        let e = entity(&[
            ("vertices", nums(&[0., 0., 0., 1., 0., 0., 1., 1., 0., 0., 1., 0., 5., 5., 5.])),
            ("faces", Value::List(vec![Value::List(vec![nums(&[0., 1., 2., 3.])])])),
            ("face_ids", nums(&[1.])),
            ("edges", nums(&[0., 4.])),
        ]);
        let s = Shape::from_entity(&e).unwrap();
        assert_eq!(s.edges.len(), 5);
        let props = s.props();
        assert_eq!(props[3].1, nums(&[0., 4.]));
        assert_eq!(props[2].1, nums(&[1.]));
        assert!(!s.is_closed());
        assert!(Shape::from_entity(&entity(&[("vertices", nums(&[1., 2.]))])).is_err());
        assert!(Shape::from_entity(&entity(&[("faces", Value::List(vec![])), ("face_ids", nums(&[1.]))])).is_err());
    }

    #[test]
    fn normalize_merges_and_splits() {
        let mut s = Shape {
            vertices: vec![[0., 0., 0.], [4., 0., 0.], [4., 3., 0.], [0., 3., 0.], [2., 0., 0.], [2., 3., 0.], [2., 3., 4e-10]],
            faces: vec![Face { id: 1, loops: vec![vec![0, 1, 2, 3]] }],
            edges: vec![(0, 1), (1, 2), (2, 3), (0, 3), (4, 6)],
            products: None,
        };
        s.normalize();
        assert_eq!(s.faces[0].loops[0], vec![0, 4, 1, 2, 5, 3], "T-junctions split in the loop");
        assert!(s.has_edge(0, 4) && s.has_edge(4, 1) && !s.has_edge(0, 1));
        assert!(s.has_edge(4, 5) && !s.has_edge(4, 6), "merged vertex 6 into 5");
        assert_eq!(s.vertices.len(), 7, "indices are kept: the merged vertex stays as an orphan");
        // Degenerate faces go.
        let mut t = Shape {
            vertices: vec![[0., 0., 0.], [1., 0., 0.], [2., 0., 0.]],
            faces: vec![Face { id: 1, loops: vec![vec![0, 1, 2]] }],
            edges: vec![],
            products: None,
        };
        t.normalize();
        assert!(t.faces.is_empty());
    }
}
