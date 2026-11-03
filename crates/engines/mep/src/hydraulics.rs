//! Single-conduit hydraulics: the friction factor (laminar 64/Re below Re 2000, Colebrook-White
//! above, no transition interpolation) and the Darcy-Weisbach pressure drop with minor losses,
//! `dp = (f·L/D + K)·ρ·v²/2`, signed with the flow.

use std::f64::consts::LN_10;

/// Below this Reynolds number the flow is laminar (`f = 64/Re`).
pub const RE_LAMINAR: f64 = 2000.0;

/// A fluid: density (kg/m³) and dynamic viscosity (Pa·s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fluid {
    pub density: f64,
    pub viscosity: f64,
}

/// A conduit between two nodes of a network, oriented `a` → `b`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pipe {
    pub a: usize,
    pub b: usize,
    /// Flow area (m²).
    pub area: f64,
    /// Hydraulic diameter (m): the diameter of a round pipe, `2wh/(w+h)` of a rectangular duct.
    pub dh: f64,
    /// Length (m).
    pub length: f64,
    /// Absolute roughness (m).
    pub roughness: f64,
    /// Minor-loss coefficient.
    pub k: f64,
}

/// The hydraulic state of a pipe at one flow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PipeState {
    /// `|Q|/A` (m/s).
    pub velocity: f64,
    /// `ρ·v·D/μ`.
    pub reynolds: f64,
    /// Darcy friction factor; 0 at zero flow.
    pub friction_factor: f64,
    /// Pressure drop from `a` to `b` (Pa), same sign as the flow.
    pub dp: f64,
}

/// The Colebrook-White friction factor at Reynolds number `re` (> 0) and relative roughness
/// `rr` = ε/D, with `Re·df/dRe`. Solved by Newton's method on `x = 1/√f`,
/// `g(x) = x + 2·log10(rr/3.7 + 2.51·x/Re) = 0`, seeded by Swamee-Jain, until the step is below
/// 1e-15·x (well inside the 1e-12 the spec asks for).
pub fn colebrook(re: f64, rr: f64) -> (f64, f64) {
    let a = rr / 3.7;
    let b = 2.51 / re;
    let seed = 0.25 / (a + 5.74 / re.powf(0.9)).log10().powi(2);
    let mut x = if seed.is_finite() && seed > 0.0 { 1.0 / seed.sqrt() } else { 8.0 };
    for _ in 0..200 {
        let s = a + b * x;
        let g = x + 2.0 * s.log10();
        let dg = 1.0 + 2.0 * b / (s * LN_10);
        let mut next = x - g / dg;
        // g is concave and increasing: a Newton step can only undershoot; never leave x > 0.
        if !(next.is_finite() && next > 0.0) {
            next = x / 2.0;
        }
        let step = (next - x).abs();
        x = next;
        if step <= 1e-15 * x {
            break;
        }
    }
    // Re·dx/dRe from the implicit function g(x, Re) = 0, then f = x⁻² ⇒ df = −2x⁻³dx.
    let s = a + b * x;
    let c = 2.0 * b / (s * LN_10);
    let re_dx = c * x / (1.0 + c);
    let f = 1.0 / (x * x);
    (f, -2.0 * re_dx / (x * x * x))
}

/// The Darcy friction factor at `re` (`64/Re` below [`RE_LAMINAR`], Colebrook-White above) with
/// `Re·df/dRe`; `(0, 0)` at `re` ≤ 0.
pub fn friction_factor(re: f64, rr: f64) -> (f64, f64) {
    if !(re > 0.0) {
        (0.0, 0.0)
    } else if re < RE_LAMINAR {
        (64.0 / re, -64.0 / re)
    } else {
        colebrook(re, rr)
    }
}

impl Pipe {
    /// The state of the pipe carrying the signed flow `q` (m³/s).
    pub fn state(&self, fluid: &Fluid, q: f64) -> PipeState {
        let velocity = q.abs() / self.area;
        let reynolds = fluid.density * velocity * self.dh / fluid.viscosity;
        let (friction_factor, _) = friction_factor(reynolds, self.roughness / self.dh);
        let dp = (friction_factor * self.length / self.dh + self.k) * fluid.density * velocity * velocity / 2.0;
        PipeState { velocity, reynolds, friction_factor, dp: if q < 0.0 { -dp } else { dp } }
    }

    /// The signed pressure drop at `q` and its derivative `d(dp)/dq` (> 0 except for a pipe with no
    /// resistance at zero flow), including the dependence of `f` on the Reynolds number.
    pub fn dp_slope(&self, fluid: &Fluid, q: f64) -> (f64, f64) {
        let st = self.state(fluid, q);
        let rho_a2 = fluid.density / (self.area * self.area);
        let slope = if st.reynolds < RE_LAMINAR {
            // dp = 32·μ·L·v/D² + K·ρ·v²/2 (signed): linear plus the minor-loss term.
            32.0 * fluid.viscosity * self.length / (self.dh * self.dh * self.area) + self.k * rho_a2 * q.abs()
        } else {
            let (f, re_df) = friction_factor(st.reynolds, self.roughness / self.dh);
            let l_d = self.length / self.dh;
            rho_a2 * q.abs() * (f * l_d + self.k + 0.5 * l_d * re_df)
        };
        (st.dp, slope)
    }

    /// [`Pipe::dp_slope`] with the slope kept away from zero: below a velocity of 1e-6 m/s the
    /// slope is taken at that velocity, so a resistance-free zero-flow pipe still has a finite
    /// conductance in the Newton and Hardy-Cross updates.
    pub fn dp_slope_regularized(&self, fluid: &Fluid, q: f64) -> (f64, f64) {
        let (dp, slope) = self.dp_slope(fluid, q);
        let q_min = self.area * 1e-6;
        if q.abs() < q_min {
            (dp, slope.max(self.dp_slope(fluid, q_min).1))
        } else {
            (dp, slope)
        }
    }
}
