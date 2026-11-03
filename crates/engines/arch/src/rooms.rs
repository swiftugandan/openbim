//! Room geometry from a bounded face of the wall arrangement (spec/schema.md "Rooms"): the inner
//! face of each edge is its centreline offset inward by the half thickness of the wall carrying
//! it; consecutive offset lines meet at mitred corners.

use crate::arrangement::signed_area2;

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn norm(a: [f64; 2]) -> f64 {
    dot(a, a).sqrt()
}

/// Whether `d` and `e` are parallel within a relative tolerance.
fn parallel(d: [f64; 2], e: [f64; 2]) -> bool {
    cross(d, e).abs() <= 1e-12 * norm(d) * norm(e)
}

/// An edge of the face: start point, direction (to its end), half thickness.
#[derive(Clone, Copy)]
struct Edge {
    p: [f64; 2],
    d: [f64; 2],
    h: f64,
}

impl Edge {
    fn end(&self) -> [f64; 2] {
        [self.p[0] + self.d[0], self.p[1] + self.d[1]]
    }

    /// The inward (left) unit normal.
    fn normal(&self) -> [f64; 2] {
        let n = norm(self.d);
        [-self.d[1] / n, self.d[0] / n]
    }

    /// A point of the offset line.
    fn offset_point(&self, at: [f64; 2]) -> [f64; 2] {
        let n = self.normal();
        [at[0] + n[0] * self.h, at[1] + n[1] * self.h]
    }

    /// Whether `next` continues this edge straight on with the same half thickness.
    fn continued_by(&self, next: &Edge) -> bool {
        parallel(self.d, next.d) && dot(self.d, next.d) > 0.0 && (self.h - next.h).abs() <= 1e-12
    }
}

/// The inner-face polygon of a counter-clockwise face whose edge `i` (from `vertices[i]` to the
/// next vertex) has half thickness `half[i]`, counter-clockwise. Consecutive straight-on edges of
/// equal thickness are one edge; between straight-on edges of different thickness the inner face
/// steps across along the normal at their common vertex. `None` if the face has fewer than three
/// non-degenerate sides, or if the offset polygon turns over along an edge or crosses itself (a
/// face narrower than its walls, a side shorter than its corners' mitres).
pub fn inner_polygon(vertices: &[[f64; 2]], half: &[f64]) -> Option<Vec<[f64; 2]>> {
    let n = vertices.len();
    let mut edges: Vec<Edge> = Vec::new();
    for i in 0..n {
        let d = sub(vertices[(i + 1) % n], vertices[i]);
        if norm(d) == 0.0 {
            continue;
        }
        let e = Edge { p: vertices[i], d, h: half[i] };
        match edges.last_mut() {
            Some(last) if last.continued_by(&e) => last.d = sub(e.end(), last.p),
            _ => edges.push(e),
        }
    }
    if edges.len() >= 2 && edges[edges.len() - 1].continued_by(&edges[0]) {
        let last = edges.pop()?;
        edges[0] = Edge { p: last.p, d: sub(edges[0].end(), last.p), h: last.h };
    }
    if edges.len() < 3 {
        return None;
    }
    let mut out = Vec::with_capacity(edges.len());
    // Per edge, the index in `out` of its first and of its last inner-face vertex.
    let mut span = Vec::with_capacity(edges.len());
    for i in 0..edges.len() {
        let (a, b) = (edges[(i + edges.len() - 1) % edges.len()], edges[i]);
        if parallel(a.d, b.d) {
            // Straight on with a change of thickness (or a reversal): step along the normals.
            out.push(a.offset_point(b.p));
            out.push(b.offset_point(b.p));
        } else {
            let (pa, pb) = (a.offset_point(a.p), b.offset_point(b.p));
            let t = cross(sub(pb, pa), b.d) / cross(a.d, b.d);
            out.push([pa[0] + t * a.d[0], pa[1] + t * a.d[1]]);
        }
        span.push(out.len() - 1);
    }
    // The inner face must run the way of the centreline along every edge (an edge shorter than
    // its corners' mitres turns over) and must not cross itself.
    if !runs_forward(&edges, &out, &span) || !simple(&out) {
        return None;
    }
    Some(out)
}

/// Whether every edge's inner face (from its first to its last inner vertex) runs along it.
fn runs_forward(edges: &[Edge], out: &[[f64; 2]], span: &[usize]) -> bool {
    let (m, k) = (edges.len(), out.len());
    (0..m).all(|i| {
        let start = out[span[i]];
        // The edge's last vertex: the first one its successor's join pushed.
        let next = &edges[(i + 1) % m];
        let first_of_next = if parallel(edges[i].d, next.d) { (span[(i + 1) % m] + k - 1) % k } else { span[(i + 1) % m] };
        dot(sub(out[first_of_next], start), edges[i].d) > 0.0
    })
}

/// Whether no two non-adjacent sides of the closed polygon `pts` touch or cross.
fn simple(pts: &[[f64; 2]]) -> bool {
    let n = pts.len();
    let seg = |i: usize| (pts[i], pts[(i + 1) % n]);
    let orient = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| cross(sub(b, a), sub(c, a));
    for i in 0..n {
        for j in i + 1..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            let ((a, b), (c, d)) = (seg(i), seg(j));
            let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
            if o1 * o2 <= 0.0 && o3 * o4 <= 0.0 && (o1 != 0.0 || o2 != 0.0 || o3 != 0.0 || o4 != 0.0) {
                return false;
            }
        }
    }
    true
}

/// The area of a counter-clockwise polygon (negative when clockwise).
pub fn area(pts: &[[f64; 2]]) -> f64 {
    signed_area2(pts) / 2.0
}

/// The area centroid of a polygon of non-zero area.
pub fn centroid(pts: &[[f64; 2]]) -> [f64; 2] {
    let n = pts.len();
    let (mut cx, mut cy, mut a2) = (0.0, 0.0, 0.0);
    // Relative to the first vertex, for accuracy far from the origin.
    let o = pts[0];
    for i in 0..n {
        let (p, q) = (sub(pts[i], o), sub(pts[(i + 1) % n], o));
        let c = cross(p, q);
        a2 += c;
        cx += (p[0] + q[0]) * c;
        cy += (p[1] + q[1]) * c;
    }
    [o[0] + cx / (3.0 * a2), o[1] + cy / (3.0 * a2)]
}

/// Whether `p` lies inside the polygon `pts` (even-odd rule; points on the boundary may go
/// either way).
pub fn contains(pts: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = pts.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn l_shaped_room_with_uniform_walls() {
        let pts = [[0.0, 0.0], [6.0, 0.0], [6.0, 3.0], [3.0, 3.0], [3.0, 6.0], [0.0, 6.0]];
        let inner = inner_polygon(&pts, &[0.11; 6]).unwrap();
        assert_eq!(inner.len(), 6);
        assert!(close(area(&inner), 5.78 * 2.78 + 2.78 * (6.0 - 0.11 - 2.89)));
        assert!(close(inner[3][0], 2.89) && close(inner[3][1], 2.89));
    }

    #[test]
    fn mixed_thicknesses_and_split_collinear_edges() {
        // 8 × 5 with a 0.15 partition on the right and the bottom edge split in two.
        let pts = [[0.0, 0.0], [1.5, 0.0], [3.0, 0.0], [3.0, 5.0], [0.0, 5.0]];
        let inner = inner_polygon(&pts, &[0.15, 0.15, 0.075, 0.15, 0.15]).unwrap();
        assert_eq!(inner.len(), 4);
        assert!(close(area(&inner), (3.0 - 0.15 - 0.075) * (5.0 - 0.3)));
        // The split may also straddle the first vertex.
        let pts = [[1.5, 0.0], [3.0, 0.0], [3.0, 5.0], [0.0, 5.0], [0.0, 0.0]];
        let inner = inner_polygon(&pts, &[0.15, 0.075, 0.15, 0.15, 0.15]).unwrap();
        assert_eq!(inner.len(), 4);
        assert!(close(area(&inner), (3.0 - 0.15 - 0.075) * (5.0 - 0.3)));
    }

    #[test]
    fn a_change_of_thickness_along_a_straight_side_steps() {
        let pts = [[0.0, 0.0], [2.0, 0.0], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0]];
        let inner = inner_polygon(&pts, &[0.1, 0.2, 0.1, 0.1, 0.1]).unwrap();
        assert_eq!(inner.len(), 6);
        assert!(close(area(&inner), 3.8 * 1.8 - 1.9 * 0.1));
    }

    #[test]
    fn centroid_and_containment() {
        let sq = [[1.0, 1.0], [3.0, 1.0], [3.0, 2.0], [1.0, 2.0]];
        let c = centroid(&sq);
        assert!(close(c[0], 2.0) && close(c[1], 1.5));
        assert!(contains(&sq, [2.0, 1.5]) && !contains(&sq, [0.5, 1.5]) && !contains(&sq, [2.0, 2.5]));
    }
}
