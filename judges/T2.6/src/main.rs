// External judge driver for T2.6 (bim_geom::hlr). Builders must not edit anything under judges/.
// Reads a scene file written by judges/T2.6/oracle.py, builds each solid with Solid::extrude, runs
// hlr and prints one JSON object per scene. All checking is done by the Python oracle.
//
// Scene file lines (whitespace separated):
//   scene <name> <dx> <dy> <dz> <ux> <uy> <uz>          starts a scene (view dir and up)
//   prism <z0> <h> <n> x0 y0 .. x(n-1) y(n-1)           adds solid #k (k counts from 0 per scene), outer CCW
//   end                                                 runs hlr on the scene's solids
use bim_geom::{hlr, OrthoView, Point2, Polygon, Seg2, Solid, Vec3};

fn segs(v: &[Seg2]) -> String {
    let s: Vec<String> = v
        .iter()
        .map(|s| format!("[{:?},{:?},{:?},{:?},{}]", s.a.x, s.a.y, s.b.x, s.b.y, s.solid))
        .collect();
    format!("[{}]", s.join(","))
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: judge-t2-6 <scenes.txt>");
    let text = std::fs::read_to_string(path).unwrap();
    let mut name = String::new();
    let mut view: Option<OrthoView> = None;
    let mut solids: Vec<Solid> = Vec::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        match toks[0] {
            "scene" => {
                name = toks[1].to_string();
                let v: Vec<f64> = toks[2..8].iter().map(|t| t.parse().unwrap()).collect();
                view = Some(OrthoView { dir: Vec3::new(v[0], v[1], v[2]), up: Vec3::new(v[3], v[4], v[5]) });
                solids.clear();
            }
            "prism" => {
                let v: Vec<f64> = toks[1..].iter().map(|t| t.parse().unwrap()).collect();
                let n = v[2] as usize;
                let outer: Vec<Point2> = (0..n).map(|i| Point2::new(v[3 + 2 * i], v[4 + 2 * i])).collect();
                solids.push(Solid::extrude(&Polygon { outer, holes: vec![] }, v[0], v[1]));
            }
            "end" => {
                let r = hlr(&solids, view.as_ref().unwrap());
                println!(
                    "{{\"case\":\"{}\",\"visible\":{},\"hidden\":{}}}",
                    name,
                    segs(&r.visible),
                    segs(&r.hidden)
                );
            }
            other => panic!("bad line kind {other}"),
        }
    }
}
