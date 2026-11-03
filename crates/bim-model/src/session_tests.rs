use std::collections::BTreeSet;

use bim_doc::{Document, EntityId, Op, Value};

use crate::drawing::{self, View};
use crate::{Model, Session};

fn id(s: &str) -> EntityId {
    EntityId::derive(s)
}
fn n(x: f64) -> Value {
    Value::Num(x)
}
fn r(s: &str) -> Value {
    Value::Ref(id(s))
}
fn set(seed: &str, key: &str, value: Value) -> Op {
    Op::Set { id: id(seed), key: key.into(), value }
}
fn create(ops: &mut Vec<Op>, seed: &str, kind: &str, props: &[(&str, Value)]) {
    ops.push(Op::Create { id: id(seed), kind: kind.into() });
    ops.push(set(seed, "_name", Value::Text(seed.into())));
    for (k, v) in props {
        ops.push(set(seed, k, v.clone()));
    }
}
fn wall(ops: &mut Vec<Op>, seed: &str, p: [f64; 4], h: f64) {
    create(ops, seed, "Wall", &[
        ("level", r("L1")), ("type", r("WT200")),
        ("x1", n(p[0])), ("y1", n(p[1])), ("x2", n(p[2])), ("y2", n(p[3])), ("height", n(h)),
    ]);
}

/// WA–WB at an L-join, WT abutting WA at a T-join, opening OA in WA, an isolated wall WI, a slab
/// and a column, on one level.
fn fixture() -> Vec<Op> {
    let mut o = Vec::new();
    create(&mut o, "L1", "Level", &[("elevation", n(0.0))]);
    create(&mut o, "L2", "Level", &[("elevation", n(3.0))]);
    create(&mut o, "WT200", "WallType", &[
        ("layer_thickness", Value::List(vec![n(0.2)])),
        ("layer_material", Value::List(vec![Value::Text("concrete".into())])),
    ]);
    wall(&mut o, "WA", [0.0, 0.0, 6.0, 0.0], 3.0);
    wall(&mut o, "WB", [6.0, 0.0, 6.0, 4.0], 3.0);
    wall(&mut o, "WT", [3.0, 0.0, 3.0, -3.0], 3.0);
    wall(&mut o, "WI", [20.0, 0.0, 26.0, 0.0], 3.0);
    create(&mut o, "OA", "Opening", &[("host", r("WA")), ("offset", n(1.0)), ("sill", n(0.0)), ("width", n(0.9)), ("height", n(2.1))]);
    create(&mut o, "S1", "Slab", &[
        ("level", r("L1")),
        ("boundary", Value::List([0.0, 0.0, 6.0, 0.0, 6.0, 4.0, 0.0, 4.0].iter().map(|&x| n(x)).collect())),
        ("thickness", n(0.2)),
    ]);
    create(&mut o, "K1", "Column", &[("x", n(10.0)), ("y", n(5.0)), ("base", r("L1")), ("top", r("L2")), ("section_b", n(0.3)), ("section_h", n(0.3))]);
    o
}

fn names(v: &[String]) -> BTreeSet<&str> {
    v.iter().map(String::as_str).collect()
}

fn document(ops: &[Op]) -> Document {
    let mut doc = Document::new();
    for op in ops {
        doc.apply(op.clone()).unwrap();
    }
    doc
}

#[test]
fn first_apply_reports_everything_and_edits_reach_exactly_the_joined_walls() {
    let mut s = Session::new();
    let stats = s.apply(fixture());
    assert_eq!(names(&stats.solids), ["K1", "OA", "S1", "WA", "WB", "WI", "WT"].into_iter().collect());
    assert!(stats.views.is_empty());
    s.add_view("plan:L1@1.2");
    s.add_view("section:30,-15,-5,-15"); // looks -y from y = -15: nothing of the fixture
    s.add_view("elevation:s");

    // The through wall: itself, its L partner, the abutting wall and its opening.
    let stats = s.apply(vec![set("WA", "height", n(3.5))]);
    assert_eq!(names(&stats.solids), ["OA", "WA", "WB", "WT"].into_iter().collect());
    assert_eq!(names(&stats.views), ["elevation:s", "plan:L1@1.2"].into_iter().collect());
    // The abutting wall: itself and the through wall only.
    let stats = s.apply(vec![set("WT", "height", n(2.5))]);
    assert_eq!(names(&stats.solids), ["WA", "WT"].into_iter().collect());
    // An isolated wall.
    let stats = s.apply(vec![set("WI", "height", n(2.0))]);
    assert_eq!(names(&stats.solids), ["WI"].into_iter().collect());
    // The level: everything on it (the column stands on it).
    let stats = s.apply(vec![set("L1", "elevation", n(0.5))]);
    assert!(names(&stats.solids).is_superset(&["K1", "S1", "WA", "WB", "WI", "WT"].into_iter().collect()));
    // The column's top level: the column only. The plan cuts the column, whose far part is
    // recomputed from a changed mesh, so the plan may be redrawn; the section behind which the
    // whole fixture lies is not.
    let stats = s.apply(vec![set("L2", "elevation", n(3.5))]);
    assert_eq!(names(&stats.solids), ["K1"].into_iter().collect());
    let views = names(&stats.views);
    assert!(views.contains("elevation:s") && !views.contains("section:30,-15,-5,-15"), "{views:?}");
    // No-ops: equal values, an empty batch, an op the document rejects.
    for ops in [vec![set("WA", "height", n(3.5))], vec![], vec![set("NOPE", "height", n(1.0))], vec![Op::Create { id: id("WA"), kind: "Wall".into() }]] {
        let stats = s.apply(ops);
        assert!(stats.solids.is_empty() && stats.views.is_empty(), "{stats:?}");
    }
}

#[test]
fn openings_reach_their_host_and_deletions_drop_their_solid() {
    let mut s = Session::new();
    s.apply(fixture());
    let before = s.solid("WA").unwrap().volume();
    let stats = s.apply(vec![set("OA", "width", n(1.5))]);
    assert_eq!(names(&stats.solids), ["OA", "WA"].into_iter().collect());
    assert!(s.solid("WA").unwrap().volume() < before);
    assert!(s.solid("OA").is_some(), "an opening has its own solid (the box it cuts)");
    let stats = s.apply(vec![Op::Delete { id: id("OA") }]);
    assert_eq!(names(&stats.solids), ["WA"].into_iter().collect());
    assert!(s.solid("OA").is_none());
    // Mitred with WB at (6, 0): the mitre adds and removes the same triangle, so 6 m × 0.2 × 3.
    assert!((s.solid("WA").unwrap().volume() - 6.0 * 0.2 * 3.0).abs() < 1e-9, "{}", s.solid("WA").unwrap().volume());
    // A new opening on the isolated wall reaches it alone.
    let mut ops = Vec::new();
    create(&mut ops, "ON", "Opening", &[("host", r("WI")), ("offset", n(1.0)), ("sill", n(1.0)), ("width", n(0.5)), ("height", n(0.5))]);
    let stats = s.apply(ops);
    assert_eq!(names(&stats.solids), ["ON", "WI"].into_iter().collect());
}

#[test]
fn ids_resolve_by_name_or_id_string() {
    let mut s = Session::new();
    let mut ops = fixture();
    // An entity without `_name`.
    ops.push(Op::Create { id: EntityId::parse("K2"), kind: "Column".into() });
    for (k, v) in [("x", n(0.0)), ("y", n(9.0)), ("base", r("L1")), ("top", r("L2")), ("section_b", n(0.2)), ("section_h", n(0.2))] {
        ops.push(Op::Set { id: EntityId::parse("K2"), key: k.into(), value: v });
    }
    s.apply(ops);
    assert!(s.solid("WA").is_some());
    assert!(s.solid(&id("WA").to_string()).is_some(), "the id's string form");
    assert!(s.solid(&id("WA").to_ulid_string()).is_some(), "the id as a ULID");
    assert!(s.solid("K2").is_some(), "the seed of a derived id");
    assert!(s.solid("L1").is_none() && s.solid("nope").is_none());
    assert_eq!(s.svg("plan:L1@1.2"), None, "unregistered views have no SVG");
}

#[test]
fn incremental_results_equal_a_from_scratch_compile() {
    let mut history = fixture();
    let mut s = Session::new();
    s.apply(history.clone());
    let views = ["plan:L1@1.2", "section:-5,-1,30,-1", "elevation:n"];
    for v in views {
        s.add_view(v);
    }
    let edits = vec![
        vec![set("WA", "height", n(3.5))],
        vec![set("L1", "elevation", n(-0.5))],
        vec![set("OA", "offset", n(1.6))],
        vec![set("WT", "x2", n(3.5))],
        vec![Op::Delete { id: id("WB") }],
        vec![set("K1", "section_b", n(0.4))],
    ];
    for ops in edits {
        history.extend(ops.iter().cloned());
        s.apply(ops);
        let doc = document(&history);
        let model = Model::compile(&doc);
        assert_eq!(model, s.model(), "Model::compile equals the live session's model");
        for e in &model.elements {
            assert_eq!(s.mesh(&e.name), Some(&e.mesh), "{}", e.name);
        }
        for v in views {
            let view = View::parse(&doc, v).unwrap();
            let scratch = drawing::svg::to_svg(&drawing::draw(&doc, &model, &view));
            assert_eq!(s.svg(v).unwrap(), scratch, "{v}");
        }
    }
}
