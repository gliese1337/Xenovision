//! Workspace window (design doc §2.1, mockup "Window 1"): tabbed Curve
//! Set editor - graph, point table, noise/luminance table, opponent
//! contrasts, metadata, all driven by the shared `AppState`.

use eframe::egui;
use xenovision_core::curve_set::OpponentContrast;
use xenovision_core::{fixture_library, CurveType, SpectralCurve};

use crate::multi_curve_editor;
use crate::plot_axis::{self, AxisOrientation};
use crate::state::{AppCurveSet, AppState, CurveListTab, CurveSetId};

fn display_color(curve: &SpectralCurve) -> egui::Color32 {
    xenovision_core::cie::curve_display_color(curve)
        .map(|(r, g, b)| {
            egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
        })
        .unwrap_or(egui::Color32::WHITE)
}

/// Above this N, deriving the adaptation matrix (done whenever the set
/// changes) pauses the UI noticeably: ~0.66s at N=32, ~2.4s at N=48
/// (`adaptation::tests::bench_realistic_n`).
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
/// stable link back to "which of the 12 built-ins is this". Tabs that
/// don't match any built-in name (custom systems) are left untouched.
fn restore_default_fixtures(app: &mut AppState) {
    let restored = fixture_library::restore_builtin_defaults();
    for core_set in restored {
        // Allocate ids for this fixture's curves *before* borrowing the
        // matching open tab, so the borrow checker sees the id counter
        // and `curve_sets` as the disjoint fields they are.
        let total = core_set.colorspace_curves.len() + core_set.isolated_curves.len();
        let base = app.alloc_id_range(total);
        let Some(set) = app
            .curve_sets
            .values_mut()
            .find(|s| s.name == core_set.name)
        else {
            continue;
        };
        // Rebuild from the restored core set with fresh ids - undo
        // history for this tab no longer applies to post-restore
        // content, so it's cleared too (below).
        let mut curves = std::collections::HashMap::new();
        let mut colorspace_curves = Vec::new();
        let mut counter = base;
        for c in core_set.colorspace_curves {
            colorspace_curves.push(counter);
            curves.insert(counter, c);
            counter += 1;
        }
        let mut isolated_curves = Vec::new();
        for c in core_set.isolated_curves {
            isolated_curves.push(counter);
            curves.insert(counter, c);
            counter += 1;
        }
        set.colorspace_curves = colorspace_curves;
        set.isolated_curves = isolated_curves;
        set.curves = curves;
        set.opponent_contrasts = core_set.opponent_contrasts;
        set.metadata = core_set.metadata;
        set.revision += 1;
        set.dirty = false;
        let set_id = set.id;
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
                    colorspace_curves: Vec::new(),
                    isolated_curves: Vec::new(),
                    curves: std::collections::HashMap::new(),
                    opponent_contrasts: Vec::new(),
                    metadata: std::collections::BTreeMap::new(),
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
    app.comparison.selected_species.retain(|&id| id != set_id);
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
/// (left rail, center graph+contrasts+undo, right inspector) as
/// side-by-side panels (`SidePanel`s flanking a `CentralPanel`), not a
/// single vertical stack. Each panel manages its own scrolling, since
/// they can grow independently (a long curve list shouldn't push the
/// graph down, nor should a long point table).
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
            let curve_tab = app
                .workspace
                .per_tab
                .get(&set_id)
                .map(|t| t.curve_tab)
                .unwrap_or_default();

            // Opponent contrasts are a colorspace-only concept (they're
            // weights over `colorspace_curves`) - showing them while
            // looking at the isolated-curve graph was both meaningless
            // and (per the design doc's isolated-curve semantics) wrong.
            match curve_tab {
                CurveListTab::Colorspace => {
                    colorspace_editor(ui, app, set_id, orientation);
                    ui.separator();
                    opponent_contrast_strip(ui, app, set_id);
                }
                CurveListTab::Isolated => {
                    isolated_editor(ui, app, set_id, orientation);
                }
            }

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
    let current_curve_tab = app
        .workspace
        .per_tab
        .get(&set_id)
        .map(|t| t.curve_tab)
        .unwrap_or_default();

    // Which graph (colorspace vs. isolated) the center pane shows is
    // settable here directly, independent of selecting any particular
    // curve - needed now that "no curve selected" is a valid state (you
    // can't rely on clicking a curve to pick the mode when there isn't
    // one to click yet, e.g. an empty isolated-curve list).
    ui.horizontal(|ui| {
        if ui
            .selectable_label(current_curve_tab == CurveListTab::Colorspace, "Colorspace")
            .clicked()
        {
            set_curve_tab(app, set_id, CurveListTab::Colorspace);
        }
        if ui
            .selectable_label(current_curve_tab == CurveListTab::Isolated, "Isolated")
            .clicked()
        {
            set_curve_tab(app, set_id, CurveListTab::Isolated);
        }
    });
    ui.separator();

    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    let colorspace_names: Vec<(u64, String)> = set
        .colorspace_curves
        .iter()
        .filter_map(|id| set.curves.get(id).map(|c| (*id, c.name.clone())))
        .collect();
    let isolated_names: Vec<(u64, String)> = set
        .isolated_curves
        .iter()
        .filter_map(|id| set.curves.get(id).map(|c| (*id, c.name.clone())))
        .collect();

    new_curve_menu(ui, app, set_id);
    ui.separator();

    ui.label("Colorspace curves:");
    for (id, name) in &colorspace_names {
        if ui
            .selectable_label(selected_curve_id == Some(*id), format!("● {name}"))
            .clicked()
        {
            toggle_select_curve(app, set_id, *id, CurveListTab::Colorspace);
        }
    }

    ui.separator();
    ui.label("Isolated curves:");
    for (id, name) in &isolated_names {
        if ui
            .selectable_label(selected_curve_id == Some(*id), format!("○ {name}"))
            .clicked()
        {
            toggle_select_curve(app, set_id, *id, CurveListTab::Isolated);
        }
    }
}

/// Switches which curve list the center pane shows, without touching
/// `selected_curve` unless the mode actually changes - a stale
/// selection from the other list would otherwise leave the right
/// inspector showing a curve that has nothing to do with the graph now
/// on screen.
fn set_curve_tab(app: &mut AppState, set_id: CurveSetId, tab: CurveListTab) {
    if let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) {
        if tab_ui.curve_tab != tab {
            tab_ui.curve_tab = tab;
            tab_ui.selected_curve = None;
            tab_ui.selected_point = None;
            tab_ui.noise_buf_for = None;
        }
    }
}

fn select_curve(app: &mut AppState, set_id: CurveSetId, curve_id: u64, tab: CurveListTab) {
    if let Some(tab_ui) = app.workspace.per_tab.get_mut(&set_id) {
        tab_ui.selected_curve = Some(curve_id);
        tab_ui.selected_point = None;
        tab_ui.curve_tab = tab;
        tab_ui.noise_buf_for = None;
    }
}

/// Clicking an already-selected curve in the rail deselects it, so "no
/// curve selected" (nothing drawn/editable on the graph, the inspector
/// showing its placeholder) is reachable from the rail, not just the
/// initial per-tab default.
fn toggle_select_curve(app: &mut AppState, set_id: CurveSetId, curve_id: u64, tab: CurveListTab) {
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
        select_curve(app, set_id, curve_id, tab);
    }
}

/// Workspace only ever creates receptor (Sensitivity) curves - a visual
/// system's own defining data. This used to also offer stimulus curves
/// and luminant generators here, which blurred the line this app now
/// draws between "a visual system" (edited here) and "something to
/// measure with one" (reflectance/radiance curves, edited in the
/// Stimulus Editor window instead, which also owns the generators).
fn new_curve_menu(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    if ui.button("+ New Curve").clicked() {
        add_curve(
            app,
            set_id,
            SpectralCurve::new("New curve", CurveType::Sensitivity)
                .with_points(vec![(400.0, 0.5), (700.0, 0.5)]),
            true,
        );
    }
}

fn add_curve(app: &mut AppState, set_id: CurveSetId, curve: SpectralCurve, colorspace: bool) {
    let id = app.alloc_id();
    if let Some(set) = app.curve_sets.get_mut(&set_id) {
        set.curves.insert(id, curve);
        if colorspace {
            set.colorspace_curves.push(id);
            for c in set.opponent_contrasts.iter_mut() {
                c.weights.push(0.0);
            }
        } else {
            set.isolated_curves.push(id);
        }
        set.revision += 1;
        set.dirty = true;
    }
    select_curve(
        app,
        set_id,
        id,
        if colorspace {
            CurveListTab::Colorspace
        } else {
            CurveListTab::Isolated
        },
    );
}

/// Clones the active curve list into a plain `Vec<SpectralCurve>`, runs
/// the existing (reused as-is) multi-curve graph editor against it, then
/// writes any changes back into the id-keyed storage - lets this window
/// reuse `multi_curve_editor` unmodified rather than rewriting it to be
/// id-aware.
fn colorspace_editor(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    orientation: AxisOrientation,
) {
    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    let isolated_count = set.isolated_curves.len();
    if set.colorspace_curves.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            format!(
                "This visual system has no colorspace curves yet{}. Luminance and \
                 chroma for a system with none are always exactly 0.",
                if isolated_count > 0 {
                    format!(
                        " - all {isolated_count} of its receptor classes are isolated \
                         instead; see the Comparison window for their individual activations"
                    )
                } else {
                    String::new()
                }
            ),
        );
        return;
    }

    let ids: Vec<u64> = set.colorspace_curves.clone();
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

    subjective_spectrum_bar(ui, app, set_id, wl_min, wl_max, orientation);

    let colorspace_curve_count = ids.len();
    ui.horizontal(|ui| {
        if ui.button("Remove this curve").clicked() {
            if let Some(id) = selected_curve_idx.and_then(|idx| ids.get(idx).copied()) {
                remove_curve(app, set_id, id, true);
            }
        }
    });
    if colorspace_curve_count > HIGH_N_WARNING_THRESHOLD {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            format!(
                "⚠ N={colorspace_curve_count} is high - each edit to this set pauses \
                 briefly (around a second or more above N≈{HIGH_N_WARNING_THRESHOLD}) \
                 while its adaptation matrix is re-derived."
            ),
        );
    }
}

/// Draws the "subjective spectrum" bar beneath the colorspace graph:
/// what this specific visual system's own receptor mix renders a given
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
    if set.colorspace_curves.is_empty() {
        return;
    }
    let core = set.to_core();
    let weights = core.luminance_weights(1.0);
    let weighted: Vec<(egui::Color32, &SpectralCurve, f64)> = core
        .colorspace_curves
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

fn blank_isolated_curve() -> SpectralCurve {
    SpectralCurve::new("New isolated curve", CurveType::Sensitivity)
        .with_points(vec![(400.0, 0.5), (700.0, 0.5)])
}

fn isolated_editor(
    ui: &mut egui::Ui,
    app: &mut AppState,
    set_id: CurveSetId,
    orientation: AxisOrientation,
) {
    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    if set.isolated_curves.is_empty() {
        ui.label("No isolated curves in this set yet.");
        if ui.button("+ Add isolated curve").clicked() {
            add_curve(app, set_id, blank_isolated_curve(), false);
        }
        return;
    }
    let ids: Vec<u64> = set.isolated_curves.clone();
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

    multi_curve_editor::multi_curve_editor(
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

    ui.horizontal(|ui| {
        if ui.button("+ Add isolated curve").clicked() {
            add_curve(app, set_id, blank_isolated_curve(), false);
        }
        if ui.button("Remove this isolated curve").clicked() {
            if let Some(id) = selected_curve_idx.and_then(|idx| ids.get(idx).copied()) {
                remove_curve(app, set_id, id, false);
            }
        }
    });
}

/// Writes `curves` (edited by `multi_curve_editor`, e.g. a dragged
/// point) back into `set_id`'s id-keyed storage, bumping `revision`
/// only if something actually changed - this is what makes a graph
/// edit in Workspace invalidate `TransformCache` and show up in an
/// already-open Comparison window on its next repaint (design doc §3).
/// A prior version of this write-back never bumped `revision` at all,
/// so dragged points silently never invalidated the cache.
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

fn remove_curve(app: &mut AppState, set_id: CurveSetId, curve_id: u64, colorspace: bool) {
    if let Some(set) = app.curve_sets.get_mut(&set_id) {
        if colorspace {
            if let Some(idx) = set.colorspace_curves.iter().position(|&id| id == curve_id) {
                set.colorspace_curves.remove(idx);
                for c in set.opponent_contrasts.iter_mut() {
                    if idx < c.weights.len() {
                        c.weights.remove(idx);
                    }
                }
            }
        } else if let Some(idx) = set.isolated_curves.iter().position(|&id| id == curve_id) {
            set.isolated_curves.remove(idx);
        }
        set.curves.remove(&curve_id);
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

fn opponent_contrast_strip(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    ui.label("Opponent contrast definitions - signed weight per colorspace curve; empty means auto-computed per-receptor fallback:");
    let Some(set) = app.curve_sets.get_mut(&set_id) else {
        return;
    };
    let curve_names: Vec<String> = set
        .colorspace_curves
        .iter()
        .filter_map(|id| set.curves.get(id).map(|c| c.name.clone()))
        .collect();
    let n = curve_names.len();
    let mut remove_idx: Option<usize> = None;

    egui::Grid::new(("opponent_contrast_table", set_id))
        .striped(true)
        .show(ui, |ui| {
            ui.label("Name");
            for name in &curve_names {
                ui.label(name);
            }
            ui.label("");
            ui.end_row();
            for (i, contrast) in set.opponent_contrasts.iter_mut().enumerate() {
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

    if let Some(i) = remove_idx {
        set.opponent_contrasts.remove(i);
        set.revision += 1;
    }

    ui.horizontal(|ui| {
        if ui.button("+ Add contrast").clicked() {
            set.opponent_contrasts.push(OpponentContrast {
                name: "New contrast".to_string(),
                weights: vec![0.0; n],
            });
            set.revision += 1;
        }
        if !set.opponent_contrasts.is_empty()
            && ui.button("Clear all (use automatic fallback)").clicked()
        {
            set.opponent_contrasts.clear();
            set.revision += 1;
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
    let selected_curve = app
        .workspace
        .per_tab
        .get(&set_id)
        .and_then(|t| t.selected_curve);
    let Some(curve_id) = selected_curve else {
        ui.label("Select a curve in the left rail to inspect it.");
        return;
    };
    let is_colorspace = app
        .curve_sets
        .get(&set_id)
        .map(|s| s.colorspace_curves.contains(&curve_id))
        .unwrap_or(false);

    ui.heading("Inspector");
    curve_name_field(ui, app, set_id, curve_id);

    ui.separator();
    selected_point_field(ui, app, set_id, curve_id);

    if is_colorspace {
        ui.separator();
        noise_row(ui, app, set_id, curve_id);
    }

    ui.separator();
    saturation_field(ui, app, set_id, curve_id);

    ui.separator();
    metadata_section(ui, app, set_id, curve_id);

    ui.separator();
    validation_flags(ui, app, set_id);
}

/// Just the curve's name - the inspector used to also show a full
/// wavelength/value point table here, but with more than a handful of
/// points it overwhelmed the whole panel. Points are still fully
/// editable directly on the graph in the center pane (drag to move,
/// double-click empty space to add, right-click or Delete to remove) -
/// this field is for the one thing the graph itself has no gesture for.
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
        let idx = set.colorspace_curves.iter().position(|&id| id == curve_id);
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

/// Receptor saturation cap - shown for every receptor curve, colorspace
/// or isolated, since both kinds' activations are capped the same way.
/// Keeps its own buffer-refresh key (`sat_buf_for`) rather than sharing
/// `noise_buf_for`, because `noise_row` (which owns that key) only runs
/// for colorspace curves.
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

fn validation_flags(ui: &mut egui::Ui, app: &mut AppState, set_id: CurveSetId) {
    let Some(set) = app.curve_sets.get(&set_id) else {
        return;
    };
    if set.has_partial_eta_coverage() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            "⚠ Some but not all receptors have η set, so the ΔS comparison metric will be incomplete/unreliable for this system.",
        );
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
        let first = app.curve_sets[&set_id].colorspace_curves[0];
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
            ("opponent_contrast_strip", |ui, app, id| {
                opponent_contrast_strip(ui, app, id)
            }),
            ("undo_redo_row", |ui, app, id| undo_redo_row(ui, app, id)),
            ("colorspace_editor", |ui, app, id| {
                colorspace_editor(ui, app, id, AxisOrientation::default())
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
