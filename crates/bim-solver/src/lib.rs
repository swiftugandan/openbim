//! `bim-solver`: a 2D geometric constraint solver with degree-of-freedom
//! analysis.
//!
//! A [`Sketch`] owns a set of points (each with two unknown coordinates) and a
//! list of [`Constraint`]s. [`Sketch::solve`] moves the points so that every
//! constraint equation is satisfied, using a Levenberg–Marquardt iteration on
//! all `2·N` coordinates with analytic Jacobians, and then classifies the
//! system by the rank of its Jacobian at the solution.
//!
//! ```
//! use bim_solver::{Constraint::*, Sketch, Status};
//!
//! let mut s = Sketch::new();
//! let a = s.add_point(0.0, 0.0);
//! let b = s.add_point(0.9, 0.2);
//! s.add(Fix(a, 0.0, 0.0));
//! s.add(Horizontal(a, b));
//! s.add(Distance(a, b, 2.0));
//! let r = s.solve();
//! assert_eq!(r.status, Status::WellConstrained);
//! assert_eq!(r.dof, 0);
//! let (x, y) = s.point(b);
//! assert!((x - 2.0).abs() < 1e-9 && y.abs() < 1e-9);
//! ```
//!
//! # Equations
//!
//! Every constraint contributes a fixed number of scalar equations, regardless
//! of the current point positions (see [`Constraint::equation_count`]). With
//! `u = b − a` and `v = d − c`:
//!
//! | constraint | equations |
//! |---|---|
//! | `Fix(p, x, y)` | `p.x − x`, `p.y − y` |
//! | `Coincident(a, b)` | `a.x − b.x`, `a.y − b.y` |
//! | `Distance(a, b, d)` | `‖b − a‖ − d` |
//! | `Horizontal(a, b)` | `a.y − b.y` |
//! | `Vertical(a, b)` | `a.x − b.x` |
//! | `Parallel(a, b, c, d)` | `(u × v) / (‖u‖‖v‖)` (sine of the angle) |
//! | `Perpendicular(a, b, c, d)` | `(u · v) / (‖u‖‖v‖)` (cosine of the angle) |
//! | `Angle(a, b, c, d, θ)` | `wrap(atan2(u × v, u · v) − θ)` into `(−π, π]` |
//!
//! Distances are therefore measured in length units and angles in radians, so
//! the reported residual is geometric. Degenerate (zero-length) directions
//! never produce NaN: `Parallel`/`Perpendicular` fall back to the raw
//! cross/dot product, `Distance` uses the `+x` direction as its gradient, and
//! `Angle` uses a zero gradient for a degenerate direction.
//!
//! # Classification
//!
//! After solving, `residual = ‖F‖₂` over all equations, `rank` is the number of
//! singular values of the Jacobian `J` (`m × 2N`) above `1e-9 · σ_max`, and
//! `dof = 2N − rank`. The status is chosen in priority order:
//!
//! 1. [`Status::Inconsistent`] if `residual > 1e-6`;
//! 2. [`Status::UnderConstrained`] if `dof > 0`;
//! 3. [`Status::OverConstrained`] if `m > rank` (redundant but consistent);
//! 4. [`Status::WellConstrained`] otherwise.

mod linalg;

use std::f64::consts::PI;
use std::sync::atomic::{AtomicU32, Ordering};

/// Residual norm above which a solved sketch is reported as
/// [`Status::Inconsistent`].
pub const INCONSISTENT_RESIDUAL: f64 = 1e-6;

/// Relative singular-value tolerance used to compute the Jacobian rank.
pub const RANK_TOLERANCE: f64 = 1e-9;

/// Lengths below this are treated as degenerate when normalising directions.
const TINY_LENGTH: f64 = 1e-12;

/// Maximum number of Levenberg–Marquardt iterations per [`Sketch::solve`].
const MAX_ITERATIONS: usize = 500;

static NEXT_SKETCH_ID: AtomicU32 = AtomicU32::new(0);

/// Handle to a point in a [`Sketch`], returned by [`Sketch::add_point`].
///
/// A `PointId` is only valid for the sketch that created it (or a clone of
/// that sketch); using it with another sketch panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PointId {
    sketch: u32,
    index: u32,
}

impl PointId {
    /// Zero-based index of the point within its sketch, in insertion order.
    pub fn index(self) -> usize {
        self.index as usize
    }
}

/// A geometric constraint between points of a [`Sketch`].
///
/// "Line `a→b`" denotes the direction vector `b − a`.
#[derive(Debug, Clone, Copy)]
pub enum Constraint {
    /// Pin a point to the coordinates `(x, y)`. Two equations.
    Fix(PointId, f64, f64),
    /// Make two points coincide. Two equations.
    Coincident(PointId, PointId),
    /// Euclidean distance between two points equals the given value. One
    /// equation (`‖b − a‖ − d`); a negative distance is unsatisfiable.
    Distance(PointId, PointId, f64),
    /// Two points share the same `y`. One equation.
    Horizontal(PointId, PointId),
    /// Two points share the same `x`. One equation.
    Vertical(PointId, PointId),
    /// Line `a→b` is parallel (or anti-parallel) to line `c→d`. One equation.
    Parallel(PointId, PointId, PointId, PointId),
    /// Line `a→b` is perpendicular to line `c→d`. One equation.
    Perpendicular(PointId, PointId, PointId, PointId),
    /// The signed counter-clockwise angle from direction `a→b` to direction
    /// `c→d` equals the given value in radians (taken modulo `2π`). One
    /// equation.
    Angle(PointId, PointId, PointId, PointId, f64),
}

impl Constraint {
    /// Number of scalar equations this constraint contributes.
    pub fn equation_count(&self) -> usize {
        match self {
            Constraint::Fix(..) | Constraint::Coincident(..) => 2,
            _ => 1,
        }
    }

    /// The points this constraint refers to (with repetitions).
    pub fn points(&self) -> Vec<PointId> {
        use Constraint::*;
        match *self {
            Fix(p, ..) => vec![p],
            Coincident(a, b) | Distance(a, b, _) | Horizontal(a, b) | Vertical(a, b) => vec![a, b],
            Parallel(a, b, c, d) | Perpendicular(a, b, c, d) | Angle(a, b, c, d, _) => {
                vec![a, b, c, d]
            }
        }
    }
}

/// Classification of a solved sketch. See the crate docs for the priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// All equations satisfied, no remaining freedom, no redundant equations.
    WellConstrained,
    /// All equations satisfied but `dof > 0` points can still move.
    UnderConstrained,
    /// All equations satisfied, `dof == 0`, but some equations are redundant
    /// (more scalar equations than the Jacobian rank).
    OverConstrained,
    /// The equations cannot all be satisfied (`residual > 1e-6` after solving).
    Inconsistent,
}

/// Outcome of [`Sketch::solve`].
#[derive(Debug, Clone, PartialEq)]
pub struct SolveReport {
    /// Classification of the system at the final point positions.
    pub status: Status,
    /// Euclidean norm of the equation residual vector after solving.
    pub residual: f64,
    /// Remaining degrees of freedom: `2·points − rank(J)`.
    pub dof: usize,
}

/// A 2D sketch: points plus constraints.
///
/// Cloning a sketch yields an independent copy in which the original's
/// [`PointId`]s remain valid.
#[derive(Debug, Clone)]
pub struct Sketch {
    id: u32,
    /// Interleaved coordinates `[x0, y0, x1, y1, ...]`.
    coords: Vec<f64>,
    constraints: Vec<Constraint>,
}

impl Default for Sketch {
    fn default() -> Self {
        Self::new()
    }
}

impl Sketch {
    /// Creates an empty sketch.
    pub fn new() -> Self {
        Sketch {
            id: NEXT_SKETCH_ID.fetch_add(1, Ordering::Relaxed),
            coords: Vec::new(),
            constraints: Vec::new(),
        }
    }

    /// Adds a point with initial position `(x, y)` and returns its handle.
    /// The position is used as the starting guess for [`Sketch::solve`].
    pub fn add_point(&mut self, x: f64, y: f64) -> PointId {
        let index = u32::try_from(self.point_count()).expect("too many points in sketch");
        self.coords.push(x);
        self.coords.push(y);
        PointId { sketch: self.id, index }
    }

    /// Adds a constraint.
    ///
    /// # Panics
    /// If the constraint refers to a point that does not belong to this sketch.
    pub fn add(&mut self, c: Constraint) {
        for p in c.points() {
            self.check(p);
        }
        self.constraints.push(c);
    }

    /// Current position of a point.
    ///
    /// # Panics
    /// If `id` does not belong to this sketch.
    pub fn point(&self, id: PointId) -> (f64, f64) {
        let i = self.check(id);
        (self.coords[2 * i], self.coords[2 * i + 1])
    }

    /// Moves a point (e.g. to provide a new starting guess before re-solving).
    ///
    /// # Panics
    /// If `id` does not belong to this sketch.
    pub fn set_point(&mut self, id: PointId, x: f64, y: f64) {
        let i = self.check(id);
        self.coords[2 * i] = x;
        self.coords[2 * i + 1] = y;
    }

    /// Number of points in the sketch.
    pub fn point_count(&self) -> usize {
        self.coords.len() / 2
    }

    /// The constraints added so far, in insertion order.
    pub fn constraints(&self) -> &[Constraint] {
        &self.constraints
    }

    /// Total number of scalar constraint equations.
    pub fn equation_count(&self) -> usize {
        self.constraints.iter().map(Constraint::equation_count).sum()
    }

    fn check(&self, id: PointId) -> usize {
        assert!(
            id.sketch == self.id && id.index() < self.point_count(),
            "PointId {id:?} does not belong to this sketch"
        );
        id.index()
    }

    /// Solves the sketch in place and classifies it.
    ///
    /// Runs Levenberg–Marquardt on all point coordinates starting from their
    /// current positions; for under-constrained sketches the damped steps keep
    /// points close to their starting guess. Inconsistent sketches are driven
    /// to a least-squares compromise and the iteration terminates cleanly.
    /// Calling `solve` again on a solved sketch is a no-op apart from
    /// re-reporting.
    pub fn solve(&mut self) -> SolveReport {
        let n = self.coords.len();
        let m = self.equation_count();
        if m > 0 && n > 0 {
            let mut x = self.coords.clone();
            levenberg_marquardt(&self.constraints, &mut x, m);
            if x.iter().all(|v| v.is_finite()) {
                self.coords = x;
            }
        }
        self.report()
    }

    /// Classifies the sketch at the current point positions without moving
    /// any point.
    pub fn report(&self) -> SolveReport {
        let n = self.coords.len();
        let m = self.equation_count();
        let mut f = vec![0.0; m];
        let mut j = vec![0.0; m * n];
        evaluate(&self.constraints, &self.coords, &mut f, Some(&mut j));
        let residual = norm(&f);
        let rank = linalg::rank(&j, m, n, RANK_TOLERANCE);
        let dof = n - rank;
        let status = if !(residual <= INCONSISTENT_RESIDUAL) {
            Status::Inconsistent
        } else if dof > 0 {
            Status::UnderConstrained
        } else if m > rank {
            Status::OverConstrained
        } else {
            Status::WellConstrained
        };
        SolveReport { status, residual, dof }
    }
}

fn norm(v: &[f64]) -> f64 {
    v.iter().map(|a| a * a).sum::<f64>().sqrt()
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Wraps an angle into `(−π, π]`.
fn wrap_angle(a: f64) -> f64 {
    let w = (a + PI).rem_euclid(2.0 * PI) - PI;
    if w <= -PI {
        w + 2.0 * PI
    } else {
        w
    }
}

/// Evaluates all constraint residuals into `f` (length `m`) and, if given,
/// the Jacobian into `jac` (`m × n`, row-major, overwritten).
fn evaluate(constraints: &[Constraint], x: &[f64], f: &mut [f64], mut jac: Option<&mut [f64]>) {
    use Constraint::*;
    let n = x.len();
    if let Some(j) = jac.as_deref_mut() {
        j.iter_mut().for_each(|v| *v = 0.0);
    }
    let pt = |p: PointId| (x[2 * p.index()], x[2 * p.index() + 1]);
    let mut row = 0;
    // Adds (gx, gy) to the partials of `row` with respect to point `p`.
    let grad = |jac: &mut Option<&mut [f64]>, row: usize, p: PointId, gx: f64, gy: f64| {
        if let Some(j) = jac.as_deref_mut() {
            j[row * n + 2 * p.index()] += gx;
            j[row * n + 2 * p.index() + 1] += gy;
        }
    };
    for con in constraints {
        match *con {
            Fix(p, fx, fy) => {
                let (px, py) = pt(p);
                f[row] = px - fx;
                f[row + 1] = py - fy;
                grad(&mut jac, row, p, 1.0, 0.0);
                grad(&mut jac, row + 1, p, 0.0, 1.0);
            }
            Coincident(a, b) => {
                let (ax, ay) = pt(a);
                let (bx, by) = pt(b);
                f[row] = ax - bx;
                f[row + 1] = ay - by;
                grad(&mut jac, row, a, 1.0, 0.0);
                grad(&mut jac, row, b, -1.0, 0.0);
                grad(&mut jac, row + 1, a, 0.0, 1.0);
                grad(&mut jac, row + 1, b, 0.0, -1.0);
            }
            Distance(a, b, d) => {
                let (ax, ay) = pt(a);
                let (bx, by) = pt(b);
                let (ux, uy) = (bx - ax, by - ay);
                let len = ux.hypot(uy);
                f[row] = len - d;
                let (ex, ey) = if len > TINY_LENGTH { (ux / len, uy / len) } else { (1.0, 0.0) };
                grad(&mut jac, row, a, -ex, -ey);
                grad(&mut jac, row, b, ex, ey);
            }
            Horizontal(a, b) => {
                f[row] = pt(a).1 - pt(b).1;
                grad(&mut jac, row, a, 0.0, 1.0);
                grad(&mut jac, row, b, 0.0, -1.0);
            }
            Vertical(a, b) => {
                f[row] = pt(a).0 - pt(b).0;
                grad(&mut jac, row, a, 1.0, 0.0);
                grad(&mut jac, row, b, -1.0, 0.0);
            }
            Parallel(a, b, c, d) | Perpendicular(a, b, c, d) => {
                let is_parallel = matches!(*con, Parallel(..));
                let (ax, ay) = pt(a);
                let (bx, by) = pt(b);
                let (cx, cy) = pt(c);
                let (dx, dy) = pt(d);
                let (ux, uy) = (bx - ax, by - ay);
                let (vx, vy) = (dx - cx, dy - cy);
                // Raw value and its partials w.r.t. u and v.
                let (raw, gu, gv) = if is_parallel {
                    (ux * vy - uy * vx, (vy, -vx), (-uy, ux))
                } else {
                    (ux * vx + uy * vy, (vx, vy), (ux, uy))
                };
                let lu = ux.hypot(uy);
                let lv = vx.hypot(vy);
                let (val, gu, gv) = if lu > TINY_LENGTH && lv > TINY_LENGTH {
                    // Normalised: g = raw / (|u||v|).
                    let s = 1.0 / (lu * lv);
                    let g = raw * s;
                    (
                        g,
                        (gu.0 * s - g * ux / (lu * lu), gu.1 * s - g * uy / (lu * lu)),
                        (gv.0 * s - g * vx / (lv * lv), gv.1 * s - g * vy / (lv * lv)),
                    )
                } else {
                    (raw, gu, gv)
                };
                f[row] = val;
                grad(&mut jac, row, a, -gu.0, -gu.1);
                grad(&mut jac, row, b, gu.0, gu.1);
                grad(&mut jac, row, c, -gv.0, -gv.1);
                grad(&mut jac, row, d, gv.0, gv.1);
            }
            Angle(a, b, c, d, theta) => {
                let (ax, ay) = pt(a);
                let (bx, by) = pt(b);
                let (cx, cy) = pt(c);
                let (dx, dy) = pt(d);
                let (ux, uy) = (bx - ax, by - ay);
                let (vx, vy) = (dx - cx, dy - cy);
                let cross = ux * vy - uy * vx;
                let dotp = ux * vx + uy * vy;
                f[row] = wrap_angle(cross.atan2(dotp) - theta);
                // phi = atan2(v) − atan2(u).
                let lu2 = ux * ux + uy * uy;
                let lv2 = vx * vx + vy * vy;
                let gu = if lu2.sqrt() > TINY_LENGTH { (uy / lu2, -ux / lu2) } else { (0.0, 0.0) };
                let gv = if lv2.sqrt() > TINY_LENGTH { (-vy / lv2, vx / lv2) } else { (0.0, 0.0) };
                grad(&mut jac, row, a, -gu.0, -gu.1);
                grad(&mut jac, row, b, gu.0, gu.1);
                grad(&mut jac, row, c, -gv.0, -gv.1);
                grad(&mut jac, row, d, gv.0, gv.1);
            }
        }
        row += con.equation_count();
    }
    debug_assert_eq!(row, f.len());
}

/// Levenberg–Marquardt with Nielsen damping updates, minimising `½‖F(x)‖²`.
/// Updates `x` in place.
fn levenberg_marquardt(constraints: &[Constraint], x: &mut [f64], m: usize) {
    let n = x.len();
    let mut f = vec![0.0; m];
    let mut j = vec![0.0; m * n];
    let mut f_new = vec![0.0; m];
    let mut x_new = vec![0.0; n];

    evaluate(constraints, x, &mut f, Some(&mut j));
    let mut cost = 0.5 * dot(&f, &f);
    let mut lambda = -1.0; // initialised from JᵀJ on first iteration
    let mut nu = 2.0;

    for _ in 0..MAX_ITERATIONS {
        if norm(&f) < 1e-14 {
            break;
        }
        // Normal equations: A = JᵀJ, g = JᵀF.
        let mut a = vec![0.0; n * n];
        let mut g = vec![0.0; n];
        for r in 0..m {
            let jr = &j[r * n..(r + 1) * n];
            for (p, &jp) in jr.iter().enumerate() {
                if jp == 0.0 {
                    continue;
                }
                g[p] += jp * f[r];
                for (q, &jq) in jr.iter().enumerate().skip(p) {
                    a[p * n + q] += jp * jq;
                }
            }
        }
        for p in 0..n {
            for q in 0..p {
                a[p * n + q] = a[q * n + p];
            }
        }
        let g_inf = g.iter().fold(0.0f64, |acc, v| acc.max(v.abs()));
        if g_inf < 1e-15 {
            break;
        }
        let max_diag = (0..n).map(|p| a[p * n + p]).fold(0.0f64, f64::max).max(1e-300);
        let lambda_min = 1e-12 * max_diag;
        if lambda < 0.0 {
            lambda = 1e-3 * max_diag;
        }

        // Inner loop: find an acceptable step.
        let mut accepted = false;
        let mut give_up = false;
        while !accepted {
            lambda = lambda.max(lambda_min);
            if lambda > 1e20 * max_diag {
                give_up = true;
                break;
            }
            let mut damped = a.clone();
            for p in 0..n {
                damped[p * n + p] += lambda;
            }
            let neg_g: Vec<f64> = g.iter().map(|v| -v).collect();
            let Some(h) = linalg::cholesky_solve(&damped, &neg_g, n) else {
                lambda *= nu;
                nu *= 2.0;
                continue;
            };
            let x_norm = norm(x);
            if norm(&h) <= 1e-15 * (x_norm + 1e-15) {
                give_up = true;
                break;
            }
            // Predicted reduction of the linear model: ½ hᵀ(λh − g).
            let predicted = 0.5 * (lambda * dot(&h, &h) - dot(&h, &g));
            if !(predicted > 0.0) {
                give_up = true;
                break;
            }
            for p in 0..n {
                x_new[p] = x[p] + h[p];
            }
            evaluate(constraints, &x_new, &mut f_new, None);
            let cost_new = 0.5 * dot(&f_new, &f_new);
            let rho = (cost - cost_new) / predicted;
            if rho > 0.0 && cost_new.is_finite() {
                x.copy_from_slice(&x_new);
                evaluate(constraints, x, &mut f, Some(&mut j));
                cost = cost_new;
                lambda *= (1.0f64 / 3.0).max(1.0 - (2.0 * rho - 1.0).powi(3));
                nu = 2.0;
                accepted = true;
            } else {
                lambda *= nu;
                nu *= 2.0;
            }
        }
        if give_up {
            break;
        }
    }
}

#[cfg(test)]
mod tests;
