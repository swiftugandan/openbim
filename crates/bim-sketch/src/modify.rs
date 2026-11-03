//! The modifying operations of spec/sketch.md on a [`Shape`]: `offset`, `followme`,
//! `move` / `rotate` / `scale` (stretching connected faces, the triangle-fan rule for faces made
//! non-planar, `copy` and `array`) and `paint`.

use std::collections::BTreeSet;

use bim_doc::EntityId;
use bim_geom::{orient2d, Point2, Polygon};

use crate::command::{Selection, Xf};
use crate::geom::{self, add, cross, dist, dot, is_finite, scale, segments_cross, sub, unit, Plane, ANG_EPS, EPS, V3};
use crate::ops::{find_faces, punch_through, same_cycle, splice, OpError};
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

/// Whether `p` lies within [`EPS`] of a segment of the ring.
fn on_ring(r: &[Point2], p: Point2) -> bool {
    let n = r.len();
    (0..n).any(|i| dist_to_segment(p, r[i], r[(i + 1) % n]) <= EPS)
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
/// edge shrinks away), cross no edge lying in the plane and, inward, stay inside the face and off
/// its holes.
///
/// Outward, the ring takes the annulus between the face's outer loop and the new loop, so a
/// coplanar face there must give it up: a face holding the outer loop as a hole (the face is an
/// island in it) gets the new loop as that hole instead, provided the new loop stays strictly
/// inside its outer loop and clear of its other holes. Any other coplanar face reaching the
/// annulus (one sharing part of the outer loop, one the new loop crosses or encloses) would
/// overlap the ring, and a face of another plane on an outer-loop edge would put that edge on
/// three faces; both are invalid. The shape is untouched when the command is invalid.
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
    // Outward: the coplanar faces that must give up the annulus. `hosts` are the faces holding
    // the outer loop as a hole, with the index of that hole.
    let mut hosts: Vec<(usize, usize)> = Vec::new();
    if !inward {
        let project = |lp: &[usize]| -> Vec<Point2> { lp.iter().map(|&v| plane.project(shape.vertices[v])).collect() };
        for g in 0..shape.faces.len() {
            if g == f || !shape.face_on_plane(g, &plane) {
                continue;
            }
            let face = &shape.faces[g];
            let gid = face.id;
            let outer_g = project(&face.loops[0]);
            match (1..face.loops.len()).find(|&k| same_cycle(&face.loops[k], &outer)) {
                Some(k) => {
                    if rings_meet(&outer_g, &q2) || !inside_ring(&outer_g, q2[0]) {
                        return Err(format!("offset: the offset loop of face {id} would reach the boundary of face {gid}, which surrounds it"));
                    }
                    for (j, h) in face.loops.iter().enumerate().skip(1) {
                        if j == k {
                            continue;
                        }
                        let h2 = project(h);
                        if rings_meet(&h2, &q2) || inside_ring(&q2, h2[0]) {
                            return Err(format!("offset: the offset loop of face {id} would reach another hole of face {gid}, which surrounds it"));
                        }
                    }
                    hosts.push((g, k));
                }
                None => {
                    let overlap = || format!("offset: the ring around face {id} would overlap face {gid}");
                    for lp in &face.loops {
                        let l2 = project(lp);
                        if rings_meet(&l2, &q2) || l2.iter().any(|&p| inside_ring(&q2, p) && !inside_ring(&p2, p) && !on_ring(&p2, p)) {
                            return Err(overlap());
                        }
                    }
                    if inside_ring(&outer_g, q2[0]) && !face.loops[1..].iter().any(|h| inside_ring(&project(h), q2[0])) {
                        return Err(overlap());
                    }
                }
            }
        }
        // The outer loop's edges are then on the face, the ring and (where the face is an island)
        // the host: a face of another plane holding one would make it an edge of three faces.
        for i in 0..m {
            let (x, y) = (outer[i], outer[(i + 1) % m]);
            if let Some(h) = shape.faces_with_edge(x, y).into_iter().find(|&h| h != f && !hosts.iter().any(|&(g, _)| g == h)) {
                return Err(format!("offset: the ring around face {id} would put edge [{x}, {y}] on three faces (it is on face {} too)", shape.faces[h].id));
            }
        }
    }
    // No edge lying in the plane may cross the new loop (a vertex on it only splits a loop edge,
    // which `normalize` does).
    for &(a, b) in &shape.edges {
        let (pa, pb) = (shape.vertices[a], shape.vertices[b]);
        if plane.contains(pa) && plane.contains(pb) && (0..m).any(|i| segments_cross(pa, pb, q[i], q[(i + 1) % m]).is_some()) {
            return Err(format!("offset: the offset loop of face {id} would cross edge [{a}, {b}]"));
        }
    }
    // The new vertices: a point on an existing vertex reuses it. Two points on one vertex, or one
    // on the outer loop, were ruled out above (the loop is simple and clear of the outer loop);
    // this is the last check before the shape changes.
    let found: Vec<Option<usize>> = q.iter().map(|&x| shape.find_vertex(x)).collect();
    if found.iter().flatten().collect::<BTreeSet<_>>().len() != found.iter().flatten().count()
        || found.iter().flatten().any(|v| outer.contains(v))
        || (0..m).any(|i| (0..i).any(|j| dist(q[i], q[j]) <= EPS))
    {
        return Err(too_far());
    }
    let ql: Vec<usize> = q.iter().map(|&x| shape.vertex(x)).collect();
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
        // A host's hole becomes the new loop, wound against the host's outer loop.
        let q_sign = signed_area2(&q2);
        for &(g, k) in &hosts {
            let outer_g: Vec<Point2> = shape.faces[g].loops[0].iter().map(|&v| plane.project(shape.vertices[v])).collect();
            let mut lp = ql.clone();
            if signed_area2(&outer_g) * q_sign > 0.0 {
                lp.reverse();
            }
            rotate_to_min(&mut lp);
            shape.faces[g].loops[k] = lp;
        }
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

// ------------------------------------------------------------------ structure checks

/// The faces connected to face `f` through shared edges (including `f`), in face order.
fn component_of(shape: &Shape, f: usize) -> Vec<usize> {
    let mut seen = vec![false; shape.faces.len()];
    seen[f] = true;
    let mut stack = vec![f];
    while let Some(g) = stack.pop() {
        for (a, b) in shape.faces[g].directed_edges() {
            for h in shape.faces_with_edge(a, b) {
                if !seen[h] {
                    seen[h] = true;
                    stack.push(h);
                }
            }
        }
    }
    (0..shape.faces.len()).filter(|&g| seen[g]).collect()
}

/// Whether the faces connected to face `f` form a closed solid (spec/sketch.md: every edge on
/// exactly two of them, used in opposite directions).
fn component_closed(shape: &Shape, f: usize) -> bool {
    let part = Shape {
        vertices: shape.vertices.clone(),
        faces: component_of(shape, f).into_iter().map(|g| shape.faces[g].clone()).collect(),
        edges: Vec::new(),
        products: None,
    };
    part.is_closed()
}

/// The edges on more than two faces.
fn crowded_edges(shape: &Shape) -> BTreeSet<(usize, usize)> {
    shape.loop_edges().into_iter().filter(|&(a, b)| shape.faces_with_edge(a, b).len() > 2).collect()
}

/// Whether the coplanar faces `a` and `b` (both on `plane`) overlap: a boundary edge of one
/// crosses one of the other, or a point inside one lies inside the other. Faces that only share
/// edges or touch at vertices do not overlap, nor does a face lying in a hole of the other.
fn faces_overlap(shape: &Shape, plane: &Plane, a: usize, b: usize) -> bool {
    let loops = |f: usize| -> Vec<Vec<Point2>> { shape.faces[f].loops.iter().map(|lp| lp.iter().map(|&v| plane.project(shape.vertices[v])).collect()).collect() };
    let (la, lb) = (loops(a), loops(b));
    let ring_edges = |rings: &[Vec<Point2>]| -> Vec<(Point2, Point2)> {
        rings.iter().flat_map(|r| (0..r.len()).map(move |i| (r[i], r[(i + 1) % r.len()]))).collect()
    };
    let (ea, eb) = (ring_edges(&la), ring_edges(&lb));
    let lift = |p: Point2| -> V3 { [p.x, p.y, 0.0] };
    if ea.iter().any(|&(p, q)| eb.iter().any(|&(r, s)| segments_cross(lift(p), lift(q), lift(r), lift(s)).is_some())) {
        return true;
    }
    // Points inside one face (triangle centroids of its triangulation) that are off the other's
    // boundary and inside its region.
    let inside_other = |mine: &[Vec<Point2>], other: &[Vec<Point2>], other_edges: &[(Point2, Point2)]| -> bool {
        let poly = Polygon::with_holes(mine[0].clone(), mine[1..].to_vec());
        let pts = poly.vertices();
        let region = Polygon::with_holes(other[0].clone(), other[1..].to_vec());
        poly.triangulate().into_iter().any(|t| {
            let [p, q, r] = t.map(|i| pts[i]);
            if orient2d(p, q, r).abs() <= 1e-18 {
                return false;
            }
            let c = Point2::new((p.x + q.x + r.x) / 3.0, (p.y + q.y + r.y) / 3.0);
            !other_edges.iter().any(|&(s, e)| dist_to_segment(c, s, e) <= EPS) && region.contains(c)
        })
    };
    inside_other(&la, &lb, &eb) || inside_other(&lb, &la, &ea)
}

/// The structural check an operation's result must pass for the faces it made or changed
/// (`touched`): none of their edges is on more than two faces (unless it already was, in
/// `crowded_before`), and none of them overlaps a coplanar face. `op` names the operation in the
/// error.
fn check_structure(shape: &Shape, touched: &[usize], crowded_before: &BTreeSet<(usize, usize)>, op: &str) -> Result<(), OpError> {
    for &t in touched {
        let tid = shape.faces[t].id;
        for (a, b) in shape.faces[t].directed_edges() {
            let on = shape.faces_with_edge(a, b);
            if on.len() > 2 && !crowded_before.contains(&(a.min(b), a.max(b))) {
                return Err(format!("{op}: edge [{a}, {b}] of face {tid} would be on {} faces", on.len()));
            }
        }
        let Some(plane) = shape.face_plane(t) else { continue };
        for h in 0..shape.faces.len() {
            if h == t || (touched.contains(&h) && h < t) || !shape.face_on_plane(h, &plane) {
                continue;
            }
            if faces_overlap(shape, &plane, t, h) {
                return Err(format!("{op}: face {tid} would overlap the coplanar face {}", shape.faces[h].id));
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ follow-me

/// `followme`: sweeps face `id` along the polyline `path`, whose first segment is perpendicular to
/// the face and starts on its plane. Each section is the previous one carried along the segment
/// onto the plane bisecting the joint (the plane perpendicular to the last segment at the end), so
/// every joint is mitred. One side face per profile edge per segment (new ids in that order); the
/// end cap is the face itself (it keeps its id). A face on no other face also gets its start cap
/// (the last new face), closing a solid; a face of a solid is replaced by the sweep, whose sides
/// join the solid along the face's edges.
///
/// Where the first segment runs into a perpendicular face holding a profile edge (the sweep goes
/// into the solid, as `pushpull` pushes in), that face is shrunk along the segment instead of
/// getting a side face over it, exactly as `pushpull` does: it keeps its id, and a profile vertex
/// whose faces are all shrunk this way moves in place. A sweep leaving the solid gets new side
/// faces beside its neighbours (adjacent coplanar faces never merge). The sweep landing on a
/// parallel face of the same solid punches through it like a push.
///
/// The result must be sound: a face of a closed solid leaves a closed solid, no edge of the sweep
/// is on more than two faces, and no face of the sweep (or shrunk beside it) overlaps a coplanar
/// face. Otherwise the command is invalid and the shape untouched.
pub fn followme(shape: &mut Shape, id: u64, path: &[V3]) -> Result<(), OpError> {
    let mut work = shape.clone();
    sweep(&mut work, id, path)?;
    *shape = work;
    Ok(())
}

fn sweep(shape: &mut Shape, id: u64, path: &[V3]) -> Result<(), OpError> {
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
    let dir_edges = face.directed_edges();
    let naked = dir_edges.iter().all(|&(x, y)| shape.faces_with_edge(x, y).iter().all(|&g| g == f));
    let closed_before = component_closed(shape, f);
    let crowded_before = crowded_edges(shape);
    let loose_before: Vec<(usize, usize)> = {
        let on = shape.loop_edges();
        shape.edges.iter().copied().filter(|e| !on.contains(e)).collect()
    };
    // The face each profile edge shrinks along the first segment: the one other face holding the
    // edge when the segment lies in its plane and runs into it (its interior is on the side of the
    // edge the sweep goes to). Every other edge gets side faces.
    let d0 = d[0];
    let stretch_with: Vec<Option<usize>> = dir_edges
        .iter()
        .map(|&(x, y)| {
            let others: Vec<usize> = shape.faces_with_edge(x, y).into_iter().filter(|&g| g != f).collect();
            let &[g] = others.as_slice() else { return None };
            let m = shape.face_normal(g)?;
            if dot(m, d0).abs() > ANG_EPS {
                return None;
            }
            // g's interior is to the left of its own traversal of the edge.
            let (p, q) = match shape.faces[g].direction(x, y) {
                Some(true) => (x, y),
                Some(false) => (y, x),
                None => return None,
            };
            let along = sub(shape.vertices[q], shape.vertices[p]);
            (dot(cross(m, along), d0) > 0.0).then_some(g)
        })
        .collect();
    // A profile vertex moves in place (as in a push) when both its profile edges shrink a face,
    // every other face at it is one of those, and no loose edge hangs from it.
    let in_place: Vec<bool> = verts
        .iter()
        .map(|&v| {
            let at_v: Vec<usize> = (0..dir_edges.len()).filter(|&e| dir_edges[e].0 == v || dir_edges[e].1 == v).collect();
            let covered = !at_v.is_empty() && at_v.iter().all(|&e| stretch_with[e].is_some());
            let others_ok = shape.faces_with_vertex(v).into_iter().filter(|&g| g != f).all(|g| at_v.iter().any(|&e| stretch_with[e] == Some(g)));
            covered && others_ok && shape.loose_edges_at(v).is_empty()
        })
        .collect();
    let movers: Vec<usize> = verts.iter().zip(&in_place).filter(|(_, &ip)| ip).map(|(&v, _)| v).collect();
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
        let mut idx: Vec<usize> = Vec::with_capacity(next.len());
        for (i, &p) in next.iter().enumerate() {
            // A point on a vertex that stays reuses it (an in-place mover whose target is taken
            // stays behind at its index, as in a push); otherwise the mover moves or the point is
            // appended.
            let taken = (0..shape.vertices.len()).find(|&w| !movers.contains(&w) && dist(shape.vertices[w], p) <= EPS);
            idx.push(match taken {
                Some(w) => w,
                None if k == 0 && in_place[i] => {
                    shape.vertices[verts[i]] = p;
                    verts[i]
                }
                None => {
                    shape.vertices.push(p);
                    shape.vertices.len() - 1
                }
            });
        }
        if idx.iter().collect::<BTreeSet<_>>().len() != idx.len() {
            return Err(format!("followme: the sweep of face {id} collapses at path point {}", k + 1));
        }
        sections.push(idx);
        cur = next;
    }
    let at = |k: usize, v: usize| sections[k][verts.iter().position(|&x| x == v).expect("a face vertex")];
    let first_new_id = shape.next_face_id();
    let mut next_id = first_new_id;
    let mut new_faces: Vec<Face> = Vec::new();
    let mut shrunk: Vec<u64> = Vec::new();
    for k in 0..nseg {
        for (e, &(x, y)) in dir_edges.iter().enumerate() {
            if k == 0 {
                if let Some(g) = stretch_with[e] {
                    splice(&mut shape.faces[g], x, y, at(1, x), at(1, y));
                    shrunk.push(shape.faces[g].id);
                    continue;
                }
            }
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
        // The face's own edges now bound the sweep's sides (or the shrunk faces) instead.
        for &(x, y) in &dir_edges {
            if shape.faces_with_edge(x, y).is_empty() {
                shape.remove_edge(x, y);
            }
        }
    }
    finish(shape);
    // The end cap landing on a parallel face of the same solid punches through it, as a push does.
    punch_through(shape, id);
    finish(shape);
    // Edges left on no face by a face that collapsed or was punched away go; the loose edges the
    // shape had before stay (as they were, or as the pieces a vertex landing on one split them into).
    let on = shape.loop_edges();
    let vertices = shape.vertices.clone();
    let was_loose = |a: usize, b: usize| {
        loose_before.iter().any(|&(p, q)| {
            (a, b) == (p, q) || (geom::on_closed_segment(vertices[a], vertices[p], vertices[q]) && geom::on_closed_segment(vertices[b], vertices[p], vertices[q]))
        })
    };
    shape.edges.retain(|&(a, b)| on.contains(&(a, b)) || was_loose(a, b));
    // The result must be sound.
    let touched: Vec<usize> = (0..shape.faces.len()).filter(|&t| shape.faces[t].id == id || shape.faces[t].id >= first_new_id || shrunk.contains(&shape.faces[t].id)).collect();
    check_structure(shape, &touched, &crowded_before, "followme")?;
    if closed_before && (touched.is_empty() || touched.iter().any(|&t| !component_closed(shape, t))) {
        return Err(format!("followme: the sweep of face {id} would not leave a closed solid"));
    }
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
/// keep their paint, copied edges that bound no copied face close faces like drawn lines. A copied
/// face landing inside a coplanar face, touching nothing, becomes an island of it (see
/// [`absorb_islands`]); one with an open edge lying over a face of a solid in any other way is
/// invalid (see [`check_copies`]). The shape is untouched by an invalid command.
pub fn transform(shape: &mut Shape, sel: &Selection, xf: Xf, copy: bool, array: u64) -> Result<(), OpError> {
    let mut work = shape.clone();
    transform_on(&mut work, sel, xf, copy, array)?;
    *shape = work;
    Ok(())
}

fn transform_on(shape: &mut Shape, sel: &Selection, xf: Xf, copy: bool, array: u64) -> Result<(), OpError> {
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
    let first_copy_id = shape.next_face_id();
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
    absorb_islands(shape, first_copy_id);
    check_copies(shape, first_copy_id, op)?;
    finish(shape);
    Ok(())
}

/// A copied face with an edge on no other face that overlaps a coplanar face of a solid (one
/// whose every edge is on other faces too) would leave that solid open: invalid. Copied solids
/// may interpenetrate, and lone faces may lie over other lone faces, as they could be drawn.
fn check_copies(shape: &Shape, first_id: u64, op: &str) -> Result<(), OpError> {
    let solid_face = |h: usize| shape.faces[h].directed_edges().iter().all(|&(a, b)| shape.faces_with_edge(a, b).len() >= 2);
    for c in 0..shape.faces.len() {
        if shape.faces[c].id < first_id || !shape.faces[c].directed_edges().iter().any(|&(a, b)| shape.faces_with_edge(a, b).len() < 2) {
            continue;
        }
        let Some(plane) = shape.face_plane(c) else { continue };
        for h in 0..shape.faces.len() {
            if h != c && shape.face_on_plane(h, &plane) && solid_face(h) && faces_overlap(shape, &plane, c, h) {
                return Err(format!("{op}: the copy (face {}) would overlap face {} of a solid", shape.faces[c].id, shape.faces[h].id));
            }
        }
    }
    Ok(())
}

/// A copied face (id at least `first_id`) that lands inside a coplanar face, touching nothing
/// else, is an island of it: the host gets the copy's outer loop as a hole, as it would had the
/// loop been drawn there (spec/sketch.md, `line`). A copy whose edges are on other faces, or that
/// crosses, encloses or lies in several faces, is left as it is (bodies may interpenetrate).
fn absorb_islands(shape: &mut Shape, first_id: u64) {
    for c in 0..shape.faces.len() {
        if shape.faces[c].id < first_id {
            continue;
        }
        let Some(plane) = shape.face_plane(c) else { continue };
        let outer = shape.faces[c].loops[0].clone();
        let n = outer.len();
        if (0..n).any(|i| shape.faces_with_edge(outer[i], outer[(i + 1) % n]).len() != 1) {
            continue;
        }
        let project = |lp: &[usize]| -> Vec<Point2> { lp.iter().map(|&v| plane.project(shape.vertices[v])).collect() };
        let c2 = project(&outer);
        let coplanar: Vec<usize> = (0..shape.faces.len()).filter(|&h| h != c && shape.face_on_plane(h, &plane)).collect();
        // Exactly one coplanar face holds the loop (inside its outer loop, clear of its holes), and
        // no other coplanar face reaches into the loop.
        let hosts: Vec<usize> = coplanar
            .iter()
            .copied()
            .filter(|&h| {
                let outer_h = project(&shape.faces[h].loops[0]);
                !rings_meet(&outer_h, &c2)
                    && inside_ring(&outer_h, c2[0])
                    && shape.faces[h].loops[1..].iter().all(|hole| {
                        let h2 = project(hole);
                        !rings_meet(&h2, &c2) && !inside_ring(&h2, c2[0]) && !inside_ring(&c2, h2[0])
                    })
            })
            .collect();
        let &[h] = hosts.as_slice() else { continue };
        if coplanar.iter().any(|&g| g != h && (rings_meet(&project(&shape.faces[g].loops[0]), &c2) || inside_ring(&c2, project(&shape.faces[g].loops[0])[0]))) {
            continue;
        }
        let mut hole = outer;
        if signed_area2(&project(&shape.faces[h].loops[0])) * signed_area2(&c2) > 0.0 {
            hole.reverse();
        }
        rotate_to_min(&mut hole);
        shape.faces[h].loops.push(hole);
    }
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
