//! `bim-arch`: the architectural trade engine ([`ArchEngine`], trade `arch`).
//!
//! Its result is the `bim solve --trade arch --json` document of spec/cli.md:
//! `{"rooms":[{"id","level","area","volume","boundary","space"}],"walls":[{"id","volume","side_area",
//! "layers":[{"material","volume"}]}],"slabs":[{"id","volume","area"}],"stairs":[],"curtain_walls":[]}`.
//!
//! * Walls and slabs are the quantities of the compiled geometry ([`bim_model::Model`]): wall and
//!   per-layer volumes after joins and openings (the material take-off), slab volume and net area.
//! * Rooms (spec/schema.md "Rooms") are computed per Level from the centrelines of its Walls (the
//!   walls the geometry compiler builds): the bounded faces of their planar arrangement
//!   ([`arrangement::bounded_faces`]: split at crossings and T-junctions within
//!   [`bim_model::JOIN_TOL`], dangling and cut edges ignored). A face's room boundary is its inner
//!   face polygon ([`rooms::inner_polygon`]), `area` its area; a face whose inner polygon turns
//!   over, crosses itself or has no positive area is no room. Collinear overlapping walls: the
//!   overlap belongs to the first wall by name. `volume` = area × (next level elevation − level elevation − slab
//!   thickness above), where the slab above is the thickest Slab on the next level (the Levels
//!   with the smallest elevation above) whose boundary contains the room centroid; `volume` is
//!   null on the top level. `space` is the first Space of the level by name whose `x y` lies
//!   inside the boundary, else null. A face nested in another face does not cut a hole in it.
//! * Rooms are listed by level (elevation, then name), each level's numbered `<level>/<n>` from 1
//!   by ascending centroid x, centroids within 1e-6 in x by ascending y.

pub mod arrangement;
pub mod rooms;

use bim_doc::{Document, Entity, EntityId, Value};
use bim_model::{Model, JOIN_TOL};
use bim_plugin::{EngineContext, EngineError, TradeEngine};
use serde_json::{json, Map, Value as Json};

use arrangement::{bounded_faces, Segment};

/// The `arch` trade engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArchEngine;

impl ArchEngine {
    pub fn new() -> Self {
        ArchEngine
    }
}

impl TradeEngine for ArchEngine {
    fn name(&self) -> &str {
        "arch"
    }

    fn solve(&self, ctx: &EngineContext<'_>) -> Result<Json, EngineError> {
        let model = ctx.model();
        let mut out = Map::new();
        out.insert("rooms".into(), Json::Array(room_json(ctx, model)));
        out.insert("walls".into(), Json::Array(model.walls.iter().map(|w| {
            let layers: Vec<Json> =
                w.layers.iter().map(|l| json!({"material": l.material, "volume": num(l.volume)})).collect();
            json!({"id": w.name, "volume": num(w.volume), "side_area": num(w.side_area), "layers": layers})
        }).collect()));
        out.insert("slabs".into(), Json::Array(model.slabs.iter().map(|s| {
            json!({"id": s.name, "volume": num(s.volume), "area": num(s.area)})
        }).collect()));
        out.insert("stairs".into(), Json::Array(Vec::new()));
        out.insert("curtain_walls".into(), Json::Array(Vec::new()));
        Ok(Json::Object(out))
    }
}

/// A finite number as JSON, else null.
fn num(x: f64) -> Json {
    serde_json::Number::from_f64(x).map_or(Json::Null, Json::Number)
}

fn get_num(e: &Entity, key: &str) -> Option<f64> {
    match e.props.get(key) {
        Some(Value::Num(x)) if x.is_finite() => Some(*x),
        _ => None,
    }
}

fn level_of(doc: &Document, e: &Entity) -> Option<EntityId> {
    match e.props.get("level") {
        Some(Value::Ref(id)) if doc.get(*id).is_some_and(|l| l.kind == "Level") => Some(*id),
        _ => None,
    }
}

struct Level {
    id: EntityId,
    name: String,
    elevation: f64,
}

struct SlabIn {
    level: EntityId,
    boundary: Vec<[f64; 2]>,
    thickness: f64,
}

struct Room {
    boundary: Vec<[f64; 2]>,
    area: f64,
    centroid: [f64; 2],
}

/// Every room of the document, as spec/cli.md `rooms[]` entries.
fn room_json(ctx: &EngineContext<'_>, model: &Model) -> Vec<Json> {
    let doc = ctx.document();
    let ents = ctx.entities(); // sorted by name: every choice below is deterministic
    let mut levels: Vec<Level> = ents
        .iter()
        .filter(|(_, e)| e.kind == "Level")
        .filter_map(|(name, e)| Some(Level { id: e.id, name: name.clone(), elevation: get_num(e, "elevation")? }))
        .collect();
    levels.sort_by(|a, b| a.elevation.total_cmp(&b.elevation).then(a.name.cmp(&b.name)));

    // Wall centrelines with the half thickness of the compiled wall.
    let walls: Vec<(EntityId, Segment, f64)> = ents
        .iter()
        .filter(|(_, e)| e.kind == "Wall")
        .filter_map(|(_, e)| {
            let t = model.walls.iter().find(|w| w.id == e.id)?.thickness;
            let level = level_of(doc, e)?;
            let a = [get_num(e, "x1")?, get_num(e, "y1")?];
            let b = [get_num(e, "x2")?, get_num(e, "y2")?];
            Some((level, Segment { a, b, owner: 0 }, t / 2.0))
        })
        .filter(|(_, s, _)| (s.b[0] - s.a[0]).hypot(s.b[1] - s.a[1]) > JOIN_TOL)
        .collect();
    let spaces: Vec<(String, EntityId, [f64; 2])> = ents
        .iter()
        .filter(|(_, e)| e.kind == "Space")
        .filter_map(|(name, e)| Some((name.clone(), level_of(doc, e)?, [get_num(e, "x")?, get_num(e, "y")?])))
        .collect();
    let slabs: Vec<SlabIn> = ents
        .iter()
        .filter(|(_, e)| e.kind == "Slab")
        .filter_map(|(_, e)| {
            let flat: Vec<f64> = match e.props.get("boundary") {
                Some(Value::List(items)) => items
                    .iter()
                    .map(|v| match v {
                        Value::Num(x) if x.is_finite() => Some(*x),
                        _ => None,
                    })
                    .collect::<Option<_>>()?,
                _ => return None,
            };
            if flat.len() < 6 || flat.len() % 2 != 0 {
                return None;
            }
            let boundary = flat.chunks(2).map(|c| [c[0], c[1]]).collect();
            Some(SlabIn { level: level_of(doc, e)?, boundary, thickness: get_num(e, "thickness")? })
        })
        .collect();

    let mut out = Vec::new();
    for level in &levels {
        let segs: Vec<Segment> = walls
            .iter()
            .enumerate()
            .filter(|(_, (l, _, _))| *l == level.id)
            .map(|(i, (_, s, _))| Segment { owner: i, ..*s })
            .collect();
        let mut found: Vec<Room> = bounded_faces(&segs, JOIN_TOL)
            .into_iter()
            .filter_map(|face| {
                let half: Vec<f64> = face.owners.iter().map(|&i| walls[i].2).collect();
                let boundary = rooms::inner_polygon(&face.vertices, &half)?;
                let area = rooms::area(&boundary);
                (area > 0.0 && area.is_finite())
                    .then(|| Room { centroid: rooms::centroid(&boundary), boundary, area })
            })
            .collect();
        order(&mut found);

        // The next level up and the slabs on it.
        let next = levels.iter().map(|l| l.elevation).filter(|&e| e > level.elevation).min_by(f64::total_cmp);
        for (n, room) in found.iter().enumerate() {
            let volume = next.map(|top| {
                let slab = slabs
                    .iter()
                    .filter(|s| levels.iter().any(|l| l.id == s.level && l.elevation == top))
                    .filter(|s| rooms::contains(&s.boundary, room.centroid))
                    .map(|s| s.thickness)
                    .fold(None, |m: Option<f64>, t| Some(m.map_or(t, |m| m.max(t))))
                    .unwrap_or(0.0);
                room.area * (top - level.elevation - slab)
            });
            let space = spaces
                .iter()
                .find(|(_, l, p)| *l == level.id && rooms::contains(&room.boundary, *p))
                .map_or(Json::Null, |(name, _, _)| Json::String(name.clone()));
            let boundary: Vec<Json> = room.boundary.iter().map(|p| json!([num(p[0]), num(p[1])])).collect();
            out.push(json!({
                "id": format!("{}/{}", level.name, n + 1),
                "level": level.name,
                "area": num(room.area),
                "volume": volume.map_or(Json::Null, num),
                "boundary": boundary,
                "space": space,
            }));
        }
    }
    out
}

/// Orders rooms by ascending centroid x; runs of centroids within 1e-6 of each other in x by
/// ascending y.
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

#[cfg(test)]
mod tests;
