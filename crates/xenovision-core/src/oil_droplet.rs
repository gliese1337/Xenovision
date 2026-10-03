//! Oil droplet transmittance curve generation (design doc §2.3.5): the
//! sharp short-wavelength cut-off filters pigeon (and other birds')
//! single cones carry in front of their opsins, modeled as a sigmoid
//! transition centered at each droplet's documented 50%-transmission
//! wavelength, plus pointwise compositing with an opsin curve to get the
//! effective sensitivity curve the perceptual pipeline actually uses.

use crate::curve::SpectralCurve;

/// How many nm the sigmoid transition occupies. The design doc specifies
/// 50%-transmission cutoff *wavelengths* but not transition steepness
/// (no source for that was located) - this is an illustrative default,
/// not calibrated to a specific measurement.
pub const DEFAULT_STEEPNESS_NM: f64 = 8.0;

/// Transmittance at `wavelength_nm` for an oil droplet with a
/// 50%-transmission cutoff at `cutoff_nm`: a sigmoid that blocks
/// (transmittance -> 0) well below the cutoff and passes (-> 1) well
/// above it, crossing exactly 0.5 at the cutoff itself.
pub fn transmittance_at(wavelength_nm: f64, cutoff_nm: f64, steepness_nm: f64) -> f64 {
    1.0 / (1.0 + (-(wavelength_nm - cutoff_nm) / steepness_nm).exp())
}

/// Generates a transmittance curve's points over `[wl_min, wl_max]`.
pub fn generate_points(
    cutoff_nm: f64,
    steepness_nm: f64,
    wl_min: f64,
    wl_max: f64,
    step_nm: f64,
) -> Vec<(f64, f64)> {
    let n = ((wl_max - wl_min) / step_nm).round().max(1.0) as usize;
    (0..=n)
        .map(|i| {
            let wl = wl_min + i as f64 * step_nm;
            (wl, transmittance_at(wl, cutoff_nm, steepness_nm))
        })
        .collect()
}

/// Pointwise-multiplies an opsin absorption curve by an oil droplet
/// transmittance curve to get the effective sensitivity curve's points
/// (§2.3.5), over the intersection of their domains.
pub fn composite_sensitivity_points(
    opsin: &SpectralCurve,
    droplet: &SpectralCurve,
    step_nm: f64,
) -> Vec<(f64, f64)> {
    let (Some((o_lo, o_hi)), Some((d_lo, d_hi))) = (opsin.domain(), droplet.domain()) else {
        return Vec::new();
    };
    let lo = o_lo.max(d_lo);
    let hi = o_hi.min(d_hi);
    if hi <= lo {
        return Vec::new();
    }
    let oi = opsin.interpolant();
    let di = droplet.interpolant();
    let n = ((hi - lo) / step_nm).round().max(1.0) as usize;
    (0..=n)
        .filter_map(|i| {
            let wl = lo + i as f64 * step_nm;
            let o = oi.value_at(wl)?;
            let d = di.value_at(wl)?;
            Some((wl, o * d))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveType;

    #[test]
    fn transmittance_is_exactly_half_at_the_cutoff() {
        assert!((transmittance_at(500.0, 500.0, 8.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn transmittance_blocks_below_and_passes_above_cutoff() {
        let below = transmittance_at(400.0, 470.0, 8.0);
        let above = transmittance_at(540.0, 470.0, 8.0);
        assert!(below < 0.01, "below={below}");
        assert!(above > 0.99, "above={above}");
    }

    #[test]
    fn low_cutoff_is_near_transparent_across_visible_range() {
        // Models the "transparent/negligible" droplet paired with the
        // UV/violet cone - a cutoff far below the visible range should
        // leave transmittance ~1 everywhere actually used.
        for wl in [380.0, 450.0, 550.0, 650.0] {
            let t = transmittance_at(wl, 200.0, DEFAULT_STEEPNESS_NM);
            assert!(t > 0.999, "wl={wl} t={t}");
        }
    }

    #[test]
    fn composite_sensitivity_matches_hand_calculation_for_flat_curves() {
        let opsin = SpectralCurve::new("opsin", CurveType::Sensitivity)
            .with_points(vec![(400.0, 0.8), (600.0, 0.8)]);
        let droplet = SpectralCurve::new("droplet", CurveType::Transmittance)
            .with_points(vec![(400.0, 0.5), (600.0, 0.5)]);
        let points = composite_sensitivity_points(&opsin, &droplet, 10.0);
        assert!(!points.is_empty());
        for &(_, v) in &points {
            assert!((v - 0.4).abs() < 1e-9, "v={v}"); // 0.8 * 0.5
        }
    }

    #[test]
    fn composite_sensitivity_is_narrower_than_raw_opsin_with_a_real_cutoff() {
        // The qualitative property §2.3.5 actually cares about: an
        // effective curve = opsin x oil-droplet-cutoff should have its
        // short-wavelength tail suppressed relative to the raw opsin.
        let opsin_points = crate::govardovskii::generate_points(567.0, 300.0, 750.0, 1.0);
        let opsin =
            SpectralCurve::new("LWS opsin", CurveType::Sensitivity).with_points(opsin_points);
        let droplet_points = generate_points(560.0, DEFAULT_STEEPNESS_NM, 300.0, 750.0, 1.0);
        let droplet = SpectralCurve::new("orange droplet", CurveType::Transmittance)
            .with_points(droplet_points);
        let effective_points = composite_sensitivity_points(&opsin, &droplet, 1.0);
        let effective =
            SpectralCurve::new("effective", CurveType::Sensitivity).with_points(effective_points);

        let raw_at_500 = opsin.value_at(500.0).unwrap();
        let effective_at_500 = effective.value_at(500.0).unwrap();
        assert!(
            effective_at_500 < raw_at_500 * 0.5,
            "cutoff should suppress the short-wavelength tail"
        );
    }
}
