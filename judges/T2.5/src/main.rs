// External judge driver for T2.5 (bim_geom::section). Builders must not edit anything under judges/.
// Reads a case file written by judges/T2.5/oracle.py, builds the solids through the T2.1/T2.2 API,
// sections them and prints one JSON object per section case on stdout. All checking is done by the
// Python oracle, which rebuilds the same solids independently with manifold3d/trimesh/shapely.
//
// Case file lines (whitespace separated):
//   prism <id> <z0> <h> <n> x0 y0 .. x(n-1) y(n-1) [hole <m> x0 y0 ..]*   outer CCW, holes CW
//   bool <id> union|difference|intersection <a> <b>
//   section <name> <solid id> <nx> <ny> <nz> <offset>                     plane n.p = offset
use bim_geom::{boolean, section, section_area, BoolOp, Plane3, Point2, Point3, Polygon, Solid, Vec3};
use std::collections::HashMap;

fn pts2(tok: &mut std::slice::Iter<&str>, n: usize) -> Vec<Point2> {
    (0..n)
        .map(|_| {
            let x: f64 = tok.next().unwrap().parse().unwrap();
            let y: f64 = tok.next().unwrap().parse().unwrap();
            Point2::new(x, y)
        })
        .collect()
}

fn p3(p: &Point3) -> String {
    format!("[{:?},{:?},{:?}]", p.x, p.y, p.z)
}

fn ring(r: &[Point3]) -> String {
    format!("[{}]", r.iter().map(p3).collect::<Vec<_>>().join(","))
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: judge-t2-5 <cases.txt>");
    let text = std::fs::read_to_string(path).unwrap();
    let mut solids: HashMap<String, Solid> = HashMap::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        let mut it = toks[1..].iter();
        match toks[0] {
            "prism" => {
                let id = it.next().unwrap().to_string();
                let z0: f64 = it.next().unwrap().parse().unwrap();
                let h: f64 = it.next().unwrap().parse().unwrap();
                let n: usize = it.next().unwrap().parse().unwrap();
                let outer = pts2(&mut it, n);
                let mut holes = Vec::new();
                while let Some(t) = it.next() {
                    assert_eq!(*t, "hole");
                    let m: usize = it.next().unwrap().parse().unwrap();
                    holes.push(pts2(&mut it, m));
                }
                let poly = Polygon { outer, holes };
                solids.insert(id, Solid::extrude(&poly, z0, h));
            }
            "bool" => {
                let id = it.next().unwrap().to_string();
                let op = match *it.next().unwrap() {
                    "union" => BoolOp::Union,
                    "difference" => BoolOp::Difference,
                    "intersection" => BoolOp::Intersection,
                    o => panic!("bad op {o}"),
                };
                let a = &solids[*it.next().unwrap()];
                let b = &solids[*it.next().unwrap()];
                let r = boolean(a, b, op).expect("boolean failed");
                solids.insert(id, r);
            }
            "section" => {
                let name = it.next().unwrap();
                let s = &solids[*it.next().unwrap()];
                let v: Vec<f64> = it.map(|t| t.parse().unwrap()).collect();
                let plane = Plane3 { normal: Vec3::new(v[0], v[1], v[2]), offset: v[3] };
                let polys = section(s, &plane);
                let area = section_area(s, &plane);
                let ps: Vec<String> = polys
                    .iter()
                    .map(|p| {
                        let holes: Vec<String> = p.holes.iter().map(|h| ring(h)).collect();
                        format!("{{\"outer\":{},\"holes\":[{}]}}", ring(&p.outer), holes.join(","))
                    })
                    .collect();
                println!("{{\"case\":\"{}\",\"area\":{:?},\"polys\":[{}]}}", name, area, ps.join(","));
            }
            other => panic!("bad line kind {other}"),
        }
    }
}
