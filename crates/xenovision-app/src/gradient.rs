//! Thin egui-flavored wrapper around `xenovision_core::gradient` (the
//! canonical implementation, shared with §1.4.2's curve-coloring fallback).

use egui::Color32;

pub fn wavelength_to_color(wl: f64) -> Color32 {
    let (r, g, b) = xenovision_core::gradient::wavelength_to_color(wl);
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}
