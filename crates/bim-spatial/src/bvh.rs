//! Bounding volume hierarchy over triangles, built top-down with a binned surface area
//! heuristic and stored as a flat, depth-first node array.

use crate::aabb::Aabb;

/// A triangle as three vertices.
pub type Triangle = [[f64; 3]; 3];

/// Number of SAH bins per axis.
const BINS: usize = 16;
/// Largest leaf the SAH may choose to keep (larger ranges are always split).
const MAX_LEAF: usize = 4;
/// Leaf size used when every centroid coincides and the SAH cannot separate primitives.
const MAX_DEGENERATE_LEAF: usize = 8;
/// Cost of one traversal step relative to one triangle intersection.
const TRAVERSAL_COST: f64 = 1.0;
/// Determinants below this magnitude are treated as "ray parallel to the triangle"
/// (also keeps `1 / det` finite).
const DET_EPS: f64 = 1e-300;

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    /// Leaf: index of the first triangle; inner: index of the right child (the left
    /// child is always the next node).
    offset: u32,
    /// Number of triangles in a leaf; 0 for inner nodes.
    count: u32,
}

/// A SAH bounding volume hierarchy over a triangle soup. Triangle indices reported by
/// queries refer to the slice passed to [`Bvh::build`].
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    /// Triangles in leaf order.
    tris: Vec<Triangle>,
    /// `ids[i]` is the caller's index of `tris[i]`.
    ids: Vec<u32>,
}

#[inline]
fn tri_bounds(t: &Triangle) -> Aabb {
    let mut b = Aabb { min: t[0], max: t[0] };
    b.add_point(t[1]);
    b.add_point(t[2]);
    b
}

#[derive(Clone, Copy)]
struct Prim {
    bounds: Aabb,
    id: u32,
}

impl Prim {
    /// Twice the centroid (min + max); the factor 2 cancels in the binning.
    #[inline(always)]
    fn c2(&self, axis: usize) -> f64 {
        self.bounds.min[axis] + self.bounds.max[axis]
    }
}

#[derive(Clone, Copy)]
struct Bin {
    bounds: Aabb,
    count: usize,
}

impl Bin {
    const EMPTY: Bin = Bin { bounds: Aabb::EMPTY, count: 0 };
}

#[inline]
fn bin_of(c: f64, lo: f64, scale: f64, nb: usize) -> usize {
    let b = ((c - lo) * scale) as usize; // saturating cast: negatives/NaN -> 0
    b.min(nb - 1)
}

impl Bvh {
    /// Builds the hierarchy over `tris` with a binned SAH (16 bins on each axis, 8 for
    /// ranges of at most 32 triangles; leaves of at most 4 triangles unless all
    /// centroids coincide).
    pub fn build(tris: &[Triangle]) -> Bvh {
        let n = tris.len();
        if n == 0 {
            return Bvh::default();
        }
        assert!(n < u32::MAX as usize, "too many triangles for the BVH");
        // Primitive records are partitioned in place (rather than an index array into
        // separate arrays) so every pass over a node's range reads memory sequentially.
        let mut prims: Vec<Prim> = tris
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let b = tri_bounds(t);
                Prim { bounds: b, id: i as u32 }
            })
            .collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(2 * n / MAX_LEAF.max(2) + 1);

        // Work stack of (start, end, parent whose right-child link must point here).
        let mut work: Vec<(usize, usize, Option<usize>)> = vec![(0, n, None)];
        let mut bins = [[Bin::EMPTY; BINS]; 3];
        let mut right_area = [0.0f64; BINS];
        let mut right_count = [0usize; BINS];
        while let Some((s, e, parent)) = work.pop() {
            let me = nodes.len();
            if let Some(p) = parent {
                nodes[p].offset = me as u32;
            }
            let mut bounds = Aabb::EMPTY;
            let mut cb = Aabb::EMPTY;
            for p in &prims[s..e] {
                bounds = bounds.union(&p.bounds);
                cb.add_point([p.c2(0), p.c2(1), p.c2(2)]);
            }
            let count = e - s;
            let leaf = Node { bounds, offset: s as u32, count: count as u32 };
            if count <= 1 {
                nodes.push(leaf);
                continue;
            }

            // Binned SAH over all three axes, binned in a single pass over the range.
            // Small ranges use fewer bins: the per-node constant cost dominates there.
            let nb = if count <= 2 * BINS { BINS / 2 } else { BINS };
            let mut scale = [0.0f64; 3];
            for axis in 0..3 {
                let ext = cb.max[axis] - cb.min[axis];
                scale[axis] = if ext > 0.0 { nb as f64 / ext } else { 0.0 };
            }
            for axis_bins in bins.iter_mut() {
                axis_bins[..nb].fill(Bin::EMPTY);
            }
            for p in &prims[s..e] {
                for axis in 0..3 {
                    let b = bin_of(p.c2(axis), cb.min[axis], scale[axis], nb);
                    let bin = &mut bins[axis][b];
                    bin.count += 1;
                    bin.bounds = bin.bounds.union(&p.bounds);
                }
            }
            let mut best: Option<(f64, usize, usize)> = None; // (cost, axis, split bin)
            for axis in 0..3 {
                if scale[axis] == 0.0 {
                    continue; // all centroids share this coordinate
                }
                let bs = &bins[axis];
                // Suffix sweep: right side of a split before bin k is bins k..nb.
                let mut acc = Aabb::EMPTY;
                let mut cnt = 0;
                for k in (1..nb).rev() {
                    acc = acc.union(&bs[k].bounds);
                    cnt += bs[k].count;
                    right_area[k] = acc.surface_area();
                    right_count[k] = cnt;
                }
                let mut acc = Aabb::EMPTY;
                let mut cnt = 0;
                for k in 1..nb {
                    acc = acc.union(&bs[k - 1].bounds);
                    cnt += bs[k - 1].count;
                    if cnt == 0 || right_count[k] == 0 {
                        continue;
                    }
                    let cost = acc.surface_area() * cnt as f64 + right_area[k] * right_count[k] as f64;
                    if best.map_or(true, |(c, _, _)| cost < c) {
                        best = Some((cost, axis, k));
                    }
                }
            }

            let mid = match best {
                Some((cost, axis, k)) => {
                    let area = bounds.surface_area();
                    let split_cost = TRAVERSAL_COST * area + cost;
                    let leaf_cost = count as f64 * area;
                    if count <= MAX_LEAF && leaf_cost <= split_cost {
                        nodes.push(leaf);
                        continue;
                    }
                    let (lo, scale) = (cb.min[axis], scale[axis]);
                    // In-place partition with the same bin function used for counting,
                    // so both sides are non-empty.
                    let slice = &mut prims[s..e];
                    let mut l = 0;
                    let mut r = slice.len();
                    while l < r {
                        if bin_of(slice[l].c2(axis), lo, scale, nb) < k {
                            l += 1;
                        } else {
                            r -= 1;
                            slice.swap(l, r);
                        }
                    }
                    s + l
                }
                None => {
                    // All centroids coincide: the SAH has nothing to separate.
                    if count <= MAX_DEGENERATE_LEAF {
                        nodes.push(leaf);
                        continue;
                    }
                    s + count / 2
                }
            };
            debug_assert!(s < mid && mid < e);
            nodes.push(Node { bounds, offset: 0, count: 0 });
            work.push((mid, e, Some(me)));
            work.push((s, mid, None));
        }

        let ordered: Vec<Triangle> = prims.iter().map(|p| tris[p.id as usize]).collect();
        let ids: Vec<u32> = prims.iter().map(|p| p.id).collect();
        Bvh { nodes, tris: ordered, ids }
    }

    /// Number of triangles.
    #[inline]
    pub fn len(&self) -> usize {
        self.tris.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Bounds of all triangles (empty box for an empty hierarchy).
    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| n.bounds)
    }

    /// Closest hit of the ray `origin + t * dir`, `t > 0`, against the triangles (both
    /// sides). Returns the caller's triangle index and `t` in units of `dir` (which need
    /// not be normalized). Hits exactly on an edge or vertex count.
    #[inline]
    pub fn raycast(&self, origin: [f64; 3], dir: [f64; 3]) -> Option<(usize, f64)> {
        if self.nodes.is_empty() || dir == [0.0; 3] {
            return None;
        }
        // Box culling uses the true inverse direction. Axes whose component is zero
        // (either sign) or whose inverse overflows (subnormal component) cannot use the
        // multiply-by-inverse slab test exactly (0 * inf = NaN, or a lost sign), so rays
        // with such an axis take the general slab test, which handles those axes exactly;
        // all other rays take the fast path. The choice is made once per ray.
        let inv = [1.0 / dir[0], 1.0 / dir[1], 1.0 / dir[2]];
        let ray = Ray { o: origin, d: dir, inv };
        if inv.iter().all(|v| v.is_finite()) {
            self.raycast_with::<false>(&ray)
        } else {
            self.raycast_with::<true>(&ray)
        }
    }

    #[inline(always)]
    fn raycast_with<const GENERAL: bool>(&self, ray: &Ray) -> Option<(usize, f64)> {
        let (origin, dir) = (ray.o, ray.d);
        let mut best_t = f64::INFINITY;
        let mut best_i = usize::MAX;

        let root_t = slab::<GENERAL>(&self.nodes[0].bounds, ray, best_t);
        if root_t.is_infinite() {
            return None;
        }
        let mut stack: Vec<(u32, f64)> = Vec::with_capacity(64);
        stack.push((0, root_t));
        while let Some((ni, entry)) = stack.pop() {
            // `entry` may be overestimated by a few ulps; prune only beyond that margin so
            // a hit marginally closer than `best_t` is never skipped.
            if entry > widen(best_t) {
                continue;
            }
            let node = &self.nodes[ni as usize];
            if node.count > 0 {
                let start = node.offset as usize;
                for j in start..start + node.count as usize {
                    if let Some(t) = intersect_triangle(origin, dir, &self.tris[j]) {
                        if t < best_t {
                            best_t = t;
                            best_i = j;
                        }
                    }
                }
            } else {
                let l = ni + 1;
                let r = node.offset;
                let tl = slab::<GENERAL>(&self.nodes[l as usize].bounds, ray, best_t);
                let tr = slab::<GENERAL>(&self.nodes[r as usize].bounds, ray, best_t);
                // Push the farther child first so the nearer one is visited next.
                let (near, tn, far, tf) = if tl <= tr { (l, tl, r, tr) } else { (r, tr, l, tl) };
                if tf.is_finite() {
                    stack.push((far, tf));
                }
                if tn.is_finite() {
                    stack.push((near, tn));
                }
            }
        }
        if best_i == usize::MAX {
            None
        } else {
            Some((self.ids[best_i] as usize, best_t))
        }
    }

    /// Every pair `(i, j)` (index into this BVH's triangles, index into `other`'s) whose
    /// triangle bounding boxes intersect (closed: touching counts), each pair once, in
    /// no particular order.
    pub fn overlaps(&self, other: &Bvh) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if self.nodes.is_empty() || other.nodes.is_empty() {
            return out;
        }
        let mut stack: Vec<(u32, u32)> = vec![(0, 0)];
        let mut abox: Vec<Aabb> = Vec::new();
        while let Some((a, b)) = stack.pop() {
            let na = &self.nodes[a as usize];
            let nb = &other.nodes[b as usize];
            if !na.bounds.intersects(&nb.bounds) {
                continue;
            }
            match (na.count > 0, nb.count > 0) {
                (true, true) => {
                    let (sa, sb) = (na.offset as usize, nb.offset as usize);
                    abox.clear();
                    abox.extend(self.tris[sa..sa + na.count as usize].iter().map(tri_bounds));
                    for j in sb..sb + nb.count as usize {
                        let bj = tri_bounds(&other.tris[j]);
                        if !bj.intersects(&na.bounds) {
                            continue;
                        }
                        for (ii, bi) in abox.iter().enumerate() {
                            if bi.intersects(&bj) {
                                out.push((self.ids[sa + ii] as usize, other.ids[j] as usize));
                            }
                        }
                    }
                }
                (true, false) => {
                    stack.push((a, b + 1));
                    stack.push((a, nb.offset));
                }
                (false, true) => {
                    stack.push((a + 1, b));
                    stack.push((na.offset, b));
                }
                (false, false) => {
                    if na.bounds.surface_area() >= nb.bounds.surface_area() {
                        stack.push((a + 1, b));
                        stack.push((na.offset, b));
                    } else {
                        stack.push((a, b + 1));
                        stack.push((a, nb.offset));
                    }
                }
            }
        }
        out
    }
}

/// A ray prepared for box tests: origin, direction and `1 / dir` per axis.
struct Ray {
    o: [f64; 3],
    d: [f64; 3],
    inv: [f64; 3],
}

/// Relative widening applied to slab exit distances and to the pruning bound. Each slab
/// distance `(b - o) * (1 / d)` carries at most three roundings (about 1.5 ulp), so the
/// computed entry may exceed and the computed exit may fall short of the true values by
/// that much; 8 ulps covers both with margin.
const SLAB_WIDEN: f64 = 8.0 * f64::EPSILON;

/// `t` enlarged by the slab rounding margin, plus an absolute term for distances that
/// underflow into the subnormal range (whose rounding error is absolute, not relative).
#[inline(always)]
fn widen(t: f64) -> f64 {
    t + t.abs() * SLAB_WIDEN + f64::MIN_POSITIVE
}

/// Entry distance of the ray into `b` clipped to `[0, limit]`, or `+inf` on a miss.
/// Closed: flat boxes, grazing contacts and origins on a face are accepted. The exit
/// distance is widened (see [`widen`]) so rounding cannot reject a box the ray touches.
///
/// With `GENERAL == false` every component of `ray.d` must have a finite inverse. With
/// `GENERAL == true` an axis whose component is `±0` is treated as parallel: the box is
/// rejected when the origin lies outside the closed slab, and otherwise that axis does
/// not constrain `t`. An axis with a nonzero component whose inverse overflows uses an
/// exact division instead of the inverse.
#[inline(always)]
fn slab<const GENERAL: bool>(b: &Aabb, ray: &Ray, limit: f64) -> f64 {
    let mut tmin = 0.0f64;
    let mut tmax = limit;
    for k in 0..3 {
        let o = ray.o[k];
        let (t1, t2) = if GENERAL && !ray.inv[k].is_finite() {
            if ray.d[k] == 0.0 {
                if !(o >= b.min[k] && o <= b.max[k]) {
                    return f64::INFINITY;
                }
                continue;
            }
            ((b.min[k] - o) / ray.d[k], (b.max[k] - o) / ray.d[k])
        } else {
            ((b.min[k] - o) * ray.inv[k], (b.max[k] - o) * ray.inv[k])
        };
        let (lo, hi) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
        if lo > tmin {
            tmin = lo;
        }
        if hi < tmax {
            tmax = hi;
        }
    }
    if tmin <= widen(tmax) {
        tmin
    } else {
        f64::INFINITY
    }
}

#[inline(always)]
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline(always)]
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[inline(always)]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Möller–Trumbore, two-sided. Returns `t > 0` (in units of `d`) when the ray hits the
/// closed triangle (barycentric coordinates `u, v, 1 - u - v` all `>= 0`).
#[inline(always)]
fn intersect_triangle(o: [f64; 3], d: [f64; 3], tri: &Triangle) -> Option<f64> {
    let e1 = sub(tri[1], tri[0]);
    let e2 = sub(tri[2], tri[0]);
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < DET_EPS {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, tri[0]);
    let u = dot(s, p) * inv;
    if !(u >= 0.0) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if !(v >= 0.0) || !(1.0 - u - v >= 0.0) {
        return None;
    }
    let t = dot(e2, q) * inv;
    if t > 0.0 {
        Some(t)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    #[test]
    fn raycast_matches_brute_force() {
        let mut s = 7u64;
        let tris: Vec<Triangle> = (0..3000)
            .map(|_| {
                let c = [lcg(&mut s) * 20.0, lcg(&mut s) * 20.0, lcg(&mut s) * 20.0];
                let mut v = || [c[0] + lcg(&mut s) - 0.5, c[1] + lcg(&mut s) - 0.5, c[2] + lcg(&mut s) - 0.5];
                [v(), v(), v()]
            })
            .collect();
        let bvh = Bvh::build(&tris);
        for n in 0..500 {
            let o = [lcg(&mut s) * 20.0, lcg(&mut s) * 20.0, lcg(&mut s) * 20.0];
            let mut d = [lcg(&mut s) - 0.5, lcg(&mut s) - 0.5, lcg(&mut s) - 0.5];
            if n % 3 == 0 {
                d = [0.0, 0.0, 0.0];
                d[n % 3] = 1.0;
                d[(n / 3) % 3] = -1.0;
            }
            let brute = tris
                .iter()
                .enumerate()
                .filter_map(|(i, t)| intersect_triangle(o, d, t).map(|t| (i, t)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            assert_eq!(bvh.raycast(o, d), brute);
        }
    }

    #[test]
    fn shared_edge_flat_boxes_and_empty() {
        let tris: Vec<Triangle> = vec![
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            [[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        ];
        let bvh = Bvh::build(&tris);
        let (i, t) = bvh.raycast([0.5, 0.5, 2.0], [0.0, 0.0, -4.0]).unwrap();
        assert!(i <= 1 && t == 0.5);
        // Origin on the box's x-plane with a zero x component.
        assert!(bvh.raycast([0.0, 0.5, 1.0], [0.0, 0.0, -1.0]).is_some());
        assert!(bvh.raycast([0.5, 0.5, 2.0], [0.0, 0.0, 1.0]).is_none());
        assert!(bvh.raycast([0.5, 0.5, 2.0], [0.0; 3]).is_none());
        let empty = Bvh::build(&[]);
        assert!(empty.is_empty() && empty.raycast([0.0; 3], [1.0, 0.0, 0.0]).is_none());
        assert!(bvh.overlaps(&empty).is_empty());
    }

    fn brute(tris: &[Triangle], o: [f64; 3], d: [f64; 3]) -> Option<(usize, f64)> {
        tris.iter()
            .enumerate()
            .filter_map(|(i, t)| intersect_triangle(o, d, t).map(|t| (i, t)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Same hit/miss decision as brute force and the same closest `t`; on ties the BVH may
    /// report a different triangle, but it must really be hit at the reported `t`.
    fn assert_matches_brute(tris: &[Triangle], bvh: &Bvh, o: [f64; 3], d: [f64; 3]) {
        let got = bvh.raycast(o, d);
        let want = brute(tris, o, d);
        match (got, want) {
            (None, None) => {}
            (Some((i, t)), Some((_, tb))) => {
                let tol = 1e-9 * t.abs().max(1.0);
                assert!((t - tb).abs() <= tol, "ray {o:?} {d:?}: got t={t} (tri {i}), brute force t={tb}");
                let ti = intersect_triangle(o, d, &tris[i]).expect("reported triangle is not hit");
                assert!((ti - t).abs() <= tol, "ray {o:?} {d:?}: triangle {i} is hit at {ti}, reported {t}");
            }
            _ => panic!("ray {o:?} {d:?}: got {got:?}, brute force {want:?}"),
        }
    }

    #[test]
    fn origin_on_max_face_with_zero_component() {
        let tri: Triangle = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        let bvh = Bvh::build(&[tri]);
        // Origin on the box's max-x face, x component zero: hits vertex (2, 0, 0).
        assert_eq!(bvh.raycast([2.0, 0.0, 1.0], [0.0, 0.0, -1.0]), Some((0, 1.0)));
        // Origin on the max-y face: hits vertex (0, 2, 0).
        assert_eq!(bvh.raycast([0.0, 2.0, 1.0], [0.0, 0.0, -1.0]), Some((0, 1.0)));
        // -0.0 component with the origin on the min-x face.
        assert_eq!(bvh.raycast([0.0, 0.5, 1.0], [-0.0, 0.0, -1.0]), Some((0, 1.0)));
        assert_eq!(bvh.raycast([0.0, 0.5, 1.0], [0.0, -0.0, -1.0]), Some((0, 1.0)));
        assert_eq!(bvh.raycast([2.0, 0.0, 1.0], [-0.0, -0.0, -1.0]), Some((0, 1.0)));
        assert_eq!(bvh.raycast([1.0, 1.0, 5.0], [0.0, 0.0, -1.0]), Some((0, 5.0)));
        // Just outside the closed slab of a zero-component axis: miss.
        assert_eq!(bvh.raycast([2.0 + 1e-15, 0.0, 1.0], [0.0, 0.0, -1.0]), None);
        assert_eq!(bvh.raycast([-1e-300, 0.5, 1.0], [-0.0, 0.0, -1.0]), None);
    }

    #[test]
    fn stacked_triangles_closest_first() {
        let tris: Vec<Triangle> = vec![
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [[0.0, 0.0, -5.0], [3.0, 0.0, -5.0], [0.0, 3.0, -5.0]],
        ];
        let bvh = Bvh::build(&tris);
        assert_eq!(bvh.raycast([1.0, 0.0, 1.0], [0.0, 0.0, -1.0]), Some((0, 1.0)));
        assert_eq!(bvh.raycast([0.0, 1.0, 1.0], [-0.0, -0.0, -1.0]), Some((0, 1.0)));
        assert_eq!(bvh.raycast([2.0, 0.0, 1.0], [0.0, 0.0, -1.0]), Some((1, 6.0)));
    }

    #[test]
    fn thin_box_with_zero_or_tiny_component() {
        // y-extent of +-1e-300; the ray stays (nearly) parallel to y and crosses the
        // triangle well beyond the distance the slab planes would give with a finite
        // stand-in for 1/0.
        let tri: Triangle = [[0.0, -1e-300, 0.0], [2.0, 1e-300, 0.0], [0.0, 0.0, 2.0]];
        let tris = [tri];
        let bvh = Bvh::build(&tris);
        for d in [
            [0.0, 0.0, -1.0],
            [0.0, -0.0, -1.0],
            [0.0, 5e-302, -1.0],
            [0.0, -5e-302, -1.0],
            [0.0, 1e-310, -1.0],
            [-0.0, -1e-310, -1.0],
        ] {
            let o = [0.5, 0.0, 5.0];
            let want = brute(&tris, o, d);
            assert!(want.is_some(), "brute force must hit for {d:?}");
            assert_matches_brute(&tris, &bvh, o, d);
        }
        let (_, t) = bvh.raycast([0.5, 0.0, 5.0], [0.0, 0.0, -1.0]).unwrap();
        assert!((t - 4.0).abs() < 1e-12, "t = {t}");
    }

    #[test]
    fn subnormal_component_outside_slab() {
        // Overflowing inverse: the slab distance is computed by division, so a ray that
        // enters the box only through its tiny component is still found.
        // The origin lies just outside the x slab [0, 4]; the subnormal x component carries
        // the ray into it at t = 2e-13, well before the hit at t = 1.
        let tris: Vec<Triangle> = vec![[[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]]];
        let bvh = Bvh::build(&tris);
        let o = [-2e-323, 1.0, 1.0];
        let d: [f64; 3] = [1e-310, 0.0, -1.0];
        assert!((1.0 / d[0]).is_infinite());
        assert!(brute(&tris, o, d).is_some());
        assert_matches_brute(&tris, &bvh, o, d);
        // With a zero component the same origin is outside the closed slab: miss.
        assert_eq!(bvh.raycast(o, [0.0, 0.0, -1.0]), None);
    }

    #[test]
    fn integer_grid_axis_rays_match_brute_force() {
        let mut s = 0x5eed_u64;
        let cell = |s: &mut u64, n: f64| (lcg(s) * n).floor();
        for round in 0..4 {
            let span = [6.0, 10.0, 16.0, 8.0][round];
            let count = [700, 700, 700, 1500][round];
            let tris: Vec<Triangle> = (0..count)
                .map(|_| {
                    let c = [cell(&mut s, span), cell(&mut s, span), cell(&mut s, span)];
                    let v = |s: &mut u64| {
                        [c[0] + cell(s, 4.0) - 1.0, c[1] + cell(s, 4.0) - 1.0, c[2] + cell(s, 4.0) - 1.0]
                    };
                    [v(&mut s), v(&mut s), v(&mut s)]
                })
                .collect();
            let bvh = Bvh::build(&tris);
            for n in 0..3000 {
                let o = [cell(&mut s, span + 4.0) - 2.0, cell(&mut s, span + 4.0) - 2.0, cell(&mut s, span + 4.0) - 2.0];
                // Axis ray with signed zeros on the other two axes; every fourth ray has a
                // second nonzero integer component.
                let axis = n % 3;
                let mut d = [0.0f64; 3];
                for (k, dk) in d.iter_mut().enumerate() {
                    let neg = lcg(&mut s) < 0.5;
                    *dk = if k == axis {
                        let mag = [1.0, 2.0, 0.5, 3.0][(lcg(&mut s) * 4.0) as usize];
                        if neg { -mag } else { mag }
                    } else if n % 4 == 0 && k == (axis + 1) % 3 {
                        let mag = 1.0 + cell(&mut s, 2.0);
                        if neg { -mag } else { mag }
                    } else if neg {
                        -0.0
                    } else {
                        0.0
                    };
                }
                assert_matches_brute(&tris, &bvh, o, d);
            }
        }
    }

    #[test]
    fn overlaps_match_brute_force() {
        let mut s = 11u64;
        let mut gen = |n: usize| -> Vec<Triangle> {
            (0..n)
                .map(|_| {
                    let c = [(lcg(&mut s) * 20.0).floor(), (lcg(&mut s) * 20.0).floor(), (lcg(&mut s) * 5.0).floor()];
                    [c, [c[0] + 1.0, c[1], c[2]], [c[0], c[1] + 1.0, c[2]]]
                })
                .collect()
        };
        let a = gen(800);
        let b = gen(700);
        let mut got = Bvh::build(&a).overlaps(&Bvh::build(&b));
        got.sort();
        let mut want = Vec::new();
        for (i, ta) in a.iter().enumerate() {
            for (j, tb) in b.iter().enumerate() {
                if tri_bounds(ta).intersects(&tri_bounds(tb)) {
                    want.push((i, j));
                }
            }
        }
        assert_eq!(got, want);
    }
}
