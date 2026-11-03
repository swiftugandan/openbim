//! The inference engine of spec/sketch.md "## Inference": given a camera, a cursor and the model's
//! world-space edges, faces and guides, pick the point a drawing tool should use.
//!
//! [`InferScene::new`] parses a scene once and builds two bounding-volume hierarchies (one over the
//! edges' boxes, one over the faces' boxes); [`InferScene::infer`] then answers a query by walking
//! them. Edges are culled with a *pick cone*: a point whose projection is within `r` CSS px of the
//! cursor lies within `k·z` of the cursor ray's line, where `z` is its depth along the view
//! direction and `k = 2·r·tan(fov/2)/height` (pixels are square), so a node whose bounding sphere
//! cannot reach that cone holds no candidate. Faces are culled with a ray–box slab test. Only the
//! few edges near the cursor ray are then tested exactly (their projected distance to the cursor),
//! and intersections are looked for among those edges' pairs.
//!
//! Priority (spec): the first line with a candidate wins; within a line the nearest on screen,
//! then the lowest entity id (byte-wise), then the lowest index.

use crate::json::{self, Json};
use crate::SketchError;

type V3 = [f64; 3];

/// Candidates count when their projection is within this many CSS px of the cursor.
pub const RADIUS_PX: f64 = 10.0;
/// Two edges intersect when their closest points are within this distance (metres).
pub const INTERSECTION_TOL: f64 = 1e-6;
/// Points nearer than this to an endpoint count as that endpoint (shared endpoints, `from` on a line).
const POINT_TOL: f64 = 1e-9;
/// Leaves of the hierarchies hold at most this many items.
const LEAF: usize = 4;

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> V3 {
    mul(a, 1.0 / len(a))
}
fn dist(a: V3, b: V3) -> f64 {
    len(sub(a, b))
}

// ------------------------------------------------------------------ camera

/// A pinhole camera (spec/sketch.md): up is +z, or +y when the view direction is parallel to z;
/// `fov` is vertical (radians); the viewport is `width` × `height` CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub eye: V3,
    pub target: V3,
    pub fov: f64,
    pub width: f64,
    pub height: f64,
}

/// A camera with its frame resolved: forward `f`, right `r`, up `u`, and `tan(fov/2)`.
#[derive(Debug, Clone, Copy)]
struct View {
    eye: V3,
    f: V3,
    r: V3,
    u: V3,
    t: f64,
    w: f64,
    h: f64,
}

impl Camera {
    fn view(&self) -> Result<View, SketchError> {
        let ok = |x: f64| x.is_finite();
        if !(self.fov > 0.0 && self.fov < std::f64::consts::PI) {
            return Err(SketchError("camera: `fov` must be in (0, π) radians".into()));
        }
        if !(ok(self.width) && ok(self.height) && self.width > 0.0 && self.height > 0.0) {
            return Err(SketchError("camera: `width` and `height` must be positive".into()));
        }
        let d = sub(self.target, self.eye);
        if !(len(d) > 0.0) || !len(d).is_finite() {
            return Err(SketchError("camera: `eye` and `target` must differ".into()));
        }
        let f = unit(d);
        let up = if (f[0] * f[0] + f[1] * f[1]).sqrt() < 1e-12 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
        let r = unit(cross(f, up));
        let u = cross(r, f);
        Ok(View { eye: self.eye, f, r, u, t: (self.fov / 2.0).tan(), w: self.width, h: self.height })
    }
}

impl View {
    /// CSS px of a point in front of the camera; `None` behind it (depth ≤ 0).
    fn project(&self, p: V3) -> Option<[f64; 2]> {
        let d = sub(p, self.eye);
        let z = dot(d, self.f);
        if !(z > 0.0) {
            return None;
        }
        let x = dot(d, self.r) / (z * self.t * self.w / self.h);
        let y = dot(d, self.u) / (z * self.t);
        Some([(x + 1.0) / 2.0 * self.w, (1.0 - y) / 2.0 * self.h])
    }
    /// Unit direction of the ray through CSS px `c`.
    fn ray(&self, c: [f64; 2]) -> V3 {
        let x = 2.0 * c[0] / self.w - 1.0;
        let y = 1.0 - 2.0 * c[1] / self.h;
        unit(add(self.f, add(mul(self.r, x * self.t * self.w / self.h), mul(self.u, y * self.t))))
    }
}

/// Closest point on the line `p0 + s·d` (s clamped to `[lo, hi]`) to the ray `e + t·v` (t ≥ 0).
fn closest_on_line(e: V3, v: V3, p0: V3, d: V3, lo: f64, hi: f64) -> V3 {
    let w0 = sub(p0, e);
    let (a, b, c) = (dot(d, d), dot(d, v), dot(v, v));
    if a == 0.0 {
        return p0;
    }
    let (dd, ee) = (dot(d, w0), dot(v, w0));
    let den = a * c - b * b;
    let mut s = if den.abs() <= 1e-18 * a * c { 0.0 } else { (b * ee - c * dd) / den };
    s = s.clamp(lo, hi);
    // the convex problem in (s, t ≥ 0): re-optimise t for the clamped s, then s for that t
    let t = (dot(sub(add(p0, mul(d, s)), e), v) / c).max(0.0);
    s = (dot(sub(add(e, mul(v, t)), p0), d) / a).clamp(lo, hi);
    add(p0, mul(d, s))
}

// ------------------------------------------------------------------ scene

/// One world-space edge of the model (spec: an `{entity, index}` of a Shape's edge).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneEdge {
    pub entity: String,
    pub index: u64,
    pub a: V3,
    pub b: V3,
}

/// One world-space face: loops of points, the first the outer boundary, the others holes.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneFace {
    pub entity: String,
    pub id: u64,
    pub loops: Vec<Vec<V3>>,
}

/// A guide point (no `direction`) or guide line (through `point` along `direction`).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneGuide {
    pub entity: String,
    pub point: V3,
    pub direction: Option<V3>,
}

/// What [`InferScene::infer`] is asked: a camera, the cursor (CSS px from the top-left), the tool's
/// last point and the last edge the cursor hovered.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub camera: Camera,
    pub cursor: [f64; 2],
    pub from: Option<V3>,
    pub reference_edge: Option<[V3; 2]>,
}

/// The kind of an inference (spec/sketch.md), in priority-line order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Endpoint,
    Midpoint,
    Intersection,
    OnEdge,
    Guide,
    AxisX,
    AxisY,
    AxisZ,
    Parallel,
    Perpendicular,
    OnFace,
    Ground,
    None,
}

impl Kind {
    /// The kind's name in the JSON result.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Endpoint => "endpoint",
            Kind::Midpoint => "midpoint",
            Kind::Intersection => "intersection",
            Kind::OnEdge => "on_edge",
            Kind::Guide => "guide",
            Kind::AxisX => "axis_x",
            Kind::AxisY => "axis_y",
            Kind::AxisZ => "axis_z",
            Kind::Parallel => "parallel",
            Kind::Perpendicular => "perpendicular",
            Kind::OnFace => "on_face",
            Kind::Ground => "ground",
            Kind::None => "none",
        }
    }
}

/// What an inference refers to: `ref` of the JSON result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    /// An edge: `{"entity", "index"}`.
    Edge { entity: String, index: u64 },
    /// A face: `{"entity", "id"}`.
    Face { entity: String, id: u64 },
    /// A guide: `{"entity"}`.
    Guide { entity: String },
}

/// The answer: the point (None for [`Kind::None`]), its kind and what it refers to.
#[derive(Debug, Clone, PartialEq)]
pub struct Inference {
    pub point: Option<V3>,
    pub kind: Kind,
    pub reference: Option<Ref>,
}

impl Inference {
    /// The result as JSON: `{"point": [x, y, z] | null, "kind": …, "ref": … | null}`.
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\"point\":");
        match self.point {
            Some(p) => {
                s.push('[');
                for (i, x) in p.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    // finite by construction; -0 prints as 0
                    let x = if *x == 0.0 { 0.0 } else { *x };
                    s.push_str(&format!("{x}"));
                }
                s.push(']');
            }
            None => s.push_str("null"),
        }
        s.push_str(",\"kind\":\"");
        s.push_str(self.kind.name());
        s.push_str("\",\"ref\":");
        match &self.reference {
            None => s.push_str("null"),
            Some(Ref::Edge { entity, index }) => {
                s.push_str("{\"entity\":");
                push_str_json(&mut s, entity);
                s.push_str(&format!(",\"index\":{index}}}"));
            }
            Some(Ref::Face { entity, id }) => {
                s.push_str("{\"entity\":");
                push_str_json(&mut s, entity);
                s.push_str(&format!(",\"id\":{id}}}"));
            }
            Some(Ref::Guide { entity }) => {
                s.push_str("{\"entity\":");
                push_str_json(&mut s, entity);
                s.push('}');
            }
        }
        s.push('}');
        s
    }
}

fn push_str_json(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ------------------------------------------------------------------ bounding-volume hierarchy

#[derive(Debug, Clone, Copy)]
struct Aabb {
    lo: V3,
    hi: V3,
}

impl Aabb {
    fn empty() -> Aabb {
        Aabb { lo: [f64::INFINITY; 3], hi: [f64::NEG_INFINITY; 3] }
    }
    fn of_points<'a>(pts: impl IntoIterator<Item = &'a V3>) -> Aabb {
        let mut b = Aabb::empty();
        for p in pts {
            b.add_point(*p);
        }
        b
    }
    fn add_point(&mut self, p: V3) {
        for k in 0..3 {
            self.lo[k] = self.lo[k].min(p[k]);
            self.hi[k] = self.hi[k].max(p[k]);
        }
    }
    fn union(&mut self, o: &Aabb) {
        self.add_point(o.lo);
        self.add_point(o.hi);
    }
    fn centre(&self) -> V3 {
        mul(add(self.lo, self.hi), 0.5)
    }
    fn radius(&self) -> f64 {
        0.5 * dist(self.lo, self.hi)
    }
    /// Whether the ray `e + t·v` (t > 0) meets the box (slab test).
    fn hit_by_ray(&self, e: V3, v: V3) -> bool {
        let (mut t0, mut t1) = (0.0f64, f64::INFINITY);
        for k in 0..3 {
            if v[k] == 0.0 {
                if e[k] < self.lo[k] || e[k] > self.hi[k] {
                    return false;
                }
                continue;
            }
            let inv = 1.0 / v[k];
            let (mut a, mut b) = ((self.lo[k] - e[k]) * inv, (self.hi[k] - e[k]) * inv);
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            t0 = t0.max(a);
            t1 = t1.min(b);
            if t0 > t1 {
                return false;
            }
        }
        true
    }
}

/// A node of a [`Bvh`]: a leaf holds `items[start..start + count]`; an inner node (count 0) has its
/// children at `left` and `left + 1`.
#[derive(Debug, Clone)]
struct Node {
    bbox: Aabb,
    centre: V3,
    radius: f64,
    left: usize,
    start: usize,
    count: usize,
}

/// A bounding-volume hierarchy over item boxes, built by median splits on the longest axis.
#[derive(Debug, Clone, Default)]
struct Bvh {
    nodes: Vec<Node>,
    /// Item indices in leaf order.
    items: Vec<usize>,
}

impl Bvh {
    fn build(boxes: &[Aabb]) -> Bvh {
        let mut bvh = Bvh { nodes: Vec::new(), items: (0..boxes.len()).collect() };
        if boxes.is_empty() {
            return bvh;
        }
        let centres: Vec<V3> = boxes.iter().map(Aabb::centre).collect();
        bvh.nodes.push(Node { bbox: Aabb::empty(), centre: [0.0; 3], radius: 0.0, left: 0, start: 0, count: boxes.len() });
        // iterative build: a stack of node indices still to split
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let (start, count) = (bvh.nodes[ni].start, bvh.nodes[ni].count);
            let slice = &mut bvh.items[start..start + count];
            let mut bb = Aabb::empty();
            let mut cb = Aabb::empty();
            for &i in slice.iter() {
                bb.union(&boxes[i]);
                cb.add_point(centres[i]);
            }
            let node = &mut bvh.nodes[ni];
            node.bbox = bb;
            node.centre = bb.centre();
            node.radius = bb.radius();
            if count <= LEAF {
                continue;
            }
            let ext = sub(cb.hi, cb.lo);
            let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 } else if ext[1] >= ext[2] { 1 } else { 2 };
            let mid = count / 2;
            slice.select_nth_unstable_by(mid, |&a, &b| {
                centres[a][axis].total_cmp(&centres[b][axis]).then(a.cmp(&b))
            });
            let left = bvh.nodes.len();
            let mk = |start, count| Node { bbox: Aabb::empty(), centre: [0.0; 3], radius: 0.0, left: 0, start, count };
            bvh.nodes.push(mk(start, mid));
            bvh.nodes.push(mk(start + mid, count - mid));
            let node = &mut bvh.nodes[ni];
            node.left = left;
            node.count = 0;
            stack.push(left);
            stack.push(left + 1);
        }
        bvh
    }

    /// Calls `visit` for every item in a leaf whose node `keep` accepts (and whose ancestors it accepted).
    fn walk(&self, keep: impl Fn(&Node) -> bool, mut visit: impl FnMut(usize)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni];
            if !keep(n) {
                continue;
            }
            if n.count > 0 {
                for &i in &self.items[n.start..n.start + n.count] {
                    visit(i);
                }
            } else {
                stack.push(n.left);
                stack.push(n.left + 1);
            }
        }
    }
}

// ------------------------------------------------------------------ prepared faces

/// A face prepared for ray hits: its plane and its loops in 2D (the plane's dominant-axis projection).
#[derive(Debug, Clone)]
struct Prepared {
    normal: V3,
    offset: f64,
    drop: usize,
    loops: Vec<Vec<[f64; 2]>>,
}

impl Prepared {
    fn new(f: &SceneFace) -> Option<Prepared> {
        let outer = f.loops.first()?;
        // Newell's normal of the outer loop
        let mut n = [0.0; 3];
        for i in 0..outer.len() {
            let (p, q) = (outer[i], outer[(i + 1) % outer.len()]);
            n[0] += (p[1] - q[1]) * (p[2] + q[2]);
            n[1] += (p[2] - q[2]) * (p[0] + q[0]);
            n[2] += (p[0] - q[0]) * (p[1] + q[1]);
        }
        let l = len(n);
        if !(l > 0.0) || !l.is_finite() {
            return None;
        }
        let normal = mul(n, 1.0 / l);
        let c = mul(outer.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / outer.len() as f64);
        let a = [normal[0].abs(), normal[1].abs(), normal[2].abs()];
        let drop = if a[0] >= a[1] && a[0] >= a[2] { 0 } else if a[1] >= a[2] { 1 } else { 2 };
        let loops = f.loops.iter().map(|lp| lp.iter().map(|p| flat(*p, drop)).collect()).collect();
        Some(Prepared { normal, offset: dot(normal, c), drop, loops })
    }

    /// Distance along the ray `e + t·v` to the face, when the ray meets it (inside the outer loop and
    /// outside every hole: the even-odd rule over all loops).
    fn hit(&self, e: V3, v: V3) -> Option<f64> {
        let den = dot(self.normal, v);
        if den.abs() < 1e-15 {
            return None;
        }
        let t = (self.offset - dot(self.normal, e)) / den;
        if !(t > 0.0) {
            return None;
        }
        let q = flat(add(e, mul(v, t)), self.drop);
        let mut inside = false;
        for lp in &self.loops {
            let n = lp.len();
            for i in 0..n {
                let (a, b) = (lp[i], lp[(i + 1) % n]);
                if (a[1] > q[1]) != (b[1] > q[1]) {
                    let x = a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                    if q[0] < x {
                        inside = !inside;
                    }
                }
            }
        }
        inside.then_some(t)
    }
}

fn flat(p: V3, drop: usize) -> [f64; 2] {
    match drop {
        0 => [p[1], p[2]],
        1 => [p[2], p[0]],
        _ => [p[0], p[1]],
    }
}

// ------------------------------------------------------------------ the scene and its queries

/// A parsed scene with its spatial indexes, ready for many queries.
#[derive(Debug, Clone)]
pub struct InferScene {
    edges: Vec<SceneEdge>,
    faces: Vec<SceneFace>,
    guides: Vec<SceneGuide>,
    level_elevation: f64,
    edge_bvh: Bvh,
    prepared: Vec<Option<Prepared>>,
    face_bvh: Bvh,
}

/// One candidate: its screen distance and the keys of the spec's tie-breaks.
#[derive(Debug, Clone)]
struct Cand {
    px: f64,
    entity: String,
    index: u64,
    kind: Kind,
    point: V3,
    reference: Option<Ref>,
}

impl Cand {
    fn better_than(&self, o: &Cand) -> bool {
        self.px
            .total_cmp(&o.px)
            .then_with(|| self.entity.as_bytes().cmp(o.entity.as_bytes()))
            .then(self.index.cmp(&o.index))
            .then(self.kind.cmp(&o.kind))
            .is_lt()
    }
}

/// The best candidate of one priority line.
#[derive(Default)]
struct Best(Option<Cand>);

impl Best {
    fn offer(&mut self, c: Cand) {
        if self.0.as_ref().is_none_or(|b| c.better_than(b)) {
            self.0 = Some(c);
        }
    }
}

impl InferScene {
    /// Parses a scene (`{"edges": [{"entity", "index", "a", "b"}], "faces": [{"entity", "id", "loops"}],
    /// "guides": [{"entity", "point", "direction"?}], "level_elevation"}`) and builds its indexes.
    pub fn new(scene: &str) -> Result<InferScene, SketchError> {
        let root = json::parse(scene).map_err(|e| SketchError(format!("scene: {e}")))?;
        let members = object(&root, "scene")?;
        let mut edges = Vec::new();
        let mut faces = Vec::new();
        let mut guides = Vec::new();
        let mut level_elevation = 0.0;
        let mut seen: Vec<&str> = Vec::new();
        for (k, v) in members {
            if seen.contains(&k.as_str()) {
                return Err(SketchError(format!("scene: duplicate key `{k}`")));
            }
            seen.push(k);
            match k.as_str() {
                "edges" => {
                    for (i, e) in array(v, "scene.edges")?.iter().enumerate() {
                        edges.push(parse_edge(e, &format!("scene.edges[{i}]"))?);
                    }
                }
                "faces" => {
                    for (i, f) in array(v, "scene.faces")?.iter().enumerate() {
                        faces.push(parse_face(f, &format!("scene.faces[{i}]"))?);
                    }
                }
                "guides" => {
                    for (i, g) in array(v, "scene.guides")?.iter().enumerate() {
                        guides.push(parse_guide(g, &format!("scene.guides[{i}]"))?);
                    }
                }
                "level_elevation" => level_elevation = num(v, "scene.level_elevation")?,
                other => return Err(SketchError(format!("scene: unknown key `{other}`"))),
            }
        }
        Ok(InferScene::from_parts(edges, faces, guides, level_elevation))
    }

    /// A scene from already-parsed parts.
    pub fn from_parts(edges: Vec<SceneEdge>, faces: Vec<SceneFace>, guides: Vec<SceneGuide>, level_elevation: f64) -> InferScene {
        let edge_boxes: Vec<Aabb> = edges.iter().map(|e| Aabb::of_points([&e.a, &e.b])).collect();
        let edge_bvh = Bvh::build(&edge_boxes);
        let prepared: Vec<Option<Prepared>> = faces.iter().map(Prepared::new).collect();
        let face_boxes: Vec<Aabb> = faces.iter().map(|f| Aabb::of_points(f.loops.iter().flatten())).collect();
        let face_bvh = Bvh::build(&face_boxes);
        InferScene { edges, faces, guides, level_elevation, edge_bvh, prepared, face_bvh }
    }

    /// Answers a query given as JSON (`{"camera": {"eye", "target", "fov", "width", "height"},
    /// "cursor": [x, y], "from"?, "reference_edge"?}`) with the result as JSON.
    pub fn infer_json(&self, query: &str) -> Result<String, SketchError> {
        let q = parse_query(query)?;
        Ok(self.infer(&q)?.to_json())
    }

    /// Answers a query (spec/sketch.md "## Inference").
    pub fn infer(&self, q: &Query) -> Result<Inference, SketchError> {
        self.infer_over(q, false)
    }

    /// [`InferScene::infer`]; with `brute` every edge is tested (no culling), which tests use to
    /// check the index.
    fn infer_over(&self, q: &Query, brute: bool) -> Result<Inference, SketchError> {
        let view = q.camera.view()?;
        let c = q.cursor;
        if !(c[0].is_finite() && c[1].is_finite()) {
            return Err(SketchError("query: `cursor` must be finite".into()));
        }
        let e = view.eye;
        let v = view.ray(c);
        let px_of = |p: V3| -> Option<f64> {
            let s = view.project(p)?;
            let d = ((s[0] - c[0]).powi(2) + (s[1] - c[1]).powi(2)).sqrt();
            (d <= RADIUS_PX).then_some(d)
        };

        // Edges whose points can project within the radius: inside the pick cone around the ray.
        let k = 2.0 * RADIUS_PX * view.t / view.h;
        // a little slack so rounding never culls a point exactly on the radius
        let k = k * (1.0 + 1e-9);
        let in_cone = |centre: V3, radius: f64| -> bool {
            let w = sub(centre, e);
            let z = dot(w, view.f);
            if z + radius <= 0.0 {
                return false;
            }
            let along = dot(w, v);
            let perp = len(sub(w, mul(v, along)));
            perp - radius <= k * (z + radius) + 1e-12
        };
        let mut near: Vec<usize> = Vec::new();
        self.edge_bvh.walk(
            |n| in_cone(n.centre, n.radius),
            |i| {
                let ed = &self.edges[i];
                if in_cone(mul(add(ed.a, ed.b), 0.5), 0.5 * dist(ed.a, ed.b)) {
                    near.push(i);
                }
            },
        );
        if brute {
            near = (0..self.edges.len()).collect();
        }
        // deterministic order whatever the hierarchy's layout
        near.sort_unstable();

        // Line 1: endpoints, midpoints, intersections.
        let mut line1 = Best::default();
        for &i in &near {
            let ed = &self.edges[i];
            let mid = add(ed.a, mul(sub(ed.b, ed.a), 0.5));
            for (kind, p) in [(Kind::Endpoint, ed.a), (Kind::Endpoint, ed.b), (Kind::Midpoint, mid)] {
                if let Some(px) = px_of(p) {
                    line1.offer(self.edge_cand(i, kind, p, px));
                }
            }
        }
        for (x, &i) in near.iter().enumerate() {
            for &j in &near[x + 1..] {
                if let Some(p) = self.intersection(i, j) {
                    if let Some(px) = px_of(p.0) {
                        line1.offer(self.edge_cand(p.1, Kind::Intersection, p.0, px));
                    }
                }
            }
        }
        if let Some(b) = line1.0 {
            return Ok(b.into_inference());
        }

        // Line 2: on edges, guides.
        let mut line2 = Best::default();
        for &i in &near {
            let ed = &self.edges[i];
            let p = closest_on_line(e, v, ed.a, sub(ed.b, ed.a), 0.0, 1.0);
            if let Some(px) = px_of(p) {
                line2.offer(self.edge_cand(i, Kind::OnEdge, p, px));
            }
        }
        for g in &self.guides {
            let p = match g.direction {
                None => g.point,
                Some(d) => closest_on_line(e, v, g.point, d, f64::NEG_INFINITY, f64::INFINITY),
            };
            if let Some(px) = px_of(p) {
                line2.offer(Cand {
                    px,
                    entity: g.entity.clone(),
                    index: 0,
                    kind: Kind::Guide,
                    point: p,
                    reference: Some(Ref::Guide { entity: g.entity.clone() }),
                });
            }
        }
        if let Some(b) = line2.0 {
            return Ok(b.into_inference());
        }

        let line_cand = |from: V3, d: V3, kind: Kind, best: &mut Best| {
            let p = closest_on_line(e, v, from, d, f64::NEG_INFINITY, f64::INFINITY);
            if let Some(px) = px_of(p) {
                best.offer(Cand { px, entity: String::new(), index: 0, kind, point: p, reference: None });
            }
        };

        // Line 3: the axes through `from`.
        if let Some(from) = q.from {
            let mut line3 = Best::default();
            line_cand(from, [1.0, 0.0, 0.0], Kind::AxisX, &mut line3);
            line_cand(from, [0.0, 1.0, 0.0], Kind::AxisY, &mut line3);
            line_cand(from, [0.0, 0.0, 1.0], Kind::AxisZ, &mut line3);
            if let Some(b) = line3.0 {
                return Ok(b.into_inference());
            }
        }

        // Line 4: parallel and perpendicular to the reference edge, through `from`.
        if let (Some(from), Some([ra, rb])) = (q.from, q.reference_edge) {
            let d = sub(rb, ra);
            if len(d) > 0.0 {
                let du = unit(d);
                let mut line4 = Best::default();
                line_cand(from, du, Kind::Parallel, &mut line4);
                line_cand(from, perpendicular(from, ra, du), Kind::Perpendicular, &mut line4);
                if let Some(b) = line4.0 {
                    return Ok(b.into_inference());
                }
            }
        }

        // Line 5: the nearest face under the cursor, then the ground.
        let mut hit: Option<(f64, usize)> = None;
        self.face_bvh.walk(
            |n| n.bbox.hit_by_ray(e, v),
            |i| {
                if let Some(t) = self.prepared[i].as_ref().and_then(|p| p.hit(e, v)) {
                    let better = match hit {
                        None => true,
                        Some((bt, bi)) => {
                            t < bt || (t == bt && face_key(&self.faces[i]) < face_key(&self.faces[bi]))
                        }
                    };
                    if better {
                        hit = Some((t, i));
                    }
                }
            },
        );
        if let Some((t, i)) = hit {
            let f = &self.faces[i];
            return Ok(Inference {
                point: Some(add(e, mul(v, t))),
                kind: Kind::OnFace,
                reference: Some(Ref::Face { entity: f.entity.clone(), id: f.id }),
            });
        }
        if v[2] != 0.0 {
            let t = (self.level_elevation - e[2]) / v[2];
            if t > 0.0 && t.is_finite() {
                let mut p = add(e, mul(v, t));
                p[2] = self.level_elevation;
                return Ok(Inference { point: Some(p), kind: Kind::Ground, reference: None });
            }
        }
        Ok(Inference { point: None, kind: Kind::None, reference: None })
    }

    fn edge_cand(&self, i: usize, kind: Kind, point: V3, px: f64) -> Cand {
        let ed = &self.edges[i];
        Cand {
            px,
            entity: ed.entity.clone(),
            index: ed.index,
            kind,
            point,
            reference: Some(Ref::Edge { entity: ed.entity.clone(), index: ed.index }),
        }
    }

    /// Where edges `i` and `j` cross (within [`INTERSECTION_TOL`] in 3D), with the edge the result
    /// refers to (the lower entity id, then index). The crossing must be interior to both edges: edges
    /// that only share an endpoint, an edge ending on another, and parallel edges do not intersect.
    fn intersection(&self, i: usize, j: usize) -> Option<(V3, usize)> {
        let (p, q) = (&self.edges[i], &self.edges[j]);
        for x in [p.a, p.b] {
            for y in [q.a, q.b] {
                if dist(x, y) <= POINT_TOL {
                    return None;
                }
            }
        }
        let (d1, d2) = (sub(p.b, p.a), sub(q.b, q.a));
        let r = sub(p.a, q.a);
        let (a, b, c) = (dot(d1, d1), dot(d1, d2), dot(d2, d2));
        let (d, f) = (dot(d1, r), dot(d2, r));
        let den = a * c - b * b;
        if !(a > 0.0 && c > 0.0) || den <= 1e-12 * a * c {
            return None;
        }
        let s = (b * f - c * d) / den;
        let t = (a * f - b * d) / den;
        if !((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t)) {
            return None;
        }
        let (x, y) = (add(p.a, mul(d1, s)), add(q.a, mul(d2, t)));
        if dist(x, y) > INTERSECTION_TOL {
            return None;
        }
        // a crossing is interior to both edges: an edge ending on another (a T) touches, not crosses
        if [p.a, p.b, q.a, q.b].iter().any(|&end| dist(x, end) <= POINT_TOL || dist(y, end) <= POINT_TOL) {
            return None;
        }
        let pk = (p.entity.as_bytes(), p.index);
        let qk = (q.entity.as_bytes(), q.index);
        Some(if pk <= qk { (x, i) } else { (y, j) })
    }
}

fn face_key(f: &SceneFace) -> (&[u8], u64) {
    (f.entity.as_bytes(), f.id)
}

/// The direction of the line through `from` perpendicular to the line `a + s·d` (unit `d`) in the
/// plane containing both: towards the foot of the perpendicular, or, when `from` is on the line,
/// the horizontal perpendicular `d × ẑ` (for a vertical `d`, x̂).
fn perpendicular(from: V3, a: V3, d: V3) -> V3 {
    let foot = add(a, mul(d, dot(sub(from, a), d)));
    let to = sub(foot, from);
    if len(to) > POINT_TOL {
        return unit(to);
    }
    let h = cross(d, [0.0, 0.0, 1.0]);
    if len(h) > 1e-12 {
        unit(h)
    } else {
        [1.0, 0.0, 0.0]
    }
}

impl Cand {
    fn into_inference(self) -> Inference {
        Inference { point: Some(self.point), kind: self.kind, reference: self.reference }
    }
}

/// [`InferScene::new`]`(scene)?.`[`infer_json`](InferScene::infer_json)`(query)`.
pub fn infer_json(scene: &str, query: &str) -> Result<String, SketchError> {
    InferScene::new(scene)?.infer_json(query)
}

// ------------------------------------------------------------------ JSON input

fn object<'a>(v: &'a Json, at: &str) -> Result<&'a [(String, Json)], SketchError> {
    match v {
        Json::Object(m) => Ok(m),
        other => Err(SketchError(format!("{at}: expected an object, got {}", other.type_name()))),
    }
}

fn array<'a>(v: &'a Json, at: &str) -> Result<&'a [Json], SketchError> {
    match v {
        Json::Array(a) => Ok(a),
        other => Err(SketchError(format!("{at}: expected an array, got {}", other.type_name()))),
    }
}

fn num(v: &Json, at: &str) -> Result<f64, SketchError> {
    match v {
        Json::Num(x) if x.is_finite() => Ok(*x),
        other => Err(SketchError(format!("{at}: expected a number, got {}", other.type_name()))),
    }
}

fn text(v: &Json, at: &str) -> Result<String, SketchError> {
    match v {
        Json::Str(s) => Ok(s.clone()),
        other => Err(SketchError(format!("{at}: expected a string, got {}", other.type_name()))),
    }
}

fn count(v: &Json, at: &str) -> Result<u64, SketchError> {
    let x = num(v, at)?;
    if x >= 0.0 && x.fract() == 0.0 && x <= 9_007_199_254_740_992.0 {
        Ok(x as u64)
    } else {
        Err(SketchError(format!("{at}: expected a non-negative integer")))
    }
}

fn vec3(v: &Json, at: &str) -> Result<V3, SketchError> {
    let a = array(v, at)?;
    if a.len() != 3 {
        return Err(SketchError(format!("{at}: expected 3 numbers")));
    }
    Ok([num(&a[0], at)?, num(&a[1], at)?, num(&a[2], at)?])
}

/// The members of an object by key: each of `keys` at most once, no other key, `required` present.
fn fields<'a>(v: &'a Json, at: &str, keys: &[&str], required: &[&str]) -> Result<Vec<Option<&'a Json>>, SketchError> {
    let mut out = vec![None; keys.len()];
    for (k, x) in object(v, at)? {
        let Some(i) = keys.iter().position(|w| w == k) else {
            return Err(SketchError(format!("{at}: unknown key `{k}`")));
        };
        if out[i].is_some() {
            return Err(SketchError(format!("{at}: duplicate key `{k}`")));
        }
        out[i] = Some(x);
    }
    for r in required {
        let i = keys.iter().position(|w| w == r).expect("required keys are keys");
        if out[i].is_none() {
            return Err(SketchError(format!("{at}: missing `{r}`")));
        }
    }
    Ok(out)
}

fn parse_edge(v: &Json, at: &str) -> Result<SceneEdge, SketchError> {
    let keys = ["entity", "index", "a", "b"];
    let f = fields(v, at, &keys, &keys)?;
    let get = |i: usize| f[i].expect("required");
    Ok(SceneEdge {
        entity: text(get(0), &format!("{at}.entity"))?,
        index: count(get(1), &format!("{at}.index"))?,
        a: vec3(get(2), &format!("{at}.a"))?,
        b: vec3(get(3), &format!("{at}.b"))?,
    })
}

fn parse_face(v: &Json, at: &str) -> Result<SceneFace, SketchError> {
    let keys = ["entity", "id", "loops"];
    let f = fields(v, at, &keys, &keys)?;
    let get = |i: usize| f[i].expect("required");
    let mut loops = Vec::new();
    for (li, lp) in array(get(2), &format!("{at}.loops"))?.iter().enumerate() {
        let lat = format!("{at}.loops[{li}]");
        let pts = array(lp, &lat)?.iter().map(|p| vec3(p, &lat)).collect::<Result<Vec<_>, _>>()?;
        if pts.len() < 3 {
            return Err(SketchError(format!("{lat}: a loop needs at least 3 points")));
        }
        loops.push(pts);
    }
    if loops.is_empty() {
        return Err(SketchError(format!("{at}.loops: a face needs an outer loop")));
    }
    Ok(SceneFace { entity: text(get(0), &format!("{at}.entity"))?, id: count(get(1), &format!("{at}.id"))?, loops })
}

fn parse_guide(v: &Json, at: &str) -> Result<SceneGuide, SketchError> {
    let f = fields(v, at, &["entity", "point", "direction"], &["entity", "point"])?;
    let direction = match f[2] {
        None | Some(Json::Null) => None,
        Some(d) => {
            let d = vec3(d, &format!("{at}.direction"))?;
            if !(len(d) > 0.0) {
                return Err(SketchError(format!("{at}.direction: must not be zero")));
            }
            Some(d)
        }
    };
    Ok(SceneGuide {
        entity: text(f[0].expect("required"), &format!("{at}.entity"))?,
        point: vec3(f[1].expect("required"), &format!("{at}.point"))?,
        direction,
    })
}

/// Parses a query: `{"camera": {"eye", "target", "fov", "width", "height"}, "cursor": [x, y],
/// "from"?: [x, y, z], "reference_edge"?: [[x, y, z], [x, y, z]]}`.
pub fn parse_query(query: &str) -> Result<Query, SketchError> {
    let root = json::parse(query).map_err(|e| SketchError(format!("query: {e}")))?;
    let f = fields(&root, "query", &["camera", "cursor", "from", "reference_edge"], &["camera", "cursor"])?;
    let ck = ["eye", "target", "fov", "width", "height"];
    let cf = fields(f[0].expect("required"), "query.camera", &ck, &ck)?;
    let camera = Camera {
        eye: vec3(cf[0].expect("required"), "query.camera.eye")?,
        target: vec3(cf[1].expect("required"), "query.camera.target")?,
        fov: num(cf[2].expect("required"), "query.camera.fov")?,
        width: num(cf[3].expect("required"), "query.camera.width")?,
        height: num(cf[4].expect("required"), "query.camera.height")?,
    };
    camera.view()?;
    let cur = array(f[1].expect("required"), "query.cursor")?;
    if cur.len() != 2 {
        return Err(SketchError("query.cursor: expected 2 numbers".into()));
    }
    let cursor = [num(&cur[0], "query.cursor")?, num(&cur[1], "query.cursor")?];
    let from = match f[2] {
        None | Some(Json::Null) => None,
        Some(x) => Some(vec3(x, "query.from")?),
    };
    let reference_edge = match f[3] {
        None | Some(Json::Null) => None,
        Some(x) => {
            let a = array(x, "query.reference_edge")?;
            if a.len() != 2 {
                return Err(SketchError("query.reference_edge: expected 2 points".into()));
            }
            Some([vec3(&a[0], "query.reference_edge")?, vec3(&a[1], "query.reference_edge")?])
        }
    };
    Ok(Query { camera, cursor, from, reference_edge })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        Camera { eye: [12.0, -9.0, 7.0], target: [0.0, 0.0, 0.0], fov: 0.8, width: 1200.0, height: 800.0 }
    }

    struct Rng(u64);
    impl Rng {
        fn f(&mut self, lo: f64, hi: f64) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            lo + (hi - lo) * ((self.0 >> 11) as f64 / (1u64 << 53) as f64)
        }
    }

    #[test]
    fn index_agrees_with_brute_force() {
        let mut rng = Rng(12345);
        let mut edges = Vec::new();
        for i in 0..300 {
            let a = [rng.f(-8.0, 8.0), rng.f(-8.0, 8.0), rng.f(-1.0, 3.0)];
            let b = add(a, [rng.f(-2.0, 2.0), rng.f(-2.0, 2.0), rng.f(-1.0, 1.0)]);
            edges.push(SceneEdge { entity: format!("e{}", i % 7), index: i, a, b });
        }
        // a few exact crossings and shared endpoints
        edges.push(SceneEdge { entity: "x".into(), index: 0, a: [-3.0, 0.0, 0.0], b: [1.0, 0.0, 0.0] });
        edges.push(SceneEdge { entity: "x".into(), index: 1, a: [0.0, -1.0, 0.0], b: [0.0, 4.0, 0.0] });
        let scene = InferScene::from_parts(edges, Vec::new(), Vec::new(), 0.0);
        let mut differ = 0;
        for _ in 0..300 {
            let q = Query { camera: cam(), cursor: [rng.f(0.0, 1200.0), rng.f(0.0, 800.0)], from: None, reference_edge: None };
            let (a, b) = (scene.infer_over(&q, false).unwrap(), scene.infer_over(&q, true).unwrap());
            if a != b {
                differ += 1;
            }
        }
        assert_eq!(differ, 0);
    }

    #[test]
    fn behind_the_camera_is_no_candidate() {
        // an endpoint behind the eye whose mirror image would project onto the cursor
        let c = cam();
        let behind = sub(mul(c.eye, 2.0), [1.0, 1.0, 1.0]);
        let scene = InferScene::from_parts(
            vec![SceneEdge { entity: "b".into(), index: 0, a: behind, b: add(behind, [0.0, 0.0, 0.5]) }],
            Vec::new(),
            Vec::new(),
            -100.0,
        );
        let view = c.view().unwrap();
        let px = view.project([1.0, 1.0, 1.0]).unwrap();
        let r = scene.infer(&Query { camera: c, cursor: px, from: None, reference_edge: None }).unwrap();
        assert_eq!(r.kind, Kind::Ground);
    }

    #[test]
    fn t_junction_and_json_output() {
        let c = cam();
        let edges = vec![
            SceneEdge { entity: "a".into(), index: 0, a: [-3.0, 0.0, 0.0], b: [3.0, 0.0, 0.0] },
            SceneEdge { entity: "b\"q".into(), index: 5, a: [1.0, 0.0, 0.0], b: [1.0, 4.0, 0.0] },
        ];
        let scene = InferScene::from_parts(edges, Vec::new(), Vec::new(), 0.0);
        let px = c.view().unwrap().project([1.0, 0.0, 0.0]).unwrap();
        let r = scene.infer(&Query { camera: c, cursor: [px[0] + 2.0, px[1]], from: None, reference_edge: None }).unwrap();
        // the endpoint of b lies on a's interior: a T touches but does not cross, so b's endpoint wins
        assert_eq!(r.kind, Kind::Endpoint);
        assert_eq!(r.reference, Some(Ref::Edge { entity: "b\"q".into(), index: 5 }));
        let out = Inference { point: Some([1.0, -0.0, 2.5]), kind: Kind::Endpoint, reference: Some(Ref::Edge { entity: "b\"q".into(), index: 5 }) };
        assert_eq!(out.to_json(), r#"{"point":[1,0,2.5],"kind":"endpoint","ref":{"entity":"b\"q","index":5}}"#);
    }

    #[test]
    fn invalid_queries_and_scenes() {
        let s = InferScene::new(r#"{"edges":[],"faces":[],"guides":[],"level_elevation":0}"#).unwrap();
        let ok = r#"{"camera":{"eye":[1,1,1],"target":[0,0,0],"fov":0.8,"width":100,"height":100},"cursor":[50,50]}"#;
        assert!(s.infer_json(ok).is_ok());
        for bad in [
            r#"{"camera":{"eye":[0,0,0],"target":[0,0,0],"fov":0.8,"width":100,"height":100},"cursor":[50,50]}"#,
            r#"{"camera":{"eye":[1,1,1],"target":[0,0,0],"fov":4,"width":100,"height":100},"cursor":[50,50]}"#,
            r#"{"camera":{"eye":[1,1,1],"target":[0,0,0],"fov":0.8,"width":0,"height":100},"cursor":[50,50]}"#,
            r#"{"camera":{"eye":[1,1,1],"target":[0,0,0],"fov":0.8,"width":100,"height":100}}"#,
            r#"{"camera":{"eye":[1,1,1],"target":[0,0,0],"fov":0.8,"width":100,"height":100},"cursor":[50]}"#,
            "[]",
        ] {
            assert!(s.infer_json(bad).is_err(), "{bad}");
        }
        for bad in [r#"{"edges":[{"entity":"a","index":-1,"a":[0,0,0],"b":[1,0,0]}]}"#, r#"{"edges":{}}"#, r#"{"bogus":1}"#, "{"] {
            assert!(InferScene::new(bad).is_err(), "{bad}");
        }
    }
}
