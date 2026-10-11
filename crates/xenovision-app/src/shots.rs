//! Scriptable documentation screenshots (`--screenshot`).
//!
//! Launched as `xenovision-app --screenshot <outdir> --demo-data <dir>`, the
//! app seeds its state with real sample data - USGS Spectral Library
//! reflectance curves (the pre-converted `*.json` artifacts under
//! `data/usgs_splib07/curves/json`, which are already in this app's own Curve
//! Set format) and ENVI hyperspectral cubes under `data/envi` - then drives
//! itself frame by frame, capturing one PNG per panel straight from the GPU
//! framebuffer via `ViewportCommand::Screenshot`, and exits.
//!
//! This is the mechanism behind `scripts/regenerate-docs-screenshots.ps1`:
//! re-running the app regenerates every image in `docs/screenshots/`, so the
//! user guide can never drift from the UI. Nothing is written to the user's
//! data directory - all seeding happens in memory, so a run is reproducible
//! and never mutates real user data.
//!
//! All captures are of the *root* window, with the active docked container
//! switched between scenes. That keeps the driver to the single, well-trodden
//! egui screenshot path (request on the root viewport, read the
//! `Event::Screenshot` reply next frame) instead of the more fragile
//! float-a-window-and-collect-per-viewport alternative.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui::{ColorImage, ViewportCommand, ViewportId};

use xenovision_core::hyperspectral::{self, Region};
use xenovision_core::{ChannelStage, CurveSet, SpectralCurve};

use crate::dock::{PanelId, PanelKind};
use crate::image_library::{ImageSource, ImageSourceId};
use crate::state::{AppState, CurveId, CurveSetId, StimulusEntry};
use crate::window_image_display::{self, Composite, DisplayMode};
use crate::window_stimulus_editor::CreationMode;

/// Hard cap on frames pumped before giving up, so a stuck render can never
/// wedge an unattended build.
const MAX_FRAMES: u64 = 4000;

/// Datasets seeded into the stimulus library, as
/// `(filename substring, display name, corpus group)`.
///
/// Every entry is a single-reflectance Curve Set JSON under
/// `data/usgs_splib07/curves/json`; the substring is matched against file
/// names while walking that tree, so the exact chapter sub-directory doesn't
/// have to be hard-coded.
const CURVE_SAMPLES: &[(&str, &str, &str)] = &[
    (
        "Lawn_Grass_GDS91_green_BECKa_AREF",
        "Lawn grass (green)",
        "USGS splib07/Vegetation",
    ),
    (
        "Oak_Oak-Leaf-1_fresh_ASDFRa_AREF",
        "Oak leaf (fresh)",
        "USGS splib07/Vegetation",
    ),
    (
        "Sagebrush_Sage-Leaves-1_dry_ASDFRa_AREF",
        "Sagebrush leaves (dry)",
        "USGS splib07/Vegetation",
    ),
    (
        "Flower_Petunia-1_Pink_ASDFRa_AREF",
        "Petunia flower (pink)",
        "USGS splib07/Vegetation",
    ),
    (
        "Lodgepole-Pine_LP-Needles-1_ASDFRa_AREF",
        "Lodgepole pine needles",
        "USGS splib07/Vegetation",
    ),
    (
        "Melting_snow_mSnw01a_ASDFRa_AREF",
        "Melting snow",
        "USGS splib07/Snow and ice",
    ),
];

/// One panel to capture, in order. The scene's name is also its PNG file
/// name (plus `.png`).
struct SceneDef {
    name: &'static str,
    /// Frames to let the scene settle (and any async work to finish) before
    /// requesting the screenshot.
    settle: u32,
}

const SCENES: [SceneDef; 5] = [
    SceneDef {
        name: "main-window",
        settle: 20,
    },
    SceneDef {
        name: "comparison",
        settle: 20,
    },
    SceneDef {
        name: "stimulus-editor",
        settle: 20,
    },
    SceneDef {
        name: "image-display",
        settle: 10,
    },
    SceneDef {
        name: "stimulus-editor-import",
        settle: 20,
    },
];

/// Parsed `--screenshot` / `--demo-data` command line.
pub struct Config {
    pub out_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Config {
    /// Returns `Some` when `--screenshot <dir>` is present; `--demo-data` is
    /// optional and falls back to the workspace `data/` directory.
    pub fn from_args(args: &[String]) -> Option<Config> {
        let mut out_dir: Option<PathBuf> = None;
        let mut data_dir: Option<PathBuf> = None;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--screenshot" => {
                    out_dir = args.get(i + 1).map(PathBuf::from);
                    i += 2;
                }
                "--demo-data" => {
                    data_dir = args.get(i + 1).map(PathBuf::from);
                    i += 2;
                }
                _ => i += 1,
            }
        }
        Some(Config {
            out_dir: out_dir?,
            data_dir: data_dir.unwrap_or_else(default_data_dir),
        })
    }
}

/// The workspace `data/` directory, resolved relative to this crate's manifest
/// at compile time (so `cargo run -- --screenshot ...` works from anywhere),
/// falling back to a plain relative `data`.
fn default_data_dir() -> PathBuf {
    if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
        let candidate = Path::new(manifest).join("..").join("..").join("data");
        if candidate.is_dir() {
            return candidate;
        }
    }
    PathBuf::from("data")
}

enum Phase {
    Settle { scene: usize, frames_left: u32 },
    Await { scene: usize, waited: u32 },
    Finished,
}

/// Drives seeding once, then one screenshot per [`SCENES`] entry, then exits.
pub struct ShotSession {
    cfg: Config,
    phase: Phase,
    /// The screenshot reply collected from this frame's input, if any.
    pending: Option<Arc<ColorImage>>,

    // Seeded state the scene setups point the UI at.
    human_set: Option<CurveSetId>,
    editor_panel: PanelId,
    image_panel: PanelId,
    butterfly: Option<ImageSourceId>,
    butterfly_path: String,
    /// Library curves to check in the Comparison panel.
    comparison_stimuli: Vec<CurveId>,

    frames: u64,
    written: Vec<PathBuf>,
}

impl ShotSession {
    /// Seeds `state` with the demo data and returns a session parked on the
    /// first scene.
    pub fn start(state: &mut AppState, cfg: Config) -> ShotSession {
        if let Err(e) = std::fs::create_dir_all(&cfg.out_dir) {
            eprintln!("[shots] could not create {}: {e}", cfg.out_dir.display());
        }

        let hot = |s: &crate::state::AppCurveSet, prefix: &str| s.name.starts_with(prefix);
        let human_set = state
            .curve_sets
            .iter()
            .find(|(_, s)| hot(s, "Human"))
            .map(|(id, _)| *id);
        let honeybee_set = state
            .curve_sets
            .iter()
            .find(|(_, s)| hot(s, "Honeybee"))
            .map(|(id, _)| *id);

        // The three built-in example stimuli are used as a fallback selection
        // if the USGS library can't be found.
        let builtin_stimuli: Vec<CurveId> = state.stimulus_order.clone();

        // --- USGS reflectance spectra -> stimulus library ---
        let mut usgs_ids: Vec<CurveId> = Vec::new();
        for &(pattern, friendly, corpus) in CURVE_SAMPLES {
            match load_usgs_curve(&cfg.data_dir, pattern) {
                Some(mut set) => {
                    if let Some(mut curve) = set.curves.pop() {
                        curve.name = friendly.to_string();
                        curve
                            .metadata
                            .insert("batch".to_string(), corpus.to_string());
                        usgs_ids.push(add_stimulus(state, curve));
                    }
                }
                None => eprintln!("[shots] USGS curve not found: {pattern}"),
            }
        }

        // --- ENVI cubes -> image library ---
        let butterfly_path = cfg
            .data_dir
            .join("envi")
            .join("tokyotech")
            .join("Butterfly.hdr");
        let butterfly = load_source(state, &butterfly_path, "Butterfly (TokyoTech)");
        let _coffee = load_source(
            state,
            &cfg.data_dir
                .join("envi")
                .join("hacarus-nir")
                .join("coffee.hdr"),
            "Coffee beans (Hacarus NIR)",
        );

        // Put Human first so both the Workspace tab and the Image Display
        // view default to a familiar system, and pre-select its first
        // receptor so the graph isn't empty.
        if let Some(h) = human_set {
            if let Some(pos) = state.workspace.open_tabs.iter().position(|&x| x == h) {
                let h = state.workspace.open_tabs.remove(pos);
                state.workspace.open_tabs.insert(0, h);
            }
            state.workspace.active_tab = 0;
            let first = state
                .curve_sets
                .get(&h)
                .and_then(|s| s.curve_order.first().copied());
            if let (Some(first), Some(tab)) = (first, state.workspace.per_tab.get_mut(&h)) {
                tab.selected_curve = Some(first);
            }
        }

        // --- Comparison: two species, a handful of USGS spectra ---
        state.comparison.reference_luminant = state.luminant_order.first().copied();
        state.comparison.selected_species =
            [human_set, honeybee_set].into_iter().flatten().collect();
        state.comparison.active_species_subtab = 0;
        let comparison_stimuli: Vec<CurveId> = if usgs_ids.len() >= 4 {
            usgs_ids.iter().take(4).copied().collect()
        } else {
            builtin_stimuli.iter().take(3).copied().collect()
        };
        state.comparison.selected_stimuli = comparison_stimuli.clone();

        // --- Stimulus Editor: open on a real reflectance curve ---
        state.stimulus_editor.selected = usgs_ids
            .first()
            .copied()
            .or_else(|| builtin_stimuli.first().copied());

        // --- create the three repeatable panels (in-container) ---
        let _comparison_panel = state.new_panel(PanelKind::Comparison, false);
        let editor_panel = state.new_panel(PanelKind::StimulusEditor, false);
        let image_panel = state.new_panel(PanelKind::ImageDisplay, false);

        // --- seed one Image Display view (Human, perceptual composite) ---
        if let Some(src) = butterfly {
            let idx = window_image_display::seed_view(state, image_panel, Some(src), None);
            if let Some(display) = state.image_displays.get_mut(&image_panel) {
                if let Some(view) = display.views.get_mut(idx) {
                    view.stage = ChannelStage::Perceptual;
                    view.display = DisplayMode::Composite;
                    view.composite = Composite::PERCEPTUAL;
                }
            }
        }

        let session = ShotSession {
            cfg,
            phase: Phase::Settle {
                scene: 0,
                frames_left: SCENES[0].settle,
            },
            pending: None,
            human_set,
            editor_panel,
            image_panel,
            butterfly,
            butterfly_path: butterfly_path.to_string_lossy().into_owned(),
            comparison_stimuli,
            frames: 0,
            written: Vec::new(),
        };
        session.setup_scene(0, state);
        session
    }

    /// Reads this frame's input for a screenshot reply. Called at the *start*
    /// of the root frame so the reply is available to [`Self::end_frame`].
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        if matches!(self.phase, Phase::Finished) {
            return;
        }
        let reply = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(Arc::clone(image)),
                _ => None,
            })
        });
        if reply.is_some() {
            self.pending = reply;
        }
    }

    /// Drives the scene state machine and, when the last image is saved,
    /// exits the process with a deterministic status code.
    pub fn end_frame(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if matches!(self.phase, Phase::Finished) {
            return;
        }
        self.frames += 1;
        if self.frames > MAX_FRAMES {
            self.finish("gave up after too many frames", 2);
        }

        match self.phase {
            Phase::Settle { scene, frames_left } => {
                if frames_left == 0 && self.scene_ready(scene, state) {
                    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Screenshot);
                    self.phase = Phase::Await { scene, waited: 0 };
                } else if frames_left > 0 {
                    self.phase = Phase::Settle {
                        scene,
                        frames_left: frames_left - 1,
                    };
                }
                ctx.request_repaint();
            }
            Phase::Await { scene, waited } => {
                if let Some(image) = self.pending.take() {
                    self.save_scene(scene, &image);
                    let next = scene + 1;
                    if next >= SCENES.len() {
                        self.finish("done", 0);
                    } else {
                        self.setup_scene(next, state);
                        self.phase = Phase::Settle {
                            scene: next,
                            frames_left: SCENES[next].settle,
                        };
                        ctx.request_repaint();
                    }
                } else if waited > 300 {
                    eprintln!(
                        "[shots] no screenshot reply for '{}' - aborting",
                        SCENES[scene].name
                    );
                    self.finish("aborted (no screenshot reply)", 3);
                } else {
                    self.phase = Phase::Await {
                        scene,
                        waited: waited + 1,
                    };
                    ctx.request_repaint();
                }
            }
            Phase::Finished => {}
        }
    }

    /// Points the UI at the scene's panel. Called once as each scene begins.
    fn setup_scene(&self, scene: usize, state: &mut AppState) {
        match SCENES[scene].name {
            "main-window" => {
                state.dock.active = Some(PanelKind::Workspace);
                if let Some(h) = self.human_set {
                    if let Some(pos) = state.workspace.open_tabs.iter().position(|&x| x == h) {
                        state.workspace.active_tab = pos;
                    }
                }
            }
            "comparison" => {
                state.dock.active = Some(PanelKind::Comparison);
            }
            "stimulus-editor" => {
                state.dock.active = Some(PanelKind::StimulusEditor);
                let selected = self.comparison_stimuli.first().copied();
                state.with_stimulus_editor(self.editor_panel, |app| {
                    app.stimulus_editor.creation_mode = CreationMode::None;
                    if let Some(id) = selected {
                        app.stimulus_editor.selected = Some(id);
                    }
                });
            }
            "image-display" => {
                state.dock.active = Some(PanelKind::ImageDisplay);
            }
            "stimulus-editor-import" => {
                state.dock.active = Some(PanelKind::StimulusEditor);
                if let Some(src) = self.butterfly {
                    state.image_library.select(src);
                }
                let path = self.butterfly_path.clone();
                state.with_stimulus_editor(self.editor_panel, |app| {
                    app.stimulus_editor.creation_mode = CreationMode::Image;
                    app.stimulus_editor.image_path = path;
                    if app.stimulus_editor.selection.region.is_none() {
                        app.stimulus_editor.selection.region = Some(Region::Rectangle {
                            x0: 150.0,
                            y0: 140.0,
                            x1: 360.0,
                            y1: 340.0,
                        });
                    }
                });
            }
            _ => {}
        }
    }

    /// Whether the scene has finished any asynchronous work (currently only
    /// the Image Display's background render).
    fn scene_ready(&self, scene: usize, state: &AppState) -> bool {
        match SCENES[scene].name {
            "image-display" => state
                .image_displays
                .get(&self.image_panel)
                .and_then(|d| d.views.first())
                .map(|v| v.is_settled())
                .unwrap_or(true),
            _ => true,
        }
    }

    fn save_scene(&mut self, scene: usize, image: &ColorImage) {
        let path = self.cfg.out_dir.join(format!("{}.png", SCENES[scene].name));
        match write_png(&path, image) {
            Ok(()) => {
                println!("[shots] wrote {}", path.display());
                self.written.push(path);
            }
            Err(e) => eprintln!("[shots] failed to write {}: {e}", path.display()),
        }
    }

    /// Prints a summary and exits. Exiting directly (rather than asking the
    /// event loop to close) gives the calling script a deterministic status
    /// code in one step.
    fn finish(&mut self, reason: &str, code: i32) -> ! {
        self.phase = Phase::Finished;
        println!(
            "[shots] {reason}: {} image(s) in {}",
            self.written.len(),
            self.cfg.out_dir.display()
        );
        for path in &self.written {
            println!("[shots]   {}", path.display());
        }
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        std::process::exit(code);
    }
}

// ---------------------------------------------------------------------------
// Seeding helpers
// ---------------------------------------------------------------------------

fn add_stimulus(state: &mut AppState, curve: SpectralCurve) -> CurveId {
    let id = state.alloc_id();
    state.stimulus_order.push(id);
    state.stimulus_curves.insert(
        id,
        StimulusEntry {
            curve,
            file_path: None,
            dirty: false,
        },
    );
    id
}

fn load_usgs_curve(data_dir: &Path, pattern: &str) -> Option<CurveSet> {
    let root = data_dir.join("usgs_splib07").join("curves").join("json");
    let path = find_file(&root, pattern)?;
    match CurveSet::load_from_file(&path) {
        Ok(set) => Some(set),
        Err(e) => {
            eprintln!("[shots] failed to parse {}: {e}", path.display());
            None
        }
    }
}

fn load_source(state: &mut AppState, path: &Path, label: &str) -> Option<ImageSourceId> {
    if !path.is_file() {
        eprintln!("[shots] image not found: {}", path.display());
        return None;
    }
    match hyperspectral::load_envi_or_geotiff(path) {
        Ok(cube) => Some(state.image_library.add(ImageSource::new(
            label,
            path.to_string_lossy(),
            cube,
        ))),
        Err(e) => {
            eprintln!("[shots] failed to load {}: {e}", path.display());
            None
        }
    }
}

/// Depth-first search for the first file whose name contains `needle`.
fn find_file(dir: &Path, needle: &str) -> Option<PathBuf> {
    let mut subdirs = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(needle))
        {
            return Some(path);
        }
    }
    subdirs.sort();
    for d in subdirs {
        if let Some(found) = find_file(&d, needle) {
            return Some(found);
        }
    }
    None
}

/// Encodes an egui screenshot as an opaque RGBA PNG.
fn write_png(path: &Path, image: &ColorImage) -> Result<(), String> {
    use image::ImageEncoder as _;

    let w = image.size[0] as u32;
    let h = image.size[1] as u32;
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for px in &image.pixels {
        // The framebuffer can carry an unused alpha channel (and eframe's
        // default clear is transparent in some configurations) - force every
        // pixel opaque so the guide never shows through the images.
        rgba.extend_from_slice(&[px.r(), px.g(), px.b(), 255]);
    }

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let writer = std::io::BufWriter::new(file);
    image::codecs::png::PngEncoder::new(writer)
        .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}
