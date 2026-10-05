//! CIE 1931 color-matching functions and curve display coloring (design
//! doc §1.4.2). This is a *display* concern only - it has no bearing on
//! the perceptual pipeline in §2, which operates on species receptor
//! curves, not CIE functions.
//!
//! Rather than transcribing the full tabulated CIE 1931 2-degree observer
//! data (easy to introduce silent transcription errors in, for a feature
//! that's cosmetic anyway), this uses the published analytic multi-Gaussian
//! fit from Wyman, Sloan & Shirley, "Simple Analytic Approximations to the
//! CIE XYZ Color Matching Functions" (JCGT 2013) - accurate to a few
//! percent, continuous at any wavelength, and easy to audit against the
//! paper's formula directly.

use crate::curve::SpectralCurve;
use crate::gradient;

/// Asymmetric Gaussian: `sigma1` on the left of `mu`, `sigma2` on the right.
fn asym_gaussian(x: f64, mu: f64, sigma1: f64, sigma2: f64) -> f64 {
    let sigma = if x < mu { sigma1 } else { sigma2 };
    (-0.5 * ((x - mu) / sigma).powi(2)).exp()
}

/// CIE 1931 2-degree x-bar, y-bar, z-bar at `wavelength_nm`, per the
/// Wyman/Sloan/Shirley fit.
pub fn color_matching(wavelength_nm: f64) -> (f64, f64, f64) {
    let wl = wavelength_nm;
    let x = 1.056 * asym_gaussian(wl, 599.8, 37.9, 31.0)
        + 0.362 * asym_gaussian(wl, 442.0, 16.0, 26.7)
        - 0.065 * asym_gaussian(wl, 501.1, 20.4, 26.2);
    let y =
        0.821 * asym_gaussian(wl, 568.8, 46.9, 40.5) + 0.286 * asym_gaussian(wl, 530.9, 16.3, 31.1);
    let z =
        1.217 * asym_gaussian(wl, 437.0, 11.8, 36.0) + 0.681 * asym_gaussian(wl, 459.0, 26.0, 13.8);
    (x, y, z)
}

/// The approximately CIE-gamut-defined wavelength range this fit is
/// meaningful over (design doc §1.4.2's "~360-830nm").
pub const CIE_GAMUT_RANGE: (f64, f64) = (360.0, 830.0);

/// Linear sRGB (D65) from CIE XYZ, no gamma encoding yet.
fn xyz_to_linear_srgb(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    let r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
    let g = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let b = 0.0557 * x - 0.2040 * y + 1.0570 * z;
    (r, g, b)
}

fn gamma_encode(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// XYZ -> gamma-encoded sRGB, normalized and clipped per §1.4.2's "sRGB
/// conversion with gamut clipping/normalization".
///
/// The raw convolution this feeds from (`curve_display_color`) has no
/// fixed luminance scale - it's a sum over however many wavelength
/// samples a curve's domain happens to span, not a properly-weighted
/// illuminant integral - so the resulting X/Y/Z magnitudes are typically
/// far outside the 0..1 range the sRGB matrix expects. Clamping each
/// channel to `0..1` *independently* at that scale is almost always
/// equivalent to picking whichever channels overflowed and maxing them
/// out together, which destroys hue (e.g. an all-channels-overflow case
/// renders as white regardless of the curve's actual color). Instead:
/// clip negative (out-of-gamut) components to 0, then rescale so the
/// brightest channel is exactly 1.0 - this preserves the relative
/// chromaticity (hue/saturation) that clamping was wiping out.
fn xyz_to_srgb(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    let (r, g, b) = xyz_to_linear_srgb(x, y, z);
    let (r, g, b) = (r.max(0.0), g.max(0.0), b.max(0.0));
    let peak = r.max(g).max(b);
    let (r, g, b) = if peak > 0.0 {
        (r / peak, g / peak, b / peak)
    } else {
        (0.0, 0.0, 0.0)
    };
    (
        gamma_encode(r.clamp(0.0, 1.0)),
        gamma_encode(g.clamp(0.0, 1.0)),
        gamma_encode(b.clamp(0.0, 1.0)),
    )
}

/// The display color for a curve (§1.4.2): the curve's own values, taken
/// as weights over wavelength, normalized to peak 1, convolved against the
/// CIE color-matching functions to get XYZ, then converted to sRGB.
///
/// For curve weight lying outside the CIE-defined range (`CIE_GAMUT_RANGE`),
/// the background-gradient's UV/IR color logic is used as a fallback
/// instead (§1.4.1/§1.4.2), blended in proportion to how much of the
/// curve's total weight falls outside that range - so a curve entirely
/// within the visible range gets a pure CIE color, a curve entirely
/// outside it (e.g. a deep-UV receptor) gets a pure gradient-fallback
/// color, and anything in between is a weighted mix of the two.
///
/// Returns `None` for a curve with no points or an all-zero/negative-only
/// curve (nothing to normalize against).
pub fn curve_display_color(curve: &SpectralCurve) -> Option<(f64, f64, f64)> {
    let domain = curve.domain()?;
    let interp = curve.interpolant();

    let peak = curve
        .points
        .iter()
        .map(|&(_, v)| v.abs())
        .fold(0.0_f64, f64::max);
    if peak <= 0.0 {
        return None;
    }

    let step = 1.0_f64;
    let (lo, hi) = domain;
    let n = ((hi - lo) / step).round().max(1.0) as usize;

    let mut xyz = (0.0, 0.0, 0.0);
    let mut in_gamut_weight = 0.0;
    let mut out_gamut_weight = 0.0;
    let mut out_gamut_color_acc = (0.0, 0.0, 0.0);

    let (gamut_lo, gamut_hi) = CIE_GAMUT_RANGE;

    for i in 0..=n {
        let wl = lo + i as f64 * step;
        let value = interp.value_at(wl).unwrap_or(0.0) / peak;
        let weight = value.abs();
        if wl < gamut_lo || wl > gamut_hi {
            out_gamut_weight += weight;
            let (r, g, b) = gradient::wavelength_to_color(wl);
            out_gamut_color_acc.0 += weight * r;
            out_gamut_color_acc.1 += weight * g;
            out_gamut_color_acc.2 += weight * b;
        } else {
            in_gamut_weight += weight;
            let (xb, yb, zb) = color_matching(wl);
            xyz.0 += value * xb;
            xyz.1 += value * yb;
            xyz.2 += value * zb;
        }
    }

    let total_weight = in_gamut_weight + out_gamut_weight;
    if total_weight <= 0.0 {
        return None;
    }

    let cie_color = if in_gamut_weight > 0.0 {
        xyz_to_srgb(xyz.0, xyz.1, xyz.2)
    } else {
        (0.0, 0.0, 0.0)
    };
    let fallback_color = if out_gamut_weight > 0.0 {
        (
            out_gamut_color_acc.0 / out_gamut_weight,
            out_gamut_color_acc.1 / out_gamut_weight,
            out_gamut_color_acc.2 / out_gamut_weight,
        )
    } else {
        (0.0, 0.0, 0.0)
    };

    let out_fraction = out_gamut_weight / total_weight;
    Some((
        cie_color.0 * (1.0 - out_fraction) + fallback_color.0 * out_fraction,
        cie_color.1 * (1.0 - out_fraction) + fallback_color.1 * out_fraction,
        cie_color.2 * (1.0 - out_fraction) + fallback_color.2 * out_fraction,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveType;

    fn narrow_curve(center: f64) -> SpectralCurve {
        let pts: Vec<(f64, f64)> = (0..=40)
            .map(|i| {
                let wl = center - 20.0 + i as f64;
                let v = (-0.5 * ((wl - center) / 8.0).powi(2)).exp();
                (wl, v)
            })
            .collect();
        SpectralCurve::new("test", CurveType::Sensitivity).with_points(pts)
    }

    #[test]
    fn red_curve_renders_reddish() {
        let (r, g, b) = curve_display_color(&narrow_curve(620.0)).unwrap();
        assert!(r > g, "r={r} g={g} b={b}");
        assert!(r > b, "r={r} g={g} b={b}");
    }

    #[test]
    fn green_curve_renders_greenish() {
        let (r, g, b) = curve_display_color(&narrow_curve(530.0)).unwrap();
        assert!(g > r, "r={r} g={g} b={b}");
        assert!(g > b, "r={r} g={g} b={b}");
    }

    #[test]
    fn blue_curve_renders_bluish() {
        let (r, g, b) = curve_display_color(&narrow_curve(460.0)).unwrap();
        assert!(b > r, "r={r} g={g} b={b}");
        assert!(b > g, "r={r} g={g} b={b}");
    }

    #[test]
    fn empty_curve_has_no_color() {
        let curve = SpectralCurve::new("empty", CurveType::Sensitivity);
        assert_eq!(curve_display_color(&curve), None);
    }

    #[test]
    fn deep_uv_curve_uses_gradient_fallback_not_black() {
        // Entirely below CIE_GAMUT_RANGE's low end -> should take the pure
        // fallback color (light-blue-ish), not collapse to black just
        // because the CIE fit has ~no support there.
        let curve = narrow_curve(300.0);
        let (r, g, b) = curve_display_color(&curve).unwrap();
        let (fr, fg, fb) = gradient::wavelength_to_color(300.0);
        assert!((r - fr).abs() < 0.05, "r={r} expected~{fr}");
        assert!((g - fg).abs() < 0.05, "g={g} expected~{fg}");
        assert!((b - fb).abs() < 0.05, "b={b} expected~{fb}");
    }

    #[test]
    fn wide_curve_does_not_wash_out_to_white_or_wrong_hue() {
        // A S-cone-shaped curve (Govardovskii template, lambda_max=420,
        // generated wide enough to span hundreds of samples): the raw
        // XYZ convolution here has no fixed luminance scale, so X and Z
        // can independently exceed 1.0. Clamping each sRGB channel to
        // 0..1 *before* checking their relative sizes would max out R
        // and B together regardless of the curve's actual (blue-ish)
        // shape. A 420nm receptor should render blue-dominated, not
        // magenta (R and B both maxed, G near zero) or white (all maxed).
        let curve = SpectralCurve::new("S-cone-like", CurveType::Sensitivity).with_points(
            crate::govardovskii::generate_points(420.0, 300.0, 750.0, 5.0),
        );
        let (r, g, b) = curve_display_color(&curve).unwrap();
        assert!(b > r, "expected blue-dominated, got r={r} g={g} b={b}");
        assert!(b > g, "expected blue-dominated, got r={r} g={g} b={b}");
        assert!(
            !(r > 0.9 && g < 0.3 && b > 0.9),
            "collapsed to magenta: r={r} g={g} b={b}"
        );
        assert!(
            !(r > 0.9 && g > 0.9 && b > 0.9),
            "collapsed to white: r={r} g={g} b={b}"
        );
    }

    #[test]
    fn color_matching_peaks_are_in_plausible_ranges() {
        // y-bar (luminosity-like) should peak somewhere in the green,
        // roughly where human photopic vision is most sensitive.
        let mut best_wl = 0.0;
        let mut best_y = -1.0;
        let mut wl = 400.0;
        while wl <= 700.0 {
            let (_, y, _) = color_matching(wl);
            if y > best_y {
                best_y = y;
                best_wl = wl;
            }
            wl += 1.0;
        }
        assert!(
            (500.0..=600.0).contains(&best_wl),
            "y-bar peak at {best_wl}"
        );
    }
}
