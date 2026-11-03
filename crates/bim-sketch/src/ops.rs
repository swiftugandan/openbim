//! The sketch operations of spec/sketch.md on a [`Shape`]: `line` (with edge splitting and face
//! finding), `rect`, `circle`, `arc`, `pushpull` and `erase`.

use std::collections::BTreeSet;

use crate::geom::{self, add, cross, dist, dot, is_finite, positive, scale, segments_cross, sub, unit, Plane, ANG_EPS, EPS, V3};
use crate::planar::{PlanarGraph, Region};
use crate::shape::{Face, Shape};

/// Why a command is invalid (exit 2 at the CLI, nothing changed).
pub type OpError = String;

// ------------------------------------------------------------------ drawing

/// `line`: adds the edge `from → to`, splitting the edges it crosses or touches, then finds the
/// faces the new edge pieces close (see [`find_faces`]).
pub fn line(shape: &mut Shape, from: V3, to: V3) -> Result<(), OpError> {
    if !is_finite(from) || !is_finite(to) {
        return Err("line: coordinates must be finite".into());
    }
    if dist(from, to) <= EPS {
        return Err("line: `from` and `to` are the same point".into());
    }
    let before = shape.edges.clone();
    let a = shape.vertex(from);
    let b = shape.vertex(to);
    if a == b {
        return Err("line: `from` and `to` are the same vertex".into());
    }
    let (pa, pb) = (shape.vertices[a], shape.vertices[b]);
    // Crossings with existing edges become vertices (both edges are split by `normalize`).
    for &(c, d) in &before {
        if c == a || c == b || d == a || d == b {
            continue;
        }
        if let Some(x) = segments_cross(pa, pb, shape.vertices[c], shape.vertices[d]) {
            shape.vertex(x);
        }
    }
    // The chain of vertices along the new segment, in order.
    let mut on: Vec<(f64, usize)> = vec![(0.0, a), (1.0, b)];
    for v in 0..shape.vertices.len() {
        if v != a && v != b && geom::on_open_segment(shape.vertices[v], pa, pb) {
            on.push((geom::param_on_line(shape.vertices[v], pa, pb), v));
        }
    }
    on.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    let mut new_edges = Vec::new();
    for w in on.windows(2) {
        let (x, y) = (w[0].1, w[1].1);
        // A piece covered by an existing edge is not new: it only splits that edge.
        let covered = before.iter().any(|&(c, d)| {
            let (pc, pd) = (shape.vertices[c], shape.vertices[d]);
            geom::on_closed_segment(shape.vertices[x], pc, pd) && geom::on_closed_segment(shape.vertices[y], pc, pd)
        });
        if shape.add_edge(x, y) && !covered {
            new_edges.push((x, y));
        }
    }
    shape.normalize();
    for (x, y) in new_edges {
        if shape.has_edge(x, y) {
            find_faces(shape, x, y);
        }
    }
    shape.orient_closed();
    Ok(())
}

/// `rect`: the four lines corner → +u → +u+v → +v → corner (u ⟂ v, both non-zero).
pub fn rect(shape: &mut Shape, corner: V3, u: V3, v: V3) -> Result<(), OpError> {
    if !is_finite(corner) || !is_finite(u) || !is_finite(v) {
        return Err("rect: coordinates must be finite".into());
    }
    let (lu, lv) = (geom::norm(u), geom::norm(v));
    if lu <= EPS || lv <= EPS {
        return Err("rect: `u` and `v` must not be zero".into());
    }
    if dot(u, v).abs() > ANG_EPS * lu * lv {
        return Err("rect: `u` must be perpendicular to `v`".into());
    }
    let p = [corner, add(corner, u), add(add(corner, u), v), add(corner, v)];
    for i in 0..4 {
        line(shape, p[i], p[(i + 1) % 4])?;
    }
    Ok(())
}

/// The frame of a circle or arc: unit normal and the unit direction of the first vertex.
fn circle_frame(normal: V3, first: V3) -> Result<(V3, V3), OpError> {
    let n = unit(normal).ok_or("`normal` must not be zero")?;
    let f = sub(first, scale(n, dot(first, n)));
    let f = unit(f).ok_or("the start direction is parallel to `normal`")?;
    Ok((n, f))
}

/// `circle`: a regular polygon of `segments` lines; the first vertex lies along x̂ projected onto
/// the plane (ŷ when x̂ is parallel to the normal).
pub fn circle(shape: &mut Shape, centre: V3, normal: V3, radius: f64, segments: usize) -> Result<(), OpError> {
    if !is_finite(centre) || !is_finite(normal) || !radius.is_finite() {
        return Err("circle: coordinates must be finite".into());
    }
    if !(radius > EPS) {
        return Err("circle: `radius` must be positive".into());
    }
    if segments < 3 {
        return Err("circle: `segments` must be at least 3".into());
    }
    let n = unit(normal).ok_or("circle: `normal` must not be zero")?;
    let x = [1.0, 0.0, 0.0];
    let first = if cross(x, n).iter().all(|c| c.abs() <= 1e-12) { [0.0, 1.0, 0.0] } else { x };
    let (n, f) = circle_frame(n, first).map_err(|e| format!("circle: {e}"))?;
    let t = cross(n, f);
    let pts: Vec<V3> = (0..segments)
        .map(|i| {
            let th = std::f64::consts::TAU * i as f64 / segments as f64;
            add(centre, add(scale(f, radius * th.cos()), scale(t, radius * th.sin())))
        })
        .collect();
    for i in 0..segments {
        line(shape, pts[i], pts[(i + 1) % segments])?;
    }
    Ok(())
}

/// `arc`: a polyline of `segments` lines from `start` through `angle` radians right-handed about
/// `normal`.
pub fn arc(shape: &mut Shape, centre: V3, start: V3, normal: V3, angle: f64, segments: usize) -> Result<(), OpError> {
    if !is_finite(centre) || !is_finite(start) || !is_finite(normal) || !angle.is_finite() {
        return Err("arc: coordinates must be finite".into());
    }
    if angle == 0.0 {
        return Err("arc: `angle` must not be zero".into());
    }
    if segments < 1 {
        return Err("arc: `segments` must be at least 1".into());
    }
    let r = sub(start, centre);
    let (n, f) = circle_frame(normal, r).map_err(|e| format!("arc: {e}"))?;
    let radius = dot(r, f);
    if !(radius > EPS) {
        return Err("arc: `start` must not be the centre".into());
    }
    let t = cross(n, f);
    let pts: Vec<V3> = (0..=segments)
        .map(|i| {
            let th = angle * i as f64 / segments as f64;
            add(centre, add(scale(f, radius * th.cos()), scale(t, radius * th.sin())))
        })
        .collect();
    for w in pts.windows(2) {
        line(shape, w[0], w[1])?;
    }
    Ok(())
}

// ------------------------------------------------------------------ face finding

/// The planes in which the new edge `a → b` could close a loop: those of the faces holding both
/// endpoints, and those spanned by the edge and every other edge at either end (following
/// collinear chains to their first turn).
fn candidate_planes(shape: &Shape, a: usize, b: usize) -> Vec<Plane> {
    let (pa, pb) = (shape.vertices[a], shape.vertices[b]);
    let dir = unit(sub(pb, pa)).expect("a and b are distinct");
    let mut planes: Vec<Plane> = Vec::new();
    let mut push = |p: Plane| {
        if !planes.iter().any(|q| q.same(&p)) {
            planes.push(p);
        }
    };
    for f in 0..shape.faces.len() {
        if let Some(plane) = shape.face_plane(f) {
            if plane.contains(pa) && plane.contains(pb) {
                push(plane);
            }
        }
    }
    // Edges at the ends, through collinear chains.
    let mut seen: BTreeSet<usize> = [a, b].into_iter().collect();
    let mut stack = vec![a, b];
    while let Some(x) = stack.pop() {
        for y in shape.neighbours(x) {
            if y == a || y == b {
                continue;
            }
            let d = sub(shape.vertices[y], pa);
            let perp = sub(d, scale(dir, dot(d, dir)));
            if geom::norm(perp) <= EPS {
                if seen.insert(y) {
                    stack.push(y);
                }
                continue;
            }
            if let Some(n) = unit(cross(dir, d)) {
                push(Plane::new(n, pa));
            }
        }
    }
    planes
}

/// Finds and creates the faces the new edge `{a, b}` closes, in every candidate plane
/// (spec/sketch.md, `line`): on each side of the edge, the smallest region bounded by it becomes a
/// face, splits the face it lies in (the part holding that face's first outer-loop vertex keeps
/// the id) or, as a loop inside a face touching nothing else, becomes a new face and a hole of it.
pub fn find_faces(shape: &mut Shape, a: usize, b: usize) {
    for plane in candidate_planes(shape, a, b) {
        let g = PlanarGraph::new(&plane, &shape.vertices, &shape.edges);
        let (Some(la), Some(lb)) = (g.local(a), g.local(b)) else { continue };
        let region_for = |s: usize, t: usize| -> Option<Region> {
            g.regions_of_walk(&g.walk(s, t)).into_iter().find(|r| r.has_edge(la, lb))
        };
        let sides = [region_for(la, lb), region_for(lb, la)];
        let hosts = {
            let coplanar: Vec<usize> = (0..shape.faces.len()).filter(|&f| shape.face_on_plane(f, &plane)).collect();
            let containing = |r: &Region| -> Option<usize> {
                let p = g.interior_point(r)?;
                coplanar.iter().copied().find(|&f| {
                    let face = &shape.faces[f];
                    let outer = g.directed(&face.loops[0]).map(|d| d.into_iter().map(|(x, _)| x).collect::<Vec<_>>());
                    let Some(outer) = outer else { return false };
                    let holes: Vec<Vec<usize>> = face.loops[1..].iter().filter_map(|h| g.directed(h).map(|d| d.into_iter().map(|(x, _)| x).collect())).collect();
                    g.region_contains(&Region { outer, holes }, p)
                })
            };
            [sides[0].as_ref().and_then(&containing), sides[1].as_ref().and_then(&containing)]
        };
        match (&sides[0], &sides[1], hosts[0], hosts[1]) {
            (Some(r1), Some(r2), Some(f1), Some(f2)) if f1 == f2 => split_face(shape, &g, f1, r1, r2),
            _ => {
                for k in 0..2 {
                    let Some(r) = &sides[k] else { continue };
                    match hosts[k] {
                        Some(f) => {
                            if !same_region(shape, &g, f, r) {
                                hole_in_face(shape, &g, f, r);
                            }
                        }
                        None => {
                            let loops = oriented_loops(shape, &g, r, None);
                            let id = shape.next_face_id();
                            shape.faces.push(Face { id, loops });
                        }
                    }
                }
            }
        }
    }
}

/// Whether region `r` is exactly the region of face `f` (same loop edges).
fn same_region(shape: &Shape, g: &PlanarGraph, f: usize, r: &Region) -> bool {
    let edges = |lp: &[usize]| -> BTreeSet<(usize, usize)> {
        let n = lp.len();
        (0..n).map(|i| (lp[i].min(lp[(i + 1) % n]), lp[i].max(lp[(i + 1) % n]))).collect()
    };
    let region: BTreeSet<(usize, usize)> = r.loops().flat_map(|lp| edges(&g.to_shape(lp))).collect();
    let face: BTreeSet<(usize, usize)> = shape.faces[f].loops.iter().flat_map(|lp| edges(lp)).collect();
    region == face
}

/// Face `f` is split into regions `r1` and `r2`: the part holding its first outer-loop vertex (and,
/// on a tie, its first edge) keeps the id; the other becomes a new face.
fn split_face(shape: &mut Shape, g: &PlanarGraph, f: usize, r1: &Region, r2: &Region) {
    let normal = shape.face_normal(f);
    let first = shape.faces[f].loops[0][0];
    let second = shape.faces[f].loops[0][1];
    let score = |r: &Region| -> (bool, bool) {
        let outer = g.to_shape(&r.outer);
        let has_first = outer.contains(&first);
        let has_edge = has_first && crate::planar::loop_has_edge(&outer, first, second);
        (has_first, has_edge)
    };
    let (s1, s2) = (score(r1), score(r2));
    let (keep, other) = if s1 >= s2 { (r1, r2) } else { (r2, r1) };
    let mut loops = oriented_loops(shape, g, keep, normal);
    if let Some(k) = loops[0].iter().position(|&v| v == first) {
        loops[0].rotate_left(k);
    }
    shape.faces[f].loops = loops;
    let loops = oriented_loops(shape, g, other, normal);
    let id = shape.next_face_id();
    shape.faces.push(Face { id, loops });
}

/// Region `r` is a loop drawn inside face `f` touching nothing else: it becomes a new face (with
/// the islands inside it as holes) and its outer loop a hole of `f`.
fn hole_in_face(shape: &mut Shape, g: &PlanarGraph, f: usize, r: &Region) {
    let normal = shape.face_normal(f);
    let outer = g.to_shape(&r.outer);
    // Holes of f now inside the new loop belong to the new face.
    let keep: Vec<Vec<usize>> = shape.faces[f].loops[1..]
        .iter()
        .filter(|h| !h.iter().any(|&v| g.local(v).map_or(false, |l| !r.outer.contains(&l) && g.inside(&r.outer, g.pos[l]))))
        .cloned()
        .collect();
    let mut hole = outer.clone();
    let face_positive = g.directed(&shape.faces[f].loops[0]).map_or(true, |d| g.area(&d.into_iter().map(|(x, _)| x).collect::<Vec<_>>()) > 0.0);
    if face_positive {
        hole.reverse();
    }
    rotate_to_min(&mut hole);
    let mut loops = vec![shape.faces[f].loops[0].clone()];
    loops.extend(keep);
    loops.push(hole);
    shape.faces[f].loops = loops;
    let loops = oriented_loops(shape, g, r, normal);
    let id = shape.next_face_id();
    shape.faces.push(Face { id, loops });
}

fn rotate_to_min(lp: &mut Vec<usize>) {
    if let Some(k) = lp.iter().enumerate().min_by_key(|(_, &v)| v).map(|(k, _)| k) {
        lp.rotate_left(k);
    }
}

/// Reverses the winding of every loop, keeping each loop's first vertex in front (a face that
/// keeps its id keeps its first outer-loop vertex).
fn reverse_keeping_first(loops: &mut [Vec<usize>]) {
    for lp in loops {
        if lp.len() > 1 {
            lp[1..].reverse();
        }
    }
}

/// The loops of a new face for region `r` (local indices, outer counter-clockwise in the plane
/// frame) as shape vertex indices, oriented to agree with the faces it shares edges with (an edge
/// shared with an existing face is traversed the other way), else with `hint` (the normal of the
/// face it came from), else with the plane normal's positive direction. Loops start at their
/// lowest vertex index.
fn oriented_loops(shape: &Shape, g: &PlanarGraph, r: &Region, hint: Option<V3>) -> Vec<Vec<usize>> {
    let mut loops: Vec<Vec<usize>> = r.loops().map(|lp| g.to_shape(lp)).collect();
    let (mut keep, mut flip) = (0, 0);
    for lp in &loops {
        let n = lp.len();
        for i in 0..n {
            let (x, y) = (lp[i], lp[(i + 1) % n]);
            for f in shape.faces_with_edge(x, y) {
                match shape.faces[f].direction(x, y) {
                    Some(true) => flip += 1,
                    Some(false) => keep += 1,
                    None => {}
                }
            }
        }
    }
    // The outer loop is counter-clockwise in the plane frame, so its normal is the plane normal.
    let n = unit(geom::newell(&shape.loop_points(&loops[0]))).unwrap_or([0.0, 0.0, 1.0]);
    let reverse = if keep != flip {
        flip > keep
    } else if let Some(h) = hint {
        dot(h, n) < 0.0
    } else {
        !positive(n)
    };
    for lp in &mut loops {
        if reverse {
            lp.reverse();
        }
        rotate_to_min(lp);
    }
    loops
}

// ------------------------------------------------------------------ push/pull

/// `pushpull`: extrudes face `id` by `distance` along its normal (spec/sketch.md).
pub fn pushpull(shape: &mut Shape, id: u64, distance: f64, new: bool) -> Result<(), OpError> {
    // A distance within the point-equality tolerance would move every vertex onto itself.
    if !distance.is_finite() || distance.abs() <= EPS {
        return Err("pushpull: `distance` must be a non-zero finite number".into());
    }
    let f = shape.face_by_id(id).ok_or_else(|| format!("pushpull: the shape has no face {id}"))?;
    let n = shape.face_normal(f).ok_or_else(|| format!("pushpull: face {id} is degenerate"))?;
    let t = scale(n, distance);
    let face = shape.faces[f].clone();
    let verts = face.vertices();
    let dir_edges = face.directed_edges();
    // The loose edges before the push, as index pairs: their ends never move (a vertex with one
    // is copied, not moved in place) and indices never shift, so afterwards each is found again
    // as it was or as the pieces `normalize` split it into.
    let loose_before: Vec<(usize, usize)> = {
        let on = shape.loop_edges();
        shape.edges.iter().copied().filter(|e| !on.contains(e)).collect()
    };
    let perpendicular = |shape: &Shape, g: usize| shape.face_normal(g).map_or(false, |m| dot(m, n).abs() <= ANG_EPS);
    // The face each boundary edge is stretched or shrunk with: the one other face holding the edge
    // when it is perpendicular to this face. An edge on no other face, on a face that is not
    // perpendicular, or on several other faces (not a manifold edge, so no one face can follow it)
    // gets a new side face instead. With `new` the original face stays, so nothing is stretched.
    let stretch_with: Vec<Option<usize>> = dir_edges
        .iter()
        .map(|&(x, y)| {
            let others: Vec<usize> = shape.faces_with_edge(x, y).into_iter().filter(|&g| g != f).collect();
            match others.as_slice() {
                &[g] if !new && perpendicular(shape, g) => Some(g),
                _ => None,
            }
        })
        .collect();
    // Which vertices move in place: those used only by this face and by perpendicular faces that
    // hold one of this face's edges at the vertex, with no loose edge, both of whose face edges at
    // the vertex are stretched (so the stretched neighbours cover every side).
    let mut in_place: Vec<bool> = vec![false; verts.len()];
    if !new {
        for (k, &v) in verts.iter().enumerate() {
            let at_v: Vec<usize> = (0..dir_edges.len()).filter(|&e| dir_edges[e].0 == v || dir_edges[e].1 == v).collect();
            let others = shape.faces_with_vertex(v).into_iter().filter(|&g| g != f).collect::<Vec<_>>();
            let ok_faces = others.iter().all(|&g| perpendicular(shape, g) && at_v.iter().any(|&e| shape.faces[g].has_edge(dir_edges[e].0, dir_edges[e].1)));
            let covered = at_v.iter().all(|&e| stretch_with[e].is_some());
            in_place[k] = ok_faces && covered && shape.loose_edges_at(v).is_empty();
        }
    }
    // Where each face vertex goes. spec/sketch.md keeps vertex indices and appends new vertices,
    // and two points within EPS are one vertex, so a target already held by a vertex the push
    // leaves where it is (every vertex but the in-place movers) reuses that vertex instead of
    // adding a duplicate that would then have to merge. An in-place mover whose target is taken
    // stays at its old position; once its neighbours are stretched past it, it is on no face or
    // edge: an orphan, kept so that no index shifts. This is the residual tension between index
    // stability and one-vertex-per-point: a collapse that would merge two vertices leaves the
    // loser behind as a stale point instead of removing it. (The reused vertex need not have the
    // lower index: a push can land on a vertex an earlier operation stranded.)
    let n = shape.vertices.len();
    let stays: Vec<bool> = (0..n).map(|i| !verts.iter().zip(&in_place).any(|(&v, &ip)| ip && v == i)).collect();
    let mut moved: Vec<usize> = Vec::with_capacity(verts.len());
    for (k, &v) in verts.iter().enumerate() {
        let p = add(shape.vertices[v], t);
        let taken = (0..n).find(|&i| stays[i] && dist(shape.vertices[i], p) <= EPS);
        moved.push(match taken {
            Some(w) => w,
            None if in_place[k] => {
                shape.vertices[v] = p;
                v
            }
            None => {
                shape.vertices.push(p);
                shape.vertices.len() - 1
            }
        });
    }
    let new_index = |v: usize| moved[verts.iter().position(|&x| x == v).expect("a face vertex")];
    let map_loops = |loops: &[Vec<usize>]| -> Vec<Vec<usize>> { loops.iter().map(|lp| lp.iter().map(|&v| new_index(v)).collect()).collect() };
    // A naked face (every edge on no other face) closes a prism between itself and its moved copy:
    // the prism is built outward by construction. The moved copy faces along the push, the base
    // against it, and the sides follow: pulled (positive) along the normal, the copy keeps the
    // face's winding and the base is reversed; pushed (negative), the base keeps it and the copy
    // is reversed. A face on other faces belongs to a solid whose outward side its normal gives:
    // the copy keeps the winding and the sides are built for that solid, whichever way it moves.
    let naked = dir_edges.iter().all(|&(x, y)| shape.faces_with_edge(x, y).iter().all(|&g| g == f));
    let pulled = distance > 0.0;
    let reverse_copy = naked && !pulled;
    let reverse_base = naked && pulled;
    let mut new_faces: Vec<Face> = Vec::new();
    let mut next_id = shape.next_face_id();
    // The moved copy; it keeps the face's first outer-loop vertex (moved) in front.
    let mut copy = map_loops(&face.loops);
    if reverse_copy {
        reverse_keeping_first(&mut copy);
    }
    // With `new` the original face stays (its id, its points and its first outer-loop vertex) and
    // the moved copy is the first new face, then the sides; without, the face moves and keeps its
    // id, the sides are new, and a naked face's base is the last new face.
    let moved_id;
    if new {
        moved_id = next_id;
        new_faces.push(Face { id: next_id, loops: copy });
        next_id += 1;
        if reverse_base {
            reverse_keeping_first(&mut shape.faces[f].loops);
        }
    } else {
        moved_id = id;
        shape.faces[f].loops = copy;
    }
    for (e, &(x, y)) in dir_edges.iter().enumerate() {
        let (nx, ny) = (new_index(x), new_index(y));
        match stretch_with[e] {
            Some(g) => splice(&mut shape.faces[g], x, y, nx, ny),
            None => {
                let mut lp = if reverse_copy { vec![y, x, nx, ny] } else { vec![x, y, ny, nx] };
                crate::shape::clean_loop(&mut lp);
                if lp.len() >= 3 {
                    new_faces.push(Face { id: next_id, loops: vec![lp] });
                    next_id += 1;
                }
            }
        }
    }
    if naked && !new {
        let mut loops = face.loops.clone();
        if reverse_base {
            for lp in &mut loops {
                lp.reverse();
            }
        }
        for lp in &mut loops {
            rotate_to_min(lp);
        }
        new_faces.push(Face { id: next_id, loops });
    }
    shape.faces.extend(new_faces);
    // An edge a stretched face no longer traverses (its ends were copied) is on no face now.
    for &(x, y) in &dir_edges {
        if shape.faces_with_edge(x, y).is_empty() {
            shape.remove_edge(x, y);
        }
    }
    shape.normalize();
    // The moved face (with `new`, the moved copy: the face that lands) pushed onto a parallel face
    // of the same solid punches through it; the original face of a `new` push stays either way.
    punch_through(shape, moved_id);
    shape.normalize();
    // A safety net: every solid the push closes is turned outward (a no-op for one already
    // consistently oriented and outward, which the construction above gives).
    shape.orient_closed();
    // Edges left on no face by a face that collapsed or was punched away go with it; the loose
    // edges the shape had before stay (as they were, or as the pieces a vertex landing on one
    // split them into). Vertices left on no edge stay at their index.
    let on = shape.loop_edges();
    let vertices = shape.vertices.clone();
    let was_loose = |a: usize, b: usize| {
        loose_before.iter().any(|&(p, q)| {
            (a, b) == (p, q) || (geom::on_closed_segment(vertices[a], vertices[p], vertices[q]) && geom::on_closed_segment(vertices[b], vertices[p], vertices[q]))
        })
    };
    shape.edges.retain(|&(a, b)| on.contains(&(a, b)) || was_loose(a, b));
    Ok(())
}

/// Replaces the edge `x → y` (either direction) of a stretched face by the path through the moved
/// copies `nx`, `ny` of its ends.
pub(crate) fn splice(face: &mut Face, x: usize, y: usize, nx: usize, ny: usize) {
    for lp in &mut face.loops {
        let n = lp.len();
        for i in 0..n {
            let (p, q) = (lp[i], lp[(i + 1) % n]);
            let path: Option<Vec<usize>> = if (p, q) == (x, y) {
                Some(vec![x, nx, ny, y])
            } else if (p, q) == (y, x) {
                Some(vec![y, ny, nx, x])
            } else {
                None
            };
            if let Some(mut path) = path {
                crate::shape::clean_loop(&mut path);
                // Insert the interior of the path after position i.
                let mut out = lp[..=i].to_vec();
                out.extend(path.iter().copied().filter(|&v| v != p && v != q));
                out.extend(lp[i + 1..].iter().copied());
                *lp = out;
                return;
            }
        }
    }
}

/// After a push, the moved face `id` may lie on a parallel face of the same solid facing it:
/// the overlap is removed from both. A moved face inside the other leaves it a hole; a moved face
/// equal to the other (or to one of its holes) removes it; a moved face sharing part of its
/// boundary shrinks the other. The other face keeps its id; when it splits, the part holding its
/// first outer-loop vertex keeps it. The moved face's holes, left uncovered, become new faces.
pub(crate) fn punch_through(shape: &mut Shape, id: u64) {
    let Some(f) = shape.face_by_id(id) else { return };
    let Some(plane) = shape.face_plane(f) else { return };
    let n = plane.n;
    let opposite: Vec<usize> = (0..shape.faces.len())
        .filter(|&h| h != f && shape.face_on_plane(h, &plane) && shape.face_normal(h).map_or(false, |m| dot(m, n) <= -(1.0 - ANG_EPS)))
        .collect();
    for h in opposite {
        let Some(f) = shape.face_by_id(id) else { return };
        let hplane = Plane::new(scale(n, -1.0), plane.o);
        let g = PlanarGraph::new(&hplane, &shape.vertices, &shape.edges);
        let (Some(df), Some(dh)) = (
            shape.faces[f].loops.iter().map(|lp| g.directed(lp)).collect::<Option<Vec<_>>>(),
            shape.faces[h].loops.iter().map(|lp| g.directed(lp)).collect::<Option<Vec<_>>>(),
        ) else {
            continue;
        };
        let to_region = |d: &Vec<Vec<(usize, usize)>>| Region {
            outer: d[0].iter().map(|&(x, _)| x).collect(),
            holes: d[1..].iter().map(|lp| lp.iter().map(|&(x, _)| x).collect()).collect(),
        };
        let (rf, rh) = (to_region(&df), to_region(&dh));
        // Proper crossings between the two boundaries: a partial overlap this does not handle.
        let crosses = df.iter().flatten().any(|&(a, b)| {
            dh.iter().flatten().any(|&(c, d)| segments_cross(shape.vertices[g.verts[a]], shape.vertices[g.verts[b]], shape.vertices[g.verts[c]], shape.vertices[g.verts[d]]).is_some())
        });
        if crosses {
            continue;
        }
        let f_in_h = g.interior_point(&rf).map_or(false, |p| g.region_contains(&rh, p));
        let h_in_f = g.interior_point(&rh).map_or(false, |p| g.region_contains(&rf, p));
        let f_is_hole_of_h = rh.holes.iter().any(|hole| same_cycle(hole, &rf.outer)) && rf.holes.is_empty();
        if f_is_hole_of_h {
            shape.faces.remove(f);
            return;
        }
        if !f_in_h && !h_in_f {
            continue;
        }
        // The regions remaining of h (if f is inside h) and of f (if h is inside f).
        let mut dir: Vec<(usize, usize)> = df.iter().flatten().copied().collect();
        dir.extend(dh.iter().flatten().copied());
        let regions = g.assemble(g.link(&dir));
        let (big, small) = if f_in_h { (h, f) } else { (f, h) };
        let big_id = shape.faces[big].id;
        let first = shape.faces[big].loops[0][0];
        let small_holes: Vec<Vec<usize>> = shape.faces[small].loops[1..].to_vec();
        let big_normal = shape.face_normal(big);
        // Rebuild: the big face's parts (one keeps the id), the small face's holes as new faces.
        let mut parts: Vec<Vec<Vec<usize>>> = regions
            .iter()
            .map(|r| {
                let mut loops: Vec<Vec<usize>> = r.loops().map(|lp| g.to_shape(lp)).collect();
                // Regions came out counter-clockwise in h's frame; orient to the big face's normal.
                if let Some(m) = big_normal {
                    if dot(m, hplane.n) < 0.0 {
                        for lp in &mut loops {
                            lp.reverse();
                        }
                    }
                }
                loops
            })
            .collect();
        let mut remove: Vec<usize> = vec![f, h];
        remove.sort_unstable();
        for &k in remove.iter().rev() {
            shape.faces.remove(k);
        }
        // The part holding the big face's first outer-loop vertex keeps its id and its place in
        // the face list; the other parts and the small face's uncovered holes are new faces.
        let keeper = parts.iter().position(|loops| loops[0].contains(&first)).or(if parts.is_empty() { None } else { Some(0) });
        let mut next_id = shape.next_face_id();
        let slot = (big - usize::from(small < big)).min(shape.faces.len());
        for (k, loops) in parts.iter_mut().enumerate() {
            if Some(k) == keeper {
                if let Some(pos) = loops[0].iter().position(|&v| v == first) {
                    loops[0].rotate_left(pos);
                }
                shape.faces.insert(slot, Face { id: big_id, loops: loops.clone() });
            } else {
                for lp in loops.iter_mut() {
                    rotate_to_min(lp);
                }
                shape.faces.push(Face { id: next_id, loops: loops.clone() });
                next_id += 1;
            }
        }
        let covered: Vec<Vec<usize>> = parts.iter().map(|loops| loops[0].clone()).collect();
        for hole in small_holes {
            if covered.iter().any(|c| same_cycle(c, &hole)) {
                continue;
            }
            let mut lp = hole.clone();
            lp.reverse();
            rotate_to_min(&mut lp);
            shape.faces.push(Face { id: next_id, loops: vec![lp] });
            next_id += 1;
        }
        return;
    }
}

/// Whether two loops are the same cycle of vertices (either direction, any start).
pub(crate) fn same_cycle(a: &[usize], b: &[usize]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let n = a.len();
    let Some(k) = b.iter().position(|&v| v == a[0]) else { return false };
    (0..n).all(|i| a[i] == b[(k + i) % n]) || (0..n).all(|i| a[i] == b[(k + n - i) % n])
}

// ------------------------------------------------------------------ erase

/// `erase`: removes the edges and the faces they bound; an edge between two coplanar faces merges
/// them into the one with the lower id.
pub fn erase(shape: &mut Shape, edges: &[(usize, usize)]) -> Result<(), OpError> {
    for &(a, b) in edges {
        if a >= shape.vertices.len() || b >= shape.vertices.len() || !shape.has_edge(a, b) {
            return Err(format!("erase: the shape has no edge [{a}, {b}]"));
        }
    }
    for &(a, b) in edges {
        if !shape.has_edge(a, b) {
            continue; // already erased by an earlier pair
        }
        let on = shape.faces_with_edge(a, b);
        let merged = match on.as_slice() {
            &[f, g] => {
                let coplanar = shape.face_plane(f).map_or(false, |p| shape.face_on_plane(g, &p));
                coplanar && merge_faces(shape, f, g)
            }
            _ => false,
        };
        if !merged {
            let mut on = on;
            on.sort_unstable();
            for &k in on.iter().rev() {
                shape.faces.remove(k);
            }
        }
        shape.remove_edge(a, b);
    }
    shape.normalize();
    Ok(())
}

/// Merges coplanar faces `f` and `g` into the one with the lower id (its orientation and first
/// outer-loop vertex kept). Edges they share stay as edges on no face. False if the union is not
/// one region.
fn merge_faces(shape: &mut Shape, f: usize, g: usize) -> bool {
    let (keep, other) = if shape.faces[f].id <= shape.faces[g].id { (f, g) } else { (g, f) };
    let Some(plane) = shape.face_plane(keep) else { return false };
    let graph = PlanarGraph::new(&plane, &shape.vertices, &shape.edges);
    let mut dir: Vec<(usize, usize)> = Vec::new();
    let Some(d) = shape.faces[keep].loops.iter().map(|lp| graph.directed(lp)).collect::<Option<Vec<_>>>() else { return false };
    dir.extend(d.into_iter().flatten());
    let flip = shape.face_normal(other).map_or(false, |m| dot(m, plane.n) < 0.0);
    let Some(d) = shape.faces[other].loops.iter().map(|lp| graph.directed(lp)).collect::<Option<Vec<_>>>() else { return false };
    for e in d.into_iter().flatten() {
        dir.push(if flip { (e.1, e.0) } else { e });
    }
    let regions = graph.assemble(graph.link(&dir));
    if regions.len() != 1 {
        return false;
    }
    let first = shape.faces[keep].loops[0][0];
    let mut loops: Vec<Vec<usize>> = regions[0].loops().map(|lp| graph.to_shape(lp)).collect();
    if let Some(k) = loops[0].iter().position(|&v| v == first) {
        loops[0].rotate_left(k);
    } else {
        rotate_to_min(&mut loops[0]);
    }
    for lp in loops.iter_mut().skip(1) {
        rotate_to_min(lp);
    }
    shape.faces[keep].loops = loops;
    shape.faces.remove(other);
    true
}
