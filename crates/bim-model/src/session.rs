//! [`Session`]: the incremental building compiler. It owns a [`Document`], mirrors it into the
//! `bim-incr` inputs of [`crate::query`] and, after every batch of ops, brings every element solid
//! and every registered drawing up to date, reporting exactly which solid and drawing queries
//! re-executed.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use bim_doc::{Document, EntityId, Op};
use bim_incr::Db;

use crate::build::{Built, Quantities};
use crate::drawing::{self, ViewQuery};
use crate::query::{ElementSolid, EntitySet, Record, SolidIds};
use crate::{entity_name, Element, Model};

/// What one [`Session::apply`] recomputed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecomputeStats {
    /// The `_name` (else id) of every entity whose solid query re-executed (walls, slabs, columns,
    /// beams, curtain walls, stairs, ramps, shapes, openings), each once, in name order.
    pub solids: Vec<String>,
    /// Every registered view whose drawing re-executed, each once, in registration order.
    pub views: Vec<String>,
}

/// An incrementally compiled building: a document plus the memoized solids and drawings over it.
pub struct Session {
    doc: Document,
    db: Db,
    views: Vec<String>,
    /// The current solid of every entity that has one (non-empty mesh), by id.
    built: HashMap<EntityId, Rc<Built>>,
    /// Every entity with a solid query, `(name, id)` sorted by name then id.
    ids: Vec<(String, EntityId)>,
    /// `_name` → id for every entity with a solid query (the first id for a repeated name).
    names: HashMap<String, EntityId>,
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

impl Session {
    /// An empty session: no entities, no views.
    pub fn new() -> Session {
        let db = Db::new();
        db.set::<EntitySet>(&(), Some(BTreeSet::new()));
        Session { doc: Document::new(), db, views: Vec::new(), built: HashMap::new(), ids: Vec::new(), names: HashMap::new() }
    }

    /// A session holding a copy of `doc`'s entities (its op log is not replayed), with every
    /// solid computed.
    pub fn from_document(doc: &Document) -> Session {
        let mut s = Session::new();
        s.doc = doc.clone();
        for e in doc.entities() {
            s.db.set::<Record>(&e.id, Some(e.clone()));
        }
        s.db.set::<EntitySet>(&(), Some(doc.entities().map(|e| e.id).collect()));
        s.refresh();
        s.db.take_log();
        s
    }

    /// Applies `ops` in order (an op the document rejects is skipped) and brings every element
    /// solid and every registered view up to date. Returns what re-executed: on the first apply
    /// every element; after a no-op edit nothing.
    pub fn apply(&mut self, ops: Vec<Op>) -> RecomputeStats {
        self.db.take_log();
        let mut touched: BTreeSet<EntityId> = BTreeSet::new();
        let mut membership = false;
        for op in ops {
            let (id, changes_set) = match &op {
                Op::Create { id, .. } | Op::Delete { id } => (*id, true),
                Op::Set { id, .. } => (*id, false),
            };
            if self.doc.apply(op).is_ok() {
                touched.insert(id);
                membership |= changes_set;
            }
        }
        for id in &touched {
            self.db.set::<Record>(id, self.doc.get(*id).cloned());
        }
        if membership {
            self.db.set::<EntitySet>(&(), Some(self.doc.entities().map(|e| e.id).collect()));
        }
        self.refresh();

        let mut solids: BTreeSet<String> = BTreeSet::new();
        let mut views: BTreeSet<String> = BTreeSet::new();
        for e in self.db.take_log() {
            if let Some(id) = e.key::<ElementSolid>() {
                solids.insert(self.doc.get(*id).map(entity_name).unwrap_or_else(|| id.to_string()));
            } else if let Some(view) = e.key::<ViewQuery>() {
                if self.views.contains(view) {
                    views.insert(view.clone());
                }
            }
        }
        RecomputeStats {
            solids: solids.into_iter().collect(),
            views: self.views.iter().filter(|v| views.contains(*v)).cloned().collect(),
        }
    }

    /// Registers a view (`plan:<level>@<h>`, `section:x0,y0,x1,y1`, `elevation:<n|s|e|w>`) and
    /// draws it now; later applies report it when its drawing re-executes. Registering a view
    /// twice is a no-op.
    pub fn add_view(&mut self, view: &str) {
        if !self.views.iter().any(|v| v == view) {
            self.views.push(view.to_string());
        }
        self.db.get::<ViewQuery>(&view.to_string());
        self.db.take_log();
    }

    /// Demands every solid and every registered drawing, then materializes the solids.
    fn refresh(&mut self) {
        let ids = self.db.get::<SolidIds>(&());
        let mut built = HashMap::with_capacity(ids.len());
        let mut names = HashMap::with_capacity(ids.len());
        for (name, id) in ids.iter() {
            if let Some(b) = self.db.get::<ElementSolid>(id).as_ref() {
                if !b.mesh.triangles.is_empty() {
                    built.insert(*id, Rc::clone(b));
                }
            }
            names.entry(name.clone()).or_insert(*id);
        }
        self.ids = (*ids).clone();
        self.built = built;
        self.names = names;
        for v in &self.views {
            self.db.get::<ViewQuery>(v);
        }
    }

    /// The id `name` denotes: an entity's `_name`, else the id string itself (its 32-digit hex
    /// form, a ULID, or the seed it was derived from).
    pub fn resolve(&self, name: &str) -> EntityId {
        if let Some(id) = self.names.get(name) {
            return *id;
        }
        if name.len() == 32 {
            if let Ok(v) = u128::from_str_radix(name, 16) {
                if self.doc.get(EntityId(v)).is_some() {
                    return EntityId(v);
                }
            }
        }
        let parsed = EntityId::parse(name);
        if self.doc.get(parsed).is_some() {
            return parsed;
        }
        EntityId::derive(name)
    }

    /// The current solid of entity `id` (its `_name` or id string), when it has one.
    pub fn solid(&self, id: &str) -> Option<&bim_geom::Solid> {
        self.built.get(&self.resolve(id)).map(|b| &b.solid)
    }

    /// The current mesh of entity `id` (its `_name` or id string), when it has one.
    pub fn mesh(&self, id: &str) -> Option<&crate::Mesh> {
        self.built.get(&self.resolve(id)).map(|b| &b.mesh)
    }

    /// The SVG of a registered view, equal to `bim export --format svg --view <view>` of the
    /// session's document. `None` for an unregistered or invalid view.
    pub fn svg(&self, view: &str) -> Option<String> {
        if !self.views.iter().any(|v| v == view) {
            return None;
        }
        let drawing = self.db.get::<ViewQuery>(&view.to_string());
        drawing.as_ref().as_ref().map(|d| drawing::svg::to_svg(d))
    }

    /// The drawing of a registered view. `None` for an unregistered or invalid view.
    pub fn drawing(&self, view: &str) -> Option<Rc<drawing::Drawing>> {
        if !self.views.iter().any(|v| v == view) {
            return None;
        }
        self.db.get::<ViewQuery>(&view.to_string()).as_ref().clone()
    }

    /// The registered views, in registration order.
    pub fn views(&self) -> &[String] {
        &self.views
    }

    /// The document as edited so far.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The incremental database (for further queries over the session, e.g. `bim_draw`'s SVG
    /// query). Queries demanded through it are memoized like the session's own.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The compiled building as a [`Model`]: every element with a non-empty mesh (openings are
    /// cutters, not elements) and the quantities, all sorted by name then id.
    pub fn model(&self) -> Model {
        let mut model = Model::default();
        for (_, id) in &self.ids {
            let Some(b) = self.db.get::<ElementSolid>(id).as_ref().clone() else { continue };
            match &b.quantities {
                Quantities::Wall(g) => model.walls.push(g.clone()),
                Quantities::Slab(g) => model.slabs.push(g.clone()),
                Quantities::CurtainWall(g) => model.curtain_walls.push(g.clone()),
                Quantities::Stair(g) => model.stairs.push(g.clone()),
                Quantities::None => {}
            }
            if b.kind != "Opening" && !b.mesh.triangles.is_empty() {
                model.elements.push(Element { id: b.id, name: b.name.clone(), kind: b.kind.clone(), mesh: b.mesh.clone() });
            }
        }
        model
    }
}
