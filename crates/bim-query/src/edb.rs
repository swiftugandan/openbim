//! The built-in relations of a document ([`DocumentEdb`]).
//!
//! * `entity(Id, Kind)`: every live entity. `prop(Id, Key, Value)`: every Num, Text and Bool prop
//!   (the reserved `_` props included). `ref(Id, Key, Target)`: every Ref prop and every Ref in a
//!   List prop (at any depth), under the prop's key; targets need not be live.
//! * `room(Room, Level)`, `bounds(Wall, Room)`, `adjacent(A, B)`: the rooms of the `arch` engine
//!   (`bim_arch`: the bounded faces of each Level's wall-centreline arrangement whose inner
//!   polygon has positive area, numbered `<level>/<n>` from 1 by ascending centroid x, then y).
//!   A wall bounds a room when an edge of the room's face lies on its centreline (where collinear
//!   walls overlap, the first by name); two rooms are adjacent when their faces share an edge of
//!   the arrangement (corner contact is not adjacency), in both directions.
//! * `bears_on(Upper, Lower)`: Slab → each Beam on its Level that receives tributary area from it
//!   (the `struct` engine's nearest-beam partition of the slab's net region; area above 1e-9 of
//!   the slab's); Beam → each Column whose `top` is the beam's `level` and whose (x, y) lies on the
//!   beam centreline (ends included) within 1e-6; Column → the Column directly below (same x, y
//!   within 1e-6, its `top` is this one's `base`); Column → each Support within 1e-6 of the
//!   column's base point (x, y, base elevation). Entities without the props a rule needs (or with
//!   non-finite numbers) take part in no edge.
//! * `feeds(A, B)`: for every Segment the `mep` engine reports, `(from, to)` if its solved flow is
//!   positive, `(to, from)` if negative, nothing at zero flow. A failure of the engine is an error
//!   of any query that uses `feeds`.
//!
//! Entities are named as [`EngineContext::name`] does (their `_name`).

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap};

use bim_arch::arrangement::{bounded_faces, Segment as ArchSeg};
use bim_arch::rooms;
use bim_doc::{Entity, EntityId, Value};
use bim_model::JOIN_TOL;
use bim_plugin::{EngineContext, TradeEngine};

use crate::tributary::{self, Segment as TribSeg};
use crate::{Edb, QueryError, Row, Val};

/// Plan matching tolerance of `bears_on` (m), the `struct` engine's node tolerance.
const TOL: f64 = 1e-6;

/// The built-in relations of the document in an [`EngineContext`], each derived on first use.
pub struct DocumentEdb<'c, 'a> {
    ctx: &'c EngineContext<'a>,
    rooms: OnceCell<RoomRelations>,
}

#[derive(Default)]
struct RoomRelations {
    room: Vec<Row>,
    bounds: Vec<Row>,
    adjacent: Vec<Row>,
}

impl<'c, 'a> DocumentEdb<'c, 'a> {
    /// The relations of the document in `ctx`.
    pub fn new(ctx: &'c EngineContext<'a>) -> Self {
        DocumentEdb { ctx, rooms: OnceCell::new() }
    }

    fn name(&self, id: EntityId) -> Val {
        Val::str(&self.ctx.name(id))
    }

    fn rooms(&self) -> &RoomRelations {
        self.rooms.get_or_init(|| room_relations(self.ctx))
    }
}

impl Edb for DocumentEdb<'_, '_> {
    fn relation(&self, name: &str) -> Result<Vec<Row>, QueryError> {
        let doc = self.ctx.document();
        Ok(match name {
            "entity" => doc.entities().map(|e| vec![self.name(e.id), Val::str(&e.kind)]).collect(),
            "prop" => {
                let mut out = Vec::new();
                for e in doc.entities() {
                    for (k, v) in &e.props {
                        let v = match v {
                            Value::Num(x) => Val::num(*x),
                            Value::Text(s) => Val::str(s),
                            Value::Bool(b) => Val::Bool(*b),
                            Value::Ref(_) | Value::List(_) => continue,
                        };
                        out.push(vec![self.name(e.id), Val::str(k), v]);
                    }
                }
                out
            }
            "ref" => {
                let mut out = Vec::new();
                for e in doc.entities() {
                    for (k, v) in &e.props {
                        v.for_each_ref(&mut |t| out.push(vec![self.name(e.id), Val::str(k), self.name(t)]));
                    }
                }
                out
            }
            "room" => self.rooms().room.clone(),
            "bounds" => self.rooms().bounds.clone(),
            "adjacent" => self.rooms().adjacent.clone(),
            "bears_on" => bears_on(self.ctx),
            "feeds" => feeds(self.ctx)?,
            _ => Vec::new(),
        })
    }
}

fn num(e: &Entity, key: &str) -> Option<f64> {
    match e.props.get(key) {
        Some(Value::Num(x)) if x.is_finite() => Some(*x),
        _ => None,
    }
}

fn reference(e: &Entity, key: &str) -> Option<EntityId> {
    match e.props.get(key) {
        Some(Value::Ref(id)) => Some(*id),
        _ => None,
    }
}

/// The live Level `e.<key>` references.
fn level_ref(ctx: &EngineContext<'_>, e: &Entity, key: &str) -> Option<EntityId> {
    reference(e, key).filter(|id| ctx.document().get(*id).is_some_and(|l| l.kind == "Level"))
}

fn nums(e: &Entity, key: &str) -> Option<Vec<f64>> {
    match e.props.get(key) {
        Some(Value::List(items)) => items
            .iter()
            .map(|v| match v {
                Value::Num(x) if x.is_finite() => Some(*x),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn polygon(e: &Entity) -> Option<Vec<[f64; 2]>> {
    let flat = nums(e, "boundary")?;
    (flat.len() >= 6 && flat.len() % 2 == 0).then(|| flat.chunks_exact(2).map(|c| [c[0], c[1]]).collect())
}

// ---------------------------------------------------------------- rooms (as bim_arch does)

struct Room {
    centroid: [f64; 2],
    /// Indices of the bounding walls.
    walls: BTreeSet<usize>,
    /// The face's edges, as vertex coordinate bits, counter-clockwise.
    edges: Vec<([u64; 2], [u64; 2])>,
}

fn room_relations(ctx: &EngineContext<'_>) -> RoomRelations {
    let model = ctx.model();
    let ents = ctx.entities(); // by name, as the arch engine visits them
    let mut levels: Vec<(EntityId, String, f64)> = ents
        .iter()
        .filter(|(_, e)| e.kind == "Level")
        .filter_map(|(name, e)| Some((e.id, name.clone(), num(e, "elevation")?)))
        .collect();
    levels.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.1.cmp(&b.1)));

    // Wall centrelines with the half thickness of the compiled wall.
    let walls: Vec<(String, EntityId, ArchSeg, f64)> = ents
        .iter()
        .filter(|(_, e)| e.kind == "Wall")
        .filter_map(|(name, e)| {
            let t = model.walls.iter().find(|w| w.id == e.id)?.thickness;
            let level = level_ref(ctx, e, "level")?;
            let a = [num(e, "x1")?, num(e, "y1")?];
            let b = [num(e, "x2")?, num(e, "y2")?];
            Some((name.clone(), level, ArchSeg { a, b, owner: 0 }, t / 2.0))
        })
        .filter(|(_, _, s, _)| (s.b[0] - s.a[0]).hypot(s.b[1] - s.a[1]) > JOIN_TOL)
        .collect();

    let bits = |p: [f64; 2]| [p[0].to_bits(), p[1].to_bits()];
    let mut out = RoomRelations::default();
    for (level_id, level_name, _) in &levels {
        let segs: Vec<ArchSeg> = walls
            .iter()
            .enumerate()
            .filter(|(_, w)| w.1 == *level_id)
            .map(|(i, w)| ArchSeg { owner: i, ..w.2 })
            .collect();
        let mut found: Vec<Room> = bounded_faces(&segs, JOIN_TOL)
            .into_iter()
            .filter_map(|face| {
                let half: Vec<f64> = face.owners.iter().map(|&i| walls[i].3).collect();
                let boundary = rooms::inner_polygon(&face.vertices, &half)?;
                let area = rooms::area(&boundary);
                if !(area > 0.0 && area.is_finite()) {
                    return None;
                }
                let n = face.vertices.len();
                let edges = (0..n).map(|i| (bits(face.vertices[i]), bits(face.vertices[(i + 1) % n]))).collect();
                Some(Room { centroid: rooms::centroid(&boundary), walls: face.owners.iter().copied().collect(), edges })
            })
            .collect();
        order(&mut found);

        let ids: Vec<Val> = (0..found.len()).map(|n| Val::str(&format!("{level_name}/{}", n + 1))).collect();
        let level = Val::str(level_name);
        let mut edge_room: HashMap<([u64; 2], [u64; 2]), usize> = HashMap::new();
        for (k, room) in found.iter().enumerate() {
            out.room.push(vec![ids[k].clone(), level.clone()]);
            for &w in &room.walls {
                out.bounds.push(vec![Val::str(&walls[w].0), ids[k].clone()]);
            }
            for &e in &room.edges {
                edge_room.insert(e, k);
            }
        }
        for (k, room) in found.iter().enumerate() {
            for &(u, v) in &room.edges {
                if let Some(&j) = edge_room.get(&(v, u)).filter(|&&j| j != k) {
                    out.adjacent.push(vec![ids[k].clone(), ids[j].clone()]);
                }
            }
        }
    }
    out
}

/// Orders rooms by ascending centroid x; runs of centroids within 1e-6 of each other in x by
/// ascending y (the arch engine's numbering).
fn order(rooms: &mut [Room]) {
    rooms.sort_by(|a, b| a.centroid[0].total_cmp(&b.centroid[0]).then(a.centroid[1].total_cmp(&b.centroid[1])));
    let mut start = 0;
    for i in 1..=rooms.len() {
        if i == rooms.len() || rooms[i].centroid[0] - rooms[i - 1].centroid[0] > 1e-6 {
            rooms[start..i].sort_by(|a, b| a.centroid[1].total_cmp(&b.centroid[1]));
            start = i;
        }
    }
}

// ---------------------------------------------------------------- bears_on

fn dist_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

fn polygon_area(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    0.5 * (0..n).map(|i| pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1]).sum::<f64>()
}

struct Column {
    name: Val,
    xy: [f64; 2],
    base: EntityId,
    top: EntityId,
    base_z: f64,
}

struct Beam {
    name: Val,
    level: EntityId,
    a: [f64; 2],
    b: [f64; 2],
}

fn bears_on(ctx: &EngineContext<'_>) -> Vec<Row> {
    let ents = ctx.entities();
    let doc = ctx.document();
    let of_kind = |kind: &'static str| ents.iter().filter(move |(_, e)| e.kind == kind);
    let elevation = |id: EntityId| doc.get(id).and_then(|l| num(l, "elevation"));

    let columns: Vec<Column> = of_kind("Column")
        .filter_map(|(name, e)| {
            let base = level_ref(ctx, e, "base")?;
            let top = level_ref(ctx, e, "top")?;
            Some(Column { name: Val::str(name), xy: [num(e, "x")?, num(e, "y")?], base, top, base_z: elevation(base)? })
        })
        .collect();
    let beams: Vec<Beam> = of_kind("Beam")
        .filter_map(|(name, e)| {
            let a = [num(e, "x1")?, num(e, "y1")?];
            let b = [num(e, "x2")?, num(e, "y2")?];
            ((b[0] - a[0]).hypot(b[1] - a[1]) > TOL)
                .then(|| Some(Beam { name: Val::str(name), level: level_ref(ctx, e, "level")?, a, b }))?
        })
        .collect();

    let mut out = Vec::new();
    // Slab → Beam: beams that receive tributary area from the slab.
    for (name, e) in of_kind("Slab") {
        let (Some(level), Some(outer)) = (level_ref(ctx, e, "level"), polygon(e)) else { continue };
        let holes: Vec<Vec<[f64; 2]>> = of_kind("SlabOpening")
            .filter(|(_, h)| reference(h, "host") == Some(e.id))
            .filter_map(|(_, h)| polygon(h))
            .collect();
        let support: Vec<&Beam> = beams.iter().filter(|b| b.level == level).collect();
        if support.is_empty() {
            continue;
        }
        let segs: Vec<TribSeg> = support.iter().map(|b| TribSeg { a: b.a, b: b.b }).collect();
        let part = tributary::partition(&outer, &holes, &segs);
        let threshold = 1e-9 * polygon_area(&outer).abs();
        for (k, b) in support.iter().enumerate() {
            if part.area(k) > threshold {
                out.push(vec![Val::str(name), b.name.clone()]);
            }
        }
    }
    // Beam → Column: columns topped at the beam's level whose axis meets the beam.
    for b in &beams {
        for c in columns.iter().filter(|c| c.top == b.level) {
            if dist_to_segment(c.xy, b.a, b.b) <= TOL {
                out.push(vec![b.name.clone(), c.name.clone()]);
            }
        }
    }
    // Column → Column below, Column → Support at the base.
    for c in &columns {
        for d in columns.iter().filter(|d| d.top == c.base) {
            if (c.xy[0] - d.xy[0]).hypot(c.xy[1] - d.xy[1]) <= TOL {
                out.push(vec![c.name.clone(), d.name.clone()]);
            }
        }
        for (name, s) in of_kind("Support") {
            let (Some(x), Some(y), Some(z)) = (num(s, "x"), num(s, "y"), num(s, "z")) else { continue };
            let d = ((x - c.xy[0]).powi(2) + (y - c.xy[1]).powi(2) + (z - c.base_z).powi(2)).sqrt();
            if d <= TOL {
                out.push(vec![c.name.clone(), Val::str(name)]);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- feeds

fn feeds(ctx: &EngineContext<'_>) -> Result<Vec<Row>, QueryError> {
    let result = bim_mep::MepEngine::new().solve(ctx).map_err(QueryError::Engine)?;
    let by_name: HashMap<String, &Entity> = ctx.entities().into_iter().collect();
    let mut out = Vec::new();
    let segments = result.get("segments").and_then(|s| s.as_array()).map(Vec::as_slice).unwrap_or(&[]);
    for s in segments {
        let (Some(id), Some(flow)) = (s.get("id").and_then(|v| v.as_str()), s.get("flow").and_then(|v| v.as_f64()))
        else {
            continue;
        };
        let Some(e) = by_name.get(id) else { continue };
        let (Some(from), Some(to)) = (reference(e, "from"), reference(e, "to")) else { continue };
        let (from, to) = (Val::str(&ctx.name(from)), Val::str(&ctx.name(to)));
        if flow > 0.0 {
            out.push(vec![from, to]);
        } else if flow < 0.0 {
            out.push(vec![to, from]);
        }
    }
    Ok(out)
}
