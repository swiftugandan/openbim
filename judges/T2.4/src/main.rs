// External judge runner for T2.4 (bim-solver 3D). Reads the generated cases, builds each one with the
// public Sketch3 API, and prints decompose() + solve() results as JSON for check.py to compare with
// python-solvespace and closed forms. A panic inside one case is reported as that case's error.
use bim_solver::{Constraint3, Plane, Sketch3};
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn f(v: &Value) -> f64 {
    v.as_f64().expect("number")
}
fn u(v: &Value) -> usize {
    v.as_u64().expect("index") as usize
}

fn run(case: &Value) -> Value {
    let mut s = Sketch3::new();
    let ids: Vec<_> = case["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| s.add_point3(f(&p[0]), f(&p[1]), f(&p[2])))
        .collect();
    let datums: Vec<_> = case["datums"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let n = &d["normal"];
            s.add_datum(Plane { normal: [f(&n[0]), f(&n[1]), f(&n[2])], offset: f(&d["offset"]) })
        })
        .collect();
    for c in case["constraints"].as_array().unwrap() {
        let p = |k: usize| ids[u(&c["p"][k])];
        let con = match c["t"].as_str().unwrap() {
            "coincident" => Constraint3::Coincident3(ids[u(&c["a"])], ids[u(&c["b"])]),
            "distance" => Constraint3::Distance3(ids[u(&c["a"])], ids[u(&c["b"])], f(&c["v"])),
            "parallel" => Constraint3::Parallel3(p(0), p(1), p(2), p(3)),
            "perpendicular" => Constraint3::Perpendicular3(p(0), p(1), p(2), p(3)),
            "angle" => Constraint3::Angle3(p(0), p(1), p(2), p(3), f(&c["v"])),
            "on" => Constraint3::OnDatum(ids[u(&c["a"])], datums[u(&c["d"])]),
            t => panic!("unknown constraint {t}"),
        };
        s.add(con);
    }
    let index = |id: &_| ids.iter().position(|q| q == id).expect("subsystem names an unknown point");
    let decompose: Vec<Vec<usize>> =
        s.decompose().iter().map(|sub| sub.points.iter().map(&index).collect()).collect();
    let report = s.solve();
    let subsystems: Vec<Value> = report
        .subsystems
        .iter()
        .map(|r| {
            json!({
                "points": r.points.iter().map(&index).collect::<Vec<_>>(),
                "status": format!("{:?}", r.status),
                "dof": r.dof,
            })
        })
        .collect();
    let positions: Vec<Value> = ids
        .iter()
        .map(|&id| {
            let (x, y, z) = s.point3(id);
            json!([x, y, z])
        })
        .collect();
    json!({"name": case["name"], "decompose": decompose, "subsystems": subsystems, "positions": positions})
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: judge-t2-4 <cases.json>");
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    std::panic::set_hook(Box::new(|_| {}));
    let out: Vec<Value> = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            catch_unwind(AssertUnwindSafe(|| run(c)))
                .unwrap_or_else(|_| json!({"name": c["name"], "error": "panicked"}))
        })
        .collect();
    println!("{}", json!({ "cases": out }));
}
