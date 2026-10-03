//! Composite narrow-band illuminant generation (design doc §5.5.2): a
//! Gaussian peak as the core primitive, named real-world source presets
//! built from it, and weighted summation to combine several components
//! (presets and/or manual peaks) into one curve.

use serde::{Deserialize, Serialize};

use crate::curve::{CurveType, QuantityKind, SpectralCurve};
use crate::preset_store;

const SOURCE_PRESETS_FILE: &str = "narrowband_source_presets.json";

/// One Gaussian-shaped spectral peak: `amplitude` at `center_nm`,
/// falling off with standard deviation `bandwidth_nm`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GaussianComponent {
    pub center_nm: f64,
    pub amplitude: f64,
    pub bandwidth_nm: f64,
}

pub fn gaussian_peak(wavelength_nm: f64, center_nm: f64, amplitude: f64, bandwidth_nm: f64) -> f64 {
    let d = (wavelength_nm - center_nm) / bandwidth_nm;
    amplitude * (-0.5 * d * d).exp()
}

impl GaussianComponent {
    pub fn value_at(&self, wavelength_nm: f64) -> f64 {
        gaussian_peak(
            wavelength_nm,
            self.center_nm,
            self.amplitude,
            self.bandwidth_nm,
        )
    }
}

/// A named real-world light source, modeled as a fixed set of Gaussian
/// components (§5.5.2's "presets are pre-configured sets of Gaussian
/// peak parameters, not a separately-implemented curve type"). Relative
/// amplitudes/bandwidths are illustrative, not spectrophotometrically
/// calibrated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedSource {
    pub name: String,
    pub components: Vec<GaussianComponent>,
}

fn component(center_nm: f64, amplitude: f64, bandwidth_nm: f64) -> GaussianComponent {
    GaussianComponent {
        center_nm,
        amplitude,
        bandwidth_nm,
    }
}

/// The embedded pristine defaults - see `blackbody::builtin_default_notches`
/// for the same pattern and rationale. The live source of truth is the
/// on-disk, user-editable file `load_source_presets` reads.
pub fn builtin_default_sources() -> Vec<NamedSource> {
    vec![
        NamedSource {
            name: "Low-pressure sodium vapor".to_string(),
            components: vec![component(589.0, 1.0, 0.3), component(589.6, 1.0, 0.3)],
        },
        NamedSource {
            name: "High-pressure sodium vapor".to_string(),
            components: vec![component(589.0, 1.0, 15.0)],
        },
        NamedSource {
            name: "Mercury vapor".to_string(),
            components: vec![
                component(365.0, 0.4, 2.0),
                component(405.0, 0.3, 2.0),
                component(436.0, 0.5, 2.0),
                component(546.0, 1.0, 2.0),
                component(578.0, 0.6, 2.0),
            ],
        },
        NamedSource {
            name: "Cool white LED".to_string(),
            components: vec![
                component(450.0, 1.0, 15.0), // blue pump chip
                component(560.0, 0.7, 60.0), // phosphor hump, blue-weighted
            ],
        },
        NamedSource {
            name: "Warm white LED".to_string(),
            components: vec![
                component(450.0, 0.5, 15.0), // smaller blue pump chip
                component(580.0, 1.0, 70.0), // larger phosphor hump
            ],
        },
    ]
}

/// The live, user-editable narrow-band source preset library.
pub fn load_source_presets() -> Vec<NamedSource> {
    preset_store::load_or_init(SOURCE_PRESETS_FILE, builtin_default_sources)
}

pub fn save_source_presets(presets: &[NamedSource]) -> std::io::Result<()> {
    preset_store::save(SOURCE_PRESETS_FILE, presets)
}

/// Combines `components` via weighted summation (§5.5.2: "each component
/// contributes a user-specified relative-intensity weight before being
/// summed pointwise") into a single `Illumination`/`Radiance` curve from
/// `wl_min` to `wl_max` at `step_nm` spacing.
pub fn generate_composite_curve(
    components: &[(GaussianComponent, f64)],
    wl_min: f64,
    wl_max: f64,
    step_nm: f64,
) -> SpectralCurve {
    let n = ((wl_max - wl_min) / step_nm).round().max(1.0) as usize;
    let points: Vec<(f64, f64)> = (0..=n)
        .map(|i| {
            let wl = wl_min + i as f64 * step_nm;
            let v = components
                .iter()
                .map(|(c, weight)| weight * c.value_at(wl))
                .sum();
            (wl, v)
        })
        .collect();
    SpectralCurve::new("Composite narrow-band source", CurveType::Illumination)
        .with_points(points)
        .with_quantity(QuantityKind::Radiance {
            unit: "relative".to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_peak_equals_amplitude_at_center() {
        assert!((gaussian_peak(500.0, 500.0, 2.5, 10.0) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn gaussian_peak_decays_away_from_center() {
        let at_center = gaussian_peak(500.0, 500.0, 1.0, 10.0);
        let one_sigma = gaussian_peak(510.0, 500.0, 1.0, 10.0);
        let two_sigma = gaussian_peak(520.0, 500.0, 1.0, 10.0);
        assert!(at_center > one_sigma && one_sigma > two_sigma);
        // Standard Gaussian: one sigma away = exp(-0.5) of center.
        assert!((one_sigma - (-0.5_f64).exp()).abs() < 1e-9);
    }

    #[test]
    fn all_five_named_presets_exist_with_components() {
        let presets = builtin_default_sources();
        assert_eq!(presets.len(), 5);
        assert!(presets.iter().all(|p| !p.components.is_empty()));
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Low-pressure sodium vapor"));
        assert!(names.contains(&"High-pressure sodium vapor"));
        assert!(names.contains(&"Mercury vapor"));
        assert!(names.contains(&"Cool white LED"));
        assert!(names.contains(&"Warm white LED"));
    }

    #[test]
    fn sodium_d_lines_are_narrow_and_close_together() {
        let sodium = &builtin_default_sources()[0];
        assert_eq!(sodium.components.len(), 2);
        let centers: Vec<f64> = sodium.components.iter().map(|c| c.center_nm).collect();
        assert!((centers[1] - centers[0] - 0.6).abs() < 1e-9);
        assert!(
            sodium.components.iter().all(|c| c.bandwidth_nm < 1.0),
            "D-lines should be narrow"
        );
    }

    #[test]
    fn composite_weighted_sum_matches_hand_calculation() {
        // Two components, weights 2 and 3: at wl=500 (component A's
        // center, far from B's), value should be ~2*A.amplitude (B's
        // contribution negligible that far away).
        let a = GaussianComponent {
            center_nm: 500.0,
            amplitude: 1.0,
            bandwidth_nm: 5.0,
        };
        let b = GaussianComponent {
            center_nm: 700.0,
            amplitude: 1.0,
            bandwidth_nm: 5.0,
        };
        let composite = generate_composite_curve(&[(a, 2.0), (b, 3.0)], 400.0, 800.0, 1.0);
        let v = composite.value_at(500.0).unwrap();
        assert!((v - 2.0).abs() < 1e-6, "v={v}");
    }

    #[test]
    fn composite_sum_is_exact_at_a_components_own_center_with_no_other_nearby() {
        let a = GaussianComponent {
            center_nm: 500.0,
            amplitude: 3.0,
            bandwidth_nm: 10.0,
        };
        let composite = generate_composite_curve(&[(a, 1.5)], 400.0, 600.0, 1.0);
        let v = composite.value_at(500.0).unwrap();
        assert!((v - 4.5).abs() < 1e-9); // weight 1.5 * amplitude 3.0
    }

    #[test]
    fn builtin_default_sources_have_unique_non_empty_names() {
        let defaults = builtin_default_sources();
        let mut names: Vec<&str> = defaults.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), defaults.len());
        assert!(defaults.iter().all(|s| !s.name.is_empty()));
    }

    #[test]
    fn source_presets_round_trip_through_save_and_load() {
        let custom = vec![NamedSource {
            name: "Test-only source".to_string(),
            components: vec![component(500.0, 1.0, 10.0)],
        }];
        save_source_presets(&custom).unwrap();
        let loaded = load_source_presets();
        assert_eq!(loaded, custom);

        // Restore so this test doesn't permanently clobber the shared
        // on-disk file for anything else that reads it.
        save_source_presets(&builtin_default_sources()).unwrap();
    }
}
