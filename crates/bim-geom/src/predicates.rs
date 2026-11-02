//! Robust geometric predicates: `orient2d`, `orient3d`, `incircle`.
//!
//! Each predicate is evaluated in two stages, after Shewchuk ("Adaptive Precision
//! Floating-Point Arithmetic and Fast Robust Geometric Predicates", 1997):
//!
//! 1. a fast floating-point evaluation together with a forward error bound; when the
//!    magnitude of the result exceeds the bound its sign is provably correct and it is returned;
//! 2. otherwise the determinant is recomputed *exactly* with floating-point expansions
//!    (non-overlapping sums of `f64` components, Shewchuk's `fast_expansion_sum` and
//!    `scale_expansion` with zero elimination) and the sign of the exact value is returned.
//!
//! The returned `f64` is only an approximation of the determinant's magnitude, but its sign is
//! exact for all coordinates in `{0} ∪ [2^-100, 2^100]` (in magnitude). The argument:
//!
//! * Expansion arithmetic is exact as long as no operation overflows and no non-zero
//!   intermediate quantity underflows (round-off terms of `two_sum` / `two_product` are only
//!   representable then). `two_product` is implemented with a fused multiply-add, whose error
//!   term is exact under the same condition.
//! * Every input coordinate is either 0 or a double of magnitude ≥ 2^-100, hence an integer
//!   multiple of 2^-152 (the ulp at 2^-100). Every exact difference of two inputs, and every
//!   component produced by splitting such a difference into a two-term expansion, is therefore
//!   an integer multiple of 2^-152 as well; a non-zero one has magnitude ≥ 2^-152.
//! * Products of two differences are multiples of 2^-304, products of three (`orient3d`) of
//!   2^-456, and `incircle` multiplies a lift `x²+y²` (multiple of 2^-304) by a 2×2 minor
//!   (multiple of 2^-304), i.e. multiples of 2^-608. Non-zero components therefore stay above
//!   2^-608 ≫ 2^-1022, so nothing ever becomes subnormal and no bit is lost.
//! * In the other direction, differences are < 2^102, products of two < 2^204, the lifts
//!   < 2^205, products of three < 2^306 and the incircle terms < 2^410, far below 2^1024, so
//!   nothing overflows either.
//!
//! Outside that range the predicates still never panic: they return a value whose sign is
//! correct unless an intermediate quantity over/underflowed. NaN or infinite inputs yield NaN.

use crate::types::{Point2, Point3};

/// Machine epsilon for `f64` in Shewchuk's sense: 2^-53 (half the distance from 1 to the next
/// double). Note that `f64::EPSILON` is 2^-52.
const EPSILON: f64 = 1.110_223_024_625_156_5e-16;

/// Forward error bounds of the floating-point filters, from Shewchuk's `exactinit`.
const CCW_ERR_BOUND_A: f64 = (3.0 + 16.0 * EPSILON) * EPSILON;
const O3D_ERR_BOUND_A: f64 = (7.0 + 56.0 * EPSILON) * EPSILON;
const ICC_ERR_BOUND_A: f64 = (10.0 + 96.0 * EPSILON) * EPSILON;

// ---------------------------------------------------------------------------------------------
// Error-free transformations

/// `a + b` as the exact two-term expansion `(sum, round-off)`.
#[inline]
fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bvirt = x - a;
    let avirt = x - bvirt;
    let bround = b - bvirt;
    let around = a - avirt;
    (x, around + bround)
}

/// `a + b` when `|a| >= |b|` (or `a == 0`): cheaper than [`two_sum`].
#[inline]
fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bvirt = x - a;
    (x, b - bvirt)
}

/// `a - b` as the exact two-term expansion `(difference, round-off)`.
#[inline]
fn two_diff(a: f64, b: f64) -> (f64, f64) {
    let x = a - b;
    let bvirt = a - x;
    let avirt = x + bvirt;
    let bround = bvirt - b;
    let around = a - avirt;
    (x, around + bround)
}

/// `a * b` as the exact two-term expansion `(product, round-off)`; the round-off is obtained
/// with a fused multiply-add, which is exact whenever the product does not underflow.
#[inline]
fn two_product(a: f64, b: f64) -> (f64, f64) {
    let x = a * b;
    let y = a.mul_add(b, -x);
    (x, y)
}

// ---------------------------------------------------------------------------------------------
// Expansions

/// A floating-point expansion: the exact value is the sum of the components. Components are
/// stored in increasing order of magnitude, are non-overlapping and (after zero elimination)
/// non-zero; the value zero is the empty expansion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Expansion(Vec<f64>);

impl Expansion {
    /// The expansion with value zero.
    pub fn zero() -> Self {
        Expansion(Vec::new())
    }

    /// The expansion of a single double.
    pub fn from_f64(x: f64) -> Self {
        if x == 0.0 {
            Self::zero()
        } else {
            Expansion(vec![x])
        }
    }

    /// The exact difference `a - b` of two doubles.
    pub fn diff(a: f64, b: f64) -> Self {
        let (x, y) = two_diff(a, b);
        let mut e = Vec::with_capacity(2);
        if y != 0.0 {
            e.push(y);
        }
        if x != 0.0 {
            e.push(x);
        }
        Expansion(e)
    }

    /// The components, smallest magnitude first.
    pub fn components(&self) -> &[f64] {
        &self.0
    }

    /// Sign of the exact value: -1, 0 or 1 (0 also for NaN components).
    pub fn sign(&self) -> i32 {
        match self.0.last() {
            Some(&c) if c > 0.0 => 1,
            Some(&c) if c < 0.0 => -1,
            _ => 0,
        }
    }

    /// The most significant component: a double with the sign of the exact value and within a
    /// factor of two of its magnitude (the lower components sum to less than its last bit).
    pub fn leading(&self) -> f64 {
        self.0.last().copied().unwrap_or(0.0)
    }

    /// Approximate value: the floating-point sum of the components.
    pub fn approx(&self) -> f64 {
        self.0.iter().sum()
    }

    /// Exact sum of two expansions (Shewchuk's `fast_expansion_sum_zeroelim`): the components
    /// are merged by magnitude and accumulated with a chain of `two_sum`s.
    pub fn add(&self, other: &Expansion) -> Expansion {
        let e = &self.0;
        let f = &other.0;
        if e.is_empty() {
            return other.clone();
        }
        if f.is_empty() {
            return self.clone();
        }
        let mut h = Vec::with_capacity(e.len() + f.len());
        let (mut ei, mut fi) = (0usize, 0usize);
        // Take the smaller-magnitude head of the two inputs.
        let take = |ei: &mut usize, fi: &mut usize| -> Option<f64> {
            match (e.get(*ei), f.get(*fi)) {
                (Some(&x), Some(&y)) => {
                    if y.abs() > x.abs() {
                        *ei += 1;
                        Some(x)
                    } else {
                        *fi += 1;
                        Some(y)
                    }
                }
                (Some(&x), None) => {
                    *ei += 1;
                    Some(x)
                }
                (None, Some(&y)) => {
                    *fi += 1;
                    Some(y)
                }
                (None, None) => None,
            }
        };
        let mut q = take(&mut ei, &mut fi).unwrap();
        if let Some(next) = take(&mut ei, &mut fi) {
            let (qn, hh) = fast_two_sum(next, q);
            q = qn;
            if hh != 0.0 {
                h.push(hh);
            }
            while let Some(next) = take(&mut ei, &mut fi) {
                let (qn, hh) = two_sum(q, next);
                q = qn;
                if hh != 0.0 {
                    h.push(hh);
                }
            }
        }
        if q != 0.0 {
            h.push(q);
        }
        Expansion(h)
    }

    /// The negation.
    pub fn neg(&self) -> Expansion {
        Expansion(self.0.iter().map(|c| -c).collect())
    }

    /// Exact difference.
    pub fn sub(&self, other: &Expansion) -> Expansion {
        self.add(&other.neg())
    }

    /// Exact product with a double (Shewchuk's `scale_expansion_zeroelim`).
    pub fn scale(&self, b: f64) -> Expansion {
        let e = &self.0;
        if e.is_empty() || b == 0.0 {
            return Expansion::zero();
        }
        let mut h = Vec::with_capacity(2 * e.len());
        let (mut q, hh) = two_product(e[0], b);
        if hh != 0.0 {
            h.push(hh);
        }
        for &enow in &e[1..] {
            let (product1, product0) = two_product(enow, b);
            let (sum, hh) = two_sum(q, product0);
            if hh != 0.0 {
                h.push(hh);
            }
            let (qn, hh) = fast_two_sum(product1, sum);
            q = qn;
            if hh != 0.0 {
                h.push(hh);
            }
        }
        if q != 0.0 {
            h.push(q);
        }
        Expansion(h)
    }

    /// Exact product of two expansions (distributes the scalar products and sums them).
    pub fn mul(&self, other: &Expansion) -> Expansion {
        let mut acc = Expansion::zero();
        for &c in &self.0 {
            acc = acc.add(&other.scale(c));
        }
        acc
    }

    /// The same value with the fewest components (Shewchuk's `compress`): the components are
    /// re-accumulated top-down and bottom-up so that each one carries as many bits as it can.
    /// Keeps the exact arithmetic of long product chains from growing linearly in length.
    pub(crate) fn compress(&self) -> Expansion {
        let e = &self.0;
        if e.len() < 2 {
            return self.clone();
        }
        let mut h = vec![0.0; e.len()];
        let mut bottom = e.len() - 1;
        let mut q = e[bottom];
        for &enow in e[..e.len() - 1].iter().rev() {
            let (qn, qq) = fast_two_sum(q, enow);
            if qq != 0.0 {
                h[bottom] = qn;
                bottom -= 1;
                q = qq;
            } else {
                q = qn;
            }
        }
        let mut top = 0;
        for i in bottom + 1..e.len() {
            let hnow = h[i];
            let (qn, qq) = fast_two_sum(hnow, q);
            if qq != 0.0 {
                h[top] = qq;
                top += 1;
            }
            q = qn;
        }
        if q != 0.0 {
            h[top] = q;
            top += 1;
        }
        h.truncate(top);
        Expansion(h)
    }
}

// ---------------------------------------------------------------------------------------------
// orient2d

/// Orientation of the triangle `a b c`: positive when the points are in counter-clockwise
/// order, negative when clockwise, zero when collinear. The value approximates twice the signed
/// area of the triangle; its sign is exact (see the module documentation).
pub fn orient2d(a: Point2, b: Point2, c: Point2) -> f64 {
    let detleft = (a.x - c.x) * (b.y - c.y);
    let detright = (a.y - c.y) * (b.x - c.x);
    let det = detleft - detright;

    let detsum;
    if detleft > 0.0 {
        if detright <= 0.0 {
            return det;
        }
        detsum = detleft + detright;
    } else if detleft < 0.0 {
        if detright >= 0.0 {
            return det;
        }
        detsum = -detleft - detright;
    } else {
        return det;
    }
    let errbound = CCW_ERR_BOUND_A * detsum;
    if det >= errbound || -det >= errbound {
        return det;
    }
    orient2d_exact(a, b, c)
}

/// `orient2d` evaluated entirely with exact expansion arithmetic.
pub fn orient2d_exact(a: Point2, b: Point2, c: Point2) -> f64 {
    let acx = Expansion::diff(a.x, c.x);
    let acy = Expansion::diff(a.y, c.y);
    let bcx = Expansion::diff(b.x, c.x);
    let bcy = Expansion::diff(b.y, c.y);
    let det = acx.mul(&bcy).sub(&acy.mul(&bcx));
    det.leading()
}

// ---------------------------------------------------------------------------------------------
// orient3d

/// Orientation of the tetrahedron `a b c d`: the determinant of the rows `a-d`, `b-d`, `c-d`.
/// Positive when `d` lies below the plane of `a b c` (seen so that `a b c` is counter-clockwise),
/// negative when above, zero when the four points are coplanar. The value approximates six times
/// the signed volume; its sign is exact (see the module documentation).
pub fn orient3d(a: Point3, b: Point3, c: Point3, d: Point3) -> f64 {
    let adx = a.x - d.x;
    let bdx = b.x - d.x;
    let cdx = c.x - d.x;
    let ady = a.y - d.y;
    let bdy = b.y - d.y;
    let cdy = c.y - d.y;
    let adz = a.z - d.z;
    let bdz = b.z - d.z;
    let cdz = c.z - d.z;

    let bdxcdy = bdx * cdy;
    let cdxbdy = cdx * bdy;
    let cdxady = cdx * ady;
    let adxcdy = adx * cdy;
    let adxbdy = adx * bdy;
    let bdxady = bdx * ady;

    let det = adz * (bdxcdy - cdxbdy) + bdz * (cdxady - adxcdy) + cdz * (adxbdy - bdxady);

    let permanent = (bdxcdy.abs() + cdxbdy.abs()) * adz.abs()
        + (cdxady.abs() + adxcdy.abs()) * bdz.abs()
        + (adxbdy.abs() + bdxady.abs()) * cdz.abs();
    let errbound = O3D_ERR_BOUND_A * permanent;
    if det > errbound || -det > errbound {
        return det;
    }
    orient3d_exact(a, b, c, d)
}

/// `orient3d` evaluated entirely with exact expansion arithmetic.
pub fn orient3d_exact(a: Point3, b: Point3, c: Point3, d: Point3) -> f64 {
    let adx = Expansion::diff(a.x, d.x);
    let bdx = Expansion::diff(b.x, d.x);
    let cdx = Expansion::diff(c.x, d.x);
    let ady = Expansion::diff(a.y, d.y);
    let bdy = Expansion::diff(b.y, d.y);
    let cdy = Expansion::diff(c.y, d.y);
    let adz = Expansion::diff(a.z, d.z);
    let bdz = Expansion::diff(b.z, d.z);
    let cdz = Expansion::diff(c.z, d.z);

    let bc = bdx.mul(&cdy).sub(&cdx.mul(&bdy));
    let ca = cdx.mul(&ady).sub(&adx.mul(&cdy));
    let ab = adx.mul(&bdy).sub(&bdx.mul(&ady));

    let det = adz.mul(&bc).add(&bdz.mul(&ca)).add(&cdz.mul(&ab));
    det.leading()
}

// ---------------------------------------------------------------------------------------------
// incircle

/// Incircle test: positive when `d` lies inside the circle through `a`, `b`, `c` (given in
/// counter-clockwise order), negative when outside, zero when the four points are cocircular.
/// For clockwise `a b c` the sign is reversed. The sign is exact (see the module documentation).
pub fn incircle(a: Point2, b: Point2, c: Point2, d: Point2) -> f64 {
    let adx = a.x - d.x;
    let bdx = b.x - d.x;
    let cdx = c.x - d.x;
    let ady = a.y - d.y;
    let bdy = b.y - d.y;
    let cdy = c.y - d.y;

    let bdxcdy = bdx * cdy;
    let cdxbdy = cdx * bdy;
    let alift = adx * adx + ady * ady;

    let cdxady = cdx * ady;
    let adxcdy = adx * cdy;
    let blift = bdx * bdx + bdy * bdy;

    let adxbdy = adx * bdy;
    let bdxady = bdx * ady;
    let clift = cdx * cdx + cdy * cdy;

    let det = alift * (bdxcdy - cdxbdy) + blift * (cdxady - adxcdy) + clift * (adxbdy - bdxady);

    let permanent = (bdxcdy.abs() + cdxbdy.abs()) * alift
        + (cdxady.abs() + adxcdy.abs()) * blift
        + (adxbdy.abs() + bdxady.abs()) * clift;
    let errbound = ICC_ERR_BOUND_A * permanent;
    if det > errbound || -det > errbound {
        return det;
    }
    incircle_exact(a, b, c, d)
}

/// `incircle` evaluated entirely with exact expansion arithmetic.
pub fn incircle_exact(a: Point2, b: Point2, c: Point2, d: Point2) -> f64 {
    let adx = Expansion::diff(a.x, d.x);
    let bdx = Expansion::diff(b.x, d.x);
    let cdx = Expansion::diff(c.x, d.x);
    let ady = Expansion::diff(a.y, d.y);
    let bdy = Expansion::diff(b.y, d.y);
    let cdy = Expansion::diff(c.y, d.y);

    let alift = adx.mul(&adx).add(&ady.mul(&ady));
    let blift = bdx.mul(&bdx).add(&bdy.mul(&bdy));
    let clift = cdx.mul(&cdx).add(&cdy.mul(&cdy));

    let bc = bdx.mul(&cdy).sub(&cdx.mul(&bdy));
    let ca = cdx.mul(&ady).sub(&adx.mul(&cdy));
    let ab = adx.mul(&bdy).sub(&bdx.mul(&ady));

    let det = alift.mul(&bc).add(&blift.mul(&ca)).add(&clift.mul(&ab));
    det.leading()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgn(x: f64) -> i32 {
        if x > 0.0 {
            1
        } else if x < 0.0 {
            -1
        } else {
            0
        }
    }

    /// Minimal deterministic PRNG (xorshift64*) so the tests need no dependencies.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn int(&mut self, lo: i64, hi: i64) -> i64 {
            lo + (self.next() % ((hi - lo + 1) as u64)) as i64
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    fn ldexp(x: f64, e: i32) -> f64 {
        x * 2f64.powi(e)
    }

    fn o2_exact_int(p: [[i128; 2]; 3]) -> i32 {
        let [a, b, c] = p;
        let d = (a[0] - c[0]) * (b[1] - c[1]) - (a[1] - c[1]) * (b[0] - c[0]);
        d.signum() as i32
    }

    fn o3_exact_int(p: [[i128; 3]; 4]) -> i32 {
        let m: Vec<[i128; 3]> = (0..3).map(|r| [p[r][0] - p[3][0], p[r][1] - p[3][1], p[r][2] - p[3][2]]).collect();
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        det.signum() as i32
    }

    fn ic_exact_int(p: [[i128; 2]; 4]) -> i32 {
        let rows: Vec<[i128; 3]> = (0..3)
            .map(|r| {
                let x = p[r][0] - p[3][0];
                let y = p[r][1] - p[3][1];
                [x, y, x * x + y * y]
            })
            .collect();
        let (a, b, c) = (rows[0], rows[1], rows[2]);
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]);
        det.signum() as i32
    }

    #[test]
    fn expansion_arithmetic_is_exact() {
        // (1 + 2^-60) - 1 is not representable as one double but is as an expansion.
        let big = Expansion::from_f64(1.0).add(&Expansion::from_f64(2f64.powi(-60)));
        assert_eq!(big.components(), &[2f64.powi(-60), 1.0]);
        let diff = big.sub(&Expansion::from_f64(1.0));
        assert_eq!(diff.components(), &[2f64.powi(-60)]);
        // Cancellation to exactly zero gives the empty expansion.
        assert_eq!(big.sub(&big).components(), &[] as &[f64]);
        // Products of 53-bit mantissas need two components.
        let x = 1.0 + 2f64.powi(-52);
        let p = Expansion::from_f64(x).mul(&Expansion::from_f64(x));
        assert_eq!(p.approx(), x * x);
        assert_eq!(p.components().len(), 2);
        assert_eq!(p.sub(&Expansion::from_f64(x * x)).sign(), 1); // x*x rounded down
    }

    #[test]
    fn orient2d_shewchuk_grid() {
        // 64x64 grid of points near (0.5, 0.5) against the line through (12,12) and (24,24).
        let u = 2f64.powi(-53);
        let s = 1i128 << 53;
        for i in 0..64 {
            for j in 0..64 {
                let a = Point2::new(0.5 + i as f64 * u, 0.5 + j as f64 * u);
                let want = o2_exact_int([[(1i128 << 52) + i, (1i128 << 52) + j], [12 * s, 12 * s], [24 * s, 24 * s]]);
                let got = orient2d(a, Point2::new(12.0, 12.0), Point2::new(24.0, 24.0));
                assert_eq!(sgn(got), want, "grid {i} {j}");
                assert_eq!(sgn(orient2d_exact(a, Point2::new(12.0, 12.0), Point2::new(24.0, 24.0))), want);
            }
        }
    }

    #[test]
    fn orient2d_scaled_integers_match_exact_oracle() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..3000 {
            let e = rng.int(-90, 90) as i32;
            let p = rng.int(-(1 << 20), 1 << 20) as i128;
            let q = rng.int(-(1 << 20), 1 << 20) as i128;
            let d1 = rng.int(-(1 << 10), 1 << 10) as i128;
            let d2 = rng.int(-(1 << 10), 1 << 10) as i128;
            let k = rng.int(-50, 50) as i128;
            let nudge = rng.int(-1, 1) as i128;
            let pts = [[p, q], [p + d1, q + d2], [p + k * d1 + nudge, q + k * d2]];
            let want = o2_exact_int(pts);
            let f = |v: [i128; 2]| Point2::new(ldexp(v[0] as f64, e), ldexp(v[1] as f64, e));
            let got = orient2d(f(pts[0]), f(pts[1]), f(pts[2]));
            assert_eq!(sgn(got), want, "{pts:?} e={e}");
            assert_eq!(sgn(orient2d_exact(f(pts[0]), f(pts[1]), f(pts[2]))), want);
        }
    }

    #[test]
    fn orient3d_scaled_integers_match_exact_oracle() {
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        for _ in 0..3000 {
            let e = rng.int(-90, 90) as i32;
            let mut r = || rng.int(-(1 << 15), 1 << 15) as i128;
            let a = [r(), r(), r()];
            let b = [r(), r(), r()];
            let c = [r(), r(), r()];
            let (s, t) = (rng.int(-20, 20) as i128, rng.int(-20, 20) as i128);
            let nudge = rng.int(-1, 1) as i128;
            let d = [
                a[0] + s * (b[0] - a[0]) + t * (c[0] - a[0]) + nudge,
                a[1] + s * (b[1] - a[1]) + t * (c[1] - a[1]),
                a[2] + s * (b[2] - a[2]) + t * (c[2] - a[2]),
            ];
            let pts = [a, b, c, d];
            let want = o3_exact_int(pts);
            let f = |v: [i128; 3]| Point3::new(ldexp(v[0] as f64, e), ldexp(v[1] as f64, e), ldexp(v[2] as f64, e));
            let got = orient3d(f(a), f(b), f(c), f(d));
            assert_eq!(sgn(got), want, "{pts:?} e={e}");
            assert_eq!(sgn(orient3d_exact(f(a), f(b), f(c), f(d))), want);
        }
        // d below CCW abc is positive.
        let o = orient3d(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0), Point3::new(0.0, 0.0, -1.0));
        assert!(o > 0.0);
    }

    #[test]
    fn incircle_cocircular_integers_match_exact_oracle() {
        let circle: Vec<[i128; 2]> = (-65..=65)
            .flat_map(|x| (-65..=65).map(move |y| [x, y]))
            .filter(|[x, y]| x * x + y * y == 65 * 65)
            .collect();
        let mut rng = Rng(0xA5A5_5A5A_1234_5678);
        for _ in 0..3000 {
            let e = rng.int(-60, 60) as i32;
            let ox = rng.int(-(1 << 12), 1 << 12) as i128;
            let oy = rng.int(-(1 << 12), 1 << 12) as i128;
            let mut pts = [[0i128; 2]; 4];
            for p in pts.iter_mut() {
                let c = circle[rng.int(0, circle.len() as i64 - 1) as usize];
                *p = [c[0] + ox, c[1] + oy];
            }
            pts[3][0] += rng.int(-1, 1) as i128;
            let want = ic_exact_int(pts);
            let f = |v: [i128; 2]| Point2::new(ldexp(v[0] as f64, e), ldexp(v[1] as f64, e));
            let got = incircle(f(pts[0]), f(pts[1]), f(pts[2]), f(pts[3]));
            assert_eq!(sgn(got), want, "{pts:?} e={e}");
            assert_eq!(sgn(incircle_exact(f(pts[0]), f(pts[1]), f(pts[2]), f(pts[3]))), want);
        }
        // Centre inside a CCW circle is positive.
        let ic = incircle(Point2::new(5.0, 0.0), Point2::new(0.0, 5.0), Point2::new(-5.0, 0.0), Point2::new(0.0, 0.0));
        assert!(ic > 0.0);
    }

    #[test]
    fn filter_and_exact_agree_on_random_doubles() {
        let mut rng = Rng(0x0123_4567_89AB_CDEF);
        for _ in 0..2000 {
            let e = rng.int(-90, 90) as i32;
            let mut p2 = || Point2::new(ldexp(rng.unit(), e), ldexp(rng.unit(), e));
            let (a, b, c, d) = (p2(), p2(), p2(), p2());
            assert_eq!(sgn(orient2d(a, b, c)), sgn(orient2d_exact(a, b, c)));
            assert_eq!(sgn(orient2d(a, b, c)), -sgn(orient2d(b, a, c)));
            assert_eq!(sgn(orient2d(a, b, c)), sgn(orient2d(b, c, a)));
            assert_eq!(sgn(incircle(a, b, c, d)), sgn(incircle_exact(a, b, c, d)));
            assert_eq!(sgn(incircle(a, b, c, d)), -sgn(incircle(b, a, c, d)));
            let mut p3 = || Point3::new(ldexp(rng.unit(), e), ldexp(rng.unit(), e), ldexp(rng.unit(), e));
            let (a, b, c, d) = (p3(), p3(), p3(), p3());
            assert_eq!(sgn(orient3d(a, b, c, d)), sgn(orient3d_exact(a, b, c, d)));
            assert_eq!(sgn(orient3d(a, b, c, d)), -sgn(orient3d(b, a, c, d)));
        }
    }

    #[test]
    fn degenerate_inputs() {
        assert_eq!(orient2d(Point2::new(1.0, 1.0), Point2::new(1.0, 1.0), Point2::new(3.0, 7.0)), 0.0);
        assert_eq!(orient2d(Point2::new(0.0, 0.0), Point2::new(0.0, 0.0), Point2::new(0.0, 0.0)), 0.0);
        assert!(orient2d(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0), Point2::new(0.0, 1.0)) > 0.0);
        let big = 2f64.powi(100);
        let tiny = 2f64.powi(-100);
        // Mixed extreme scales: (big, big), (big+ulp, big+ulp) and (tiny, tiny) are collinear.
        let a = Point2::new(big, big);
        let b = Point2::new(big + 2f64.powi(48), big + 2f64.powi(48));
        let c = Point2::new(tiny, tiny);
        assert_eq!(orient2d(a, b, c), 0.0);
        assert!(orient2d(a, b, Point2::new(tiny, 0.0)) != 0.0);
        assert!(orient2d(Point2::new(f64::NAN, 0.0), b, c).is_nan());
    }
}
