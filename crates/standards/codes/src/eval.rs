//! Rule evaluation: what each check measures and when a measurement is a violation.
//!
//! For each rule of a pack, in pack order, and for each live entity of the rule's `applies_to` kind
//! in name order (`EngineContext::entities`), that satisfies the rule's `where`:
//!
//! 1. The check *measures* the entity. A quantity that cannot be measured (a missing, non-finite or
//!    ill-typed prop, a Ref that does not lead to the right kind, a Space that lies in no room, a
//!    room with no door, no level or slab above) is no measurement and no violation, and a
//!    non-finite result is none either.
//! 2. The measurement is compared with the limit. Every comparison is strict and exact, with no
//!    tolerance: `measured < limit` for `min_width`, `min_area`, `min_clear_height`, `riser_min` and
//!    `tread_min`; `measured > limit` for `riser_max`, `u_value_max`, `slope_max` and
//!    `egress_distance_max`; for `blondel`, `measured < lo` or `measured > hi`, the reported limit
//!    being the bound crossed.
//!
//! All props are *effective* props (spec/catalogs.md): where an entity has a `product` Ref, a prop
//! it lacks itself is the product's, and its own value always wins. `where` compares the JSON
//! encoding of that prop (`EngineContext::value_json`, Refs as `{"ref": "<name>"}`) with the pack's
//! value: numbers by value (`1` equals `1.0`), arrays element by element, objects as key sets; a
//! missing prop or a different JSON type does not match.
//!
//! Rooms are those of the `arch` trade ([`crate::rooms`]). A Space's room is the room on the
//! Space's `level` that contains its `x y` (the smallest if rooms are nested), and is computed once
//! per run, only if a rule needs it.

use std::cell::OnceCell;

use bim_arch::rooms::contains;
use bim_doc::{Document, Entity, EntityId, Value};
use bim_plugin::{EngineContext, Violation};
use serde_json::Value as Json;

use crate::geom;
use crate::pack::{Check, Limit, Pack, Rule};
use crate::rooms::{self, Room};

/// Every violation of `pack` in the document of `ctx`: by rule in pack order, within a rule by
/// entity name (byte-wise).
pub(crate) fn run(pack: &Pack, ctx: &EngineContext<'_>) -> Vec<Violation> {
    let mut run = Run { ctx, doc: ctx.document(), ents: ctx.entities(), levels: Vec::new(), rooms: OnceCell::new() };
    let levels: Vec<(EntityId, f64)> =
        run.ents.iter().filter(|(_, e)| e.kind == "Level").filter_map(|(_, e)| Some((e.id, run.num(e, "elevation")?))).collect();
    run.levels = levels;

    let mut out = Vec::new();
    for rule in &pack.rules {
        for &(ref name, entity) in &run.ents {
            if entity.kind != rule.applies_to || !run.matches(entity, &rule.where_) {
                continue;
            }
            let Some(measured) = run.measure(rule.check, entity).filter(|m| m.is_finite()) else { continue };
            if let Some((bound, below)) = breach(rule.check, rule.limit, measured) {
                out.push(violation(rule, name, measured, bound, below));
            }
        }
    }
    out
}

/// The bound that `measured` crosses, and whether it falls below it, if it breaks `limit`.
fn breach(check: Check, limit: Limit, measured: f64) -> Option<(f64, bool)> {
    match (check, limit) {
        (Check::Blondel, Limit::Range(lo, hi)) => {
            if measured < lo {
                Some((lo, true))
            } else if measured > hi {
                Some((hi, false))
            } else {
                None
            }
        }
        (Check::MinWidth | Check::MinArea | Check::MinClearHeight | Check::RiserMin | Check::TreadMin, Limit::Value(x)) => {
            (measured < x).then_some((x, true))
        }
        (Check::RiserMax | Check::UValueMax | Check::SlopeMax | Check::EgressDistanceMax, Limit::Value(x)) => {
            (measured > x).then_some((x, false))
        }
        _ => None, // a limit of the wrong shape for its check: Pack::parse never makes one
    }
}

fn violation(rule: &Rule, entity: &str, measured: f64, bound: f64, below: bool) -> Violation {
    let message = format!(
        "{} {entity}: {} {measured} {} limit {bound}",
        rule.applies_to,
        rule.check.name(),
        if below { "<" } else { ">" }
    );
    Violation { rule: rule.id.clone(), entity: entity.to_string(), message, measured, limit: bound }
}

/// JSON equality for `where`: numbers by value (so `1` equals `1.0`), arrays element by element,
/// objects as key sets with equal values, everything else by value; different types differ.
pub(crate) fn json_eq(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Null, Json::Null) => true,
        (Json::Bool(x), Json::Bool(y)) => x == y,
        (Json::Number(x), Json::Number(y)) => x.as_f64().zip(y.as_f64()).is_some_and(|(x, y)| x == y),
        (Json::String(x), Json::String(y)) => x == y,
        (Json::Array(x), Json::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q)),
        (Json::Object(x), Json::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(key, p)| y.get(key).is_some_and(|q| json_eq(p, q)))
        }
        _ => false,
    }
}

struct Run<'c, 'a> {
    ctx: &'c EngineContext<'a>,
    doc: &'a Document,
    /// Every live entity with its name, sorted by name.
    ents: Vec<(String, &'a Entity)>,
    /// Every Level with a finite elevation.
    levels: Vec<(EntityId, f64)>,
    rooms: OnceCell<Vec<Room>>,
}

impl<'a> Run<'_, 'a> {
    // ---------------------------------------------------------------- effective props

    /// `key` of `e`, else of its `product` (spec/catalogs.md "Effective props").
    fn prop(&self, e: &'a Entity, key: &str) -> Option<&'a Value> {
        e.props.get(key).or_else(|| match e.props.get("product") {
            Some(Value::Ref(p)) if *p != e.id => self.doc.get(*p).and_then(|p| p.props.get(key)),
            _ => None,
        })
    }

    /// A finite number prop.
    fn num(&self, e: &'a Entity, key: &str) -> Option<f64> {
        match self.prop(e, key) {
            Some(Value::Num(x)) if x.is_finite() => Some(*x),
            _ => None,
        }
    }

    /// A list prop of finite numbers.
    fn num_list(&self, e: &'a Entity, key: &str) -> Option<Vec<f64>> {
        let Some(Value::List(items)) = self.prop(e, key) else { return None };
        items
            .iter()
            .map(|v| match v {
                Value::Num(x) if x.is_finite() => Some(*x),
                _ => None,
            })
            .collect()
    }

    /// The live entity of `kind` that the Ref prop `key` leads to.
    fn target(&self, e: &'a Entity, key: &str, kind: &str) -> Option<&'a Entity> {
        match self.prop(e, key) {
            Some(Value::Ref(id)) => self.doc.get(*id).filter(|t| t.kind == kind),
            _ => None,
        }
    }

    /// The elevation of the Level that the Ref prop `key` leads to.
    fn level_elevation(&self, e: &'a Entity, key: &str) -> Option<f64> {
        self.num(self.target(e, key, "Level")?, "elevation")
    }

    /// A polygon prop (`x0, y0, x1, y1, …`): at least three finite points.
    fn polygon(&self, e: &'a Entity, key: &str) -> Option<Vec<[f64; 2]>> {
        let flat = self.num_list(e, key)?;
        (flat.len() >= 6 && flat.len() % 2 == 0).then(|| flat.chunks(2).map(|c| [c[0], c[1]]).collect())
    }

    /// Whether every `where` entry equals the entity's (effective) prop.
    fn matches(&self, e: &'a Entity, filter: &[(String, Json)]) -> bool {
        filter.iter().all(|(key, want)| self.prop(e, key).is_some_and(|v| json_eq(&self.ctx.value_json(v), want)))
    }

    // ---------------------------------------------------------------- rooms

    fn rooms(&self) -> &[Room] {
        self.rooms.get_or_init(|| rooms::rooms(self.ctx, &self.ents))
    }

    /// The room a Space labels.
    fn room_of(&self, space: &'a Entity) -> Option<&Room> {
        let level = self.target(space, "level", "Level")?.id;
        let at = [self.num(space, "x")?, self.num(space, "y")?];
        rooms::room_at(self.rooms(), level, at)
    }

    // ---------------------------------------------------------------- checks

    fn measure(&self, check: Check, e: &'a Entity) -> Option<f64> {
        match check {
            // Opening: its `width`. Space: the shorter side of the minimum-area bounding rectangle
            // of its room polygon (the rectangle of a hull edge direction of least area).
            Check::MinWidth => match e.kind.as_str() {
                "Opening" => self.num(e, "width"),
                "Space" => geom::min_rect_short_side(&self.room_of(e)?.boundary),
                _ => None,
            },
            // The area of the room polygon (inside the walls' inner faces).
            Check::MinArea => Some(self.room_of(e)?.area),
            Check::MinClearHeight => self.clear_height(e),
            Check::RiserMax | Check::RiserMin => self.riser(e),
            Check::TreadMin => self.num(e, "tread"),
            // 2 · riser + tread, in that order of operations.
            Check::Blondel => Some(2.0 * self.riser(e)? + self.num(e, "tread")?),
            Check::UValueMax => self.u_value(e),
            Check::SlopeMax => self.slope(e),
            Check::EgressDistanceMax => self.egress(e),
        }
    }

    /// Stair: (top − base elevation) / `riser_count`, the count being positive.
    fn riser(&self, stair: &'a Entity) -> Option<f64> {
        let (top, base) = (self.level_elevation(stair, "top")?, self.level_elevation(stair, "base")?);
        let count = self.num(stair, "riser_count").filter(|n| *n > 0.0)?;
        Some((top - base) / count)
    }

    /// Ramp: (top − base elevation) / `length`, the length being positive.
    fn slope(&self, ramp: &'a Entity) -> Option<f64> {
        let (top, base) = (self.level_elevation(ramp, "top")?, self.level_elevation(ramp, "base")?);
        let length = self.num(ramp, "length").filter(|l| *l > 0.0)?;
        Some((top - base) / length)
    }

    /// Wall: U = 1 / (0.13 + Σ tᵢ/λᵢ + 0.04) over the layers of its WallType (`layer_thickness`
    /// tᵢ ≥ 0, `layer_conductivity` λᵢ > 0, equally many, at least one). `bim-model` ignores the
    /// conductivities, so they are read here.
    fn u_value(&self, wall: &'a Entity) -> Option<f64> {
        let wall_type = self.target(wall, "type", "WallType")?;
        let thickness = self.num_list(wall_type, "layer_thickness")?;
        let conductivity = self.num_list(wall_type, "layer_conductivity")?;
        if thickness.is_empty()
            || thickness.len() != conductivity.len()
            || thickness.iter().any(|t| !(*t >= 0.0))
            || conductivity.iter().any(|l| !(*l > 0.0))
        {
            return None;
        }
        let mut resistance = 0.0;
        for (t, lambda) in thickness.iter().zip(&conductivity) {
            resistance += t / lambda;
        }
        Some(1.0 / (0.13 + resistance + 0.04))
    }

    /// Space: floor-to-underside of the slab above, (next − floor) − t. `floor` is the elevation of
    /// the Space's Level, `next` the smallest Level elevation above it, and `t` the greatest
    /// `thickness` of the Slabs on a Level at `next` whose `boundary` contains the centroid of the
    /// room. With no Level above or no such slab there is nothing to measure (the room volume of
    /// the arch trade treats a missing slab as 0 thick; a code check does not invent a ceiling).
    fn clear_height(&self, space: &'a Entity) -> Option<f64> {
        let room = self.room_of(space)?;
        let floor = self.level_elevation(space, "level")?;
        let next = self.levels.iter().map(|l| l.1).filter(|&z| z > floor).min_by(f64::total_cmp)?;
        let thickest = self
            .ents
            .iter()
            .filter(|(_, e)| e.kind == "Slab")
            .filter_map(|&(_, slab)| {
                if self.level_elevation(slab, "level")? != next {
                    return None;
                }
                let thickness = self.num(slab, "thickness")?;
                contains(&self.polygon(slab, "boundary")?, room.centroid).then_some(thickness)
            })
            .max_by(f64::total_cmp)?;
        Some(next - floor - thickest)
    }

    /// Space: the longest, over the vertices of its room polygon, of the shortest path inside the
    /// polygon to the nearest door. A door is an Opening of `kind` `door` hosted by one of the
    /// walls bounding the room; its point is the one at `offset + width / 2` along the host
    /// centreline from (x1, y1), moved to the nearest point of the room boundary.
    fn egress(&self, space: &'a Entity) -> Option<f64> {
        let room = self.room_of(space)?;
        let mut doors = Vec::new();
        for &(_, opening) in &self.ents {
            if opening.kind != "Opening" || !matches!(self.prop(opening, "kind"), Some(Value::Text(k)) if k == "door") {
                continue;
            }
            let Some(host) = self.target(opening, "host", "Wall").filter(|h| room.walls.contains(&h.id)) else { continue };
            let (Some(offset), Some(width)) = (self.num(opening, "offset"), self.num(opening, "width")) else { continue };
            let (Some(x1), Some(y1), Some(x2), Some(y2)) =
                (self.num(host, "x1"), self.num(host, "y1"), self.num(host, "x2"), self.num(host, "y2"))
            else {
                continue;
            };
            let (dx, dy) = (x2 - x1, y2 - y1);
            let length = dx.hypot(dy);
            if !(length > 0.0) {
                continue;
            }
            let along = offset + width / 2.0;
            let midpoint = [x1 + along * (dx / length), y1 + along * (dy / length)];
            if let Some((point, _)) = geom::nearest_on_boundary(&room.boundary, midpoint) {
                doors.push(point);
            }
        }
        geom::egress_distance(&room.boundary, &doors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdoc::{base_doc, create, id, nums, num, rect_room, reference, text};
    use serde_json::json;

    fn check(doc: &Document, pack: Json) -> Vec<Violation> {
        let pack = Pack::parse(&pack.to_string()).unwrap();
        run(&pack, &EngineContext::new(doc))
    }

    fn rule(id: &str, applies_to: &str, check: &str, limit: Json) -> Json {
        json!({"id": id, "applies_to": applies_to, "check": check, "limit": limit})
    }

    fn pack(rules: Vec<Json>) -> Json {
        json!({"pack": "t", "rules": rules})
    }

    fn keys(violations: &[Violation]) -> Vec<(&str, &str)> {
        violations.iter().map(|v| (v.rule.as_str(), v.entity.as_str())).collect()
    }

    #[test]
    fn json_equality() {
        assert!(json_eq(&json!(1), &json!(1.0)));
        assert!(json_eq(&json!(1.5), &json!(1.5)));
        assert!(!json_eq(&json!(1), &json!(2)));
        assert!(json_eq(&json!("a"), &json!("a")));
        assert!(!json_eq(&json!("a"), &json!({"ref": "a"})));
        assert!(!json_eq(&json!("1"), &json!(1)));
        assert!(!json_eq(&json!(true), &json!(1)));
        assert!(json_eq(&json!(null), &json!(null)));
        assert!(!json_eq(&json!(null), &json!(false)));
        assert!(json_eq(&json!([1, "a", [true]]), &json!([1.0, "a", [true]])));
        assert!(!json_eq(&json!([1, 2]), &json!([2, 1])));
        assert!(!json_eq(&json!([1]), &json!([1, 2])));
        assert!(json_eq(&json!({"a": 1, "b": [2]}), &json!({"b": [2.0], "a": 1})));
        assert!(!json_eq(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
        assert!(!json_eq(&json!({"a": 1, "c": 2}), &json!({"a": 1, "b": 2})));
    }

    #[test]
    fn where_filters_on_props_and_a_missing_prop_does_not_match() {
        let mut doc = base_doc();
        create(&mut doc, "w0", "Wall", vec![("level", reference("L0")), ("type", reference("T"))]);
        for (name, kind, width) in [("d1", "door", 0.7), ("d2", "window", 0.7), ("d3", "door", 1.0)] {
            create(&mut doc, name, "Opening", vec![("host", reference("w0")), ("kind", text(kind)), ("width", num(width))]);
        }
        create(&mut doc, "d4", "Opening", vec![("host", reference("w0")), ("width", num(0.1))]); // no kind
        let with_where = json!({"id": "R", "applies_to": "Opening", "where": {"kind": "door"}, "check": "min_width", "limit": 0.9});
        assert_eq!(keys(&check(&doc, pack(vec![with_where]))), [("R", "d1")]);
        let any = rule("R", "Opening", "min_width", json!(0.9));
        assert_eq!(keys(&check(&doc, pack(vec![any]))), [("R", "d1"), ("R", "d2"), ("R", "d4")]);
        // several keys all must match; a Ref prop compares as {"ref": name}
        let both = json!({"id": "R", "applies_to": "Opening", "check": "min_width", "limit": 0.9,
                          "where": {"kind": "window", "host": {"ref": "w0"}}});
        assert_eq!(keys(&check(&doc, pack(vec![both]))), [("R", "d2")]);
        let wrong_type = json!({"id": "R", "applies_to": "Opening", "where": {"kind": 1}, "check": "min_width", "limit": 0.9});
        assert!(check(&doc, pack(vec![wrong_type])).is_empty());
        let numeric = json!({"id": "R", "applies_to": "Opening", "where": {"width": 1}, "check": "min_width", "limit": 2});
        assert_eq!(keys(&check(&doc, pack(vec![numeric]))), [("R", "d3")]);
    }

    #[test]
    fn comparisons_are_strict_and_report_the_pack_limit() {
        let mut doc = base_doc();
        create(&mut doc, "w0", "Wall", vec![]);
        for (name, width) in [("below", 0.9 - 1e-12), ("equal", 0.9), ("above", 0.9 + 1e-12)] {
            create(&mut doc, name, "Opening", vec![("host", reference("w0")), ("kind", text("door")), ("width", num(width))]);
        }
        let found = check(&doc, pack(vec![rule("R", "Opening", "min_width", json!(0.9))]));
        assert_eq!(keys(&found), [("R", "below")]);
        assert_eq!(found[0].limit, 0.9);
        assert_eq!(found[0].measured, 0.9 - 1e-12);
        assert_eq!(found[0].message, format!("Opening below: min_width {} < limit 0.9", 0.9 - 1e-12));
    }

    /// Stairs from level 0 up `rise` in `count` risers with `tread`.
    fn stair(doc: &mut Document, name: &str, rise: f64, count: f64, tread: f64) {
        let top = format!("{name}-top");
        create(doc, &top, "Level", vec![("elevation", num(rise))]);
        create(
            doc,
            name,
            "Stair",
            vec![("base", reference("L0")), ("top", reference(&top)), ("riser_count", num(count)), ("tread", num(tread))],
        );
    }

    #[test]
    fn stair_checks() {
        let mut doc = base_doc();
        stair(&mut doc, "ok", 3.0, 17.0, 0.28); // riser 0.17647
        stair(&mut doc, "tall", 3.2, 17.0, 0.28); // riser 0.18823
        stair(&mut doc, "shallow", 3.0, 17.0, 0.27);
        let found = check(
            &doc,
            pack(vec![
                rule("max", "Stair", "riser_max", json!(0.1778)),
                rule("min", "Stair", "riser_min", json!(0.18)),
                rule("tread", "Stair", "tread_min", json!(0.2794)),
            ]),
        );
        assert_eq!(keys(&found), [("max", "tall"), ("min", "ok"), ("min", "shallow"), ("tread", "shallow")]);
        assert_eq!(found[0].measured, 3.2 / 17.0);
        assert_eq!(found[3].measured, 0.27);
        assert_eq!(found[3].limit, 0.2794);
    }

    #[test]
    fn blondel_reports_the_bound_it_crosses() {
        let mut doc = base_doc();
        // riser 0.17 -> 2r = 0.34; tread 0.25 -> 0.59 (low), 0.27 -> 0.61 (in), 0.32 -> 0.66 (high)
        for (name, tread) in [("low", 0.25), ("in", 0.27), ("high", 0.32)] {
            stair(&mut doc, name, 3.4, 20.0, tread);
        }
        let found = check(&doc, pack(vec![rule("B", "Stair", "blondel", json!([0.6, 0.65]))]));
        assert_eq!(keys(&found), [("B", "high"), ("B", "low")]);
        assert_eq!((found[0].limit, found[1].limit), (0.65, 0.6));
        assert_eq!(found[0].measured, 2.0 * (3.4 / 20.0) + 0.32);
        assert_eq!(found[1].measured, 2.0 * (3.4 / 20.0) + 0.25);
        assert!(found[0].message.contains('>') && found[1].message.contains('<'), "{:?}", found);
        // exactly on a bound is inside
        let mut doc = base_doc();
        stair(&mut doc, "edge", 2.0, 10.0, 0.25); // 2 * 0.2 + 0.25 = 0.65 exactly
        assert!(check(&doc, pack(vec![rule("B", "Stair", "blondel", json!([0.6, 0.65]))])).is_empty());
    }

    #[test]
    fn unmeasurable_stairs_ramps_and_walls_are_skipped() {
        let mut doc = base_doc();
        create(&mut doc, "no-count", "Stair", vec![("base", reference("L0")), ("top", reference("L0")), ("tread", num(0.1))]);
        create(&mut doc, "zero-count", "Stair", vec![("base", reference("L0")), ("top", reference("L0")), ("riser_count", num(0.0))]);
        create(&mut doc, "no-top", "Stair", vec![("base", reference("L0")), ("riser_count", num(10.0))]);
        create(&mut doc, "top-not-level", "Stair", vec![("base", reference("L0")), ("top", reference("T")), ("riser_count", num(10.0))]);
        create(&mut doc, "ramp0", "Ramp", vec![("base", reference("L0")), ("top", reference("L0")), ("length", num(0.0))]);
        create(&mut doc, "wall0", "Wall", vec![("type", reference("T"))]);
        let all = pack(vec![
            rule("a", "Stair", "riser_max", json!(0.0)),
            rule("b", "Stair", "riser_min", json!(1.0)),
            rule("c", "Stair", "blondel", json!([1.0, 2.0])),
            rule("d", "Ramp", "slope_max", json!(-1.0)),
            rule("e", "Wall", "u_value_max", json!(0.0)),
        ]);
        assert!(check(&doc, all).is_empty());
    }

    fn wall_type(doc: &mut Document, name: &str, t: &[f64], lambda: Option<&[f64]>) {
        let mut props = vec![("layer_thickness", nums(t))];
        if let Some(l) = lambda {
            props.push(("layer_conductivity", nums(l)));
        }
        create(doc, name, "WallType", props);
    }

    #[test]
    fn u_value_is_one_over_the_resistances() {
        let mut doc = base_doc();
        wall_type(&mut doc, "poor", &[0.2, 0.05], Some(&[2.0, 0.035]));
        wall_type(&mut doc, "good", &[0.2, 0.3], Some(&[2.0, 0.035]));
        wall_type(&mut doc, "no-lambda", &[0.2], None);
        wall_type(&mut doc, "short-lambda", &[0.2, 0.1], Some(&[2.0]));
        wall_type(&mut doc, "zero-lambda", &[0.2], Some(&[0.0]));
        wall_type(&mut doc, "negative", &[-0.2], Some(&[1.0]));
        wall_type(&mut doc, "empty", &[], Some(&[]));
        for t in ["poor", "good", "no-lambda", "short-lambda", "zero-lambda", "negative", "empty"] {
            create(&mut doc, &format!("wall-{t}"), "Wall", vec![("type", reference(t))]);
        }
        create(&mut doc, "wall-untyped", "Wall", vec![]);
        // Only the walls with a complete, valid WallType are measured; only the poor one is over 0.35.
        let found = check(&doc, pack(vec![rule("U", "Wall", "u_value_max", json!(0.35))]));
        assert_eq!(keys(&found), [("U", "wall-poor")]);
        let expect = |layers: &[(f64, f64)]| 1.0 / (0.13 + layers.iter().map(|(t, l)| t / l).sum::<f64>() + 0.04);
        assert_eq!(found[0].measured, expect(&[(0.2, 2.0), (0.05, 0.035)]));
        assert_eq!(found[0].limit, 0.35);
        let all = check(&doc, pack(vec![rule("U", "Wall", "u_value_max", json!(0.0))]));
        assert_eq!(keys(&all), [("U", "wall-good"), ("U", "wall-poor")]);
        assert_eq!(all[0].measured, expect(&[(0.2, 2.0), (0.3, 0.035)]));
        assert!(all[0].measured < 0.35, "{}", all[0].measured);
    }

    #[test]
    fn ramp_slope() {
        let mut doc = base_doc();
        create(&mut doc, "top", "Level", vec![("elevation", num(0.5))]);
        for (name, length) in [("steep", 5.0), ("ok", 6.0), ("flat", 60.0)] {
            create(&mut doc, name, "Ramp", vec![("base", reference("L0")), ("top", reference("top")), ("length", num(length))]);
        }
        let found = check(&doc, pack(vec![rule("S", "Ramp", "slope_max", json!(1.0 / 12.0))]));
        assert_eq!(keys(&found), [("S", "steep")]);
        assert_eq!(found[0].measured, 0.1);
        // exactly 1:12 passes
        let mut doc = base_doc();
        create(&mut doc, "top", "Level", vec![("elevation", num(1.0))]);
        create(&mut doc, "r", "Ramp", vec![("base", reference("L0")), ("top", reference("top")), ("length", num(12.0))]);
        assert!(check(&doc, pack(vec![rule("S", "Ramp", "slope_max", json!(1.0 / 12.0))])).is_empty());
    }

    fn space(doc: &mut Document, name: &str, x: f64, y: f64, level: &str, usage: &str) {
        create(doc, name, "Space", vec![("level", reference(level)), ("x", num(x)), ("y", num(y)), ("use", text(usage))]);
    }

    #[test]
    fn room_checks_measure_the_inner_polygon_of_the_spaces_room() {
        let mut doc = base_doc();
        // inner 1.8 x 7.8 (corridor) and 4.8 x 2.8 (office); the corridor is rotated nowhere here
        rect_room(&mut doc, "c", "L0", 0.0, 0.0, 8.0, 2.0);
        space(&mut doc, "corr", 4.0, 1.0, "L0", "corridor");
        rect_room(&mut doc, "o", "L0", 20.0, 0.0, 25.0, 3.0);
        space(&mut doc, "off", 22.0, 1.0, "L0", "office");
        space(&mut doc, "outside", 100.0, 100.0, "L0", "corridor"); // in no room
        create(&mut doc, "lost", "Space", vec![("x", num(4.0)), ("y", num(1.0))]); // no level
        let found = check(
            &doc,
            pack(vec![
                json!({"id": "W", "applies_to": "Space", "check": "min_width", "limit": 2.0}),
                json!({"id": "A", "applies_to": "Space", "where": {"use": "office"}, "check": "min_area", "limit": 14.0}),
                json!({"id": "A2", "applies_to": "Space", "check": "min_area", "limit": 1000.0}),
            ]),
        );
        assert_eq!(keys(&found), [("W", "corr"), ("A", "off"), ("A2", "corr"), ("A2", "off")]);
        assert!((found[0].measured - 1.8).abs() <= 1e-12, "{}", found[0].measured);
        assert!((found[1].measured - 4.8 * 2.8).abs() <= 1e-12);
        assert_eq!(found[1].limit, 14.0);
    }

    /// A slab on `level` between x = `xs[0]` and `xs[1]` (and y = -1 .. 5).
    fn slab(doc: &mut Document, name: &str, level: &str, thickness: f64, xs: [f64; 2]) {
        let boundary = nums(&[xs[0], -1.0, xs[1], -1.0, xs[1], 5.0, xs[0], 5.0]);
        create(doc, name, "Slab", vec![("level", reference(level)), ("thickness", num(thickness)), ("boundary", boundary)]);
    }

    #[test]
    fn clear_height_is_the_next_level_minus_the_slab_above() {
        let mut doc = base_doc();
        create(&mut doc, "L1", "Level", vec![("elevation", num(3.0))]);
        create(&mut doc, "L2", "Level", vec![("elevation", num(6.0))]);
        for (name, x0, thickness) in [("thin", 0.0, 0.25), ("thick", 20.0, 0.35), ("bare", 40.0, 0.0)] {
            rect_room(&mut doc, name, "L0", x0, 0.0, x0 + 5.0, 4.0);
            space(&mut doc, name, x0 + 2.5, 2.0, "L0", "lab");
            if name != "bare" {
                // Over each room: the slab under test, a thinner one (the thickest wins), one over
                // another place and one on the wrong level.
                slab(&mut doc, &format!("{name}-slab"), "L1", thickness, [x0 - 1.0, x0 + 6.0]);
                slab(&mut doc, &format!("{name}-thin"), "L1", 0.1, [x0 - 1.0, x0 + 6.0]);
                slab(&mut doc, &format!("{name}-away"), "L1", 0.9, [x0 + 50.0, x0 + 60.0]);
                slab(&mut doc, &format!("{name}-high"), "L2", 0.9, [x0 - 1.0, x0 + 6.0]);
            }
        }
        // a room on the top level has no level above
        rect_room(&mut doc, "top", "L2", 0.0, 0.0, 5.0, 4.0);
        space(&mut doc, "top", 2.5, 2.0, "L2", "lab");
        let found = check(&doc, pack(vec![rule("H", "Space", "min_clear_height", json!(2.7))]));
        assert_eq!(keys(&found), [("H", "thick")]);
        assert_eq!(found[0].measured, 3.0 - 0.0 - 0.35);
        let found = check(&doc, pack(vec![rule("H", "Space", "min_clear_height", json!(2.9))]));
        assert_eq!(keys(&found), [("H", "thick"), ("H", "thin")]);
        assert_eq!(found[1].measured, 3.0 - 0.25);
    }

    #[test]
    fn egress_goes_around_a_reflex_corner_to_the_nearest_door() {
        let mut doc = base_doc();
        let centreline = [(50.0, 0.0), (56.0, 0.0), (56.0, 3.0), (53.0, 3.0), (53.0, 6.0), (50.0, 6.0)];
        for k in 0..6 {
            let ((x1, y1), (x2, y2)) = (centreline[k], centreline[(k + 1) % 6]);
            let coordinates = [("x1", x1), ("y1", y1), ("x2", x2), ("y2", y2), ("height", 2.8)];
            let mut props: Vec<(&str, Value)> = coordinates.iter().map(|&(key, v)| (key, num(v))).collect();
            props.extend([("level", reference("L0")), ("type", reference("T"))]);
            create(&mut doc, &format!("L-w{k}"), "Wall", props);
        }
        space(&mut doc, "L", 51.0, 1.0, "L0", "hall");
        let door = |doc: &mut Document, name: &str, host: &str, kind: &str, offset: f64, width: f64| {
            create(doc, name, "Opening", vec![("host", reference(host)), ("kind", text(kind)), ("offset", num(offset)), ("width", num(width))]);
        };
        let egress = |doc: &Document, limit: f64| check(doc, pack(vec![rule("E", "Space", "egress_distance_max", json!(limit))]));
        // no door: nothing to measure
        assert!(egress(&doc, 0.0).is_empty());
        // a window is no exit
        door(&mut doc, "win", "L-w1", "window", 1.0, 1.0);
        assert!(egress(&doc, 0.0).is_empty());
        door(&mut doc, "door", "L-w1", "door", 1.0, 1.0); // midpoint (56, 1.5), target (55.9, 1.5)
        let found = egress(&doc, 7.0);
        assert_eq!(keys(&found), [("E", "L")]);
        assert!((found[0].measured - 7.414245977186006).abs() <= 1e-12, "{}", found[0].measured);
        assert!(egress(&doc, 7.42).is_empty());
        // a second door on the first wall: (50.1, 5.9) is now served by a door on the last wall
        door(&mut doc, "door2", "L-w5", "door", 2.0, 1.0); // wall (50,6)->(50,0)? centreline (50,6)->(50,0): midpoint (50, 3.5)
        let found = egress(&doc, 0.0);
        assert_eq!(found.len(), 1);
        assert!(found[0].measured < 7.414245977186006 - 1.0, "{}", found[0].measured);
    }

    #[test]
    fn effective_props_come_from_the_product_and_own_values_win() {
        let mut doc = base_doc();
        create(&mut doc, "P", "Product", vec![("width", num(0.7)), ("kind", text("door"))]);
        create(&mut doc, "w0", "Wall", vec![]);
        create(&mut doc, "from-product", "Opening", vec![("host", reference("w0")), ("product", reference("P"))]);
        create(&mut doc, "own-wins", "Opening", vec![("host", reference("w0")), ("product", reference("P")), ("width", num(1.0))]);
        create(&mut doc, "self-ref", "Opening", vec![("host", reference("w0"))]);
        doc.apply(bim_doc::Op::Set { id: id("self-ref"), key: "product".to_string(), value: reference("self-ref") }).unwrap();
        create(&mut doc, "dangling", "Opening", vec![("host", reference("w0")), ("product", reference("gone")), ("width", num(0.1))]);
        let filtered = json!({"id": "R", "applies_to": "Opening", "where": {"kind": "door"}, "check": "min_width", "limit": 0.9});
        assert_eq!(keys(&check(&doc, pack(vec![filtered]))), [("R", "from-product")]);
        let found = check(&doc, pack(vec![rule("R", "Opening", "min_width", json!(0.9))]));
        assert_eq!(keys(&found), [("R", "dangling"), ("R", "from-product")]);
        assert_eq!(found[1].measured, 0.7);
    }

    /// A pack with one rule per (check, kind) pair of the vocabulary, limits that nearly everything breaks.
    fn every_check() -> Json {
        let mut rules = Vec::new();
        for check in Check::ALL {
            for kind in check.applies_to() {
                let limit = if check == Check::Blondel { json!([0.0, 0.0]) } else { json!(0.5) };
                rules.push(rule(&format!("{}-{kind}", check.name()), kind, check.name(), limit));
            }
        }
        pack(rules)
    }

    #[test]
    fn garbage_props_never_panic() {
        // Every prop of every kind a check reads gets, in turn, numbers that are not finite, text, bools,
        // lists of the wrong length or element type, refs to nothing and refs to the wrong kind.
        let mut doc = base_doc();
        let junk = [
            num(f64::NAN),
            num(f64::INFINITY),
            num(-1.0),
            num(0.0),
            num(1e300),
            text("x"),
            Value::Bool(true),
            Value::List(vec![]),
            nums(&[1.0]),
            nums(&[f64::NAN, 1.0, 2.0]),
            Value::List(vec![text("a"), num(1.0)]),
            reference("gone"),
            reference("T"),
        ];
        let kinds: [(&str, &[&str]); 8] = [
            ("Space", &["level", "x", "y", "use"]),
            ("Opening", &["host", "kind", "offset", "width"]),
            ("Stair", &["base", "top", "riser_count", "tread"]),
            ("Ramp", &["base", "top", "length"]),
            ("Wall", &["level", "type", "x1", "y1", "x2", "y2", "height"]),
            ("Slab", &["level", "thickness", "boundary"]),
            ("Level", &["elevation"]),
            ("WallType", &["layer_thickness", "layer_conductivity", "layer_material"]),
        ];
        for (kind, keys) in kinds {
            for (a, first) in junk.iter().enumerate() {
                for (b, second) in junk.iter().enumerate() {
                    let props = keys.iter().enumerate().map(|(i, k)| (*k, if i % 2 == 0 { first } else { second }.clone())).collect();
                    create(&mut doc, &format!("{kind}-{a}-{b}"), kind, props);
                }
            }
        }
        let _ = check(&doc, every_check());
    }

    #[test]
    fn random_wall_soups_never_panic() {
        // Walls on a coarse grid overlap, cross, touch and double back on each other; spaces sit on the grid.
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as f64
        };
        let mut seen = std::collections::BTreeMap::new();
        for round in 0..300 {
            let mut doc = base_doc();
            for k in 0..14 {
                let (x1, y1, x2, y2) = (next(5), next(5), next(5), next(5));
                let props = vec![
                    ("level", reference("L0")),
                    ("type", reference("T")),
                    ("x1", num(x1)),
                    ("y1", num(y1)),
                    ("x2", num(x2)),
                    ("y2", num(y2)),
                    ("height", num(3.0)),
                ];
                create(&mut doc, &format!("w{round}-{k}"), "Wall", props);
            }
            create(&mut doc, "up", "Level", vec![("elevation", num(3.0))]);
            slab(&mut doc, "roof", "up", 0.3, [-1.0, 6.0]);
            for k in 0..6 {
                space(&mut doc, &format!("s{k}"), next(5) + 0.5, next(5) + 0.5, "L0", "hall");
            }
            for k in 0..5 {
                let host = format!("w{round}-{}", next(14));
                let props = vec![("host", reference(&host)), ("kind", text("door")), ("offset", num(next(4) - 1.0)), ("width", num(next(3) * 0.5))];
                create(&mut doc, &format!("d{k}"), "Opening", props);
            }
            let found = check(&doc, every_check());
            assert!(found.iter().all(|v| v.measured.is_finite() && v.limit.is_finite() && !v.message.is_empty()));
            for v in &found {
                *seen.entry(v.rule.clone()).or_insert(0usize) += 1;
            }
        }
        // The soups do reach rooms and egress paths, not just the empty cases.
        for rule in ["min_area-Space", "min_width-Space", "egress_distance_max-Space", "min_width-Opening"] {
            assert!(seen.contains_key(rule), "no {rule} violation in {seen:?}");
        }
    }

    #[test]
    fn output_order_is_rules_in_pack_order_then_entities_by_name() {
        let mut doc = base_doc();
        create(&mut doc, "w0", "Wall", vec![]);
        for name in ["b", "a", "c"] {
            create(&mut doc, name, "Opening", vec![("host", reference("w0")), ("kind", text("door")), ("width", num(0.1))]);
        }
        let found = check(&doc, pack(vec![rule("Z", "Opening", "min_width", json!(1.0)), rule("A", "Opening", "min_width", json!(1.0))]));
        assert_eq!(keys(&found), [("Z", "a"), ("Z", "b"), ("Z", "c"), ("A", "a"), ("A", "b"), ("A", "c")]);
        // and a rule of another kind finds nothing
        assert!(check(&doc, pack(vec![rule("S", "Stair", "tread_min", json!(1.0))])).is_empty());
    }
}
