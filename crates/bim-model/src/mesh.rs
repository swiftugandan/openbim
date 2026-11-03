//! Indexed triangle meshes and the two solid constructions every element uses: a vertical prism
//! over a convex plan polygon minus axis-aligned "slot" boxes ([`cut_prism`]), and the union of
//! axis-aligned boxes ([`box_union`]).
//!
//! Both work on a cell complex instead of a general boolean (which is what keeps them robust, see
//! `bim_geom` issue #24). [`cut_prism`]: the plan polygon is cut into convex pieces by the lines
//! `s = c` at every box edge, the height into intervals at every box bottom and top, and each
//! (piece, interval) cell is either solid or removed. Volume is the exact sum over solid cells;
//! the mesh is the boundary between solid and removed cells. Pieces share the exact same cut
//! points (each is computed once) and every column uses every height level, so the mesh is
//! edge-to-edge: closed and consistently oriented by construction. [`box_union`] does the same on
//! the grid of all box faces.

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

/// An axis-aligned box: the product of the intervals `lo[a]..hi[a]` over the three axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub lo: [f64; 3],
    pub hi: [f64; 3],
}

/// Most cells [`box_union`] lays out; a union of boxes that needs more gets no mesh.
const MAX_CELLS: usize = 1 << 24;

/// The union of `boxes` as one closed, outward-oriented 2-manifold mesh, built on a cell complex
/// without booleans (issue #24). Returns the exact volume of the union and its boundary mesh, in
/// the boxes' own coordinates.
///
/// The grid lines are all box faces, per axis, with faces closer than 1e-7 merged into the
/// smallest of them (so boxes that touch, overlap or nearly coincide share exact coordinates); a
/// cell is occupied when some box covers it; the volume is the sum over occupied cells and the
/// mesh the boundary between occupied and empty cells (one quad of two triangles per cell face,
/// no coplanar merging, which would leave T-junctions). Vertices are shared through their grid
/// index, so the mesh is edge-to-edge: closed and consistently oriented by construction.
///
/// A box with a non-finite bound or an extent of at most 1e-7 along any axis is ignored. The
/// result is empty (volume 0) when no box is left, when the grid would exceed 2²⁴ cells, or when
/// the union is not a manifold solid: two of its parts meet only along an edge or at a vertex (two
/// boxes touching along an edge, or faces merged into such a contact by the 1e-7 snapping).
pub fn box_union(boxes: &[Aabb]) -> (f64, Mesh) {
    match union_cells(boxes) {
        Some((volume, mesh, false)) => (volume, mesh),
        _ => (0.0, Mesh::default()),
    }
}

/// [`box_union`] before it rejects a pinched surface: `None` when there is nothing to mesh or the
/// grid is too large, else the volume, the mesh and whether the surface pinches (is not a
/// 2-manifold at some vertex or edge).
fn union_cells(boxes: &[Aabb]) -> Option<(f64, Mesh, bool)> {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    let boxes: Vec<&Aabb> = boxes
        .iter()
        .filter(|b| (0..3).all(|a| b.lo[a].is_finite() && b.hi[a].is_finite() && b.hi[a] - b.lo[a] > SNAP))
        .collect();
    if boxes.is_empty() {
        return None;
    }
    let coords: [Vec<f64>; 3] =
        std::array::from_fn(|a| merge_levels(boxes.iter().flat_map(|b| [b.lo[a], b.hi[a]]).collect()));
    // Cells per axis (every kept box has at least two distinct coordinates on each axis).
    let n: [usize; 3] = std::array::from_fn(|a| coords[a].len() - 1);
    let cells = n[0].checked_mul(n[1]).and_then(|c| c.checked_mul(n[2])).filter(|&c| c <= MAX_CELLS)?;
    let at = |i: usize, j: usize, k: usize| (i * n[1] + j) * n[2] + k;
    // The grid coordinate a bound falls on: the last kept coordinate that is ≤ it. This relies on
    // `merge_levels` keeping the first (smallest) value of each run of close values, so that is
    // the run the bound belongs to; do not change it to keep a mean or the last value.
    let index = |a: usize, v: f64| coords[a].partition_point(|&c| c <= v) - 1;

    let mut occupied = vec![false; cells];
    for b in &boxes {
        let range: [(usize, usize); 3] = std::array::from_fn(|a| (index(a, b.lo[a]), index(a, b.hi[a])));
        for i in range[0].0..range[0].1 {
            for j in range[1].0..range[1].1 {
                occupied[at(i, j, range[2].0)..at(i, j, range[2].1)].fill(true);
            }
        }
    }

    let mut volume = 0.0;
    for i in 0..n[0] {
        for j in 0..n[1] {
            let area = (coords[0][i + 1] - coords[0][i]) * (coords[1][j + 1] - coords[1][j]);
            for k in 0..n[2] {
                if occupied[at(i, j, k)] {
                    volume += area * (coords[2][k + 1] - coords[2][k]);
                }
            }
        }
    }

    // The eight cells around a grid vertex, as bits `dx | dy << 1 | dz << 2` (`d` = 0 for the cell
    // before the vertex along an axis, 1 for the one after); cells outside the grid are empty.
    let around = |g: [usize; 3]| -> u8 {
        let side: [[Option<usize>; 2]; 3] =
            std::array::from_fn(|a| [g[a].checked_sub(1), (g[a] < n[a]).then_some(g[a])]);
        let mut mask = 0;
        for c in 0..8usize {
            if let (Some(i), Some(j), Some(k)) = (side[0][c & 1], side[1][c >> 1 & 1], side[2][c >> 2 & 1]) {
                mask |= (occupied[at(i, j, k)] as u8) << c;
            }
        }
        mask
    };
    static MANIFOLD_VERTEX: OnceLock<[bool; 256]> = OnceLock::new();
    let manifold_vertex = MANIFOLD_VERTEX.get_or_init(|| std::array::from_fn(|mask| is_manifold_vertex(mask as u8)));

    // Faces: on each plane `p` of axis `a`, every cell of the plane whose two neighbours (outside
    // the grid counts as empty) differ. With (b, c) the next two axes cyclically, the grid corners
    // (0,0) (1,0) (1,1) (0,1) in (b, c) run counter-clockwise seen from +a, so that order is the
    // face of the occupied cell below the plane and the reverse the face of the one above it.
    let mut mesh = Mesh::default();
    let mut pinched = false;
    let mut vertex_ids: HashMap<[u32; 3], u32> = HashMap::new();
    for a in 0..3 {
        let (b, c) = ((a + 1) % 3, (a + 2) % 3);
        for p in 0..=n[a] {
            for jb in 0..n[b] {
                for jc in 0..n[c] {
                    let cell = |pa: usize| {
                        let mut g = [0; 3];
                        (g[a], g[b], g[c]) = (pa, jb, jc);
                        occupied[at(g[0], g[1], g[2])]
                    };
                    let (below, above) = (p > 0 && cell(p - 1), p < n[a] && cell(p));
                    if below == above {
                        continue;
                    }
                    let mut quad = [0u32; 4];
                    for (slot, (db, dc)) in quad.iter_mut().zip([(0, 0), (1, 0), (1, 1), (0, 1)]) {
                        // The vertex key is the grid index per axis (x, y, z), whatever `a` is.
                        let mut g = [0usize; 3];
                        (g[a], g[b], g[c]) = (p, jb + db, jc + dc);
                        *slot = *vertex_ids.entry(g.map(|i| i as u32)).or_insert_with(|| {
                            pinched |= !manifold_vertex[around(g) as usize];
                            mesh.positions.push(Point3::new(coords[0][g[0]], coords[1][g[1]], coords[2][g[2]]));
                            (mesh.positions.len() - 1) as u32
                        });
                    }
                    if !below {
                        quad.reverse();
                    }
                    mesh.triangles.push([quad[0], quad[1], quad[2]]);
                    mesh.triangles.push([quad[0], quad[2], quad[3]]);
                }
            }
        }
    }
    Some((volume, mesh, pinched))
}

/// Whether the boundary of the occupied cells is a 2-manifold at a grid vertex whose eight
/// surrounding cells are occupied as `mask` says (bit `dx | dy << 1 | dz << 2`). On a small sphere
/// around the vertex the eight cells are triangles, and the surface is the curve between occupied
/// and empty ones: it is one simple closed curve (or absent) exactly when no edge at the vertex
/// has two diagonal occupied cells and two empty ones around it (the curve would cross itself) and
/// the occupied cells form one group joined through shared faces, as do the empty ones.
fn is_manifold_vertex(mask: u8) -> bool {
    for axis in 0..3 {
        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
        for side in 0..2 {
            // The four cells around the edge leaving the vertex along (or against) `axis`.
            let cell = |db: usize, dc: usize| mask >> (side << axis | db << b | dc << c) & 1 == 1;
            let (q00, q10, q01, q11) = (cell(0, 0), cell(1, 0), cell(0, 1), cell(1, 1));
            if (q00 && q11 && !q10 && !q01) || (q10 && q01 && !q00 && !q11) {
                return false;
            }
        }
    }
    // Cells reachable from the first of `set` through cells that differ in one bit (share a face).
    let one_group = |set: u8| {
        let mut seen = set & set.wrapping_neg();
        loop {
            let mut next = seen;
            for c in 0..8 {
                if seen >> c & 1 == 1 {
                    next |= 1 << (c ^ 1) | 1 << (c ^ 2) | 1 << (c ^ 4);
                }
            }
            next &= set;
            if next == seen {
                return seen == set;
            }
            seen = next;
        }
    };
    one_group(mask) && one_group(!mask)
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

    fn aabb(x: [f64; 2], y: [f64; 2], z: [f64; 2]) -> Aabb {
        Aabb { lo: [x[0], y[0], z[0]], hi: [x[1], y[1], z[1]] }
    }

    /// Closed, outward (positive signed volume equal to the reported one), a 2-manifold.
    fn assert_solid(volume: f64, mesh: &Mesh) {
        assert!(mesh.is_closed());
        assert!(mesh.to_solid().is_closed_manifold());
        assert!((mesh.volume() - volume).abs() <= 1e-12 * volume.max(1.0), "{} vs {volume}", mesh.volume());
        assert!(volume > 0.0);
    }

    #[test]
    fn box_union_of_one_box_is_that_box() {
        let (v, m) = box_union(&[aabb([-1.0, 1.0], [0.0, 3.0], [2.0, 6.0])]);
        assert!((v - 24.0).abs() < 1e-12);
        assert_solid(v, &m);
        assert_eq!((m.positions.len(), m.triangles.len()), (8, 12));
        let (lo, hi) = m.bounds().unwrap();
        assert_eq!((lo, hi), (Point3::new(-1.0, 0.0, 2.0), Point3::new(1.0, 3.0, 6.0)));
    }

    #[test]
    fn box_union_merges_overlapping_touching_and_nearly_touching_boxes() {
        let unit = [0.0, 1.0];
        // Overlapping: volume of the union, not the sum.
        let (v, m) = box_union(&[aabb([0.0, 2.0], unit, unit), aabb([1.0, 3.0], unit, unit)]);
        assert!((v - 3.0).abs() < 1e-12);
        assert_solid(v, &m);
        // Touching along a face: one solid with no interior face (10 quads, not 12).
        let (v, m) = box_union(&[aabb(unit, unit, unit), aabb([1.0, 2.0], unit, unit)]);
        assert!((v - 2.0).abs() < 1e-12);
        assert_solid(v, &m);
        assert_eq!(m.triangles.len(), 20);
        // Faces closer than SNAP are one plane, at the smallest coordinate.
        let (v, m) = box_union(&[aabb(unit, unit, unit), aabb([1.0 + 5e-8, 2.0], unit, unit)]);
        assert!((v - 2.0).abs() < 1e-12);
        assert_solid(v, &m);
        assert!(m.positions.iter().all(|p| [0.0, 1.0, 2.0].contains(&p.x)));
        // One box inside another adds nothing (its grid lines still subdivide the outer faces).
        let (v, m) = box_union(&[aabb([0.0, 4.0], [0.0, 4.0], [0.0, 4.0]), aabb([1.0, 2.0], [1.0, 2.0], [1.0, 2.0])]);
        assert!((v - 64.0).abs() < 1e-12);
        assert_solid(v, &m);
        assert_eq!(m.bounds(), Some((Point3::new(0.0, 0.0, 0.0), Point3::new(4.0, 4.0, 4.0))));
    }

    #[test]
    fn box_union_of_a_cross_a_frame_and_a_staircase_is_a_manifold() {
        let u = [0.0, 1.0];
        // A plus sign: 3 + 3 - 1.
        let (v, m) = box_union(&[aabb([-1.0, 2.0], u, u), aabb(u, [-1.0, 2.0], u)]);
        assert!((v - 5.0).abs() < 1e-12);
        assert_solid(v, &m);
        // A square ring around a hole (genus 1): 9 - 1.
        let (v, m) = box_union(&[
            aabb([0.0, 3.0], [0.0, 1.0], u),
            aabb([0.0, 3.0], [2.0, 3.0], u),
            aabb([0.0, 1.0], [0.0, 3.0], u),
            aabb([2.0, 3.0], [0.0, 3.0], u),
        ]);
        assert!((v - 8.0).abs() < 1e-12);
        assert_solid(v, &m);
        // Blocks of growing height: n(n + 1)/2 cells of a staircase.
        let blocks: Vec<Aabb> = (1..=6).map(|k| aabb([(k - 1) as f64, k as f64], u, [0.0, k as f64])).collect();
        let (v, m) = box_union(&blocks);
        assert!((v - 21.0).abs() < 1e-12);
        assert_solid(v, &m);
        // Every triangle faces away from the solid: moving a little along its normal leaves it.
        for t in &m.triangles {
            let [a, b, c] = t.map(|i| m.positions[i as usize]);
            let nrm = [
                (b.y - a.y) * (c.z - a.z) - (b.z - a.z) * (c.y - a.y),
                (b.z - a.z) * (c.x - a.x) - (b.x - a.x) * (c.z - a.z),
                (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x),
            ];
            let len = nrm.iter().map(|x| x * x).sum::<f64>().sqrt();
            let q = [
                (a.x + b.x + c.x) / 3.0 + 1e-3 * nrm[0] / len,
                (a.y + b.y + c.y) / 3.0 + 1e-3 * nrm[1] / len,
                (a.z + b.z + c.z) / 3.0 + 1e-3 * nrm[2] / len,
            ];
            let inside = blocks.iter().any(|b| (0..3).all(|k| b.lo[k] < q[k] && q[k] < b.hi[k]));
            assert!(!inside, "triangle {t:?} faces into the solid");
        }
    }

    #[test]
    fn box_union_gives_nothing_for_a_surface_that_pinches() {
        let u = [0.0, 1.0];
        let empty = (0.0, Mesh::default());
        // Two boxes sharing only an edge, or only a corner.
        assert_eq!(box_union(&[aabb(u, u, u), aabb([1.0, 2.0], [1.0, 2.0], u)]), empty);
        assert_eq!(box_union(&[aabb(u, u, u), aabb([1.0, 2.0], [1.0, 2.0], [1.0, 2.0])]), empty);
        // Faces within 1e-7 are merged first: this is an edge contact too.
        assert_eq!(box_union(&[aabb(u, u, u), aabb([1.0 + 5e-8, 2.0], [1.0 + 5e-8, 2.0], u)]), empty);
        // The pieces themselves are fine: the cells are all there, and the contact is the only fault.
        let (volume, mesh, pinched) = union_cells(&[aabb(u, u, u), aabb([1.0, 2.0], [1.0, 2.0], u)]).unwrap();
        assert!((volume - 2.0).abs() < 1e-12 && (mesh.volume() - 2.0).abs() < 1e-12);
        assert!(pinched && !mesh.is_closed(), "the shared edge is used four times");
        // Touching along a face, even with the contact shifted, is not a pinch.
        let (v, m) = box_union(&[aabb([0.0, 2.0], u, u), aabb([2.0, 3.0], [0.5, 1.5], [0.5, 1.5])]);
        assert!((v - 3.0).abs() < 1e-12);
        assert_solid(v, &m);
        // Two separate boxes are two solids in one mesh.
        let (v, m) = box_union(&[aabb(u, u, u), aabb([3.0, 4.0], u, u)]);
        assert!((v - 2.0).abs() < 1e-12);
        assert_solid(v, &m);
    }

    /// `union_cells` over `cells` (unit cells of a lattice): the pinch verdict, which must be exactly
    /// "the mesh is not a closed 2-manifold", and the volume the number of cells.
    fn pinch_verdict(cells: &[[usize; 3]]) -> bool {
        let boxes: Vec<Aabb> = cells
            .iter()
            .map(|&c| {
                let lo = c.map(|i| i as f64);
                Aabb { lo, hi: lo.map(|x| x + 1.0) }
            })
            .collect();
        let (volume, mesh, pinched) = union_cells(&boxes).unwrap();
        assert!((volume - cells.len() as f64).abs() < 1e-12, "{cells:?}");
        assert!((mesh.volume() - volume).abs() < 1e-9, "{cells:?}");
        let manifold = mesh.is_closed() && mesh.to_solid().is_closed_manifold();
        assert_eq!(pinched, !manifold, "{cells:?}");
        assert_eq!(box_union(&boxes) == (volume, mesh), !pinched, "{cells:?}");
        pinched
    }

    #[test]
    fn the_pinch_test_is_exact_for_every_arrangement_of_a_2x2x2_block() {
        let mut pinched = 0;
        for mask in 1u32..256 {
            let cells: Vec<[usize; 3]> =
                (0..8).filter(|c| mask >> c & 1 == 1).map(|c| [c & 1, c >> 1 & 1, c >> 2 & 1]).collect();
            pinched += pinch_verdict(&cells) as u32;
        }
        // Not everything is fine (diagonals, rings) and not everything is a pinch.
        assert!(pinched > 50 && pinched < 200, "{pinched}");
    }

    #[test]
    fn the_pinch_test_is_exact_for_random_voxel_sets() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        let (mut pinched, mut fine) = (0, 0);
        for _ in 0..1500 {
            // A few random boxes of a 4 × 4 × 4 lattice, and sometimes loose cells besides.
            let mut cells = std::collections::BTreeSet::new();
            for _ in 0..1 + next(4) {
                let mut r = [[0usize; 2]; 3];
                for axis in &mut r {
                    let (a, b) = (next(4) as usize, next(4) as usize);
                    *axis = [a, a + 1 + b.min(3 - a)];
                }
                for i in r[0][0]..r[0][1] {
                    for j in r[1][0]..r[1][1] {
                        for k in r[2][0]..r[2][1] {
                            cells.insert([i, j, k]);
                        }
                    }
                }
            }
            for _ in 0..if next(10) < 4 { 1 + next(3) } else { 0 } {
                cells.insert([next(4) as usize, next(4) as usize, next(4) as usize]);
            }
            if pinch_verdict(&cells.into_iter().collect::<Vec<_>>()) {
                pinched += 1;
            } else {
                fine += 1;
            }
        }
        assert!(pinched > 100 && fine > 100, "{pinched} pinched, {fine} fine");
    }

    #[test]
    fn box_union_ignores_unusable_boxes_and_oversized_grids() {
        let u = [0.0, 1.0];
        let good = aabb(u, u, u);
        let bad = [
            aabb([0.0, 0.0], u, u),
            aabb([1.0, 0.0], u, u),
            aabb(u, u, [0.0, 1e-7]),
            aabb([0.0, f64::NAN], u, u),
            aabb(u, [0.0, f64::INFINITY], u),
        ];
        assert_eq!(box_union(&bad), (0.0, Mesh::default()));
        assert_eq!(box_union(&[]), (0.0, Mesh::default()));
        let mut all = bad.to_vec();
        all.push(good);
        assert_eq!(box_union(&all), box_union(&[good]));
        // 511³ cells need more than the grid limit: no mesh, and no time or memory spent on one.
        let diagonal: Vec<Aabb> = (0..256)
            .map(|i| {
                let r = [2.0 * i as f64, 2.0 * i as f64 + 1.0];
                aabb(r, r, r)
            })
            .collect();
        assert_eq!(box_union(&diagonal), (0.0, Mesh::default()));
    }
}
