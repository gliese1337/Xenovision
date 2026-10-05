//! Interactive overlay of multiple curves on one graph: select and edit
//! any curve's points directly here, rather than requiring a separate
//! single-curve point editor plus a separate curve-picker row. Each
//! curve is colored by its own CIE-derived display color (same as the
//! read-only `multi_curve_view`); the legend row beneath is the *only*
//! way to change which curve is selected for editing - clicking or
//! dragging on the plot itself only ever acts on the already-selected
//! curve's own points, never retargets the selection: hit-testing every
//! curve's points instead (even though only the selected curve's points
//! are drawn, per the note below) would let a click near an invisible
//! point belonging to a different curve silently switch which curve
//! you're editing mid-drag.
//!
//! `selected_curve` is an `Option<usize>` rather than a plain index so
//! "no curve selected" is a representable state (clicking the
//! already-selected curve's legend entry toggles back to it) - with
//! nothing selected, no points are drawn or hit-testable at all.
//!
//! Interaction is handled *before* anything is drawn, specifically so a
//! drag's effect on a curve's shape - and therefore its CIE-derived
//! display color, which depends on that shape - is visible the same
//! frame the point moves, not one frame behind it.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use xenovision_core::cie;
use xenovision_core::SpectralCurve;

use crate::plot_axis::{self, AxisOrientation, PlotAxes};

const FALLBACK_WL_RANGE: (f64, f64) = (300.0, 800.0);
const FALLBACK_VAL_RANGE: (f64, f64) = (-0.05, 1.1);
const SANITY_WL_RANGE: (f64, f64) = (0.1, 1_000_000.0);
const SANITY_VAL_RANGE: (f64, f64) = (-1.0e6, 1.0e6);
const HANDLE_RADIUS: f32 = 5.0;
/// See `curve_editor::LINE_STEP_PX`.
const LINE_STEP_PX: f32 = 2.0;

#[cfg(test)]
const LAST_GRAPH_RECT_ID: &str = "multi_curve_editor_last_graph_rect";

/// The rect the most recently drawn graph was allocated - lets headless
/// layout tests measure it.
#[cfg(test)]
pub fn last_graph_rect(ctx: &egui::Context) -> Option<Rect> {
    ctx.data(|d| d.get_temp(egui::Id::new(LAST_GRAPH_RECT_ID)))
}

fn display_color(curve: &SpectralCurve) -> Color32 {
    cie::curve_display_color(curve)
        .map(|(r, g, b)| Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8))
        .unwrap_or(Color32::WHITE)
}

/// Draws every curve in `curves` overlaid on one graph, but only
/// handles point interaction - drag/click-to-select-a-point/right-click-
/// or-Delete-to-remove - for whichever curve `selected_curve` names, if
/// any. Switching *which* curve is selected (including to/from nothing
/// selected) only happens via the legend row beneath (also the target
/// for double-click-to-add-a-point, since a point added in empty space
/// can't by itself say which curve it belongs to) - never by
/// clicking/dragging on the plot itself.
///
/// Returns the `(wl_min, wl_max)` range actually used, so a caller that
/// wants to draw something else aligned to this same horizontal mapping
/// (e.g. Workspace's subjective-spectrum bar) doesn't have to duplicate
/// the auto-range computation and risk it drifting out of sync.
pub fn multi_curve_editor(
    ui: &mut Ui,
    curves: &mut [SpectralCurve],
    selected_curve: &mut Option<usize>,
    selected_point: &mut Option<usize>,
    orientation: AxisOrientation,
) -> (f64, f64) {
    if curves.is_empty() {
        return FALLBACK_WL_RANGE;
    }
    // Clamp a stale index (e.g. the selected curve was just removed)
    // back into range rather than silently selecting some other curve;
    // `None` stays `None`.
    if let Some(ci) = *selected_curve {
        if ci >= curves.len() {
            *selected_curve = None;
            *selected_point = None;
        }
    }

    // Fills the full width of whatever container it's placed in (now
    // the Workspace window's center pane) rather than capping at a
    // fixed pixel width - the cap made no sense once this widget had an
    // independently resizable column to itself instead of sharing a
    // single scrolling page with everything else.
    let desired_size = Vec2::new(ui.available_width(), 360.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());
    #[cfg(test)]
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(LAST_GRAPH_RECT_ID), rect));
    let painter = ui.painter_at(rect);

    let (plot_rect, bar_rect, label_rect) = plot_axis::split_plot_bar_and_labels(rect);
    let all_points = curves.iter().flat_map(|c| c.points.iter());
    let (wl_min, wl_max) = plot_axis::auto_range(
        all_points.clone().map(|p| p.0),
        0.15,
        100.0,
        FALLBACK_WL_RANGE,
    );
    let (val_min, val_max) =
        plot_axis::auto_range(all_points.map(|p| p.1), 0.2, 0.2, FALLBACK_VAL_RANGE);
    let axes = PlotAxes {
        rect: plot_rect,
        wl_min,
        wl_max,
        val_min,
        val_max,
        orientation,
    };

    // --- Interaction pass first: hit-test and mutate only the selected
    // curve (if any) before anything is drawn, so the draw pass below
    // always renders this frame's post-edit state (line shape, point
    // positions, and the shape-derived display color) rather than
    // lagging one frame behind a drag. Deliberately restricted to
    // `curves[ci]` - see the module/function docs above on why other
    // curves' points must never be hit-testable here, and why nothing
    // is hit-testable at all when no curve is selected.
    let mut remove: Option<usize> = None;
    let mut new_selected_point: Option<usize> = None;
    if let Some(ci) = *selected_curve {
        let curve = &mut curves[ci];
        let points_snapshot = curve.points.clone();
        for (pi, &(wl, v)) in points_snapshot.iter().enumerate() {
            let center = axes.to_screen(wl, v);
            let handle_rect = Rect::from_center_size(center, Vec2::splat(HANDLE_RADIUS * 2.5));
            let id = response.id.with(("multi_point_handle", ci, pi));
            let point_response = ui.interact(handle_rect, id, Sense::click_and_drag());

            if point_response.dragged() {
                let new_center = center + point_response.drag_delta();
                let (new_wl, new_v) = axes.screen_to_wl_value(new_center);
                curve.points[pi] = (
                    new_wl.clamp(SANITY_WL_RANGE.0, SANITY_WL_RANGE.1),
                    new_v.clamp(SANITY_VAL_RANGE.0, SANITY_VAL_RANGE.1),
                );
                new_selected_point = Some(pi);
            }
            if point_response.clicked() {
                new_selected_point = Some(pi);
            }
            if point_response.secondary_clicked() {
                remove = Some(pi);
            }
        }
    }

    if let Some(pi) = new_selected_point {
        *selected_point = Some(pi);
    }

    // Double-click on empty graph area adds a new point to the
    // currently-selected curve, if any (pick one via the legend row
    // below first if the curve you want has no points yet to click on,
    // or if nothing is selected yet).
    if response.double_clicked() {
        if let (Some(ci), Some(pos)) = (*selected_curve, response.interact_pointer_pos()) {
            let (wl, v) = axes.screen_to_wl_value(pos);
            let curve = &mut curves[ci];
            curve.points.push((
                wl.clamp(SANITY_WL_RANGE.0, SANITY_WL_RANGE.1),
                v.clamp(SANITY_VAL_RANGE.0, SANITY_VAL_RANGE.1),
            ));
            *selected_point = Some(curve.points.len() - 1);
        }
    }

    // Delete/Backspace removes the selected point, and the arrow keys
    // nudge it - but only while no *other* widget (a text field, a
    // DragValue) currently holds keyboard focus. `ui.input` reads raw
    // per-frame key events regardless of focus, so without this guard,
    // e.g. pressing Left to move a text cursor in the curve-name field
    // would also nudge whatever point happens to be selected here.
    let no_other_focus = ui.ctx().memory(|m| m.focused()).is_none();
    if no_other_focus {
        if let (Some(ci), Some(idx)) = (*selected_curve, *selected_point) {
            let delete_pressed = ui
                .input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace));
            if delete_pressed && idx < curves[ci].points.len() {
                remove = Some(idx);
            }

            let (dx, dy) = ui.input(|i| {
                let step_mult = if i.modifiers.shift { 10.0 } else { 1.0 };
                let mut dx = 0.0_f64;
                let mut dy = 0.0_f64;
                if i.key_pressed(egui::Key::ArrowLeft) {
                    dx -= 1.0;
                }
                if i.key_pressed(egui::Key::ArrowRight) {
                    dx += 1.0;
                }
                if i.key_pressed(egui::Key::ArrowUp) {
                    dy += 1.0;
                }
                if i.key_pressed(egui::Key::ArrowDown) {
                    dy -= 1.0;
                }
                (dx * step_mult, dy * step_mult)
            });
            if (dx != 0.0 || dy != 0.0) && idx < curves[ci].points.len() {
                const WL_STEP_NM: f64 = 1.0;
                let val_step = (axes.val_max - axes.val_min) * 0.01;
                let (wl, v) = curves[ci].points[idx];
                curves[ci].points[idx] = (
                    (wl + dx * WL_STEP_NM).clamp(SANITY_WL_RANGE.0, SANITY_WL_RANGE.1),
                    (v + dy * val_step).clamp(SANITY_VAL_RANGE.0, SANITY_VAL_RANGE.1),
                );
            }
        }
    }

    if let (Some(ci), Some(pi)) = (*selected_curve, remove) {
        if pi < curves[ci].points.len() {
            curves[ci].points.remove(pi);
        }
        if *selected_point == Some(pi) {
            *selected_point = None;
        }
    }

    // --- Draw pass: entirely from `curves`' now-current state.
    painter.rect_filled(plot_rect, 0.0, Color32::BLACK);
    plot_axis::draw_spectrum_bar(&painter, &axes, bar_rect);
    plot_axis::draw_axis_labels(&painter, &axes, label_rect, ui.visuals().text_color());

    for curve in curves.iter() {
        let color = display_color(curve);
        let interp = curve.interpolant();
        let mut prev: Option<Pos2> = None;
        let n_line_samples = (plot_rect.width() / LINE_STEP_PX).ceil().max(1.0) as usize;
        for i in 0..=n_line_samples {
            let wl = axes.frac_to_wl(i as f64 / n_line_samples as f64);
            match interp.value_at(wl) {
                Some(v) => {
                    let p = axes.to_screen(wl, v);
                    if let Some(pp) = prev {
                        painter.line_segment([pp, p], Stroke::new(2.5_f32, color));
                    }
                    prev = Some(p);
                }
                None => prev = None,
            }
        }
    }

    // Point handles are only drawn for the curve currently selected for
    // editing, if any - with every curve's points dotted at once,
    // overlapping handles from different curves became impossible to
    // tell apart or click precisely once more than a couple of curves
    // were shown.
    if let Some(curve) = selected_curve.and_then(|ci| curves.get(ci)) {
        let color = display_color(curve);
        for (pi, &(wl, v)) in curve.points.iter().enumerate() {
            let center = axes.to_screen(wl, v);
            let is_selected = *selected_point == Some(pi);
            let handle_color = if is_selected { Color32::YELLOW } else { color };
            painter.circle_filled(center, HANDLE_RADIUS, handle_color);
            painter.circle_stroke(center, HANDLE_RADIUS, Stroke::new(1.0_f32, Color32::BLACK));
        }
    }

    painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, Color32::GRAY));

    // Legend row: shows every curve's color + name, and doubles as the
    // curve picker (click a name to make it the add-point/remove-curve
    // target; click the already-selected one again to deselect it
    // entirely). Colors here are freshly recomputed too, so the legend
    // swatch stays in sync with a curve's shape while it's being edited.
    ui.horizontal_wrapped(|ui| {
        for (ci, curve) in curves.iter().enumerate() {
            let color = display_color(curve);
            ui.colored_label(color, "■");
            if ui
                .selectable_label(*selected_curve == Some(ci), &curve.name)
                .clicked()
            {
                *selected_curve = if *selected_curve == Some(ci) {
                    None
                } else {
                    Some(ci)
                };
                *selected_point = None;
            }
        }
    });

    (wl_min, wl_max)
}
