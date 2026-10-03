//! Monotonic cubic (Fritsch-Carlson) interpolation over arbitrary,
//! irregularly-spaced (x, y) points. Design doc §1.2.1.

/// A set of (x, y) points, sorted by `x` and de-duplicated (exact-duplicate
/// `x` values collapse to the first occurrence), ready for monotonic cubic
/// interpolation.
#[derive(Debug, Clone, PartialEq)]
pub struct MonotonicCubic {
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// Hermite tangents, one per point.
    tangents: Vec<f64>,
}

impl MonotonicCubic {
    /// Builds the interpolant from arbitrary (possibly unsorted, possibly
    /// containing duplicate x values) points.
    pub fn new(points: &[(f64, f64)]) -> Self {
        let mut sorted: Vec<(f64, f64)> = points.to_vec();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut xs: Vec<f64> = Vec::with_capacity(sorted.len());
        let mut ys: Vec<f64> = Vec::with_capacity(sorted.len());
        for (x, y) in sorted {
            if let Some(&last_x) = xs.last() {
                if (x - last_x).abs() < f64::EPSILON {
                    // Exact-duplicate wavelength: keep the first occurrence,
                    // drop this one, rather than dividing by a zero-length
                    // interval later.
                    continue;
                }
            }
            xs.push(x);
            ys.push(y);
        }
        let tangents = fritsch_carlson_tangents(&xs, &ys);
        MonotonicCubic { xs, ys, tangents }
    }

    /// The curve's domain, i.e. (min_x, max_x), or `None` if there are no
    /// points at all.
    pub fn domain(&self) -> Option<(f64, f64)> {
        match (self.xs.first(), self.xs.last()) {
            (Some(&a), Some(&b)) => Some((a, b)),
            _ => None,
        }
    }

    /// Interpolated value at `x`. Returns `None` if `x` falls outside the
    /// curve's domain (no extrapolation policy is imposed here; callers
    /// that need extrapolation semantics - e.g. "treat as zero outside
    /// support" - decide that themselves).
    pub fn value_at(&self, x: f64) -> Option<f64> {
        match self.xs.len() {
            0 => None,
            1 => {
                if (x - self.xs[0]).abs() < f64::EPSILON {
                    Some(self.ys[0])
                } else {
                    None
                }
            }
            _ => {
                let (lo, hi) = self.domain().unwrap();
                if x < lo || x > hi {
                    return None;
                }
                // Find the interval [xs[i], xs[i+1]] containing x via binary search.
                let i = match self
                    .xs
                    .binary_search_by(|probe| probe.partial_cmp(&x).unwrap())
                {
                    Ok(exact) => {
                        return Some(self.ys[exact]);
                    }
                    Err(insert_pos) => insert_pos.saturating_sub(1).min(self.xs.len() - 2),
                };
                Some(hermite_eval(
                    self.xs[i],
                    self.xs[i + 1],
                    self.ys[i],
                    self.ys[i + 1],
                    self.tangents[i],
                    self.tangents[i + 1],
                    x,
                ))
            }
        }
    }
}

fn hermite_eval(x0: f64, x1: f64, y0: f64, y1: f64, m0: f64, m1: f64, x: f64) -> f64 {
    let h = x1 - x0;
    let t = (x - x0) / h;
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    h00 * y0 + h10 * h * m0 + h01 * y1 + h11 * h * m1
}

/// Fritsch-Carlson tangent estimation + monotonicity-preserving rescaling.
/// `xs` must be strictly increasing.
fn fritsch_carlson_tangents(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = xs.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0.0];
    }

    // Secant slopes between consecutive points.
    let secants: Vec<f64> = (0..n - 1)
        .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
        .collect();

    // Initial tangent estimates: endpoints use their one adjacent secant;
    // interior points use the average of their two adjacent secants.
    let mut m = vec![0.0; n];
    m[0] = secants[0];
    m[n - 1] = secants[n - 2];
    for i in 1..n - 1 {
        m[i] = (secants[i - 1] + secants[i]) / 2.0;
    }

    // Monotonicity enforcement: on any interval with a flat secant, both
    // endpoint tangents must be zero; otherwise, rescale the tangent pair
    // if it would overshoot (Fritsch-Carlson 1980 sufficient condition:
    // alpha^2 + beta^2 <= 9).
    for i in 0..n - 1 {
        let d = secants[i];
        if d == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let alpha = m[i] / d;
        let beta = m[i + 1] / d;
        let s = alpha * alpha + beta * beta;
        if s > 9.0 {
            let tau = 3.0 / s.sqrt();
            m[i] = tau * alpha * d;
            m[i + 1] = tau * beta * d;
        }
    }

    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_curve_has_no_domain_and_no_value() {
        let c = MonotonicCubic::new(&[]);
        assert_eq!(c.domain(), None);
        assert_eq!(c.value_at(500.0), None);
    }

    #[test]
    fn single_point_only_matches_at_that_point() {
        let c = MonotonicCubic::new(&[(500.0, 1.0)]);
        assert_eq!(c.domain(), Some((500.0, 500.0)));
        assert_eq!(c.value_at(500.0), Some(1.0));
        assert_eq!(c.value_at(500.1), None);
        assert_eq!(c.value_at(400.0), None);
    }

    #[test]
    fn two_points_interpolate_linearly() {
        let c = MonotonicCubic::new(&[(400.0, 0.0), (500.0, 1.0)]);
        assert!((c.value_at(450.0).unwrap() - 0.5).abs() < 1e-9);
        assert!((c.value_at(400.0).unwrap() - 0.0).abs() < 1e-9);
        assert!((c.value_at(500.0).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn out_of_domain_returns_none() {
        let c = MonotonicCubic::new(&[(400.0, 0.0), (500.0, 1.0), (600.0, 0.5)]);
        assert_eq!(c.value_at(399.9), None);
        assert_eq!(c.value_at(600.1), None);
    }

    #[test]
    fn unsorted_input_is_handled() {
        let sorted = MonotonicCubic::new(&[(400.0, 0.0), (500.0, 1.0), (600.0, 0.2)]);
        let shuffled = MonotonicCubic::new(&[(600.0, 0.2), (400.0, 0.0), (500.0, 1.0)]);
        for x in [400.0, 450.0, 500.0, 550.0, 600.0] {
            assert!((sorted.value_at(x).unwrap() - shuffled.value_at(x).unwrap()).abs() < 1e-12);
        }
    }

    #[test]
    fn duplicate_wavelength_keeps_first_and_does_not_panic() {
        let c = MonotonicCubic::new(&[(400.0, 0.0), (400.0, 999.0), (500.0, 1.0)]);
        assert_eq!(c.domain(), Some((400.0, 500.0)));
        assert_eq!(c.value_at(400.0), Some(0.0));
    }

    #[test]
    fn no_overshoot_on_a_step_like_shape() {
        // A classic case where naive cubic splines overshoot (dip below 0
        // or above 1) but monotonic cubic must not.
        let c = MonotonicCubic::new(&[
            (0.0, 0.0),
            (1.0, 0.0),
            (2.0, 0.0),
            (3.0, 1.0),
            (4.0, 1.0),
            (5.0, 1.0),
        ]);
        let mut x = 0.0;
        while x <= 5.0 {
            let y = c.value_at(x).unwrap();
            assert!(
                (-1e-9..=1.0 + 1e-9).contains(&y),
                "overshoot at x={x}: y={y}"
            );
            x += 0.05;
        }
    }

    #[test]
    fn exact_data_points_are_reproduced() {
        let pts = [(380.0, 0.1), (420.0, 0.9), (470.0, 0.3), (560.0, 0.6)];
        let c = MonotonicCubic::new(&pts);
        for &(x, y) in &pts {
            assert!((c.value_at(x).unwrap() - y).abs() < 1e-9);
        }
    }
}
