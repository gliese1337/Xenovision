//! Illuminant-independent reflectance derivation and the forward
//! conversion back to a predicted measurement under a different
//! illuminant (design doc §5.3).

use crate::curve::{CurveType, QuantityKind, SpectralCurve};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReflectanceError {
    #[error("{curve_name} has no quantity kind/unit specified - set it to Radiance before deriving reflectance")]
    MissingQuantityKind { curve_name: String },
    #[error("{curve_name} is {kind} - reflectance derivation needs a Radiance curve")]
    WrongQuantityKind { curve_name: String, kind: String },
    #[error("unit mismatch: {} is {} \"{}\", {} is {} \"{}\" - reconcile units/kind before dividing", .0.a_name, .0.a_kind, .0.a_unit, .0.b_name, .0.b_kind, .0.b_unit)]
    MismatchedUnits(Box<MismatchedUnitsDetails>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MismatchedUnitsDetails {
    pub a_name: String,
    pub a_kind: String,
    pub a_unit: String,
    pub b_name: String,
    pub b_kind: String,
    pub b_unit: String,
}

fn kind_label(q: &QuantityKind) -> &'static str {
    match q {
        QuantityKind::Reflectance => "Reflectance",
        QuantityKind::Absorption => "Absorption",
        QuantityKind::Transmittance => "Transmittance",
        QuantityKind::Sensitivity => "Sensitivity",
        QuantityKind::Radiance { .. } => "Radiance",
        QuantityKind::Unspecified => "Unspecified",
    }
}

/// The hard-block unit-consistency check (§5.3.1): both curves must be
/// `Radiance`, with identical unit strings.
pub fn check_units_for_division(
    a: &SpectralCurve,
    b: &SpectralCurve,
) -> Result<(), ReflectanceError> {
    for c in [a, b] {
        if c.quantity == QuantityKind::Unspecified {
            return Err(ReflectanceError::MissingQuantityKind {
                curve_name: c.name.clone(),
            });
        }
    }
    for c in [a, b] {
        if !matches!(c.quantity, QuantityKind::Radiance { .. }) {
            return Err(ReflectanceError::WrongQuantityKind {
                curve_name: c.name.clone(),
                kind: kind_label(&c.quantity).to_string(),
            });
        }
    }
    let same_kind_and_unit = match (&a.quantity, &b.quantity) {
        (QuantityKind::Radiance { unit: ua }, QuantityKind::Radiance { unit: ub }) => ua == ub,
        _ => false,
    };
    if !same_kind_and_unit {
        let unit_of = |q: &QuantityKind| match q {
            QuantityKind::Radiance { unit } => unit.clone(),
            _ => String::new(),
        };
        return Err(ReflectanceError::MismatchedUnits(Box::new(
            MismatchedUnitsDetails {
                a_name: a.name.clone(),
                a_kind: kind_label(&a.quantity).to_string(),
                a_unit: unit_of(&a.quantity),
                b_name: b.name.clone(),
                b_kind: kind_label(&b.quantity).to_string(),
                b_unit: unit_of(&b.quantity),
            },
        )));
    }
    Ok(())
}

/// Pointwise-divides two curves over the intersection of their domains
/// (reflectance is only defined where both a measurement and an
/// illuminant value exist), skipping points where the denominator is
/// too close to zero to divide meaningfully.
fn divide_over_intersection(
    numerator: &SpectralCurve,
    denominator: &SpectralCurve,
    step_nm: f64,
) -> Vec<(f64, f64)> {
    let (Some((n_lo, n_hi)), Some((d_lo, d_hi))) = (numerator.domain(), denominator.domain())
    else {
        return Vec::new();
    };
    let lo = n_lo.max(d_lo);
    let hi = n_hi.min(d_hi);
    if hi <= lo {
        return Vec::new();
    }
    let n_interp = numerator.interpolant();
    let d_interp = denominator.interpolant();
    let n = ((hi - lo) / step_nm).round().max(1.0) as usize;
    (0..=n)
        .filter_map(|i| {
            let wl = lo + i as f64 * step_nm;
            let num = n_interp.value_at(wl)?;
            let den = d_interp.value_at(wl)?;
            if den.abs() < 1e-12 {
                None
            } else {
                Some((wl, num / den))
            }
        })
        .collect()
}

/// `Reflectance(λ) = M(λ) / I(λ)` (§5.3.1): back-derives illuminant-
/// independent reflectance from a measured curve `measured` and the
/// illuminant `illuminant` it was measured under. Hard-blocked by
/// `check_units_for_division` - returns `Err` without computing anything
/// if the units aren't consistent.
pub fn derive_reflectance(
    measured: &SpectralCurve,
    illuminant: &SpectralCurve,
    step_nm: f64,
) -> Result<SpectralCurve, ReflectanceError> {
    check_units_for_division(measured, illuminant)?;
    let points = divide_over_intersection(measured, illuminant, step_nm);
    Ok(SpectralCurve::new(
        format!("{} (reflectance)", measured.name),
        CurveType::Reflectance,
    )
    .with_points(points)
    .with_quantity(QuantityKind::Reflectance))
}

/// `Predicted_measured(λ) = Reflectance(λ) × I_new(λ)` (§5.3.2): the same
/// physical relationship run forward, over the intersection of the
/// reflectance curve's own (measured) domain and the new illuminant's
/// domain - outside the reflectance curve's measured range, its value
/// isn't known, so it isn't extrapolated.
pub fn predict_under_illuminant(
    reflectance: &SpectralCurve,
    new_illuminant: &SpectralCurve,
    step_nm: f64,
) -> SpectralCurve {
    let (Some((r_lo, r_hi)), Some((i_lo, i_hi))) = (reflectance.domain(), new_illuminant.domain())
    else {
        return SpectralCurve::new(
            format!("{} (predicted)", reflectance.name),
            CurveType::Other("Predicted".to_string()),
        );
    };
    let lo = r_lo.max(i_lo);
    let hi = r_hi.min(i_hi);
    let r_interp = reflectance.interpolant();
    let i_interp = new_illuminant.interpolant();
    let points = if hi <= lo {
        Vec::new()
    } else {
        let n = ((hi - lo) / step_nm).round().max(1.0) as usize;
        (0..=n)
            .filter_map(|i| {
                let wl = lo + i as f64 * step_nm;
                let r = r_interp.value_at(wl)?;
                let illum = i_interp.value_at(wl)?;
                Some((wl, r * illum))
            })
            .collect()
    };
    let quantity = new_illuminant.quantity.clone();
    let mut curve = SpectralCurve::new(
        format!("{} (predicted)", reflectance.name),
        CurveType::Other("Predicted".to_string()),
    )
    .with_points(points)
    .with_quantity(quantity);
    curve.metadata.insert(
        "derived".to_string(),
        format!(
            "reflectance of \"{}\" under illuminant \"{}\"",
            reflectance.name, new_illuminant.name
        ),
    );
    curve
}

/// `curve(λ) × absorption(λ)^weight` over `curve`'s own domain - e.g.
/// atmospheric notches applied to a luminant. Outside `absorption`'s
/// domain nothing is absorbed (factor 1). `weight` scales the absorber's
/// strength the way path length or concentration does (Beer-Lambert):
/// 0 removes it, 1 applies it as-is, 2 is like passing through it twice.
///
/// Samples at every `step_nm` across `curve`'s domain, plus at every
/// point either curve defines there, so a notch narrower than `step_nm`
/// still shows up instead of falling between samples.
pub fn apply_absorption(
    curve: &SpectralCurve,
    absorption: &SpectralCurve,
    weight: f64,
    step_nm: f64,
) -> SpectralCurve {
    let mut out = curve.clone();
    let Some((lo, hi)) = curve.domain() else {
        return out;
    };
    let mut grid: Vec<f64> = Vec::new();
    let n = ((hi - lo) / step_nm).floor() as usize;
    grid.extend((0..=n).map(|i| lo + i as f64 * step_nm));
    grid.push(hi);
    grid.extend(curve.points.iter().map(|p| p.0));
    grid.extend(
        absorption
            .points
            .iter()
            .map(|p| p.0)
            .filter(|&wl| (lo..=hi).contains(&wl)),
    );
    grid.sort_by(|a, b| a.partial_cmp(b).unwrap());
    grid.dedup_by(|a, b| (*a - *b).abs() < 1e-9);

    let c = curve.interpolant();
    let a = absorption.interpolant();
    out.points = grid
        .into_iter()
        .filter_map(|wl| {
            let v = c.value_at(wl)?;
            let t = a.value_at(wl).unwrap_or(1.0).clamp(0.0, 1.0);
            Some((wl, v * t.powf(weight)))
        })
        .collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(name: &str, lo: f64, hi: f64, v: f64) -> SpectralCurve {
        SpectralCurve::new(name, CurveType::Other(String::new()))
            .with_points(vec![(lo, v), (hi, v)])
    }

    #[test]
    fn absorption_multiplies_with_weight_exponent() {
        let luminant = flat("lum", 400.0, 700.0, 10.0);
        let absorber = flat("abs", 500.0, 600.0, 0.5).with_quantity(QuantityKind::Absorption);
        for (w, expected) in [(0.0, 10.0), (1.0, 5.0), (2.0, 2.5)] {
            let out = apply_absorption(&luminant, &absorber, w, 10.0);
            assert!(
                (out.value_at(550.0).unwrap() - expected).abs() < 1e-9,
                "w={w}"
            );
            // Outside the absorber's domain: untouched.
            assert!((out.value_at(450.0).unwrap() - 10.0).abs() < 1e-9);
            assert_eq!(out.domain(), luminant.domain());
        }
    }

    #[test]
    fn absorption_narrower_than_step_is_not_lost() {
        let luminant = flat("lum", 400.0, 700.0, 1.0);
        let notch = SpectralCurve::new("n", CurveType::Other(String::new())).with_points(vec![
            (549.9, 1.0),
            (550.03, 0.2),
            (550.1, 1.0),
        ]);
        let out = apply_absorption(&luminant, &notch, 1.0, 10.0);
        assert!((out.value_at(550.03).unwrap() - 0.2).abs() < 1e-9);
    }

    fn radiance_curve(name: &str, unit: &str, points: Vec<(f64, f64)>) -> SpectralCurve {
        SpectralCurve::new(name, CurveType::Illumination)
            .with_points(points)
            .with_quantity(QuantityKind::Radiance {
                unit: unit.to_string(),
            })
    }

    #[test]
    fn unspecified_quantity_is_blocked() {
        let m = SpectralCurve::new("m", CurveType::Reflectance)
            .with_points(vec![(400.0, 1.0), (700.0, 1.0)]);
        let i = radiance_curve("i", "W.m-2.nm-1", vec![(400.0, 1.0), (700.0, 1.0)]);
        let err = check_units_for_division(&m, &i).unwrap_err();
        assert!(matches!(err, ReflectanceError::MissingQuantityKind { .. }));
    }

    #[test]
    fn already_dimensionless_kind_is_blocked() {
        let m = SpectralCurve::new("m", CurveType::Reflectance)
            .with_points(vec![(400.0, 1.0), (700.0, 1.0)])
            .with_quantity(QuantityKind::Reflectance);
        let i = radiance_curve("i", "W.m-2.nm-1", vec![(400.0, 1.0), (700.0, 1.0)]);
        let err = check_units_for_division(&m, &i).unwrap_err();
        assert!(matches!(err, ReflectanceError::WrongQuantityKind { .. }));
    }

    #[test]
    fn mismatched_units_are_blocked() {
        let m = radiance_curve("m", "W.m-2.nm-1", vec![(400.0, 1.0), (700.0, 1.0)]);
        let i = radiance_curve("i", "mW.cm-2.nm-1", vec![(400.0, 1.0), (700.0, 1.0)]);
        let err = check_units_for_division(&m, &i).unwrap_err();
        assert!(matches!(err, ReflectanceError::MismatchedUnits(_)));
    }

    #[test]
    fn matching_units_pass() {
        let m = radiance_curve("m", "W.m-2.nm-1", vec![(400.0, 1.0), (700.0, 1.0)]);
        let i = radiance_curve("i", "W.m-2.nm-1", vec![(400.0, 1.0), (700.0, 1.0)]);
        assert!(check_units_for_division(&m, &i).is_ok());
    }

    #[test]
    fn reflectance_is_hand_calculable_for_flat_curves() {
        // M=6 flat, I=3 flat over [400,700] -> Reflectance=2 everywhere.
        let m = radiance_curve("m", "u", vec![(400.0, 6.0), (700.0, 6.0)]);
        let i = radiance_curve("i", "u", vec![(400.0, 3.0), (700.0, 3.0)]);
        let r = derive_reflectance(&m, &i, 10.0).unwrap();
        assert_eq!(r.quantity, QuantityKind::Reflectance);
        assert_eq!(r.curve_type, CurveType::Reflectance);
        for &(_, v) in &r.points {
            assert!((v - 2.0).abs() < 1e-9);
        }
    }

    #[test]
    fn reflectance_derivation_is_blocked_on_bad_units() {
        let m = SpectralCurve::new("m", CurveType::Illumination).with_points(vec![(400.0, 1.0)]);
        let i = radiance_curve("i", "u", vec![(400.0, 1.0)]);
        assert!(derive_reflectance(&m, &i, 10.0).is_err());
    }

    #[test]
    fn forward_and_back_round_trip() {
        // Measure M under I1, derive reflectance, predict under I2,
        // confirm Predicted = Reflectance * I2 at a sample point.
        let i1 = radiance_curve("i1", "u", vec![(400.0, 2.0), (700.0, 4.0)]);
        let m = radiance_curve("m", "u", vec![(400.0, 1.0), (700.0, 2.0)]); // reflectance 0.5 throughout
        let reflectance = derive_reflectance(&m, &i1, 10.0).unwrap();

        let i2 = radiance_curve("i2", "v", vec![(400.0, 10.0), (700.0, 10.0)]);
        let predicted = predict_under_illuminant(&reflectance, &i2, 10.0);
        for &(wl, v) in &predicted.points {
            let r = reflectance.value_at(wl).unwrap();
            let illum = i2.value_at(wl).unwrap();
            assert!((v - r * illum).abs() < 1e-9);
        }
        // Known numerically: reflectance ~0.5 everywhere here, I2=10 -> predicted ~5.
        for &(_, v) in &predicted.points {
            assert!((v - 5.0).abs() < 0.1, "v={v}");
        }
    }

    #[test]
    fn division_by_near_zero_denominator_is_skipped_not_nan() {
        let m = radiance_curve("m", "u", vec![(400.0, 1.0), (700.0, 1.0)]);
        let i = radiance_curve("i", "u", vec![(400.0, 0.0), (700.0, 1.0)]);
        let r = derive_reflectance(&m, &i, 10.0).unwrap();
        assert!(r.points.iter().all(|&(_, v)| v.is_finite()));
        assert!(
            !r.points.iter().any(|&(wl, _)| wl == 400.0),
            "zero-denominator point should be skipped"
        );
    }

    /// End-to-end exercise of Phase 5's actual exit criteria, not just
    /// each piece in isolation: generate a 5778K black body with
    /// atmospheric notches, use it as the measuring illuminant for a
    /// synthetic "measured" curve, back-derive reflectance, then predict
    /// that reflectance's appearance under a generated composite LED
    /// illuminant - checking the result is numerically sane (finite,
    /// non-negative-illuminant-driven sign, roughly the expected
    /// magnitude) end to end.
    #[test]
    fn full_phase5_workflow_blackbody_to_reflectance_to_led_prediction() {
        use crate::blackbody;
        use crate::curve::QuantityKind;
        use crate::narrowband;

        let notches: Vec<_> = blackbody::builtin_default_notches()
            .into_iter()
            .map(|n| n.notch)
            .collect();
        let mut sun = blackbody::generate_blackbody_curve(5778.0, 380.0, 780.0, 2.0, &notches);
        sun.quantity = QuantityKind::Radiance {
            unit: "relative".to_string(),
        };

        // A synthetic measured curve: half of the illuminant's own
        // shape, so reflectance should come out ~0.5 everywhere.
        let measured_points: Vec<(f64, f64)> =
            sun.points.iter().map(|&(wl, v)| (wl, v * 0.5)).collect();
        let mut measured =
            SpectralCurve::new("measured", CurveType::Illumination).with_points(measured_points);
        measured.quantity = QuantityKind::Radiance {
            unit: "relative".to_string(),
        };

        let reflectance =
            derive_reflectance(&measured, &sun, 2.0).expect("units match by construction");
        assert!(!reflectance.points.is_empty());
        for &(_, v) in &reflectance.points {
            assert!(
                (v - 0.5).abs() < 1e-6,
                "reflectance should be ~0.5, got {v}"
            );
        }

        let cool_white = &narrowband::builtin_default_sources()[3]; // "Cool white LED"
        assert_eq!(cool_white.name, "Cool white LED");
        let led_components: Vec<(narrowband::GaussianComponent, f64)> = cool_white
            .components
            .iter()
            .cloned()
            .map(|c| (c, 1.0))
            .collect();
        let led = narrowband::generate_composite_curve(&led_components, 380.0, 780.0, 2.0);

        let predicted = predict_under_illuminant(&reflectance, &led, 2.0);
        assert!(!predicted.points.is_empty());
        assert!(predicted
            .points
            .iter()
            .all(|&(_, v)| v.is_finite() && v >= 0.0));
        // Reflectance ~0.5 everywhere -> predicted should be ~half the LED curve's own shape.
        for &(wl, v) in &predicted.points {
            let led_v = led.value_at(wl).unwrap();
            assert!(
                (v - 0.5 * led_v).abs() < 1e-6,
                "wl={wl} v={v} led_v={led_v}"
            );
        }
    }
}
