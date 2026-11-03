//! `bim-model`: the building geometry compiler. [`Model::compile`] turns a [`Document`] into one
//! closed, outward-oriented triangle [`Mesh`] per building element, following the "Derived
//! geometry rules" of spec/schema.md, plus the quantities `bim solve --trade arch` reports.
//! [`Session`] does the same incrementally: it keeps a document, compiles it through the
//! `bim-incr` queries of [`query`] (one memoized solid query per entity, one drawing query per
//! registered view, see [`drawing`]) and after each batch of ops recomputes only what the edit
//! reaches, reporting exactly what re-executed ([`RecomputeStats`]). `Model::compile` is a
//! from-scratch session, so both paths are one implementation.
//!
//! | Kind | Solid |
//! |---|---|
//! | `Wall` | centreline offset ±t/2 (t = Σ `layer_thickness`), from `level.elevation + base_offset` up by `height`; ends square, mitred at L-joins (plane through the intersections of the outer faces and of the inner faces), stopped at the through wall's facing face at T-joins; minus its Openings. Layers left first. |
//! | `Opening` | box over `[offset, offset + width]` along the host centreline from (x1,y1), through the full thickness, from the wall base + `sill` up by `height` (a cutter, not a `Model` element; it has its own solid query, so `Session::solid` returns the box and `Session::apply` reports it) |
//! | `Slab` | `boundary` minus SlabOpenings, from the level elevation down by `thickness` |
//! | `Column` | `section_b` (x) × `section_h` (y) centred on (x,y), base to top elevation |
//! | `Beam` | `section_b` wide centred on the axis × `section_h` deep, top at elevation − `z_offset` |
//! | `CurtainWall` | the union of its mullions and panels: grid lines every `u_spacing` along the centreline and `v_spacing` up it, the last bay taking the remainder; a `mullion_width` × `mullion_depth` mullion on every grid line (vertical ones level to level + `height`, horizontal ones the full length), a `panel_thickness` panel between the mullion faces of each bay |
//! | `Stair` | the union of `riser_count` blocks, block `k` spanning `[(k−1)·tread, k·tread]` along the run from (x, y) in direction `angle`, `width` across (centred), from the base level up by `k` risers of (top − base) / `riser_count` |
//! | `Ramp` | the wedge over the `length` × `width` rectangle from (x, y) in direction `angle` (centred): underside at the base level, top rising linearly to the top level at the far end; none when the top is not above the base |
//!
//! Direct-modelling `Shape`s and `ComponentInstance`s (spec/sketch.md) are compiled by [`shape`]:
//! one element each, the Shape's faces triangulated and placed by its `transform`; closed Shapes
//! give closed meshes with their exact volume, open surfaces single-sided open meshes.
//!
//! Joins are found in plan only: an L-join is two wall endpoints within 1e-6, a T-join a wall
//! endpoint within 1e-6 of the interior of another wall's centreline. Walls are visited in order of
//! their names (`_name`). An end on a through wall's interior is T-joined (the first such wall) and
//! stops at its facing face even when other walls end there too (a cross junction), and is also
//! mitred with each of those on the same side of the through wall, so they never overlap. Any other
//! end with L candidates is mitred with the last one. An end reports its T-join if any, else its
//! last L partner.
//!
//! **Construction.** Walls are never combined with each other by booleans. Each wall (and each of
//! its layers) is its plan strip clipped by the half-planes of its end cuts — a convex polygon —
//! extruded, minus the opening boxes; [`mesh::cut_prism`] builds that as a cell complex, so the
//! volume is exact and the mesh closed by construction. Slabs whose openings lie strictly inside
//! the boundary and apart from each other are extruded polygons with holes; other slabs fall back
//! to `bim_geom::boolean` of prisms. Curtain walls and stairs are unions of boxes meshed by
//! [`mesh::box_union`] on the grid of all their box faces (no booleans either, so the pieces never
//! overlap in the mesh), ramps a six-vertex wedge. Elements whose props are missing, non-finite,
//! of the wrong type or degenerate (zero length, non-positive size) or whose refs dangle get no
//! solid.
//!
//! **Curtain walls, stairs and ramps.** A curtain wall of length L (from (x1,y1) to (x2,y2)) has
//! grid lines along it at `0, u, 2u, …` and L and up it at `0, v, 2v, …` and `height` (a last bay
//! shorter than a billionth of the spacing is not created); `panels` = bays along × bays up and
//! `mullion_length` = (bays along + 1)·`height` + (bays up + 1)·L. Vertical mullions span the
//! level up by `height` and horizontal ones `0..L`, each centred on its grid line, so the bottom
//! and top horizontals stick out half a `mullion_width` below and above, and the end verticals
//! beyond the ends; a bay no wider than a mullion has no panel (it still counts in `panels`).
//! A stair's `riser` is (top − base) / `riser_count`, `tread` its prop; the stair's plan is
//! `riser_count`·`tread` long. Not built (no quantities, no solid): a stair with a `riser_count`
//! that is not a whole number in 1..=1000, a curtain wall with more than 65 536 panels, a stair
//! or ramp whose top level is not above its base. Dimensions are snapped together within 1e-7
//! (boxes of at most that in any direction vanish); a curtain wall whose pieces would then touch
//! only along an edge or at a vertex (mullions 0.1–0.2 µm wide) keeps its quantities but gets no
//! solid, so every mesh is a closed 2-manifold.

#![forbid(unsafe_code)]

mod arch;
mod build;
pub mod drawing;
pub mod gltf;
pub mod mesh;
pub mod query;
mod session;
pub mod shape;

use bim_doc::{Document, Entity, EntityId, Value};

pub use build::{Built, OpeningIn, Quantities, WallIn, WallJoins};
pub use mesh::{box_union, cut_prism, Aabb, Mesh, Slot};
pub use session::{RecomputeStats, Session};

/// Endpoint coincidence tolerance for wall joins (spec/schema.md).
pub const JOIN_TOL: f64 = 1e-6;

/// One building element's solid.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// The entity id.
    pub id: EntityId,
    /// The id the entity was created with (its `_name`, else the id's string form).
    pub name: String,
    /// The entity kind (`Wall`, `Slab`, `Column`, `Beam`, `CurtainWall`, `Stair`, `Ramp`, `Shape`,
    /// `ComponentInstance`).
    pub kind: String,
    /// Boundary mesh in the model frame (metres, z up): closed and outward-oriented for every
    /// building element and every closed Shape; an open-surface Shape's mesh is open.
    pub mesh: Mesh,
}

/// How one end of a wall is joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    /// L-join with the given wall (endpoints coincide).
    L(EntityId),
    /// T-join: this end abuts the interior of the given wall.
    T(EntityId),
}

/// One wall layer's quantities.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerGeom {
    /// `layer_material` entry.
    pub material: String,
    /// `layer_thickness` entry.
    pub thickness: f64,
    /// Volume of the layer after end cuts and openings.
    pub volume: f64,
}

/// One wall's quantities.
#[derive(Debug, Clone, PartialEq)]
pub struct WallGeom {
    pub id: EntityId,
    pub name: String,
    /// Volume after end cuts and openings.
    pub volume: f64,
    /// Total thickness.
    pub thickness: f64,
    /// Area of the section by the vertical plane through the centreline (= volume / thickness).
    pub side_area: f64,
    /// Layers in WallType order (left side first, looking from (x1,y1) to (x2,y2)).
    pub layers: Vec<LayerGeom>,
    /// Joins at the (x1,y1) end and the (x2,y2) end.
    pub joins: [Option<Join>; 2],
}

/// One slab's quantities.
#[derive(Debug, Clone, PartialEq)]
pub struct SlabGeom {
    pub id: EntityId,
    pub name: String,
    /// Volume after SlabOpenings.
    pub volume: f64,
    /// Net plan area (boundary minus SlabOpenings).
    pub area: f64,
}

/// One stair's quantities.
#[derive(Debug, Clone, PartialEq)]
pub struct StairGeom {
    pub id: EntityId,
    pub name: String,
    /// Height of one riser: (top − base elevation) / `riser_count`.
    pub riser: f64,
    /// Depth of one tread (the `tread` prop).
    pub tread: f64,
    /// Number of risers (the `riser_count` prop).
    pub risers: u64,
    /// Volume of the stepped solid (0 when no solid could be built).
    pub volume: f64,
}

/// One curtain wall's quantities.
#[derive(Debug, Clone, PartialEq)]
pub struct CurtainWallGeom {
    pub id: EntityId,
    pub name: String,
    /// Bays along the wall × bays up it.
    pub panels: u64,
    /// Total mullion length: (bays along + 1) · height + (bays up + 1) · length of the wall.
    pub mullion_length: f64,
    /// Volume of the union of mullions and panels (0 when no solid could be built).
    pub volume: f64,
}

/// The compiled building.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    /// Every element with a solid, sorted by name.
    pub elements: Vec<Element>,
    /// Every valid wall, sorted by name.
    pub walls: Vec<WallGeom>,
    /// Every valid slab, sorted by name.
    pub slabs: Vec<SlabGeom>,
    /// Every valid stair, sorted by name.
    pub stairs: Vec<StairGeom>,
    /// Every valid curtain wall, sorted by name.
    pub curtain_walls: Vec<CurtainWallGeom>,
}

// ------------------------------------------------------------------ prop access

/// The name an entity was created with.
pub fn entity_name(e: &Entity) -> String {
    match e.props.get("_name") {
        Some(Value::Text(s)) => s.clone(),
        _ => e.id.to_string(),
    }
}

// ------------------------------------------------------------------ compile

impl Model {
    /// Compiles every Wall, Slab, Column, Beam, CurtainWall, Stair and Ramp of `doc`, and every
    /// placed Shape and ComponentInstance ([`shape::shape_elements`]). Never panics on bad input:
    /// an element that cannot be built is left out. Equivalent to [`Session::from_document`]
    /// followed by [`Session::model`].
    pub fn compile(doc: &Document) -> Model {
        Session::from_document(doc).model()
    }

    /// The element of entity `id`, if it has a solid.
    pub fn element(&self, id: EntityId) -> Option<&Element> {
        self.elements.iter().find(|e| e.id == id)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod session_tests;
