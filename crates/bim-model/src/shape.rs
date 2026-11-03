//! Direct-modelling geometry (spec/sketch.md): every placed `Shape` and every `ComponentInstance`
//! becomes one [`Element`] whose mesh is the Shape's faces triangulated in their own planes and
//! placed by the `transform` (16 numbers, column-major).
//!
//! The mesh shares one position per Shape vertex its faces use (the Shape's indices are stable,
//! so a vertex on no face, such as the end of a loose edge or one an operation left behind, is
//! left out), so a Shape that is a closed, consistently oriented 2-manifold gives a closed mesh
//! (by indices) with the exact volume of its faces; any other Shape gives its faces once,
//! single-sided and open. Ear clipping keeps every loop vertex, so a vertex collinear on a loop
//! (a split edge another face uses) yields a zero-area triangle that keeps the mesh edge-to-edge;
//! such triangles are kept. A Shape referenced by a ComponentDefinition is not placed itself; a
//! Shape with no faces, or with malformed props, has no element.

use bim_doc::{Document, Entity, Value};
use bim_geom::{Point2, Point3, Polygon};
use bim_sketch::geom::Plane;
use bim_sketch::Shape;

use crate::build::Cx;
use crate::query::IsDefinitionShape;
use crate::{Element, Mesh, Session};

/// A 4×4 transform as 16 column-major numbers.
type Mat4 = [f64; 16];

const IDENTITY: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// The `transform` prop of `e`: identity when absent, `None` when malformed.
fn transform(e: &Entity) -> Option<Mat4> {
    match e.props.get("transform") {
        None => Some(IDENTITY),
        Some(Value::List(items)) if items.len() == 16 => {
            let mut m = [0.0; 16];
            for (k, item) in items.iter().enumerate() {
                match item {
                    Value::Num(x) if x.is_finite() => m[k] = *x,
                    _ => return None,
                }
            }
            Some(m)
        }
        Some(_) => None,
    }
}

/// `a · b` for column-major matrices.
fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn apply(m: &Mat4, p: [f64; 3]) -> Point3 {
    let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    let w = if w != 0.0 && w.is_finite() { w } else { 1.0 };
    Point3::new(
        (m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12]) / w,
        (m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13]) / w,
        (m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14]) / w,
    )
}

/// Whether `m` mirrors (negative determinant of its linear part): triangles must then be flipped
/// to stay outward.
fn mirrors(m: &Mat4) -> bool {
    let det = m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2]) + m[8] * (m[1] * m[6] - m[5] * m[2]);
    det < 0.0
}

/// The triangulated mesh of a Shape under `m`, or `None` when it has no faces.
pub fn shape_mesh(shape: &Shape, m: &Mat4) -> Option<Mesh> {
    // Mesh position of each Shape vertex a face uses, in index order.
    let mut mesh_index: Vec<Option<u32>> = vec![None; shape.vertices.len()];
    let mut mesh = Mesh { positions: Vec::new(), triangles: Vec::new() };
    for &v in shape.faces.iter().flat_map(|f| f.loops.iter().flatten()) {
        if mesh_index[v].is_none() {
            mesh_index[v] = Some(mesh.positions.len() as u32);
            mesh.positions.push(apply(m, shape.vertices[v]));
        }
    }
    let flip = mirrors(m);
    for face in &shape.faces {
        let points = shape.loop_points(&face.loops[0]);
        let Some(plane) = Plane::from_loop(&points) else { continue };
        let project = |lp: &Vec<usize>| -> Vec<Point2> { lp.iter().map(|&i| plane.project(shape.vertices[i])).collect() };
        let poly = Polygon::with_holes(project(&face.loops[0]), face.loops[1..].iter().map(project).collect());
        let index: Vec<u32> = face.loops.iter().flatten().map(|&i| mesh_index[i].expect("every loop vertex has a position")).collect();
        for t in poly.triangulate() {
            let [a, b, c] = t.map(|i| index[i]);
            if a == b || b == c || a == c {
                continue;
            }
            mesh.triangles.push(if flip { [a, c, b] } else { [a, b, c] });
        }
    }
    if mesh.triangles.is_empty() {
        return None;
    }
    Some(mesh)
}

/// The mesh of a placed Shape or a ComponentInstance (the Shape/ComponentInstance arm of an
/// element's solid query): reads the entity's record, for an instance its definition's and the
/// definition's Shape's, and whether a Shape is a definition's (then it is not placed).
pub(crate) fn shape_element(cx: &Cx, e: &Entity) -> Option<Mesh> {
    match e.kind.as_str() {
        "Shape" => {
            if *cx.db.get::<IsDefinitionShape>(&e.id) {
                return None;
            }
            let (Ok(shape), Some(m)) = (Shape::from_entity(e), transform(e)) else { return None };
            shape_mesh(&shape, &m)
        }
        "ComponentInstance" => {
            let Some(Value::Ref(def)) = e.props.get("definition") else { return None };
            let def = cx.get(*def).filter(|d| d.kind == "ComponentDefinition")?;
            let Some(Value::Ref(sid)) = def.props.get("shape") else { return None };
            let se = cx.get(*sid).filter(|s| s.kind == "Shape")?;
            let (Ok(shape), Some(ms), Some(mi)) = (Shape::from_entity(&se), transform(&se), transform(e)) else { return None };
            shape_mesh(&shape, &mul(&mi, &ms))
        }
        _ => None,
    }
}

/// The elements of every placed Shape and every ComponentInstance of `doc`, sorted by name.
pub fn shape_elements(doc: &Document) -> Vec<Element> {
    Session::from_document(doc)
        .model()
        .elements
        .into_iter()
        .filter(|e| e.kind == "Shape" || e.kind == "ComponentInstance")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bim_doc::{EntityId, Op};

    fn doc_with(json: &str) -> Document {
        let mut doc = Document::new();
        for op in bim_sketch::run(&doc, json).unwrap() {
            doc.apply(op).unwrap();
        }
        doc
    }

    const BOX: &str = r#"{"schema": 1, "commands": [
        {"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]},
        {"op": "pushpull", "shape": "box", "face": 1, "distance": 2.5}]}"#;

    #[test]
    fn a_box_is_a_closed_mesh_with_exact_volume() {
        let doc = doc_with(BOX);
        let els = shape_elements(&doc);
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].name, "box");
        assert!(els[0].mesh.is_closed());
        assert!((els[0].mesh.volume() - 30.0).abs() < 1e-12);
        assert_eq!(els[0].mesh.triangles.len(), 12);
    }

    #[test]
    fn transforms_and_open_surfaces() {
        let mut doc = doc_with(
            r#"{"schema": 1, "commands": [
            {"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]},
            {"op": "pushpull", "shape": "box", "face": 1, "distance": 2.5},
            {"op": "rect", "shape": "open", "corner": [0, 0, 5], "u": [1, 0, 0], "v": [0, 2, 0]}]}"#,
        );
        // Translate the box by (10, 20, 1) and mirror it in x.
        let t: Vec<Value> = [-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 20.0, 1.0, 1.0].iter().map(|&x| Value::Num(x)).collect();
        doc.apply(Op::Set { id: EntityId::parse("box"), key: "transform".into(), value: Value::List(t) }).unwrap();
        let els = shape_elements(&doc);
        let b = els.iter().find(|e| e.name == "box").unwrap();
        assert!(b.mesh.is_closed());
        assert!((b.mesh.volume() - 30.0).abs() < 1e-9, "{}", b.mesh.volume());
        let (lo, hi) = b.mesh.bounds().unwrap();
        assert!((lo.x - 6.0).abs() < 1e-12 && (hi.x - 10.0).abs() < 1e-12 && (lo.z - 1.0).abs() < 1e-12);
        let o = els.iter().find(|e| e.name == "open").unwrap();
        assert!(!o.mesh.is_closed());
        assert_eq!(o.mesh.triangles.len(), 2);
    }

    #[test]
    fn a_new_pushpull_prism_is_a_closed_mesh_either_way() {
        for d in [1.5, -1.5] {
            let doc = doc_with(&format!(
                r#"{{"schema": 1, "commands": [
                {{"op": "rect", "shape": "lone", "corner": [1, 1, 1], "u": [0, 3, 0], "v": [0, 0, 2]}},
                {{"op": "pushpull", "shape": "lone", "face": 1, "distance": {d}, "new": true}}]}}"#
            ));
            let els = shape_elements(&doc);
            assert_eq!(els.len(), 1);
            assert!(els[0].mesh.is_closed(), "d = {d}");
            assert!((els[0].mesh.volume() - 9.0).abs() < 1e-12, "d = {d}: {}", els[0].mesh.volume());
            assert_eq!(els[0].mesh.triangles.len(), 12);
        }
    }

    #[test]
    fn punched_box_is_closed_with_a_ring_face() {
        let doc = doc_with(
            r#"{"schema": 1, "commands": [
            {"op": "rect", "shape": "p", "corner": [20, 0, 0], "u": [3, 0, 0], "v": [0, 3, 0]},
            {"op": "pushpull", "shape": "p", "face": 1, "distance": 1},
            {"op": "rect", "shape": "p", "corner": [21, 1, 1], "u": [1, 0, 0], "v": [0, 1, 0]},
            {"op": "pushpull", "shape": "p", "face": 7, "distance": -1}]}"#,
        );
        let els = shape_elements(&doc);
        assert!(els[0].mesh.is_closed());
        assert!((els[0].mesh.volume() - 8.0).abs() < 1e-12, "{}", els[0].mesh.volume());
    }

    #[test]
    fn make_component_places_each_instance_and_edits_reach_them_all() {
        let mut doc = doc_with(BOX);
        let t: Vec<Value> = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 0.0, 0.0, 1.0].iter().map(|&x| Value::Num(x)).collect();
        doc.apply(Op::Set { id: EntityId::parse("box"), key: "transform".into(), value: Value::List(t) }).unwrap();
        let sketch = |doc: &mut Document, cmds: &str| {
            for op in bim_sketch::run(doc, &format!(r#"{{"schema": 1, "commands": [{cmds}]}}"#)).unwrap() {
                doc.apply(op).unwrap();
            }
        };
        sketch(&mut doc, r#"{"op": "make_component", "shape": "box", "definition": "D", "instance": "I", "name": "Crate"}"#);
        let i2 = EntityId::parse("I2");
        doc.apply(Op::Create { id: i2, kind: "ComponentInstance".into() }).unwrap();
        doc.apply(Op::Set { id: i2, key: "definition".into(), value: Value::Ref(EntityId::parse("D")) }).unwrap();
        for (vol, x0) in [(30.0, 10.0), (42.0, 10.0)] {
            if vol > 30.0 {
                sketch(&mut doc, r#"{"op": "pushpull", "shape": "box", "face": 1, "distance": 1}"#);
            }
            let els = shape_elements(&doc);
            assert_eq!(els.len(), 2, "the definition's Shape is not placed itself");
            let i = els.iter().find(|e| e.id == EntityId::parse("I")).unwrap();
            let j = els.iter().find(|e| e.id == i2).unwrap();
            for e in [i, j] {
                assert!(e.mesh.is_closed());
                assert!((e.mesh.volume() - vol).abs() < 1e-9);
            }
            assert!((i.mesh.bounds().unwrap().0.x - x0).abs() < 1e-12, "the instance carries the former transform");
            assert!(j.mesh.bounds().unwrap().0.x.abs() < 1e-12);
        }
    }

    #[test]
    fn component_instances_are_placed_and_definition_shapes_are_not() {
        let mut doc = doc_with(BOX);
        let (b, d, i) = (EntityId::parse("box"), EntityId::parse("def"), EntityId::parse("inst"));
        doc.apply(Op::Create { id: d, kind: "ComponentDefinition".into() }).unwrap();
        doc.apply(Op::Set { id: d, key: "shape".into(), value: Value::Ref(b) }).unwrap();
        doc.apply(Op::Create { id: i, kind: "ComponentInstance".into() }).unwrap();
        doc.apply(Op::Set { id: i, key: "_name".into(), value: Value::Text("inst".into()) }).unwrap();
        doc.apply(Op::Set { id: i, key: "definition".into(), value: Value::Ref(d) }).unwrap();
        let t: Vec<Value> = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 7.0, 1.0].iter().map(|&x| Value::Num(x)).collect();
        doc.apply(Op::Set { id: i, key: "transform".into(), value: Value::List(t) }).unwrap();
        let els = shape_elements(&doc);
        assert_eq!(els.len(), 1);
        assert_eq!((els[0].name.as_str(), els[0].kind.as_str()), ("inst", "ComponentInstance"));
        assert!((els[0].mesh.bounds().unwrap().0.z - 7.0).abs() < 1e-12);
    }
}
