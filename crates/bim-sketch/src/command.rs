//! The command file of `bim sketch` (spec/sketch.md): `{"schema": 1, "commands": [...]}` read into
//! typed [`Command`]s. Every field is checked here, so a malformed file is rejected before any
//! geometry is touched.

use crate::geom::V3;
use crate::json::{self, Json};

/// One sketch command, with the name of the Shape it acts on.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub shape: String,
    pub op: Op,
}

/// The sketch operations of spec/sketch.md.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Line { from: V3, to: V3 },
    Rect { corner: V3, u: V3, v: V3 },
    Circle { centre: V3, normal: V3, radius: f64, segments: usize },
    Arc { centre: V3, start: V3, normal: V3, angle: f64, segments: usize },
    PushPull { face: u64, distance: f64, new: bool },
    Erase { edges: Vec<(usize, usize)> },
    Offset { face: u64, distance: f64 },
    FollowMe { face: u64, path: Vec<V3> },
    /// `move`, `rotate` or `scale` of a selection, optionally as `array` copies.
    Transform { selection: Selection, xf: Xf, copy: bool, array: u64 },
    /// `paint`: the Product (by the id or name the file gives) or `None` to clear.
    Paint { faces: Vec<u64>, product: Option<String> },
    MakeComponent { definition: String, instance: String, name: String },
}

/// What a `move`, `rotate` or `scale` acts on: vertex indices, edges (index pairs) and face ids.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub vertices: Vec<usize>,
    pub edges: Vec<(usize, usize)>,
    pub faces: Vec<u64>,
}

/// The transformation of a `move`, `rotate` or `scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Xf {
    Move { vector: V3 },
    Rotate { centre: V3, axis: V3, angle: f64 },
    Scale { origin: V3, factors: V3 },
}

impl Op {
    /// Whether the command draws (and so creates a missing Shape).
    pub fn draws(&self) -> bool {
        matches!(self, Op::Line { .. } | Op::Rect { .. } | Op::Circle { .. } | Op::Arc { .. })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Op::Line { .. } => "line",
            Op::Rect { .. } => "rect",
            Op::Circle { .. } => "circle",
            Op::Arc { .. } => "arc",
            Op::PushPull { .. } => "pushpull",
            Op::Erase { .. } => "erase",
            Op::Offset { .. } => "offset",
            Op::FollowMe { .. } => "followme",
            Op::Transform { xf: Xf::Move { .. }, .. } => "move",
            Op::Transform { xf: Xf::Rotate { .. }, .. } => "rotate",
            Op::Transform { xf: Xf::Scale { .. }, .. } => "scale",
            Op::Paint { .. } => "paint",
            Op::MakeComponent { .. } => "make_component",
        }
    }
}

/// The members of one JSON object, taken one by one; [`Fields::finish`] rejects leftovers.
struct Fields<'a> {
    members: Vec<(&'a str, &'a Json)>,
    at: String,
}

impl<'a> Fields<'a> {
    fn new(v: &'a Json, at: &str) -> Result<Fields<'a>, String> {
        let Json::Object(members) = v else { return Err(format!("{at}: expected an object, found {}", v.type_name())) };
        for (i, (k, _)) in members.iter().enumerate() {
            if members[..i].iter().any(|(j, _)| j == k) {
                return Err(format!("{at}: duplicate member `{k}`"));
            }
        }
        Ok(Fields { members: members.iter().map(|(k, v)| (k.as_str(), v)).collect(), at: at.to_string() })
    }

    fn take(&mut self, name: &str) -> Option<&'a Json> {
        let i = self.members.iter().position(|(k, _)| *k == name)?;
        Some(self.members.remove(i).1)
    }

    fn require(&mut self, name: &str) -> Result<&'a Json, String> {
        self.take(name).ok_or_else(|| format!("{}: missing field `{name}`", self.at))
    }

    fn string(&mut self, name: &str) -> Result<String, String> {
        match self.require(name)? {
            Json::Str(s) => Ok(s.clone()),
            other => Err(format!("{}: `{name}` must be a string, found {}", self.at, other.type_name())),
        }
    }

    fn point(&mut self, name: &str) -> Result<V3, String> {
        let v = self.require(name)?;
        point(v).ok_or_else(|| format!("{}: `{name}` must be three finite numbers", self.at))
    }

    fn number(&mut self, name: &str) -> Result<f64, String> {
        match self.require(name)? {
            Json::Num(x) => Ok(*x),
            other => Err(format!("{}: `{name}` must be a number, found {}", self.at, other.type_name())),
        }
    }

    fn integer(&mut self, name: &str, default: Option<u64>) -> Result<u64, String> {
        match self.take(name) {
            None => default.ok_or_else(|| format!("{}: missing field `{name}`", self.at)),
            Some(Json::Num(x)) if x.fract() == 0.0 && *x >= 0.0 && *x < 1e15 => Ok(*x as u64),
            Some(other) => Err(format!("{}: `{name}` must be a non-negative integer, found {other:?}", self.at)),
        }
    }

    fn integers(&mut self, name: &str) -> Result<Vec<u64>, String> {
        match self.take(name) {
            None => Ok(Vec::new()),
            Some(Json::Array(items)) => items.iter().map(|v| index(v).ok_or_else(|| format!("{}: `{name}` entries must be non-negative integers, found {v:?}", self.at))).collect(),
            Some(other) => Err(format!("{}: `{name}` must be an array, found {}", self.at, other.type_name())),
        }
    }

    fn pairs(&mut self, name: &str) -> Result<Vec<(usize, usize)>, String> {
        match self.take(name) {
            None => Ok(Vec::new()),
            Some(Json::Array(pairs)) => pairs
                .iter()
                .map(|pair| match pair {
                    Json::Array(ab) if ab.len() == 2 => match (index(&ab[0]), index(&ab[1])) {
                        (Some(a), Some(b)) => Ok((a as usize, b as usize)),
                        _ => Err(format!("{}: `{name}` entries must be vertex index pairs, found {pair:?}", self.at)),
                    },
                    other => Err(format!("{}: `{name}` entries must be index pairs, found {other:?}", self.at)),
                })
                .collect(),
            Some(other) => Err(format!("{}: `{name}` must be an array, found {}", self.at, other.type_name())),
        }
    }

    fn points(&mut self, name: &str) -> Result<Vec<V3>, String> {
        match self.require(name)? {
            Json::Array(items) => items.iter().map(|v| point(v).ok_or_else(|| format!("{}: `{name}` entries must be three finite numbers", self.at))).collect(),
            other => Err(format!("{}: `{name}` must be an array of points, found {}", self.at, other.type_name())),
        }
    }

    fn bool_or(&mut self, name: &str, default: bool) -> Result<bool, String> {
        match self.take(name) {
            None => Ok(default),
            Some(Json::Bool(b)) => Ok(*b),
            Some(other) => Err(format!("{}: `{name}` must be true or false, found {}", self.at, other.type_name())),
        }
    }

    fn finish(self) -> Result<(), String> {
        match self.members.first() {
            Some((k, _)) => Err(format!("{}: unknown field `{k}`", self.at)),
            None => Ok(()),
        }
    }
}

fn index(v: &Json) -> Option<u64> {
    match v {
        Json::Num(x) if x.fract() == 0.0 && *x >= 0.0 && *x < 1e15 => Some(*x as u64),
        _ => None,
    }
}

fn point(v: &Json) -> Option<V3> {
    let Json::Array(items) = v else { return None };
    if items.len() != 3 {
        return None;
    }
    let mut p = [0.0; 3];
    for (k, item) in items.iter().enumerate() {
        match item {
            Json::Num(x) if x.is_finite() => p[k] = *x,
            _ => return None,
        }
    }
    Some(p)
}

/// Parses a command file. The error names the command index and field at fault.
pub fn parse(text: &str) -> Result<Vec<Command>, String> {
    let root = json::parse(text)?;
    let mut top = Fields::new(&root, "command file")?;
    match top.require("schema")? {
        Json::Num(x) if *x == 1.0 => {}
        other => return Err(format!("command file: unsupported `schema` {other:?} (expected 1)")),
    }
    let commands = top.require("commands")?;
    top.finish()?;
    let Json::Array(items) = commands else { return Err(format!("command file: `commands` must be an array, found {}", commands.type_name())) };
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let at = format!("commands[{i}]");
        let mut f = Fields::new(item, &at)?;
        let name = f.string("op")?;
        let shape = f.string("shape")?;
        let op = match name.as_str() {
            "line" => Op::Line { from: f.point("from")?, to: f.point("to")? },
            "rect" => Op::Rect { corner: f.point("corner")?, u: f.point("u")?, v: f.point("v")? },
            "circle" => Op::Circle {
                centre: f.point("centre")?,
                normal: f.point("normal")?,
                radius: f.number("radius")?,
                segments: f.integer("segments", Some(24))? as usize,
            },
            "arc" => Op::Arc {
                centre: f.point("centre")?,
                start: f.point("start")?,
                normal: f.point("normal")?,
                angle: f.number("angle")?,
                segments: f.integer("segments", Some(12))? as usize,
            },
            "pushpull" => Op::PushPull { face: f.integer("face", None)?, distance: f.number("distance")?, new: f.bool_or("new", false)? },
            "erase" => {
                if f.members.iter().all(|(k, _)| *k != "edges") {
                    return Err(format!("{at}: missing field `edges`"));
                }
                let edges = f.pairs("edges")?;
                if edges.is_empty() {
                    return Err(format!("{at}: `edges` must name at least one edge"));
                }
                Op::Erase { edges }
            }
            "offset" => Op::Offset { face: f.integer("face", None)?, distance: f.number("distance")? },
            "followme" => Op::FollowMe { face: f.integer("face", None)?, path: f.points("path")? },
            "move" | "rotate" | "scale" => {
                let selection = Selection { vertices: f.integers("vertices")?.into_iter().map(|v| v as usize).collect(), edges: f.pairs("edges")?, faces: f.integers("faces")? };
                if selection.vertices.is_empty() && selection.edges.is_empty() && selection.faces.is_empty() {
                    return Err(format!("{at}: `{name}` needs a selection (`vertices`, `edges` or `faces`)"));
                }
                let xf = match name.as_str() {
                    "move" => Xf::Move { vector: f.point("vector")? },
                    "rotate" => Xf::Rotate { centre: f.point("centre")?, axis: f.point("axis")?, angle: f.number("angle")? },
                    _ => Xf::Scale { origin: f.point("origin")?, factors: f.point("factors")? },
                };
                let copy = f.bool_or("copy", false)?;
                let array = f.integer("array", Some(1))?;
                if array < 1 {
                    return Err(format!("{at}: `array` must be at least 1"));
                }
                if array > 1 && !copy {
                    return Err(format!("{at}: `array` needs `copy: true`"));
                }
                if array > 10_000 {
                    return Err(format!("{at}: `array` must be at most 10000"));
                }
                Op::Transform { selection, xf, copy, array }
            }
            "paint" => {
                let faces = f.integers("faces")?;
                if faces.is_empty() {
                    return Err(format!("{at}: `faces` must name at least one face"));
                }
                let product = match f.require("product")? {
                    Json::Null => None,
                    Json::Str(s) if !s.is_empty() => Some(s.clone()),
                    other => return Err(format!("{at}: `product` must be a Product id or null, found {other:?}")),
                };
                Op::Paint { faces, product }
            }
            "make_component" => Op::MakeComponent { definition: f.string("definition")?, instance: f.string("instance")?, name: f.string("name")? },
            other => return Err(format!("{at}: unknown op `{other}`")),
        };
        f.finish()?;
        out.push(Command { shape, op });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_spec_example() {
        let cmds = parse(
            r#"{"schema": 1, "commands": [
              {"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]},
              {"op": "pushpull", "shape": "box", "face": 1, "distance": 2.5}]}"#,
        )
        .unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[1].op, Op::PushPull { face: 1, distance: 2.5, new: false });
        assert_eq!(parse(r#"{"schema": 1, "commands": []}"#).unwrap(), vec![]);
    }

    #[test]
    fn rejects_bad_files() {
        for (text, want) in [
            (r#"{"schema": 1}"#, "missing field `commands`"),
            (r#"{"schema": 2, "commands": []}"#, "schema"),
            (r#"{"schema": 1, "commands": [ {"op": "rect""#, "invalid JSON"),
            (r#"{"schema": 1, "commands": [{"op": "extrude-everything", "shape": "b"}]}"#, "unknown op"),
            (r#"{"schema": 1, "commands": [{"op": "offset", "shape": "b", "face": 1}]}"#, "missing field `distance`"),
            (r#"{"schema": 1, "commands": [{"op": "move", "shape": "b", "vector": [1, 0, 0]}]}"#, "selection"),
            (r#"{"schema": 1, "commands": [{"op": "move", "shape": "b", "faces": [1], "vector": [1, 0, 0], "array": 3}]}"#, "`copy: true`"),
            (r#"{"schema": 1, "commands": [{"op": "rotate", "shape": "b", "faces": [1], "centre": [0, 0, 0], "angle": 1}]}"#, "`axis`"),
            (r#"{"schema": 1, "commands": [{"op": "paint", "shape": "b", "faces": [1]}]}"#, "`product`"),
            (r#"{"schema": 1, "commands": [{"op": "followme", "shape": "b", "face": 1, "path": [[0, 0]]}]}"#, "`path`"),
            (r#"{"schema": 1, "commands": [{"op": "make_component", "shape": "b", "definition": "d", "name": "n"}]}"#, "`instance`"),
            (r#"{"schema": 1, "commands": [{"op": "rect", "shape": "b", "corner": [0, 0], "u": [1, 0, 0], "v": [0, 1, 0]}]}"#, "`corner`"),
            (r#"{"schema": 1, "commands": [{"op": "pushpull", "shape": "b", "face": 1.5, "distance": 1}]}"#, "`face`"),
            (r#"{"schema": 1, "commands": [{"op": "pushpull", "shape": "b", "face": 1, "distance": 1, "extra": 1}]}"#, "unknown field"),
            (r#"{"schema": 1, "commands": [{"op": "erase", "shape": "b", "edges": [[0]]}]}"#, "`edges`"),
            (r#"{"schema": 1, "commands": [{"op": "line", "from": [0,0,0], "to": [1,0,0]}]}"#, "missing field `shape`"),
            (r#"{"schema": 1, "commands": [{"op": "circle", "shape": "c", "centre": [0,0,0], "normal": [0,0,1], "radius": 1, "segments": -2}]}"#, "`segments`"),
        ] {
            let err = parse(text).unwrap_err();
            assert!(err.contains(want), "{text}: {err}");
        }
    }
}
