//! CSV export of the coordinate table (and, optionally, each stimulus's
//! distance to one chosen reference stimulus) over every currently-
//! selected input spectrum, across every selected species - the "large
//! group" counterpart to `window_comparison`'s live, interactive tables,
//! which don't scale to thousands of rows in an egui table.

use std::fs::File;
use std::io::{BufWriter, Write};

use eframe::egui;
use xenovision_core::comparison;
use xenovision_core::pipeline::Coordinates;
use xenovision_core::{illumination, QuantityKind, SpectralCurve};

use crate::state::AppState;

/// Escapes one CSV field per RFC 4180: wraps in quotes (doubling any
/// inner quotes) whenever the value contains a comma, quote, or
/// newline; plain values pass through unquoted.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Same reflectance x luminant rule as `window_comparison::resolve_stimulus`.
fn resolve(curve: &SpectralCurve, luminant: &SpectralCurve) -> SpectralCurve {
    if curve.quantity == QuantityKind::Reflectance {
        illumination::predict_under_illuminant(curve, luminant, 1.0)
    } else {
        curve.clone()
    }
}

/// Runs the export: one row per currently-`selected_stimuli` curve, with
/// each selected species' luminance/chroma/saturation (and, optionally,
/// its distance to `export.reference_stimulus` under `comparison.metric`)
/// as that species' own block of columns. Returns the row count written,
/// or an error message (no luminant/stimuli/species selected, an
/// unusable reference, or a file-system error) without writing anything
/// partial - the error is returned before the file is created.
pub fn run_export(app: &mut AppState, path: &str) -> Result<usize, String> {
    let Some(luminant_id) = app.comparison.reference_luminant else {
        return Err("Pick a reference luminant above first.".to_string());
    };
    let Some(luminant) = app.luminants.get(&luminant_id).cloned() else {
        return Err("That luminant is no longer available.".to_string());
    };
    if app.comparison.selected_stimuli.is_empty() {
        return Err("Select at least one input spectrum first.".to_string());
    }
    let species_ids = app.comparison.selected_species.clone();
    if species_ids.is_empty() {
        return Err("Select at least one species first.".to_string());
    }

    let stimulus_ids = app.comparison.selected_stimuli.clone();
    let (stimuli, stimulus_names): (Vec<SpectralCurve>, Vec<String>) = stimulus_ids
        .iter()
        .filter_map(|id| app.stimulus_curves.get(id))
        .filter(|e| e.curve.quantity != QuantityKind::Absorption)
        .map(|e| (resolve(&e.curve, &luminant), e.curve.name.clone()))
        .unzip();
    if stimuli.is_empty() {
        return Err("None of the selected input spectra are usable (all Absorption?).".to_string());
    }

    let include_distance = app.comparison.export.include_distance_to_reference;
    let metric = app.comparison.metric;
    let reference_curve = app
        .comparison
        .export
        .reference_stimulus
        .and_then(|id| app.stimulus_curves.get(&id))
        .map(|e| resolve(&e.curve, &luminant));
    if include_distance && reference_curve.is_none() {
        return Err("Pick a reference stimulus for the distance column first.".to_string());
    }

    // One Pipeline (and the reference's own coordinates in it) built
    // once per species here, not once per stimulus row below.
    struct SpeciesColumn {
        name: String,
        pipeline: std::rc::Rc<xenovision_core::pipeline::Pipeline>,
        reference_coords: Option<Coordinates>,
    }
    let mut columns = Vec::with_capacity(species_ids.len());
    for &set_id in &species_ids {
        let Some(set) = app.curve_sets.get(&set_id) else {
            continue;
        };
        let pipeline = app
            .transform_cache
            .get_or_build(set, luminant_id, &luminant, 1.0);
        let reference_coords = reference_curve.as_ref().map(|r| pipeline.coordinates(r));
        columns.push(SpeciesColumn {
            name: set.name.clone(),
            pipeline,
            reference_coords,
        });
    }
    if columns.is_empty() {
        return Err("None of the selected species are open anymore.".to_string());
    }

    let file = File::create(path).map_err(|e| format!("Couldn't create {path}: {e}"))?;
    let mut w = BufWriter::new(file);

    // Header: Stimulus, then per species: L, C1..C{n-1}, Saturation[, Distance-to-ref].
    let mut header = vec!["Stimulus".to_string()];
    for col in &columns {
        header.push(format!("{} L", col.name));
        for i in 1..=col.pipeline.chroma_axis_count() {
            header.push(format!("{} C{i}", col.name));
        }
        header.push(format!("{} Saturation", col.name));
        if include_distance {
            header.push(format!("{} Distance-to-ref", col.name));
        }
    }
    writeln!(
        w,
        "{}",
        header.iter().map(|h| csv_field(h)).collect::<Vec<_>>().join(",")
    )
    .map_err(|e| e.to_string())?;

    let mut rows_written = 0usize;
    for (stimulus, name) in stimuli.iter().zip(&stimulus_names) {
        let mut row = vec![csv_field(name)];
        for col in &columns {
            let coords = col.pipeline.coordinates(stimulus);
            row.push(coords.luminance.to_string());
            for c in &coords.chroma {
                row.push(c.to_string());
            }
            row.push(coords.saturation.to_string());
            if include_distance {
                let d = col.reference_coords.as_ref().and_then(|rc| {
                    comparison::distance(&coords, rc, metric, col.pipeline.noise())
                });
                row.push(d.map(|v| v.to_string()).unwrap_or_default());
            }
        }
        writeln!(w, "{}", row.join(",")).map_err(|e| e.to_string())?;
        rows_written += 1;
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(rows_written)
}

/// The Comparison window's "Batch export to CSV" control.
pub fn export_panel(ui: &mut egui::Ui, app: &mut AppState) {
    ui.collapsing("Batch export to CSV", |ui| {
        ui.label(format!(
            "Exports every currently-selected input spectrum ({} selected) against every \
             selected species to a CSV file - for running this over a corpus too large to \
             render as an interactive table.",
            app.comparison.selected_stimuli.len()
        ));

        ui.checkbox(
            &mut app.comparison.export.include_distance_to_reference,
            "Include each row's distance to a reference stimulus (uses the Metric above)",
        );
        if app.comparison.export.include_distance_to_reference {
            ui.horizontal(|ui| {
                ui.label("Reference:");
                let mut reference = app.comparison.export.reference_stimulus;
                let name = reference
                    .and_then(|id| app.stimulus_curves.get(&id))
                    .map(|e| e.curve.name.clone())
                    .unwrap_or_else(|| "(choose one)".to_string());
                egui::ComboBox::from_id_salt("batch_export_reference")
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for &id in &app.stimulus_order {
                            if let Some(entry) = app.stimulus_curves.get(&id) {
                                if ui
                                    .selectable_label(reference == Some(id), &entry.curve.name)
                                    .clicked()
                                {
                                    reference = Some(id);
                                }
                            }
                        }
                    });
                app.comparison.export.reference_stimulus = reference;
            });
        }

        ui.horizontal(|ui| {
            ui.label("File path:");
            ui.text_edit_singleline(&mut app.comparison.export.path_buf);
            if ui.button("Export").clicked() {
                let path = app.comparison.export.path_buf.clone();
                app.comparison.export.status = match run_export(app, &path) {
                    Ok(n) => format!("Wrote {n} row(s) to {path}"),
                    Err(e) => e,
                };
            }
        });
        if !app.comparison.export.status.is_empty() {
            ui.label(&app.comparison.export.status);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CurveId;

    #[test]
    fn csv_field_quotes_only_when_needed() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_field("a\nb"), "\"a\nb\"");
    }

    fn setup_app() -> (AppState, CurveId, CurveId) {
        let mut app = AppState::new();
        app.stimulus_curves.clear();
        app.stimulus_order.clear();
        let reddish = SpectralCurve::new("Reddish", xenovision_core::CurveType::Reflectance)
            .with_points(vec![(400.0, 0.2), (500.0, 0.2), (600.0, 0.8), (700.0, 0.8)])
            .with_quantity(QuantityKind::Reflectance);
        let greenish = SpectralCurve::new("Greenish", xenovision_core::CurveType::Reflectance)
            .with_points(vec![(400.0, 0.2), (530.0, 0.8), (600.0, 0.2), (700.0, 0.2)])
            .with_quantity(QuantityKind::Reflectance);
        let id_a = app.alloc_id();
        app.stimulus_order.push(id_a);
        app.stimulus_curves.insert(
            id_a,
            crate::state::StimulusEntry {
                curve: reddish,
                file_path: None,
                dirty: false,
            },
        );
        let id_b = app.alloc_id();
        app.stimulus_order.push(id_b);
        app.stimulus_curves.insert(
            id_b,
            crate::state::StimulusEntry {
                curve: greenish,
                file_path: None,
                dirty: false,
            },
        );
        app.comparison.selected_stimuli = vec![id_a, id_b];
        let set_id = app.workspace.open_tabs[0];
        app.comparison.selected_species = vec![set_id];
        (app, id_a, id_b)
    }

    #[test]
    fn export_requires_selections() {
        let (mut app, _, _) = setup_app();
        app.comparison.selected_species.clear();
        let err = run_export(&mut app, "/tmp/does-not-matter.csv").unwrap_err();
        assert!(err.contains("species"));
    }

    #[test]
    fn export_writes_one_row_per_stimulus_with_header() {
        let (mut app, _, _) = setup_app();
        let dir = std::env::temp_dir().join(format!(
            "xenovision-batch-export-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.csv");

        let n = run_export(&mut app, path.to_str().unwrap()).unwrap();
        assert_eq!(n, 2);
        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3, "header + 2 rows");
        assert!(lines[0].starts_with("Stimulus,"));
        assert!(lines[1].starts_with("Reddish,"));
        assert!(lines[2].starts_with("Greenish,"));

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn export_with_reference_adds_distance_column() {
        let (mut app, id_a, _) = setup_app();
        app.comparison.export.include_distance_to_reference = true;
        app.comparison.export.reference_stimulus = Some(id_a);
        let dir = std::env::temp_dir().join(format!(
            "xenovision-batch-export-dist-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.csv");

        run_export(&mut app, path.to_str().unwrap()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let mut lines = contents.lines();
        let header = lines.next().unwrap();
        assert!(header.contains("Distance-to-ref"));
        // The reference row's own distance to itself must be 0.
        let reddish_row = lines.next().unwrap();
        let last_field: &str = reddish_row.split(',').next_back().unwrap();
        let d: f64 = last_field.parse().unwrap();
        assert!(d.abs() < 1e-9, "reference row's distance to itself should be ~0, got {d}");

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn export_fails_without_a_reference_when_distance_requested() {
        let (mut app, _, _) = setup_app();
        app.comparison.export.include_distance_to_reference = true;
        app.comparison.export.reference_stimulus = None;
        let err = run_export(&mut app, "/tmp/does-not-matter-2.csv").unwrap_err();
        assert!(err.contains("reference"));
    }
}
