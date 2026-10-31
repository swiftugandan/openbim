// External judge for T2.3 (bim-spatial). Brute force is the oracle: every query is
// compared with an exhaustive scan (or an independent uniform-grid broad phase) over
// the same random data. Timing budgets are checked in release builds.
use bim_spatial::{Aabb, Bvh, RStarTree};
use std::collections::{BTreeSet, HashMap};
use std::sync::Once;
use std::time::{Duration, Instant};

type Tri = [[f64; 3]; 3];

/// Abort the whole test binary if anything hangs.
fn watchdog() {
    static START: Once = Once::new();
    START.call_once(|| {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(150));
            eprintln!("JUDGE FAIL: T2.3 judge timed out after 150 s (hang or far too slow)");
            std::process::exit(1);
        });
    });
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// A multiple of `step` in [0, hi).
    fn grid(&mut self, hi: f64, step: f64) -> f64 {
        self.below((hi / step) as u64) as f64 * step
    }
}

fn bx(min: [f64; 3], max: [f64; 3]) -> Aabb {
    Aabb { min, max }
}

/// Closed boxes: touching faces, edges or corners intersect.
fn intersects(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|k| a.min[k] <= b.max[k] && b.min[k] <= a.max[k])
}

fn point_box_dist(p: [f64; 3], b: &Aabb) -> f64 {
    (0..3)
        .map(|k| {
            let d = (b.min[k] - p[k]).max(p[k] - b.max[k]).max(0.0);
            d * d
        })
        .sum::<f64>()
        .sqrt()
}

/// 100k boxes on a 0.5 grid (so exact touching is common), sizes 0..10 including degenerate boxes.
fn random_boxes(rng: &mut Rng, n: usize) -> Vec<Aabb> {
    (0..n)
        .map(|_| {
            let min = [rng.grid(1000.0, 0.5), rng.grid(1000.0, 0.5), rng.grid(1000.0, 0.5)];
            let s = [rng.grid(10.0, 0.5), rng.grid(10.0, 0.5), rng.grid(10.0, 0.5)];
            bx(min, [min[0] + s[0], min[1] + s[1], min[2] + s[2]])
        })
        .collect()
}

fn query_boxes(rng: &mut Rng, items: &[Aabb]) -> Vec<Aabb> {
    let mut qs = Vec::new();
    for _ in 0..300 {
        let min = [rng.range(-20.0, 1000.0), rng.range(-20.0, 1000.0), rng.range(-20.0, 1000.0)];
        let s = rng.range(0.0, 60.0);
        qs.push(bx(min, [min[0] + s, min[1] + s * 0.7, min[2] + s * 1.3]));
    }
    // Queries that exactly touch an item at a corner, or are a single point on its face.
    for _ in 0..100 {
        let b = items[rng.below(items.len() as u64) as usize];
        qs.push(bx(b.max, [b.max[0] + 3.0, b.max[1] + 2.5, b.max[2] + 1.0]));
        qs.push(bx([b.min[0], b.min[1], b.max[2]], [b.min[0], b.min[1], b.max[2]]));
    }
    // Far away: must return nothing.
    qs.push(bx([5000.0; 3], [5001.0; 3]));
    qs
}

fn brute_query(live: &[(Aabb, usize)], q: &Aabb) -> BTreeSet<usize> {
    live.iter().filter(|(b, _)| intersects(b, q)).map(|(_, i)| *i).collect()
}

fn check_queries(tree: &RStarTree<usize>, live: &[(Aabb, usize)], qs: &[Aabb], label: &str) {
    for (n, q) in qs.iter().enumerate() {
        let got_vec: Vec<usize> = tree.query_intersects(q).into_iter().copied().collect();
        let got: BTreeSet<usize> = got_vec.iter().copied().collect();
        assert_eq!(got.len(), got_vec.len(), "{label}: query {n} returned duplicates");
        let want = brute_query(live, q);
        assert!(got == want, "{label}: query_intersects #{n} {q:?}: got {} items, brute force {} (missing {:?}, extra {:?})",
            got.len(), want.len(),
            want.difference(&got).take(5).collect::<Vec<_>>(),
            got.difference(&want).take(5).collect::<Vec<_>>());
    }
}

fn check_nearest(tree: &RStarTree<usize>, live: &[(Aabb, usize)], boxes: &HashMap<usize, Aabb>, rng: &mut Rng, label: &str) {
    for n in 0..120 {
        let p = [rng.range(-50.0, 1050.0), rng.range(-50.0, 1050.0), rng.range(-50.0, 1050.0)];
        let k = [1usize, 7, 50][n % 3];
        let got: Vec<usize> = tree.nearest(p, k).into_iter().copied().collect();
        assert_eq!(got.len(), k.min(live.len()), "{label}: nearest({p:?}, {k}) returned {} items", got.len());
        let distinct: BTreeSet<usize> = got.iter().copied().collect();
        assert_eq!(distinct.len(), got.len(), "{label}: nearest returned duplicates");
        let got_d: Vec<f64> = got.iter().map(|i| point_box_dist(p, &boxes[i])).collect();
        for w in got_d.windows(2) {
            assert!(w[0] <= w[1] + 1e-12, "{label}: nearest results not ordered by distance: {got_d:?}");
        }
        let mut all: Vec<f64> = live.iter().map(|(b, _)| point_box_dist(p, b)).collect();
        all.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for (j, (g, w)) in got_d.iter().zip(&all).enumerate() {
            assert!((g - w).abs() <= 1e-9, "{label}: nearest({p:?}, {k}) rank {j}: distance {g}, brute force {w}");
        }
    }
    // A point inside a box has distance 0 to it.
    let (b, _) = live[rng.below(live.len() as u64) as usize];
    let c = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0, (b.min[2] + b.max[2]) / 2.0];
    let first = *tree.nearest(c, 1)[0];
    assert_eq!(point_box_dist(c, &boxes[&first]), 0.0, "{label}: nearest to a point inside a box must be at distance 0");
}

#[test]
fn rstar_bulk_load_matches_brute_force_and_is_fast() {
    watchdog();
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let boxes = random_boxes(&mut rng, 100_000);
    let items: Vec<(Aabb, usize)> = boxes.iter().copied().zip(0..).collect();
    let by_id: HashMap<usize, Aabb> = items.iter().map(|(b, i)| (*i, *b)).collect();

    let mut best = Duration::MAX;
    let mut tree = RStarTree::bulk_load(items.clone());
    for _ in 0..3 {
        let data = items.clone();
        let t0 = Instant::now();
        tree = RStarTree::bulk_load(data);
        best = best.min(t0.elapsed());
    }
    assert!(best < Duration::from_millis(300), "100k bulk load took {best:?} (budget 300 ms)");
    assert_eq!(tree.len(), 100_000);

    let qs = query_boxes(&mut rng, &boxes);
    check_queries(&tree, &items, &qs, "bulk");
    check_nearest(&tree, &items, &by_id, &mut rng, "bulk");

    // Remove 30k entries, then insert 10k more; answers must track the live set.
    let mut live = items.clone();
    let removed: BTreeSet<usize> = (0..100_000).filter(|i| i % 10 < 3).collect();
    for &i in &removed {
        assert!(tree.remove(&boxes[i], &i), "remove of present item {i} returned false");
    }
    live.retain(|(_, i)| !removed.contains(i));
    assert!(!tree.remove(&boxes[0], &0), "removing an absent item must return false");
    let wrong_box = bx([-1.0; 3], [-0.5; 3]);
    assert!(!tree.remove(&wrong_box, &5), "remove must match the stored box, not just the item");
    assert!(tree.remove(&boxes[5], &5) && tree.remove(&boxes[5], &5) == false, "item 5 is removed exactly once");
    live.retain(|(_, i)| *i != 5);
    let extra = random_boxes(&mut rng, 10_000);
    let mut by_id = by_id;
    for (j, b) in extra.iter().enumerate() {
        let id = 100_000 + j;
        tree.insert(*b, id);
        live.push((*b, id));
        by_id.insert(id, *b);
    }
    assert_eq!(tree.len(), live.len(), "len after removes and inserts");
    check_queries(&tree, &live, &qs, "bulk+remove+insert");
    check_nearest(&tree, &live, &by_id, &mut rng, "bulk+remove+insert");
}

#[test]
fn rstar_incremental_inserts_match_brute_force() {
    watchdog();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let boxes = random_boxes(&mut rng, 30_000);
    let mut tree: RStarTree<usize> = RStarTree::new();
    assert_eq!(tree.len(), 0);
    assert!(tree.query_intersects(&bx([0.0; 3], [1e9; 3])).is_empty());
    assert!(tree.nearest([0.0; 3], 3).is_empty());
    let items: Vec<(Aabb, usize)> = boxes.iter().copied().zip(0..).collect();
    for (b, i) in &items {
        tree.insert(*b, *i);
    }
    assert_eq!(tree.len(), items.len());
    let by_id: HashMap<usize, Aabb> = items.iter().map(|(b, i)| (*i, *b)).collect();
    let qs = query_boxes(&mut rng, &boxes);
    check_queries(&tree, &items, &qs, "insert");
    check_nearest(&tree, &items, &by_id, &mut rng, "insert");
    // Remove everything: the tree must end empty.
    for (b, i) in &items {
        assert!(tree.remove(b, i), "remove {i}");
    }
    assert_eq!(tree.len(), 0);
    assert!(tree.query_intersects(&bx([-1e9; 3], [1e9; 3])).is_empty());
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Möller–Trumbore without early exits: returns (t, barycentric margin) for the ray's
/// plane hit, or None when the ray is parallel to the triangle's plane. A hit is
/// margin >= 0 and t > 0 (two-sided, no back-face culling).
fn mt(o: [f64; 3], d: [f64; 3], tri: &Tri) -> Option<(f64, f64)> {
    let e1 = sub(tri[1], tri[0]);
    let e2 = sub(tri[2], tri[0]);
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-300 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, tri[0]);
    let u = dot(s, p) * inv;
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    let t = dot(e2, q) * inv;
    Some((t, u.min(v).min(1.0 - u - v)))
}

const FUZZ: f64 = 1e-9;

/// Accepts the BVH answer iff it is the closest hit up to grazing (edge) ambiguity.
fn check_ray(tris: &[Tri], o: [f64; 3], d: [f64; 3], got: Option<(usize, f64)>, label: &str) {
    let mut strict_min = f64::INFINITY;
    let mut fuzzy_min = f64::INFINITY;
    for tri in tris {
        if let Some((t, m)) = mt(o, d, tri) {
            if t > 1e-12 && m >= -FUZZ {
                fuzzy_min = fuzzy_min.min(t);
                if m >= FUZZ {
                    strict_min = strict_min.min(t);
                }
            }
        }
    }
    match got {
        None => assert!(strict_min.is_infinite(), "{label}: ray {o:?} {d:?} missed but brute force hits at t={strict_min}"),
        Some((i, t)) => {
            assert!(i < tris.len(), "{label}: triangle index {i} out of range");
            let (ti, mi) = mt(o, d, &tris[i]).unwrap_or_else(|| panic!("{label}: reported triangle {i} is parallel to the ray"));
            let tol = 1e-9 * t.abs().max(1.0);
            assert!(mi >= -FUZZ && ti > 0.0, "{label}: reported triangle {i} is not hit (margin {mi}, t {ti})");
            assert!((t - ti).abs() <= tol, "{label}: reported t={t} but triangle {i} is hit at t={ti}");
            assert!(t <= strict_min + tol && t >= fuzzy_min - tol,
                "{label}: ray {o:?} {d:?} returned t={t}, closest brute-force hit t={strict_min}");
        }
    }
}

fn random_tris(rng: &mut Rng, n: usize, span: f64, size: f64) -> Vec<Tri> {
    (0..n)
        .map(|_| {
            let c = [rng.range(0.0, span), rng.range(0.0, span), rng.range(0.0, span)];
            let mut v = || [c[0] + rng.range(-size, size), c[1] + rng.range(-size, size), c[2] + rng.range(-size, size)];
            [v(), v(), v()]
        })
        .collect()
}

fn random_dir(rng: &mut Rng, n: usize) -> [f64; 3] {
    match n % 5 {
        // Axis-aligned rays exercise slab tests with zero direction components.
        0 => {
            let mut d = [0.0; 3];
            d[rng.below(3) as usize] = if rng.below(2) == 0 { 1.0 } else { -1.0 };
            d
        }
        1 => [rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)],
        _ => [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)],
    }
}

#[test]
fn bvh_raycast_on_a_million_triangles_matches_brute_force_and_is_fast() {
    watchdog();
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let tris = random_tris(&mut rng, 1_000_000, 100.0, 0.5);
    let bvh = Bvh::build(&tris);

    let mut rays = Vec::new();
    for n in 0..2000 {
        let o = if n % 4 == 0 {
            // From outside the cloud, aimed at its interior.
            let target = [rng.range(10.0, 90.0), rng.range(10.0, 90.0), rng.range(10.0, 90.0)];
            let o = [rng.range(-50.0, 150.0), rng.range(-50.0, 150.0), -40.0];
            rays.push((o, sub(target, o)));
            continue;
        } else {
            [rng.range(0.0, 100.0), rng.range(0.0, 100.0), rng.range(0.0, 100.0)]
        };
        rays.push((o, random_dir(&mut rng, n)));
    }
    // Rays leaving the cloud: no hits.
    rays.push(([50.0, 50.0, 200.0], [0.0, 0.0, 1.0]));
    rays.push(([-10.0, 50.0, 50.0], [-1.0, 0.3, 0.0]));

    // Timing: mean over all rays, best of three passes.
    let mut best = f64::INFINITY;
    let mut results = Vec::new();
    for _ in 0..3 {
        let t0 = Instant::now();
        results = rays.iter().map(|(o, d)| bvh.raycast(*o, *d)).collect::<Vec<_>>();
        best = best.min(t0.elapsed().as_secs_f64() / rays.len() as f64);
    }
    assert!(best < 1e-3, "mean raycast on 1M triangles took {:.3} ms (budget 1 ms)", best * 1e3);

    // Correctness: brute force on a sample of rays, including both no-hit rays.
    let n = rays.len();
    for idx in (0..n).step_by(40).chain([n - 2, n - 1]) {
        let (o, d) = rays[idx];
        check_ray(&tris, o, d, results[idx], "1M");
    }
    assert!(results[n - 2].is_none() && results[n - 1].is_none(), "rays leaving the cloud must miss");

    // t is in units of `dir`: scaling dir by 4 divides t by 4.
    for idx in (1..n).step_by(97).take(20) {
        let (o, d) = rays[idx];
        if let Some((i, t)) = results[idx] {
            let (i4, t4) = bvh.raycast(o, [d[0] * 4.0, d[1] * 4.0, d[2] * 4.0]).expect("scaled ray must still hit");
            assert!((t4 * 4.0 - t).abs() <= 1e-9 * t.max(1.0), "t not in units of dir: {t} vs 4*{t4}");
            let _ = (i, i4);
        }
    }
}

#[test]
fn bvh_raycast_small_cases() {
    watchdog();
    // A unit quad at z = 0 as two triangles, and a triangle at z = 2 above part of it.
    let tris: Vec<Tri> = vec![
        [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
        [[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        [[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]],
    ];
    let bvh = Bvh::build(&tris);
    let near = |got: Option<(usize, f64)>, i: usize, t: f64, what: &str| {
        let (gi, gt) = got.unwrap_or_else(|| panic!("{what}: expected a hit on triangle {i}"));
        assert!(gi == i && (gt - t).abs() < 1e-12, "{what}: got ({gi}, {gt}), want ({i}, {t})");
    };
    near(bvh.raycast([0.7, 0.2, 5.0], [0.0, 0.0, -1.0]), 2, 3.0, "closest of two stacked hits");
    near(bvh.raycast([0.8, 0.7, 5.0], [0.0, 0.0, -1.0]), 0, 5.0, "beside the upper triangle");
    near(bvh.raycast([0.2, 0.7, -1.0], [0.0, 0.0, 1.0]), 1, 1.0, "two-sided: hit from below");
    near(bvh.raycast([0.2, 0.2, 1.0], [0.0, 0.0, 0.5]), 2, 2.0, "t in units of an unnormalized dir");
    let diag = bvh.raycast([0.5, 0.5, 1.0], [0.0, 0.0, -1.0]).expect("ray onto the shared diagonal must hit");
    assert!(diag.0 <= 1 && (diag.1 - 1.0).abs() < 1e-12, "shared edge hit: {diag:?}");
    assert!(bvh.raycast([0.2, 0.2, -1.0], [0.0, 0.0, -1.0]).is_none(), "hits behind the origin must be ignored");
    assert!(bvh.raycast([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0]).is_none(), "ray parallel to every plane misses");
    assert!(bvh.raycast([3.0, 3.0, 5.0], [0.0, 0.0, -1.0]).is_none(), "ray beside everything misses");
    let empty = Bvh::build(&[]);
    assert!(empty.raycast([0.0; 3], [1.0, 0.0, 0.0]).is_none(), "empty BVH");
    assert!(empty.overlaps(&bvh).is_empty() && bvh.overlaps(&empty).is_empty(), "overlaps with an empty BVH");
}

fn tri_box(t: &Tri) -> Aabb {
    let mut b = bx(t[0], t[0]);
    for v in &t[1..] {
        for k in 0..3 {
            b.min[k] = b.min[k].min(v[k]);
            b.max[k] = b.max[k].max(v[k]);
        }
    }
    b
}

/// Independent broad phase: uniform grid with cell size above every box extent.
fn grid_pairs(a: &[Tri], b: &[Tri], cell: f64) -> Vec<(usize, usize)> {
    let key = |p: f64| (p / cell).floor() as i64;
    let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    let bb: Vec<Aabb> = b.iter().map(tri_box).collect();
    for (j, x) in bb.iter().enumerate() {
        grid.entry((key(x.min[0]), key(x.min[1]), key(x.min[2]))).or_default().push(j);
    }
    let mut out = BTreeSet::new();
    for (i, t) in a.iter().enumerate() {
        let x = tri_box(t);
        for cx in key(x.min[0]) - 1..=key(x.max[0]) {
            for cy in key(x.min[1]) - 1..=key(x.max[1]) {
                for cz in key(x.min[2]) - 1..=key(x.max[2]) {
                    for &j in grid.get(&(cx, cy, cz)).into_iter().flatten() {
                        if intersects(&x, &bb[j]) {
                            out.insert((i, j));
                        }
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

#[test]
fn bvh_overlaps_match_brute_force() {
    watchdog();
    let mut rng = Rng(0x0BAD_5EED_1234_5678);
    let a = random_tris(&mut rng, 100_000, 100.0, 0.5);
    let b = random_tris(&mut rng, 100_000, 100.0, 0.5);
    let (ba, bb) = (Bvh::build(&a), Bvh::build(&b));
    let mut got = ba.overlaps(&bb);
    let n = got.len();
    got.sort();
    got.dedup();
    assert_eq!(got.len(), n, "overlaps returned duplicate pairs");
    let want = grid_pairs(&a, &b, 1.0);
    assert!(want.len() > 1000, "oracle sanity: only {} overlapping pairs", want.len());
    assert!(got == want, "overlaps: {} pairs, brute force {} pairs", got.len(), want.len());

    // Exactly touching triangle boxes count (closed boxes); order is (self, other).
    let t1: Vec<Tri> = vec![[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]];
    let t2: Vec<Tri> = vec![
        [[5.0, 5.0, 5.0], [6.0, 5.0, 5.0], [5.0, 6.0, 5.0]],
        [[1.0, 1.0, 0.0], [2.0, 1.0, 0.0], [1.0, 2.0, 0.0]],
    ];
    assert_eq!(Bvh::build(&t1).overlaps(&Bvh::build(&t2)), vec![(0, 1)]);
    assert_eq!(Bvh::build(&t2).overlaps(&Bvh::build(&t1)), vec![(1, 0)]);
}
