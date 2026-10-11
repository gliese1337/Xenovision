//! A reusable rectangle/polygon region selector over an image, shared by
//! the Stimulus Editor (extracting a curve from a region) and the Image
//! Display window (rendering a region through a visual system).
//!
//! The widget draws a base texture (a band preview or a rendered channel),
//! lets the user drag a rectangle or click out a polygon, and keeps the
//! in-progress gesture in a [`RegionDraft`]. Coordinates are in cube pixel
//! space, matching `xenovision_core::hyperspectral::Region`.

use eframe::egui;
use xenovision_core::hyperspectral::Region;

/// Which shape the selector is currently drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    #[default]
    Rectangle,
    Polygon,
}

/// A region selection gesture's state: the committed region, the shape
/// mode, and any in-progress polygon vertices / drag anchor.
#[derive(Default)]
pub struct RegionDraft {
    pub mode: SelectionMode,
    pub region: Option<Region>,
    pub polygon_points: Vec<(f64, f64)>,
    drag_start: Option<egui::Pos2>,
}

impl RegionDraft {
    /// Clears the committed region and any in-progress gesture.
    pub fn clear(&mut self) {
        self.region = None;
        self.polygon_points.clear();
        self.drag_start = None;
    }
}

/// Maps a screen position inside `rect` to cube pixel coordinates. `samples`/
/// `lines` are clamped to at least 1 so a degenerate (empty) image can't
/// divide by zero.
fn pixel_coords(p: egui::Pos2, rect: egui::Rect, samples: usize, lines: usize) -> (f64, f64) {
    let samples_f = samples.max(1) as f64;
    let lines_f = lines.max(1) as f64;
    let rel_x = ((p.x - rect.min.x) / rect.width().max(1.0)) as f64;
    let rel_y = ((p.y - rect.min.y) / rect.height().max(1.0)) as f64;
    (rel_x * samples_f, rel_y * lines_f)
}

/// The inverse of [`pixel_coords`]: a cube pixel coordinate back to a screen
/// position inside `rect`.
fn screen_pos(x: f64, y: f64, rect: egui::Rect, samples: usize, lines: usize) -> egui::Pos2 {
    let samples_f = samples.max(1) as f64;
    let lines_f = lines.max(1) as f64;
    egui::pos2(
        rect.min.x + (x / samples_f) as f32 * rect.width(),
        rect.min.y + (y / lines_f) as f32 * rect.height(),
    )
}

/// Draws a rectangle/polygon region selector over `texture`. Returns the
/// image's response (for pixel-readout on hover/click).
pub fn region_selector(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    samples: usize,
    lines: usize,
    draft: &mut RegionDraft,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label("Selection shape:");
        ui.radio_value(&mut draft.mode, SelectionMode::Rectangle, "Rectangle");
        ui.radio_value(
            &mut draft.mode,
            SelectionMode::Polygon,
            "Polygon (click to add vertices)",
        );
    });

    let max_display_width = ui.available_width().min(640.0);
    let aspect = if samples == 0 {
        1.0
    } else {
        lines as f32 / samples as f32
    };
    let display_size = egui::vec2(max_display_width, (max_display_width * aspect).max(1.0));
    let (response, painter) = ui.allocate_painter(display_size, egui::Sense::click_and_drag());
    painter.image(
        texture.id(),
        response.rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    let rect = response.rect;
    let to_pixel = |p: egui::Pos2| -> (f64, f64) { pixel_coords(p, rect, samples, lines) };
    let to_screen = |x: f64, y: f64| -> egui::Pos2 { screen_pos(x, y, rect, samples, lines) };

    let green = egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(60, 220, 60));
    let yellow = egui::Stroke::new(2.0_f32, egui::Color32::YELLOW);

    match draft.mode {
        SelectionMode::Rectangle => {
            if response.drag_started() {
                draft.drag_start = response.interact_pointer_pos();
            }
            if let (true, Some(start)) = (response.dragged(), draft.drag_start) {
                if let Some(current) = response.interact_pointer_pos() {
                    painter.rect_stroke(egui::Rect::from_two_pos(start, current), 0.0, yellow);
                }
            }
            if response.drag_stopped() {
                if let (Some(start), Some(end)) =
                    (draft.drag_start, response.interact_pointer_pos())
                {
                    let (x0, y0) = to_pixel(start);
                    let (x1, y1) = to_pixel(end);
                    draft.region = Some(Region::Rectangle { x0, y0, x1, y1 });
                }
                draft.drag_start = None;
            }
            if let Some(Region::Rectangle { x0, y0, x1, y1 }) = &draft.region {
                painter.rect_stroke(
                    egui::Rect::from_two_pos(to_screen(*x0, *y0), to_screen(*x1, *y1)),
                    0.0,
                    green,
                );
            }
        }
        SelectionMode::Polygon => {
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    draft.polygon_points.push(to_pixel(pos));
                }
            }
            if draft.polygon_points.len() >= 2 {
                let screen_points: Vec<egui::Pos2> = draft
                    .polygon_points
                    .iter()
                    .map(|&(x, y)| to_screen(x, y))
                    .collect();
                painter.add(egui::Shape::line(screen_points, yellow));
            }
            if let Some(Region::Polygon(points)) = &draft.region {
                let screen_points: Vec<egui::Pos2> =
                    points.iter().map(|&(x, y)| to_screen(x, y)).collect();
                painter.add(egui::Shape::closed_line(screen_points, green));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        draft.polygon_points.len() >= 3,
                        egui::Button::new("Close polygon"),
                    )
                    .clicked()
                {
                    draft.region = Some(Region::Polygon(std::mem::take(&mut draft.polygon_points)));
                }
                if ui.button("Clear points").clicked() {
                    draft.polygon_points.clear();
                }
            });
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 100.0))
    }

    #[test]
    fn pixel_coords_map_corners_and_centre() {
        let rect = sample_rect();
        let (samples, lines) = (100usize, 50usize);
        assert_eq!(
            pixel_coords(egui::pos2(10.0, 20.0), rect, samples, lines),
            (0.0, 0.0)
        );
        assert_eq!(
            pixel_coords(egui::pos2(210.0, 120.0), rect, samples, lines),
            (100.0, 50.0)
        );
        assert_eq!(
            pixel_coords(egui::pos2(110.0, 70.0), rect, samples, lines),
            (50.0, 25.0)
        );
    }

    #[test]
    fn screen_pos_inverts_pixel_coords() {
        let rect = sample_rect();
        let (samples, lines) = (640usize, 480usize);
        for &(x, y) in &[(0.0, 0.0), (123.0, 456.0), (640.0, 480.0), (320.0, 240.0)] {
            let p = screen_pos(x, y, rect, samples, lines);
            let (rx, ry) = pixel_coords(p, rect, samples, lines);
            assert!((rx - x).abs() < 1e-4, "x round-trip: {rx} vs {x}");
            assert!((ry - y).abs() < 1e-4, "y round-trip: {ry} vs {y}");
        }
    }

    #[test]
    fn degenerate_dimensions_do_not_divide_by_zero() {
        let rect = sample_rect();
        let (x, y) = pixel_coords(egui::pos2(110.0, 70.0), rect, 0, 0);
        assert!(x.is_finite() && y.is_finite());
        let p = screen_pos(x, y, rect, 0, 0);
        assert!(p.x.is_finite() && p.y.is_finite());
    }

    #[test]
    fn clear_resets_region_and_gesture() {
        let mut draft = RegionDraft {
            mode: SelectionMode::Polygon,
            region: Some(Region::Polygon(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)])),
            polygon_points: vec![(1.0, 1.0), (2.0, 2.0)],
            drag_start: Some(egui::pos2(1.0, 2.0)),
        };
        draft.clear();
        assert!(draft.region.is_none(), "committed region cleared");
        assert!(
            draft.polygon_points.is_empty(),
            "in-progress points cleared"
        );
        assert!(draft.drag_start.is_none(), "drag anchor cleared");
    }
}
