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
//! - `comparison`/`stimulus_editor` are always-present structs rather than
//!   `Option<...>`: they hold the *parked* (most recently closed) state of
//!   each repeatable panel, which is what a new instance inherits when no other
//!   instance of that kind is open, and which other panels read as app-wide
//!   defaults. Every open instance - of Comparison, Stimulus Editor and Image
//!   Display alike - lives in a per-panel map keyed by `PanelId`, ordered into
//!   each kind's container by `AppState::repeatable_tabs`, and the window
//!   modules never need to know which instance they are drawing: the
//!   instance's state is swapped into the `comparison`/`stimulus_editor` field
//!   for the duration of that instance's draw (see
//!   `AppState::with_comparison`). Closing an instance *parks* its state (last
//!   closed wins) instead of discarding it; Workspace's Curve Set instances are
//!   the exception - closing one discards it (after an unsaved-changes prompt),
//!   since Curve Sets are shared data.
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

use crate::dock::{DockAction, DockLayout, DragTabState, PanelId, PanelInstance, PanelKind};
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

/// Instance-tab order and active tab for one repeatable [`PanelKind`] (the
/// Workspace's equivalent is [`WorkspaceState::open_tabs`]/`active_tab`).
/// `order` lists *every* live instance of the kind, including instances
/// popped out into their own OS windows; the in-container tabs are those not
/// in [`DockLayout::floating_instances`].
#[derive(Default)]
pub struct ContainerTabs {
    pub order: Vec<PanelId>,
    pub active: Option<PanelId>,
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

impl ComparisonState {
    /// A blank comparison run. `reference_luminant` is supplied by the caller:
    /// the built-in Uniform illuminant seeds the primary panel (matching the
    /// app's historical default), while a newly opened extra panel seeds from
    /// the first luminant in the library.
    pub fn new(reference_luminant: Option<CurveId>) -> Self {
        ComparisonState {
            selected_stimuli: Vec::new(),
            selected_species: Vec::new(),
            reference_luminant,
            metric: DistanceMetric::Euclidean,
            active_species_subtab: 0,
            active_regime: HashMap::new(),
            luminant_selected_point: None,
            luminant_scale_factor: 1.0,
            luminant_target_power: 1.0,
            absorption_to_apply: None,
            absorption_weight: 1.0,
            export: BatchExportState::default(),
        }
    }
}

impl Default for ComparisonState {
    fn default() -> Self {
        ComparisonState::new(None)
    }
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

/// Cache storage for [`TransformCache`]: `(species set, reference luminant)`
/// -> (that set's regime pipelines, the species `revision` they were built
/// from).
type PipelineCache = HashMap<(CurveSetId, CurveId), (Rc<Vec<Pipeline>>, u64)>;

/// Cross-window-shared cache (design doc §4.6): keyed by (species set,
/// reference luminant), invalidated when the set's `revision` no longer
/// matches what the cached pipelines were built from. `Pipeline` isn't
/// `Clone`, so cache hits share one `Rc` rather than re-deriving each
/// regime's adaptation matrix. The value is *all* of the set's regime
/// pipelines in regime order - empty for a regime-less species (mantis
/// shrimp, whose receptors are reported individually, §10.2).
#[derive(Default)]
pub struct TransformCache {
    cache: PipelineCache,
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
    /// Every open Comparison panel, keyed by its [`PanelId`]. This is the
    /// state a panel actually draws from; [`AppState::with_comparison`] swaps
    /// the instance's state into the `comparison` field only while it draws.
    pub comparisons: HashMap<PanelId, ComparisonState>,
    /// The *parked*, most-recently-closed Comparison state. A newly opened
    /// Comparison window inherits it when no other Comparison is open;
    /// otherwise the window starts blank. Other panels read its reference
    /// luminant / export settings as app-wide defaults, and the currently
    /// drawing Comparison is swapped in here (see `with_comparison`).
    pub comparison: ComparisonState,
    /// The shared stimulus-curve library (reflectance/radiance/
    /// illuminance samples) - edited primarily from the Stimulus Editor
    /// window, but read by Comparison the same way `curve_sets` is.
    pub stimulus_curves: HashMap<CurveId, StimulusEntry>,
    pub stimulus_order: Vec<CurveId>,
    /// Every open Stimulus Editor panel, keyed by its [`PanelId`]; swapped
    /// into `stimulus_editor` while that instance draws.
    pub stimulus_editors: HashMap<PanelId, StimulusEditorState>,
    /// The *parked*, most-recently-closed Stimulus Editor state, inherited by
    /// a newly opened editor when none other is open (see `comparisons`).
    pub stimulus_editor: StimulusEditorState,
    /// Loaded image cubes, shared by the Stimulus Editor (curve extraction)
    /// and every Image Display panel (receptor renderings).
    pub image_library: ImageLibrary,
    /// One state per open Image Display instance, keyed by its [`PanelId`].
    /// Image Display is a repeatable kind, so several independent instances -
    /// each with its own view list - can be open at once as in-container tabs
    /// and/or popped out into their own OS windows, all reading the one shared
    /// `image_library`.
    pub image_displays: HashMap<PanelId, ImageDisplayState>,
    /// The *parked*, most-recently-closed Image Display state, inherited by a
    /// newly opened panel when none other is open. Unlike the other repeatable
    /// kinds this owns its render worker, so it is `Option`al - nothing is
    /// spawned until a panel actually opens - and parks (rather than drops)
    /// that worker so the views can be restored.
    pub remembered_image_display: Option<ImageDisplayState>,
    /// Lazily-initialized wgpu compute context for the receptor renderer,
    /// shared (`Arc`) with the background render worker; `gpu_attempted`
    /// records that a probe has run (so a machine without a usable adapter
    /// doesn't retry every frame).
    pub gpu: Option<Arc<xenovision_core::gpu::GpuContext>>,
    pub gpu_attempted: bool,
    /// Per-kind instance-tab order and active tab for Comparison, Stimulus
    /// Editor and Image Display. Workspace's equivalent lives in
    /// [`AppState::workspace`] (its instances are Curve Sets).
    pub repeatable_tabs: HashMap<PanelKind, ContainerTabs>,
    /// Which panels are docked as tabs in the main window, floating in
    /// their own windows, or hidden.
    pub dock: DockLayout,
    /// Dock changes requested from inside a panel (e.g. Workspace's
    /// Window menu), applied by `main` after the frame's UI has run so
    /// the layout never changes mid-render.
    pub pending_dock_actions: Vec<DockAction>,
    /// The in-progress tab drag gesture (tear-off or drag-in), if any; see
    /// `dock::DragTabState`.
    pub drag_tab: Option<DragTabState>,
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

    /// The lowest instance number of `kind` not currently live (in a
    /// container or popped out), so the first panel of a kind is always
    /// instance `0` - titled "Comparison", "Image Display", … - and its
    /// repeats are numbered deterministically.
    fn next_free_instance(&self, kind: PanelKind) -> u32 {
        let used: std::collections::HashSet<u32> = self
            .repeatable_tabs
            .get(&kind)
            .map(|tabs| tabs.order.iter().map(|id| id.instance).collect())
            .unwrap_or_default();
        (0u32..)
            .find(|i| !used.contains(i))
            .expect("u32 instance space")
    }

    /// Allocates a fresh instance of a repeatable panel kind and adds it to
    /// that kind's tab order (in-container, or popped out when `floating`).
    /// The new panel inherits its kind's *parked* state - the most recently
    /// closed panel of that kind - when no other instance of that kind is
    /// open; otherwise it starts blank. Comparison panels default their
    /// reference luminant to the first luminant in the library.
    pub fn new_panel(&mut self, kind: PanelKind, floating: bool) -> PanelId {
        debug_assert!(
            !kind.instances_are_curve_sets(),
            "Workspace instances are Curve Sets; use add_curve_set"
        );
        let others_open = self
            .repeatable_tabs
            .get(&kind)
            .map(|tabs| !tabs.order.is_empty())
            .unwrap_or(false);
        let id = PanelId::new(kind, self.next_free_instance(kind));
        match kind {
            PanelKind::Comparison => {
                let reference = self.luminant_order.first().copied();
                let state = if others_open {
                    ComparisonState::new(reference)
                } else {
                    std::mem::replace(&mut self.comparison, ComparisonState::new(reference))
                };
                self.comparisons.insert(id, state);
            }
            PanelKind::StimulusEditor => {
                let state = if others_open {
                    StimulusEditorState::default()
                } else {
                    std::mem::take(&mut self.stimulus_editor)
                };
                self.stimulus_editors.insert(id, state);
            }
            PanelKind::ImageDisplay => {
                let state = if others_open {
                    ImageDisplayState::default()
                } else {
                    self.remembered_image_display.take().unwrap_or_default()
                };
                self.image_displays.insert(id, state);
            }
            PanelKind::Workspace => unreachable!("Workspace instances are Curve Sets"),
        }
        let tabs = self.repeatable_tabs.entry(kind).or_default();
        tabs.order.push(id);
        tabs.active = Some(id);
        if floating {
            self.dock
                .apply(DockAction::FloatInstance(PanelInstance::Panel(id)));
        } else {
            self.dock.active = Some(kind);
        }
        self.reconcile_containers();
        id
    }

    /// Creates a fresh, empty Image Display panel and adds it to the kind's
    /// tab order (in-container, or popped out when `floating`).
    pub fn new_image_display(&mut self, floating: bool) -> PanelId {
        self.new_panel(PanelKind::ImageDisplay, floating)
    }

    /// The Image Display panel a new view should go to: the first one already
    /// open, or a newly created in-container one if none is open.
    pub fn primary_image_display(&mut self) -> PanelId {
        if let Some(id) = self
            .repeatable_tabs
            .get(&PanelKind::ImageDisplay)
            .and_then(|tabs| tabs.order.first().copied())
        {
            return id;
        }
        self.new_image_display(false)
    }

    /// Adds a new blank Curve Set as a Workspace instance (in-container) and
    /// returns its id.
    pub fn add_curve_set(&mut self, name: String) -> CurveSetId {
        let id = self.alloc_id();
        self.curve_sets.insert(
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
        self.workspace.per_tab.insert(id, TabUiState::default());
        self.workspace.open_tabs.push(id);
        self.workspace.active_tab = self.workspace.open_tabs.len() - 1;
        self.reconcile_containers();
        id
    }

    /// Removes a Curve Set and its per-tab UI state for good (the caller is
    /// responsible for any unsaved-changes prompt - unlike a repeatable
    /// panel, a closed Curve Set is discarded, not parked). Also drops it from
    /// Comparison's species selection so that doesn't dangle.
    pub fn close_curve_set(&mut self, set_id: CurveSetId) {
        self.curve_sets.remove(&set_id);
        self.workspace.per_tab.remove(&set_id);
        if let Some(idx) = self.workspace.open_tabs.iter().position(|&id| id == set_id) {
            self.workspace.open_tabs.remove(idx);
            if idx < self.workspace.active_tab {
                self.workspace.active_tab -= 1;
            }
        }
        self.dock
            .floating_instances
            .retain(|&i| i != PanelInstance::Workspace(set_id));
        self.prune_comparison_species(set_id);
        if self.workspace.pending_close == Some(set_id) {
            self.workspace.pending_close = None;
        }
        self.reconcile_containers();
    }

    /// Runs `f` with the Comparison instance `id` installed as
    /// `AppState::comparison`. The instance's live state is moved in for the
    /// duration of the call and moved back out afterwards (the parked state
    /// that normally occupies the field is restored), so the window module can
    /// keep reading and writing `app.comparison` unchanged.
    pub fn with_comparison<R>(&mut self, id: PanelId, f: impl FnOnce(&mut Self) -> R) -> R {
        debug_assert_eq!(id.kind, PanelKind::Comparison);
        let live = self.comparisons.remove(&id).unwrap_or_default();
        let parked = std::mem::replace(&mut self.comparison, live);
        let result = f(self);
        let live = std::mem::replace(&mut self.comparison, parked);
        self.comparisons.insert(id, live);
        result
    }

    /// The Stimulus Editor counterpart of [`AppState::with_comparison`].
    pub fn with_stimulus_editor<R>(&mut self, id: PanelId, f: impl FnOnce(&mut Self) -> R) -> R {
        debug_assert_eq!(id.kind, PanelKind::StimulusEditor);
        let live = self.stimulus_editors.remove(&id).unwrap_or_default();
        let parked = std::mem::replace(&mut self.stimulus_editor, live);
        let result = f(self);
        let live = std::mem::replace(&mut self.stimulus_editor, parked);
        self.stimulus_editors.insert(id, live);
        result
    }

    /// Every live instance of `kind`, in tab order (including instances popped
    /// out into their own OS windows).
    pub fn live_instances(&self, kind: PanelKind) -> Vec<PanelInstance> {
        if kind.instances_are_curve_sets() {
            self.workspace
                .open_tabs
                .iter()
                .map(|&set_id| PanelInstance::Workspace(set_id))
                .collect()
        } else {
            self.repeatable_tabs
                .get(&kind)
                .map(|tabs| {
                    tabs.order
                        .iter()
                        .map(|&id| PanelInstance::Panel(id))
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    /// The selected instance of `kind`, if any.
    pub fn active_instance(&self, kind: PanelKind) -> Option<PanelInstance> {
        if kind.instances_are_curve_sets() {
            self.workspace
                .open_tabs
                .get(self.workspace.active_tab)
                .map(|&set_id| PanelInstance::Workspace(set_id))
        } else {
            self.repeatable_tabs
                .get(&kind)
                .and_then(|tabs| tabs.active)
                .map(PanelInstance::Panel)
        }
    }

    /// Selects an instance tab.
    pub fn set_active_instance(&mut self, inst: PanelInstance) {
        match inst {
            PanelInstance::Workspace(set_id) => {
                if let Some(idx) = self.workspace.open_tabs.iter().position(|&s| s == set_id) {
                    self.workspace.active_tab = idx;
                }
            }
            PanelInstance::Panel(id) => {
                if let Some(tabs) = self.repeatable_tabs.get_mut(&id.kind) {
                    tabs.active = Some(id);
                }
            }
        }
    }

    /// The display title of an instance tab/window (a Curve Set's name, or the
    /// numbered panel title).
    pub fn instance_title(&self, inst: PanelInstance) -> String {
        match inst {
            PanelInstance::Workspace(set_id) => self
                .curve_sets
                .get(&set_id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "(closed)".to_string()),
            PanelInstance::Panel(id) => id.title(),
        }
    }

    /// Applies a dock action and keeps per-panel state consistent with the
    /// layout: `NewInstance` allocates that panel's data, `CloseInstance`
    /// parks a repeatable panel (or discards a Curve Set), and the container
    /// tab strip is reconciled to whatever instances remain in-container.
    pub fn dispatch_dock_action(&mut self, action: DockAction) {
        match action {
            DockAction::NewInstance(kind) => {
                self.new_panel(kind, false);
                return;
            }
            DockAction::ActivateInstance(inst) => self.set_active_instance(inst),
            DockAction::CloseInstance(inst) => self.close_instance(inst),
            other => self.dock.apply(other),
        }
        self.reconcile_containers();
    }

    /// Removes an instance. A repeatable panel's state is *parked* (last closed
    /// wins, inherited by the next instance of that kind); a Workspace Curve
    /// Set is discarded, so a dirty one is instead brought back into its
    /// container and parked in `pending_close` for the unsaved-changes prompt.
    fn close_instance(&mut self, inst: PanelInstance) {
        match inst {
            PanelInstance::Workspace(set_id) => {
                let dirty = self
                    .curve_sets
                    .get(&set_id)
                    .map(|s| s.dirty)
                    .unwrap_or(false);
                if dirty {
                    self.dock.floating_instances.retain(|&i| i != inst);
                    self.workspace.pending_close = Some(set_id);
                    self.reconcile_containers();
                    // Bring the Workspace container forward so the
                    // unsaved-changes prompt is actually visible.
                    self.dock.active = Some(PanelKind::Workspace);
                } else {
                    self.close_curve_set(set_id);
                }
            }
            PanelInstance::Panel(id) => {
                if let Some(tabs) = self.repeatable_tabs.get_mut(&id.kind) {
                    if let Some(pos) = tabs.order.iter().position(|&x| x == id) {
                        tabs.order.remove(pos);
                        if tabs.active == Some(id) {
                            tabs.active = tabs
                                .order
                                .get(pos.saturating_sub(1))
                                .or_else(|| tabs.order.get(pos))
                                .copied();
                        }
                    }
                }
                self.dock.floating_instances.retain(|&i| i != inst);
                match id.kind {
                    PanelKind::Comparison => {
                        if let Some(state) = self.comparisons.remove(&id) {
                            self.comparison = state;
                        }
                    }
                    PanelKind::StimulusEditor => {
                        if let Some(state) = self.stimulus_editors.remove(&id) {
                            self.stimulus_editor = state;
                        }
                    }
                    PanelKind::ImageDisplay => {
                        if let Some(state) = self.image_displays.remove(&id) {
                            self.remembered_image_display = Some(state);
                        }
                    }
                    PanelKind::Workspace => {}
                }
                self.reconcile_containers();
            }
        }
    }

    /// Whether `kind` still has at least one instance shown *inside* its
    /// container (not popped out). A container tab exists only while this
    /// holds.
    fn container_has_in_container(&self, kind: PanelKind) -> bool {
        if kind.instances_are_curve_sets() {
            self.workspace.open_tabs.iter().any(|&set_id| {
                !self
                    .dock
                    .instance_is_floating(PanelInstance::Workspace(set_id))
            })
        } else {
            self.repeatable_tabs
                .get(&kind)
                .map(|tabs| {
                    tabs.order
                        .iter()
                        .any(|&id| !self.dock.instance_is_floating(PanelInstance::Panel(id)))
                })
                .unwrap_or(false)
        }
    }

    /// Adds/removes kind container tabs so exactly the kinds with an
    /// in-container instance are shown, and keeps `dock.active` valid. Call
    /// after any change to instance order or instance placement.
    fn reconcile_containers(&mut self) {
        for kind in PanelKind::ALL {
            if self.container_has_in_container(kind) {
                if !self.dock.docked.contains(&kind)
                    && !self.dock.floating_containers.contains(&kind)
                {
                    self.dock.docked.push(kind);
                }
            } else {
                self.dock.docked.retain(|&k| k != kind);
                self.dock.floating_containers.retain(|&k| k != kind);
            }
        }
        self.dock.fix_active();
    }

    /// Removes `set_id` from every Comparison's species selection and
    /// per-species regime memory - the parked state and every open instance -
    /// so no panel keeps a dangling reference when a visual system is deleted.
    pub fn prune_comparison_species(&mut self, set_id: CurveSetId) {
        for comparison in std::iter::once(&mut self.comparison).chain(self.comparisons.values_mut())
        {
            comparison.selected_species.retain(|&id| id != set_id);
            comparison.active_regime.remove(&set_id);
        }
    }

    /// Clears every Comparison's remembered active regime for `set_id`, for
    /// when a set's regimes change under the panels' feet.
    pub fn prune_comparison_active_regime(&mut self, set_id: CurveSetId) {
        for comparison in std::iter::once(&mut self.comparison).chain(self.comparisons.values_mut())
        {
            comparison.active_regime.remove(&set_id);
        }
    }

    /// Drops a stimulus curve from every Comparison selection and every
    /// Stimulus Editor's selection/pending-unload, for when it is unloaded.
    pub fn forget_stimulus(&mut self, id: CurveId) {
        for comparison in std::iter::once(&mut self.comparison).chain(self.comparisons.values_mut())
        {
            comparison.selected_stimuli.retain(|&i| i != id);
        }
        for editor in
            std::iter::once(&mut self.stimulus_editor).chain(self.stimulus_editors.values_mut())
        {
            if editor.selected == Some(id) {
                editor.selected = None;
            }
            if editor.pending_unload == Some(id) {
                editor.pending_unload = None;
            }
        }
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

        let mut state = AppState {
            next_id,
            curve_sets,
            workspace: WorkspaceState {
                open_tabs,
                active_tab: 0,
                per_tab,
                new_tab_name_buf: String::new(),
                pending_close: None,
            },
            comparisons: HashMap::new(),
            comparison: ComparisonState::new(Some(uniform_id)),
            stimulus_curves,
            stimulus_order,
            stimulus_editors: HashMap::new(),
            stimulus_editor: StimulusEditorState::default(),
            image_library: ImageLibrary::default(),
            image_displays: HashMap::new(),
            remembered_image_display: None,
            gpu: None,
            gpu_attempted: false,
            repeatable_tabs: HashMap::new(),
            dock: DockLayout::default(),
            pending_dock_actions: Vec::new(),
            drag_tab: None,
            transform_cache: TransformCache::default(),
            luminants,
            luminant_order,
            axis_orientation: AxisOrientation::default(),
        };
        state.reconcile_containers();
        state
    }
}
