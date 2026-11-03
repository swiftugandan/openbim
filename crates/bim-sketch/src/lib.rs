//! `bim-sketch`: SketchUp-style direct modelling on the document (spec/sketch.md).
//!
//! A [`Shape`] entity holds a vertex table, planar faces with stable ids and loose edges.
//! [`run`] applies a whole command file (`line`, `rect`, `circle`, `arc`, `pushpull`, `erase`,
//! `offset`, `followme`, `move`, `rotate`, `scale`, `paint`, `make_component`) to the Shapes it names and returns the document ops that record the result, so a front end
//! (the CLI's `bim sketch`, later the MCP server and the Studio through WebAssembly) applies them
//! atomically: an invalid command yields an error and no ops.
//!
//! The geometry lives in three layers: [`geom`] (vectors, the 1e-9 m equality tolerance, planes),
//! [`planar`] (the planar graph of one plane's edges and the face walk that finds the smallest
//! regions edges bound) and [`ops`] / [`modify`] (the operations themselves).
//! [`shape::Shape::normalize`] restores the structural rules after every operation: points within
//! 1e-9 m are one vertex, no T-junctions, no degenerate faces. `make_component` is handled here:
//! it creates a ComponentDefinition of the Shape and a ComponentInstance carrying the Shape's
//! former `transform`, and resets the Shape's `transform` to identity.
//!
//! [`infer`] is the inference engine drawing tools snap with ([`InferScene`], [`infer_json`]) and
//! [`measure`] parses typed measurements ([`parse_measurement`]).

#![forbid(unsafe_code)]

pub mod command;
pub mod geom;
pub mod infer;
mod json;
pub mod measure;
pub mod modify;
pub mod ops;
pub mod planar;
pub mod shape;

use std::collections::BTreeMap;
use std::fmt;

use bim_doc::{Document, EntityId, Op, Value};

pub use command::{Command, Op as SketchOp};
pub use infer::{infer_json, InferScene};
pub use measure::{parse_measurement, Measurement};
pub use shape::{Face, Shape};

/// Why a command file could not be applied: nothing is changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SketchError(pub String);

impl fmt::Display for SketchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SketchError {}

/// What [`apply`] produces: the ops, and the name each created or changed entity was given in the
/// command file (what a front end journals the entity under).
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub ops: Vec<Op>,
    pub names: BTreeMap<EntityId, String>,
}

/// The ops that apply the whole command file `json` to `doc`: for every Shape the file draws on
/// or edits, its new geometry props (`vertices`, `faces`, `face_ids`, `edges`, and
/// `face_products` once it is painted; `transform` when `make_component` resets it); a Shape a
/// drawing command names but the document lacks is created first (with the reserved `_name` and
/// `_schema` props, like any `bim` create); then the ComponentDefinitions and ComponentInstances
/// that `make_component` creates. Fails, with nothing to apply, on a malformed file or any invalid
/// command.
pub fn run(doc: &Document, json: &str) -> Result<Vec<Op>, SketchError> {
    apply(doc, json).map(|o| o.ops)
}

/// A Shape the command file works on.
struct Working {
    id: EntityId,
    name: String,
    shape: Shape,
    created: bool,
    /// The `transform` as the file leaves it, when a command changed it.
    transform: Option<Value>,
}

/// A ComponentDefinition and its first ComponentInstance made by `make_component`.
struct Component {
    shape: EntityId,
    definition: (EntityId, String),
    instance: (EntityId, String),
    name: String,
    transform: Value,
    level: Option<Value>,
}

/// The 16 numbers of the identity transform (column-major).
fn identity() -> Value {
    let mut m = vec![Value::Num(0.0); 16];
    for k in [0, 5, 10, 15] {
        m[k] = Value::Num(1.0);
    }
    Value::List(m)
}

/// [`run`] plus the names of the entities the ops touch.
pub fn apply(doc: &Document, json: &str) -> Result<Outcome, SketchError> {
    let commands = command::parse(json).map_err(SketchError)?;
    let mut shapes: Vec<Working> = Vec::new();
    let mut components: Vec<Component> = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        let at = format!("commands[{i}] ({} on `{}`)", cmd.op.name(), cmd.shape);
        let err = |m: String| SketchError(format!("{at}: {m}"));
        let id = EntityId::parse(&cmd.shape);
        if components.iter().any(|c| c.definition.0 == id || c.instance.0 == id) {
            return Err(err(format!("`{}` is not a Shape", cmd.shape)));
        }
        let k = match shapes.iter().position(|w| w.id == id) {
            Some(k) => k,
            None => {
                let loaded = match doc.get(id) {
                    Some(e) if e.kind == "Shape" => Some(Shape::from_entity(e).map_err(err)?),
                    Some(e) => return Err(err(format!("entity `{}` is a {}, not a Shape", cmd.shape, e.kind))),
                    None => None,
                };
                let (shape, created) = match loaded {
                    Some(shape) => (shape, false),
                    None if cmd.op.draws() => (Shape::default(), true),
                    None => return Err(err(format!("there is no Shape `{}`", cmd.shape))),
                };
                shapes.push(Working { id, name: cmd.shape.clone(), shape, created, transform: None });
                shapes.len() - 1
            }
        };
        let shape = &mut shapes[k].shape;
        shape.prune_products();
        let result = match &cmd.op {
            SketchOp::Line { from, to } => ops::line(shape, *from, *to),
            SketchOp::Rect { corner, u, v } => ops::rect(shape, *corner, *u, *v),
            SketchOp::Circle { centre, normal, radius, segments } => ops::circle(shape, *centre, *normal, *radius, *segments),
            SketchOp::Arc { centre, start, normal, angle, segments } => ops::arc(shape, *centre, *start, *normal, *angle, *segments),
            SketchOp::PushPull { face, distance, new } => ops::pushpull(shape, *face, *distance, *new),
            SketchOp::Erase { edges } => ops::erase(shape, edges),
            SketchOp::Offset { face, distance } => modify::offset(shape, *face, *distance),
            SketchOp::FollowMe { face, path } => modify::followme(shape, *face, path),
            SketchOp::Transform { selection, xf, copy, array } => modify::transform(shape, selection, *xf, *copy, *array),
            SketchOp::Paint { faces, product } => {
                let product = match product {
                    None => None,
                    Some(p) => match doc.get(EntityId::parse(p)) {
                        Some(e) if e.kind == "Product" => Some(e.id),
                        Some(e) => return Err(err(format!("paint: `{p}` is a {}, not a Product", e.kind))),
                        None => return Err(err(format!("paint: there is no Product `{p}`"))),
                    },
                };
                modify::paint(shape, faces, product)
            }
            SketchOp::MakeComponent { definition, instance, name } => {
                let (d, n) = (EntityId::parse(definition), EntityId::parse(instance));
                for (what, eid, text) in [("definition", d, definition), ("instance", n, instance)] {
                    let taken = doc.get(eid).is_some()
                        || shapes.iter().any(|w| w.id == eid)
                        || components.iter().any(|c| [c.definition.0, c.instance.0].contains(&eid));
                    if taken {
                        return Err(err(format!("make_component: the {what} `{text}` already exists")));
                    }
                }
                if d == n {
                    return Err(err("make_component: `definition` and `instance` must differ".to_string()));
                }
                let entity = doc.get(id);
                let former = match &shapes[k].transform {
                    Some(t) => t.clone(),
                    None => match entity.and_then(|e| e.props.get("transform")) {
                        None => identity(),
                        Some(Value::List(items)) if items.len() == 16 && items.iter().all(|x| matches!(x, Value::Num(v) if v.is_finite())) => Value::List(items.clone()),
                        Some(other) => return Err(err(format!("make_component: the shape's `transform` is not 16 finite numbers: {other:?}"))),
                    },
                };
                let level = match entity.and_then(|e| e.props.get("level")) {
                    Some(Value::Ref(l)) => Some(Value::Ref(*l)),
                    _ => None,
                };
                shapes[k].transform = Some(identity());
                components.push(Component { shape: id, definition: (d, definition.clone()), instance: (n, instance.clone()), name: name.clone(), transform: former, level });
                Ok(())
            }
        };
        result.map_err(err)?;
        shapes[k].shape.prune_products();
    }
    let mut out = Outcome { ops: Vec::new(), names: BTreeMap::new() };
    let create = |out: &mut Outcome, id: EntityId, kind: &str, name: &str| {
        out.ops.push(Op::Create { id, kind: kind.to_string() });
        out.ops.push(Op::Set { id, key: "_name".to_string(), value: Value::Text(name.to_string()) });
        out.ops.push(Op::Set { id, key: "_schema".to_string(), value: Value::Num(f64::from(Document::SCHEMA_VERSION)) });
        out.names.insert(id, name.to_string());
    };
    for w in shapes {
        if w.created {
            create(&mut out, w.id, "Shape", &w.name);
        }
        for (key, value) in w.shape.props() {
            out.ops.push(Op::Set { id: w.id, key, value });
        }
        if let Some(t) = w.transform {
            out.ops.push(Op::Set { id: w.id, key: "transform".to_string(), value: t });
        }
        out.names.insert(w.id, w.name);
    }
    for c in components {
        let (d, n) = (c.definition.0, c.instance.0);
        create(&mut out, d, "ComponentDefinition", &c.definition.1);
        out.ops.push(Op::Set { id: d, key: "name".to_string(), value: Value::Text(c.name) });
        out.ops.push(Op::Set { id: d, key: "shape".to_string(), value: Value::Ref(c.shape) });
        create(&mut out, n, "ComponentInstance", &c.instance.1);
        out.ops.push(Op::Set { id: n, key: "definition".to_string(), value: Value::Ref(d) });
        out.ops.push(Op::Set { id: n, key: "transform".to_string(), value: c.transform });
        if let Some(l) = c.level {
            out.ops.push(Op::Set { id: n, key: "level".to_string(), value: l });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
