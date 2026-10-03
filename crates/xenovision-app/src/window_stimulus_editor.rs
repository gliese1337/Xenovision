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

    // --- text/CSV import ---
    pub csv_name: String,
    pub csv_text: String,
    pub csv_status: String,

    // --- derive reflectance/radiance ---
    pub derive_source: Option<CurveId>,
    pub derive_luminant: Option<CurveId>,
    pub derive_status: String,

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

            csv_name: String::new(),
            csv_text: String::new(),
            csv_status: String::new(),

            derive_source: None,
            derive_luminant: None,
            derive_status: String::new(),

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

    let order = app.stimulus_order.clone();
    for id in order {
        let Some(entry) = app.stimulus_curves.get(&id) else {
            continue;
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
/// artifact, or a genuinely retroreflective/fluorescent sample), and an
/// earlier fix deliberately stopped the graph from clipping such values
/// out of view, so this doesn't fight that by silently rewriting the data.
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
/// far outside anything a real measurement would produce).
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

    if hyperspectral::GDAL_AVAILABLE {
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
    } else {
        ui.label(
            "ENVI and GeoTIFF import isn't included in this build (it needs the GDAL \
             library). MATLAB .mat files below, and Import from text/CSV, still work.",
        );
    }

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
                        egui::Stroke::new(2.0, egui::Color32::YELLOW),
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
                    egui::Stroke::new(2.0, egui::Color32::GREEN),
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
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                ));
            }
            if let Some(Region::Polygon(points)) = &state.region {
                let screen_points: Vec<egui::Pos2> =
                    points.iter().map(|&(x, y)| to_screen(x, y)).collect();
                painter.add(egui::Shape::closed_line(
                    screen_points,
                    egui::Stroke::new(2.0, egui::Color32::GREEN),
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
                app.stimulus_editor.image_status =
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
}
