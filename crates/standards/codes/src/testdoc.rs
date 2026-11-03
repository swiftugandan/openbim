//! Small documents for the unit tests (built with `bim_doc` ops, as the `bim-plugin` tests do).

use bim_doc::{Document, EntityId, Op, Value};

pub(crate) fn id(name: &str) -> EntityId {
    EntityId::derive(name)
}

/// Creates entity `name` of `kind` with its `_name` and `props`.
pub(crate) fn create(doc: &mut Document, name: &str, kind: &str, props: Vec<(&str, Value)>) {
    doc.apply(Op::Create { id: id(name), kind: kind.to_string() }).unwrap();
    doc.apply(Op::Set { id: id(name), key: "_name".to_string(), value: Value::Text(name.to_string()) }).unwrap();
    for (key, value) in props {
        doc.apply(Op::Set { id: id(name), key: key.to_string(), value }).unwrap();
    }
}

pub(crate) fn num(x: f64) -> Value {
    Value::Num(x)
}

pub(crate) fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

pub(crate) fn reference(name: &str) -> Value {
    Value::Ref(id(name))
}

pub(crate) fn nums(xs: &[f64]) -> Value {
    Value::List(xs.iter().map(|&x| Value::Num(x)).collect())
}

/// Level `L0` at elevation 0 and the 0.2 m wall type `T`.
pub(crate) fn base_doc() -> Document {
    let mut doc = Document::new();
    create(&mut doc, "L0", "Level", vec![("elevation", num(0.0))]);
    create(&mut doc, "T", "WallType", vec![("layer_thickness", nums(&[0.2]))]);
    doc
}

/// Four walls `<prefix>-w0..3` of type `T` closing the rectangle with corners (x0, y0), (x1, y1) on `level`.
pub(crate) fn rect_room(doc: &mut Document, prefix: &str, level: &str, x0: f64, y0: f64, x1: f64, y1: f64) {
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    for k in 0..4 {
        let ((ax, ay), (bx, by)) = (corners[k], corners[(k + 1) % 4]);
        create(
            doc,
            &format!("{prefix}-w{k}"),
            "Wall",
            vec![
                ("level", reference(level)),
                ("x1", num(ax)),
                ("y1", num(ay)),
                ("x2", num(bx)),
                ("y2", num(by)),
                ("height", num(3.0)),
                ("type", reference("T")),
            ],
        );
    }
}
