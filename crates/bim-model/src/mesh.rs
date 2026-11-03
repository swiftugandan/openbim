//! Indexed triangle meshes and the one solid construction every element uses: a vertical prism
//! over a convex plan polygon minus axis-aligned "slot" boxes.
//!
//! [`cut_prism`] works on a cell complex instead of a general boolean (which is what keeps it
//! robust, see `bim_geom` issue #24): the plan polygon is cut into convex pieces by the lines
//! `s = c` at every box edge, the height into intervals at every box bottom and top, and each
//! (piece, interval) cell is either solid or removed. Volume is the exact sum over solid cells;
//! the mesh is the boundary between solid and removed cells. Pieces share the exact same cut
//! points (each is computed once) and every column uses every height level, so the mesh is
//! edge-to-edge: closed and consistently oriented by construction.

use bim_geom::{Point3, Solid};

/// An indexed triangle mesh, triangles wound counter-clockwise seen from outside.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    /// Vertex positions (model frame, metres, z up).
    pub positions: Vec<Point3>,
    /// Triangles as indices into `positions`.
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// Signed volume enclosed by the mesh (positive for an outward-oriented closed mesh).
    pub fn volume(&self) -> f64 {
        let mut v = 0.0;
        for t in &self.triangles {
            let [a, b, c] = t.map(|i| self.positions[i as usize]);
            v += a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x) + a.z * (b.x * c.y - b.y * c.x);
        }
        v / 6.0
    }

    /// Whether every directed edge appears exactly once and its reverse exactly once (a closed,
    /// consistently oriented 2-manifold surface, by indices).
    pub fn is_closed(&self) -> bool {
        use std::collections::HashMap;
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &self.triangles {
            for k in 0..3 {
                *edges.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
            }
        }
        !self.triangles.is_empty()
            && edges.iter().all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1))
    }

    /// Axis-aligned bounds `(min, max)`, or `None` for an empty mesh.
    pub fn bounds(&self) -> Option<(Point3, Point3)> {
        let first = *self.positions.first()?;
        let (mut lo, mut hi) = (first, first);
        for p in &self.positions {
            lo = Point3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Point3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        Some((lo, hi))
    }

    /// The mesh as a half-edge [`Solid`] (one triangular face per triangle).
    pub fn to_solid(&self) -> Solid {
        let faces = self.triangles.iter().map(|t| vec![t.iter().map(|&i| i as usize).collect()]).collect();
        Solid::from_faces(self.positions.clone(), faces)
    }

    /// Indexes a triangle soup, merging bit-identical points.
    pub fn from_triangle_soup(tris: &[[Point3; 3]]) -> Mesh {
        use std::collections::HashMap;
        let mut index: HashMap<[u64; 3], u32> = HashMap::new();
        let mut mesh = Mesh::default();
        for t in tris {
            let mut ids = [0u32; 3];
            for (k, p) in t.iter().enumerate() {
                let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
                ids[k] = *index.entry(key).or_insert_with(|| {
                    mesh.positions.push(*p);
                    (mesh.positions.len() - 1) as u32
                });
            }
            if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
                mesh.triangles.push(ids);
            }
        }
        mesh
    }
}

/// A box removed from a prism: `s ∈ [s0, s1]` along the plan's first coordinate (unbounded in the
/// second) and `z ∈ [z0, z1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub s0: f64,
    pub s1: f64,
    pub z0: f64,
    pub z1: f64,
}

/// Breakpoints closer than this are merged (and snapped onto polygon vertices), so no mesh gets
/// two distinct vertices that a float32 round trip and a 1e-6 vertex weld would fuse.
const SNAP: f64 = 1e-7;

/// Signed area of a ring of 2D points (positive when counter-clockwise).
pub fn ring_area(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (ring[i], ring[(i + 1) % n]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a / 2.0
}

/// Clips a convex polygon to the half-plane `c + s·ns + l·nl ≥ 0` (Sutherland–Hodgman).
pub fn clip_half_plane(poly: &[[f64; 2]], c: f64, ns: f64, nl: f64) -> Vec<[f64; 2]> {
    let f = |p: [f64; 2]| c + p[0] * ns + p[1] * nl;
    let n = poly.len();
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let (fp, fq) = (f(p), f(q));
        if fp >= 0.0 {
            out.push(p);
        }
        if (fp >= 0.0) != (fq >= 0.0) {
            let t = fp / (fp - fq);
            out.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
        }
    }
    dedup_ring(out)
}

/// Drops consecutive (and wrap-around) near-duplicate points.
pub fn dedup_ring(ring: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() <= 1e-12 && (a[1] - b[1]).abs() <= 1e-12;
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(ring.len());
    for p in ring {
        if out.last().map_or(true, |&q| !close(p, q)) {
            out.push(p);
        }
    }
    while out.len() > 1 && close(out[0], out[out.len() - 1]) {
        out.pop();
    }
    out
}

/// Merges sorted values closer than [`SNAP`], keeping the first of each run.
fn merge_levels(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.total_cmp(b));
    let mut out: Vec<f64> = Vec::with_capacity(v.len());
    for x in v {
        if out.last().map_or(true, |&y| x - y > SNAP) {
            out.push(x);
        }
    }
    out
}

/// The prism over the convex counter-clockwise polygon `poly` (plan coordinates `(s, l)`) from
/// `z0` up by `h`, minus every [`Slot`]. Returns the exact volume and, when `with_mesh`, the
/// closed outward-oriented boundary mesh with vertices `(s, l, z)` (empty when nothing is left).
pub fn cut_prism(poly: &[[f64; 2]], z0: f64, h: f64, slots: &[Slot], with_mesh: bool) -> (f64, Mesh) {
    let mut mesh = Mesh::default();
    let poly = dedup_ring(poly.to_vec());
    if poly.len() < 3 || !(h > 0.0) || !(ring_area(&poly) > 0.0) {
        return (0.0, mesh);
    }
    let z1 = z0 + h;
    let smin = poly.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let smax = poly.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);

    // Slots clipped to the prism; empty ones dropped.
    let slots: Vec<Slot> = slots
        .iter()
        .filter(|b| b.s0.is_finite() && b.s1.is_finite() && b.z0.is_finite() && b.z1.is_finite())
        .map(|b| Slot { s0: b.s0.max(smin), s1: b.s1.min(smax), z0: b.z0.max(z0), z1: b.z1.min(z1) })
        .filter(|b| b.s1 - b.s0 > SNAP && b.z1 - b.z0 > SNAP)
        .collect();

    // Cut lines in s: the slot edges strictly inside (smin, smax), snapped onto polygon vertices.
    let mut cuts: Vec<f64> = Vec::new();
    for c in merge_levels(slots.iter().flat_map(|b| [b.s0, b.s1]).collect()) {
        if c - smin <= SNAP || smax - c <= SNAP {
            continue;
        }
        let snapped = poly.iter().map(|p| p[0]).find(|&s| (s - c).abs() <= SNAP).unwrap_or(c);
        cuts.push(snapped);
    }
    let mut cols = vec![smin];
    cols.extend(merge_levels(cuts));
    cols.push(smax);
    let cols = merge_levels(cols);
    // Height levels.
    let mut zs = vec![z0, z1];
    for b in &slots {
        if b.z0 - z0 > SNAP && z1 - b.z0 > SNAP {
            zs.push(b.z0);
        }
        if b.z1 - z0 > SNAP && z1 - b.z1 > SNAP {
            zs.push(b.z1);
        }
    }
    let zs = merge_levels(zs);
    let (ncol, nz) = (cols.len() - 1, zs.len() - 1);

    // Solid cells: not covered by any slot (tested at the cell centre).
    let solid: Vec<Vec<bool>> = (0..ncol)
        .map(|j| {
            let sm = (cols[j] + cols[j + 1]) / 2.0;
            (0..nz)
                .map(|k| {
                    let zm = (zs[k] + zs[k + 1]) / 2.0;
                    !slots.iter().any(|b| b.s0 <= sm && sm <= b.s1 && b.z0 <= zm && zm <= b.z1)
                })
                .collect()
        })
        .collect();

    // Boundary refined at every internal cut: each cut point is computed once and shared.
    let internal = &cols[1..ncol];
    let mut q: Vec<[f64; 2]> = Vec::new();
    let n = poly.len();
    for i in 0..n {
        let (p, r) = (poly[i], poly[(i + 1) % n]);
        q.push(p);
        let (lo, hi) = (p[0].min(r[0]), p[0].max(r[0]));
        let mut between: Vec<f64> = internal.iter().copied().filter(|&c| c > lo && c < hi).collect();
        if r[0] < p[0] {
            between.reverse();
        }
        for c in between {
            let t = (c - p[0]) / (r[0] - p[0]);
            q.push([c, p[1] + t * (r[1] - p[1])]);
        }
    }
    // Pieces: the refined vertices inside each column, in boundary order (convexity keeps them
    // a convex polygon).
    let pieces: Vec<Vec<usize>> = (0..ncol)
        .map(|j| (0..q.len()).filter(|&i| q[i][0] >= cols[j] && q[i][0] <= cols[j + 1]).collect())
        .collect();
    let areas: Vec<f64> = pieces
        .iter()
        .map(|idx| if idx.len() < 3 { 0.0 } else { ring_area(&idx.iter().map(|&i| q[i]).collect::<Vec<_>>()) })
        .collect();

    let mut volume = 0.0;
    for j in 0..ncol {
        let height: f64 = (0..nz).filter(|&k| solid[j][k]).map(|k| zs[k + 1] - zs[k]).sum();
        volume += areas[j].max(0.0) * height;
    }
    if !with_mesh {
        return (volume, mesh);
    }

    let mut vid = vec![vec![u32::MAX; nz + 1]; q.len()];
    let mut vertex = |mesh: &mut Mesh, i: usize, k: usize| -> u32 {
        if vid[i][k] == u32::MAX {
            mesh.positions.push(Point3::new(q[i][0], q[i][1], zs[k]));
            vid[i][k] = (mesh.positions.len() - 1) as u32;
        }
        vid[i][k]
    };
    let is_solid = |j: usize, k: isize| k >= 0 && (k as usize) < nz && solid[j][k as usize];
    for j in 0..ncol {
        let idx = &pieces[j];
        if idx.len() < 3 || !(areas[j] > 0.0) {
            continue;
        }
        // Horizontal faces where the cell below and the cell above differ.
        for k in 0..=nz {
            let (below, above) = (is_solid(j, k as isize - 1), is_solid(j, k as isize));
            if below == above {
                continue;
            }
            let m = idx.len() as f64;
            let cs = idx.iter().map(|&i| q[i][0]).sum::<f64>() / m;
            let cl = idx.iter().map(|&i| q[i][1]).sum::<f64>() / m;
            mesh.positions.push(Point3::new(cs, cl, zs[k]));
            let centre = (mesh.positions.len() - 1) as u32;
            for e in 0..idx.len() {
                let a = vertex(&mut mesh, idx[e], k);
                let b = vertex(&mut mesh, idx[(e + 1) % idx.len()], k);
                // Top of a solid cell faces +z (counter-clockwise), bottom faces −z.
                mesh.triangles.push(if below { [centre, a, b] } else { [centre, b, a] });
            }
        }
        // Vertical faces on the piece's edges.
        for e in 0..idx.len() {
            let (ia, ib) = (idx[e], idx[(e + 1) % idx.len()]);
            let neighbour = if q[ia][0] == q[ib][0] {
                if j > 0 && q[ia][0] == cols[j] {
                    Some(j - 1)
                } else if j + 1 < ncol && q[ia][0] == cols[j + 1] {
                    Some(j + 1)
                } else {
                    None
                }
            } else {
                None
            };
            for k in 0..nz {
                if !solid[j][k] || neighbour.map_or(false, |o| solid[o][k] && areas[o] > 0.0) {
                    continue;
                }
                let a0 = vertex(&mut mesh, ia, k);
                let b0 = vertex(&mut mesh, ib, k);
                let b1 = vertex(&mut mesh, ib, k + 1);
                let a1 = vertex(&mut mesh, ia, k + 1);
                mesh.triangles.push([a0, b0, b1]);
                mesh.triangles.push([a0, b1, a1]);
            }
        }
    }
    (volume, mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(s0: f64, s1: f64, l0: f64, l1: f64) -> Vec<[f64; 2]> {
        vec![[s0, l0], [s1, l0], [s1, l1], [s0, l1]]
    }

    #[test]
    fn plain_prism() {
        let (v, m) = cut_prism(&rect(0.0, 4.0, -0.1, 0.1), 0.0, 3.0, &[], true);
        assert!((v - 2.4).abs() < 1e-12);
        assert!(m.is_closed());
        assert!((m.volume() - v).abs() < 1e-12);
    }

    #[test]
    fn slots_inside_outside_overlapping_and_at_ends() {
        let poly = vec![[0.0, -0.1], [5.0, -0.1], [5.2, 0.1], [-0.3, 0.1]];
        let cases: Vec<Vec<Slot>> = vec![
            vec![Slot { s0: 1.0, s1: 2.0, z0: 0.5, z1: 2.0 }],
            vec![Slot { s0: -1.0, s1: 0.5, z0: 0.0, z1: 2.0 }],
            vec![Slot { s0: 1.0, s1: 2.0, z0: 0.5, z1: 2.0 }, Slot { s0: 1.5, s1: 3.0, z0: 1.0, z1: 3.5 }],
            vec![Slot { s0: 4.0, s1: 9.0, z0: -1.0, z1: 1.0 }],
            vec![Slot { s0: 1.0, s1: 2.0, z0: 0.0, z1: 1.0 }, Slot { s0: 2.0, s1: 3.0, z0: 1.0, z1: 3.0 }],
            vec![Slot { s0: 0.0, s1: 5.0, z0: 1.0, z1: 2.0 }],
        ];
        for (i, slots) in cases.iter().enumerate() {
            let (v, m) = cut_prism(&poly, 0.0, 3.0, slots, true);
            // Case 4 touches itself along an edge (not a 2-manifold by indices).
            assert!(m.is_closed() || i == 4, "{slots:?}");
            assert!((m.volume() - v).abs() < 1e-9, "{slots:?}: {} vs {v}", m.volume());
        }
        // Overlapping slots are not subtracted twice.
        let (v, _) = cut_prism(&rect(0.0, 4.0, 0.0, 1.0), 0.0, 3.0,
            &[Slot { s0: 1.0, s1: 2.0, z0: 0.0, z1: 2.0 }, Slot { s0: 1.5, s1: 2.5, z0: 1.0, z1: 3.0 }], false);
        let removed = 2.0 + 2.0 - 0.5;
        assert!((v - (12.0 - removed)).abs() < 1e-12, "{v}");
    }
}
