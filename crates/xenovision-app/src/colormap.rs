//! Display color mappings for receptor-channel images.
//!
//! Two families, both of which the Image Display window must *state* on
//! screen (the "what false-color convention is being used" requirement):
//!
//! - A per-channel **colormap** (`Colormap`), applied to one scalar
//!   receptor channel normalized to 0..1. The default is a viridis-like
//!   perceptually-uniform ramp; grayscale and a single-hue blue ramp are
//!   the alternatives.
//! - A multi-channel **opponent composite** ([`perceptual_lab`]) for the
//!   perceptual stage's `[L, C1, C2]`: `L` drives lightness, `C1` the
//!   green↔magenta axis, `C2` the blue↔yellow axis. This is a deliberate
//!   false color (the channels are opponent contrasts, not measured
//!   sRGB), so the mapping is documented here and echoed in the UI legend.

use eframe::egui;

/// A per-channel scalar→color ramp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colormap {
    /// Perceptually-uniform dark-purple→teal→yellow (a compact viridis
    /// approximation, so low-end response differences stay legible).
    Viridis,
    /// Neutral black→white.
    Grayscale,
    /// Single-hue dark→bright blue.
    Blue,
}

impl Colormap {
    pub const ALL: [Colormap; 3] = [Colormap::Viridis, Colormap::Grayscale, Colormap::Blue];

    pub fn name(self) -> &'static str {
        match self {
            Colormap::Viridis => "Viridis",
            Colormap::Grayscale => "Grayscale",
            Colormap::Blue => "Blue",
        }
    }

    /// Maps a normalized value `t` (clamped to 0..1) to a color.
    pub fn apply(self, t: f32) -> egui::Color32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Colormap::Grayscale => {
                let g = (t * 255.0).round() as u8;
                egui::Color32::from_gray(g)
            }
            Colormap::Blue => {
                // Dark navy -> bright sky blue; never fully black, so a
                // zero-response pixel is still distinguishable from "no
                // data" in a region mask.
                let r = (20.0 + t * 180.0) as u8;
                let g = (30.0 + t * 190.0) as u8;
                let b = (60.0 + t * 195.0) as u8;
                egui::Color32::from_rgb(r, g, b)
            }
            Colormap::Viridis => {
                let (r, g, b) = viridis(t);
                egui::Color32::from_rgb(r, g, b)
            }
        }
    }

    /// A small swatch gradient for the on-screen legend.
    pub fn gradient(self, steps: usize) -> Vec<egui::Color32> {
        (0..steps.max(2))
            .map(|i| self.apply(i as f32 / (steps.max(2) - 1) as f32))
            .collect()
    }
}

/// Linear interpolation through five anchor points of the viridis
/// colormap (enough for a smooth-looking, monotone-lightness ramp).
fn viridis(t: f32) -> (u8, u8, u8) {
    const ANCHORS: [(f32, (f32, f32, f32)); 5] = [
        (0.00, (68.0, 1.0, 84.0)),
        (0.25, (59.0, 82.0, 139.0)),
        (0.50, (33.0, 145.0, 140.0)),
        (0.75, (94.0, 201.0, 98.0)),
        (1.00, (253.0, 231.0, 37.0)),
    ];
    let t = t.clamp(0.0, 1.0);
    let mut lo = ANCHORS[0];
    let mut hi = ANCHORS[ANCHORS.len() - 1];
    for w in ANCHORS.windows(2) {
        if t >= w[0].0 && t <= w[1].0 {
            lo = w[0];
            hi = w[1];
            break;
        }
    }
    let span = (hi.0 - lo.0).max(f32::MIN_POSITIVE);
    let f = (t - lo.0) / span;
    let mix = |a: f32, b: f32| (a + (b - a) * f).round().clamp(0.0, 255.0) as u8;
    (mix(lo.1 .0, hi.1 .0), mix(lo.1 .1, hi.1 .1), mix(lo.1 .2, hi.1 .2))
}

/// The perceptual stage's `[L, C1, C2]` → color map. `l` is lightness in
/// 0..1; `c1` is the green(−1)↔magenta/red(+1) opponent axis; `c2` is the
/// blue(−1)↔yellow(+1) opponent axis. Lightness sets the base, `c1`
/// splits red/green, `c2` adds to red+green and subtracts from blue (so
/// +C2 reads as yellow, −C2 as blue). Coefficients are chosen for visual
/// separation, not colorimetric accuracy - the UI labels this mapping.
pub fn perceptual_lab(l: f32, c1: f32, c2: f32) -> egui::Color32 {
    let l = l.clamp(0.0, 1.0);
    let c1 = c1.clamp(-1.0, 1.0);
    let c2 = c2.clamp(-1.0, 1.0);
    let r = l + 0.55 * c1 + 0.45 * c2;
    let g = l - 0.55 * c1 + 0.45 * c2;
    let b = l - 0.60 * c2;
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    egui::Color32::from_rgb(to_u8(r), to_u8(g), to_u8(b))
}

/// Normalizes a signed value against a `(min, max)` range to −1..1 (a
/// flat channel maps to 0). Used for the opponent axes; `L` uses
/// `normalize_unit` (0..1) instead.
pub fn normalize_opponent(v: f32, min: f32, max: f32) -> f32 {
    let span = (max - min).max(f32::MIN_POSITIVE);
    ((v - min) / span) * 2.0 - 1.0
}

/// Normalizes a value against a `(min, max)` range to 0..1 (a flat
/// channel maps to 0).
pub fn normalize_unit(v: f32, min: f32, max: f32) -> f32 {
    let span = (max - min).max(f32::MIN_POSITIVE);
    ((v - min) / span).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colormaps_are_monotone_at_the_endpoints() {
        for cm in Colormap::ALL {
            let lo = cm.apply(0.0);
            let hi = cm.apply(1.0);
            assert_ne!(lo, hi, "{cm:?} is flat across its range");
            // Grayscale is strictly increasing per channel.
            if cm == Colormap::Grayscale {
                assert!(lo.r() < hi.r());
            }
        }
    }

    #[test]
    fn apply_clamps_out_of_range_input() {
        let cm = Colormap::Viridis;
        assert_eq!(cm.apply(-5.0), cm.apply(0.0));
        assert_eq!(cm.apply(5.0), cm.apply(1.0));
    }

    #[test]
    fn perceptual_axes_map_in_the_documented_direction() {
        // +C2 should read as yellow (red+green high, blue low).
        let yellow = perceptual_lab(0.5, 0.0, 1.0);
        assert!(yellow.b() < yellow.r() && yellow.b() < yellow.g());
        // +C1 should push red over green.
        let red = perceptual_lab(0.5, 1.0, 0.0);
        assert!(red.r() > red.g());
    }
}
