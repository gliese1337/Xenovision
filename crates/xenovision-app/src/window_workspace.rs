//! Workspace window (design doc §2.1, mockup "Window 1"): tabbed Curve
//! Set editor - one pooled receptor list (graph, point table, noise/
//! luminance table, operating windows, metadata), a Colorspace-regimes
//! block (add/rename/remove regimes, membership, per-regime luminance/
//! opponent overrides, derived light-level band), all driven by the
//! shared `AppState`.

use std::collections::{BTreeMap, HashMap, HashSet};

use eframe::egui;
use xenovision_core::curve_set::{
    CurveSet as CoreCurveSet, OpponentContrast, Regime as CoreRegime,
};
use xenovision_core::{fixture_library, natural_opponent, CurveType, QuantityKind, SpectralCurve};

use crate::multi_curve_editor;
use crate::plot_axis::{self, AxisOrientation};
use crate::state::{AppCurveSet, AppRegime, AppState, CurveId, CurveSetId, NaturalSceneMode};
use crate::stimulus_picker;

const STEP_NM: f64 = 1.0;

fn display_color(curve: &SpectralCurve) -> egui::Color32 {
    xenovision_core::cie::curve_display_color(curve)
        .map(|(r, g, b)| {
            egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
        })
        .unwrap_or(egui::Color32::WHITE)
}

/// Above this N, deriving the adaptation matrix (done whenever the set
/// changes) pauses the UI noticeably: ~0.66s at N=32, ~2.4s at N=48
/// (`adaptati
/// on::tests::bench_realistic_n`).
const HIGH_N_WARNING_THRESHOLD: usize = 32;

pub fn ui(ui: &mut egui::Ui, app: &mut AppState) {
    menu_bar(ui, app);
    ui.separator();
    tab_bar(ui, app);
    ui.separator();

    if app.workspace.open_tabs.is_empty() {
        ui.label("No Curve Sets open - use the \"+\" tab above to create one.");
        return;
    }
    app.workspace.active_tab = app
        .workspace
        .active_tab
        .min(app.workspace.open_tabs.len() - 1);
    let set_id = app.workspace.open_tabs[app.workspace.active_tab];

    tab_body(ui, app, set_id);

    // Drive this tab's undo/redo from every editing surface above
    // uniformly (design doc §1.7) - one `observe` call per frame, after
    // every widget had a chance to mutate the set.
    let focused = ui.ctx().memory(|m| m.focused());
    if let (Some(set), Some(tab_ui)) = (
        app.curve_sets.get(&set_id),
        app.workspace.per_tab.get_mut(&set_id),
    ) {
        let snapshot = set.snapshot();
        tab_ui.undo.observe(&snapshot, focused);
    }
}

fn menu_bar(ui: &mut egui::Ui, app: &mut AppState) {
    ui.horizontal(|ui| {
        ui.menu_button("File", |ui| file_menu(ui, app));
        ui.menu_button("View", |ui| view_menu(ui, app));
        ui.menu_button("Window", |ui| {
            crate::dock::window_menu(ui, &app.dock, &mut app.pending_dock_actions);
        });
    });
}

fn view_menu(ui: &mut egui::Ui, app: &mut AppState) {
    ui.label("X-axis orientation:");
    if ui
        .radio_value(
            &mut app.axis_orientation,
            AxisOrientation::IncreasingWavelength,
            "Increasing wavelength",
        )
        .clicked()
    {
        ui.close_menu();
    }
    if ui
        .radio_value(
            &mut app.axis_orientation,
            AxisOrientation::IncreasingFrequency,
            "Increasing frequency",
        )
        .clicked()
    {
        ui.close_menu();
    }
}

fn file_menu(ui: &mut egui::Ui, app: &mut AppState) {
    if app.workspace.open_tabs.is_empty() {
        ui.label("(no tab open)");
        return;
    }
    let set_id = app.workspace.open_tabs[app
        .workspace
        .active_tab
        .min(app.workspace.open_tabs.len() - 1)];

    if ui.button("Save").clicked() {
        if let Some(set) = app.curve_sets.get_mut(&set_id) {
            let core = set.to_core();
            let result = match &set.file_path {
                Some(path) => core.save_to_file(path),
                None => core.save_to_file(format!("{}.json", set.name.replace(' ', "_"))),
            };
            if let Some(tab) = app.workspace.per_tab.get_mut(&set_id) {
                tab.status = match result {
                    Ok(()) => "Saved".to_string(),
                    Err(e) => format!("Save failed: {e}"),
                };
            }
        }
        ui.close_menu();
    }

    ui.horizontal(|ui| {
        if let Some(tab) = app.workspace.per_tab.get_mut(&set_id) {
            ui.text_edit_singleline(&mut tab.save_path_buf);
            if ui.button("Save As...").clicked() {
                if let Some(set) = app.curve_sets.get_mut(&set_id) {
                    let core = set.to_core();
                    let result = core.save_to_file(&tab.save_path_buf);
                    tab.status = match result {
                        Ok(()) => format!("Saved to {}", tab.save_path_buf),
                        Err(e) => format!("Save failed: {e}"),
                    };
                    set.file_path = Some(std::path::PathBuf::from(&tab.save_path_buf));
                }
            }
        }
    });

    if ui.button("Restore Default Fixtures...").clicked() {
        restore_default_fixtures(app);
        ui.close_menu();
    }

    ui.horizontal(|ui| {
        if let Some(tab) = app.workspace.per_tab.get_mut(&set_id) {
            ui.text_edit_singleline(&mut tab.csv_path_buf);
            if ui.button("Export selected curve as CSV").clicked() {
                let result = app.curve_sets.get(&set_id).and_then(|set| {
                    let curve_id = tab.selected_curve?;
                    let curve = set.curves.get(&curve_id)?;
                    Some(export_csv(curve, &tab.csv_path_buf))
                });
                tab.status = match result {
                    Some(Ok(())) => format!("Exported to {}", tab.csv_path_buf),
                    Some(Err(e)) => format!("Export failed: {e}"),
                    None => "Select a curve first".to_string(),
                };
            }
        }
    });
}

fn export_csv(curve: &SpectralCurve, path: &str) -> std::io::Result<()> {
    let mut text = String::from("wavelength_nm,value\n");
    for (wl, v) in &curve.points {
        text.push_str(&format!("{wl},{v}\n"));
    }
    std::fs::write(path, text)
}

/// Overwrites the on-disk + currently-open data for whichever open tabs
/// match a built-in fixture's name (design doc §8.3's recovery
/// mechanism), by name since this app's id-keyed model has no other
/// stable link back to "which of the 11 built-ins is this". Tabs that
/// don't match any built-in name (custom systems) are left untouched.
/// The whole pool and its regimes are rebuilt with fresh ids (undo
/// history for the tab no longer applies, so it's cleared too).
fn restore_default_fixtures(app: &mut AppState) {
    let restored = fixture_library::restore_builtin_defaults();
    for core_set in restored {
        // Allocate ids for this fixture's curves *before* borrowing the
        // matching open tab, so the borrow checker sees the id counter
        // and `curve_sets` as the disjoint fields they are.
        let base = app.alloc_id_range(core_set.curves.len());
        let Some(set) = app
            .curve_sets
            .values_mut()
            .find(|s| s.name == core_set.name)
        else {
            continue;
        };
        let mut curves = HashMap::new();
        let mut curve_order = Vec::new();
        for (i, c) in core_set.curves.into_iter().enumerate() {
            let id = base + i as u64;
            curve_order.push(id);
            curves.insert(id, c);
        }
        let regimes: Vec<AppRegime> = core_set
            .regimes
            .iter()
            .map(|r| AppRegime {
                name: r.name.clone(),
                curves: r
                    .curves
                    .iter()
                    .filter_map(|&i| curve_order.get(i as usize).copied())
                    .collect(),
                luminance_weights: r.luminance_weights.clone(),
                opponent_contrasts: r.opponent_contrasts.clone(),
            })
            .collect();
        set.curve_order = curve_order;
        set.curves = curves;
        set.regimes = regimes;
        set.metadata = core_set.metadata;
        set.illumination_min = core_set.illumination_min;
        set.illumination_max = core_set.illumination_max;
        set.illumination_softness = core_set.illumination_softness;
        set.revision += 1;
        set.dirty = false;
        let set_id = set.id;
        app.prune_comparison_active_regime(set_id);
        if let Some(tab) = app.workspace.per_tab.get_mut(&set_id) {
            *tab = crate::state::TabUiState::default();
            tab.status = "Restored to built-in defaults".to_string();
        }
    }
}

fn tab_bar(ui: &mut egui::Ui, app: &mut AppState) {
    ui.horizontal_wrapped(|ui| {
        for i in 0..app.workspace.open_tabs.len() {
            let set_id = app.workspace.open_tabs[i];
            let name = app
                .curve_sets
                .get(&set_id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "?".to_string());
            // Label and × go straight into the wrapping row, not into a
            // nested `ui.horizontal`: a wrapping layout can only wrap
            // individual widgets, so nested groups made the whole tab row
            // one unbreakable line, forcing the Workspace far wider than
            // the window.
            if ui
                .selectable_label(app.workspace.active_tab == i, name)
                .clicked()
            {
                app.workspace.active_tab = i;
            }
            if ui.small_button("×").clicked() {
                request_close_tab(app, set_id);
            }
        }
        ui.separator();
        ui.text_edit_singleline(&mut app.workspace.new_tab_name_buf);
        let name_is_blank = app.workspace.new_tab_name_buf.trim().is_empty();
        if ui
            .add_enabled(!name_is_blank, egui::Button::new("+ New Curve Set"))
            .clicked()
        {
            let name = app.workspace.new_tab_name_buf.trim().to_string();
            let id = app.alloc_id();
            app.curve_sets.insert(
                id,
                AppCurveSet {
                    id,
                    name,
                    curve_order: Vec::new(),
                    curves: HashMap::new(),
                    regimes: Vec::new(),
                    metadata: BTreeMap::new(),
                    illumination_min: None,
                    illumination_max: None,
                    illumination_softness: None,
                    file_path: None,
                    dirty: true,
                    revision: 0,
                },
            );
            app.workspace
                .per_tab
                .insert(id, crate::state::TabUiState::default());
            app.workspace.open_tabs.push(id);
            app.workspace.active_tab = app.workspace.open_tabs.len() - 1;
            app.workspace.new_tab_name_buf.clear();
        }
    });

    close_tab_confirmation(ui, app);
}

/// Closes immediately if the tab has no unsaved changes; otherwise
/// parks it in `pending_close` for `close_tab_confirmation` to resolve.
fn request_close_tab(app: &mut AppState, set_id: CurveSetId) {
    let dirty = app
        .curve_sets
        .get(&set_id)
        .map(|s| s.dirty)
        .unwrap_or(false);
    if dirty {
        app.workspace.pending_close = Some(set_id);
    } else {
        close_tab(app, set_id);
    }
}

/// Actually removes the tab and its `CurveSet` - unlike closing the
/// Comparison/Stimulus Editor *windows*, this really does discard the
/// data (hence the unsaved-changes warning before getting here), and
/// also drops it from Comparison's species selection if it was checked
/// there, so that doesn't dangle.
fn close_tab(app: &mut AppState, set_id: CurveSetId) {
    app.curve_sets.remove(&set_id);
    app.workspace.per_tab.remove(&set_id);
    if let Some(idx) = app.workspace.open_tabs.iter().position(|&id| id == set_id) {
        app.workspace.open_tabs.remove(idx);
        if idx < app.workspace.active_tab {
            app.workspace.active_tab -= 1;
        }
    }
    app.prune_comparison_species(set_id);
    if app.workspace.pending_close == Some(set_id) {
        app.workspace.pending_close = None;
    }
}

/// An inline modal (egui has no blocking native dialog) asking whether
/// to discard a dirty tab's changes - closing a tab actually discards
/// its in-memory data, unlike closing a whole window.
fn close_tab_confirmation(ui: &mut egui::Ui, app: &mut AppState) {
    let Some(set_id) = app.workspace.pending_close else {
        return;
    };
    let name = app
        .curve_sets
        .get(&set_id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "This tab".to_string());
    let mut open = true;
    let mut confirmed = false;
    egui::Window::new("Unsaved changes")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.label(format!("\"{name}\" has unsaved changes. Close it anyway?"));
            ui.horizontal(|ui| {
                if ui.button("Close without saving").clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    app.workspace.pending_close = None;
                }
            });
        });
    if !open {
        app.workspace.pending_close = None;
    }
    if confirmed {
        close_tab(app, set_id);
    }
}

/// Lays out the three regions design doc §2.1 specifies as columns
/// (left rail, center graph+regimes+undo, right inspector) as
/// side-by-side panels (`SidePanel`s flanking a `CentralPanel`), not a
/// single vertical stack. Each panel manages its own scrolling, since
/// they can grow independently (a long receptor list shouldn't push the
/// graph down, nor should a long edit list).
fn tab_body(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    egui::SidePanel::left("workspace_left_rail")
        .resizable(true)
        .default_width(220.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                left_rail(ui, app, set_id);
            });
        });

    egui::SidePanel::right("workspace_right_inspector")
        .resizable(true)
        .default_width(320.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                right_inspector(ui, app, set_id);
            });
        });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let orientation = app.axis_orientation;
            pool_editor(ui, app, set_id, orientation);
            ui.separator();
            regimes_panel(ui, app, set_id);
            ui.separator();
            undo_redo_row(ui, app, set_id);
        });
    });
}

fn left_rail(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    let selected_curve_id = app
        .workspace
        .per_tab
        .get(&set_id)
        .and_then(|t| t.selected_curve);
    let graph_regime = app
        .workspace
        .per_tab
        .get(&set_id)
        .and_then(|t| t.graph_regime);

    // Which receptors the center graph shows: the whole pooled list, or
    // one regime's members. Settable here independently of selecting any
    // curve, so an empty regime filter (or empty pool) can still be
    // steered out of.
    let regime_names: Vec<String> = app
        .curve_sets
        .get(&set_id)
        .map(|s| s.regimes.iter().map(|r| r.name.clone()).collect())
        .unwrap_or_default();
    ui.label("Graph shows:");
    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(graph_regime.is_none(), "All pooled")
            .clicked()
        {
            set_graph_regime(app, set_id, None);
        }
        for (i, name) in regime_names.iter().enumerate() {
            if ui.selectable_label(graph_regime == Some(i), name).clicked() {
                set_graph_regime(app, set_id, Some(i));
            }
        }
    });
    ui.separator();

    new_curve_menu(ui, app, set_id);
    ui.separator();

    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    let members: HashSet<CurveId> = set
        .regimes
        .iter()
        .flat_map(|r| r.curves.iter().copied())
        .collect();
    let unaffiliated: Vec<(CurveId, String)> = set
        .curve_order
        .iter()
        .filter_map(|id| set.curves.get(id).map(|c| (*id, c.name.clone())))
        .filter(|(id, _)| !members.contains(id))
        .collect();
    let entries: Vec<(CurveId, String, bool)> = set
        .curve_order
        .iter()
        .filter_map(|id| {
            set.curves
                .get(id)
                .map(|c| (*id, c.name.clone(), members.contains(id)))
        })
        .collect();
    // (entries above own the names, so `set`'s borrow ends here - the
    // rail below mutates `app` freely via toggle/select.)

    ui.label("Receptor pool:");
    // ● = member of at least one colorspace regime, ○ = unaffiliated
    // (reported individually - never part of a luminance/chroma
    // calculation).
    let mut any_in_regime = false;
    for (id, name, in_regime) in &entries {
        any_in_regime |= *in_regime;
        let badge = if *in_regime { "●" } else { "○" };
        if ui
            .selectable_label(selected_curve_id == Some(*id), format!("{badge} {name}"))
            .clicked()
        {
            toggle_select_curve(app, set_id, *id);
        }
    }
    if !any_in_regime {
        ui.small("(no receptor is in a colorspace regime yet)");
    }

    if !unaffiliated.is_empty() {
        ui.separator();
        ui.label("Unaffiliated (in no regime):");
        for (id, name) in &unaffiliated {
            if ui
                .selectable_label(selected_curve_id == Some(*id), name)
                .clicked()
            {
                toggle_select_curve(app, set_id, *id);
            }
        }
    }
}

/// Switches which receptors the center graph shows, without touching
/// `selected_curve` - a stale selection from the other view would leave
/// the right inspector showing a curve that has nothing to do with the
/// graph now on screen.
fn set_graph_regime(app: &mut AppState, set_id: CurveSetId, regime: Option<usize>) {
    if let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) {
        if tab_ui.graph_regime != regime {
            tab_ui.graph_regime = regime;
            tab_ui.selected_curve = None;
            tab_ui.selected_point = None;
            tab_ui.noise_buf_for = None;
        }
    }
}

fn select_curve(app: &mut AppState, set_id: CurveSetId, curve_id: u64) {
    if let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) {
        tab_ui.selected_curve = Some(curve_id);
        tab_ui.selected_point = None;
        tab_ui.noise_buf_for = None;
        // Selecting straight from the rail shows the whole pool, so the
        // picked receptor is never hidden behind a regime filter.
        tab_ui.graph_regime = None;
    }
}

/// Clicking an already-selected curve in the rail deselects it, so "no
/// curve selected" (nothing drawn/editable on the graph, the inspector
/// showing its placeholder) is reachable from the rail, not just the
/// initial per-tab default.
fn toggle_select_curve(app: &mut AppState, set_id: CurveSetId, curve_id: u64) {
    let already_selected = app
        .workspace
        .per_tab
        .get(&set_id)
        .and_then(|t| t.selected_curve)
        == Some(curve_id);
    if already_selected {
        if let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) {
            tab_ui.selected_curve = None;
            tab_ui.selected_point = None;
            tab_ui.noise_buf_for = None;
        }
    } else {
        select_curve(app, set_id, curve_id);
    }
}

/// Workspace only ever creates receptor (Sensitivity) curves - a visual
/// system's own defining data - keeping a clean line between "a visual
/// system" (edited here) and "something to measure with one"
/// (reflectance/radiance curves, edited in the Stimulus Editor window
/// instead, which also owns the generators). A new receptor joins the
/// pool; it also joins the currently-filtered regime when the graph
/// shows one, otherwise it starts unaffiliated (membership is edited in
/// the Colorspace regimes table).
fn new_curve_menu(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    let graph_regime = app
        .workspace
        .per_tab
        .get(&set_id)
        .and_then(|t| t.graph_regime);
    if ui.button("+ New Curve").clicked() {
        add_curve(
            app,
            set_id,
            SpectralCurve::new("New curve", CurveType::Sensitivity)
                .with_points(vec![(400.0, 0.5), (700.0, 0.5)]),
            graph_regime,
        );
    }
}

fn add_curve(app: &mut AppState, set_id: CurveSetId, curve: SpectralCurve, regime: Option<usize>) {
    let id = app.alloc_id();
    if let Some(set) = app.curve_sets.get_mut(&set_id) {
        set.curves.insert(id, curve);
        set.curve_order.push(id);
        if let Some(idx) = regime {
            if let Some(r) = set.regimes.get_mut(idx) {
                r.curves.push(id);
            }
        }
        set.revision += 1;
        set.dirty = true;
    }
    select_curve(app, set_id, id);
}

/// Clones the visible curve list - the whole pool, or (when the graph's
/// regime filter is active) one regime's members - into a plain
/// `Vec<SpectralCurve>`, runs the existing (reused as-is) multi-curve
/// graph editor against it, then writes any changes back into the
/// id-keyed storage - lets this window reuse `multi_curve_editor`
/// unmodified rather than rewriting it to be id-aware.
fn pool_editor(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    orientation: AxisOrientation,
) {
    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    let graph_regime = app
        .workspace
        .per_tab
        .get(&set_id)
        .and_then(|t| t.graph_regime);
    let regime_name: Option<String> = graph_regime
        .and_then(|i| set.regimes.get(i))
        .map(|r| r.name.clone());
    let ids: Vec<u64> = match graph_regime {
        // A stale filter (regime removed while it was active) degrades
        // to the whole pool rather than a blank pane.
        Some(i) if i < set.regimes.len() => set.regimes[i].curves.clone(),
        _ => set.curve_order.clone(),
    };
    if ids.is_empty() {
        match (&graph_regime, regime_name) {
            (Some(_), Some(name)) => {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 180, 40),
                    format!(
                        "The \"{name}\" regime has no member receptors yet - tick some in the \
                         Colorspace regimes table below, or switch the graph back to \
                         \"All pooled\"."
                    ),
                );
            }
            _ => {
                ui.label(
                    "No receptor curves in this set yet - use \"+ New Curve\" in the left rail.",
                );
            }
        }
        return;
    }
    let visible = ids.len();
    let mut curves: Vec<SpectralCurve> = ids
        .iter()
        .map(|id| {
            set.curves
                .get(id)
                .cloned()
                .unwrap_or_else(|| SpectralCurve::new("?", CurveType::Sensitivity))
        })
        .collect();

    let tab_ui = app.workspace.per_tab.get(&set_id);
    let mut selected_curve_idx: Option<usize> = tab_ui
        .and_then(|t| t.selected_curve)
        .and_then(|id| ids.iter().position(|&i| i == id));
    let mut selected_point = tab_ui.and_then(|t| t.selected_point);

    let (wl_min, wl_max) = multi_curve_editor::multi_curve_editor(
        ui,
        &mut curves,
        &mut selected_curve_idx,
        &mut selected_point,
        orientation,
    );

    write_back_curves(app, set_id, &ids, curves);
    if let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) {
        let new_selected_id = selected_curve_idx.and_then(|idx| ids.get(idx).copied());
        if new_selected_id != tab_ui.selected_curve {
            tab_ui.noise_buf_for = None;
        }
        tab_ui.selected_curve = new_selected_id;
        tab_ui.selected_point = selected_point;
    }

    // The subjective spectrum is a property of the whole receptor pool,
    // so it spans the full pool regardless of the graph filter.
    subjective_spectrum_bar(ui, app, set_id, wl_min, wl_max, orientation);

    ui.horizontal(|ui| {
        if ui.button("Remove this curve").clicked() {
            if let Some(id) = selected_curve_idx.and_then(|idx| ids.get(idx).copied()) {
                remove_curve(app, set_id, id);
            }
        }
    });
    if visible > HIGH_N_WARNING_THRESHOLD {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            format!(
                "⚠ N={visible} is high - each edit to this set pauses briefly (around a \
                 second or more above N≈{HIGH_N_WARNING_THRESHOLD}) while its adaptation \
                 matrices are re-derived."
            ),
        );
    }
}

/// Draws the "subjective spectrum" bar beneath the graph: what this
/// specific visual system's own *whole receptor pool* renders a given
/// wavelength as, rather than the physical wavelength->color mapping the
/// gradient bar above it shows. Uses the same curve-integral-or-override
/// luminance weight the rest of the app already treats as each
/// receptor's effective contribution/density, since eta (the more
/// literal "density" field) is too often left unset to use directly -
/// most fixtures would render this bar entirely black if it required
/// eta on every curve.
fn subjective_spectrum_bar(
    ui: &mut egui::Ui,
    app: &AppState,
    set_id: CurveSetId,
    wl_min: f64,
    wl_max: f64,
    orientation: AxisOrientation,
) {
    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    if set.curve_order.is_empty() {
        return;
    }
    let core = set.to_core();
    let weights = core.luminance_weights(1.0);
    let weighted: Vec<(egui::Color32, &SpectralCurve, f64)> = core
        .curves
        .iter()
        .zip(weights.iter())
        .map(|(c, &w)| (display_color(c), c, w))
        .collect();

    ui.label("Subjective spectrum (this system's own weighted receptor response, for comparison with the physical spectrum above):");
    let desired_size = egui::Vec2::new(ui.available_width(), plot_axis::SPECTRUM_BAR_HEIGHT);
    let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let axes = plot_axis::PlotAxes {
        rect,
        wl_min,
        wl_max,
        val_min: 0.0,
        val_max: 1.0,
        orientation,
    };
    plot_axis::draw_subjective_spectrum_bar(&painter, &axes, rect, &weighted);
}

/// Writes `curves` (edited by `multi_curve_editor`, e.g. a dragged
/// point) back into `set_id`'s id-keyed storage, bumping `revision`
/// only if something actually changed - this is what makes a graph
/// edit in Workspace invalidate `TransformCache` and show up in an
/// already-open Comparison window on its next repaint (design doc §3).
fn write_back_curves(
    app: &mut AppState,
    set_id: CurveSetId,
    ids: &[u64],
    curves: Vec<SpectralCurve>,
) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let mut changed = false;
    for (id, curve) in ids.iter().zip(curves) {
        if set.curves.get(id) != Some(&curve) {
            changed = true;
        }
        set.curves.insert(*id, curve);
    }
    if changed {
        set.revision += 1;
        set.dirty = true;
    }
}

/// Removes a receptor from the pool entirely: from every regime's
/// member list (dropping its slot from any parallel per-regime
/// luminance/opponent weight vectors) and from the pool itself.
fn remove_curve(app: &mut AppState, set_id: CurveSetId, curve_id: u64) {
    if let Some(set) = app.curve_sets.get_mut(&set_id) {
        set.curve_order.retain(|&id| id != curve_id);
        set.curves.remove(&curve_id);
        for r in set.regimes.iter_mut() {
            if let Some(pos) = r.curves.iter().position(|&id| id == curve_id) {
                r.curves.remove(pos);
                if let Some(w) = r.luminance_weights.as_mut() {
                    if pos < w.len() {
                        w.remove(pos);
                    }
                }
                if let Some(v) = r.opponent_contrasts.as_mut() {
                    for c in v.iter_mut() {
                        if pos < c.weights.len() {
                            c.weights.remove(pos);
                        }
                    }
                }
            }
        }
        set.revision += 1;
        set.dirty = true;
    }
    if let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) {
        if tab_ui.selected_curve == Some(curve_id) {
            tab_ui.selected_curve = None;
            tab_ui.selected_point = None;
        }
    }
}

/// The colorspace-regimes block: add/rename/remove regimes, edit
/// membership (which pool receptors belong where), per-regime luminance
/// and opponent overrides, and the derived light-level band. Structural
/// validation lives here too (§2.2, §9.4).
fn regimes_panel(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    ui.heading("Colorspace regimes");
    ui.label(
        "A regime is a named colorspace: its own set of pool receptors, luminance wiring, \
         opponent contrasts, and (implicitly, from its members' operating windows) a \
         light-level band. Regimes are structurally distinct - scotopic/mesopic/photopic \
         aren't blends of each other, they're separate receiver configurations over \
         overlapping receptors (frog §9.4).",
    );
    ui.separator();

    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    let pool: Vec<(CurveId, String)> = set
        .curve_order
        .iter()
        .filter_map(|id| set.curves.get(id).map(|c| (*id, c.name.clone())))
        .collect();
    let regime_count = set.regimes.len();
    let regime_names: Vec<String> = set.regimes.iter().map(|r| r.name.clone()).collect();
    let pool_def_weights = set.to_core().luminance_weights(1.0);

    if regime_count == 0 {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            "This visual system has no colorspace regimes: no receptor belongs to a \
             colorspace, so luminance, chroma, and ΔS are all unavailable (mantis shrimp \
             §10.2). Add a regime below to build one; every receptor reports individually \
             until a regime claims it.",
        );
    } else {
        membership_matrix(ui, app, set_id, &pool, &regime_names);
    }

    // Per-regime sections are re-read from `app` by their helpers, so
    // the whole list is processed even while sections mutate the set.
    let mut remove_idx: Option<usize> = None;
    for i in 0..regime_count {
        regime_section(ui, app, set_id, i, &pool_def_weights, &mut remove_idx);
    }
    if let Some(i) = remove_idx {
        remove_regime(app, set_id, i);
    }

    ui.horizontal(|ui| {
        if ui.button("+ Add regime").clicked() {
            add_regime(app, set_id);
        }
    });
}

/// One checkbox per (receptor, regime) cell: the membership table. Cell
/// toggles are collected during painting and applied after, so the grid
/// renders a consistent snapshot even though several regimes' member
/// lists can change at once.
fn membership_matrix(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    pool: &[(CurveId, String)],
    regime_names: &[String],
) {
    ui.label("Membership - which pool receptors each regime uses:");
    let mut toggled: Vec<(usize, CurveId, bool)> = Vec::new();
    egui::Grid::new(("regime_membership", set_id))
        .striped(true)
        .show(ui, |ui| {
            ui.label("Receptor");
            for name in regime_names {
                ui.label(name);
            }
            ui.end_row();
            let Some(set) = app.curve_sets.get(&set_id) else {
                return;
            };
            for (cid, name) in pool {
                ui.label(name);
                for (ri, r) in set.regimes.iter().enumerate() {
                    let mut on = r.curves.contains(cid);
                    if ui.checkbox(&mut on, "").changed() {
                        toggled.push((ri, *cid, on));
                    }
                }
                ui.end_row();
            }
        });
    for (ri, cid, on) in toggled {
        if let Some(set) = app.curve_sets.get_mut(&set_id) {
            if let Some(r) = set.regimes.get_mut(ri) {
                let mut changed = false;
                if on {
                    if !r.curves.contains(&cid) {
                        r.curves.push(cid);
                        changed = true;
                    }
                } else if let Some(pos) = r.curves.iter().position(|&id| id == cid) {
                    r.curves.remove(pos);
                    if let Some(w) = r.luminance_weights.as_mut() {
                        if pos < w.len() {
                            w.remove(pos);
                        }
                    }
                    if let Some(v) = r.opponent_contrasts.as_mut() {
                        for c in v.iter_mut() {
                            if pos < c.weights.len() {
                                c.weights.remove(pos);
                            }
                        }
                    }
                    changed = true;
                }
                if changed {
                    set.revision += 1;
                    set.dirty = true;
                }
            }
        }
    }
}

/// One regime's collapsible section: name, derived light-level band,
/// per-regime luminance override, opponent contrasts, and its
/// natural-scene auto-generator.
fn regime_section(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    idx: usize,
    pool_def_weights: &[f64],
    remove_me: &mut Option<usize>,
) {
    let info: Option<(String, usize)> = app
        .curve_sets
        .get(&set_id)
        .and_then(|s| s.regimes.get(idx))
        .map(|r| (r.name.clone(), r.curves.len()));
    let Some((name, n)) = info else {
        return;
    };
    let open_all = app
        .curve_sets
        .get(&set_id)
        .map(|s| s.regimes.len() <= 1)
        .unwrap_or(true);

    egui::CollapsingHeader::new(format!("{name} — {n} member(s)"))
        .id_salt(("regime_section", set_id, idx))
        .default_open(open_all)
        .show(ui, |ui| {
            regime_name_field(ui, app, set_id, idx);
            regime_band_field(ui, app, set_id, idx);
            regime_luminance_field(ui, app, set_id, idx, pool_def_weights);
            ui.separator();
            opponent_contrast_strip(ui, app, set_id, idx);
            natural_scene_generator(ui, app, set_id, idx);
        });
    if ui.button(format!("🗑 Remove \"{name}\"")).clicked() {
        *remove_me = Some(idx);
    }
}

fn regime_name_field(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId, idx: usize) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(regime) = set.regimes.get_mut(idx) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Name:");
        if ui.text_edit_singleline(&mut regime.name).changed() {
            set.revision += 1;
            set.dirty = true;
        }
    });
}

/// The light-level band this regime is active in: the intersection of
/// its members' effective operating windows. Shown descriptively (and a
/// disjoint window flagged); the Comparison window's dominance plot
/// draws the actual activity curves (§2.2).
fn regime_band_field(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId, idx: usize) {
    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    let core = set.to_core();
    let Some(regime) = core.regimes.get(idx) else {
        return;
    };
    let (lo, hi) = regime_band_of(&core, regime);
    let disjoint = matches!((lo, hi), (Some(l), Some(h)) if l > h);
    ui.label(format!(
        "Light-level band (intersection of member operating windows): {}",
        band_text(lo, hi)
    ));
    if disjoint {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            "⚠ Member windows don't overlap, so this regime can never be active at any light level.",
        );
    }
}

/// Per-regime luminance-weight override (`None` = automatic per-receptor
/// fallback). Toggling it on materializes the current integral-derived
/// pool defaults as explicit override values; toggling off (or Reset)
/// returns to automatic.
fn regime_luminance_field(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    idx: usize,
    pool_def_weights: &[f64],
) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(regime) = set.regimes.get_mut(idx) else {
        return;
    };
    let member_names: Vec<String> = regime
        .curves
        .iter()
        .filter_map(|id| set.curves.get(id).map(|c| c.name.clone()))
        .collect();
    let mut override_on = regime.luminance_weights.is_some();
    let mut activate: Option<bool> = None;
    ui.horizontal(|ui| {
        ui.label("Luminance weights:");
        if ui
            .checkbox(&mut override_on, "override per-receptor weights")
            .changed()
        {
            activate = Some(override_on);
        }
    });
    if let Some(on) = activate {
        if on {
            let vals: Vec<f64> = regime
                .curves
                .iter()
                .filter_map(|id| {
                    set.curve_order
                        .iter()
                        .position(|o| o == id)
                        .and_then(|i| pool_def_weights.get(i))
                        .copied()
                })
                .collect();
            regime.luminance_weights = Some(vals);
        } else {
            regime.luminance_weights = None;
        }
        set.revision += 1;
        set.dirty = true;
    }
    if let Some(w) = regime.luminance_weights.as_mut() {
        for (name, val) in member_names.iter().zip(w.iter_mut()) {
            ui.horizontal(|ui| {
                ui.label(name);
                ui.add(egui::DragValue::new(val).speed(0.01));
            });
        }
        if ui.button("Reset to auto (per-receptor)").clicked() {
            regime.luminance_weights = None;
            set.revision += 1;
            set.dirty = true;
        }
    }
}

fn add_regime(app: &mut AppState, set_id: CurveSetId) {
    if let Some(set) = app.curve_sets.get_mut(&set_id) {
        let name = format!("Regime {}", set.regimes.len() + 1);
        set.regimes.push(AppRegime::new(name, Vec::new()));
        set.revision += 1;
        set.dirty = true;
    }
}

fn remove_regime(app: &mut AppState, set_id: CurveSetId, idx: usize) {
    let pruned = app
        .curve_sets
        .get_mut(&set_id)
        .map(|set| {
            if idx < set.regimes.len() {
                set.regimes.remove(idx);
                set.revision += 1;
                set.dirty = true;
                true
            } else {
                false
            }
        })
        .unwrap_or(false);
    if pruned {
        // Re-point derived views at surviving regimes rather than leaving
        // stale indexes behind (graph filter here, Comparison's active
        // regime for this species).
        if let Some(tab) = app.workspace.per_tab.get_mut(&set_id) {
            tab.graph_regime = match tab.graph_regime {
                Some(g) if g == idx => None,
                Some(g) if g > idx => Some(g - 1),
                other => other,
            };
        }
        app.prune_comparison_active_regime(set_id);
    }
}

/// A regime's effective light-level band is the intersection of its
/// members' effective operating windows (curve override else set-level
/// default, exactly how `Pipeline::build` resolves them): the regime is
/// active only where *every* member receptor is within its window.
/// `None` on a side = unbounded on that side; `lo > hi` = the member
/// windows are disjoint, so no light level can ever satisfy them all.
fn regime_band_of(core: &CoreCurveSet, regime: &CoreRegime) -> (Option<f64>, Option<f64>) {
    let mut lo: Option<f64> = None; // max of the lower bounds
    let mut hi: Option<f64> = None; // min of the upper bounds
    for &i in &regime.curves {
        let c = &core.curves[i as usize];
        if let Some(m) = c.illumination_min.or(core.illumination_min) {
            lo = Some(lo.map_or(m, |l: f64| l.max(m)));
        }
        if let Some(m) = c.illumination_max.or(core.illumination_max) {
            hi = Some(hi.map_or(m, |h: f64| h.min(m)));
        }
    }
    (lo, hi)
}

fn band_text(lo: Option<f64>, hi: Option<f64>) -> String {
    match (lo, hi) {
        (None, None) => "unbounded - active at every light level".to_string(),
        (Some(l), None) => format!("≥ {l:.3e} W/m²"),
        (None, Some(h)) => format!("≤ {h:.3e} W/m²"),
        (Some(l), Some(h)) if l <= h => format!("{l:.3e}..{h:.3e} W/m²"),
        (Some(l), Some(h)) => format!("disjoint ({l:.3e} > {h:.3e} W/m²)"),
    }
}

/// Opponent contrast definitions for one regime - signed weight per
/// regime member curve; empty means the auto-computed per-receptor
/// fallback (§2.2).
fn opponent_contrast_strip(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    regime_idx: usize,
) {
    ui.label("Opponent contrast definitions - signed weight per regime member curve; empty means auto-computed per-receptor fallback:");
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(regime) = set.regimes.get_mut(regime_idx) else {
        return;
    };
    let curve_names: Vec<String> = regime
        .curves
        .iter()
        .filter_map(|id| set.curves.get(id).map(|c| c.name.clone()))
        .collect();
    let n = curve_names.len();
    let mut remove_idx: Option<usize> = None;

    if let Some(contrasts) = regime.opponent_contrasts.as_mut() {
        egui::Grid::new(("opponent_contrast_table", set_id, regime_idx))
            .striped(true)
            .show(ui, |ui| {
                ui.label("Name");
                for name in &curve_names {
                    ui.label(name);
                }
                ui.label("");
                ui.end_row();
                for (i, contrast) in contrasts.iter_mut().enumerate() {
                    ui.text_edit_singleline(&mut contrast.name);
                    for w in contrast.weights.iter_mut() {
                        ui.add(egui::DragValue::new(w).speed(0.1));
                    }
                    if ui.button("Remove").clicked() {
                        remove_idx = Some(i);
                    }
                    ui.end_row();
                }
            });
    }

    if let Some(i) = remove_idx {
        if let Some(contrasts) = regime.opponent_contrasts.as_mut() {
            contrasts.remove(i);
        }
        set.revision += 1;
        set.dirty = true;
    }

    ui.horizontal(|ui| {
        if ui.button("+ Add contrast").clicked() {
            regime
                .opponent_contrasts
                .get_or_insert_with(Vec::new)
                .push(OpponentContrast {
                    name: "New contrast".to_string(),
                    weights: vec![0.0; n],
                });
            set.revision += 1;
            set.dirty = true;
        }
        if !regime
            .opponent_contrasts
            .as_ref()
            .is_none_or(|v| v.is_empty())
            && ui.button("Clear all (use automatic fallback)").clicked()
        {
            regime.opponent_contrasts = None;
            set.revision += 1;
            set.dirty = true;
        }
    });
}

/// The "auto-generate from natural-scene statistics" control (§4.2.6):
/// derives one regime's `opponent_contrasts` from the receptor-response
/// covariance of either a parametric smoothness model or a user-picked
/// weighted corpus from the shared stimulus library, replacing the table
/// above outright on click (no confirmation - same precedent as "Clear
/// all", and the per-tab undo history already makes this one reversible
/// step).
fn natural_scene_generator(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    regime_idx: usize,
) {
    ui.separator();
    ui.collapsing("Auto-generate from natural-scene statistics", |ui| {
        let Some(tab_ui) = app.workspace.per_tab.get(&set_id) else {
            return;
        };
        let mut mode = tab_ui.natural_scene.mode;
        let mut illuminant = tab_ui.natural_scene.illuminant;
        let mut correlation_length_nm = tab_ui.natural_scene.correlation_length_nm;
        let mut corpus_weights = tab_ui.natural_scene.corpus_weights.clone();
        let mut shared_weight_buf = tab_ui.natural_scene.corpus_shared_weight;
        let mut status = tab_ui.natural_scene.status.clone();

        let colorspace_curves: Vec<SpectralCurve> = app
            .curve_sets
            .get(&set_id)
            .and_then(|s| s.regimes.get(regime_idx).map(|r| (s, r)))
            .map(|(s, r)| {
                r.curves
                    .iter()
                    .filter_map(|id| s.curves.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default();

        ui.label(
            "Derives this regime's opponent contrasts from the receptor-response \
             covariance of a \"natural scene\" ensemble - the empirical basis for human \
             L-M/S-(L+M) (Buchsbaum & Gottschalk 1983; Ruderman, Cronin & Chiao 1998; \
             Wachtler, Lee & Sejnowski 2001) - instead of only literature pairings or \
             manual entry.",
        );

        if colorspace_curves.len() < 2 {
            ui.colored_label(
                egui::Color32::from_rgb(220, 180, 40),
                format!(
                    "This regime has {} member receptor(s). Extraction needs at least 2 to \
                     find opponent structure - with fewer, the per-receptor automatic \
                     fallback is the meaningful option.",
                    colorspace_curves.len()
                ),
            );
        }

        ui.horizontal(|ui| {
            ui.label("Ensemble:");
            ui.selectable_value(&mut mode, NaturalSceneMode::Parametric, "Parametric model");
            ui.selectable_value(&mut mode, NaturalSceneMode::Corpus, "Custom corpus");
        });

        ui.horizontal(|ui| {
            ui.label("Illuminant:");
            let name = illuminant
                .and_then(|id| app.luminants.get(&id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "(choose one)".to_string());
            egui::ComboBox::from_id_salt(("natural_scene_illuminant", set_id, regime_idx))
                .selected_text(name)
                .show_ui(ui, |ui| {
                    for &id in &app.luminant_order {
                        if let Some(curve) = app.luminants.get(&id) {
                            if ui
                                .selectable_label(illuminant == Some(id), &curve.name)
                                .clicked()
                            {
                                illuminant = Some(id);
                            }
                        }
                    }
                });
        });

        match mode {
            NaturalSceneMode::Parametric => {
                ui.horizontal(|ui| {
                    ui.label("Correlation length (nm):");
                    ui.add(
                        egui::DragValue::new(&mut correlation_length_nm)
                            .range(1.0..=500.0)
                            .speed(1.0),
                    )
                    .on_hover_text(
                        "How smoothly natural reflectance spectra are assumed to vary with \
                         wavelength (Maloney 1986) - a tunable approximation, not a measured \
                         constant.",
                    );
                });
            }
            NaturalSceneMode::Corpus => {
                ui.label(
                    "Select reflectance/radiance curves (or whole corpora/sub-corpora, \
                     organized in the Stimulus Editor) to use as the corpus:",
                );
                let eligible_kind = |c: &SpectralCurve| {
                    !matches!(
                        c.quantity,
                        QuantityKind::Absorption | QuantityKind::Sensitivity
                    )
                };
                let tree = stimulus_picker::corpus_tree(app, &eligible_kind);
                let is_included = |id: CurveId| corpus_weights.contains_key(&id);
                let mut toggled: Vec<(CurveId, bool)> = Vec::new();
                let mut on_toggle = |ids: &[CurveId], included: bool| {
                    toggled.extend(ids.iter().map(|&id| (id, included)));
                };
                stimulus_picker::render_corpus_checklist(ui, &tree, &is_included, &mut on_toggle);
                for (id, included) in toggled {
                    if included {
                        corpus_weights.insert(id, 1.0);
                    } else {
                        corpus_weights.remove(&id);
                    }
                }

                if corpus_weights.len() < 2 {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 180, 40),
                        "Select at least 2 curves with positive weight.",
                    );
                } else {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{} curve(s) selected. Apply weight:",
                            corpus_weights.len()
                        ));
                        ui.add(
                            egui::DragValue::new(&mut shared_weight_buf)
                                .speed(0.1)
                                .range(0.0..=f64::MAX),
                        );
                        if ui.button("Apply to all selected").clicked() {
                            for w in corpus_weights.values_mut() {
                                *w = shared_weight_buf;
                            }
                        }
                    });
                }
            }
        }

        let can_generate = illuminant.is_some()
            && colorspace_curves.len() >= 2
            && (mode == NaturalSceneMode::Parametric || corpus_weights.len() >= 2);
        if ui
            .add_enabled(can_generate, egui::Button::new("Generate"))
            .clicked()
        {
            if let Some(illum_curve) = illuminant.and_then(|id| app.luminants.get(&id).cloned()) {
                let result = match mode {
                    NaturalSceneMode::Parametric => {
                        Some(natural_opponent::derive_from_parametric_model(
                            &colorspace_curves,
                            &illum_curve,
                            correlation_length_nm,
                            STEP_NM,
                        ))
                    }
                    NaturalSceneMode::Corpus => {
                        let entries: Vec<natural_opponent::CorpusEntry> = corpus_weights
                            .iter()
                            .filter_map(|(id, &weight)| {
                                app.stimulus_curves
                                    .get(id)
                                    .map(|e| natural_opponent::CorpusEntry {
                                        curve: &e.curve,
                                        weight,
                                    })
                            })
                            .collect();
                        natural_opponent::derive_from_corpus(
                            &colorspace_curves,
                            &illum_curve,
                            &entries,
                            STEP_NM,
                        )
                    }
                };
                match result {
                    Some(contrasts) => {
                        let n = contrasts.len();
                        if let Some(set) = app.curve_sets.get_mut(&set_id) {
                            if let Some(r) = set.regimes.get_mut(regime_idx) {
                                r.opponent_contrasts = Some(contrasts);
                            }
                            set.revision += 1;
                            set.dirty = true;
                        }
                        status = format!("Generated {n} contrast(s).");
                    }
                    None => {
                        status = "Not enough corpus data to generate (need >= 2 entries with \
                                   positive total weight)."
                            .to_string();
                    }
                }
            }
        }
        if !status.is_empty() {
            ui.label(&status);
        }

        if let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) {
            tab_ui.natural_scene.mode = mode;
            tab_ui.natural_scene.illuminant = illuminant;
            tab_ui.natural_scene.correlation_length_nm = correlation_length_nm;
            tab_ui.natural_scene.corpus_weights = corpus_weights;
            tab_ui.natural_scene.corpus_shared_weight = shared_weight_buf;
            tab_ui.natural_scene.status = status;
        }
    });
}

fn undo_redo_row(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    let can_undo = app
        .workspace
        .per_tab
        .get(&set_id)
        .map(|t| t.undo.can_undo())
        .unwrap_or(false);
    let can_redo = app
        .workspace
        .per_tab
        .get(&set_id)
        .map(|t| t.undo.can_redo())
        .unwrap_or(false);

    ui.horizontal(|ui| {
        ui.label("Undo/redo (Ctrl+Z / Ctrl+Shift+Z) - shared across every editing surface above:");
        if ui
            .add_enabled(can_undo, egui::Button::new("Undo"))
            .clicked()
        {
            apply_undo(app, set_id);
        }
        if ui
            .add_enabled(can_redo, egui::Button::new("Redo"))
            .clicked()
        {
            apply_redo(app, set_id);
        }
    });

    let (undo_pressed, redo_pressed) = ui.input(|i| {
        let ctrl = i.modifiers.command;
        let z = i.key_pressed(egui::Key::Z);
        let y = i.key_pressed(egui::Key::Y);
        (
            ctrl && z && !i.modifiers.shift,
            ctrl && (y || (z && i.modifiers.shift)),
        )
    });
    if undo_pressed && can_undo {
        apply_undo(app, set_id);
    }
    if redo_pressed && can_redo {
        apply_redo(app, set_id);
    }
}

fn apply_undo(app: &mut AppState, set_id: CurveSetId) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) else {
        return;
    };
    let mut snapshot = set.snapshot();
    tab_ui.undo.undo(&mut snapshot);
    set.restore(&snapshot);
    invalidate_inspector_buffers(tab_ui);
}

/// The inspector's text fields are parsed back into the curve every
/// frame, so after undo/redo restores a curve they must be reloaded from
/// it - otherwise the stale pre-undo text is re-parsed and written right
/// back, silently reverting the undo.
fn invalidate_inspector_buffers(tab_ui: &mut crate::state::TabUiState) {
    tab_ui.noise_buf_for = None;
    tab_ui.sat_buf_for = None;
    tab_ui.ill_buf_for = None;
    tab_ui.set_ill_buf_for = None;
}

fn apply_redo(app: &mut AppState, set_id: CurveSetId) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) else {
        return;
    };
    let mut snapshot = set.snapshot();
    tab_ui.undo.redo(&mut snapshot);
    set.restore(&snapshot);
    invalidate_inspector_buffers(tab_ui);
}

fn right_inspector(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    ui.heading("Inspector");
    set_level_window_fields(ui, app, set_id);

    let selected_curve = app
        .workspace
        .per_tab
        .get(&set_id)
        .and_then(|t| t.selected_curve);
    let Some(curve_id) = selected_curve else {
        ui.separator();
        ui.label("Select a receptor in the left rail to inspect it.");
        return;
    };

    let membership: Vec<String> = app
        .curve_sets
        .get(&set_id)
        .map(|s| {
            s.regimes
                .iter()
                .filter(|r| r.curves.contains(&curve_id))
                .map(|r| r.name.clone())
                .collect()
        })
        .unwrap_or_default();

    ui.separator();
    if membership.is_empty() {
        ui.small(
            "Unaffiliated to any colorspace regime - reported individually, never part of \
             a luminance/chroma calculation.",
        );
    } else {
        ui.small(format!(
            "Member of colorspace regime(s): {}.",
            membership.join(", ")
        ));
    }
    ui.separator();
    curve_name_field(ui, app, set_id, curve_id);

    ui.separator();
    selected_point_field(ui, app, set_id, curve_id);

    ui.separator();
    noise_row(ui, app, set_id, curve_id);

    ui.separator();
    saturation_field(ui, app, set_id, curve_id);

    ui.separator();
    illumination_window_field(ui, app, set_id, curve_id);

    ui.separator();
    metadata_section(ui, app, set_id, curve_id);

    ui.separator();
    validation_flags(ui, app, set_id);
}

/// Just the curve's name - a full wavelength/value point table here
/// would overwhelm the panel once a curve has more than a handful of
/// points. Points are still fully editable directly on the graph in the
/// center pane (drag to move, double-click empty space to add,
/// right-click or Delete to remove) - this field is for the one thing
/// the graph itself has no gesture for.
fn curve_name_field(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId, curve_id: u64) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(curve) = set.curves.get_mut(&curve_id) else {
        return;
    };
    ui.label("Curve name:");
    if ui.text_edit_singleline(&mut curve.name).changed() {
        set.revision += 1;
        set.dirty = true;
    }
}

/// Numeric wavelength/value fields for the one point currently selected
/// on the graph - not a table of every point (that overwhelmed the
/// panel and was removed), just the one a user is already looking at.
fn selected_point_field(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId, curve_id: u64) {
    let Some(idx) = app
        .workspace
        .per_tab
        .get(&set_id)
        .and_then(|t| t.selected_point)
    else {
        ui.label("Select a point on the graph to edit its exact values.");
        return;
    };
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(curve) = set.curves.get_mut(&curve_id) else {
        return;
    };
    if idx >= curve.points.len() {
        return;
    }
    let (mut wl, mut v) = curve.points[idx];
    ui.label(format!(
        "Selected point ({} of {}):",
        idx + 1,
        curve.points.len()
    ));
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("λ (nm):");
        changed |= ui.add(egui::DragValue::new(&mut wl).speed(1.0)).changed();
        ui.label("value:");
        changed |= ui.add(egui::DragValue::new(&mut v).speed(0.01)).changed();
    });
    if changed {
        curve.points[idx] = (wl, v);
        set.revision += 1;
        set.dirty = true;
    }
}

fn noise_row(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId, curve_id: u64) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let default_w = {
        let idx = set.curve_order.iter().position(|&id| id == curve_id);
        idx.map(|i| set.luminance_weight_default(i)).unwrap_or(0.0)
    };
    let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) else {
        return;
    };
    if tab_ui.noise_buf_for != Some(curve_id) {
        if let Some(curve) = set.curves.get(&curve_id) {
            tab_ui.noise_omega_buf = curve.omega.map(|x| x.to_string()).unwrap_or_default();
            tab_ui.noise_eta_buf = curve.eta.map(|x| x.to_string()).unwrap_or_default();
            tab_ui.noise_w_buf = curve
                .luminance_weight
                .map(|x| x.to_string())
                .unwrap_or_else(|| format!("{default_w:.4}"));
        }
        tab_ui.noise_buf_for = Some(curve_id);
    }

    ui.label("Per-receptor noise (ω, η) and luminance weight (w_i). Blank ω/η means \"no data\"; w_i shows an override or the integral-derived default.");
    // Each field on its own line, not one wide horizontal row - three
    // side-by-side labeled fields were forcing this panel wider than it
    // needed to be, since a `SidePanel` grows to fit an unwrapped
    // `horizontal` row's full natural width.
    ui.label("ω (Weber fraction):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.noise_omega_buf).desired_width(80.0));
    ui.label("η (relative density):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.noise_eta_buf).desired_width(80.0));
    ui.label("w_i (luminance weight):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.noise_w_buf).desired_width(80.0));

    if let Some(curve) = set.curves.get_mut(&curve_id) {
        let omega = parse_opt_keep_old(&tab_ui.noise_omega_buf, curve.omega);
        let eta = parse_opt_keep_old(&tab_ui.noise_eta_buf, curve.eta);
        let w = parse_opt_keep_old(&tab_ui.noise_w_buf, curve.luminance_weight);
        if omega != curve.omega || eta != curve.eta || w != curve.luminance_weight {
            curve.omega = omega;
            curve.eta = eta;
            curve.luminance_weight = w;
            set.revision += 1;
            set.dirty = true;
        }
        if curve.luminance_weight.is_none() {
            tab_ui.noise_w_buf = format!("{default_w:.4}");
        }
    }
}

/// Receptor saturation cap - shown for every receptor curve, whether it
/// belongs to a colorspace regime or not, since both kinds' activations
/// are capped the same way. Keeps its own buffer-refresh key
/// (`sat_buf_for`) rather than sharing `noise_buf_for`, because
/// `noise_row` (which owns that key) only runs on one view.
fn saturation_field(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId, curve_id: u64) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) else {
        return;
    };
    let Some(curve) = set.curves.get_mut(&curve_id) else {
        return;
    };
    if tab_ui.sat_buf_for != Some(curve_id) {
        tab_ui.sat_buf = curve.saturation.map(|x| x.to_string()).unwrap_or_default();
        tab_ui.sat_buf_for = Some(curve_id);
    }
    ui.label("Receptor saturation (max signal; blank = no cap):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.sat_buf).desired_width(80.0));
    let sat = parse_opt_keep_old(&tab_ui.sat_buf, curve.saturation);
    if sat != curve.saturation {
        curve.saturation = sat;
        set.revision += 1;
        set.dirty = true;
    }
}

fn parse_opt_keep_old(s: &str, old: Option<f64>) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        t.parse::<f64>().ok().or(old)
    }
}

/// Receptor operating window (§2.2) editing: min/max ambient irradiance
/// (W/m²) and shoulder softness (decades). A receptor "sees" only the
/// light its own sensitivity passes (E_i = ∫S·I dλ), so both bounds are
/// in that per-receptor W/m² scale. Blank = no bound on that side; the
/// set-level defaults (`set_level_window_fields`) then apply, and the
/// effective window is shown beneath.
fn illumination_window_field(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    curve_id: u64,
) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let (set_min, set_max) = (set.illumination_min, set.illumination_max);
    let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) else {
        return;
    };
    let Some(curve) = set.curves.get_mut(&curve_id) else {
        return;
    };
    if tab_ui.ill_buf_for != Some(curve_id) {
        tab_ui.ill_min_buf = curve
            .illumination_min
            .map(|x| x.to_string())
            .unwrap_or_default();
        tab_ui.ill_max_buf = curve
            .illumination_max
            .map(|x| x.to_string())
            .unwrap_or_default();
        tab_ui.ill_soft_buf = curve
            .illumination_softness
            .map(|x| x.to_string())
            .unwrap_or_default();
        tab_ui.ill_buf_for = Some(curve_id);
    }
    ui.label("Operating window - ambient irradiance (W/m², as seen by this receptor) this receptor operates in:");
    ui.label("Min (W/m²; blank = set default):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.ill_min_buf).desired_width(80.0));
    ui.label("Max (W/m²; blank = unbounded upward):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.ill_max_buf).desired_width(80.0));
    ui.label("Shoulder softness (decades; blank = default 0.5):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.ill_soft_buf).desired_width(80.0));
    let min = parse_opt_keep_old(&tab_ui.ill_min_buf, curve.illumination_min);
    let max = parse_opt_keep_old(&tab_ui.ill_max_buf, curve.illumination_max);
    let soft = parse_opt_keep_old(&tab_ui.ill_soft_buf, curve.illumination_softness);
    if min != curve.illumination_min
        || max != curve.illumination_max
        || soft != curve.illumination_softness
    {
        curve.illumination_min = min;
        curve.illumination_max = max;
        curve.illumination_softness = soft;
        set.revision += 1;
        set.dirty = true;
    }
    let eff_min = curve.illumination_min.or(set_min);
    let eff_max = curve.illumination_max.or(set_max);
    let eff = match (eff_min, eff_max) {
        (None, None) => "unbounded".to_string(),
        (Some(lo), None) => format!("{lo} W/m² and up"),
        (None, Some(hi)) => format!("up to {hi} W/m²"),
        (Some(lo), Some(hi)) => format!("{lo}..{hi} W/m²"),
    };
    ui.small(format!("Effective window: {eff}"));
}

/// The whole system's operating-window defaults (§2.2), edited in one
/// place and applied to every receptor without its own bound. Same units
/// as the per-curve fields (W/m², decades). Undo-tracked like any other
/// set edit.
fn set_level_window_fields(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) else {
        return;
    };
    if tab_ui.set_ill_buf_for != Some(set_id) {
        tab_ui.set_ill_min_buf = set
            .illumination_min
            .map(|x| x.to_string())
            .unwrap_or_default();
        tab_ui.set_ill_max_buf = set
            .illumination_max
            .map(|x| x.to_string())
            .unwrap_or_default();
        tab_ui.set_ill_soft_buf = set
            .illumination_softness
            .map(|x| x.to_string())
            .unwrap_or_default();
        tab_ui.set_ill_buf_for = Some(set_id);
    }
    ui.label("System operating-window defaults (any receptor without its own bound uses these):");
    ui.label("Default min (W/m²; blank = none):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.set_ill_min_buf).desired_width(80.0));
    ui.label("Default max (W/m²; blank = none):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.set_ill_max_buf).desired_width(80.0));
    ui.label("Default softness (decades; blank = default 0.5):");
    ui.add(egui::TextEdit::singleline(&mut tab_ui.set_ill_soft_buf).desired_width(80.0));
    let min = parse_opt_keep_old(&tab_ui.set_ill_min_buf, set.illumination_min);
    let max = parse_opt_keep_old(&tab_ui.set_ill_max_buf, set.illumination_max);
    let soft = parse_opt_keep_old(&tab_ui.set_ill_soft_buf, set.illumination_softness);
    if min != set.illumination_min
        || max != set.illumination_max
        || soft != set.illumination_softness
    {
        set.illumination_min = min;
        set.illumination_max = max;
        set.illumination_softness = soft;
        set.revision += 1;
        set.dirty = true;
    }
}

fn metadata_section(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId, curve_id: u64) {
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let Some(curve) = set.curves.get_mut(&curve_id) else {
        return;
    };
    ui.label("Metadata / provenance:");
    let mut remove_key: Option<String> = None;
    for (k, v) in curve.metadata.iter_mut() {
        // Value field explicitly capped (egui's unset TextEdit default
        // is a fixed 280px, regardless of the field's actual content -
        // e.g. a short numeric value like a fixture's `lambda_max_nm`
        // metadata still wanted the full 280px) - this plus the key
        // label was forcing the inspector panel wider than intended,
        // which could crowd out the graph on its side of the window.
        ui.horizontal(|ui| {
            ui.label(k);
            ui.add(egui::TextEdit::singleline(v).desired_width(120.0));
            if ui.small_button("x").clicked() {
                remove_key = Some(k.clone());
            }
        });
    }
    if let Some(k) = remove_key {
        curve.metadata.remove(&k);
    }
    ui.horizontal(|ui| {
        if ui.button("+ Add field").clicked() {
            curve
                .metadata
                .insert(format!("field_{}", curve.metadata.len()), String::new());
        }
    });
}

/// Structural warnings, always visible at the bottom of the inspector
/// (§2.2, §9.4): malformed operating windows, partial η coverage (ΔS
/// reliability), empty regimes, unaffiliated receptors, and disjoint
/// member windows (a regime that can never turn on).
fn validation_flags(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    if set.has_invalid_illumination_window() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            "⚠ A receptor operating window is malformed (min ≥ max, or a non-positive bound) and is being treated as unbounded.",
        );
    }
    if set.has_partial_eta_coverage() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            "⚠ Some but not all receptors have η set, so the ΔS comparison metric will be incomplete/unreliable for this system.",
        );
    }
    for r in &set.regimes {
        if r.curves.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(220, 180, 40),
                format!(
                    "⚠ Regime \"{}\" is empty - it has no member receptors, so it contributes nothing.",
                    r.name
                ),
            );
        }
    }
    let members: HashSet<CurveId> = set
        .regimes
        .iter()
        .flat_map(|r| r.curves.iter().copied())
        .collect();
    let unaffiliated: Vec<String> = set
        .curve_order
        .iter()
        .filter(|id| !members.contains(id))
        .filter_map(|id| set.curves.get(id).map(|c| c.name.clone()))
        .collect();
    if !unaffiliated.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(200, 160, 60),
            format!(
                "{} receptor(s) belong to no colorspace regime - reported individually, \
                 never part of a luminance/chroma calculation: {}.",
                unaffiliated.len(),
                unaffiliated.join(", ")
            ),
        );
    }
    let core = set.to_core();
    for regime in &core.regimes {
        let (lo, hi) = regime_band_of(&core, regime);
        if let (Some(l), Some(h)) = (lo, hi) {
            if l > h {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 180, 40),
                    format!(
                        "⚠ Regime \"{}\" has disjoint member operating windows ({l:.2e} > {h:.2e} \
                         W/m²) - it can never be active at any light level.",
                        regime.name
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// Every Workspace section must fit inside the window it's drawn in.
    /// A section with a minimum width larger than the window silently
    /// widens the whole layout past the window edge, which pushes the
    /// inspector off-screen and freezes the graph's width - see the tab
    /// bar's comment on why it avoids nested layouts in a wrapping row.
    #[test]
    fn every_section_fits_a_1000px_window() {
        let mut app = AppState::new();
        let set_id = app.workspace.open_tabs[0];
        // Select a curve so the inspector renders all its fields.
        let first = app.curve_sets[&set_id].curve_order[0];
        app.workspace
            .per_tab
            .get_mut(&set_id)
            .unwrap()
            .selected_curve = Some(first);
        let ctx = egui::Context::default();
        type Section = fn(&mut egui::Ui, &mut AppState, CurveSetId);
        let sections: Vec<(&str, Section)> = vec![
            ("menu_bar", |ui, app, _| menu_bar(ui, app)),
            ("tab_bar", |ui, app, _| tab_bar(ui, app)),
            ("left_rail", |ui, app, id| left_rail(ui, app, id)),
            ("regimes_panel", |ui, app, id| regimes_panel(ui, app, id)),
            ("opponent_contrast_strip", |ui, app, id| {
                opponent_contrast_strip(ui, app, id, 0)
            }),
            ("undo_redo_row", |ui, app, id| undo_redo_row(ui, app, id)),
            ("pool_editor", |ui, app, id| {
                pool_editor(ui, app, id, AxisOrientation::default())
            }),
            ("right_inspector", |ui, app, id| {
                right_inspector(ui, app, id)
            }),
        ];
        let mut too_wide = Vec::new();
        for (name, f) in sections {
            let mut w = 0.0;
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 900.0),
                    )),
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        f(ui, &mut app, set_id);
                        w = ui.min_rect().width();
                    });
                });
            }
            if w > 1000.0 {
                too_wide.push(format!("{name}: {w:.0}px"));
            }
        }
        assert!(
            too_wide.is_empty(),
            "sections wider than the window: {too_wide:?}"
        );
    }
}
