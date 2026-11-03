use super::*;
use bim_doc::Op;

struct B(Document);

impl B {
    fn add(&mut self, name: &str, kind: &str, props: &[(&str, Value)]) -> &mut B {
        let id = EntityId::derive(name);
        self.0.apply(Op::Create { id, kind: kind.into() }).unwrap();
        self.0.apply(Op::Set { id, key: "_name".into(), value: Value::Text(name.into()) }).unwrap();
        for (k, v) in props {
            self.0.apply(Op::Set { id, key: (*k).into(), value: v.clone() }).unwrap();
        }
        self
    }
    fn set(&mut self, name: &str, key: &str, v: Value) {
        self.0.apply(Op::Set { id: EntityId::derive(name), key: key.into(), value: v }).unwrap();
    }
}

fn n(x: f64) -> Value {
    Value::Num(x)
}
fn r(name: &str) -> Value {
    Value::Ref(EntityId::derive(name))
}

/// One 4 m × 0.2 m × 3 m wall along x on L1 (elevation 1), a 0.3 × 0.4 column, two points and a
/// dimension on the plan.
fn sample() -> B {
    let mut b = B(Document::new());
    b.add("L1", "Level", &[("elevation", n(1.0))]);
    b.add("L2", "Level", &[("elevation", n(4.0))]);
    b.add("WT", "WallType", &[
        ("layer_thickness", Value::List(vec![n(0.2)])),
        ("layer_material", Value::List(vec![Value::Text("brick".into())])),
    ]);
    b.add("W", "Wall", &[
        ("level", r("L1")), ("type", r("WT")),
        ("x1", n(0.0)), ("y1", n(0.0)), ("x2", n(4.0)), ("y2", n(0.0)), ("height", n(3.0)),
    ]);
    b.add("C", "Column", &[
        ("x", n(6.0)), ("y", n(2.0)), ("base", r("L1")), ("top", r("L2")),
        ("section_b", n(0.3)), ("section_h", n(0.4)),
    ]);
    b.add("P1", "Point", &[("x", n(0.0)), ("y", n(-1.0))]);
    b.add("P2", "Point", &[("x", n(4.0)), ("y", n(-1.0)), ("z", n(0.0))]);
    b.add("D", "Dimension", &[("a", r("P1")), ("b", r("P2")), ("view", Value::Text("plan:L1@1.2".into()))]);
    b
}

fn area(rings: &[Vec<P2>]) -> f64 {
    rings
        .iter()
        .map(|r| {
            let mut s = 0.0;
            for i in 0..r.len() {
                let (a, b) = (r[i], r[(i + 1) % r.len()]);
                s += a[0] * b[1] - b[0] * a[1];
            }
            0.5 * s
        })
        .sum::<f64>()
        .abs()
}

#[test]
fn view_parsing() {
    let b = sample();
    let doc = &b.0;
    let v = View::parse(doc, "plan:L1@1.2").unwrap();
    assert!(matches!(v.kind, ViewKind::Plan { z, .. } if (z - 2.2).abs() < 1e-12));
    assert!(matches!(View::parse(doc, "section:0,0,3,4").unwrap().kind, ViewKind::Section { d, .. } if (d[0] - 0.6).abs() < 1e-12));
    for side in ["n", "s", "e", "w"] {
        assert!(View::parse(doc, &format!("elevation:{side}")).is_ok());
    }
    for bad in [
        "plan:NOPE@1.2", "plan:W@1", "plan:L1", "plan:L1@x", "plan:L1@inf", "section:1,2,3", "section:1,2,1,2",
        "section:1,2,3,4,5", "section:a,b,c,d", "elevation:q", "elevation:", "isometric", "", "plan:",
    ] {
        assert!(View::parse(doc, bad).is_err(), "{bad}");
    }
}

#[test]
fn view_numbers_reject_whitespace() {
    let b = sample();
    let doc = &b.0;
    for bad in [
        "plan:L1@ 1", "plan:L1@1 ", "plan:L1@\t1", "section: 0,0,1,0", "section:0, 0,1,0", "section:0,0,1,0 ",
        "section:0,0 ,1,0", "section:0,0,1,\n0",
    ] {
        assert!(View::parse(doc, bad).is_err(), "{bad:?}");
    }
    assert!(View::parse(doc, "plan:L1@1").is_ok());
    assert!(View::parse(doc, "section:0,0,1,0").is_ok());
}

#[test]
fn section_length_overflow_is_not_called_zero() {
    let b = sample();
    let doc = &b.0;
    for bad in ["section:1e308,0,-1e308,0", "section:0,1.7e308,0,-1.7e308", "section:-1.7e308,-1.7e308,1.7e308,1.7e308"] {
        let e = View::parse(doc, bad).unwrap_err();
        assert!(e.0.contains("out of range") && !e.0.contains("zero length"), "{bad}: {e}");
    }
    let e = View::parse(doc, "section:1,2,1,2").unwrap_err();
    assert!(e.0.contains("zero length"), "{e}");
}

#[test]
fn projection_frames_match_the_spec() {
    let b = sample();
    let p = Point3::new(1.0, 2.0, 3.0);
    let cases = [
        ("plan:L1@1", [1.0, 2.0]),
        ("elevation:s", [1.0, 3.0]),
        ("elevation:n", [-1.0, 3.0]),
        ("elevation:e", [2.0, 3.0]),
        ("elevation:w", [-2.0, 3.0]),
        // u = (p - p0)·d up to a constant: d = (0.6, 0.8), so u = 0.6 + 1.6 = 2.2 (+ const).
        ("section:0,0,3,4", [2.2, 3.0]),
    ];
    for (view, want) in cases {
        let v = View::parse(&b.0, view).unwrap();
        let got = v.project(p);
        let o = v.project(Point3::ORIGIN);
        assert!((got[0] - o[0] - want[0]).abs() < 1e-12 && (got[1] - o[1] - want[1]).abs() < 1e-12, "{view}: {got:?}");
        // Viewing direction: the plan looks down, the section to the left of its line.
        let f = frame(&v.ortho())[0];
        let wantf = match view {
            "plan:L1@1" => Vec3::new(0.0, 0.0, -1.0),
            "elevation:s" => Vec3::Y,
            "elevation:n" => -Vec3::Y,
            "elevation:e" => -Vec3::X,
            "elevation:w" => Vec3::X,
            _ => Vec3::new(-0.8, 0.6, 0.0),
        };
        assert!((f - wantf).norm() < 1e-12, "{view}");
    }
}

#[test]
fn plan_cut_hatch_beyond_and_dimension() {
    let b = sample();
    let model = Model::compile(&b.0);
    let v = View::parse(&b.0, "plan:L1@1.2").unwrap();
    let d = draw(&b.0, &model, &v);
    let cut: Vec<&Item> = d.items.iter().filter(|i| i.layer == Layer::Cut).collect();
    assert_eq!(cut.len(), 2);
    for it in &cut {
        let Shape::Region(rings) = &it.shape else { panic!() };
        let want = if it.entity == "W" { 0.8 } else { 0.12 };
        assert!((area(rings) - want).abs() < 1e-9, "{}: {}", it.entity, area(rings));
        // Hatched, and every hatch point inside the region.
        let h = d.items.iter().find(|i| i.layer == Layer::Hatch && i.entity == it.entity).expect("hatch");
        let Shape::Lines(segs) = &h.shape else { panic!() };
        assert!(!segs.is_empty());
        for s in segs {
            let m = [(s[0][0] + s[1][0]) / 2.0, (s[0][1] + s[1][1]) / 2.0];
            assert!(inside(rings, m));
        }
    }
    // Beyond: only the bottom of the wall and of the column, at the outline; the outline itself is
    // on CUT, so the cap edges are dropped and nothing above the cut appears.
    for it in d.items.iter().filter(|i| matches!(i.layer, Layer::Projection | Layer::Hidden)) {
        let Shape::Lines(segs) = &it.shape else { panic!() };
        for s in segs {
            for p in s {
                assert!(p[0] > -0.11 && p[0] < 6.16 && p[1] > -0.11 && p[1] < 2.21, "{p:?}");
            }
        }
    }
    let dim = d.items.iter().find(|i| i.layer == Layer::Dimension).expect("dimension");
    assert_eq!(dim.entity, "D");
    assert!(matches!(&dim.shape, Shape::Dimension { text, .. } if text == "4000"));
    // Associative: moving a point changes the text.
    let mut b2 = sample();
    b2.set("P2", "x", n(4.2506));
    b2.set("P2", "z", n(0.0));
    let d2 = draw(&b2.0, &Model::compile(&b2.0), &v);
    let dim = d2.items.iter().find(|i| i.layer == Layer::Dimension).unwrap();
    assert!(matches!(&dim.shape, Shape::Dimension { text, .. } if text == "4251"));
    // Other views do not show it.
    let e = draw(&b.0, &model, &View::parse(&b.0, "elevation:s").unwrap());
    assert!(e.items.iter().all(|i| i.layer != Layer::Dimension && i.layer != Layer::Cut));
}

#[test]
fn plan_of_a_column_room_shows_what_lies_below() {
    // An upper plan cuts only the column; the wall (below the cut, 1..3) is beyond and visible.
    let mut b = sample();
    b.set("W", "height", n(2.0));
    let model = Model::compile(&b.0);
    let v = View::parse(&b.0, "plan:L1@2.5").unwrap();
    let d = draw(&b.0, &model, &v);
    let cut: Vec<&str> = d.items.iter().filter(|i| i.layer == Layer::Cut).map(|i| i.entity.as_str()).collect();
    assert_eq!(cut, ["C"]);
    let w = d.items.iter().find(|i| i.layer == Layer::Projection && i.entity == "W").expect("wall top beyond");
    let Shape::Lines(segs) = &w.shape else { panic!() };
    let len: f64 = segs.iter().map(|s| (s[1][0] - s[0][0]).hypot(s[1][1] - s[0][1])).sum();
    // Top and bottom outlines are both visible (the rays run along the side faces) and project
    // onto the same rectangle, drawn once.
    assert!((len - 2.0 * (4.0 + 0.2)).abs() < 1e-9, "{len}");
}

#[test]
fn elevation_has_visible_and_hidden_edges() {
    // The column moved behind the wall (seen from the south): the wall is visible, the column's
    // edges below the wall top are hidden; seen from the north the column is in front.
    let mut b = sample();
    b.set("C", "x", n(2.0));
    b.set("C", "top", r("L1"));
    b.add("L3", "Level", &[("elevation", n(5.0))]);
    b.set("C", "top", r("L3"));
    let model = Model::compile(&b.0);
    let d = draw(&b.0, &model, &View::parse(&b.0, "elevation:s").unwrap());
    assert!(d.items.iter().any(|i| i.layer == Layer::Projection && i.entity == "W"));
    assert!(d.items.iter().any(|i| i.layer == Layer::Hidden && i.entity == "C"));
    assert!(d.items.iter().any(|i| i.layer == Layer::Projection && i.entity == "C"));
    let d = draw(&b.0, &model, &View::parse(&b.0, "elevation:n").unwrap());
    assert!(d.items.iter().all(|i| !(i.layer == Layer::Hidden && i.entity == "C")));
}

#[test]
fn section_clips_to_the_far_side() {
    // Section along y = 2 looking +y (left of +x): cuts the column only; the wall (y = 0) is on
    // the viewer's side and not drawn at all.
    let b = sample();
    let model = Model::compile(&b.0);
    let v = View::parse(&b.0, "section:-1,2,9,2").unwrap();
    let d = draw(&b.0, &model, &v);
    assert!(d.items.iter().all(|i| i.entity != "W"), "{:?}", d.items.iter().map(|i| &i.entity).collect::<Vec<_>>());
    let c = d.items.iter().find(|i| i.layer == Layer::Cut).unwrap();
    let Shape::Region(rings) = &c.shape else { panic!() };
    assert!((area(rings) - 0.3 * 3.0).abs() < 1e-9);
    let svg = svg::to_svg(&d);
    assert!(svg.contains("<g id=\"CUT\"") && svg.contains("data-entity=\"C\"") && svg.contains("stroke-width=\"0.5\""));
}

#[test]
fn hatch_covers_small_regions() {
    let ring = vec![[0.0, 0.0], [0.01, 0.0], [0.01, 0.01], [0.0, 0.01]];
    let h = hatch_lines(&[ring.clone()], 0.15);
    assert!(!h.is_empty());
    for s in &h {
        for p in s {
            assert!(p[0] >= -1e-12 && p[0] <= 0.01 + 1e-12 && p[1] >= -1e-12 && p[1] <= 0.01 + 1e-12);
        }
    }
    // A ring with a hole: no hatch midpoint inside the hole.
    let hole = vec![[0.3, 0.3], [0.3, 0.7], [0.7, 0.7], [0.7, 0.3]];
    let outer = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    for s in hatch_lines(&[outer, hole], 0.05) {
        let m = [(s[0][0] + s[1][0]) / 2.0, (s[0][1] + s[1][1]) / 2.0];
        assert!(!(m[0] > 0.3 && m[0] < 0.7 && m[1] > 0.3 && m[1] < 0.7));
    }
}

#[test]
fn faces_in_the_cut_plane_are_beyond_it() {
    // Cut exactly at the wall top (z = 4): the wall is not cut, its top is drawn beyond; the
    // column (1..4) likewise. Cut exactly at the wall bottom (z = 1): both are cut.
    let b = sample();
    let model = Model::compile(&b.0);
    let d = draw(&b.0, &model, &View::parse(&b.0, "plan:L1@3").unwrap());
    assert!(d.items.iter().all(|i| i.layer != Layer::Cut));
    assert!(d.items.iter().any(|i| i.layer == Layer::Projection && i.entity == "W"));
    let d = draw(&b.0, &model, &View::parse(&b.0, "plan:L1@0").unwrap());
    let mut cut: Vec<(&str, f64)> = d
        .items
        .iter()
        .filter_map(|i| match (&i.layer, &i.shape) {
            (Layer::Cut, Shape::Region(r)) => Some((i.entity.as_str(), area(r))),
            _ => None,
        })
        .collect();
    cut.sort_by(|a, b| a.0.cmp(b.0));
    assert_eq!(cut.len(), 2, "{cut:?}");
    assert!((cut[0].1 - 0.12).abs() < 1e-5 && (cut[1].1 - 0.8).abs() < 1e-5, "{cut:?}");
    // A section along the wall's face looking into it cuts the wall's face (it is beyond the
    // plane, touching): it is not cut, and it is visible.
    let d = draw(&b.0, &model, &View::parse(&b.0, "section:-1,-0.1,9,-0.1").unwrap());
    assert!(d.items.iter().all(|i| !(i.layer == Layer::Cut && i.entity == "W")));
    assert!(d.items.iter().any(|i| i.layer == Layer::Projection && i.entity == "W"));
}
