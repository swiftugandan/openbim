//! Exact rational points and the predicates on them used by the boolean kernel.
//!
//! A constructed point (an edge piercing a plane, three planes meeting, two coplanar segments
//! crossing) is kept in homogeneous coordinates `(X, Y, Z, W)` whose entries are exact dyadic
//! numbers ([`crate::big::Dyadic`]: arbitrary-precision integer mantissas, so that products of
//! deeply constructed values never underflow or overflow). Two constructions of the same
//! geometric point therefore compare *equal*, whatever formula produced them, and a constructed
//! point that coincides with an input vertex is detected as such. Every point also carries its
//! correctly rounded `f64` coordinates (used as a hash key and as the output coordinates) and
//! rigorous interval enclosures of them, which let the predicates answer the easy cases in
//! floating point and fall back to exact arithmetic only when the interval straddles zero.

use crate::big::Dyadic;
use crate::types::Point3;

// ---------------------------------------------------------------------------------------------
// Exact number helpers

pub(crate) type Ex = Dyadic;

pub(crate) fn ex(x: f64) -> Ex {
    Dyadic::from_f64(x)
}

pub(crate) fn add(a: &Ex, b: &Ex) -> Ex {
    a.add(b)
}

pub(crate) fn sub(a: &Ex, b: &Ex) -> Ex {
    a.sub(b)
}

pub(crate) fn mul(a: &Ex, b: &Ex) -> Ex {
    a.mul(b)
}

pub(crate) fn scale(a: &Ex, s: f64) -> Ex {
    a.scale(s)
}

pub(crate) fn sign(a: &Ex) -> i32 {
    a.sign()
}

/// `a · b` for exact 3-vectors.
pub(crate) fn dot3(a: &[Ex; 3], b: &[Ex; 3]) -> Ex {
    add(&add(&mul(&a[0], &b[0]), &mul(&a[1], &b[1])), &mul(&a[2], &b[2]))
}

/// `a × b` for exact 3-vectors.
pub(crate) fn cross3(a: &[Ex; 3], b: &[Ex; 3]) -> [Ex; 3] {
    [
        sub(&mul(&a[1], &b[2]), &mul(&a[2], &b[1])),
        sub(&mul(&a[2], &b[0]), &mul(&a[0], &b[2])),
        sub(&mul(&a[0], &b[1]), &mul(&a[1], &b[0])),
    ]
}

pub(crate) fn exact3(p: Point3) -> [Ex; 3] {
    [ex(p.x), ex(p.y), ex(p.z)]
}

/// Exact difference of two input points as a vector.
pub(crate) fn diff3(a: Point3, b: Point3) -> [Ex; 3] {
    [sub(&ex(a.x), &ex(b.x)), sub(&ex(a.y), &ex(b.y)), sub(&ex(a.z), &ex(b.z))]
}

// ---------------------------------------------------------------------------------------------
// Intervals

/// A closed interval of doubles, used as a rigorous enclosure of an exact value. Arithmetic is
/// outward rounded by stepping the round-to-nearest result one ulp outwards.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Iv {
    pub lo: f64,
    pub hi: f64,
}

impl Iv {
    pub fn point(x: f64) -> Iv {
        Iv { lo: x, hi: x }
    }

    /// Enclosure of a rounded-to-nearest value: the true value is within half an ulp.
    pub fn around(x: f64) -> Iv {
        Iv { lo: x.next_down(), hi: x.next_up() }
    }

    pub fn sub(self, o: Iv) -> Iv {
        Iv { lo: (self.lo - o.hi).next_down(), hi: (self.hi - o.lo).next_up() }
    }

    pub fn mul(self, o: Iv) -> Iv {
        let p = [self.lo * o.lo, self.lo * o.hi, self.hi * o.lo, self.hi * o.hi];
        let lo = p.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = p.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Iv { lo: lo.next_down(), hi: hi.next_up() }
    }

    /// The sign of every value in the interval, if it is the same for all of them.
    pub fn sign(self) -> Option<i32> {
        if self.lo > 0.0 {
            Some(1)
        } else if self.hi < 0.0 {
            Some(-1)
        } else if self.lo == 0.0 && self.hi == 0.0 {
            Some(0)
        } else {
            None
        }
    }
}

/// A rigorous enclosure of an exact value: zero exactly, or around its correctly rounded
/// double.
pub(crate) fn iv_of(e: &Ex) -> Iv {
    if e.is_zero() {
        Iv::point(0.0)
    } else {
        Iv::around(e.approx())
    }
}

// ---------------------------------------------------------------------------------------------
// Planes

/// A plane `n · p = d` with exact coefficients, oriented by the triangle it was built from.
#[derive(Clone, Debug)]
pub(crate) struct Plane {
    pub n: [Ex; 3],
    pub d: Ex,
    pub n_iv: [Iv; 3],
    pub d_iv: Iv,
    /// Floating-point normal, for choosing projection axes and orientation tests.
    pub n_approx: [f64; 3],
}

impl Plane {
    /// The plane through three input points, with normal `(b - a) × (c - a)`.
    pub fn through(a: Point3, b: Point3, c: Point3) -> Plane {
        let n = cross3(&diff3(b, a), &diff3(c, a));
        let d = dot3(&n, &exact3(a));
        let n_iv = [iv_of(&n[0]), iv_of(&n[1]), iv_of(&n[2])];
        let d_iv = iv_of(&d);
        let n_approx = [n[0].approx(), n[1].approx(), n[2].approx()];
        Plane { n, d, n_iv, d_iv, n_approx }
    }

    /// The plane through three exact points, with normal `(b - a) × (c - a)` (scaled by the
    /// positive product of the denominators).
    pub fn through_h(a: &HPoint, b: &HPoint, c: &HPoint) -> Plane {
        let u = hdiff(b, a);
        let v = hdiff(c, a);
        let n = cross3(&u, &v);
        // n · p = d with p = a: multiply through by W_a to stay polynomial.
        let d = dot3(&n, &a.x);
        let n = [mul(&n[0], &a.w), mul(&n[1], &a.w), mul(&n[2], &a.w)];
        Plane::from_parts(n, d)
    }

    /// The coordinate plane `p_k = c`, with normal along `+k`.
    pub fn axis(k: usize, c: f64) -> Plane {
        let mut n = [ex(0.0), ex(0.0), ex(0.0)];
        n[k] = ex(1.0);
        Plane::from_parts(n, ex(c))
    }

    fn from_parts(n: [Ex; 3], d: Ex) -> Plane {
        let n_iv = [iv_of(&n[0]), iv_of(&n[1]), iv_of(&n[2])];
        let d_iv = iv_of(&d);
        let n_approx = [n[0].approx(), n[1].approx(), n[2].approx()];
        Plane { n, d, n_iv, d_iv, n_approx }
    }

    /// The same plane with the opposite orientation.
    pub fn negated(&self) -> Plane {
        Plane::from_parts([self.n[0].neg(), self.n[1].neg(), self.n[2].neg()], self.d.neg())
    }

    /// Whether the coefficients are finite (always, the inputs being finite; kept as a guard).
    pub fn is_finite(&self) -> bool {
        self.n.iter().all(|c| c.approx().is_finite()) && self.d.approx().is_finite()
    }

    /// `(d - n · p) · W` for an exact point `p = X / W`: the signed value `d - n · p`
    /// (positive on the inner side, i.e. the side opposite to the normal) scaled by the
    /// positive denominator.
    pub fn eval_h(&self, p: &HPoint) -> Ex {
        sub(&mul(&self.d, &p.w), &dot3(&self.n, &p.x))
    }
}

/// The direction `b - a` of two exact points, scaled by the positive product of their
/// denominators: `X_b W_a - X_a W_b`.
pub(crate) fn hdiff(b: &HPoint, a: &HPoint) -> [Ex; 3] {
    [0, 1, 2].map(|k| sub(&mul(&b.x[k], &a.w), &mul(&a.x[k], &b.w)))
}

// ---------------------------------------------------------------------------------------------
// Homogeneous points

/// An exact point `(X/W, Y/W, Z/W)` with `W > 0`.
#[derive(Clone, Debug)]
pub(crate) struct HPoint {
    pub x: [Ex; 3],
    pub w: Ex,
    /// Correctly rounded affine coordinates.
    pub r: Point3,
    /// Rigorous enclosures of the affine coordinates.
    pub iv: [Iv; 3],
}

impl HPoint {
    pub fn input(p: Point3) -> HPoint {
        HPoint {
            x: exact3(p),
            w: ex(1.0),
            r: p,
            iv: [Iv::point(p.x), Iv::point(p.y), Iv::point(p.z)],
        }
    }

    /// A point from homogeneous coordinates; `None` when `w == 0` or anything is not finite.
    pub fn new(x: [Ex; 3], w: Ex) -> Option<HPoint> {
        let (x, w) = match sign(&w) {
            0 => return None,
            s if s < 0 => ([x[0].neg(), x[1].neg(), x[2].neg()], w.neg()),
            _ => (x, w),
        };
        let rx = round_div(&x[0], &w)?;
        let ry = round_div(&x[1], &w)?;
        let rz = round_div(&x[2], &w)?;
        let enclose = |r: f64, xx: &Ex| if xx.is_zero() { Iv::point(0.0) } else { Iv::around(r) };
        let iv = [enclose(rx, &x[0]), enclose(ry, &x[1]), enclose(rz, &x[2])];
        Some(HPoint { x, w, r: Point3::new(rx, ry, rz), iv })
    }

    /// Where the segment `a -> b` of input points crosses `plane` (the endpoints must lie on
    /// different sides, which the caller guarantees exactly).
    #[cfg(test)]
    pub fn pierce(a: Point3, b: Point3, plane: &Plane) -> Option<HPoint> {
        HPoint::pierce_h(&HPoint::input(a), &HPoint::input(b), plane)
    }

    /// Where the segment `a -> b` of exact points crosses `plane` (the endpoints must lie on
    /// different sides, which the caller guarantees exactly).
    pub fn pierce_h(a: &HPoint, b: &HPoint, plane: &Plane) -> Option<HPoint> {
        let da = plane.eval_h(a);
        let db = plane.eval_h(b);
        // With Da = (d - n·A) W_a and Db likewise: P = (Da X_b - Db X_a) / (Da W_b - Db W_a).
        let x = [0, 1, 2].map(|k| sub(&mul(&da, &b.x[k]), &mul(&db, &a.x[k])));
        let w = sub(&mul(&da, &b.w), &mul(&db, &a.w));
        HPoint::new(x, w)
    }

    /// The common point of three planes; `None` when they are not independent.
    pub fn triple(p1: &Plane, p2: &Plane, p3: &Plane) -> Option<HPoint> {
        let c23 = cross3(&p2.n, &p3.n);
        let det = dot3(&p1.n, &c23);
        if sign(&det) == 0 {
            return None;
        }
        let c31 = cross3(&p3.n, &p1.n);
        let c12 = cross3(&p1.n, &p2.n);
        let x = [0, 1, 2].map(|k| add(&add(&mul(&c23[k], &p1.d), &mul(&c31[k], &p2.d)), &mul(&c12[k], &p3.d)));
        HPoint::new(x, det)
    }

    /// The point at parameter `t` (a double, typically dyadic) along `p -> q`.
    pub fn along(p: &HPoint, q: &HPoint, t: f64) -> Option<HPoint> {
        // (1 - t) P Wq + t Q Wp over Wp Wq.
        let x = [0, 1, 2].map(|k| add(&scale(&mul(&p.x[k], &q.w), 1.0 - t), &scale(&mul(&q.x[k], &p.w), t)));
        HPoint::new(x, mul(&p.w, &q.w))
    }

    /// Intersection of the lines `p q` and `r s`, which are coplanar and cross in the
    /// projection `ax`, computed as the point of `p q` at the crossing parameter.
    pub fn cross_in_plane(p: &HPoint, q: &HPoint, r: &HPoint, s: &HPoint, ax: (usize, usize)) -> Option<HPoint> {
        let h = |a: &HPoint| [a.x[ax.0].clone(), a.x[ax.1].clone(), a.w.clone()];
        let line = cross3(&h(r), &h(s));
        let alpha = mul(&dot3(&line, &h(p)), &q.w);
        let beta = mul(&dot3(&line, &h(q)), &p.w);
        let den = sub(&alpha, &beta);
        if sign(&den) == 0 {
            return None;
        }
        // (-beta P Wq + alpha Q Wp) / ((alpha - beta) Wp Wq).
        let x = [0, 1, 2].map(|k| sub(&mul(&mul(&alpha, &q.x[k]), &p.w), &mul(&mul(&beta, &p.x[k]), &q.w)));
        HPoint::new(x, mul(&den, &mul(&p.w, &q.w)))
    }

    /// The affine coordinate `k` as an exact fraction `(X_k, W)`.
    fn coord(&self, k: usize) -> (&Ex, &Ex) {
        (&self.x[k], &self.w)
    }
}

/// The double nearest to the exact quotient `x / w` (`w > 0`), ties to even; `None` when the
/// estimate is not finite.
fn round_div(x: &Ex, w: &Ex) -> Option<f64> {
    if x.is_zero() {
        return Some(0.0);
    }
    let q0 = x.approx() / w.approx();
    if !q0.is_finite() {
        return None;
    }
    // Sign of x - q w: positive when the true quotient exceeds q.
    let resid = |q: f64| sign(&x.sub(&w.scale(q)));
    // Bracket: q <= true < next_up(q).
    let mut q = q0;
    let mut guard = 0;
    while resid(q) < 0 {
        q = q.next_down();
        guard += 1;
        if guard > 64 {
            return Some(q0);
        }
    }
    while resid(q.next_up()) >= 0 {
        q = q.next_up();
        guard += 1;
        if guard > 64 {
            return Some(q0);
        }
    }
    let up = q.next_up();
    if !up.is_finite() {
        return Some(q);
    }
    // Nearest of q and up: compare 2x with (q + up) w, exactly.
    let mid = x.scale(2.0).sub(&w.scale(q).add(&w.scale(up)));
    Some(match sign(&mid) {
        s if s > 0 => up,
        s if s < 0 => q,
        _ => {
            if q.to_bits() & 1 == 0 {
                q
            } else {
                up
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Predicates

/// Sign of the affine coordinate difference `a_k - b_k`.
pub(crate) fn cmp_coord(a: &HPoint, b: &HPoint, k: usize) -> i32 {
    if let Some(s) = a.iv[k].sub(b.iv[k]).sign() {
        return s;
    }
    let (xa, wa) = a.coord(k);
    let (xb, wb) = b.coord(k);
    sign(&sub(&mul(xa, wb), &mul(xb, wa)))
}

/// Exact equality of two points.
pub(crate) fn same_point(a: &HPoint, b: &HPoint) -> bool {
    (0..3).all(|k| cmp_coord(a, b, k) == 0)
}

/// Sign of `d - n · p`: positive on the inner side of the plane (opposite to its normal), zero
/// on it. Equals the sign of `orient3d(a, b, c, p)` for the plane through `a b c`.
pub(crate) fn plane_sign(pl: &Plane, p: &HPoint) -> i32 {
    let f = pl.d_iv.sub(pl.n_iv[0].mul(p.iv[0])).sub(pl.n_iv[1].mul(p.iv[1])).sub(pl.n_iv[2].mul(p.iv[2]));
    if let Some(s) = f.sign() {
        return s;
    }
    sign(&sub(&mul(&pl.d, &p.w), &dot3(&pl.n, &p.x)))
}

/// Orientation of `a b c` in the projection onto axes `ax` (counter-clockwise positive).
pub(crate) fn orient2d_h(a: &HPoint, b: &HPoint, c: &HPoint, ax: (usize, usize)) -> i32 {
    let (u, v) = ax;
    let f = (b.iv[u].sub(a.iv[u])).mul(c.iv[v].sub(a.iv[v])).sub((b.iv[v].sub(a.iv[v])).mul(c.iv[u].sub(a.iv[u])));
    if let Some(s) = f.sign() {
        return s;
    }
    // det [[Au Av Aw] [Bu Bv Bw] [Cu Cv Cw]] with all W > 0.
    let (au, av, aw) = (&a.x[u], &a.x[v], &a.w);
    let (bu, bv, bw) = (&b.x[u], &b.x[v], &b.w);
    let (cu, cv, cw) = (&c.x[u], &c.x[v], &c.w);
    let m1 = sub(&mul(bv, cw), &mul(bw, cv));
    let m2 = sub(&mul(bu, cw), &mul(bw, cu));
    let m3 = sub(&mul(bu, cv), &mul(bv, cu));
    sign(&add(&sub(&mul(au, &m1), &mul(av, &m2)), &mul(aw, &m3)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    #[test]
    fn rounding_is_canonical_and_exact_on_representable_values() {
        let a = p(1.5, 1.0, 0.5);
        let b = p(1.5, 1.0, 3.5);
        let pl = Plane::through(p(0.0, 0.0, 2.0), p(1.0, 0.0, 2.0), p(0.0, 1.0, 2.0));
        let q = HPoint::pierce(a, b, &pl).unwrap();
        assert_eq!(q.r, p(1.5, 1.0, 2.0));
        // Decimal inputs: the pierce point of a vertical edge through z = 0.7 is exactly (x, y, 0.7).
        let pl = Plane::through(p(0.3, 0.4, 0.7), p(1.2, 0.4, 0.7), p(0.3, 1.2000000000000002, 0.7));
        let q = HPoint::pierce(p(0.8, 1.2, 0.1), p(0.8, 1.2, 1.3), &pl).unwrap();
        assert_eq!(q.r, p(0.8, 1.2, 0.7));
        assert!(same_point(&q, &HPoint::input(p(0.8, 1.2, 0.7))));
    }

    #[test]
    fn different_constructions_agree() {
        // Three planes of a tilted box corner, and the edge of two of them crossing the third.
        let p1 = Plane::through(p(0.1, 0.2, 0.3), p(1.3, 0.25, 0.35), p(0.15, 1.1, 0.4));
        let p2 = Plane::through(p(0.1, 0.2, 0.3), p(0.15, 1.1, 0.4), p(0.2, 0.3, 1.7));
        // A tilted third plane crossing the segment a -> b = p1 ∩ p2.
        let p3 = Plane::through(p(0.0, 0.0, 0.35), p(1.0, 0.0, 0.36), p(0.0, 1.0, 0.34));
        let (a, b) = (p(0.1, 0.2, 0.3), p(0.15, 1.1, 0.4));
        let (ha, hb) = (HPoint::input(a), HPoint::input(b));
        assert!(plane_sign(&p3, &ha) * plane_sign(&p3, &hb) < 0);
        let t = HPoint::triple(&p1, &p2, &p3).unwrap();
        let e = HPoint::pierce(a, b, &p3).unwrap();
        assert!(same_point(&t, &e));
        assert_eq!(t.r, e.r);
        for pl in [&p1, &p2, &p3] {
            assert_eq!(plane_sign(pl, &t), 0);
            assert_eq!(plane_sign(pl, &e), 0);
        }
        // The crossing of two coplanar segments equals the pierce of one of them through a plane
        // containing the other: segment a -> b and the trace of p3 in plane p1.
        let p4 = Plane::through(p(0.5, 0.0, 0.0), p(0.5, 1.0, 0.0), p(0.5, 0.0, 1.0));
        let c = HPoint::triple(&p1, &p3, &p4).unwrap();
        let d = HPoint::pierce(p(0.15, 1.1, 0.4), p(1.3, 0.25, 0.35), &p3).unwrap();
        assert_eq!(plane_sign(&p1, &c), 0);
        assert_eq!(plane_sign(&p3, &c), 0);
        let ax = (1, 2); // p1's normal is mostly along x
        let x = HPoint::cross_in_plane(&ha, &hb, &c, &d, ax).unwrap();
        assert!(same_point(&x, &t));
        assert_eq!(x.r, t.r);
        let y = HPoint::cross_in_plane(&c, &d, &ha, &hb, ax).unwrap();
        assert!(same_point(&y, &t));
        assert_eq!(y.r, t.r);
        let mid = HPoint::along(&ha, &hb, 0.5).unwrap();
        assert_eq!(mid.r, p(0.125, 0.65, 0.35));
        assert_eq!(plane_sign(&p1, &mid), 0);
    }

    #[test]
    fn predicates_match_filter_and_exact() {
        let a = HPoint::input(p(0.1, 0.2, 0.0));
        let b = HPoint::input(p(0.7, 0.2, 0.0));
        let c = HPoint::input(p(0.4, 0.9, 0.0));
        assert_eq!(orient2d_h(&a, &b, &c, (0, 1)), 1);
        assert_eq!(orient2d_h(&a, &c, &b, (0, 1)), -1);
        let m = HPoint::along(&a, &b, 0.5).unwrap();
        assert_eq!(orient2d_h(&a, &b, &m, (0, 1)), 0);
        assert_eq!(cmp_coord(&a, &m, 0), -1);
        assert_eq!(cmp_coord(&m, &b, 0), -1);
        assert_eq!(cmp_coord(&m, &m, 1), 0);
        let pl = Plane::through(p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(0.0, 1.0, 0.0));
        assert_eq!(plane_sign(&pl, &HPoint::input(p(0.3, 0.3, -1.0))), 1);
        assert_eq!(plane_sign(&pl, &HPoint::input(p(0.3, 0.3, 1.0))), -1);
        assert_eq!(plane_sign(&pl, &m), 0);
    }
}
