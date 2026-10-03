//! Fixed background spectrum gradient (design doc §1.4.1): visible rainbow
//! 380-700nm, IR fading dark-red-to-black above 700nm, UV fading
//! light-blue-to-white below 380nm. Not user-configurable.
//!
//! Lives in core (not the app crate) because §1.4.2's curve-coloring
//! fallback for out-of-CIE-gamut wavelengths explicitly reuses this same
//! logic (see `cie::curve_display_color`) - one implementation shared by
//! both the display band and that fallback, rather than two that could
//! drift apart.
//!
//! The visible-range mapping is the standard public-domain
//! wavelength-to-RGB approximation (Dan Bruton); it's a decorative
//! reference, not the CIE-based curve coloring itself.

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lerp_rgb(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    (lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t))
}

fn visible_rainbow(wl: f64) -> (f64, f64, f64) {
    let (mut r, mut g, mut b);
    if wl < 440.0 {
        r = -(wl - 440.0) / (440.0 - 380.0);
        g = 0.0;
        b = 1.0;
    } else if wl < 490.0 {
        r = 0.0;
        g = (wl - 440.0) / (490.0 - 440.0);
        b = 1.0;
    } else if wl < 510.0 {
        r = 0.0;
        g = 1.0;
        b = -(wl - 510.0) / (510.0 - 490.0);
    } else if wl < 580.0 {
        r = (wl - 510.0) / (580.0 - 510.0);
        g = 1.0;
        b = 0.0;
    } else if wl < 645.0 {
        r = 1.0;
        g = -(wl - 645.0) / (645.0 - 580.0);
        b = 0.0;
    } else {
        r = 1.0;
        g = 0.0;
        b = 0.0;
    }

    // Intensity taper near the low-wavelength edge of human visibility
    // (this function is only ever called for wl in [380, 700], so there's
    // no symmetric high-wavelength taper to apply here).
    let factor = if wl < 420.0 {
        (0.3 + 0.7 * (wl - 380.0) / (420.0 - 380.0)).clamp(0.3, 1.0)
    } else {
        1.0
    };

    r *= factor;
    g *= factor;
    b *= factor;

    (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
}

/// Color for the fixed background gradient band at a given wavelength
/// (nm), as linear-ish `(r, g, b)` in `0.0..=1.0` (not gamma-encoded -
/// this is a decorative approximation, not a colorimetric calculation).
pub fn wavelength_to_color(wl: f64) -> (f64, f64, f64) {
    const UV_FADE_LIMIT: f64 = 200.0;
    const IR_FADE_LIMIT: f64 = 1000.0;

    if wl < 380.0 {
        let t = ((380.0 - wl) / (380.0 - UV_FADE_LIMIT)).clamp(0.0, 1.0);
        // Anchor the fade at the visible band's own edge color (a dim
        // violet, per its intensity taper below 420nm) rather than an
        // unrelated constant, so there's no seam at the 380nm boundary -
        // the fade then passes through progressively lighter violets on
        // its way to white, as requested.
        let violet_edge = visible_rainbow(380.0);
        let white = (1.0, 1.0, 1.0);
        lerp_rgb(violet_edge, white, t)
    } else if wl > 700.0 {
        let t = ((wl - 700.0) / (IR_FADE_LIMIT - 700.0)).clamp(0.0, 1.0);
        // Same reasoning: anchor at the visible band's actual 700nm red
        // rather than an arbitrary darker constant, so it fades smoothly
        // through dark red on its way to black instead of jumping there.
        let red_edge = visible_rainbow(700.0);
        let black = (0.0, 0.0, 0.0);
        lerp_rgb(red_edge, black, t)
    } else {
        visible_rainbow(wl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_blue_dominates_around_450nm() {
        let (r, g, b) = wavelength_to_color(450.0);
        assert!(b > r);
        assert!(b > g);
    }

    #[test]
    fn green_dominates_around_550nm() {
        let (r, g, b) = wavelength_to_color(550.0);
        assert!(g > r);
        assert!(g > b);
    }

    #[test]
    fn red_dominates_around_650nm() {
        let (r, g, b) = wavelength_to_color(650.0);
        assert!(r > g);
        assert!(r > b);
    }

    #[test]
    fn fades_are_continuous_at_the_visible_band_boundaries() {
        // No seam at 380/700nm: approaching from either side must land on
        // (approximately) the same color, since both fades are anchored
        // at the visible band's own edge value.
        let just_above_380 = wavelength_to_color(380.0001);
        let just_below_380 = wavelength_to_color(379.9999);
        assert!((just_above_380.0 - just_below_380.0).abs() < 1e-3);
        assert!((just_above_380.1 - just_below_380.1).abs() < 1e-3);
        assert!((just_above_380.2 - just_below_380.2).abs() < 1e-3);

        let just_below_700 = wavelength_to_color(699.9999);
        let just_above_700 = wavelength_to_color(700.0001);
        assert!((just_below_700.0 - just_above_700.0).abs() < 1e-3);
        assert!((just_below_700.1 - just_above_700.1).abs() < 1e-3);
        assert!((just_below_700.2 - just_above_700.2).abs() < 1e-3);
    }

    #[test]
    fn uv_fades_toward_white_as_wavelength_decreases() {
        let near_380 = wavelength_to_color(379.0);
        let deep_uv = wavelength_to_color(250.0);
        let far_uv = wavelength_to_color(100.0);
        assert!(deep_uv.0 >= near_380.0);
        assert_eq!(far_uv, (1.0, 1.0, 1.0));
    }

    #[test]
    fn ir_fades_toward_black_as_wavelength_increases() {
        let near_700 = wavelength_to_color(701.0);
        let deep_ir = wavelength_to_color(900.0);
        let far_ir = wavelength_to_color(2000.0);
        assert!(deep_ir.0 <= near_700.0);
        assert_eq!(far_ir, (0.0, 0.0, 0.0));
    }
}
