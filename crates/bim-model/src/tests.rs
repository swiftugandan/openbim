use super::*;
use bim_doc::Op;

struct B(Document);

impl B {
    fn new() -> B {
        B(Document::new())
    }
    fn add(&mut self, name: &str, kind: &str, props: &[(&str, Value)]) -> &mut B {
        let id = EntityId::derive(name);
        self.0.apply(Op::Create { id, kind: kind.into() }).unwrap();
        self.0.apply(Op::Set { id, key: "_name".into(), value: Value::Text(name.into()) }).unwrap();
        for (k, v) in props {
            self.0.apply(Op::Set { id, key: (*k).into(), value: v.clone() }).unwrap();
        }
        self
    }
}

fn n(x: f64) -> Value {
    Value::Num(x)
}
fn r(name: &str) -> Value {
    Value::Ref(EntityId::derive(name))
}
fn nl(xs: &[f64]) -> Value {
    Value::List(xs.iter().map(|&x| Value::Num(x)).collect())
}

fn base(b: &mut B, t: &[f64]) {
    b.add("L1", "Level", &[("elevation", n(0.0))]);
    let mats: Vec<Value> = (0..t.len()).map(|i| Value::Text(format!("m{i}"))).collect();
    b.add("WT", "WallType", &[("layer_thickness", nl(t)), ("layer_material", Value::List(mats))]);
}

fn wall(b: &mut B, name: &str, p: [f64; 4], h: f64) {
    b.add(name, "Wall", &[
        ("level", r("L1")), ("type", r("WT")),
        ("x1", n(p[0])), ("y1", n(p[1])), ("x2", n(p[2])), ("y2", n(p[3])), ("height", n(h)),
    ]);
}

fn check_closed(m: &Model) {
    for e in &m.elements {
        assert!(e.mesh.is_closed(), "{} not closed", e.name);
        let w = m.walls.iter().find(|w| w.id == e.id).map(|w| w.volume);
        if let Some(v) = w {
            assert!((e.mesh.volume() - v).abs() < 1e-9 * v.max(1.0), "{}: mesh {} vs {v}", e.name, e.mesh.volume());
        }
        assert!(e.mesh.to_solid().is_closed_manifold(), "{}", e.name);
    }
}

#[test]
fn free_wall_with_layers() {
    let mut b = B::new();
    base(&mut b, &[0.1, 0.2]);
    wall(&mut b, "W", [0.0, 0.0, 4.0, 0.0], 3.0);
    let m = Model::compile(&b.0);
    let w = &m.walls[0];
    assert!((w.volume - 3.6).abs() < 1e-12);
    assert!((w.side_area - 12.0).abs() < 1e-12);
    assert!((w.layers[0].volume - 1.2).abs() < 1e-12 && w.layers[0].material == "m0");
    check_closed(&m);
}

#[test]
fn l_join_equal_mitre_volumes_and_no_overlap() {
    let mut b = B::new();
    base(&mut b, &[0.2]);
    wall(&mut b, "A", [0.0, 0.0, 4.0, 0.0], 3.0);
    wall(&mut b, "B", [4.0, 0.0, 4.0, 3.0], 3.0);
    let m = Model::compile(&b.0);
    // Mitred at 45°: each wall's centreline length times thickness.
    assert!((m.walls[0].volume - 4.0 * 0.2 * 3.0).abs() < 1e-12, "{}", m.walls[0].volume);
    assert!((m.walls[1].volume - 3.0 * 0.2 * 3.0).abs() < 1e-12, "{}", m.walls[1].volume);
    assert_eq!(m.walls[0].joins[1], Some(Join::L(EntityId::derive("B"))));
    check_closed(&m);
}

#[test]
fn t_join_stops_at_face() {
    let mut b = B::new();
    base(&mut b, &[0.2]);
    wall(&mut b, "A", [0.0, 0.0, 6.0, 0.0], 3.0);
    wall(&mut b, "B", [3.0, 0.0, 3.0, 4.0], 3.0);
    let m = Model::compile(&b.0);
    let bw = m.walls.iter().find(|w| w.name == "B").unwrap();
    assert!((bw.volume - 3.9 * 0.2 * 3.0).abs() < 1e-12, "{}", bw.volume);
    assert_eq!(bw.joins[0], Some(Join::T(EntityId::derive("A"))));
    check_closed(&m);
}

#[test]
fn openings_overlapping_outside_and_invalid() {
    let mut b = B::new();
    base(&mut b, &[0.1, 0.1]);
    wall(&mut b, "W", [0.0, 0.0, 5.0, 0.0], 3.0);
    let op = |b: &mut B, name: &str, off: f64, sill: f64, w: f64, h: f64| {
        b.add(name, "Opening", &[("host", r("W")), ("offset", n(off)), ("sill", n(sill)), ("width", n(w)), ("height", n(h))]);
    };
    op(&mut b, "O1", 1.0, 0.0, 1.0, 2.0);
    op(&mut b, "O2", 1.5, 1.0, 1.0, 1.0); // overlaps O1
    op(&mut b, "O3", 4.5, 2.5, 2.0, 2.0); // sticks out of the end and the top
    op(&mut b, "O4", 2.0, 0.0, -1.0, 1.0); // invalid: ignored
    b.add("O5", "Opening", &[("host", r("W")), ("offset", Value::Text("x".into()))]);
    let m = Model::compile(&b.0);
    let w = &m.walls[0];
    let removed = (1.0 * 2.0 + 1.0 * 1.0 - 0.5 * 1.0) * 0.2 + 0.5 * 0.5 * 0.2;
    assert!((w.volume - (3.0 - removed)).abs() < 1e-12, "{}", w.volume);
    let sum: f64 = w.layers.iter().map(|l| l.volume).sum();
    assert!((sum - w.volume).abs() < 1e-12);
    check_closed(&m);
}

#[test]
fn unequal_l_join_and_three_way() {
    let mut b = B::new();
    base(&mut b, &[0.2]);
    b.add("WT2", "WallType", &[("layer_thickness", nl(&[0.3])), ("layer_material", Value::List(vec![Value::Text("b".into())]))]);
    wall(&mut b, "A", [0.0, 0.0, 4.0, 0.0], 3.0);
    b.add("B", "Wall", &[("level", r("L1")), ("type", r("WT2")), ("x1", n(4.0)), ("y1", n(3.0)), ("x2", n(4.0)), ("y2", n(0.0)), ("height", n(3.0))]);
    wall(&mut b, "C", [4.0, 0.0, 8.0, 0.0], 3.0);
    wall(&mut b, "D", [8.0, 0.0, 8.0, 0.0], 3.0); // zero length: no solid
    let m = Model::compile(&b.0);
    assert_eq!(m.walls.len(), 3);
    check_closed(&m);
}

#[test]
fn slabs_columns_beams_and_dangling() {
    let mut b = B::new();
    base(&mut b, &[0.2]);
    b.add("L2", "Level", &[("elevation", n(3.0))]);
    b.add("S", "Slab", &[("level", r("L1")), ("boundary", nl(&[0.0, 0.0, 6.0, 0.0, 6.0, 4.0, 3.0, 4.0, 3.0, 2.0, 0.0, 2.0])), ("thickness", n(0.2))]);
    b.add("H1", "SlabOpening", &[("host", r("S")), ("boundary", nl(&[1.0, 0.5, 2.0, 0.5, 2.0, 1.5, 1.0, 1.5]))]);
    b.add("H2", "SlabOpening", &[("host", r("S")), ("boundary", nl(&[4.0, 3.0, 5.0, 3.0, 5.0, 3.5, 4.0, 3.5]))]);
    b.add("S2", "Slab", &[("level", r("L2")), ("boundary", nl(&[0.0, 0.0, 4.0, 0.0, 4.0, 4.0, 0.0, 4.0])), ("thickness", n(0.2))]);
    b.add("H3", "SlabOpening", &[("host", r("S2")), ("boundary", nl(&[3.0, 1.0, 5.0, 1.0, 5.0, 2.0, 3.0, 2.0]))]);
    b.add("C", "Column", &[("base", r("L1")), ("top", r("L2")), ("x", n(9.0)), ("y", n(9.0)), ("section_b", n(0.3)), ("section_h", n(0.4))]);
    b.add("Bm", "Beam", &[("level", r("L2")), ("x1", n(9.0)), ("y1", n(9.0)), ("x2", n(12.0)), ("y2", n(11.0)), ("section_b", n(0.2)), ("section_h", n(0.5)), ("z_offset", n(0.1))]);
    b.add("X", "Wall", &[("level", r("nope")), ("type", r("WT")), ("x1", n(0.0)), ("y1", n(0.0)), ("x2", n(1.0)), ("y2", n(0.0)), ("height", n(3.0))]);
    let m = Model::compile(&b.0);
    let s = m.slabs.iter().find(|s| s.name == "S").unwrap();
    assert!((s.area - (18.0 - 1.0 - 0.5)).abs() < 1e-12, "{}", s.area);
    assert!((s.volume - s.area * 0.2).abs() < 1e-12);
    let s2 = m.slabs.iter().find(|s| s.name == "S2").unwrap();
    assert!((s2.area - 15.0).abs() < 1e-9, "{}", s2.area);
    assert!(m.walls.is_empty());
    for e in &m.elements {
        assert!(e.mesh.is_closed(), "{}", e.name);
    }
    let c = m.elements.iter().find(|e| e.name == "C").unwrap();
    assert!((c.mesh.volume() - 0.36).abs() < 1e-12);
    let bm = m.elements.iter().find(|e| e.name == "Bm").unwrap();
    let (lo, hi) = bm.mesh.bounds().unwrap();
    assert!((hi.z - 2.9).abs() < 1e-12 && (lo.z - 2.4).abs() < 1e-12);
    assert!((bm.mesh.volume() - 13f64.sqrt() * 0.1).abs() < 1e-9);
    let gltf = gltf::to_gltf(&m.elements);
    assert!(gltf.contains("\"extras\":{\"id\":\"Bm\""));
}

#[test]
fn same_side_walls_on_a_through_wall_stop_at_face_and_mitre() {
    // P and Q end at the same point of XT's interior, on the same side: both stop at XT's face
    // and are mitred against each other (equal thickness: along the bisector of their directions).
    let mut b = B::new();
    base(&mut b, &[0.2]);
    wall(&mut b, "XT", [-4.0, 0.0, 4.0, 0.0], 3.0);
    wall(&mut b, "P", [0.0, -3.0, 0.0, 0.0], 3.0);
    wall(&mut b, "Q", [-3.0, -3.0, 0.0, 0.0], 3.0);
    let m = Model::compile(&b.0);
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let (bx, by) = (-h, -1.0 - h);
    let get = |name: &str| m.walls.iter().find(|w| w.name == name).unwrap();
    for (name, sign) in [("P", 1.0f64), ("Q", -1.0)] {
        let w = get(name);
        assert_eq!(w.joins[1], Some(Join::T(EntityId::derive("XT"))));
        for p in &m.element(w.id).unwrap().mesh.positions {
            assert!(p.y <= -0.1 + 1e-12, "{name}: vertex {p:?} inside XT");
            assert!(sign * (bx * p.y - by * p.x) >= -1e-12, "{name}: vertex {p:?} across the mitre");
        }
    }
    assert!(get("P").volume < 2.9 * 0.2 * 3.0 - 1e-6, "P is mitred against Q");
    check_closed(&m);
}

#[test]
fn cross_junction_t_joins_take_precedence() {
    // X1 and X2 end at the same point on XT's interior: both T-join XT and stop at its faces
    // instead of L-joining each other through it.
    let mut b = B::new();
    base(&mut b, &[0.2]);
    wall(&mut b, "XT", [-4.0, 0.0, 4.0, 0.0], 3.0);
    wall(&mut b, "X1", [0.0, -3.0, 0.0, 0.0], 3.0);
    wall(&mut b, "X2", [0.0, 3.0, 0.0, 0.0], 3.0);
    let m = Model::compile(&b.0);
    let get = |name: &str| m.walls.iter().find(|w| w.name == name).unwrap();
    assert!((get("XT").volume - 8.0 * 0.2 * 3.0).abs() < 1e-12);
    for (name, ylim) in [("X1", -0.1f64), ("X2", 0.1)] {
        let w = get(name);
        assert!((w.volume - 2.9 * 0.2 * 3.0).abs() < 1e-12, "{name}: {}", w.volume);
        assert_eq!(w.joins[1], Some(Join::T(EntityId::derive("XT"))));
        // Every vertex on the far side of XT's facing face: no overlap with XT or each other.
        let e = m.element(w.id).unwrap();
        for p in &e.mesh.positions {
            assert!(p.y * ylim.signum() >= ylim.abs() - 1e-12, "{name}: vertex {p:?} inside XT");
        }
    }
    check_closed(&m);
}

// ------------------------------------------------------------------ curtain walls, stairs, ramps

fn curtain_wall(b: &mut B, name: &str, p: [f64; 4], height: f64, spacing: [f64; 2], mullion: [f64; 2], panel: f64) {
    b.add(name, "CurtainWall", &[
        ("level", r("L0")), ("x1", n(p[0])), ("y1", n(p[1])), ("x2", n(p[2])), ("y2", n(p[3])), ("height", n(height)),
        ("u_spacing", n(spacing[0])), ("v_spacing", n(spacing[1])),
        ("mullion_width", n(mullion[0])), ("mullion_depth", n(mullion[1])), ("panel_thickness", n(panel)),
    ]);
}

fn stair(b: &mut B, name: &str, at: [f64; 3], width: f64, risers: f64, tread: f64) {
    b.add(name, "Stair", &[
        ("base", r("L0")), ("top", r("L1")), ("x", n(at[0])), ("y", n(at[1])), ("angle", n(at[2])),
        ("width", n(width)), ("riser_count", n(risers)), ("tread", n(tread)),
    ]);
}

fn ramp(b: &mut B, name: &str, at: [f64; 3], width: f64, length: f64, top: &str) {
    b.add(name, "Ramp", &[
        ("base", r("L0")), ("top", r(top)), ("x", n(at[0])), ("y", n(at[1])), ("angle", n(at[2])),
        ("width", n(width)), ("length", n(length)),
    ]);
}

fn levels() -> B {
    let mut b = B::new();
    b.add("L0", "Level", &[("elevation", n(1.0))]).add("L1", "Level", &[("elevation", n(4.15))]);
    b
}

/// The mesh of `name` in the frame of a run from `(x, y)` along `angle` from `z0`: `(s, l, z)` per
/// vertex, s along the run, l to its left.
fn local(m: &Model, name: &str, origin: [f64; 2], angle: f64, z0: f64) -> Vec<[f64; 3]> {
    let el = m.elements.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("no element {name}"));
    let (c, s) = (angle.cos(), angle.sin());
    el.mesh.positions.iter().map(|p| {
        let (dx, dy) = (p.x - origin[0], p.y - origin[1]);
        [dx * c + dy * s, -dx * s + dy * c, p.z - z0]
    }).collect()
}

/// The (min, max) of one coordinate of `pts`.
fn span(pts: &[[f64; 3]], k: usize) -> (f64, f64) {
    pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p[k]), hi.max(p[k])))
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

#[test]
fn grid_lines_every_spacing_with_the_remainder_in_the_last_bay() {
    use crate::arch::grid;
    assert_eq!(grid(5.0, 1.5), [0.0, 1.5, 3.0, 4.5, 5.0]);
    assert_eq!(grid(6.0, 1.5), [0.0, 1.5, 3.0, 4.5, 6.0], "no zero-width last bay");
    assert_eq!(grid(1.2, 2.0), [0.0, 1.2], "a spacing beyond the length is one bay");
    assert_eq!(grid(3.0, 1.25), [0.0, 1.25, 2.5, 3.0]);
    assert_eq!(grid(3.0, 3.0), [0.0, 3.0]);
    // Lines are k·spacing, not accumulated: 0.1 · 3 is not 0.1 + 0.1 + 0.1.
    assert_eq!(grid(1.0, 0.1)[3], 3.0 * 0.1);
    // A last bay below a billionth of the spacing is absorbed by the one before, a longer one kept.
    assert_eq!(grid(3.0 + 1e-10, 1.0), [0.0, 1.0, 2.0, 3.0 + 1e-10]);
    assert_eq!(grid(3.0 + 2e-9, 1.0), [0.0, 1.0, 2.0, 3.0, 3.0 + 2e-9]);
    // Absurd ratios are clamped (the callers reject them) instead of allocating without bound.
    assert!(grid(1e300, 1e-300).len() < 100_000);
}

#[test]
fn curtain_wall_with_remainder_bays() {
    // 5 × 3 m wall, bays 1.5 × 1.25: 4 × 3 bays, the last ones 0.5 and 0.5 wide.
    let (w, d, pt, len, h) = (0.05, 0.15, 0.024, 5.0, 3.0);
    let mut b = levels();
    curtain_wall(&mut b, "cw", [1.0, 2.0, 5.0, 5.0], h, [1.5, 1.25], [w, d], pt);
    let m = Model::compile(&b.0);
    assert_eq!(m.curtain_walls.len(), 1);
    let c = &m.curtain_walls[0];
    assert_eq!((c.name.as_str(), c.panels), ("cw", 12));
    assert!(near(c.mullion_length, 5.0 * 3.0 + 4.0 * 5.0));
    // Mullions minus their crossings, plus the panels (spec closed form of the same union).
    let (nu, nv) = (4.0, 3.0);
    let panels = pt * (len - nu * w) * (h - nv * w);
    let expected = w * d * ((nu + 1.0) * h + (nv + 1.0) * len) - d * (nu * w) * (nv * w) + panels;
    let el = m.element(c.id).unwrap();
    assert_eq!(el.kind, "CurtainWall");
    assert!(near(el.mesh.volume(), expected), "{} vs {expected}", el.mesh.volume());
    assert!(near(c.volume, expected));
    check_closed(&m);
    // In the wall's frame: the end verticals stick out by half a mullion, so do the outer
    // horizontals above and below; the mullions are centred on the centreline.
    let pts = local(&m, "cw", [1.0, 2.0], (3.0f64).atan2(4.0), 1.0);
    let (s, l, z) = (span(&pts, 0), span(&pts, 1), span(&pts, 2));
    assert!(near(s.0, -w / 2.0) && near(s.1, len + w / 2.0), "{s:?}");
    assert!(near(l.0, -d / 2.0) && near(l.1, d / 2.0), "{l:?}");
    assert!(near(z.0, -w / 2.0) && near(z.1, h + w / 2.0), "{z:?}");
}

#[test]
fn curtain_wall_grids_exact_multiples_and_single_bays() {
    let mut b = levels();
    // 6 m / 1.5 and 3 m / 1: no zero-width last bay (4 × 3 bays, 39 m of mullion).
    curtain_wall(&mut b, "exact", [10.0, 0.0, 16.0, 0.0], 3.0, [1.5, 1.0], [0.06, 0.2], 0.03);
    // Spacing beyond the wall: one bay each way, pointing down the y axis.
    curtain_wall(&mut b, "one", [0.0, 10.0, 0.0, 8.8], 1.0, [2.0, 2.0], [0.05, 0.1], 0.02);
    // Thicker panel than mullion depth, and equal.
    curtain_wall(&mut b, "thick", [0.0, 0.0, 4.0, 0.0], 3.0, [1.0, 1.0], [0.05, 0.1], 0.3);
    curtain_wall(&mut b, "equal", [0.0, 5.0, 4.0, 5.0], 3.0, [1.0, 1.0], [0.05, 0.1], 0.1);
    let m = Model::compile(&b.0);
    let get = |name: &str| m.curtain_walls.iter().find(|c| c.name == name).unwrap();
    assert_eq!(get("exact").panels, 12);
    assert!(near(get("exact").mullion_length, 5.0 * 3.0 + 4.0 * 6.0));
    assert_eq!(get("one").panels, 1);
    assert!(near(get("one").mullion_length, 2.0 * 1.0 + 2.0 * 1.2));
    // 4 × 3 bays. Panels lie between the mullion faces, whatever their thickness (here 3 times the
    // mullion depth, so they stick out on both sides): the volumes just add.
    let (nu, nv, w, d, pt, len, h): (f64, f64, f64, f64, f64, f64, f64) = (4.0, 3.0, 0.05, 0.1, 0.3, 4.0, 3.0);
    let mullions = w * d * ((nu + 1.0) * h + (nv + 1.0) * len) - d * (nu * w) * (nv * w);
    let pan = pt * (len - nu * w) * (h - nv * w);
    assert!(near(m.element(get("thick").id).unwrap().mesh.volume(), mullions + pan));
    assert!(near(m.element(get("equal").id).unwrap().mesh.volume(), mullions + 0.1 * (len - nu * w) * (h - nv * w)));
    check_closed(&m);
    assert_eq!(m.curtain_walls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["equal", "exact", "one", "thick"]);
    // "one" runs along −y: its local frame has x towards −y, so the solid lies at x = 0 ± 0.05.
    let pts = local(&m, "one", [0.0, 10.0], (-1.2f64).atan2(0.0), 1.0);
    assert!(near(span(&pts, 0).1, 1.2 + 0.025) && near(span(&pts, 2).0, -0.025));
}

#[test]
fn curtain_wall_bays_no_wider_than_a_mullion_have_no_panels() {
    // Spacing 0.04 under the 0.05 mullions: the mullions merge into plates, no panel anywhere,
    // and the remainder bay (0.02 m) is covered too.
    let mut b = levels();
    curtain_wall(&mut b, "narrow", [0.0, 0.0, 0.5, 0.0], 0.5, [0.04, 0.04], [0.05, 0.1], 0.02);
    let m = Model::compile(&b.0);
    let c = &m.curtain_walls[0];
    assert_eq!(c.panels, 13 * 13);
    let el = m.element(c.id).unwrap();
    // A plus-shaped plate: (0.5 + 0.05) × 0.5 + 0.5 × (0.5 + 0.05) − 0.5 × 0.5, 0.1 thick.
    assert!(near(el.mesh.volume(), 0.1 * (0.55 * 0.5 + 0.5 * 0.55 - 0.25)), "{}", el.mesh.volume());
    check_closed(&m);
}

#[test]
fn curtain_wall_dimensions_at_the_snap_tolerance_never_give_a_pinched_mesh() {
    let wall = |w: f64| {
        let mut b = levels();
        curtain_wall(&mut b, "cw", [0.0, 0.0, 5.0, 0.0], 3.0, [1.5, 1.25], [w, 0.1], 0.02);
        Model::compile(&b.0)
    };
    // Mullions of 0.15 µm: their faces are snapped into the grid lines, which would make the
    // pieces touch along edges. The quantities stay, there is no solid.
    let m = wall(1.5e-7);
    assert_eq!(m.curtain_walls.len(), 1);
    assert_eq!((m.curtain_walls[0].panels, m.curtain_walls[0].volume), (12, 0.0));
    assert!(near(m.curtain_walls[0].mullion_length, 35.0) && m.elements.is_empty());
    // Mullions thinner than the tolerance vanish, and the panels, a nanometre apart, merge into one
    // plate: 5 m × 3 m × 0.02 m.
    let m = wall(1e-9);
    assert_eq!(m.elements.len(), 1);
    assert!((m.elements[0].mesh.volume() - 0.3).abs() < 1e-6, "{}", m.elements[0].mesh.volume());
    check_closed(&m);
    // A tenth of a micron more than the snap (twice) is a proper mullion again.
    let m = wall(3e-7);
    assert_eq!(m.elements.len(), 1);
    check_closed(&m);
}

#[test]
fn curtain_wall_is_not_built_from_bad_props_or_oversized_grids() {
    let ok = |b: &B| {
        let m = Model::compile(&b.0);
        (m.curtain_walls.len(), m.elements.len())
    };
    let one = |tweak: &dyn Fn(&mut B)| {
        let mut b = levels();
        curtain_wall(&mut b, "cw", [0.0, 0.0, 4.0, 0.0], 3.0, [1.0, 1.0], [0.05, 0.1], 0.02);
        tweak(&mut b);
        ok(&b)
    };
    let set = |b: &mut B, key: &str, v: Value| {
        b.0.apply(Op::Set { id: EntityId::derive("cw"), key: key.into(), value: v }).unwrap();
    };
    assert_eq!(one(&|_| {}), (1, 1));
    for key in ["height", "u_spacing", "v_spacing", "mullion_width", "mullion_depth", "panel_thickness"] {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(one(&|b| set(b, key, n(bad))), (0, 0), "{key} = {bad}");
        }
        assert_eq!(one(&|b| set(b, key, Value::Text("x".into()))), (0, 0), "{key} as text");
    }
    assert_eq!(one(&|b| set(b, "x2", n(0.0))), (0, 0), "zero length");
    assert_eq!(one(&|b| set(b, "x2", n(1e-10))), (0, 0), "length under 1e-9");
    assert_eq!(one(&|b| set(b, "y1", n(f64::NAN))), (0, 0));
    assert_eq!(one(&|b| set(b, "level", r("nope"))), (0, 0), "dangling level");
    assert_eq!(one(&|b| set(b, "level", r("cw"))), (0, 0), "level that is no Level");
    // Limits: 256 × 256 bays are built, one bay more is not, and absurd ratios cost nothing.
    assert_eq!(one(&|b| { set(b, "x2", n(256.0)); set(b, "height", n(256.0)); }).0, 1);
    assert_eq!(one(&|b| { set(b, "x2", n(257.0)); set(b, "height", n(256.0)); }), (0, 0));
    assert_eq!(one(&|b| set(b, "u_spacing", n(1e-300))), (0, 0));
    assert_eq!(one(&|b| set(b, "v_spacing", n(5e-324))), (0, 0));
    assert_eq!(one(&|b| set(b, "x2", n(1e300))), (0, 0), "a wall of 1e300 m has 1e300 bays");
    assert_eq!(one(&|b| { set(b, "x2", n(f64::MAX)); set(b, "x1", n(-f64::MAX)); }), (0, 0), "length overflows");
}

#[test]
fn stairs_have_exact_risers_and_a_stepped_solid() {
    let mut b = levels();
    stair(&mut b, "axis", [20.0, 5.0, 0.0], 1.2, 18.0, 0.28);
    stair(&mut b, "skew", [-4.0, 3.0, std::f64::consts::FRAC_PI_6], 1.0, 7.0, 0.3);
    stair(&mut b, "north", [0.0, 0.0, std::f64::consts::FRAC_PI_2], 0.9, 1.0, 0.25);
    let m = Model::compile(&b.0);
    assert_eq!(m.stairs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["axis", "north", "skew"]);
    let get = |name: &str| m.stairs.iter().find(|s| s.name == name).unwrap();
    let (rise, risers) = (4.15 - 1.0, 18.0);
    let s = get("axis");
    assert_eq!((s.risers, s.tread), (18, 0.28));
    assert_eq!(s.riser, rise / risers);
    // Block k is 1.2 wide, 0.28 deep and k risers high: W·t·r·n(n+1)/2.
    let volume = 1.2 * 0.28 * (rise / risers) * risers * (risers + 1.0) / 2.0;
    assert!(near(s.volume, volume) && near(m.element(s.id).unwrap().mesh.volume(), volume));
    check_closed(&m);
    // Placement: the run starts at (x, y) and goes along `angle`, centred on the run line.
    for (name, at, angle, width, count, tread) in [
        ("axis", [20.0, 5.0], 0.0, 1.2, 18.0, 0.28),
        ("skew", [-4.0, 3.0], std::f64::consts::FRAC_PI_6, 1.0, 7.0, 0.3),
        ("north", [0.0, 0.0], std::f64::consts::FRAC_PI_2, 0.9, 1.0, 0.25),
    ] {
        let pts = local(&m, name, at, angle, 1.0);
        let (s, l, z) = (span(&pts, 0), span(&pts, 1), span(&pts, 2));
        assert!(s.0.abs() < 1e-9 && near(s.1, count * tread), "{name}: {s:?}");
        assert!(near(l.0, -width / 2.0) && near(l.1, width / 2.0), "{name}: {l:?}");
        assert!(z.0.abs() < 1e-9 && near(z.1, rise), "{name}: {z:?}");
    }
    // The single riser is a plain box: 0.9 × 0.25 × 3.15, 12 triangles.
    let single = m.element(get("north").id).unwrap();
    assert_eq!(single.mesh.triangles.len(), 12);
    assert!(near(single.mesh.volume(), 0.9 * 0.25 * rise));
    assert_eq!(get("north").riser, rise);
    // Heading north: x ≈ 0 ± 0.45, y from 0 to 0.25.
    let (lo, hi) = single.mesh.bounds().unwrap();
    assert!(near(lo.x, -0.45) && near(hi.x, 0.45) && lo.y.abs() < 1e-9 && near(hi.y, 0.25));
}

#[test]
fn stairs_are_not_built_from_bad_props() {
    let one = |tweak: &dyn Fn(&mut B)| {
        let mut b = levels();
        stair(&mut b, "st", [0.0, 0.0, 0.0], 1.0, 10.0, 0.3);
        tweak(&mut b);
        let m = Model::compile(&b.0);
        (m.stairs.len(), m.elements.len())
    };
    let set = |b: &mut B, key: &str, v: Value| {
        b.0.apply(Op::Set { id: EntityId::derive("st"), key: key.into(), value: v }).unwrap();
    };
    assert_eq!(one(&|_| {}), (1, 1));
    for (key, bad) in [
        ("riser_count", 0.0), ("riser_count", -3.0), ("riser_count", 2.5), ("riser_count", 1001.0), ("riser_count", 1e300),
        ("riser_count", f64::NAN), ("width", 0.0), ("width", -1.0), ("tread", 0.0), ("tread", -0.3), ("x", f64::INFINITY),
        ("angle", f64::NAN),
    ] {
        assert_eq!(one(&|b| set(b, key, n(bad))), (0, 0), "{key} = {bad}");
    }
    assert_eq!(one(&|b| set(b, "top", r("L0"))), (0, 0), "top level = base level");
    assert_eq!(one(&|b| set(b, "top", r("nope"))), (0, 0), "dangling top");
    assert_eq!(one(&|b| set(b, "base", r("st"))), (0, 0), "base that is no Level");
    assert_eq!(one(&|b| set(b, "tread", Value::Text("x".into()))), (0, 0));
    // A downward stair (top below base) is not built either.
    assert_eq!(one(&|b| b.0.apply(Op::Set { id: EntityId::derive("L1"), key: "elevation".into(), value: n(0.0) }).unwrap()), (0, 0));
    // The limit itself is built.
    assert_eq!(one(&|b| set(b, "riser_count", n(1000.0))), (1, 1));
}

#[test]
fn ramps_are_wedges_rising_along_the_run() {
    let mut b = levels();
    b.add("L2", "Level", &[("elevation", n(2.0))]);
    ramp(&mut b, "r", [30.0, -2.0, std::f64::consts::FRAC_PI_2], 1.5, 14.4, "L1");
    ramp(&mut b, "flat", [0.0, 0.0, 0.0], 1.0, 5.0, "L0");
    ramp(&mut b, "down", [0.0, 0.0, 0.0], 1.0, 5.0, "nope");
    b.0.apply(Op::Create { id: EntityId::derive("L-1"), kind: "Level".into() }).unwrap();
    b.0.apply(Op::Set { id: EntityId::derive("L-1"), key: "elevation".into(), value: n(0.0) }).unwrap();
    ramp(&mut b, "below", [0.0, 0.0, 0.0], 1.0, 5.0, "L-1");
    let m = Model::compile(&b.0);
    assert_eq!(m.elements.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["r"]);
    assert!(m.stairs.is_empty() && m.curtain_walls.is_empty());
    let e = &m.elements[0];
    assert_eq!(e.kind, "Ramp");
    let rise = 4.15 - 1.0;
    assert!(near(e.mesh.volume(), 1.5 * 14.4 * rise / 2.0), "{}", e.mesh.volume());
    assert_eq!((e.mesh.positions.len(), e.mesh.triangles.len()), (6, 8));
    check_closed(&m);
    // Running north from (30, -2): x within 30 ± 0.75, y from -2 to 12.4; the top is at the far end.
    let pts = local(&m, "r", [30.0, -2.0], std::f64::consts::FRAC_PI_2, 1.0);
    let (s, l, z) = (span(&pts, 0), span(&pts, 1), span(&pts, 2));
    assert!(s.0.abs() < 1e-9 && near(s.1, 14.4) && near(l.0, -0.75) && near(l.1, 0.75) && z.0.abs() < 1e-9 && near(z.1, rise));
    for p in &pts {
        assert!(near(p[2].max(0.0), rise * p[0] / 14.4) || p[2].abs() < 1e-9, "{p:?} is neither on the slope nor on the underside");
    }
}

// ------------------------------------------------------------------ randomised checks

/// xorshift64*: a deterministic stream of numbers in `[0, 1)`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next() * items.len() as f64) as usize]
    }
}

/// Length of `⋃ [s − w/2, s + w/2]` over the increasing `lines`, clipped to `[lo, hi]`.
fn strip_union(lines: &[f64], w: f64, lo: f64, hi: f64) -> f64 {
    let (mut total, mut cursor) = (0.0, lo);
    for &s in lines {
        let (a, b) = ((s - w / 2.0).max(lo), (s + w / 2.0).min(hi));
        if b > a.max(cursor) {
            total += b - a.max(cursor);
            cursor = b;
        }
    }
    total
}

/// The volume of a curtain wall computed from its definition, not from its mesh: the xz regions of
/// the mullions and of the panels are disjoint, so the volumes of the two prisms just add.
fn curtain_wall_volume(len: f64, h: f64, spacing: [f64; 2], w: f64, d: f64, pt: f64) -> f64 {
    let (us, vs) = (crate::arch::grid(len, spacing[0]), crate::arch::grid(h, spacing[1]));
    let mullions = strip_union(&us, w, f64::MIN, f64::MAX) * h + len * strip_union(&vs, w, f64::MIN, f64::MAX)
        - strip_union(&us, w, 0.0, len) * strip_union(&vs, w, 0.0, h);
    let bays = |lines: &[f64]| lines.windows(2).map(|b| (b[1] - b[0] - w).max(0.0)).sum::<f64>();
    d * mullions + pt * bays(&us) * bays(&vs)
}

#[test]
fn random_curtain_walls_stairs_and_ramps_are_closed_manifolds_with_exact_volumes() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut tested = 0;
    for case in 0..120 {
        let mut b = levels();
        // Bays from far wider than a mullion to narrower than one; thick and thin panels.
        let w = rng.range(0.02, 0.2);
        let (d, pt) = (rng.range(0.005, 0.4), rng.range(0.002, 0.5));
        let spacing = [rng.range(0.5 * w, 3.0), rng.range(0.5 * w, 3.0)];
        let (len, h) = (rng.range(0.1, 12.0), rng.range(0.1, 8.0));
        let (nu, nv) = (crate::arch::grid(len, spacing[0]).len() as u64 - 1, crate::arch::grid(h, spacing[1]).len() as u64 - 1);
        if nu * nv > 250 {
            continue; // fine grids are valid but slow to check in a debug build
        }
        tested += 1;
        let angle = rng.range(-7.0, 7.0);
        let (x1, y1) = (rng.range(-50.0, 50.0), rng.range(-50.0, 50.0));
        curtain_wall(&mut b, "cw", [x1, y1, x1 + len * angle.cos(), y1 + len * angle.sin()], h, spacing, [w, d], pt);
        let risers = (rng.range(1.0, 60.0)).floor();
        stair(&mut b, "st", [x1, y1, angle], rng.range(0.2, 3.0), risers, rng.range(0.05, 0.6));
        ramp(&mut b, "rp", [x1, y1, angle], rng.range(0.2, 3.0), rng.range(0.2, 20.0), "L1");
        let m = Model::compile(&b.0);
        check_closed(&m);
        assert_eq!(m.elements.len(), 3, "case {case}");

        let c = &m.curtain_walls[0];
        let expected = curtain_wall_volume(len, h, spacing, w, d, pt);
        let mesh_volume = m.element(c.id).unwrap().mesh.volume();
        assert!((mesh_volume - expected).abs() <= 1e-9 * expected, "case {case}: {mesh_volume} vs {expected}");
        assert!((c.volume - expected).abs() <= 1e-9 * expected);
        assert_eq!(c.panels, nu * nv);
        assert!(near(c.mullion_length, (nu + 1) as f64 * h + (nv + 1) as f64 * len));

        let s = &m.stairs[0];
        let t = s.tread;
        let width = match &b.0.get(EntityId::derive("st")).unwrap().props["width"] {
            Value::Num(x) => *x,
            _ => unreachable!(),
        };
        let expected = width * t * s.riser * risers * (risers + 1.0) / 2.0;
        assert!((m.element(s.id).unwrap().mesh.volume() - expected).abs() <= 1e-9 * expected, "case {case}");
        assert_eq!(s.risers as f64, risers);
        assert_eq!(s.riser, (4.15 - 1.0) / risers);
    }
    assert!(tested >= 60, "only {tested} of 120 random cases were small enough to check");
}

#[test]
fn compiling_arbitrary_curtain_wall_stair_and_ramp_props_never_panics() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let nums = [
        0.0, -0.0, 1.0, -1.0, 0.5, 2.5, 3.0, 18.0, 1001.0, 1e-12, 1e-9, 1e-7, 1e-300, 5e-324, 1e9, 1e300,
        f64::MAX, f64::MIN, f64::NAN, f64::INFINITY, f64::NEG_INFINITY,
    ];
    let keys = [
        ("CurtainWall", ["level", "x1", "y1", "x2", "y2", "height", "u_spacing", "v_spacing", "mullion_width", "mullion_depth", "panel_thickness"]),
        ("Stair", ["base", "top", "x", "y", "angle", "width", "riser_count", "tread", "", "", ""]),
        ("Ramp", ["base", "top", "x", "y", "angle", "width", "length", "", "", "", ""]),
    ];
    for case in 0..400 {
        let mut b = levels();
        let (kind, props) = keys[case % 3];
        let mut set: Vec<(&str, Value)> = Vec::new();
        for key in props.iter().filter(|k| !k.is_empty()) {
            let is_ref = matches!(*key, "level" | "base" | "top");
            let v = match (rng.next() * 12.0) as u32 {
                0 => continue, // missing
                1 => Value::Text("x".into()),
                2 => Value::Bool(true),
                3 => Value::List(vec![n(1.0)]),
                4 => r("nope"),
                5 | 6 if is_ref => r("L0"),
                7 | 8 if is_ref => r("L1"),
                _ if is_ref => r(rng.pick(&["L0", "L1"])),
                // Mostly modest numbers, so that some elements really are built.
                9 => n(rng.pick(&nums)),
                _ => n(rng.range(0.05, 6.0)),
            };
            set.push((key, v));
        }
        b.add("e", kind, &set);
        let m = Model::compile(&b.0);
        check_closed(&m);
        for c in &m.curtain_walls {
            assert!(c.mullion_length.is_finite() && c.panels >= 1);
        }
        for s in &m.stairs {
            assert!(s.riser.is_finite() && s.riser > 0.0 && s.risers >= 1);
        }
    }
}

// ------------------------------------------------------------------ MEP segments

fn port(b: &mut B, name: &str, p: [f64; 3]) {
    b.add(name, "Port", &[("x", n(p[0])), ("y", n(p[1])), ("z", n(p[2])), ("kind", Value::Text("junction".into()))]);
}

#[test]
fn segments_are_elements_named_after_the_entity() {
    let mut b = B::new();
    port(&mut b, "pa", [2.0, -1.0, 1.5]);
    port(&mut b, "pb", [2.0, 1.0, 1.5]);
    port(&mut b, "pc", [4.0, 0.0, 1.0]);
    // Neither a system nor a roughness nor any hydraulics is needed for a solid.
    b.add("P1", "Segment", &[("from", r("pa")), ("to", r("pb")), ("shape", Value::Text("round".into())), ("diameter", n(0.1))]);
    b.add("D1", "Segment", &[
        ("from", r("pa")), ("to", r("pc")), ("shape", Value::Text("rect".into())), ("width", n(0.4)), ("height", n(0.3)),
    ]);
    let m = Model::compile(&b.0);
    assert_eq!(m.elements.iter().map(|e| (e.name.as_str(), e.kind.as_str())).collect::<Vec<_>>(), [("D1", "Segment"), ("P1", "Segment")]);
    let p1 = m.elements.iter().find(|e| e.name == "P1").unwrap();
    assert_eq!(p1.id, EntityId::derive("P1"));
    assert!(p1.mesh.is_closed());
    let (lo, hi) = p1.mesh.bounds().unwrap();
    assert!((lo.y + 1.0).abs() < 1e-12 && (hi.y - 1.0).abs() < 1e-12, "runs between the ports");
    let a16 = 8.0 * (std::f64::consts::PI / 8.0).sin() * 0.05 * 0.05;
    assert!((p1.mesh.volume() - a16 * 2.0).abs() < 1e-12);
    let d1 = m.elements.iter().find(|e| e.name == "D1").unwrap();
    let len = (4.0f64 + 1.0 + 0.25).sqrt();
    assert!((d1.mesh.volume() - 0.4 * 0.3 * len).abs() < 1e-12);
    check_closed(&m);
    // Segments leave the quantities of the architectural trade alone.
    assert!(m.walls.is_empty() && m.slabs.is_empty() && m.stairs.is_empty() && m.curtain_walls.is_empty());
}

#[test]
fn a_segment_without_a_usable_description_has_no_solid() {
    let mut b = B::new();
    port(&mut b, "pa", [0.0, 0.0, 0.0]);
    port(&mut b, "pb", [1.0, 0.0, 0.0]);
    b.add("nowhere", "Wall", &[]); // not a Port
    b.add("pn", "Port", &[("x", n(1.0)), ("y", n(1.0))]); // no z
    let round = |b: &mut B, name: &str, from: &str, to: &str, extra: &[(&str, Value)]| {
        let mut props = vec![("from", r(from)), ("to", r(to)), ("shape", Value::Text("round".into()))];
        props.extend(extra.iter().cloned());
        b.add(name, "Segment", &props);
    };
    round(&mut b, "ok", "pa", "pb", &[("diameter", n(0.05))]);
    round(&mut b, "no_diameter", "pa", "pb", &[]);
    round(&mut b, "zero_diameter", "pa", "pb", &[("diameter", n(0.0))]);
    round(&mut b, "dangling", "pa", "gone", &[("diameter", n(0.05))]);
    round(&mut b, "not_a_port", "pa", "nowhere", &[("diameter", n(0.05))]);
    round(&mut b, "no_z", "pa", "pn", &[("diameter", n(0.05))]);
    round(&mut b, "same_place", "pa", "pa", &[("diameter", n(0.05))]);
    b.add("unknown_shape", "Segment", &[("from", r("pa")), ("to", r("pb")), ("shape", Value::Text("oval".into())), ("diameter", n(0.05))]);
    b.add("no_shape", "Segment", &[("from", r("pa")), ("to", r("pb")), ("diameter", n(0.05))]);
    b.add("rect_no_height", "Segment", &[("from", r("pa")), ("to", r("pb")), ("shape", Value::Text("rect".into())), ("width", n(0.3))]);
    b.add("no_from", "Segment", &[("to", r("pb")), ("shape", Value::Text("round".into())), ("diameter", n(0.05))]);
    let m = Model::compile(&b.0);
    assert_eq!(m.elements.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["ok"]);
}

#[test]
fn moving_a_port_moves_the_segment() {
    let mut b = B::new();
    port(&mut b, "pa", [0.0, 0.0, 0.0]);
    port(&mut b, "pb", [1.0, 0.0, 0.0]);
    b.add("s", "Segment", &[("from", r("pa")), ("to", r("pb")), ("shape", Value::Text("round".into())), ("diameter", n(0.1))]);
    b.0.apply(bim_doc::Op::Set { id: EntityId::derive("pb"), key: "x".into(), value: n(3.0) }).unwrap();
    let m = Model::compile(&b.0);
    assert!((m.elements[0].mesh.bounds().unwrap().1.x - 3.0).abs() < 1e-12);
}

// ------------------------------------------------------------------ clashes

fn t(s: &str) -> Value {
    Value::Text(s.into())
}

fn clashes(b: &B, clearance: f64) -> clash::ClashReport {
    clash::clash(&b.0, &Model::compile(&b.0), clearance)
}

fn hard_names(r: &clash::ClashReport) -> Vec<(&str, &str)> {
    r.hard.iter().map(|c| (c.a.as_str(), c.b.as_str())).collect()
}

fn soft_names(r: &clash::ClashReport) -> Vec<(&str, &str)> {
    r.soft.iter().map(|c| (c.a.as_str(), c.b.as_str())).collect()
}

/// A column of `size` × `size` centred on (x, y) between the elevations of levels `base` and `top`.
fn column_at(b: &mut B, name: &str, x: f64, y: f64, size: f64, base: &str, top: &str) {
    b.add(name, "Column", &[
        ("x", n(x)), ("y", n(y)), ("base", r(base)), ("top", r(top)), ("section_b", n(size)), ("section_h", n(size)),
    ]);
}

/// Levels `Z0` (0 m), `Z1` (1 m), `Z2` (2 m), `Z3` (3 m), `Z4` (4 m).
fn five_levels() -> B {
    let mut b = B::new();
    for k in 0..5 {
        b.add(&format!("Z{k}"), "Level", &[("elevation", n(k as f64))]);
    }
    b
}

/// A segment between two new ports (`<name>a`, `<name>b`), `dims` = [diameter] or [width, height].
fn segment_at(b: &mut B, name: &str, from: [f64; 3], to: [f64; 3], dims: &[f64]) {
    port(b, &format!("{name}a"), from);
    port(b, &format!("{name}b"), to);
    let mut props = vec![("from", r(&format!("{name}a"))), ("to", r(&format!("{name}b")))];
    if let [d] = dims {
        props.extend([("shape", t("round")), ("diameter", n(*d))]);
    } else {
        props.extend([("shape", t("rect")), ("width", n(dims[0])), ("height", n(dims[1]))]);
    }
    b.add(name, "Segment", &props);
}

#[test]
fn overlapping_boxes_clash_by_the_exact_common_volume() {
    let mut b = five_levels();
    column_at(&mut b, "A", 0.0, 0.0, 1.0, "Z0", "Z3");
    column_at(&mut b, "B", 0.5, 0.25, 1.0, "Z1", "Z4"); // overlaps A in 0.5 × 0.75 × 2
    column_at(&mut b, "C", 7.0, 0.0, 1.0, "Z0", "Z3"); // far away
    let r = clashes(&b, 0.0);
    assert_eq!(hard_names(&r), [("A", "B")]);
    assert!((r.hard[0].volume - 0.5 * 0.75 * 2.0).abs() < 1e-14, "{}", r.hard[0].volume);
    assert!(r.soft.is_empty());
    assert_eq!(r.matrix, std::collections::BTreeMap::from([("Column".to_string(), std::collections::BTreeMap::from([("Column".to_string(), 1)]))]));
    // The default clearance is none: the same report with it.
    assert_eq!(clashes(&b, clash::DEFAULT_CLEARANCE), r);
    // Clearance does not change what is hard.
    let wide = clashes(&b, 100.0);
    assert_eq!(hard_names(&wide), [("A", "B")]);
    assert_eq!(wide.hard, r.hard);
    // Soft clashes join it: C is 5.5 m from B, 6 m from A.
    assert_eq!(soft_names(&clashes(&b, 6.1)), [("A", "C"), ("B", "C")]);
    assert!(clashes(&b, 6.0).soft.iter().all(|c| c.b != "C" || c.a != "A"), "6 m apart is not closer than 6 m");
    assert_eq!(soft_names(&clashes(&b, 5.6)), [("B", "C")]);
}

#[test]
fn touching_solids_are_soft_at_distance_zero_but_never_hard() {
    let mut b = five_levels();
    column_at(&mut b, "A", 0.0, 0.0, 1.0, "Z0", "Z2");
    column_at(&mut b, "Face", 1.0, 0.0, 1.0, "Z0", "Z2"); // shares the face x = 0.5 with A
    column_at(&mut b, "Edge", 1.0, 1.0, 1.0, "Z0", "Z2"); // shares the edge x = 0.5, y = 0.5 with A
    column_at(&mut b, "Top", 0.0, 0.0, 1.0, "Z2", "Z4"); // rests on A's top face
    let r = clashes(&b, 0.0);
    assert!(r.hard.is_empty() && r.soft.is_empty() && r.matrix.is_empty());
    let r = clashes(&b, 0.01);
    assert!(r.hard.is_empty());
    for c in &r.soft {
        assert_eq!(c.distance, 0.0, "{c:?}");
    }
    // Every pair touches: Face and Edge share a face, Top meets Face along an edge and Edge at a corner.
    assert_eq!(soft_names(&r), [("A", "Edge"), ("A", "Face"), ("A", "Top"), ("Edge", "Face"), ("Edge", "Top"), ("Face", "Top")]);
}

#[test]
fn soft_clashes_report_the_exact_distance_and_use_a_strict_clearance() {
    let mut b = five_levels();
    column_at(&mut b, "D4", 0.0, 0.0, 0.2, "Z0", "Z3");
    column_at(&mut b, "D5", 0.24, 0.0, 0.2, "Z0", "Z3"); // 0.04 off
    column_at(&mut b, "Far", 5.0, 5.0, 0.2, "Z0", "Z3");
    let d = |r: &clash::ClashReport| r.soft.iter().map(|c| c.distance).collect::<Vec<_>>();
    let r = clashes(&b, 0.05);
    assert_eq!(soft_names(&r), [("D4", "D5")]);
    assert!((r.soft[0].distance - 0.04).abs() < 1e-14, "{:?}", d(&r));
    assert!(clashes(&b, 0.035).soft.is_empty());
    assert!(clashes(&b, 0.04 - 1e-9).soft.is_empty());
    assert_eq!(clashes(&b, 0.04 + 1e-9).soft.len(), 1);
    // A clearance that is not a positive number means none.
    for bad in [0.0, -1.0, f64::NAN, f64::NEG_INFINITY] {
        assert!(clashes(&b, bad).soft.is_empty(), "{bad}");
    }
    // Infinite clearance is not a distance either (treated as none rather than looping).
    assert!(clashes(&b, f64::INFINITY).soft.is_empty());
    // Diagonal gaps are Euclidean, not per axis: D5's corner (0.34, 0.1) to Dg's (0.38, 0.13) is 0.05 away.
    column_at(&mut b, "Dg", 0.48, 0.23, 0.2, "Z0", "Z3");
    let r = clashes(&b, 0.06);
    let diag = r.soft.iter().find(|c| c.a == "D5" && c.b == "Dg").unwrap();
    assert!((diag.distance - 0.05).abs() < 1e-14, "{}", diag.distance);
    assert!(clashes(&b, 0.045).soft.iter().all(|c| c.b != "Dg"), "0.05 apart along the diagonal, though 0.04 and 0.03 per axis");
}

#[test]
fn an_element_wholly_inside_another_clashes_by_its_whole_volume() {
    let mut b = five_levels();
    column_at(&mut b, "Big", 0.0, 0.0, 2.0, "Z0", "Z4");
    column_at(&mut b, "Small", 0.1, 0.1, 0.5, "Z1", "Z3"); // no surfaces meet
    let r = clashes(&b, 0.0);
    assert_eq!(hard_names(&r), [("Big", "Small")]);
    assert!((r.hard[0].volume - 0.5 * 0.5 * 2.0).abs() < 1e-14, "{}", r.hard[0].volume);
    // An identical twin clashes by its whole volume too.
    column_at(&mut b, "Twin", 0.1, 0.1, 0.5, "Z1", "Z3");
    let r = clashes(&b, 0.0);
    assert_eq!(hard_names(&r), [("Big", "Small"), ("Big", "Twin"), ("Small", "Twin")]);
    assert!(r.hard.iter().all(|c| (c.volume - 0.5).abs() < 1e-14));
}

#[test]
fn only_a_part_of_a_multi_part_element_inside_another_still_clashes() {
    // A curtain wall-like mesh of two separate parts: one part inside the big column, one outside.
    let mut b = five_levels();
    column_at(&mut b, "Big", 0.0, 0.0, 4.0, "Z0", "Z4");
    b.add("S", "Stair", &[("base", r("Z0")), ("top", r("Z2")), ("x", n(0.0)), ("y", n(0.0)), ("angle", n(0.0)), ("width", n(1.0)), ("riser_count", n(2.0)), ("tread", n(0.25))]);
    let m = Model::compile(&b.0);
    assert!(m.element(EntityId::derive("S")).is_some());
    let r = clash::clash(&b.0, &m, 0.0);
    // The stair (0.5 × 1 plan, up to 1 m) is inside the 4 × 4 × 4 column: its whole volume.
    assert_eq!(hard_names(&r), [("Big", "S")]);
    assert!((r.hard[0].volume - m.stairs[0].volume).abs() < 1e-12);
}

#[test]
fn pipes_and_ducts_through_walls_columns_and_each_other() {
    let a16 = 8.0 * (std::f64::consts::PI / 8.0).sin() * 0.05 * 0.05;
    let mut b = B::new();
    base(&mut b, &[0.1, 0.1]);
    b.add("L2", "Level", &[("elevation", n(3.0))]);
    wall(&mut b, "W1", [0.0, 0.0, 6.0, 0.0], 3.0);
    wall(&mut b, "W2", [6.0, 0.0, 6.0, 4.0], 3.0);
    b.add("O1", "Opening", &[("host", r("W1")), ("kind", t("window")), ("offset", n(4.6)), ("sill", n(0.5)), ("width", n(0.8)), ("height", n(1.5))]);
    column_at(&mut b, "C1", 8.0, 2.0, 0.3, "L1", "L2");
    segment_at(&mut b, "P1", [2.0, -1.0, 1.5], [2.0, 1.0, 1.5], &[0.1]); // through W1
    segment_at(&mut b, "D1", [4.0, -1.0, 1.0], [4.0, 1.0, 1.0], &[0.4, 0.3]); // through W1
    segment_at(&mut b, "P2", [5.0, -1.0, 1.2], [5.0, 1.0, 1.2], &[0.1]); // through the window: no clash
    segment_at(&mut b, "D2", [1.0, 0.23, 2.5], [3.0, 0.23, 2.5], &[0.2, 0.2]); // 0.03 off W1
    segment_at(&mut b, "P5", [1.0, 0.18, 0.5], [3.0, 0.18, 0.5], &[0.1]); // 0.03 off W1
    segment_at(&mut b, "P3", [7.0, 2.0, 1.0], [9.0, 2.0, 1.0], &[0.1]); // through C1
    segment_at(&mut b, "D3", [10.0, 0.0, 1.0], [10.0, 4.0, 1.0], &[0.4, 0.4]);
    segment_at(&mut b, "P4", [9.5, 2.0, 1.0], [10.5, 2.0, 1.0], &[0.1]); // through D3
    segment_at(&mut b, "D4", [12.0, 0.0, 1.0], [12.0, 4.0, 1.0], &[0.2, 0.2]);
    segment_at(&mut b, "D5", [12.24, 0.0, 1.0], [12.24, 4.0, 1.0], &[0.2, 0.2]); // 0.04 off D4
    let m = Model::compile(&b.0);
    // Every solid is closed by indices, the precondition of the volume test.
    for e in &m.elements {
        assert!(e.mesh.is_closed(), "{}", e.name);
    }
    assert_eq!(m.elements.len(), 2 + 1 + 10);

    let r = clash::clash(&b.0, &m, 0.05);
    assert_eq!(hard_names(&r), [("C1", "P3"), ("D1", "W1"), ("D3", "P4"), ("P1", "W1")]);
    let want = [a16 * 0.3, 0.4 * 0.3 * 0.2, a16 * 0.4, a16 * 0.2];
    for (c, w) in r.hard.iter().zip(want) {
        assert!((c.volume - w).abs() < 1e-14, "{}/{}: {} vs {w}", c.a, c.b, c.volume);
    }
    assert_eq!(soft_names(&r), [("D2", "W1"), ("D4", "D5"), ("P5", "W1")]);
    for (c, w) in r.soft.iter().zip([0.03, 0.04, 0.03]) {
        assert!((c.distance - w).abs() < 1e-12, "{}/{}: {}", c.a, c.b, c.distance);
    }
    // W1 and W2 are joined: not reported though they touch; the pipe through the window is clear.
    assert!(!hard_names(&r).contains(&("W1", "W2")) && !soft_names(&r).contains(&("W1", "W2")));
    assert!(!hard_names(&r).iter().chain(&soft_names(&r)).any(|(a, b)| *a == "P2" || *b == "P2"));
    // Matrix: Segment/Wall 2, Column/Segment 1, Segment/Segment 1.
    let count = |a: &str, b: &str| r.matrix.get(a).and_then(|row| row.get(b)).copied().unwrap_or(0);
    assert_eq!((count("Segment", "Wall"), count("Column", "Segment"), count("Segment", "Segment")), (2, 1, 1));
    assert_eq!(count("Wall", "Wall") + count("Column", "Wall") + count("Segment", "Column"), 0);
    assert_eq!(r.matrix.values().flat_map(|row| row.values()).sum::<u64>(), 4);
    // A tighter clearance drops the 0.04 pair.
    assert_eq!(soft_names(&clashes(&b, 0.035)), [("D2", "W1"), ("P5", "W1")]);
    // Without the clashing pipes nothing is hard, with soft clashes remaining.
    for s in ["P1", "D1", "P3", "P4"] {
        b.0.apply(bim_doc::Op::Delete { id: EntityId::derive(s) }).unwrap();
    }
    let r = clashes(&b, 0.05);
    assert!(r.hard.is_empty() && r.matrix.is_empty());
    assert_eq!(soft_names(&r), [("D2", "W1"), ("D4", "D5"), ("P5", "W1")]);
}

#[test]
fn joined_walls_are_excluded_and_unjoined_ones_are_not() {
    let mut b = B::new();
    base(&mut b, &[0.2]);
    wall(&mut b, "A", [0.0, 0.0, 4.0, 0.0], 3.0);
    wall(&mut b, "B", [4.0, 0.0, 4.0, 3.0], 3.0); // L-join with A
    wall(&mut b, "T", [2.0, 0.0, 2.0, -3.0], 3.0); // T-join: ends on A's interior
    wall(&mut b, "X1", [10.0, -3.0, 10.0, 0.0], 3.0); // a cross junction on XT: X1, X2 T-join XT
    wall(&mut b, "X2", [10.0, 3.0, 10.0, 0.0], 3.0);
    wall(&mut b, "XT", [6.0, 0.0, 14.0, 0.0], 3.0);
    for clearance in [0.0, 0.5] {
        let r = clashes(&b, clearance);
        assert!(r.hard.is_empty(), "{:?}", r.hard);
        // Only pairs of walls that are not joined may be soft: A–XT, B–XT lie apart by ≥ 2 m.
        assert!(r.soft.is_empty(), "{:?}", r.soft);
    }
    // Two walls crossing in their interiors are not joined: the crossing is a clash.
    wall(&mut b, "Y1", [12.0, -1.0, 12.0, 1.0], 3.0);
    let r = clashes(&b, 0.0);
    // Y1 ends nowhere on XT's centreline: it crosses it (XT is 0.2 thick, Y1 0.2 thick: 0.2 × 0.2 × 3).
    assert_eq!(hard_names(&r), [("XT", "Y1")]);
    assert!((r.hard[0].volume - 0.2 * 0.2 * 3.0).abs() < 1e-12, "{}", r.hard[0].volume);
    // Walls touching end to end in line (an L-join with straight continuation) are joined too.
    wall(&mut b, "Line1", [0.0, 20.0, 5.0, 20.0], 3.0);
    wall(&mut b, "Line2", [5.0, 20.0, 9.0, 20.0], 3.0);
    let r = clashes(&b, 0.1);
    assert!(!hard_names(&r).iter().chain(&soft_names(&r)).any(|(a, _)| a.starts_with("Line")));
}

#[test]
fn walls_with_openings_clash_exactly_with_each_other_and_with_slabs() {
    // Two crossing walls that both have openings away from the crossing (both meshes non-convex).
    let mut b = B::new();
    base(&mut b, &[0.1, 0.1]);
    wall(&mut b, "Wx", [0.0, 0.0, 6.0, 0.0], 3.0);
    wall(&mut b, "Wy", [3.0, -2.0, 3.0, 2.0], 3.0);
    b.add("Ox", "Opening", &[("host", r("Wx")), ("offset", n(0.5)), ("sill", n(0.0)), ("width", n(1.0)), ("height", n(2.0))]);
    b.add("Oy", "Opening", &[("host", r("Wy")), ("offset", n(0.5)), ("sill", n(1.0)), ("width", n(1.0)), ("height", n(1.0))]);
    let m = Model::compile(&b.0);
    assert!(m.elements.iter().all(|e| !clash::is_convex(&e.mesh)));
    let rep = clash::clash(&b.0, &m, 0.0);
    assert_eq!(hard_names(&rep), [("Wx", "Wy")]);
    assert!((rep.hard[0].volume - 0.2 * 0.2 * 3.0).abs() < 1e-12, "{}", rep.hard[0].volume);
    // An opening through the crossing region removes it from both.
    b.0.apply(bim_doc::Op::Set { id: EntityId::derive("Ox"), key: "offset".into(), value: n(2.5) }).unwrap();
    let rep = clashes(&b, 0.0);
    // Ox now spans s ∈ [2.5, 3.5], z ∈ [0, 2] of Wx, which is the crossing's footprint below 2 m.
    assert_eq!(hard_names(&rep), [("Wx", "Wy")]);
    assert!((rep.hard[0].volume - 0.2 * 0.2 * 1.0).abs() < 1e-12, "{}", rep.hard[0].volume);
    // A slab with an opening crossing the walls' top.
    b.add("L2", "Level", &[("elevation", n(2.5))]);
    b.add("S", "Slab", &[("level", r("L2")), ("boundary", nl(&[-1.0, -1.0, 7.0, -1.0, 7.0, 1.0, -1.0, 1.0])), ("thickness", n(1.0))]);
    b.add("H", "SlabOpening", &[("host", r("S")), ("boundary", nl(&[0.0, -0.5, 1.0, -0.5, 1.0, 0.5, 0.0, 0.5]))]);
    let m = Model::compile(&b.0);
    for e in &m.elements {
        assert!(e.mesh.is_closed(), "{}", e.name);
    }
    let rep = clash::clash(&b.0, &m, 0.0);
    // The slab spans z ∈ [1.5, 2.5]: it crosses Wx (4.2 m of its 0.2 thickness, minus the hole, minus Ox up to 2 m)
    // and Wy; compare against closed forms from the cut prisms' own volumes.
    let wx = m.walls.iter().find(|w| w.name == "Wx").unwrap();
    assert!(rep.hard.iter().any(|c| c.a == "S" && c.b == "Wx"), "{:?}", rep.hard);
    let sx = rep.hard.iter().find(|c| c.a == "S" && c.b == "Wx").unwrap();
    // Wx within the slab's z range 1.5..2.5, x in -1..7 (all of it), minus the hole's x 0..1 (a 1 m × 0.2 sliver
    // of wall in the opening: wall is at y ∈ ±0.1, inside the hole's y ∈ ±0.5), minus Ox (x 2.5..3.5, z ≤ 2).
    let expect = (6.0 - 1.0) * 0.2 * 1.0 - 1.0 * 0.2 * 0.5;
    assert!((sx.volume - expect).abs() < 1e-12, "{} vs {expect} (wall volume {})", sx.volume, wx.volume);
}

#[test]
fn open_surfaces_are_never_hard_but_can_be_soft() {
    let mut doc = Document::new();
    let sketch = |doc: &mut Document, cmds: &str| {
        for op in bim_sketch::run(doc, &format!(r#"{{"schema": 1, "commands": [{cmds}]}}"#)).unwrap() {
            doc.apply(op).unwrap();
        }
    };
    sketch(&mut doc, r#"{"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]},
        {"op": "pushpull", "shape": "box", "face": 1, "distance": 2.5},
        {"op": "rect", "shape": "sheet", "corner": [-1, 1, 1], "u": [2, 0, 0], "v": [0, 1, 0]},
        {"op": "rect", "shape": "inside", "corner": [1, 0.2, 1], "u": [1, 0, 0], "v": [0, 0.5, 0]},
        {"op": "rect", "shape": "away", "corner": [9, 9, 9], "u": [1, 0, 0], "v": [0, 1, 0]}"#);
    let m = Model::compile(&doc);
    assert!(m.element(EntityId::derive("box")).unwrap().mesh.is_closed());
    assert!(!m.element(EntityId::derive("sheet")).unwrap().mesh.is_closed());
    let r = clash::clash(&doc, &m, 0.0);
    assert!(r.hard.is_empty(), "{:?}", r.hard);
    let r = clash::clash(&doc, &m, 0.1);
    assert!(r.hard.is_empty());
    // The sheet pokes through the box's side: distance 0. The one inside it is 0.2 m from the box's
    // surface (the side y = 0), beyond this clearance; the far one is not within reach either.
    assert_eq!(soft_names(&r), [("box", "sheet")]);
    assert_eq!(r.soft[0].distance, 0.0);
    let r = clash::clash(&doc, &m, 1.5);
    assert!(soft_names(&r).contains(&("box", "inside")) && !soft_names(&r).iter().any(|(a, b)| *a == "away" || *b == "away"));
    let inside = r.soft.iter().find(|c| c.b == "inside").unwrap();
    assert!((inside.distance - 0.2).abs() < 1e-12, "{}", inside.distance);
}

#[test]
fn an_element_is_never_paired_with_itself_and_each_pair_is_reported_once() {
    let mut b = five_levels();
    for k in 0..6 {
        column_at(&mut b, &format!("K{k}"), 0.1 * k as f64, 0.0, 1.0, "Z0", "Z3");
    }
    let r = clashes(&b, 0.1);
    assert_eq!(r.hard.len(), 15, "all 6 columns overlap pairwise");
    let mut seen = std::collections::HashSet::new();
    for c in &r.hard {
        assert!(c.a < c.b, "{c:?}");
        assert!(seen.insert((c.a.clone(), c.b.clone())), "{c:?} twice");
        let (i, j): (usize, usize) = (c.a[1..].parse().unwrap(), c.b[1..].parse().unwrap());
        let overlap = 1.0 - 0.1 * (j - i) as f64;
        assert!((c.volume - overlap * 1.0 * 3.0).abs() < 1e-13, "{c:?}");
    }
    let sorted: Vec<_> = r.hard.iter().map(|c| (c.a.clone(), c.b.clone())).collect();
    let mut expect = sorted.clone();
    expect.sort();
    assert_eq!(sorted, expect);
    assert_eq!(r.matrix["Column"]["Column"], 15);
}

#[test]
fn the_clash_of_an_empty_or_unclashing_model_is_empty() {
    let b = B::new();
    assert_eq!(clashes(&b, 0.5), clash::ClashReport::default());
    let mut b = five_levels();
    column_at(&mut b, "A", 0.0, 0.0, 1.0, "Z0", "Z3");
    assert_eq!(clashes(&b, 0.5), clash::ClashReport::default());
}
