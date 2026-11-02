//! Closed half-edge boundary representations with planar polygonal faces.
//!
//! A [`Solid`] stores vertices, half-edges and faces. Every face is a planar polygon given by one
//! outer loop (counter-clockwise seen from outside, i.e. by the right-hand rule around the outward
//! normal) and optional inner loops (holes, clockwise). Each face also carries an edge-to-edge
//! triangulation of itself, computed once when the solid is built and shared by the mass
//! properties and the STL export so that both describe exactly the same surface.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::polygon::{signed_area, Polygon};
use crate::types::{Point2, Point3, Vec3};

/// A vertex of the mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub point: Point3,
    /// One half-edge leaving this vertex, if any.
    pub half_edge: Option<usize>,
}

/// A directed edge along a face loop.
#[derive(Clone, Debug, PartialEq)]
pub struct HalfEdge {
    /// Vertex the half-edge starts at.
    pub origin: usize,
    /// The oppositely directed half-edge of the neighbouring face, if the edge is shared.
    pub twin: Option<usize>,
    /// Next half-edge around the same loop.
    pub next: usize,
    /// Previous half-edge around the same loop.
    pub prev: usize,
    /// Face this half-edge bounds.
    pub face: usize,
}

/// A planar polygonal face.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// One half-edge of each loop; `loops[0]` is the outer boundary, the rest are holes.
    pub loops: Vec<usize>,
    /// Edge-to-edge triangulation of the face as vertex index triples, wound like the outer loop.
    pub triangles: Vec<[usize; 3]>,
}

/// A half-edge mesh. Built by [`Solid::extrude`], [`Solid::sweep`] or [`Solid::from_faces`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Solid {
    pub vertices: Vec<Vertex>,
    pub half_edges: Vec<HalfEdge>,
    pub faces: Vec<Face>,
}

impl Solid {
    /// The empty solid (no vertices, no faces). It is not a closed manifold.
    pub fn new() -> Self {
        Solid::default()
    }

    /// Build a solid from a point list and faces. Each face is a list of loops of point
    /// indices: the first loop is the outer boundary (counter-clockwise seen from outside), the
    /// others are holes (clockwise). Loops with fewer than three indices are ignored. Shared
    /// edges are paired into twins by matching opposite directed edges; the mesh is a closed
    /// manifold when every directed edge appears once and has its opposite.
    pub fn from_faces(points: Vec<Point3>, faces: Vec<Vec<Vec<usize>>>) -> Solid {
        let mut vertices: Vec<Vertex> = points.into_iter().map(|p| Vertex { point: p, half_edge: None }).collect();
        let mut half_edges: Vec<HalfEdge> = Vec::new();
        let mut out_faces: Vec<Face> = Vec::new();
        let mut directed: HashMap<(usize, usize), usize> = HashMap::new();

        for face_loops in faces {
            let face_id = out_faces.len();
            let mut loops = Vec::new();
            for lp in face_loops.iter().filter(|l| l.len() >= 3) {
                let base = half_edges.len();
                let n = lp.len();
                for (k, &v) in lp.iter().enumerate() {
                    half_edges.push(HalfEdge {
                        origin: v,
                        twin: None,
                        next: base + (k + 1) % n,
                        prev: base + (k + n - 1) % n,
                        face: face_id,
                    });
                    directed.entry((v, lp[(k + 1) % n])).or_insert(base + k);
                    if let Some(vert) = vertices.get_mut(v) {
                        vert.half_edge.get_or_insert(base + k);
                    }
                }
                loops.push(base);
            }
            if loops.is_empty() {
                continue;
            }
            out_faces.push(Face { loops, triangles: Vec::new() });
        }

        for i in 0..half_edges.len() {
            let origin = half_edges[i].origin;
            let dest = half_edges[half_edges[i].next].origin;
            half_edges[i].twin = directed.get(&(dest, origin)).copied();
        }

        let mut solid = Solid { vertices, half_edges, faces: out_faces };
        for f in 0..solid.faces.len() {
            solid.faces[f].triangles = solid.triangulate_face(f);
        }
        solid
    }

    /// The vertex indices around a loop, starting at half-edge `start`.
    pub fn loop_vertices(&self, start: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut h = start;
        loop {
            out.push(self.half_edges[h].origin);
            h = self.half_edges[h].next;
            if h == start || out.len() > self.half_edges.len() {
                break;
            }
        }
        out
    }

    /// Outward normal of a face (unnormalised, Newell's method on the outer loop; its length is
    /// twice the loop's area).
    pub fn face_normal(&self, face: usize) -> Vec3 {
        let lp = self.loop_vertices(self.faces[face].loops[0]);
        let o = self.vertices[lp[0]].point;
        let mut n = Vec3::ZERO;
        for i in 1..lp.len().saturating_sub(1) {
            let a = self.vertices[lp[i]].point - o;
            let b = self.vertices[lp[i + 1]].point - o;
            n += a.cross(b);
        }
        n
    }

    /// Edge-to-edge triangulation of a face: the loops are projected onto the face plane (an
    /// isometry) and triangulated as a polygon with holes; no points are added.
    fn triangulate_face(&self, face: usize) -> Vec<[usize; 3]> {
        let n = match self.face_normal(face).normalized() {
            Some(n) => n,
            None => return Vec::new(), // degenerate face
        };
        // In-plane basis (e1, e2) with e1 x e2 = n, so a CCW projected loop faces outward.
        let axis = if n.x.abs() <= n.y.abs() && n.x.abs() <= n.z.abs() {
            Vec3::X
        } else if n.y.abs() <= n.z.abs() {
            Vec3::Y
        } else {
            Vec3::Z
        };
        let e1 = axis.cross(n).normalized().unwrap_or(Vec3::X);
        let e2 = n.cross(e1);

        let loops: Vec<Vec<usize>> = self.faces[face].loops.iter().map(|&h| self.loop_vertices(h)).collect();
        let origin = self.vertices[loops[0][0]].point;
        let project = |v: usize| {
            let d = self.vertices[v].point - origin;
            Point2::new(d.dot(e1), d.dot(e2))
        };
        let outer: Vec<Point2> = loops[0].iter().map(|&v| project(v)).collect();
        let holes: Vec<Vec<Point2>> = loops[1..].iter().map(|l| l.iter().map(|&v| project(v)).collect()).collect();
        let flat: Vec<usize> = loops.iter().flatten().copied().collect();
        Polygon { outer, holes }
            .triangulate()
            .into_iter()
            .map(|t| [flat[t[0]], flat[t[1]], flat[t[2]]])
            .collect()
    }

    /// Every face triangle as three points, wound counter-clockwise seen from outside.
    pub fn triangles(&self) -> Vec<[Point3; 3]> {
        self.faces
            .iter()
            .flat_map(|f| f.triangles.iter())
            .map(|t| [self.vertices[t[0]].point, self.vertices[t[1]].point, self.vertices[t[2]].point])
            .collect()
    }

    // ------------------------------------------------------------------ construction

    /// Extrude a polygon lying in the plane `z = z0` by `height` along +z. The result has a
    /// bottom cap at `z0`, a top cap at `z0 + height` (swapped when `height` is negative) and one
    /// planar quadrilateral per polygon edge, all with outward normals. A polygon with holes
    /// yields caps with inner loops (a solid of genus = number of holes). Ring orientation is
    /// normalised, so a clockwise outer ring or counter-clockwise holes are accepted. Fewer than
    /// three outer vertices give the empty solid.
    pub fn extrude(polygon: &Polygon, z0: f64, height: f64) -> Solid {
        let (rings, pts) = match normalised_rings(polygon) {
            Some(r) => r,
            None => return Solid::new(),
        };
        let (lo, hi) = if height >= 0.0 { (z0, z0 + height) } else { (z0 + height, z0) };
        let bottom: Vec<Point3> = pts.iter().map(|p| Point3::new(p.x, p.y, lo)).collect();
        let top: Vec<Point3> = pts.iter().map(|p| Point3::new(p.x, p.y, hi)).collect();
        loft(&rings, vec![bottom, top])
    }

    /// Sweep a profile along a polyline. The profile's 2D origin is placed on `path[0]` and its
    /// plane is normal to the first segment: `x` and `y` map to the frame `(u, v)` obtained by
    /// rotating `(+X, +Y)` with the minimal rotation taking `+Z` to the first segment direction
    /// (so a path along `+Z` reproduces [`Solid::extrude`]; for a path along `-Z` the frame is
    /// `(+X, -Y)`). At every interior path vertex the section is mitred: the previous section is
    /// projected along the incoming segment onto the bisector plane of the two segments. The
    /// last section lies in the plane normal to the final segment at the last point. Sides are
    /// planar trapezoids, caps are copies of the profile, and the volume equals the profile area
    /// times the path length when the profile centroid is at the 2D origin.
    ///
    /// Consecutive duplicate path points are dropped; a path with fewer than two distinct
    /// points, a path with non-finite coordinates, or a profile with fewer than three outer
    /// vertices, gives the empty solid. A segment reversing the previous one (180° turn) cannot
    /// be mitred and is joined with a section normal to the incoming segment instead; the
    /// result is then a self-overlapping surface (closed, but its legs cancel in the signed
    /// volume). Likewise a path turning so sharply that the mitred sections intersect each other
    /// yields a self-intersecting surface: the mesh is still a closed manifold but the volume is
    /// only meaningful for turns the profile can follow.
    pub fn sweep(polygon: &Polygon, path: &[Point3]) -> Solid {
        let (rings, pts) = match normalised_rings(polygon) {
            Some(r) => r,
            None => return Solid::new(),
        };
        let mut path: Vec<Point3> = path.to_vec();
        path.dedup_by(|b, a| (*b - *a).norm2() == 0.0);
        if path.len() < 2 {
            return Solid::new();
        }
        let dirs: Option<Vec<Vec3>> = path.windows(2).map(|w| (w[1] - w[0]).normalized()).collect();
        let dirs = match dirs {
            Some(d) => d,
            None => return Solid::new(), // non-finite path
        };
        let (u, v) = frame_from_z(dirs[0]);
        let p0 = path[0];
        let first: Vec<Point3> = pts.iter().map(|p| p0 + u * p.x + v * p.y).collect();
        let mut sections = vec![first];
        for k in 1..path.len() {
            let d_in = path[k] - path[k - 1];
            let mut normal = dirs[k - 1];
            if k + 1 < path.len() {
                let d_out = dirs[k];
                let bisector = normal + d_out;
                // A reversal gives a zero bisector; keep the section normal to the incoming leg.
                if bisector.dot(normal) > 1e-12 {
                    normal = bisector;
                }
            }
            let prev = sections.last().unwrap();
            let at = path[k];
            let next: Vec<Point3> = prev
                .iter()
                .map(|&s| {
                    let t = (at - s).dot(normal) / d_in.dot(normal);
                    s + d_in * t
                })
                .collect();
            sections.push(next);
        }
        loft(&rings, sections)
    }

    // ------------------------------------------------------------------ queries

    /// Number of vertices referenced by at least one half-edge.
    pub fn vertex_count(&self) -> usize {
        self.vertices.iter().filter(|v| v.half_edge.is_some()).count()
    }

    /// Number of distinct undirected edges.
    pub fn edge_count(&self) -> usize {
        let mut set = HashSet::new();
        for h in &self.half_edges {
            let a = h.origin;
            let b = self.half_edges[h.next].origin;
            set.insert((a.min(b), a.max(b)));
        }
        set.len()
    }

    /// Number of faces.
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    /// `V - E + F`, counting every face once whatever its number of loops. It is 2 for a solid
    /// of genus 0; an extrusion of a polygon with holes also gives 2 because each hole adds
    /// one inner loop per cap, which offsets its handle (the full Euler–Poincaré formula
    /// `V - E + F - R = 2 - 2G` with `R` inner rings holds with `R = 2h`, `G = h`).
    pub fn euler_characteristic(&self) -> i64 {
        self.vertex_count() as i64 - self.edge_count() as i64 + self.face_count() as i64
    }

    /// Whether the mesh is a closed 2-manifold: every directed edge occurs exactly once and is
    /// paired with its opposite, and the faces around every vertex form a single fan.
    pub fn is_closed_manifold(&self) -> bool {
        if self.faces.is_empty() {
            return false;
        }
        let mut seen = HashSet::new();
        for (i, h) in self.half_edges.iter().enumerate() {
            let dest = self.half_edges[h.next].origin;
            if !seen.insert((h.origin, dest)) || h.origin == dest {
                return false;
            }
            match h.twin {
                Some(t) => {
                    let tw = &self.half_edges[t];
                    if tw.twin != Some(i) || tw.origin != dest || self.half_edges[tw.next].origin != h.origin {
                        return false;
                    }
                }
                None => return false,
            }
            if self.half_edges[h.next].prev != i || self.half_edges[h.prev].next != i {
                return false;
            }
        }
        // Single umbrella around each vertex.
        let mut degree = vec![0usize; self.vertices.len()];
        for h in &self.half_edges {
            degree[h.origin] += 1;
        }
        for (v, vert) in self.vertices.iter().enumerate() {
            let start = match vert.half_edge {
                Some(h) => h,
                None => continue,
            };
            let mut h = start;
            let mut count = 0;
            loop {
                count += 1;
                h = self.half_edges[self.half_edges[h].twin.unwrap()].next;
                if h == start {
                    break;
                }
                if count > degree[v] {
                    return false;
                }
            }
            if count != degree[v] {
                return false;
            }
        }
        true
    }

    /// A reference point near the solid (the vertex average) used to condition the integrals.
    fn reference(&self) -> Vec3 {
        let used: Vec<&Vertex> = self.vertices.iter().filter(|v| v.half_edge.is_some()).collect();
        if used.is_empty() {
            return Vec3::ZERO;
        }
        let sum = used.iter().fold(Vec3::ZERO, |acc, v| acc + v.point.to_vec());
        sum / used.len() as f64
    }

    /// Volume, area and first moments by the divergence theorem over the face triangles.
    /// Returns `(volume, area, moment)` with `moment = ∫ (p - r) dV` and `r` the reference.
    fn integrals(&self) -> (f64, f64, Vec3, Vec3) {
        let r = self.reference();
        let mut vol6 = 0.0;
        let mut area2 = 0.0;
        let mut m = Vec3::ZERO;
        for face in &self.faces {
            for t in &face.triangles {
                let a = self.vertices[t[0]].point.to_vec() - r;
                let b = self.vertices[t[1]].point.to_vec() - r;
                let c = self.vertices[t[2]].point.to_vec() - r;
                let n = (b - a).cross(c - a);
                vol6 += a.dot(n);
                area2 += n.norm();
                // Eberly, "Polyhedral Mass Properties": ∫ x dV = Σ n_x · f2(x) / 24 with
                // f2(w) = w0² + w0 w1 + w1² + w1 w2 + w2² + w2 w0.
                let f2 = |w0: f64, w1: f64, w2: f64| w0 * w0 + w0 * w1 + w1 * w1 + w1 * w2 + w2 * w2 + w2 * w0;
                m.x += n.x * f2(a.x, b.x, c.x);
                m.y += n.y * f2(a.y, b.y, c.y);
                m.z += n.z * f2(a.z, b.z, c.z);
            }
        }
        (vol6 / 6.0, area2 / 2.0, m / 24.0, r)
    }

    /// Enclosed volume (positive for outward-facing faces).
    pub fn volume(&self) -> f64 {
        self.integrals().0
    }

    /// Total surface area.
    pub fn area(&self) -> f64 {
        self.integrals().1
    }

    /// Centroid of the enclosed volume. For a solid of zero volume the vertex average is
    /// returned instead.
    pub fn centroid(&self) -> Point3 {
        let (vol, _, moment, r) = self.integrals();
        if vol == 0.0 {
            return r.to_point();
        }
        (r + moment / vol).to_point()
    }

    /// Write the triangulated surface as an ASCII STL file. Coordinates are printed with
    /// round-trip precision, facets are wound counter-clockwise seen from outside with the
    /// normal computed from that winding, and the triangulation is the edge-to-edge one shared
    /// with the mass properties, so a closed manifold solid gives a watertight STL.
    pub fn to_stl(&self, path: &Path) -> io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut w = BufWriter::new(file);
        self.write_stl(&mut w)?;
        w.flush()
    }

    /// Write ASCII STL to any writer (see [`Solid::to_stl`]).
    pub fn write_stl<W: Write>(&self, w: &mut W) -> io::Result<()> {
        writeln!(w, "solid bim_geom")?;
        for [a, b, c] in self.triangles() {
            let n = (b - a).cross(c - a).normalized().unwrap_or(Vec3::ZERO);
            writeln!(w, "  facet normal {:e} {:e} {:e}", n.x, n.y, n.z)?;
            writeln!(w, "    outer loop")?;
            for p in [a, b, c] {
                writeln!(w, "      vertex {:e} {:e} {:e}", p.x, p.y, p.z)?;
            }
            writeln!(w, "    endloop")?;
            writeln!(w, "  endfacet")?;
        }
        writeln!(w, "endsolid bim_geom")
    }
}

/// The polygon's rings as index lists into its vertex list (outer ++ holes), oriented outer
/// counter-clockwise and holes clockwise; `None` when the outer ring has fewer than three
/// vertices. Holes with fewer than three vertices are dropped.
fn normalised_rings(polygon: &Polygon) -> Option<(Vec<Vec<usize>>, Vec<Point2>)> {
    if polygon.outer.len() < 3 {
        return None;
    }
    let mut rings = Vec::with_capacity(1 + polygon.holes.len());
    let mut outer: Vec<usize> = (0..polygon.outer.len()).collect();
    if signed_area(&polygon.outer) < 0.0 {
        outer.reverse();
    }
    rings.push(outer);
    let mut pts = polygon.outer.clone();
    for h in &polygon.holes {
        if h.len() < 3 {
            continue;
        }
        let base = pts.len();
        let mut idx: Vec<usize> = (base..base + h.len()).collect();
        if signed_area(h) > 0.0 {
            idx.reverse();
        }
        rings.push(idx);
        pts.extend_from_slice(h);
    }
    Some((rings, pts))
}

/// An orthonormal pair `(u, v)` with `u × v = t` for a unit vector `t`: the images of `+X` and
/// `+Y` under the minimal rotation taking `+Z` to `t`. For `t = +Z` this is `(+X, +Y)`; for `t`
/// (nearly) equal to `-Z` the rotation is a half turn about `+X`, giving `(+X, -Y)`.
fn frame_from_z(t: Vec3) -> (Vec3, Vec3) {
    if t.x == 0.0 && t.y == 0.0 {
        return if t.z >= 0.0 { (Vec3::X, Vec3::Y) } else { (Vec3::X, -Vec3::Y) };
    }
    if t.z < -1.0 + 1e-9 {
        return (Vec3::X, -Vec3::Y);
    }
    let k = 1.0 / (1.0 + t.z);
    let u = Vec3::new(1.0 - t.x * t.x * k, -t.x * t.y * k, -t.x);
    let v = Vec3::new(-t.x * t.y * k, 1.0 - t.y * t.y * k, -t.y);
    (u, v)
}

/// Join successive sections of the same profile into a solid: a start cap (loops reversed so it
/// faces backwards), an end cap, and one quadrilateral per profile edge and segment. `rings`
/// index the profile vertices (outer counter-clockwise, holes clockwise), and every section
/// lists the images of those vertices in the same order.
fn loft(rings: &[Vec<usize>], sections: Vec<Vec<Point3>>) -> Solid {
    let per = sections[0].len();
    let m = sections.len();
    let mut points = Vec::with_capacity(per * m);
    for s in &sections {
        debug_assert_eq!(s.len(), per);
        points.extend_from_slice(s);
    }
    let at = |k: usize, i: usize| k * per + i;

    let mut faces: Vec<Vec<Vec<usize>>> = Vec::new();
    // Start cap: every loop reversed, so the outer loop is CCW seen from behind.
    faces.push(rings.iter().map(|r| r.iter().rev().map(|&i| at(0, i)).collect()).collect());
    // End cap.
    faces.push(rings.iter().map(|r| r.iter().map(|&i| at(m - 1, i)).collect()).collect());
    // Sides: for the edge i -> j of a ring, the quad (k,i) (k,j) (k+1,j) (k+1,i) faces outward
    // for both the CCW outer ring and the CW holes.
    for k in 0..m - 1 {
        for r in rings {
            for e in 0..r.len() {
                let i = r[e];
                let j = r[(e + 1) % r.len()];
                faces.push(vec![vec![at(k, i), at(k, j), at(k + 1, j), at(k + 1, i)]]);
            }
        }
    }
    Solid::from_faces(points, faces)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p2(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }
    fn p3(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    fn unit_square() -> Polygon {
        Polygon::new(vec![p2(0.0, 0.0), p2(1.0, 0.0), p2(1.0, 1.0), p2(0.0, 1.0)])
    }

    #[test]
    fn unit_cube() {
        let s = Solid::extrude(&unit_square(), 0.0, 1.0);
        assert_eq!(s.vertex_count(), 8);
        assert_eq!(s.edge_count(), 12);
        assert_eq!(s.face_count(), 6);
        assert_eq!(s.euler_characteristic(), 2);
        assert!(s.is_closed_manifold());
        assert!(close(s.volume(), 1.0));
        assert!(close(s.area(), 6.0));
        let c = s.centroid();
        assert!(close(c.x, 0.5) && close(c.y, 0.5) && close(c.z, 0.5));
        assert_eq!(s.triangles().len(), 12);
        // Negative height extrudes downwards with the same orientation.
        let d = Solid::extrude(&unit_square(), 1.0, -1.0);
        assert!(close(d.volume(), 1.0));
        assert!(close(d.centroid().z, 0.5));
    }

    #[test]
    fn asymmetric_centroid() {
        // L shape: centroid (1.1, 1.1), which differs from the vertex average (4/3, 4/3).
        let l = Polygon::new(vec![p2(0.0, 0.0), p2(3.0, 0.0), p2(3.0, 1.0), p2(1.0, 1.0), p2(1.0, 3.0), p2(0.0, 3.0)]);
        let s = Solid::extrude(&l, 1.0, 2.0);
        let c = s.centroid();
        assert!(close(c.x, 1.1) && close(c.y, 1.1) && close(c.z, 2.0), "{c:?}");
        assert!(close(s.volume(), 10.0));
    }

    #[test]
    fn extrusion_with_holes() {
        let poly = Polygon::with_holes(
            vec![p2(0.0, 0.0), p2(10.0, 0.0), p2(10.0, 6.0), p2(0.0, 6.0)],
            vec![
                vec![p2(1.0, 1.0), p2(1.0, 5.0), p2(4.0, 5.0), p2(4.0, 1.0)],
                vec![p2(6.0, 1.0), p2(6.0, 5.0), p2(9.0, 5.0), p2(9.0, 1.0)],
            ],
        );
        let s = Solid::extrude(&poly, -2.0, 3.0);
        assert!(s.is_closed_manifold());
        assert!(close(s.volume(), poly.area() * 3.0));
        assert!(close(s.area(), 2.0 * poly.area() + poly.perimeter() * 3.0));
        assert!(close(s.centroid().z, -0.5));
        assert_eq!(s.euler_characteristic(), 2);
        assert_eq!(s.face_count(), 2 + 12);
    }

    #[test]
    fn vertical_sweep_equals_extrusion() {
        let poly = Polygon::with_holes(
            vec![p2(-0.3, -0.3), p2(0.3, -0.3), p2(0.3, 0.3), p2(-0.3, 0.3)],
            vec![vec![p2(-0.1, 0.1), p2(0.1, 0.1), p2(0.1, -0.1), p2(-0.1, -0.1)]],
        );
        let sw = Solid::sweep(&poly, &[p3(1.0, 2.0, -1.0), p3(1.0, 2.0, 3.5)]);
        let shifted = Polygon::with_holes(
            poly.outer.iter().map(|p| p2(p.x + 1.0, p.y + 2.0)).collect(),
            poly.holes.iter().map(|h| h.iter().map(|p| p2(p.x + 1.0, p.y + 2.0)).collect()).collect(),
        );
        let ex = Solid::extrude(&shifted, -1.0, 4.5);
        assert_eq!(sw.vertices, ex.vertices);
        assert_eq!(sw.faces, ex.faces);
        assert!(sw.is_closed_manifold());
        assert!(close(sw.volume(), poly.area() * 4.5));
    }

    #[test]
    fn mitred_sweeps() {
        let rect = Polygon::new(vec![p2(-0.25, -0.15), p2(0.25, -0.15), p2(0.25, 0.15), p2(-0.25, 0.15)]);
        let paths: Vec<Vec<Point3>> = vec![
            vec![p3(0.0, 0.0, 0.0), p3(3.0, 4.0, 12.0)],
            vec![p3(0.0, 0.0, 0.0), p3(4.0, 0.0, 0.0), p3(4.0, 3.0, 0.0)],
            vec![p3(0.0, 0.0, 0.0), p3(3.0, 0.0, 0.0), p3(3.0, 2.5, 1.0), p3(3.0, 3.0, 4.0), p3(5.5, 4.0, 5.0)],
            vec![p3(0.0, 0.0, 0.0), p3(0.0, 0.0, -2.0)],
        ];
        for path in &paths {
            let s = Solid::sweep(&rect, path);
            let len: f64 = path.windows(2).map(|w| (w[1] - w[0]).norm()).sum();
            assert!(s.is_closed_manifold(), "{path:?}");
            assert_eq!(s.euler_characteristic(), 2);
            assert!(close(s.volume(), rect.area() * len), "volume {} vs {}", s.volume(), rect.area() * len);
            assert!(close(s.area(), 2.0 * rect.area() + rect.perimeter() * len), "area {}", s.area());
        }
        // Two-point paths have their centroid at the midpoint.
        let s = Solid::sweep(&rect, &paths[0]);
        let c = s.centroid();
        assert!(close(c.x, 1.5) && close(c.y, 2.0) && close(c.z, 6.0));
        // Degenerate paths give the empty solid.
        assert!(Solid::sweep(&rect, &[p3(0.0, 0.0, 0.0)]).faces.is_empty());
        assert!(Solid::sweep(&rect, &[p3(0.0, 0.0, 0.0), p3(0.0, 0.0, 0.0)]).faces.is_empty());
        assert!(Solid::sweep(&rect, &[p3(0.0, 0.0, 0.0), p3(f64::NAN, 0.0, 0.0)]).faces.is_empty());
        // A reversal is joined without a mitre: topologically closed, but the two legs overlap
        // with opposite orientation so the signed volume cancels.
        let back = Solid::sweep(&rect, &[p3(0.0, 0.0, 0.0), p3(2.0, 0.0, 0.0), p3(0.0, 0.0, 0.0)]);
        assert!(back.is_closed_manifold());
        assert!(back.volume().abs() < 1e-12);
        assert!(close(back.area(), 2.0 * rect.area() + rect.perimeter() * 4.0));
        assert!(!Solid::new().is_closed_manifold());
    }

    #[test]
    fn stl_export() {
        let s = Solid::extrude(&unit_square(), 0.0, 1.0);
        let mut buf = Vec::new();
        s.write_stl(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.starts_with("solid bim_geom\n"));
        assert!(text.trim_end().ends_with("endsolid bim_geom"));
        assert_eq!(text.matches("facet normal").count(), 12);
        assert_eq!(text.matches("vertex").count(), 36);
        let dir = std::env::temp_dir().join(format!("bim-geom-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cube.stl");
        s.to_stl(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_mesh_is_not_closed() {
        let pts = vec![p3(0.0, 0.0, 0.0), p3(1.0, 0.0, 0.0), p3(0.0, 1.0, 0.0), p3(0.0, 0.0, 1.0)];
        let open = Solid::from_faces(pts.clone(), vec![vec![vec![0, 2, 1]], vec![vec![0, 1, 3]]]);
        assert!(!open.is_closed_manifold());
        let tet = Solid::from_faces(pts, vec![vec![vec![0, 2, 1]], vec![vec![0, 1, 3]], vec![vec![1, 2, 3]], vec![vec![0, 3, 2]]]);
        assert!(tet.is_closed_manifold());
        assert_eq!(tet.euler_characteristic(), 2);
        assert!(close(tet.volume(), 1.0 / 6.0));
    }
}
