//! Shared application state for the windowed redesign
//! (`docs/design-doc.md`): one `AppState`, read and written by every
//! window (`window_workspace`, `window_comparison`,
//! `window_stimulus_editor`, `window_image_display`), with no per-window
//! copy of curve data (nor of loaded image cubes - those live in the
//! shared `ImageLibrary`).
//!
//! Deviations from §4's Rust sketches, pragmatic rather than exact:
//! - `Curve` is just `xenovision_core::SpectralCurve` directly (it
//!   already has every field §4.2's `Curve` sketch wants - name, points,
//!   curve_type, quantity, omega, eta, luminance_weight, metadata) -
//!   no separate wrapper type.
//! - `CurveSetId`/`CurveId` are plain incrementing `u64`s from one
//!   shared counter (`AppState::alloc_id`), not a `slotmap` - ids are
//!   never reused, so this behaves identically for stability purposes.
//! - Undo (§4.3) is implemented as whole-`CurveSet`-shaped snapshots
//!   (`EditableSnapshot`) via `undo::UndoManager`, generalized from this
//!   app's existing, already-tested gesture-detecting snapshot manager,
//!   rather than a hand-rolled `EditAction` command log - the design doc
//!   explicitly allows either, and reusing proven logic is lower-risk.
//! - `comparison`/`stimulus_editor` are always-present structs plus a
//!   `show_*` visibility bool, rather than `Option<...>` - behaviorally
//!   identical ("state persists while hidden") with less null-handling.
//! - `DistanceMetric` is reused directly from `xenovision_core::comparison`
//!   rather than redefined.
//!
//! Model shape (multi-regime §2.2): an `AppCurveSet` holds one receptor
//! pool (`curves` + `curve_order`) and a list of `AppRegime`, each a
//! colorspace over some pool indices (its own luminance/opponent
//! overrides). There is no "colorspace vs. isolated" split: a receptor
//! is either a member of at least one regime (in a colorspace) or of none
//! (reported individually, formerly "isolated"). `to_core`/`from_core`
//! reconcile stable `CurveId`s against the core's pool indices.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use xenovision_core::comparison::DistanceMetric;
use xenovision_core::curve_set::{OpponentContrast, Regime as CoreRegime};
use xenovision_core::pipeline::Pipeline;
use xenovision_core::{
    fixture_library, fixtures, CurveSet as CoreCurveSet, CurveType, QuantityKind, SpectralCurve,
};

use crate::dock::{DockAction, DockLayout, PanelId, PanelKind, Placement};
use crate::image_library::ImageLibrary;
use crate::plot_axis::AxisOrientation;
use crate::undo::UndoManager;
use crate::window_image_display::ImageDisplayState;
use crate::window_stimulus_editor::StimulusEditorState;

pub type CurveSetId = u64;
pub type CurveId = u64;

/// Everything the per-tab undo/redo stack (§4.3/§1.7) needs to restore -
/// every field a Workspace editing surface can touch, *except* `dirty`/
/// `revision`/`file_path`, which are bookkeeping, not user-visible data.
/// The whole receptor pool is captured in pool order, along with the set's
/// colorspace regimes (which own the opponent contrasts now, §2.2).
#[derive(Clone, PartialEq)]
pub struct EditableSnapshot {
    pub curves: Vec<(CurveId, SpectralCurve)>,
    pub regimes: Vec<AppRegime>,
    pub metadata: BTreeMap<String, String>,
    /// Set-level receptor operating-window defaults (§2.2) - part of the
    /// editable curve-set data, so undo/redo of a Workspace edit to them
    /// restores the window state too.
    pub illumination_min: Option<f64>,
    pub illumination_max: Option<f64>,
    pub illumination_softness: Option<f64>,
}

/// The app-side mirror of `xenovision_core::curve_set::Regime`: a named
/// colorspace over pool members, referenced by stable `CurveId`s (so a
/// tab/selection/Comparison-window reference survives an edit elsewhere
/// in the set). `to_core` maps the member ids onto pool indices.
#[derive(Debug, Clone, PartialEq)]
pub struct AppRegime {
    pub name: String,
    pub curves: Vec<CurveId>,
    pub luminance_weights: Option<Vec<f64>>,
    pub opponent_contrasts: Option<Vec<OpponentContrast>>,
}

impl AppRegime {
    pub fn new(name: impl Into<String>, curves: Vec<CurveId>) -> Self {
        AppRegime {
            name: name.into(),
            curves,
            luminance_weights: None,
            opponent_contrasts: None,
        }
    }
}

/// Maps an app regime (member `CurveId`s) to a core regime (pool
/// indices), dropping members whose id isn't in the pool (a reconcile
/// against a hand-edited file that deleted a curve).
fn core_regime(r: &AppRegime, order: &[CurveId]) -> CoreRegime {
    CoreRegime {
        name: r.name.clone(),
        curves: r
            .curves
            .iter()
            .filter_map(|id| order.iter().position(|o| o == id).map(|i| i as u64))
            .collect(),
        luminance_weights: r.luminance_weights.clone(),
        opponent_contrasts: r.opponent_contrasts.clone(),
    }
}

/// App-level Curve Set (design doc §4.2): like `xenovision_core::CurveSet`,
/// but curves are referenced by stable `CurveId` rather than held inline,
/// so a tab/selection/Comparison-window reference survives an edit
/// elsewhere in the set. `curve_order` is the receptor pool order (what
/// a regime's indices index into at the core level); `regimes` hold the
/// colorspaces.
pub struct AppCurveSet {
    pub id: CurveSetId,
    pub name: String,
    pub curve_order: Vec<CurveId>,
    pub curves: HashMap<CurveId, SpectralCurve>,
    pub regimes: Vec<AppRegime>,
    pub metadata: BTreeMap<String, String>,
    /// Set-level receptor operating-window defaults (§2.2): effective
    /// only for receptors whose own `illumination_min`/`max`/`softness`
    /// fields are unset. Units: ambient irradiance W/m² (min/max), and
    /// decimals-of-a-decade shoulder width (softness).
    pub illumination_min: Option<f64>,
    pub illumination_max: Option<f64>,
    pub illumination_softness: Option<f64>,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    /// Bumped on every edit affecting the receptor curves, regimes,
    /// opponent contrasts, or luminance weights - `TransformCache`'s
    /// invalidation key (§4.6).
    pub revision: u64,
}

impl AppCurveSet {
    pub fn snapshot(&self) -> EditableSnapshot {
        EditableSnapshot {
            curves: self
                .curve_order
                .iter()
                .map(|id| (*id, self.curves[id].clone()))
                .collect(),
            regimes: self.regimes.clone(),
            metadata: self.metadata.clone(),
            illumination_min: self.illumination_min,
            illumination_max: self.illumination_max,
            illumination_softness: self.illumination_softness,
        }
    }

    /// Restores every undo-tracked field from `snap`, preserving whatever
    /// `CurveId`s the snapshot itself carries (so undo/redo of a curve
    /// add/remove doesn't disturb the ids of curves that weren't touched).
    pub fn restore(&mut self, snap: &EditableSnapshot) {
        self.curve_order = snap.curves.iter().map(|(id, _)| *id).collect();
        self.curves = snap.curves.iter().cloned().collect();
        self.regimes = snap.regimes.clone();
        self.metadata = snap.metadata.clone();
        self.illumination_min = snap.illumination_min;
        self.illumination_max = snap.illumination_max;
        self.illumination_softness = snap.illumination_softness;
        self.revision += 1;
        self.dirty = true;
    }

    pub fn to_core(&self) -> CoreCurveSet {
        CoreCurveSet {
            name: self.name.clone(),
            curves: self
                .curve_order
                .iter()
                .map(|id| self.curves[id].clone())
                .collect(),
            metadata: self.metadata.clone(),
            regimes: self
                .regimes
                .iter()
                .map(|r| core_regime(r, &self.curve_order))
                .collect(),
            illumination_min: self.illumination_min,
            illumination_max: self.illumination_max,
            illumination_softness: self.illumination_softness,
        }
    }

    pub fn has_partial_eta_coverage(&self) -> bool {
        self.to_core().has_partial_eta_coverage()
    }

    /// `true` if any receptor or the set-level defaults declare a
    /// malformed operating window (non-positive bound, or `min >= max`) -
    /// calculation treats it as unbounded, and the Workspace warns (§2.2).
    pub fn has_invalid_illumination_window(&self) -> bool {
        self.to_core().has_invalid_illumination_window()
    }

    pub fn receptor_noise_complete(&self) -> bool {
        self.to_core().receptor_noise().is_some()
    }

    /// The integral-derived default luminance weight for the pool curve
    /// at `index` (i.e. what applies when that curve has no explicit
    /// `luminance_weight` override) - see
    /// `xenovision_core::CurveSet::luminance_weights` (pool-wide).
    pub fn luminance_weight_default(&self, index: usize) -> f64 {
        self.to_core()
            .luminance_weights(1.0)
            .get(index)
            .copied()
            .unwrap_or(0.0)
    }
}

fn from_core(core: CoreCurveSet, id: CurveSetId, next_id: &mut u64) -> AppCurveSet {
    let mut curves = HashMap::new();
    let mut curve_order: Vec<CurveId> = Vec::with_capacity(core.curves.len());
    for c in core.curves {
        let cid = *next_id;
        *next_id += 1;
        curve_order.push(cid);
        curves.insert(cid, c);
    }
    let regimes: Vec<AppRegime> = core
        .regimes
        .iter()
        .map(|r| {
            let mut member_ids = Vec::new();
            for &i in &r.curves {
                if let Some(&cid) = curve_order.get(i as usize) {
                    member_ids.push(cid);
                }
            }
            AppRegime {
                name: r.name.clone(),
                curves: member_ids,
                luminance_weights: r.luminance_weights.clone(),
                opponent_contrasts: r.opponent_contrasts.clone(),
            }
        })
        .collect();
    AppCurveSet {
        id,
        name: core.name,
        curve_order,
        curves,
        regimes,
        metadata: core.metadata,
        illumination_min: core.illumination_min,
        illumination_max: core.illumination_max,
        illumination_softness: core.illumination_softness,
        file_path: None,
        dirty: false,
        revision: 0,
    }
}

/// Which "natural scene" ensemble (§4.2.6) the opponent-contrast
/// auto-generate control uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NaturalSceneMode {
    #[default]
    Parametric,
    Corpus,
}

/// Transient (not persisted - like Comparison's `reference_luminant`)
/// per-tab state for the opponent-contrast "auto-generate from natural-
/// scene statistics" control (§4.2.6).
pub struct NaturalSceneUiState {
    pub mode: NaturalSceneMode,
    pub illuminant: Option<CurveId>,
    /// Parametric mode's smoothness-kernel correlation length (nm) - a
    /// tunable approximation, not a measured literature constant.
    pub correlation_length_nm: f64,
    /// Corpus mode's selection: presence in the map means selected,
    /// defaulting to weight 1.0 (see `stimulus_picker`'s corpus tree for
    /// how whole corpora/sub-corpora get selected at once).
    pub corpus_weights: HashMap<CurveId, f64>,
    /// Scratch value for the "apply this weight to every selected
    /// curve" control, kept here (not a per-frame local) so it doesn't
    /// reset to its default every frame.
    pub corpus_shared_weight: f64,
    pub status: String,
}

impl Default for NaturalSceneUiState {
    fn default() -> Self {
        NaturalSceneUiState {
            mode: NaturalSceneMode::default(),
            illuminant: None,
            correlation_length_nm: 50.0,
            corpus_weights: HashMap::new(),
            corpus_shared_weight: 1.0,
            status: String::new(),
        }
    }
}

/// Per-tab UI state (design doc §4.3): selection, undo/redo, and
/// transient editing-surface scratch buffers.
pub struct TabUiState {
    pub selected_curve: Option<CurveId>,
    pub selected_point: Option<usize>,
    /// Which regime's curves the center graph shows; `None` = the whole
    /// pooled receptor list. (Replaces the old colorspace/isolated
    /// tab switcher - with one pooled list there's nothing to toggle
    /// between; the filter is *which colorspace* to highlight.)
    pub graph_regime: Option<usize>,
    pub undo: UndoManager<EditableSnapshot>,
    pub noise_omega_buf: String,
    pub noise_eta_buf: String,
    pub noise_w_buf: String,
    pub sat_buf: String,
    pub sat_buf_for: Option<CurveId>,
    /// Operating-window (§2.2) per-curve editing buffers and the curve
    /// they currently show, or `None` when they need reloading.
    pub ill_min_buf: String,
    pub ill_max_buf: String,
    pub ill_soft_buf: String,
    pub ill_buf_for: Option<CurveId>,
    /// Set-level operating-window-default editing buffers and the set
    /// they currently show.
    pub set_ill_min_buf: String,
    pub set_ill_max_buf: String,
    pub set_ill_soft_buf: String,
    pub set_ill_buf_for: Option<CurveSetId>,
    pub noise_buf_for: Option<CurveId>,
    pub save_path_buf: String,
    pub csv_path_buf: String,
    pub status: String,
    pub natural_scene: NaturalSceneUiState,
}

impl Default for TabUiState {
    fn default() -> Self {
        TabUiState {
            selected_curve: None,
            selected_point: None,
            graph_regime: None,
            undo: UndoManager::default(),
            noise_omega_buf: String::new(),
            noise_eta_buf: String::new(),
            noise_w_buf: String::new(),
            sat_buf: String::new(),
            sat_buf_for: None,
            ill_min_buf: String::new(),
            ill_max_buf: String::new(),
            ill_soft_buf: String::new(),
            ill_buf_for: None,
            set_ill_min_buf: String::new(),
            set_ill_max_buf: String::new(),
            set_ill_soft_buf: String::new(),
            set_ill_buf_for: None,
            noise_buf_for: None,
            save_path_buf: "visual_system.json".to_string(),
            csv_path_buf: "curve.csv".to_string(),
            status: String::new(),
            natural_scene: NaturalSceneUiState::default(),
        }
    }
}

/// Workspace window state (design doc §4.3).
pub struct WorkspaceState {
    pub open_tabs: Vec<CurveSetId>,
    pub active_tab: usize,
    pub per_tab: HashMap<CurveSetId, TabUiState>,
    pub new_tab_name_buf: String,
    /// A tab the user clicked the close button on, awaiting an inline
    /// "unsaved changes" confirmation before it's actually removed -
    /// `None` means no close confirmation is in progress.
    pub pending_close: Option<CurveSetId>,
}

/// One entry in the shared stimulus-curve library (reflectance/
/// radiance/illuminance samples - never a visual system's own
/// Sensitivity curves, which live in `AppCurveSet` instead). Owns its
/// own save state, like `AppCurveSet` does, so "unload" can warn on
/// unsaved changes the same way closing a Workspace tab does.
pub struct StimulusEntry {
    pub curve: SpectralCurve,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
}

/// Comparison window state (design doc §4.4). Coordinate tables and
/// difference matrices are deliberately *not* stored here - they're
/// recomputed each frame (via `TransformCache`) from the fields below,
/// so there's no derived-data copy that could drift from the underlying
/// curves after a Workspace edit.
pub struct ComparisonState {
    /// Ids into `AppState::stimulus_curves` currently included in this
    /// comparison run - a subset of the shared library, not a copy of
    /// any curve data.
    pub selected_stimuli: Vec<CurveId>,
    pub selected_species: Vec<CurveSetId>,
    pub reference_luminant: Option<CurveId>,
    pub metric: DistanceMetric,
    pub active_species_subtab: usize,
    /// The regime each selected species' coordinate table currently
    /// shows, keyed by species set id. Pills above each species' table
    /// switch it (the dominant regime is marked); a multi-regime species
    /// like the frog shows one regime's table at a time. Absent entry =
    /// the dominant regime; regime-less species (mantis shrimp) display
    /// the no-colorspace report instead, and never get an entry.
    pub active_regime: HashMap<CurveSetId, usize>,
    /// Selected point index while editing the current reference
    /// luminant's curve directly in the Comparison window - kept here
    /// (not a throwaway per-frame local) so Delete-to-remove and the
    /// drag highlight work across frames like they do in Workspace.
    pub luminant_selected_point: Option<usize>,
    /// Pending "scale by" factor for the luminant power control.
    pub luminant_scale_factor: f64,
    /// Pending "set total power to" target for the same control.
    pub luminant_target_power: f64,
    /// Absorption curve (from the stimulus library) chosen to apply to
    /// the selected luminant, and the exponent weight to apply it with.
    pub absorption_to_apply: Option<CurveId>,
    pub absorption_weight: f64,
    /// Batch export (large-group-to-CSV) scratch state.
    pub export: BatchExportState,
}

/// Transient scratch state for exporting the coordinate table (and,
/// optionally, each stimulus's distance to one reference stimulus)
/// over every currently-`selected_stimuli` to a CSV file - the "large
/// group" counterpart to the live, interactive tables above, which
/// don't scale to thousands of rows in an egui table.
pub struct BatchExportState {
    pub path_buf: String,
    pub include_distance_to_reference: bool,
    pub reference_stimulus: Option<CurveId>,
    pub status: String,
}

impl Default for BatchExportState {
    fn default() -> Self {
        BatchExportState {
            path_buf: "comparison.csv".to_string(),
            include_distance_to_reference: false,
            reference_stimulus: None,
            status: String::new(),
        }
    }
}

/// Cross-window-shared cache (design doc §4.6): keyed by (species set,
/// reference luminant), invalidated when the set's `revision` no longer
/// matches what the cached pipelines were built from. `Pipeline` isn't
/// `Clone`, so cache hits share one `Rc` rather than re-deriving each
/// regime's adaptation matrix. The value is *all* of the set's regime
/// pipelines in regime order - empty for a regime-less species (mantis
/// shrimp, whose receptors are reported individually, §10.2).
#[derive(Default)]
pub struct TransformCache {
    cache: HashMap<(CurveSetId, CurveId), (Rc<Vec<Pipeline>>, u64)>,
}

impl TransformCache {
    pub fn get_or_build(
        &mut self,
        set: &AppCurveSet,
        luminant_id: CurveId,
        luminant: &SpectralCurve,
        step_nm: f64,
    ) -> Rc<Vec<Pipeline>> {
        if let Some((pipelines, rev)) = self.cache.get(&(set.id, luminant_id)) {
            if *rev == set.revision {
                return Rc::clone(pipelines);
            }
        }
        let core = set.to_core();
        let pipelines = Rc::new(
            core.regimes
                .iter()
                .map(|r| Pipeline::build(core.regime_config(r), luminant, step_nm))
                .collect::<Vec<_>>(),
        );
        self.cache
            .insert((set.id, luminant_id), (Rc::clone(&pipelines), set.revision));
        pipelines
    }

    /// Drops every cached pipeline built against `luminant_id`. A cache
    /// entry's key is `(species, luminant)`, but its invalidation above
    /// only checks the *species'* revision - editing a luminant's own
    /// curve in place changes neither the species' `CurveSet` nor its
    /// revision, so without this, an edited luminant's stale pipelines
    /// (built from its pre-edit curve) would keep being reused for
    /// anything that still matches on id. Call this whenever a
    /// luminant's curve content changes or it's removed.
    pub fn invalidate_luminant(&mut self, luminant_id: CurveId) {
        self.cache.retain(|&(_, iid), _| iid != luminant_id);
    }
}

/// Top-level application state (design doc §4.1), shared by all three
/// windows.
pub struct AppState {
    next_id: u64,
    pub curve_sets: HashMap<CurveSetId, AppCurveSet>,
    pub workspace: WorkspaceState,
    pub comparison: ComparisonState,
    /// The shared stimulus-curve library (reflectance/radiance/
    /// illuminance samples) - edited primarily from the Stimulus Editor
    /// window, but read by Comparison the same way `curve_sets` is.
    pub stimulus_curves: HashMap<CurveId, StimulusEntry>,
    pub stimulus_order: Vec<CurveId>,
    pub stimulus_editor: StimulusEditorState,
    /// Loaded image cubes, shared by the Stimulus Editor (curve extraction)
    /// and every Image Display panel (receptor renderings).
    pub image_library: ImageLibrary,
    /// One state per open Image Display panel, keyed by its [`PanelId`].
    /// Image Display is repeatable (`PanelKind::is_multi_instance`), so several
    /// independent panels - each with its own view list - can be open at once,
    /// all reading the one shared `image_library`.
    pub image_displays: HashMap<PanelId, ImageDisplayState>,
    /// Next instance number handed out to a repeatable panel (see
    /// `PanelId::instance`).
    next_panel_instance: u32,
    /// Lazily-initialized wgpu compute context for the receptor renderer,
    /// shared (`Arc`) with the background render worker; `gpu_attempted`
    /// records that a probe has run (so a machine without a usable adapter
    /// doesn't retry every frame).
    pub gpu: Option<Arc<xenovision_core::gpu::GpuContext>>,
    pub gpu_attempted: bool,
    /// Which panels are docked as tabs in the main window, floating in
    /// their own windows, or hidden.
    pub dock: DockLayout,
    /// Dock changes requested from inside a panel (e.g. Workspace's
    /// Window menu), applied by `main` after the frame's UI has run so
    /// the layout never changes mid-render.
    pub pending_dock_actions: Vec<DockAction>,
    pub transform_cache: TransformCache,
    pub luminants: HashMap<CurveId, SpectralCurve>,
    pub luminant_order: Vec<CurveId>,
    /// Global display preference for every wavelength axis in the app -
    /// see `plot_axis::AxisOrientation`'s docs for the full rationale.
    /// Toggled from the Workspace window's View menu (§2.1/§6).
    pub axis_orientation: AxisOrientation,
}

impl AppState {
    pub fn alloc_id(&mut self) -> CurveId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Reserves `count` consecutive ids at once, returning the first -
    /// for bulk id allocation (e.g. restoring a whole fixture's curves)
    /// where allocating one at a time while already holding a `&mut`
    /// borrow of `curve_sets` would fight the borrow checker.
    pub fn alloc_id_range(&mut self, count: usize) -> CurveId {
        let first = self.next_id;
        self.next_id += count as u64;
        first
    }

    /// Creates a fresh, empty Image Display panel and places it (docked or
    /// floating), returning its id.
    pub fn new_image_display(&mut self, floating: bool) -> PanelId {
        let id = PanelId::new(PanelKind::ImageDisplay, self.next_panel_instance);
        self.next_panel_instance += 1;
        self.image_displays.insert(id, ImageDisplayState::default());
        self.dispatch_dock_action(if floating {
            DockAction::OpenFloating(id)
        } else {
            DockAction::OpenDocked(id)
        });
        id
    }

    /// The Image Display panel a new view should go to: the first one already
    /// open, or a newly created docked one if none is open.
    pub fn primary_image_display(&mut self) -> PanelId {
        if let Some(id) = self
            .dock
            .panels_of_kind(PanelKind::ImageDisplay)
            .first()
            .copied()
        {
            return id;
        }
        self.new_image_display(false)
    }

    /// Applies a dock action and keeps per-panel state consistent with the
    /// layout: `NewInstance` allocates that panel's data, and closing a
    /// repeatable panel drops its state (releasing its render worker).
    pub fn dispatch_dock_action(&mut self, action: DockAction) {
        match action {
            DockAction::NewInstance(kind) => {
                debug_assert!(
                    kind.is_multi_instance(),
                    "only repeatable panels can open another instance"
                );
                // Today Image Display is the only repeatable kind.
                self.new_image_display(true);
            }
            other => self.dock.apply(other),
        }
        self.prune_detached_panels();
    }

    /// Handles a floating window's OS close button, then drops any panel state
    /// that leaves behind (a repeatable panel closed this way is discarded).
    pub fn dismiss_floating(&mut self, id: PanelId) {
        self.dock.floating_window_closed(id);
        self.prune_detached_panels();
    }

    /// Drops state for repeatable panels that are no longer docked or floating
    /// (i.e. were closed), so their render workers are released.
    fn prune_detached_panels(&mut self) {
        let layout = &self.dock;
        self.image_displays
            .retain(|id, _| layout.placement(*id) != Placement::Hidden);
    }

    pub fn new() -> Self {
        let mut next_id: u64 = 1;
        let mut curve_sets = HashMap::new();
        let mut open_tabs = Vec::new();
        let mut per_tab = HashMap::new();
        for core_set in fixture_library::load_library() {
            let id = next_id;
            next_id += 1;
            let app_set = from_core(core_set, id, &mut next_id);
            open_tabs.push(id);
            per_tab.insert(id, TabUiState::default());
            curve_sets.insert(id, app_set);
        }

        let mut luminants = HashMap::new();
        let mut luminant_order = Vec::new();
        // Not a "placeholder" - a perfectly flat spectrum is a
        // legitimate reference environment in its own right (no
        // wavelength-dependent bias at all), not just a stand-in for a
        // better luminant someone hasn't picked yet.
        let uniform_id = next_id;
        next_id += 1;
        luminants.insert(
            uniform_id,
            SpectralCurve::new("Uniform", CurveType::Illumination)
                .with_points(vec![(300.0, 1.0), (750.0, 1.0)])
                .with_quantity(QuantityKind::Radiance {
                    unit: "relative".to_string(),
                }),
        );
        luminant_order.push(uniform_id);
        let solar_id = next_id;
        next_id += 1;
        luminants.insert(solar_id, fixtures::default_solar_illuminant());
        luminant_order.push(solar_id);

        // Seeded example stimuli so the Comparison window has something
        // to work with immediately, rather than an empty library -
        // matches this app's pre-redesign default stimulus set.
        let mut stimulus_curves = HashMap::new();
        let mut stimulus_order = Vec::new();
        for curve in [
            SpectralCurve::new("Reddish", CurveType::Reflectance)
                .with_points(vec![(400.0, 0.2), (500.0, 0.2), (600.0, 0.8), (700.0, 0.8)])
                .with_quantity(QuantityKind::Reflectance),
            SpectralCurve::new("Greenish", CurveType::Reflectance)
                .with_points(vec![(400.0, 0.2), (530.0, 0.8), (600.0, 0.2), (700.0, 0.2)])
                .with_quantity(QuantityKind::Reflectance),
            SpectralCurve::new("Blueish", CurveType::Reflectance)
                .with_points(vec![(400.0, 0.8), (450.0, 0.8), (550.0, 0.2), (700.0, 0.2)])
                .with_quantity(QuantityKind::Reflectance),
        ] {
            let id = next_id;
            next_id += 1;
            stimulus_order.push(id);
            stimulus_curves.insert(
                id,
                StimulusEntry {
                    curve,
                    file_path: None,
                    dirty: false,
                },
            );
        }

        AppState {
            next_id,
            curve_sets,
            workspace: WorkspaceState {
                open_tabs,
                active_tab: 0,
                per_tab,
                new_tab_name_buf: String::new(),
                pending_close: None,
            },
            comparison: ComparisonState {
                selected_stimuli: Vec::new(),
                selected_species: Vec::new(),
                reference_luminant: Some(uniform_id),
                metric: DistanceMetric::Euclidean,
                active_species_subtab: 0,
                active_regime: HashMap::new(),
                luminant_selected_point: None,
                luminant_scale_factor: 1.0,
                luminant_target_power: 1.0,
                absorption_to_apply: None,
                absorption_weight: 1.0,
                export: BatchExportState::default(),
            },
            stimulus_curves,
            stimulus_order,
            stimulus_editor: StimulusEditorState::default(),
            image_library: ImageLibrary::default(),
            image_displays: HashMap::new(),
            next_panel_instance: 0,
            gpu: None,
            gpu_attempted: false,
            dock: DockLayout::default(),
            pending_dock_actions: Vec::new(),
            transform_cache: TransformCache::default(),
            luminants,
            luminant_order,
            axis_orientation: AxisOrientation::default(),
        }
    }
}
