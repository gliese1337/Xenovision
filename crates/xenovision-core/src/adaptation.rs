//! Chromatic adaptation matrix derivation (design doc §2.2.3), operating
//! on actual `SpectralCurve` data. A *fixed* regularization strength
//! produces degenerate (off-diagonal-dominated) matrices for highly-
//! correlated receptors, so the regularization strength is searched per
//! Curve Set instead of hardcoded.

use crate::curve::SpectralCurve;
use crate::linalg::{dot, Mat};

/// Samples `curves` onto a common grid (the union of their domains, at
/// `step_nm` spacing), treating each curve as zero outside its own domain,
/// consistent with how `pipeline::integrate_product` treats curves.
/// `pub(crate)` so `natural_opponent`'s parametric model can reuse it
/// rather than duplicating grid-sampling logic.
pub(crate) fn sample_curves_on_common_grid(curves: &[SpectralCurve], step_nm: f64) -> Vec<Vec<f64>> {
    let (lo, hi) = curves
        .iter()
        .filter_map(|c| c.domain())
        .fold(None, |acc: Option<(f64, f64)>, (a, b)| match acc {
            None => Some((a, b)),
            Some((lo, hi)) => Some((lo.min(a), hi.max(b))),
        })
        .unwrap_or((0.0, 0.0));
    let n = ((hi - lo) / step_nm).round().max(1.0) as usize;
    curves
        .iter()
        .map(|c| {
            let interp = c.interpolant();
            (0..=n)
                .map(|i| interp.value_at(lo + i as f64 * step_nm).unwrap_or(0.0))
                .collect()
        })
        .collect()
}

fn dot_scaled(a: &[f64], b: &[f64], step_nm: f64) -> f64 {
    dot(a, b) * step_nm
}

/// Transform `curves` (sampled vectors) by matrix `m`: `transformed_i(λ) =
/// Σ_k m[i,k] * curves_k(λ)`.
#[cfg(test)]
fn transform_curves(m: &Mat, curves: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = m.n;
    let grid_len = curves[0].len();
    (0..n)
        .map(|i| {
            let row = m.row(i);
            let mut out = vec![0.0; grid_len];
            for (k, curve) in curves.iter().enumerate() {
                let w = row[k];
                if w == 0.0 {
                    continue;
                }
                for (o, c) in out.iter_mut().zip(curve.iter()) {
                    *o += w * c;
                }
            }
            out
        })
        .collect()
}

fn mat_from_offdiag(n: usize, offdiag: &[f64]) -> Mat {
    let mut m = Mat::zeros(n);
    let mut k = 0;
    for i in 0..n {
        for j in 0..n {
            if i == j {
                m.set(i, j, 1.0);
            } else {
                m.set(i, j, offdiag[k]);
                k += 1;
            }
        }
    }
    m
}

/// Reference objective on the sampled curves directly - kept for tests,
/// which check the Gram-matrix version against it.
#[cfg(test)]
fn objective(offdiag: &[f64], curves: &[Vec<f64>], n: usize, step_nm: f64, reg_lambda: f64) -> f64 {
    let m = mat_from_offdiag(n, offdiag);
    let transformed = transform_curves(&m, curves);
    let mut overlap_sum = 0.0;
    for i in 0..n {
        for j in (i + 1)..n {
            let ov = dot_scaled(&transformed[i], &transformed[j], step_nm);
            overlap_sum += ov * ov;
        }
    }
    let reg: f64 = offdiag.iter().map(|p| p * p).sum::<f64>() * reg_lambda;
    overlap_sum + reg
}

/// The curves' pairwise overlap (Gram) matrix, `G[k][l] = step * <c_k, c_l>`.
/// Overlaps of transformed curves are then `O = M G Mᵀ`, so the objective
/// never needs the sampled curves again - O(n³) per evaluation instead of
/// O(n² · grid length).
fn gram_matrix(curves: &[Vec<f64>], step_nm: f64) -> Mat {
    let n = curves.len();
    let mut g = Mat::zeros(n);
    for k in 0..n {
        for l in k..n {
            let v = dot_scaled(&curves[k], &curves[l], step_nm);
            g.set(k, l, v);
            g.set(l, k, v);
        }
    }
    g
}

fn transpose(m: &Mat) -> Mat {
    let mut t = Mat::zeros(m.n);
    for i in 0..m.n {
        for j in 0..m.n {
            t.set(i, j, m.get(j, i));
        }
    }
    t
}

/// `objective()` computed from the Gram matrix, plus its exact gradient.
/// With `f = Σ_{i<j} O_ij² + λ Σ m_ab²` and `O = M G Mᵀ` (symmetric),
/// `∂f/∂m_ab = 2 Σ_{j≠a} O_aj (MG)_jb + 2λ m_ab` for each off-diagonal
/// `(a, b)` - replacing the `2·(n²−n)` extra objective evaluations a
/// central finite difference needs per step.
fn objective_and_gradient_gram(offdiag: &[f64], gram: &Mat, reg_lambda: f64) -> (f64, Vec<f64>) {
    let n = gram.n;
    let m = mat_from_offdiag(n, offdiag);
    let mg = m.mul_mat(gram);
    let o = mg.mul_mat(&transpose(&m));
    let mut f = 0.0;
    for i in 0..n {
        for j in (i + 1)..n {
            f += o.get(i, j) * o.get(i, j);
        }
    }
    f += offdiag.iter().map(|p| p * p).sum::<f64>() * reg_lambda;

    let mut grad = Vec::with_capacity(offdiag.len());
    for a in 0..n {
        for b in 0..n {
            if a == b {
                continue;
            }
            let mut g = 0.0;
            for j in 0..n {
                if j != a {
                    g += o.get(a, j) * mg.get(j, b);
                }
            }
            grad.push(2.0 * g + 2.0 * reg_lambda * m.get(a, b));
        }
    }
    (f, grad)
}

fn objective_gram(offdiag: &[f64], gram: &Mat, reg_lambda: f64) -> f64 {
    let n = gram.n;
    let m = mat_from_offdiag(n, offdiag);
    let o = m.mul_mat(gram).mul_mat(&transpose(&m));
    let mut f = 0.0;
    for i in 0..n {
        for j in (i + 1)..n {
            f += o.get(i, j) * o.get(i, j);
        }
    }
    f + offdiag.iter().map(|p| p * p).sum::<f64>() * reg_lambda
}

#[cfg(test)]
fn numerical_gradient(
    offdiag: &[f64],
    curves: &[Vec<f64>],
    n: usize,
    step_nm: f64,
    reg_lambda: f64,
) -> Vec<f64> {
    let h = 1e-4;
    let mut grad = vec![0.0; offdiag.len()];
    for k in 0..offdiag.len() {
        let mut plus = offdiag.to_vec();
        plus[k] += h;
        let mut minus = offdiag.to_vec();
        minus[k] -= h;
        let fp = objective(&plus, curves, n, step_nm, reg_lambda);
        let fm = objective(&minus, curves, n, step_nm, reg_lambda);
        grad[k] = (fp - fm) / (2.0 * h);
    }
    grad
}

fn optimize(gram: &Mat, n: usize, reg_lambda: f64, max_iters: usize) -> Mat {
    let mut offdiag = vec![0.0; n * n - n];
    if offdiag.is_empty() {
        return Mat::identity(n);
    }
    let mut lr = 0.5;
    for _ in 0..max_iters {
        let (f, grad) = objective_and_gradient_gram(&offdiag, gram, reg_lambda);
        let grad_norm: f64 = grad.iter().map(|g| g * g).sum::<f64>().sqrt();
        if grad_norm < 1e-10 {
            break;
        }
        let mut step = lr;
        loop {
            let candidate: Vec<f64> = offdiag
                .iter()
                .zip(grad.iter())
                .map(|(x, g)| x - step * g)
                .collect();
            let f_candidate = objective_gram(&candidate, gram, reg_lambda);
            if f_candidate < f || step < 1e-8 {
                offdiag = candidate;
                lr = step * 1.2;
                break;
            }
            step *= 0.5;
        }
    }
    mat_from_offdiag(n, &offdiag)
}

fn max_abs_offdiag(m: &Mat) -> f64 {
    (0..m.n)
        .flat_map(|i| {
            m.row(i)
                .iter()
                .enumerate()
                .filter(move |(j, _)| *j != i)
                .map(|(_, v)| v.abs())
        })
        .fold(0.0_f64, f64::max)
}

#[derive(Debug, Clone)]
pub struct AdaptationResult {
    pub matrix: Mat,
    pub inverse: Mat,
    /// The regularization strength that was used.
    pub lambda_used: f64,
    /// Whether `matrix` came out diagonal-dominant (every off-diagonal
    /// entry smaller in magnitude than the diagonal's 1.0) - the
    /// structural property necessary to avoid degenerate channels.
    /// `false` means no `lambda` in the search range achieved
    /// it; `matrix` is still the best (smallest max-off-diagonal) found,
    /// but callers may want to warn the user (e.g. near-duplicate
    /// receptor curves can make this unreachable).
    pub diagonal_dominant: bool,
}

/// Derives the adaptation ("sharpening") matrix for a set of receptor
/// curves by minimizing pairwise spectral overlap between the
/// transformed basis (§2.2.3), searching for the smallest regularization
/// strength that yields a diagonal-dominant result: a fixed strength
/// either under-regularizes into degenerate matrices for highly-
/// correlated receptors, or over-regularizes for well-separated ones.
pub fn derive_adaptation_matrix(curves: &[SpectralCurve], step_nm: f64) -> AdaptationResult {
    let n = curves.len();

    // N=0 (§10.2's Mantis Shrimp fixture: an entirely-isolated-curves
    // species, with zero curves in the colorspace set Pipeline operates
    // on) - a trivial 0x0 identity, same reasoning as N=1 below: there's
    // nothing to decorrelate, so this is diagonal-dominant by vacuous
    // truth rather than by having actually searched for a regularization
    // strength.
    if n == 0 {
        let m = Mat::identity(0);
        return AdaptationResult {
            inverse: m.inverse().unwrap(),
            matrix: m,
            lambda_used: 0.0,
            diagonal_dominant: true,
        };
    }

    let sampled = sample_curves_on_common_grid(curves, step_nm);
    let max_iters = 3000;

    if n == 1 {
        let m = Mat::identity(1);
        return AdaptationResult {
            inverse: m.inverse().unwrap(),
            matrix: m,
            lambda_used: 0.0,
            diagonal_dominant: true,
        };
    }

    let lambdas = [
        1e-3, 3e-3, 1e-2, 3e-2, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0, 100.0, 300.0, 1000.0, 3000.0,
        10000.0,
    ];
    let mut best_matrix = Mat::identity(n);
    let mut best_max_offdiag = f64::MAX;
    let mut best_lambda = 0.0;

    let gram = gram_matrix(&sampled, step_nm);
    for &lambda in &lambdas {
        let m = optimize(&gram, n, lambda, max_iters);
        let max_off = max_abs_offdiag(&m);
        if max_off < best_max_offdiag {
            best_max_offdiag = max_off;
            best_matrix = m.clone();
            best_lambda = lambda;
        }
        if max_off < 1.0 {
            // Diagonal-dominant: good enough, no need to regularize harder
            // (which would just throw away more of the overlap reduction).
            return AdaptationResult {
                inverse: m.inverse().unwrap_or_else(|| Mat::identity(n)),
                matrix: m,
                lambda_used: lambda,
                diagonal_dominant: true,
            };
        }
    }

    AdaptationResult {
        inverse: best_matrix.inverse().unwrap_or_else(|| Mat::identity(n)),
        matrix: best_matrix,
        lambda_used: best_lambda,
        diagonal_dominant: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveType;
    use crate::govardovskii;

    fn cone_curve(name: &str, lambda_max: f64) -> SpectralCurve {
        SpectralCurve::new(name, CurveType::Sensitivity)
            .with_points(govardovskii::generate_points(lambda_max, 300.0, 750.0, 5.0))
    }

    #[test]
    fn human_cones_reach_diagonal_dominance() {
        let curves = vec![
            cone_curve("S-cone", 420.0),
            cone_curve("M-cone", 530.0),
            cone_curve("L-cone", 560.0),
        ];
        let result = derive_adaptation_matrix(&curves, 1.0);
        assert!(
            result.diagonal_dominant,
            "max off-diag = {}",
            max_abs_offdiag(&result.matrix)
        );
        // The two closely-spaced receptors (M, L) need a non-trivial
        // cross-term; the well-separated one (S) stays close to
        // independent.
        assert!(
            result.matrix.get(0, 1).abs() < 0.3,
            "S-M cross-term too large"
        );
        assert!(
            result.matrix.get(1, 2).abs() > 0.05,
            "M-L cross-term suspiciously small"
        );
    }

    #[test]
    fn well_separated_dog_receptors_need_little_correction() {
        let curves = vec![cone_curve("S-cone", 432.0), cone_curve("L/M-cone", 555.0)];
        let result = derive_adaptation_matrix(&curves, 1.0);
        assert!(result.diagonal_dominant);
        // Govardovskii curves have longer tails than idealized
        // Gaussians, so even "well separated" receptors leave some
        // residual overlap - still comfortably diagonal-dominant, just
        // not negligible.
        assert!(max_abs_offdiag(&result.matrix) < 0.25);
    }

    #[test]
    fn single_receptor_is_identity() {
        let curves = vec![cone_curve("only", 500.0)];
        let result = derive_adaptation_matrix(&curves, 1.0);
        assert_eq!(result.matrix, Mat::identity(1));
        assert!(result.diagonal_dominant);
    }

    /// A large custom system (N=24), well past where the §7.2 soft cap
    /// starts warning - fast enough via the CPU exact-gradient method
    /// (see `gpu.rs`'s doc comment for the now-unused GPU path's own
    /// per-dispatch size limit) to run by default rather than `#[ignore]`.
    #[test]
    fn large_n_system_derives_a_valid_matrix() {
        let n = 24;
        let curves: Vec<SpectralCurve> = (0..n)
            .map(|i| {
                let lmax = 350.0 + i as f64 * (300.0 / (n as f64 - 1.0));
                cone_curve(&format!("R{i}"), lmax)
            })
            .collect();
        let result = derive_adaptation_matrix(&curves, 1.0);
        assert_eq!(result.matrix.n, n);
        // The parametrization pins the diagonal at exactly 1.0.
        for i in 0..n {
            assert_eq!(result.matrix.get(i, i), 1.0);
        }
        let product = result.matrix.mul_mat(&result.inverse);
        for i in 0..n {
            for j in 0..n {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((product.get(i, j) - expected).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn matrix_and_inverse_multiply_to_identity() {
        let curves = vec![
            cone_curve("S-cone", 420.0),
            cone_curve("M-cone", 530.0),
            cone_curve("L-cone", 560.0),
        ];
        let result = derive_adaptation_matrix(&curves, 1.0);
        let product = result.matrix.mul_mat(&result.inverse);
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((product.get(i, j) - expected).abs() < 1e-6);
            }
        }
    }

    /// Not run by default (`cargo test -- --ignored --nocapture` to
    /// reproduce) - measured runtimes informing §7.2's soft-cap warning
    /// threshold, against this implementation's actual lambda-schedule-
    /// searching optimizer. Receptors evenly spaced across a 300nm
    /// range (reasonably realistic, well-separated - the worst case of
    /// closely-spaced/highly-correlated receptors needing several
    /// lambda attempts instead of one would be slower still):
    ///
    /// | N  | measured   |
    /// |----|------------|
    /// | 3  | ~2ms       |
    /// | 5  | ~4ms       |
    /// | 8  | ~15ms      |
    /// | 12 | ~34ms      |
    /// | 16 | ~82ms      |
    /// | 24 | ~269ms     |
    ///
    /// (With the Gram-matrix objective and exact gradient. The earlier
    /// finite-difference version took ~3s at N=8 and ~16s at N=12.)
    /// `Pipeline::build` calls this synchronously on the UI thread, so
    /// Workspace warns about the pause for very large systems.
    /// The Gram-matrix objective must equal the original sampled-curve
    /// objective, and its exact gradient must match central finite
    /// differences, at arbitrary (non-zero) matrix entries.
    #[test]
    fn gram_objective_and_exact_gradient_match_reference() {
        let curves: Vec<SpectralCurve> = [420.0, 500.0, 530.0, 560.0]
            .iter()
            .map(|&l| cone_curve("c", l))
            .collect();
        let n = curves.len();
        let sampled = sample_curves_on_common_grid(&curves, 1.0);
        let gram = gram_matrix(&sampled, 1.0);
        let offdiag: Vec<f64> = (0..n * n - n)
            .map(|k| ((k as f64) * 0.37).sin() * 0.4)
            .collect();
        let lambda = 0.3;

        let reference = objective(&offdiag, &sampled, n, 1.0, lambda);
        let (f, grad) = objective_and_gradient_gram(&offdiag, &gram, lambda);
        assert!(
            (f - reference).abs() <= 1e-9 * reference.abs().max(1.0),
            "gram objective {f} != reference {reference}"
        );
        assert!((objective_gram(&offdiag, &gram, lambda) - f).abs() < 1e-12);

        let numeric = numerical_gradient(&offdiag, &sampled, n, 1.0, lambda);
        for (k, (a, b)) in grad.iter().zip(&numeric).enumerate() {
            assert!(
                (a - b).abs() <= 1e-4 * b.abs().max(1.0),
                "grad[{k}]: exact {a} vs finite-difference {b}"
            );
        }
    }

    #[test]
    #[ignore]
    fn bench_realistic_n() {
        for n in [3usize, 5, 8, 10, 12, 16, 24] {
            let curves: Vec<SpectralCurve> = (0..n)
                .map(|i| {
                    let lmax = 350.0 + i as f64 * (300.0 / (n as f64 - 1.0).max(1.0));
                    cone_curve(&format!("R{i}"), lmax)
                })
                .collect();
            let start = std::time::Instant::now();
            let result = derive_adaptation_matrix(&curves, 1.0);
            let elapsed = start.elapsed();
            println!(
                "N={n}: {:?} diagonal_dominant={} lambda={}",
                elapsed, result.diagonal_dominant, result.lambda_used
            );
        }
    }
}
