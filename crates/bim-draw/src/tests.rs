use super::*;
use bim_doc::{Document, EntityId, Op, Value};
use bim_model::{Model, Session};

fn n(x: f64) -> Value {
    Value::Num(x)
}
fn r(name: &str) -> Value {
    Value::Ref(EntityId::derive(name))
}

fn create(ops: &mut Vec<Op>, name: &str, kind: &str, props: &[(&str, Value)]) {
    let id = EntityId::derive(name);
    ops.push(Op::Create { id, kind: kind.into() });
    ops.push(Op::Set { id, key: "_name".into(), value: Value::Text(name.into()) });
    for (k, v) in props {
        ops.push(Op::Set { id, key: (*k).into(), value: v.clone() });
    }
}

/// One wall on L1, a column from L1 to L2 and a dimension on the plan.
fn sample() -> Vec<Op> {
    let mut ops = Vec::new();
    create(&mut ops, "L1", "Level", &[("elevation", n(1.0))]);
    create(&mut ops, "L2", "Level", &[("elevation", n(4.0))]);
    create(&mut ops, "WT", "WallType", &[
        ("layer_thickness", Value::List(vec![n(0.2)])),
        ("layer_material", Value::List(vec![Value::Text("brick".into())])),
    ]);
    create(&mut ops, "W", "Wall", &[
        ("level", r("L1")), ("type", r("WT")),
        ("x1", n(0.0)), ("y1", n(0.0)), ("x2", n(4.0)), ("y2", n(0.0)), ("height", n(3.0)),
    ]);
    create(&mut ops, "C", "Column", &[
        ("x", n(6.0)), ("y", n(2.0)), ("base", r("L1")), ("top", r("L2")),
        ("section_b", n(0.3)), ("section_h", n(0.4)),
    ]);
    create(&mut ops, "P1", "Point", &[("x", n(0.0)), ("y", n(-1.0))]);
    create(&mut ops, "P2", "Point", &[("x", n(4.0)), ("y", n(-1.0)), ("z", n(0.0))]);
    create(&mut ops, "D", "Dimension", &[("a", r("P1")), ("b", r("P2")), ("view", Value::Text("plan:L1@1.2".into()))]);
    ops
}

fn document(ops: &[Op]) -> Document {
    let mut doc = Document::new();
    for op in ops {
        doc.apply(op.clone()).unwrap();
    }
    doc
}

#[test]
fn facade_draws_like_the_session() {
    let ops = sample();
    let doc = document(&ops);
    let mut session = Session::new();
    session.apply(ops);
    for view in ["plan:L1@1.2", "section:-1,2,9,2", "elevation:s"] {
        session.add_view(view);
        let v = View::parse(&doc, view).unwrap();
        let from_scratch = svg::to_svg(&draw(&doc, &Model::compile(&doc), &v));
        assert_eq!(session.svg(view).unwrap(), from_scratch, "{view}");
        assert_eq!(session_svg(&session, view).unwrap(), from_scratch, "{view}");
    }
    assert!(View::parse(&doc, "plan:NOPE@1").is_err());
    assert_eq!(session_svg(&session, "plan:NOPE@1"), None);
    assert_eq!(Layer::ALL.len(), 6);
    assert_eq!(SCALE, 10.0);
}

#[test]
fn svg_query_is_memoized_and_follows_edits() {
    let mut session = Session::new();
    session.apply(sample());
    session.add_view("plan:L1@1.2");
    let db = session.db();
    let first = db.get::<SvgQuery>(&"plan:L1@1.2".to_string());
    db.take_log();
    // Same revision: no re-render.
    let again = db.get::<SvgQuery>(&"plan:L1@1.2".to_string());
    assert!(Rc::ptr_eq(&first, &again));
    assert!(db.take_log().is_empty());
    // An edit that leaves the drawing unchanged (the column's top is above the cut either way)
    // re-runs the drawing but not the SVG rendering (early cutoff).
    session.apply(vec![Op::Set { id: EntityId::derive("L2"), key: "elevation".into(), value: n(5.0) }]);
    let db = session.db();
    let same = db.get::<SvgQuery>(&"plan:L1@1.2".to_string());
    assert_eq!(*same, *first);
    assert!(db.take_log().iter().all(|e| !e.is::<SvgQuery>()), "unchanged drawing re-rendered");
    // An edit that changes the drawing re-renders it.
    session.apply(vec![Op::Set { id: EntityId::derive("P2"), key: "x".into(), value: n(4.2506) }]);
    let db = session.db();
    let changed = db.get::<SvgQuery>(&"plan:L1@1.2".to_string());
    assert!(db.take_log().iter().any(|e| e.is::<SvgQuery>()));
    assert!(changed.contains(">4251<") && !first.contains(">4251<"));
    assert_eq!(**changed, session.svg("plan:L1@1.2").unwrap());
}
