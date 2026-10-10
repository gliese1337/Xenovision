//! Spectral Curve data model (design doc §1.2.1).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::interp::MonotonicCubic;

/// What a curve represents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveType {
    Reflectance,
    Absorption,
    Emission,
    Transmittance,
    Sensitivity,
    Illumination,
    Other(String),
}

/// Physical quantity kind and unit, used to enforce unit consistency for
/// operations that depend on it (design doc §5.3.1). Dimensionless kinds
/// carry no unit; `Radiance` carries an explicit unit string (power per
/// area - this app's standardized taxonomy recognizes one such category,
/// not a separate Radiance/Irradiance split).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuantityKind {
    /// Unitless; a sample is meaningful only relative to other curves in
    /// the same visual system.
    Sensitivity,
    /// Power per area, with an explicit unit string.
    Radiance {
        unit: String,
    },
    /// Unitless; a normalized ratio, every sample meant to fall in
    /// `[0, 1]`.
    Reflectance,
    /// Unitless, every sample in `[0, 1]`: the fraction of light an
    /// absorber lets through at that wavelength (1 = no absorption).
    /// Applied by point-wise multiplication, with a weight exponent -
    /// see `illumination::apply_absorption`.
    Absorption,
    Transmittance,
    #[default]
    Unspecified,
}

/// A general-purpose, typed container for arbitrary (wavelength_nm, value)
/// point pairs - not a fixed-resolution sampled array (design doc §1.2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpectralCurve {
    pub name: String,
    pub curve_type: CurveType,
    #[serde(default)]
    pub quantity: QuantityKind,
    /// Arbitrary (wavelength_nm, value) pairs. Not required to be sorted
    /// by the caller - use `points()` for the as-stored order, or
    /// `interpolant()` for a sorted/deduplicated view.
    pub points: Vec<(f64, f64)>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,

    /// Weber fraction (§4.2.1). `None` means no noise data defined for
    /// this receptor - distinct from a measured value of zero.
    #[serde(default)]
    pub omega: Option<f64>,
    /// Relative receptor density, raw/unnormalized as transcribed from a
    /// source (§4.2.2) - meaningful only relative to the other receptor
    /// curves in the same `CurveSet`. Normalized at calculation time, not
    /// here. `None` means no density data defined.
    #[serde(default)]
    pub eta: Option<f64>,
    /// Explicit luminance weight override (§4.2.3). `None` means "use
    /// the curve-integral-derived default" (`CurveSet::luminance_weights`)
    /// rather than a measured value of zero - distinguishing "no override"
    /// from "an override of exactly 0".
    #[serde(default)]
    pub luminance_weight: Option<f64>,
    /// Receptor saturation: the maximum activation this receptor can
    /// produce, however much power reaches it. `None` means no cap.
    /// Unrelated to the perceptual `Coordinates::saturation` (chroma
    /// magnitude) - this is a property of a single receptor's response.
    #[serde(default)]
    pub saturation: Option<f64>,

    /// Lower bound of this receptor's operating window, as *ambient
    /// irradiance in W/m²* (`illumination::irradiance_w_m2` convention).
    /// Below this irradiance the receptor's operating gain falls toward
    /// zero - e.g. a (photopic) cone is silent in dim scotopic light.
    /// `None` means no lower bound.
    #[serde(default)]
    pub illumination_min: Option<f64>,
    /// Upper bound of this receptor's operating window, in the same
    /// W/m² irradiance units as `illumination_min`. Above this irradiance
    /// the receptor's gain falls toward zero - e.g. a rod is out of
    /// regime in bright light. `None` means no upper bound. Distinct from
    /// `saturation`: the window is about the *ambient light level* at
    /// which the receptor operates, saturation is the ceiling on the
    /// receptor's own transduced signal.
    #[serde(default)]
    pub illumination_max: Option<f64>,
    /// Softness of the operating window's shoulders, in decades (base-10
    /// log units of irradiance): the gain ramps over roughly
    /// ±`illumination_softness` decades around each bound. `None` falls
    /// back to `CurveSet`'s set-level default, then
    /// `DEFAULT_ILLUMINATION_SOFTNESS`. A value of 0 gives a hard edge.
    #[serde(default)]
    pub illumination_softness: Option<f64>,
}

/// Shoulder half-width (decades) used when a receptor specifies an
/// illumination bound but no explicit softness (neither per-curve nor
/// set-level). 0.5 decades ≈ a factor of ~3, a gentle transition.
pub const DEFAULT_ILLUMINATION_SOFTNESS: f64 = 0.5;

/// Smooth operating gain `g ∈ [0, 1]` for a receptor at ambient
/// irradiance `level` (W/m² - see `illumination::irradiance_w_m2`),
/// given its operating window `[min, max]` (either bound optional) and
/// shoulder width `softness` in decades. The window is evaluated in
/// `log10` of the irradiance, since naturally occurring light levels
/// span many orders of magnitude.
///
/// - Below `min` the gain falls toward 0 (receptor not yet operating);
/// - above `max` it falls toward 0 (receptor out of regime);
/// - inside the window it is 1.
///
/// A non-positive `softness` degenerates to a hard edge. A `level <= 0`
/// is treated as infinitely dark (all lower-bounded windows gated off).
/// Invalid bounds (non-positive, or `min >= max`) are treated as an
/// unbounded window, so a bad value degrades to "always on" rather than
/// silently zeroing a receptor - `CurveSet::has_invalid_illumination_window`
/// surfaces the mistake to the user.
pub fn operating_gain(
    level: f64,
    min: Option<f64>,
    max: Option<f64>,
    softness: f64,
) -> f64 {
    let valid_min = min.filter(|m| *m > 0.0);
    let valid_max = max.filter(|m| *m > 0.0);
    // Reject a crossed/empty window (or one whose bounds make no sense) as
    // invalid; the caller flags it, and here it behaves as unbounded.
    let (valid_min, valid_max) = match (valid_min, valid_max) {
        (Some(lo), Some(hi)) if lo >= hi => (None, None),
        other => other,
    };

    let log_level = if level.is_finite() && level > 0.0 {
        level.log10()
    } else {
        f64::NEG_INFINITY
    };

    let smooth = |bound: f64, rising: bool| -> f64 {
        let t = log_level - bound.log10();
        let sig = if softness > 1e-9 {
            1.0 / (1.0 + 10f64.powf(-t / softness))
        } else if t >= 0.0 {
            1.0
        } else {
            0.0
        };
        if rising {
            sig
        } else {
            1.0 - sig
        }
    };

    let lower = valid_min.map(|m| smooth(m, true)).unwrap_or(1.0);
    let upper = valid_max.map(|m| smooth(m, false)).unwrap_or(1.0);
    (lower * upper).clamp(0.0, 1.0)
}

impl SpectralCurve {
    pub fn new(name: impl Into<String>, curve_type: CurveType) -> Self {
        SpectralCurve {
            name: name.into(),
            curve_type,
            quantity: QuantityKind::default(),
            points: Vec::new(),
            metadata: BTreeMap::new(),
            omega: None,
            eta: None,
            luminance_weight: None,
            saturation: None,
            illumination_min: None,
            illumination_max: None,
            illumination_softness: None,
        }
    }

    pub fn with_points(mut self, points: Vec<(f64, f64)>) -> Self {
        self.points = points;
        self
    }

    pub fn with_quantity(mut self, quantity: QuantityKind) -> Self {
        self.quantity = quantity;
        self
    }

    pub fn with_omega(mut self, omega: f64) -> Self {
        self.omega = Some(omega);
        self
    }

    pub fn with_eta(mut self, eta: f64) -> Self {
        self.eta = Some(eta);
        self
    }

    pub fn with_luminance_weight(mut self, w: f64) -> Self {
        self.luminance_weight = Some(w);
        self
    }

    pub fn with_saturation(mut self, cap: f64) -> Self {
        self.saturation = Some(cap);
        self
    }

    /// Sets this receptor's operating window (§2.2) - either bound may be
    /// `None` (unbounded on that side).
    pub fn with_illumination_window(mut self, min: Option<f64>, max: Option<f64>) -> Self {
        self.illumination_min = min;
        self.illumination_max = max;
        self
    }

    pub fn with_illumination_softness(mut self, softness: f64) -> Self {
        self.illumination_softness = Some(softness);
        self
    }

    /// `raw` activation clamped to this curve's saturation cap, if any.
    pub fn saturate(&self, raw: f64) -> f64 {
        match self.saturation {
            Some(cap) => raw.min(cap),
            None => raw,
        }
    }

    /// This curve's operating gain `g(level)` (§2.2, receptor operating
    /// window) at ambient irradiance `level` (W/m²), using
    /// `default_softness` if the curve has no explicit
    /// `illumination_softness`.
    pub fn window_gain(&self, level: f64, default_softness: f64) -> f64 {
        operating_gain(
            level,
            self.illumination_min,
            self.illumination_max,
            self.illumination_softness.unwrap_or(default_softness),
        )
    }

    /// `true` if this curve's illumination window is malformed: a bound is
    /// non-positive, or `min >= max`. Such a window is treated as
    /// unbounded at calculation time (see `operating_gain`) and flagged by
    /// `CurveSet::has_invalid_illumination_window`.
    pub fn has_invalid_illumination_window(&self) -> bool {
        let bad_bound = |b: Option<f64>| b.is_some_and(|v| v <= 0.0);
        if bad_bound(self.illumination_min) || bad_bound(self.illumination_max) {
            return true;
        }
        matches!(
            (self.illumination_min, self.illumination_max),
            (Some(lo), Some(hi)) if lo >= hi
        )
    }

    /// Integral of this curve's values over its own domain (trapezoidal),
    /// used as the curve-integral-derived default luminance weight
    /// (§4.2.3) when `luminance_weight` is unset. `0.0` for a curve with
    /// fewer than 2 points.
    pub fn integral(&self, step_nm: f64) -> f64 {
        let Some((lo, hi)) = self.domain() else {
            return 0.0;
        };
        if hi <= lo {
            return 0.0;
        }
        let interp = self.interpolant();
        let n = ((hi - lo) / step_nm).ceil().max(1.0) as usize;
        let dx = (hi - lo) / n as f64;
        let mut sum = 0.0;
        let mut prev = interp.value_at(lo).unwrap_or(0.0);
        for i in 1..=n {
            let wl = lo + i as f64 * dx;
            let cur = interp.value_at(wl).unwrap_or(0.0);
            sum += 0.5 * (prev + cur) * dx;
            prev = cur;
        }
        sum
    }

    /// A monotonic-cubic interpolant built from this curve's current
    /// points. Rebuilt each call - cheap for the point counts this app
    /// deals with (tens, not thousands); callers doing many evaluations
    /// against an unchanging curve should build this once and reuse it.
    pub fn interpolant(&self) -> MonotonicCubic {
        MonotonicCubic::new(&self.points)
    }

    /// Interpolated value at `wavelength_nm`, or `None` if outside this
    /// curve's domain.
    pub fn value_at(&self, wavelength_nm: f64) -> Option<f64> {
        self.interpolant().value_at(wavelength_nm)
    }

    /// This curve's (min, max) wavelength, or `None` if it has no points.
    pub fn domain(&self) -> Option<(f64, f64)> {
        self.interpolant().domain()
    }

    pub fn add_point(&mut self, wavelength_nm: f64, value: f64) {
        self.points.push((wavelength_nm, value));
    }

    /// Removes the point nearest to `wavelength_nm`, if any point exists.
    pub fn remove_nearest_point(&mut self, wavelength_nm: f64) {
        if let Some((idx, _)) = self.points.iter().enumerate().min_by(|(_, a), (_, b)| {
            (a.0 - wavelength_nm)
                .abs()
                .partial_cmp(&(b.0 - wavelength_nm).abs())
                .unwrap()
        }) {
            self.points.remove(idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_json() {
        let curve = SpectralCurve::new("L-cone sensitivity", CurveType::Sensitivity)
            .with_points(vec![(420.0, 0.1), (530.0, 0.8), (560.0, 1.0)])
            .with_quantity(QuantityKind::Sensitivity);
        let json = serde_json::to_string(&curve).unwrap();
        let back: SpectralCurve = serde_json::from_str(&json).unwrap();
        assert_eq!(curve, back);
    }

    #[test]
    fn round_trip_with_radiance_unit_and_metadata() {
        let mut curve = SpectralCurve::new("Solar irradiance", CurveType::Illumination)
            .with_points(vec![(400.0, 1.5), (700.0, 1.2)])
            .with_quantity(QuantityKind::Radiance {
                unit: "W.m-2.nm-1".to_string(),
            });
        curve
            .metadata
            .insert("source".to_string(), "ASTM G173".to_string());
        let json = serde_json::to_string(&curve).unwrap();
        let back: SpectralCurve = serde_json::from_str(&json).unwrap();
        assert_eq!(curve, back);
    }

    #[test]
    fn default_quantity_is_unspecified_when_missing_from_json() {
        let json = r#"{"name":"x","curve_type":"sensitivity","points":[]}"#;
        let curve: SpectralCurve = serde_json::from_str(json).unwrap();
        assert_eq!(curve.quantity, QuantityKind::Unspecified);
    }

    #[test]
    fn value_at_delegates_to_interpolant() {
        let curve = SpectralCurve::new("test", CurveType::Sensitivity)
            .with_points(vec![(400.0, 0.0), (500.0, 1.0)]);
        assert!((curve.value_at(450.0).unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(curve.value_at(999.0), None);
    }

    #[test]
    fn remove_nearest_point() {
        let mut curve = SpectralCurve::new("test", CurveType::Sensitivity).with_points(vec![
            (400.0, 0.0),
            (500.0, 1.0),
            (600.0, 0.5),
        ]);
        curve.remove_nearest_point(510.0);
        assert_eq!(curve.points.len(), 2);
        assert!(!curve.points.iter().any(|p| p.0 == 500.0));
    }

    #[test]
    fn noise_and_luminance_fields_default_to_none() {
        let curve = SpectralCurve::new("test", CurveType::Sensitivity);
        assert_eq!(curve.omega, None);
        assert_eq!(curve.eta, None);
        assert_eq!(curve.luminance_weight, None);
    }

    #[test]
    fn noise_and_luminance_fields_round_trip_through_json() {
        let curve = SpectralCurve::new("test", CurveType::Sensitivity)
            .with_points(vec![(400.0, 0.0), (500.0, 1.0)])
            .with_omega(0.05)
            .with_eta(16.0)
            .with_luminance_weight(0.9);
        let json = serde_json::to_string(&curve).unwrap();
        let back: SpectralCurve = serde_json::from_str(&json).unwrap();
        assert_eq!(curve, back);
    }

    #[test]
    fn saturation_defaults_to_none_round_trips_and_clamps() {
        let plain = SpectralCurve::new("test", CurveType::Sensitivity);
        assert_eq!(plain.saturation, None);
        assert_eq!(plain.saturate(1e9), 1e9, "no cap means pass-through");

        let capped = plain.clone().with_saturation(2.5);
        assert_eq!(capped.saturate(1.0), 1.0);
        assert_eq!(capped.saturate(10.0), 2.5);
        let json = serde_json::to_string(&capped).unwrap();
        let back: SpectralCurve = serde_json::from_str(&json).unwrap();
        assert_eq!(back.saturation, Some(2.5));

        // Files saved before this field existed still load.
        let old = r#"{"name":"x","curve_type":"sensitivity","points":[]}"#;
        let curve: SpectralCurve = serde_json::from_str(old).unwrap();
        assert_eq!(curve.saturation, None);
    }

    #[test]
    fn illumination_fields_default_to_none_and_round_trip() {
        let plain = SpectralCurve::new("test", CurveType::Sensitivity);
        assert_eq!(plain.illumination_min, None);
        assert_eq!(plain.illumination_max, None);
        assert_eq!(plain.illumination_softness, None);

        let windowed = plain
            .clone()
            .with_illumination_window(Some(0.01), Some(10.0))
            .with_illumination_softness(0.25);
        let json = serde_json::to_string(&windowed).unwrap();
        let back: SpectralCurve = serde_json::from_str(&json).unwrap();
        assert_eq!(windowed, back);

        // Old files (no window fields) still load as unbounded.
        let old = r#"{"name":"x","curve_type":"sensitivity","points":[]}"#;
        let curve: SpectralCurve = serde_json::from_str(old).unwrap();
        assert_eq!(curve.illumination_min, None);
        assert_eq!(curve.illumination_max, None);
        assert_eq!(curve.illumination_softness, None);
    }

    #[test]
    fn operating_gain_is_inside_window_and_decades_outside() {
        // Wide window [1e-8, 1e8] W/m² with 0.5-decade shoulders, so the
        // interior points to check are many decades from the bounds and
        // the sigmoid has fully saturated to 1.
        let g = |level| operating_gain(level, Some(1e-8), Some(1e8), 0.5);
        assert!((g(1.0) - 1.0).abs() < 1e-9, "mid-window should be fully on");
        assert!((g(1e-3) - 1.0).abs() < 1e-9, "5 decades inside the lower shoulder");
        assert!((g(1e3) - 1.0).abs() < 1e-9, "5 decades inside the upper shoulder");
        // Well outside (many decades) -> ~0.
        assert!(g(1e-12) < 1e-3, "far below min: {}", g(1e-12));
        assert!(g(1e12) < 1e-3, "far above max: {}", g(1e12));
        // The lower shoulder is monotonically increasing, the upper
        // monotonically decreasing.
        assert!(g(1e-7) < g(1e-6), "lower shoulder should rise, got {} then {}", g(1e-7), g(1e-6));
        assert!(g(1e7) > g(1e8), "upper shoulder should fall, got {} then {}", g(1e7), g(1e8));
        // Zero/negative irradiance is treated as infinitely dark.
        assert_eq!(g(0.0), 0.0);
        assert_eq!(g(-5.0), 0.0);
    }

    #[test]
    fn operating_gain_unbounded_sides_and_hard_edge() {
        // No bounds at all -> always fully on.
        assert_eq!(operating_gain(1e-9, None, None, 0.5), 1.0);
        assert_eq!(operating_gain(1e9, None, None, 0.5), 1.0);
        // Only a lower bound -> 1 far above it, 0 far below.
        assert!(operating_gain(100.0, Some(1.0), None, 0.5) > 0.999);
        assert!(operating_gain(1e-6, Some(1.0), None, 0.5) < 1e-3);
        // A hard edge (zero softness).
        assert_eq!(operating_gain(0.99, Some(1.0), None, 0.0), 0.0);
        assert_eq!(operating_gain(1.0, Some(1.0), None, 0.0), 1.0);
        assert_eq!(operating_gain(1.01, Some(1.0), Some(2.0), 0.0), 1.0);
        assert_eq!(operating_gain(2.01, Some(1.0), Some(2.0), 0.0), 0.0);
    }

    #[test]
    fn invalid_windows_are_detected_and_degrade_to_unbounded() {
        let crossed = SpectralCurve::new("t", CurveType::Sensitivity)
            .with_illumination_window(Some(10.0), Some(1.0));
        assert!(crossed.has_invalid_illumination_window());
        // Degrades to always-on rather than silently zeroing the receptor.
        assert_eq!(crossed.window_gain(0.001, 0.5), 1.0);
        assert_eq!(crossed.window_gain(1e6, 0.5), 1.0);

        let negative = SpectralCurve::new("t", CurveType::Sensitivity)
            .with_illumination_window(Some(-1.0), None);
        assert!(negative.has_invalid_illumination_window());
        assert_eq!(negative.window_gain(1.0, 0.5), 1.0);

        let ok = SpectralCurve::new("t", CurveType::Sensitivity)
            .with_illumination_window(Some(0.1), Some(1.0));
        assert!(!ok.has_invalid_illumination_window());
    }

    #[test]
    fn window_gain_uses_default_when_curve_softness_unset() {
        let curve = SpectralCurve::new("t", CurveType::Sensitivity)
            .with_illumination_window(Some(1.0), None);
        // Explicit override on the curve wins over the passed default.
        let explicit = curve.clone().with_illumination_softness(0.0);
        assert_eq!(explicit.window_gain(0.9, 0.5), 0.0);
        assert!(curve.window_gain(0.9, 0.5) > 0.0);
    }

    #[test]
    fn missing_noise_fields_in_json_default_to_none_not_an_error() {
        // Confirms old saved files (without these newer noise fields) still load.
        let json = r#"{"name":"x","curve_type":"sensitivity","points":[]}"#;
        let curve: SpectralCurve = serde_json::from_str(json).unwrap();
        assert_eq!(curve.omega, None);
        assert_eq!(curve.eta, None);
        assert_eq!(curve.luminance_weight, None);
    }

    #[test]
    fn integral_of_a_flat_box() {
        let curve = SpectralCurve::new("box", CurveType::Sensitivity)
            .with_points(vec![(100.0, 2.0), (200.0, 2.0)]);
        assert!((curve.integral(0.5) - 200.0).abs() < 1e-6); // 2.0 * 100nm width
    }

    #[test]
    fn integral_of_a_peaked_curve_is_not_the_naive_triangle_area() {
        // Points forming a 0->1->0 "triangle" over a 100nm base, BUT the
        // monotonic cubic interpolant between them is an eased S-curve on
        // each half (zero slope at the peak and at both flat ends,
        // matching this curve's secants), not a straight ramp - so the
        // true area isn't the naive triangle formula (0.5*base*height =
        // 50). 175/3 ≈ 58.33 is the interpolant's own exact closed-form
        // area for this specific Hermite construction (independently
        // confirmed by sampling at a much finer step, 0.01 vs. the
        // 0.1 used in the assertion, and seeing the same value).
        let curve = SpectralCurve::new("peaked", CurveType::Sensitivity).with_points(vec![
            (100.0, 0.0),
            (150.0, 1.0),
            (200.0, 0.0),
        ]);
        assert!((curve.integral(0.1) - 175.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn integral_of_empty_curve_is_zero() {
        let curve = SpectralCurve::new("empty", CurveType::Sensitivity);
        assert_eq!(curve.integral(1.0), 0.0);
    }
}
