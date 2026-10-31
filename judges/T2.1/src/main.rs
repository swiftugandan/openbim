// Driver for the T2.1 judge: `judge-t2-1 <cases.json> <results.json> <stl-dir>`.
// Floats cross the boundary as IEEE-754 bit patterns (u64) so nothing is lost in text.
use bim_geom::{incircle, orient2d, orient3d, Point2, Point3, Polygon, Solid};
use serde_json::{json, Value as J};
use std::path::Path;

fn f(v: &J) -> f64 {
    f64::from_bits(v.as_u64().expect("float bits"))
}
fn b(x: f64) -> J {
    json!(x.to_bits())
}
fn p2(v: &J) -> Point2 {
    Point2::new(f(&v[0]), f(&v[1]))
}
fn p3(v: &J) -> Point3 {
    Point3::new(f(&v[0]), f(&v[1]), f(&v[2]))
}
fn ring(v: &J) -> Vec<Point2> {
    v.as_array().unwrap().iter().map(p2).collect()
}
fn polygon(v: &J) -> Polygon {
    Polygon { outer: ring(&v["outer"]), holes: v["holes"].as_array().unwrap().iter().map(ring).collect() }
}
fn sign(x: f64) -> i64 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

fn solid_report(s: &Solid, stl: &Path) -> J {
    s.to_stl(stl).expect("to_stl");
    let c = s.centroid();
    json!({
        "volume": b(s.volume()), "area": b(s.area()),
        "centroid": [b(c.x), b(c.y), b(c.z)],
        "closed": s.is_closed_manifold(), "euler": s.euler_characteristic() as i64,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cases: J = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let stl_dir = Path::new(&args[3]);
    let arr = |k: &str| cases[k].as_array().unwrap().clone();

    let o2: Vec<i64> = arr("orient2d").iter().map(|c| sign(orient2d(p2(&c[0]), p2(&c[1]), p2(&c[2])))).collect();
    let o3: Vec<i64> = arr("orient3d").iter().map(|c| sign(orient3d(p3(&c[0]), p3(&c[1]), p3(&c[2]), p3(&c[3])))).collect();
    let ic: Vec<i64> = arr("incircle").iter().map(|c| sign(incircle(p2(&c[0]), p2(&c[1]), p2(&c[2]), p2(&c[3])))).collect();

    let polys: Vec<J> = arr("polygons")
        .iter()
        .map(|c| {
            let poly = polygon(&c["polygon"]);
            let inside: Vec<bool> = c["probes"].as_array().unwrap().iter().map(|p| poly.contains(p2(p))).collect();
            let tris: Vec<[usize; 3]> = poly.triangulate();
            json!({"area": b(poly.area()), "contains": inside, "triangles": tris})
        })
        .collect();

    let extrusions: Vec<J> = arr("extrusions")
        .iter()
        .map(|c| {
            let s = Solid::extrude(&polygon(&c["polygon"]), f(&c["z0"]), f(&c["height"]));
            solid_report(&s, &stl_dir.join(c["stl"].as_str().unwrap()))
        })
        .collect();

    let sweeps: Vec<J> = arr("sweeps")
        .iter()
        .map(|c| {
            let path: Vec<Point3> = c["path"].as_array().unwrap().iter().map(p3).collect();
            let s = Solid::sweep(&polygon(&c["polygon"]), &path);
            solid_report(&s, &stl_dir.join(c["stl"].as_str().unwrap()))
        })
        .collect();

    let out = json!({"orient2d": o2, "orient3d": o3, "incircle": ic, "polygons": polys, "extrusions": extrusions, "sweeps": sweeps});
    std::fs::write(&args[2], out.to_string()).unwrap();
}
