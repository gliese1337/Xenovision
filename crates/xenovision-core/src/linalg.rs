//! Minimal dense NxN matrix/vector math - just enough for the perceptual
//! pipeline (§2.2). No external linalg crate: matrices here are always
//! small (N = receptor count, single digits in practice, soft-capped per
//! §7.2), so a plain Gauss-Jordan inverse is simple, fast enough, and easy
//! to audit.

#[derive(Debug, Clone, PartialEq)]
pub struct Mat {
    pub n: usize,
    data: Vec<f64>,
}

impl Mat {
    pub fn zeros(n: usize) -> Self {
        Mat {
            n,
            data: vec![0.0; n * n],
        }
    }

    pub fn identity(n: usize) -> Self {
        let mut m = Mat::zeros(n);
        for i in 0..n {
            m.set(i, i, 1.0);
        }
        m
    }

    /// Builds a matrix from `n` rows, each of length `n`.
    pub fn from_rows(rows: &[Vec<f64>]) -> Self {
        let n = rows.len();
        let mut m = Mat::zeros(n);
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(
                row.len(),
                n,
                "row {i} has wrong length for an {n}x{n} matrix"
            );
            for (j, &v) in row.iter().enumerate() {
                m.set(i, j, v);
            }
        }
        m
    }

    pub fn get(&self, i: usize, j: usize) -> f64 {
        self.data[i * self.n + j]
    }

    pub fn set(&mut self, i: usize, j: usize, v: f64) {
        self.data[i * self.n + j] = v;
    }

    pub fn row(&self, i: usize) -> &[f64] {
        &self.data[i * self.n..(i + 1) * self.n]
    }

    pub fn mul_vec(&self, v: &[f64]) -> Vec<f64> {
        assert_eq!(v.len(), self.n);
        (0..self.n).map(|i| dot(self.row(i), v)).collect()
    }

    pub fn mul_mat(&self, other: &Mat) -> Mat {
        assert_eq!(self.n, other.n);
        let n = self.n;
        let mut out = Mat::zeros(n);
        for i in 0..n {
            for j in 0..n {
                let mut s = 0.0;
                for k in 0..n {
                    s += self.get(i, k) * other.get(k, j);
                }
                out.set(i, j, s);
            }
        }
        out
    }

    /// Gauss-Jordan inverse with partial pivoting. `None` if singular
    /// (or numerically indistinguishable from singular).
    #[allow(clippy::needless_range_loop)]
    pub fn inverse(&self) -> Option<Mat> {
        let n = self.n;
        // Augmented [self | identity], worked on as plain row vectors.
        let mut aug: Vec<Vec<f64>> = (0..n)
            .map(|i| {
                let mut row = self.row(i).to_vec();
                row.resize(2 * n, 0.0);
                row[n + i] = 1.0;
                row
            })
            .collect();

        for col in 0..n {
            // Partial pivot: largest absolute value in this column, at or below the diagonal.
            let pivot_row = (col..n)
                .max_by(|&a, &b| aug[a][col].abs().partial_cmp(&aug[b][col].abs()).unwrap())?;
            if aug[pivot_row][col].abs() < 1e-12 {
                return None; // singular
            }
            aug.swap(col, pivot_row);

            let pivot = aug[col][col];
            for v in aug[col].iter_mut() {
                *v /= pivot;
            }

            for r in 0..n {
                if r == col {
                    continue;
                }
                let factor = aug[r][col];
                if factor == 0.0 {
                    continue;
                }
                for c in 0..2 * n {
                    aug[r][c] -= factor * aug[col][c];
                }
            }
        }

        let mut inv = Mat::zeros(n);
        for i in 0..n {
            for j in 0..n {
                inv.set(i, j, aug[i][n + j]);
            }
        }
        Some(inv)
    }
}

/// Eigendecomposition of a symmetric NxN matrix via the classical cyclic
/// Jacobi algorithm (repeatedly zeroing one off-diagonal pair with a
/// rotation, sweeping over every `i < j` pair, until the off-diagonal
/// mass is negligible). Converges quadratically and needs only a handful
/// of sweeps for the small N this app deals with (receptor counts are
/// single digits in practice, soft-capped per §7.2) - simple, auditable,
/// and avoids pulling in an external linalg crate, matching this
/// module's existing Gauss-Jordan-inverse philosophy.
///
/// Returns `(eigenvalues, eigenvectors)`, both sorted by descending
/// eigenvalue; `eigenvectors[k]` is the unit eigenvector for
/// `eigenvalues[k]`. Eigenvector sign is whatever the rotations happen to
/// produce (both `v` and `-v` are valid) - callers that need a
/// deterministic sign should canonicalize it themselves.
pub fn symmetric_eigen(m: &Mat) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = m.n;
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    let mut a = m.clone();
    let mut v = Mat::identity(n);
    const MAX_SWEEPS: usize = 100;

    for _ in 0..MAX_SWEEPS {
        let off_diag_sq: f64 = (0..n)
            .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
            .map(|(i, j)| a.get(i, j) * a.get(i, j))
            .sum();
        if off_diag_sq < 1e-20 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a.get(p, q);
                if apq.abs() < 1e-300 {
                    continue;
                }
                let app = a.get(p, p);
                let aqq = a.get(q, q);
                // Standard Jacobi rotation angle, via the numerically
                // stable half-angle form rather than atan(2*apq/(app-aqq)).
                let theta = (aqq - app) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;

                for k in 0..n {
                    let akp = a.get(k, p);
                    let akq = a.get(k, q);
                    a.set(k, p, c * akp - s * akq);
                    a.set(k, q, s * akp + c * akq);
                }
                for k in 0..n {
                    let apk = a.get(p, k);
                    let aqk = a.get(q, k);
                    a.set(p, k, c * apk - s * aqk);
                    a.set(q, k, s * apk + c * aqk);
                }
                for k in 0..n {
                    let vkp = v.get(k, p);
                    let vkq = v.get(k, q);
                    v.set(k, p, c * vkp - s * vkq);
                    v.set(k, q, s * vkp + c * vkq);
                }
            }
        }
    }

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| a.get(j, j).partial_cmp(&a.get(i, i)).unwrap());
    let eigenvalues = order.iter().map(|&i| a.get(i, i)).collect();
    let eigenvectors = order
        .iter()
        .map(|&i| (0..n).map(|k| v.get(k, i)).collect())
        .collect();
    (eigenvalues, eigenvectors)
}

pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

pub fn norm(a: &[f64]) -> f64 {
    dot(a, a).sqrt()
}

/// Decomposes an n-dimensional vector into its `n-1` hyperspherical
/// angles (the radius itself is just `norm(x)` - see `Coordinates::
/// saturation`, which already is that norm for the chroma vector this
/// is meant to be called on). Standard recursive n-sphere parametrization
/// (an n-dimensional point is one radius plus n-1 angles): all but the
/// last angle range over `[0, pi]`, the last ranges over `(-pi, pi]`.
///
/// `x.len() < 2` returns an empty vec - a single chroma dimension (a
/// dichromat's one chroma axis) has a sign but no angle to speak of; the
/// existing signed chroma value already carries that.
pub fn hyperspherical_angles(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    if n < 2 {
        return Vec::new();
    }
    let mut angles = Vec::with_capacity(n - 1);
    for k in 0..n - 2 {
        let tail_norm = norm(&x[k + 1..]);
        angles.push(tail_norm.atan2(x[k]));
    }
    angles.push(x[n - 1].atan2(x[n - 2]));
    angles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_inverts_to_itself() {
        let m = Mat::identity(3);
        let inv = m.inverse().unwrap();
        assert_eq!(inv, m);
    }

    #[test]
    fn inverse_of_simple_2x2() {
        // [[2,0],[0,4]] inverse is [[0.5,0],[0,0.25]]
        let m = Mat::from_rows(&[vec![2.0, 0.0], vec![0.0, 4.0]]);
        let inv = m.inverse().unwrap();
        assert!((inv.get(0, 0) - 0.5).abs() < 1e-9);
        assert!((inv.get(1, 1) - 0.25).abs() < 1e-9);
        assert!(inv.get(0, 1).abs() < 1e-9);
        assert!(inv.get(1, 0).abs() < 1e-9);
    }

    #[test]
    fn mat_times_inverse_is_identity() {
        let m = Mat::from_rows(&[
            vec![4.0, 3.0, 2.0],
            vec![1.0, 5.0, 2.0],
            vec![3.0, 1.0, 6.0],
        ]);
        let inv = m.inverse().unwrap();
        let product = m.mul_mat(&inv);
        let identity = Mat::identity(3);
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (product.get(i, j) - identity.get(i, j)).abs() < 1e-9,
                    "mismatch at ({i},{j}): {} vs {}",
                    product.get(i, j),
                    identity.get(i, j)
                );
            }
        }
    }

    #[test]
    fn singular_matrix_has_no_inverse() {
        let m = Mat::from_rows(&[vec![1.0, 2.0], vec![2.0, 4.0]]);
        assert_eq!(m.inverse(), None);
    }

    #[test]
    fn mul_vec_matches_hand_calculation() {
        let m = Mat::from_rows(&[vec![1.0, 2.0], vec![3.0, 4.0]]);
        let v = m.mul_vec(&[5.0, 6.0]);
        assert_eq!(v, vec![1.0 * 5.0 + 2.0 * 6.0, 3.0 * 5.0 + 4.0 * 6.0]);
    }

    #[test]
    fn symmetric_eigen_of_diagonal_matrix_is_itself() {
        let m = Mat::from_rows(&[
            vec![5.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 3.0],
        ]);
        let (vals, vecs) = symmetric_eigen(&m);
        assert_eq!(vals, vec![5.0, 3.0, 1.0]);
        // Each eigenvector should be a standard basis vector (up to sign).
        for v in &vecs {
            let nonzero = v.iter().filter(|x| x.abs() > 1e-9).count();
            assert_eq!(nonzero, 1, "{v:?} isn't a basis vector");
        }
    }

    #[test]
    fn symmetric_eigen_hand_computed_2x2() {
        // [[2,1],[1,2]] has eigenvalues 3 and 1, eigenvectors (1,1)/sqrt2
        // and (1,-1)/sqrt2 (up to sign/order).
        let m = Mat::from_rows(&[vec![2.0, 1.0], vec![1.0, 2.0]]);
        let (vals, _vecs) = symmetric_eigen(&m);
        assert!((vals[0] - 3.0).abs() < 1e-9);
        assert!((vals[1] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn symmetric_eigen_satisfies_mv_eq_lambda_v_and_is_orthonormal() {
        let cases = [
            Mat::from_rows(&[vec![4.0, 1.0, 0.0], vec![1.0, 3.0, 1.0], vec![0.0, 1.0, 2.0]]),
            Mat::from_rows(&[
                vec![2.0, -1.0, 0.5, 0.0],
                vec![-1.0, 3.0, 0.2, 0.1],
                vec![0.5, 0.2, 1.5, -0.3],
                vec![0.0, 0.1, -0.3, 4.0],
            ]),
        ];
        for m in cases {
            let n = m.n;
            let (vals, vecs) = symmetric_eigen(&m);
            for k in 0..n {
                let mv = m.mul_vec(&vecs[k]);
                for i in 0..n {
                    assert!(
                        (mv[i] - vals[k] * vecs[k][i]).abs() < 1e-7,
                        "M*v != lambda*v at eigenvector {k}, component {i}"
                    );
                }
                assert!((norm(&vecs[k]) - 1.0).abs() < 1e-9, "eigenvector not unit norm");
            }
            for i in 0..n {
                for j in (i + 1)..n {
                    assert!(
                        dot(&vecs[i], &vecs[j]).abs() < 1e-7,
                        "eigenvectors {i},{j} not orthogonal"
                    );
                }
            }
        }
    }

    #[test]
    fn symmetric_eigen_n_zero_and_n_one() {
        let (vals, vecs) = symmetric_eigen(&Mat::zeros(0));
        assert!(vals.is_empty() && vecs.is_empty());
        let (vals, vecs) = symmetric_eigen(&Mat::from_rows(&[vec![7.0]]));
        assert_eq!(vals, vec![7.0]);
        assert_eq!(vecs.len(), 1);
        assert!((vecs[0][0].abs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn hyperspherical_angles_empty_below_2d() {
        assert_eq!(hyperspherical_angles(&[]), Vec::<f64>::new());
        assert_eq!(hyperspherical_angles(&[5.0]), Vec::<f64>::new());
    }

    #[test]
    fn hyperspherical_angles_2d_matches_hand_calculation() {
        // (1,1): standard polar angle atan2(1,1) = pi/4.
        let angles = hyperspherical_angles(&[1.0, 1.0]);
        assert_eq!(angles.len(), 1);
        assert!((angles[0] - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
    }

    /// Reconstructs Cartesian coordinates from a radius + hyperspherical
    /// angles via the standard forward formula, as an independent check
    /// on `hyperspherical_angles` that doesn't require hand-deriving
    /// every atan2 value for higher dimensions.
    fn reconstruct(r: f64, angles: &[f64]) -> Vec<f64> {
        let n = angles.len() + 1;
        let mut x = vec![0.0; n];
        let mut sin_prod = 1.0;
        for i in 0..n {
            if i < angles.len() {
                x[i] = r * sin_prod * angles[i].cos();
                sin_prod *= angles[i].sin();
            } else {
                x[i] = r * sin_prod;
            }
        }
        x
    }

    #[test]
    fn hyperspherical_angles_round_trip_3d_and_4d() {
        for x in [
            vec![0.6, -0.3, 0.9],
            vec![1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![0.4, -0.2, 0.1, 0.8],
        ] {
            let r = norm(&x);
            let angles = hyperspherical_angles(&x);
            assert_eq!(angles.len(), x.len() - 1);
            let recon = reconstruct(r, &angles);
            for (a, b) in x.iter().zip(recon.iter()) {
                assert!((a - b).abs() < 1e-9, "x={x:?} recon={recon:?}");
            }
        }
    }
}
