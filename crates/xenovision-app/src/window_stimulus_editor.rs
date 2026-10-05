//! Stimulus Editor window (formerly "Image Extraction," broadened per
//! user feedback into the general home for the shared stimulus-curve
//! library - reflectance/radiance curves, never a visual system's own
//! Sensitivity curves, which stay in Workspace). Image import is one of
//! several ways to create a stimulus curve here, not the window's whole
//! purpose:
//!
//! - Blank curve
//! - Edit a copy of an existing library curve
//! - Derive reflectance from a radiance curve divided by a luminant
//! - Derive radiance from a reflectance curve multiplied by a luminant
//! - Import a region from a hyperspectral/multispectral image
//! - Import from pasted text/CSV (e.g. a USGS spectral library file)
//!
//! The image-loading/band-preview/region-selection logic is a direct
//! port of this app's prior single-page `ui_hyperspectral` module (and,
//! after that, the first draft of this window) - already covered by
//! `xenovision_core::hyperspectral`'s own tests. What's new here is
//! everything else: the library itself (shared with Comparison via
//! `AppState::stimulus_curves`), the other creation modes, and
//! unload-with-unsaved-changes-warning, mirroring how Workspace tabs
//! now close.

use eframe::egui;
use xenovision_core::hyperspectral::{self, HyperspectralCube, MatVariableInfo, Region};
use xenovision_core::sensor_presets::{self, SensorBandPreset};
use xenovision_core::{illumination, CurveType, QuantityKind, SpectralCurve};

use crate::multi_curve_editor;
use crate::state::{AppState, CurveId, StimulusEntry};
use crate::stimulus_picker;

const STEP_NM: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    Rectangle,
    Polygon,
}

/// Which creation flow (if any) the center pane is showing instead of
/// the normal selected-curve editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CreationMode {
    #[default]
    None,
    Image,
    Csv,
    DeriveReflectance,
    DeriveRadiance,
    BulkDeriveReflectance,
    Notch,
}

pub struct StimulusEditorState {
    pub selected: Option<CurveId>,
    pub selected_point: Option<usize>,
    pub creation_mode: CreationMode,
    pub pending_unload: Option<CurveId>,
    pub status: String,
    pub save_path_buf: String,

    // --- image import ---
    pub image_path: String,
    pub mat_path: String,
    pub image_status: String,
    pub cube: Option<HyperspectralCube>,
    cube_generation: usize,
    pub band: usize,
    texture: Option<egui::TextureHandle>,
    texture_key: Option<(usize, usize)>,
    pub mat_variables: Vec<MatVariableInfo>,
    pub mat_cube_choice: Option<usize>,
    pub mat_wavelength_choice: Option<usize>,
    pub selection_mode: SelectionMode,
    drag_start: Option<egui::Pos2>,
    pub polygon_points: Vec<(f64, f64)>,
    pub region: Option<Region>,
    pub as_illumination: bool,
    pub sensor_presets: Vec<SensorBandPreset>,
    pub sensor_preset_picker: usize,
    pub manual_wavelengths: String,
    /// Pixel stride for "Extract every pixel as curves" (1 = every
    /// pixel) - a practical throttle on how many curves one bulk import
    /// creates, not a hard cap (§4.2.6's addendum).
    pub pixel_import_stride: usize,
    pub pixel_import_status: String,

    // --- text/CSV import ---
    pub csv_name: String,
    pub csv_text: String,
    pub csv_status: String,

    // --- derive reflectance/radiance ---
    pub derive_source: Option<CurveId>,
    pub derive_luminant: Option<CurveId>,
    pub derive_status: String,

    // --- bulk derive reflectance ---
    /// Which stimulus curves are checked in the bulk-convert checklist.
    pub bulk_derive_selection: std::collections::HashSet<CurveId>,

    // --- custom notch -> absorption curve ---
    pub notch_presets: Vec<xenovision_core::blackbody::NamedNotch>,
    pub notch: xenovision_core::blackbody::NamedNotch,
    pub notch_status: String,
}

impl Default for StimulusEditorState {
    fn default() -> Self {
        StimulusEditorState {
            selected: None,
            selected_point: None,
            creation_mode: CreationMode::default(),
            pending_unload: None,
            status: String::new(),
            save_path_buf: "stimulus.json".to_string(),

            image_path: String::new(),
            mat_path: String::new(),
            image_status: String::new(),
            cube: None,
            cube_generation: 0,
            band: 0,
            texture: None,
            texture_key: None,
            mat_variables: Vec::new(),
            mat_cube_choice: None,
            mat_wavelength_choice: None,
            selection_mode: SelectionMode::Rectangle,
            drag_start: None,
            polygon_points: Vec::new(),
            region: None,
            as_illumination: false,
            sensor_presets: sensor_presets::load_sensor_presets(),
            sensor_preset_picker: 0,
            manual_wavelengths: String::new(),
            pixel_import_stride: 1,
            pixel_import_status: String::new(),

            csv_name: String::new(),
            csv_text: String::new(),
            csv_status: String::new(),

            derive_source: None,
            derive_luminant: None,
            derive_status: String::new(),

            bulk_derive_selection: std::collections::HashSet::new(),

            notch_presets: xenovision_core::blackbody::load_notch_presets(),
            notch: xenovision_core::blackbody::NamedNotch {
                name: "Custom notch".to_string(),
                notch: xenovision_core::blackbody::AbsorptionNotch {
                    center_nm: 550.0,
                    width_nm: 5.0,
                    depth: 0.5,
                },
            },
            notch_status: String::new(),
        }
    }
}

pub fn ui(ui: &mut egui::Ui, app: &mut AppState) {
    egui::SidePanel::left("stimulus_editor_rail")
        .resizable(true)
        .default_width(240.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                left_rail(ui, app);
            });
        });
    egui::CentralPanel::default().show_inside(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| match app.stimulus_editor.creation_mode {
            CreationMode::None => editor_panel(ui, app),
            CreationMode::Image => image_import_panel(ui, app),
            CreationMode::Csv => csv_import_panel(ui, app),
            CreationMode::DeriveReflectance => derive_panel(ui, app, true),
            CreationMode::DeriveRadiance => derive_panel(ui, app, false),
            CreationMode::BulkDeriveReflectance => bulk_derive_reflectance_panel(ui, app),
            CreationMode::Notch => notch_panel(ui, app),
        });
    });
    unload_confirmation(ui, app);
}

fn quantity_label(q: &QuantityKind) -> &'static str {
    match q {
        QuantityKind::Unspecified => "Unspecified",
        QuantityKind::Reflectance => "Reflectance",
        QuantityKind::Transmittance => "Transmittance",
        QuantityKind::Sensitivity => "Sensitivity",
        QuantityKind::Radiance { .. } => "Radiance",
        QuantityKind::Absorption => "Absorption",
    }
}

fn left_rail(ui: &mut egui::Ui, app: &mut AppState) {
    new_stimulus_menu(ui, app);
    ui.separator();

    let tree = stimulus_picker::corpus_tree(app, &|_| true);
    render_stimulus_tree(ui, app, &tree);
}

/// Walks a corpus tree rendering `stimulus_row` (select-for-editing +
/// unload) at each leaf, nesting named corpora as `CollapsingHeader`s -
/// the Stimulus Editor's own curve list isn't a multi-select checklist
/// like the other three pickers, so it doesn't go through
/// `stimulus_picker::render_corpus_checklist`.
fn render_stimulus_tree(ui: &mut egui::Ui, app: &mut AppState, nodes: &[stimulus_picker::CorpusNode]) {
    for node in nodes {
        if !node.is_named_group {
            stimulus_row(ui, app, node.curves[0].0);
            continue;
        }
        let count = node.all_curves().len();
        let plural = if count == 1 { "" } else { "s" };
        ui.collapsing(format!("{} ({count} curve{plural})", node.name), |ui| {
            for (id, _) in &node.curves {
                stimulus_row(ui, app, *id);
            }
            render_stimulus_tree(ui, app, &node.children);
        });
    }
}

fn stimulus_row(ui: &mut egui::Ui, app: &mut AppState, id: CurveId) {
    let Some(entry) = app.stimulus_curves.get(&id) else {
        return;
    };
    let label = format!(
        "{} [{}]{}",
        entry.curve.name,
        quantity_label(&entry.curve.quantity),
        if entry.dirty { " *" } else { "" }
    );
    ui.horizontal(|ui| {
        if ui
            .selectable_label(app.stimulus_editor.selected == Some(id), label)
            .clicked()
        {
            app.stimulus_editor.selected = Some(id);
            app.stimulus_editor.selected_point = None;
            app.stimulus_editor.creation_mode = CreationMode::None;
        }
        if ui.small_button("×").clicked() {
            request_unload(app, id);
        }
    });
}

/// Adds `curve` as a new library entry, selects it, and leaves creation
/// mode - the common tail of every "+ New stimulus" action.
fn add_and_select(app: &mut AppState, curve: SpectralCurve) -> CurveId {
    let id = app.alloc_id();
    app.stimulus_order.push(id);
    app.stimulus_curves.insert(
        id,
        StimulusEntry {
            curve,
            file_path: None,
            dirty: true,
        },
    );
    app.stimulus_editor.selected = Some(id);
    app.stimulus_editor.selected_point = None;
    app.stimulus_editor.creation_mode = CreationMode::None;
    id
}

/// Adds many curves at once (a bulk import, §4.2.6's addendum) without
/// reassigning selection/creation-mode on every single one the way
/// `add_and_select` does - the caller decides what, if anything, should
/// end up selected afterward.
fn add_many(app: &mut AppState, curves: Vec<SpectralCurve>) {
    if curves.is_empty() {
        return;
    }
    let first_id = app.alloc_id_range(curves.len());
    for (i, curve) in curves.into_iter().enumerate() {
        let id = first_id + i as u64;
        app.stimulus_order.push(id);
        app.stimulus_curves.insert(
            id,
            StimulusEntry {
                curve,
                file_path: None,
                dirty: true,
            },
        );
    }
}

fn new_stimulus_menu(ui: &mut egui::Ui, app: &mut AppState) {
    ui.menu_button("+ New stimulus ▾", |ui| {
        if ui.button("Blank curve").clicked() {
            let curve = SpectralCurve::new("New stimulus", CurveType::Reflectance)
                .with_points(vec![(400.0, 0.5), (700.0, 0.5)])
                .with_quantity(QuantityKind::Reflectance);
            add_and_select(app, curve);
            ui.close_menu();
        }
        let order = app.stimulus_order.clone();
        if !order.is_empty() {
            ui.menu_button("Edit a copy of...", |ui| {
                for id in &order {
                    let Some(entry) = app.stimulus_curves.get(id) else {
                        continue;
                    };
                    let name = entry.curve.name.clone();
                    if ui.button(name).clicked() {
                        let mut copy = entry.curve.clone();
                        copy.name = format!("{} (copy)", copy.name);
                        add_and_select(app, copy);
                        ui.close_menu();
                    }
                }
            });
        }
        if ui.button("Derive reflectance from radiance...").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::DeriveReflectance;
            app.stimulus_editor.derive_source = None;
            app.stimulus_editor.derive_luminant = app.comparison.reference_luminant;
            app.stimulus_editor.derive_status.clear();
            ui.close_menu();
        }
        if ui.button("Derive radiance from reflectance...").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::DeriveRadiance;
            app.stimulus_editor.derive_source = None;
            app.stimulus_editor.derive_luminant = app.comparison.reference_luminant;
            app.stimulus_editor.derive_status.clear();
            ui.close_menu();
        }
        if ui.button("Bulk-convert radiance to reflectance...").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::BulkDeriveReflectance;
            app.stimulus_editor.bulk_derive_selection.clear();
            app.stimulus_editor.derive_luminant = app.comparison.reference_luminant;
            app.stimulus_editor.derive_status.clear();
            ui.close_menu();
        }
        if ui.button("Import from image...").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::Image;
            ui.close_menu();
        }
        if ui.button("Import from text/CSV...").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::Csv;
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Blank absorption curve").clicked() {
            let curve = SpectralCurve::new("New absorption", CurveType::Absorption)
                .with_points(vec![(400.0, 1.0), (700.0, 1.0)])
                .with_quantity(QuantityKind::Absorption);
            add_and_select(app, curve);
            ui.close_menu();
        }
        let presets = app.stimulus_editor.notch_presets.clone();
        ui.menu_button("Absorption curve from notch preset", |ui| {
            for preset in &presets {
                if ui.button(&preset.name).clicked() {
                    add_and_select(app, preset.notch.absorption_curve(preset.name.clone()));
                    ui.close_menu();
                }
            }
        });
        if ui.button("Custom notch...").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::Notch;
            app.stimulus_editor.notch_status.clear();
            ui.close_menu();
        }
    });
}

/// A single Gaussian absorption notch (center, width, depth) as an
/// Absorption curve - and optionally saved into the notch preset library
/// for reuse.
fn notch_panel(ui: &mut egui::Ui, app: &mut AppState) {
    ui.heading("Custom notch");
    ui.label(
        "A Gaussian dip: at its center it lets through (1 - depth) of the light, rising \
         back to 1 (no absorption) a few widths away.",
    );
    let st = &mut app.stimulus_editor;
    ui.horizontal(|ui| {
        ui.label("Name:");
        ui.text_edit_singleline(&mut st.notch.name);
    });
    ui.horizontal(|ui| {
        ui.label("Center (nm):");
        ui.add(egui::DragValue::new(&mut st.notch.notch.center_nm).speed(1.0));
    });
    ui.horizontal(|ui| {
        ui.label("Width (nm, std. dev.):");
        ui.add(
            egui::DragValue::new(&mut st.notch.notch.width_nm)
                .speed(0.1)
                .range(0.01..=1000.0),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Depth (0-1):");
        ui.add(
            egui::DragValue::new(&mut st.notch.notch.depth)
                .speed(0.01)
                .range(0.0..=1.0),
        );
    });
    let mut create = false;
    ui.horizontal(|ui| {
        create = ui.button("Create").clicked();
        if ui.button("Save as notch preset").clicked() {
            let st = &mut app.stimulus_editor;
            let preset = st.notch.clone();
            match st.notch_presets.iter_mut().find(|p| p.name == preset.name) {
                Some(existing) => *existing = preset,
                None => st.notch_presets.push(preset),
            }
            st.notch_status =
                match xenovision_core::blackbody::save_notch_presets(&st.notch_presets) {
                    Ok(()) => format!("Saved preset \"{}\"", st.notch.name),
                    Err(e) => format!("Couldn't save presets: {e}"),
                };
        }
        if ui.button("Cancel").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::None;
        }
    });
    if !app.stimulus_editor.notch_status.is_empty() {
        ui.label(&app.stimulus_editor.notch_status);
    }
    if create {
        let n = app.stimulus_editor.notch.clone();
        add_and_select(app, n.notch.absorption_curve(n.name));
    }
}

fn request_unload(app: &mut AppState, id: CurveId) {
    let dirty = app
        .stimulus_curves
        .get(&id)
        .map(|e| e.dirty)
        .unwrap_or(false);
    if dirty {
        app.stimulus_editor.pending_unload = Some(id);
    } else {
        unload(app, id);
    }
}

fn unload(app: &mut AppState, id: CurveId) {
    app.stimulus_curves.remove(&id);
    app.stimulus_order.retain(|&i| i != id);
    app.comparison.selected_stimuli.retain(|&i| i != id);
    if app.stimulus_editor.selected == Some(id) {
        app.stimulus_editor.selected = None;
    }
    if app.stimulus_editor.pending_unload == Some(id) {
        app.stimulus_editor.pending_unload = None;
    }
}

/// An inline modal (egui has no native confirm dialog) warning before
/// discarding a dirty stimulus curve - same pattern as Workspace's
/// tab-close confirmation.
fn unload_confirmation(ui: &mut egui::Ui, app: &mut AppState) {
    let Some(id) = app.stimulus_editor.pending_unload else {
        return;
    };
    let name = app
        .stimulus_curves
        .get(&id)
        .map(|e| e.curve.name.clone())
        .unwrap_or_else(|| "This curve".to_string());
    let mut open = true;
    let mut confirmed = false;
    egui::Window::new("Unsaved changes")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.label(format!("\"{name}\" has unsaved changes. Unload it anyway?"));
            ui.horizontal(|ui| {
                if ui.button("Unload without saving").clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    app.stimulus_editor.pending_unload = None;
                }
            });
        });
    if !open {
        app.stimulus_editor.pending_unload = None;
    }
    if confirmed {
        unload(app, id);
    }
}

/// Normal mode: name, quantity kind/unit, the graph, the one selected
/// point's numeric fields, and save/export - for whichever curve is
/// selected in the rail.
fn editor_panel(ui: &mut egui::Ui, app: &mut AppState) {
    let Some(id) = app.stimulus_editor.selected else {
        ui.label(
            "Select a stimulus curve in the left rail, or create one with \"+ New stimulus\".",
        );
        return;
    };
    if !app.stimulus_curves.contains_key(&id) {
        app.stimulus_editor.selected = None;
        return;
    }

    ui.horizontal(|ui| {
        ui.label("Name:");
        let entry = app.stimulus_curves.get_mut(&id).unwrap();
        if ui.text_edit_singleline(&mut entry.curve.name).changed() {
            entry.dirty = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Corpus:");
        let entry = app.stimulus_curves.get_mut(&id).unwrap();
        if stimulus_picker::corpus_path_field(ui, &mut entry.curve.metadata) {
            entry.dirty = true;
        }
    })
    .response
    .on_hover_text(
        "A \"/\"-separated path grouping this curve into a corpus/sub-corpus for bulk \
         selection elsewhere (e.g. \"forest.tif/Canopy\") - blank means ungrouped.",
    );

    quantity_kind_editor(ui, app, id);
    reflectance_range_warning(ui, app, id);

    let entry = app.stimulus_curves.get(&id).unwrap();
    let mut curves = vec![entry.curve.clone()];
    let mut selected_idx = Some(0usize);
    let orientation = app.axis_orientation;
    multi_curve_editor::multi_curve_editor(
        ui,
        &mut curves,
        &mut selected_idx,
        &mut app.stimulus_editor.selected_point,
        orientation,
    );
    if let Some(edited) = curves.into_iter().next() {
        let entry = app.stimulus_curves.get_mut(&id).unwrap();
        if entry.curve != edited {
            entry.curve = edited;
            entry.dirty = true;
        }
    }

    selected_point_field(ui, app, id);

    ui.separator();
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut app.stimulus_editor.save_path_buf);
        if ui.button("Save As...").clicked() {
            let entry = app.stimulus_curves.get_mut(&id).unwrap();
            let mut set = xenovision_core::CurveSet::new(entry.curve.name.clone());
            set.colorspace_curves = vec![entry.curve.clone()];
            app.stimulus_editor.status = match set.save_to_file(&app.stimulus_editor.save_path_buf)
            {
                Ok(()) => {
                    entry.file_path =
                        Some(std::path::PathBuf::from(&app.stimulus_editor.save_path_buf));
                    entry.dirty = false;
                    format!("Saved to {}", app.stimulus_editor.save_path_buf)
                }
                Err(e) => format!("Save failed: {e}"),
            };
        }
    });
    if !app.stimulus_editor.status.is_empty() {
        ui.label(&app.stimulus_editor.status);
    }
}

fn selected_point_field(ui: &mut egui::Ui, app: &mut AppState, id: CurveId) {
    let Some(idx) = app.stimulus_editor.selected_point else {
        ui.label("Select a point on the graph to edit its exact values.");
        return;
    };
    let Some(entry) = app.stimulus_curves.get_mut(&id) else {
        return;
    };
    if idx >= entry.curve.points.len() {
        return;
    }
    let (mut wl, mut v) = entry.curve.points[idx];
    ui.label(format!(
        "Selected point ({} of {}):",
        idx + 1,
        entry.curve.points.len()
    ));
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("λ (nm):");
        changed |= ui.add(egui::DragValue::new(&mut wl).speed(1.0)).changed();
        ui.label("value:");
        changed |= ui.add(egui::DragValue::new(&mut v).speed(0.01)).changed();
    });
    if changed {
        entry.curve.points[idx] = (wl, v);
        entry.dirty = true;
    }
}

/// Stimulus curves are never Sensitivity-typed (that's reserved for a
/// visual system's own receptor curves, see `new_curve_menu` in
/// `window_workspace`), so that option is deliberately not offered here.
fn quantity_kind_editor(ui: &mut egui::Ui, app: &mut AppState, id: CurveId) {
    let Some(entry) = app.stimulus_curves.get_mut(&id) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Quantity kind:");
        let current = quantity_label(&entry.curve.quantity);
        egui::ComboBox::from_id_salt(("stimulus_quantity_kind", id))
            .selected_text(current)
            .show_ui(ui, |ui| {
                for option in [
                    "Unspecified",
                    "Reflectance",
                    "Transmittance",
                    "Radiance",
                    "Absorption",
                ] {
                    if ui.selectable_label(current == option, option).clicked() {
                        entry.curve.quantity = match option {
                            "Reflectance" => QuantityKind::Reflectance,
                            "Transmittance" => QuantityKind::Transmittance,
                            "Absorption" => QuantityKind::Absorption,
                            "Radiance" => QuantityKind::Radiance {
                                unit: "W.m-2.nm-1".to_string(),
                            },
                            _ => QuantityKind::Unspecified,
                        };
                        entry.dirty = true;
                    }
                }
            });
        if let QuantityKind::Radiance { unit } = &mut entry.curve.quantity {
            ui.label("Unit:");
            if ui
                .add(egui::TextEdit::singleline(unit).desired_width(100.0))
                .changed()
            {
                entry.dirty = true;
            }
        }
    });
}

/// Per the app's standardized taxonomy, a Reflectance curve is "unitless,
/// normalized so every sample is between 0 and 1." This is a soft,
/// non-blocking flag, not a clamp - a value measured or derived outside
/// that range is unusual but not meaningless (e.g. a calibration
/// artifact, or a retroreflective/fluorescent sample), so values outside
/// [0, 1] stay visible on the graph and in the data rather than being
/// clipped or silently rewritten.
fn reflectance_range_warning(ui: &mut egui::Ui, app: &AppState, id: CurveId) {
    let Some(entry) = app.stimulus_curves.get(&id) else {
        return;
    };
    let kind = match entry.curve.quantity {
        QuantityKind::Reflectance => "reflectance",
        QuantityKind::Absorption => "an absorption curve",
        _ => return,
    };
    let out_of_range = entry
        .curve
        .points
        .iter()
        .any(|&(_, v)| !(0.0..=1.0).contains(&v));
    if out_of_range {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            format!("⚠ Some values are outside [0, 1], the expected range for {kind}."),
        );
    }
}

/// Derives a reflectance curve from a radiance one (dividing out the
/// luminant it was measured under), or runs the forward direction
/// instead, predicting a reflectance curve's appearance under a chosen
/// luminant. One form covers both, since each just needs a source
/// curve, a luminant, and a direction.
fn derive_panel(ui: &mut egui::Ui, app: &mut AppState, reflectance_direction: bool) {
    ui.heading(if reflectance_direction {
        "Derive reflectance from radiance ÷ luminant"
    } else {
        "Derive radiance from reflectance × luminant"
    });
    if reflectance_direction {
        ui.label(
            "Needs a Radiance-tagged curve (the measurement) and the luminant it was measured \
             under, with matching unit text - set that via the source curve's own editor first \
             if it doesn't match yet.",
        );
    } else {
        ui.label(
            "Takes a Reflectance curve and multiplies it by a chosen luminant to predict how it \
             would appear measured under that light.",
        );
    }

    ui.label(if reflectance_direction {
        "Measured radiance curve:"
    } else {
        "Reflectance curve:"
    });
    let order = app.stimulus_order.clone();
    let mut source = app.stimulus_editor.derive_source;
    egui::ComboBox::from_id_salt("derive_source")
        .selected_text(
            source
                .and_then(|id| app.stimulus_curves.get(&id))
                .map(|e| e.curve.name.clone())
                .unwrap_or_else(|| "(choose one)".to_string()),
        )
        .show_ui(ui, |ui| {
            for id in &order {
                if let Some(entry) = app.stimulus_curves.get(id) {
                    if ui
                        .selectable_label(source == Some(*id), &entry.curve.name)
                        .clicked()
                    {
                        source = Some(*id);
                    }
                }
            }
        });
    app.stimulus_editor.derive_source = source;

    ui.label(if reflectance_direction {
        "Luminant it was measured under:"
    } else {
        "Luminant to predict under:"
    });
    let mut illum = app.stimulus_editor.derive_luminant;
    egui::ComboBox::from_id_salt("derive_luminant")
        .selected_text(
            illum
                .and_then(|id| app.luminants.get(&id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "(choose one)".to_string()),
        )
        .show_ui(ui, |ui| {
            for &id in &app.luminant_order {
                if let Some(curve) = app.luminants.get(&id) {
                    if ui
                        .selectable_label(illum == Some(id), &curve.name)
                        .clicked()
                    {
                        illum = Some(id);
                    }
                }
            }
        });
    app.stimulus_editor.derive_luminant = illum;

    ui.horizontal(|ui| {
        let can_create = source.is_some() && illum.is_some();
        if ui
            .add_enabled(can_create, egui::Button::new("Create"))
            .clicked()
        {
            if let (Some(sid), Some(iid)) = (source, illum) {
                let source_curve = app.stimulus_curves.get(&sid).map(|e| e.curve.clone());
                let illum_curve = app.luminants.get(&iid).cloned();
                if let (Some(sc), Some(ic)) = (source_curve, illum_curve) {
                    if reflectance_direction {
                        match illumination::derive_reflectance(&sc, &ic, STEP_NM) {
                            Ok(curve) => {
                                add_and_select(app, curve);
                                app.stimulus_editor.derive_status.clear();
                            }
                            Err(e) => app.stimulus_editor.derive_status = e.to_string(),
                        }
                    } else {
                        let curve = illumination::predict_under_illuminant(&sc, &ic, STEP_NM);
                        add_and_select(app, curve);
                        app.stimulus_editor.derive_status.clear();
                    }
                }
            }
        }
        if ui.button("Cancel").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::None;
        }
    });
    if !app.stimulus_editor.derive_status.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 90, 90),
            &app.stimulus_editor.derive_status,
        );
    }
}

/// Lenient wavelength,value parser for pasted text/CSV (e.g. a USGS
/// spectral library ASCII export): comma- or whitespace-separated,
/// skips blank/comment/header lines and any row that doesn't parse as
/// two numbers. USGS splib wavelengths are conventionally in
/// micrometers rather than nm - since plain two-column text doesn't
/// declare its own units, this applies a heuristic: if every parsed
/// wavelength is under 50 (nothing in this app's actual nm range is
/// ever that small), the whole file is assumed to be in micrometers and
/// converted. Also drops USGS's "no data" sentinel values (magnitudes
/// far outside anything an actual measurement would produce).
fn parse_csv_points(text: &str) -> Vec<(f64, f64)> {
    let mut raw: Vec<(f64, f64)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let tokens: Vec<&str> = line
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        if tokens.len() < 2 {
            continue;
        }
        if let (Ok(x), Ok(y)) = (tokens[0].parse::<f64>(), tokens[1].parse::<f64>()) {
            if x.is_finite() && y.is_finite() && y.abs() < 1.0e30 {
                raw.push((x, y));
            }
        }
    }
    if raw.is_empty() {
        return raw;
    }
    let max_wl = raw.iter().map(|&(x, _)| x).fold(f64::MIN, f64::max);
    if max_wl > 0.0 && max_wl < 50.0 {
        raw.into_iter().map(|(x, y)| (x * 1000.0, y)).collect()
    } else {
        raw
    }
}

fn csv_import_panel(ui: &mut egui::Ui, app: &mut AppState) {
    ui.heading("Import from text / CSV");
    ui.label(
        "Paste wavelength,value rows below (comma- or whitespace-separated; header or \
         non-numeric lines are skipped). Wavelengths that look like micrometers (USGS spectral \
         library convention) are converted to nm automatically.",
    );
    ui.horizontal(|ui| {
        ui.label("Name:");
        ui.text_edit_singleline(&mut app.stimulus_editor.csv_name);
    });
    ui.add(
        egui::TextEdit::multiline(&mut app.stimulus_editor.csv_text)
            .desired_rows(12)
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        if ui.button("Import").clicked() {
            let points = parse_csv_points(&app.stimulus_editor.csv_text);
            if points.is_empty() {
                app.stimulus_editor.csv_status =
                    "No numeric wavelength,value rows found".to_string();
            } else {
                let name = if app.stimulus_editor.csv_name.trim().is_empty() {
                    "Imported curve".to_string()
                } else {
                    app.stimulus_editor.csv_name.trim().to_string()
                };
                let curve = SpectralCurve::new(name, CurveType::Reflectance)
                    .with_points(points)
                    .with_quantity(QuantityKind::Reflectance);
                add_and_select(app, curve);
                app.stimulus_editor.csv_text.clear();
                app.stimulus_editor.csv_name.clear();
                app.stimulus_editor.csv_status.clear();
            }
        }
        if ui.button("Cancel").clicked() {
            app.stimulus_editor.creation_mode = CreationMode::None;
        }
    });
    if !app.stimulus_editor.csv_status.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 90, 90),
            &app.stimulus_editor.csv_status,
        );
    }
}

fn band_to_color_image(cube: &HyperspectralCube, band: usize) -> egui::ColorImage {
    let slice = cube.band_slice(band);
    let (min, max) = slice
        .iter()
        .fold((f32::MAX, f32::MIN), |(mn, mx), &v| (mn.min(v), mx.max(v)));
    let range = (max - min).max(1e-6);
    let pixels: Vec<egui::Color32> = slice
        .iter()
        .map(|&v| {
            let t = (((v - min) / range) * 255.0).clamp(0.0, 255.0) as u8;
            egui::Color32::from_gray(t)
        })
        .collect();
    egui::ColorImage {
        size: [cube.samples, cube.lines],
        pixels,
    }
}

fn reset_after_new_cube(state: &mut StimulusEditorState) {
    state.cube_generation += 1;
    state.band = 0;
    state.region = None;
    state.polygon_points.clear();
    state.drag_start = None;
}

fn image_import_panel(ui: &mut egui::Ui, app: &mut AppState) {
    ui.heading("Import from image");
    if ui.button("Done").clicked() {
        app.stimulus_editor.creation_mode = CreationMode::None;
    }
    ui.separator();

    let state = &mut app.stimulus_editor;

    ui.label("Load an ENVI (point at the .hdr) or GeoTIFF file:");
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut state.image_path);
        if ui.button("Load").clicked() {
            match hyperspectral::load_envi_or_geotiff(&state.image_path) {
                Ok(cube) => {
                    let has_wavelengths = cube.wavelengths_nm.is_some();
                    state.image_status = format!(
                        "Loaded {}x{} pixels, {} bands{}",
                        cube.samples,
                        cube.lines,
                        cube.bands,
                        if has_wavelengths {
                            " (wavelength metadata found)"
                        } else {
                            " (no wavelength metadata - assign one below before extracting)"
                        }
                    );
                    state.cube = Some(cube);
                    reset_after_new_cube(state);
                }
                Err(e) => state.image_status = format!("Load failed: {e}"),
            }
        }
    });

    ui.separator();
    ui.label("Or load a MATLAB .mat hypercube:");
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut state.mat_path);
        if ui.button("List variables").clicked() {
            match hyperspectral::list_mat_variables(&state.mat_path) {
                Ok(vars) => {
                    let guess = hyperspectral::guess_mat_mapping(&vars);
                    state.mat_cube_choice = guess
                        .cube_name
                        .as_ref()
                        .and_then(|name| vars.iter().position(|v| &v.name == name));
                    state.mat_wavelength_choice = guess
                        .wavelength_name
                        .as_ref()
                        .and_then(|name| vars.iter().position(|v| &v.name == name));
                    state.image_status = format!("Found {} variable(s)", vars.len());
                    state.mat_variables = vars;
                }
                Err(e) => state.image_status = format!("Failed to list variables: {e}"),
            }
        }
    });
    if !state.mat_variables.is_empty() {
        egui::Grid::new("mat_variables_table")
            .striped(true)
            .show(ui, |ui| {
                ui.label("Variable");
                ui.label("Shape");
                ui.label("Use as cube?");
                ui.label("Use as wavelengths?");
                ui.end_row();
                for i in 0..state.mat_variables.len() {
                    ui.label(&state.mat_variables[i].name);
                    ui.label(format!("{:?}", state.mat_variables[i].shape));
                    if ui.radio(state.mat_cube_choice == Some(i), "").clicked() {
                        state.mat_cube_choice = Some(i);
                    }
                    if ui
                        .radio(state.mat_wavelength_choice == Some(i), "")
                        .clicked()
                    {
                        state.mat_wavelength_choice = Some(i);
                    }
                    ui.end_row();
                }
            });
        if ui.button("None (no wavelength variable)").clicked() {
            state.mat_wavelength_choice = None;
        }
        let can_load = state.mat_cube_choice.is_some();
        if ui
            .add_enabled(can_load, egui::Button::new("Load selected cube"))
            .clicked()
        {
            let cube_name = state.mat_variables[state.mat_cube_choice.unwrap()]
                .name
                .clone();
            let wavelength_name = state
                .mat_wavelength_choice
                .map(|i| state.mat_variables[i].name.clone());
            match hyperspectral::load_mat_cube(
                &state.mat_path,
                &cube_name,
                wavelength_name.as_deref(),
            ) {
                Ok(cube) => {
                    state.image_status = format!(
                        "Loaded {}x{} pixels, {} bands from \"{cube_name}\"",
                        cube.samples, cube.lines, cube.bands
                    );
                    state.cube = Some(cube);
                    reset_after_new_cube(state);
                }
                Err(e) => state.image_status = format!("Load failed: {e}"),
            }
        }
    }

    if !state.image_status.is_empty() {
        ui.label(&state.image_status);
    }

    let Some(cube) = state.cube.as_ref() else {
        return;
    };
    let bands = cube.bands;
    let has_wavelength_axis = cube.wavelengths_nm.is_some();

    ui.separator();

    if !has_wavelength_axis {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            format!("⚠ No wavelength metadata for these {bands} bands - assign one before extracting a curve:"),
        );
        ui.horizontal(|ui| {
            egui::ComboBox::new("sensor_preset_picker", "Sensor preset")
                .selected_text(
                    state
                        .sensor_presets
                        .get(state.sensor_preset_picker)
                        .map(|p| p.name.as_str())
                        .unwrap_or("(none)"),
                )
                .show_ui(ui, |ui| {
                    for (i, preset) in state.sensor_presets.iter().enumerate() {
                        ui.selectable_value(&mut state.sensor_preset_picker, i, &preset.name);
                    }
                });
            if let Some(preset) = state.sensor_presets.get(state.sensor_preset_picker) {
                let matches = preset.wavelengths_nm.len() == bands;
                if ui
                    .add_enabled(matches, egui::Button::new("Apply preset"))
                    .clicked()
                {
                    let wavelengths = preset.wavelengths_nm.clone();
                    state
                        .cube
                        .as_mut()
                        .unwrap()
                        .assign_wavelengths(wavelengths)
                        .ok();
                }
                if !matches {
                    ui.label(format!(
                        "({} bands in preset, need {bands})",
                        preset.wavelengths_nm.len(),
                    ));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Manual (comma-separated nm, one per band):");
            ui.text_edit_singleline(&mut state.manual_wavelengths);
            if ui.button("Apply manual").clicked() {
                let parsed: Result<Vec<f64>, _> = state
                    .manual_wavelengths
                    .split(',')
                    .map(|s| s.trim().parse::<f64>())
                    .collect();
                match parsed {
                    Ok(wavelengths) => {
                        match state.cube.as_mut().unwrap().assign_wavelengths(wavelengths) {
                            Ok(()) => state.image_status = "Wavelengths assigned".to_string(),
                            Err(e) => state.image_status = format!("{e}"),
                        }
                    }
                    Err(_) => {
                        state.image_status = "Couldn't parse manual wavelength list".to_string()
                    }
                }
            }
        });
        ui.separator();
    }

    let cube = state.cube.as_ref().unwrap();

    ui.horizontal(|ui| {
        ui.label("Preview band:");
        let mut band = state.band;
        if ui
            .add(egui::Slider::new(
                &mut band,
                0..=cube.bands.saturating_sub(1),
            ))
            .changed()
        {
            state.band = band;
        }
        if let Some(wavelengths) = &cube.wavelengths_nm {
            ui.label(format!("({:.0} nm)", wavelengths[state.band]));
        }
    });

    ui.horizontal(|ui| {
        ui.label("Selection shape:");
        ui.radio_value(
            &mut state.selection_mode,
            SelectionMode::Rectangle,
            "Rectangle",
        );
        ui.radio_value(
            &mut state.selection_mode,
            SelectionMode::Polygon,
            "Polygon (click to add vertices)",
        );
    });

    let needs_rebuild = state.texture_key != Some((state.cube_generation, state.band));
    if needs_rebuild {
        let image = band_to_color_image(cube, state.band);
        let handle = ui.ctx().load_texture(
            "hyperspectral_preview",
            image,
            egui::TextureOptions::NEAREST,
        );
        state.texture = Some(handle);
        state.texture_key = Some((state.cube_generation, state.band));
    }

    let max_display_width = ui.available_width().min(640.0);
    let aspect = cube.lines as f32 / cube.samples as f32;
    let display_size = egui::vec2(max_display_width, max_display_width * aspect);

    let (response, painter) = ui.allocate_painter(display_size, egui::Sense::click_and_drag());
    if let Some(texture) = &state.texture {
        painter.image(
            texture.id(),
            response.rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    let to_pixel = |p: egui::Pos2| -> (f64, f64) {
        let rel_x = ((p.x - response.rect.min.x) / response.rect.width()) as f64;
        let rel_y = ((p.y - response.rect.min.y) / response.rect.height()) as f64;
        (rel_x * cube.samples as f64, rel_y * cube.lines as f64)
    };
    let to_screen = |x: f64, y: f64| -> egui::Pos2 {
        egui::pos2(
            response.rect.min.x + (x / cube.samples as f64) as f32 * response.rect.width(),
            response.rect.min.y + (y / cube.lines as f64) as f32 * response.rect.height(),
        )
    };

    match state.selection_mode {
        SelectionMode::Rectangle => {
            if response.drag_started() {
                state.drag_start = response.interact_pointer_pos();
            }
            if let (true, Some(start)) = (response.dragged(), state.drag_start) {
                if let Some(current) = response.interact_pointer_pos() {
                    painter.rect_stroke(
                        egui::Rect::from_two_pos(start, current),
                        0.0,
                        egui::Stroke::new(2.0_f32, egui::Color32::YELLOW),
                    );
                }
            }
            if response.drag_stopped() {
                if let (Some(start), Some(end)) =
                    (state.drag_start, response.interact_pointer_pos())
                {
                    let (x0, y0) = to_pixel(start);
                    let (x1, y1) = to_pixel(end);
                    state.region = Some(Region::Rectangle { x0, y0, x1, y1 });
                }
                state.drag_start = None;
            }
            if let Some(Region::Rectangle { x0, y0, x1, y1 }) = &state.region {
                painter.rect_stroke(
                    egui::Rect::from_two_pos(to_screen(*x0, *y0), to_screen(*x1, *y1)),
                    0.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::GREEN),
                );
            }
        }
        SelectionMode::Polygon => {
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    state.polygon_points.push(to_pixel(pos));
                }
            }
            if state.polygon_points.len() >= 2 {
                let screen_points: Vec<egui::Pos2> = state
                    .polygon_points
                    .iter()
                    .map(|&(x, y)| to_screen(x, y))
                    .collect();
                painter.add(egui::Shape::closed_line(
                    screen_points,
                    egui::Stroke::new(2.0_f32, egui::Color32::YELLOW),
                ));
            }
            if let Some(Region::Polygon(points)) = &state.region {
                let screen_points: Vec<egui::Pos2> =
                    points.iter().map(|&(x, y)| to_screen(x, y)).collect();
                painter.add(egui::Shape::closed_line(
                    screen_points,
                    egui::Stroke::new(2.0_f32, egui::Color32::GREEN),
                ));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        state.polygon_points.len() >= 3,
                        egui::Button::new("Close polygon"),
                    )
                    .clicked()
                {
                    state.region = Some(Region::Polygon(std::mem::take(&mut state.polygon_points)));
                }
                if ui.button("Clear points").clicked() {
                    state.polygon_points.clear();
                }
            });
        }
    }

    ui.separator();
    ui.checkbox(
        &mut state.as_illumination,
        "Extract as an Illumination/radiance curve instead of a reflectance sample",
    );
    let has_wavelengths = cube.wavelengths_nm.is_some();
    let can_extract = state.region.is_some() && has_wavelengths;
    if !has_wavelengths {
        ui.label("Assign a wavelength axis above before extracting.");
    }
    if ui
        .add_enabled(can_extract, egui::Button::new("Extract region to curve"))
        .clicked()
    {
        let region = state.region.clone().unwrap();
        let region_label = match &region {
            Region::Rectangle { x0, y0, x1, y1 } => {
                format!("rect ({x0:.0},{y0:.0})-({x1:.0},{y1:.0})")
            }
            Region::Polygon(points) => format!("polygon ({} vertices)", points.len()),
        };
        let as_illumination = state.as_illumination;
        match hyperspectral::extracted_curve(cube, &region, &region_label, as_illumination) {
            Some(curve) => {
                state.image_status =
                    format!("Extracted \"{}\" into the stimulus library", curve.name);
                add_and_select(app, curve);
                // Extraction can continue from the same loaded image, so
                // stay in Image mode rather than following add_and_select
                // back to the normal editor.
                app.stimulus_editor.creation_mode = CreationMode::Image;
            }
            None => {
                app.stimulus_editor.image_status = "Extraction failed - empty region".to_string()
            }
        }
    }

    bulk_pixel_import_section(ui, app);
}

/// The "extract every pixel as curves" control (§4.2.6's addendum),
/// factored into its own re-borrow of `app.stimulus_editor`/its cube so
/// it doesn't extend the outer `image_import_panel`'s own `state`/`cube`
/// borrows across the `add_many(app, ...)` call this needs at the end.
fn bulk_pixel_import_section(ui: &mut egui::Ui, app: &mut AppState) {
    ui.separator();
    ui.label(
        "Bulk natural-scene corpus import (§4.2.6): one radiance curve per pixel, tagged \
         so the opponent-contrast \"Custom corpus\" picker can select the whole batch at once.",
    );
    ui.horizontal(|ui| {
        ui.label("Keep every Nth pixel:");
        ui.add(
            egui::DragValue::new(&mut app.stimulus_editor.pixel_import_stride)
                .range(1..=1_000_000)
                .speed(1),
        );
    });
    if !app.stimulus_editor.pixel_import_status.is_empty() {
        ui.label(app.stimulus_editor.pixel_import_status.clone());
    }
    let stride = app.stimulus_editor.pixel_import_stride.max(1);
    let region = app.stimulus_editor.region.clone();
    let Some(cube) = app.stimulus_editor.cube.as_ref() else {
        return;
    };
    let has_wavelengths = cube.wavelengths_nm.is_some();
    let estimated_curves = estimate_region_pixel_count(cube, region.as_ref()).div_ceil(stride);
    ui.label(format!("~{estimated_curves} curve(s) will be created."));
    const WARN_THRESHOLD: usize = 5000;
    if estimated_curves > WARN_THRESHOLD {
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 40),
            format!(
                "⚠ That's a lot of curves ({estimated_curves}) - consider a larger stride. \
                 Not blocked, just slow and memory-heavy."
            ),
        );
    }
    if !has_wavelengths {
        ui.label("Assign a wavelength axis above before extracting.");
    }
    let clicked = ui
        .add_enabled(
            has_wavelengths,
            egui::Button::new("Extract every pixel as curves"),
        )
        .clicked();
    if !clicked {
        return;
    }

    let region_label = match &region {
        Some(Region::Rectangle { x0, y0, x1, y1 }) => {
            format!("rect ({x0:.0},{y0:.0})-({x1:.0},{y1:.0})")
        }
        Some(Region::Polygon(points)) => format!("polygon ({} vertices)", points.len()),
        None => "whole image".to_string(),
    };
    let (curves, batch_label) = {
        let cube = app.stimulus_editor.cube.as_ref().unwrap();
        // "/" nests these under a corpus named for the source image, with
        // the region as a sub-corpus (stimulus_picker's hierarchy, §4.2.6).
        let batch_label = format!("{}/{region_label}", cube.source_label);
        let spectra = hyperspectral::extract_region_pixel_spectra(cube, region.as_ref(), stride);
        let curves: Vec<SpectralCurve> = spectra
            .iter()
            .filter_map(|(line, sample, spectrum)| {
                hyperspectral::pixel_curve(cube, *line, *sample, spectrum, &batch_label)
            })
            .collect();
        (curves, batch_label)
    };
    let count = curves.len();
    add_many(app, curves);
    app.stimulus_editor.pixel_import_status =
        format!("Imported {count} pixel curve(s) into batch \"{batch_label}\"");
}

/// Cheap, approximate pixel count for the live "~N curves" estimate
/// above - exact for `None` (whole image) and a `Rectangle`; a bounding-
/// box upper bound for a `Polygon` (an exact point-in-polygon scan is
/// the extraction's own job, not something to redo every frame just to
/// label a button).
fn estimate_region_pixel_count(cube: &HyperspectralCube, region: Option<&Region>) -> usize {
    let (w, h) = match region {
        None => (cube.samples as f64, cube.lines as f64),
        Some(Region::Rectangle { x0, y0, x1, y1 }) => ((x1 - x0).abs(), (y1 - y0).abs()),
        Some(Region::Polygon(points)) => {
            let (mut xmin, mut ymin, mut xmax, mut ymax) =
                (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for &(x, y) in points {
                xmin = xmin.min(x);
                ymin = ymin.min(y);
                xmax = xmax.max(x);
                ymax = ymax.max(y);
            }
            if points.is_empty() {
                (0.0, 0.0)
            } else {
                (xmax - xmin, ymax - ymin)
            }
        }
    };
    (w.max(0.0) * h.max(0.0)).round() as usize
}

/// Bulk radiance → reflectance panel (§4.2.6's addendum): the same
/// physical operation as `derive_panel(..., true)`, applied to many
/// selected curves against one shared luminant instead of one at a time.
fn bulk_derive_reflectance_panel(ui: &mut egui::Ui, app: &mut AppState) {
    ui.heading("Bulk-convert radiance to reflectance");
    ui.label(
        "Select Radiance-tagged curves (e.g. a batch imported from an image's pixels) and a \
         shared luminant they were measured under; each gets its own new Reflectance curve, \
         added alongside the original.",
    );
    if ui.button("Done").clicked() {
        app.stimulus_editor.creation_mode = CreationMode::None;
    }
    ui.separator();

    ui.label("Luminant it was measured under:");
    let mut illum = app.stimulus_editor.derive_luminant;
    egui::ComboBox::from_id_salt("bulk_derive_luminant")
        .selected_text(
            illum
                .and_then(|id| app.luminants.get(&id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "(choose one)".to_string()),
        )
        .show_ui(ui, |ui| {
            for &id in &app.luminant_order {
                if let Some(curve) = app.luminants.get(&id) {
                    if ui
                        .selectable_label(illum == Some(id), &curve.name)
                        .clicked()
                    {
                        illum = Some(id);
                    }
                }
            }
        });
    app.stimulus_editor.derive_luminant = illum;

    ui.separator();
    ui.label("Curves to convert:");
    let is_radiance = |c: &SpectralCurve| matches!(c.quantity, QuantityKind::Radiance { .. });
    let tree = stimulus_picker::corpus_tree(app, &is_radiance);
    let selection = app.stimulus_editor.bulk_derive_selection.clone();
    let is_included = |id: CurveId| selection.contains(&id);
    let mut toggled: Vec<(CurveId, bool)> = Vec::new();
    let mut on_toggle = |ids: &[CurveId], included: bool| {
        toggled.extend(ids.iter().map(|&id| (id, included)));
    };
    stimulus_picker::render_corpus_checklist(ui, &tree, &is_included, &mut on_toggle);
    for (id, included) in toggled {
        toggle_bulk_derive_selection(app, id, included);
    }

    let selected_count = app.stimulus_editor.bulk_derive_selection.len();
    let can_convert = selected_count > 0 && illum.is_some();
    if ui
        .add_enabled(
            can_convert,
            egui::Button::new(format!("Convert {selected_count} curve(s)")),
        )
        .clicked()
    {
        let Some(iid) = illum else { return };
        let Some(illum_curve) = app.luminants.get(&iid).cloned() else {
            return;
        };
        let selected: Vec<CurveId> = app.stimulus_editor.bulk_derive_selection.iter().copied().collect();
        let mut converted = Vec::new();
        let mut skipped = 0;
        for id in selected {
            let Some(entry) = app.stimulus_curves.get(&id) else {
                continue;
            };
            let source_batch = entry.curve.metadata.get("batch").cloned();
            match illumination::derive_reflectance(&entry.curve, &illum_curve, STEP_NM) {
                Ok(mut curve) => {
                    if let Some(batch) = source_batch {
                        // "/" nests the converted curve as a sub-corpus of
                        // its source batch (stimulus_picker's hierarchy).
                        curve
                            .metadata
                            .insert("batch".to_string(), format!("{batch}/reflectance"));
                    }
                    converted.push(curve);
                }
                Err(_) => skipped += 1,
            }
        }
        let converted_count = converted.len();
        add_many(app, converted);
        app.stimulus_editor.derive_status = if skipped > 0 {
            format!(
                "Converted {converted_count} of {} curves; {skipped} skipped - unit mismatch",
                converted_count + skipped
            )
        } else {
            format!("Converted {converted_count} curve(s)")
        };
    }
    if !app.stimulus_editor.derive_status.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(90, 160, 90),
            &app.stimulus_editor.derive_status,
        );
    }
}

fn toggle_bulk_derive_selection(app: &mut AppState, id: CurveId, included: bool) {
    if included {
        app.stimulus_editor.bulk_derive_selection.insert(id);
    } else {
        app.stimulus_editor.bulk_derive_selection.remove(&id);
    }
}
