//! Dense linear algebra for the direct-stiffness solvers.
//!
//! The matrix size is a runtime parameter and nothing here knows about nodes or
//! degrees of freedom per node, so the same solver serves the 2D frame (3 DOF
//! per node) and any later element family with more DOFs per node.

/// Smallest acceptable pivot of the Jacobi-scaled Cholesky factorisation.
///
/// After scaling, every diagonal entry of the matrix is 1, so a pivot reads as
/// "the fraction of this unknown's own stiffness that is still left once all
/// earlier unknowns have been eliminated". Pivots of a stable structure stay
/// many orders above this threshold; the pivot of a mechanism or rigid-body mode
/// is pure round-off noise (about `1e-16`). `1e-10` sits between the two, with
/// roughly five orders of margin on either side for the test structures.
///
/// Consequence (by design): a structure whose stiffness ratio along a single
/// pivot chain exceeds about `1e10` cannot be told apart from a mechanism and is
/// reported as singular.
pub(crate) const PIVOT_TOL: f64 = 1e-10;

/// Why [`solve_spd`] refused to produce a solution. `index` is a position in
/// the system that was passed in, not in the structure it came from; the caller
/// translates it to a node and degree of freedom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SolveError {
    /// The diagonal entry of unknown `index` is zero, negative or not finite,
    /// so that unknown has no stiffness of its own.
    NoStiffness { index: usize },
    /// The pivot of unknown `index` is not above [`PIVOT_TOL`] (or is NaN): the
    /// matrix is singular, or too ill-conditioned to tell apart from singular.
    Singular { index: usize, pivot: f64 },
    /// The solution contains NaN or infinity.
    NonFinite,
}

/// Solves `A x = b` for a symmetric positive-definite `A`, given row-major as an
/// `n × n` vector (only the lower triangle is read).
///
/// Equivalent to [`Cholesky::factor`] followed by one [`Cholesky::solve`]; see
/// there for the scaling and the singularity test.
///
/// # Panics
///
/// If `a.len() != n * n` or `b.len() != n` (a programming error).
pub(crate) fn solve_spd(a: Vec<f64>, n: usize, b: &[f64]) -> Result<Vec<f64>, SolveError> {
    assert_eq!(b.len(), n, "right-hand side must have n entries");
    Cholesky::factor(a, n)?.solve(b)
}

/// A Jacobi-scaled Cholesky factorisation `S A S = L Lᵀ` that can be reused for
/// any number of right-hand sides.
#[derive(Debug, Clone)]
pub(crate) struct Cholesky {
    n: usize,
    /// `L` in the lower triangle, row-major `n × n`.
    l: Vec<f64>,
    /// `S = diag(1 / sqrt(a_kk))`.
    scale: Vec<f64>,
}

impl Cholesky {
    /// Factorises a symmetric positive-definite `A`, given row-major as an `n × n`
    /// vector (only the lower triangle is read).
    ///
    /// The matrix is first Jacobi-scaled to unit diagonal (`S A S`,
    /// `S = diag(1/sqrt(a_kk))`), which makes the singularity test independent of
    /// the units of each unknown (a rotation and a translation, kN and N, m and mm
    /// all behave alike), then factorised as `L Lᵀ` with a pivot test against
    /// [`PIVOT_TOL`]; a NaN pivot is rejected like a small one.
    ///
    /// # Panics
    ///
    /// If `a.len() != n * n` (a programming error).
    pub(crate) fn factor(mut a: Vec<f64>, n: usize) -> Result<Self, SolveError> {
        assert_eq!(a.len(), n * n, "matrix must be n x n");

        // Jacobi scaling. The diagonal is checked before it is used as a divisor.
        let mut scale = Vec::with_capacity(n);
        for k in 0..n {
            let d = a[k * n + k];
            if !d.is_finite() || d <= 0.0 {
                return Err(SolveError::NoStiffness { index: k });
            }
            scale.push(1.0 / d.sqrt());
        }
        for (i, row) in a.chunks_exact_mut(n.max(1)).enumerate().take(n) {
            for (j, v) in row[..=i].iter_mut().enumerate() {
                *v *= scale[i] * scale[j];
            }
        }

        // Cholesky A = L Lᵀ, one row at a time, L overwriting the lower triangle.
        for i in 0..n {
            let (done, rest) = a.split_at_mut(i * n);
            let row_i = &mut rest[..n];
            for j in 0..i {
                let row_j = &done[j * n..(j + 1) * n];
                let dot: f64 = row_i[..j].iter().zip(&row_j[..j]).map(|(x, y)| x * y).sum();
                row_i[j] = (row_i[j] - dot) / row_j[j];
            }
            let dot: f64 = row_i[..i].iter().map(|x| x * x).sum();
            let pivot = row_i[i] - dot;
            // A NaN pivot must fail too, which `pivot <= PIVOT_TOL` alone would let through.
            if pivot.is_nan() || pivot <= PIVOT_TOL {
                return Err(SolveError::Singular { index: i, pivot });
            }
            row_i[i] = pivot.sqrt();
        }
        Ok(Self { n, l: a, scale })
    }

    /// Solves `A x = b` with the stored factorisation.
    ///
    /// # Panics
    ///
    /// If `b.len()` differs from the factorised size (a programming error).
    pub(crate) fn solve(&self, b: &[f64]) -> Result<Vec<f64>, SolveError> {
        let (n, a, scale) = (self.n, &self.l, &self.scale);
        assert_eq!(b.len(), n, "right-hand side must have n entries");

        // (S A S) y = S b by forward and back substitution, then x = S y.
        let mut x: Vec<f64> = b.iter().zip(scale).map(|(bi, s)| bi * s).collect();
        for i in 0..n {
            let row = &a[i * n..(i + 1) * n];
            let dot: f64 = row[..i].iter().zip(&x[..i]).map(|(l, xm)| l * xm).sum();
            x[i] = (x[i] - dot) / row[i];
        }
        for i in (0..n).rev() {
            let row = &a[i * n..(i + 1) * n];
            x[i] /= row[i];
            let xi = x[i];
            for (xm, l) in x[..i].iter_mut().zip(&row[..i]) {
                *xm -= l * xi;
            }
        }
        for (xi, s) in x.iter_mut().zip(scale) {
            *xi *= s;
        }

        if x.iter().all(|v| v.is_finite()) {
            Ok(x)
        } else {
            Err(SolveError::NonFinite)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64, rel: f64) {
        assert!(
            (actual - expected).abs() <= rel * expected.abs(),
            "got {actual}, expected {expected}"
        );
    }

    #[test]
    fn solves_a_known_system() {
        // A x = b with x = [1, 2, 3].
        let a = vec![4.0, 1.0, 0.0, 1.0, 3.0, 1.0, 0.0, 1.0, 2.0];
        let x = solve_spd(a, 3, &[6.0, 10.0, 8.0]).unwrap();
        for (got, want) in x.iter().zip([1.0, 2.0, 3.0]) {
            assert_close(*got, want, 1e-14);
        }
    }

    #[test]
    fn only_the_lower_triangle_is_read() {
        let a = vec![4.0, 999.0, 999.0, 1.0, 3.0, 999.0, 0.0, 1.0, 2.0];
        let x = solve_spd(a, 3, &[6.0, 10.0, 8.0]).unwrap();
        for (got, want) in x.iter().zip([1.0, 2.0, 3.0]) {
            assert_close(*got, want, 1e-14);
        }
    }

    #[test]
    fn unknowns_in_wildly_different_units_do_not_trip_the_singularity_test() {
        // D A D with D = diag(1, 1e8, 1e-8): the same system with unknown 1 measured
        // in units 1e8 times smaller and unknown 2 in units 1e8 times larger.
        let d = [1.0, 1e8, 1e-8];
        let base = [4.0, 1.0, 0.0, 1.0, 3.0, 1.0, 0.0, 1.0, 2.0];
        let mut a = vec![0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                a[i * 3 + j] = d[i] * base[i * 3 + j] * d[j];
            }
        }
        let b: Vec<f64> = [6.0, 10.0, 8.0].iter().zip(d).map(|(v, s)| v * s).collect();
        let x = solve_spd(a, 3, &b).unwrap();
        for ((got, want), s) in x.iter().zip([1.0, 2.0, 3.0]).zip(d) {
            assert_close(*got, want / s, 1e-12);
        }
    }

    #[test]
    fn rank_deficient_matrix_is_singular() {
        let err = solve_spd(vec![1.0, 1.0, 1.0, 1.0], 2, &[1.0, 1.0]).unwrap_err();
        assert!(
            matches!(err, SolveError::Singular { index: 1, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn indefinite_matrix_is_rejected() {
        let err = solve_spd(vec![1.0, 2.0, 2.0, 1.0], 2, &[1.0, 1.0]).unwrap_err();
        assert!(
            matches!(err, SolveError::Singular { index: 1, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn zero_or_negative_diagonal_has_no_stiffness() {
        let err = solve_spd(vec![1.0, 0.0, 0.0, 0.0], 2, &[1.0, 1.0]).unwrap_err();
        assert_eq!(err, SolveError::NoStiffness { index: 1 });
        let err = solve_spd(vec![-1.0], 1, &[1.0]).unwrap_err();
        assert_eq!(err, SolveError::NoStiffness { index: 0 });
    }

    #[test]
    fn non_finite_input_is_rejected_not_propagated() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            // A bad diagonal entry has no usable stiffness.
            let err = solve_spd(vec![bad], 1, &[1.0]).unwrap_err();
            assert_eq!(err, SolveError::NoStiffness { index: 0 });
            let err = solve_spd(vec![1.0, 0.0, 0.0, bad], 2, &[1.0, 1.0]).unwrap_err();
            assert_eq!(err, SolveError::NoStiffness { index: 1 });
            // A bad off-diagonal entry in the lower triangle poisons the pivot.
            let err = solve_spd(vec![1.0, 0.0, bad, 1.0], 2, &[1.0, 1.0]).unwrap_err();
            assert!(
                matches!(err, SolveError::Singular { index: 1, .. }),
                "{err:?}"
            );
            // A bad right-hand side gives a non-finite solution.
            let err = solve_spd(vec![2.0], 1, &[bad]).unwrap_err();
            assert_eq!(err, SolveError::NonFinite);
        }
    }

    #[test]
    fn empty_system_has_an_empty_solution() {
        assert_eq!(solve_spd(Vec::new(), 0, &[]), Ok(Vec::new()));
    }

    #[test]
    fn pivot_threshold_separates_tiny_from_round_off() {
        // Unit-diagonal 2x2 with off-diagonal r: the second pivot is 1 - r^2.
        let with_pivot = |p: f64| {
            let r = (1.0 - p).sqrt();
            solve_spd(vec![1.0, r, r, 1.0], 2, &[1.0, 1.0])
        };
        assert!(
            with_pivot(1e-8).is_ok(),
            "pivot 1e-8 is a legitimate stiffness ratio"
        );
        assert!(
            with_pivot(1e-12).is_err(),
            "pivot 1e-12 is indistinguishable from singular"
        );
    }
}
