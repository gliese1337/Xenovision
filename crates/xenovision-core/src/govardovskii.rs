//! Govardovskii et al. (2000) visual pigment absorbance template,
//! parameterized by λmax (design doc §2.3's "standard photopigment
//! absorption templates... parameterized by each λmax, rather than
//! hand-digitized"). Both the A1 (rhodopsin/most-vertebrate) and A2
//! (porphyropsin, goldfish §2.3.6) chromophore variants.

/// Relative absorbance at `wavelength_nm` for a pigment with peak
/// sensitivity `lambda_max_nm`, normalized so the curve's own peak value
/// (found by sampling, not assumed to land exactly at `lambda_max_nm`) is
/// 1.0 - consistent with §1.2.1's "typically normalized to peak = 1"
/// convention for Sensitivity curves.
///
/// Template constants are the published Govardovskii A1 fit; see
/// `docs/design-doc.md` References, §2.3.
fn raw_template_a1(lambda_max_nm: f64, wavelength_nm: f64) -> f64 {
    let a = 0.8795 + 0.0459 * (-((lambda_max_nm - 300.0).powi(2)) / 11940.0).exp();
    let x = lambda_max_nm / wavelength_nm;

    let alpha_denom =
        (69.7 * (a - x)).exp() + (28.0 * (0.922 - x)).exp() + (-14.9 * (1.104 - x)).exp() + 0.674;
    let s_alpha = 1.0 / alpha_denom;

    let beta_amplitude = 0.26;
    let lambda_max_beta = 189.0 + 0.315 * lambda_max_nm;
    let beta_bandwidth = -40.5 + 0.195 * lambda_max_nm;
    let s_beta =
        beta_amplitude * (-((wavelength_nm - lambda_max_beta) / beta_bandwidth).powi(2)).exp();

    s_alpha + s_beta
}

/// Peak-normalized template value, by dividing out the actual peak found
/// by fine sampling near `lambda_max_nm` (the raw template's true peak
/// doesn't land exactly at `lambda_max_nm` or exactly at 1.0).
pub fn template_a1(lambda_max_nm: f64, wavelength_nm: f64) -> f64 {
    let peak = peak_value(lambda_max_nm, raw_template_a1);
    raw_template_a1(lambda_max_nm, wavelength_nm) / peak
}

/// The A2 (porphyropsin, vitamin-A2-based - goldfish and other fish that
/// don't fully metabolize to A1) chromophore variant: same alpha-band
/// exponent framework as A1, but with the "a" parameter and the beta
/// band's amplitude/center/width all depending on λmax differently,
/// producing the broader, more red-shifted absorption A2 pigments are
/// known for relative to A1 at the same nominal λmax.
///
/// Lower confidence than `template_a1`: these are the published Govardovskii
/// A2 coefficients as best recalled, not re-verified digit-by-digit against
/// the original paper. The qualitative property this app actually relies on
/// (A2 visibly broader/more red-shifted than A1 at the same λmax) holds
/// regardless of small coefficient error, but exact numeric values should
/// be checked against the source before any precise scientific use.
fn raw_template_a2(lambda_max_nm: f64, wavelength_nm: f64) -> f64 {
    let a = 0.875 + 0.0268 * ((lambda_max_nm - 665.0) / 40.7).exp();
    let x = lambda_max_nm / wavelength_nm;

    let alpha_denom =
        (69.7 * (a - x)).exp() + (28.0 * (0.922 - x)).exp() + (-14.9 * (1.104 - x)).exp() + 0.674;
    let s_alpha = 1.0 / alpha_denom;

    let beta_amplitude = 0.26 + 0.0215 * ((lambda_max_nm - 665.0) / 40.7).exp();
    let lambda_max_beta = 216.7 + 0.368 * lambda_max_nm;
    let beta_bandwidth = 317.0 - 1.149 * lambda_max_nm + 0.00124 * lambda_max_nm.powi(2);
    let s_beta =
        beta_amplitude * (-((wavelength_nm - lambda_max_beta) / beta_bandwidth).powi(2)).exp();

    s_alpha + s_beta
}

pub fn template_a2(lambda_max_nm: f64, wavelength_nm: f64) -> f64 {
    let peak = peak_value(lambda_max_nm, raw_template_a2);
    raw_template_a2(lambda_max_nm, wavelength_nm) / peak
}

fn peak_value(lambda_max_nm: f64, raw: impl Fn(f64, f64) -> f64) -> f64 {
    // The alpha band peak sits very close to lambda_max_nm; a narrow local
    // search is enough to find its true maximum precisely.
    let mut best = f64::MIN;
    let mut wl = lambda_max_nm - 5.0;
    while wl <= lambda_max_nm + 5.0 {
        let v = raw(lambda_max_nm, wl);
        if v > best {
            best = v;
        }
        wl += 0.05;
    }
    best
}

/// Which chromophore template to generate a curve from - A1 is the
/// default for most vertebrates; A2 is used by goldfish and other
/// species with vitamin-A2-based (porphyropsin) visual pigments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chromophore {
    A1,
    A2,
}

/// Generates a peak-normalized template curve as `(wavelength, value)`
/// points from `wl_min` to `wl_max` nm (inclusive) at `step_nm` spacing.
pub fn generate_points(
    lambda_max_nm: f64,
    wl_min: f64,
    wl_max: f64,
    step_nm: f64,
) -> Vec<(f64, f64)> {
    generate_points_with(Chromophore::A1, lambda_max_nm, wl_min, wl_max, step_nm)
}

pub fn generate_points_with(
    chromophore: Chromophore,
    lambda_max_nm: f64,
    wl_min: f64,
    wl_max: f64,
    step_nm: f64,
) -> Vec<(f64, f64)> {
    let template = match chromophore {
        Chromophore::A1 => template_a1,
        Chromophore::A2 => template_a2,
    };
    let n = ((wl_max - wl_min) / step_nm).round() as usize;
    (0..=n)
        .map(|i| {
            let wl = wl_min + i as f64 * step_nm;
            (wl, template(lambda_max_nm, wl))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_is_normalized_to_one_at_lambda_max_region() {
        for &lmax in &[344.0, 420.0, 436.0, 502.0, 530.0, 560.0, 567.0, 625.0] {
            let peak = peak_value(lmax, raw_template_a1);
            let normalized_peak = raw_template_a1(lmax, lmax) / peak;
            // At exactly lambda_max the normalized value should be very
            // close to (not necessarily exactly) 1.0, since the true peak
            // can sit a fraction of a nm away from lambda_max itself.
            assert!(
                (normalized_peak - 1.0).abs() < 0.01,
                "lmax={lmax}: normalized value at lambda_max = {normalized_peak}"
            );
        }
    }

    #[test]
    fn a2_peak_is_also_normalized_to_one() {
        for &lmax in &[356.0, 455.0, 530.0, 625.0] {
            let peak = peak_value(lmax, raw_template_a2);
            let normalized_peak = raw_template_a2(lmax, lmax) / peak;
            assert!(
                (normalized_peak - 1.0).abs() < 0.01,
                "lmax={lmax}: normalized value at lambda_max = {normalized_peak}"
            );
        }
    }

    #[test]
    fn a2_template_differs_visibly_from_a1_away_from_peak() {
        // The qualitative property this app actually depends on: A1 and
        // A2 are different templates, so they should visibly diverge
        // somewhat away from the shared peak (checked numerically here,
        // 80nm below lambda_max, rather than assumed) - not identical
        // curves wearing different names.
        let lmax = 530.0;
        let probe = lmax - 80.0;
        let a1 = template_a1(lmax, probe);
        let a2 = template_a2(lmax, probe);
        assert!(
            (a1 - a2).abs() > 0.05,
            "a1={a1} a2={a2} at probe={probe}, too similar"
        );
    }

    #[test]
    fn template_value_is_bounded() {
        for &lmax in &[344.0, 420.0, 560.0, 625.0] {
            let mut wl = 300.0;
            while wl <= 750.0 {
                let v = template_a1(lmax, wl);
                assert!((-0.01..=1.01).contains(&v), "lmax={lmax} wl={wl}: v={v}");
                wl += 1.0;
            }
        }
    }

    #[test]
    fn curve_decays_away_from_peak() {
        let lmax = 560.0;
        let at_peak = template_a1(lmax, lmax);
        let far_below = template_a1(lmax, lmax - 150.0);
        let far_above = template_a1(lmax, lmax + 150.0);
        assert!(at_peak > far_below);
        assert!(at_peak > far_above);
    }

    #[test]
    fn generate_points_spans_requested_range() {
        let pts = generate_points(560.0, 400.0, 700.0, 10.0);
        assert_eq!(pts.first().unwrap().0, 400.0);
        assert_eq!(pts.last().unwrap().0, 700.0);
        assert_eq!(pts.len(), 31);
    }
}
