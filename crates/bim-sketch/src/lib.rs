//! `bim-sketch`: SketchUp-style direct modelling on the document (spec/sketch.md).
//!
//! A [`Shape`] entity holds a vertex table, planar faces with stable ids and loose edges.
//! [`run`] applies a whole command file (`line`, `rect`, `circle`, `arc`, `pushpull`, `erase`)
//! to the Shapes it names and returns the document ops that record the result, so a front end
//! (the CLI's `bim sketch`, later the MCP server and the Studio through WebAssembly) applies them
//! atomically: an invalid command yields an error and no ops.
//!
//! The geometry lives in three layers: [`geom`] (vectors, the 1e-9 m equality tolerance, planes),
//! [`planar`] (the planar graph of one plane's edges and the face walk that finds the smallest
//! regions edges bound) and [`ops`] (the operations themselves). [`shape::Shape::normalize`]
//! restores the structural rules after every operation: points within 1e-9 m are one vertex, no
//! T-junctions, no degenerate faces. The remaining ops of spec/sketch.md (`offset`, `followme`,
//! `move`/`rotate`/`scale`, `paint`, `make_component`) are later tasks and are rejected as invalid.

#![forbid(unsafe_code)]

pub mod command;
pub mod geom;
mod json;
pub mod ops;
pub mod planar;
pub mod shape;

use std::collections::BTreeMap;
use std::fmt;

use bim_doc::{Document, EntityId, Op, Value};

pub use command::{Command, Op as SketchOp};
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
/// or edits, its new geometry props (`vertices`, `faces`, `face_ids`, `edges`); a Shape a drawing
/// command names but the document lacks is created first (with the reserved `_name` and `_schema`
/// props, like any `bim` create). Fails, with nothing to apply, on a malformed file or any invalid
/// command.
pub fn run(doc: &Document, json: &str) -> Result<Vec<Op>, SketchError> {
    apply(doc, json).map(|o| o.ops)
}

/// [`run`] plus the names of the entities the ops touch.
pub fn apply(doc: &Document, json: &str) -> Result<Outcome, SketchError> {
    let commands = command::parse(json).map_err(SketchError)?;
    let mut shapes: Vec<(EntityId, String, Shape, bool)> = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        let at = format!("commands[{i}] ({} on `{}`)", cmd.op.name(), cmd.shape);
        let id = EntityId::parse(&cmd.shape);
        let k = match shapes.iter().position(|(sid, _, _, _)| *sid == id) {
            Some(k) => k,
            None => {
                let loaded = match doc.get(id) {
                    Some(e) if e.kind == "Shape" => Some(Shape::from_entity(e).map_err(|m| SketchError(format!("{at}: {m}")))?),
                    Some(e) => return Err(SketchError(format!("{at}: entity `{}` is a {}, not a Shape", cmd.shape, e.kind))),
                    None => None,
                };
                match loaded {
                    Some(shape) => shapes.push((id, cmd.shape.clone(), shape, false)),
                    None if cmd.op.draws() => shapes.push((id, cmd.shape.clone(), Shape::default(), true)),
                    None => return Err(SketchError(format!("{at}: there is no Shape `{}`", cmd.shape))),
                }
                shapes.len() - 1
            }
        };
        let shape = &mut shapes[k].2;
        let result = match &cmd.op {
            SketchOp::Line { from, to } => ops::line(shape, *from, *to),
            SketchOp::Rect { corner, u, v } => ops::rect(shape, *corner, *u, *v),
            SketchOp::Circle { centre, normal, radius, segments } => ops::circle(shape, *centre, *normal, *radius, *segments),
            SketchOp::Arc { centre, start, normal, angle, segments } => ops::arc(shape, *centre, *start, *normal, *angle, *segments),
            SketchOp::PushPull { face, distance, new } => ops::pushpull(shape, *face, *distance, *new),
            SketchOp::Erase { edges } => ops::erase(shape, edges),
        };
        result.map_err(|m| SketchError(format!("{at}: {m}")))?;
    }
    let mut out = Outcome { ops: Vec::new(), names: BTreeMap::new() };
    for (id, name, shape, created) in shapes {
        if created {
            out.ops.push(Op::Create { id, kind: "Shape".to_string() });
            out.ops.push(Op::Set { id, key: "_name".to_string(), value: Value::Text(name.clone()) });
            out.ops.push(Op::Set { id, key: "_schema".to_string(), value: Value::Num(f64::from(Document::SCHEMA_VERSION)) });
        }
        for (key, value) in shape.props() {
            out.ops.push(Op::Set { id, key, value });
        }
        out.names.insert(id, name);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
