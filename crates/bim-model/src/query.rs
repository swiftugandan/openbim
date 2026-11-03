//! The `bim-incr` inputs and queries the building is compiled through (see [`crate::Session`]).
//!
//! **Inputs.** One [`Record`] cell per entity (its whole kind + prop map; an edit that leaves the
//! record equal changes nothing) and the [`EntitySet`] of live ids.
//!
//! **Indices.** [`Catalog`] scans every record once into the lists the builders need; the
//! projections [`KindList`], [`OpeningsOf`], [`SlabOpeningsOf`], [`DimensionsFor`],
//! [`IsDefinitionShape`], [`ElementIds`] and [`SolidIds`] cut off when their slice of it is
//! unchanged, so an edit to one wall's props never dirties another element through the indices.
//!
//! **Per-entity derived data.** [`Elevation`] of a level, [`WallInput`] of a wall, [`OpeningParams`]
//! of an opening; the global [`Joins`] of all walls (plan-only, so a height or elevation edit cuts
//! off) projected per wall by [`WallJoinsOf`], and [`MaxThickness`].
//!
//! **Solids.** [`ElementSolid`] builds one entity (the per-element builder). A wall's solid reads
//! its own inputs, its joins, the inputs of every joined wall (both directions of L- and T-joins)
//! and its openings' params; an opening's solid reads its own params and its host wall's inputs;
//! slabs, beams and columns read their records and their levels' elevations. No solid reads
//! another solid.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use bim_doc::{Entity, EntityId, Value};
use bim_incr::{Db, Input, Query};

use crate::build::{self, Built, Cx, OpeningIn, WallIn, WallJoins};
use crate::entity_name;

/// Input: the record (kind and props) of one entity; cleared when it is deleted.
pub struct Record;
impl Input for Record {
    type Key = EntityId;
    type Value = Entity;
}

/// Input: the ids of every live entity.
pub struct EntitySet;
impl Input for EntitySet {
    type Key = ();
    type Value = BTreeSet<EntityId>;
}

/// The kinds compiled to drawable elements (everything [`crate::Model::compile`] lists).
pub const ELEMENT_KINDS: [&str; 9] = ["Wall", "Slab", "Column", "Beam", "CurtainWall", "Stair", "Ramp", "Shape", "ComponentInstance"];

/// What one scan of every record yields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogData {
    /// Per kind: `(name, id)` of every entity of that kind, sorted by name then id.
    pub by_kind: BTreeMap<String, Vec<(String, EntityId)>>,
    /// Openings by host, in name order.
    pub openings: BTreeMap<EntityId, Vec<EntityId>>,
    /// SlabOpenings by host, in name order.
    pub slab_openings: BTreeMap<EntityId, Vec<EntityId>>,
    /// Dimensions by `view` prop, in name order.
    pub dimensions: BTreeMap<String, Vec<EntityId>>,
    /// Shapes referenced by a ComponentDefinition (not placed themselves).
    pub definition_shapes: BTreeSet<EntityId>,
}

/// The index of every entity by kind, host and view (reads every record; projections below).
pub struct Catalog;
impl Query for Catalog {
    type Key = ();
    type Value = Rc<CatalogData>;
    fn execute(db: &Db, _: &()) -> Rc<CatalogData> {
        let cx = Cx { db };
        let ids = db.input::<EntitySet>(&());
        let mut ents: Vec<Rc<Entity>> = ids.iter().flat_map(|s| s.iter()).filter_map(|id| cx.get(*id)).collect();
        let mut named: Vec<(String, Rc<Entity>)> = ents.drain(..).map(|e| (entity_name(&e), e)).collect();
        named.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
        let mut data = CatalogData::default();
        for (name, e) in &named {
            data.by_kind.entry(e.kind.clone()).or_default().push((name.clone(), e.id));
            if let Some(Value::Ref(host)) = e.props.get("host") {
                match e.kind.as_str() {
                    "Opening" => data.openings.entry(*host).or_default().push(e.id),
                    "SlabOpening" => data.slab_openings.entry(*host).or_default().push(e.id),
                    _ => {}
                }
            }
            if e.kind == "Dimension" {
                if let Some(Value::Text(view)) = e.props.get("view") {
                    data.dimensions.entry(view.clone()).or_default().push(e.id);
                }
            }
            if e.kind == "ComponentDefinition" {
                if let Some(Value::Ref(shape)) = e.props.get("shape") {
                    data.definition_shapes.insert(*shape);
                }
            }
        }
        Rc::new(data)
    }
}

/// `(name, id)` of every entity of a kind, sorted by name then id.
pub struct KindList;
impl Query for KindList {
    type Key = String;
    type Value = Vec<(String, EntityId)>;
    fn execute(db: &Db, kind: &String) -> Self::Value {
        db.get::<Catalog>(&()).by_kind.get(kind).cloned().unwrap_or_default()
    }
}

/// The Openings hosted by a wall, in name order.
pub struct OpeningsOf;
impl Query for OpeningsOf {
    type Key = EntityId;
    type Value = Vec<EntityId>;
    fn execute(db: &Db, host: &EntityId) -> Self::Value {
        db.get::<Catalog>(&()).openings.get(host).cloned().unwrap_or_default()
    }
}

/// The SlabOpenings hosted by a slab, in name order.
pub struct SlabOpeningsOf;
impl Query for SlabOpeningsOf {
    type Key = EntityId;
    type Value = Vec<EntityId>;
    fn execute(db: &Db, host: &EntityId) -> Self::Value {
        db.get::<Catalog>(&()).slab_openings.get(host).cloned().unwrap_or_default()
    }
}

/// The Dimensions whose `view` prop equals a view string, in name order.
pub struct DimensionsFor;
impl Query for DimensionsFor {
    type Key = String;
    type Value = Vec<EntityId>;
    fn execute(db: &Db, view: &String) -> Self::Value {
        db.get::<Catalog>(&()).dimensions.get(view).cloned().unwrap_or_default()
    }
}

/// Whether a Shape is referenced by a ComponentDefinition (and so not placed itself).
pub struct IsDefinitionShape;
impl Query for IsDefinitionShape {
    type Key = EntityId;
    type Value = bool;
    fn execute(db: &Db, id: &EntityId) -> bool {
        db.get::<Catalog>(&()).definition_shapes.contains(id)
    }
}

fn merged_kinds(db: &Db, kinds: &[&str]) -> Vec<(String, EntityId)> {
    let catalog = db.get::<Catalog>(&());
    let mut out: Vec<(String, EntityId)> = kinds.iter().filter_map(|k| catalog.by_kind.get(*k)).flatten().cloned().collect();
    out.sort();
    out
}

/// `(name, id)` of every entity of a drawable element kind ([`ELEMENT_KINDS`]), sorted by name
/// then id: the order of [`crate::Model::elements`] and of the solids fed to a drawing.
pub struct ElementIds;
impl Query for ElementIds {
    type Key = ();
    type Value = Vec<(String, EntityId)>;
    fn execute(db: &Db, _: &()) -> Self::Value {
        merged_kinds(db, &ELEMENT_KINDS)
    }
}

/// `(name, id)` of every entity with a solid query: the element kinds and Openings.
pub struct SolidIds;
impl Query for SolidIds {
    type Key = ();
    type Value = Vec<(String, EntityId)>;
    fn execute(db: &Db, _: &()) -> Self::Value {
        let mut kinds = ELEMENT_KINDS.to_vec();
        kinds.push("Opening");
        merged_kinds(db, &kinds)
    }
}

/// The elevation of a Level (`None` when the entity is not a Level with a numeric elevation).
pub struct Elevation;
impl Query for Elevation {
    type Key = EntityId;
    type Value = Option<f64>;
    fn execute(db: &Db, id: &EntityId) -> Option<f64> {
        build::level_elevation(&Cx { db }, *id)
    }
}

/// The parsed inputs of a Wall (`None` when it cannot be built).
pub struct WallInput;
impl Query for WallInput {
    type Key = EntityId;
    type Value = Option<Rc<WallIn>>;
    fn execute(db: &Db, id: &EntityId) -> Self::Value {
        let cx = Cx { db };
        let e = cx.get(*id).filter(|e| e.kind == "Wall")?;
        build::wall_in(&cx, &e).map(Rc::new)
    }
}

/// The parsed props of an Opening (`None` when it cannot cut anything).
pub struct OpeningParams;
impl Query for OpeningParams {
    type Key = EntityId;
    type Value = Option<Rc<OpeningIn>>;
    fn execute(db: &Db, id: &EntityId) -> Self::Value {
        let cx = Cx { db };
        let e = cx.get(*id).filter(|e| e.kind == "Opening")?;
        build::opening_in(&cx, &e).map(Rc::new)
    }
}

/// The joins of every valid wall, found in plan over the walls in name order (spec/schema.md).
/// The value holds ids only, so edits that leave every wall's plan line unchanged cut off here.
pub struct Joins;
impl Query for Joins {
    type Key = ();
    type Value = Rc<BTreeMap<EntityId, WallJoins>>;
    fn execute(db: &Db, _: &()) -> Self::Value {
        let walls: Vec<Rc<WallIn>> =
            db.get::<KindList>(&"Wall".to_string()).iter().filter_map(|(_, id)| db.get::<WallInput>(id).as_ref().clone()).collect();
        let found = build::find_joins(&walls);
        let mut out: BTreeMap<EntityId, WallJoins> = walls.iter().map(|w| (w.id, WallJoins::default())).collect();
        for (wi, ends) in found.iter().enumerate() {
            for (k, j) in ends.iter().enumerate() {
                let t = j.t.map(|o| walls[o].id);
                let l: Vec<(EntityId, usize)> = j.l.iter().map(|&(o, ko)| (walls[o].id, ko)).collect();
                if let Some(wj) = out.get_mut(&walls[wi].id) {
                    wj.ends[k] = (t, l);
                }
                if let Some(t) = t {
                    if let Some(through) = out.get_mut(&t) {
                        through.abutters.push(walls[wi].id);
                    }
                }
            }
        }
        Rc::new(out)
    }
}

/// One wall's joins (`None` for an entity that is not a valid wall).
pub struct WallJoinsOf;
impl Query for WallJoinsOf {
    type Key = EntityId;
    type Value = Option<Rc<WallJoins>>;
    fn execute(db: &Db, id: &EntityId) -> Self::Value {
        db.get::<Joins>(&()).get(id).cloned().map(Rc::new)
    }
}

/// The largest thickness of any valid wall (sizes the strip extension before end cuts).
pub struct MaxThickness;
impl Query for MaxThickness {
    type Key = ();
    type Value = f64;
    fn execute(db: &Db, _: &()) -> f64 {
        db.get::<KindList>(&"Wall".to_string())
            .iter()
            .filter_map(|(_, id)| db.get::<WallInput>(id).as_ref().as_ref().map(|w| w.t))
            .fold(0.0, f64::max)
    }
}

/// The solid of one entity (the per-element builder); `None` when it has none.
pub struct ElementSolid;
impl Query for ElementSolid {
    type Key = EntityId;
    type Value = Option<Rc<Built>>;
    fn execute(db: &Db, id: &EntityId) -> Self::Value {
        build::element(&Cx { db }, *id).map(Rc::new)
    }
}
