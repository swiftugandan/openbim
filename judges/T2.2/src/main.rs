// Driver for the T2.2 judge: `judge-t2-2 <cases.json> <results.json> <stl-dir>`.
// Inputs are built with the T2.1 API (extrude / sweep); floats cross as IEEE-754 bits.
use bim_geom::{boolean, BoolOp, Point2, Point3, Polygon, Solid};
use serde_json::{json, Value as J};
use std::path::Path;

fn f(v: &J) -> f64 {
    f64::from_bits(v.as_u64().expect("float bits"))
}
fn ring(v: &J) -> Vec<Point2> {
    v.as_array().unwrap().iter().map(|p| Point2::new(f(&p[0]), f(&p[1]))).collect()
}
fn polygon(v: &J) -> Polygon {
    Polygon { outer: ring(&v["outer"]), holes: v["holes"].as_array().unwrap().iter().map(ring).collect() }
}

fn build(d: &J) -> Solid {
    match d["type"].as_str().unwrap() {
        "extrude" => Solid::extrude(&polygon(&d["polygon"]), f(&d["z0"]), f(&d["height"])),
        "sweep" => {
            let path: Vec<Point3> = d["path"].as_array().unwrap().iter().map(|p| Point3::new(f(&p[0]), f(&p[1]), f(&p[2]))).collect();
            Solid::sweep(&polygon(&d["polygon"]), &path)
        }
        t => panic!("unknown solid type {t}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cases: J = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let dir = Path::new(&args[3]);
    let mut out = Vec::new();
    for (k, c) in cases.as_array().unwrap().iter().enumerate() {
        let a = build(&c["a"]);
        let b = build(&c["b"]);
        a.to_stl(&dir.join(format!("c{k}_a.stl"))).expect("to_stl");
        b.to_stl(&dir.join(format!("c{k}_b.stl"))).expect("to_stl");
        let mut ops = Vec::new();
        for (name, x, y, op) in [
            ("union", &a, &b, BoolOp::Union),
            ("difference", &a, &b, BoolOp::Difference),
            ("difference_ba", &b, &a, BoolOp::Difference),
            ("intersection", &a, &b, BoolOp::Intersection),
        ] {
            let stl = format!("c{k}_{name}.stl");
            ops.push(match boolean(x, y, op) {
                Ok(s) => {
                    s.to_stl(&dir.join(&stl)).expect("to_stl");
                    json!({"op": name, "ok": true, "volume": s.volume().to_bits(), "faces": s.face_count(),
                           "closed": s.is_closed_manifold(), "stl": stl})
                }
                Err(e) => json!({"op": name, "ok": false, "error": format!("{e:?}")}),
            });
        }
        out.push(json!({"a_volume": a.volume().to_bits(), "b_volume": b.volume().to_bits(), "ops": ops}));
        // Progress marker so a hang can be attributed to a case.
        eprintln!("case {k} done");
    }
    std::fs::write(&args[2], J::Array(out).to_string()).unwrap();
}
