//! Clipping a closed triangle mesh to a half-space, capped by its section.

use std::collections::HashMap;

use bim_geom::{Point3, Polygon3, Solid, Vec3};
use crate::Mesh;

/// The part of `mesh` with depth `p·f ≥ c`, as a [`Solid`] whose faces are the clipped triangles
/// plus the `cap` regions (the section by the plane, outer loops counter-clockwise about `−f`).
/// `None` when nothing of the mesh lies on the far side; the mesh itself when nothing lies on the
/// near side.
///
/// Vertices within a small tolerance of the plane count as kept. A crossing point is computed
/// once per mesh edge from its endpoints in a fixed order, so neighbouring triangles share it
/// exactly and the clipped faces stay edge-to-edge.
pub(crate) fn clip_far(mesh: &Mesh, f: Vec3, c: f64, cap: &[Polygon3]) -> Option<Solid> {
    let (lo, hi) = mesh.bounds()?;
    let extent = (hi - lo).norm();
    let tol = 1e-9 * (extent + c.abs()).max(1e-3);
    let depth: Vec<f64> = mesh.positions.iter().map(|p| p.to_vec().dot(f) - c).collect();
    let near: Vec<bool> = depth.iter().map(|&d| d < -tol).collect();
    if !near.iter().any(|&n| n) {
        return Some(mesh.to_solid());
    }
    if mesh.triangles.iter().flatten().all(|&i| near[i as usize]) {
        return None;
    }

    let mut points: Vec<Point3> = mesh.positions.clone();
    let mut crossings: HashMap<(usize, usize), usize> = HashMap::new();
    let mut crossing = |i: usize, j: usize, points: &mut Vec<Point3>| -> usize {
        let kept = if near[i] { j } else { i };
        if depth[kept].abs() <= tol {
            return kept;
        }
        let key = (i.min(j), i.max(j));
        *crossings.entry(key).or_insert_with(|| {
            let (a, b) = key;
            let t = -depth[a] / (depth[b] - depth[a]);
            let (pa, pb) = (points[a], points[b]);
            points.push(pa + (pb - pa) * t);
            points.len() - 1
        })
    };

    let mut faces: Vec<Vec<Vec<usize>>> = Vec::new();
    for tri in &mesh.triangles {
        let t = tri.map(|i| i as usize);
        let kept = t.map(|i| !near[i]);
        if kept.iter().all(|&k| k) {
            faces.push(vec![t.to_vec()]);
            continue;
        }
        if !kept.iter().any(|&k| k) {
            continue;
        }
        let mut poly: Vec<usize> = Vec::with_capacity(4);
        for e in 0..3 {
            let (i, j) = (t[e], t[(e + 1) % 3]);
            if !near[i] {
                poly.push(i);
            }
            if near[i] != near[j] {
                let x = crossing(i, j, &mut points);
                poly.push(x);
            }
        }
        poly.dedup();
        while poly.len() > 1 && poly.first() == poly.last() {
            poly.pop();
        }
        if poly.len() >= 3 {
            faces.push(vec![poly]);
        }
    }

    for region in cap {
        let mut face = Vec::new();
        for ring in std::iter::once(&region.outer).chain(region.holes.iter()) {
            if ring.len() < 3 {
                continue;
            }
            let base = points.len();
            points.extend(ring.iter().copied());
            face.push((base..points.len()).collect::<Vec<usize>>());
        }
        if !face.is_empty() && region.outer.len() >= 3 {
            faces.push(face);
        }
    }
    Some(Solid::from_faces(points, faces))
}
