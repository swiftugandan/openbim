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

    fn wall(&mut self, name: &str, level: &str, ty: &str, p: [f64; 4]) -> &mut B {
        self.add(name, "Wall", &[
            ("level", r(level)), ("type", r(ty)), ("height", n(2.7)),
            ("x1", n(p[0])), ("y1", n(p[1])), ("x2", n(p[2])), ("y2", n(p[3])),
        ])
    }

    fn rect(&mut self, prefix: &str, level: &str, ty: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> &mut B {
        self.wall(&format!("{prefix}0"), level, ty, [x0, y0, x1, y0])
            .wall(&format!("{prefix}1"), level, ty, [x1, y1, x1, y0])
            .wall(&format!("{prefix}2"), level, ty, [x1, y1, x0, y1])
            .wall(&format!("{prefix}3"), level, ty, [x0, y0, x0, y1])
    }

    fn solve(&self) -> Json {
        ArchEngine::new().solve(&EngineContext::new(&self.0)).unwrap()
    }
}

fn n(x: f64) -> Value {
    Value::Num(x)
}
fn r(name: &str) -> Value {
    Value::Ref(EntityId::derive(name))
}
fn nl(xs: &[f64]) -> Value {
    Value::List(xs.iter().map(|&x| Value::Num(x)).collect())
}

fn base() -> B {
    let mut b = B(Document::new());
    b.add("L0", "Level", &[("elevation", n(0.0))])
        .add("L1", "Level", &[("elevation", n(3.0))])
        .add("EXT", "WallType", &[("layer_thickness", nl(&[0.1, 0.2])), ("layer_material", Value::List(vec![
            Value::Text("brick".into()), Value::Text("block".into())]))])
        .add("INT", "WallType", &[("layer_thickness", nl(&[0.15])), ("layer_material", Value::List(vec![Value::Text("stud".into())]))]);
    b
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

fn f(v: &Json) -> f64 {
    v.as_f64().unwrap()
}

#[test]
fn the_engine_is_arch() {
    assert_eq!(ArchEngine::new().name(), "arch");
    let empty = B(Document::new()).solve();
    assert_eq!(empty.to_string(), r#"{"rooms":[],"walls":[],"slabs":[],"stairs":[],"curtain_walls":[]}"#);
}

#[test]
fn two_rooms_with_a_t_partition_slab_above_and_spaces() {
    let mut b = base();
    b.rect("a", "L0", "EXT", 0.0, 0.0, 8.0, 5.0)
        .wall("p", "L0", "INT", [3.0, 5.0, 3.0, 0.0])
        .add("S", "Slab", &[("level", r("L1")), ("boundary", nl(&[-1.0, -1.0, 20.0, -1.0, 20.0, 10.0, -1.0, 10.0])), ("thickness", n(0.25))])
        .add("K", "Space", &[("level", r("L0")), ("x", n(1.5)), ("y", n(2.5)), ("name", Value::Text("kitchen".into()))])
        .add("Wx", "Space", &[("level", r("L0")), ("x", n(0.05)), ("y", n(2.5))]); // inside a wall
    let out = b.solve();
    let rooms = out["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 2, "{out}");
    let (a1, a2) = ((3.0 - 0.15 - 0.075) * 4.7, (8.0 - 3.0 - 0.15 - 0.075) * 4.7);
    assert_eq!(rooms[0]["id"], "L0/1");
    assert_eq!(rooms[0]["level"], "L0");
    assert!(close(f(&rooms[0]["area"]), a1) && close(f(&rooms[1]["area"]), a2), "{out}");
    assert!(close(f(&rooms[0]["volume"]), a1 * 2.75));
    assert_eq!(rooms[0]["space"], "K");
    assert_eq!(rooms[1]["id"], "L0/2");
    assert!(rooms[1]["space"].is_null());
    assert_eq!(rooms[0]["boundary"].as_array().unwrap().len(), 4);
    // Walls and slabs are still reported; the layer volumes sum to the wall volume.
    assert_eq!(out["walls"].as_array().unwrap().len(), 5);
    for w in out["walls"].as_array().unwrap() {
        let sum: f64 = w["layers"].as_array().unwrap().iter().map(|l| f(&l["volume"])).sum();
        assert!(close(sum, f(&w["volume"])));
    }
    assert_eq!(out["slabs"][0]["id"], "S");
}

#[test]
fn ties_in_x_order_by_y_and_top_level_volume_is_null() {
    let mut b = base();
    b.rect("b", "L1", "EXT", 0.0, 0.0, 10.0, 6.0)
        .wall("v", "L1", "INT", [4.0, 0.0, 4.0, 6.0])
        .wall("h", "L1", "INT", [10.0, 2.5, 0.0, 2.5])
        .wall("stub", "L1", "INT", [7.0, 6.0, 7.0, 4.5]);
    let out = b.solve();
    let rooms = out["rooms"].as_array().unwrap();
    let ids: Vec<&str> = rooms.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["L1/1", "L1/2", "L1/3", "L1/4"]);
    let c = |k: usize, i: usize| rooms[k]["boundary"].as_array().unwrap().iter().map(|p| f(&p[i])).sum::<f64>() / 4.0;
    assert!(c(0, 0) < 4.0 && c(1, 0) < 4.0 && c(0, 1) < c(1, 1));
    assert!(c(2, 0) > 4.0 && c(2, 1) < c(3, 1));
    // The stub does not split the top-right room, whose top side is two collinear edges.
    assert!(close(f(&rooms[3]["area"]), (10.0 - 4.0 - 0.15 - 0.075) * (6.0 - 2.5 - 0.15 - 0.075)));
    assert!(rooms.iter().all(|r| r["volume"].is_null()));
    assert_eq!(b.solve(), out, "deterministic");
}

#[test]
fn an_isolated_wall_and_walls_on_other_levels_give_no_rooms() {
    let mut b = base();
    b.wall("solo", "L0", "EXT", [1.0, 1.0, 5.0, 4.0]).rect("x", "L1", "EXT", 0.0, 0.0, 0.2, 0.2);
    let out = b.solve();
    // The 0.2 square on L1 is narrower than its walls: no room.
    assert_eq!(out["rooms"], Json::Array(Vec::new()), "{out}");
}

#[test]
fn ordering_groups_near_equal_x() {
    let room = |x: f64, y: f64| Room { boundary: Vec::new(), area: 1.0, centroid: [x, y] };
    let mut rs = vec![room(1.0 + 5e-7, 0.0), room(1.0, 2.0), room(0.5, 9.0), room(1.0 + 9e-7, 1.0)];
    order(&mut rs);
    let got: Vec<[f64; 2]> = rs.iter().map(|r| r.centroid).collect();
    assert_eq!(got, vec![[0.5, 9.0], [1.0 + 5e-7, 0.0], [1.0 + 9e-7, 1.0], [1.0, 2.0]]);
}

#[test]
fn stairs_and_curtain_walls_are_listed_with_their_quantities() {
    let mut b = base();
    let stair = |b: &mut B, name: &str, risers: f64, tread: f64| {
        b.add(name, "Stair", &[
            ("base", r("L0")), ("top", r("L1")), ("x", n(2.0)), ("y", n(1.0)), ("angle", n(0.5)),
            ("width", n(1.2)), ("riser_count", n(risers)), ("tread", n(tread)),
        ]);
    };
    let wall = |b: &mut B, name: &str, spacing: f64| {
        b.add(name, "CurtainWall", &[
            ("level", r("L0")), ("x1", n(1.0)), ("y1", n(2.0)), ("x2", n(5.0)), ("y2", n(5.0)), ("height", n(3.0)),
            ("u_spacing", n(spacing)), ("v_spacing", n(1.25)), ("mullion_width", n(0.05)), ("mullion_depth", n(0.15)),
            ("panel_thickness", n(0.024)),
        ]);
    };
    stair(&mut b, "st-b", 18.0, 0.28);
    stair(&mut b, "st-a", 7.0, 0.3);
    stair(&mut b, "st-bad", 2.5, 0.3); // not a whole number of risers: not listed
    wall(&mut b, "cw-b", 1.5);
    wall(&mut b, "cw-a", 5.0); // one bay along, three up
    wall(&mut b, "cw-bad", 0.0); // no spacing: not listed
    b.add("ramp", "Ramp", &[
        ("base", r("L0")), ("top", r("L1")), ("x", n(0.0)), ("y", n(0.0)), ("angle", n(0.0)),
        ("width", n(1.0)), ("length", n(10.0)),
    ]);
    let out = b.solve();
    let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["rooms", "walls", "slabs", "stairs", "curtain_walls"]);

    let stairs = out["stairs"].as_array().unwrap();
    assert_eq!(stairs.iter().map(|s| s["id"].as_str().unwrap()).collect::<Vec<_>>(), ["st-a", "st-b"]);
    assert_eq!(stairs[0].as_object().unwrap().keys().collect::<Vec<_>>(), ["id", "riser", "tread", "risers"]);
    assert!(close(f(&stairs[0]["riser"]), 3.0 / 7.0) && close(f(&stairs[0]["tread"]), 0.3));
    assert_eq!(stairs[0]["risers"].as_u64(), Some(7));
    assert!(close(f(&stairs[1]["riser"]), 3.0 / 18.0) && close(f(&stairs[1]["tread"]), 0.28));
    assert_eq!(stairs[1]["risers"].as_u64(), Some(18));

    let walls = out["curtain_walls"].as_array().unwrap();
    assert_eq!(walls.iter().map(|c| c["id"].as_str().unwrap()).collect::<Vec<_>>(), ["cw-a", "cw-b"]);
    assert_eq!(walls[0].as_object().unwrap().keys().collect::<Vec<_>>(), ["id", "panels", "mullion_length"]);
    // 4 × 3 bays along a 5 m wall that is 3 m high: (4 + 1)·3 + (3 + 1)·5.
    assert_eq!(walls[1]["panels"].as_u64(), Some(12));
    assert!(close(f(&walls[1]["mullion_length"]), 35.0));
    // 1 × 3 bays: (1 + 1)·3 + (3 + 1)·5.
    assert_eq!(walls[0]["panels"].as_u64(), Some(3));
    assert!(close(f(&walls[0]["mullion_length"]), 26.0));
    assert!(out["rooms"].as_array().unwrap().is_empty() && out.get("ramps").is_none());
    assert_eq!(b.solve(), out, "deterministic");
}
