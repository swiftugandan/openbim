//! Span loads on an Euler–Bernoulli member: a piecewise-linear intensity profile `w(s)`,
//! `s ∈ [0, L]` from the member's start, integrated exactly (three-point Gauss–Legendre on every
//! linear piece is exact for the cubic weights used here).
//!
//! * Equivalent (consistent) nodal loads, which for a prismatic Euler–Bernoulli member are the
//!   negated fixed-end reactions: axial weights `1 − ξ, ξ`, transverse Hermite weights
//!   `H1 = 1 − 3ξ² + 2ξ³`, `H2 = L(ξ − 2ξ² + ξ³)`, `H3 = 3ξ² − 2ξ³`, `H4 = L(ξ³ − ξ²)`.
//! * The resultants `∫₀ˣ w` and `∫₀ˣ (x − s) w` that enter the internal actions at a station.
//! * The fixed–fixed deflection under the load (Green's function of the clamped beam), which is
//!   added to the end-displacement (Hermite) field for the deflection envelope.

/// A piecewise-linear profile: `(s, w)` samples with ascending `s`, zero outside.
pub(crate) type Profile = [(f64, f64)];

const G3: [(f64, f64); 3] = [(-0.774_596_669_241_483_4, 5.0 / 9.0), (0.0, 8.0 / 9.0), (0.774_596_669_241_483_4, 5.0 / 9.0)];

/// `∫_{x0}^{x1} g(s) w(s) ds`, exact when `g` is a polynomial of degree ≤ 4 on every piece.
pub(crate) fn weighted(p: &Profile, x0: f64, x1: f64, g: impl Fn(f64) -> f64) -> f64 {
    let mut sum = 0.0;
    for w in p.windows(2) {
        let ((sa, wa), (sb, wb)) = (w[0], w[1]);
        let (a, b) = (sa.max(x0), sb.min(x1));
        if b <= a || sb <= sa {
            continue;
        }
        let (c, h) = (0.5 * (a + b), 0.5 * (b - a));
        for (xi, wt) in G3 {
            let s = c + h * xi;
            let ws = wa + (wb - wa) * (s - sa) / (sb - sa);
            sum += wt * h * g(s) * ws;
        }
    }
    sum
}

/// Consistent nodal weights `[∫(1−ξ)w, ∫ξw, ∫H1 w, ∫H2 w, ∫H3 w, ∫H4 w]` on a member of length `l`.
pub(crate) fn nodal_weights(p: &Profile, l: f64) -> [f64; 6] {
    let f = |k: usize| {
        weighted(p, 0.0, l, |s| {
            let x = s / l;
            match k {
                0 => 1.0 - x,
                1 => x,
                2 => 1.0 - 3.0 * x * x + 2.0 * x * x * x,
                3 => l * (x - 2.0 * x * x + x * x * x),
                4 => 3.0 * x * x - 2.0 * x * x * x,
                _ => l * (x * x * x - x * x),
            }
        })
    };
    [f(0), f(1), f(2), f(3), f(4), f(5)]
}

/// `(∫₀ˣ w, ∫₀ˣ (x − s) w)`.
pub(crate) fn resultants(p: &Profile, x: f64) -> (f64, f64) {
    (weighted(p, 0.0, x, |_| 1.0), weighted(p, 0.0, x, |s| x - s))
}

/// Deflection at `x` of a clamped–clamped beam of length `l` and bending stiffness 1 under the
/// profile (in the load's direction); divide by `E·I`.
pub(crate) fn clamped_deflection(p: &Profile, l: f64, x: f64) -> f64 {
    let l3 = 6.0 * l * l * l;
    // Unit load at a, b = l − a. For x ≤ a: b² x² (3 a l − (3a + b) x) / (6 l³); mirrored for x > a.
    let right = weighted(p, x, l, |a| {
        let b = l - a;
        b * b * x * x * (3.0 * a * l - (3.0 * a + b) * x) / l3
    });
    let left = weighted(p, 0.0, x, |a| {
        let (b, xm) = (l - a, l - x);
        a * a * xm * xm * (3.0 * b * l - (3.0 * b + a) * xm) / l3
    });
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_load_closed_forms() {
        let (l, w) = (4.0, 3.0);
        let p = [(0.0, w), (1.3, w), (l, w)];
        let n = nodal_weights(&p, l);
        let want = [w * l / 2.0, w * l / 2.0, w * l / 2.0, w * l * l / 12.0, w * l / 2.0, -w * l * l / 12.0];
        for (g, e) in n.iter().zip(want) {
            assert!((g - e).abs() < 1e-12, "{g} vs {e}");
        }
        let (r0, r1) = resultants(&p, 2.5);
        assert!((r0 - w * 2.5).abs() < 1e-12 && (r1 - w * 2.5 * 2.5 / 2.0).abs() < 1e-12);
        // Mid-span deflection of a clamped beam: w l⁴ / 384.
        assert!((clamped_deflection(&p, l, l / 2.0) - w * l.powi(4) / 384.0).abs() < 1e-12);
        assert!(clamped_deflection(&p, l, 0.0).abs() < 1e-14);
    }

    #[test]
    fn triangular_load_fixed_end_moments() {
        // Load rising linearly from 0 to w: fixed-end moments w l²/30 and w l²/20.
        let (l, w) = (5.0, 2.0);
        let p = [(0.0, 0.0), (l, w)];
        let n = nodal_weights(&p, l);
        assert!((n[3] - w * l * l / 30.0).abs() < 1e-12);
        assert!((n[5] + w * l * l / 20.0).abs() < 1e-12);
        assert!((n[2] - 3.0 * w * l / 20.0).abs() < 1e-12);
    }
}
