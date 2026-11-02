//! 3D constraint sketches ([`Sketch3`]): symbolic equations, decomposition
//! into subsystems and per-subsystem solving with DOF analysis.

use crate::{linalg, Expr, Status, INCONSISTENT_RESIDUAL, NEXT_SKETCH_ID, RANK_TOLERANCE};
use std::sync::atomic::Ordering;

/// Handle to a point of a [`Sketch3`], returned by [`Sketch3::add_point3`].
///
/// Only valid for the sketch that created it (or a clone of it); using it
/// with another sketch panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Point3Id {
    sketch: u32,
    index: u32,
}

impl Point3Id {
    /// Zero-based index of the point within its sketch, in insertion order.
    pub fn index(self) -> usize {
        self.index as usize
    }
}

/// Handle to a fixed datum of a [`Sketch3`], returned by [`Sketch3::add_datum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DatumId {
    sketch: u32,
    index: u32,
}

impl DatumId {
    /// Zero-based index of the datum within its sketch, in insertion order.
    pub fn index(self) -> usize {
        self.index as usize
    }
}

/// The plane `{p : normal · p = offset}`. The normal need not be unit length
/// but must be finite and non-zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    /// Plane normal.
    pub normal: [f64; 3],
    /// Signed offset along the normal (scaled by `‖normal‖`).
    pub offset: f64,
}

/// A geometric constraint between points of a [`Sketch3`]. "Line `a→b`"
/// denotes the direction `b − a`. See [`Sketch3`] for the equations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Constraint3 {
    /// The two points coincide. Three equations.
    Coincident3(Point3Id, Point3Id),
    /// Euclidean distance between the points equals the value. One equation.
    Distance3(Point3Id, Point3Id, f64),
    /// Line `a→b` is parallel (or anti-parallel) to line `c→d`. Three
    /// equations (the normalised cross product), of rank at most two.
    Parallel3(Point3Id, Point3Id, Point3Id, Point3Id),
    /// Line `a→b` is perpendicular to line `c→d`. One equation.
    Perpendicular3(Point3Id, Point3Id, Point3Id, Point3Id),
    /// The unsigned angle between lines `a→b` and `c→d` equals the value
    /// (radians, meaningful in `[0, π]`). One equation.
    Angle3(Point3Id, Point3Id, Point3Id, Point3Id, f64),
    /// The point lies on the (fixed) datum plane. One equation.
    OnDatum(Point3Id, DatumId),
}

impl Constraint3 {
    /// Number of scalar equations this constraint contributes.
    pub fn equation_count(&self) -> usize {
        match self {
            Constraint3::Coincident3(..) | Constraint3::Parallel3(..) => 3,
            _ => 1,
        }
    }

    /// The points this constraint refers to (with repetitions).
    pub fn points(&self) -> Vec<Point3Id> {
        use Constraint3::*;
        match *self {
            OnDatum(p, _) => vec![p],
            Coincident3(a, b) | Distance3(a, b, _) => vec![a, b],
            Parallel3(a, b, c, d) | Perpendicular3(a, b, c, d) | Angle3(a, b, c, d, _) => {
                vec![a, b, c, d]
            }
        }
    }
}

/// An independently solvable part of a [`Sketch3`] (a connected component of
/// the constraint graph).
#[derive(Debug, Clone, PartialEq)]
pub struct Subsystem {
    /// The points of the component, in index order.
    pub points: Vec<Point3Id>,
    /// Indices (into [`Sketch3::constraints`]) of the constraints acting on
    /// these points, in insertion order.
    pub constraints: Vec<usize>,
}

/// Solve outcome for one [`Subsystem`].
#[derive(Debug, Clone, PartialEq)]
pub struct SubReport {
    /// The points of the subsystem, in index order.
    pub points: Vec<Point3Id>,
    /// Indices of the subsystem's constraints, in insertion order.
    pub constraints: Vec<usize>,
    /// Classification at the solved positions.
    pub status: Status,
    /// `3·points − rank(J)`.
    pub dof: usize,
    /// Euclidean norm of the subsystem's residual vector after solving.
    pub residual: f64,
}

/// Outcome of [`Sketch3::solve`]: one report per subsystem, in the order of
/// [`Sketch3::decompose`].
#[derive(Debug, Clone, PartialEq)]
pub struct SolveReport3 {
    /// Per-subsystem results.
    pub subsystems: Vec<SubReport>,
}

/// A 3D sketch. A `Sketch3` owns points (three unknown coordinates each), fixed datum
/// [`Plane`]s and a list of [`Constraint3`]s. Every constraint is turned into
/// scalar residual equations built as [`Expr`] trees; the Jacobian is obtained
/// by symbolic differentiation ([`Expr::diff`]), never by finite differences.
///
/// # Equations
///
/// With `u = b − a`, `v = d − c` and a datum stored as a unit normal `n` and
/// offset `o` (the plane `{p : n·p = o}`; [`Sketch3::add_datum`] normalises):
///
/// | constraint | equations |
/// |---|---|
/// | `Coincident3(p, q)` | `p − q` (3 equations) |
/// | `Distance3(p, q, d)` | `‖p − q‖ − d` |
/// | `Parallel3(a, b, c, d)` | `(u × v) / (‖u‖‖v‖)` (3 equations, rank ≤ 2) |
/// | `Perpendicular3(a, b, c, d)` | `(u · v) / (‖u‖‖v‖)` |
/// | `Angle3(a, b, c, d, θ)` | `(u · v) / (‖u‖‖v‖) − cos θ` (unsigned angle) |
/// | `OnDatum(p, datum)` | `n · p − o` |
///
/// # Decomposition and classification
///
/// [`Sketch3::decompose`] partitions the points into the connected components
/// of the graph that joins all points of each constraint; a point that no
/// constraint mentions is a subsystem on its own. Subsystems are listed in
/// order of their smallest point index, with points in index order.
///
/// [`Sketch3::solve`] runs Levenberg–Marquardt separately on each subsystem's
/// coordinates (other subsystems' points are never touched) and classifies it
/// at its solution. With `residual = ‖F‖₂` over the subsystem's equations,
/// `rank` = number of singular values of its Jacobian above
/// [`RANK_TOLERANCE`]` · σ_max` and `dof = 3·points − rank`, the status is, in
/// priority order:
///
/// 1. [`Status::Inconsistent`] if `residual > `[`INCONSISTENT_RESIDUAL`];
/// 2. [`Status::UnderConstrained`] if `dof > 0`;
/// 3. [`Status::OverConstrained`] if some constraint is redundant: removing
///    all of its equations leaves the Jacobian rank unchanged (so a lone
///    `Parallel3`, whose three equations have rank 2, is not redundant);
/// 4. [`Status::WellConstrained`] otherwise.
///
/// # Limitations
///
/// Levenberg–Marquardt converges to a solution near the starting guess; there
/// is no global search. A starting guess in which a `Distance3` pair coincides
/// or a direction `u`/`v` has zero length has no defined gradient there (such
/// Jacobian entries are treated as zero and NaN residuals are not stepped
/// from), so that subsystem is left in place and reported
/// [`Status::Inconsistent`]. `Angle3` at θ = 0 or π has a zero gradient at the
/// solution, so its DOF count is over-reported; use `Parallel3` instead.
///
/// ```
/// use bim_solver::{Constraint3::*, Plane, Sketch3, Status};
///
/// let mut s = Sketch3::new();
/// let p = s.add_point3(0.2, 0.1, 0.0);
/// let free = s.add_point3(5.0, 5.0, 5.0);
/// for normal in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
///     let d = s.add_datum(Plane { normal, offset: 1.0 });
///     s.add(OnDatum(p, d));
/// }
/// let r = s.solve();
/// assert_eq!(r.subsystems.len(), 2);
/// assert_eq!((r.subsystems[0].status, r.subsystems[0].dof), (Status::WellConstrained, 0));
/// assert_eq!((r.subsystems[1].status, r.subsystems[1].dof), (Status::UnderConstrained, 3));
/// let (x, y, z) = s.point3(p);
/// assert!((x - 1.0).abs() < 1e-12 && (y - 1.0).abs() < 1e-12 && (z - 1.0).abs() < 1e-12);
/// assert_eq!(s.point3(free), (5.0, 5.0, 5.0));
/// ```
#[derive(Debug, Clone)]
pub struct Sketch3 {
    id: u32,
    /// Interleaved coordinates `[x0, y0, z0, x1, ...]`.
    coords: Vec<f64>,
    /// Datums with unit normals.
    datums: Vec<Plane>,
    constraints: Vec<Constraint3>,
}

impl Default for Sketch3 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sketch3 {
    /// Creates an empty sketch.
    pub fn new() -> Self {
        Sketch3 {
            id: NEXT_SKETCH_ID.fetch_add(1, Ordering::Relaxed),
            coords: Vec::new(),
            datums: Vec::new(),
            constraints: Vec::new(),
        }
    }

    /// Adds a point with initial position `(x, y, z)` (the starting guess for
    /// [`Sketch3::solve`]).
    pub fn add_point3(&mut self, x: f64, y: f64, z: f64) -> Point3Id {
        let index = u32::try_from(self.point_count()).expect("too many points in sketch");
        self.coords.extend_from_slice(&[x, y, z]);
        Point3Id { sketch: self.id, index }
    }

    /// Adds a fixed datum plane. The plane is stored normalised (unit normal,
    /// offset divided by `‖normal‖`), which describes the same point set.
    ///
    /// # Panics
    /// If the normal is zero or not finite, or the offset is not finite.
    pub fn add_datum(&mut self, plane: Plane) -> DatumId {
        let [a, b, c] = plane.normal;
        let len = (a * a + b * b + c * c).sqrt();
        assert!(
            len.is_finite() && len > 0.0 && plane.offset.is_finite(),
            "datum plane needs a finite non-zero normal and finite offset, got {plane:?}"
        );
        let index = u32::try_from(self.datums.len()).expect("too many datums in sketch");
        self.datums.push(Plane { normal: [a / len, b / len, c / len], offset: plane.offset / len });
        DatumId { sketch: self.id, index }
    }

    /// Adds a constraint.
    ///
    /// # Panics
    /// If it refers to a point or datum that does not belong to this sketch.
    pub fn add(&mut self, c: Constraint3) {
        for p in c.points() {
            self.check(p);
        }
        if let Constraint3::OnDatum(_, d) = c {
            self.check_datum(d);
        }
        self.constraints.push(c);
    }

    /// Current position of a point.
    ///
    /// # Panics
    /// If `id` does not belong to this sketch.
    pub fn point3(&self, id: Point3Id) -> (f64, f64, f64) {
        let i = 3 * self.check(id);
        (self.coords[i], self.coords[i + 1], self.coords[i + 2])
    }

    /// Moves a point (e.g. to give a new starting guess before re-solving).
    pub fn set_point3(&mut self, id: Point3Id, x: f64, y: f64, z: f64) {
        let i = 3 * self.check(id);
        self.coords[i..i + 3].copy_from_slice(&[x, y, z]);
    }

    /// The stored (normalised) datum plane.
    pub fn datum(&self, id: DatumId) -> Plane {
        self.datums[self.check_datum(id)]
    }

    /// Number of points.
    pub fn point_count(&self) -> usize {
        self.coords.len() / 3
    }

    /// The constraints added so far, in insertion order.
    pub fn constraints(&self) -> &[Constraint3] {
        &self.constraints
    }

    fn check(&self, id: Point3Id) -> usize {
        assert!(
            id.sketch == self.id && id.index() < self.point_count(),
            "Point3Id {id:?} does not belong to this sketch"
        );
        id.index()
    }

    fn check_datum(&self, id: DatumId) -> usize {
        assert!(
            id.sketch == self.id && id.index() < self.datums.len(),
            "DatumId {id:?} does not belong to this sketch"
        );
        id.index()
    }

    fn pid(&self, index: usize) -> Point3Id {
        Point3Id { sketch: self.id, index: index as u32 }
    }

    /// Splits the sketch into independently solvable subsystems: the
    /// connected components of the graph joining every constraint's points.
    /// Every point belongs to exactly one subsystem.
    pub fn decompose(&self) -> Vec<Subsystem> {
        let n = self.point_count();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for c in &self.constraints {
            let pts = c.points();
            let r0 = find(&mut parent, pts[0].index());
            for q in &pts[1..] {
                let r = find(&mut parent, q.index());
                if r != r0 {
                    // Keep the smaller index as root so roots are deterministic.
                    let (lo, hi) = if r < r0 { (r, r0) } else { (r0, r) };
                    parent[hi] = lo;
                }
            }
        }
        // Component slot per root, in order of first (smallest) member.
        let mut slot = vec![usize::MAX; n];
        let mut out: Vec<Subsystem> = Vec::new();
        for i in 0..n {
            let r = find(&mut parent, i);
            if slot[r] == usize::MAX {
                slot[r] = out.len();
                out.push(Subsystem { points: Vec::new(), constraints: Vec::new() });
            }
            out[slot[r]].points.push(self.pid(i));
        }
        for (k, c) in self.constraints.iter().enumerate() {
            let r = find(&mut parent, c.points()[0].index());
            out[slot[r]].constraints.push(k);
        }
        out
    }

    /// Solves every subsystem in place (each only moves its own points) and
    /// classifies each one. An inconsistent subsystem is left at a
    /// least-squares compromise and does not affect the others.
    pub fn solve(&mut self) -> SolveReport3 {
        let subs = self.decompose();
        let mut reports = Vec::with_capacity(subs.len());
        for sub in subs {
            let sys = System::build(self, &sub);
            let mut x = self.gather(&sub.points);
            if sys.m > 0 {
                crate::levenberg_marquardt(|x, f, j| sys.eval(x, f, j), &mut x, sys.m);
                if x.iter().all(|v| v.is_finite()) {
                    self.scatter(&sub.points, &x);
                } else {
                    x = self.gather(&sub.points);
                }
            }
            reports.push(sys.classify(&x, sub));
        }
        SolveReport3 { subsystems: reports }
    }

    /// Classifies every subsystem at the current positions without moving
    /// any point.
    pub fn report(&self) -> SolveReport3 {
        let subsystems = self
            .decompose()
            .into_iter()
            .map(|sub| {
                let x = self.gather(&sub.points);
                System::build(self, &sub).classify(&x, sub)
            })
            .collect();
        SolveReport3 { subsystems }
    }

    fn gather(&self, points: &[Point3Id]) -> Vec<f64> {
        points.iter().flat_map(|p| self.coords[3 * p.index()..3 * p.index() + 3].to_vec()).collect()
    }

    fn scatter(&mut self, points: &[Point3Id], x: &[f64]) {
        for (k, p) in points.iter().enumerate() {
            self.coords[3 * p.index()..3 * p.index() + 3].copy_from_slice(&x[3 * k..3 * k + 3]);
        }
    }

    /// Residual expressions of constraint `c`, with point `p`'s coordinates
    /// as variables `3·local(p) + {0, 1, 2}`.
    fn equations(&self, c: &Constraint3, local: &dyn Fn(Point3Id) -> usize) -> Vec<Expr> {
        use Constraint3::*;
        let pt = |p: Point3Id| -> [Expr; 3] {
            let b = 3 * local(p);
            [Expr::var(b), Expr::var(b + 1), Expr::var(b + 2)]
        };
        let sub = |p: &[Expr; 3], q: &[Expr; 3]| -> [Expr; 3] {
            [p[0].clone() - q[0].clone(), p[1].clone() - q[1].clone(), p[2].clone() - q[2].clone()]
        };
        let dot = |u: &[Expr; 3], v: &[Expr; 3]| -> Expr {
            u[0].clone() * v[0].clone() + u[1].clone() * v[1].clone() + u[2].clone() * v[2].clone()
        };
        let dirs = |a, b, c, d| {
            let u = sub(&pt(b), &pt(a));
            let v = sub(&pt(d), &pt(c));
            let norms = dot(&u, &u).sqrt() * dot(&v, &v).sqrt();
            (u, v, norms)
        };
        match *c {
            Coincident3(p, q) => sub(&pt(p), &pt(q)).to_vec(),
            Distance3(p, q, d) => {
                let w = sub(&pt(p), &pt(q));
                vec![dot(&w, &w).sqrt() - Expr::constant(d)]
            }
            Parallel3(a, b, c, d) => {
                let (u, v, norms) = dirs(a, b, c, d);
                let cross = |i: usize, j: usize| u[i].clone() * v[j].clone() - u[j].clone() * v[i].clone();
                vec![cross(1, 2) / norms.clone(), cross(2, 0) / norms.clone(), cross(0, 1) / norms]
            }
            Perpendicular3(a, b, c, d) => {
                let (u, v, norms) = dirs(a, b, c, d);
                vec![dot(&u, &v) / norms]
            }
            Angle3(a, b, c, d, theta) => {
                let (u, v, norms) = dirs(a, b, c, d);
                vec![dot(&u, &v) / norms - Expr::constant(theta.cos())]
            }
            OnDatum(p, datum) => {
                let plane = self.datums[datum.index()];
                let n = plane.normal.map(Expr::constant);
                vec![dot(&n, &pt(p)) - Expr::constant(plane.offset)]
            }
        }
    }
}

/// The equations of one subsystem on local variables, with their symbolic
/// Jacobian (only the structurally non-zero entries are stored).
struct System {
    /// Number of local variables (`3 · points`).
    n: usize,
    /// Number of equations.
    m: usize,
    rows: Vec<Expr>,
    /// `(row, column, ∂row/∂column)`.
    jac: Vec<(usize, usize, Expr)>,
    /// Row range of each constraint of the subsystem.
    spans: Vec<std::ops::Range<usize>>,
}

impl System {
    fn build(sketch: &Sketch3, sub: &Subsystem) -> System {
        let mut local = vec![usize::MAX; sketch.point_count()];
        for (k, p) in sub.points.iter().enumerate() {
            local[p.index()] = k;
        }
        let local_of = |p: Point3Id| local[p.index()];
        let mut rows = Vec::new();
        let mut spans = Vec::new();
        for &ci in &sub.constraints {
            let start = rows.len();
            rows.extend(sketch.equations(&sketch.constraints[ci], &local_of));
            spans.push(start..rows.len());
        }
        let mut jac = Vec::new();
        for (r, e) in rows.iter().enumerate() {
            for v in e.vars() {
                let d = e.diff(v);
                if d.as_constant() != Some(0.0) {
                    jac.push((r, v, d));
                }
            }
        }
        System { n: 3 * sub.points.len(), m: rows.len(), rows, jac, spans }
    }

    /// Residuals into `f`; Jacobian (`m × n`, row-major, overwritten) into
    /// `j`. Non-finite Jacobian entries (a derivative at a degenerate,
    /// zero-length direction) are replaced by zero.
    fn eval(&self, x: &[f64], f: &mut [f64], j: Option<&mut [f64]>) {
        for (fr, e) in f.iter_mut().zip(&self.rows) {
            *fr = e.eval(x);
        }
        if let Some(j) = j {
            j.iter_mut().for_each(|v| *v = 0.0);
            for (r, c, d) in &self.jac {
                let v = d.eval(x);
                j[r * self.n + c] = if v.is_finite() { v } else { 0.0 };
            }
        }
    }

    fn classify(&self, x: &[f64], sub: Subsystem) -> SubReport {
        let (m, n) = (self.m, self.n);
        let mut f = vec![0.0; m];
        let mut j = vec![0.0; m * n];
        self.eval(x, &mut f, Some(&mut j));
        let residual = f.iter().map(|v| v * v).sum::<f64>().sqrt();
        let rank = linalg::rank(&j, m, n, RANK_TOLERANCE);
        let dof = n - rank;
        let status = if !(residual <= INCONSISTENT_RESIDUAL) {
            Status::Inconsistent
        } else if dof > 0 {
            Status::UnderConstrained
        } else if self.spans.iter().any(|span| self.rank_without(&j, span) == rank) {
            Status::OverConstrained
        } else {
            Status::WellConstrained
        };
        SubReport { points: sub.points, constraints: sub.constraints, status, dof, residual }
    }

    /// Rank of `j` with the rows in `span` removed.
    fn rank_without(&self, j: &[f64], span: &std::ops::Range<usize>) -> usize {
        let n = self.n;
        let kept: Vec<f64> = (0..self.m)
            .filter(|r| !span.contains(r))
            .flat_map(|r| j[r * n..(r + 1) * n].iter().copied())
            .collect();
        linalg::rank(&kept, self.m - span.len(), n, RANK_TOLERANCE)
    }
}

#[cfg(test)]
mod tests;
