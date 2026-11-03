//! IFC4 / IFC2X3 import into document entities (`bim import <dir> <file.ifc>`, spec/cli.md).
//!
//! [`import_ifc`] maps a parsed [`StepFile`] to a list of [`ImportedEntity`]s (kind and props, in
//! creation order); the CLI turns them into ops. The mapping is a pure function of the file.
//!
//! **Identity.** An entity made from an IFC object has the 128 bits of the object's GlobalId as
//! its id, spelled as a 26-character ULID (so `bim export` reproduces the GlobalId, T5.2).
//! Entities the import derives (WallTypes, SlabOpenings from profile voids) get seeds that are
//! functions of their content or of their owner's id.
//!
//! **Units and placements.** Lengths are converted to metres from the project's length unit
//! (`IfcSIUnit` with any prefix, or an `IfcConversionBasedUnit` such as the foot); area and
//! volume units likewise for quantities and property values of those measures. Every
//! `IfcLocalPlacement` chain is resolved to a world transform; representation items are resolved
//! through `IfcMappedItem`s (`MappingTarget ∘ MappingOrigin`).
//!
//! **Mapping.**
//!
//! | IFC | Entity |
//! |---|---|
//! | `IfcBuildingStorey` | `Level`, `elevation` = the storey's world z (its `Elevation` when it has no placement) |
//! | `IfcWall` whose body is one vertical extrusion of a rectangle | `Wall` (centreline = the rectangle's midline along the placement's x axis, else along its long side) with a `WallType` of its material layers (left layer first) |
//! | `IfcSlab` whose body is one vertical extrusion whose top lies at a storey's elevation | `Slab` (`boundary` counter-clockwise; profile voids become `SlabOpening`s) |
//! | `IfcColumn`: one vertical extrusion of an axis-aligned rectangle from one storey's elevation to another's | `Column` |
//! | `IfcBeam`: one horizontal extrusion of a rectangle with a horizontal and a vertical side (or one vertical extrusion of a rectangle) | `Beam` |
//! | `IfcOpeningElement` voiding an imported Wall with a box through its full thickness | `Opening` (`kind` door/window from its `IfcDoor`/`IfcWindow` filling (or subtype), which is not imported separately; an opening without such a filling is a `door` when it starts at the wall's base (`sill` 0) and a `window` otherwise, with a warning) |
//! | `IfcOpeningElement` voiding an imported Slab with a vertical prism through it | `SlabOpening` |
//! | `IfcSpace` | `Space` (`x y` a point inside its footprint, `name` its LongName, else its Name) |
//! | any other element, or one of the above whose body is not such an extrusion, or a Wall / Slab with an opening that is not such a box / prism, or a Column / Beam with openings | `Shape` (spec/sketch.md): `classification` = its IFC class, `level` = its storey, faces = its body's faces in its placement frame, with the bodies of its openings subtracted, `transform` = its placement |
//!
//! Classes are matched with their subtypes (`IfcWallStandardCase`, `IfcColumnStandardCase`,
//! `IfcDoorStandardCase`, …; see [`Schema::is_subtype_of`]).
//!
//! Shape faces come from extrusions (polygonal profiles; circles and arcs are tessellated),
//! faceted breps, shell- and face-based surface models, IFC4 triangulated / polygonal face sets,
//! `IfcBlock`s and boolean results (`IfcBooleanResult`, `IfcBooleanClippingResult`, `IfcCsgSolid`)
//! of those, evaluated with `bim_geom::boolean`: an `IfcHalfSpaceSolid` operand (on an `IfcPlane`)
//! becomes a box covering the other operand on the material side of the plane (`AgreementFlag`
//! true: the side the plane normal points away from), an `IfcPolygonalBoundedHalfSpace` that box
//! intersected with its boundary's prism. An element's openings are subtracted from its body the
//! same way when it becomes a Shape; those openings are not imported on their own (their fillings
//! are imported as elements in their own right). Vertices are shared by exact coordinates, a face
//! that is not planar within 1e-9 m is split into triangles fanned from its first vertex, and
//! closed shells are oriented outwards.
//!
//! Every element keeps its `Name` as the prop `name` and its property sets (occurrence over type)
//! as props `<PsetName>.<PropName>`: labels, texts and identifiers → Text, numeric measures and
//! integers → Num, IfcBoolean (and a definite IfcLogical) → Bool, enumerated and list values →
//! List; element quantities likewise as `<QtoName>.<QuantityName>`.
//!
//! **Invalid files.** A `FILE_SCHEMA` other than IFC4 (`IFC4`, `IFC4_ADD1`, `IFC4_ADD2`,
//! `IFC4_ADD2_TC1`) or IFC2X3 (`IFC2X3`, `IFC2X3_TC1`), and a reference to an instance the file
//! does not define anywhere under a product's placement or representation, make the whole file an
//! [`ImportError`].
//!
//! What cannot be mapped (curved sweeps, booleans the kernel cannot evaluate, unbounded results,
//! openings whose host is not imported) is skipped with a warning in [`Imported::warnings`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use bim_doc::{EntityId, Value};
use bim_geom::{boolean, BoolOp, Point3, Solid};
use bim_sketch::geom::{newell, Plane};
use bim_sketch::shape::clean_loop;
use bim_sketch::{Face, Shape};

use crate::guid;
use crate::model::{Param, StepFile};
use crate::schema::Schema;
use crate::typed::Entity;
use crate::writer::format_real;

/// Geometric equality (metres) for placement and profile tests.
const LEN_TOL: f64 = 1e-9;
/// Tolerance on unit-vector components (parallel / perpendicular tests).
const DIR_TOL: f64 = 1e-9;
/// How close a height must be to a storey elevation to sit on that Level (metres).
const LEVEL_TOL: f64 = 1e-6;
/// spec/sketch.md: every Shape face is planar within this distance.
const PLANAR_TOL: f64 = 1e-9;
/// Segments of a tessellated circle profile.
const CIRCLE_SEGMENTS: usize = 32;
/// Nesting limit for placements and mapped representations (cycle guard).
const MAX_DEPTH: usize = 64;

/// One entity the import creates (or updates, when the project already has it).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedEntity {
    /// The name the entity is written with: its id's ULID, or a seed for derived entities.
    pub seed: String,
    pub id: EntityId,
    pub kind: String,
    pub props: Vec<(String, Value)>,
}

/// The result of [`import_ifc`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Imported {
    /// In creation order (Levels first; a WallType before its first Wall).
    pub entities: Vec<ImportedEntity>,
    /// What was skipped or approximated, one line each.
    pub warnings: Vec<String>,
}

/// Why a file cannot be imported at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError(pub String);

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ImportError {}

/// The `FILE_SCHEMA` identifiers the import reads.
const READ_SCHEMAS: [(&str, Schema); 6] = [
    ("IFC4", Schema::Ifc4),
    ("IFC4_ADD1", Schema::Ifc4),
    ("IFC4_ADD2", Schema::Ifc4),
    ("IFC4_ADD2_TC1", Schema::Ifc4),
    ("IFC2X3", Schema::Ifc2x3),
    ("IFC2X3_TC1", Schema::Ifc2x3),
];

/// The schema of a `FILE_SCHEMA` identifier the import reads (exact, case-insensitive).
fn read_schema(id: &str) -> Option<Schema> {
    let id = id.trim();
    READ_SCHEMAS.iter().find(|(s, _)| s.eq_ignore_ascii_case(id)).map(|(_, s)| *s)
}

/// Maps an IFC4 or IFC2X3 file to document entities. A file whose `FILE_SCHEMA` is neither, or
/// whose products' placements or representations refer to undefined instances, is an error;
/// everything else imports what it can (see the module docs).
pub fn import_ifc(file: &StepFile) -> Result<Imported, ImportError> {
    let ids = file.schema_identifiers();
    if ids.is_empty() || !ids.iter().all(|s| read_schema(s) == Some(file.schema())) {
        let names: Vec<&str> = READ_SCHEMAS.iter().map(|(s, _)| *s).collect();
        return Err(ImportError(format!("unsupported FILE_SCHEMA {ids:?}: bim imports IFC4 and IFC2X3 ({})", names.join(", "))));
    }
    check_references(file)?;
    let mut im = Importer::new(file);
    im.run();
    Ok(Imported { entities: im.out, warnings: im.warnings })
}

/// Every instance reachable from a product's placement or representation exists.
fn check_references(file: &StepFile) -> Result<(), ImportError> {
    fn refs(p: &Param, out: &mut Vec<u64>) {
        match p {
            Param::Ref(id) => out.push(*id),
            Param::List(items) => items.iter().for_each(|i| refs(i, out)),
            Param::Typed(_, inner) => refs(inner, out),
            _ => {}
        }
    }
    let schema = file.schema();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut stack: Vec<(u64, u64)> = Vec::new();
    for inst in file.instances() {
        if !schema.is_subtype_of(&inst.entity, "IfcProduct") {
            continue;
        }
        let mut ids = Vec::new();
        for k in [P_PLACEMENT, P_REPRESENTATION] {
            if let Some(p) = inst.params.get(k) {
                refs(p, &mut ids);
            }
        }
        stack.extend(ids.into_iter().map(|r| (r, inst.id)));
        while let Some((id, from)) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(target) = file.get(id) else {
                let holder = file.get(from).map(|i| i.entity.as_str()).unwrap_or("?");
                return Err(ImportError(format!("#{from} ({holder}) refers to #{id}, which the file does not define (product #{} {})", inst.id, inst.entity)));
            };
            let mut ids = Vec::new();
            target.params.iter().for_each(|p| refs(p, &mut ids));
            stack.extend(ids.into_iter().map(|r| (r, id)));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ linear algebra

type V3 = [f64; 3];
type V2 = [f64; 2];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> Option<V3> {
    let n = norm(a);
    (n > 1e-300 && n.is_finite()).then(|| scale(a, 1.0 / n))
}
fn dot2(a: V2, b: V2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn sub2(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}
fn cross2(a: V2, b: V2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn perp(a: V2) -> V2 {
    [-a[1], a[0]]
}

/// An affine map `p ↦ m·p + t` (`m` row-major).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Xf {
    m: [[f64; 3]; 3],
    t: V3,
}

impl Xf {
    const ID: Xf = Xf { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    /// The map whose columns are `x`, `y`, `z` and whose origin is `t`.
    fn from_axes(x: V3, y: V3, z: V3, t: V3) -> Xf {
        Xf { m: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]], t }
    }
    fn dir(&self, d: V3) -> V3 {
        let m = &self.m;
        [
            m[0][0] * d[0] + m[0][1] * d[1] + m[0][2] * d[2],
            m[1][0] * d[0] + m[1][1] * d[1] + m[1][2] * d[2],
            m[2][0] * d[0] + m[2][1] * d[1] + m[2][2] * d[2],
        ]
    }
    fn apply(&self, p: V3) -> V3 {
        add(self.dir(p), self.t)
    }
    /// `self ∘ inner`.
    fn then(&self, inner: &Xf) -> Xf {
        let mut m = [[0.0; 3]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, x) in row.iter_mut().enumerate() {
                *x = (0..3).map(|k| self.m[r][k] * inner.m[k][c]).sum();
            }
        }
        Xf { m, t: self.apply(inner.t) }
    }
    /// The inverse map, `None` when the linear part is singular.
    fn inverse(&self) -> Option<Xf> {
        let m = &self.m;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if !(det.abs() > 1e-300) || !det.is_finite() {
            return None;
        }
        let c = |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
        let inv = [
            [c(1, 2, 1, 2) / det, -c(0, 2, 1, 2) / det, c(0, 1, 1, 2) / det],
            [-c(1, 2, 0, 2) / det, c(0, 2, 0, 2) / det, -c(0, 1, 0, 2) / det],
            [c(1, 2, 0, 1) / det, -c(0, 2, 0, 1) / det, c(0, 1, 0, 1) / det],
        ];
        let lin = Xf { m: inv, t: [0.0; 3] };
        let t = lin.dir(self.t);
        Some(Xf { m: inv, t: [-t[0], -t[1], -t[2]] })
    }
    /// The 16 numbers of the Shape `transform` prop (column-major 4×4).
    fn column_major(&self) -> [f64; 16] {
        let m = &self.m;
        [
            m[0][0], m[1][0], m[2][0], 0.0, m[0][1], m[1][1], m[2][1], 0.0, m[0][2], m[1][2], m[2][2], 0.0, self.t[0], self.t[1],
            self.t[2], 1.0,
        ]
    }
}

// ------------------------------------------------------------------ plan polygons

fn signed_area(p: &[V2]) -> f64 {
    (0..p.len()).map(|i| cross2(p[i], p[(i + 1) % p.len()])).sum::<f64>() / 2.0
}

/// Drops consecutive (and closing) duplicate points.
fn dedup2(p: &mut Vec<V2>, tol: f64) {
    let mut out: Vec<V2> = Vec::with_capacity(p.len());
    for &q in p.iter() {
        if out.last().is_none_or(|l| (l[0] - q[0]).abs() > tol || (l[1] - q[1]).abs() > tol) {
            out.push(q);
        }
    }
    while out.len() > 1 {
        let (f, l) = (out[0], out[out.len() - 1]);
        if (f[0] - l[0]).abs() <= tol && (f[1] - l[1]).abs() <= tol {
            out.pop();
        } else {
            break;
        }
    }
    *p = out;
}

/// Drops vertices on the straight line between their neighbours.
fn drop_collinear(p: &[V2]) -> Vec<V2> {
    let mut pts = p.to_vec();
    loop {
        let n = pts.len();
        if n < 4 {
            return pts;
        }
        let ext = pts.iter().map(|q| q[0].abs().max(q[1].abs())).fold(1.0, f64::max);
        let k = (0..n).find(|&i| {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let (ab, bc) = (sub2(b, a), sub2(c, b));
            let len = (dot2(ab, ab).sqrt()) * (dot2(bc, bc).sqrt());
            cross2(ab, bc).abs() <= DIR_TOL * len.max(ext * LEN_TOL) && dot2(ab, bc) >= 0.0
        });
        match k {
            Some(i) => {
                pts.remove(i);
            }
            None => return pts,
        }
    }
}

fn bbox2(p: &[V2]) -> (V2, V2) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for q in p {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    (lo, hi)
}

/// The rectangle `p` (4 corners after dropping collinear points, right angles): a corner and the
/// two side vectors from it.
fn rectangle(p: &[V2]) -> Option<(V2, V2, V2)> {
    let q = drop_collinear(p);
    if q.len() != 4 {
        return None;
    }
    let (a, b) = (sub2(q[1], q[0]), sub2(q[3], q[0]));
    let (la, lb) = (dot2(a, a).sqrt(), dot2(b, b).sqrt());
    if !(la > LEN_TOL && lb > LEN_TOL) || dot2(a, b).abs() > DIR_TOL * la * lb {
        return None;
    }
    let far = [q[0][0] + a[0] + b[0], q[0][1] + a[1] + b[1]];
    let ext = q.iter().map(|v| v[0].abs().max(v[1].abs())).fold(1.0, f64::max);
    if (far[0] - q[2][0]).abs() > LEN_TOL * ext || (far[1] - q[2][1]).abs() > LEN_TOL * ext {
        return None;
    }
    Some((q[0], a, b))
}

/// Whether `p` lies strictly inside the polygon (even-odd rule).
fn inside(poly: &[V2], p: V2) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                c = !c;
            }
        }
    }
    c
}

/// A point inside a simple polygon: its centroid when that is inside, else the middle of the
/// first interval a horizontal line between two vertex heights cuts out of it.
fn interior_point(poly: &[V2]) -> Option<V2> {
    if poly.len() < 3 {
        return None;
    }
    let a = signed_area(poly);
    if a.abs() > 0.0 {
        let n = poly.len();
        let (mut cx, mut cy) = (0.0, 0.0);
        for i in 0..n {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            let w = cross2(p, q);
            cx += (p[0] + q[0]) * w;
            cy += (p[1] + q[1]) * w;
        }
        let c = [cx / (6.0 * a), cy / (6.0 * a)];
        if inside(poly, c) {
            return Some(c);
        }
    }
    let mut ys: Vec<f64> = poly.iter().map(|p| p[1]).collect();
    ys.sort_by(f64::total_cmp);
    ys.dedup();
    for w in ys.windows(2) {
        let y = (w[0] + w[1]) / 2.0;
        let n = poly.len();
        let mut xs: Vec<f64> = (0..n)
            .filter_map(|i| {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                ((a[1] > y) != (b[1] > y)).then(|| a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]))
            })
            .collect();
        xs.sort_by(f64::total_cmp);
        if xs.len() >= 2 && xs[1] > xs[0] {
            return Some([(xs[0] + xs[1]) / 2.0, y]);
        }
    }
    None
}

fn flat(p: &[V2]) -> Vec<Value> {
    p.iter().flat_map(|q| [Value::Num(clean(q[0])), Value::Num(clean(q[1]))]).collect()
}

/// `-0.0` → `0.0`.
fn clean(x: f64) -> f64 {
    if x == 0.0 {
        0.0
    } else {
        x
    }
}

// ------------------------------------------------------------------ geometry carriers

/// A 2D profile in its own coordinates (metres): outer loop counter-clockwise, holes clockwise.
#[derive(Debug, Clone)]
struct Profile {
    outer: Vec<V2>,
    holes: Vec<Vec<V2>>,
    /// False when curves were tessellated.
    exact: bool,
}

impl Profile {
    fn normalized(mut outer: Vec<V2>, holes: Vec<Vec<V2>>, exact: bool) -> Result<Profile, String> {
        let ext = outer.iter().map(|q| q[0].abs().max(q[1].abs())).fold(1.0, f64::max);
        dedup2(&mut outer, LEN_TOL * ext);
        if outer.len() < 3 || signed_area(&outer).abs() <= LEN_TOL * ext {
            return Err("degenerate profile".into());
        }
        if signed_area(&outer) < 0.0 {
            outer.reverse();
        }
        let mut hs = Vec::new();
        for mut h in holes {
            dedup2(&mut h, LEN_TOL * ext);
            if h.len() < 3 || signed_area(&h).abs() <= LEN_TOL * ext {
                continue;
            }
            if signed_area(&h) > 0.0 {
                h.reverse();
            }
            hs.push(h);
        }
        Ok(Profile { outer, holes: hs, exact })
    }
}

/// An extruded solid in some frame: the profile's loops on its base plane and the extrusion vector.
#[derive(Debug, Clone)]
struct Prism {
    outer: Vec<V3>,
    holes: Vec<Vec<V3>>,
    ext: V3,
    exact: bool,
}

impl Prism {
    fn vertices(&self) -> Vec<V3> {
        let base: Vec<V3> = self.outer.iter().chain(self.holes.iter().flatten()).copied().collect();
        base.iter().copied().chain(base.iter().map(|&p| add(p, self.ext))).collect()
    }

    /// The exact volume: net profile area times the extrusion's component along the profile normal.
    fn volume(&self) -> f64 {
        let n = newell(&self.outer);
        let Some(nu) = unit(n) else { return 0.0 };
        let holes: f64 = self.holes.iter().map(|h| norm(newell(h)) / 2.0).sum();
        (norm(n) / 2.0 - holes) * dot(self.ext, nu).abs()
    }

    /// The faces: base, top and sides, oriented outwards when the extrusion runs to the side the
    /// outer loop's normal points to (else all inwards; the caller orients closed shells).
    fn faces(&self) -> Vec<Vec<Vec<V3>>> {
        let up = |l: &Vec<V3>| -> Vec<V3> { l.iter().map(|&p| add(p, self.ext)).collect() };
        let rev = |l: &Vec<V3>| -> Vec<V3> { l.iter().rev().copied().collect() };
        let mut out = Vec::new();
        let mut bottom = vec![rev(&self.outer)];
        bottom.extend(self.holes.iter().map(rev));
        out.push(bottom);
        let mut top = vec![up(&self.outer)];
        top.extend(self.holes.iter().map(up));
        out.push(top);
        for l in std::iter::once(&self.outer).chain(self.holes.iter()) {
            for i in 0..l.len() {
                let (a, b) = (l[i], l[(i + 1) % l.len()]);
                out.push(vec![vec![a, b, add(b, self.ext), add(a, self.ext)]]);
            }
        }
        out
    }

    /// A vertical prism: its plan outline (counter-clockwise), plan holes and z range; `None` when
    /// the extrusion is not vertical or the base is not horizontal.
    fn vertical(&self) -> Option<(Vec<V2>, Vec<Vec<V2>>, f64, f64)> {
        let e = self.ext;
        let len = norm(e);
        if !(len > LEN_TOL) || (e[0] * e[0] + e[1] * e[1]).sqrt() > DIR_TOL * len {
            return None;
        }
        let z = self.outer[0][2];
        let ext = self.vertices().iter().map(|p| p[0].abs().max(p[1].abs()).max(p[2].abs())).fold(1.0, f64::max);
        if self.outer.iter().chain(self.holes.iter().flatten()).any(|p| (p[2] - z).abs() > LEN_TOL * ext) {
            return None;
        }
        let plan = |l: &Vec<V3>| -> Vec<V2> { l.iter().map(|p| [p[0], p[1]]).collect() };
        let mut outer = plan(&self.outer);
        if signed_area(&outer) < 0.0 {
            outer.reverse();
        }
        let holes = self.holes.iter().map(plan).collect();
        Some((outer, holes, z.min(z + e[2]), z.max(z + e[2])))
    }
}

/// Faces gathered for a Shape: vertices shared by exact coordinates.
#[derive(Default)]
struct FaceSet {
    vertices: Vec<V3>,
    index: HashMap<[u64; 3], usize>,
    faces: Vec<Vec<Vec<usize>>>,
    fanned: usize,
    holes_dropped: usize,
}

impl FaceSet {
    fn vertex(&mut self, p: V3) -> usize {
        let p = [clean(p[0]), clean(p[1]), clean(p[2])];
        let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        let n = self.vertices.len();
        *self.index.entry(key).or_insert_with(|| {
            self.vertices.push(p);
            n
        })
    }

    /// Adds a face (outer loop first, then holes). A degenerate face is dropped; a face that is
    /// not planar is split into triangles fanned from its first vertex (holes dropped).
    fn add(&mut self, loops: Vec<Vec<V3>>) {
        let mut idx: Vec<Vec<usize>> = Vec::new();
        for (k, l) in loops.into_iter().enumerate() {
            let mut lp: Vec<usize> = l.into_iter().map(|p| self.vertex(p)).collect();
            clean_loop(&mut lp);
            if lp.len() < 3 {
                if k == 0 {
                    return;
                }
                continue;
            }
            idx.push(lp);
        }
        if idx.is_empty() {
            return;
        }
        let pts = |lp: &Vec<usize>, vs: &Vec<V3>| -> Vec<V3> { lp.iter().map(|&i| vs[i]).collect() };
        let outer = pts(&idx[0], &self.vertices);
        let Some(plane) = Plane::from_loop(&outer) else { return };
        let planar = idx.iter().flatten().all(|&i| plane.dist(self.vertices[i]).abs() <= PLANAR_TOL);
        if planar {
            self.faces.push(idx);
            return;
        }
        if idx.len() > 1 {
            self.holes_dropped += idx.len() - 1;
        }
        self.fanned += 1;
        let o = &idx[0];
        for k in 1..o.len() - 1 {
            let tri = vec![o[0], o[k], o[k + 1]];
            let p = pts(&tri, &self.vertices);
            if norm(newell(&p)) > 0.0 {
                self.faces.push(vec![tri]);
            }
        }
    }
}

// ------------------------------------------------------------------ solids

/// The closed solid of a face set: oriented outwards; an error when the faces do not close a
/// volume.
fn solid_of_set(set: FaceSet) -> Result<Solid, String> {
    let mut shape = Shape {
        vertices: set.vertices,
        faces: set.faces.into_iter().enumerate().map(|(k, loops)| Face { id: k as u64 + 1, loops }).collect(),
        edges: Vec::new(),
        products: None,
    };
    shape.orient_closed();
    if !shape.is_closed() || !(shape.volume() > 0.0) {
        return Err("not a closed solid".into());
    }
    let points = shape.vertices.iter().map(|p| Point3::new(p[0], p[1], p[2])).collect();
    Ok(Solid::from_faces(points, shape.faces.into_iter().map(|f| f.loops).collect()))
}

/// The closed solid bounded by faces given as point loops (outer loop first).
fn solid_of_faces(faces: Vec<Vec<Vec<V3>>>) -> Result<Solid, String> {
    let mut set = FaceSet::default();
    for f in faces {
        set.add(f);
    }
    solid_of_set(set)
}

/// Adds the faces of a solid (outer loop first, then holes).
fn add_solid(set: &mut FaceSet, s: &Solid) {
    for f in &s.faces {
        let loops = f
            .loops
            .iter()
            .map(|&h| {
                s.loop_vertices(h)
                    .into_iter()
                    .map(|v| {
                        let p = s.vertices[v].point;
                        [p.x, p.y, p.z]
                    })
                    .collect()
            })
            .collect();
        set.add(loops);
    }
}

/// The prism of plan polygon `poly` between heights `z0 < z1`, in the frame `m`.
fn prism_solid(m: &Xf, poly: &[V2], z0: f64, z1: f64) -> Result<Solid, String> {
    let p = Prism { outer: poly.iter().map(|q| m.apply([q[0], q[1], z0])).collect(), holes: Vec::new(), ext: m.dir([0.0, 0.0, z1 - z0]), exact: true };
    solid_of_faces(p.faces())
}

/// The bounds of points mapped by `m`; `None` for no points.
fn bounds_in(m: &Xf, pts: impl Iterator<Item = V3>) -> Option<(V3, V3)> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let mut any = false;
    for p in pts {
        let q = m.apply(p);
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
        any = true;
    }
    any.then_some((lo, hi))
}

fn solid_points(s: &Solid) -> impl Iterator<Item = V3> + '_ {
    s.vertices.iter().map(|v| [v.point.x, v.point.y, v.point.z])
}

// ------------------------------------------------------------------ importer

struct LevelInfo {
    id: EntityId,
    elevation: f64,
}

/// A Wall the import made parametric, for its openings.
#[derive(Clone, Copy)]
struct WallFrame {
    id: EntityId,
    p1: V2,
    u: V2,
    z0: f64,
    len: f64,
    height: f64,
    thickness: f64,
}

/// A Slab the import made parametric, for its openings.
struct SlabFrame {
    id: EntityId,
    z0: f64,
    z1: f64,
}

struct Importer<'a> {
    f: &'a StepFile,
    length: f64,
    area: f64,
    volume: f64,
    placements: HashMap<u64, Xf>,
    container: HashMap<u64, u64>,
    parent: HashMap<u64, u64>,
    voids: HashMap<u64, u64>,
    /// The openings voiding each element, in file order.
    hosted: HashMap<u64, Vec<u64>>,
    fills: HashMap<u64, u64>,
    filled_by: HashMap<u64, u64>,
    psets: HashMap<u64, Vec<u64>>,
    types: HashMap<u64, u64>,
    materials: HashMap<u64, u64>,
    levels: BTreeMap<u64, LevelInfo>,
    walls: HashMap<u64, WallFrame>,
    slabs: HashMap<u64, SlabFrame>,
    /// Fillings an imported Opening stands for.
    represented: HashSet<u64>,
    /// Elements imported as Shapes with their openings subtracted.
    cut: HashSet<u64>,
    emitted: HashSet<EntityId>,
    out: Vec<ImportedEntity>,
    warnings: Vec<String>,
}

/// Attribute positions every `IfcProduct` shares in both schemas.
const P_GLOBAL_ID: usize = 0;
const P_NAME: usize = 2;
const P_PLACEMENT: usize = 5;
const P_REPRESENTATION: usize = 6;

fn param_ref(p: Option<&Param>) -> Option<u64> {
    p.and_then(Param::as_ref_id)
}

fn exact_class(e: &Entity, name: &str) -> bool {
    e.entity().eq_ignore_ascii_case(name)
}

fn length_measure(t: &str) -> bool {
    matches!(t, "IFCLENGTHMEASURE" | "IFCPOSITIVELENGTHMEASURE" | "IFCNONNEGATIVELENGTHMEASURE")
}

fn si_prefix(p: Option<&str>) -> f64 {
    match p {
        Some("EXA") => 1e18,
        Some("PETA") => 1e15,
        Some("TERA") => 1e12,
        Some("GIGA") => 1e9,
        Some("MEGA") => 1e6,
        Some("KILO") => 1e3,
        Some("HECTO") => 1e2,
        Some("DECA") => 1e1,
        Some("DECI") => 1e-1,
        Some("CENTI") => 1e-2,
        Some("MILLI") => 1e-3,
        Some("MICRO") => 1e-6,
        Some("NANO") => 1e-9,
        Some("PICO") => 1e-12,
        Some("FEMTO") => 1e-15,
        Some("ATTO") => 1e-18,
        _ => 1.0,
    }
}

impl<'a> Importer<'a> {
    fn new(f: &'a StepFile) -> Importer<'a> {
        let mut im = Importer {
            f,
            length: 1.0,
            area: 1.0,
            volume: 1.0,
            placements: HashMap::new(),
            container: HashMap::new(),
            parent: HashMap::new(),
            voids: HashMap::new(),
            hosted: HashMap::new(),
            fills: HashMap::new(),
            filled_by: HashMap::new(),
            psets: HashMap::new(),
            types: HashMap::new(),
            materials: HashMap::new(),
            levels: BTreeMap::new(),
            walls: HashMap::new(),
            slabs: HashMap::new(),
            represented: HashSet::new(),
            cut: HashSet::new(),
            emitted: HashSet::new(),
            out: Vec::new(),
            warnings: Vec::new(),
        };
        im.units();
        im.relationships();
        im
    }

    fn warn(&mut self, msg: String) {
        self.warnings.push(msg);
    }

    fn describe(e: &Entity) -> String {
        let name = e.param(P_NAME).and_then(Param::as_str).unwrap_or("");
        format!("#{} {} '{}'", e.id(), e.type_name(), name)
    }

    // ---------------------------------------------------------- units

    /// The SI factor of a unit (`IfcSIUnit` or `IfcConversionBasedUnit`), and its unit type.
    fn unit_factor(&self, e: Entity, depth: usize) -> Option<(String, f64)> {
        if depth > 8 {
            return None;
        }
        let ty = e.get_enum("UnitType")?.to_string();
        if e.is_a("IfcSIUnit") {
            let p = si_prefix(e.get_enum("Prefix"));
            let f = match e.get_enum("Name") {
                Some("SQUARE_METRE") => p * p,
                Some("CUBIC_METRE") => p * p * p,
                _ => p,
            };
            return Some((ty, f));
        }
        if e.is_a("IfcConversionBasedUnit") {
            let m = e.get_entity("ConversionFactor")?;
            let v = m.get("ValueComponent")?.as_f64()?;
            let (_, f) = self.unit_factor(m.get_entity("UnitComponent")?, depth + 1)?;
            let x = v * f;
            return (x.is_finite() && x > 0.0).then_some((ty, x));
        }
        None
    }

    fn units(&mut self) {
        let f = self.f;
        let assignment = f
            .entities_of("IfcProject")
            .find_map(|p| p.get_entity("UnitsInContext"))
            .or_else(|| f.entities_of("IfcUnitAssignment").next());
        let Some(ua) = assignment else { return };
        for u in ua.get_entities("Units") {
            match self.unit_factor(u, 0) {
                Some((t, x)) if t == "LENGTHUNIT" => self.length = x,
                Some((t, x)) if t == "AREAUNIT" => self.area = x,
                Some((t, x)) if t == "VOLUMEUNIT" => self.volume = x,
                _ => {}
            }
        }
    }

    // ---------------------------------------------------------- relationships

    fn relationships(&mut self) {
        let f = self.f;
        for r in f.entities_of("IfcRelContainedInSpatialStructure") {
            let Some(s) = r.get_ref("RelatingStructure") else { continue };
            for e in r.get_refs("RelatedElements") {
                self.container.entry(e).or_insert(s);
            }
        }
        for r in f.entities_of("IfcRelAggregates") {
            let Some(p) = r.get_ref("RelatingObject") else { continue };
            for c in r.get_refs("RelatedObjects") {
                self.parent.entry(c).or_insert(p);
            }
        }
        for r in f.entities_of("IfcRelVoidsElement") {
            if let (Some(h), Some(o)) = (r.get_ref("RelatingBuildingElement"), r.get_ref("RelatedOpeningElement")) {
                if !self.voids.contains_key(&o) {
                    self.voids.insert(o, h);
                    self.hosted.entry(h).or_default().push(o);
                }
            }
        }
        for r in f.entities_of("IfcRelFillsElement") {
            if let (Some(o), Some(e)) = (r.get_ref("RelatingOpeningElement"), r.get_ref("RelatedBuildingElement")) {
                self.fills.entry(o).or_insert(e);
                self.filled_by.entry(e).or_insert(o);
            }
        }
        for r in f.entities_of("IfcRelDefinesByProperties") {
            let defs: Vec<u64> = match r.get("RelatingPropertyDefinition") {
                Some(p) => p.as_ref_id().map(|x| vec![x]).unwrap_or_else(|| p.ref_ids()),
                None => continue,
            };
            for o in r.get_refs("RelatedObjects") {
                self.psets.entry(o).or_default().extend(defs.iter().copied());
            }
        }
        for r in f.entities_of("IfcRelDefinesByType") {
            let Some(t) = r.get_ref("RelatingType") else { continue };
            for o in r.get_refs("RelatedObjects") {
                self.types.entry(o).or_insert(t);
            }
        }
        for r in f.entities_of("IfcRelAssociatesMaterial") {
            let Some(m) = r.get_ref("RelatingMaterial") else { continue };
            for o in r.get_refs("RelatedObjects") {
                self.materials.entry(o).or_insert(m);
            }
        }
    }

    /// The storey an object belongs to: through containment, decomposition, the host it voids or
    /// the opening it fills.
    fn level_of(&self, id: u64) -> Option<&LevelInfo> {
        let mut cur = id;
        for _ in 0..MAX_DEPTH {
            if let Some(l) = self.levels.get(&cur) {
                return Some(l);
            }
            cur = *self
                .container
                .get(&cur)
                .or_else(|| self.parent.get(&cur))
                .or_else(|| self.voids.get(&cur))
                .or_else(|| self.filled_by.get(&cur))?;
        }
        None
    }

    /// The Level at height `z`: `preferred` when it is there, else the first in file order.
    fn level_at(&self, z: f64, preferred: Option<EntityId>) -> Option<EntityId> {
        let at = |l: &&LevelInfo| (l.elevation - z).abs() <= LEVEL_TOL;
        if let Some(p) = preferred {
            if self.levels.values().filter(at).any(|l| l.id == p) {
                return Some(p);
            }
        }
        self.levels.values().find(at).map(|l| l.id)
    }

    // ---------------------------------------------------------- placements

    fn point(&self, e: &Entity) -> V3 {
        let c = e.get_f64s("Coordinates");
        let g = |k: usize| c.get(k).copied().unwrap_or(0.0) * self.length;
        [g(0), g(1), g(2)]
    }

    fn direction(e: Option<Entity>) -> Option<V3> {
        let r = e?.get_f64s("DirectionRatios");
        unit([r.first().copied().unwrap_or(0.0), r.get(1).copied().unwrap_or(0.0), r.get(2).copied().unwrap_or(0.0)])
    }

    /// An `IfcAxis2Placement3D` / `2D` as a map (identity for anything else).
    fn axis_placement(&self, e: Option<Entity>) -> Xf {
        let Some(e) = e else { return Xf::ID };
        let loc = e.get_entity("Location").map(|p| self.point(&p)).unwrap_or([0.0; 3]);
        if e.is_a("IfcAxis2Placement3D") {
            let z = Importer::direction(e.get_entity("Axis")).unwrap_or([0.0, 0.0, 1.0]);
            let r = Importer::direction(e.get_entity("RefDirection")).unwrap_or([1.0, 0.0, 0.0]);
            return frame(z, r, loc);
        }
        if e.is_a("IfcAxis2Placement2D") {
            let r = Importer::direction(e.get_entity("RefDirection")).unwrap_or([1.0, 0.0, 0.0]);
            return frame([0.0, 0.0, 1.0], [r[0], r[1], 0.0], [loc[0], loc[1], 0.0]);
        }
        Xf::ID
    }

    /// The world map of an `IfcObjectPlacement`.
    fn placement(&mut self, id: u64, depth: usize) -> Xf {
        if let Some(x) = self.placements.get(&id) {
            return *x;
        }
        let Some(e) = self.f.entity(id) else { return Xf::ID };
        let x = if depth < MAX_DEPTH && e.is_a("IfcLocalPlacement") {
            let rel = e.get_ref("PlacementRelTo").map(|r| self.placement(r, depth + 1)).unwrap_or(Xf::ID);
            rel.then(&self.axis_placement(e.get_entity("RelativePlacement")))
        } else {
            let msg = format!("placement #{id} ({}) is not supported; taken as the identity", e.type_name());
            self.warn(msg);
            Xf::ID
        };
        self.placements.insert(id, x);
        x
    }

    fn object_xf(&mut self, e: &Entity) -> Xf {
        match param_ref(e.param(P_PLACEMENT)) {
            Some(p) => self.placement(p, 0),
            None => Xf::ID,
        }
    }

    /// An `IfcCartesianTransformationOperator3D` (or its non-uniform subtype).
    fn operator(&self, e: Option<Entity>) -> Xf {
        let Some(e) = e else { return Xf::ID };
        let z = Importer::direction(e.get_entity("Axis3")).unwrap_or([0.0, 0.0, 1.0]);
        let x_in = Importer::direction(e.get_entity("Axis1")).unwrap_or([1.0, 0.0, 0.0]);
        let o = e.get_entity("LocalOrigin").map(|p| self.point(&p)).unwrap_or([0.0; 3]);
        let base = frame(z, x_in, o);
        let s = e.get_f64("Scale").unwrap_or(1.0);
        let s2 = e.get_f64("Scale2").unwrap_or(s);
        let s3 = e.get_f64("Scale3").unwrap_or(s);
        let col = |k: usize, f: f64| [base.m[0][k] * f, base.m[1][k] * f, base.m[2][k] * f];
        Xf::from_axes(col(0, s), col(1, s2), col(2, s3), o)
    }

    // ---------------------------------------------------------- representations

    /// The body items of a product, each with its map into the product's placement frame;
    /// `Ok(None)` when it has no body representation.
    fn body_items(&self, e: &Entity) -> Result<Option<Vec<(Xf, Entity<'a>)>>, String> {
        let Some(pds) = param_ref(e.param(P_REPRESENTATION)).and_then(|r| self.f.entity(r)) else { return Ok(None) };
        let reps = pds.get_entities("Representations");
        let solid = |r: &Entity| {
            matches!(
                r.get_str("RepresentationType").unwrap_or(""),
                "SweptSolid" | "AdvancedSweptSolid" | "Brep" | "AdvancedBrep" | "SolidModel" | "CSG" | "Clipping" | "Tessellation" | "SurfaceModel" | "MappedRepresentation"
            )
        };
        let rep = reps
            .iter()
            .find(|r| r.get_str("RepresentationIdentifier").is_some_and(|s| s.eq_ignore_ascii_case("Body")))
            .or_else(|| reps.iter().find(|r| r.get_str("RepresentationIdentifier").is_none() && solid(r)))
            .or_else(|| reps.iter().find(|r| solid(r)));
        let Some(rep) = rep else { return Ok(None) };
        let mut out = Vec::new();
        self.items(rep, Xf::ID, 0, &mut out)?;
        Ok(Some(out))
    }

    fn items(&self, rep: &Entity<'a>, xf: Xf, depth: usize, out: &mut Vec<(Xf, Entity<'a>)>) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("mapped representations nest too deep".into());
        }
        for it in rep.get_entities("Items") {
            if it.is_a("IfcMappedItem") {
                let src = it.get_entity("MappingSource").ok_or("IfcMappedItem without a MappingSource")?;
                let origin = self.axis_placement(src.get_entity("MappingOrigin"));
                let target = self.operator(it.get_entity("MappingTarget"));
                let inner = src.get_entity("MappedRepresentation").ok_or("IfcRepresentationMap without a representation")?;
                self.items(&inner, xf.then(&target).then(&origin), depth + 1, out)?;
            } else {
                out.push((xf, it));
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------- profiles and curves

    fn curve(&self, e: &Entity) -> Result<(Vec<V2>, bool), String> {
        let l = self.length;
        if e.is_a("IfcPolyline") {
            return Ok((e.get_entities("Points").iter().map(|p| self.point(p)).map(|p| [p[0], p[1]]).collect(), true));
        }
        if e.is_a("IfcIndexedPolyCurve") {
            let list = e.get_entity("Points").ok_or("IfcIndexedPolyCurve without points")?;
            let pts: Vec<V2> = list
                .get_list("CoordList")
                .unwrap_or(&[])
                .iter()
                .map(|c| {
                    let v = c.f64s();
                    [v.first().copied().unwrap_or(0.0) * l, v.get(1).copied().unwrap_or(0.0) * l]
                })
                .collect();
            let at = |i: &Param| -> Result<V2, String> {
                let k = i.as_i64().ok_or("bad index")?;
                pts.get((k - 1).max(0) as usize).copied().filter(|_| k >= 1).ok_or_else(|| format!("index {k} out of range"))
            };
            let Some(segs) = e.get_list("Segments") else { return Ok((pts.clone(), true)) };
            let mut out = Vec::new();
            let mut exact = true;
            for s in segs {
                let (ty, inner) = s.as_typed().ok_or("untyped IfcIndexedPolyCurve segment")?;
                let ix = inner.as_list().ok_or("segment without indices")?;
                match ty {
                    "IFCLINEINDEX" => {
                        for i in ix {
                            out.push(at(i)?);
                        }
                    }
                    "IFCARCINDEX" if ix.len() == 3 => {
                        exact = false;
                        out.extend(arc(at(&ix[0])?, at(&ix[1])?, at(&ix[2])?, 16));
                    }
                    other => return Err(format!("unsupported segment {other}")),
                }
            }
            return Ok((out, exact));
        }
        Err(format!("unsupported profile curve {}", e.type_name()))
    }

    fn profile(&self, e: &Entity) -> Result<Profile, String> {
        let l = self.length;
        if e.get_enum("ProfileType") == Some("CURVE") {
            return Err("a CURVE profile has no area".into());
        }
        let pos = |e: &Entity| -> Xf { self.axis_placement(e.get_entity("Position")) };
        let place = |x: &Xf, p: Vec<V2>| -> Vec<V2> { p.into_iter().map(|q| x.apply([q[0], q[1], 0.0])).map(|q| [q[0], q[1]]).collect() };
        if exact_class(e, "IfcRectangleProfileDef") {
            let (x, y) = (e.get_f64("XDim").ok_or("no XDim")? * l / 2.0, e.get_f64("YDim").ok_or("no YDim")? * l / 2.0);
            let pts = place(&pos(e), vec![[-x, -y], [x, -y], [x, y], [-x, y]]);
            return Profile::normalized(pts, Vec::new(), true);
        }
        if exact_class(e, "IfcCircleProfileDef") {
            let r = e.get_f64("Radius").ok_or("no Radius")? * l;
            let pts = (0..CIRCLE_SEGMENTS)
                .map(|k| {
                    let a = std::f64::consts::TAU * k as f64 / CIRCLE_SEGMENTS as f64;
                    [r * a.cos(), r * a.sin()]
                })
                .collect();
            return Profile::normalized(place(&pos(e), pts), Vec::new(), false);
        }
        if exact_class(e, "IfcIShapeProfileDef") {
            let g = |k: &str| e.get_f64(k).map(|v| v * l).ok_or_else(|| format!("no {k}"));
            let (w, d, tw, tf) = (g("OverallWidth")? / 2.0, g("OverallDepth")? / 2.0, g("WebThickness")? / 2.0, g("FlangeThickness")?);
            let fillet = e.get_f64("FilletRadius").unwrap_or(0.0) != 0.0;
            let pts = vec![
                [-w, -d],
                [w, -d],
                [w, -d + tf],
                [tw, -d + tf],
                [tw, d - tf],
                [w, d - tf],
                [w, d],
                [-w, d],
                [-w, d - tf],
                [-tw, d - tf],
                [-tw, -d + tf],
                [-w, -d + tf],
            ];
            return Profile::normalized(place(&pos(e), pts), Vec::new(), !fillet);
        }
        if e.is_a("IfcArbitraryClosedProfileDef") {
            let (outer, mut exact) = self.curve(&e.get_entity("OuterCurve").ok_or("no OuterCurve")?)?;
            let mut holes = Vec::new();
            for c in e.get_entities("InnerCurves") {
                let (h, ex) = self.curve(&c)?;
                exact &= ex;
                holes.push(h);
            }
            return Profile::normalized(outer, holes, exact);
        }
        Err(format!("unsupported profile {}", e.type_name()))
    }

    /// An `IfcExtrudedAreaSolid` mapped by `xf`.
    fn prism(&self, solid: &Entity, xf: &Xf) -> Result<Prism, String> {
        let prof = self.profile(&solid.get_entity("SweptArea").ok_or("no SweptArea")?)?;
        let s = xf.then(&self.axis_placement(solid.get_entity("Position")));
        let d = Importer::direction(solid.get_entity("ExtrudedDirection")).ok_or("no ExtrudedDirection")?;
        let depth = solid.get_f64("Depth").ok_or("no Depth")? * self.length;
        if !(depth > 0.0) {
            return Err("non-positive extrusion depth".into());
        }
        let lift = |l: &Vec<V2>| -> Vec<V3> { l.iter().map(|p| s.apply([p[0], p[1], 0.0])).collect() };
        Ok(Prism { outer: lift(&prof.outer), holes: prof.holes.iter().map(lift).collect(), ext: s.dir(scale(d, depth)), exact: prof.exact })
    }

    // ---------------------------------------------------------- faces

    fn loop_points(&self, lp: &Entity, xf: &Xf) -> Result<Vec<V3>, String> {
        if lp.is_a("IfcPolyLoop") {
            return Ok(lp.get_entities("Polygon").iter().map(|p| xf.apply(self.point(p))).collect());
        }
        Err(format!("unsupported loop {}", lp.type_name()))
    }

    fn face(&self, face: &Entity, xf: &Xf, set: &mut FaceSet) -> Result<(), String> {
        let mut outer: Option<Vec<V3>> = None;
        let mut others: Vec<Vec<V3>> = Vec::new();
        for b in face.get_entities("Bounds") {
            let mut pts = self.loop_points(&b.get_entity("Bound").ok_or("face bound without a loop")?, xf)?;
            if b.get_bool("Orientation") == Some(false) {
                pts.reverse();
            }
            if b.is_a("IfcFaceOuterBound") && outer.is_none() {
                outer = Some(pts);
            } else {
                others.push(pts);
            }
        }
        let outer = match outer {
            Some(o) => o,
            None => {
                let Some(k) = (0..others.len()).max_by(|&a, &b| norm(newell(&others[a])).total_cmp(&norm(newell(&others[b])))) else {
                    return Ok(());
                };
                others.remove(k)
            }
        };
        let mut loops = vec![outer];
        loops.extend(others);
        set.add(loops);
        Ok(())
    }

    fn faces_of_set(&self, fs: &Entity, xf: &Xf, set: &mut FaceSet) -> Result<(), String> {
        for face in fs.get_entities("CfsFaces") {
            self.face(&face, xf, set)?;
        }
        Ok(())
    }

    /// Points of an `IfcCartesianPointList3D`, mapped.
    fn point_list(&self, e: &Entity, xf: &Xf) -> Result<Vec<V3>, String> {
        let list = e.get_entity("Coordinates").ok_or("face set without coordinates")?;
        let l = self.length;
        Ok(list
            .get_list("CoordList")
            .unwrap_or(&[])
            .iter()
            .map(|c| {
                let v = c.f64s();
                let g = |k: usize| v.get(k).copied().unwrap_or(0.0) * l;
                xf.apply([g(0), g(1), g(2)])
            })
            .collect())
    }

    /// Adds the faces of one representation item.
    fn item_faces(&self, item: &Entity, xf: &Xf, set: &mut FaceSet, approx: &mut bool) -> Result<(), String> {
        if item.is_a("IfcBooleanResult") || item.is_a("IfcCsgSolid") {
            let s = self.closed_solid(item, xf, approx, 0)?;
            add_solid(set, &s);
            return Ok(());
        }
        if item.is_a("IfcBlock") {
            let g = |k: &str| item.get_f64(k).map(|v| v * self.length).filter(|v| *v > 0.0).ok_or_else(|| format!("IfcBlock without a positive {k}"));
            let (x, y, z) = (g("XLength")?, g("YLength")?, g("ZLength")?);
            let m = xf.then(&self.axis_placement(item.get_entity("Position")));
            let p = Prism { outer: [[0.0, 0.0], [x, 0.0], [x, y], [0.0, y]].iter().map(|q| m.apply([q[0], q[1], 0.0])).collect(), holes: Vec::new(), ext: m.dir([0.0, 0.0, z]), exact: true };
            for f in p.faces() {
                set.add(f);
            }
            return Ok(());
        }
        if item.is_a("IfcExtrudedAreaSolid") {
            let p = self.prism(item, xf)?;
            *approx |= !p.exact;
            for f in p.faces() {
                set.add(f);
            }
            return Ok(());
        }
        if item.is_a("IfcFacetedBrep") {
            let outer = item.get_entity("Outer").ok_or("brep without a shell")?;
            if !item.get_refs("Voids").is_empty() {
                return Err("IfcFacetedBrepWithVoids voids are not supported".into());
            }
            return self.faces_of_set(&outer, xf, set);
        }
        if item.is_a("IfcShellBasedSurfaceModel") {
            for s in item.get_entities("SbsmBoundary") {
                self.faces_of_set(&s, xf, set)?;
            }
            return Ok(());
        }
        if item.is_a("IfcFaceBasedSurfaceModel") {
            for s in item.get_entities("FbsmFaces") {
                self.faces_of_set(&s, xf, set)?;
            }
            return Ok(());
        }
        if item.is_a("IfcTriangulatedFaceSet") || item.is_a("IfcPolygonalFaceSet") {
            let pts = self.point_list(item, xf)?;
            let pn: Vec<i64> = item.get_list("PnIndex").map(|l| l.iter().filter_map(Param::as_i64).collect()).unwrap_or_default();
            let at = |k: i64| -> Result<V3, String> {
                let k = if pn.is_empty() { k } else { *pn.get((k - 1).max(0) as usize).filter(|_| k >= 1).ok_or("PnIndex out of range")? };
                pts.get((k - 1).max(0) as usize).copied().filter(|_| k >= 1).ok_or_else(|| format!("coordinate index {k} out of range"))
            };
            let lp = |p: &Param| -> Result<Vec<V3>, String> { p.as_list().unwrap_or(&[]).iter().map(|i| at(i.as_i64().unwrap_or(0))).collect() };
            if item.is_a("IfcTriangulatedFaceSet") {
                for t in item.get_list("CoordIndex").unwrap_or(&[]) {
                    set.add(vec![lp(t)?]);
                }
            } else {
                for face in item.get_entities("Faces") {
                    let mut loops = vec![lp(face.get("CoordIndex").ok_or("face without indices")?)?];
                    if let Some(inner) = face.get_list("InnerCoordIndices") {
                        for h in inner {
                            loops.push(lp(h)?);
                        }
                    }
                    set.add(loops);
                }
            }
            return Ok(());
        }
        Err(format!("unsupported body item {}", item.type_name()))
    }

    /// One representation item (or boolean operand) as a closed solid, mapped by `xf`.
    fn closed_solid(&self, item: &Entity, xf: &Xf, approx: &mut bool, depth: usize) -> Result<Solid, String> {
        if depth > MAX_DEPTH {
            return Err("boolean operands nest too deep".into());
        }
        if item.is_a("IfcBooleanResult") {
            return self.boolean_result(item, xf, approx, depth);
        }
        if item.is_a("IfcCsgSolid") {
            let root = item.get_entity("TreeRootExpression").ok_or("IfcCsgSolid without a tree root")?;
            return self.closed_solid(&root, xf, approx, depth + 1);
        }
        if item.is_a("IfcHalfSpaceSolid") {
            return Err(format!("an unbounded {} is not a solid", item.type_name()));
        }
        let mut set = FaceSet::default();
        self.item_faces(item, xf, &mut set, approx)?;
        solid_of_set(set).map_err(|m| format!("{} #{}: {m}", item.type_name(), item.id()))
    }

    /// An `IfcBooleanResult` (or `IfcBooleanClippingResult`) evaluated with `bim_geom::boolean`.
    fn boolean_result(&self, item: &Entity, xf: &Xf, approx: &mut bool, depth: usize) -> Result<Solid, String> {
        let op = match item.get_enum("Operator") {
            Some("DIFFERENCE") => BoolOp::Difference,
            Some("UNION") => BoolOp::Union,
            Some("INTERSECTION") => BoolOp::Intersection,
            other => return Err(format!("{} #{} with operator {other:?}", item.type_name(), item.id())),
        };
        let first = item.get_entity("FirstOperand").ok_or("boolean result without a first operand")?;
        let second = item.get_entity("SecondOperand").ok_or("boolean result without a second operand")?;
        let a = self.closed_solid(&first, xf, approx, depth + 1)?;
        let b = if second.is_a("IfcHalfSpaceSolid") {
            if op == BoolOp::Union {
                return Err(format!("{} #{}: the union with a half space is unbounded", item.type_name(), item.id()));
            }
            self.half_space(&second, xf, &a, approx)?
        } else {
            self.closed_solid(&second, xf, approx, depth + 1)?
        };
        boolean(&a, &b, op).map_err(|e| format!("{} #{} could not be evaluated: {e}", item.type_name(), item.id()))
    }

    /// The part of a half space (`IfcHalfSpaceSolid` on an `IfcPlane`, or an
    /// `IfcPolygonalBoundedHalfSpace`) that matters to `other`: a box in the plane's frame on the
    /// material side, reaching well past `other` in every direction (and, for a polygonal bounded
    /// one, intersected with the prism of its boundary).
    fn half_space(&self, h: &Entity, xf: &Xf, other: &Solid, approx: &mut bool) -> Result<Solid, String> {
        let what = format!("{} #{}", h.type_name(), h.id());
        let plane = h.get_entity("BaseSurface").filter(|p| p.is_a("IfcPlane")).ok_or_else(|| format!("{what}: its base surface is not an IfcPlane"))?;
        let m = xf.then(&self.axis_placement(plane.get_entity("Position")));
        let inv = m.inverse().ok_or_else(|| format!("{what}: degenerate plane placement"))?;
        let Some((lo, hi)) = bounds_in(&inv, solid_points(other)) else { return Ok(Solid::new()) };
        let margin = |lo: V3, hi: V3| (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max) / 2.0 + 1.0;
        let mg = margin(lo, hi);
        // AgreementFlag true: the plane normal points away from the material (z ≤ 0).
        let (z0, z1) = if h.get_bool("AgreementFlag") != Some(false) { (lo[2].min(0.0) - mg, 0.0) } else { (0.0, hi[2].max(0.0) + mg) };
        let rect = [[lo[0] - mg, lo[1] - mg], [hi[0] + mg, lo[1] - mg], [hi[0] + mg, hi[1] + mg], [lo[0] - mg, hi[1] + mg]];
        let half = prism_solid(&m, &rect, z0, z1).map_err(|e| format!("{what}: {e}"))?;
        if !h.is_a("IfcPolygonalBoundedHalfSpace") {
            return Ok(half);
        }
        let q = xf.then(&self.axis_placement(h.get_entity("Position")));
        let qi = q.inverse().ok_or_else(|| format!("{what}: degenerate placement"))?;
        let boundary = h.get_entity("PolygonalBoundary").ok_or_else(|| format!("{what}: no boundary"))?;
        let (mut poly, exact) = self.curve(&boundary).map_err(|e| format!("{what}: {e}"))?;
        *approx |= !exact;
        let ext = poly.iter().map(|p| p[0].abs().max(p[1].abs())).fold(1.0, f64::max);
        dedup2(&mut poly, LEN_TOL * ext);
        if poly.len() < 3 || signed_area(&poly).abs() <= LEN_TOL * ext {
            return Err(format!("{what}: degenerate boundary"));
        }
        if signed_area(&poly) < 0.0 {
            poly.reverse();
        }
        let (lo, hi) = bounds_in(&qi, solid_points(&half).chain(solid_points(other))).unwrap_or(([0.0; 3], [0.0; 3]));
        let mg = margin(lo, hi);
        let prism = prism_solid(&q, &poly, lo[2] - mg, hi[2] + mg).map_err(|e| format!("{what}: {e}"))?;
        boolean(&half, &prism, BoolOp::Intersection).map_err(|e| format!("{what} could not be evaluated: {e}"))
    }

    // ---------------------------------------------------------- property sets

    fn prop_value(&self, p: &Param) -> Option<Value> {
        let num = |x: f64| x.is_finite().then_some(Value::Num(x));
        match p {
            Param::Typed(t, inner) => match t.as_str() {
                "IFCBOOLEAN" | "IFCLOGICAL" => inner.as_bool().map(Value::Bool),
                t if length_measure(t) => num(inner.as_f64()? * self.length),
                "IFCAREAMEASURE" => num(inner.as_f64()? * self.area),
                "IFCVOLUMEMEASURE" => num(inner.as_f64()? * self.volume),
                _ => match &**inner {
                    Param::String(s) => Some(Value::Text(s.clone())),
                    Param::Real(x) => num(*x),
                    Param::Integer(i) => num(*i as f64),
                    Param::Enum(e) => Some(Value::Text(e.clone())),
                    other => self.prop_value(other),
                },
            },
            Param::String(s) => Some(Value::Text(s.clone())),
            Param::Real(x) => num(*x),
            Param::Integer(i) => num(*i as f64),
            Param::Enum(e) => match e.as_str() {
                "T" => Some(Value::Bool(true)),
                "F" => Some(Value::Bool(false)),
                "U" => None,
                _ => Some(Value::Text(e.clone())),
            },
            Param::List(items) => Some(Value::List(items.iter().filter_map(|i| self.prop_value(i)).collect())),
            _ => None,
        }
    }

    fn property_set(&self, def: &Entity, out: &mut BTreeMap<String, Value>) {
        let Some(set) = def.get_str("Name").filter(|s| !s.is_empty()) else { return };
        if def.is_a("IfcPropertySet") {
            for p in def.get_entities("HasProperties") {
                let Some(name) = p.get_str("Name").filter(|s| !s.is_empty()) else { continue };
                let v = if p.is_a("IfcPropertySingleValue") {
                    p.get("NominalValue").and_then(|v| self.prop_value(v))
                } else if p.is_a("IfcPropertyEnumeratedValue") {
                    p.get("EnumerationValues").and_then(|v| self.prop_value(v)).map(|v| match v {
                        Value::List(mut l) if l.len() == 1 => l.remove(0),
                        v => v,
                    })
                } else if p.is_a("IfcPropertyListValue") {
                    p.get("ListValues").and_then(|v| self.prop_value(v))
                } else {
                    None
                };
                if let Some(v) = v {
                    out.insert(format!("{set}.{name}"), v);
                }
            }
        } else if def.is_a("IfcElementQuantity") {
            for q in def.get_entities("Quantities") {
                let Some(name) = q.get_str("Name").filter(|s| !s.is_empty()) else { continue };
                let v = if q.is_a("IfcQuantityLength") {
                    q.get_f64("LengthValue").map(|x| x * self.length)
                } else if q.is_a("IfcQuantityArea") {
                    q.get_f64("AreaValue").map(|x| x * self.area)
                } else if q.is_a("IfcQuantityVolume") {
                    q.get_f64("VolumeValue").map(|x| x * self.volume)
                } else if q.is_a("IfcQuantityCount") {
                    q.get_f64("CountValue")
                } else if q.is_a("IfcQuantityWeight") {
                    q.get_f64("WeightValue")
                } else {
                    None
                };
                if let Some(v) = v.filter(|x| x.is_finite()) {
                    out.insert(format!("{set}.{name}"), Value::Num(v));
                }
            }
        }
    }

    /// Property sets and quantities of an object: its type's first, then its own.
    fn props_of(&self, obj: u64, out: &mut BTreeMap<String, Value>) {
        if let Some(t) = self.types.get(&obj).and_then(|t| self.f.entity(*t)) {
            for d in t.get_entities("HasPropertySets") {
                self.property_set(&d, out);
            }
            for d in self.psets.get(&t.id()).into_iter().flatten().filter_map(|d| self.f.entity(*d)) {
                self.property_set(&d, out);
            }
        }
        for d in self.psets.get(&obj).into_iter().flatten().filter_map(|d| self.f.entity(*d)) {
            self.property_set(&d, out);
        }
    }

    // ---------------------------------------------------------- output

    /// The id of a product, from its GlobalId; `None` (with a warning) when it has none or it is
    /// a duplicate.
    fn product_id(&mut self, e: &Entity) -> Option<EntityId> {
        let g = e.param(P_GLOBAL_ID).and_then(Param::as_str).unwrap_or("");
        let Some(v) = guid::expand(g) else {
            let msg = format!("{}: invalid GlobalId {g:?}; skipped", Importer::describe(e));
            self.warn(msg);
            return None;
        };
        let id = EntityId(v);
        if self.emitted.contains(&id) {
            let msg = format!("{}: duplicate GlobalId {g}; skipped", Importer::describe(e));
            self.warn(msg);
            return None;
        }
        Some(id)
    }

    fn emit(&mut self, seed: String, id: EntityId, kind: &str, props: Vec<(String, Value)>) {
        if !self.emitted.insert(id) {
            return;
        }
        self.out.push(ImportedEntity { seed, id, kind: kind.to_string(), props });
    }

    /// Emits an entity made from IFC object `e` with `props`, then its name and property sets.
    fn emit_object(&mut self, e: &Entity, id: EntityId, kind: &str, mut props: Vec<(String, Value)>, extra_psets: &[u64]) {
        if let Some(n) = e.param(P_NAME).and_then(Param::as_str).filter(|s| !s.is_empty()) {
            if !props.iter().any(|(k, _)| k == "name") {
                props.insert(0, ("name".into(), Value::Text(n.to_string())));
            }
        }
        let mut ps = BTreeMap::new();
        for x in extra_psets {
            self.props_of(*x, &mut ps);
        }
        self.props_of(e.id(), &mut ps);
        props.extend(ps);
        self.emit(id.to_ulid_string(), id, kind, props);
    }

    // ---------------------------------------------------------- the passes

    fn run(&mut self) {
        let f = self.f;
        // Levels.
        for st in f.entities_of("IfcBuildingStorey") {
            let Some(id) = self.product_id(&st) else { continue };
            let elevation = match param_ref(st.param(P_PLACEMENT)) {
                Some(p) => self.placement(p, 0).t[2],
                None => st.get_f64("Elevation").unwrap_or(0.0) * self.length,
            };
            self.levels.insert(st.id(), LevelInfo { id, elevation });
            self.emit_object(&st, id, "Level", vec![("elevation".into(), Value::Num(clean(elevation)))], &[]);
        }

        // Elements other than openings and the fillings of openings.
        let candidates: Vec<Entity> = f
            .instances()
            .filter_map(|i| f.entity(i.id))
            .filter(|e| {
                if e.def.is_some() {
                    e.is_a("IfcElement") && !e.is_a("IfcFeatureElement")
                } else {
                    // Classes outside the tables: products by their relationships.
                    (self.container.contains_key(&e.id()) || self.parent.contains_key(&e.id()))
                        && e.param(P_GLOBAL_ID).and_then(Param::as_str).is_some()
                }
            })
            .collect();
        for e in &candidates {
            if !self.filled_by.contains_key(&e.id()) {
                self.element(e);
            }
        }
        // Openings.
        for o in f.entities_of("IfcOpeningElement") {
            self.opening(&o);
        }
        // Fillings no Opening stands for.
        for e in &candidates {
            if self.filled_by.contains_key(&e.id()) && !self.represented.contains(&e.id()) {
                self.element(e);
            }
        }
        for e in f.entities_of("IfcFeatureElement").filter(|e| !e.is_a("IfcOpeningElement")) {
            let msg = format!("{}: feature elements other than openings are not imported", Importer::describe(&e));
            self.warn(msg);
        }
        // Spaces.
        for sp in f.entities_of("IfcSpace") {
            self.space(&sp);
        }
    }

    /// A single exact extrusion in world coordinates, if that is the whole body.
    fn single_prism(&self, items: &[(Xf, Entity)], world: &Xf) -> Option<Prism> {
        let [(xf, item)] = items else { return None };
        if !exact_class(item, "IfcExtrudedAreaSolid") {
            return None;
        }
        let p = self.prism(item, &world.then(xf)).ok()?;
        p.exact.then_some(p)
    }

    fn element(&mut self, e: &Entity<'a>) {
        let Some(id) = self.product_id(e) else { return };
        let items = match self.body_items(e) {
            Ok(Some(items)) if !items.is_empty() => items,
            Ok(_) => {
                let msg = format!("{}: no body geometry; skipped", Importer::describe(e));
                self.warn(msg);
                return;
            }
            Err(m) => {
                let msg = format!("{}: {m}; skipped", Importer::describe(e));
                self.warn(msg);
                return;
            }
        };
        let world = self.object_xf(e);
        let level = self.level_of(e.id()).map(|l| (l.id, l.elevation));
        // Columns and Beams have no openings: one with voids is a Shape with the voids cut.
        let voided = self.hosted.get(&e.id()).is_some_and(|v| !v.is_empty());
        if let (Some(p), Some(level)) = (self.single_prism(&items, &world), level) {
            let done = if e.is_a("IfcWall") {
                self.wall(e, id, &p, &world, level)
            } else if e.is_a("IfcSlab") {
                self.slab(e, id, &p, level)
            } else if e.is_a("IfcColumn") && !voided {
                self.column(e, id, &p, level)
            } else if e.is_a("IfcBeam") && !voided {
                self.beam(e, id, &p, level)
            } else {
                false
            };
            if done {
                return;
            }
        }
        self.shape(e, id, &items, &world, level.map(|l| l.0));
    }

    /// The bodies of the openings voiding `e`, as closed solids in `e`'s placement frame
    /// (`world`); what cannot be made a solid is reported and left out.
    fn void_solids(&mut self, e: &Entity, world: &Xf, approx: &mut bool) -> Vec<Solid> {
        let openings = self.hosted.get(&e.id()).cloned().unwrap_or_default();
        let mut out = Vec::new();
        if openings.is_empty() {
            return out;
        }
        let Some(inv) = world.inverse() else {
            let msg = format!("{}: singular placement; its openings are not cut", Importer::describe(e));
            self.warn(msg);
            return out;
        };
        for o in openings {
            let Some(oe) = self.f.entity(o) else { continue };
            let rel = inv.then(&self.object_xf(&oe));
            let items = match self.body_items(&oe) {
                Ok(Some(items)) if !items.is_empty() => items,
                Ok(_) => {
                    let msg = format!("{}: no body geometry; not cut into its host", Importer::describe(&oe));
                    self.warn(msg);
                    continue;
                }
                Err(m) => {
                    let msg = format!("{}: {m}; not cut into its host", Importer::describe(&oe));
                    self.warn(msg);
                    continue;
                }
            };
            for (ixf, it) in items {
                match self.closed_solid(&it, &rel.then(&ixf), approx, 0) {
                    Ok(s) => out.push(s),
                    Err(m) => {
                        let msg = format!("{}: {m}; not cut into its host", Importer::describe(&oe));
                        self.warn(msg);
                    }
                }
            }
        }
        out
    }

    fn shape(&mut self, e: &Entity, id: EntityId, items: &[(Xf, Entity)], world: &Xf, level: Option<EntityId>) {
        let mut set = FaceSet::default();
        let mut approx = false;
        let voids = self.void_solids(e, world, &mut approx);
        for (xf, item) in items {
            // Faces of a solid the kernel made are oriented already (an inner shell, a cavity,
            // faces inwards); those of any other item are oriented here, per item.
            let mut part = FaceSet::default();
            let mut solid = item.is_a("IfcBooleanResult") || item.is_a("IfcCsgSolid");
            if !voids.is_empty() {
                // (A ∪ B) − V = (A − V) ∪ (B − V): the voids are cut from each item.
                match self.closed_solid(item, xf, &mut approx, 0) {
                    Ok(mut s) => {
                        for v in &voids {
                            match boolean(&s, v, BoolOp::Difference) {
                                Ok(r) => s = r,
                                Err(err) => {
                                    let msg = format!("{}: an opening could not be cut ({err})", Importer::describe(e));
                                    self.warn(msg);
                                }
                            }
                        }
                        add_solid(&mut part, &s);
                        solid = true;
                    }
                    Err(m) => {
                        let msg = format!("{}: {m}; its openings are not cut into it", Importer::describe(e));
                        self.warn(msg);
                    }
                }
            }
            if part.faces.is_empty() {
                if let Err(m) = self.item_faces(item, xf, &mut part, &mut approx) {
                    let msg = format!("{}: {m}; skipped", Importer::describe(e));
                    self.warn(msg);
                    return;
                }
            }
            set.holes_dropped += part.holes_dropped;
            let mut piece = Shape {
                vertices: part.vertices,
                faces: part.faces.into_iter().enumerate().map(|(k, loops)| Face { id: k as u64 + 1, loops }).collect(),
                edges: Vec::new(),
                products: None,
            };
            if !solid {
                piece.orient_closed();
            }
            for f in &piece.faces {
                set.add(f.loops.iter().map(|lp| piece.loop_points(lp)).collect());
            }
        }
        if set.faces.is_empty() {
            let msg = format!("{}: body has no faces; skipped", Importer::describe(e));
            self.warn(msg);
            return;
        }
        if approx {
            let msg = format!("{}: curved profile tessellated", Importer::describe(e));
            self.warn(msg);
        }
        if set.holes_dropped > 0 {
            let msg = format!("{}: {} hole(s) of non-planar faces dropped", Importer::describe(e), set.holes_dropped);
            self.warn(msg);
        }
        let shape = Shape {
            vertices: set.vertices,
            faces: set.faces.into_iter().enumerate().map(|(k, loops)| Face { id: k as u64 + 1, loops }).collect(),
            edges: Vec::new(),
            products: None,
        };
        let mut props: Vec<(String, Value)> = shape.props();
        props.push(("transform".into(), Value::List(world.column_major().iter().map(|&x| Value::Num(clean(x))).collect())));
        if let Some(l) = level {
            props.push(("level".into(), Value::Ref(l)));
        }
        props.push(("classification".into(), Value::Text(e.type_name().to_string())));
        if !voids.is_empty() {
            self.cut.insert(e.id());
        }
        self.emit_object(e, id, "Shape", props, &[]);
    }

    /// `u` for a wall rectangle: the side parallel to the placement's x axis, else the longer one.
    fn wall_axis(a: V2, b: V2, world: &Xf) -> V2 {
        let x = world.dir([1.0, 0.0, 0.0]);
        let xp = [x[0], x[1]];
        let lx = dot2(xp, xp).sqrt();
        let units = [a, b].map(|s| {
            let l = dot2(s, s).sqrt();
            [s[0] / l, s[1] / l]
        });
        if lx > 1.0 - DIR_TOL {
            let xp = [xp[0] / lx, xp[1] / lx];
            for u in units {
                if cross2(u, xp).abs() <= DIR_TOL {
                    return if dot2(u, xp) < 0.0 { [-u[0], -u[1]] } else { u };
                }
            }
        }
        let u = if dot2(a, a) >= dot2(b, b) { units[0] } else { units[1] };
        if lx > 0.0 && dot2(u, xp) < 0.0 {
            [-u[0], -u[1]]
        } else {
            u
        }
    }

    fn wall(&mut self, e: &Entity, id: EntityId, p: &Prism, world: &Xf, level: (EntityId, f64)) -> bool {
        let Some((plan, holes, z0, z1)) = p.vertical() else { return false };
        if !holes.is_empty() {
            return false;
        }
        let Some((_, a, b)) = rectangle(&plan) else { return false };
        let u = Importer::wall_axis(a, b, world);
        let v = perp(u);
        let s: Vec<f64> = plan.iter().map(|q| dot2(*q, u)).collect();
        let w: Vec<f64> = plan.iter().map(|q| dot2(*q, v)).collect();
        let (s0, s1) = (s.iter().copied().fold(f64::INFINITY, f64::min), s.iter().copied().fold(f64::NEG_INFINITY, f64::max));
        let (w0, w1) = (w.iter().copied().fold(f64::INFINITY, f64::min), w.iter().copied().fold(f64::NEG_INFINITY, f64::max));
        let (len, t, h) = (s1 - s0, w1 - w0, z1 - z0);
        if !(len > LEN_TOL && t > LEN_TOL && h > LEN_TOL) {
            return false;
        }
        let wc = (w0 + w1) / 2.0;
        let p1 = [s0 * u[0] + wc * v[0], s0 * u[1] + wc * v[1]];
        let p2 = [s1 * u[0] + wc * v[0], s1 * u[1] + wc * v[1]];
        let frame = WallFrame { id, p1, u, z0, len, height: h, thickness: t };
        // Every opening must be an Opening of the parametric wall, else the wall is a Shape with
        // its voids cut (so its volume is kept).
        for o in self.hosted.get(&e.id()).cloned().unwrap_or_default() {
            let Some(oe) = self.f.entity(o) else { continue };
            if let Err(m) = self.wall_void(&frame, &oe) {
                let msg = format!("{}: {m}; its host {} is imported as a Shape with its voids cut", Importer::describe(&oe), Importer::describe(e));
                self.warn(msg);
                return false;
            }
        }
        let (layers, set_name) = self.wall_layers(e, world, v, t);
        let wt = self.wall_type(&layers, set_name);
        let props = vec![
            ("level".into(), Value::Ref(level.0)),
            ("x1".into(), Value::Num(clean(p1[0]))),
            ("y1".into(), Value::Num(clean(p1[1]))),
            ("x2".into(), Value::Num(clean(p2[0]))),
            ("y2".into(), Value::Num(clean(p2[1]))),
            ("height".into(), Value::Num(h)),
            ("type".into(), Value::Ref(wt)),
            ("base_offset".into(), Value::Num(clean(z0 - level.1))),
        ];
        self.walls.insert(e.id(), frame);
        self.emit_object(e, id, "Wall", props, &[]);
        true
    }

    /// The material layers of a wall (left first) and the layer set's name; a single layer of the
    /// geometric thickness when the wall has none or they do not add up to it.
    fn wall_layers(&mut self, e: &Entity, world: &Xf, left: V2, t: f64) -> (Vec<(f64, String)>, String) {
        let f = self.f;
        let mat = self.materials.get(&e.id()).or_else(|| self.types.get(&e.id()).and_then(|ty| self.materials.get(ty))).and_then(|m| f.entity(*m));
        let (set, usage) = match mat {
            Some(m) if m.is_a("IfcMaterialLayerSetUsage") => (m.get_entity("ForLayerSet"), Some(m)),
            Some(m) if m.is_a("IfcMaterialLayerSet") => (Some(m), None),
            _ => (None, None),
        };
        let mut layers: Vec<(f64, String)> = set
            .map(|s| {
                s.get_entities("MaterialLayers")
                    .iter()
                    .map(|l| {
                        let name = l.get_entity("Material").and_then(|m| m.get_str("Name").map(str::to_string)).unwrap_or_default();
                        (l.get_f64("LayerThickness").unwrap_or(0.0) * self.length, name)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let set_name = set.and_then(|s| s.get_str("LayerSetName").map(str::to_string)).unwrap_or_default();
        let sum: f64 = layers.iter().map(|l| l.0).sum();
        if layers.is_empty() || layers.iter().any(|l| !(l.0 >= 0.0)) || (sum - t).abs() > LEVEL_TOL {
            if !layers.is_empty() {
                let msg = format!("{}: material layers add up to {sum} m, the body is {t} m thick; one layer of the body's thickness used", Importer::describe(e));
                self.warn(msg);
            }
            return (vec![(t, layers.first().map(|l| l.1.clone()).unwrap_or_default())], set_name);
        }
        // Layers stack along the placement's y axis (AXIS2) in DirectionSense; left first.
        if let Some(u) = usage {
            let y = world.dir([0.0, 1.0, 0.0]);
            let along = dot2([y[0], y[1]], left);
            let positive = u.get_enum("DirectionSense") != Some("NEGATIVE");
            let axis2 = u.get_enum("LayerSetDirection").is_none_or(|d| d == "AXIS2");
            if axis2 && ((positive && along > 0.5) || (!positive && along < -0.5)) {
                layers.reverse();
            }
        }
        (layers, set_name)
    }

    /// The WallType with these layers (one per distinct layer list), emitted on first use.
    fn wall_type(&mut self, layers: &[(f64, String)], name: String) -> EntityId {
        let key: Vec<String> = layers.iter().map(|(t, m)| format!("{}:{}", format_real(*t).unwrap_or_default(), m)).collect();
        let seed = format!("ifc-walltype/{}", key.join("|"));
        let id = EntityId::derive(&seed);
        if !self.emitted.contains(&id) {
            let mut props = vec![
                ("layer_thickness".into(), Value::List(layers.iter().map(|l| Value::Num(l.0)).collect())),
                ("layer_material".into(), Value::List(layers.iter().map(|l| Value::Text(l.1.clone())).collect())),
            ];
            if !name.is_empty() {
                props.insert(0, ("name".into(), Value::Text(name)));
            }
            self.emit(seed, id, "WallType", props);
        }
        id
    }

    fn slab(&mut self, e: &Entity, id: EntityId, p: &Prism, level: (EntityId, f64)) -> bool {
        let Some((plan, holes, z0, z1)) = p.vertical() else { return false };
        let Some(lv) = self.level_at(z1, Some(level.0)) else { return false };
        for o in self.hosted.get(&e.id()).cloned().unwrap_or_default() {
            let Some(oe) = self.f.entity(o) else { continue };
            if let Err(m) = self.slab_void(z0, z1, &oe) {
                let msg = format!("{}: {m}; its host {} is imported as a Shape with its voids cut", Importer::describe(&oe), Importer::describe(e));
                self.warn(msg);
                return false;
            }
        }
        let ulid = id.to_ulid_string();
        let props = vec![
            ("level".into(), Value::Ref(lv)),
            ("boundary".into(), Value::List(flat(&plan))),
            ("thickness".into(), Value::Num(z1 - z0)),
        ];
        self.slabs.insert(e.id(), SlabFrame { id, z0, z1 });
        self.emit_object(e, id, "Slab", props, &[]);
        for (k, mut h) in holes.into_iter().enumerate() {
            if signed_area(&h) < 0.0 {
                h.reverse();
            }
            let seed = format!("{ulid}/void-{}", k + 1);
            let hid = EntityId::derive(&seed);
            self.emit(seed, hid, "SlabOpening", vec![("host".into(), Value::Ref(id)), ("boundary".into(), Value::List(flat(&h)))]);
        }
        true
    }

    fn column(&mut self, e: &Entity, id: EntityId, p: &Prism, level: (EntityId, f64)) -> bool {
        let Some((plan, holes, z0, z1)) = p.vertical() else { return false };
        if !holes.is_empty() || drop_collinear(&plan).len() != 4 {
            return false;
        }
        // An axis-aligned rectangle fills its bounding box.
        let (lo, hi) = bbox2(&plan);
        let (bx, hy) = (hi[0] - lo[0], hi[1] - lo[1]);
        if (signed_area(&plan) - bx * hy).abs() > LEN_TOL * (bx * hy).max(1.0) || !(bx > LEN_TOL && hy > LEN_TOL) {
            return false;
        }
        let (Some(base), Some(top)) = (self.level_at(z0, Some(level.0)), self.level_at(z1, None)) else { return false };
        let props = vec![
            ("x".into(), Value::Num(clean((lo[0] + hi[0]) / 2.0))),
            ("y".into(), Value::Num(clean((lo[1] + hi[1]) / 2.0))),
            ("base".into(), Value::Ref(base)),
            ("top".into(), Value::Ref(top)),
            ("section_b".into(), Value::Num(bx)),
            ("section_h".into(), Value::Num(hy)),
        ];
        self.emit_object(e, id, "Column", props, &[]);
        true
    }

    fn beam(&mut self, e: &Entity, id: EntityId, p: &Prism, level: (EntityId, f64)) -> bool {
        let (a, b, width, depth, top) = if let Some((plan, holes, z0, z1)) = p.vertical() {
            // A beam drawn as a plan rectangle extruded up: its axis is the long side.
            if !holes.is_empty() {
                return false;
            }
            let Some((c, sa, sb)) = rectangle(&plan) else { return false };
            let (long, short) = if dot2(sa, sa) >= dot2(sb, sb) { (sa, sb) } else { (sb, sa) };
            let m = [c[0] + short[0] / 2.0, c[1] + short[1] / 2.0];
            (m, [m[0] + long[0], m[1] + long[1]], dot2(short, short).sqrt(), z1 - z0, z1)
        } else {
            let ext = p.ext;
            let len = norm(ext);
            if !p.holes.is_empty() || !(len > LEN_TOL) || ext[2].abs() > DIR_TOL * len {
                return false;
            }
            let eu = scale(ext, 1.0 / len);
            let o = p.outer[0];
            let span = p.outer.iter().map(|q| norm(sub(*q, o))).fold(1.0, f64::max);
            // The profile must be perpendicular to the axis (not an oblique extrusion).
            if p.outer.iter().any(|q| dot(sub(*q, o), eu).abs() > LEN_TOL * span) {
                return false;
            }
            let hdir = [-eu[1], eu[0], 0.0];
            let hdir = scale(hdir, 1.0 / norm(hdir));
            let sec: Vec<V2> = p.outer.iter().map(|q| [dot(sub(*q, o), hdir), q[2]]).collect();
            if drop_collinear(&sec).len() != 4 {
                return false;
            }
            let (lo, hi) = bbox2(&sec);
            let (w, d) = (hi[0] - lo[0], hi[1] - lo[1]);
            if (signed_area(&sec).abs() - w * d).abs() > LEN_TOL * (w * d).max(1.0) || !(w > LEN_TOL && d > LEN_TOL) {
                return false;
            }
            let c = (lo[0] + hi[0]) / 2.0;
            let start = [o[0] + c * hdir[0], o[1] + c * hdir[1]];
            (start, [start[0] + ext[0], start[1] + ext[1]], w, d, hi[1])
        };
        if !(width > LEN_TOL && depth > LEN_TOL) || dot2(sub2(b, a), sub2(b, a)).sqrt() <= LEN_TOL {
            return false;
        }
        let props = vec![
            ("level".into(), Value::Ref(level.0)),
            ("x1".into(), Value::Num(clean(a[0]))),
            ("y1".into(), Value::Num(clean(a[1]))),
            ("x2".into(), Value::Num(clean(b[0]))),
            ("y2".into(), Value::Num(clean(b[1]))),
            ("section_b".into(), Value::Num(width)),
            ("section_h".into(), Value::Num(depth)),
            ("z_offset".into(), Value::Num(clean(level.1 - top))),
        ];
        self.emit_object(e, id, "Beam", props, &[]);
        true
    }

    /// The single exact extrusion of an opening's body, in world coordinates.
    fn opening_prism(&mut self, o: &Entity) -> Result<Prism, String> {
        let world = self.object_xf(o);
        match self.body_items(o) {
            Ok(Some(items)) => self.single_prism(&items, &world).ok_or_else(|| "body is not a single extrusion".to_string()),
            Ok(None) => Err("no body geometry".into()),
            Err(m) => Err(m),
        }
    }

    /// An opening of a parametric wall: `[offset, sill, width, height]` of its box (clipped to the
    /// wall), when its body is a box through the wall's full thickness.
    fn wall_void(&mut self, w: &WallFrame, o: &Entity) -> Result<[f64; 4], String> {
        let p = self.opening_prism(o)?;
        let v = perp(w.u);
        let local: Vec<V3> = p
            .vertices()
            .iter()
            .map(|q| {
                let d = [q[0] - w.p1[0], q[1] - w.p1[1]];
                [dot2(d, w.u), dot2(d, v), q[2] - w.z0]
            })
            .collect();
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for q in &local {
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        let bbox = (hi[0] - lo[0]) * (hi[1] - lo[1]) * (hi[2] - lo[2]);
        let is_box = (p.volume() - bbox).abs() <= 1e-9 * bbox.max(1e-9);
        let half = w.thickness / 2.0;
        let through = lo[1] <= -half + LEN_TOL && hi[1] >= half - LEN_TOL;
        let (s0, s1, z0, z1) = (lo[0].max(0.0), hi[0].min(w.len), lo[2].max(0.0), hi[2].min(w.height));
        if !is_box || !through || !(s1 - s0 > LEN_TOL) || !(z1 - z0 > LEN_TOL) {
            return Err("not a box through its host wall's thickness".into());
        }
        Ok([s0, z0, s1 - s0, z1 - z0])
    }

    /// An opening of a parametric slab spanning `z0..z1`: its plan outline, when its body is a
    /// vertical prism through the slab.
    fn slab_void(&mut self, z0: f64, z1: f64, o: &Entity) -> Result<Vec<V2>, String> {
        match self.opening_prism(o)?.vertical() {
            Some((plan, holes, a, b)) if holes.is_empty() && a <= z0 + LEN_TOL && b >= z1 - LEN_TOL => Ok(plan),
            _ => Err("not a vertical prism through its host slab".into()),
        }
    }

    fn opening(&mut self, o: &Entity<'a>) {
        let Some(host) = self.voids.get(&o.id()).copied() else {
            let msg = format!("{}: voids no element; skipped", Importer::describe(o));
            self.warn(msg);
            return;
        };
        if self.cut.contains(&host) {
            let msg = format!("{}: cut into its host's Shape (host #{host}); not imported on its own", Importer::describe(o));
            self.warn(msg);
            return;
        }
        if !self.walls.contains_key(&host) && !self.slabs.contains_key(&host) {
            let msg = format!("{}: its host #{host} was not imported as a Wall or Slab; the void is not cut", Importer::describe(o));
            self.warn(msg);
            return;
        }
        let Some(id) = self.product_id(o) else { return };
        let filling = self.fills.get(&o.id()).and_then(|f| self.f.entity(*f));
        if let Some(&w) = self.walls.get(&host) {
            let [offset, sill, width, height] = match self.wall_void(&w, o) {
                Ok(b) => b,
                Err(m) => {
                    let msg = format!("{}: {m}; the void is not cut", Importer::describe(o));
                    self.warn(msg);
                    return;
                }
            };
            let filled = filling.as_ref().and_then(|fl| {
                if fl.is_a("IfcDoor") {
                    Some("door")
                } else if fl.is_a("IfcWindow") {
                    Some("window")
                } else {
                    None
                }
            });
            let mut extra = Vec::new();
            let kind = match (filled, filling.as_ref()) {
                (Some(k), Some(fl)) => {
                    self.represented.insert(fl.id());
                    extra.push(fl.id());
                    k
                }
                _ => {
                    // spec/schema.md: an Opening is a door or a window; an opening without one
                    // is a door when it starts at the wall's base and a window otherwise.
                    let k = if sill <= LEN_TOL { "door" } else { "window" };
                    let msg = format!("{}: no IfcDoor / IfcWindow filling; imported as a {k} (sill {})", Importer::describe(o), format_real(sill).unwrap_or_default());
                    self.warn(msg);
                    k
                }
            };
            let props = vec![
                ("host".into(), Value::Ref(w.id)),
                ("kind".into(), Value::Text(kind.into())),
                ("offset".into(), Value::Num(clean(offset))),
                ("sill".into(), Value::Num(clean(sill))),
                ("width".into(), Value::Num(width)),
                ("height".into(), Value::Num(height)),
            ];
            self.emit_object(o, id, "Opening", props, &extra);
            return;
        }
        let s = &self.slabs[&host];
        let (sid, sz0, sz1) = (s.id, s.z0, s.z1);
        match self.slab_void(sz0, sz1, o) {
            Ok(plan) => {
                let props = vec![("host".into(), Value::Ref(sid)), ("boundary".into(), Value::List(flat(&plan)))];
                self.emit_object(o, id, "SlabOpening", props, &[]);
            }
            Err(m) => {
                let msg = format!("{}: {m}; the void is not cut", Importer::describe(o));
                self.warn(msg);
            }
        }
    }

    fn space(&mut self, sp: &Entity<'a>) {
        let Some(level) = self.level_of(sp.id()).map(|l| l.id) else {
            let msg = format!("{}: in no storey; skipped", Importer::describe(sp));
            self.warn(msg);
            return;
        };
        let Some(id) = self.product_id(sp) else { return };
        let world = self.object_xf(sp);
        let footprint = match self.body_items(sp) {
            Ok(Some(items)) => match items.as_slice() {
                [(xf, item)] if item.is_a("IfcExtrudedAreaSolid") => self.prism(item, &world.then(xf)).ok().and_then(|p| p.vertical()),
                _ => None,
            },
            _ => None,
        };
        let at = footprint.and_then(|(plan, _, _, _)| interior_point(&plan)).unwrap_or([world.t[0], world.t[1]]);
        let mut props = vec![
            ("level".into(), Value::Ref(level)),
            ("x".into(), Value::Num(clean(at[0]))),
            ("y".into(), Value::Num(clean(at[1]))),
        ];
        let long = sp.get_str("LongName").filter(|s| !s.is_empty());
        let short = sp.param(P_NAME).and_then(Param::as_str).filter(|s| !s.is_empty());
        if let Some(n) = long.or(short) {
            props.insert(0, ("name".into(), Value::Text(n.to_string())));
        }
        self.emit_object(sp, id, "Space", props, &[]);
    }
}

/// The orthonormal frame with z axis `z` and x axis `x` projected perpendicular to it, at `o`.
fn frame(z: V3, x: V3, o: V3) -> Xf {
    let z = unit(z).unwrap_or([0.0, 0.0, 1.0]);
    let x = unit(sub(x, scale(z, dot(x, z))))
        .or_else(|| {
            let alt = if z[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            unit(sub(alt, scale(z, dot(alt, z))))
        })
        .unwrap_or([1.0, 0.0, 0.0]);
    let y = cross(z, x);
    Xf::from_axes(x, y, z, o)
}

/// Points along the circular arc from `a` through `m` to `c` (`a` included, `c` included).
fn arc(a: V2, m: V2, c: V2, segments: usize) -> Vec<V2> {
    let d = 2.0 * (a[0] * (m[1] - c[1]) + m[0] * (c[1] - a[1]) + c[0] * (a[1] - m[1]));
    if d.abs() < 1e-300 {
        return vec![a, m, c];
    }
    let sq = |p: V2| p[0] * p[0] + p[1] * p[1];
    let ux = (sq(a) * (m[1] - c[1]) + sq(m) * (c[1] - a[1]) + sq(c) * (a[1] - m[1])) / d;
    let uy = (sq(a) * (c[0] - m[0]) + sq(m) * (a[0] - c[0]) + sq(c) * (m[0] - a[0])) / d;
    let r = ((a[0] - ux).powi(2) + (a[1] - uy).powi(2)).sqrt();
    let ang = |p: V2| (p[1] - uy).atan2(p[0] - ux);
    let (t0, tm, t1) = (ang(a), ang(m), ang(c));
    let tau = std::f64::consts::TAU;
    let ccw = cross2(sub2(m, a), sub2(c, m)) > 0.0;
    let norm_ang = |t: f64| ((t % tau) + tau) % tau;
    let sweep = if ccw { norm_ang(t1 - t0) } else { -norm_ang(t0 - t1) };
    let _ = tm;
    (0..=segments)
        .map(|k| {
            if k == 0 {
                return a;
            }
            if k == segments {
                return c;
            }
            let t = t0 + sweep * k as f64 / segments as f64;
            [ux + r * t.cos(), uy + r * t.sin()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bim_doc::{Document, Entity as DocEntity, Op};
    use std::collections::BTreeMap as Map;

    /// A STEP file of `schema` around `data`, with `'G<n>'` replaced by a valid GlobalId.
    fn file(schema: &str, data: &str) -> StepFile {
        let mut data = data.to_string();
        for n in (1..=40).rev() {
            data = data.replace(&format!("'G{n}'"), &format!("'{}'", gid(n)));
        }
        let text = format!(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('t','',(''),(''),'','','');\nFILE_SCHEMA(('{schema}'));\nENDSEC;\nDATA;\n{data}\nENDSEC;\nEND-ISO-10303-21;\n"
        );
        StepFile::parse_str(&text).unwrap()
    }

    fn gid(n: u32) -> String {
        guid::compress(0x0123_4567_89ab_cdef_0000_0000_0000_0000 + n as u128)
    }

    fn id(n: u32) -> EntityId {
        EntityId(guid::expand(&gid(n)).unwrap())
    }

    fn get(im: &Imported, n: u32) -> &ImportedEntity {
        im.entities.iter().find(|e| e.id == id(n)).unwrap_or_else(|| panic!("no entity for G{n}: {:?}", im.warnings))
    }

    fn prop<'a>(e: &'a ImportedEntity, k: &str) -> &'a Value {
        e.props.iter().find(|(key, _)| key == k).map(|(_, v)| v).unwrap_or_else(|| panic!("{} has no {k}: {:?}", e.seed, e.props))
    }

    fn num(e: &ImportedEntity, k: &str) -> f64 {
        match prop(e, k) {
            Value::Num(x) => *x,
            v => panic!("{k} = {v:?}"),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn doc_entity(e: &ImportedEntity) -> DocEntity {
        DocEntity { id: e.id, kind: e.kind.clone(), props: e.props.iter().cloned().collect::<Map<_, _>>() }
    }

    fn document(im: &Imported) -> Document {
        let mut doc = Document::new();
        for e in &im.entities {
            doc.apply(Op::Create { id: e.id, kind: e.kind.clone() }).unwrap();
            doc.apply(Op::Set { id: e.id, key: "_name".into(), value: Value::Text(e.seed.clone()) }).unwrap();
            for (k, v) in &e.props {
                doc.apply(Op::Set { id: e.id, key: k.clone(), value: v.clone() }).unwrap();
            }
        }
        doc
    }

    /// Millimetres, a site offset, a storey at 3 m, a wall running +y with two layers, a window
    /// opening placed relative to the wall, and a property set of every value type.
    const WALL: &str = "
#1=IFCPROJECT('G1',$,'p',$,$,$,$,$,#3);
#2=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);
#3=IFCUNITASSIGNMENT((#2));
#10=IFCCARTESIANPOINT((1000.,2000.,0.));
#11=IFCAXIS2PLACEMENT3D(#10,$,$);
#12=IFCLOCALPLACEMENT($,#11);
#13=IFCSITE('G2',$,'site',$,$,#12,$,$,$,$,$,$,$,$);
#20=IFCCARTESIANPOINT((0.,0.,3000.));
#21=IFCAXIS2PLACEMENT3D(#20,$,$);
#22=IFCLOCALPLACEMENT(#12,#21);
#23=IFCBUILDINGSTOREY('G3',$,'L1',$,$,#22,$,$,$,3000.);
#24=IFCRELAGGREGATES('G4',$,$,$,#13,(#23));
#30=IFCCARTESIANPOINT((500.,0.,0.));
#31=IFCDIRECTION((0.,0.,1.));
#32=IFCDIRECTION((0.,1.,0.));
#33=IFCAXIS2PLACEMENT3D(#30,#31,#32);
#34=IFCLOCALPLACEMENT(#22,#33);
#35=IFCCARTESIANPOINT((2000.,0.));
#36=IFCAXIS2PLACEMENT2D(#35,$);
#37=IFCRECTANGLEPROFILEDEF(.AREA.,$,#36,4000.,300.);
#38=IFCEXTRUDEDAREASOLID(#37,$,#31,2500.);
#39=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#38));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#39));
#42=IFCWALL('G5',$,'W',$,$,#34,#40,$,$);
#43=IFCRELCONTAINEDINSPATIALSTRUCTURE('G6',$,$,$,(#42,#60),#23);
#44=IFCMATERIAL('Brick',$,$);
#45=IFCMATERIAL('Block',$,$);
#46=IFCMATERIALLAYER(#44,100.,$,$,$,$,$);
#47=IFCMATERIALLAYER(#45,200.,$,$,$,$,$);
#48=IFCMATERIALLAYERSET((#46,#47),'BB',$);
#49=IFCMATERIALLAYERSETUSAGE(#48,.AXIS2.,.NEGATIVE.,150.,$);
#50=IFCRELASSOCIATESMATERIAL('G7',$,$,$,(#42),#49);
#51=IFCCARTESIANPOINT((1000.,0.,900.));
#52=IFCAXIS2PLACEMENT3D(#51,$,$);
#53=IFCLOCALPLACEMENT(#34,#52);
#54=IFCCARTESIANPOINT((400.,0.));
#55=IFCAXIS2PLACEMENT2D(#54,$);
#56=IFCRECTANGLEPROFILEDEF(.AREA.,$,#55,800.,1000.);
#58=IFCEXTRUDEDAREASOLID(#56,$,#31,1200.);
#59=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#58));
#61=IFCPRODUCTDEFINITIONSHAPE($,$,(#59));
#62=IFCOPENINGELEMENT('G8',$,'O',$,$,#53,#61,$,$);
#63=IFCRELVOIDSELEMENT('G9',$,$,$,#42,#62);
#60=IFCWINDOW('G10',$,'Win',$,$,$,$,$,$,$,$,$,$);
#64=IFCRELFILLSELEMENT('G11',$,$,$,#62,#60);
#70=IFCPROPERTYSINGLEVALUE('Width',$,IFCPOSITIVELENGTHMEASURE(250.),$);
#71=IFCPROPERTYSINGLEVALUE('Load',$,IFCBOOLEAN(.F.),$);
#72=IFCPROPERTYSINGLEVALUE('Code',$,IFCIDENTIFIER('X1'),$);
#73=IFCPROPERTYSINGLEVALUE('N',$,IFCINTEGER(3),$);
#74=IFCPROPERTYSET('G12',$,'PS',$,(#70,#71,#72,#73));
#75=IFCRELDEFINESBYPROPERTIES('G13',$,$,$,(#42),#74);
#76=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('EI30'),$);
#77=IFCPROPERTYSET('G14',$,'Pset_WindowCommon',$,(#76));
#78=IFCRELDEFINESBYPROPERTIES('G15',$,$,$,(#60),#77);
";

    #[test]
    fn units_placements_wall_layers_opening_and_psets() {
        let im = import_ifc(&file("IFC4", WALL)).unwrap();
        assert!(im.warnings.is_empty(), "{:?}", im.warnings);
        let level = get(&im, 3);
        assert_eq!((level.kind.as_str(), level.seed.clone()), ("Level", id(3).to_ulid_string()));
        assert!(close(num(level, "elevation"), 3.0));
        let w = get(&im, 5);
        assert_eq!(w.kind, "Wall");
        assert_eq!(prop(w, "level"), &Value::Ref(id(3)));
        for (k, v) in [("x1", 1.5), ("y1", 2.0), ("x2", 1.5), ("y2", 6.0), ("height", 2.5), ("base_offset", 0.0)] {
            assert!(close(num(w, k), v), "{k} = {}", num(w, k));
        }
        let Value::Ref(t) = prop(w, "type") else { panic!() };
        let wt = im.entities.iter().find(|e| e.id == *t).unwrap();
        assert_eq!(wt.kind, "WallType");
        assert_eq!(prop(wt, "layer_thickness"), &Value::List(vec![Value::Num(0.1), Value::Num(0.2)]));
        assert_eq!(prop(wt, "layer_material"), &Value::List(vec![Value::Text("Brick".into()), Value::Text("Block".into())]));
        // Property values by type; lengths in metres.
        assert!(close(num(w, "PS.Width"), 0.25));
        assert_eq!(prop(w, "PS.Load"), &Value::Bool(false));
        assert_eq!(prop(w, "PS.Code"), &Value::Text("X1".into()));
        assert_eq!(prop(w, "PS.N"), &Value::Num(3.0));
        assert_eq!(prop(w, "name"), &Value::Text("W".into()));
        // The opening, in the wall's frame; its window is not imported on its own.
        let o = get(&im, 8);
        assert_eq!(o.kind, "Opening");
        assert_eq!(prop(o, "host"), &Value::Ref(id(5)));
        assert_eq!(prop(o, "kind"), &Value::Text("window".into()));
        for (k, v) in [("offset", 1.0), ("sill", 0.9), ("width", 0.8), ("height", 1.2)] {
            assert!(close(num(o, k), v), "{k} = {}", num(o, k));
        }
        assert_eq!(prop(o, "Pset_WindowCommon.FireRating"), &Value::Text("EI30".into()));
        assert!(!im.entities.iter().any(|e| e.id == id(10)));
        // The model builds the wall with the opening cut.
        let model = bim_model::Model::compile(&document(&im));
        let v = model.walls.iter().find(|g| g.id == id(5)).unwrap().volume;
        assert!(close(v, (4.0 * 2.5 - 0.8 * 1.2) * 0.3), "{v}");
    }

    #[test]
    fn positive_layer_sense_puts_the_last_layer_left() {
        let im = import_ifc(&file("IFC4", &WALL.replace(".NEGATIVE.,150.", ".POSITIVE.,-150."))).unwrap();
        let Value::Ref(t) = prop(get(&im, 5), "type") else { panic!() };
        let wt = im.entities.iter().find(|e| e.id == *t).unwrap();
        assert_eq!(prop(wt, "layer_thickness"), &Value::List(vec![Value::Num(0.2), Value::Num(0.1)]));
    }

    #[test]
    fn a_non_rectangular_wall_becomes_a_shape_with_its_opening_cut() {
        // A trapezoid profile is not a parametric wall.
        let data = WALL
            .replace("#37=IFCRECTANGLEPROFILEDEF(.AREA.,$,#36,4000.,300.);", "#37=IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#80);")
            .replace("#38=", "#80=IFCPOLYLINE((#81,#82,#83,#84,#81));\n#81=IFCCARTESIANPOINT((0.,-150.));\n#82=IFCCARTESIANPOINT((4000.,-150.));\n#83=IFCCARTESIANPOINT((3500.,150.));\n#84=IFCCARTESIANPOINT((500.,150.));\n#38=");
        let im = import_ifc(&file("IFC4", &data)).unwrap();
        let w = get(&im, 5);
        assert_eq!(w.kind, "Shape");
        assert_eq!(prop(w, "classification"), &Value::Text("IfcWall".into()));
        assert_eq!(prop(w, "level"), &Value::Ref(id(3)));
        let shape = Shape::from_entity(&doc_entity(w)).unwrap();
        assert!(shape.is_closed());
        // The trapezoid prism minus the window opening's box (0.8 × 1.2 through 0.3).
        let want = (4.0 + 3.0) / 2.0 * 0.3 * 2.5 - 0.8 * 1.2 * 0.3;
        assert!(close(shape.volume(), want), "{} vs {want}", shape.volume());
        // Six faces, the front and back with the hole, and the four faces of the tunnel.
        assert_eq!(shape.faces.len(), 10);
        assert_eq!(shape.faces.iter().filter(|f| f.loops.len() == 2).count(), 2);
        // The opening is cut, not imported; the window has no body, so it is reported.
        assert!(!im.entities.iter().any(|e| e.id == id(8) || e.id == id(10)));
        assert!(im.warnings.iter().any(|w| w.contains("IfcOpeningElement") && w.contains("cut into its host")), "{:?}", im.warnings);
        assert!(im.warnings.iter().any(|w| w.contains("IfcWindow") && w.contains("no body")), "{:?}", im.warnings);
        // The Shape exports as an IfcWall with a faceted brep of its faces and re-imports as the
        // same Shape.
        let text = crate::export::export_ifc4_string(&document(&im), "t.ifc");
        let f = StepFile::parse_str(&text).unwrap();
        let wall = f.entities_of("IfcWall").find(|e| e.get_str("GlobalId") == Some(gid(5).as_str())).unwrap();
        let rep = wall.get_entity("Representation").unwrap().get_entities("Representations")[0];
        let brep = rep.get_entities("Items")[0];
        assert!(brep.is_a("IfcFacetedBrep"));
        assert_eq!(brep.get_entity("Outer").unwrap().get_refs("CfsFaces").len(), 10);
        assert!(f.entities_of("IfcOpeningElement").next().is_none());
        let again = import_ifc(&f).unwrap();
        let w2 = get(&again, 5);
        assert_eq!(w2.kind, "Shape");
        let s2 = Shape::from_entity(&doc_entity(w2)).unwrap();
        assert!(s2.is_closed() && close(s2.volume(), shape.volume()));
    }

    #[test]
    fn a_faceted_brep_is_oriented_outwards_ifc2x3() {
        // A unit cube whose faces all point inwards, one of them through Orientation .F.; no
        // units (metres).
        let pts = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]];
        let outward = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
        let mut data = String::from(
            "#1=IFCPROJECT('G1',$,'p',$,$,$,$,$,$);\n#2=IFCBUILDINGSTOREY('G2',$,'L0',$,$,$,$,$,.ELEMENT.,0.);\n#3=IFCRELCONTAINEDINSPATIALSTRUCTURE('G3',$,$,$,(#4),#2);\n#4=IFCBUILDINGELEMENTPROXY('G4',$,'Box',$,$,#5,#9,$,$);\n#5=IFCLOCALPLACEMENT($,#6);\n#6=IFCAXIS2PLACEMENT3D(#7,$,$);\n#7=IFCCARTESIANPOINT((10.,0.,0.));\n#9=IFCPRODUCTDEFINITIONSHAPE($,$,(#10));\n#10=IFCSHAPEREPRESENTATION($,'Body','Brep',(#11));\n#11=IFCFACETEDBREP(#12);\n",
        );
        let faces: Vec<String> = (0..6).map(|k| format!("#{}", 30 + 3 * k + 2)).collect();
        data += &format!("#12=IFCCLOSEDSHELL(({}));\n", faces.join(","));
        for (k, p) in pts.iter().enumerate() {
            data += &format!("#{}=IFCCARTESIANPOINT(({}.,{}.,{}.));\n", 20 + k, p[0], p[1], p[2]);
        }
        for (k, f) in outward.iter().enumerate() {
            let (mut lp, sense) = (f.to_vec(), if k == 2 { ".F." } else { ".T." });
            if k != 2 {
                lp.reverse();
            }
            let refs: Vec<String> = lp.iter().map(|i| format!("#{}", 20 + i)).collect();
            data += &format!("#{}=IFCPOLYLOOP(({}));\n", 30 + 3 * k, refs.join(","));
            data += &format!("#{}=IFCFACEOUTERBOUND(#{},{sense});\n", 31 + 3 * k, 30 + 3 * k);
            data += &format!("#{}=IFCFACE((#{}));\n", 32 + 3 * k, 31 + 3 * k);
        }
        let im = import_ifc(&file("IFC2X3", &data)).unwrap();
        let s = get(&im, 4);
        assert_eq!((s.kind.as_str(), prop(s, "classification")), ("Shape", &Value::Text("IfcBuildingElementProxy".into())));
        let shape = Shape::from_entity(&doc_entity(s)).unwrap();
        assert!(shape.is_closed());
        assert!(close(shape.volume(), 1.0), "{}", shape.volume());
        assert_eq!(shape.vertices.len(), 8);
        let t = prop(s, "transform");
        assert_eq!(t, &Value::List([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 0.0, 0.0, 1.0].map(Value::Num).to_vec()));
        let model = bim_model::Model::compile(&document(&im));
        let el = model.element(id(4)).unwrap();
        assert!(el.mesh.is_closed() && close(el.mesh.volume(), 1.0));
    }

    #[test]
    fn face_sets_are_one_based_and_non_planar_faces_are_fanned() {
        let data = "
#1=IFCPROJECT('G1',$,'p',$,$,$,$,$,#3);
#2=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);
#3=IFCUNITASSIGNMENT((#2));
#4=IFCBUILDINGELEMENTPROXY('G4',$,'Sheet',$,$,$,#9,$,$);
#9=IFCPRODUCTDEFINITIONSHAPE($,$,(#10));
#10=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#11,#14));
#11=IFCPOLYGONALFACESET(#12,$,(#13),$);
#12=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1000.,0.,0.),(1000.,1000.,500.),(0.,1000.,0.)));
#13=IFCINDEXEDPOLYGONALFACE((1,2,3,4));
#14=IFCTRIANGULATEDFACESET(#15,$,.T.,((1,3,2),(1,2,4),(2,3,4),(3,1,4)),$);
#15=IFCCARTESIANPOINTLIST3D(((5000.,0.,0.),(6000.,0.,0.),(5000.,1000.,0.),(5000.,0.,1000.)));
";
        let im = import_ifc(&file("IFC4", data)).unwrap();
        let s = get(&im, 4);
        let shape = Shape::from_entity(&doc_entity(s)).unwrap();
        // The bent quad: two triangles fanned from its first vertex; the tetrahedron: four faces.
        assert_eq!(shape.faces.len(), 6);
        assert_eq!(shape.faces[0].loops, vec![vec![0, 1, 2]]);
        assert_eq!(shape.faces[1].loops, vec![vec![0, 2, 3]]);
        assert_eq!(shape.vertices[2], [1.0, 1.0, 0.5]);
        assert!(!shape.is_closed(), "the open sheet makes the whole shape an open surface");
        assert!(!im.entities.iter().any(|e| e.props.iter().any(|(k, _)| k == "level")));
    }

    #[test]
    fn columns_need_levels_at_both_ends() {
        let column = |n: u32, x: f64, depth: f64, angle: f64| -> String {
            let b = 100 + 10 * n;
            format!(
                "#{b}=IFCCOLUMN('G{n}',$,'C{n}',$,$,#{p},#{s},$,$);\n#{p}=IFCLOCALPLACEMENT($,#{a});\n#{a}=IFCAXIS2PLACEMENT3D(#{c},$,#{d});\n#{c}=IFCCARTESIANPOINT(({x},0.,0.));\n#{d}=IFCDIRECTION(({},{},0.));\n#{s}=IFCPRODUCTDEFINITIONSHAPE($,$,(#{r}));\n#{r}=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#{e}));\n#{e}=IFCEXTRUDEDAREASOLID(#{f},$,#9,{depth});\n#{f}=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,0.3,0.4);\n",
                angle.cos(),
                angle.sin(),
                p = b + 1,
                a = b + 2,
                c = b + 3,
                d = b + 4,
                s = b + 5,
                r = b + 6,
                e = b + 7,
                f = b + 8
            )
        };
        let mut data = String::from(
            "#1=IFCPROJECT('G1',$,'p',$,$,$,$,$,$);\n#2=IFCBUILDINGSTOREY('G2',$,'L0',$,$,$,$,$,$,0.);\n#3=IFCBUILDINGSTOREY('G3',$,'L1',$,$,$,$,$,$,3.);\n#4=IFCRELCONTAINEDINSPATIALSTRUCTURE('G4',$,$,$,(#210,#220,#230),#2);\n#9=IFCDIRECTION((0.,0.,1.));\n",
        );
        data += &column(11, 1.0, 3.0, 0.0);
        data += &column(12, 2.0, 2.5, 0.0);
        data += &column(13, 3.0, 3.0, 0.5);
        let im = import_ifc(&file("IFC4", &data)).unwrap();
        let c1 = get(&im, 11);
        assert_eq!(c1.kind, "Column");
        assert_eq!((prop(c1, "base"), prop(c1, "top")), (&Value::Ref(id(2)), &Value::Ref(id(3))));
        assert!(close(num(c1, "x"), 1.0) && close(num(c1, "y"), 0.0));
        assert!(close(num(c1, "section_b"), 0.3) && close(num(c1, "section_h"), 0.4));
        for n in [12, 13] {
            let c = get(&im, n);
            assert_eq!((c.kind.as_str(), prop(c, "classification")), ("Shape", &Value::Text("IfcColumn".into())), "C{n}");
            assert_eq!(prop(c, "level"), &Value::Ref(id(2)));
        }
    }

    #[test]
    fn only_ifc4_and_ifc2x3_are_read() {
        let e = import_ifc(&file("CONFIG_CONTROL_DESIGN", "#1=IFCPROJECT('G1',$,'p',$,$,$,$,$,$);")).unwrap_err();
        assert!(e.0.contains("FILE_SCHEMA"), "{e}");
        assert!(import_ifc(&file("IFC2X3", "#1=IFCPROJECT('G1',$,'p',$,$,$,$,$,$);")).unwrap().entities.is_empty());
        for ok in ["IFC4", "IFC4_ADD2_TC1", "ifc2x3_tc1"] {
            assert!(import_ifc(&file(ok, WALL)).is_ok(), "{ok}");
        }
        // Later and earlier schemas are refused, not read as IFC4 / IFC2X3.
        for bad in ["IFC4X3", "IFC4X3_ADD2", "IFC2X2_FINAL", "IFC4X1"] {
            let e = import_ifc(&file(bad, WALL)).unwrap_err();
            assert!(e.0.contains("unsupported FILE_SCHEMA"), "{bad}: {e}");
        }
    }

    #[test]
    fn a_dangling_reference_under_a_product_is_an_error() {
        // In a placement chain, in a representation item, and in a product's own placement.
        for (from, to) in [("#34=IFCLOCALPLACEMENT(#22,#33);", "#34=IFCLOCALPLACEMENT(#922,#33);"), ("#36=IFCAXIS2PLACEMENT2D(#35,$);", "#36=IFCAXIS2PLACEMENT2D(#935,$);"), ("#62=IFCOPENINGELEMENT('G8',$,'O',$,$,#53,", "#62=IFCOPENINGELEMENT('G8',$,'O',$,$,#953,")] {
            let data = WALL.replace(from, to);
            assert_ne!(data, WALL);
            let e = import_ifc(&file("IFC4", &data)).unwrap_err();
            assert!(e.0.contains("which the file does not define"), "{e}");
        }
        // A dangling reference outside products' geometry (a relationship) is not checked here.
        let data = WALL.replace("(#42,#60),#23);", "(#42,#60,#999),#23);");
        assert!(import_ifc(&file("IFC4", &data)).is_ok());
    }

    #[test]
    fn standard_case_subtypes_import_like_their_class() {
        let data = WALL.replace("IFCWALL('G5'", "IFCWALLSTANDARDCASE('G5'").replace("#60=IFCWINDOW('G10',$,'Win',$,$,$,$,$,$,$,$,$,$);", "#60=IFCDOORSTANDARDCASE('G10',$,'Door',$,$,$,$,$,$,$,$,$,$);");
        let im = import_ifc(&file("IFC4", &data)).unwrap();
        assert_eq!(get(&im, 5).kind, "Wall");
        let o = get(&im, 8);
        assert_eq!((o.kind.as_str(), prop(o, "kind")), ("Opening", &Value::Text("door".into())));
        assert!(!im.entities.iter().any(|e| e.id == id(10)), "the door is the Opening's filling");
        let col = "#1=IFCPROJECT('G1',$,'p',$,$,$,$,$,$);\n#2=IFCBUILDINGSTOREY('G2',$,'L0',$,$,$,$,$,$,0.);\n#3=IFCBUILDINGSTOREY('G3',$,'L1',$,$,$,$,$,$,3.);\n#4=IFCRELCONTAINEDINSPATIALSTRUCTURE('G4',$,$,$,(#5,#15),#2);\n#5=IFCCOLUMNSTANDARDCASE('G5',$,'C',$,$,$,#6,$,$);\n#6=IFCPRODUCTDEFINITIONSHAPE($,$,(#7));\n#7=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#8));\n#8=IFCEXTRUDEDAREASOLID(#9,$,#10,3.);\n#9=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,0.3,0.4);\n#10=IFCDIRECTION((0.,0.,1.));\n#15=IFCMEMBERSTANDARDCASE('G15',$,'M',$,$,$,#6,$,$);\n";
        let im = import_ifc(&file("IFC4", col)).unwrap();
        assert_eq!(get(&im, 5).kind, "Column");
        let m = get(&im, 15);
        assert_eq!((m.kind.as_str(), prop(m, "classification")), ("Shape", &Value::Text("IfcMemberStandardCase".into())));
    }

    #[test]
    fn an_unfilled_opening_is_a_door_at_the_wall_base_else_a_window() {
        let unfilled = WALL.replace("#64=IFCRELFILLSELEMENT('G11',$,$,$,#62,#60);", "");
        let im = import_ifc(&file("IFC4", &unfilled)).unwrap();
        assert_eq!(prop(get(&im, 8), "kind"), &Value::Text("window".into()));
        assert!(im.warnings.iter().any(|w| w.contains("no IfcDoor / IfcWindow filling")), "{:?}", im.warnings);
        let at_base = unfilled.replace("#51=IFCCARTESIANPOINT((1000.,0.,900.));", "#51=IFCCARTESIANPOINT((1000.,0.,0.));");
        let im = import_ifc(&file("IFC4", &at_base)).unwrap();
        let o = get(&im, 8);
        assert_eq!((prop(o, "kind"), num(o, "sill")), (&Value::Text("door".into()), 0.0));
        // Export fills it with a door, which re-imports as the same kind.
        let text = crate::export::export_ifc4_string(&document(&im), "t.ifc");
        let again = import_ifc(&StepFile::parse_str(&text).unwrap()).unwrap();
        assert_eq!(prop(get(&again, 8), "kind"), &Value::Text("door".into()));
    }

    #[test]
    fn an_opening_not_through_its_wall_makes_the_wall_a_shape_with_the_void_cut() {
        // The window's box reaches 0.1 into a 0.3 wall from its face (a niche), or is a 0.1 deep
        // pocket inside it (a cavity, an inner shell facing inwards): not an Opening either way.
        for (centre, depth) in [("400.,150.", "200."), ("400.,0.", "100.")] {
            let data = WALL
                .replace("#54=IFCCARTESIANPOINT((400.,0.));", &format!("#54=IFCCARTESIANPOINT(({centre}));"))
                .replace("#56=IFCRECTANGLEPROFILEDEF(.AREA.,$,#55,800.,1000.);", &format!("#56=IFCRECTANGLEPROFILEDEF(.AREA.,$,#55,800.,{depth});"));
            let im = import_ifc(&file("IFC4", &data)).unwrap();
            let w = get(&im, 5);
            assert_eq!((w.kind.as_str(), prop(w, "classification")), ("Shape", &Value::Text("IfcWall".into())));
            let shape = Shape::from_entity(&doc_entity(w)).unwrap();
            assert!(shape.is_closed());
            assert!(close(shape.volume(), 4.0 * 2.5 * 0.3 - 0.8 * 1.2 * 0.1), "{centre}: {}", shape.volume());
            assert!(!im.entities.iter().any(|e| e.kind == "Opening" || e.kind == "WallType"));
            let model = bim_model::Model::compile(&document(&im));
            let el = model.element(id(5)).unwrap();
            assert!(el.mesh.is_closed() && close(el.mesh.volume(), shape.volume()), "{}", el.mesh.volume());
        }
    }

    /// The WALL file with the wall's extrusion `#38` replaced by `item` (extra instances in `more`).
    fn wall_body(item: &str, more: &str) -> String {
        WALL.replace("#39=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#38));", &format!("#39=IFCSHAPEREPRESENTATION($,'Body','Clipping',({item}));\n{more}"))
    }

    #[test]
    fn clipped_bodies_are_shapes_with_the_half_space_removed() {
        // The 4 × 0.3 × 2.5 wall clipped at z = 2 (wall frame); the opening box (z 0.9..2.1) is
        // cut from what remains.
        let plane = "#91=IFCPLANE(#92);\n#92=IFCAXIS2PLACEMENT3D(#93,#31,$);\n#93=IFCCARTESIANPOINT((0.,0.,2000.));\n";
        let volume = |data: &str| -> f64 {
            let im = import_ifc(&file("IFC4", data)).unwrap();
            let w = get(&im, 5);
            assert_eq!((w.kind.as_str(), prop(w, "classification")), ("Shape", &Value::Text("IfcWall".into())), "{:?}", im.warnings);
            assert!(!im.entities.iter().any(|e| e.id == id(8)));
            let s = Shape::from_entity(&doc_entity(w)).unwrap();
            assert!(s.is_closed());
            s.volume()
        };
        // AgreementFlag false: the normal (+z) points into the half space's material, so the top
        // is removed.
        let below = wall_body("#90", &format!("#90=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#38,#94);\n#94=IFCHALFSPACESOLID(#91,.F.);\n{plane}"));
        let v = volume(&below);
        assert!(close(v, 4.0 * 0.3 * 2.0 - 0.8 * 1.1 * 0.3), "{v}");
        // AgreementFlag true: the material is below the plane, so the top remains.
        let above = below.replace("IFCHALFSPACESOLID(#91,.F.)", "IFCHALFSPACESOLID(#91,.T.)");
        let v = volume(&above);
        assert!(close(v, 4.0 * 0.3 * 0.5 - 0.8 * 0.1 * 0.3), "{v}");
        // A polygonal bounded half space removes only the part over its boundary (x 0..1).
        let bounded = wall_body(
            "#90",
            &format!("#90=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#38,#94);\n#94=IFCPOLYGONALBOUNDEDHALFSPACE(#91,.F.,#95,#96);\n#95=IFCAXIS2PLACEMENT3D(#97,$,$);\n#97=IFCCARTESIANPOINT((0.,0.,0.));\n#96=IFCPOLYLINE((#98,#99,#100,#101,#98));\n#98=IFCCARTESIANPOINT((0.,-500.));\n#99=IFCCARTESIANPOINT((1000.,-500.));\n#100=IFCCARTESIANPOINT((1000.,500.));\n#101=IFCCARTESIANPOINT((0.,500.));\n{plane}"),
        );
        let v = volume(&bounded);
        assert!(close(v, 4.0 * 0.3 * 2.5 - 1.0 * 0.3 * 0.5 - 0.8 * 1.2 * 0.3), "{v}");
        // Nested: clipped twice (the second plane cuts x > 3.5 away).
        let twice = wall_body(
            "#102",
            &format!("#90=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#38,#94);\n#94=IFCHALFSPACESOLID(#91,.F.);\n{plane}#102=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#90,#103);\n#103=IFCHALFSPACESOLID(#104,.F.);\n#104=IFCPLANE(#105);\n#105=IFCAXIS2PLACEMENT3D(#106,#107,$);\n#106=IFCCARTESIANPOINT((3500.,0.,0.));\n#107=IFCDIRECTION((1.,0.,0.));\n"),
        );
        let v = volume(&twice);
        assert!(close(v, 3.5 * 0.3 * 2.0 - 0.8 * 1.1 * 0.3), "{v}");
        // A half space as the first operand, or a union with one, is reported and skipped.
        let bad = wall_body("#90", &format!("#90=IFCBOOLEANRESULT(.UNION.,#38,#94);\n#94=IFCHALFSPACESOLID(#91,.F.);\n{plane}"));
        let im = import_ifc(&file("IFC4", &bad)).unwrap();
        assert!(!im.entities.iter().any(|e| e.id == id(5)));
        assert!(im.warnings.iter().any(|w| w.contains("unbounded")), "{:?}", im.warnings);
    }

    #[test]
    fn csg_solids_and_blocks_ifc2x3() {
        // A unit block minus a unit block moved by (0.5, 0.5, 0.5); metres.
        let data = "
#1=IFCPROJECT('G1',$,'p',$,$,$,$,$,$);
#2=IFCBUILDINGSTOREY('G2',$,'L0',$,$,$,$,$,.ELEMENT.,0.);
#3=IFCRELCONTAINEDINSPATIALSTRUCTURE('G3',$,$,$,(#4),#2);
#4=IFCBUILDINGELEMENTPROXY('G4',$,'Csg',$,$,$,#9,$,$);
#9=IFCPRODUCTDEFINITIONSHAPE($,$,(#10));
#10=IFCSHAPEREPRESENTATION($,'Body','CSG',(#11));
#11=IFCCSGSOLID(#12);
#12=IFCBOOLEANRESULT(.DIFFERENCE.,#13,#14);
#13=IFCBLOCK(#15,1.,1.,1.);
#14=IFCBLOCK(#16,1.,1.,1.);
#15=IFCAXIS2PLACEMENT3D(#17,$,$);
#16=IFCAXIS2PLACEMENT3D(#18,$,$);
#17=IFCCARTESIANPOINT((0.,0.,0.));
#18=IFCCARTESIANPOINT((0.5,0.5,0.5));
";
        let im = import_ifc(&file("IFC2X3", data)).unwrap();
        let s = Shape::from_entity(&doc_entity(get(&im, 4))).unwrap();
        assert!(s.is_closed());
        assert!(close(s.volume(), 0.875), "{}", s.volume());
        let union = import_ifc(&file("IFC2X3", &data.replace(".DIFFERENCE.", ".UNION."))).unwrap();
        assert!(close(Shape::from_entity(&doc_entity(get(&union, 4))).unwrap().volume(), 1.875));
        let inter = import_ifc(&file("IFC2X3", &data.replace(".DIFFERENCE.", ".INTERSECTION."))).unwrap();
        assert!(close(Shape::from_entity(&doc_entity(get(&inter, 4))).unwrap().volume(), 0.125));
    }

    #[test]
    fn transforms_invert() {
        let x = frame([0.3, -0.2, 1.0], [1.0, 2.0, 0.0], [4.0, -5.0, 6.0]);
        let s = Xf::from_axes([2.0, 0.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.0, 3.0], [1.0, 1.0, 1.0]).then(&x);
        for m in [x, s] {
            let i = m.inverse().unwrap();
            let p = [0.7, -1.3, 2.9];
            let q = i.apply(m.apply(p));
            assert!((0..3).all(|k| (q[k] - p[k]).abs() < 1e-12), "{q:?}");
        }
        assert!(Xf { m: [[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] }.inverse().is_none());
    }

    #[test]
    fn interior_points_of_non_convex_polygons() {
        let c = [[0.0, 0.0], [4.0, 0.0], [4.0, 1.0], [1.0, 1.0], [1.0, 3.0], [4.0, 3.0], [4.0, 4.0], [0.0, 4.0]];
        let p = interior_point(&c).unwrap();
        assert!(inside(&c, p), "{p:?}");
        let arc_pts = arc([1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], 4);
        assert_eq!(arc_pts.len(), 5);
        assert!(arc_pts.iter().all(|q| close(q[0].hypot(q[1]), 1.0) && q[1] >= -1e-12));
    }
}
