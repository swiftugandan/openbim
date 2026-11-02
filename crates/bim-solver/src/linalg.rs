//! Small dense linear-algebra helpers used by the solver.
//!
//! Matrices are stored row-major in a flat `Vec<f64>`. Everything here is
//! sized for sketches of at most a few hundred unknowns; no attempt is made to
//! exploit sparsity.

/// Solves `A x = b` for a symmetric positive-definite `n × n` matrix `A`
/// (row-major) via Cholesky factorisation.
///
/// Returns `None` if a non-positive (or non-finite) pivot is encountered.
pub(crate) fn cholesky_solve(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if !(s > 0.0) || !s.is_finite() {
                    return None;
                }
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    // Forward substitution: L y = b.
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i * n + k] * y[k];
        }
        y[i] = s / l[i * n + i];
    }
    // Back substitution: Lᵀ x = y.
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in i + 1..n {
            s -= l[k * n + i] * x[k];
        }
        x[i] = s / l[i * n + i];
    }
    if x.iter().all(|v| v.is_finite()) {
        Some(x)
    } else {
        None
    }
}

/// Singular values of an `m × n` row-major matrix, in descending order.
///
/// Uses one-sided Jacobi rotations on the columns of whichever orientation
/// (`A` or `Aᵀ`) is at least as tall as it is wide, so that the number of
/// returned values is `min(m, n)`.
pub(crate) fn singular_values(a: &[f64], m: usize, n: usize) -> Vec<f64> {
    debug_assert_eq!(a.len(), m * n);
    if m == 0 || n == 0 {
        return Vec::new();
    }
    // Build column vectors of the tall orientation: `cols` columns of length `len`.
    let (len, ncols) = if m >= n { (m, n) } else { (n, m) };
    let mut cols: Vec<Vec<f64>> = (0..ncols)
        .map(|c| {
            (0..len)
                .map(|r| if m >= n { a[r * n + c] } else { a[c * n + r] })
                .collect()
        })
        .collect();

    let eps = f64::EPSILON;
    for _sweep in 0..100 {
        let mut rotated = false;
        for i in 0..ncols {
            for j in i + 1..ncols {
                let (alpha, beta, gamma) = {
                    let (ci, cj) = (&cols[i], &cols[j]);
                    let mut alpha = 0.0;
                    let mut beta = 0.0;
                    let mut gamma = 0.0;
                    for k in 0..len {
                        alpha += ci[k] * ci[k];
                        beta += cj[k] * cj[k];
                        gamma += ci[k] * cj[k];
                    }
                    (alpha, beta, gamma)
                };
                if alpha == 0.0 || beta == 0.0 {
                    continue;
                }
                if gamma.abs() <= eps * (alpha * beta).sqrt() {
                    continue;
                }
                rotated = true;
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let t = if zeta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                let (lo, hi) = cols.split_at_mut(j);
                let (ci, cj) = (&mut lo[i], &mut hi[0]);
                for k in 0..len {
                    let xi = ci[k];
                    let xj = cj[k];
                    ci[k] = c * xi - s * xj;
                    cj[k] = s * xi + c * xj;
                }
            }
        }
        if !rotated {
            break;
        }
    }
    let mut sv: Vec<f64> = cols
        .iter()
        .map(|c| c.iter().map(|v| v * v).sum::<f64>().sqrt())
        .collect();
    sv.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    sv
}

/// Numerical rank: the number of singular values greater than
/// `rel_tol · σ_max`. A matrix whose largest singular value is zero (or that
/// has no rows/columns) has rank 0.
pub(crate) fn rank(a: &[f64], m: usize, n: usize, rel_tol: f64) -> usize {
    let sv = singular_values(a, m, n);
    let smax = sv.first().copied().unwrap_or(0.0);
    if !(smax > 0.0) {
        return 0;
    }
    sv.iter().filter(|&&s| s > rel_tol * smax).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singular_values_of_diagonal() {
        let a = [3.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let sv = singular_values(&a, 2, 3);
        assert_eq!(sv.len(), 2);
        assert!((sv[0] - 3.0).abs() < 1e-14 && (sv[1] - 1.0).abs() < 1e-14);
    }

    #[test]
    fn singular_values_of_rotated_matrix() {
        // [[1,1],[1,-1]] / sqrt(2) * diag(2, 0.5)
        let r = std::f64::consts::FRAC_1_SQRT_2;
        let a = [2.0 * r, 0.5 * r, 2.0 * r, -0.5 * r, 0.0, 0.0];
        let sv = singular_values(&a, 3, 2);
        assert!((sv[0] - 2.0).abs() < 1e-14 && (sv[1] - 0.5).abs() < 1e-14, "{sv:?}");
    }

    #[test]
    fn rank_detects_dependent_rows() {
        let a = [1.0, 2.0, 3.0, 2.0, 4.0, 6.0, 0.0, 1.0, 0.0];
        assert_eq!(rank(&a, 3, 3, 1e-9), 2);
        assert_eq!(rank(&[0.0; 4], 2, 2, 1e-9), 0);
        assert_eq!(rank(&[], 0, 4, 1e-9), 0);
    }

    #[test]
    fn cholesky_solves_spd_system() {
        let a = [4.0, 1.0, 1.0, 3.0];
        let x = cholesky_solve(&a, &[1.0, 2.0], 2).unwrap();
        assert!((4.0 * x[0] + x[1] - 1.0).abs() < 1e-14);
        assert!((x[0] + 3.0 * x[1] - 2.0).abs() < 1e-14);
        assert!(cholesky_solve(&[0.0], &[1.0], 1).is_none());
    }
}
