//! `bim-model`: the building geometry compiler. [`Model::compile`] turns a [`Document`] into one
//! closed, outward-oriented triangle [`Mesh`] per building element, following the "Derived
//! geometry rules" of spec/schema.md, plus the quantities `bim solve --trade arch` reports.
//!
//! | Kind | Solid |
//! |---|---|
//! | `Wall` | centreline offset ±t/2 (t = Σ `layer_thickness`), from `level.elevation + base_offset` up by `height`; ends square, mitred at L-joins (plane through the intersections of the outer faces and of the inner faces), stopped at the through wall's facing face at T-joins; minus its Openings. Layers left first. |
//! | `Opening` | box over `[offset, offset + width]` along the host centreline from (x1,y1), through the full thickness, from the wall base + `sill` up by `height` (not an element of its own) |
//! | `Slab` | `boundary` minus SlabOpenings, from the level elevation down by `thickness` |
//! | `Column` | `section_b` (x) × `section_h` (y) centred on (x,y), base to top elevation |
//! | `Beam` | `section_b` wide centred on the axis × `section_h` deep, top at elevation − `z_offset` |
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
//! to `bim_geom::boolean` of prisms. Elements whose props are missing, non-finite, of the wrong
//! type or degenerate (zero length, non-positive size) or whose refs dangle get no solid.

#![forbid(unsafe_code)]

pub mod gltf;
pub mod mesh;
pub mod shape;

use std::collections::HashMap;

use bim_doc::{Document, Entity, EntityId, Value};
use bim_geom::{boolean, BoolOp, Point2, Point3, Polygon, Solid};

pub use mesh::{cut_prism, Mesh, Slot};

/// Endpoint coincidence tolerance for wall joins (spec/schema.md).
pub const JOIN_TOL: f64 = 1e-6;

/// One building element's solid.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// The entity id.
    pub id: EntityId,
    /// The id the entity was created with (its `_name`, else the id's string form).
    pub name: String,
    /// The entity kind (`Wall`, `Slab`, `Column`, `Beam`, `Shape`, `ComponentInstance`).
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

/// The compiled building.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    /// Every element with a solid, sorted by name.
    pub elements: Vec<Element>,
    /// Every valid wall, sorted by name.
    pub walls: Vec<WallGeom>,
    /// Every valid slab, sorted by name.
    pub slabs: Vec<SlabGeom>,
}

// ------------------------------------------------------------------ prop access

/// The name an entity was created with.
pub fn entity_name(e: &Entity) -> String {
    match e.props.get("_name") {
        Some(Value::Text(s)) => s.clone(),
        _ => e.id.to_string(),
    }
}

/// `key` of `e`, else of its `product` (spec/catalogs.md effective props).
fn prop<'a>(doc: &'a Document, e: &'a Entity, key: &str) -> Option<&'a Value> {
    e.props.get(key).or_else(|| match e.props.get("product") {
        Some(Value::Ref(p)) if *p != e.id => doc.get(*p).and_then(|p| p.props.get(key)),
        _ => None,
    })
}

fn num(doc: &Document, e: &Entity, key: &str) -> Option<f64> {
    match prop(doc, e, key) {
        Some(Value::Num(x)) if x.is_finite() => Some(*x),
        _ => None,
    }
}

fn num_or(doc: &Document, e: &Entity, key: &str, default: f64) -> Option<f64> {
    match prop(doc, e, key) {
        None => Some(default),
        Some(Value::Num(x)) if x.is_finite() => Some(*x),
        _ => None,
    }
}

fn target<'a>(doc: &'a Document, e: &Entity, key: &str, kind: &str) -> Option<&'a Entity> {
    match e.props.get(key) {
        Some(Value::Ref(id)) => doc.get(*id).filter(|t| t.kind == kind),
        _ => None,
    }
}

fn elevation(doc: &Document, e: &Entity, key: &str) -> Option<f64> {
    target(doc, e, key, "Level").and_then(|l| num(doc, l, "elevation"))
}

fn num_list(v: Option<&Value>) -> Option<Vec<f64>> {
    match v {
        Some(Value::List(items)) => items
            .iter()
            .map(|x| match x {
                Value::Num(x) if x.is_finite() => Some(*x),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// A flat `[x0, y0, x1, y1, …]` list as a ring: closing duplicate and repeated points dropped,
/// collinear points removed, counter-clockwise. `None` if fewer than 3 points or no area.
fn ring(v: Option<&Value>) -> Option<Vec<[f64; 2]>> {
    let flat = num_list(v)?;
    if flat.len() % 2 != 0 {
        return None;
    }
    let pts: Vec<[f64; 2]> = flat.chunks(2).map(|c| [c[0], c[1]]).collect();
    let mut pts = mesh::dedup_ring(pts);
    // Remove exactly collinear vertices (they would only produce degenerate cap triangles).
    loop {
        let n = pts.len();
        if n < 3 {
            return None;
        }
        let k = (0..n).find(|&i| {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            bim_geom::orient2d(p2(a), p2(b), p2(c)) == 0.0
        });
        match k {
            Some(i) => {
                pts.remove(i);
            }
            None => break,
        }
    }
    let a = mesh::ring_area(&pts);
    if !(a.abs() > 0.0) {
        return None;
    }
    if a < 0.0 {
        pts.reverse();
    }
    Some(pts)
}

fn p2(p: [f64; 2]) -> Point2 {
    Point2::new(p[0], p[1])
}

// ------------------------------------------------------------------ walls

struct WallIn<'a> {
    e: &'a Entity,
    name: String,
    a: [f64; 2],
    b: [f64; 2],
    len: f64,
    u: [f64; 2],
    left: [f64; 2],
    layers: Vec<(f64, String)>,
    t: f64,
    z0: f64,
    h: f64,
}

impl WallIn<'_> {
    fn end(&self, k: usize) -> [f64; 2] {
        if k == 0 {
            self.a
        } else {
            self.b
        }
    }
    fn inward(&self, k: usize) -> [f64; 2] {
        if k == 0 {
            self.u
        } else {
            [-self.u[0], -self.u[1]]
        }
    }
    /// `pt` lies on the open interior of the centreline (within [`JOIN_TOL`]).
    fn on_interior(&self, pt: [f64; 2]) -> bool {
        let v = sub(pt, self.a);
        let s = dot(v, self.u);
        dot(v, self.left).abs() <= JOIN_TOL && s > JOIN_TOL && s < self.len - JOIN_TOL
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn perp(a: [f64; 2]) -> [f64; 2] {
    [-a[1], a[0]]
}

/// Intersection of the lines `p + s·d` and `q + r·e`, `None` when (nearly) parallel.
fn line_intersect(p: [f64; 2], d: [f64; 2], q: [f64; 2], e: [f64; 2]) -> Option<[f64; 2]> {
    let det = -d[0] * e[1] + e[0] * d[1];
    if det.abs() < 1e-12 {
        return None;
    }
    let w = sub(q, p);
    let s = (-w[0] * e[1] + e[0] * w[1]) / det;
    Some(add(p, scale(d, s)))
}

fn wall_in<'a>(doc: &'a Document, e: &'a Entity) -> Option<WallIn<'a>> {
    let (x1, y1, x2, y2) = (num(doc, e, "x1")?, num(doc, e, "y1")?, num(doc, e, "x2")?, num(doc, e, "y2")?);
    let h = num(doc, e, "height")?;
    let z0 = elevation(doc, e, "level")? + num_or(doc, e, "base_offset", 0.0)?;
    let wt = target(doc, e, "type", "WallType")?;
    let thick = num_list(wt.props.get("layer_thickness"))?;
    let mats: Vec<String> = match wt.props.get("layer_material") {
        Some(Value::List(m)) => m.iter().map(|v| if let Value::Text(s) = v { s.clone() } else { String::new() }).collect(),
        _ => Vec::new(),
    };
    if thick.iter().any(|&t| t < 0.0) {
        return None;
    }
    let t: f64 = thick.iter().sum();
    let (a, b) = ([x1, y1], [x2, y2]);
    let len = dot(sub(b, a), sub(b, a)).sqrt();
    if !(len > 1e-9) || !(t > 0.0) || !(h > 0.0) || !z0.is_finite() || !t.is_finite() {
        return None;
    }
    let u = scale(sub(b, a), 1.0 / len);
    let layers = thick.iter().enumerate().map(|(i, &lt)| (lt, mats.get(i).cloned().unwrap_or_default())).collect();
    Some(WallIn { e, name: entity_name(e), a, b, len, u, left: perp(u), layers, t, z0, h })
}

/// A kept half-plane `n · (p − origin) ≥ 0`.
type Cut = ([f64; 2], [f64; 2]);

/// The joins at one wall end.
#[derive(Default)]
struct EndJoins {
    /// The through wall, when the end lies on the interior of another wall's centreline (the
    /// first such wall in name order).
    t: Option<usize>,
    /// The walls with an endpoint at this end, as `(wall, its end)`, in name order.
    l: Vec<(usize, usize)>,
}

impl EndJoins {
    /// The join reported for this end: the T-join if any, else the last L partner.
    fn primary(&self) -> Option<(char, usize)> {
        self.t.map(|o| ('T', o)).or_else(|| self.l.last().map(|&(o, _)| ('L', o)))
    }
}

fn find_joins(walls: &[WallIn]) -> Vec<[EndJoins; 2]> {
    walls
        .iter()
        .enumerate()
        .map(|(wi, w)| {
            [0, 1].map(|k| {
                let p = w.end(k);
                let mut j = EndJoins::default();
                for (oi, o) in walls.iter().enumerate() {
                    if oi == wi {
                        continue;
                    }
                    for ko in 0..2 {
                        let d = sub(o.end(ko), p);
                        if dot(d, d).sqrt() <= JOIN_TOL {
                            j.l.push((oi, ko));
                        }
                    }
                    if j.t.is_none() && o.on_interior(p) {
                        j.t = Some(oi);
                    }
                }
                j
            })
        })
        .collect()
}

/// The side (±1) of the through wall `o`'s centreline that a wall leaving it in direction `d` lies on.
fn side_of(o: &WallIn, d: [f64; 2]) -> f64 {
    if dot(o.left, d) > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// The half-planes cutting end `k` of `w`. Without a through wall: the mitre with the last L
/// partner. With one: the facing face of the through wall, plus the mitre with every L partner on
/// the same side of it (partners across it are already separated from this wall by it). The
/// mitre between two walls is the same line seen from either side, so the walls abutting a
/// through wall at one point are pairwise disjoint and disjoint from it.
fn end_cuts(w: &WallIn, k: usize, j: &EndJoins, walls: &[WallIn]) -> Vec<Cut> {
    let d1 = w.inward(k);
    let Some(ti) = j.t else {
        return j.l.last().map(|&(oi, ko)| mitre_cut(w, k, &walls[oi], ko)).into_iter().collect();
    };
    let tw = &walls[ti];
    let side = side_of(tw, d1);
    let n = scale(tw.left, side);
    let mut cuts = vec![(n, add(w.end(k), scale(n, tw.t / 2.0)))];
    for &(oi, ko) in &j.l {
        if side_of(tw, walls[oi].inward(ko)) == side {
            cuts.push(mitre_cut(w, k, &walls[oi], ko));
        }
    }
    cuts
}

/// The kept half-plane of end `k` of `w` at its L-join with end `ko` of `o`: bounded by the plane
/// through the intersections of the outer faces and of the inner faces, or square at the shared
/// endpoint when the walls are in line.
fn mitre_cut(w: &WallIn, k: usize, o: &WallIn, ko: usize) -> Cut {
    let p = w.end(k);
    let d1 = w.inward(k);
    let square = (d1, p);
    let d2 = o.inward(ko);
    let (n1, n2) = (perp(d1), perp(d2));
    let s1 = if dot(n1, d2) > 0.0 { 1.0 } else { -1.0 };
    let s2 = if dot(n2, d1) > 0.0 { 1.0 } else { -1.0 };
    let inner = line_intersect(add(p, scale(n1, s1 * w.t / 2.0)), d1, add(p, scale(n2, s2 * o.t / 2.0)), d2);
    let outer = line_intersect(add(p, scale(n1, -s1 * w.t / 2.0)), d1, add(p, scale(n2, -s2 * o.t / 2.0)), d2);
    let (Some(inner), Some(outer)) = (inner, outer) else { return square };
    let m = sub(outer, inner);
    let mut nm = perp(m);
    if !(dot(nm, nm) > 1e-24) {
        return square;
    }
    if dot(nm, d1) < 0.0 {
        nm = scale(nm, -1.0);
    }
    (nm, inner)
}

/// The wall's plan polygon between lateral offsets `lo..hi` (left positive), in local
/// coordinates `(s along u from (x1,y1), l along left)`, clipped by the end cuts.
fn wall_plan(w: &WallIn, lo: f64, hi: f64, ext: [f64; 2], cuts: &[Cut]) -> Vec<[f64; 2]> {
    let (s0, s1) = (-ext[0], w.len + ext[1]);
    let mut poly = vec![[s0, lo], [s1, lo], [s1, hi], [s0, hi]];
    for &(n, origin) in cuts {
        let c = dot(n, sub(w.a, origin));
        poly = mesh::clip_half_plane(&poly, c, dot(n, w.u), dot(n, w.left));
        if poly.len() < 3 {
            return Vec::new();
        }
    }
    poly
}

fn opening_slot(doc: &Document, op: &Entity, w: &WallIn) -> Option<Slot> {
    let offset = num(doc, op, "offset")?;
    let width = num(doc, op, "width")?;
    let sill = num(doc, op, "sill")?;
    let height = num(doc, op, "height")?;
    if !(width > 0.0) || !(height > 0.0) {
        return None;
    }
    Some(Slot { s0: offset, s1: offset + width, z0: w.z0 + sill, z1: w.z0 + sill + height })
}

// ------------------------------------------------------------------ compile

impl Model {
    /// Compiles every Wall, Slab, Column and Beam of `doc`, and every placed Shape and
    /// ComponentInstance ([`shape::shape_elements`]). Never panics on bad input: an element that
    /// cannot be built is left out.
    pub fn compile(doc: &Document) -> Model {
        let mut model = Model::default();
        let mut ents: Vec<&Entity> = doc.entities().collect();
        ents.sort_by_cached_key(|e| entity_name(e));

        // Openings by host.
        let mut openings: HashMap<EntityId, Vec<&Entity>> = HashMap::new();
        let mut slab_openings: HashMap<EntityId, Vec<&Entity>> = HashMap::new();
        for e in &ents {
            if let Some(Value::Ref(host)) = e.props.get("host") {
                match e.kind.as_str() {
                    "Opening" => openings.entry(*host).or_default().push(e),
                    "SlabOpening" => slab_openings.entry(*host).or_default().push(e),
                    _ => {}
                }
            }
        }

        // Walls.
        let walls: Vec<WallIn> = ents.iter().filter(|e| e.kind == "Wall").filter_map(|e| wall_in(doc, e)).collect();
        let joins = find_joins(&walls);
        let tmax = walls.iter().map(|w| w.t).fold(0.0, f64::max);
        for (wi, w) in walls.iter().enumerate() {
            let mut cuts = Vec::new();
            let mut ext = [0.0, 0.0];
            let mut jout = [None, None];
            for k in 0..2 {
                if let Some((kind, oi)) = joins[wi][k].primary() {
                    cuts.extend(end_cuts(w, k, &joins[wi][k], &walls));
                    ext[k] = 4.0 * tmax + 1.0;
                    let other = walls[oi].e.id;
                    jout[k] = Some(if kind == 'L' { Join::L(other) } else { Join::T(other) });
                }
            }
            let slots: Vec<Slot> = openings
                .get(&w.e.id)
                .map(|ops| ops.iter().filter_map(|op| opening_slot(doc, op, w)).collect())
                .unwrap_or_default();
            let plan = wall_plan(w, -w.t / 2.0, w.t / 2.0, ext, &cuts);
            let (volume, local) = cut_prism(&plan, w.z0, w.h, &slots, true);
            let mut layers = Vec::new();
            let mut top = w.t / 2.0;
            for (lt, mat) in &w.layers {
                let lp = wall_plan(w, top - lt, top, ext, &cuts);
                let (v, _) = cut_prism(&lp, w.z0, w.h, &slots, false);
                layers.push(LayerGeom { material: mat.clone(), thickness: *lt, volume: v });
                top -= lt;
            }
            model.walls.push(WallGeom {
                id: w.e.id,
                name: w.name.clone(),
                volume,
                thickness: w.t,
                side_area: volume / w.t,
                layers,
                joins: jout,
            });
            if !local.triangles.is_empty() {
                let mut mesh = local;
                for p in &mut mesh.positions {
                    let g = add(w.a, add(scale(w.u, p.x), scale(w.left, p.y)));
                    *p = Point3::new(g[0], g[1], p.z);
                }
                model.elements.push(Element { id: w.e.id, name: w.name.clone(), kind: "Wall".into(), mesh });
            }
        }

        for e in &ents {
            match e.kind.as_str() {
                "Slab" => {
                    if let Some((geom, mesh)) = slab(doc, e, slab_openings.get(&e.id).map(Vec::as_slice).unwrap_or(&[])) {
                        if !mesh.triangles.is_empty() {
                            model.elements.push(Element { id: e.id, name: geom.name.clone(), kind: "Slab".into(), mesh });
                        }
                        model.slabs.push(geom);
                    }
                }
                "Column" => {
                    if let Some(mesh) = column(doc, e) {
                        model.elements.push(Element { id: e.id, name: entity_name(e), kind: "Column".into(), mesh });
                    }
                }
                "Beam" => {
                    if let Some(mesh) = beam(doc, e) {
                        model.elements.push(Element { id: e.id, name: entity_name(e), kind: "Beam".into(), mesh });
                    }
                }
                _ => {}
            }
        }
        model.elements.extend(shape::shape_elements(doc));
        model.elements.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        model
    }

    /// The element of entity `id`, if it has a solid.
    pub fn element(&self, id: EntityId) -> Option<&Element> {
        self.elements.iter().find(|e| e.id == id)
    }
}

fn column(doc: &Document, e: &Entity) -> Option<Mesh> {
    let (x, y) = (num(doc, e, "x")?, num(doc, e, "y")?);
    let (zb, zt) = (elevation(doc, e, "base")?, elevation(doc, e, "top")?);
    let (b, h) = (num(doc, e, "section_b")?, num(doc, e, "section_h")?);
    if !(b > 0.0) || !(h > 0.0) || !(zt > zb) {
        return None;
    }
    let poly = [[x - b / 2.0, y - h / 2.0], [x + b / 2.0, y - h / 2.0], [x + b / 2.0, y + h / 2.0], [x - b / 2.0, y + h / 2.0]];
    let (_, m) = cut_prism(&poly, zb, zt - zb, &[], true);
    Some(m).filter(|m| !m.triangles.is_empty())
}

fn beam(doc: &Document, e: &Entity) -> Option<Mesh> {
    let (x1, y1, x2, y2) = (num(doc, e, "x1")?, num(doc, e, "y1")?, num(doc, e, "x2")?, num(doc, e, "y2")?);
    let top = elevation(doc, e, "level")? - num_or(doc, e, "z_offset", 0.0)?;
    let (b, h) = (num(doc, e, "section_b")?, num(doc, e, "section_h")?);
    let (a, c) = ([x1, y1], [x2, y2]);
    let len = dot(sub(c, a), sub(c, a)).sqrt();
    if !(len > 1e-9) || !(b > 0.0) || !(h > 0.0) {
        return None;
    }
    let lf = scale(perp(scale(sub(c, a), 1.0 / len)), b / 2.0);
    let poly = [sub(a, lf), sub(c, lf), add(c, lf), add(a, lf)];
    let (_, m) = cut_prism(&poly, top - h, h, &[], true);
    Some(m).filter(|m| !m.triangles.is_empty())
}

// ------------------------------------------------------------------ slabs

/// Whether the closed segments `ab` and `cd` share a point.
fn segments_meet(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    use bim_geom::orient2d;
    let (o1, o2) = (orient2d(p2(a), p2(b), p2(c)), orient2d(p2(a), p2(b), p2(d)));
    let (o3, o4) = (orient2d(p2(c), p2(d), p2(a)), orient2d(p2(c), p2(d), p2(b)));
    let within = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        r[0] >= p[0].min(q[0]) && r[0] <= p[0].max(q[0]) && r[1] >= p[1].min(q[1]) && r[1] <= p[1].max(q[1])
    };
    if o1 * o2 < 0.0 && o3 * o4 < 0.0 {
        return true;
    }
    (o1 == 0.0 && within(a, b, c)) || (o2 == 0.0 && within(a, b, d)) || (o3 == 0.0 && within(c, d, a)) || (o4 == 0.0 && within(c, d, b))
}

fn rings_meet(r: &[[f64; 2]], s: &[[f64; 2]]) -> bool {
    (0..r.len()).any(|i| (0..s.len()).any(|j| segments_meet(r[i], r[(i + 1) % r.len()], s[j], s[(j + 1) % s.len()])))
}

fn ring_contains(r: &[[f64; 2]], p: [f64; 2]) -> bool {
    Polygon::new(r.iter().map(|&q| p2(q)).collect()).contains(p2(p))
}

fn slab(doc: &Document, e: &Entity, openings: &[&Entity]) -> Option<(SlabGeom, Mesh)> {
    let top = elevation(doc, e, "level")?;
    let th = num(doc, e, "thickness")?;
    let outer = ring(e.props.get("boundary"))?;
    if !(th > 0.0) {
        return None;
    }
    let name = entity_name(e);
    let holes: Vec<Vec<[f64; 2]>> = openings.iter().filter_map(|o| ring(o.props.get("boundary"))).collect();
    // Holes strictly inside and pairwise apart: an exact polygon with holes.
    let simple = holes.iter().all(|h| !rings_meet(h, &outer) && ring_contains(&outer, h[0]))
        && (0..holes.len()).all(|i| {
            (i + 1..holes.len()).all(|j| {
                !rings_meet(&holes[i], &holes[j]) && !ring_contains(&holes[i], holes[j][0]) && !ring_contains(&holes[j], holes[i][0])
            })
        });
    if simple {
        let area = mesh::ring_area(&outer) - holes.iter().map(|h| mesh::ring_area(h)).sum::<f64>();
        let mesh = extrude_with_holes(&outer, &holes, top - th, top);
        return Some((SlabGeom { id: e.id, name, volume: area * th, area }, mesh));
    }
    // General case: CSG of prisms (opening prisms taller than the slab, so no coplanar caps).
    let to_poly = |r: &[[f64; 2]]| Polygon::new(r.iter().map(|&q| p2(q)).collect());
    let mut solid = Solid::extrude(&to_poly(&outer), top - th, th);
    for h in &holes {
        let cutter = Solid::extrude(&to_poly(h), top - th - 1.0, th + 2.0);
        if let Ok(s) = boolean(&solid, &cutter, BoolOp::Difference) {
            solid = s;
        }
    }
    let volume = solid.volume();
    let mesh = Mesh::from_triangle_soup(&solid.triangles());
    Some((SlabGeom { id: e.id, name, volume, area: volume / th }, mesh))
}

/// Extrusion between `z0 < z1` of a counter-clockwise outer ring with counter-clockwise holes
/// lying strictly inside it and apart from each other.
fn extrude_with_holes(outer: &[[f64; 2]], holes: &[Vec<[f64; 2]>], z0: f64, z1: f64) -> Mesh {
    let poly = Polygon::with_holes(
        outer.iter().map(|&q| p2(q)).collect(),
        holes.iter().map(|h| h.iter().map(|&q| p2(q)).collect()).collect(),
    );
    let flat = poly.vertices();
    let n = flat.len() as u32;
    let mut mesh = Mesh::default();
    for &z in &[z0, z1] {
        mesh.positions.extend(flat.iter().map(|p| Point3::new(p.x, p.y, z)));
    }
    for t in poly.triangulate() {
        let [a, b, c] = t.map(|i| i as u32);
        mesh.triangles.push([a + n, b + n, c + n]); // top, +z
        mesh.triangles.push([a, c, b]); // bottom, −z
    }
    // Sides: material on the left of each ring as traversed (outer CCW, holes CW).
    let mut start = 0u32;
    let mut rings: Vec<(u32, usize, bool)> = vec![(0, outer.len(), false)];
    start += outer.len() as u32;
    for h in holes {
        rings.push((start, h.len(), true));
        start += h.len() as u32;
    }
    for (s, len, reverse) in rings {
        let mut idx: Vec<u32> = (s..s + len as u32).collect();
        if reverse {
            idx.reverse();
        }
        for i in 0..len {
            let (a, b) = (idx[i], idx[(i + 1) % len]);
            mesh.triangles.push([a, b, b + n]);
            mesh.triangles.push([a, b + n, a + n]);
        }
    }
    mesh
}

#[cfg(test)]
mod tests;
