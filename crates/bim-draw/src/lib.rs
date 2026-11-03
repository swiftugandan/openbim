//! `bim-draw`: 2D drawings of the compiled building (spec/cli.md, "Export, import, drawings").
//!
//! [`View::parse`] reads a `--view` string, [`draw`] builds the [`Drawing`] of that view from the
//! document and its compiled [`Model`], and [`svg::to_svg`], [`dxf::to_dxf`] and [`pdf::to_pdf`]
//! write it as SVG, DXF (AutoCAD 2013) and PDF 1.7.
//!
//! # Views
//!
//! Each view is an orthographic projection with a viewing direction `f` (from the eye into the
//! scene) and paper axes `(u, v)` = `(p·r, p·v)` of [`bim_geom::OrthoView`] (`r = f × up`,
//! `v = r × f`):
//!
//! | View | `f` | `u` | `v` | Cut plane |
//! |---|---|---|---|---|
//! | `plan:<level>@<h>` | −z | x | y | z = level elevation + h |
//! | `section:x0,y0,x1,y1` | left of `d = (x1−x0, y1−y0)` | along `d` | z | the vertical plane through the line (unbounded) |
//! | `elevation:s` / `n` / `e` / `w` | +y / −y / −x / +x | x / −x / y / −y | z | none |
//!
//! # Content
//!
//! * **CUT** — in a plan or section, every element solid's section by the cut plane
//!   ([`bim_geom::section`]): one region per entity whose rings are the outer loops and holes
//!   (even-odd).
//! * **HATCH** — 45° lines clipped to each cut region (every hatch point lies inside its region);
//!   every cut entity gets at least one hatch line.
//! * **PROJECTION / HIDDEN** — the visible and hidden feature edges of [`bim_geom::hlr`]. In a
//!   plan or section only what lies beyond the cut plane is drawn: every solid is clipped to the
//!   far half-space (solids entirely on the viewer's side are dropped) and capped with its
//!   section, and the cap's own edges (which coincide with the CUT outline) are not repeated on
//!   these layers. In an elevation the whole model is projected.
//! * **Faces in the cut plane.** When a solid has a vertex on the cut plane (a plan cut exactly at
//!   a wall top or at a slab top), that solid is cut a micrometre on the viewer's side of the
//!   plane instead: faces lying in the plane count as beyond the cut, not as cut.
//! * **DIMENSION** — every `Dimension` entity whose `view` prop equals the view string: a line
//!   between the projections of its two referenced points and a text equal to their 3D distance in
//!   millimetres, rounded half up to an integer (`floor(d·1000 + 0.5)`). The value is derived from
//!   the document at drawing time, so it follows every edit (associativity). A Dimension whose
//!   references do not name entities with numeric `x` and `y` is not drawn.
//!
//! Every item carries the id (`_name`) of the entity it shows. Coordinates of a [`Drawing`] are
//! model metres in the view's `(u, v)` frame; [`svg::to_svg`] scales them to paper millimetres at
//! 1:100.

#![forbid(unsafe_code)]

mod clip;
pub mod dxf;
mod paper;
pub mod pdf;
pub mod svg;

use bim_doc::{Document, EntityId, Value};
use bim_geom::{hlr, section, OrthoView, Plane3, Point3, Solid, Vec3};
use bim_model::{entity_name, Model};

/// Paper millimetres per model metre at the default scale 1:100.
pub const SCALE: f64 = 10.0;

/// A drawing layer of spec/cli.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    Cut,
    Projection,
    Hidden,
    Hatch,
    Dimension,
    Annotation,
}

impl Layer {
    /// Every layer, in drawing order.
    pub const ALL: [Layer; 6] = [Layer::Hatch, Layer::Hidden, Layer::Projection, Layer::Cut, Layer::Dimension, Layer::Annotation];

    /// The layer name (SVG `<g id>`, DXF layer, PDF OCG).
    pub fn name(self) -> &'static str {
        match self {
            Layer::Cut => "CUT",
            Layer::Projection => "PROJECTION",
            Layer::Hidden => "HIDDEN",
            Layer::Hatch => "HATCH",
            Layer::Dimension => "DIMENSION",
            Layer::Annotation => "ANNOTATION",
        }
    }

    /// The lineweight in paper millimetres.
    pub fn width(self) -> f64 {
        match self {
            Layer::Cut => 0.50,
            Layer::Projection => 0.25,
            Layer::Hidden => 0.18,
            Layer::Hatch => 0.13,
            Layer::Dimension | Layer::Annotation => 0.18,
        }
    }

    /// Whether strokes on this layer are dashed (ISO 128 dashed line).
    pub fn dashed(self) -> bool {
        self == Layer::Hidden
    }
}

/// A point of the drawing: `(u, v)` in model metres.
pub type P2 = [f64; 2];

/// The geometry of one drawing item.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A region bounded by closed rings under the even-odd rule (outer loops and holes).
    Region(Vec<Vec<P2>>),
    /// Separate straight segments.
    Lines(Vec<[P2; 2]>),
    /// A dimension between two points, labelled `text`.
    Dimension { a: P2, b: P2, text: String },
}

/// One item: geometry on a layer, showing one entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub layer: Layer,
    /// The id (`_name`) of the entity shown.
    pub entity: String,
    pub shape: Shape,
}

/// A drawing of one view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    /// The view string it was drawn for.
    pub view: String,
    pub items: Vec<Item>,
}

/// The kind of a view, with its resolved parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewKind {
    /// Plan of a level, cut at absolute height `z`.
    Plan { level: EntityId, z: f64 },
    /// Vertical section through the line from `p0` along the unit direction `d`.
    Section { p0: P2, d: P2 },
    /// Elevation looking from the side `side` (`'n'`, `'s'`, `'e'` or `'w'`).
    Elevation { side: char },
}

/// A parsed `--view`.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// The view string as given (Dimensions are matched against it).
    pub text: String,
    pub kind: ViewKind,
}

/// Why a view string is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewError(pub String);

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ViewError {}

/// A finite number written exactly as `s` (no surrounding whitespace is accepted).
fn finite(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|x| x.is_finite())
}

impl View {
    /// Parses `plan:<level>@<h>`, `section:<x0>,<y0>,<x1>,<y1>` or `elevation:<n|s|e|w>`. The
    /// level must name a `Level` of `doc` with a numeric elevation; the section line must have
    /// non-zero length; all numbers must be finite.
    pub fn parse(doc: &Document, text: &str) -> Result<View, ViewError> {
        let err = |m: String| Err(ViewError(format!("invalid view `{text}`: {m}")));
        let Some((kind, arg)) = text.split_once(':') else {
            return err("expected plan:<level>@<h>, section:<x0>,<y0>,<x1>,<y1> or elevation:<n|s|e|w>".into());
        };
        let kind = match kind {
            "plan" => {
                let Some((name, h)) = arg.rsplit_once('@') else {
                    return err("a plan view is plan:<level>@<cut height>".into());
                };
                let Some(h) = finite(h) else {
                    return err(format!("cut height `{h}` is not a finite number"));
                };
                let level = EntityId::parse(name);
                let elevation = match doc.get(level) {
                    Some(e) if e.kind == "Level" => match e.props.get("elevation") {
                        Some(Value::Num(z)) if z.is_finite() => *z,
                        _ => return err(format!("level `{name}` has no numeric elevation")),
                    },
                    _ => return err(format!("`{name}` is not a Level of the document")),
                };
                ViewKind::Plan { level, z: elevation + h }
            }
            "section" => {
                let parts: Vec<&str> = arg.split(',').collect();
                let nums: Option<Vec<f64>> = parts.iter().map(|s| finite(s)).collect();
                let Some(n) = nums.filter(|n| n.len() == 4) else {
                    return err("a section view is section:<x0>,<y0>,<x1>,<y1> (four finite numbers)".into());
                };
                let (dx, dy) = (n[2] - n[0], n[3] - n[1]);
                let len = dx.hypot(dy);
                if !len.is_finite() {
                    return err("the section line's length is out of range (not a finite number)".into());
                }
                if !(len > 0.0) {
                    return err("the section line has zero length".into());
                }
                ViewKind::Section { p0: [n[0], n[1]], d: [dx / len, dy / len] }
            }
            "elevation" => match arg {
                "n" | "s" | "e" | "w" => ViewKind::Elevation { side: arg.chars().next().unwrap_or('s') },
                _ => return err("an elevation view is elevation:<n|s|e|w>".into()),
            },
            _ => return err(format!("unknown view kind `{kind}`")),
        };
        Ok(View { text: text.to_string(), kind })
    }

    /// The orthographic view (direction into the scene and up hint).
    pub fn ortho(&self) -> OrthoView {
        match self.kind {
            ViewKind::Plan { .. } => OrthoView::new(Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
            ViewKind::Section { d, .. } => OrthoView::new(Vec3::new(-d[1], d[0], 0.0), Vec3::Z),
            ViewKind::Elevation { side } => {
                let f = match side {
                    'n' => Vec3::new(0.0, -1.0, 0.0),
                    'e' => Vec3::new(-1.0, 0.0, 0.0),
                    'w' => Vec3::new(1.0, 0.0, 0.0),
                    _ => Vec3::new(0.0, 1.0, 0.0),
                };
                OrthoView::new(f, Vec3::Z)
            }
        }
    }

    /// The depth `c` of the cut plane `p·f = c` (`f` the unit viewing direction), if the view
    /// cuts.
    pub fn cut_depth(&self) -> Option<f64> {
        match self.kind {
            ViewKind::Plan { z, .. } => Some(-z),
            ViewKind::Section { p0, d } => Some(-d[1] * p0[0] + d[0] * p0[1]),
            ViewKind::Elevation { .. } => None,
        }
    }

    /// The paper coordinates `(u, v)` of a model point, in metres.
    pub fn project(&self, p: Point3) -> P2 {
        let [_, r, v] = frame(&self.ortho());
        [p.to_vec().dot(r), p.to_vec().dot(v)]
    }
}

fn frame(o: &OrthoView) -> [Vec3; 3] {
    // Every view built by `View::ortho` has a unit direction perpendicular to its up hint.
    o.frame().unwrap_or([Vec3::Y, Vec3::X, Vec3::Z])
}

/// Hatch line spacing in model metres (1.5 mm on paper at 1:100).
pub(crate) const HATCH_SPACING: f64 = 0.15;

/// Builds the drawing of `view` from `doc` and its compiled `model` (see the crate docs).
pub fn draw(doc: &Document, model: &Model, view: &View) -> Drawing {
    let ortho = view.ortho();
    let [f, r, v] = frame(&ortho);
    let proj = |p: Point3| -> P2 { [p.to_vec().dot(r), p.to_vec().dot(v)] };
    let mut drawing = Drawing { view: view.text.clone(), items: Vec::new() };

    let solids: Vec<(String, Solid, &bim_model::Mesh)> =
        model.elements.iter().map(|e| (e.name.clone(), e.mesh.to_solid(), &e.mesh)).collect();

    // ------------------------------------------------------------------ cut and beyond
    let mut scene: Vec<(String, Solid)> = Vec::new();
    let mut cut_rings: Vec<Vec<P2>> = Vec::new();
    match view.cut_depth() {
        Some(c) => {
            // The plane with its normal towards the viewer: section outer loops are then
            // counter-clockwise about the outward normal of the far part's cap.
            for (name, solid, mesh) in &solids {
                let (c, shift) = cut_depth_for(mesh, f, c);
                let plane = Plane3 { normal: -f, offset: -c };
                let polys = section(solid, &plane);
                let to_rings = |p: &bim_geom::Polygon3| -> Vec<Vec<P2>> {
                    std::iter::once(&p.outer)
                        .chain(p.holes.iter())
                        .filter(|r| r.len() >= 3)
                        .map(|r| r.iter().map(|&q| proj(q)).collect())
                        .collect()
                };
                let mut rings: Vec<Vec<P2>> = Vec::new();
                let mut area = 0.0;
                for p in &polys {
                    let rs = to_rings(p);
                    let a = p.area(-f).abs();
                    let perimeter: f64 = rs.iter().map(|r| ring_length(r)).sum();
                    // After a shift, regions only a few shifts thick are slivers of faces or
                    // edges touching the plane, which vanish in the limit: not cut.
                    if shift > 0.0 && a < 10.0 * shift * perimeter {
                        cut_rings.extend(rs);
                        continue;
                    }
                    area += a;
                    rings.extend(rs);
                }
                if area > 1e-12 && !rings.is_empty() {
                    let hatch = hatch_lines(&rings, HATCH_SPACING);
                    drawing.items.push(Item { layer: Layer::Cut, entity: name.clone(), shape: Shape::Region(rings.clone()) });
                    if !hatch.is_empty() {
                        drawing.items.push(Item { layer: Layer::Hatch, entity: name.clone(), shape: Shape::Lines(hatch) });
                    }
                }
                cut_rings.extend(rings);
                if let Some(s) = clip::clip_far(mesh, f, c, &polys) {
                    scene.push((name.clone(), s));
                }
            }
        }
        None => scene = solids.iter().map(|(n, s, _)| (n.clone(), s.clone())).collect(),
    }

    let bodies: Vec<Solid> = scene.iter().map(|(_, s)| s.clone()).collect();
    let res = hlr(&bodies, &ortho);
    for (layer, segs) in [(Layer::Projection, &res.visible), (Layer::Hidden, &res.hidden)] {
        let mut per: Vec<Vec<[P2; 2]>> = vec![Vec::new(); scene.len()];
        // Edges at different depths can project onto the same segment: drawn once per layer.
        let mut seen = std::collections::HashSet::new();
        for s in segs {
            let seg = [[s.a.x, s.a.y], [s.b.x, s.b.y]];
            let key = |p: P2| [(p[0] * 1e7).round() as i64, (p[1] * 1e7).round() as i64];
            let (ka, kb) = (key(seg[0]), key(seg[1]));
            if !seen.insert(if ka <= kb { (ka, kb) } else { (kb, ka) }) || on_rings(seg, &cut_rings) {
                continue;
            }
            if let Some(list) = per.get_mut(s.solid) {
                list.push(seg);
            }
        }
        for (k, lines) in per.into_iter().enumerate() {
            if !lines.is_empty() {
                drawing.items.push(Item { layer, entity: scene[k].0.clone(), shape: Shape::Lines(lines) });
            }
        }
    }

    // ------------------------------------------------------------------ dimensions
    let mut dims: Vec<(String, Item)> = Vec::new();
    for e in doc.entities() {
        if e.kind != "Dimension" || !matches!(e.props.get("view"), Some(Value::Text(t)) if *t == view.text) {
            continue;
        }
        let point = |key: &str| -> Option<Point3> {
            let Some(Value::Ref(id)) = e.props.get(key) else { return None };
            let t = doc.get(*id)?;
            let n = |k: &str, default: Option<f64>| match t.props.get(k) {
                Some(Value::Num(x)) if x.is_finite() => Some(*x),
                None => default,
                _ => None,
            };
            Some(Point3::new(n("x", None)?, n("y", None)?, n("z", Some(0.0))?))
        };
        let (Some(a), Some(b)) = (point("a"), point("b")) else { continue };
        let d = (b - a).norm();
        let text = format!("{}", (d * 1000.0 + 0.5).floor() as i64);
        let name = entity_name(e);
        dims.push((name.clone(), Item { layer: Layer::Dimension, entity: name, shape: Shape::Dimension { a: proj(a), b: proj(b), text } }));
    }
    dims.sort_by(|x, y| x.0.cmp(&y.0));
    drawing.items.extend(dims.into_iter().map(|(_, it)| it));
    drawing
}

/// The depth at which `mesh` is cut: `c` itself, unless a vertex of the mesh lies on that plane
/// (a face or edge in the plane, e.g. a plan cut exactly at a wall top or a level). Then the cut is
/// moved towards the viewer by a micrometre (or more on large meshes), so the cut is always the
/// limit of sections taken just on the viewer's side: a face in the plane is beyond the cut (drawn
/// on PROJECTION if visible) rather than cut, and a solid lying entirely beyond, touching the
/// plane, is not cut.
/// Returns the depth and the shift (0 when unshifted). Regions of such a shifted section that are
/// only a few shifts thick (area < 10 · shift · perimeter) are slivers of edges touching the
/// plane: they are not drawn as cut.
fn cut_depth_for(mesh: &bim_model::Mesh, f: Vec3, c: f64) -> (f64, f64) {
    let Some((lo, hi)) = mesh.bounds() else { return (c, 0.0) };
    let extent = (hi - lo).norm();
    let tol = 1e-9 * (extent + c.abs()).max(1e-3);
    // The shift is ten times the window that triggers it, so the shifted plane is always well
    // clear of the vertices that fired it (and of `clip_far`'s on-plane tolerance).
    let window = 100.0 * tol;
    let on = mesh.positions.iter().any(|p| (p.to_vec().dot(f) - c).abs() <= window);
    if on {
        let shift = (1e-6f64).max(10.0 * window);
        (c - shift, shift)
    } else {
        (c, 0.0)
    }
}

fn ring_length(r: &[P2]) -> f64 {
    (0..r.len())
        .map(|i| {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            (b[0] - a[0]).hypot(b[1] - a[1])
        })
        .sum()
}

/// Whether a segment lies on an edge of one of the rings (its midpoint within a small distance of
/// a parallel ring edge).
fn on_rings(seg: [P2; 2], rings: &[Vec<P2>]) -> bool {
    const EPS: f64 = 1e-6;
    let (a, b) = (seg[0], seg[1]);
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    if !(len > 0.0) {
        return true;
    }
    let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    for r in rings {
        for i in 0..r.len() {
            let (p, q) = (r[i], r[(i + 1) % r.len()]);
            let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
            let el = ex.hypot(ey);
            if !(el > 0.0) {
                continue;
            }
            if ((dx * ey - dy * ex) / (len * el)).abs() > 1e-6 {
                continue;
            }
            if dist_to_segment(m, p, q) <= EPS && dist_to_segment(a, p, q) <= EPS && dist_to_segment(b, p, q) <= EPS {
                return true;
            }
        }
    }
    false
}

fn dist_to_segment(x: P2, p: P2, q: P2) -> f64 {
    let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
    let l2 = ex * ex + ey * ey;
    let t = if l2 > 0.0 { (((x[0] - p[0]) * ex + (x[1] - p[1]) * ey) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (x[0] - p[0] - t * ex).hypot(x[1] - p[1] - t * ey)
}

/// Even-odd point containment in a set of rings.
pub fn inside(rings: &[Vec<P2>], p: P2) -> bool {
    let mut odd = false;
    for r in rings {
        let n = r.len();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                if p[0] < x {
                    odd = !odd;
                }
            }
        }
    }
    odd
}

/// 45° hatch lines clipped to the even-odd region of `rings`, `spacing` apart (anchored at the
/// origin so neighbouring regions line up). A region too small for one line at that spacing gets
/// lines at a quarter of its own width instead, so every region with area is hatched.
pub fn hatch_lines(rings: &[Vec<P2>], spacing: f64) -> Vec<[P2; 2]> {
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let n = [-h, h];
    let t = [h, h];
    let dot = |a: P2, b: P2| a[0] * b[0] + a[1] * b[1];
    let (mut smin, mut smax) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in rings.iter().flatten() {
        let s = dot(*p, n);
        smin = smin.min(s);
        smax = smax.max(s);
    }
    if !(smax > smin) {
        return Vec::new();
    }
    let lines = |anchor: f64, sp: f64| -> Vec<[P2; 2]> {
        let mut out = Vec::new();
        let mut k = ((smin - anchor) / sp).ceil();
        loop {
            let s = anchor + k * sp;
            if s >= smax {
                break;
            }
            if s > smin {
                out.extend(clip_line(rings, n, t, s));
            }
            k += 1.0;
        }
        out
    };
    let out = if (smax - smin) / spacing < 1e5 { lines(0.0, spacing) } else { Vec::new() };
    if !out.is_empty() {
        return out;
    }
    let sp = (smax - smin) / 4.0;
    lines(smin + sp / 2.0, sp)
}

/// The pieces of the line `n·p = s` (direction `t`) inside the even-odd region of `rings`.
fn clip_line(rings: &[Vec<P2>], n: P2, t: P2, s: f64) -> Vec<[P2; 2]> {
    let dot = |a: P2, b: P2| a[0] * b[0] + a[1] * b[1];
    let mut taus = Vec::new();
    for r in rings {
        let m = r.len();
        for i in 0..m {
            let (a, b) = (r[i], r[(i + 1) % m]);
            let (da, db) = (dot(a, n) - s, dot(b, n) - s);
            if (da > 0.0) != (db > 0.0) {
                let k = da / (da - db);
                let p = [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
                taus.push(dot(p, t));
            }
        }
    }
    taus.sort_by(f64::total_cmp);
    let at = |tau: f64| [n[0] * s + t[0] * tau, n[1] * s + t[1] * tau];
    let mut out = Vec::new();
    for pair in taus.chunks(2) {
        if let [t0, t1] = *pair {
            if t1 - t0 > 1e-9 && inside(rings, at(0.5 * (t0 + t1))) {
                out.push([at(t0), at(t1)]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
