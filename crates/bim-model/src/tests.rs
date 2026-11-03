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
