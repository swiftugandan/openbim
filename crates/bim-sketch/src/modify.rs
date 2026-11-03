//! The modifying operations of spec/sketch.md on a [`Shape`]: `offset`, `followme`,
//! `move` / `rotate` / `scale` (stretching connected faces, the triangle-fan rule for faces made
//! non-planar, `copy` and `array`) and `paint`.

use std::collections::BTreeSet;

use bim_doc::EntityId;
use bim_geom::Point2;

use crate::command::{Selection, Xf};
use crate::geom::{add, cross, dist, dot, is_finite, scale, sub, unit, ANG_EPS, EPS, V3};
use crate::ops::{find_faces, OpError};
use crate::shape::{clean_loop, Face, Shape};

fn rotate_to_min(lp: &mut [usize]) {
    if let Some(k) = lp.iter().enumerate().min_by_key(|(_, &v)| v).map(|(k, _)| k) {
        lp.rotate_left(k);
    }
}

/// Reverses a loop's winding, keeping its first vertex in front.
fn reverse_keeping_first(lp: &mut [usize]) {
    if lp.len() > 1 {
        lp[1..].reverse();
    }
}

// ------------------------------------------------------------------ 2D helpers

fn cross2(o: Point2, a: Point2, b: Point2) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

fn dist_to_segment(p: Point2, a: Point2, b: Point2) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (fx, fy) = (a.x + t * dx - p.x, a.y + t * dy - p.y);
    (fx * fx + fy * fy).sqrt()
}

/// Whether the closed segments `ab` and `cd` meet (cross, or come within [`EPS`]).
fn segments_meet(a: Point2, b: Point2, c: Point2, d: Point2) -> bool {
    let (o1, o2, o3, o4) = (cross2(a, b, c), cross2(a, b, d), cross2(c, d, a), cross2(c, d, b));
    if ((o1 > 0.0 && o2 < 0.0) || (o1 < 0.0 && o2 > 0.0)) && ((o3 > 0.0 && o4 < 0.0) || (o3 < 0.0 && o4 > 0.0)) {
        return true;
    }
    dist_to_segment(c, a, b) <= EPS || dist_to_segment(d, a, b) <= EPS || dist_to_segment(a, c, d) <= EPS || dist_to_segment(b, c, d) <= EPS
}

/// Whether a ring of 2D points is simple: no two non-adjacent edges meet, and adjacent edges meet
/// only at their shared vertex (no fold-back).
fn ring_is_simple(r: &[Point2]) -> bool {
    let n = r.len();
    for i in 0..n {
        let (a, b) = (r[i], r[(i + 1) % n]);
        for j in i + 1..n {
            let (c, d) = (r[j], r[(j + 1) % n]);
            let adjacent = j == i + 1 || (i == 0 && j == n - 1);
            if adjacent {
                // The far end of each edge must not lie on the other edge.
                let (fa, fb) = if j == i + 1 { (a, d) } else { (b, c) };
                if dist_to_segment(fa, c, d) <= EPS || dist_to_segment(fb, a, b) <= EPS {
                    return false;
                }
            } else if segments_meet(a, b, c, d) {
                return false;
            }
        }
    }
    true
}

/// Even-odd point in ring.
fn inside_ring(r: &[Point2], p: Point2) -> bool {
    let n = r.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (r[i], r[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

fn rings_meet(r: &[Point2], s: &[Point2]) -> bool {
    let (n, m) = (r.len(), s.len());
    (0..n).any(|i| (0..m).any(|j| segments_meet(r[i], r[(i + 1) % n], s[j], s[(j + 1) % m])))
}

fn signed_area2(r: &[Point2]) -> f64 {
    let n = r.len();
    (0..n).map(|i| r[i].x * r[(i + 1) % n].y - r[(i + 1) % n].x * r[i].y).sum::<f64>() / 2.0
}

// ------------------------------------------------------------------ offset

/// `offset`: a loop at `distance` inside (positive) or outside (negative) the outer loop of face
/// `id`, mitred at every vertex. Inward the face becomes a ring around a new inner face (which
/// takes the holes the new loop encloses); outward the face is unchanged and a new ring face
/// surrounds it (the face keeps its holes). The offset loop must be simple, keep every edge (no
/// edge shrinks away) and, inward, stay inside the face and off its holes.
pub fn offset(shape: &mut Shape, id: u64, distance: f64) -> Result<(), OpError> {
    if !distance.is_finite() || distance.abs() <= EPS {
        return Err("offset: `distance` must be a non-zero finite number".into());
    }
    let f = shape.face_by_id(id).ok_or_else(|| format!("offset: the shape has no face {id}"))?;
    let plane = shape.face_plane(f).ok_or_else(|| format!("offset: face {id} is degenerate"))?;
    let n = plane.n;
    let outer = shape.faces[f].loops[0].clone();
    let p = shape.loop_points(&outer);
    let m = p.len();
    let mut q: Vec<V3> = Vec::with_capacity(m);
    for i in 0..m {
        let prev = p[(i + m - 1) % m];
        let next = p[(i + 1) % m];
        let (Some(e1), Some(e2)) = (unit(sub(p[i], prev)), unit(sub(next, p[i]))) else {
            return Err(format!("offset: face {id} has a zero-length edge"));
        };
        let (l1, l2) = (cross(n, e1), cross(n, e2));
        let denom = 1.0 + dot(l1, l2);
        if denom <= 1e-9 {
            return Err(format!("offset: face {id} folds back on itself at vertex {}", outer[i]));
        }
        q.push(add(p[i], scale(add(l1, l2), distance / denom)));
    }
    // Validity in the face's plane.
    let p2: Vec<Point2> = p.iter().map(|&x| plane.project(x)).collect();
    let q2: Vec<Point2> = q.iter().map(|&x| plane.project(x)).collect();
    let too_far = || format!("offset: distance {distance} is too large for face {id}");
    for i in 0..m {
        let j = (i + 1) % m;
        let (de, dq) = ((p2[j].x - p2[i].x, p2[j].y - p2[i].y), (q2[j].x - q2[i].x, q2[j].y - q2[i].y));
        if de.0 * dq.0 + de.1 * dq.1 <= 0.0 || (dq.0 * dq.0 + dq.1 * dq.1).sqrt() <= EPS {
            return Err(too_far());
        }
    }
    if !ring_is_simple(&q2) || signed_area2(&q2) * signed_area2(&p2) <= 0.0 || rings_meet(&p2, &q2) {
        return Err(too_far());
    }
    let inward = distance > 0.0;
    if inward != inside_ring(&p2, q2[0]) {
        return Err(too_far());
    }
    // Holes. Outward, the new loop surrounds the outer loop without meeting it, so it surrounds the
    // face's holes too (they lie inside the outer loop) and they stay on the unchanged face. Inward,
    // each hole must lie wholly inside the new loop (it moves to the inner face) or outside it.
    let mut inside_holes: Vec<Vec<usize>> = Vec::new();
    let mut outside_holes: Vec<Vec<usize>> = Vec::new();
    if inward {
        for h in shape.faces[f].loops[1..].to_vec() {
            let h2: Vec<Point2> = h.iter().map(|&v| plane.project(shape.vertices[v])).collect();
            if rings_meet(&h2, &q2) || inside_ring(&h2, q2[0]) {
                return Err(format!("offset: the offset loop of face {id} crosses or lies in one of its holes"));
            }
            if inside_ring(&q2, h2[0]) {
                inside_holes.push(h);
            } else {
                outside_holes.push(h);
            }
        }
    }
    let ql: Vec<usize> = q.iter().map(|&x| shape.vertex(x)).collect();
    if ql.iter().collect::<BTreeSet<_>>().len() != m || ql.iter().any(|v| outer.contains(v)) {
        return Err(too_far());
    }
    let mut hole = ql.clone();
    hole.reverse();
    rotate_to_min(&mut hole);
    let new_id = shape.next_face_id();
    if inward {
        let mut loops = vec![outer];
        loops.extend(outside_holes);
        loops.push(hole);
        shape.faces[f].loops = loops;
        let mut loops = vec![ql];
        loops.extend(inside_holes);
        shape.faces.push(Face { id: new_id, loops });
        // The inner face is part of the painted surface.
        if let Some(prod) = shape.product(id) {
            shape.set_product(new_id, Some(prod));
        }
    } else {
        let mut back = outer;
        back.reverse();
        rotate_to_min(&mut back);
        shape.faces.push(Face { id: new_id, loops: vec![ql, back] });
    }
    finish(shape);
    Ok(())
}

/// Restores the invariants and makes closed solids face outward.
fn finish(shape: &mut Shape) {
    shape.normalize();
    shape.orient_closed();
}

// ------------------------------------------------------------------ follow-me

/// `followme`: sweeps face `id` along the polyline `path`, whose first segment is perpendicular to
/// the face and starts on its plane. Each section is the previous one carried along the segment
/// onto the plane bisecting the joint (the plane perpendicular to the last segment at the end), so
/// every joint is mitred. One side face per profile edge per segment (new ids in that order); the
/// end cap is the face itself (it keeps its id). A face on no other face also gets its start cap
/// (the last new face), closing a solid; a face of a solid is replaced by the sweep, whose sides
/// join the solid along the face's edges.
pub fn followme(shape: &mut Shape, id: u64, path: &[V3]) -> Result<(), OpError> {
    if path.len() < 2 {
        return Err("followme: `path` needs at least two points".into());
    }
    if path.iter().any(|&p| !is_finite(p)) {
        return Err("followme: `path` coordinates must be finite".into());
    }
    let f = shape.face_by_id(id).ok_or_else(|| format!("followme: the shape has no face {id}"))?;
    let plane = shape.face_plane(f).ok_or_else(|| format!("followme: face {id} is degenerate"))?;
    if !plane.contains(path[0]) {
        return Err(format!("followme: the path must start on the plane of face {id}"));
    }
    let mut d: Vec<V3> = Vec::with_capacity(path.len() - 1);
    for k in 0..path.len() - 1 {
        if dist(path[k], path[k + 1]) <= EPS {
            return Err(format!("followme: path points {k} and {} coincide", k + 1));
        }
        d.push(unit(sub(path[k + 1], path[k])).expect("distinct points"));
    }
    if dot(d[0], plane.n).abs() < 1.0 - ANG_EPS {
        return Err(format!("followme: the first path segment must be perpendicular to face {id}"));
    }
    let nseg = d.len();
    let mut mitre: Vec<V3> = Vec::with_capacity(nseg);
    for k in 0..nseg {
        if k + 1 < nseg {
            mitre.push(unit(add(d[k], d[k + 1])).filter(|m| dot(*m, d[k]) > 1e-9).ok_or_else(|| format!("followme: the path turns back on itself at point {}", k + 1))?);
        } else {
            mitre.push(d[k]);
        }
    }
    let face = shape.faces[f].clone();
    let verts = face.vertices();
    let naked = face.directed_edges().iter().all(|&(x, y)| shape.faces_with_edge(x, y).iter().all(|&g| g == f));
    // sections[k][i]: the shape vertex of profile vertex verts[i] at the start of segment k (k = nseg: the end).
    let mut sections: Vec<Vec<usize>> = vec![verts.clone()];
    let mut cur: Vec<V3> = verts.iter().map(|&v| shape.vertices[v]).collect();
    for k in 0..nseg {
        let mut next = Vec::with_capacity(cur.len());
        for &c in &cur {
            let t = dot(sub(path[k + 1], c), mitre[k]) / dot(d[k], mitre[k]);
            if !(t > EPS) {
                return Err(format!("followme: path segment {k} is too short for the profile of face {id}"));
            }
            next.push(add(c, scale(d[k], t)));
        }
        let idx: Vec<usize> = next.iter().map(|&p| shape.vertex(p)).collect();
        if idx.iter().collect::<BTreeSet<_>>().len() != idx.len() {
            return Err(format!("followme: the sweep of face {id} collapses at path point {}", k + 1));
        }
        sections.push(idx);
        cur = next;
    }
    let at = |k: usize, v: usize| sections[k][verts.iter().position(|&x| x == v).expect("a face vertex")];
    let mut next_id = shape.next_face_id();
    let mut new_faces: Vec<Face> = Vec::new();
    for k in 0..nseg {
        for (x, y) in face.directed_edges() {
            let mut lp = vec![at(k, x), at(k, y), at(k + 1, y), at(k + 1, x)];
            clean_loop(&mut lp);
            if lp.len() >= 3 {
                new_faces.push(Face { id: next_id, loops: vec![lp] });
                next_id += 1;
            }
        }
    }
    // The end cap: the face carried to the last section.
    shape.faces[f].loops = face.loops.iter().map(|lp| lp.iter().map(|&v| at(nseg, v)).collect()).collect();
    if naked {
        let mut loops = face.loops.clone();
        for lp in &mut loops {
            lp.reverse();
            rotate_to_min(lp);
        }
        new_faces.push(Face { id: next_id, loops });
    }
    shape.faces.extend(new_faces);
    if !naked {
        // The face's own edges now bound the sweep's sides instead.
        for (x, y) in face.directed_edges() {
            if shape.faces_with_edge(x, y).is_empty() {
                shape.remove_edge(x, y);
            }
        }
    }
    finish(shape);
    Ok(())
}

// ------------------------------------------------------------------ move / rotate / scale

/// The point map of a `move`, `rotate` or `scale`.
struct Map {
    xf: Xf,
    /// Rotation matrix (rows) for `rotate`.
    r: [V3; 3],
}

impl Map {
    fn new(xf: Xf) -> Result<Map, OpError> {
        let mut r = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        match xf {
            Xf::Move { vector } => {
                if !is_finite(vector) {
                    return Err("move: `vector` must be finite".into());
                }
            }
            Xf::Rotate { centre, axis, angle } => {
                if !is_finite(centre) || !is_finite(axis) || !angle.is_finite() {
                    return Err("rotate: coordinates must be finite".into());
                }
                let k = unit(axis).ok_or("rotate: `axis` must not be zero")?;
                let (s, c) = angle.sin_cos();
                let kx = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
                for i in 0..3 {
                    for j in 0..3 {
                        let kk: f64 = (0..3).map(|m| kx[i][m] * kx[m][j]).sum();
                        r[i][j] = if i == j { 1.0 } else { 0.0 } + s * kx[i][j] + (1.0 - c) * kk;
                    }
                }
            }
            Xf::Scale { origin, factors } => {
                if !is_finite(origin) || !is_finite(factors) {
                    return Err("scale: coordinates must be finite".into());
                }
                if factors.iter().any(|&x| x == 0.0) {
                    return Err("scale: `factors` must not be zero".into());
                }
            }
        }
        Ok(Map { xf, r })
    }

    fn apply(&self, p: V3) -> V3 {
        match self.xf {
            Xf::Move { vector } => add(p, vector),
            Xf::Rotate { centre, .. } => {
                let q = sub(p, centre);
                add(centre, [dot(self.r[0], q), dot(self.r[1], q), dot(self.r[2], q)])
            }
            Xf::Scale { origin, factors } => {
                let q = sub(p, origin);
                add(origin, [q[0] * factors[0], q[1] * factors[1], q[2] * factors[2]])
            }
        }
    }

    /// Whether the map reverses orientation (a scale with an odd number of negative factors).
    fn mirrors(&self) -> bool {
        match self.xf {
            Xf::Scale { factors, .. } => factors.iter().filter(|&&x| x < 0.0).count() % 2 == 1,
            _ => false,
        }
    }
}

/// `move` / `rotate` / `scale`: transforms the selected vertices (those named, the ends of the
/// named edges and every vertex of the named faces). Without `copy` the vertices move in place
/// (indices kept), stretching every face that uses them; a face made non-planar is split into a
/// fan of triangles (see [`fan`]). With `copy`, the selection is copied `array` times, the nth
/// copy transformed n times: copied vertices are appended (or reuse a vertex already at the
/// point), copied faces take new ids after the existing ones (copy by copy, in face order) and
/// keep their paint, copied edges that bound no copied face close faces like drawn lines.
pub fn transform(shape: &mut Shape, sel: &Selection, xf: Xf, copy: bool, array: u64) -> Result<(), OpError> {
    let op = match xf {
        Xf::Move { .. } => "move",
        Xf::Rotate { .. } => "rotate",
        Xf::Scale { .. } => "scale",
    };
    let map = Map::new(xf)?;
    let nv = shape.vertices.len();
    let mut moved: BTreeSet<usize> = BTreeSet::new();
    for &v in &sel.vertices {
        if v >= nv {
            return Err(format!("{op}: the shape has no vertex {v}"));
        }
        moved.insert(v);
    }
    for &(a, b) in &sel.edges {
        if a >= nv || b >= nv || !shape.has_edge(a, b) {
            return Err(format!("{op}: the shape has no edge [{a}, {b}]"));
        }
        moved.insert(a);
        moved.insert(b);
    }
    let mut faces: Vec<usize> = Vec::new();
    for &id in &sel.faces {
        let f = shape.face_by_id(id).ok_or_else(|| format!("{op}: the shape has no face {id}"))?;
        if !faces.contains(&f) {
            faces.push(f);
        }
        moved.extend(shape.faces[f].vertices());
    }
    faces.sort_unstable();
    if !copy {
        for &v in &moved {
            shape.vertices[v] = map.apply(shape.vertices[v]);
        }
        fan(shape)?;
        finish(shape);
        return Ok(());
    }
    let originals: Vec<Face> = faces.iter().map(|&f| shape.faces[f].clone()).collect();
    let face_edges: BTreeSet<(usize, usize)> = originals.iter().flat_map(|f| f.directed_edges()).map(|(a, b)| (a.min(b), a.max(b))).collect();
    let loose_edges: Vec<(usize, usize)> = sel.edges.iter().map(|&(a, b)| (a.min(b), a.max(b))).filter(|e| !face_edges.contains(e)).collect();
    let mut pos: Vec<(usize, V3)> = moved.iter().map(|&v| (v, shape.vertices[v])).collect();
    let mut drawn: Vec<(usize, usize)> = Vec::new();
    for c in 1..=array {
        for entry in &mut pos {
            entry.1 = map.apply(entry.1);
        }
        if pos.iter().all(|&(v, p)| dist(p, shape.vertices[v]) <= EPS) {
            return Err(format!("{op}: copy {c} would coincide with the original"));
        }
        let index: Vec<(usize, usize)> = pos.iter().map(|&(v, p)| (v, shape.vertex(p))).collect();
        let to = |v: usize| index.iter().find(|&&(o, _)| o == v).map(|&(_, n)| n).expect("a selected vertex");
        let mirror = map.mirrors() && c % 2 == 1;
        for face in &originals {
            let mut loops: Vec<Vec<usize>> = face.loops.iter().map(|lp| lp.iter().map(|&v| to(v)).collect()).collect();
            if mirror {
                loops.iter_mut().for_each(|lp| reverse_keeping_first(lp));
            }
            let id = shape.next_face_id();
            shape.faces.push(Face { id, loops });
            if let Some(p) = shape.product(face.id) {
                shape.set_product(id, Some(p));
            }
        }
        for &(a, b) in &loose_edges {
            let (x, y) = (to(a), to(b));
            if shape.add_edge(x, y) {
                drawn.push((x, y));
            }
        }
    }
    shape.normalize();
    for (x, y) in drawn {
        if shape.has_edge(x, y) && shape.faces_with_edge(x, y).is_empty() {
            find_faces(shape, x, y);
        }
    }
    finish(shape);
    Ok(())
}

/// Splits every face made non-planar into triangles fanned from its lowest-index vertex
/// (spec/sketch.md): the fan runs along the outer loop from that vertex; the first triangle in fan
/// order holding the face's first outer-loop vertex keeps the id (and starts at that vertex), the
/// others take new ids in fan order (faces in face order) and keep the face's paint. A face with
/// holes cannot be fanned, so making one non-planar is invalid.
fn fan(shape: &mut Shape) -> Result<(), OpError> {
    let mut next_id = shape.next_face_id();
    let mut extra: Vec<Face> = Vec::new();
    for f in 0..shape.faces.len() {
        let Some(plane) = shape.face_plane(f) else { continue };
        if shape.face_on_plane(f, &plane) {
            continue;
        }
        let id = shape.faces[f].id;
        if shape.faces[f].loops.len() > 1 {
            return Err(format!("the transform would make face {id}, which has holes, non-planar"));
        }
        let lp = shape.faces[f].loops[0].clone();
        let first = lp[0];
        let mut rl = lp.clone();
        rotate_to_min(&mut rl);
        let tris: Vec<Vec<usize>> = (1..rl.len() - 1).map(|j| vec![rl[0], rl[j], rl[j + 1]]).collect();
        let keep = tris.iter().position(|t| t.contains(&first)).expect("every loop vertex is on a fan triangle");
        let product = shape.product(id);
        for (k, t) in tris.into_iter().enumerate() {
            if k == keep {
                let mut t = t;
                let at = t.iter().position(|&v| v == first).expect("holds the first vertex");
                t.rotate_left(at);
                shape.faces[f].loops = vec![t];
            } else {
                extra.push(Face { id: next_id, loops: vec![t] });
                if let Some(p) = product {
                    shape.set_product(next_id, Some(p));
                }
                next_id += 1;
            }
        }
    }
    shape.faces.extend(extra);
    Ok(())
}

// ------------------------------------------------------------------ paint

/// `paint`: sets the Product of each face in `faces` (`None` clears it).
pub fn paint(shape: &mut Shape, faces: &[u64], product: Option<EntityId>) -> Result<(), OpError> {
    for &id in faces {
        if shape.face_by_id(id).is_none() {
            return Err(format!("paint: the shape has no face {id}"));
        }
    }
    for &id in faces {
        shape.set_product(id, product);
    }
    Ok(())
}
