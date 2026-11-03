//! IFC4 export of a [`Document`] (`bim export <dir> --format ifc`).
//!
//! **What is exported.** One `IfcProject` → `IfcSite` → `IfcBuilding` (`IfcRelAggregates`), one
//! `IfcBuildingStorey` per `Level` (aggregated into the building, `Elevation` and placement z equal to
//! the level elevation), and every building element [`bim_model::Model::compile`] builds:
//!
//! | Entity | IFC | Body | Containment |
//! |---|---|---|---|
//! | `Wall` | `IfcWall` + `IfcMaterialLayerSetUsage` of its WallType's `IfcMaterialLayerSet` (left layer first), `Pset_WallCommon` | the wall solid *before* openings (end cuts applied), extruded in the wall frame | its `level` |
//! | `Opening` | `IfcOpeningElement` voiding its host (`IfcRelVoidsElement`), filled (`IfcRelFillsElement`) by an `IfcDoor` (`Pset_DoorCommon`) or `IfcWindow` (`Pset_WindowCommon`) per `kind` | a box over the opening, through the full wall thickness | filling: the host's storey |
//! | `Slab` | `IfcSlab` (`Pset_SlabCommon`) | the slab solid (SlabOpenings already removed) | its `level` |
//! | `Column` | `IfcColumn` (`Pset_ColumnCommon`, its `Material` as `IfcMaterial`) | the column prism | its `base` level |
//! | `Beam` | `IfcBeam` (`Pset_BeamCommon`, its `Material`) | the beam prism | its `level` |
//! | `Space` | `IfcSpace` (`Pset_SpaceCommon`), placed at its `x y` | none (room geometry is derived) | aggregated into its `level` |
//!
//! **Geometry.** Bodies come from the bim-model meshes, so IFC volumes equal `bim solve` volumes: a
//! mesh that is a vertical prism over a convex plan polygon (walls, columns, beams) becomes an
//! `IfcExtrudedAreaSolid` of that polygon; any other mesh (e.g. a non-convex slab or a slab with
//! holes) an `IfcFacetedBrep` of its triangles. Walls carry their unopened solid and the openings
//! are `IfcOpeningElement`s, as IFC intends; since bim-model clips opening boxes to the wall, the
//! wall minus its opening boxes is exactly bim-model's wall solid. Opening boxes are extended a
//! little beyond the wall wherever they reach or cross a wall face, so no subtraction has coplanar
//! faces. Units: metre, square metre, cubic metre, radian.
//!
//! **GlobalIds.** The object exported for entity `e` has GlobalId `guid::compress(e.id)` (storeys
//! their Level's, opening elements their Opening's). Every other rooted object (project, site,
//! building, relationships, property sets, fillings) gets `compress(hash(role, owner id))`, where the
//! owner is the entity it belongs to (for project, site and building: the smallest entity id of
//! the document). Instances are emitted in entity-id order, the header time stamp is fixed and no
//! owner history is written, so the file is a pure function of the document and the output file
//! name: equal documents built in any op order export identical files.

use std::collections::{BTreeMap, HashMap};

use bim_doc::{Document, Entity, EntityId, Op, Value};
use bim_model::{Mesh, Model};

use crate::guid;
use crate::model::{Param, StepFile};
use crate::schema::Schema;

/// The `FILE_NAME` time stamp: fixed, so the file depends on the document only.
const TIME_STAMP: &str = "1970-01-01T00:00:00";
/// How far an opening box reaches past a wall face it touches or crosses.
const OPENING_MARGIN: f64 = 0.01;
/// Tolerance for the vertical-prism test of a mesh.
const PRISM_TOL: f64 = 1e-9;

/// The IFC4 file of `doc`. `file_name` goes into the `FILE_NAME` header entry.
pub fn export_ifc4(doc: &Document, file_name: &str) -> StepFile {
    let mut x = Exporter::new(doc, file_name);
    x.run();
    x.f
}

/// [`export_ifc4`] serialised as ISO 10303-21 text.
pub fn export_ifc4_string(doc: &Document, file_name: &str) -> String {
    export_ifc4(doc, file_name).to_step_string()
}

/// The GlobalId of the object exported for entity `id`.
pub fn entity_guid(id: EntityId) -> String {
    guid::compress(id.0)
}

/// The GlobalId of a derived object: `role` of the entity (or document key) `owner`.
pub fn derived_guid(role: &str, owner: u128) -> String {
    guid::compress(hash128(role, owner))
}

/// FNV-1a (128-bit) over `role`, a separator and `owner` (little endian), then an avalanche so
/// close inputs give unrelated GlobalIds.
fn hash128(role: &str, owner: u128) -> u128 {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = (1 << 88) + 0x13b;
    let mut h = OFFSET;
    for b in role.bytes().chain([0xff]).chain(owner.to_le_bytes()) {
        h ^= b as u128;
        h = h.wrapping_mul(PRIME);
    }
    h ^= h >> 67;
    h = h.wrapping_mul(0x9e3779b97f4a7c15f39cc0605cedc835);
    h ^ (h >> 59)
}

// ------------------------------------------------------------------ prop access (as bim-model)

fn entity_name(e: &Entity) -> String {
    bim_model::entity_name(e)
}

/// `key` of `e`, else of its `product` (spec/catalogs.md effective props), as bim-model reads it.
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

fn text<'a>(doc: &'a Document, e: &'a Entity, key: &str) -> Option<&'a str> {
    match prop(doc, e, key) {
        Some(Value::Text(s)) => Some(s),
        _ => None,
    }
}

fn target<'a>(doc: &'a Document, e: &Entity, key: &str, kind: &str) -> Option<&'a Entity> {
    match e.props.get(key) {
        Some(Value::Ref(id)) => doc.get(*id).filter(|t| t.kind == kind),
        _ => None,
    }
}

// ------------------------------------------------------------------ plan geometry

/// Convex hull (counter-clockwise, no collinear points) of 2D points; Andrew's monotone chain.
fn convex_hull(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut hull: Vec<[f64; 2]> = Vec::with_capacity(2 * pts.len());
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &[f64; 2]>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

fn ring_area(r: &[[f64; 2]]) -> f64 {
    (0..r.len()).map(|i| r[i][0] * r[(i + 1) % r.len()][1] - r[(i + 1) % r.len()][0] * r[i][1]).sum::<f64>() / 2.0
}

/// A local frame in plan: origin `o` (with height `z`), x axis `u` (unit), y axis = `u` turned left.
#[derive(Debug, Clone, Copy)]
struct Frame {
    o: [f64; 3],
    u: [f64; 2],
}

impl Frame {
    fn local(&self, p: [f64; 3]) -> [f64; 3] {
        let (dx, dy) = (p[0] - self.o[0], p[1] - self.o[1]);
        [dx * self.u[0] + dy * self.u[1], -dx * self.u[1] + dy * self.u[0], p[2] - self.o[2]]
    }
}

/// A body in local coordinates.
enum Body {
    /// Counter-clockwise convex polygon extruded from `z0` up by `depth`.
    Prism { poly: Vec<[f64; 2]>, z0: f64, depth: f64 },
    /// A closed triangle mesh.
    Brep { points: Vec<[f64; 3]>, triangles: Vec<[u32; 3]> },
}

/// The body of `mesh` in `frame`: an extrusion when the mesh is a vertical prism over a convex
/// polygon (every vertex at its bottom or top height, and the hull prism has the mesh's volume),
/// else the mesh itself.
fn body_of(mesh: &Mesh, frame: &Frame) -> Option<Body> {
    let points: Vec<[f64; 3]> = mesh.positions.iter().map(|p| frame.local([p.x, p.y, p.z])).collect();
    if mesh.triangles.is_empty() || points.is_empty() {
        return None;
    }
    let zlo = points.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
    let zhi = points.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
    let depth = zhi - zlo;
    let tol = PRISM_TOL * (1.0 + zlo.abs().max(zhi.abs()));
    if depth > 0.0 && points.iter().all(|p| (p[2] - zlo).abs() <= tol || (p[2] - zhi).abs() <= tol) {
        let poly = convex_hull(points.iter().map(|p| [p[0], p[1]]).collect());
        let volume = mesh.volume();
        let hull_volume = ring_area(&poly) * depth;
        if poly.len() >= 3 && (hull_volume - volume).abs() <= PRISM_TOL * volume.abs().max(1.0) * 10.0 {
            return Some(Body::Prism { poly, z0: zlo, depth });
        }
    }
    Some(Body::Brep { points, triangles: mesh.triangles.clone() })
}

// ------------------------------------------------------------------ exporter

/// One exported wall: its placement (origin at (x1, y1, base), x along the centreline) and extent.
struct WallOut {
    ifc: u64,
    placement: u64,
    thickness: f64,
    /// Plan extent of the unopened solid along the centreline, and its height.
    s_range: [f64; 2],
    height: f64,
    level: EntityId,
}

struct Storey {
    ifc: u64,
    placement: u64,
    elevation: f64,
    contained: Vec<u64>,
    spaces: Vec<u64>,
}

struct Exporter<'a> {
    doc: &'a Document,
    f: StepFile,
    /// Owner of the project / site / building GlobalIds.
    doc_key: u128,
    body_ctx: u64,
    origin3: u64,
    dir_z: u64,
    dir_x: u64,
    storeys: BTreeMap<EntityId, Storey>,
    materials: HashMap<String, u64>,
}

impl<'a> Exporter<'a> {
    fn new(doc: &'a Document, file_name: &str) -> Exporter<'a> {
        let f = StepFile::with_schema(Schema::Ifc4.identifier(), file_name, TIME_STAMP, "openbim bim-ifc");
        let doc_key = doc.entities().map(|e| e.id.0).min().unwrap_or(0);
        Exporter {
            doc,
            f,
            doc_key,
            body_ctx: 0,
            origin3: 0,
            dir_z: 0,
            dir_x: 0,
            storeys: BTreeMap::new(),
            materials: HashMap::new(),
        }
    }

    /// Adds an instance by attribute names. The entity and attribute names are compile-time
    /// constants of this module that the IFC4 table holds (the unit tests export every path).
    fn add(&mut self, entity: &str, attrs: &[(&str, Param)]) -> u64 {
        self.f
            .add_entity(&entity.to_ascii_uppercase(), attrs)
            .unwrap_or_else(|e| panic!("bim-ifc export: IFC4 table lacks {entity}: {e}"))
    }

    fn point3(&mut self, p: [f64; 3]) -> u64 {
        self.add("IfcCartesianPoint", &[("Coordinates", Param::reals(p))])
    }
    fn point2(&mut self, p: [f64; 2]) -> u64 {
        self.add("IfcCartesianPoint", &[("Coordinates", Param::reals(p))])
    }
    fn direction(&mut self, d: &[f64]) -> u64 {
        self.add("IfcDirection", &[("DirectionRatios", Param::reals(d.iter().copied()))])
    }

    /// An `IfcAxis2Placement3D` at `p` with x axis `u` in plan (z up).
    fn axis(&mut self, p: [f64; 3], u: [f64; 2]) -> u64 {
        let loc = if p == [0.0; 3] { self.origin3 } else { self.point3(p) };
        let refd = if u == [1.0, 0.0] { self.dir_x } else { self.direction(&[u[0], u[1], 0.0]) };
        let (z, x) = (self.dir_z, refd);
        self.add("IfcAxis2Placement3D", &[("Location", Param::Ref(loc)), ("Axis", Param::Ref(z)), ("RefDirection", Param::Ref(x))])
    }

    fn placement(&mut self, rel_to: Option<u64>, p: [f64; 3], u: [f64; 2]) -> u64 {
        let ax = self.axis(p, u);
        self.add(
            "IfcLocalPlacement",
            &[("PlacementRelTo", rel_to.map_or(Param::Unset, Param::Ref)), ("RelativePlacement", Param::Ref(ax))],
        )
    }

    fn label(s: &str) -> Param {
        Param::string(s)
    }

    fn root(&self, gid: String, name: &str) -> Vec<(&'static str, Param)> {
        vec![("GlobalId", Param::string(gid)), ("Name", Exporter::label(name))]
    }

    fn add_rooted(&mut self, entity: &str, gid: String, name: &str, mut more: Vec<(&'static str, Param)>) -> u64 {
        let mut attrs = self.root(gid, name);
        attrs.append(&mut more);
        self.add(entity, &attrs)
    }

    fn run(&mut self) {
        let doc = self.doc;
        // Shared geometry and the project context.
        self.origin3 = self.point3([0.0; 3]);
        self.dir_z = self.direction(&[0.0, 0.0, 1.0]);
        self.dir_x = self.direction(&[1.0, 0.0, 0.0]);
        let wcs = self.add(
            "IfcAxis2Placement3D",
            &[("Location", Param::Ref(self.origin3)), ("Axis", Param::Ref(self.dir_z)), ("RefDirection", Param::Ref(self.dir_x))],
        );
        let ctx = self.add(
            "IfcGeometricRepresentationContext",
            &[
                ("ContextType", Param::string("Model")),
                ("CoordinateSpaceDimension", Param::Integer(3)),
                ("Precision", Param::Real(1e-5)),
                ("WorldCoordinateSystem", Param::Ref(wcs)),
            ],
        );
        self.body_ctx = self.add(
            "IfcGeometricRepresentationSubContext",
            &[
                ("ContextIdentifier", Param::string("Body")),
                ("ContextType", Param::string("Model")),
                ("CoordinateSpaceDimension", Param::Derived),
                ("Precision", Param::Derived),
                ("WorldCoordinateSystem", Param::Derived),
                ("TrueNorth", Param::Derived),
                ("ParentContext", Param::Ref(ctx)),
                ("TargetView", Param::enumeration("MODEL_VIEW")),
            ],
        );
        let units: Vec<u64> = [("LENGTHUNIT", "METRE"), ("AREAUNIT", "SQUARE_METRE"), ("VOLUMEUNIT", "CUBIC_METRE"), ("PLANEANGLEUNIT", "RADIAN")]
            .iter()
            .map(|(t, n)| {
                self.add(
                    "IfcSIUnit",
                    &[("Dimensions", Param::Derived), ("UnitType", Param::enumeration(*t)), ("Name", Param::enumeration(*n))],
                )
            })
            .collect();
        let ua = self.add("IfcUnitAssignment", &[("Units", Param::refs(units))]);
        let key = self.doc_key;
        let project = self.add_rooted(
            "IfcProject",
            derived_guid("IfcProject", key),
            "Project",
            vec![("RepresentationContexts", Param::refs([ctx])), ("UnitsInContext", Param::Ref(ua))],
        );
        let site_pl = self.placement(None, [0.0; 3], [1.0, 0.0]);
        let site = self.add_rooted(
            "IfcSite",
            derived_guid("IfcSite", key),
            "Site",
            vec![("ObjectPlacement", Param::Ref(site_pl)), ("CompositionType", Param::enumeration("ELEMENT"))],
        );
        let bld_pl = self.placement(Some(site_pl), [0.0; 3], [1.0, 0.0]);
        let building = self.add_rooted(
            "IfcBuilding",
            derived_guid("IfcBuilding", key),
            "Building",
            vec![("ObjectPlacement", Param::Ref(bld_pl)), ("CompositionType", Param::enumeration("ELEMENT"))],
        );
        self.aggregate(derived_guid("IfcRelAggregates/project", key), project, vec![site]);
        self.aggregate(derived_guid("IfcRelAggregates/site", key), site, vec![building]);

        // Storeys, one per Level, aggregated by ascending elevation.
        for e in doc.entities().filter(|e| e.kind == "Level") {
            let elev = num(doc, e, "elevation");
            let z = elev.unwrap_or(0.0);
            let pl = self.placement(Some(bld_pl), [0.0, 0.0, z], [1.0, 0.0]);
            let st = self.add_rooted(
                "IfcBuildingStorey",
                entity_guid(e.id),
                &entity_name(e),
                vec![
                    ("ObjectPlacement", Param::Ref(pl)),
                    ("CompositionType", Param::enumeration("ELEMENT")),
                    ("Elevation", elev.map_or(Param::Unset, Param::Real)),
                ],
            );
            self.storeys.insert(e.id, Storey { ifc: st, placement: pl, elevation: z, contained: Vec::new(), spaces: Vec::new() });
        }
        let mut order: Vec<(f64, EntityId, u64)> = self.storeys.iter().map(|(id, s)| (s.elevation, *id, s.ifc)).collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        if !order.is_empty() {
            self.aggregate(derived_guid("IfcRelAggregates/building", key), building, order.iter().map(|o| o.2).collect());
        }

        // Elements.
        let model = Model::compile(doc);
        let mut unopened = doc.clone();
        let openings: Vec<EntityId> = doc.entities().filter(|e| e.kind == "Opening").map(|e| e.id).collect();
        for id in &openings {
            let _ = unopened.apply(Op::Delete { id: *id });
        }
        let bare = if openings.is_empty() { None } else { Some(Model::compile(&unopened)) };
        let bare = bare.as_ref().unwrap_or(&model);

        let mut walls: BTreeMap<EntityId, WallOut> = BTreeMap::new();
        let mut walls_by_type: BTreeMap<EntityId, Vec<u64>> = BTreeMap::new();
        let mut by_material: BTreeMap<EntityId, Vec<u64>> = BTreeMap::new();
        let mut elements: Vec<&bim_model::Element> = model.elements.iter().collect();
        elements.sort_by_key(|el| el.id);
        for el in elements {
            let Some(e) = doc.get(el.id) else { continue };
            match el.kind.as_str() {
                "Wall" => {
                    let Some(mesh) = bare.element(el.id).map(|b| &b.mesh) else { continue };
                    if let Some(w) = self.wall(e, mesh) {
                        if let Some(wt) = target(doc, e, "type", "WallType") {
                            walls_by_type.entry(wt.id).or_default().push(w.ifc);
                        }
                        walls.insert(e.id, w);
                    }
                }
                "Slab" | "Column" | "Beam" => {
                    let (class, level_key, pset) = match el.kind.as_str() {
                        "Slab" => ("IfcSlab", "level", "Pset_SlabCommon"),
                        "Column" => ("IfcColumn", "base", "Pset_ColumnCommon"),
                        _ => ("IfcBeam", "level", "Pset_BeamCommon"),
                    };
                    let Some(level) = target(doc, e, level_key, "Level").map(|l| l.id) else { continue };
                    let Some(ifc) = self.simple_element(e, class, level, &el.mesh) else { continue };
                    let mut props = vec![("Reference", Param::typed("IfcIdentifier", Param::string(entity_name(e))))];
                    if class != "IfcSlab" {
                        props.push(("LoadBearing", Param::typed("IfcBoolean", Param::boolean(true))));
                        if let Some(m) = target(doc, e, "material", "Material") {
                            by_material.entry(m.id).or_default().push(ifc);
                        }
                    }
                    self.pset(e.id, ifc, pset, props);
                }
                _ => {}
            }
        }

        // Openings and their fillings.
        for e in doc.entities().filter(|e| e.kind == "Opening") {
            let Some(host) = target(doc, e, "host", "Wall") else { continue };
            let Some(w) = walls.get(&host.id) else { continue };
            self.opening(e, w);
        }

        // Wall materials: one layer set (and usage) per WallType.
        for (wt_id, ifcs) in &walls_by_type {
            let Some(wt) = doc.get(*wt_id) else { continue };
            self.layer_set(wt, ifcs.clone());
        }
        for (m_id, ifcs) in &by_material {
            let Some(m) = doc.get(*m_id) else { continue };
            let name = text(doc, m, "name").map(str::to_string).unwrap_or_else(|| entity_name(m));
            let mat = self.material(&name);
            self.add_rooted(
                "IfcRelAssociatesMaterial",
                derived_guid("IfcRelAssociatesMaterial", m_id.0),
                &name,
                vec![("RelatedObjects", Param::refs(ifcs.clone())), ("RelatingMaterial", Param::Ref(mat))],
            );
        }

        // Spaces.
        for e in doc.entities().filter(|e| e.kind == "Space") {
            self.space(e);
        }

        // Spatial containment.
        let ids: Vec<EntityId> = self.storeys.keys().copied().collect();
        for id in ids {
            let (st, contained, spaces) = {
                let s = &self.storeys[&id];
                (s.ifc, s.contained.clone(), s.spaces.clone())
            };
            if !contained.is_empty() {
                self.add_rooted(
                    "IfcRelContainedInSpatialStructure",
                    derived_guid("IfcRelContainedInSpatialStructure", id.0),
                    "",
                    vec![("RelatedElements", Param::refs(contained)), ("RelatingStructure", Param::Ref(st))],
                );
            }
            if !spaces.is_empty() {
                self.aggregate(derived_guid("IfcRelAggregates/storey", id.0), st, spaces);
            }
        }
    }

    fn aggregate(&mut self, gid: String, parent: u64, children: Vec<u64>) -> u64 {
        self.add_rooted("IfcRelAggregates", gid, "", vec![("RelatingObject", Param::Ref(parent)), ("RelatedObjects", Param::refs(children))])
    }

    fn material(&mut self, name: &str) -> u64 {
        if let Some(&m) = self.materials.get(name) {
            return m;
        }
        let m = self.add("IfcMaterial", &[("Name", Param::string(name))]);
        self.materials.insert(name.to_string(), m);
        m
    }

    /// `Pset_*Common` with the given properties, assigned to `object`.
    fn pset(&mut self, owner: EntityId, object: u64, name: &str, props: Vec<(&str, Param)>) {
        let props: Vec<u64> = props
            .into_iter()
            .map(|(n, v)| self.add("IfcPropertySingleValue", &[("Name", Param::string(n)), ("NominalValue", v)]))
            .collect();
        let ps = self.add_rooted("IfcPropertySet", derived_guid(name, owner.0), name, vec![("HasProperties", Param::refs(props))]);
        self.add_rooted(
            "IfcRelDefinesByProperties",
            derived_guid(&format!("IfcRelDefinesByProperties/{name}"), owner.0),
            "",
            vec![("RelatedObjects", Param::refs([object])), ("RelatingPropertyDefinition", Param::Ref(ps))],
        );
    }

    /// The body representation of `body`.
    fn shape(&mut self, body: Body) -> u64 {
        let (item, kind) = match body {
            Body::Prism { poly, z0, depth } => {
                let mut pts: Vec<u64> = poly.iter().map(|&p| self.point2(p)).collect();
                pts.push(pts[0]);
                let line = self.add("IfcPolyline", &[("Points", Param::refs(pts))]);
                let profile =
                    self.add("IfcArbitraryClosedProfileDef", &[("ProfileType", Param::enumeration("AREA")), ("OuterCurve", Param::Ref(line))]);
                let pos = self.axis([0.0, 0.0, z0], [1.0, 0.0]);
                let dz = self.dir_z;
                let solid = self.add(
                    "IfcExtrudedAreaSolid",
                    &[("SweptArea", Param::Ref(profile)), ("Position", Param::Ref(pos)), ("ExtrudedDirection", Param::Ref(dz)), ("Depth", Param::Real(depth))],
                );
                (solid, "SweptSolid")
            }
            Body::Brep { points, triangles } => {
                let ids: Vec<u64> = points.iter().map(|&p| self.point3(p)).collect();
                let faces: Vec<u64> = triangles
                    .iter()
                    .map(|t| {
                        let lp = self.add("IfcPolyLoop", &[("Polygon", Param::refs(t.iter().map(|&i| ids[i as usize])))]);
                        let b = self.add("IfcFaceOuterBound", &[("Bound", Param::Ref(lp)), ("Orientation", Param::boolean(true))]);
                        self.add("IfcFace", &[("Bounds", Param::refs([b]))])
                    })
                    .collect();
                let shell = self.add("IfcClosedShell", &[("CfsFaces", Param::refs(faces))]);
                (self.add("IfcFacetedBrep", &[("Outer", Param::Ref(shell))]), "Brep")
            }
        };
        let ctx = self.body_ctx;
        let rep = self.add(
            "IfcShapeRepresentation",
            &[
                ("ContextOfItems", Param::Ref(ctx)),
                ("RepresentationIdentifier", Param::string("Body")),
                ("RepresentationType", Param::string(kind)),
                ("Items", Param::refs([item])),
            ],
        );
        self.add("IfcProductDefinitionShape", &[("Representations", Param::refs([rep]))])
    }

    /// A slab, column or beam: placed at its storey, body from its mesh in storey coordinates.
    fn simple_element(&mut self, e: &Entity, class: &str, level: EntityId, mesh: &Mesh) -> Option<u64> {
        let (st_pl, elev) = self.storeys.get(&level).map(|s| (s.placement, s.elevation))?;
        let frame = Frame { o: [0.0, 0.0, elev], u: [1.0, 0.0] };
        let body = body_of(mesh, &frame)?;
        let pl = self.placement(Some(st_pl), [0.0; 3], [1.0, 0.0]);
        let shape = self.shape(body);
        let ifc = self.add_rooted(
            class,
            entity_guid(e.id),
            &entity_name(e),
            vec![
                ("ObjectType", Param::string(e.kind.clone())),
                ("ObjectPlacement", Param::Ref(pl)),
                ("Representation", Param::Ref(shape)),
                ("PredefinedType", Param::enumeration(if class == "IfcSlab" { "FLOOR" } else { "NOTDEFINED" })),
            ],
        );
        self.storeys.get_mut(&level)?.contained.push(ifc);
        Some(ifc)
    }

    /// A wall in its own frame: origin at (x1, y1, base), x along the centreline.
    fn wall(&mut self, e: &Entity, mesh: &Mesh) -> Option<WallOut> {
        let doc = self.doc;
        let level = target(doc, e, "level", "Level")?.id;
        let (st_pl, elev) = self.storeys.get(&level).map(|s| (s.placement, s.elevation))?;
        let (x1, y1, x2, y2) = (num(doc, e, "x1")?, num(doc, e, "y1")?, num(doc, e, "x2")?, num(doc, e, "y2")?);
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        let u = [(x2 - x1) / len, (y2 - y1) / len];
        let wt = target(doc, e, "type", "WallType")?;
        let thickness: f64 = match wt.props.get("layer_thickness") {
            Some(Value::List(l)) => l.iter().map(|v| if let Value::Num(x) = v { *x } else { 0.0 }).sum(),
            _ => return None,
        };
        let zb = mesh.positions.iter().map(|p| p.z).fold(f64::INFINITY, f64::min);
        let frame = Frame { o: [x1, y1, zb], u };
        let body = body_of(mesh, &frame)?;
        let (s_range, height) = match &body {
            Body::Prism { poly, depth, .. } => (
                [poly.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), poly.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max)],
                *depth,
            ),
            Body::Brep { points, .. } => (
                [points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), points.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max)],
                points.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max),
            ),
        };
        let pl = self.placement(Some(st_pl), [x1, y1, zb - elev], u);
        let shape = self.shape(body);
        let ifc = self.add_rooted(
            "IfcWall",
            entity_guid(e.id),
            &entity_name(e),
            vec![
                ("ObjectType", Param::string(entity_name(wt))),
                ("ObjectPlacement", Param::Ref(pl)),
                ("Representation", Param::Ref(shape)),
                ("PredefinedType", Param::enumeration("STANDARD")),
            ],
        );
        self.storeys.get_mut(&level)?.contained.push(ifc);
        self.pset(e.id, ifc, "Pset_WallCommon", vec![("Reference", Param::typed("IfcIdentifier", Param::string(entity_name(wt))))]);
        Some(WallOut { ifc, placement: pl, thickness, s_range, height, level })
    }

    /// An opening of a wall: the void box, the voids relationship and its door or window.
    fn opening(&mut self, e: &Entity, w: &WallOut) {
        let doc = self.doc;
        let (Some(offset), Some(width), Some(sill), Some(height)) =
            (num(doc, e, "offset"), num(doc, e, "width"), num(doc, e, "sill"), num(doc, e, "height"))
        else {
            return;
        };
        if !(width > 0.0) || !(height > 0.0) {
            return;
        }
        // The box in wall coordinates, pushed past every wall face it reaches.
        let m = OPENING_MARGIN;
        let eps = 1e-9 * (1.0 + w.s_range[1].abs().max(w.height));
        let s0 = if offset <= w.s_range[0] + eps { offset.min(w.s_range[0]) - m } else { offset };
        let s1 = if offset + width >= w.s_range[1] - eps { (offset + width).max(w.s_range[1]) + m } else { offset + width };
        let z0 = if sill <= eps { sill.min(0.0) - m } else { sill };
        let z1 = if sill + height >= w.height - eps { (sill + height).max(w.height) + m } else { sill + height };
        let half = w.thickness / 2.0 + m;
        let name = entity_name(e);
        let pl = self.placement(Some(w.placement), [offset, 0.0, sill], [1.0, 0.0]);
        let body = Body::Prism {
            poly: vec![[s0 - offset, -half], [s1 - offset, -half], [s1 - offset, half], [s0 - offset, half]],
            z0: z0 - sill,
            depth: z1 - z0,
        };
        let shape = self.shape(body);
        let op = self.add_rooted(
            "IfcOpeningElement",
            entity_guid(e.id),
            &name,
            vec![
                ("ObjectPlacement", Param::Ref(pl)),
                ("Representation", Param::Ref(shape)),
                ("PredefinedType", Param::enumeration("OPENING")),
            ],
        );
        self.add_rooted(
            "IfcRelVoidsElement",
            derived_guid("IfcRelVoidsElement", e.id.0),
            "",
            vec![("RelatingBuildingElement", Param::Ref(w.ifc)), ("RelatedOpeningElement", Param::Ref(op))],
        );
        let kind = text(doc, e, "kind").unwrap_or("");
        let (class, pset) = match kind {
            "door" => ("IfcDoor", "Pset_DoorCommon"),
            "window" => ("IfcWindow", "Pset_WindowCommon"),
            _ => return,
        };
        let fpl = self.placement(Some(pl), [0.0; 3], [1.0, 0.0]);
        let mut attrs = vec![
            ("ObjectType", Param::string(kind)),
            ("ObjectPlacement", Param::Ref(fpl)),
            ("OverallHeight", Param::Real(height)),
            ("OverallWidth", Param::Real(width)),
        ];
        attrs.push(("PredefinedType", Param::enumeration(if class == "IfcDoor" { "DOOR" } else { "WINDOW" })));
        let fill = self.add_rooted(class, derived_guid(class, e.id.0), &name, attrs);
        self.add_rooted(
            "IfcRelFillsElement",
            derived_guid("IfcRelFillsElement", e.id.0),
            "",
            vec![("RelatingOpeningElement", Param::Ref(op)), ("RelatedBuildingElement", Param::Ref(fill))],
        );
        if let Some(st) = self.storeys.get_mut(&w.level) {
            st.contained.push(fill);
        }
        let reference = match prop(doc, e, "product") {
            Some(Value::Ref(p)) => doc.get(*p).map(entity_name).unwrap_or_else(|| name.clone()),
            _ => name.clone(),
        };
        self.pset(e.id, fill, pset, vec![("Reference", Param::typed("IfcIdentifier", Param::string(reference)))]);
    }

    /// The WallType's layer set (left layer first) and its usage, associated with its walls.
    fn layer_set(&mut self, wt: &Entity, walls: Vec<u64>) {
        let thick: Vec<f64> = match wt.props.get("layer_thickness") {
            Some(Value::List(l)) => l.iter().map(|v| if let Value::Num(x) = v { *x } else { 0.0 }).collect(),
            _ => return,
        };
        let names: Vec<String> = match wt.props.get("layer_material") {
            Some(Value::List(l)) => l.iter().map(|v| if let Value::Text(s) = v { s.clone() } else { String::new() }).collect(),
            _ => Vec::new(),
        };
        let total: f64 = thick.iter().sum();
        let layers: Vec<u64> = thick
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                let name = names.get(i).cloned().unwrap_or_default();
                let mat = self.material(&name);
                self.add("IfcMaterialLayer", &[("Material", Param::Ref(mat)), ("LayerThickness", Param::Real(t)), ("Name", Param::string(name))])
            })
            .collect();
        let wt_name = entity_name(wt);
        let set = self.add("IfcMaterialLayerSet", &[("MaterialLayers", Param::refs(layers)), ("LayerSetName", Param::string(wt_name.clone()))]);
        // Layers run from the left face (+y in the wall frame) towards −y.
        let usage = self.add(
            "IfcMaterialLayerSetUsage",
            &[
                ("ForLayerSet", Param::Ref(set)),
                ("LayerSetDirection", Param::enumeration("AXIS2")),
                ("DirectionSense", Param::enumeration("NEGATIVE")),
                ("OffsetFromReferenceLine", Param::Real(total / 2.0)),
            ],
        );
        self.add_rooted(
            "IfcRelAssociatesMaterial",
            derived_guid("IfcRelAssociatesMaterial", wt.id.0),
            &wt_name,
            vec![("RelatedObjects", Param::refs(walls)), ("RelatingMaterial", Param::Ref(usage))],
        );
    }

    fn space(&mut self, e: &Entity) {
        let doc = self.doc;
        let Some(level) = target(doc, e, "level", "Level").map(|l| l.id) else { return };
        let Some(st_pl) = self.storeys.get(&level).map(|s| s.placement) else { return };
        let (x, y) = (num(doc, e, "x").unwrap_or(0.0), num(doc, e, "y").unwrap_or(0.0));
        let pl = self.placement(Some(st_pl), [x, y, 0.0], [1.0, 0.0]);
        let long = text(doc, e, "name").map(str::to_string);
        let usage = text(doc, e, "use").map(str::to_string);
        let sp = self.add_rooted(
            "IfcSpace",
            entity_guid(e.id),
            &entity_name(e),
            vec![
                ("ObjectType", usage.map_or(Param::Unset, Param::string)),
                ("ObjectPlacement", Param::Ref(pl)),
                ("LongName", long.map_or(Param::Unset, Param::string)),
                ("CompositionType", Param::enumeration("ELEMENT")),
                ("PredefinedType", Param::enumeration("SPACE")),
            ],
        );
        if let Some(s) = self.storeys.get_mut(&level) {
            s.spaces.push(sp);
        }
        let props = vec![("Reference", Param::typed("IfcIdentifier", Param::string(entity_name(e))))];
        self.pset(e.id, sp, "Pset_SpaceCommon", props);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_drops_interior_and_collinear_points() {
        let h = convex_hull(vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 0.5], [0.0, 1.0]]);
        assert_eq!(h, vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]]);
        assert!((ring_area(&h) - 2.0).abs() < 1e-15);
    }

    #[test]
    fn derived_guids_are_distinct_and_stable() {
        let a = derived_guid("IfcRelVoidsElement", 7);
        assert_eq!(a, derived_guid("IfcRelVoidsElement", 7));
        assert_ne!(a, derived_guid("IfcRelVoidsElement", 8));
        assert_ne!(a, derived_guid("IfcRelFillsElement", 7));
        assert_eq!(a.len(), 22);
        assert_eq!(entity_guid(EntityId(0)), "0000000000000000000000");
    }

    fn n(x: f64) -> Value {
        Value::Num(x)
    }
    fn r(s: &str) -> Value {
        Value::Ref(EntityId::derive(s))
    }
    fn list(xs: &[f64]) -> Value {
        Value::List(xs.iter().map(|&x| Value::Num(x)).collect())
    }

    /// A small building: two L-joined walls (one with a door, one with a window), a slab with a
    /// hole, a column, a beam and a space on two levels.
    fn entities() -> Vec<(&'static str, &'static str, Vec<(&'static str, Value)>)> {
        let t = |s: &str| Value::Text(s.into());
        vec![
            ("L0", "Level", vec![("elevation", n(0.0))]),
            ("L1", "Level", vec![("elevation", n(3.0))]),
            ("WT", "WallType", vec![("layer_thickness", list(&[0.1, 0.2])), ("layer_material", Value::List(vec![t("Brick"), t("Block")]))]),
            ("M", "Material", vec![("name", t("Steel"))]),
            ("W1", "Wall", vec![("level", r("L0")), ("type", r("WT")), ("x1", n(0.0)), ("y1", n(0.0)), ("x2", n(5.0)), ("y2", n(0.0)), ("height", n(3.0))]),
            ("W2", "Wall", vec![("level", r("L0")), ("type", r("WT")), ("x1", n(5.0)), ("y1", n(0.0)), ("x2", n(5.0)), ("y2", n(4.0)), ("height", n(3.0))]),
            ("D", "Opening", vec![("host", r("W1")), ("kind", t("door")), ("offset", n(1.0)), ("sill", n(0.0)), ("width", n(0.9)), ("height", n(2.1))]),
            ("N", "Opening", vec![("host", r("W2")), ("kind", t("window")), ("offset", n(1.0)), ("sill", n(1.0)), ("width", n(1.0)), ("height", n(1.0))]),
            ("S", "Slab", vec![("level", r("L1")), ("boundary", list(&[0.0, 0.0, 5.0, 0.0, 5.0, 4.0, 0.0, 4.0])), ("thickness", n(0.2))]),
            ("H", "SlabOpening", vec![("host", r("S")), ("boundary", list(&[1.0, 1.0, 2.0, 1.0, 2.0, 2.0, 1.0, 2.0]))]),
            ("C", "Column", vec![("x", n(2.0)), ("y", n(2.0)), ("base", r("L0")), ("top", r("L1")), ("section_b", n(0.3)), ("section_h", n(0.3)), ("material", r("M"))]),
            ("B", "Beam", vec![("level", r("L1")), ("x1", n(0.0)), ("y1", n(2.0)), ("x2", n(5.0)), ("y2", n(2.0)), ("section_b", n(0.2)), ("section_h", n(0.4)), ("material", r("M"))]),
            ("SP", "Space", vec![("level", r("L0")), ("x", n(2.0)), ("y", n(2.0)), ("name", t("Hall"))]),
        ]
    }

    fn build(creates_first_reversed: bool) -> Document {
        let mut doc = Document::new();
        let ents = entities();
        let mut ops = Vec::new();
        if creates_first_reversed {
            for (name, kind, _) in ents.iter().rev() {
                ops.push(Op::Create { id: EntityId::derive(name), kind: kind.to_string() });
            }
            for (name, _, props) in ents.iter().rev() {
                for (k, v) in props.iter().rev() {
                    ops.push(Op::Set { id: EntityId::derive(name), key: k.to_string(), value: v.clone() });
                }
            }
        } else {
            for (name, kind, props) in &ents {
                ops.push(Op::Create { id: EntityId::derive(name), kind: kind.to_string() });
                for (k, v) in props {
                    ops.push(Op::Set { id: EntityId::derive(name), key: k.to_string(), value: v.clone() });
                }
            }
        }
        for (name, _, _) in &ents {
            ops.push(Op::Set { id: EntityId::derive(name), key: "_name".into(), value: Value::Text(name.to_string()) });
        }
        for op in ops {
            doc.apply(op).unwrap();
        }
        doc
    }

    fn by_guid<'a>(f: &'a StepFile, id: &str) -> crate::Entity<'a> {
        let g = entity_guid(EntityId::derive(id));
        f.entities_of("IfcRoot").find(|e| e.get_str("GlobalId") == Some(g.as_str())).unwrap_or_else(|| panic!("no {id}"))
    }

    #[test]
    fn exports_hierarchy_elements_relationships_and_psets() {
        let text = export_ifc4_string(&build(false), "t.ifc");
        let f = StepFile::parse_str(&text).unwrap();
        assert_eq!(f.schema(), Schema::Ifc4);
        for (cls, k) in [("IfcProject", 1), ("IfcSite", 1), ("IfcBuilding", 1), ("IfcBuildingStorey", 2), ("IfcWall", 2), ("IfcSlab", 1),
            ("IfcColumn", 1), ("IfcBeam", 1), ("IfcOpeningElement", 2), ("IfcRelVoidsElement", 2), ("IfcRelFillsElement", 2),
            ("IfcDoor", 1), ("IfcWindow", 1), ("IfcSpace", 1), ("IfcMaterialLayerSet", 1)]
        {
            assert_eq!(f.entities_of(cls).count(), k, "{cls}");
        }
        // Entity GlobalIds, unique over IfcRoot.
        assert!(by_guid(&f, "L1").is_a("IfcBuildingStorey"));
        assert_eq!(by_guid(&f, "L1").get_f64("Elevation"), Some(3.0));
        assert!(by_guid(&f, "W1").is_a("IfcWall") && by_guid(&f, "D").is_a("IfcOpeningElement") && by_guid(&f, "SP").is_a("IfcSpace"));
        let mut gids: Vec<&str> = f.entities_of("IfcRoot").filter_map(|e| e.get_str("GlobalId")).collect();
        let total = gids.len();
        gids.sort();
        gids.dedup();
        assert_eq!(gids.len(), total);
        // Opening D voids W1 and is filled by a door.
        let d = by_guid(&f, "D").id();
        let voids = f.entities_of("IfcRelVoidsElement").find(|r| r.get_ref("RelatedOpeningElement") == Some(d)).unwrap();
        assert_eq!(voids.get_ref("RelatingBuildingElement"), Some(by_guid(&f, "W1").id()));
        let fills = f.entities_of("IfcRelFillsElement").find(|r| r.get_ref("RelatingOpeningElement") == Some(d)).unwrap();
        assert!(fills.get_entity("RelatedBuildingElement").unwrap().is_a("IfcDoor"));
        // Every element carries its Pset_<Class>Common.
        let psets: Vec<(u64, String)> = f
            .entities_of("IfcRelDefinesByProperties")
            .flat_map(|r| {
                let name = r.get_entity("RelatingPropertyDefinition").and_then(|p| p.get_str("Name").map(str::to_string)).unwrap();
                r.get_refs("RelatedObjects").into_iter().map(move |o| (o, name.clone()))
            })
            .collect();
        for (id, pset) in [("W1", "Pset_WallCommon"), ("S", "Pset_SlabCommon"), ("C", "Pset_ColumnCommon"), ("B", "Pset_BeamCommon"), ("SP", "Pset_SpaceCommon")] {
            assert!(psets.contains(&(by_guid(&f, id).id(), pset.to_string())), "{id} {pset}");
        }
        let door = f.entities_of("IfcDoor").next().unwrap().id();
        assert!(psets.contains(&(door, "Pset_DoorCommon".to_string())));
        // Wall layers left first; the slab with a hole is a faceted brep, walls are extrusions.
        let set = f.entities_of("IfcMaterialLayerSet").next().unwrap();
        let t: Vec<f64> = set.get_entities("MaterialLayers").iter().map(|l| l.get_f64("LayerThickness").unwrap()).collect();
        assert_eq!(t, vec![0.1, 0.2]);
        assert_eq!(f.entities_of("IfcFacetedBrep").count(), 1);
        assert_eq!(f.entities_of("IfcExtrudedAreaSolid").count(), 2 + 1 + 1 + 2);
    }

    #[test]
    fn output_does_not_depend_on_op_order() {
        assert_eq!(export_ifc4_string(&build(false), "x.ifc"), export_ifc4_string(&build(true), "x.ifc"));
    }

    #[test]
    fn wall_extrusion_matches_the_unopened_solid() {
        let f = StepFile::parse_str(&export_ifc4_string(&build(false), "t.ifc")).unwrap();
        // W1 runs 0..5 along x, mitred at x = 5 with W2: the profile spans the full wall length
        // (the door is not cut into it) and the depth is the wall height.
        let w1 = by_guid(&f, "W1");
        let rep = w1.get_entity("Representation").unwrap().get_entities("Representations")[0];
        let solid = rep.get_entities("Items")[0];
        assert!(solid.is_a("IfcExtrudedAreaSolid"));
        assert_eq!(solid.get_f64("Depth"), Some(3.0));
        let line = solid.get_entity("SweptArea").unwrap().get_entity("OuterCurve").unwrap();
        let pts: Vec<Vec<f64>> = line.get_entities("Points").iter().map(|p| p.get_f64s("Coordinates")).collect();
        assert_eq!(pts.first(), pts.last());
        let area = ring_area(&pts[..pts.len() - 1].iter().map(|p| [p[0], p[1]]).collect::<Vec<_>>());
        let model = Model::compile(&build(false));
        let mut bare = build(false);
        for id in ["D", "N"] {
            bare.apply(Op::Delete { id: EntityId::derive(id) }).unwrap();
        }
        let v = Model::compile(&bare).walls.iter().find(|w| w.name == "W1").unwrap().volume;
        assert!((area * 3.0 - v).abs() < 1e-9, "{area} {v}");
        assert!(model.walls.iter().find(|w| w.name == "W1").unwrap().volume < v);
    }
}
