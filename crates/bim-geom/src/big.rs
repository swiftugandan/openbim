//! Exact dyadic numbers `m · 2^e` with an arbitrary-precision integer mantissa.
//!
//! Every double is dyadic, and sums and products of dyadic numbers are dyadic, so the
//! coordinates and plane coefficients the boolean kernel constructs from its inputs are
//! represented exactly, whatever the depth of the construction. Unlike floating-point
//! expansions, which are exact only while no component underflows, these numbers have an
//! unbounded exponent range: a product of two deeply constructed values (a few thousand
//! bits wide) stays exact, and in particular a value that is exactly zero is found to be
//! zero. Only the operations the kernel needs are provided: addition, multiplication, sign,
//! and the correctly rounded double.

use std::cmp::Ordering;

/// `(-1)^neg · mag · 2^exp`, with `mag` in little-endian 64-bit limbs without leading zero
/// limbs and with its lowest bit set (zero is the empty mantissa).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dyadic {
    neg: bool,
    mag: Vec<u64>,
    exp: i64,
}

impl Dyadic {
    pub fn zero() -> Dyadic {
        Dyadic { neg: false, mag: Vec::new(), exp: 0 }
    }

    /// The exact value of a finite double (a non-finite one is treated as zero).
    pub fn from_f64(x: f64) -> Dyadic {
        if !x.is_finite() || x == 0.0 {
            return Dyadic::zero();
        }
        let bits = x.to_bits();
        let neg = bits >> 63 != 0;
        let e = ((bits >> 52) & 0x7ff) as i64;
        let frac = bits & ((1u64 << 52) - 1);
        let (m, exp) = if e == 0 { (frac, -1074) } else { (frac | (1u64 << 52), e - 1075) };
        Dyadic::normalized(neg, vec![m], exp)
    }

    fn normalized(neg: bool, mut mag: Vec<u64>, mut exp: i64) -> Dyadic {
        while mag.last() == Some(&0) {
            mag.pop();
        }
        if mag.is_empty() {
            return Dyadic::zero();
        }
        // Drop whole zero limbs, then zero bits, at the low end.
        let zero_limbs = mag.iter().position(|&l| l != 0).unwrap_or(0);
        if zero_limbs > 0 {
            mag.drain(..zero_limbs);
            exp += 64 * zero_limbs as i64;
        }
        let tz = mag[0].trailing_zeros();
        if tz > 0 {
            shr_in_place(&mut mag, tz);
            exp += tz as i64;
            while mag.last() == Some(&0) {
                mag.pop();
            }
        }
        Dyadic { neg, mag, exp }
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    pub fn sign(&self) -> i32 {
        if self.mag.is_empty() {
            0
        } else if self.neg {
            -1
        } else {
            1
        }
    }

    pub fn neg(&self) -> Dyadic {
        Dyadic { neg: !self.neg && !self.mag.is_empty(), mag: self.mag.clone(), exp: self.exp }
    }

    pub fn add(&self, o: &Dyadic) -> Dyadic {
        if self.mag.is_empty() {
            return o.clone();
        }
        if o.mag.is_empty() {
            return self.clone();
        }
        let exp = self.exp.min(o.exp);
        let a = shl(&self.mag, (self.exp - exp) as u64);
        let b = shl(&o.mag, (o.exp - exp) as u64);
        if self.neg == o.neg {
            return Dyadic::normalized(self.neg, add_mag(&a, &b), exp);
        }
        match cmp_mag(&a, &b) {
            Ordering::Equal => Dyadic::zero(),
            Ordering::Greater => Dyadic::normalized(self.neg, sub_mag(&a, &b), exp),
            Ordering::Less => Dyadic::normalized(o.neg, sub_mag(&b, &a), exp),
        }
    }

    pub fn sub(&self, o: &Dyadic) -> Dyadic {
        self.add(&o.neg())
    }

    pub fn mul(&self, o: &Dyadic) -> Dyadic {
        if self.mag.is_empty() || o.mag.is_empty() {
            return Dyadic::zero();
        }
        Dyadic::normalized(self.neg != o.neg, mul_mag(&self.mag, &o.mag), self.exp + o.exp)
    }

    pub fn scale(&self, b: f64) -> Dyadic {
        self.mul(&Dyadic::from_f64(b))
    }

    /// The double nearest to the value (ties to even), `±inf` beyond the double range and a
    /// correctly rounded subnormal (or zero) below it.
    pub fn approx(&self) -> f64 {
        if self.mag.is_empty() {
            return 0.0;
        }
        let len = bit_len(&self.mag);
        // The top 64 bits as an integer, and whether anything below them is set.
        let (top, sticky) = if len <= 64 {
            (shl(&self.mag, 64 - len)[0], false)
        } else {
            let shift = len - 64;
            let hi = shr(&self.mag, shift);
            let sticky = low_bits_set(&self.mag, shift);
            (hi[0], sticky)
        };
        // Value = top · 2^(exp + len - 64) with top's bit 63 set; round to 53 bits.
        let mut e = self.exp + len as i64 - 64;
        let mut m = top >> 11;
        let rest = top & 0x7ff;
        let half = 1u64 << 10;
        if rest > half || (rest == half && (sticky || m & 1 == 1)) {
            m += 1;
            if m == 1u64 << 53 {
                m >>= 1;
                e += 1;
            }
        }
        e += 11;
        let sign = if self.neg { -1.0 } else { 1.0 };
        sign * ldexp(m as f64, e)
    }
}

/// `x · 2^e`, with the scaling split so that no intermediate step overflows or underflows
/// before the last one.
fn ldexp(mut x: f64, mut e: i64) -> f64 {
    while e > 1000 {
        x *= 2f64.powi(1000);
        e -= 1000;
        if !x.is_finite() {
            return x;
        }
    }
    while e < -1000 {
        x *= 2f64.powi(-1000);
        e += 1000;
        if x == 0.0 {
            return x;
        }
    }
    x * 2f64.powi(e as i32)
}

fn bit_len(mag: &[u64]) -> u64 {
    match mag.last() {
        None => 0,
        Some(&top) => 64 * (mag.len() as u64 - 1) + (64 - top.leading_zeros() as u64),
    }
}

fn shl(mag: &[u64], bits: u64) -> Vec<u64> {
    if mag.is_empty() {
        return Vec::new();
    }
    let limbs = (bits / 64) as usize;
    let b = (bits % 64) as u32;
    let mut out = vec![0u64; limbs];
    if b == 0 {
        out.extend_from_slice(mag);
    } else {
        let mut carry = 0u64;
        for &l in mag {
            out.push((l << b) | carry);
            carry = l >> (64 - b);
        }
        if carry != 0 {
            out.push(carry);
        }
    }
    out
}

/// `mag >> bits` (the dropped bits are discarded).
fn shr(mag: &[u64], bits: u64) -> Vec<u64> {
    let limbs = (bits / 64) as usize;
    if limbs >= mag.len() {
        return Vec::new();
    }
    let mut out = mag[limbs..].to_vec();
    let b = (bits % 64) as u32;
    if b > 0 {
        shr_in_place(&mut out, b);
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

fn shr_in_place(mag: &mut [u64], b: u32) {
    if b == 0 {
        return;
    }
    for i in 0..mag.len() {
        let hi = if i + 1 < mag.len() { mag[i + 1] << (64 - b) } else { 0 };
        mag[i] = (mag[i] >> b) | hi;
    }
}

/// Whether any of the lowest `bits` bits is set.
fn low_bits_set(mag: &[u64], bits: u64) -> bool {
    let limbs = (bits / 64) as usize;
    if mag[..limbs.min(mag.len())].iter().any(|&l| l != 0) {
        return true;
    }
    let b = bits % 64;
    b > 0 && limbs < mag.len() && mag[limbs] & ((1u64 << b) - 1) != 0
}

fn cmp_mag(a: &[u64], b: &[u64]) -> Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    Ordering::Equal
}

fn add_mag(a: &[u64], b: &[u64]) -> Vec<u64> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::with_capacity(long.len() + 1);
    let mut carry = 0u64;
    for i in 0..long.len() {
        let (s1, c1) = long[i].overflowing_add(if i < short.len() { short[i] } else { 0 });
        let (s2, c2) = s1.overflowing_add(carry);
        out.push(s2);
        carry = (c1 as u64) + (c2 as u64);
    }
    if carry != 0 {
        out.push(carry);
    }
    out
}

/// `a - b` for `a >= b`.
fn sub_mag(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0u64;
    for i in 0..a.len() {
        let (d1, b1) = a[i].overflowing_sub(if i < b.len() { b[i] } else { 0 });
        let (d2, b2) = d1.overflowing_sub(borrow);
        out.push(d2);
        borrow = (b1 as u64) + (b2 as u64);
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

fn mul_mag(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = vec![0u64; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0u128;
        for (j, &y) in b.iter().enumerate() {
            let t = x as u128 * y as u128 + out[i + j] as u128 + carry;
            out[i + j] = t as u64;
            carry = t >> 64;
        }
        let mut k = i + b.len();
        while carry != 0 {
            let t = out[k] as u128 + carry;
            out[k] = t as u64;
            carry = t >> 64;
            k += 1;
        }
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(x: f64) -> Dyadic {
        Dyadic::from_f64(x)
    }

    #[test]
    fn doubles_round_trip_and_arithmetic_is_exact() {
        for &x in &[0.0, 1.0, -1.0, 0.1, 2.65, 1e300, -1e-300, 5e-324, f64::MAX, 123456789.0 / 1024.0] {
            assert_eq!(d(x).approx(), x, "{x}");
        }
        // 0.1 + 0.2 != 0.3 in doubles, but the exact sum differs from 0.3 by a known amount.
        let s = d(0.1).add(&d(0.2));
        assert_eq!(s.sub(&d(0.3)).sign(), 1);
        assert_eq!(s.approx(), 0.30000000000000004);
        // Products that underflow as doubles stay exact.
        let tiny = d(2f64.powi(-700)).mul(&d(2f64.powi(-700)));
        assert_eq!(tiny.sign(), 1);
        assert_eq!(tiny.approx(), 0.0);
        assert_eq!(tiny.mul(&d(2f64.powi(1000))).mul(&d(2f64.powi(1000))).approx(), 2f64.powi(600));
        // Exact cancellation.
        let a = d(2.65).mul(&d(1e-17)).add(&d(3.0));
        let b = d(3.0).add(&d(1e-17).mul(&d(2.65)));
        assert!(a.sub(&b).is_zero());
        assert_eq!(a.sub(&b).approx(), 0.0);
        // Sign and negation.
        assert_eq!(d(-2.0).mul(&d(3.0)).sign(), -1);
        assert_eq!(d(-2.0).neg().approx(), 2.0);
        assert_eq!(d(0.0).neg().sign(), 0);
    }

    #[test]
    fn normalisation_strips_limbs_emptied_by_the_shift() {
        // 2^63 + 2^64: the low limb's trailing zeros shift the top limb to zero.
        let v = Dyadic::normalized(false, vec![1u64 << 63, 1], 0);
        assert_eq!(v.mag.len(), 1);
        assert_eq!(v.approx(), 3.0 * 2f64.powi(63));
        assert_eq!(v.sub(&d(2f64.powi(63))).sub(&d(2f64.powi(64))).sign(), 0);
        assert_eq!(v.add(&d(1.0)).sub(&v).approx(), 1.0);
    }

    #[test]
    fn rounding_to_double_is_nearest_even() {
        // 2^53 + 1 is a tie: rounds to 2^53 (even); 2^53 + 3 rounds to 2^53 + 4.
        let two53 = d(9007199254740992.0);
        assert_eq!(two53.add(&d(1.0)).approx(), 9007199254740992.0);
        assert_eq!(two53.add(&d(3.0)).approx(), 9007199254740996.0);
        // A value slightly above the tie rounds up: 2^53 + 1 + 2^-10.
        assert_eq!(two53.add(&d(1.0)).add(&d(2f64.powi(-10))).approx(), 9007199254740994.0);
        // Wide mantissas: (2^60 + 1) · (2^60 + 1) = 2^120 + 2^61 + 1.
        let w = d(2f64.powi(60)).add(&d(1.0));
        let p = w.mul(&w);
        assert_eq!(p.sub(&d(2f64.powi(120))).sub(&d(2f64.powi(61))).sub(&d(1.0)).sign(), 0);
        assert_eq!(p.approx(), 2f64.powi(120) + 2f64.powi(61));
        // Subnormal and overflow limits.
        assert_eq!(d(5e-324).mul(&d(0.5)).approx(), 0.0);
        assert_eq!(d(1e308).mul(&d(10.0)).approx(), f64::INFINITY);
    }
}
