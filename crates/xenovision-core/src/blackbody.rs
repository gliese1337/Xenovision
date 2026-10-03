//! Black-body (Planck's law) illuminant generation, with optional
//! atmospheric absorption bands and manual notches as multiplicative
//! attenuation (design doc §5.5.1).

use serde::{Deserialize, Serialize};

use crate::curve::{CurveType, QuantityKind, SpectralCurve};
use crate::preset_store;

const NOTCH_PRESETS_FILE: &str = "notch_presets.json";

const PLANCK_H: f64 = 6.62607015e-34; // J*s
const SPEED_OF_LIGHT: f64 = 2.99792458e8; // m/s
const BOLTZMANN_K: f64 = 1.380649e-23; // J/K

/// Spectral radiance (W.sr^-1.m^-2.nm^-1) of a black body at
/// `temperature_k`, at `wavelength_nm`, per Planck's law. The usual
/// per-meter formula is scaled by `1e-9` to express the result per
/// nanometer, matching how wavelengths are stored throughout this app.
pub fn planck_radiance(wavelength_nm: f64, temperature_k: f64) -> f64 {
    let lambda_m = wavelength_nm * 1e-9;
    let numerator = 2.0 * PLANCK_H * SPEED_OF_LIGHT.powi(2);
    let exponent = (PLANCK_H * SPEED_OF_LIGHT) / (lambda_m * BOLTZMANN_K * temperature_k);
    let denominator = lambda_m.powi(5) * (exponent.exp() - 1.0);
    (numerator / denominator) * 1e-9
}

/// A Gaussian-shaped multiplicative absorption notch: `center_nm` and
/// `width_nm` (standard deviation), `depth` in `0.0..=1.0` (fraction of
/// transmittance removed at the center; `1.0` blocks completely there).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbsorptionNotch {
    pub center_nm: f64,
    pub width_nm: f64,
    pub depth: f64,
}

/// A named, user-editable notch preset (one entry in the on-disk preset
/// library - see `load_notch_presets`/`save_notch_presets`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedNotch {
    pub name: String,
    pub notch: AbsorptionNotch,
}

impl AbsorptionNotch {
    /// This notch as an Absorption curve, sampled across ±5 widths of its
    /// center (beyond which it absorbs essentially nothing) finely enough
    /// to keep its shape.
    pub fn absorption_curve(&self, name: impl Into<String>) -> SpectralCurve {
        let half = 5.0 * self.width_nm.max(1e-3);
        let step = (self.width_nm / 4.0).clamp(0.01, 1.0);
        let n = ((2.0 * half) / step).ceil() as usize;
        let points = (0..=n)
            .map(|i| {
                let wl = self.center_nm - half + i as f64 * step;
                (wl, self.transmittance_at(wl))
            })
            .collect();
        SpectralCurve::new(name, CurveType::Absorption)
            .with_points(points)
            .with_quantity(QuantityKind::Absorption)
    }

    pub fn transmittance_at(&self, wavelength_nm: f64) -> f64 {
        let d = (wavelength_nm - self.center_nm) / self.width_nm;
        1.0 - self.depth * (-0.5 * d * d).exp()
    }
}

/// The embedded pristine defaults (§5.5.1): O2 A/B-bands and the
/// documented water vapor band centers. Width/depth are illustrative
/// (the design doc gives band centers, not calibrated widths/depths) -
/// flagged as such, not precise atmospheric-transmission data. This is
/// the fallback `load_notch_presets` writes out the first time it runs
/// (or after "restore defaults") - the *live* source of truth is the
/// on-disk, user-editable preset file.
pub fn builtin_default_notches() -> Vec<NamedNotch> {
    fn n(name: &str, center_nm: f64, width_nm: f64, depth: f64) -> NamedNotch {
        NamedNotch {
            name: name.to_string(),
            notch: AbsorptionNotch {
                center_nm,
                width_nm,
                depth,
            },
        }
    }
    vec![
        n("O2 A-band (~760nm)", 760.0, 2.0, 0.6),
        n("O2 B-band (~687nm)", 687.0, 1.5, 0.4),
        n("Water vapor 720nm", 720.0, 5.0, 0.3),
        n("Water vapor 820nm", 820.0, 8.0, 0.3),
        n("Water vapor 940nm", 940.0, 10.0, 0.5),
        n("Water vapor 1100nm", 1100.0, 10.0, 0.3),
        n("Water vapor 1130nm", 1130.0, 10.0, 0.3),
        n("Water vapor 1370nm", 1370.0, 15.0, 0.6),
        n("Water vapor 1450nm", 1450.0, 15.0, 0.5),
        n("Water vapor 1950nm", 1950.0, 20.0, 0.6),
        n("Water vapor 2500nm", 2500.0, 25.0, 0.5),
    ]
}

/// The live, user-editable notch preset library: reads
/// `notch_presets.json` from the app's data directory, bootstrapping it
/// from `builtin_default_notches()` the first time (or if it's missing/
/// corrupt).
pub fn load_notch_presets() -> Vec<NamedNotch> {
    preset_store::load_or_init(NOTCH_PRESETS_FILE, builtin_default_notches)
}

pub fn save_notch_presets(presets: &[NamedNotch]) -> std::io::Result<()> {
    preset_store::save(NOTCH_PRESETS_FILE, presets)
}

/// Generates a black-body curve at `temperature_k` from `wl_min` to
/// `wl_max` nm at `step_nm` spacing, attenuated by `notches` (preset
/// bands and/or manual notches alike - both are just `AbsorptionNotch`),
/// normalized so the unattenuated curve's own peak would be 1.0 (the
/// generated curve's actual peak may be lower if a notch sits near it).
/// Tagged `Illumination`/`Radiance` with a `"relative"` unit, since this
/// is a normalized synthetic curve, not a calibrated radiometric
/// measurement.
pub fn generate_blackbody_curve(
    temperature_k: f64,
    wl_min: f64,
    wl_max: f64,
    step_nm: f64,
    notches: &[AbsorptionNotch],
) -> SpectralCurve {
    let n = ((wl_max - wl_min) / step_nm).round().max(1.0) as usize;
    let raw: Vec<f64> = (0..=n)
        .map(|i| planck_radiance(wl_min + i as f64 * step_nm, temperature_k))
        .collect();
    let peak = raw.iter().cloned().fold(0.0_f64, f64::max);
    let points: Vec<(f64, f64)> = (0..=n)
        .map(|i| {
            let wl = wl_min + i as f64 * step_nm;
            let mut v = if peak > 0.0 { raw[i] / peak } else { 0.0 };
            for notch in notches {
                v *= notch.transmittance_at(wl);
            }
            (wl, v)
        })
        .collect();

    let mut curve = SpectralCurve::new(
        format!("{temperature_k:.0}K black body"),
        CurveType::Illumination,
    )
    .with_points(points)
    .with_quantity(QuantityKind::Radiance {
        unit: "relative".to_string(),
    });
    curve
        .metadata
        .insert("temperature_k".to_string(), temperature_k.to_string());
    curve
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notch_absorption_curve_matches_transmittance_and_is_in_unit_range() {
        let notch = AbsorptionNotch {
            center_nm: 760.0,
            width_nm: 2.0,
            depth: 0.8,
        };
        let curve = notch.absorption_curve("O2 A-band");
        assert_eq!(curve.quantity, QuantityKind::Absorption);
        assert!(curve.points.iter().all(|&(_, v)| (0.0..=1.0).contains(&v)));
        assert!((curve.value_at(760.0).unwrap() - 0.2).abs() < 1e-6);
        let (lo, hi) = curve.domain().unwrap();
        assert!((lo - 750.0).abs() < 1e-6 && (hi - 770.0).abs() < 0.1);
        assert!(
            curve.value_at(lo).unwrap() > 0.999,
            "tails reach ~no absorption"
        );
    }

    #[test]
    fn sun_like_blackbody_peaks_in_visible_range() {
        // Wien's law: peak wavelength (nm) ~= 2.898e6 / T(K). For 5778K
        // that's ~501nm - squarely in the visible range, a hand-
        // calculable sanity check independent of this module's own code.
        let mut best_wl = 0.0;
        let mut best_v = -1.0;
        let mut wl = 300.0;
        while wl <= 2500.0 {
            let v = planck_radiance(wl, 5778.0);
            if v > best_v {
                best_v = v;
                best_wl = wl;
            }
            wl += 1.0;
        }
        let expected = 2.898e6 / 5778.0;
        assert!(
            (best_wl - expected).abs() < 5.0,
            "peak at {best_wl}, expected ~{expected}"
        );
    }

    #[test]
    fn hotter_blackbody_peaks_at_shorter_wavelength() {
        let peak_at = |t: f64| {
            let mut best_wl = 0.0;
            let mut best_v = -1.0;
            let mut wl = 100.0;
            while wl <= 3000.0 {
                let v = planck_radiance(wl, t);
                if v > best_v {
                    best_v = v;
                    best_wl = wl;
                }
                wl += 1.0;
            }
            best_wl
        };
        assert!(peak_at(10000.0) < peak_at(3000.0));
    }

    #[test]
    fn generated_curve_peak_is_normalized_to_one_without_notches() {
        let curve = generate_blackbody_curve(5778.0, 300.0, 1000.0, 1.0, &[]);
        let peak = curve.points.iter().map(|&(_, v)| v).fold(0.0_f64, f64::max);
        assert!((peak - 1.0).abs() < 1e-9);
    }

    #[test]
    fn notch_reduces_value_at_its_center() {
        let notch = AbsorptionNotch {
            center_nm: 760.0,
            width_nm: 2.0,
            depth: 0.6,
        };
        let without = generate_blackbody_curve(5778.0, 700.0, 820.0, 1.0, &[]);
        let with = generate_blackbody_curve(5778.0, 700.0, 820.0, 1.0, &[notch]);
        let v_without = without.value_at(760.0).unwrap();
        let v_with = with.value_at(760.0).unwrap();
        assert!(
            v_with < v_without * 0.45,
            "v_with={v_with} v_without={v_without}"
        );
    }

    #[test]
    fn notch_transmittance_matches_hand_calculation_at_center_and_far_away() {
        let notch = AbsorptionNotch {
            center_nm: 500.0,
            width_nm: 10.0,
            depth: 0.6,
        };
        assert!((notch.transmittance_at(500.0) - 0.4).abs() < 1e-9); // 1 - depth at center
        assert!((notch.transmittance_at(1000.0) - 1.0).abs() < 1e-6); // far away: ~no effect
    }

    #[test]
    fn multiple_notches_compound_multiplicatively() {
        let n1 = AbsorptionNotch {
            center_nm: 500.0,
            width_nm: 50.0,
            depth: 0.5,
        };
        let n2 = AbsorptionNotch {
            center_nm: 500.0,
            width_nm: 50.0,
            depth: 0.5,
        };
        let one = generate_blackbody_curve(5778.0, 400.0, 600.0, 1.0, std::slice::from_ref(&n1));
        let two = generate_blackbody_curve(5778.0, 400.0, 600.0, 1.0, &[n1, n2]);
        let v_one = one.value_at(500.0).unwrap();
        let v_two = two.value_at(500.0).unwrap();
        // Each notch independently removes 50% at center -> compounded ~25% remains.
        assert!(
            (v_two / v_one - 0.5).abs() < 1e-6,
            "v_two/v_one={}",
            v_two / v_one
        );
    }

    #[test]
    fn builtin_default_notches_have_unique_non_empty_names() {
        let defaults = builtin_default_notches();
        assert!(!defaults.is_empty());
        let mut names: Vec<&str> = defaults.iter().map(|n| n.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), defaults.len(), "preset names should be unique");
        assert!(defaults.iter().all(|n| !n.name.is_empty()));
    }

    #[test]
    fn notch_presets_round_trip_through_save_and_load() {
        // Use a scratch filename-independent round trip via preset_store
        // directly isn't exposed here, so exercise the public API: save a
        // custom set, confirm load returns exactly that (not silently
        // falling back to builtins), then restore.
        let custom = vec![NamedNotch {
            name: "Test-only notch".to_string(),
            notch: AbsorptionNotch {
                center_nm: 123.0,
                width_nm: 4.0,
                depth: 0.5,
            },
        }];
        save_notch_presets(&custom).unwrap();
        let loaded = load_notch_presets();
        assert_eq!(loaded, custom);

        // Restore so this test doesn't leave the shared on-disk file
        // (notch_presets.json) permanently clobbered for anything else
        // that reads it, including the app itself.
        save_notch_presets(&builtin_default_notches()).unwrap();
    }
}
