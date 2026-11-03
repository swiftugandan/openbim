//! Judge-shaped scenarios of spec/sketch.md, checked against closed forms.

use super::*;
use crate::geom::{dist, newell, unit, V3};

/// Applies command files one after another to one document and returns the named Shape.
struct Bench {
    doc: Document,
}

impl Bench {
    fn new() -> Bench {
        Bench { doc: Document::new() }
    }

    fn sketch(&mut self, commands: &str) -> Result<(), SketchError> {
        let ops = run(&self.doc, &format!(r#"{{"schema": 1, "commands": [{commands}]}}"#))?;
        for op in ops {
            self.doc.apply(op).unwrap();
        }
        Ok(())
    }

    fn ok(&mut self, commands: &str) -> &mut Bench {
        self.sketch(commands).unwrap_or_else(|e| panic!("{commands}: {e}"));
        self
    }

    fn shape(&self, name: &str) -> Shape {
        Shape::from_entity(self.doc.get(EntityId::parse(name)).expect("shape exists")).unwrap()
    }
}

fn ids(s: &Shape) -> Vec<u64> {
    let mut v: Vec<u64> = s.faces.iter().map(|f| f.id).collect();
    v.sort_unstable();
    v
}

fn area(s: &Shape, id: u64) -> f64 {
    let f = &s.faces[s.face_by_id(id).unwrap()];
    let n = unit(newell(&s.loop_points(&f.loops[0]))).unwrap();
    f.loops.iter().map(|lp| geom::dot(newell(&s.loop_points(lp)), n) / 2.0).sum()
}

fn loose(s: &Shape) -> Vec<(usize, usize)> {
    let on = s.loop_edges();
    s.edges.iter().copied().filter(|e| !on.contains(e)).collect()
}

fn find(s: &Shape, p: V3) -> usize {
    s.find_vertex(p).unwrap_or_else(|| panic!("no vertex at {p:?} in {:?}", s.vertices))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

const RECT: &str = r#"{"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]}"#;
const PUSH: &str = r#"{"op": "pushpull", "shape": "box", "face": 1, "distance": 2.5}"#;

fn planar_faces(s: &Shape) {
    for f in &s.faces {
        let plane = s.face_plane(s.face_by_id(f.id).unwrap()).unwrap();
        for lp in &f.loops {
            assert!(lp.len() >= 3);
            let set: std::collections::BTreeSet<usize> = lp.iter().copied().collect();
            assert_eq!(set.len(), lp.len(), "face {} repeats a vertex", f.id);
            for &v in lp {
                assert!(plane.contains(s.vertices[v]), "face {} is not planar", f.id);
            }
        }
    }
    for i in 0..s.vertices.len() {
        for j in 0..i {
            assert!(dist(s.vertices[i], s.vertices[j]) > 1e-9, "vertices {i} and {j} coincide");
        }
    }
}

#[test]
fn spec_example_is_a_30_m3_box() {
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}"));
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.is_closed());
    assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6]);
    assert!(close(s.volume(), 30.0), "{}", s.volume());
    assert_eq!(s.vertices.len(), 8);
    assert!(loose(&s).is_empty());
    // Face 1 kept its normal and moved by 2.5 along it.
    let f1 = s.face_by_id(1).unwrap();
    let n = s.face_normal(f1).unwrap();
    assert_eq!(n, [0.0, 0.0, 1.0]);
    assert!(s.faces[f1].loops[0].iter().all(|&v| (s.vertices[v][2] - 2.5).abs() < 1e-12));
}

#[test]
fn rect_alone_is_one_face_with_normal_up() {
    let mut b = Bench::new();
    b.ok(RECT);
    let s = b.shape("box");
    assert_eq!(ids(&s), vec![1]);
    assert!(!s.is_closed());
    assert!(close(area(&s, 1), 12.0));
    assert_eq!(s.face_normal(0).unwrap(), [0.0, 0.0, 1.0]);
    // Tilted rectangles.
    for (c, u, v, d) in [
        ([1.0, 2.0, 3.0], [3.0, 0.0, 0.0], [0.0, 4.0, 3.0], 2.0),
        ([-2.5, 1.25, 0.5], [1.0, 2.0, 2.0], [3.0, -1.5, 0.0], 1.75),
        ([4.0, -3.0, 2.5], [1.0, -1.5, 3.0], [1.5, 1.0, 0.0], 0.4),
    ] {
        let mut b = Bench::new();
        b.ok(&format!(r#"{{"op": "rect", "shape": "t", "corner": {c:?}, "u": {u:?}, "v": {v:?}}}"#));
        let s = b.shape("t");
        assert_eq!(ids(&s), vec![1]);
        let a = geom::norm(u) * geom::norm(v);
        assert!(close(area(&s, 1), a));
        b.ok(&format!(r#"{{"op": "pushpull", "shape": "t", "face": 1, "distance": {d}}}"#));
        let s = b.shape("t");
        planar_faces(&s);
        assert!(s.is_closed());
        assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6]);
        assert!(close(s.volume(), a * d), "{} vs {}", s.volume(), a * d);
        assert_eq!(s.vertices.len(), 8);
    }
}

#[test]
fn push_in_and_pull_whole_top_stretch_sides() {
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "pushpull", "shape": "box", "face": 1, "distance": -1}"#);
    let s = b.shape("box");
    assert!(s.is_closed() && close(s.volume(), 18.0) && s.vertices.len() == 8 && ids(&s) == vec![1, 2, 3, 4, 5, 6]);
    b.ok(r#"{"op": "pushpull", "shape": "box", "face": 1, "distance": 2}"#);
    let s = b.shape("box");
    assert!(s.is_closed() && close(s.volume(), 42.0) && s.vertices.len() == 8 && ids(&s) == vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn split_top_and_pull_half() {
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "line", "shape": "box", "from": [2, 0, 2.5], "to": [2, 3, 2.5]}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.is_closed(), "{s:?}");
    assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6, 7]);
    assert!(close(s.volume(), 30.0));
    assert!(close(area(&s, 1), 6.0) && close(area(&s, 7), 6.0));
    // Face 1 keeps its first vertex (0,0,2.5): the left half.
    let f1 = &s.faces[s.face_by_id(1).unwrap()];
    assert!(f1.loops[0].iter().all(|&v| s.vertices[v][0] <= 2.0));
    b.ok(r#"{"op": "pushpull", "shape": "box", "face": 7, "distance": 1}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.is_closed(), "{s:?}");
    assert!(close(s.volume(), 36.0), "{}", s.volume());
    assert!(loose(&s).is_empty(), "{:?}", loose(&s));
    assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    let f8 = s.face_by_id(8).unwrap();
    assert_eq!(s.face_normal(f8).unwrap()[0].abs(), 1.0);
    assert!(close(area(&s, 8), 3.0));
    // Erasing the split edge merges 1 and 7 back into 1.
    let (i, j) = (find(&s, [2.0, 0.0, 2.5]), find(&s, [2.0, 3.0, 2.5]));
    let mut b2 = Bench::new();
    b2.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "line", "shape": "box", "from": [2, 0, 2.5], "to": [2, 3, 2.5]}"#);
    let _ = (i, j);
    let s2 = b2.shape("box");
    let (i, j) = (find(&s2, [2.0, 0.0, 2.5]), find(&s2, [2.0, 3.0, 2.5]));
    b2.ok(&format!(r#"{{"op": "erase", "shape": "box", "edges": [[{i}, {j}]]}}"#));
    let s2 = b2.shape("box");
    planar_faces(&s2);
    assert_eq!(ids(&s2), vec![1, 2, 3, 4, 5, 6]);
    assert!(close(area(&s2, 1), 12.0));
    assert!(s2.is_closed(), "{s2:?}");
    assert!(close(s2.volume(), 30.0));
}

#[test]
fn pocket_and_punch_through() {
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "rect", "shape": "box", "corner": [1, 1, 2.5], "u": [2, 0, 0], "v": [0, 1, 0]}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6, 7]);
    assert!(s.is_closed(), "{s:?}");
    assert!(close(s.volume(), 30.0));
    assert!(close(area(&s, 7), 2.0) && close(area(&s, 1), 10.0));
    assert_eq!(s.face_normal(s.face_by_id(7).unwrap()).unwrap(), [0.0, 0.0, 1.0]);
    assert_eq!(s.faces[s.face_by_id(1).unwrap()].loops.len(), 2);
    let mut pocket = Bench::new();
    pocket.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "rect", "shape": "box", "corner": [1, 1, 2.5], "u": [2, 0, 0], "v": [0, 1, 0]}"#);
    pocket.ok(r#"{"op": "pushpull", "shape": "box", "face": 7, "distance": -1}"#);
    let s = pocket.shape("box");
    planar_faces(&s);
    assert!(s.is_closed(), "{s:?}");
    assert!(close(s.volume(), 28.0), "{}", s.volume());
    assert_eq!(s.faces.len(), 11);
    b.ok(r#"{"op": "pushpull", "shape": "box", "face": 7, "distance": -2.5}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.is_closed(), "{s:?}");
    assert!(close(s.volume(), 25.0), "{}", s.volume());
    assert_eq!(s.faces.len(), 10);
    assert!(ids(&s).starts_with(&[1, 2, 3, 4, 5, 6]));
    let bottom = (0..s.faces.len()).find(|&f| s.face_normal(f).unwrap()[2] < -0.5).unwrap();
    assert_eq!(s.faces[bottom].loops.len(), 2);
    assert!(close(area(&s, s.faces[bottom].id), 10.0));
    assert!(close(area(&s, 1), 10.0));
}

/// Every directed edge of a closed shape is used once each way, so every undirected edge is on
/// exactly two faces that traverse it in opposite directions.
fn two_faces_per_edge_opposite(s: &Shape) {
    let mut count: std::collections::BTreeMap<(usize, usize), usize> = std::collections::BTreeMap::new();
    for f in &s.faces {
        for e in f.directed_edges() {
            *count.entry(e).or_insert(0) += 1;
        }
    }
    for (&(a, b), &n) in &count {
        assert_eq!(n, 1, "edge {a}->{b} used {n} times");
        assert_eq!(count.get(&(b, a)), Some(&1), "edge {a}->{b} has no reverse");
        assert_eq!(s.faces_with_edge(a, b).len(), 2, "edge {{{a}, {b}}} is not on exactly two faces");
    }
}

#[test]
fn pushpull_new_keeps_the_original() {
    let mut b = Bench::new();
    b.ok(r#"{"op": "rect", "shape": "lone", "corner": [0, 5, 0], "u": [2, 0, 0], "v": [0, 1, 0]}"#)
        .ok(r#"{"op": "pushpull", "shape": "lone", "face": 1, "distance": 1.5, "new": true}"#);
    let s = b.shape("lone");
    assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(s.vertices.len(), 8);
    let f1 = &s.faces[s.face_by_id(1).unwrap()];
    assert!(f1.loops[0].iter().all(|&v| s.vertices[v][2] == 0.0));
    let f2 = &s.faces[s.face_by_id(2).unwrap()];
    assert!(f2.loops[0].iter().all(|&v| s.vertices[v][2] == 1.5));
}

#[test]
fn pushpull_new_on_a_lone_face_closes_an_outward_prism_either_way() {
    // In z = 0, a vertical plane and a tilted one; pulled and pushed.
    let rects = [
        (r#""corner": [0, 5, 0], "u": [2, 0, 0], "v": [0, 1, 0]"#, 2.0),
        (r#""corner": [1, 1, 1], "u": [0, 3, 0], "v": [0, 0, 2]"#, 6.0),
        (r#""corner": [-2.5, 1.25, 0.5], "u": [1, 2, 2], "v": [3, -1.5, 0]"#, 3.0 * 3.3541019662496847),
    ];
    for (rect, area_f) in rects {
        for d in [1.5f64, -1.5, 0.75, -2.0] {
            let mut b = Bench::new();
            b.ok(&format!(r#"{{"op": "rect", "shape": "p", {rect}}}"#));
            let s0 = b.shape("p");
            let first0 = s0.faces[0].loops[0][0];
            let pts0: Vec<V3> = s0.loop_points(&s0.faces[0].loops[0]);
            let n0 = s0.face_normal(0).unwrap();
            b.ok(&format!(r#"{{"op": "pushpull", "shape": "p", "face": 1, "distance": {d}, "new": true}}"#));
            let s = b.shape("p");
            let what = format!("{rect} d = {d}");
            planar_faces(&s);
            assert!(s.is_closed(), "{what}: not closed: {s:?}");
            two_faces_per_edge_opposite(&s);
            assert!(close(s.volume(), area_f * d.abs()), "{what}: volume {} vs {}", s.volume(), area_f * d.abs());
            assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6], "{what}");
            assert_eq!(s.vertices.len(), 8, "{what}");
            assert!(loose(&s).is_empty(), "{what}");
            // Face 1 stays: same id, same points, same first outer-loop vertex; it faces away from
            // the moved copy (outward for the prism).
            let f1 = &s.faces[s.face_by_id(1).unwrap()];
            assert_eq!(f1.loops[0][0], first0, "{what}: face 1 changed its first vertex");
            let mut pts: Vec<V3> = s.loop_points(&f1.loops[0]);
            pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut want = pts0.clone();
            want.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(pts, want, "{what}: face 1 moved");
            let n1 = s.face_normal(s.face_by_id(1).unwrap()).unwrap();
            assert!(close(geom::dot(n1, n0), -d.signum()), "{what}: face 1 normal {n1:?} vs {n0:?}");
            // The moved copy is face 2: face 1 moved d along its old normal, facing outward.
            let f2 = s.face_by_id(2).unwrap();
            let n2 = s.face_normal(f2).unwrap();
            assert!(close(geom::dot(n2, n0), d.signum()), "{what}: copy normal");
            for p in &pts0 {
                let q = geom::add(*p, geom::scale(n0, d));
                assert!(s.faces[f2].loops[0].iter().any(|&v| dist(s.vertices[v], q) < 1e-9), "{what}: copy lacks {q:?}");
            }
            // The four sides (ids 3..6) are perpendicular to the face.
            for id in 3..=6 {
                let m = s.face_normal(s.face_by_id(id).unwrap()).unwrap();
                assert!(geom::dot(m, n0).abs() < 1e-9, "{what}: side {id} is not perpendicular");
            }
            assert!(close(s.faces.iter().map(|f| area(&s, f.id)).sum::<f64>(), 2.0 * area_f + d.abs() * perimeter(&s0)), "{what}: total area");
        }
    }
}

#[test]
fn a_face_edge_on_several_faces_gets_a_new_side_not_a_stretch() {
    // A fin standing on a top edge of the box: its bottom edge is on the fin, the top (which is
    // perpendicular to the fin) and the y = 0 side. No single face can follow that edge, so pushing
    // the fin makes four new sides and leaves the box's six faces as they were.
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "rect", "shape": "box", "corner": [0, 0, 2.5], "u": [4, 0, 0], "v": [0, 0, 1]}"#);
    let s0 = b.shape("box");
    assert_eq!(ids(&s0), (1..=7).collect::<Vec<u64>>());
    assert!(close(area(&s0, 7), 4.0));
    let n7 = s0.face_normal(s0.face_by_id(7).unwrap()).unwrap();
    assert_eq!(n7[1].abs(), 1.0);
    for d in [-0.5, 0.5] {
        let mut b2 = Bench::new();
        b2.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "rect", "shape": "box", "corner": [0, 0, 2.5], "u": [4, 0, 0], "v": [0, 0, 1]}"#);
        b2.ok(&format!(r#"{{"op": "pushpull", "shape": "box", "face": 7, "distance": {d}}}"#));
        let s = b2.shape("box");
        planar_faces(&s);
        assert_eq!(ids(&s), (1..=11).collect::<Vec<u64>>(), "d = {d}");
        for id in 1..=6 {
            let (g, g0) = (s.face_by_id(id).unwrap(), s0.face_by_id(id).unwrap());
            if d < 0.0 {
                assert_eq!(s.faces[g], s0.faces[g0], "box face {id} untouched (d = {d})");
            } else {
                // Pushed over the top, the copies land on the top's boundary and are spliced into
                // its loop (no T-junctions); its area and normal are unchanged.
                assert!(close(area(&s, id), area(&s0, id)), "box face {id} area (d = {d})");
                assert_eq!(s.face_normal(g).unwrap(), s0.face_normal(g0).unwrap(), "box face {id} normal (d = {d})");
            }
        }
        let f7 = s.face_by_id(7).unwrap();
        assert!(s.faces[f7].loops[0].iter().all(|&v| (s.vertices[v][1] - d * n7[1]).abs() < 1e-12), "d = {d}: the fin moved");
        for id in 8..=11 {
            assert!(s.face_normal(s.face_by_id(id).unwrap()).unwrap()[1].abs() < 1e-9, "side {id} is perpendicular to the fin");
        }
        // The box alone is still a closed 30 m3 solid.
        let only_box = Shape { vertices: s.vertices.clone(), faces: s.faces.iter().filter(|f| f.id <= 6).cloned().collect(), edges: Vec::new() };
        assert!(only_box.is_closed() && close(only_box.volume(), 30.0));
    }
}

fn perimeter(s: &Shape) -> f64 {
    let lp = &s.faces[0].loops[0];
    (0..lp.len()).map(|i| dist(s.vertices[lp[i]], s.vertices[lp[(i + 1) % lp.len()]])).sum()
}

#[test]
fn pushpull_new_on_a_solid_face_is_a_deterministic_open_surface() {
    // The original top stays shared by the box and the new prism: three faces on each of its
    // edges, so no solid; everything stays planar, nothing panics, and the result is reproducible.
    let mut results = Vec::new();
    for d in [1.0, -1.0] {
        for _ in 0..2 {
            let mut b = Bench::new();
            b.ok(&format!("{RECT}, {PUSH}")).ok(&format!(r#"{{"op": "pushpull", "shape": "box", "face": 1, "distance": {d}, "new": true}}"#));
            let s = b.shape("box");
            planar_faces(&s);
            assert!(!s.is_closed());
            assert_eq!(ids(&s), (1..=11).collect::<Vec<u64>>());
            assert_eq!(s.vertices.len(), 12);
            let f1 = s.face_by_id(1).unwrap();
            assert!(s.faces[f1].loops[0].iter().all(|&v| (s.vertices[v][2] - 2.5).abs() < 1e-12), "the original stays");
            assert_eq!(s.face_normal(f1).unwrap(), [0.0, 0.0, 1.0], "and keeps its winding");
            let f7 = s.face_by_id(7).unwrap();
            assert!(s.faces[f7].loops[0].iter().all(|&v| (s.vertices[v][2] - 2.5 - d).abs() < 1e-12), "the copy moved");
            for (a, b) in s.faces[f1].directed_edges() {
                assert_eq!(s.faces_with_edge(a, b).len(), 3);
            }
            results.push((d, s));
        }
    }
    assert_eq!(results[0].1, results[1].1);
    assert_eq!(results[2].1, results[3].1);
    // Pushed down by the full height with `new`, the copy lands on the bottom and punches
    // through it (both go); the original top stays. No panic, planar faces, loops without
    // repeated vertices.
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "pushpull", "shape": "box", "face": 1, "distance": -2.5, "new": true}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.face_by_id(1).is_some() && s.face_by_id(7).is_none());
    assert_eq!(s.vertices.len(), 8);
    // An inner rect on the top pushed through with `new`: the top and bottom become rings, the
    // original inner face stays over the hole, the copy goes.
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "rect", "shape": "box", "corner": [1, 1, 2.5], "u": [2, 0, 0], "v": [0, 1, 0]}"#);
    b.ok(r#"{"op": "pushpull", "shape": "box", "face": 7, "distance": -2.5, "new": true}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.face_by_id(7).is_some() && s.face_by_id(8).is_none());
    assert_eq!(s.faces[s.face_by_id(1).unwrap()].loops.len(), 2);
    let bottom = (0..s.faces.len()).find(|&f| s.face_normal(f).unwrap()[2] < -0.5).unwrap();
    assert_eq!(s.faces[bottom].loops.len(), 2);
    assert!(close(area(&s, s.faces[bottom].id), 10.0));
    assert_eq!(s.faces.len(), 11);
}

#[test]
fn lines_close_loops_split_faces_and_leave_stubs() {
    // Triangle in a tilted plane, closing point 4e-10 off.
    let mut b = Bench::new();
    b.ok(r#"{"op": "line", "shape": "tri", "from": [1, 0, 0], "to": [0, 2, 0]},
            {"op": "line", "shape": "tri", "from": [0, 2, 0], "to": [0, 0, 3]},
            {"op": "line", "shape": "tri", "from": [0, 0, 3], "to": [1.0000000004, 0, 0]}"#);
    let s = b.shape("tri");
    assert_eq!(ids(&s), vec![1]);
    assert_eq!(s.vertices.len(), 3);
    assert!(loose(&s).is_empty());
    // Two squares in two drawing orders.
    let sq = [([0.0, 0.0], [2.0, 0.0]), ([2.0, 0.0], [2.0, 1.0]), ([2.0, 1.0], [0.0, 1.0]), ([0.0, 1.0], [0.0, 0.0]), ([1.0, 0.0], [1.0, 1.0])];
    for order in [[0, 1, 2, 3, 4], [4, 0, 1, 2, 3]] {
        let mut b = Bench::new();
        let cmds: Vec<String> = order
            .iter()
            .map(|&k| {
                let (p, q) = sq[k];
                format!(r#"{{"op": "line", "shape": "g", "from": [{}, {}, 1], "to": [{}, {}, 1]}}"#, p[0], p[1], q[0], q[1])
            })
            .collect();
        b.ok(&cmds.join(","));
        let s = b.shape("g");
        planar_faces(&s);
        assert_eq!(ids(&s), vec![1, 2], "{order:?}: {s:?}");
        assert!(loose(&s).is_empty());
        assert!(close(area(&s, 1), 1.0) && close(area(&s, 2), 1.0));
        assert_eq!(s.vertices.len(), 6);
    }
    // Non-planar loop: no face.
    let mut b = Bench::new();
    b.ok(r#"{"op": "line", "shape": "k", "from": [0, 0, 0], "to": [1, 0, 0]},
            {"op": "line", "shape": "k", "from": [1, 0, 0], "to": [1, 1, 0.5]},
            {"op": "line", "shape": "k", "from": [1, 1, 0.5], "to": [0, 1, 0]},
            {"op": "line", "shape": "k", "from": [0, 1, 0], "to": [0, 0, 0]}"#);
    let s = b.shape("k");
    assert!(s.faces.is_empty() && loose(&s).len() == 4);
    // A line across a rectangle.
    let mut b = Bench::new();
    b.ok(RECT).ok(r#"{"op": "line", "shape": "box", "from": [2, -1, 0], "to": [2, 4, 0]}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert_eq!(ids(&s), vec![1, 2]);
    assert!(close(area(&s, 1), 6.0) && close(area(&s, 2), 6.0));
    assert_eq!(loose(&s).len(), 2);
    assert_eq!(s.vertices.len(), 8);
    assert!(s.faces[s.face_by_id(1).unwrap()].loops[0].contains(&0), "face 1 keeps its first vertex");
    let (i, j) = (find(&s, [2.0, -1.0, 0.0]), find(&s, [2.0, 0.0, 0.0]));
    b.ok(&format!(r#"{{"op": "erase", "shape": "box", "edges": [[{i}, {j}]]}}"#));
    let s = b.shape("box");
    assert_eq!(loose(&s).len(), 1);
    assert_eq!(ids(&s), vec![1, 2]);
    assert_eq!(s.vertices.len(), 8, "the stub's free end stays at its index (indices are kept)");
    assert!(s.neighbours(i).is_empty() && dist(s.vertices[i], [2.0, -1.0, 0.0]) < 1e-12);
    // The other stub's indices, read before, are still valid in a later file.
    let (k, l) = (find(&s, [2.0, 3.0, 0.0]), find(&s, [2.0, 4.0, 0.0]));
    assert_eq!(loose(&s), vec![(k.min(l), k.max(l))]);
    b.ok(&format!(r#"{{"op": "erase", "shape": "box", "edges": [[{k}, {l}]]}}"#));
    let s = b.shape("box");
    assert!(loose(&s).is_empty() && ids(&s) == vec![1, 2] && s.vertices.len() == 8);
    // A new point on an orphan reuses it; one elsewhere is appended.
    b.ok(r#"{"op": "line", "shape": "box", "from": [2, 4, 0], "to": [2, 5, 0]}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert_eq!(s.vertices.len(), 9);
    assert_eq!(loose(&s), vec![(l.min(8), l.max(8))]);
    // A line between two boundary points of a tilted face.
    let mut b = Bench::new();
    b.ok(r#"{"op": "rect", "shape": "t", "corner": [1, 1, 1], "u": [0, 3, 0], "v": [2, 0, 2]}"#)
        .ok(r#"{"op": "line", "shape": "t", "from": [1, 2, 1], "to": [3, 2, 3]}"#);
    let s = b.shape("t");
    planar_faces(&s);
    assert_eq!(ids(&s), vec![1, 2]);
    let a = 3.0 * 8f64.sqrt();
    assert!(close(area(&s, 1), a / 3.0), "{}", area(&s, 1));
    assert!(close(area(&s, 2), 2.0 * a / 3.0));
}

#[test]
fn circles_and_arcs() {
    let mut b = Bench::new();
    b.ok(r#"{"op": "circle", "shape": "c", "centre": [1, 1, 0.5], "normal": [0, 0, 1], "radius": 1.5}"#);
    let s = b.shape("c");
    assert_eq!(ids(&s), vec![1]);
    assert_eq!(s.vertices.len(), 24);
    assert!(close(area(&s, 1), 12.0 * 1.5 * 1.5 * (std::f64::consts::TAU / 24.0).sin()));
    assert!(dist(s.vertices[0], [2.5, 1.0, 0.5]) < 1e-12, "first vertex along x");
    b.ok(r#"{"op": "pushpull", "shape": "c", "face": 1, "distance": 1.25}"#);
    let s = b.shape("c");
    assert!(s.is_closed() && s.faces.len() == 26);
    // x parallel to the normal: first vertex along y.
    let mut b = Bench::new();
    b.ok(r#"{"op": "circle", "shape": "c", "centre": [2, -1, 3], "normal": [1, 0, 0], "radius": 0.8, "segments": 7}"#);
    let s = b.shape("c");
    assert!(dist(s.vertices[0], [2.0, -0.2, 3.0]) < 1e-12);
    assert_eq!(s.faces.len(), 1);
    // Arc: an open polyline, closed by a chord.
    let mut b = Bench::new();
    b.ok(r#"{"op": "arc", "shape": "a", "centre": [5, 5, 0], "start": [7, 5, 0], "normal": [0, 0, 1], "angle": 3.141592653589793}"#);
    let s = b.shape("a");
    assert!(s.faces.is_empty() && loose(&s).len() == 12 && s.vertices.len() == 13);
    assert!(dist(s.vertices[12], [3.0, 5.0, 0.0]) < 1e-12);
    b.ok(r#"{"op": "line", "shape": "a", "from": [3, 5, 0], "to": [7, 5, 0]}"#);
    let s = b.shape("a");
    assert_eq!(ids(&s), vec![1]);
    assert!(loose(&s).is_empty());
    assert!(close(area(&s, 1), 6.0 * 4.0 * (std::f64::consts::PI / 12.0).sin()));
    let mut b = Bench::new();
    b.ok(r#"{"op": "arc", "shape": "a2", "centre": [0, 0, 1], "start": [0, 1, 1], "normal": [1, 0, 0], "angle": -1.5707963267948966, "segments": 3}"#);
    let s = b.shape("a2");
    assert_eq!(loose(&s).len(), 3);
    assert!(dist(s.vertices[3], [0.0, 0.0, 0.0]) < 1e-12);
}

#[test]
fn erase_box_edge_removes_two_faces() {
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}"));
    let s = b.shape("box");
    let (i, j) = (find(&s, [0.0, 0.0, 0.0]), find(&s, [0.0, 0.0, 2.5]));
    b.ok(&format!(r#"{{"op": "erase", "shape": "box", "edges": [[{j}, {i}]]}}"#));
    let s = b.shape("box");
    assert_eq!(s.faces.len(), 4);
    assert!(!s.is_closed());
    assert_eq!(s.vertices.len(), 8);
}

#[test]
fn invalid_commands_change_nothing() {
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}"));
    let before = b.doc.clone();
    for cmds in [
        r#"{"op": "rect", "shape": "zz", "corner": [0, 0, 0], "u": [1, 0, 0], "v": [0, 1, 0]}, {"op": "pushpull", "shape": "box", "face": 99, "distance": 1}"#,
        r#"{"op": "pushpull", "shape": "box", "face": 1, "distance": 1}, {"op": "extrude-everything", "shape": "box"}"#,
        r#"{"op": "rect", "shape": "box", "corner": [9, 9, 9], "u": [1, 0, 0], "v": [1, 1, 0]}"#,
        r#"{"op": "erase", "shape": "box", "edges": [[0, 9999]]}"#,
        r#"{"op": "pushpull", "shape": "nothing", "face": 1, "distance": 1}"#,
        r#"{"op": "pushpull", "shape": "box", "face": 1, "distance": 0}"#,
        r#"{"op": "line", "shape": "box", "from": [0, 0, 0], "to": [0, 0, 0]}"#,
    ] {
        assert!(b.sketch(cmds).is_err(), "{cmds}");
        assert_eq!(b.doc, before);
    }
    let level = EntityId::parse("L");
    b.doc.apply(Op::Create { id: level, kind: "Level".into() }).unwrap();
    assert!(b.sketch(r#"{"op": "rect", "shape": "L", "corner": [0, 0, 0], "u": [1, 0, 0], "v": [0, 1, 0]}"#).is_err());
}

#[test]
fn ops_name_created_shapes() {
    let out = apply(&Document::new(), &format!(r#"{{"schema": 1, "commands": [{RECT}]}}"#)).unwrap();
    let id = EntityId::parse("box");
    assert_eq!(out.ops[0], Op::Create { id, kind: "Shape".into() });
    assert_eq!(out.ops[1], Op::Set { id, key: "_name".into(), value: Value::Text("box".into()) });
    assert_eq!(out.names.get(&id).map(String::as_str), Some("box"));
    assert_eq!(out.ops.len(), 3 + 4);
}

#[test]
fn a_cube_drawn_edge_by_edge_faces_outward() {
    let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]];
    let e = [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)];
    let cmds: Vec<String> = e.iter().map(|&(i, j)| format!(r#"{{"op": "line", "shape": "c", "from": {:?}, "to": {:?}}}"#, p[i], p[j])).collect();
    let mut b = Bench::new();
    b.ok(&cmds.join(","));
    let s = b.shape("c");
    assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6]);
    assert!(s.is_closed());
    assert!(close(s.volume(), 1.0), "{}", s.volume());
}

#[test]
fn half_of_the_top_pushed_onto_the_bottom_shrinks_it() {
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "line", "shape": "box", "from": [2, 0, 2.5], "to": [2, 3, 2.5]}"#);
    b.ok(r#"{"op": "pushpull", "shape": "box", "face": 7, "distance": -2.5}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.is_closed(), "{s:?}");
    assert!(close(s.volume(), 15.0), "{}", s.volume());
    assert!(loose(&s).is_empty(), "{:?}", loose(&s));
    assert_eq!(s.faces.len(), 6);
    // Indices are kept: the two bottom corners the bottom face shrank away from and the two top
    // corners the push left behind (their targets were the bottom corners) stay as orphans.
    assert_eq!(s.vertices.len(), 12);
    let used: std::collections::BTreeSet<usize> = s.faces.iter().flat_map(|f| f.loops.iter().flatten().copied()).collect();
    assert_eq!(used.len(), 8);
    for v in [1, 2, 5, 6] {
        assert!(!used.contains(&v) && s.neighbours(v).is_empty(), "vertex {v} is an orphan");
    }
    assert!(dist(s.vertices[5], [4.0, 0.0, 2.5]) < 1e-12 && dist(s.vertices[1], [4.0, 0.0, 0.0]) < 1e-12);
    assert!(dist(s.vertices[3], [0.0, 3.0, 0.0]) < 1e-12, "(0,3,0) keeps index 3");
    // The bottom face kept its id and shrank to the left half.
    let bottom = (0..s.faces.len()).find(|&f| s.face_normal(f).unwrap()[2] < -0.5).unwrap();
    assert!(close(area(&s, s.faces[bottom].id), 6.0));
    assert!(s.faces[bottom].id <= 6);
}

#[test]
fn a_naked_face_pushed_against_its_normal_is_an_outward_solid() {
    let mut b = Bench::new();
    b.ok(RECT).ok(r#"{"op": "pushpull", "shape": "box", "face": 1, "distance": -2.5}"#);
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.is_closed());
    assert!(close(s.volume(), 30.0), "{}", s.volume());
    assert_eq!(ids(&s), vec![1, 2, 3, 4, 5, 6]);
    assert!(s.vertices.iter().all(|v| v[2] <= 0.0), "the box forms on the -z side");
    let mut b = Bench::new();
    b.ok(r#"{"op": "rect", "shape": "t", "corner": [1, 2, 3], "u": [3, 0, 0], "v": [0, 4, 3]}"#)
        .ok(r#"{"op": "pushpull", "shape": "t", "face": 1, "distance": -2}"#);
    let s = b.shape("t");
    assert!(s.is_closed() && close(s.volume(), 30.0), "{}", s.volume());
}

#[test]
fn a_loose_edge_survives_a_push_onto_its_vertex() {
    // Bottom vertices pushed onto the top ones reuse the top vertices (no duplicate is made, no
    // index shifts); the bottom vertices stay behind as orphans and the loose edge at the top
    // corner keeps its indices.
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}")).ok(r#"{"op": "line", "shape": "box", "from": [0, 0, 2.5], "to": [-1, -1, 2.5]}"#);
    let s = b.shape("box");
    let base = s.faces.iter().find(|f| s.face_normal(s.face_by_id(f.id).unwrap()).unwrap()[2] < -0.5).unwrap().id;
    let before = s.vertices.clone();
    assert_eq!(loose(&s), vec![(4, 8)]);
    b.ok(&format!(r#"{{"op": "pushpull", "shape": "box", "face": {base}, "distance": -2.5}}"#));
    let s = b.shape("box");
    planar_faces(&s);
    assert!(s.faces.is_empty(), "the bottom pushed onto the top removes the solid");
    assert_eq!(loose(&s), vec![(4, 8)]);
    assert_eq!(s.vertices, before, "no vertex moved, appeared or shifted");
}

#[test]
fn a_push_onto_a_stranded_vertex_reuses_it() {
    // Erasing every edge of the box but the bottom's leaves the four top corners as orphans;
    // pulling the bottom back up to their height reuses them instead of appending duplicates.
    let mut b = Bench::new();
    b.ok(&format!("{RECT}, {PUSH}"));
    let s = b.shape("box");
    let top: Vec<usize> = s.faces[s.face_by_id(1).unwrap()].loops[0].clone();
    let bottom = s.faces.iter().find(|f| s.face_normal(s.face_by_id(f.id).unwrap()).unwrap()[2] < -0.5).unwrap();
    let keep: std::collections::BTreeSet<(usize, usize)> = bottom.directed_edges().into_iter().map(|(a, b)| (a.min(b), a.max(b))).collect();
    let bottom_id = bottom.id;
    let pairs: Vec<String> = s.edges.iter().filter(|e| !keep.contains(e)).map(|(a, b)| format!("[{a}, {b}]")).collect();
    assert_eq!(pairs.len(), 8);
    b.ok(&format!(r#"{{"op": "erase", "shape": "box", "edges": [{}]}}"#, pairs.join(", ")));
    let s = b.shape("box");
    assert_eq!(s.vertices.len(), 8);
    assert!(top.iter().all(|&v| s.neighbours(v).is_empty()));
    assert_eq!(ids(&s), vec![bottom_id]);
    assert!(close(area(&s, bottom_id), 12.0) && loose(&s).is_empty(), "only the bottom is left");
    b.ok(&format!(r#"{{"op": "pushpull", "shape": "box", "face": {bottom_id}, "distance": -2.5}}"#));
    let s = b.shape("box");
    planar_faces(&s);
    assert_eq!(s.vertices.len(), 8, "the copies land on the orphans and reuse them");
    assert!(s.is_closed() && close(s.volume(), 30.0), "{}", s.volume());
}
