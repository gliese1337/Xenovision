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

    /// `raw` activation clamped to this curve's saturation cap, if any.
    pub fn saturate(&self, raw: f64) -> f64 {
        match self.saturation {
            Some(cap) => raw.min(cap),
            None => raw,
        }
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
    fn missing_noise_fields_in_json_default_to_none_not_an_error() {
        // Confirms old saved files (without these Phase 4 fields) still load.
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
