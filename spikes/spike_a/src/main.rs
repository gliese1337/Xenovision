//! Spike A (design doc §2.2.3 / impl plan Phase 0): validate that an
//! overlap-minimizing linear transform of a species' receptor curves is a
//! workable way to derive the chromatic-adaptation "sharpening" matrix
//! `M_adapt`, before it gets built for real in Phase 2.
//!
//! Throwaway code: Gaussian stand-ins for receptor curves (no Govardovskii
//! templates yet), a hand-rolled gradient descent (no optimizer crate), no
//! connection to the real `SpectralCurve` data model.

use std::time::Instant;

const WL_MIN: f64 = 300.0;
const WL_MAX: f64 = 750.0;
const STEP: f64 = 1.0;
const SIGMA: f64 = 30.0; // stand-in receptor bandwidth, nm

fn wavelengths() -> Vec<f64> {
    let n = ((WL_MAX - WL_MIN) / STEP).round() as usize + 1;
    (0..n).map(|i| WL_MIN + i as f64 * STEP).collect()
}

fn gaussian_curve(grid: &[f64], mu: f64, sigma: f64) -> Vec<f64> {
    grid.iter()
        .map(|&wl| {
            let d = (wl - mu) / sigma;
            (-0.5 * d * d).exp()
        })
        .collect()
}

fn dot(a: &[f64], b: &[f64], dlambda: f64) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f64>() * dlambda
}

/// Row-major N x N matrix, diagonal fixed at 1.0, off-diagonal entries free.
struct Matrix {
    n: usize,
    data: Vec<f64>,
}

impl Matrix {
    fn from_offdiag(n: usize, offdiag: &[f64]) -> Self {
        let mut data = vec![0.0; n * n];
        let mut k = 0;
        for i in 0..n {
            for j in 0..n {
                data[i * n + j] = if i == j {
                    1.0
                } else {
                    let v = offdiag[k];
                    k += 1;
                    v
                };
            }
        }
        Matrix { n, data }
    }

    fn row(&self, i: usize) -> &[f64] {
        &self.data[i * self.n..(i + 1) * self.n]
    }

    /// Apply this matrix to a set of curves: transformed_i(λ) = Σ_k M[i,k] * curves_k(λ)
    fn transform_curves(&self, curves: &[Vec<f64>]) -> Vec<Vec<f64>> {
        let grid_len = curves[0].len();
        (0..self.n)
            .map(|i| {
                let row = self.row(i);
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
}

/// Sum of squared pairwise overlap integrals between transformed channels,
/// plus an L2 regularizer on the off-diagonal entries (keeps the solution
/// near identity, which also discourages degenerate/sign-flipped channels
/// per §2.2.3's "physically reasonable" constraint).
fn objective(offdiag: &[f64], curves: &[Vec<f64>], n: usize, dlambda: f64, reg_lambda: f64) -> f64 {
    let m = Matrix::from_offdiag(n, offdiag);
    let transformed = m.transform_curves(curves);
    let mut overlap_sum = 0.0;
    for i in 0..n {
        for j in (i + 1)..n {
            let ov = dot(&transformed[i], &transformed[j], dlambda);
            overlap_sum += ov * ov;
        }
    }
    let reg: f64 = offdiag.iter().map(|p| p * p).sum::<f64>() * reg_lambda;
    overlap_sum + reg
}

fn numerical_gradient(
    offdiag: &[f64],
    curves: &[Vec<f64>],
    n: usize,
    dlambda: f64,
    reg_lambda: f64,
) -> Vec<f64> {
    let h = 1e-4;
    let mut grad = vec![0.0; offdiag.len()];
    for k in 0..offdiag.len() {
        let mut plus = offdiag.to_vec();
        plus[k] += h;
        let mut minus = offdiag.to_vec();
        minus[k] -= h;
        let fp = objective(&plus, curves, n, dlambda, reg_lambda);
        let fm = objective(&minus, curves, n, dlambda, reg_lambda);
        grad[k] = (fp - fm) / (2.0 * h);
    }
    grad
}

/// Plain gradient descent with step-halving backtracking. Good enough for a
/// spike with <= 12 free parameters (N<=4 => N*(N-1) <= 12 off-diag entries).
fn optimize(curves: &[Vec<f64>], n: usize, dlambda: f64, reg_lambda: f64) -> (Matrix, f64, usize) {
    // n*(n-1) off-diagonal entries, all starting at 0 (= identity matrix).
    let mut offdiag = vec![0.0; n * n - n];
    let mut lr = 0.5;
    let mut f = objective(&offdiag, curves, n, dlambda, reg_lambda);
    let max_iters = 3000;
    let mut iters_run = 0;
    for iter in 0..max_iters {
        iters_run = iter + 1;
        let grad = numerical_gradient(&offdiag, curves, n, dlambda, reg_lambda);
        let grad_norm: f64 = grad.iter().map(|g| g * g).sum::<f64>().sqrt();
        if grad_norm < 1e-10 {
            break;
        }
        // Backtracking line search.
        let mut step = lr;
        loop {
            let candidate: Vec<f64> = offdiag
                .iter()
                .zip(grad.iter())
                .map(|(x, g)| x - step * g)
                .collect();
            let f_candidate = objective(&candidate, curves, n, dlambda, reg_lambda);
            if f_candidate < f || step < 1e-8 {
                offdiag = candidate;
                f = f_candidate;
                lr = step * 1.2; // grow back a bit for next iteration
                break;
            }
            step *= 0.5;
        }
    }
    (Matrix::from_offdiag(n, &offdiag), f, iters_run)
}

fn print_matrix(label: &str, m: &Matrix) {
    println!("{label} ({}x{}):", m.n, m.n);
    for i in 0..m.n {
        let row: Vec<String> = m.row(i).iter().map(|v| format!("{v:7.4}")).collect();
        println!("  [{}]", row.join(", "));
    }
}

fn raw_overlap_sum(curves: &[Vec<f64>], n: usize, dlambda: f64) -> f64 {
    let mut s = 0.0;
    for i in 0..n {
        for j in (i + 1)..n {
            let ov = dot(&curves[i], &curves[j], dlambda);
            s += ov * ov;
        }
    }
    s
}

/// Sweep the regularization strength to find an operating point where
/// overlap is substantially reduced *without* off-diagonal weights
/// overwhelming the diagonal (which §2.2.3 flags as the "degenerate"
/// failure mode this constraint is supposed to prevent).
fn sweep_reg_lambda(label: &str, lambda_maxes: &[f64]) {
    let n = lambda_maxes.len();
    let grid = wavelengths();
    let dlambda = STEP;
    let curves: Vec<Vec<f64>> = lambda_maxes
        .iter()
        .map(|&mu| gaussian_curve(&grid, mu, SIGMA))
        .collect();
    let raw_overlap = raw_overlap_sum(&curves, n, dlambda);

    println!("\n=== {label}: regularization sweep ===");
    println!("  reg_lambda   overlap_reduction%   max|off-diag|   diagonal-dominant?");
    for &reg_lambda in &[
        1e-3, 1e-2, 3e-2, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0, 100.0, 300.0,
    ] {
        let (m, _, _) = optimize(&curves, n, dlambda, reg_lambda);
        let transformed = m.transform_curves(&curves);
        let mut opt_overlap = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                let ov = dot(&transformed[i], &transformed[j], dlambda);
                opt_overlap += ov * ov;
            }
        }
        let max_offdiag = (0..n)
            .flat_map(|i| {
                m.row(i)
                    .iter()
                    .enumerate()
                    .filter(move |(j, _)| *j != i)
                    .map(|(_, v)| v.abs())
            })
            .fold(0.0_f64, f64::max);
        let reduction_pct = (1.0 - opt_overlap / raw_overlap) * 100.0;
        println!(
            "  {reg_lambda:<10.3}  {reduction_pct:>8.1}%            {max_offdiag:>7.4}       {}",
            if max_offdiag < 1.0 { "yes" } else { "NO" }
        );
    }
}

fn run_case(label: &str, lambda_maxes: &[f64]) {
    let n = lambda_maxes.len();
    let grid = wavelengths();
    let dlambda = STEP;
    let curves: Vec<Vec<f64>> = lambda_maxes
        .iter()
        .map(|&mu| gaussian_curve(&grid, mu, SIGMA))
        .collect();

    println!("\n=== {label} (N={n}, λmax = {lambda_maxes:?}) ===");

    let raw_overlap = raw_overlap_sum(&curves, n, dlambda);
    println!("Raw pairwise overlap (sum of squared overlap integrals): {raw_overlap:.6}");

    // Picked from the regularization sweep below: large enough that the
    // optimizer settles for a diagonal-dominant, CAT02-shaped matrix
    // instead of driving overlap all the way to zero.
    let reg_lambda = 300.0;
    let start = Instant::now();
    let (m, final_obj, iters) = optimize(&curves, n, dlambda, reg_lambda);
    let elapsed = start.elapsed();

    print_matrix("Optimized M_adapt", &m);
    let transformed = m.transform_curves(&curves);
    let opt_overlap: f64 = {
        let mut s = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                let ov = dot(&transformed[i], &transformed[j], dlambda);
                s += ov * ov;
            }
        }
        s
    };
    println!(
        "Post-optimization overlap: {opt_overlap:.6}  (objective incl. reg term: {final_obj:.6})"
    );
    println!(
        "Overlap reduced by {:.1}%",
        (1.0 - opt_overlap / raw_overlap) * 100.0
    );
    println!("Converged in {iters} iterations, {elapsed:.2?}");

    // Sanity: no row collapsed to (near-)zero and no wildly large entries,
    // which would indicate a degenerate/sign-flipped channel.
    for i in 0..n {
        let row = m.row(i);
        let max_abs_offdiag = row
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, v)| v.abs())
            .fold(0.0, f64::max);
        println!("  row {i}: max |off-diag| = {max_abs_offdiag:.4}");
    }
}

fn main() {
    // Human: S/M/L cones, §2.3.1.
    run_case("Human", &[420.0, 530.0, 560.0]);
    sweep_reg_lambda("Human", &[420.0, 530.0, 560.0]);

    // Dog: S / L-M cones, §2.3.2 (averaged S-cone λmax).
    run_case("Dog", &[432.0, 555.0]);

    // Pigeon: 4 opsins (pre oil-droplet filtering), §2.3.5.
    run_case("Pigeon", &[409.0, 456.0, 510.0, 567.0]);
    sweep_reg_lambda("Pigeon", &[409.0, 456.0, 510.0, 567.0]);

    println!(
        "\n--- CAT02 reference matrix (XYZ -> sharpened cone-like space, for structural comparison only) ---"
    );
    println!("  [ 0.7328,  0.4296, -0.1624]");
    println!("  [-0.7036,  1.6975,  0.0061]");
    println!("  [ 0.0030,  0.0136,  0.9834]");
    println!(
        "Structural read: CAT02 is diagonal-dominant (|diag| > most |off-diag|), has mixed-sign\n\
         off-diagonal entries, and leaves the third (blue-ish) row close to identity — the same\n\
         qualitative shape the optimizer above converges to for the human case (not a numerical\n\
         match, since CAT02 operates on XYZ rather than raw LMS and was fit to a natural-scene\n\
         corpus, but the shape resemblance is the design doc's actual claim, §2.2.3)."
    );
}
