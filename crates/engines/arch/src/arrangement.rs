//! The planar arrangement of a set of segments and its bounded faces.
//!
//! [`bounded_faces`] nodes the segments (splits every segment at its crossings with the others,
//! at the endpoints of others that lie on its interior within the tolerance — T-junctions — and
//! at the ends of collinear overlaps), merges vertices closer than the tolerance, drops dangling
//! edges (repeatedly removes vertices of degree 1) and cut edges (edges with the same face on both
//! sides, as shapely's `polygonize` does), traces the faces of the remaining half-edge graph and
//! returns the bounded ones (positive signed area) as counter-clockwise cycles. Every edge of a
//! face carries the index of the segment it came from; where segments overlap, the edge belongs
//! to the one with the smallest index.

use std::collections::{BTreeMap, BTreeSet};

/// A segment from `a` to `b` carried by `owner` (an index chosen by the caller).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub owner: usize,
}

/// A bounded face: its boundary counter-clockwise, edge `i` from `vertices[i]` to
/// `vertices[(i + 1) % n]`, carried by segment owner `owners[i]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub vertices: Vec<[f64; 2]>,
    pub owners: Vec<usize>,
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = sub(a, b);
    dot(d, d).sqrt()
}

/// Twice the signed area of a closed polygon (positive when counter-clockwise).
pub fn signed_area2(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| cross(pts[i], pts[(i + 1) % n])).sum()
}

/// The parameter of `p` projected on segment `a`–`b` and the distance of `p` from the segment.
fn project(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let d = sub(b, a);
    let l2 = dot(d, d);
    let t = if l2 > 0.0 { dot(sub(p, a), d) / l2 } else { 0.0 };
    let c = t.clamp(0.0, 1.0);
    (t, dist(p, [a[0] + c * d[0], a[1] + c * d[1]]))
}

/// Vertices merged within a tolerance, in insertion order.
struct Vertices {
    pts: Vec<[f64; 2]>,
    tol: f64,
}

impl Vertices {
    fn id(&mut self, p: [f64; 2]) -> usize {
        if let Some(i) = self.pts.iter().position(|q| dist(*q, p) <= self.tol) {
            return i;
        }
        self.pts.push(p);
        self.pts.len() - 1
    }
}

/// The noded edges `(u, v) → owner` (u < v) and the vertex positions.
fn node(segs: &[Segment], tol: f64) -> (Vec<[f64; 2]>, BTreeMap<(usize, usize), usize>) {
    // Split points of every segment: (parameter, point).
    let mut splits: Vec<Vec<(f64, [f64; 2])>> = segs.iter().map(|s| vec![(0.0, s.a), (1.0, s.b)]).collect();
    for i in 0..segs.len() {
        for j in i + 1..segs.len() {
            let (si, sj) = (segs[i], segs[j]);
            // Endpoints of one segment on the interior of the other (T-junctions, overlaps): split
            // the other at the endpoint itself so the vertices coincide exactly.
            for (x, y, k) in [(si, sj, i), (sj, si, j)] {
                for p in [y.a, y.b] {
                    let (t, d) = project(p, x.a, x.b);
                    if d <= tol && dist(p, x.a) > tol && dist(p, x.b) > tol {
                        splits[k].push((t.clamp(0.0, 1.0), p));
                    }
                }
            }
            // Proper crossings.
            let (d1, d2) = (sub(si.b, si.a), sub(sj.b, sj.a));
            let den = cross(d1, d2);
            if den.abs() <= 1e-12 * dot(d1, d1).sqrt() * dot(d2, d2).sqrt() {
                continue;
            }
            let w = sub(sj.a, si.a);
            let (t, u) = (cross(w, d2) / den, cross(w, d1) / den);
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                let p = [si.a[0] + t * d1[0], si.a[1] + t * d1[1]];
                splits[i].push((t, p));
                splits[j].push((u, p));
            }
        }
    }
    let mut verts = Vertices { pts: Vec::new(), tol };
    let mut edges = BTreeMap::new();
    for (s, mut sp) in segs.iter().zip(splits) {
        sp.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ids: Vec<usize> = sp.iter().map(|(_, p)| verts.id(*p)).collect();
        for w in ids.windows(2) {
            if w[0] != w[1] {
                edges.entry((w[0].min(w[1]), w[0].max(w[1]))).or_insert(s.owner);
            }
        }
    }
    (verts.pts, edges)
}

/// Removes edges at vertices of degree 1 until there are none.
fn prune(edges: &mut BTreeMap<(usize, usize), usize>) {
    loop {
        let mut degree: BTreeMap<usize, usize> = BTreeMap::new();
        for &(u, v) in edges.keys() {
            *degree.entry(u).or_default() += 1;
            *degree.entry(v).or_default() += 1;
        }
        let before = edges.len();
        edges.retain(|&(u, v), _| degree[&u] > 1 && degree[&v] > 1);
        if edges.len() == before {
            return;
        }
    }
}

/// The faces of the half-edge graph of `edges`: half-edge `2k` runs u→v of the k-th edge, `2k+1`
/// back. Returns the half-edges and, per half-edge, its face index, faces as half-edge cycles.
fn trace(pts: &[[f64; 2]], edges: &[(usize, usize)]) -> (Vec<usize>, Vec<Vec<usize>>) {
    let he = |h: usize| if h % 2 == 0 { edges[h / 2] } else { (edges[h / 2].1, edges[h / 2].0) };
    // Outgoing half-edges of every vertex, by angle.
    let mut out: BTreeMap<usize, Vec<(f64, usize)>> = BTreeMap::new();
    for h in 0..2 * edges.len() {
        let (u, v) = he(h);
        let d = sub(pts[v], pts[u]);
        out.entry(u).or_default().push((d[1].atan2(d[0]), h));
    }
    let mut pos = vec![0; 2 * edges.len()];
    for list in out.values_mut() {
        list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (k, &(_, h)) in list.iter().enumerate() {
            pos[h] = k;
        }
    }
    // next(u→v) = the outgoing half-edge at v just clockwise of v→u: the face lies on the left.
    let next = |h: usize| {
        let twin = h ^ 1;
        let list = &out[&he(h).1];
        list[(pos[twin] + list.len() - 1) % list.len()].1
    };
    let mut face = vec![usize::MAX; 2 * edges.len()];
    let mut faces = Vec::new();
    for start in 0..2 * edges.len() {
        if face[start] != usize::MAX {
            continue;
        }
        let mut cycle = Vec::new();
        let mut h = start;
        while face[h] == usize::MAX {
            face[h] = faces.len();
            cycle.push(h);
            h = next(h);
        }
        faces.push(cycle);
    }
    (face, faces)
}

/// The bounded faces of the arrangement of `segs`, vertices merged within `tol`.
pub fn bounded_faces(segs: &[Segment], tol: f64) -> Vec<Face> {
    let (pts, mut edges) = node(segs, tol);
    loop {
        prune(&mut edges);
        let list: Vec<(usize, usize)> = edges.keys().copied().collect();
        let (face, faces) = trace(&pts, &list);
        let cut: BTreeSet<(usize, usize)> =
            (0..list.len()).filter(|&k| face[2 * k] == face[2 * k + 1]).map(|k| list[k]).collect();
        if !cut.is_empty() {
            edges.retain(|e, _| !cut.contains(e));
            continue;
        }
        let owners: Vec<usize> = edges.values().copied().collect();
        let mut result = Vec::new();
        for cycle in faces {
            let vertices: Vec<[f64; 2]> =
                cycle.iter().map(|&h| pts[if h % 2 == 0 { list[h / 2].0 } else { list[h / 2].1 }]).collect();
            if signed_area2(&vertices) > 0.0 {
                result.push(Face { vertices, owners: cycle.iter().map(|&h| owners[h / 2]).collect() });
            }
        }
        return result;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(a: [f64; 2], b: [f64; 2], owner: usize) -> Segment {
        Segment { a, b, owner }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Segment> {
        vec![
            seg([x0, y0], [x1, y0], 0),
            seg([x1, y1], [x1, y0], 1),
            seg([x1, y1], [x0, y1], 2),
            seg([x0, y0], [x0, y1], 3),
        ]
    }

    fn areas(faces: &[Face]) -> Vec<f64> {
        let mut a: Vec<f64> = faces.iter().map(|f| signed_area2(&f.vertices) / 2.0).collect();
        a.sort_by(f64::total_cmp);
        a
    }

    #[test]
    fn a_square_has_one_bounded_face_whatever_the_wall_directions() {
        let faces = bounded_faces(&rect(0.0, 0.0, 2.0, 1.0), 1e-6);
        assert_eq!(areas(&faces), vec![2.0]);
        assert_eq!(faces[0].vertices.len(), 4);
    }

    #[test]
    fn t_junctions_crossings_and_dangling_walls() {
        let mut s = rect(0.0, 0.0, 10.0, 6.0);
        s.push(seg([4.0, 0.0], [4.0, 6.0], 4)); // T at both ends
        s.push(seg([10.0, 2.5], [0.0, 2.5], 5)); // crosses the other partition
        s.push(seg([7.0, 6.0], [7.0, 4.5], 6)); // dangling stub
        s.push(seg([20.0, 0.0], [21.0, 1.0], 7)); // isolated wall
        let faces = bounded_faces(&s, 1e-6);
        assert_eq!(areas(&faces), vec![10.0, 14.0, 15.0, 21.0]);
        assert!(faces.iter().all(|f| !f.owners.contains(&6) && !f.owners.contains(&7)));
    }

    #[test]
    fn cut_edges_and_nested_islands() {
        // A bridge wall from the outer room to an inner closed room: no face for the bridge, both
        // rooms bounded.
        let mut s = rect(0.0, 0.0, 10.0, 10.0);
        s.extend(rect(4.0, 4.0, 6.0, 6.0).into_iter().map(|g| seg(g.a, g.b, g.owner + 10)));
        s.push(seg([0.0, 5.0], [4.0, 5.0], 20));
        let faces = bounded_faces(&s, 1e-6);
        assert_eq!(areas(&faces), vec![4.0, 100.0]);
        assert!(faces.iter().all(|f| !f.owners.contains(&20)));
    }

    #[test]
    fn near_coincident_endpoints_merge() {
        let s = vec![
            seg([0.0, 0.0], [1.0, 0.0], 0),
            seg([1.0 + 4e-7, 0.0], [1.0, 1.0], 1),
            seg([1.0, 1.0 - 3e-7], [0.0, 1.0], 2),
            seg([0.0, 1.0], [0.0, 2e-7], 3),
        ];
        let faces = bounded_faces(&s, 1e-6);
        assert_eq!(faces.len(), 1);
        assert!((signed_area2(&faces[0].vertices) / 2.0 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn collinear_overlaps_keep_the_first_owner() {
        let mut s = rect(0.0, 0.0, 4.0, 2.0);
        s.push(seg([1.0, 0.0], [3.0, 0.0], 9));
        let faces = bounded_faces(&s, 1e-6);
        assert_eq!(areas(&faces), vec![8.0]);
        assert!(!faces[0].owners.contains(&9));
    }
}
