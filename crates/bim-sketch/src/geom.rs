//! Plain 3D vector helpers over `[f64; 3]`, the equality tolerance of spec/sketch.md and planes
//! with an orthonormal 2D basis for projecting coplanar geometry.

use bim_geom::Point2;

/// A point or vector in the shape's local frame (metres).
pub type V3 = [f64; 3];

/// spec/sketch.md: within 1e-9 m points are equal, and a face is planar when its vertices lie within
/// 1e-9 m of its plane.
pub const EPS: f64 = 1e-9;

/// Tolerance on unit-vector dot products for parallel / perpendicular tests.
pub const ANG_EPS: f64 = 1e-9;

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

pub fn dist(a: V3, b: V3) -> f64 {
    norm(sub(a, b))
}

/// The unit vector along `a`, or `None` for a zero (or non-finite) vector.
pub fn unit(a: V3) -> Option<V3> {
    let n = norm(a);
    if n > 0.0 && n.is_finite() {
        Some(scale(a, 1.0 / n))
    } else {
        None
    }
}

pub fn is_finite(a: V3) -> bool {
    a.iter().all(|x| x.is_finite())
}

/// Whether `p` lies on the open segment `ab` (within [`EPS`] of the line, strictly between the
/// endpoints, further than [`EPS`] from both).
pub fn on_open_segment(p: V3, a: V3, b: V3) -> bool {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    if l2 == 0.0 {
        return false;
    }
    let t = dot(sub(p, a), ab) / l2;
    if t <= 0.0 || t >= 1.0 {
        return false;
    }
    let foot = add(a, scale(ab, t));
    dist(p, foot) <= EPS && dist(p, a) > EPS && dist(p, b) > EPS
}

/// The parameter of the point of the line `ab` closest to `p` (0 at `a`, 1 at `b`).
pub fn param_on_line(p: V3, a: V3, b: V3) -> f64 {
    let ab = sub(b, a);
    dot(sub(p, a), ab) / dot(ab, ab)
}

/// Whether `p` lies on the closed segment `ab` within [`EPS`].
pub fn on_closed_segment(p: V3, a: V3, b: V3) -> bool {
    dist(p, a) <= EPS || dist(p, b) <= EPS || on_open_segment(p, a, b)
}

/// The crossing point of the open segments `ab` and `cd` when they meet at a single point in their
/// interiors (within [`EPS`]), away from all four endpoints. Collinear overlaps give `None`: they
/// are handled by splitting edges at the vertices lying on them.
pub fn segments_cross(a: V3, b: V3, c: V3, d: V3) -> Option<V3> {
    let u = sub(b, a);
    let v = sub(d, c);
    let w = sub(a, c);
    let (uu, uv, vv, uw, vw) = (dot(u, u), dot(u, v), dot(v, v), dot(u, w), dot(v, w));
    let det = uu * vv - uv * uv;
    if !(det > 1e-12 * uu * vv) {
        return None; // parallel (or degenerate)
    }
    let s = (uv * vw - vv * uw) / det;
    let t = (uu * vw - uv * uw) / det;
    if !(s > 0.0 && s < 1.0 && t > 0.0 && t < 1.0) {
        return None;
    }
    let p = add(a, scale(u, s));
    let q = add(c, scale(v, t));
    if dist(p, q) > EPS {
        return None;
    }
    let x = scale(add(p, q), 0.5);
    if [a, b, c, d].iter().any(|&e| dist(x, e) <= EPS) {
        return None;
    }
    Some(x)
}

/// The Newell normal of a closed loop of points (not unit; its length is twice the loop's signed
/// area, its direction follows the loop by the right-hand rule).
pub fn newell(points: &[V3]) -> V3 {
    let mut n = [0.0; 3];
    if points.len() < 3 {
        return n;
    }
    let o = points[0];
    for i in 1..points.len() - 1 {
        let a = sub(points[i], o);
        let b = sub(points[i + 1], o);
        n = add(n, cross(a, b));
    }
    n
}

/// A plane with a right-handed orthonormal frame `(u, v, n)` for projecting points onto it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    /// Unit normal.
    pub n: V3,
    /// `n · p` for every point `p` of the plane.
    pub d: f64,
    /// A point of the plane, the origin of the 2D frame.
    pub o: V3,
    pub u: V3,
    pub v: V3,
}

impl Plane {
    /// The plane through `o` with unit normal `n`.
    pub fn new(n: V3, o: V3) -> Plane {
        let n = unit(n).unwrap_or([0.0, 0.0, 1.0]);
        let axis = {
            let a = [n[0].abs(), n[1].abs(), n[2].abs()];
            let k = if a[0] <= a[1] && a[0] <= a[2] {
                0
            } else if a[1] <= a[2] {
                1
            } else {
                2
            };
            let mut e = [0.0; 3];
            e[k] = 1.0;
            e
        };
        let u = unit(cross(n, axis)).expect("axis is not parallel to n");
        let v = cross(n, u);
        Plane { n, d: dot(n, o), o, u, v }
    }

    /// The plane of a loop of points by its Newell normal; `None` when the loop has no area.
    pub fn from_loop(points: &[V3]) -> Option<Plane> {
        let n = unit(newell(points))?;
        Some(Plane::new(n, points[0]))
    }

    /// Signed distance of `p` from the plane.
    pub fn dist(&self, p: V3) -> f64 {
        dot(self.n, p) - self.d
    }

    /// Whether `p` lies on the plane within [`EPS`].
    pub fn contains(&self, p: V3) -> bool {
        self.dist(p).abs() <= EPS
    }

    /// 2D coordinates of `p` in the plane's frame.
    pub fn project(&self, p: V3) -> Point2 {
        let r = sub(p, self.o);
        Point2::new(dot(r, self.u), dot(r, self.v))
    }

    /// Whether this is the same plane as `other` (either orientation).
    pub fn same(&self, other: &Plane) -> bool {
        dot(self.n, other.n).abs() >= 1.0 - 1e-12 && self.contains(other.o) && other.contains(self.o)
    }
}

/// A deterministic tie-break orientation for a plane normal: the one pointing to +z, else +y, else +x.
pub fn positive(n: V3) -> bool {
    for k in [2, 1, 0] {
        if n[k].abs() > 1e-12 {
            return n[k] > 0.0;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments() {
        let x = segments_cross([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0]).unwrap();
        assert!(dist(x, [1.0, 0.0, 0.0]) < 1e-15);
        assert!(segments_cross([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]).is_none(), "touch, not cross");
        assert!(segments_cross([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 2.0, 0.0]).is_none());
        assert!(segments_cross([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0]).is_none(), "skew");
        assert!(on_open_segment([1.0, 4e-10, 0.0], [0.0, 0.0, 0.0], [2.0, 0.0, 0.0]));
        assert!(!on_open_segment([2.0, 0.0, 0.0], [0.0, 0.0, 0.0], [2.0, 0.0, 0.0]));
        assert!(!on_open_segment([1.0, 2e-9, 0.0], [0.0, 0.0, 0.0], [2.0, 0.0, 0.0]));
    }

    #[test]
    fn planes() {
        let p = Plane::from_loop(&[[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 3.0, 0.0], [0.0, 3.0, 0.0]]).unwrap();
        assert_eq!(p.n, [0.0, 0.0, 1.0]);
        assert!(p.contains([1.0, 1.0, 5e-10]));
        assert!(!p.contains([1.0, 1.0, 2e-9]));
        let q = p.project([4.0, 3.0, 0.0]);
        assert!((q.x * q.x + q.y * q.y - 25.0).abs() < 1e-12);
        // (u, v, n) is right-handed.
        assert!(dist(cross(p.u, p.v), p.n) < 1e-15);
        assert!(positive([0.0, 0.0, 1.0]) && !positive([0.0, 0.0, -1.0]) && positive([0.0, 1.0, 0.0]) && !positive([-1.0, 0.0, 0.0]));
    }
}
