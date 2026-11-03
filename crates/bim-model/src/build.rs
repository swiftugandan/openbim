//! The per-element solid builders (walls with joins and openings, opening boxes, slabs, columns,
//! beams; curtain walls, stairs and ramps in [`crate::arch`], Shapes in [`crate::shape`]). Every
//! builder reads the document through a [`Cx`], which records what it reads as dependencies of the
//! executing `bim-incr` query, so the same code serves [`crate::Model::compile`] and the
//! incremental [`crate::Session`].

use std::rc::Rc;

use bim_doc::{Entity, EntityId, Value};
use bim_geom::{boolean, BoolOp, Point2, Point3, Polygon, Solid};
use bim_incr::Db;

use crate::mesh::{self, cut_prism, Mesh, Slot};
use crate::query::{Elevation, OpeningParams, OpeningsOf, Record, SlabOpeningsOf};
use crate::{entity_name, CurtainWallGeom, Join, LayerGeom, SlabGeom, StairGeom, WallGeom, JOIN_TOL};

/// Read access to the document from inside a query: every entity read is a recorded dependency.
#[derive(Clone, Copy)]
pub(crate) struct Cx<'a> {
    pub db: &'a Db,
}

impl Cx<'_> {
    /// The live entity `id`, if any (a dependency on its record).
    pub fn get(&self, id: EntityId) -> Option<Rc<Entity>> {
        self.db.input::<Record>(&id)
    }

    /// The elevation of Level `id` (a dependency on the [`Elevation`] query, not the whole record).
    pub fn elevation(&self, id: EntityId) -> Option<f64> {
        *self.db.get::<Elevation>(&id)
    }
}

// ------------------------------------------------------------------ prop access

/// `key` of `e`, else of its `product` (spec/catalogs.md effective props).
pub(crate) fn prop(cx: &Cx, e: &Entity, key: &str) -> Option<Value> {
    e.props.get(key).cloned().or_else(|| match e.props.get("product") {
        Some(Value::Ref(p)) if *p != e.id => cx.get(*p).and_then(|p| p.props.get(key).cloned()),
        _ => None,
    })
}

pub(crate) fn num(cx: &Cx, e: &Entity, key: &str) -> Option<f64> {
    match prop(cx, e, key) {
        Some(Value::Num(x)) if x.is_finite() => Some(x),
        _ => None,
    }
}

pub(crate) fn num_or(cx: &Cx, e: &Entity, key: &str, default: f64) -> Option<f64> {
    match prop(cx, e, key) {
        None => Some(default),
        Some(Value::Num(x)) if x.is_finite() => Some(x),
        _ => None,
    }
}

/// The entity `e.key` refers to, if it is live and of `kind`.
pub(crate) fn target(cx: &Cx, e: &Entity, key: &str, kind: &str) -> Option<Rc<Entity>> {
    match e.props.get(key) {
        Some(Value::Ref(id)) => cx.get(*id).filter(|t| t.kind == kind),
        _ => None,
    }
}

/// The elevation of the Level `e.key` refers to.
pub(crate) fn elevation(cx: &Cx, e: &Entity, key: &str) -> Option<f64> {
    match e.props.get(key) {
        Some(Value::Ref(id)) => cx.elevation(*id),
        _ => None,
    }
}

/// The elevation of entity `id` when it is a Level with a numeric `elevation` (effective props).
pub(crate) fn level_elevation(cx: &Cx, id: EntityId) -> Option<f64> {
    let l = cx.get(id).filter(|l| l.kind == "Level")?;
    num(cx, &l, "elevation")
}

pub(crate) fn num_list(v: Option<&Value>) -> Option<Vec<f64>> {
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

// ------------------------------------------------------------------ the built element

/// The quantities of a built element, by kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Quantities {
    Wall(WallGeom),
    Slab(SlabGeom),
    CurtainWall(CurtainWallGeom),
    Stair(StairGeom),
    /// Columns, beams, ramps, Shapes, ComponentInstances and Openings report no quantities.
    None,
}

/// The result of one entity's solid query: its mesh (possibly empty, e.g. a wall its openings
/// consume entirely), the same as a half-edge [`Solid`], its name and kind, and its quantities.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub id: EntityId,
    /// The entity's `_name` (else its id string).
    pub name: String,
    /// The entity kind (`Wall`, `Opening`, `Slab`, …).
    pub kind: String,
    pub quantities: Quantities,
    pub mesh: Mesh,
    pub solid: Solid,
}

impl Built {
    fn new(e: &Entity, quantities: Quantities, mesh: Mesh) -> Built {
        let solid = mesh.to_solid();
        Built { id: e.id, name: entity_name(e), kind: e.kind.clone(), quantities, mesh, solid }
    }
}

/// Builds entity `id` (the body of the [`crate::query::ElementSolid`] query). `None` when the entity is
/// missing, of a kind without a solid, or cannot be built from its props.
pub(crate) fn element(cx: &Cx, id: EntityId) -> Option<Built> {
    let e = cx.get(id)?;
    match e.kind.as_str() {
        "Wall" => wall(cx, &e),
        "Opening" => opening(cx, &e),
        "Slab" => {
            let openings: Vec<Rc<Entity>> = cx.db.get::<SlabOpeningsOf>(&id).iter().filter_map(|o| cx.get(*o)).collect();
            slab(cx, &e, &openings).map(|(geom, mesh)| Built::new(&e, Quantities::Slab(geom), mesh))
        }
        "Column" => column(cx, &e).map(|m| Built::new(&e, Quantities::None, m)),
        "Beam" => beam(cx, &e).map(|m| Built::new(&e, Quantities::None, m)),
        "CurtainWall" => crate::arch::curtain_wall(cx, &e).map(|(geom, mesh)| Built::new(&e, Quantities::CurtainWall(geom), mesh)),
        "Stair" => crate::arch::stair(cx, &e).map(|(geom, mesh)| Built::new(&e, Quantities::Stair(geom), mesh)),
        "Ramp" => crate::arch::ramp(cx, &e).map(|m| Built::new(&e, Quantities::None, m)),
        "Shape" | "ComponentInstance" => crate::shape::shape_element(cx, &e).map(|m| Built::new(&e, Quantities::None, m)),
        _ => None,
    }
}

// ------------------------------------------------------------------ walls

/// A wall's parsed inputs: plan line, layers, thickness, base and height.
#[derive(Debug, Clone, PartialEq)]
pub struct WallIn {
    pub id: EntityId,
    pub name: String,
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub len: f64,
    pub u: [f64; 2],
    pub left: [f64; 2],
    pub layers: Vec<(f64, String)>,
    pub t: f64,
    pub z0: f64,
    pub h: f64,
}

impl WallIn {
    pub(crate) fn end(&self, k: usize) -> [f64; 2] {
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
    pub(crate) fn on_interior(&self, pt: [f64; 2]) -> bool {
        let v = sub(pt, self.a);
        let s = dot(v, self.u);
        dot(v, self.left).abs() <= JOIN_TOL && s > JOIN_TOL && s < self.len - JOIN_TOL
    }
    /// The model point of local coordinates `(s along u from (x1,y1), l along left)`.
    fn place(&self, s: f64, l: f64) -> [f64; 2] {
        add(self.a, add(scale(self.u, s), scale(self.left, l)))
    }
}

pub(crate) fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}
pub(crate) fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
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

/// The parsed inputs of Wall `e` (the body of the [`crate::query::WallInput`] query): reads the
/// wall's record (and product), its level's elevation and its WallType.
pub(crate) fn wall_in(cx: &Cx, e: &Entity) -> Option<WallIn> {
    let (x1, y1, x2, y2) = (num(cx, e, "x1")?, num(cx, e, "y1")?, num(cx, e, "x2")?, num(cx, e, "y2")?);
    let h = num(cx, e, "height")?;
    let z0 = elevation(cx, e, "level")? + num_or(cx, e, "base_offset", 0.0)?;
    let wt = target(cx, e, "type", "WallType")?;
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
    Some(WallIn { id: e.id, name: entity_name(e), a, b, len, u, left: perp(u), layers, t, z0, h })
}

/// A kept half-plane `n · (p − origin) ≥ 0`.
type Cut = ([f64; 2], [f64; 2]);

/// The joins at one wall end, as indices into a wall list.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct EndJoins {
    /// The through wall, when the end lies on the interior of another wall's centreline (the
    /// first such wall in name order).
    pub t: Option<usize>,
    /// The walls with an endpoint at this end, as `(wall, its end)`, in name order.
    pub l: Vec<(usize, usize)>,
}

impl EndJoins {
    /// The join reported for this end: the T-join if any, else the last L partner.
    fn primary(&self) -> Option<(char, usize)> {
        self.t.map(|o| ('T', o)).or_else(|| self.l.last().map(|&(o, _)| ('L', o)))
    }
}

/// The joins of every wall of `walls` (in that order, normally by name) at both ends.
pub(crate) fn find_joins(walls: &[Rc<WallIn>]) -> Vec<[EndJoins; 2]> {
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
fn end_cuts(w: &WallIn, k: usize, j: &EndJoins, walls: &[Rc<WallIn>]) -> Vec<Cut> {
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

/// An opening's own props (the body of the [`OpeningParams`] query): `sill` and `height` relative
/// to the host wall's base.
#[derive(Debug, Clone, PartialEq)]
pub struct OpeningIn {
    pub host: EntityId,
    pub offset: f64,
    pub width: f64,
    pub sill: f64,
    pub height: f64,
}

pub(crate) fn opening_in(cx: &Cx, op: &Entity) -> Option<OpeningIn> {
    let Some(Value::Ref(host)) = op.props.get("host") else { return None };
    let offset = num(cx, op, "offset")?;
    let width = num(cx, op, "width")?;
    let sill = num(cx, op, "sill")?;
    let height = num(cx, op, "height")?;
    if !(width > 0.0) || !(height > 0.0) {
        return None;
    }
    Some(OpeningIn { host: *host, offset, width, sill, height })
}

impl OpeningIn {
    fn slot(&self, w: &WallIn) -> Slot {
        Slot { s0: self.offset, s1: self.offset + self.width, z0: w.z0 + self.sill, z1: w.z0 + self.sill + self.height }
    }
}

/// A wall's joins by entity id (the per-wall projection of the global join query).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WallJoins {
    /// At the (x1,y1) end and the (x2,y2) end: the through wall and the L partners `(wall, end)`.
    pub ends: [(Option<EntityId>, Vec<(EntityId, usize)>); 2],
    /// The walls that abut this wall at a T-join (it is their through wall).
    pub abutters: Vec<EntityId>,
}

impl WallJoins {
    /// Every wall joined to this one, each once, in order of first mention.
    pub fn partners(&self) -> Vec<EntityId> {
        let mut out: Vec<EntityId> = Vec::new();
        let mut push = |id: EntityId| {
            if !out.contains(&id) {
                out.push(id);
            }
        };
        for (t, l) in &self.ends {
            if let Some(t) = t {
                push(*t);
            }
            for (o, _) in l {
                push(*o);
            }
        }
        for a in &self.abutters {
            push(*a);
        }
        out
    }
}

/// Moves a mesh built in the wall's local `(s, l, z)` frame into the model.
fn place_wall_mesh(w: &WallIn, mut mesh: Mesh) -> Mesh {
    for p in &mut mesh.positions {
        let g = w.place(p.x, p.y);
        *p = Point3::new(g[0], g[1], p.z);
    }
    mesh
}

/// The wall's solid and quantities: its plan strip clipped by the end cuts of its joins, extruded,
/// minus its openings. Reads the wall's inputs, its joins, the inputs of every joined wall (so an
/// edit to a joined wall reaches it), the largest wall thickness and its openings' params.
fn wall(cx: &Cx, e: &Entity) -> Option<Built> {
    let db = cx.db;
    let w = db.get::<crate::query::WallInput>(&e.id).as_ref().clone()?;
    let joins = db.get::<crate::query::WallJoinsOf>(&e.id).as_ref().clone().unwrap_or_default();
    let tmax = *db.get::<crate::query::MaxThickness>(&());

    // The joined walls, each read once (a dependency whether or not its geometry is used).
    let mut partners: Vec<Rc<WallIn>> = Vec::new();
    let index_of = |id: EntityId, partners: &mut Vec<Rc<WallIn>>| -> Option<usize> {
        let p = db.get::<crate::query::WallInput>(&id).as_ref().clone()?;
        if let Some(i) = partners.iter().position(|q| q.id == id) {
            return Some(i);
        }
        partners.push(p);
        Some(partners.len() - 1)
    };
    let mut ends: [EndJoins; 2] = [EndJoins::default(), EndJoins::default()];
    for k in 0..2 {
        let (t, l) = &joins.ends[k];
        ends[k].t = t.and_then(|id| index_of(id, &mut partners));
        ends[k].l = l.iter().filter_map(|&(id, ko)| index_of(id, &mut partners).map(|i| (i, ko))).collect();
    }
    for a in &joins.abutters {
        index_of(*a, &mut partners);
    }

    let mut cuts = Vec::new();
    let mut ext = [0.0, 0.0];
    let mut jout = [None, None];
    for k in 0..2 {
        if let Some((kind, oi)) = ends[k].primary() {
            cuts.extend(end_cuts(&w, k, &ends[k], &partners));
            ext[k] = 4.0 * tmax + 1.0;
            let other = partners[oi].id;
            jout[k] = Some(if kind == 'L' { Join::L(other) } else { Join::T(other) });
        }
    }
    let slots: Vec<Slot> = db
        .get::<OpeningsOf>(&e.id)
        .iter()
        .filter_map(|op| db.get::<OpeningParams>(op).as_ref().clone())
        .map(|p| p.slot(&w))
        .collect();
    let plan = wall_plan(&w, -w.t / 2.0, w.t / 2.0, ext, &cuts);
    let (volume, local) = cut_prism(&plan, w.z0, w.h, &slots, true);
    let mut layers = Vec::new();
    let mut top = w.t / 2.0;
    for (lt, mat) in &w.layers {
        let lp = wall_plan(&w, top - lt, top, ext, &cuts);
        let (v, _) = cut_prism(&lp, w.z0, w.h, &slots, false);
        layers.push(LayerGeom { material: mat.clone(), thickness: *lt, volume: v });
        top -= lt;
    }
    let geom = WallGeom { id: w.id, name: w.name.clone(), volume, thickness: w.t, side_area: volume / w.t, layers, joins: jout };
    Some(Built::new(e, Quantities::Wall(geom), place_wall_mesh(&w, local)))
}

/// An opening's own solid: the box it cuts from its host, over `[offset, offset + width]` along
/// the host centreline, through the full thickness, from the host base + `sill` up by `height`.
/// Reads the opening's params and its host wall's inputs (not the host's solid).
fn opening(cx: &Cx, e: &Entity) -> Option<Built> {
    let p = cx.db.get::<OpeningParams>(&e.id).as_ref().clone()?;
    let host = cx.get(p.host).filter(|h| h.kind == "Wall")?;
    let w = cx.db.get::<crate::query::WallInput>(&host.id).as_ref().clone()?;
    let slot = p.slot(&w);
    let plan = [[slot.s0, -w.t / 2.0], [slot.s1, -w.t / 2.0], [slot.s1, w.t / 2.0], [slot.s0, w.t / 2.0]];
    let (_, local) = cut_prism(&plan, slot.z0, slot.z1 - slot.z0, &[], true);
    Some(Built::new(e, Quantities::None, place_wall_mesh(&w, local)))
}

// ------------------------------------------------------------------ columns and beams

fn column(cx: &Cx, e: &Entity) -> Option<Mesh> {
    let (x, y) = (num(cx, e, "x")?, num(cx, e, "y")?);
    let (zb, zt) = (elevation(cx, e, "base")?, elevation(cx, e, "top")?);
    let (b, h) = (num(cx, e, "section_b")?, num(cx, e, "section_h")?);
    if !(b > 0.0) || !(h > 0.0) || !(zt > zb) {
        return None;
    }
    let poly = [[x - b / 2.0, y - h / 2.0], [x + b / 2.0, y - h / 2.0], [x + b / 2.0, y + h / 2.0], [x - b / 2.0, y + h / 2.0]];
    let (_, m) = cut_prism(&poly, zb, zt - zb, &[], true);
    Some(m).filter(|m| !m.triangles.is_empty())
}

fn beam(cx: &Cx, e: &Entity) -> Option<Mesh> {
    let (x1, y1, x2, y2) = (num(cx, e, "x1")?, num(cx, e, "y1")?, num(cx, e, "x2")?, num(cx, e, "y2")?);
    let top = elevation(cx, e, "level")? - num_or(cx, e, "z_offset", 0.0)?;
    let (b, h) = (num(cx, e, "section_b")?, num(cx, e, "section_h")?);
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

fn slab(cx: &Cx, e: &Entity, openings: &[Rc<Entity>]) -> Option<(SlabGeom, Mesh)> {
    let top = elevation(cx, e, "level")?;
    let th = num(cx, e, "thickness")?;
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
