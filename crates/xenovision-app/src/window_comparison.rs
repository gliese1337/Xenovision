//! Comparison window (design doc §2.2, mockup "Window 2"): multi-
//! spectrum coordinate table, difference matrix, and (with 2+ species
//! selected) a cross-species ΔS summary - all driven by the shared
//! `AppState` and recomputed plain-functionally each frame from current
//! selections (design doc §5's recomputation-timing note), using
//! `AppState::transform_cache` so repeat views don't redo the expensive
//! adaptation-matrix derivation.
//!
//! Multi-regime (§2.2): each selected species contributes *several*
//! pipelines (one per colorspace regime). The species table shows the
//! regime its pills select (dominant starred/★); the regime-dominance
//! plot sweeps light level and draws each regime's activity
//! (`regime_gain = ∏ g_i` over member gains) against log₁₀ total
//! reference irradiance, with a cursor at the current level. Receptors
//! in the pool but in no regime are reported individually; a species
//! with no regimes at all (mantis shrimp, §10.2) shows the
//! no-colorspace report instead of a coordinate table.

use std::collections::HashSet;

use eframe::egui;
use eframe::egui::{Color32, Pos2, Rect, Stroke, Vec2};
use xenovision_core::comparison::{self, DistanceMetric, SpeciesDeltaSColumn};
use xenovision_core::curve::{operating_gain, DEFAULT_ILLUMINATION_SOFTNESS};
use xenovision_core::curve_set::CurveSet as CoreCurveSet;
use xenovision_core::pipeline::{Coordinates, Pipeline};
use xenovision_core::{illumination, CurveType, QuantityKind, SpectralCurve};

use crate::batch_export;
use crate::multi_curve_editor;
use crate::state::{AppState, CurveId, CurveSetId};
use crate::stimulus_picker;

const STEP_NM: f64 = 1.0;

/// Distinct per-regime line colors for the dominance plot.
const REGIME_COLORS: &[Color32] = &[
    Color32::from_rgb(240, 240, 240),
    Color32::from_rgb(255, 180, 60),
    Color32::from_rgb(140, 200, 255),
    Color32::from_rgb(150, 255, 150),
    Color32::from_rgb(255, 140, 200),
    Color32::from_rgb(200, 160, 255),
];

pub fn ui(ui: &mut egui::Ui, app: &mut AppState) {
    // The luminant editor and result tables together are taller than a
    // typical window, so the whole panel scrolls vertically; the center
    // column also scrolls sideways, since coordinate tables and
    // difference matrices grow with the number of stimuli and receptors.
    egui::ScrollArea::vertical()
        .id_salt("comparison_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            top_bar(ui, app);
            luminant_editor(ui, app);
            ui.separator();
            batch_export::export_panel(ui, app);
            ui.separator();

            ui.columns(3, |columns| {
                left_column(&mut columns[0], app);
                egui::ScrollArea::horizontal()
                    .id_salt("comparison_results_scroll")
                    .auto_shrink([false, true])
                    .show(&mut columns[1], |ui| center_column(ui, app));
                right_column(&mut columns[2], app);
            });
        });
}

fn top_bar(ui: &mut egui::Ui, app: &mut AppState) {
    ui.horizontal(|ui| {
        ui.label("Luminant:");
        let current_name = app
            .comparison
            .reference_luminant
            .and_then(|id| app.luminants.get(&id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "(none)".to_string());
        egui::ComboBox::from_id_salt("reference_luminant")
            .selected_text(current_name)
            .show_ui(ui, |ui| {
                for &id in &app.luminant_order {
                    if let Some(curve) = app.luminants.get(&id) {
                        if ui
                            .selectable_label(
                                app.comparison.reference_luminant == Some(id),
                                &curve.name,
                            )
                            .clicked()
                        {
                            app.comparison.reference_luminant = Some(id);
                            app.comparison.luminant_selected_point = None;
                        }
                    }
                }
            });

        ui.separator();
        ui.label("Metric:");
        ui.radio_value(
            &mut app.comparison.metric,
            DistanceMetric::Euclidean,
            "Euclidean",
        );
        ui.radio_value(
            &mut app.comparison.metric,
            DistanceMetric::ChromaOnly,
            "Chroma-only",
        );
        ui.radio_value(
            &mut app.comparison.metric,
            DistanceMetric::DeltaS,
            "ΔS (Vorobyev-Osorio)",
        );
    });
}

/// Lets the luminant dropdown above actually be populated and edited -
/// previously the only two luminants that would ever exist were the two
/// seeded at startup, with no way to add, generate, or edit one anywhere
/// in the app.
fn luminant_editor(ui: &mut egui::Ui, app: &mut AppState) {
    ui.horizontal(|ui| {
        ui.menu_button("+ New luminant ▾", |ui| {
            if ui.button("Blank luminant").clicked() {
                add_luminant(
                    app,
                    SpectralCurve::new("New luminant", CurveType::Illumination)
                        .with_points(vec![(400.0, 1.0), (700.0, 1.0)]),
                );
                ui.close_menu();
            }
            if ui.button("Black-body generator...").clicked() {
                let curve = xenovision_core::blackbody::generate_blackbody_curve(
                    5778.0,
                    280.0,
                    2500.0,
                    2.0,
                    &[],
                );
                add_luminant(app, curve);
                ui.close_menu();
            }
            if ui.button("Composite LED generator...").clicked() {
                let presets = xenovision_core::narrowband::load_source_presets();
                let components: Vec<(xenovision_core::narrowband::GaussianComponent, f64)> =
                    presets
                        .first()
                        .map(|p| p.components.iter().cloned().map(|c| (c, 1.0)).collect())
                        .unwrap_or_default();
                let curve = xenovision_core::narrowband::generate_composite_curve(
                    &components,
                    300.0,
                    800.0,
                    1.0,
                );
                add_luminant(app, curve);
                ui.close_menu();
            }
        });
        if let Some(id) = app.comparison.reference_luminant {
            if ui.button("Remove this luminant").clicked() {
                remove_luminant(app, id);
            }
        }
    });

    let Some(selected_id) = app.comparison.reference_luminant else {
        return;
    };
    let Some(mut curve) = app.luminants.get(&selected_id).cloned() else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Name:");
        if ui.text_edit_singleline(&mut curve.name).changed() {
            app.luminants.insert(selected_id, curve.clone());
        }
    });

    // Reuses the same point-drag/add/remove graph as Workspace: a
    // single-element slice, with its one entry always treated as
    // selected (there's nothing else in the list to switch to).
    let mut curves = vec![curve];
    let mut selected = Some(0usize);
    multi_curve_editor::multi_curve_editor(
        ui,
        &mut curves,
        &mut selected,
        &mut app.comparison.luminant_selected_point,
        app.axis_orientation,
    );
    if let Some(edited) = curves.into_iter().next() {
        if app.luminants.get(&selected_id) != Some(&edited) {
            app.luminants.insert(selected_id, edited);
            app.transform_cache.invalidate_luminant(selected_id);
        }
    }

    luminant_power_control(ui, app, selected_id);
    luminant_absorption_control(ui, app, selected_id);
}

/// Multiplies the selected luminant by an absorption curve from the
/// stimulus library raised to a weight: `luminant(λ) × a(λ)^w` (see
/// `illumination::apply_absorption`).
fn luminant_absorption_control(ui: &mut egui::Ui, app: &mut AppState, id: CurveId) {
    let absorbers: Vec<(CurveId, String)> = app
        .stimulus_order
        .iter()
        .filter_map(|cid| app.stimulus_curves.get(cid).map(|e| (*cid, e)))
        .filter(|(_, e)| e.curve.quantity == QuantityKind::Absorption)
        .map(|(cid, e)| (cid, e.curve.name.clone()))
        .collect();
    if absorbers.is_empty() {
        ui.label(
            "To add absorption notches, create an absorption curve in the Stimulus Editor \
             (\"+ New stimulus\" → from a notch preset, or a custom notch).",
        );
        return;
    }
    if app
        .comparison
        .absorption_to_apply
        .is_none_or(|a| !absorbers.iter().any(|(cid, _)| *cid == a))
    {
        app.comparison.absorption_to_apply = Some(absorbers[0].0);
    }
    let mut apply = false;
    ui.horizontal(|ui| {
        ui.label("Apply absorption:");
        let current = app.comparison.absorption_to_apply;
        egui::ComboBox::from_id_salt("luminant_absorption")
            .selected_text(
                absorbers
                    .iter()
                    .find(|(cid, _)| Some(*cid) == current)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_default(),
            )
            .show_ui(ui, |ui| {
                for (cid, name) in &absorbers {
                    ui.selectable_value(&mut app.comparison.absorption_to_apply, Some(*cid), name);
                }
            });
        ui.label("weight:");
        ui.add(
            egui::DragValue::new(&mut app.comparison.absorption_weight)
                .speed(0.05)
                .range(0.0..=f64::MAX),
        )
        .on_hover_text(
            "Exponent on the absorption samples: 0 = none, 1 = as-is, 2 = twice as strong",
        );
        apply = ui.button("Apply").clicked();
    });
    if apply {
        let absorber = app
            .comparison
            .absorption_to_apply
            .and_then(|cid| app.stimulus_curves.get(&cid))
            .map(|e| e.curve.clone());
        if let (Some(absorber), Some(lum)) = (absorber, app.luminants.get(&id).cloned()) {
            let weight = app.comparison.absorption_weight;
            let mut out = illumination::apply_absorption(&lum, &absorber, weight, STEP_NM);
            out.metadata.insert(
                format!("absorption: {}", absorber.name),
                format!("weight {weight}"),
            );
            app.luminants.insert(id, out);
            app.comparison.luminant_selected_point = None;
            app.transform_cache.invalidate_luminant(id);
        }
    }
}

/// Scales a luminant's overall power without changing its spectral
/// shape - every sample multiplied by one factor. Matters now that
/// receptors can saturate: the same spectrum at 10x the power can push
/// receptors into their caps, which changes the result in a way a
/// shape-only edit can't express.
fn luminant_power_control(ui: &mut egui::Ui, app: &mut AppState, id: CurveId) {
    let Some(curve) = app.luminants.get(&id) else {
        return;
    };
    let total = curve.integral(STEP_NM);
    ui.label(format!(
        "Irradiance (W/m², ∫ over domain): {total:.3}"
    ));

    let mut factor: Option<f64> = None;
    ui.horizontal(|ui| {
        ui.label("Scale by:");
        ui.add(
            egui::DragValue::new(&mut app.comparison.luminant_scale_factor)
                .speed(0.01)
                .range(0.0..=f64::MAX),
        );
        if ui.button("Apply").clicked() {
            factor = Some(app.comparison.luminant_scale_factor);
            app.comparison.luminant_scale_factor = 1.0;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Set total power to:");
        ui.add(
            egui::DragValue::new(&mut app.comparison.luminant_target_power)
                .speed(0.01)
                .range(0.0..=f64::MAX),
        );
        // A zero-power curve has no shape to rescale toward any target.
        let can_set = total > 0.0;
        if ui.add_enabled(can_set, egui::Button::new("Set")).clicked() {
            factor = Some(app.comparison.luminant_target_power / total);
        }
    });

    if let Some(f) = factor {
        if let Some(curve) = app.luminants.get_mut(&id) {
            for p in curve.points.iter_mut() {
                p.1 *= f;
            }
            app.transform_cache.invalidate_luminant(id);
        }
    }
}

fn add_luminant(app: &mut AppState, mut curve: SpectralCurve) {
    // A luminant is a radiance curve by definition - enforce that
    // regardless of which generator produced it (the black-body/LED
    // generators and the blank option don't tag a quantity kind
    // themselves), so it's always usable with the derive-reflectance/
    // derive-radiance operations' strict unit check.
    if !matches!(curve.quantity, QuantityKind::Radiance { .. }) {
        curve.quantity = QuantityKind::Radiance {
            unit: "relative".to_string(),
        };
    }
    let id = app.alloc_id();
    app.luminants.insert(id, curve);
    app.luminant_order.push(id);
    app.comparison.reference_luminant = Some(id);
    app.comparison.luminant_selected_point = None;
}

fn remove_luminant(app: &mut AppState, id: CurveId) {
    app.luminants.remove(&id);
    app.luminant_order.retain(|&i| i != id);
    app.transform_cache.invalidate_luminant(id);
    if app.comparison.reference_luminant == Some(id) {
        app.comparison.reference_luminant = app.luminant_order.first().copied();
        app.comparison.luminant_selected_point = None;
    }
}

fn left_column(ui: &mut egui::Ui, app: &mut AppState) {
    ui.heading("Input spectra");
    ui.label(
        "Stimuli to evaluate through each selected species' pipelines, from the shared \
         stimulus library (manage it in the Stimulus Editor window).",
    );
    if app.stimulus_curves.is_empty() {
        ui.label("No stimulus curves yet - create one in the Stimulus Editor window.");
        return;
    }
    // Absorption curves modify luminants; they aren't stimuli.
    let not_absorption = |c: &SpectralCurve| c.quantity != QuantityKind::Absorption;
    let tree = stimulus_picker::corpus_tree(app, &not_absorption);
    let selected = app.comparison.selected_stimuli.clone();
    let is_included = |id: CurveId| selected.contains(&id);
    let mut toggled: Vec<(CurveId, bool)> = Vec::new();
    let mut on_toggle = |ids: &[CurveId], included: bool| {
        toggled.extend(ids.iter().map(|&id| (id, included)));
    };
    stimulus_picker::render_corpus_checklist(ui, &tree, &is_included, &mut on_toggle);
    for (id, included) in toggled {
        toggle_stimulus(app, id, included);
    }
}

fn toggle_stimulus(app: &mut AppState, id: CurveId, included: bool) {
    if included {
        if !app.comparison.selected_stimuli.contains(&id) {
            app.comparison.selected_stimuli.push(id);
        }
    } else {
        app.comparison.selected_stimuli.retain(|&i| i != id);
    }
}

fn right_column(ui: &mut egui::Ui, app: &mut AppState) {
    ui.heading("Species");
    for &set_id in &app.workspace.open_tabs {
        let Some(set) = app.curve_sets.get(&set_id) else {
            continue;
        };
        let name = set.name.clone();
        let complete = set.receptor_noise_complete();
        let mut included = app.comparison.selected_species.contains(&set_id);
        ui.horizontal(|ui| {
            ui.colored_label(
                if complete {
                    Color32::from_rgb(90, 200, 90)
                } else {
                    Color32::from_rgb(220, 180, 40)
                },
                "●",
            );
            if ui.checkbox(&mut included, &name).changed() {
                if included {
                    app.comparison.selected_species.push(set_id);
                } else {
                    app.comparison.selected_species.retain(|&id| id != set_id);
                }
                app.comparison.active_species_subtab = 0;
            }
        });
    }
}

/// Resolves a library entry into the curve actually fed to a pipeline:
/// per the app's standardized taxonomy, "a stimulus curve is a radiance
/// curve, or a reflectance curve that's used to filter the luminant" -
/// a Reflectance-quantity curve isn't itself light reaching a receptor,
/// it's a ratio that needs multiplying by the current luminant first
/// (`Reflectance(λ) × Luminant(λ)`, exactly what `predict_under_
/// illuminant` already computes). Anything else (Radiance, or an
/// unspecified/other kind) is assumed to already be radiance-like and
/// passes through unchanged. Previously nothing here did this
/// multiplication at all - every stimulus was integrated directly
/// against each receptor curve regardless of its quantity kind, so a
/// reflectance curve was silently treated as if it were already the
/// full spectrum reaching the eye.
fn resolve_stimulus(curve: &SpectralCurve, luminant: &SpectralCurve) -> SpectralCurve {
    if curve.quantity == QuantityKind::Reflectance {
        illumination::predict_under_illuminant(curve, luminant, STEP_NM)
    } else {
        curve.clone()
    }
}

fn center_column(ui: &mut egui::Ui, app: &mut AppState) {
    let Some(luminant_id) = app.comparison.reference_luminant else {
        ui.label("Pick a luminant above.");
        return;
    };
    let Some(luminant) = app.luminants.get(&luminant_id).cloned() else {
        ui.label("That luminant is no longer available.");
        return;
    };
    if app.comparison.selected_stimuli.is_empty() {
        ui.label("Add at least one input spectrum on the left.");
        return;
    }
    if app.comparison.selected_species.is_empty() {
        ui.label("Select at least one species on the right.");
        return;
    }

    let (stimuli, stimulus_names): (Vec<SpectralCurve>, Vec<String>) = app
        .comparison
        .selected_stimuli
        .iter()
        .filter_map(|id| app.stimulus_curves.get(id))
        .filter(|entry| entry.curve.quantity != QuantityKind::Absorption)
        .map(|entry| {
            (
                resolve_stimulus(&entry.curve, &luminant),
                entry.curve.name.clone(),
            )
        })
        .unzip();

    let species_ids = app.comparison.selected_species.clone();

    if species_ids.len() == 1 {
        let set_id = species_ids[0];
        single_species_tables(
            ui,
            app,
            set_id,
            luminant_id,
            &luminant,
            &stimuli,
            &stimulus_names,
        );
        return;
    }

    app.comparison.active_species_subtab = app
        .comparison
        .active_species_subtab
        .min(species_ids.len().saturating_sub(1));
    ui.horizontal_wrapped(|ui| {
        for (i, &set_id) in species_ids.iter().enumerate() {
            let name = app
                .curve_sets
                .get(&set_id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if ui
                .selectable_label(app.comparison.active_species_subtab == i, name)
                .clicked()
            {
                app.comparison.active_species_subtab = i;
            }
        }
    });
    ui.separator();
    let active_set_id = species_ids[app.comparison.active_species_subtab];
    single_species_tables(
        ui,
        app,
        active_set_id,
        luminant_id,
        &luminant,
        &stimuli,
        &stimulus_names,
    );

    ui.separator();
    cross_species_summary(
        ui,
        app,
        &species_ids,
        luminant_id,
        &luminant,
        &stimuli,
        &stimulus_names,
    );
}

/// The regime with the highest regime activity (`regime_gain`) among
/// `pipelines` - "dominant" at the luminance the reference light
/// produces. Ties resolve to the earlier regime (stable under floating
/// point noise at ~0 gains, where all regimes are effectively off).
fn dominant_regime(pipelines: &[Pipeline]) -> Option<usize> {
    pipelines
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.regime_gain().total_cmp(&b.regime_gain()))
        .map(|(i, _)| i)
}

/// The regime-dominance series: at each scale factor `f` over the
/// reference illuminant, each regime's `regime_gain = ∏ g_i` where each
/// member's gain is pure `operating_gain` at `f ·` its *cached* seen
/// irradiance (E_i ∝ luminant amplitude, so scaling the stored E_i by f
/// is exactly the seen irradiance a scaled luminant would produce). No
/// pipeline rebuilds - this sweeps 96 decades of light level in
/// microseconds a frame. Returns `(log₁₀ total W/m², per-regime gains)`.
///
/// `f = 1.0` (i = 48) is on the x-axis at today's light level, so the
/// cursor in the plot always lands on a computed point.
fn compute_dominance_series(
    core: &CoreCurveSet,
    pipelines: &[Pipeline],
) -> Vec<(f64, Vec<f64>)> {
    if pipelines.is_empty() {
        return Vec::new();
    }
    let total = pipelines[0].reference_irradiance_w_m2();
    let def_min = core.illumination_min;
    let def_max = core.illumination_max;
    let def_soft = core.illumination_softness;
    // Resolve each regime's member curves once so the sweep can read
    // each member's own window (curve-level overrides take precedence
    // over the set-level defaults, exactly as `Pipeline::build` does).
    let members: Vec<Vec<SpectralCurve>> = core
        .regimes
        .iter()
        .map(|r| core.regime_config(r).curves)
        .collect();
    (0..=96u32)
        .map(|i| {
            let f = 10f64.powf(-24.0 + f64::from(i) * (48.0 / 96.0));
            let gains: Vec<f64> = members
                .iter()
                .enumerate()
                .map(|(r, curves)| {
                    curves
                        .iter()
                        .zip(pipelines[r].seen_irradiances().iter())
                        .map(|(c, &seen)| {
                            operating_gain(
                                f * seen,
                                c.illumination_min.or(def_min),
                                c.illumination_max.or(def_max),
                                c.illumination_softness
                                    .or(def_soft)
                                    .unwrap_or(DEFAULT_ILLUMINATION_SOFTNESS),
                            )
                        })
                        .product::<f64>()
                })
                .collect();
            ((f * total).log10(), gains)
        })
        .collect()
}

/// Painter-drawn dominance plot (this egui version ships no `Plot`):
/// one colored line per regime - `regime_gain` vs `log₁₀ total reference
/// W/m²` - plus a vertical cursor at `current_total`, y pinned to 0..1.1
/// because gains are products of sigmoids in [0, 1].
fn draw_dominance_plot(
    ui: &mut egui::Ui,
    series: &[(f64, Vec<f64>)],
    regime_names: &[String],
    current_total: f64,
) {
    if series.is_empty() {
        return;
    }
    let (x_lo, x_hi) = series.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(lo, hi), &(x, _)| (lo.min(x), hi.max(x)),
    );
    let height = 150.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);

    const MARGIN_LEFT: f32 = 6.0;
    const MARGIN_BOTTOM: f32 = 42.0;
    const MARGIN_TOP: f32 = 4.0;
    let plot_rect = Rect::from_min_max(
        Pos2::new(rect.left() + MARGIN_LEFT, rect.top() + MARGIN_TOP),
        Pos2::new(rect.right() - 4.0, rect.bottom() - MARGIN_BOTTOM),
    );

    painter.rect_filled(plot_rect, 0.0, Color32::from_gray(18));
    painter.rect_stroke(plot_rect, 0.0, Stroke::new(1.0_f32, Color32::from_gray(80)));

    let to_screen = |x: f64, y: f64| -> Pos2 {
        let fx = ((x - x_lo) / (x_hi - x_lo)) as f32;
        let fy = (y.clamp(0.0, 1.2) / 1.2) as f32;
        Pos2::new(
            plot_rect.left() + fx * plot_rect.width(),
            plot_rect.bottom() - fy * plot_rect.height(),
        )
    };

    // Y gridlines + labels at 0, 0.25, 0.5, 0.75, 1.
    for frac in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let y = plot_rect.bottom() - frac * plot_rect.height();
        painter.line_segment(
            [
                Pos2::new(plot_rect.left(), y),
                Pos2::new(plot_rect.right(), y),
            ],
            Stroke::new(1.0_f32, Color32::from_gray(40)),
        );
        painter.text(
            Pos2::new(plot_rect.left() + 2.0, y - 4.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{:.2}", frac * 1.2),
            egui::FontId::proportional(9.0),
            ui.visuals().text_color(),
        );
    }

    for (r, name) in regime_names.iter().enumerate() {
        let color = REGIME_COLORS[r % REGIME_COLORS.len()];
        let points: Vec<Pos2> = series.iter().map(|&(x, ref rows)| to_screen(x, rows[r])).collect();
        for w in points.windows(2) {
            painter.line_segment([w[0], w[1]], Stroke::new(2.0_f32, color));
        }
        let legend_y = plot_rect.bottom() + 12.0 + (r as f32) * 14.0;
        let lx = plot_rect.left() + 8.0 + (r as f32) * (plot_rect.width() * 0.5);
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(lx, legend_y), Vec2::splat(8.0)),
            1.0,
            color,
        );
        painter.text(
            Pos2::new(lx + 11.0, legend_y + 4.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(10.0),
            ui.visuals().text_color(),
        );
    }

    // Vertical cursor at the current light level; f=1 is always one of
    // the sampled x values, so the cursor sits exactly on the curves.
    let current_x = if current_total > 0.0 {
        current_total.log10()
    } else {
        x_lo
    };
    if current_x >= x_lo - 1e-9 && current_x <= x_hi + 1e-9 {
        let cx = to_screen(current_x, 0.0).x;
        painter.line_segment(
            [
                Pos2::new(cx, plot_rect.top()),
                Pos2::new(cx, plot_rect.bottom()),
            ],
            Stroke::new(1.5_f32, Color32::from_rgb(120, 220, 120)),
        );
        painter.text(
            Pos2::new(cx, plot_rect.top() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            "current",
            egui::FontId::proportional(9.0),
            Color32::from_rgb(120, 220, 120),
        );
    }

    // X-axis: a few tic labels in log₁₀ total W/m².
    painter.text(
        Pos2::new(plot_rect.center().x, plot_rect.bottom() + 4.0),
        egui::Align2::CENTER_TOP,
        format!("log₁₀ total reference irradiance (W/m²), {x_lo:.1} … {x_hi:.1}"),
        egui::FontId::proportional(10.0),
        ui.visuals().text_color(),
    );
}

/// The mantis-shrimp branch of `single_species_tables`: a species whose
/// `regimes` list is empty has no colorspace at all (§10.2), so there is
/// no luminance/chroma, no ΔS noise, and no coordinate table to draw -
/// its only meaningful output is each receptor's individual operating
/// state, reported pool-wide. Kept a separate function so the rest of
/// this file's rendering logic never has to special-case an empty
/// pipeline vector.
fn no_colorspace_report(
    ui: &mut egui::Ui,
    core: &CoreCurveSet,
    luminant: &SpectralCurve,
    stimuli: &[SpectralCurve],
    stimulus_names: &[String],
    set_id: CurveSetId,
) {
    ui.colored_label(
        Color32::from_rgb(220, 180, 40),
        "This species has no colorspace regimes: none of its receptors belong to a \
         colorspace, so luminance, chroma, and ΔS are all unavailable by construction \
         (see §10.2 - mantis shrimp model a receptor set that isn't combined into \
         opponent channels). Each receptor is reported individually below.",
    );
    let seen = xenovision_core::pipeline::seen_irradiances_for(&core.curves, luminant, STEP_NM);
    let gains = core.receptor_gains(&seen);
    let total = luminant.integral(STEP_NM);
    ui.label(format!(
        "Reference light: {total:.3} W/m² total. Receptor operating states:"
    ));
    egui::Grid::new(("no_regime_op_state", set_id))
        .striped(true)
        .show(ui, |ui| {
            ui.label("Receptor");
            ui.label("Seen irradiance (W/m²)");
            ui.label("Gain");
            ui.end_row();
            for (curve, (&seen, &gain)) in core
                .curves
                .iter()
                .zip(seen.iter().zip(gains.iter()))
            {
                ui.label(&curve.name);
                ui.label(format!("{seen:.3e}"));
                ui.label(format!("{gain:.3}"));
                ui.end_row();
            }
        });

    if stimuli.len() < 2 || stimulus_names.is_empty() {
        return;
    }
    ui.separator();
    ui.label(
        "Raw (pre-adaptation) activations - the only stimulus-dependent output this \
         species has:",
    );
    let raw: Vec<Vec<f64>> = stimuli
        .iter()
        .map(|stim| {
            xenovision_core::pipeline::raw_activations_with_gains(&core.curves, &gains, stim, STEP_NM)
        })
        .collect();
    egui::Grid::new(("no_regime_activations", set_id))
        .striped(true)
        .show(ui, |ui| {
            ui.label("Receptor");
            for name in stimulus_names {
                ui.label(name);
            }
            ui.end_row();
            for (i, curve) in core.curves.iter().enumerate() {
                ui.label(&curve.name);
                for per_stim in &raw {
                    ui.label(format!("{:.4}", per_stim[i]));
                }
                ui.end_row();
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn single_species_tables(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    luminant_id: u64,
    luminant: &SpectralCurve,
    stimuli: &[SpectralCurve],
    stimulus_names: &[String],
) {
    let Some(set) = app.curve_sets.get(&set_id) else {
        ui.label("That species is no longer open.");
        return;
    };
    let pipelines = app
        .transform_cache
        .get_or_build(set, luminant_id, luminant, STEP_NM);
    let core = set.to_core();

    if pipelines.is_empty() {
        no_colorspace_report(ui, &core, luminant, stimuli, stimulus_names, set_id);
        return;
    }

    let dominant = dominant_regime(&pipelines).unwrap_or(0);
    // Read the entry out into a plain usize before any widget mutates
    // the map, so the `entry().or_insert` borrow isn't live across the
    // pill loop's `insert` calls.
    let mut active = *app
        .comparison
        .active_regime
        .entry(set_id)
        .or_insert(dominant);
    active = active.min(pipelines.len() - 1);
    app.comparison.active_regime.insert(set_id, active);
    let active_pipeline = &pipelines[active];

    ui.label(format!(
        "Reference light: {:.3} W/m² total.",
        active_pipeline.reference_irradiance_w_m2()
    ));
    if !active_pipeline.is_active() {
        ui.colored_label(
            Color32::from_rgb(220, 180, 40),
            "Every receptor in this regime is gated off at this light level, so its \
             coordinates are ~0.",
        );
    }

    // Regime pills: one per colorspace, each showing its regime activity
    // (∏ member gains); the dominant one is starred; clicking switches
    // which regime's table the rest of this panel shows.
    ui.horizontal_wrapped(|ui| {
        ui.label("Regimes:");
        for (r, pipeline) in pipelines.iter().enumerate() {
            let label = format!(
                "{} · ∏g = {:.3}{}",
                pipeline.name(),
                pipeline.regime_gain(),
                if r == dominant { " ★" } else { "" }
            );
            if ui.selectable_label(r == active, label).clicked() {
                app.comparison.active_regime.insert(set_id, r);
            }
        }
    });
    ui.label(
        "★ marks the dominant regime at this light level; click a pill to switch the table.",
    );

    ui.separator();

    // Regime-dominance plot: how each regime's activity trades off with
    // light level, sweeping the reference illuminant's power up/down.
    let series = compute_dominance_series(&core, &pipelines);
    let regime_names: Vec<String> = pipelines
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    draw_dominance_plot(
        ui,
        &series,
        &regime_names,
        active_pipeline.reference_irradiance_w_m2(),
    );

    ui.separator();

    // Per-regime operating-state tables (one per colorspace): each
    // receptor gates on the irradiance it absorbs from the reference
    // light (E_i = ∫S·I dλ, W/m²), so the same spectrum reads
    // differently at different light levels (scotopic vs photopic
    // regime).
    ui.label("Receptor operating state per colorspace regime:");
    for (r, pipeline) in pipelines.iter().enumerate() {
        egui::CollapsingHeader::new(format!(
            "{} — N = {} {}",
            pipeline.name(),
            pipeline.curve_names().len(),
            if r == active { "(shown in table)" } else { "" }
        ))
        .default_open(pipelines.len() <= 2 || r == active)
        .show(ui, |ui| {
            egui::Grid::new(("op_state", set_id, r))
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Receptor");
                    ui.label("Seen irradiance (W/m²)");
                    ui.label("Gain");
                    ui.end_row();
                    for (name, &seen, &gain) in pipeline
                        .curve_names()
                        .iter()
                        .zip(pipeline.seen_irradiances().iter())
                        .zip(pipeline.response_gains().iter())
                        .map(|((n, s), g)| (*n, s, g))
                    {
                        ui.label(name);
                        ui.label(format!("{seen:.3e}"));
                        ui.label(format!("{gain:.3}"));
                        ui.end_row();
                    }
                });
        });
    }

    // Unaffiliated receptors: in the pool, in no regime. Reported
    // individually; they never enter any luminance/chroma calculation.
    let members: HashSet<usize> = core
        .regimes
        .iter()
        .flat_map(|r| r.curves.iter().map(|&i| i as usize))
        .collect();
    let unaffiliated: Vec<usize> = (0..core.curves.len())
        .filter(|i| !members.contains(i))
        .collect();
    if !unaffiliated.is_empty() {
        ui.separator();
        ui.label(
            "Receptors in the pool but in no colorspace regime (reported individually; \
             they never enter any luminance/chroma calculation):",
        );
        let seen = xenovision_core::pipeline::seen_irradiances_for(&core.curves, luminant, STEP_NM);
        let gains = core.receptor_gains(&seen);
        egui::Grid::new(("unaff_op_state", set_id))
            .striped(true)
            .show(ui, |ui| {
                ui.label("Receptor");
                ui.label("Seen irradiance (W/m²)");
                ui.label("Gain");
                ui.end_row();
                for &i in &unaffiliated {
                    ui.label(&core.curves[i].name);
                    ui.label(format!("{:.3e}", seen[i]));
                    ui.label(format!("{:.3}", gains[i]));
                    ui.end_row();
                }
            });
    }
    ui.separator();

    let coords: Vec<Coordinates> = stimuli
        .iter()
        .map(|s| active_pipeline.coordinates(s))
        .collect();

    // Saturation + hue angles are only shown for N > 2 (chroma.len() > 1)
    // - a dichromat's single signed chroma value already carries its
    // "hue" as a sign, with no angle to decompose it into.
    let show_hue = coords.first().is_some_and(|c| c.chroma.len() > 1);
    let hue_count = coords.first().map(|c| c.hue_angles().len()).unwrap_or(0);

    ui.label(format!("Coordinate table ({}) —", active_pipeline.name()));
    egui::Grid::new(("coord_table", set_id))
        .striped(true)
        .show(ui, |ui| {
            ui.label("Spectrum");
            ui.label("Luminance");
            if let Some(first) = coords.first() {
                for i in 0..first.chroma.len() {
                    ui.label(format!("Chroma {}", i + 1));
                }
            }
            if show_hue {
                ui.label("Saturation");
                for i in 0..hue_count {
                    ui.label(format!("Hue φ{}", i + 1));
                }
            }
            ui.end_row();
            for (name, c) in stimulus_names.iter().zip(coords.iter()) {
                ui.label(name);
                ui.label(format!("{:.4}", c.luminance));
                for v in &c.chroma {
                    ui.label(format!("{v:.4}"));
                }
                if show_hue {
                    ui.label(format!("{:.4}", c.saturation));
                    for a in c.hue_angles() {
                        ui.label(format!("{:.3}", a));
                    }
                }
                ui.end_row();
            }
        });

    if coords.len() < 2 {
        return;
    }

    ui.separator();
    ui.label(format!("Difference matrix ({}) —", active_pipeline.name()));
    let noise = active_pipeline.noise().map(|n| n.to_vec());
    match comparison::difference_matrix(&coords, app.comparison.metric, noise.as_deref()) {
        Some(matrix) => {
            egui::Grid::new(("diff_matrix", set_id))
                .striped(true)
                .show(ui, |ui| {
                    ui.label("");
                    for name in stimulus_names {
                        ui.label(name);
                    }
                    ui.end_row();
                    for (i, row) in matrix.iter().enumerate() {
                        ui.label(&stimulus_names[i]);
                        for v in row {
                            ui.label(format!("{v:.4}"));
                        }
                        ui.end_row();
                    }
                });
        }
        None => {
            ui.label(
                "ΔS unavailable: this visual system doesn't have complete noise data \
                 on every receptor in its colorspace.",
            );
        }
    }
}

/// Cross-species ΔS table. Builds each species' column from the shared
/// `TransformCache` (same as the per-species tables) rather than
/// `comparison::cross_species_delta_s`, which rebuilds every species'
/// `Pipeline` - re-deriving its adaptation matrix - on every call, and
/// so on every frame here. Each species is judged through its *dominant*
/// regime at the current light level (§2.2 - the regime that's actually
/// driving its vision); regime-less species contribute no column.
#[allow(clippy::too_many_arguments)]
fn cross_species_summary(
    ui: &mut egui::Ui,
    app: &mut AppState,
    species_ids: &[CurveSetId],
    luminant_id: CurveId,
    luminant: &SpectralCurve,
    stimuli: &[SpectralCurve],
    stimulus_names: &[String],
) {
    if stimulus_names.len() < 2 {
        ui.label("Add a second input spectrum to see cross-species ΔS comparisons.");
        return;
    }
    let mut columns: Vec<SpeciesDeltaSColumn> = Vec::new();
    for id in species_ids {
        let Some(set) = app.curve_sets.get(id) else {
            continue;
        };
        let pipelines = app
            .transform_cache
            .get_or_build(set, luminant_id, luminant, STEP_NM);
        let Some(regime_idx) = dominant_regime(&pipelines) else {
            continue; // no colorspace -> no ΔS (maintained differentiable)
        };
        let pipeline = &pipelines[regime_idx];
        let noise = pipeline.noise().map(|n| n.to_vec());
        let coords: Vec<Coordinates> = stimuli.iter().map(|s| pipeline.coordinates(s)).collect();
        columns.push(SpeciesDeltaSColumn {
            species_name: set.name.clone(),
            has_complete_noise_data: noise.is_some(),
            delta_s_matrix: comparison::difference_matrix(
                &coords,
                DistanceMetric::DeltaS,
                noise.as_deref(),
            ),
        });
    }

    ui.label(
        "Cross-species ΔS summary (JND units; ⚠ = incomplete/missing noise data for that species; \
         each species judged through its dominant regime at this light level):",
    );
    egui::Grid::new("xspecies_summary_table")
        .striped(true)
        .show(ui, |ui| {
            ui.label("Stimulus pair");
            for col in &columns {
                ui.label(if col.has_complete_noise_data {
                    col.species_name.clone()
                } else {
                    format!("⚠ {}", col.species_name)
                });
            }
            ui.end_row();

            for i in 0..stimulus_names.len() {
                for j in (i + 1)..stimulus_names.len() {
                    ui.label(format!("{} vs. {}", stimulus_names[i], stimulus_names[j]));
                    for col in &columns {
                        match &col.delta_s_matrix {
                            Some(matrix) => {
                                ui.label(format!("{:.4}", matrix[i][j]));
                            }
                            None => {
                                ui.label("unavailable");
                            }
                        }
                    }
                    ui.end_row();
                }
            }
        });
}