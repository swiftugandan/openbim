// External judge for T7.2 (bim-render). Expected values are closed forms (prism volumes, areas and
// bounding boxes from the profile polygons) and brute force (per-instance AABB frustum tests).
use bim_doc::EntityId;
use bim_geom::{Point2, Polygon, Solid};
use bim_render::{BufferKind, DirtyRange, Frustum, RenderScene};
use std::collections::{BTreeMap, BTreeSet, HashMap};

// ---------- helpers ----------

/// Accept `Solid::extrude` returning either `Solid` or `Result<Solid, E>`.
trait IntoSolid {
    fn into_solid(self) -> Solid;
}
impl IntoSolid for Solid {
    fn into_solid(self) -> Solid {
        self
    }
}
impl<E: std::fmt::Debug> IntoSolid for Result<Solid, E> {
    fn into_solid(self) -> Solid {
        self.expect("extrude")
    }
}

fn eid(s: &str) -> EntityId {
    EntityId::derive(s)
}

/// A profile (outer CCW, holes CW) with its closed-form area and bbox, placed by a rigid motion in plan.
#[derive(Clone)]
struct Prism {
    outer: Vec<(f64, f64)>,
    holes: Vec<Vec<(f64, f64)>>,
    z0: f64,
    h: f64,
}

impl Prism {
    fn moved(&self, angle: f64, dx: f64, dy: f64, dz: f64) -> Prism {
        let (s, c) = angle.sin_cos();
        let f = |p: &(f64, f64)| (c * p.0 - s * p.1 + dx, s * p.0 + c * p.1 + dy);
        Prism {
            outer: self.outer.iter().map(f).collect(),
            holes: self.holes.iter().map(|h| h.iter().map(f).collect()).collect(),
            z0: self.z0 + dz,
            h: self.h,
        }
    }
    fn solid(&self) -> Solid {
        let pts = |v: &Vec<(f64, f64)>| v.iter().map(|&(x, y)| Point2 { x, y }).collect::<Vec<_>>();
        let poly = Polygon { outer: pts(&self.outer), holes: self.holes.iter().map(pts).collect() };
        Solid::extrude(&poly, self.z0, self.h).into_solid()
    }
    fn area(&self) -> f64 {
        let shoelace = |v: &Vec<(f64, f64)>| {
            (0..v.len()).map(|i| { let (a, b) = (v[i], v[(i + 1) % v.len()]); a.0 * b.1 - b.0 * a.1 }).sum::<f64>() / 2.0
        };
        shoelace(&self.outer).abs() - self.holes.iter().map(|h| shoelace(h).abs()).sum::<f64>()
    }
    fn volume(&self) -> f64 {
        self.area() * self.h
    }
    fn bbox(&self) -> ([f64; 3], [f64; 3]) {
        let mut lo = [f64::INFINITY, f64::INFINITY, self.z0];
        let mut hi = [f64::NEG_INFINITY, f64::NEG_INFINITY, self.z0 + self.h];
        for p in &self.outer {
            lo[0] = lo[0].min(p.0); lo[1] = lo[1].min(p.1);
            hi[0] = hi[0].max(p.0); hi[1] = hi[1].max(p.1);
        }
        (lo, hi)
    }
}

fn rect(w: f64, d: f64, h: f64) -> Prism {
    Prism { outer: vec![(0.0, 0.0), (w, 0.0), (w, d), (0.0, d)], holes: vec![], z0: 0.0, h }
}
fn ell(s: f64) -> Prism {
    let v = [(0.0, 0.0), (2.0, 0.0), (2.0, 0.5), (0.5, 0.5), (0.5, 1.5), (0.0, 1.5)];
    Prism { outer: v.iter().map(|&(x, y)| (x * s, y * s)).collect(), holes: vec![], z0: 0.0, h: 2.5 }
}
fn holed() -> Prism {
    Prism {
        outer: vec![(0.0, 0.0), (3.0, 0.0), (3.0, 0.3), (0.0, 0.3)],
        holes: vec![vec![(1.0, 0.0 + 0.1), (1.0, 0.2), (2.0, 0.2), (2.0, 0.1)]],
        z0: 0.0,
        h: 2.0,
    }
}

#[derive(Default, Clone, Debug)]
struct Summary {
    tris: usize,
    vol: f64,
    area: f64,
    lo: [f64; 3],
    hi: [f64; 3],
}
fn summarize(tris: impl Iterator<Item = [[f64; 3]; 3]>) -> Summary {
    let mut s = Summary { lo: [f64::INFINITY; 3], hi: [f64::NEG_INFINITY; 3], ..Default::default() };
    for [a, b, c] in tris {
        s.tris += 1;
        s.vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        s.area += (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0;
        for p in [a, b, c] {
            for k in 0..3 {
                s.lo[k] = s.lo[k].min(p[k]);
                s.hi[k] = s.hi[k].max(p[k]);
            }
        }
    }
    s
}

/// World triangles per entity index, from the flattened soup.
fn soup(sc: &RenderScene) -> BTreeMap<u32, Vec<[[f64; 3]; 3]>> {
    let (p, x, e) = (sc.positions(), sc.indices(), sc.triangle_entity());
    assert_eq!(p.len() % 3, 0, "positions length");
    assert_eq!(sc.normals().len(), p.len(), "normals length must equal positions length");
    assert_eq!(x.len() % 3, 0, "indices length");
    assert_eq!(e.len(), x.len() / 3, "triangle_entity length must equal triangle count");
    let v = |i: u32| { let i = i as usize; assert!(3 * i + 2 < p.len(), "index out of range"); [p[3 * i] as f64, p[3 * i + 1] as f64, p[3 * i + 2] as f64] };
    let mut m: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    for t in 0..e.len() {
        m.entry(e[t]).or_default().push([v(x[3 * t]), v(x[3 * t + 1]), v(x[3 * t + 2])]);
    }
    m
}

/// World triangles per instance: family mesh × instance transform (column-major mat4).
fn instance_tris(sc: &RenderScene, i: usize) -> Vec<[[f64; 3]; 3]> {
    let m: Vec<f64> = sc.instances()[16 * i..16 * i + 16].iter().map(|&f| f as f64).collect();
    // rigid: orthonormal 3x3 with det +1, last row (0,0,0,1)
    let col = |c: usize| [m[4 * c], m[4 * c + 1], m[4 * c + 2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    for a in 0..3 {
        for b in 0..3 {
            let want = if a == b { 1.0 } else { 0.0 };
            assert!((dot(col(a), col(b)) - want).abs() < 1e-5, "instance {i} transform is not rigid: {m:?}");
        }
    }
    let (c0, c1, c2) = (col(0), col(1), col(2));
    let det = c0[0] * (c1[1] * c2[2] - c1[2] * c2[1]) - c1[0] * (c0[1] * c2[2] - c0[2] * c2[1]) + c2[0] * (c0[1] * c1[2] - c0[2] * c1[1]);
    assert!((det - 1.0).abs() < 1e-5, "instance {i} transform has det {det} (reflection or scale)");
    assert!(m[3].abs() < 1e-6 && m[7].abs() < 1e-6 && m[11].abs() < 1e-6 && (m[15] - 1.0).abs() < 1e-6, "instance {i}: bad last row");
    let fam = sc.instance_mesh()[i];
    let (mp, mi) = (sc.mesh_positions(fam), sc.mesh_indices(fam));
    let v = |k: u32| {
        let k = k as usize;
        let p = [mp[3 * k] as f64, mp[3 * k + 1] as f64, mp[3 * k + 2] as f64];
        [0, 1, 2].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
    };
    mi.chunks(3).map(|t| [v(t[0]), v(t[1]), v(t[2])]).collect()
}

fn check_summary(got: &Summary, p: &Prism, what: &str) {
    let rel = |a: f64, b: f64| (a - b).abs() <= 1e-5 * b.abs().max(1.0);
    assert!(rel(got.vol, p.volume()), "{what}: volume {} != closed form {}", got.vol, p.volume());
    // lateral + 2 caps; lateral = perimeter (outer + holes) × h
    let perim = |v: &Vec<(f64, f64)>| (0..v.len()).map(|i| { let (a, b) = (v[i], v[(i + 1) % v.len()]); ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() }).sum::<f64>();
    let area = 2.0 * p.area() + p.h * (perim(&p.outer) + p.holes.iter().map(perim).sum::<f64>());
    assert!(rel(got.area, area), "{what}: surface area {} != closed form {}", got.area, area);
    let (lo, hi) = p.bbox();
    for k in 0..3 {
        assert!((got.lo[k] - lo[k]).abs() < 1e-4 && (got.hi[k] - hi[k]).abs() < 1e-4, "{what}: bbox {:?}..{:?} != {:?}..{:?}", got.lo, got.hi, lo, hi);
    }
}

fn index_of(sc: &RenderScene, name: &str, n: u32) -> u32 {
    (0..n).find(|&i| sc.entity_id(i) == eid(name)).unwrap_or_else(|| panic!("entity {name} has no entity index"))
}
fn instance_of(sc: &RenderScene, ent: u32) -> usize {
    let found: Vec<usize> = sc.instance_entity().iter().enumerate().filter(|(_, &e)| e == ent).map(|(i, _)| i).collect();
    assert_eq!(found.len(), 1, "entity index {ent} must have exactly one instance");
    found[0]
}

// ---------- 1. family instancing ----------

/// (name, class, prism): same class = congruent up to a proper rigid motion.
fn dedupe_scene() -> Vec<(String, u32, Prism)> {
    let mut v = vec![];
    let col = rect(0.4, 0.6, 3.0);
    for (i, &(a, x, y)) in [(0.0, 0.0, 0.0), (0.0, 5.0, 0.0), (0.0, 10.0, 0.0), (std::f64::consts::FRAC_PI_2, 15.0, 0.0), (0.6457718232379019, 20.0, 3.0), (std::f64::consts::PI, 25.0, 0.0)]
        .iter()
        .enumerate()
    {
        v.push((format!("col{i}"), 0, col.moved(a, x, y, 0.0)));
    }
    // same rectangle listed from a different start vertex, and lifted to another storey
    let mut shifted = col.moved(0.0, 30.0, 0.0, 3.5);
    shifted.outer.rotate_left(2);
    v.push(("col_shifted".into(), 0, shifted));
    for (i, &(a, x, y)) in [(0.0, 0.0, 10.0), (2.0943951023931953, 6.0, 10.0), (-1.0, 12.0, 12.0)].iter().enumerate() {
        v.push((format!("ell{i}"), 1, ell(1.0).moved(a, x, y, 0.0)));
    }
    v.push(("col_tall".into(), 2, rect(0.4, 0.6, 3.5).moved(0.0, 0.0, 20.0, 0.0)));
    v.push(("col_square".into(), 3, rect(0.4, 0.4, 3.0).moved(0.0, 5.0, 20.0, 0.0)));
    v.push(("ell_big".into(), 4, ell(1.5).moved(0.0, 10.0, 20.0, 0.0)));
    v.push(("wall_a".into(), 5, holed().moved(0.0, 0.0, 30.0, 0.0)));
    v.push(("wall_b".into(), 5, holed().moved(std::f64::consts::FRAC_PI_2, 10.0, 30.0, 0.0)));
    v.push(("wall_c".into(), 5, holed().moved(std::f64::consts::PI, 20.0, 33.0, 0.0)));
    v
}

#[test]
fn identical_solids_share_one_family_mesh() {
    let items = dedupe_scene();
    let mut sc = RenderScene::new();
    for (n, _, p) in &items {
        sc.set_entity(eid(n), &p.solid());
    }
    sc.flush();
    let n = items.len() as u32;
    assert_eq!(sc.instance_mesh().len(), items.len(), "one instance per entity");
    assert_eq!(sc.instances().len(), 16 * items.len(), "16 floats per instance");
    assert_eq!(sc.instance_entity().len(), items.len());
    let by_soup = soup(&sc);
    let mut fam_of_class: HashMap<u32, u32> = HashMap::new();
    let mut families = BTreeSet::new();
    for (name, class, p) in &items {
        let e = index_of(&sc, name, n);
        let i = instance_of(&sc, e);
        let fam = sc.instance_mesh()[i];
        families.insert(fam);
        let prev = *fam_of_class.entry(*class).or_insert(fam);
        assert_eq!(prev, fam, "{name} is congruent to an earlier entity of class {class} but got a different family mesh");
        check_summary(&summarize(instance_tris(&sc, i).into_iter()), p, &format!("{name} (family × instance transform)"));
        let tris = by_soup.get(&e).unwrap_or_else(|| panic!("{name} has no triangles in the soup"));
        check_summary(&summarize(tris.iter().copied()), p, &format!("{name} (world triangles)"));
    }
    assert_eq!(families.len(), 6, "6 congruence classes must give exactly 6 family meshes");
    assert_eq!(fam_of_class.len(), 6);
}

// ---------- 2. BVH frustum culling equals brute force ----------

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}
fn norm(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
/// Six inward planes (n, d) with n·p + d ≥ 0 inside, for a perspective camera.
fn camera_planes(eye: [f64; 3], target: [f64; 3], fovy: f64, aspect: f64, near: f64, far: f64) -> [[f64; 4]; 6] {
    let f = norm([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
    let r = norm(cross(f, [0.0, 0.0, 1.0]));
    let u = cross(r, f);
    let (tv, th) = ((fovy / 2.0).tan(), aspect * (fovy / 2.0).tan());
    let plane = |n: [f64; 3], p: [f64; 3]| { let n = norm(n); [n[0], n[1], n[2], -dot(n, p)] };
    let add = |a: [f64; 3], s: f64, b: [f64; 3]| [a[0] + s * b[0], a[1] + s * b[1], a[2] + s * b[2]];
    [
        plane(f, add(eye, near, f)),
        plane([-f[0], -f[1], -f[2]], add(eye, far, f)),
        plane(add(r, th, f), eye),
        plane(add([-r[0], -r[1], -r[2]], th, f), eye),
        plane(add(u, tv, f), eye),
        plane(add([-u[0], -u[1], -u[2]], tv, f), eye),
    ]
}
/// max over the AABB corners of the plane function (the "p-vertex" test).
fn pmax(pl: &[f64; 4], lo: [f64; 3], hi: [f64; 3]) -> f64 {
    let mut s = pl[3];
    for k in 0..3 {
        s += if pl[k] >= 0.0 { pl[k] * hi[k] } else { pl[k] * lo[k] };
    }
    s
}

#[test]
fn bvh_culling_equals_brute_force_on_random_cameras() {
    let mut rng = Lcg(0x7_2_2026);
    let mut sc = RenderScene::new();
    let mut count = 0usize;
    for gx in 0..40 {
        for gy in 0..40 {
            let w = rng.range(0.2, 2.0);
            let d = rng.range(0.2, 2.0);
            let h = rng.range(0.5, 8.0);
            let p = rect(w, d, h).moved(rng.range(0.0, 6.28), gx as f64 * 3.0, gy as f64 * 3.0, rng.range(0.0, 10.0));
            sc.set_entity(eid(&format!("box{gx}_{gy}")), &p.solid());
            count += 1;
        }
    }
    sc.flush();
    assert_eq!(sc.instance_mesh().len(), count);
    // world AABB per instance, from the instance's own world triangles
    let boxes: Vec<([f64; 3], [f64; 3])> = (0..count)
        .map(|i| { let s = summarize(instance_tris(&sc, i).into_iter()); (s.lo, s.hi) })
        .collect();
    let mut nonempty = 0;
    let mut partial = 0;
    for cam in 0..250 {
        let eye = [rng.range(-30.0, 150.0), rng.range(-30.0, 150.0), rng.range(-5.0, 60.0)];
        let target = [rng.range(0.0, 120.0), rng.range(0.0, 120.0), rng.range(0.0, 15.0)];
        let f = norm([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
        if f[2].abs() > 0.98 {
            continue;
        }
        let planes = camera_planes(eye, target, rng.range(0.3, 1.6), rng.range(0.5, 2.0), rng.range(0.05, 1.0), rng.range(10.0, 200.0));
        let got: Vec<u32> = sc.cull(&Frustum::new(planes));
        assert!(got.windows(2).all(|w| w[0] < w[1]), "camera {cam}: cull result must be sorted and deduplicated");
        let got_set: BTreeSet<u32> = got.iter().copied().collect();
        for (i, &(lo, hi)) in boxes.iter().enumerate() {
            let worst = planes.iter().map(|pl| pmax(pl, lo, hi)).fold(f64::INFINITY, f64::min);
            let inside = got_set.contains(&(i as u32));
            if worst > 1e-4 && !inside {
                panic!("camera {cam}: instance {i} intersects the frustum but was culled");
            }
            if worst < -1e-4 && inside {
                panic!("camera {cam}: instance {i} is outside a frustum plane but was returned");
            }
        }
        if !got.is_empty() {
            nonempty += 1;
        }
        if !got.is_empty() && got.len() < count {
            partial += 1;
        }
    }
    assert!(nonempty > 50 && partial > 50, "random cameras too degenerate ({nonempty} nonempty, {partial} partial)");
    // everything / nothing
    let all = [[1.0, 0.0, 0.0, 1e6], [-1.0, 0.0, 0.0, 1e6], [0.0, 1.0, 0.0, 1e6], [0.0, -1.0, 0.0, 1e6], [0.0, 0.0, 1.0, 1e6], [0.0, 0.0, -1.0, 1e6]];
    assert_eq!(sc.cull(&Frustum::new(all)).len(), count, "a frustum enclosing the scene must return every instance");
    let mut none = all;
    none[0] = [1.0, 0.0, 0.0, -1e5];
    assert!(sc.cull(&Frustum::new(none)).is_empty(), "a frustum beyond the scene must return nothing");
}

// ---------- 3. incremental updates report minimal dirty ranges ----------

struct Snap {
    bufs: Vec<(BufferKind, Vec<u32>)>,
}
fn snap(sc: &RenderScene) -> Snap {
    let f = |s: &[f32]| s.iter().map(|x| x.to_bits()).collect::<Vec<u32>>();
    Snap {
        bufs: vec![
            (BufferKind::Positions, f(sc.positions())),
            (BufferKind::Normals, f(sc.normals())),
            (BufferKind::Indices, sc.indices().to_vec()),
            (BufferKind::TriangleEntity, sc.triangle_entity().to_vec()),
            (BufferKind::Instances, f(sc.instances())),
            (BufferKind::InstanceMesh, sc.instance_mesh().to_vec()),
            (BufferKind::InstanceEntity, sc.instance_entity().to_vec()),
        ],
    }
}
/// Element indices of each buffer that belong to entity index `e` in the scene's current buffers.
fn slots_of(sc: &RenderScene, e: u32, kind: BufferKind) -> BTreeSet<usize> {
    let tris: Vec<usize> = sc.triangle_entity().iter().enumerate().filter(|(_, &x)| x == e).map(|(t, _)| t).collect();
    let i = instance_of(sc, e);
    match kind {
        BufferKind::Positions | BufferKind::Normals => tris.iter().flat_map(|&t| (0..3).map(move |k| t * 3 + k)).flat_map(|k| { let v = sc.indices()[k] as usize; [3 * v, 3 * v + 1, 3 * v + 2] }).collect(),
        BufferKind::Indices => tris.iter().flat_map(|&t| [3 * t, 3 * t + 1, 3 * t + 2]).collect(),
        BufferKind::TriangleEntity => tris.into_iter().collect(),
        BufferKind::Instances => (16 * i..16 * i + 16).collect(),
        BufferKind::InstanceMesh | BufferKind::InstanceEntity => [i].into_iter().collect(),
    }
}
fn check_dirty(before: &Snap, sc: &RenderScene, ranges: &[DirtyRange], changed: u32, what: &str) {
    let after = snap(sc);
    for ((kind, old), (_, new)) in before.bufs.iter().zip(after.bufs.iter()) {
        let mine: Vec<&DirtyRange> = ranges.iter().filter(|r| r.kind == *kind).collect();
        let allowed = slots_of(sc, changed, *kind);
        for r in &mine {
            assert!(r.start < r.end && r.end <= new.len(), "{what}: bad {kind:?} range {}..{}", r.start, r.end);
            for k in r.start..r.end {
                assert!(allowed.contains(&k), "{what}: {kind:?} range {}..{} touches element {k}, which belongs to another entity", r.start, r.end);
            }
        }
        for k in 0..old.len().max(new.len()) {
            if old.get(k) != new.get(k) {
                assert!(k < new.len(), "{what}: {kind:?} shrank; other entities' elements moved");
                assert!(mine.iter().any(|r| r.start <= k && k < r.end), "{what}: {kind:?} element {k} changed but no dirty range covers it");
            }
        }
    }
}

#[test]
fn incremental_update_touches_only_changed_entities() {
    let items = dedupe_scene();
    let mut sc = RenderScene::new();
    for (n, _, p) in &items {
        sc.set_entity(eid(n), &p.solid());
    }
    let first = sc.flush();
    assert!(!first.is_empty(), "the first flush must report the new buffers as dirty");
    assert!(sc.flush().is_empty(), "a flush with no changes must report no dirty ranges");

    // rigid move of one entity (same family)
    let (name, _, p) = &items[2];
    let n = items.len() as u32;
    let e = index_of(&sc, name, n);
    let fam = sc.instance_mesh()[instance_of(&sc, e)];
    let before = snap(&sc);
    let moved = p.moved(0.0, 1.25, -0.5, 0.0);
    sc.set_entity(eid(name), &moved.solid());
    let ranges = sc.flush();
    assert!(!ranges.is_empty(), "moving {name} must report dirty ranges");
    assert!(ranges.iter().any(|r| r.kind == BufferKind::Instances), "moving {name} must dirty its instance transform");
    check_dirty(&before, &sc, &ranges, e, "rigid move");
    assert_eq!(index_of(&sc, name, n), e, "entity indices must be stable across updates");
    assert_eq!(sc.instance_mesh()[instance_of(&sc, e)], fam, "a rigid move keeps the family mesh");
    check_summary(&summarize(soup(&sc)[&e].iter().copied()), &moved, "moved entity (world triangles)");
    check_summary(&summarize(instance_tris(&sc, instance_of(&sc, e)).into_iter()), &moved, "moved entity (instance)");

    // reshape one entity with the same topology (taller box)
    let (name2, _, p2) = &items[0];
    let e2 = index_of(&sc, name2, n);
    let before = snap(&sc);
    let mut taller = p2.clone();
    taller.h = 4.25;
    sc.set_entity(eid(name2), &taller.solid());
    let ranges = sc.flush();
    check_dirty(&before, &sc, &ranges, e2, "reshape");
    check_summary(&summarize(soup(&sc)[&e2].iter().copied()), &taller, "reshaped entity (world triangles)");
    check_summary(&summarize(instance_tris(&sc, instance_of(&sc, e2)).into_iter()), &taller, "reshaped entity (instance)");

    // a new entity is appended; nothing else moves
    let before = snap(&sc);
    let extra = ell(1.0).moved(0.3, 40.0, 40.0, 0.0);
    sc.set_entity(eid("extra"), &extra.solid());
    let ranges = sc.flush();
    let e3 = index_of(&sc, "extra", n + 1);
    assert_eq!(e3, n, "a new entity takes the next entity index");
    check_dirty(&before, &sc, &ranges, e3, "create");
    check_summary(&summarize(soup(&sc)[&e3].iter().copied()), &extra, "created entity");

    // the incrementally updated scene equals a from-scratch build of the same solids
    let mut fresh = RenderScene::new();
    for (k, (nm, _, pp)) in items.iter().enumerate() {
        let q = if k == 2 { moved.clone() } else if k == 0 { taller.clone() } else { pp.clone() };
        fresh.set_entity(eid(nm), &q.solid());
    }
    fresh.set_entity(eid("extra"), &extra.solid());
    fresh.flush();
    let (a, b) = (soup(&sc), soup(&fresh));
    for (nm, _, _) in items.iter().chain(std::iter::once(&("extra".to_string(), 0, extra.clone()))) {
        let (ia, ib) = (index_of(&sc, nm, n + 1), index_of(&fresh, nm, n + 1));
        let (sa, sb) = (summarize(a[&ia].iter().copied()), summarize(b[&ib].iter().copied()));
        assert!(sa.tris == sb.tris && (sa.vol - sb.vol).abs() < 1e-6 && (sa.area - sb.area).abs() < 1e-6, "{nm}: incremental result differs from a from-scratch build");
    }
    let fams = |s: &RenderScene| s.instance_mesh().iter().copied().collect::<BTreeSet<u32>>().len();
    assert_eq!(fams(&sc) , fams(&fresh), "family count after updates differs from a from-scratch build");
}
