//! Image Display window: renders a loaded image (or one region of it)
//! through a visual system's receptor channels, using
//! `xenovision_core::receptor_image`.
//!
//! The window hosts a list of **views**. Each view is one configured
//! render - a source image, an optional region, a visual system + regime,
//! a receptor stage (raw / von Kries-adapted / perceptual), an input
//! interpretation (reflectance vs radiance), and a display mapping. Two
//! things are deliberately always on screen so the result is never
//! ambiguous:
//!
//! - a **provenance bar** naming every input (source + band axis, region,
//!   input kind + luminant, species + regime, stage, step, GPU/CPU), and
//! - a **false-color legend** spelling out the mapping actually used.
//!
//! Stage-aware defaults: raw/adapted stages show one monochrome panel per
//! receptor (a viridis-like ramp, per-channel min–max), while the
//! perceptual stage shows a single `[L, C_a, C_b]` composite through an
//! L\*a\*b\*-style encoder. The two chroma axes default to `C1`/`C2` but are
//! selectable, so a regime with more than three receptors can represent any
//! pair of its chroma axes. Any stage can instead be shown as an explicit
//! channel→R/G/B composite.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use xenovision_core::gpu::GpuContext;
use xenovision_core::hyperspectral::{HyperspectralCube, Region};
use xenovision_core::{ChannelStage, InputKind, ReceptorImage, ReceptorKernel, SpectralCurve};

use crate::colormap::{self, Colormap};
use crate::dock::{DockAction, PanelId, PanelInstance, PanelKind};
use crate::image_library::{self, region_label, ImageLibrary, ImageSource, ImageSourceId};
use crate::region_selector::{region_selector, RegionDraft};
use crate::render_worker::{RenderEngine, RenderJob, RenderOutcome, RenderProduct};
use crate::state::{AppCurveSet, AppState, CurveId, CurveSetId};

const STEP_NM: f64 = 1.0;

/// How a view turns its output channels into pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// One monochrome panel per channel (the default for raw/adapted).
    Channels,
    /// One combined image (the default for perceptual).
    Composite,
}

/// Which combined image a `DisplayMode::Composite` view draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Composite {
    /// `[L, C_a, C_b]` through the L\*a\*b\*-style encoder: channel 0 drives
    /// lightness, channel `chroma_a` drives green↔magenta, and channel
    /// `chroma_b` drives blue↔yellow. Both chroma axes are selectable so a
    /// regime with more than three receptors can represent any pair of its
    /// `N-1` chroma axes.
    Perceptual { chroma_a: usize, chroma_b: usize },
    /// Three chosen channels into R/G/B.
    Rgb { r: usize, g: usize, b: usize },
}

impl Composite {
    /// The default perceptual mapping: `C1`→green/magenta, `C2`→blue/yellow.
    pub const PERCEPTUAL: Composite = Composite::Perceptual {
        chroma_a: 1,
        chroma_b: 2,
    };
}

/// Which region a view renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionChoice {
    Whole,
    Saved(usize),
    Custom,
}

/// One configured, cached render inside the Image Display window.
pub struct ImageView {
    pub id: u64,
    pub title: String,
    pub source: Option<ImageSourceId>,
    pub region_choice: RegionChoice,
    /// The region actually rendered (`None` = whole image). Kept in sync
    /// with `region_choice` + (for `Custom`) the draft.
    pub region: Option<Region>,
    pub draft: RegionDraft,
    pub input_kind: InputKind,
    pub luminant: Option<CurveId>,
    pub set_id: Option<CurveSetId>,
    /// `None` = the dominant regime of the selected set.
    pub regime: Option<usize>,
    pub stage: ChannelStage,
    pub display: DisplayMode,
    pub composite: Composite,
    pub colormap: Colormap,
    pub step_nm: f64,

    /// The display mapping (`(display, composite, colormap)`) the textures in
    /// `render` were built from. A change here rebuilds only the textures; it
    /// does not re-render the channel data.
    texture_mapping: Option<(DisplayMode, Composite, Colormap)>,
    render: Option<RenderedView>,
    cache_key: Option<String>,
    /// Cache key of the render currently in flight in the worker, `None`
    /// when nothing is pending. A finished outcome is installed only if
    /// its key still matches this.
    pending: Option<String>,
    pub error: Option<String>,
}

impl ImageView {
    fn new(id: u64, source: Option<ImageSourceId>) -> Self {
        ImageView {
            id,
            title: format!("View {id}"),
            source,
            region_choice: RegionChoice::Whole,
            region: None,
            draft: RegionDraft::default(),
            input_kind: InputKind::Reflectance,
            luminant: None,
            set_id: None,
            regime: None,
            stage: ChannelStage::RawReceptor,
            display: DisplayMode::Channels,
            composite: Composite::Rgb { r: 0, g: 0, b: 0 },
            colormap: Colormap::Viridis,
            step_nm: STEP_NM,
            texture_mapping: None,
            render: None,
            cache_key: None,
            pending: None,
            error: None,
        }
    }

    /// Whether this view has reached a resting state: a finished render is
    /// installed, or the render failed. Used by the documentation-screenshot
    /// driver (`shots.rs`) to wait for the background renderer before
    /// capturing the Image Display panel.
    pub fn is_settled(&self) -> bool {
        self.render.is_some() || self.error.is_some()
    }
}

/// A finished render: the channel data plus the textures currently shown.
pub struct RenderedView {
    pub image: ReceptorImage,
    /// One texture for `DisplayMode::Channels`, one for `Composite`.
    pub textures: Vec<egui::TextureHandle>,
    pub used_gpu: bool,
}

/// Image Display window state.
#[derive(Default)]
pub struct ImageDisplayState {
    pub views: Vec<ImageView>,
    next_view_id: u64,
    pub load_path: String,
    pub load_status: String,
    /// The background render worker every view's job is submitted to.
    pub engine: RenderEngine,
}

impl ImageDisplayState {
    fn alloc_view_id(&mut self) -> u64 {
        self.next_view_id += 1;
        self.next_view_id
    }
}

// ---------------------------------------------------------------------------
// Seeding / adding views
// ---------------------------------------------------------------------------

/// Adds a view seeded from `source_id` (and, when provided, `region`) to the
/// Image Display panel `panel`, with defaults chosen for the currently-open
/// visual system. Returns the new view's index.
pub fn seed_view(
    app: &mut AppState,
    panel: PanelId,
    source_id: Option<ImageSourceId>,
    region: Option<Region>,
) -> usize {
    // Gather everything that reads shared state *before* borrowing the panel,
    // so the two borrows never overlap.
    let set_id = default_set(app);
    let luminant = app.comparison.reference_luminant;
    let title = source_id
        .and_then(|sid| app.image_library.get(sid))
        .map(|s| s.label.clone());
    let state = app
        .image_displays
        .get_mut(&panel)
        .expect("target Image Display panel must exist");
    let id = state.alloc_view_id();
    let mut view = ImageView::new(id, source_id);
    view.set_id = set_id;
    view.luminant = luminant;
    if let Some(r) = region {
        view.draft.region = Some(r);
        view.region = Some(view.draft.region.clone().unwrap());
        view.region_choice = RegionChoice::Custom;
    }
    view.title = title.unwrap_or_else(|| format!("View {id}"));
    state.views.push(view);
    state.views.len() - 1
}

fn default_set(app: &AppState) -> Option<CurveSetId> {
    app.workspace
        .open_tabs
        .first()
        .copied()
        .or_else(|| app.curve_sets.keys().copied().min())
}

/// The button in the Stimulus Editor: opens (or reuses) an Image Display
/// panel and seeds a view from the currently selected image + region.
pub fn seed_from_editor(app: &mut AppState) {
    let panel = app.primary_image_display();
    let source = app.image_library.selected_id();
    let region = app.stimulus_editor.selection.region.clone();
    seed_view(app, panel, source, region);
    // Bring the panel forward: select the instance and, when it is in its
    // container, activate the Image Display container tab (a popped-out one is
    // already visible).
    let inst = PanelInstance::Panel(panel);
    app.set_active_instance(inst);
    if !app.dock.instance_is_floating(inst) {
        app.pending_dock_actions
            .push(DockAction::ActivateContainer(PanelKind::ImageDisplay));
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn region_sig(r: &Option<Region>) -> String {
    match r {
        None => "whole".to_string(),
        Some(Region::Rectangle { x0, y0, x1, y1 }) => format!("R{x0},{y0},{x1},{y1}"),
        Some(Region::Polygon(points)) => {
            let mut s = String::from("P");
            for (x, y) in points {
                s.push_str(&format!("{x},{y};"));
            }
            s
        }
    }
}

fn dominant_index(kernels: &[ReceptorKernel]) -> usize {
    kernels
        .iter()
        .enumerate()
        .max_by(|a, b| {
            a.1.regime_gain()
                .partial_cmp(&b.1.regime_gain())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Builds a view's cache key: changes exactly when any input that affects
/// the rendered pixels changes. The display mapping (colormap, composite,
/// chroma axes) is deliberately *not* part of it - it only changes the
/// textures, which are rebuilt without a worker round-trip (see `mapping`).
fn view_key(view: &ImageView, source: &ImageSource, set_revision: u64) -> String {
    format!(
        "src={} gen={} region={} kind={:?} lum={:?} set={:?} rev={} regime={:?} stage={:?} step={}",
        source.id,
        source.generation,
        region_sig(&view.region),
        view.input_kind,
        view.luminant,
        view.set_id,
        set_revision,
        view.regime,
        view.stage,
        view.step_nm,
    )
}

/// The display mapping that turns rendered channels into the shown textures,
/// as opposed to the inputs in `view_key`, which change the channels
/// themselves. When this changes, `render_pass` rebuilds the textures from
/// the channels already in hand.
fn mapping(view: &ImageView) -> (DisplayMode, Composite, Colormap) {
    (view.display, view.composite, view.colormap)
}

fn stage_label(stage: ChannelStage) -> &'static str {
    match stage {
        ChannelStage::RawReceptor => "raw receptor",
        ChannelStage::AdaptedReceptor => "von Kries adapted",
        ChannelStage::Perceptual => "perceptual",
    }
}

/// Selects a view's stage and applies its stage-aware display default: raw
/// and adapted stages each produce one channel per receptor, so they show one
/// panel per channel; the perceptual stage shows a single `[L, C_a, C_b]`
/// composite. (An explicit channel→R/G/B composite remains available at any
/// stage.)
fn apply_stage(view: &mut ImageView, stage: ChannelStage) {
    view.stage = stage;
    match stage {
        ChannelStage::Perceptual => {
            view.display = DisplayMode::Composite;
            view.composite = Composite::PERCEPTUAL;
        }
        ChannelStage::RawReceptor | ChannelStage::AdaptedReceptor => {
            view.display = DisplayMode::Channels;
        }
    }
}

/// One frame's render bookkeeping for every view: install any finished
/// worker renders, then dispatch the views whose inputs changed. Called
/// before drawing; the heavy pixel work itself happens off-thread.
fn render_pass(ctx: &egui::Context, app: &mut AppState, id: PanelId) {
    let AppState {
        image_displays,
        image_library,
        curve_sets,
        luminants,
        gpu,
        ..
    } = app;

    let Some(image_display) = image_displays.get_mut(&id) else {
        return; // the panel was closed
    };

    // Band previews (for region drawing) - cheap, one band each.
    for view in &image_display.views {
        if let Some(source_id) = view.source {
            if let Some(source) = image_library.get_mut(source_id) {
                ensure_preview(ctx, source);
            }
        }
    }

    let ImageDisplayState { views, engine, .. } = image_display;

    // Install finished renders, dropping any the view has moved past.
    while let Some(outcome) = engine.try_recv() {
        install_outcome(ctx, outcome, views);
    }

    // Dispatch views whose inputs changed since their last (or pending)
    // render.
    for view in views.iter_mut() {
        dispatch_view(
            view,
            image_library,
            curve_sets,
            luminants,
            gpu.as_ref(),
            engine,
        );
    }

    // A display-only change (colormap, composite, or which chroma axes the
    // composite encodes) reuses the channels already rendered: rebuild the
    // textures here instead of round-tripping the worker. This is also what
    // makes picking chroma axes instant on a large cube.
    for view in views.iter_mut() {
        if view.render.is_none() || view.texture_mapping == Some(mapping(view)) {
            continue;
        }
        if let Some(mut rendered) = view.render.take() {
            rendered.textures = build_textures(ctx, view, &rendered.image);
            view.render = Some(rendered);
            view.texture_mapping = Some(mapping(view));
        }
    }

    // egui is reactive by default: with no input, it would stop painting
    // and the finished render would not be collected until the next event.
    // Keep the loop ticking while any job is in flight so results land
    // promptly (and the spinner animates).
    if views.iter().any(|v| v.pending.is_some()) {
        ctx.request_repaint();
    }
}

/// Installs a finished render, unless the view is gone or has since been
/// re-pointed at different inputs (a stale outcome).
fn install_outcome(ctx: &egui::Context, outcome: RenderOutcome, views: &mut [ImageView]) {
    let Some(view) = views.iter_mut().find(|v| v.id == outcome.view_id) else {
        return; // the view was removed while it was rendering
    };
    if view.pending.as_deref() != Some(outcome.key.as_str()) {
        return; // inputs changed again; this result is stale
    }
    view.pending = None;
    match outcome.result {
        Ok(RenderProduct { image, used_gpu }) => {
            let textures = build_textures(ctx, view, &image);
            view.render = Some(RenderedView {
                image,
                textures,
                used_gpu,
            });
            view.texture_mapping = Some(mapping(view));
            view.error = None;
            view.cache_key = Some(outcome.key);
        }
        Err(e) => {
            view.render = None;
            view.error = Some(e);
            view.cache_key = Some(outcome.key);
        }
    }
}

/// Submits `view` to the worker when its inputs no longer match the render
/// it is showing (or already has in flight).
fn dispatch_view(
    view: &mut ImageView,
    lib: &ImageLibrary,
    sets: &HashMap<CurveSetId, AppCurveSet>,
    luminants: &HashMap<CurveId, SpectralCurve>,
    gpu: Option<&Arc<GpuContext>>,
    engine: &RenderEngine,
) {
    // The perceptual composite encodes the perceptual stage's [L, C_a, C_b];
    // at any other (effective) stage - including a regime-less pool, which is
    // always rendered raw - fall back to a channel→R/G/B composite so a stale
    // choice can never paint a mislabelled image.
    let regime_less = view
        .set_id
        .and_then(|id| sets.get(&id))
        .map(|s| s.regimes.is_empty())
        .unwrap_or(true);
    let effective_stage = if regime_less {
        ChannelStage::RawReceptor
    } else {
        view.stage
    };
    if effective_stage != ChannelStage::Perceptual
        && matches!(view.composite, Composite::Perceptual { .. })
    {
        view.composite = Composite::Rgb { r: 0, g: 1, b: 2 };
    }

    let Some(source) = view.source.and_then(|id| lib.get(id)) else {
        view.render = None;
        view.cache_key = None;
        view.pending = None;
        view.error = Some("No image selected.".to_string());
        return;
    };

    let set_revision = view
        .set_id
        .and_then(|id| sets.get(&id))
        .map(|s| s.revision)
        .unwrap_or(0);
    let key = view_key(view, source, set_revision);
    // Settled for these inputs: either we have the render, or we already
    // tried and it failed (retrying the same failing inputs every frame
    // would just spam the worker). A changed input - or the user fixing the
    // cause (assigning a wavelength axis, picking a visual system) - changes
    // the key and re-dispatches.
    if view.cache_key.as_deref() == Some(key.as_str())
        && (view.render.is_some() || view.error.is_some())
    {
        view.pending = None;
        return;
    }
    if view.pending.as_deref() == Some(key.as_str()) {
        return; // already requested with these exact inputs
    }

    match build_job(view, source, sets, luminants) {
        Ok((kernel, region, cube)) => {
            engine.submit(RenderJob {
                view_id: view.id,
                key: key.clone(),
                cube,
                kernel,
                region,
                gpu: gpu.cloned(),
            });
            view.pending = Some(key);
            view.error = None;
        }
        Err(e) => {
            view.render = None;
            view.cache_key = Some(key);
            view.pending = None;
            view.error = Some(e);
        }
    }
}

/// Everything needed to render `view` off-thread: the precomputed kernel,
/// the region, and a shared handle to the source cube. The cheap,
/// app-type-bound work (resolving the regime and building the kernel)
/// happens here; only the per-pixel pass is offloaded.
fn build_job(
    view: &ImageView,
    source: &ImageSource,
    sets: &HashMap<CurveSetId, AppCurveSet>,
    luminants: &HashMap<CurveId, SpectralCurve>,
) -> Result<(ReceptorKernel, Option<Region>, Arc<HyperspectralCube>), String> {
    let cube = source.cube.clone();
    let wl = cube.wavelengths_nm.as_deref().ok_or_else(|| {
        "Assign a wavelength axis to this image (Stimulus Editor) first.".to_string()
    })?;
    if wl.is_empty() {
        return Err("The image's wavelength axis is empty.".to_string());
    }
    let set_id = view
        .set_id
        .ok_or_else(|| "Choose a visual system.".to_string())?;
    let set = sets
        .get(&set_id)
        .ok_or_else(|| "The selected visual system no longer exists.".to_string())?;
    let core = set.to_core();
    let lum = view.luminant.and_then(|id| luminants.get(&id));

    let regime_less = core.regimes.is_empty();
    let stage = if regime_less {
        ChannelStage::RawReceptor
    } else {
        view.stage
    };

    let kernel = if regime_less {
        ReceptorKernel::for_pool(&core, wl, lum, view.input_kind, view.step_nm)
            .map_err(|e| e.to_string())?
    } else {
        let kernels = ReceptorKernel::kernels_for_all_regimes(
            &core,
            wl,
            lum,
            view.input_kind,
            stage,
            view.step_nm,
        )
        .map_err(|e| e.to_string())?;
        if kernels.is_empty() {
            return Err("The visual system has no regimes to render.".to_string());
        }
        let idx = view
            .regime
            .unwrap_or_else(|| dominant_index(&kernels))
            .min(kernels.len() - 1);
        kernels
            .into_iter()
            .nth(idx)
            .expect("index clamped in range")
    };
    Ok((kernel, view.region.clone(), cube))
}

/// Builds the texture(s) shown for a finished render, per display mode.
fn build_textures(
    ctx: &egui::Context,
    view: &ImageView,
    image: &ReceptorImage,
) -> Vec<egui::TextureHandle> {
    let opts = egui::TextureOptions::NEAREST;
    match view.display {
        DisplayMode::Channels => (0..image.channels.len())
            .map(|c| {
                let ci = channel_color_image(image, c, view.colormap);
                ctx.load_texture(format!("rec_v{}_c{}", view.id, c), ci, opts)
            })
            .collect(),
        DisplayMode::Composite => {
            let ci = composite_color_image(image, view.composite);
            vec![ctx.load_texture(format!("rec_v{}_comp", view.id), ci, opts)]
        }
    }
}

fn channel_color_image(image: &ReceptorImage, c: usize, cmap: Colormap) -> egui::ColorImage {
    let n_pixels = image.n_pixels();
    let norm = image
        .normalized_channel(c)
        .unwrap_or_else(|| vec![0.0; n_pixels]);
    let pixels = norm.iter().map(|&t| cmap.apply(t)).collect();
    egui::ColorImage {
        size: [image.width, image.height],
        pixels,
    }
}

fn composite_color_image(image: &ReceptorImage, comp: Composite) -> egui::ColorImage {
    let n_pixels = image.n_pixels();
    let mask = image.mask();
    let range = image.range();
    let n_channels = image.channels.len();
    let channel = |c: usize| image.channel(c);
    let mut pixels = Vec::with_capacity(n_pixels);
    for i in 0..n_pixels {
        let inside = mask.is_none_or(|m| m[i]);
        if !inside || n_channels == 0 {
            pixels.push(egui::Color32::BLACK);
            continue;
        }
        let color = match comp {
            Composite::Perceptual { chroma_a, chroma_b } if n_channels >= 3 => {
                // Channels are [L, C1..C_(N-1)]; clamp the selected chroma
                // axes in case the render shown is momentarily stale.
                let ia = chroma_a.clamp(1, n_channels - 1);
                let ib = chroma_b.clamp(1, n_channels - 1);
                let l = colormap::normalize_unit(channel(0).unwrap()[i], range[0].0, range[0].1);
                let c1 =
                    colormap::normalize_opponent(channel(ia).unwrap()[i], range[ia].0, range[ia].1);
                let c2 =
                    colormap::normalize_opponent(channel(ib).unwrap()[i], range[ib].0, range[ib].1);
                colormap::perceptual_lab(l, c1, c2)
            }
            Composite::Perceptual { .. } => egui::Color32::BLACK,
            Composite::Rgb { r, g, b } => {
                let idx = |c: usize| c.min(n_channels - 1);
                let get = |c: usize| {
                    let c = idx(c);
                    colormap::normalize_unit(channel(c).unwrap()[i], range[c].0, range[c].1)
                };
                let to_u8 = |v: f32| (v * 255.0).round() as u8;
                egui::Color32::from_rgb(to_u8(get(r)), to_u8(get(g)), to_u8(get(b)))
            }
        };
        pixels.push(color);
    }
    egui::ColorImage {
        size: [image.width, image.height],
        pixels,
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// The panel entry point (`PanelKind::ImageDisplay`) for the instance `id`.
pub fn ui(ui: &mut egui::Ui, app: &mut AppState, id: PanelId) {
    if !app.gpu_attempted {
        app.gpu = xenovision_core::gpu::try_init().map(Arc::new);
        app.gpu_attempted = true;
    }

    // The rail id is per-instance so several Image Display panels don't share
    // a remembered width.
    egui::SidePanel::left(egui::Id::new(("image_display_rail", id)))
        .resizable(true)
        .default_width(260.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| rail(ui, app, id));
        });
    egui::CentralPanel::default().show_inside(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| center(ui, app, id));
    });
}

fn rail(ui: &mut egui::Ui, app: &mut AppState, id: PanelId) {
    let gpu_text = match (&app.gpu, app.gpu_attempted) {
        (Some(_), _) => "GPU: wgpu adapter available",
        (None, true) => "GPU: unavailable — CPU fallback",
        (None, false) => "GPU: not probed yet",
    };

    // Actions that need the whole `AppState` (creating a panel, or seeding a
    // view) are recorded here and run once the panel/library borrows end.
    let mut add_selected = false;
    let mut add_blank = false;
    let mut new_window = false;
    {
        let AppState {
            image_displays,
            image_library,
            ..
        } = app;
        let Some(state) = image_displays.get_mut(&id) else {
            return; // the panel was closed
        };

        ui.heading(id.title());
        ui.label(gpu_text);

        ui.separator();
        ui.label("Load an ENVI (point at the .hdr), GeoTIFF, or .mat file:");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut state.load_path);
        });
        if ui.button("Load image").clicked() {
            let path = state.load_path.clone();
            match xenovision_core::hyperspectral::load_envi_or_geotiff(&path) {
                Ok(cube) => {
                    let label = source_label_from_path(&path);
                    image_library.add(ImageSource::new(label, path, cube));
                    state.load_status = "Loaded.".to_string();
                }
                Err(e) => state.load_status = format!("Load failed: {e}"),
            }
        }
        if !state.load_status.is_empty() {
            ui.label(&state.load_status);
        }

        ui.separator();
        ui.label(format!("Loaded images ({})", image_library.sources.len()));
        let mut to_select: Option<ImageSourceId> = None;
        let mut to_remove: Option<ImageSourceId> = None;
        for source in &image_library.sources {
            ui.horizontal(|ui| {
                let selected = image_library.selected_id() == Some(source.id);
                if ui
                    .selectable_label(selected, image_library::source_label(source))
                    .clicked()
                {
                    to_select = Some(source.id);
                }
                if ui.small_button("✕").on_hover_text("Unload image").clicked() {
                    to_remove = Some(source.id);
                }
            });
        }
        if let Some(sid) = to_select {
            image_library.select(sid);
        }
        if let Some(sid) = to_remove {
            image_library.remove(sid);
            for view in &mut state.views {
                if view.source == Some(sid) {
                    view.source = None;
                    view.region = None;
                }
            }
        }
        if let Some(selected) = image_library.selected() {
            ui.label(format!("Selected: {}", selected.label));
        }

        ui.separator();
        if ui
            .add_enabled(
                image_library.selected_id().is_some(),
                egui::Button::new("+ Add view for selected image"),
            )
            .on_hover_text("Add the selected image to this panel")
            .clicked()
        {
            add_selected = true;
        }
        if ui
            .button("+ Add blank view")
            .on_hover_text("Start an empty view here and pick its image on the right")
            .clicked()
        {
            add_blank = true;
        }
        if ui
            .button("＋ New Image Display window")
            .on_hover_text(
                "Open another independent panel as its own window; every panel shares \
                 the loaded images",
            )
            .clicked()
        {
            new_window = true;
        }
    }

    if new_window {
        app.new_image_display(true);
    }
    if add_selected {
        let source = app.image_library.selected_id();
        seed_view(app, id, source, None);
    }
    if add_blank {
        seed_view(app, id, None, None);
    }
}

fn source_label_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn center(ui: &mut egui::Ui, app: &mut AppState, id: PanelId) {
    let empty = app
        .image_displays
        .get(&id)
        .map(|s| s.views.is_empty())
        .unwrap_or(true);
    if empty {
        ui.label(
            "No views yet. Load an image on the left, then \"+ Add view for selected image\".",
        );
        return;
    }

    // Install finished background renders and dispatch any whose inputs
    // changed, before drawing them this frame.
    let ctx = ui.ctx().clone();
    render_pass(&ctx, app, id);

    let AppState {
        image_displays,
        image_library,
        curve_sets,
        luminants,
        ..
    } = app;
    let Some(state) = image_displays.get_mut(&id) else {
        return; // the panel was closed
    };

    let mut remove: Option<usize> = None;
    let mut duplicate: Option<usize> = None;
    for (i, view) in state.views.iter_mut().enumerate() {
        draw_view(
            ui,
            i,
            view,
            image_library,
            curve_sets,
            luminants,
            &mut remove,
            &mut duplicate,
        );
    }
    if let Some(i) = duplicate {
        let mut copy = ImageView::new(state.alloc_view_id(), state.views[i].source);
        copy.set_id = state.views[i].set_id;
        copy.luminant = state.views[i].luminant;
        copy.input_kind = state.views[i].input_kind;
        copy.stage = state.views[i].stage;
        copy.display = state.views[i].display;
        copy.composite = state.views[i].composite;
        copy.colormap = state.views[i].colormap;
        copy.region = state.views[i].region.clone();
        copy.title = format!("{} (copy)", state.views[i].title);
        state.views.push(copy);
    }
    if let Some(i) = remove {
        state.views.remove(i);
    }
}

fn ensure_preview(ctx: &egui::Context, source: &mut ImageSource) {
    let key = Some((source.generation, source.band));
    if source.preview_key == key && source.preview.is_some() {
        return;
    }
    if source.band >= source.cube.bands {
        source.band = 0;
    }
    let image = band_preview_image(&source.cube, source.band);
    source.preview =
        Some(ctx.load_texture("img_display_preview", image, egui::TextureOptions::NEAREST));
    source.preview_key = key;
}

fn band_preview_image(
    cube: &xenovision_core::hyperspectral::HyperspectralCube,
    band: usize,
) -> egui::ColorImage {
    let slice = cube.band_slice(band);
    let (min, max) = slice
        .iter()
        .fold((f32::MAX, f32::MIN), |(mn, mx), &v| (mn.min(v), mx.max(v)));
    let range = (max - min).max(1e-6);
    let pixels: Vec<egui::Color32> = slice
        .iter()
        .map(|&v| {
            let t = ((v - min) / range).clamp(0.0, 1.0);
            Colormap::Grayscale.apply(t)
        })
        .collect();
    egui::ColorImage {
        size: [cube.samples, cube.lines],
        pixels,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_view(
    ui: &mut egui::Ui,
    index: usize,
    view: &mut ImageView,
    lib: &ImageLibrary,
    sets: &HashMap<CurveSetId, AppCurveSet>,
    luminants: &HashMap<CurveId, SpectralCurve>,
    remove: &mut Option<usize>,
    duplicate: &mut Option<usize>,
) {
    ui.push_id(view.id, |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            // --- header ---
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut view.title).desired_width(180.0));
                if view.pending.is_some() {
                    ui.spinner();
                    ui.label("rendering…").on_hover_text(
                        "Rendering in the background; the previous result stays until it's ready.",
                    );
                }
                if ui.small_button("Duplicate").clicked() {
                    *duplicate = Some(index);
                }
                if ui.small_button("✕").on_hover_text("Remove view").clicked() {
                    *remove = Some(index);
                }
            });

            settings_row(ui, view, lib, sets, luminants);

            provenance_bar(ui, view, lib, sets, luminants);

            ui.separator();

            match &view.render {
                Some(rendered) => view_images(ui, view, rendered),
                None => {
                    if view.pending.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Rendering…");
                        });
                    } else {
                        let msg = view.error.as_deref().unwrap_or("Nothing to render yet.");
                        ui.colored_label(egui::Color32::from_rgb(220, 180, 40), msg);
                    }
                }
            }
        });
    });
}

fn settings_row(
    ui: &mut egui::Ui,
    view: &mut ImageView,
    lib: &ImageLibrary,
    sets: &HashMap<CurveSetId, AppCurveSet>,
    luminants: &HashMap<CurveId, SpectralCurve>,
) {
    egui::CollapsingHeader::new("Settings")
        .default_open(false)
        .show(ui, |ui| {
            // Source image.
            ui.horizontal(|ui| {
                ui.label("Image:");
                let current = view
                    .source
                    .and_then(|id| lib.get(id))
                    .map(|s| s.label.clone())
                    .unwrap_or_else(|| "(none)".to_string());
                egui::ComboBox::from_id_salt(("view_source", view.id))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for source in &lib.sources {
                            if ui
                                .selectable_label(view.source == Some(source.id), &source.label)
                                .clicked()
                                && view.source != Some(source.id)
                            {
                                view.source = Some(source.id);
                                view.region_choice = RegionChoice::Whole;
                                view.region = None;
                                view.draft.clear();
                            }
                        }
                    });
            });

            // Visual system.
            ui.horizontal(|ui| {
                ui.label("Visual system:");
                let current = view
                    .set_id
                    .and_then(|id| sets.get(&id))
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "(none)".to_string());
                egui::ComboBox::from_id_salt(("view_set", view.id))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        let mut ordered: Vec<&AppCurveSet> = sets.values().collect();
                        ordered.sort_by(|a, b| a.name.cmp(&b.name));
                        for set in ordered {
                            if ui
                                .selectable_label(view.set_id == Some(set.id), &set.name)
                                .clicked()
                            {
                                view.set_id = Some(set.id);
                                view.regime = None;
                                if set.regimes.is_empty() {
                                    // A regime-less pool has no adapted or
                                    // perceptual stages; fall back to raw,
                                    // one panel per receptor.
                                    view.stage = ChannelStage::RawReceptor;
                                    view.display = DisplayMode::Channels;
                                }
                            }
                        }
                    });
            });

            // Regime pills (only for a set that has regimes).
            let regimes: Vec<(usize, String)> = view
                .set_id
                .and_then(|id| sets.get(&id))
                .map(|s| {
                    s.regimes
                        .iter()
                        .enumerate()
                        .map(|(i, r)| (i, r.name.clone()))
                        .collect()
                })
                .unwrap_or_default();
            if !regimes.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Regime:");
                    if ui
                        .selectable_label(view.regime.is_none(), "dominant")
                        .clicked()
                    {
                        view.regime = None;
                    }
                    for (i, name) in &regimes {
                        if ui.selectable_label(view.regime == Some(*i), name).clicked() {
                            view.regime = Some(*i);
                        }
                    }
                });
            }

            // Stage.
            let regime_less = view
                .set_id
                .and_then(|id| sets.get(&id))
                .map(|s| s.regimes.is_empty())
                .unwrap_or(true);
            ui.horizontal(|ui| {
                ui.label("Stage:");
                ui.add_enabled_ui(!regime_less, |ui| {
                    if ui
                        .selectable_label(view.stage == ChannelStage::RawReceptor, "raw")
                        .clicked()
                    {
                        apply_stage(view, ChannelStage::RawReceptor);
                    }
                    if ui
                        .selectable_label(
                            view.stage == ChannelStage::AdaptedReceptor,
                            "von Kries adapted",
                        )
                        .clicked()
                    {
                        apply_stage(view, ChannelStage::AdaptedReceptor);
                    }
                    if ui
                        .selectable_label(view.stage == ChannelStage::Perceptual, "perceptual")
                        .clicked()
                    {
                        apply_stage(view, ChannelStage::Perceptual);
                    }
                });
            });
            if regime_less {
                ui.label("(This species has no regimes — raw receptor pool only.)");
            }

            // Input kind.
            ui.horizontal(|ui| {
                ui.label("Treat pixels as:");
                ui.radio_value(&mut view.input_kind, InputKind::Reflectance, "Reflectance");
                ui.radio_value(&mut view.input_kind, InputKind::Radiance, "Radiance");
            });

            // Luminant.
            ui.horizontal(|ui| {
                ui.label("Luminant:");
                let current = view
                    .luminant
                    .and_then(|id| luminants.get(&id))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| "(none)".to_string());
                egui::ComboBox::from_id_salt(("view_lum", view.id))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        let mut ordered: Vec<(&CurveId, &SpectralCurve)> =
                            luminants.iter().collect();
                        ordered.sort_by(|a, b| a.1.name.cmp(&b.1.name));
                        for (&id, curve) in ordered {
                            if ui
                                .selectable_label(view.luminant == Some(id), &curve.name)
                                .clicked()
                            {
                                view.luminant = Some(id);
                            }
                        }
                    });
            });

            // Region.
            region_settings(ui, view, lib);

            // Sampling step.
            ui.horizontal(|ui| {
                ui.label("Integration step:");
                ui.add(
                    egui::DragValue::new(&mut view.step_nm)
                        .range(0.1..=20.0)
                        .speed(0.1)
                        .suffix(" nm"),
                );
            });

            // Display mapping.
            display_settings(ui, view);
        });
}

fn region_settings(ui: &mut egui::Ui, view: &mut ImageView, lib: &ImageLibrary) {
    let Some(source) = view.source.and_then(|id| lib.get(id)) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Region:");
        let text = match view.region_choice {
            RegionChoice::Whole => "whole image".to_string(),
            RegionChoice::Saved(i) => source
                .regions
                .get(i)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "saved region".to_string()),
            RegionChoice::Custom => "custom".to_string(),
        };
        egui::ComboBox::from_id_salt(("view_region", view.id))
            .selected_text(text)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(view.region_choice == RegionChoice::Whole, "whole image")
                    .clicked()
                {
                    view.region_choice = RegionChoice::Whole;
                    view.region = None;
                }
                for (i, named) in source.regions.iter().enumerate() {
                    if ui
                        .selectable_label(view.region_choice == RegionChoice::Saved(i), &named.name)
                        .clicked()
                    {
                        view.region_choice = RegionChoice::Saved(i);
                        view.region = Some(named.region.clone());
                    }
                }
                if ui
                    .selectable_label(
                        view.region_choice == RegionChoice::Custom,
                        "custom (draw below)",
                    )
                    .clicked()
                {
                    view.region_choice = RegionChoice::Custom;
                    view.region = view.draft.region.clone();
                }
            });
    });

    if view.region_choice == RegionChoice::Custom {
        if let Some(preview) = &source.preview {
            let before = view.draft.region.clone();
            region_selector(
                ui,
                preview,
                source.cube.samples,
                source.cube.lines,
                &mut view.draft,
            );
            if !regions_equal(&before, &view.draft.region) {
                view.region = view.draft.region.clone();
            }
        } else if source.regions.is_empty() {
            ui.label("No saved regions. Draw one here, or save one in the Stimulus Editor.");
        }
    }
}

fn regions_equal(a: &Option<Region>, b: &Option<Region>) -> bool {
    region_sig(a) == region_sig(b)
}

fn display_settings(ui: &mut egui::Ui, view: &mut ImageView) {
    ui.horizontal(|ui| {
        ui.label("Show:");
        ui.selectable_value(
            &mut view.display,
            DisplayMode::Channels,
            "One panel per channel",
        );
        ui.selectable_value(&mut view.display, DisplayMode::Composite, "Composite image");
    });
    match view.display {
        DisplayMode::Channels => {
            ui.horizontal(|ui| {
                ui.label("Colormap:");
                for cm in Colormap::ALL {
                    ui.selectable_value(&mut view.colormap, cm, cm.name());
                }
            });
        }
        DisplayMode::Composite => {
            ui.horizontal(|ui| {
                ui.label("Composite:");
                // The perceptual composite encodes [L, C_a, C_b], which only
                // the perceptual stage produces; the R/G/B composite works on
                // any channels, so it is always offered.
                if view.stage == ChannelStage::Perceptual {
                    let is_perceptual = matches!(view.composite, Composite::Perceptual { .. });
                    if ui
                        .selectable_label(is_perceptual, "perceptual (L, chroma)")
                        .clicked()
                    {
                        view.composite = Composite::PERCEPTUAL;
                    }
                }
                let is_rgb = matches!(view.composite, Composite::Rgb { .. });
                if ui.selectable_label(is_rgb, "channels→RGB").clicked() && !is_rgb {
                    view.composite = Composite::Rgb { r: 0, g: 1, b: 2 };
                }
            });
            if let Composite::Perceptual { chroma_a, chroma_b } = view.composite {
                // The number of chroma axes comes from the finished render
                // (every stage of a regime has the same channel count, so
                // `N-1` chroma axes for `N` channels). Until it lands, the
                // defaults `C1`/`C2` apply.
                let n_channels = view
                    .render
                    .as_ref()
                    .map(|r| r.image.channels.len())
                    .unwrap_or(0);
                if n_channels >= 3 {
                    let n_chroma = n_channels - 1;
                    ui.horizontal(|ui| {
                        ui.label("Chroma axes:");
                        let a = chroma_axis_combo(ui, "green/magenta", chroma_a, n_chroma);
                        let b = chroma_axis_combo(ui, "blue/yellow", chroma_b, n_chroma);
                        view.composite = Composite::Perceptual {
                            chroma_a: a,
                            chroma_b: b,
                        };
                    });
                }
            }
            if let Composite::Rgb { r, g, b } = &mut view.composite {
                ui.horizontal(|ui| {
                    channel_index_picker(ui, "R", r);
                    channel_index_picker(ui, "G", g);
                    channel_index_picker(ui, "B", b);
                });
            }
        }
    }
}

/// A dropdown over a regime's chroma axes (`C1..C{n_chroma}`). Returns the
/// selected axis (clamped into range) for the perceptual composite to encode.
fn chroma_axis_combo(ui: &mut egui::Ui, role: &str, value: usize, n_chroma: usize) -> usize {
    let mut chosen = value.clamp(1, n_chroma);
    ui.label(role);
    egui::ComboBox::from_id_salt(("chroma_axis", role))
        .selected_text(format!("C{chosen}"))
        .show_ui(ui, |ui| {
            for c in 1..=n_chroma {
                ui.selectable_value(&mut chosen, c, format!("C{c}"));
            }
        });
    chosen
}

fn channel_index_picker(ui: &mut egui::Ui, label: &str, value: &mut usize) {
    ui.label(label);
    ui.add(egui::DragValue::new(value).range(0..=15).speed(1));
}

fn provenance_bar(
    ui: &mut egui::Ui,
    view: &ImageView,
    lib: &ImageLibrary,
    sets: &HashMap<CurveSetId, AppCurveSet>,
    luminants: &HashMap<CurveId, SpectralCurve>,
) {
    let source = view.source.and_then(|id| lib.get(id));
    let source_text = source
        .map(image_library::source_label)
        .unwrap_or_else(|| "(no image)".to_string());
    let source_resp = ui.label(egui::RichText::new(source_text).strong());
    if let Some(source) = source {
        source_resp.on_hover_text(format!("File: {}", source.path));
    }
    let region_text = match &view.region {
        None => "whole image".to_string(),
        Some(r) => region_label(r),
    };
    let kind_text = match view.input_kind {
        InputKind::Reflectance => "reflectance".to_string(),
        InputKind::Radiance => "radiance".to_string(),
    };
    let lum_text = view
        .luminant
        .and_then(|id| luminants.get(&id))
        .map(|c| c.name.clone());

    let set_name = view
        .set_id
        .and_then(|id| sets.get(&id))
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "(no visual system)".to_string());
    let regime_text = view
        .render
        .as_ref()
        .and_then(|r| r.image.regime_name.clone())
        .map(|n| format!("Regime: {n}"))
        .unwrap_or_else(|| "receptor pool (no regime)".to_string());
    let used_gpu = view.render.as_ref().map(|r| r.used_gpu).unwrap_or(false);
    let renderer = if used_gpu { "GPU" } else { "CPU" };
    // Report the stage actually rendered (a regime-less species is forced
    // to raw even if the view's stage control says otherwise).
    let effective_stage = view
        .render
        .as_ref()
        .map(|r| r.image.stage)
        .unwrap_or(view.stage);

    ui.label(format!("Region: {region_text}"));
    match lum_text {
        Some(l) => ui.label(format!("Input: {kind_text} · Luminant: {l}")),
        None => ui.label(format!("Input: {kind_text}")),
    };
    ui.label(format!("Species: {set_name} · {regime_text}"));
    ui.label(format!(
        "Stage: {} · step {:.1} nm · {renderer}",
        stage_label(effective_stage),
        view.step_nm
    ));
}

fn view_images(ui: &mut egui::Ui, view: &ImageView, rendered: &RenderedView) {
    let image = &rendered.image;
    match view.display {
        DisplayMode::Channels => {
            let n = rendered.textures.len().max(1);
            let avail = ui.available_width();
            let panel_w = (avail / n as f32).clamp(80.0, 320.0);
            ui.horizontal_wrapped(|ui| {
                for (c, tex) in rendered.textures.iter().enumerate() {
                    let label = image
                        .channels
                        .get(c)
                        .cloned()
                        .unwrap_or_else(|| format!("channel {c}"));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&label).strong());
                        let size = panel_size(image.width, image.height, panel_w);
                        let resp = draw_texture(ui, tex, size);
                        pixel_readout(ui, image, &resp);
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.label(channel_legend(view, image));
                draw_colormap_gradient(ui, view.colormap);
            });
        }
        DisplayMode::Composite => {
            let max_w = ui.available_width().min(720.0);
            let size = panel_size(image.width, image.height, max_w);
            let resp = draw_texture(ui, &rendered.textures[0], size);
            pixel_readout(ui, image, &resp);
            ui.label(composite_legend(view, image));
        }
    }
}

fn panel_size(w: usize, h: usize, max_w: f32) -> egui::Vec2 {
    let aspect = if w == 0 { 1.0 } else { h as f32 / w as f32 };
    let width = max_w.max(32.0);
    egui::vec2(width, (width * aspect).max(16.0))
}

fn draw_texture(ui: &mut egui::Ui, tex: &egui::TextureHandle, size: egui::Vec2) -> egui::Response {
    let (response, painter) = ui.allocate_painter(size, egui::Sense::hover());
    painter.image(
        tex.id(),
        response.rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    response
}

/// Shows the hovered pixel's channel values under a rendered image.
fn pixel_readout(ui: &mut egui::Ui, image: &ReceptorImage, response: &egui::Response) {
    let Some(pos) = response.hover_pos() else {
        return;
    };
    if !response.rect.contains(pos) {
        return;
    }
    let rel_x = (pos.x - response.rect.min.x) / response.rect.width().max(1.0);
    let rel_y = (pos.y - response.rect.min.y) / response.rect.height().max(1.0);
    let x = (rel_x * image.width as f32).clamp(0.0, image.width as f32 - 1.0) as usize;
    let y = (rel_y * image.height as f32).clamp(0.0, image.height as f32 - 1.0) as usize;
    let values: Vec<String> = image
        .pixel(x, y)
        .zip(image.channels.iter())
        .map(|(v, name)| format!("{name}={v:.4}"))
        .collect();
    ui.label(format!("({x},{y}) {}", values.join("  ")));
}

/// Paints a small low→high swatch of a colormap next to its legend.
fn draw_colormap_gradient(ui: &mut egui::Ui, colormap: Colormap) {
    let stops = colormap.gradient(24);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 12.0), egui::Sense::hover());
    let painter = ui.painter();
    let n = stops.len() as f32;
    for (i, c) in stops.iter().enumerate() {
        let x0 = rect.left() + rect.width() * (i as f32 / n);
        let x1 = rect.left() + rect.width() * ((i + 1) as f32 / n);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            *c,
        );
    }
}

fn channel_legend(view: &ImageView, image: &ReceptorImage) -> String {
    let names = image.channels.join(", ");
    format!(
        "Each panel: one receptor channel [{names}] · {} ramp · per-channel min–max",
        view.colormap.name()
    )
}

fn composite_legend(view: &ImageView, image: &ReceptorImage) -> String {
    match view.composite {
        Composite::Perceptual { chroma_a, chroma_b } => {
            // Match `composite_color_image`'s clamping so the legend always
            // names the axes actually encoded.
            let n_chroma = image.channels.len().saturating_sub(1);
            let (a, b) = if n_chroma >= 1 {
                (chroma_a.clamp(1, n_chroma), chroma_b.clamp(1, n_chroma))
            } else {
                (chroma_a, chroma_b)
            };
            format!(
                "Composite: L→lightness, C{a}→green/magenta, C{b}→blue/yellow · per-channel range"
            )
        }
        Composite::Rgb { r, g, b } => {
            let name = |c: usize| {
                image
                    .channels
                    .get(c)
                    .cloned()
                    .unwrap_or_else(|| format!("ch{c}"))
            };
            format!(
                "Composite: {}→R, {}→G, {}→B · per-channel min–max",
                name(r),
                name(g),
                name(b)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the worker handshake (`render_pass` + a short poll) until the
    /// view at `idx` has settled, so tests exercise the real off-thread
    /// path rather than a synchronous shortcut. A dispatch always leaves the
    /// view pending, so `pending == None` with a result means it is settled -
    /// this is what makes a *re*-render (old result still on screen) wait
    /// too, rather than returning on the stale result.
    fn render_now(app: &mut AppState, ctx: &egui::Context, panel: PanelId, idx: usize) {
        for _ in 0..10_000 {
            render_pass(ctx, app, panel);
            let view = &app.image_displays[&panel].views[idx];
            if view.pending.is_none() && (view.render.is_some() || view.error.is_some()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("render did not complete in time");
    }

    #[test]
    fn region_signature_distinguishes_shapes() {
        let whole = region_sig(&None);
        let rect = region_sig(&Some(Region::Rectangle {
            x0: 0.0,
            y0: 0.0,
            x1: 4.0,
            y1: 4.0,
        }));
        let poly = region_sig(&Some(Region::Polygon(vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
        ])));
        assert_ne!(whole, rect);
        assert_ne!(rect, poly);
    }

    #[test]
    fn stage_labels_are_human_readable() {
        assert_eq!(stage_label(ChannelStage::RawReceptor), "raw receptor");
        assert_eq!(
            stage_label(ChannelStage::AdaptedReceptor),
            "von Kries adapted"
        );
        assert_eq!(stage_label(ChannelStage::Perceptual), "perceptual");
    }

    /// End-to-end: a synthetic cube flows through the library into a view,
    /// is rendered off-thread, and comes back with one texture per channel.
    #[test]
    fn renders_a_synthetic_cube_end_to_end() {
        use crate::image_library::ImageSource;
        use xenovision_core::hyperspectral::HyperspectralCube;

        let mut app = crate::state::AppState::new();
        let (w, h, b) = (4usize, 3usize, 3usize);
        let cube = HyperspectralCube::from_band_major(
            w,
            h,
            b,
            Some(vec![450.0, 550.0, 650.0]),
            vec![0.5f32; w * h * b],
            "synthetic",
        );
        let id = app
            .image_library
            .add(ImageSource::new("synthetic", "synthetic", cube));
        let panel = app.new_image_display(false);
        let idx = seed_view(&mut app, panel, Some(id), None);
        // Raw receptor stage: one panel per channel.
        app.image_displays.get_mut(&panel).unwrap().views[idx].stage = ChannelStage::RawReceptor;
        app.image_displays.get_mut(&panel).unwrap().views[idx].display = DisplayMode::Channels;

        let ctx = egui::Context::default();
        render_now(&mut app, &ctx, panel, idx);

        let view = &app.image_displays[&panel].views[idx];
        assert!(view.error.is_none(), "render error: {:?}", view.error);
        let rendered = view.render.as_ref().expect("a render exists");
        assert!(!rendered.image.channels.is_empty(), "no channels rendered");
        assert_eq!(rendered.textures.len(), rendered.image.channels.len());
        assert_eq!(rendered.image.width, w);
        assert_eq!(rendered.image.height, h);
        assert!(
            view.pending.is_none(),
            "the render should no longer be pending"
        );
    }

    /// A changed view hands its render to the worker (marked pending) rather
    /// than rendering inline - the offload is observable, not a synchronous
    /// shortcut.
    #[test]
    fn render_is_dispatched_to_the_worker() {
        use crate::image_library::ImageSource;
        use xenovision_core::hyperspectral::HyperspectralCube;

        let mut app = crate::state::AppState::new();
        let cube = HyperspectralCube::from_band_major(
            8,
            8,
            3,
            Some(vec![450.0, 550.0, 650.0]),
            vec![0.5f32; 8 * 8 * 3],
            "synthetic",
        );
        let id = app
            .image_library
            .add(ImageSource::new("synthetic", "synthetic", cube));
        let panel = app.new_image_display(false);
        let idx = seed_view(&mut app, panel, Some(id), None);

        let ctx = egui::Context::default();
        render_pass(&ctx, &mut app, panel);
        // The frame that first sees changed inputs must submit to the worker
        // and mark the view pending; the result arrives on a later frame.
        assert!(
            app.image_displays[&panel].views[idx].pending.is_some(),
            "dispatch should mark the view pending"
        );
        assert!(app.image_displays[&panel].views[idx].render.is_none());

        render_now(&mut app, &ctx, panel, idx);
        assert!(app.image_displays[&panel].views[idx].render.is_some());
    }

    /// Changing an input invalidates the cache and re-renders; leaving the
    /// view alone does not.
    #[test]
    fn cache_reuses_until_inputs_change() {
        use crate::image_library::ImageSource;
        use xenovision_core::hyperspectral::HyperspectralCube;

        let mut app = crate::state::AppState::new();
        let cube = HyperspectralCube::from_band_major(
            2,
            2,
            3,
            Some(vec![450.0, 550.0, 650.0]),
            vec![0.5f32; 12],
            "synthetic",
        );
        let id = app
            .image_library
            .add(ImageSource::new("synthetic", "synthetic", cube));
        let panel = app.new_image_display(false);
        let idx = seed_view(&mut app, panel, Some(id), None);
        let ctx = egui::Context::default();

        render_now(&mut app, &ctx, panel, idx);
        let key1 = app.image_displays[&panel].views[idx].cache_key.clone();
        assert!(key1.is_some());

        // Same inputs: pumping again is a no-op (cache hit, nothing pending).
        render_pass(&ctx, &mut app, panel);
        assert_eq!(app.image_displays[&panel].views[idx].cache_key, key1);
        assert!(app.image_displays[&panel].views[idx].pending.is_none());

        // Flipping the input interpretation changes the key.
        app.image_displays.get_mut(&panel).unwrap().views[idx].input_kind = InputKind::Radiance;
        render_now(&mut app, &ctx, panel, idx);
        assert_ne!(app.image_displays[&panel].views[idx].cache_key, key1);
    }

    /// An outcome whose key no longer matches the view's pending key is
    /// dropped, so a slow render for superseded inputs can never clobber a
    /// newer state. (One worker is FIFO, so "out of order" here means
    /// "arrived after the inputs changed again", which is what matters.)
    #[test]
    fn stale_outcomes_are_dropped() {
        let ctx = egui::Context::default();
        let mut view = ImageView::new(1, None);
        view.pending = Some("current".to_string());
        let mut views = vec![view];

        // Stale key: ignored entirely - no result, no error, still pending.
        install_outcome(
            &ctx,
            RenderOutcome {
                view_id: 1,
                key: "stale".to_string(),
                result: Err("boom".to_string()),
            },
            &mut views,
        );
        assert_eq!(views[0].pending.as_deref(), Some("current"));
        assert!(views[0].error.is_none());
        assert!(views[0].render.is_none());

        // Matching key: installed.
        install_outcome(
            &ctx,
            RenderOutcome {
                view_id: 1,
                key: "current".to_string(),
                result: Err("boom".to_string()),
            },
            &mut views,
        );
        assert!(views[0].pending.is_none());
        assert_eq!(views[0].error.as_deref(), Some("boom"));
    }

    /// An outcome for a view removed while it was rendering is dropped.
    #[test]
    fn outcomes_for_removed_views_are_dropped() {
        let ctx = egui::Context::default();
        let mut views: Vec<ImageView> = Vec::new();
        install_outcome(
            &ctx,
            RenderOutcome {
                view_id: 42,
                key: "x".to_string(),
                result: Err("gone".to_string()),
            },
            &mut views,
        );
        assert!(views.is_empty());
    }

    /// Stage-aware defaults mirror the physics: raw and adapted stages each
    /// produce one channel per receptor (one panel per channel); the
    /// perceptual stage shows its `[L, C1, C2]` composite.
    #[test]
    fn stage_defaults_match_the_physics() {
        let mut view = ImageView::new(1, None);
        assert_eq!(view.stage, ChannelStage::RawReceptor);
        assert_eq!(view.display, DisplayMode::Channels);

        apply_stage(&mut view, ChannelStage::AdaptedReceptor);
        assert_eq!(view.stage, ChannelStage::AdaptedReceptor);
        assert_eq!(
            view.display,
            DisplayMode::Channels,
            "adapted is one panel per receptor"
        );

        apply_stage(&mut view, ChannelStage::Perceptual);
        assert_eq!(view.display, DisplayMode::Composite);
        assert_eq!(view.composite, Composite::PERCEPTUAL);

        // Returning to raw goes back to one panel per channel.
        apply_stage(&mut view, ChannelStage::RawReceptor);
        assert_eq!(view.display, DisplayMode::Channels);
    }

    /// The perceptual composite only encodes the perceptual stage's
    /// `[L, C1, C2]`; off-stage (here the default raw stage) it degrades to an
    /// explicit channel→R/G/B composite so the image can't be mislabelled.
    #[test]
    fn perceptual_composite_falls_back_to_rgb_off_stage() {
        use crate::image_library::ImageSource;
        use xenovision_core::hyperspectral::HyperspectralCube;

        let mut app = crate::state::AppState::new();
        let cube = HyperspectralCube::from_band_major(
            2,
            2,
            3,
            Some(vec![450.0, 550.0, 650.0]),
            vec![0.5f32; 12],
            "synthetic",
        );
        let id = app
            .image_library
            .add(ImageSource::new("synthetic", "synthetic", cube));
        let panel = app.new_image_display(false);
        let idx = seed_view(&mut app, panel, Some(id), None);
        {
            let view = &mut app.image_displays.get_mut(&panel).unwrap().views[idx];
            view.stage = ChannelStage::RawReceptor;
            view.display = DisplayMode::Composite;
            view.composite = Composite::PERCEPTUAL;
        }

        let ctx = egui::Context::default();
        render_pass(&ctx, &mut app, panel);

        assert_eq!(
            app.image_displays[&panel].views[idx].composite,
            Composite::Rgb { r: 0, g: 1, b: 2 },
            "off-stage perceptual composite should degrade to channel→RGB"
        );
    }

    /// Selecting different chroma axes is a display-only change: it must feed
    /// the texture mapping, yet it must *not* invalidate the render key (which
    /// would force a full re-render of a potentially huge cube just to change
    /// a color assignment).
    #[test]
    fn chroma_selection_is_display_only() {
        use crate::image_library::ImageSource;
        use xenovision_core::hyperspectral::HyperspectralCube;

        let source = ImageSource::new(
            "synthetic",
            "synthetic",
            HyperspectralCube::from_band_major(
                1,
                1,
                3,
                Some(vec![450.0, 550.0, 650.0]),
                vec![0.5f32; 3],
                "synthetic",
            ),
        );

        let mut view = ImageView::new(1, Some(source.id));
        let base_key = view_key(&view, &source, 0);
        view.composite = Composite::PERCEPTUAL;
        let base_mapping = mapping(&view);

        // A different chroma pair changes the mapping ...
        view.composite = Composite::Perceptual {
            chroma_a: 1,
            chroma_b: 3,
        };
        assert_ne!(
            mapping(&view),
            base_mapping,
            "the chosen chroma axes must feed the textures"
        );
        // ... but not the render key: the channel data is unchanged.
        assert_eq!(
            view_key(&view, &source, 0),
            base_key,
            "a display-only change must not force a re-render"
        );

        // A change that does alter the pixels still invalidates the key.
        view.stage = ChannelStage::Perceptual;
        assert_ne!(view_key(&view, &source, 0), base_key);
    }

    /// The whole point of the repeatable panel: two Image Display windows keep
    /// independent view lists over the one shared library, and closing one
    /// discards only its own state.
    #[test]
    fn separate_panels_keep_separate_views() {
        use crate::image_library::ImageSource;
        use xenovision_core::hyperspectral::HyperspectralCube;

        let mut app = crate::state::AppState::new();
        let cube = HyperspectralCube::from_band_major(
            2,
            2,
            3,
            Some(vec![450.0, 550.0, 650.0]),
            vec![0.5f32; 12],
            "synthetic",
        );
        let id = app
            .image_library
            .add(ImageSource::new("synthetic", "synthetic", cube));

        let a = app.new_image_display(false);
        let b = app.new_image_display(true);
        assert_ne!(a, b, "each new panel gets its own id");
        seed_view(&mut app, a, Some(id), None);
        seed_view(&mut app, b, Some(id), None);
        seed_view(&mut app, b, None, None);

        assert_eq!(app.image_displays[&a].views.len(), 1);
        assert_eq!(app.image_displays[&b].views.len(), 2);
        // The library is shared, not duplicated per panel.
        assert_eq!(app.image_library.sources.len(), 1);

        // Closing b drops only b's state (and its render worker).
        app.dispatch_dock_action(DockAction::CloseInstance(PanelInstance::Panel(b)));
        assert!(app.image_displays.contains_key(&a));
        assert!(!app.image_displays.contains_key(&b));
        assert_eq!(app.image_displays[&a].views.len(), 1);
    }

    /// Closing the last Image Display window parks its views; the next window
    /// inherits them (last closed wins), while a window opened alongside an
    /// existing one starts blank.
    #[test]
    fn last_closed_image_display_setup_is_restored() {
        use crate::image_library::ImageSource;
        use xenovision_core::hyperspectral::HyperspectralCube;

        let mut app = crate::state::AppState::new();
        let cube = HyperspectralCube::from_band_major(
            2,
            2,
            3,
            Some(vec![450.0, 550.0, 650.0]),
            vec![0.5f32; 12],
            "synthetic",
        );
        let src = app
            .image_library
            .add(ImageSource::new("synthetic", "synthetic", cube));

        let a = app.new_image_display(false);
        seed_view(&mut app, a, Some(src), None);
        seed_view(&mut app, a, None, None);
        assert_eq!(app.image_displays[&a].views.len(), 2);

        // A window opened while one is already open starts blank.
        let b = app.new_image_display(true);
        assert!(app.image_displays[&b].views.is_empty());

        // Close b, then a: a is now the last-closed, so the next window
        // inherits a's two views.
        app.dispatch_dock_action(DockAction::CloseInstance(PanelInstance::Panel(b)));
        app.dispatch_dock_action(DockAction::CloseInstance(PanelInstance::Panel(a)));
        let c = app.new_image_display(false);
        assert_eq!(
            app.image_displays[&c].views.len(),
            2,
            "the reopened window inherits the last-closed window's views"
        );
    }

    /// `primary_image_display` reuses an open panel if there is one (so the
    /// Stimulus Editor's "Show channels" button doesn't spawn a new window on
    /// every click) and creates one only when none is open.
    #[test]
    fn primary_image_display_reuses_then_creates() {
        let mut app = crate::state::AppState::new();
        let first = app.primary_image_display();
        assert_eq!(
            app.primary_image_display(),
            first,
            "an open panel is reused, not duplicated"
        );
        let second = app.new_image_display(true);
        assert_ne!(first, second);
        assert_eq!(
            app.primary_image_display(),
            first,
            "the first open panel stays primary"
        );
    }
}
