// External judge for T2.12 (bim-sketch inference and measurements, spec/sketch.md and TASKS.md T2.12).
// The judge projects points itself with the pinhole camera of spec/sketch.md (eye, target, up +z, vertical
// fov, viewport in CSS px from the top-left) so every cursor is placed exactly, and computes every expected
// point independently (closest points between the cursor ray and lines/segments, ray-plane hits).
// Only the pinned signatures are used: `InferScene::new(&str) -> Result<InferScene, SketchError>`,
// `InferScene::infer_json(&self, &str) -> Result<String, SketchError>`, `infer_json(&str, &str) -> Result<String, SketchError>`,
// `parse_measurement(&str, &str, bool) -> Result<Measurement, SketchError>` and the `Measurement` variants.
use bim_sketch::{infer_json, parse_measurement, InferScene, Measurement};
use serde_json::{json, Value};
use std::sync::Once;
use std::time::{Duration, Instant};

type V3 = [f64; 3];

fn watchdog() {
    static START: Once = Once::new();
    START.call_once(|| {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(150));
            // write to the raw stderr handle: the test harness captures eprintln! output
            use std::io::Write;
            let _ = std::io::stderr().write_all(b"JUDGE FAIL: T2.12 judge timed out after 150 s (hang or far too slow)\n");
            std::process::exit(1);
        });
    });
}

fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn mul(a: V3, s: f64) -> V3 { [a[0] * s, a[1] * s, a[2] * s] }
fn dot(a: V3, b: V3) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn len(a: V3) -> f64 { dot(a, a).sqrt() }
fn unit(a: V3) -> V3 { mul(a, 1.0 / len(a)) }
fn dist(a: V3, b: V3) -> f64 { len(sub(a, b)) }
fn lerp(a: V3, b: V3, t: f64) -> V3 { add(a, mul(sub(b, a), t)) }

/// Pinhole camera of spec/sketch.md: up is +z, `fov` is vertical, CSS px measured from the top-left corner.
#[derive(Clone, Copy)]
struct Cam { eye: V3, target: V3, fov: f64, w: f64, h: f64 }

impl Cam {
    fn frame(&self) -> (V3, V3, V3) {
        let f = unit(sub(self.target, self.eye));
        // spec/sketch.md: up is +z, or +y when the view direction is parallel to z
        let up = if (f[0] * f[0] + f[1] * f[1]).sqrt() < 1e-12 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
        let r = unit(cross(f, up));
        let u = cross(r, f);
        (f, r, u)
    }
    fn depth(&self, p: V3) -> f64 { dot(sub(p, self.eye), self.frame().0) }
    fn project(&self, p: V3) -> [f64; 2] {
        let (f, r, u) = self.frame();
        let d = sub(p, self.eye);
        let z = dot(d, f);
        let t = (self.fov / 2.0).tan();
        let x = dot(d, r) / (z * t * self.w / self.h);
        let y = dot(d, u) / (z * t);
        [(x + 1.0) / 2.0 * self.w, (1.0 - y) / 2.0 * self.h]
    }
    /// Unit direction of the ray through cursor pixel (px, py).
    fn ray(&self, c: [f64; 2]) -> V3 {
        let (f, r, u) = self.frame();
        let t = (self.fov / 2.0).tan();
        let x = 2.0 * c[0] / self.w - 1.0;
        let y = 1.0 - 2.0 * c[1] / self.h;
        unit(add(f, add(mul(r, x * t * self.w / self.h), mul(u, y * t))))
    }
    fn json(&self) -> Value {
        json!({"eye": self.eye, "target": self.target, "fov": self.fov, "width": self.w, "height": self.h})
    }
}

fn px_dist(a: [f64; 2], b: [f64; 2]) -> f64 { ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() }
fn off(c: [f64; 2], dx: f64, dy: f64) -> [f64; 2] { [c[0] + dx, c[1] + dy] }

/// Closest point on the line/segment p0 + s*d (s clamped to [lo, hi]) to the ray e + t*r (t >= 0).
fn closest_on_line(e: V3, r: V3, p0: V3, d: V3, lo: f64, hi: f64) -> V3 {
    let w0 = sub(p0, e);
    let (a, b, c) = (dot(d, d), dot(d, r), dot(r, r));
    let (dd, ee) = (dot(d, w0), dot(r, w0));
    let den = a * c - b * b;
    let mut s = if den.abs() < 1e-18 { 0.0 } else { (b * ee - c * dd) / den };
    s = s.clamp(lo, hi);
    // re-optimise t for the clamped s, then s for that t (convex problem: two steps suffice here).
    // re-optimise s for the ray point nearest the clamped one (t >= 0 holds in every scenario here)
    let t = (dot(sub(add(p0, mul(d, s)), e), r) / c).max(0.0);
    s = (dot(sub(add(e, mul(r, t)), p0), d) / a).clamp(lo, hi);
    add(p0, mul(d, s))
}

fn seg_point(cam: &Cam, c: [f64; 2], a: V3, b: V3) -> V3 {
    closest_on_line(cam.eye, cam.ray(c), a, sub(b, a), 0.0, 1.0)
}
fn line_point(cam: &Cam, c: [f64; 2], p: V3, d: V3) -> V3 {
    closest_on_line(cam.eye, cam.ray(c), p, d, -1e12, 1e12)
}
/// Spec distance of a candidate: its projection to the cursor, in CSS px.
fn cand_px(cam: &Cam, c: [f64; 2], p: V3) -> f64 { px_dist(cam.project(p), c) }
fn plane_hit(cam: &Cam, c: [f64; 2], z: f64) -> Option<V3> {
    let r = cam.ray(c);
    if r[2].abs() < 1e-15 { return None; }
    let t = (z - cam.eye[2]) / r[2];
    if t <= 0.0 { None } else { Some(add(cam.eye, mul(r, t))) }
}

// ------------------------------------------------------------------ scenes and queries

#[derive(Default, Clone)]
struct Scene { edges: Vec<Value>, faces: Vec<Value>, guides: Vec<Value>, level: f64 }

impl Scene {
    fn edge(mut self, entity: &str, index: u32, a: V3, b: V3) -> Self {
        self.edges.push(json!({"entity": entity, "index": index, "a": a, "b": b}));
        self
    }
    fn face(mut self, entity: &str, id: u32, loops: Vec<Vec<V3>>) -> Self {
        self.faces.push(json!({"entity": entity, "id": id, "loops": loops}));
        self
    }
    fn guide(mut self, entity: &str, point: V3, direction: Option<V3>) -> Self {
        let mut g = json!({"entity": entity, "point": point});
        if let Some(d) = direction { g["direction"] = json!(d); }
        self.guides.push(g);
        self
    }
    fn level(mut self, z: f64) -> Self { self.level = z; self }
    fn json(&self) -> String {
        json!({"edges": self.edges, "faces": self.faces, "guides": self.guides, "level_elevation": self.level})
            .to_string()
    }
}

struct Ans { point: Option<V3>, kind: String, r#ref: Value }

fn infer(scene: &Scene, cam: &Cam, c: [f64; 2], from: Option<V3>, reference: Option<[V3; 2]>, what: &str) -> Ans {
    let mut q = json!({"camera": cam.json(), "cursor": c});
    if let Some(f) = from { q["from"] = json!(f); }
    if let Some(r) = reference { q["reference_edge"] = json!(r); }
    let sj = scene.json();
    let idx = match InferScene::new(&sj) {
        Ok(x) => x,
        Err(_) => panic!("{what}: InferScene::new returned Err for a valid scene"),
    };
    let out = match idx.infer_json(&q.to_string()) {
        Ok(s) => s,
        Err(_) => panic!("{what}: InferScene::infer_json returned Err for a valid scene and query"),
    };
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{what}: result is not JSON ({e}): {out}"));
    // TASKS.md: infer_json(scene, query) equals InferScene::new(scene)?.infer_json(query)
    match infer_json(&sj, &q.to_string()) {
        Ok(o2) => {
            let v2: Value = serde_json::from_str(&o2).unwrap_or_else(|e| panic!("{what}: infer_json result is not JSON ({e})"));
            assert_eq!(v2, v, "{what}: infer_json(scene, query) differs from InferScene::infer_json");
        }
        Err(_) => panic!("{what}: infer_json returned Err for a valid scene and query"),
    }
    let kind = v.get("kind").and_then(|k| k.as_str())
        .unwrap_or_else(|| panic!("{what}: result has no string `kind`: {out}")).to_string();
    let point = match v.get("point") {
        Some(Value::Null) => None,
        Some(Value::Array(a)) if a.len() == 3 && a.iter().all(|x| x.is_f64() || x.is_i64() || x.is_u64()) =>
            Some([a[0].as_f64().unwrap(), a[1].as_f64().unwrap(), a[2].as_f64().unwrap()]),
        _ => panic!("{what}: `point` must be [x,y,z] or null: {out}"),
    };
    let r = v.get("ref").cloned().unwrap_or_else(|| panic!("{what}: result has no `ref` key: {out}"));
    check_ref_shape(&kind, &r, what);
    if kind == "none" { assert!(point.is_none(), "{what}: kind none must have a null point: {out}"); }
    Ans { point, kind, r#ref: r }
}

/// spec/sketch.md: ref is {"entity","index"} for edge kinds, {"entity","id"} for on_face, {"entity"} for guide,
/// and null for axis, parallel, perpendicular, ground and none.
fn check_ref_shape(kind: &str, r: &Value, what: &str) {
    let keys: Option<Vec<&str>> = match kind {
        "endpoint" | "midpoint" | "intersection" | "on_edge" => Some(vec!["entity", "index"]),
        "on_face" => Some(vec!["entity", "id"]),
        "guide" => Some(vec!["entity"]),
        "axis_x" | "axis_y" | "axis_z" | "parallel" | "perpendicular" | "ground" | "none" => None,
        other => panic!("{what}: unknown kind {other:?}"),
    };
    match keys {
        None => assert!(r.is_null(), "{what}: ref for kind {kind} must be null, got {r}"),
        Some(ks) => {
            let o = r.as_object().unwrap_or_else(|| panic!("{what}: ref for kind {kind} must be an object, got {r}"));
            let mut got: Vec<&str> = o.keys().map(|k| k.as_str()).collect();
            got.sort();
            let mut want = ks.clone();
            want.sort();
            assert_eq!(got, want, "{what}: ref keys for kind {kind}: {r}");
            assert!(o["entity"].is_string(), "{what}: ref entity must be a string: {r}");
            for k in ["index", "id"] {
                if let Some(x) = o.get(k) { assert!(x.is_u64() || x.as_f64().map(|f| f >= 0.0 && f.fract() == 0.0).unwrap_or(false), "{what}: ref {k} must be a non-negative integer: {r}"); }
            }
        }
    }
}

fn expect(a: &Ans, kind: &str, p: V3, tol: f64, what: &str) {
    assert_eq!(a.kind, kind, "{what}: kind");
    let got = a.point.unwrap_or_else(|| panic!("{what}: point is null for kind {kind}"));
    assert!(dist(got, p) <= tol, "{what}: point {got:?}, want {p:?} (tolerance {tol} m)");
}

fn expect_ref(a: &Ans, entity: &str, key: &str, val: u64, what: &str) {
    let e = a.r#ref.get("entity").and_then(|x| x.as_str());
    let k = a.r#ref.get(key).and_then(|x| x.as_f64());
    assert!(e == Some(entity) && k == Some(val as f64),
        "{what}: ref {}, want {{\"entity\": \"{entity}\", \"{key}\": {val}}}", a.r#ref);
}

/// Scenario premise checks of the judge itself (not the code under test).
fn premise(ok: bool, msg: &str) {
    assert!(ok, "judge scenario error: {msg}");
}

fn cam_a() -> Cam { Cam { eye: [12.0, -9.0, 7.0], target: [0.0, 0.0, 0.0], fov: 0.8, w: 1200.0, h: 800.0 } }
/// Horizontal camera at z = 5: the target projects to the exact centre and rays through the middle row
/// never meet the ground z = 0.
fn cam_h() -> Cam { Cam { eye: [0.0, -10.0, 5.0], target: [0.0, 0.0, 5.0], fov: 0.9, w: 1000.0, h: 700.0 } }

const PT: f64 = 1e-9;   // exact candidate points (endpoints, midpoints, intersections, guide points)
const RAY: f64 = 2e-5;  // points computed from the cursor ray (1e-6 of the ~20 m viewing distance)

// ------------------------------------------------------------------ kinds

#[test]
fn endpoint_and_radius_boundary() {
    watchdog();
    let cam = cam_a();
    let (a, b) = ([-2.0, 1.0, 0.0], [3.0, 2.0, 0.5]);
    let s = Scene::default().edge("a", 0, a, b);
    premise(px_dist(cam.project(b), cam.project(lerp(a, b, 0.5))) > 40.0, "edge too short on screen");
    let c = off(cam.project(b), 5.0, -3.0);
    let r = infer(&s, &cam, c, None, None, "endpoint");
    expect(&r, "endpoint", b, PT, "endpoint");
    expect_ref(&r, "a", "index", 0, "endpoint");

    // The 10 CSS px radius: the endpoint sits at `target`, which projects to the exact centre.
    let cam = cam_h();
    let p = [0.0, 0.0, 5.0];
    let q = [0.0, 0.0, 7.0];
    let s = Scene::default().edge("v", 3, p, q).level(0.0);
    let ctr = cam.project(p);
    premise(px_dist(ctr, [500.0, 350.0]) < 1e-9, "target should project to the centre");
    for (dx, dy) in [(9.5, 0.0), (-9.5, 0.0), (0.0, 9.5)] {
        let r = infer(&s, &cam, off(ctr, dx, dy), None, None, "9.5 px from an endpoint");
        expect(&r, "endpoint", p, PT, &format!("cursor 9.5 px from the endpoint ({dx}, {dy})"));
        expect_ref(&r, "v", "index", 3, "endpoint ref");
    }
    for (dx, dy) in [(10.5, 0.0), (-10.5, 0.0)] {
        // the endpoint and the edge's nearest point (the same point) are out of the radius; what remains
        // (none, or a far ground hit on the horizon row) is not asserted here
        let r = infer(&s, &cam, off(ctr, dx, dy), None, None, "10.5 px beside an endpoint");
        assert!(r.kind != "endpoint" && r.kind != "on_edge",
            "cursor 10.5 px beside the endpoint ({dx}, {dy}): kind {} (outside the 10 px radius)", r.kind);
    }
    // 10.5 px below: out of the endpoint's radius, the ray meets the ground.
    let c = off(ctr, 0.0, 10.5);
    let g = plane_hit(&cam, c, 0.0).unwrap();
    let r = infer(&s, &cam, c, None, None, "10.5 px below an endpoint");
    expect(&r, "ground", g, 1e-6 * dist(cam.eye, g), "10.5 px below the endpoint -> ground");
    // 30 px above: on the edge (the ray meets the vertical edge).
    let c = off(ctr, 0.0, -30.0);
    let e = seg_point(&cam, c, p, q);
    premise(cand_px(&cam, c, e) < 1e-6 && cand_px(&cam, c, [0.0, 0.0, 6.0]) > 20.0, "on_edge premise");
    let r = infer(&s, &cam, c, None, None, "on_edge");
    expect(&r, "on_edge", e, RAY, "30 px up the edge -> on_edge");
    expect_ref(&r, "v", "index", 3, "on_edge ref");
}

#[test]
fn midpoint_and_intersection() {
    watchdog();
    let cam = cam_a();
    let (a, b) = ([-3.0, -1.0, 0.0], [1.0, 3.0, 1.0]);
    let m = lerp(a, b, 0.5);
    let s = Scene::default().edge("m", 2, a, b);
    premise(px_dist(cam.project(a), cam.project(m)) > 40.0, "edge too short on screen");
    let r = infer(&s, &cam, off(cam.project(m), 4.0, 4.0), None, None, "midpoint");
    expect(&r, "midpoint", m, PT, "midpoint");
    expect_ref(&r, "m", "index", 2, "midpoint");

    // Two edges crossing at the origin (in 3D): intersection.
    let s = Scene::default().edge("p", 0, [-3.0, 0.0, 0.0], [1.0, 0.0, 0.0]).edge("q", 0, [0.0, -1.0, 0.0], [0.0, 4.0, 0.0]);
    let o = [0.0, 0.0, 0.0];
    for e in [[-3.0, 0.0, 0.0], [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 4.0, 0.0], [0.0, 1.5, 0.0]] {
        premise(px_dist(cam.project(e), cam.project(o)) > 25.0, "ends and midpoints far from the crossing");
    }
    let r = infer(&s, &cam, off(cam.project(o), 6.0, -2.0), None, None, "intersection");
    expect(&r, "intersection", o, PT, "intersection");
    let ent = r.r#ref.get("entity").and_then(|x| x.as_str());
    assert!(ent == Some("p") || ent == Some("q"), "intersection ref should name one of the crossing edges: {}", r.r#ref);

    // The same edges 0.1 m apart in z cross on screen only: no intersection, the nearer edge wins on_edge.
    let (q0, q1) = ([0.0, -1.0, 0.1], [0.0, 4.0, 0.1]);
    let s = Scene::default().edge("p", 0, [-3.0, 0.0, 0.0], [1.0, 0.0, 0.0]).edge("q", 0, q0, q1);
    // cursor on p's projected line, 6 px from where the projections cross
    let (pa, pb) = (cam.project([-3.0, 0.0, 0.0]), cam.project([1.0, 0.0, 0.0]));
    let (qa, qb) = (cam.project(q0), cam.project(q1));
    let x = {
        let d1 = [pb[0] - pa[0], pb[1] - pa[1]];
        let d2 = [qb[0] - qa[0], qb[1] - qa[1]];
        let den = d1[0] * d2[1] - d1[1] * d2[0];
        let t = ((qa[0] - pa[0]) * d2[1] - (qa[1] - pa[1]) * d2[0]) / den;
        [pa[0] + t * d1[0], pa[1] + t * d1[1]]
    };
    let l = ((pb[0] - pa[0]).powi(2) + (pb[1] - pa[1]).powi(2)).sqrt();
    let c = [x[0] + 6.0 * (pb[0] - pa[0]) / l, x[1] + 6.0 * (pb[1] - pa[1]) / l];
    let ep = seg_point(&cam, c, [-3.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    let eq = seg_point(&cam, c, q0, q1);
    premise(cand_px(&cam, c, ep) < 1e-6 && cand_px(&cam, c, eq) > 1.0 && cand_px(&cam, c, eq) < 10.0, "skew premise");
    let r = infer(&s, &cam, c, None, None, "skew crossing");
    expect(&r, "on_edge", ep, RAY, "edges crossing on screen but 0.1 m apart -> on_edge (nearest)");
    expect_ref(&r, "p", "index", 0, "skew crossing ref");
}

#[test]
fn guides() {
    watchdog();
    let cam = cam_a();
    let gp = [1.0, 1.0, 1.0];
    let s = Scene::default().guide("g", gp, None);
    let r = infer(&s, &cam, off(cam.project(gp), 3.0, 3.0), None, None, "guide point");
    expect(&r, "guide", gp, PT, "guide point");
    assert_eq!(r.r#ref.get("entity").and_then(|x| x.as_str()), Some("g"), "guide ref entity: {}", r.r#ref);

    let (p, d) = ([0.0, 0.0, 2.0], [1.0, 1.0, 0.0]);
    let s = Scene::default().guide("gl", p, Some(d));
    let on = cam.project(add(p, mul(d, 2.0)));
    let dir = { let o = cam.project(add(p, mul(d, 2.1))); let l = px_dist(o, on); [(o[0] - on[0]) / l, (o[1] - on[1]) / l] };
    let c = off(on, -4.0 * dir[1], 4.0 * dir[0]);
    let e = line_point(&cam, c, p, d);
    premise(cand_px(&cam, c, e) < 10.0, "guide line premise");
    let r = infer(&s, &cam, c, None, None, "guide line");
    expect(&r, "guide", e, RAY, "guide line: closest point on the line to the cursor ray");
}

/// Unit screen direction of the projected line p + s d at parameter s.
fn screen_dir(cam: &Cam, p: V3, d: V3, s: f64) -> [f64; 2] {
    let a = cam.project(add(p, mul(d, s)));
    let b = cam.project(add(p, mul(d, s + 1e-3)));
    let l = px_dist(a, b);
    [(b[0] - a[0]) / l, (b[1] - a[1]) / l]
}

#[test]
fn axes_parallel_perpendicular() {
    watchdog();
    let cam = cam_a();
    let f = [1.0, 1.0, 0.0];
    let s = Scene::default().level(-5.0);
    let axes = [("axis_x", [1.0, 0.0, 0.0]), ("axis_y", [0.0, 1.0, 0.0]), ("axis_z", [0.0, 0.0, 1.0])];
    for (k, (kind, d)) in axes.iter().enumerate() {
        let on = cam.project(add(f, mul(*d, 3.0)));
        let sd = screen_dir(&cam, f, *d, 3.0);
        let c = off(on, -4.0 * sd[1], 4.0 * sd[0]);
        let e = line_point(&cam, c, f, *d);
        premise(cand_px(&cam, c, e) < 6.0, "axis premise");
        for (j, (_, d2)) in axes.iter().enumerate() {
            if j != k { premise(cand_px(&cam, c, line_point(&cam, c, f, *d2)) > 20.0, "other axes far"); }
        }
        let r = infer(&s, &cam, c, Some(f), None, kind);
        expect(&r, kind, e, RAY, &format!("{kind}: closest point on the axis through `from` to the cursor ray"));
    }

    // parallel to the reference edge, from `from`
    let re = [[5.0, 5.0, 0.0], [7.0, 6.0, 0.0]];
    let d = unit(sub(re[1], re[0]));
    let on = cam.project(add(f, mul(d, 3.0)));
    let sd = screen_dir(&cam, f, d, 3.0);
    let c = off(on, 3.0 * sd[1], -3.0 * sd[0]);
    let e = line_point(&cam, c, f, d);
    premise(cand_px(&cam, c, e) < 5.0, "parallel premise");
    for (_, a) in axes.iter() { premise(cand_px(&cam, c, line_point(&cam, c, f, *a)) > 15.0, "axes far from parallel"); }
    let r = infer(&s, &cam, c, Some(f), Some(re), "parallel");
    expect(&r, "parallel", e, RAY, "parallel: closest point on the line through `from` along the reference edge");

    // perpendicular to the reference edge, from `from` (horizontal, in the plane of `from` and the edge)
    // reference edge along (0.8, -0.6, 0); `from` lies 3 m off its line, so the perpendicular through `from`
    // runs along (0.6, 0.8, 0) towards the foot of the perpendicular (across the view of camera A).
    // `from` is also 1.5 m above the edge's plane, so the perpendicular (in the plane containing `from` and the
    // edge) is not horizontal: it runs from `from` to the foot of the perpendicular on the edge's line.
    let re = [[5.0, 5.0, 0.0], [6.6, 3.8, 0.0]];
    let f2 = add(sub([5.0, 5.0, 0.0], mul([0.6, 0.8, 0.0], 3.0)), [0.0, 0.0, 1.5]);
    let pd = unit(sub([5.0, 5.0, 0.0], f2));
    let dre = unit(sub(re[1], re[0]));
    let c = find_cursor(&cam, f2, pd, 3.0, |c| {
        axes.iter().all(|(_, a)| cand_px(&cam, c, line_point(&cam, c, f2, *a)) > 15.0)
            && cand_px(&cam, c, line_point(&cam, c, f2, dre)) > 15.0
    }).expect("judge scenario error: no cursor near the perpendicular with the axes and parallel far");
    let r = infer(&s, &cam, c, Some(f2), Some(re), "perpendicular");
    assert_eq!(r.kind, "perpendicular", "perpendicular: kind");
    let p = r.point.expect("perpendicular: point is null");
    assert!(dot(sub(p, f2), unit(sub(re[1], re[0]))).abs() < 1e-6, "perpendicular: point {p:?} - from is not perpendicular to the reference edge");
    expect(&r, "perpendicular", line_point(&cam, c, f2, pd), RAY,
        "perpendicular: closest point on the line through `from` towards the foot of the perpendicular");

    // `from` on the reference edge's line: the horizontal perpendicular.
    let pd = [0.6, 0.8, 0.0];
    let f3 = [5.8, 4.4, 0.0]; // on the line through (5,5,0) along (0.8,-0.6,0)
    premise(dot(sub(f3, re[0]), [0.6, 0.8, 0.0]).abs() < 1e-12, "f3 on the reference line");
    let on = cam.project(add(f3, mul(pd, 2.0)));
    let sd = screen_dir(&cam, f3, pd, 2.0);
    let c = off(on, 3.0 * sd[1], -3.0 * sd[0]);
    for (_, a) in axes.iter() { premise(cand_px(&cam, c, line_point(&cam, c, f3, *a)) > 15.0, "axes far from perpendicular (on line)"); }
    premise(cand_px(&cam, c, line_point(&cam, c, f3, unit(sub(re[1], re[0])))) > 15.0, "parallel far (on line)");
    let r = infer(&s, &cam, c, Some(f3), Some(re), "perpendicular from a point on the edge");
    expect(&r, "perpendicular", line_point(&cam, c, f3, pd), RAY,
        "perpendicular from a point on the edge's line: the horizontal perpendicular");
}

#[test]
fn faces_ground_none() {
    watchdog();
    let cam = cam_a();
    let sq = |z: f64, a: f64, b: f64| vec![[a, a, z], [b, a, z], [b, b, z], [a, b, z]];
    let s = Scene::default().face("w", 3, vec![sq(1.0, -1.0, 2.0)]).face("w2", 1, vec![sq(0.0, -1.0, 2.0)]).level(0.25);
    let target = [0.5, 0.3, 1.0];
    let c = cam.project(target);
    let r = infer(&s, &cam, c, None, None, "on_face");
    expect(&r, "on_face", target, RAY, "on_face: nearest face hit by the cursor ray");
    expect_ref(&r, "w", "id", 3, "on_face ref");
    // outside both faces: the ground plane at the level elevation
    let c = cam.project([-4.0, 3.0, 0.25]);
    let r = infer(&s, &cam, c, None, None, "ground");
    expect(&r, "ground", [-4.0, 3.0, 0.25], RAY, "ground: hit on z = level_elevation");
    // through a hole: falls through to the ground
    let hole = vec![[-1.0, -1.0, 2.0], [-1.0, 1.0, 2.0], [1.0, 1.0, 2.0], [1.0, -1.0, 2.0]];
    let s = Scene::default().face("h", 5, vec![sq(2.0, -3.0, 3.0), hole]).level(0.0);
    let g = plane_hit(&cam, cam.project([0.0, 0.0, 2.0]), 0.0).unwrap();
    let r = infer(&s, &cam, cam.project([0.0, 0.0, 2.0]), None, None, "through a hole");
    expect(&r, "ground", g, RAY, "cursor over a face's hole -> ground");
    let r = infer(&s, &cam, cam.project([2.0, -2.5, 2.0]), None, None, "on the ring");
    expect(&r, "on_face", [2.0, -2.5, 2.0], RAY, "cursor over the ring face");
    expect_ref(&r, "h", "id", 5, "ring face ref");
    // nothing: above the horizon of the horizontal camera
    let cam = cam_h();
    let s = Scene::default().edge("v", 0, [0.0, 0.0, 5.0], [0.0, 0.0, 7.0]).level(0.0);
    let r = infer(&s, &cam, [100.0, 50.0], None, None, "sky");
    assert_eq!(r.kind, "none", "sky: kind");
    assert!(r.point.is_none(), "kind none: point must be null");
}

#[test]
fn priority_across_tiers() {
    watchdog();
    let cam = cam_a();
    // endpoint (7 px) beats on_edge of another edge (2 px)
    let p = [2.0, -1.0, 0.0];
    let pp = cam.project(p);
    let c = off(pp, 7.0, 0.0);
    // second edge through a point 2 px from the cursor, perpendicular-ish on screen, far from its own ends
    let r0 = cam.ray(off(c, 0.0, 2.0));
    // on z = 0.3 under that pixel: skew to e1 (z = 0), so the two edges have no 3D intersection
    let x = add(cam.eye, mul(r0, (0.3 - cam.eye[2]) / r0[2]));
    let (q0, q1) = (sub(x, [1.5, 1.5, 0.0]), add(x, [3.0, 3.0, 0.0]));
    let s = Scene::default().edge("e1", 0, p, [-2.0, -3.0, 0.0]).edge("e2", 0, q0, q1).level(-1.0);
    premise(cand_px(&cam, c, seg_point(&cam, c, q0, q1)) < 3.0, "on_edge nearer than the endpoint");
    premise(px_dist(cam.project(q0), c) > 20.0 && px_dist(cam.project(q1), c) > 20.0
        && px_dist(cam.project(x), c) < 3.0 && px_dist(cam.project(lerp(p, [-2.0, -3.0, 0.0], 0.5)), c) > 20.0
        && px_dist(cam.project(lerp(q0, q1, 0.5)), c) > 20.0, "tier premise");
    let r = infer(&s, &cam, c, None, None, "endpoint vs on_edge");
    expect(&r, "endpoint", p, PT, "endpoint (tier 1) beats a nearer on_edge (tier 2)");

    // on_edge beats a nearer axis_x
    let f = [0.0, 0.0, 0.0];
    let on = cam.project([3.0, 0.0, 0.0]);
    let sd = screen_dir(&cam, f, [1.0, 0.0, 0.0], 3.0);
    let c = off(on, -1.0 * sd[1], 1.0 * sd[0]); // 1 px from the x axis
    let nrm = [-sd[1], sd[0]];
    let ray_at = |px: [f64; 2]| plane_hit(&cam, px, 0.0).unwrap();
    let a1 = ray_at(off(c, 6.0 * nrm[0] + 40.0 * sd[0], 6.0 * nrm[1] + 40.0 * sd[1]));
    let a2 = ray_at(off(c, 6.0 * nrm[0] - 80.0 * sd[0], 6.0 * nrm[1] - 80.0 * sd[1])); // midpoint ~20 px away
    let s = Scene::default().edge("near", 0, a1, a2).level(-1.0);
    let e = seg_point(&cam, c, a1, a2);
    premise(cand_px(&cam, c, e) > 4.0 && cand_px(&cam, c, e) < 8.0, "edge 6 px away");
    premise(cand_px(&cam, c, lerp(a1, a2, 0.5)) > 15.0, "the edge's midpoint is far from the cursor");
    premise(cand_px(&cam, c, line_point(&cam, c, f, [1.0, 0.0, 0.0])) < 2.0, "axis 1 px away");
    let r = infer(&s, &cam, c, Some(f), None, "on_edge vs axis");
    expect(&r, "on_edge", e, RAY, "on_edge (tier 2) beats a nearer axis_x (tier 3)");

    // axis_x beats a nearer parallel line (reference edge slightly off the x direction)
    let d = unit([1.0, 0.08, 0.0]);
    let re = [[0.0, 10.0, 0.0], add([0.0, 10.0, 0.0], mul(d, 2.0))];
    let mut s_par = 0.0;
    let mut cur = [0.0, 0.0];
    for i in 1..400 {
        let t = i as f64 * 0.05;
        let c = cam.project(add(f, mul(d, t)));
        let ax = cand_px(&cam, c, line_point(&cam, c, f, [1.0, 0.0, 0.0]));
        if ax > 5.0 && ax < 8.0 { s_par = t; cur = c; break; }
    }
    premise(s_par > 0.0, "found a cursor on the parallel line 5-8 px from the x axis");
    for a in [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] { premise(cand_px(&cam, cur, line_point(&cam, cur, f, a)) > 15.0, "y/z axes far"); }
    let s = Scene::default().level(-1.0);
    let ax = line_point(&cam, cur, f, [1.0, 0.0, 0.0]);
    let r = infer(&s, &cam, cur, Some(f), Some(re), "axis vs parallel");
    expect(&r, "axis_x", ax, RAY, "axis_x (tier 3) beats a nearer parallel (tier 4)");

    // parallel beats on_face under the cursor
    let d = unit([1.0, 2.0, 0.0]);
    let re = [[5.0, -5.0, 0.0], add([5.0, -5.0, 0.0], mul(d, 2.0))];
    let c = cam.project(add(f, mul(d, 1.5)));
    for a in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] { premise(cand_px(&cam, c, line_point(&cam, c, f, a)) > 15.0, "axes far"); }
    let s = Scene::default().face("fl", 1, vec![vec![[-9.0, -9.0, 0.0], [9.0, -9.0, 0.0], [9.0, 9.0, 0.0], [-9.0, 9.0, 0.0]]]).level(-1.0);
    let r = infer(&s, &cam, c, Some(f), Some(re), "parallel vs on_face");
    expect(&r, "parallel", line_point(&cam, c, f, d), RAY, "parallel (tier 4) beats on_face (tier 5)");

    // guide (tier 2) beats axis (tier 3)
    let gp = [3.0, 0.0, 0.0];
    let c = off(cam.project(gp), 0.0, 5.0);
    let s = Scene::default().guide("g", gp, None).level(-1.0);
    let r = infer(&s, &cam, c, Some(f), None, "guide vs axis");
    expect(&r, "guide", gp, PT, "guide (tier 2) beats axis_x (tier 3)");
}

#[test]
fn tie_breaks() {
    watchdog();
    let cam = cam_a();
    // nearest on screen wins within a kind, before the entity id
    let pa = [0.0, 0.0, 0.0];
    let pb = [0.0, 0.0, 12.0 / px_dist(cam.project(pa), cam.project([0.0, 0.0, 1.0]))]; // ~12 px above pa
    let s = Scene::default().edge("a", 0, pa, [-3.0, 2.0, 0.0]).edge("b", 0, pb, [3.0, 3.0, 2.0]);
    let (sa, sb) = (cam.project(pa), cam.project(pb));
    let l = px_dist(sa, sb);
    premise(l > 11.0 && l < 13.0, "the two endpoints are about 12 px apart on screen");
    // cursor on the segment between them, 4 px from b's endpoint
    let c = [sb[0] + (sa[0] - sb[0]) * 4.0 / l, sb[1] + (sa[1] - sb[1]) * 4.0 / l];
    premise(cand_px(&cam, c, pa) < 10.0, "both endpoints are candidates");
    let r = infer(&s, &cam, c, None, None, "nearest endpoint");
    expect(&r, "endpoint", pb, PT, "the nearer endpoint wins (entity b, although a is lower)");
    expect_ref(&r, "b", "index", 0, "nearest endpoint ref");
    // equal distance: lowest entity id, then lowest index
    let p = [1.0, -2.0, 0.0];
    let s = Scene::default().edge("b", 0, p, [4.0, -2.0, 0.0]).edge("a", 4, p, [1.0, -5.0, 0.0]).edge("a", 9, p, [-2.0, -2.0, 1.0]);
    // Edges that only share an endpoint are not an intersection (spec/sketch.md).
    let r = infer(&s, &cam, off(cam.project(p), 3.0, 2.0), None, None, "tie on entity");
    assert_eq!(r.kind, "endpoint", "edges that only share an endpoint are not an intersection: kind");
    assert!(dist(r.point.expect("point"), p) <= PT, "shared endpoint: point");
    expect_ref(&r, "a", "index", 4, "equal distance: lowest entity id, then lowest index");
    let s = Scene::default().edge("c", 7, p, [4.0, -2.0, 0.0]).edge("c", 2, p, [1.0, -5.0, 0.0]);
    let r = infer(&s, &cam, off(cam.project(p), 3.0, 2.0), None, None, "tie on index");
    assert_eq!(r.kind, "endpoint", "edges that only share an endpoint are not an intersection: kind");
    expect_ref(&r, "c", "index", 2, "equal distance and entity: lowest index");
}

/// A cursor `offset` px beside the projected line p + s d (s scanned outwards from p) accepted by `ok`.
fn find_cursor(cam: &Cam, p: V3, d: V3, offset: f64, ok: impl Fn([f64; 2]) -> bool) -> Option<[f64; 2]> {
    for i in 1..3000 {
        for sgn in [1.0, -1.0] {
            let t = sgn * i as f64 * 0.005;
            let q = add(p, mul(d, t));
            if cam.depth(q) < 0.5 { continue; }
            let on = cam.project(q);
            let sd = screen_dir(cam, p, d, t);
            let c = off(on, offset * sd[1], -offset * sd[0]);
            if c[0] < 0.0 || c[0] > cam.w || c[1] < 0.0 || c[1] > cam.h { continue; }
            if ok(c) { return Some(c); }
        }
    }
    None
}

/// Unit screen-space normal (perpendicular) to the projected segment a-b.
fn screen_normal(cam: &Cam, a: V3, b: V3) -> [f64; 2] {
    let (p, q) = (cam.project(a), cam.project(b));
    let l = px_dist(p, q);
    [-(q[1] - p[1]) / l, (q[0] - p[0]) / l]
}

#[test]
fn same_line_nearest_wins() {
    watchdog();
    let cam = cam_a();
    // line 1: endpoint vs midpoint of a short edge (16 px on screen): the nearer one wins either way
    let a = [1.0, -1.0, 0.0];
    let dir = unit([1.0, 1.0, 0.0]);
    let k = 16.0 / px_dist(cam.project(a), cam.project(add(a, dir)));
    let b = add(a, mul(dir, k));
    let m = lerp(a, b, 0.5);
    premise((px_dist(cam.project(a), cam.project(b)) - 16.0).abs() < 0.5, "short edge 16 px");
    let n = screen_normal(&cam, a, b);
    let s = Scene::default().edge("s", 1, a, b).level(-1.0);
    let c = off(cam.project(m), 2.0 * n[0], 2.0 * n[1]);
    premise(cand_px(&cam, c, a) < 10.0 && cand_px(&cam, c, b) < 10.0, "endpoints are candidates too");
    let r = infer(&s, &cam, c, None, None, "midpoint nearer than endpoints");
    expect(&r, "midpoint", m, PT, "same line: the nearer midpoint beats the endpoints");
    let c = off(cam.project(a), 2.0 * n[0], 2.0 * n[1]);
    premise(cand_px(&cam, c, m) < 10.0, "midpoint is a candidate too");
    let r = infer(&s, &cam, c, None, None, "endpoint nearer than midpoint");
    expect(&r, "endpoint", a, PT, "same line: the nearer endpoint beats the midpoint");

    // line 1: intersection vs a third edge's endpoint 8 px away
    let o = [0.0, 0.0, 0.0];
    let po = cam.project(o);
    let n2 = screen_normal(&cam, [-3.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    // a point on z = 0.2 under the pixel 8 px from the crossing, off both crossing edges' screen lines
    let pe = off(po, -8.0 * (0.6 * n2[0] + 0.8 * n2[1]), -8.0 * (0.6 * n2[1] - 0.8 * n2[0]));
    let re_ = cam.ray(pe);
    let e = add(cam.eye, mul(re_, (0.2 - cam.eye[2]) / re_[2]));
    let far = add(e, [0.0, 0.0, 3.0]);
    let s = Scene::default().edge("p", 0, [-3.0, 0.0, 0.0], [1.0, 0.0, 0.0]).edge("q", 0, [0.0, -1.0, 0.0], [0.0, 4.0, 0.0])
        .edge("e", 0, e, far).level(-1.0);
    premise((cand_px(&cam, po, e) - 8.0).abs() < 1e-6, "third endpoint 8 px from the crossing");
    let toward = [(pe[0] - po[0]) / 8.0, (pe[1] - po[1]) / 8.0];
    let c = off(po, -2.0 * toward[0], -2.0 * toward[1]); // 2 px from the crossing, 10 from e... move closer:
    let c = off(c, 3.0 * toward[0], 3.0 * toward[1]);    // 1 px past the crossing towards e: 1 px vs 7 px
    premise(cand_px(&cam, c, e) < 10.0 && cand_px(&cam, c, o) < 2.0, "both candidates");
    let r = infer(&s, &cam, c, None, None, "intersection nearer than an endpoint");
    expect(&r, "intersection", o, PT, "same line: the nearer intersection beats an endpoint");
    let c = off(po, 6.5 * toward[0], 6.5 * toward[1]); // 1.5 px from e, 6.5 px from the crossing
    let r = infer(&s, &cam, c, None, None, "endpoint nearer than an intersection");
    expect(&r, "endpoint", e, PT, "same line: the nearer endpoint beats an intersection");
    expect_ref(&r, "e", "index", 0, "endpoint ref");

    // line 2: on_edge vs guide
    let (ea, eb) = ([-4.0, 2.0, 0.0], [3.0, 3.0, 0.0]);
    let mid = cam.project(lerp(ea, eb, 0.3));
    let nn = screen_normal(&cam, ea, eb);
    let gpx = off(mid, 5.0 * nn[0], 5.0 * nn[1]);
    let rg = cam.ray(gpx);
    let gp = add(cam.eye, mul(rg, (0.5 - cam.eye[2]) / rg[2]));
    let s = Scene::default().edge("ed", 0, ea, eb).guide("gd", gp, None).level(-1.0);
    let c = off(mid, 1.0 * nn[0], 1.0 * nn[1]); // ~1 px from the edge, 4 px from the guide point
    let r = infer(&s, &cam, c, None, None, "on_edge nearer than a guide");
    expect(&r, "on_edge", seg_point(&cam, c, ea, eb), RAY, "same line: the nearer on_edge beats a guide");
    let c = off(mid, 4.5 * nn[0], 4.5 * nn[1]); // 0.5 px from the guide point, ~4.5 px from the edge
    let r = infer(&s, &cam, c, None, None, "guide nearer than on_edge");
    expect(&r, "guide", gp, PT, "same line: the nearer guide beats on_edge");

    // line 3: axis_x vs axis_y near `from`, and line 4: parallel vs perpendicular near `from`
    let f = [1.0, 1.0, 0.0];
    let s = Scene::default().level(-5.0);
    for (want, d, other) in [("axis_x", [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), ("axis_y", [0.0, 1.0, 0.0], [1.0, 0.0, 0.0])] {
        let mut cur = None;
        for i in 1..2000 {
            let c = cam.project(add(f, mul(d, i as f64 * 0.002)));
            let o = cand_px(&cam, c, line_point(&cam, c, f, other));
            if o > 4.0 && o < 7.0 { cur = Some(c); break; }
        }
        let c = cur.expect("judge scenario error: no cursor 4-7 px from the other axis");
        premise(cand_px(&cam, c, line_point(&cam, c, f, [0.0, 0.0, 1.0])) > 0.5, "z axis not nearer");
        let r = infer(&s, &cam, c, Some(f), None, want);
        expect(&r, want, line_point(&cam, c, f, d), RAY, &format!("same line: the nearer {want} wins near `from`"));
    }
    // `from` above the edge's plane: the perpendicular is tilted, so near `from` it can be within the radius
    // while the axes are not.
    let re = [[5.0, 5.0, 0.0], [6.6, 3.8, 0.0]]; // along (0.8, -0.6, 0)
    let dpar = [0.8, -0.6, 0.0];
    let f2 = add(sub([5.0, 5.0, 0.0], mul([0.6, 0.8, 0.0], 3.0)), [0.0, 0.0, 1.5]);
    let dper = unit(sub([5.0, 5.0, 0.0], f2));
    // A camera that sees the plane of the two lines nearly edge-on, so they are close on screen while the axes
    // through `from` are not (searched over a fixed set of viewpoints).
    let u2 = unit(sub(dper, mul(dpar, dot(dper, dpar))));
    let nrm = unit(cross(dpar, dper));
    let mut found = None;
    'outer: for k in 0..36 {
        for eps in [0.3, 0.6, 1.0, 1.5, 2.5] {
            let ph = k as f64 * std::f64::consts::PI / 18.0;
            let eye = add(f2, add(mul(add(mul(dpar, ph.cos()), mul(u2, ph.sin())), 15.0), mul(nrm, eps)));
            let cm = Cam { eye, target: f2, fov: 0.8, w: 1200.0, h: 800.0 };
            let pick = |d: V3, other: V3| find_cursor(&cm, f2, d, 0.0, |c| {
                let o = cand_px(&cm, c, line_point(&cm, c, f2, other));
                o > 3.0 && o < 9.0 && [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].iter()
                    .all(|a| cand_px(&cm, c, line_point(&cm, c, f2, *a)) > 10.5)
            });
            if let (Some(a), Some(b)) = (pick(dpar, dper), pick(dper, dpar)) { found = Some((cm, a, b)); break 'outer; }
        }
    }
    let (cm, ca, cb) = found.expect("judge scenario error: no viewpoint separates parallel/perpendicular from the axes");
    for (want, d, c) in [("parallel", dpar, ca), ("perpendicular", dper, cb)] {
        let cam = cm;
        let r = infer(&s, &cam, c, Some(f2), Some(re), want);
        expect(&r, want, line_point(&cam, c, f2, d), RAY, &format!("same line: the nearer {want} wins"));
    }
}

#[test]
fn camera_looking_straight_down() {
    watchdog();
    // view parallel to z: the camera's up is +y, so +x is right and +y is up on screen
    let cam = Cam { eye: [2.0, 3.0, 20.0], target: [2.0, 3.0, 0.0], fov: 0.8, w: 1000.0, h: 800.0 };
    let pr = cam.project([5.0, 3.0, 0.0]);
    let pu = cam.project([2.0, 6.0, 0.0]);
    premise(pr[0] > 600.0 && (pr[1] - 400.0).abs() < 1e-9 && pu[1] < 300.0 && (pu[0] - 500.0).abs() < 1e-9, "frame");
    let s = Scene::default().edge("r", 0, [5.0, 3.0, 0.0], [5.0, 1.0, 0.0]).edge("u", 0, [2.0, 6.0, 0.0], [0.0, 6.0, 0.0]).level(0.0);
    let r = infer(&s, &cam, off(pr, 2.0, 1.0), None, None, "straight down, right");
    expect(&r, "endpoint", [5.0, 3.0, 0.0], PT, "straight-down camera: +x is right of centre");
    expect_ref(&r, "r", "index", 0, "straight-down camera");
    let r = infer(&s, &cam, off(pu, 1.0, 2.0), None, None, "straight down, up");
    expect(&r, "endpoint", [2.0, 6.0, 0.0], PT, "straight-down camera: +y is above centre");
    expect_ref(&r, "u", "index", 0, "straight-down camera");
    let c = [700.0, 650.0];
    let r = infer(&s, &cam, c, None, None, "straight down, ground");
    expect(&r, "ground", plane_hit(&cam, c, 0.0).unwrap(), RAY, "straight-down camera: ground");
}

// ------------------------------------------------------------------ measurements

fn len_of(t: &str, unit: &str) -> f64 {
    match parse_measurement(t, unit, false) {
        Ok(Measurement::Length(x)) => x,
        Ok(_) => panic!("parse_measurement({t:?}, {unit:?}) should be a Length"),
        Err(_) => panic!("parse_measurement({t:?}, {unit:?}) returned Err"),
    }
}

fn near(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() <= 1e-12 * b.abs().max(1.0), "{what}: got {a}, want {b}");
}

#[test]
fn measurements() {
    watchdog();
    for (t, u, want) in [
        ("2500", "mm", 2.5), ("2500mm", "mm", 2.5), ("250cm", "mm", 2.5), ("2.5m", "mm", 2.5),
        ("2500mm", "m", 2.5), ("2500", "m", 2500.0), ("2.5", "m", 2.5), ("250", "cm", 2.5),
        ("8'6\"", "mm", 8.0 * 0.3048 + 6.0 * 0.0254), ("8'", "mm", 8.0 * 0.3048), ("6\"", "mm", 6.0 * 0.0254),
        ("8'6\"", "m", 2.5908), ("0.5", "mm", 0.0005),
    ] {
        near(len_of(t, u), want, &format!("parse_measurement({t:?}, {u:?})"));
    }
    match parse_measurement("4000,3000", "mm", false) {
        Ok(Measurement::Lengths(a, b)) => { near(a, 4.0, "4000,3000 first"); near(b, 3.0, "4000,3000 second"); }
        _ => panic!("parse_measurement(\"4000,3000\") should be Lengths(4.0, 3.0)"),
    }
    match parse_measurement("2,1.5", "m", false) {
        Ok(Measurement::Lengths(a, b)) => { near(a, 2.0, "2,1.5 (m) first"); near(b, 1.5, "2,1.5 (m) second"); }
        _ => panic!("parse_measurement(\"2,1.5\", \"m\") should be Lengths(2.0, 1.5)"),
    }
    for t in ["x5", "5x"] {
        match parse_measurement(t, "mm", false) {
            Ok(Measurement::Array(5)) => {}
            _ => panic!("parse_measurement({t:?}) should be Array(5)"),
        }
    }
    match parse_measurement("x12", "mm", false) { Ok(Measurement::Array(12)) => {}, _ => panic!("x12 should be Array(12)") }
    match parse_measurement("/5", "mm", false) { Ok(Measurement::Divide(5)) => {}, _ => panic!("/5 should be Divide(5)") }
    for (t, deg) in [("45", 45.0), ("90", 90.0), ("22.5", 22.5)] {
        match parse_measurement(t, "mm", true) {
            Ok(Measurement::Angle(a)) => near(a, deg * std::f64::consts::PI / 180.0, &format!("angle {t}")),
            _ => panic!("parse_measurement({t:?}, angle) should be Angle(radians)"),
        }
    }
    near(len_of("45", "mm"), 0.045, "45 without an angle tool is a length");
    for t in ["", "abc", "4000,3000,2000", "5xx", "x", "/", "2.5.1m", "12parsecs", "8'6\"7", "m"] {
        assert!(parse_measurement(t, "mm", false).is_err(), "parse_measurement({t:?}) should be an error");
    }
}

// ------------------------------------------------------------------ 100,000 edges

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    fn unit(&mut self) -> f64 { (self.next() >> 11) as f64 / (1u64 << 53) as f64 }
    fn range(&mut self, lo: f64, hi: f64) -> f64 { lo + (hi - lo) * self.unit() }
}

/// Screen distance from c to the projected segment, clipped to the part in front of the camera.
fn seg_px(cam: &Cam, c: [f64; 2], a: V3, b: V3) -> f64 {
    let (da, db) = (cam.depth(a), cam.depth(b));
    if da <= 1e-3 && db <= 1e-3 { return f64::INFINITY; }
    let (mut a, mut b) = (a, b);
    if da <= 1e-3 { a = lerp(a, b, (1e-3 - da) / (db - da)); }
    if db <= 1e-3 { b = lerp(b, a, (1e-3 - db) / (da - db)); }
    let (p, q) = (cam.project(a), cam.project(b));
    let d = [q[0] - p[0], q[1] - p[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 == 0.0 { 0.0 } else { (((c[0] - p[0]) * d[0] + (c[1] - p[1]) * d[1]) / l2).clamp(0.0, 1.0) };
    px_dist(c, [p[0] + t * d[0], p[1] + t * d[1]])
}

#[test]
fn hundred_thousand_edges() {
    watchdog();
    let mut rng = Rng(0x5EED_2012);
    let n = 100_000;
    let mut edges = Vec::with_capacity(n);
    for _ in 0..n {
        let a = [rng.range(-1000.0, 1000.0), rng.range(-1000.0, 1000.0), rng.range(0.0, 10.0)];
        let th = rng.range(0.0, std::f64::consts::TAU);
        let l = rng.range(2.0, 6.0);
        let b = [a[0] + l * th.cos(), a[1] + l * th.sin(), a[2] + rng.range(-1.0, 1.0)];
        edges.push((a, b));
    }
    let mut sj = String::with_capacity(n * 120);
    sj.push_str("{\"edges\":[");
    for (i, (a, b)) in edges.iter().enumerate() {
        if i > 0 { sj.push(','); }
        sj.push_str(&format!("{{\"entity\":\"E\",\"index\":{i},\"a\":[{:?},{:?},{:?}],\"b\":[{:?},{:?},{:?}]}}",
            a[0], a[1], a[2], b[0], b[1], b[2]));
    }
    sj.push_str("],\"faces\":[],\"guides\":[],\"level_elevation\":0.0}");
    let cam = Cam { eye: [10.0, -50.0, 70.0], target: [10.0, 0.0, 0.0], fov: 0.8, w: 1200.0, h: 800.0 };
    let q = |c: [f64; 2]| json!({"camera": cam.json(), "cursor": c}).to_string();
    let index = match InferScene::new(&sj) { Ok(x) => x, Err(_) => panic!("InferScene::new failed on the 100k scene") };
    let run = |c: [f64; 2]| -> Value {
        let out = match index.infer_json(&q(c)) { Ok(s) => s, Err(_) => panic!("InferScene::infer_json failed on the 100k scene") };
        let v: Value = serde_json::from_str(&out).expect("result is JSON");
        check_ref_shape(v["kind"].as_str().expect("kind"), &v["ref"], "100k scene");
        v
    };
    // Correctness on a subset vs brute force: cursors 4 px from an isolated endpoint, and empty spots.
    let visible: Vec<usize> = (0..n).filter(|&i| {
        let (a, b) = edges[i];
        let (pa, pb) = (cam.project(a), cam.project(b));
        cam.depth(a) > 1.0 && cam.depth(b) > 1.0 && pa[0] > 30.0 && pa[0] < 1170.0 && pa[1] > 30.0 && pa[1] < 770.0
            && px_dist(pa, pb) > 40.0
    }).collect();
    premise(visible.len() > 20, "enough visible edges");
    // Edges whose clipped projection can come within 25 px of a cursor inside the viewport (brute force).
    let onscreen: Vec<usize> = (0..n).filter(|&j| {
        let (a, b) = edges[j];
        let (da, db) = (cam.depth(a), cam.depth(b));
        if da <= 1e-3 && db <= 1e-3 { return false; }
        if da <= 1e-3 || db <= 1e-3 { return true; }
        let (pa, pb) = (cam.project(a), cam.project(b));
        !(pa[0].max(pb[0]) < -30.0 || pa[0].min(pb[0]) > 1230.0 || pa[1].max(pb[1]) < -30.0 || pa[1].min(pb[1]) > 830.0)
    }).collect();
    premise(onscreen.len() < 20_000, "the camera sees a small part of the scene");
    let mut checked = 0;
    for &i in &visible {
        if checked == 15 { break; }
        let (a, b) = edges[i];
        let c = off(cam.project(a), 3.0, 2.0);
        if onscreen.iter().any(|&j| j != i && seg_px(&cam, c, edges[j].0, edges[j].1) < 25.0) { continue; }
        let v = run(c);
        assert_eq!(v["kind"], "endpoint", "100k scene: cursor 3.6 px from edge {i}'s endpoint: {v}");
        assert_eq!(v["ref"]["index"].as_f64(), Some(i as f64), "100k scene: ref index: {v}");
        let p = &v["point"];
        let got = [p[0].as_f64().unwrap(), p[1].as_f64().unwrap(), p[2].as_f64().unwrap()];
        assert!(dist(got, a) < PT, "100k scene: endpoint point {got:?}, want {a:?}");
        let _ = b;
        checked += 1;
    }
    premise(checked >= 10, "found at least 10 isolated endpoints");
    let mut empty = 0;
    let mut tries = 0;
    let mut rng2 = Rng(77);
    while empty < 10 {
        tries += 1;
        premise(tries < 5000, "found 10 empty cursor spots");
        let c = [rng2.range(50.0, 1150.0), rng2.range(300.0, 750.0)];
        if onscreen.iter().any(|&j| seg_px(&cam, c, edges[j].0, edges[j].1) < 25.0) { continue; }
        let v = run(c);
        let g = plane_hit(&cam, c, 0.0).expect("cursor below the horizon");
        assert_eq!(v["kind"], "ground", "100k scene: empty spot -> ground: {v}");
        let p = &v["point"];
        let got = [p[0].as_f64().unwrap(), p[1].as_f64().unwrap(), p[2].as_f64().unwrap()];
        assert!(dist(got, g) < 1e-6 * dist(cam.eye, g), "100k scene: ground point {got:?}, want {g:?}");
        empty += 1;
    }
    // The convenience form gives the same answers.
    for c in [[600.0, 400.0], [300.0, 650.0]] {
        let a = run(c);
        let b: Value = match infer_json(&sj, &q(c)) {
            Ok(o) => serde_json::from_str(&o).expect("JSON"),
            Err(_) => panic!("infer_json failed on the 100k scene"),
        };
        assert_eq!(a, b, "100k scene: infer_json(scene, query) differs from InferScene::infer_json");
    }
    // Timing: mean of InferScene::infer_json over 300 queries at random cursors after construction and one
    // warm-up query (release build).
    let _ = run([600.0, 400.0]);
    let mut rng3 = Rng(4242);
    let cursors: Vec<[f64; 2]> = (0..300).map(|_| [rng3.range(0.0, 1200.0), rng3.range(0.0, 800.0)]).collect();
    let queries: Vec<String> = cursors.iter().map(|&c| q(c)).collect();
    let t0 = Instant::now();
    for qs in &queries {
        let r = index.infer_json(qs);
        assert!(r.is_ok(), "InferScene::infer_json failed during timing");
    }
    let mean = t0.elapsed().as_secs_f64() / queries.len() as f64;
    eprintln!("T2.12: mean InferScene::infer_json time on 100,000 edges: {:.3} ms", mean * 1e3);
    assert!(mean < 2e-3, "mean query time on 100,000 edges {:.3} ms, budget 2 ms (release)", mean * 1e3);
}
