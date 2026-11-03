//! The rooms of the document, built exactly as the `arch` trade builds them (spec/schema.md
//! "Rooms"), but keeping, for each room, the walls that bound it and the Level it is on, which
//! `bim solve --trade arch` does not report and the checks need.
//!
//! This repeats the room loop of `bim_arch` (whose `room_json` is private) on its public pieces: per
//! Level, the bounded faces of the arrangement of the centrelines of the Walls the geometry
//! compiler built ([`bim_arch::arrangement::bounded_faces`], vertices merged within
//! [`bim_model::JOIN_TOL`], walls in name order so a collinear overlap belongs to the first wall by
//! name), each face's inner polygon ([`bim_arch::rooms::inner_polygon`]: every edge offset by half
//! the thickness of the wall carrying it) and its area; a face with no inner polygon or no positive
//! area is no room. As in the arch engine, a wall's `level` and centreline are its own props and its
//! thickness is the compiled wall's.

use std::collections::HashMap;

use bim_arch::arrangement::{bounded_faces, Segment};
use bim_arch::rooms::{area, centroid, contains, inner_polygon};
use bim_doc::{Document, Entity, EntityId, Value};
use bim_model::JOIN_TOL;
use bim_plugin::EngineContext;

/// One room.
#[derive(Debug, Clone)]
pub(crate) struct Room {
    /// The Level it is on.
    pub level: EntityId,
    /// Its inner-face polygon, counter-clockwise.
    pub boundary: Vec<[f64; 2]>,
    /// The polygon's area.
    pub area: f64,
    /// The polygon's area centroid.
    pub centroid: [f64; 2],
    /// The distinct Walls carrying its edges, in order of first appearance along the boundary.
    pub walls: Vec<EntityId>,
}

/// A finite number prop of `e` (its own, as the arch engine reads it).
fn own_num(e: &Entity, key: &str) -> Option<f64> {
    match e.props.get(key) {
        Some(Value::Num(x)) if x.is_finite() => Some(*x),
        _ => None,
    }
}

/// The id of the live Level that `e.level` refers to.
fn level_of(doc: &Document, e: &Entity) -> Option<EntityId> {
    match e.props.get("level") {
        Some(Value::Ref(id)) if doc.get(*id).is_some_and(|l| l.kind == "Level") => Some(*id),
        _ => None,
    }
}

/// Every room of every Level, level by level (elevation, then name).
pub(crate) fn rooms(ctx: &EngineContext<'_>, ents: &[(String, &Entity)]) -> Vec<Room> {
    let doc = ctx.document();
    let thickness: HashMap<EntityId, f64> = ctx.model().walls.iter().map(|w| (w.id, w.thickness)).collect();

    struct Wall {
        id: EntityId,
        level: EntityId,
        segment: Segment,
        half: f64,
    }
    let walls: Vec<Wall> = ents
        .iter()
        .filter(|(_, e)| e.kind == "Wall")
        .filter_map(|(_, e)| {
            let half = *thickness.get(&e.id)? / 2.0;
            let level = level_of(doc, e)?;
            let a = [own_num(e, "x1")?, own_num(e, "y1")?];
            let b = [own_num(e, "x2")?, own_num(e, "y2")?];
            Some(Wall { id: e.id, level, segment: Segment { a, b, owner: 0 }, half })
        })
        .filter(|w| (w.segment.b[0] - w.segment.a[0]).hypot(w.segment.b[1] - w.segment.a[1]) > JOIN_TOL)
        .collect();

    let mut levels: Vec<(EntityId, f64)> =
        ents.iter().filter(|(_, e)| e.kind == "Level").filter_map(|(_, e)| Some((e.id, own_num(e, "elevation")?))).collect();
    levels.sort_by(|a, b| a.1.total_cmp(&b.1)); // stable: equal elevations stay in name order

    let mut out = Vec::new();
    for (level, _) in levels {
        let segments: Vec<Segment> = walls
            .iter()
            .enumerate()
            .filter(|(_, w)| w.level == level)
            .map(|(i, w)| Segment { owner: i, ..w.segment })
            .collect();
        for face in bounded_faces(&segments, JOIN_TOL) {
            let half: Vec<f64> = face.owners.iter().map(|&i| walls[i].half).collect();
            let Some(boundary) = inner_polygon(&face.vertices, &half) else { continue };
            let room_area = area(&boundary);
            if !(room_area > 0.0 && room_area.is_finite()) {
                continue;
            }
            let mut carriers: Vec<EntityId> = Vec::new();
            for &i in &face.owners {
                if !carriers.contains(&walls[i].id) {
                    carriers.push(walls[i].id);
                }
            }
            out.push(Room { level, centroid: centroid(&boundary), boundary, area: room_area, walls: carriers });
        }
    }
    out
}

/// The room on `level` that the point `p` lies in: of several nested ones (a face inside another
/// face does not cut a hole in it) the smallest, and of equal ones the first.
pub(crate) fn room_at(rooms: &[Room], level: EntityId, p: [f64; 2]) -> Option<&Room> {
    rooms.iter().filter(|r| r.level == level && contains(&r.boundary, p)).min_by(|a, b| a.area.total_cmp(&b.area))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdoc::{base_doc, id, rect_room};
    use bim_doc::{Op, Value};

    #[test]
    fn a_rectangle_is_one_room_bounded_by_its_four_walls() {
        let mut doc = base_doc();
        rect_room(&mut doc, "a", "L0", 0.0, 0.0, 5.0, 3.0);
        let ctx = EngineContext::new(&doc);
        let ents = ctx.entities();
        let found = rooms(&ctx, &ents);
        assert_eq!(found.len(), 1);
        let room = &found[0];
        assert!((room.area - 4.8 * 2.8).abs() <= 1e-12, "{}", room.area);
        assert!((room.centroid[0] - 2.5).abs() <= 1e-12 && (room.centroid[1] - 1.5).abs() <= 1e-12);
        assert_eq!(room.walls.len(), 4);
        assert_eq!(room.level, id("L0"));
        assert!(room_at(&found, id("L0"), [2.5, 1.5]).is_some());
        assert!(room_at(&found, id("L0"), [2.5, 3.5]).is_none());
        assert!(room_at(&found, id("T"), [2.5, 1.5]).is_none());
    }

    #[test]
    fn nested_rooms_pick_the_smallest_and_levels_are_separate() {
        let mut doc = base_doc();
        crate::testdoc::create(&mut doc, "L1", "Level", vec![("elevation", Value::Num(3.0))]);
        rect_room(&mut doc, "outer", "L0", 0.0, 0.0, 10.0, 10.0);
        rect_room(&mut doc, "inner", "L0", 4.0, 4.0, 6.0, 6.0);
        rect_room(&mut doc, "up", "L1", 0.0, 0.0, 10.0, 10.0);
        let ctx = EngineContext::new(&doc);
        let ents = ctx.entities();
        let found = rooms(&ctx, &ents);
        assert_eq!(found.len(), 3);
        let at = |level: &str, p: [f64; 2]| room_at(&found, id(level), p).map(|r| r.area);
        assert!((at("L0", [5.0, 5.0]).unwrap() - 1.8 * 1.8).abs() <= 1e-9);
        assert!((at("L0", [1.0, 1.0]).unwrap() - 9.8 * 9.8).abs() <= 1e-9);
        assert!((at("L1", [5.0, 5.0]).unwrap() - 9.8 * 9.8).abs() <= 1e-9);
    }

    #[test]
    fn walls_without_a_compiled_geometry_make_no_room() {
        // a wall whose type is not a WallType is left out of the model, and so out of the rooms
        let mut doc = base_doc();
        rect_room(&mut doc, "a", "L0", 0.0, 0.0, 5.0, 3.0);
        doc.apply(Op::Set { id: id("a-w2"), key: "type".to_string(), value: Value::Text("none".to_string()) }).unwrap();
        let ctx = EngineContext::new(&doc);
        let ents = ctx.entities();
        assert!(rooms(&ctx, &ents).is_empty());
    }
}
