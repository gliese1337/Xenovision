//! Shared wavelength/value <-> screen-space mapping, used by both the
//! single-curve editor and the multi-curve overlay so they can't drift
//! out of sync the way two separately-written mappings could.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect};
use xenovision_core::SpectralCurve;

/// Height in pixels of the spectrum reference bar beneath the x-axis.
pub const SPECTRUM_BAR_HEIGHT: f32 = 14.0;
/// Height in pixels of the numeric wavelength tick-label strip beneath
/// the spectrum bar.
pub const AXIS_LABEL_HEIGHT: f32 = 16.0;
/// Width in pixels of each gradient-bar strip. One `rect_filled` per
/// screen pixel is unnecessary for a smooth decorative gradient and was
/// reported to cause noticeable UI lag with several of these graphs
/// visible at once.
const GRADIENT_STRIP_PX: f32 = 3.0;

/// Splits a widget's full allocated rect into the plot area (curve line,
/// points, axes), a thin strip for the spectrum reference bar, and a
/// strip beneath that for numeric wavelength tick labels - instead of
/// washing the whole background with the gradient, which made it hard to
/// see the curve against it and (at per-pixel resolution) was expensive
/// to draw.
pub fn split_plot_bar_and_labels(rect: Rect) -> (Rect, Rect, Rect) {
    let reserved = SPECTRUM_BAR_HEIGHT + AXIS_LABEL_HEIGHT;
    let plot = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - reserved));
    let bar = Rect::from_min_max(
        Pos2::new(rect.min.x, rect.max.y - reserved),
        Pos2::new(rect.max.x, rect.max.y - AXIS_LABEL_HEIGHT),
    );
    let labels = Rect::from_min_max(
        Pos2::new(rect.min.x, rect.max.y - AXIS_LABEL_HEIGHT),
        rect.max,
    );
    (plot, bar, labels)
}

/// Chooses a "nice" tick step (1/2/2.5/5 x10^n) giving roughly
/// `target_ticks` ticks across `span`.
fn nice_tick_step(span: f64, target_ticks: f64) -> f64 {
    if !span.is_finite() || span <= 0.0 {
        return 100.0;
    }
    let raw = span / target_ticks.max(1.0);
    let magnitude = 10f64.powf(raw.log10().floor());
    for &c in &[1.0, 2.0, 2.5, 5.0, 10.0] {
        let step = c * magnitude;
        if span / step <= target_ticks {
            return step;
        }
    }
    10.0 * magnitude
}

/// Draws numeric wavelength tick labels (in nm) into `label_rect`, at a
/// "nice" step chosen from `axes`' current wavelength range - the only
/// numeric scale on the x-axis; the gradient bar above it is decorative.
pub fn draw_axis_labels(painter: &Painter, axes: &PlotAxes, label_rect: Rect, text_color: Color32) {
    let lo = axes.wl_min.min(axes.wl_max);
    let hi = axes.wl_min.max(axes.wl_max);
    let step = nice_tick_step(hi - lo, 6.0);
    if step <= 0.0 {
        return;
    }
    let first = (lo / step).ceil() * step;
    let mut wl = first;
    while wl <= hi + step * 1e-6 {
        let x = axes.to_screen_x(wl);
        if x >= label_rect.left() - 1.0 && x <= label_rect.right() + 1.0 {
            painter.text(
                Pos2::new(x, label_rect.center().y),
                Align2::CENTER_CENTER,
                format!("{wl:.0}"),
                FontId::proportional(10.0),
                text_color,
            );
        }
        wl += step;
    }
}

/// Draws a second bar beneath the normal spectrum-gradient bar showing a
/// specific visual system's own "subjective spectrum": at each
/// wavelength, blends every given curve's own display color weighted by
/// that curve's response there (`value_at(wl) * weight`, weight being a
/// stand-in for "receptor density" - see the caller for why), then scales
/// the blended color's brightness by the total weighted response
/// relative to the strongest response seen across the sampled range.
/// This is deliberately NOT the physical wavelength->color mapping the
/// bar above it shows - the two are meant to be compared, since the gap
/// between them is a visual system's own perceptual bias against the
/// physical spectrum.
pub fn draw_subjective_spectrum_bar(
    painter: &Painter,
    axes: &PlotAxes,
    bar_rect: Rect,
    weighted_curves: &[(Color32, &SpectralCurve, f64)],
) {
    let n = (bar_rect.width() / GRADIENT_STRIP_PX).ceil().max(1.0) as usize;
    let mut samples: Vec<(f64, f64, f64, f64)> = Vec::with_capacity(n + 1);
    let mut max_response = 0.0_f64;
    for i in 0..=n {
        let wl = axes.frac_to_wl(i as f64 / n as f64);
        let mut r = 0.0;
        let mut g = 0.0;
        let mut b = 0.0;
        let mut total = 0.0;
        for (color, curve, weight) in weighted_curves {
            let height = curve.value_at(wl).unwrap_or(0.0).max(0.0);
            let response = height * weight.max(0.0);
            if response <= 0.0 {
                continue;
            }
            r += color.r() as f64 * response;
            g += color.g() as f64 * response;
            b += color.b() as f64 * response;
            total += response;
        }
        if total > 0.0 {
            samples.push((r / total, g / total, b / total, total));
            max_response = max_response.max(total);
        } else {
            samples.push((0.0, 0.0, 0.0, 0.0));
        }
    }

    for (i, &(r, g, b, total)) in samples.iter().enumerate() {
        let x0 = bar_rect.left() + i as f32 * GRADIENT_STRIP_PX;
        let brightness = if max_response > 0.0 {
            total / max_response
        } else {
            0.0
        };
        let color = Color32::from_rgb(
            (r * brightness).clamp(0.0, 255.0) as u8,
            (g * brightness).clamp(0.0, 255.0) as u8,
            (b * brightness).clamp(0.0, 255.0) as u8,
        );
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x0, bar_rect.top()),
                Pos2::new(x0 + GRADIENT_STRIP_PX, bar_rect.bottom()),
            ),
            0.0,
            color,
        );
    }
    painter.rect_stroke(bar_rect, 0.0, egui::Stroke::new(1.0_f32, Color32::GRAY));
}

/// Draws the fixed spectrum gradient bar (§1.4.1) into `bar_rect`, using
/// `axes`' horizontal wavelength mapping so it lines up with whatever is
/// plotted above it.
pub fn draw_spectrum_bar(painter: &Painter, axes: &PlotAxes, bar_rect: Rect) {
    let n = (bar_rect.width() / GRADIENT_STRIP_PX).ceil().max(1.0) as usize;
    for i in 0..n {
        let x0 = bar_rect.left() + i as f32 * GRADIENT_STRIP_PX;
        let wl = axes.frac_to_wl(i as f64 / n as f64);
        let color = crate::gradient::wavelength_to_color(wl);
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x0, bar_rect.top()),
                Pos2::new(x0 + GRADIENT_STRIP_PX, bar_rect.bottom()),
            ),
            0.0,
            color,
        );
    }
    painter.rect_stroke(bar_rect, 0.0, egui::Stroke::new(1.0_f32, Color32::GRAY));
}

/// Min/max of `values` expanded by `padding_fraction` of their span, with
/// a `min_span` floor (so a single point or a flat curve still gets a
/// sane viewing window instead of a degenerate zero-width range), falling
/// back to `fallback` when `values` is empty.
///
/// Used instead of a fixed axis range because curves in this app can have
/// wildly different magnitudes (normalized 0-1 sensitivity curves, but
/// also reflectance ratios that exceed 1, blackbody curves out to
/// thousands of nm, weighted sums of several Gaussian peaks) - a single
/// hardcoded range clips most of them.
pub fn auto_range(
    values: impl Iterator<Item = f64>,
    padding_fraction: f64,
    min_span: f64,
    fallback: (f64, f64),
) -> (f64, f64) {
    let (lo, hi) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
        (lo.min(v), hi.max(v))
    });
    if !lo.is_finite() || !hi.is_finite() {
        return fallback;
    }
    let span = (hi - lo).max(min_span);
    let center = (hi + lo) / 2.0;
    let half = (span / 2.0) * (1.0 + padding_fraction);
    (center - half, center + half)
}

/// Which physical quantity increases left-to-right across a plot's
/// x-axis. Wavelength and frequency run in opposite directions (ν = c/λ),
/// so "long wavelength on the left" and "high frequency on the left" are
/// the same statement, just said two ways - this makes which one a given
/// user wants an explicit, per-preference toggle (§4.1/§6 of the GUI
/// design doc) rather than a hardcoded choice.
///
/// `IncreasingFrequency` is the default so that a freshly-added toggle
/// changes nothing for existing users: it reduces to exactly the mapping
/// this app used before the toggle existed (long wavelength/low
/// frequency on the left, short wavelength/high frequency on the right),
/// which was itself added per explicit user request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisOrientation {
    /// Short wavelength (low frequency) on the left, long wavelength
    /// (high frequency) on the right - the "textbook spectrum" layout.
    IncreasingWavelength,
    /// Long wavelength (low frequency) on the left, short wavelength
    /// (high frequency) on the right. Default; matches this app's
    /// original (pre-toggle) hardcoded orientation exactly.
    #[default]
    IncreasingFrequency,
}

pub struct PlotAxes {
    pub rect: Rect,
    pub wl_min: f64,
    pub wl_max: f64,
    pub val_min: f64,
    pub val_max: f64,
    pub orientation: AxisOrientation,
}

impl PlotAxes {
    pub fn wl_to_frac(&self, wl: f64) -> f64 {
        let increasing = (wl - self.wl_min) / (self.wl_max - self.wl_min);
        match self.orientation {
            AxisOrientation::IncreasingWavelength => increasing,
            AxisOrientation::IncreasingFrequency => 1.0 - increasing,
        }
    }

    pub fn frac_to_wl(&self, frac: f64) -> f64 {
        match self.orientation {
            AxisOrientation::IncreasingWavelength => {
                self.wl_min + frac * (self.wl_max - self.wl_min)
            }
            AxisOrientation::IncreasingFrequency => {
                self.wl_max - frac * (self.wl_max - self.wl_min)
            }
        }
    }

    pub fn to_screen_x(&self, wl: f64) -> f32 {
        self.rect.left() + self.wl_to_frac(wl) as f32 * self.rect.width()
    }

    pub fn to_screen_y(&self, v: f64) -> f32 {
        self.rect.bottom()
            - ((v - self.val_min) / (self.val_max - self.val_min)) as f32 * self.rect.height()
    }

    pub fn to_screen(&self, wl: f64, v: f64) -> Pos2 {
        Pos2::new(self.to_screen_x(wl), self.to_screen_y(v))
    }

    pub fn screen_to_wl_value(&self, pos: Pos2) -> (f64, f64) {
        let frac = ((pos.x - self.rect.left()) / self.rect.width()) as f64;
        let wl = self.frac_to_wl(frac);
        let v = self.val_min
            + ((self.rect.bottom() - pos.y) / self.rect.height()) as f64
                * (self.val_max - self.val_min);
        (wl, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_values_use_fallback() {
        let (lo, hi) = auto_range(std::iter::empty(), 0.1, 1.0, (10.0, 20.0));
        assert_eq!((lo, hi), (10.0, 20.0));
    }

    #[test]
    fn no_padding_matches_hand_calculation() {
        // [0, 10], no padding, span 10 > min_span -> exactly [0, 10].
        let (lo, hi) = auto_range([0.0, 10.0].into_iter(), 0.0, 1.0, (0.0, 1.0));
        assert!((lo - 0.0).abs() < 1e-9);
        assert!((hi - 10.0).abs() < 1e-9);
    }

    #[test]
    fn padding_expands_symmetrically() {
        // [0, 10], 20% padding -> span 10*1.2=12, centered on 5 -> [-1, 11].
        let (lo, hi) = auto_range([0.0, 10.0].into_iter(), 0.2, 1.0, (0.0, 1.0));
        assert!((lo - (-1.0)).abs() < 1e-9, "lo={lo}");
        assert!((hi - 11.0).abs() < 1e-9, "hi={hi}");
    }

    #[test]
    fn single_point_gets_min_span_window() {
        // One value (5.0): span floors to min_span=4.0, centered on 5 -> [3, 7].
        let (lo, hi) = auto_range([5.0].into_iter(), 0.0, 4.0, (0.0, 1.0));
        assert!((lo - 3.0).abs() < 1e-9, "lo={lo}");
        assert!((hi - 7.0).abs() < 1e-9, "hi={hi}");
    }

    #[test]
    fn default_orientation_matches_old_hardcoded_formula() {
        // Must stay numerically identical to the pre-toggle formula this
        // replaced: `1.0 - (wl - wl_min) / (wl_max - wl_min)`.
        let axes = PlotAxes {
            rect: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0)),
            wl_min: 300.0,
            wl_max: 800.0,
            val_min: 0.0,
            val_max: 1.0,
            orientation: AxisOrientation::default(),
        };
        assert_eq!(axes.orientation, AxisOrientation::IncreasingFrequency);
        for wl in [300.0, 420.0, 550.0, 700.0, 800.0] {
            let old = 1.0 - (wl - axes.wl_min) / (axes.wl_max - axes.wl_min);
            assert!(
                (axes.wl_to_frac(wl) - old).abs() < 1e-12,
                "wl={wl}: new={}, old={old}",
                axes.wl_to_frac(wl)
            );
        }
    }

    #[test]
    fn increasing_wavelength_is_the_mirror_image() {
        let base = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let freq = PlotAxes {
            rect: base,
            wl_min: 300.0,
            wl_max: 800.0,
            val_min: 0.0,
            val_max: 1.0,
            orientation: AxisOrientation::IncreasingFrequency,
        };
        let wave = PlotAxes {
            rect: base,
            wl_min: 300.0,
            wl_max: 800.0,
            val_min: 0.0,
            val_max: 1.0,
            orientation: AxisOrientation::IncreasingWavelength,
        };
        for wl in [300.0, 420.0, 550.0, 700.0, 800.0] {
            assert!((freq.wl_to_frac(wl) - (1.0 - wave.wl_to_frac(wl))).abs() < 1e-12);
        }
        // Round-trips in both orientations.
        for frac in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!((freq.wl_to_frac(freq.frac_to_wl(frac)) - frac).abs() < 1e-9);
            assert!((wave.wl_to_frac(wave.frac_to_wl(frac)) - frac).abs() < 1e-9);
        }
    }

    #[test]
    fn exceeds_1_1_reflectance_style_range_is_not_clipped() {
        // The axis range is derived from the data, not a hardcoded max -
        // values up to 1.25 must not be clipped.
        let (lo, hi) = auto_range([0.5, 1.25, 0.75].into_iter(), 0.15, 0.1, (-0.05, 1.1));
        assert!(hi > 1.25, "hi={hi} must exceed the data max");
        assert!(lo < 0.5, "lo={lo} must be below the data min");
    }
}
