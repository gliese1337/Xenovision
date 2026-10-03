//! Shared application state for the three-window redesign:
//! one `AppState`, read and written by all three windows
//! (`window_workspace`, `window_comparison`, `window_stimulus_editor`),
//! with no per-window copy of curve data.


use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use xenovision_core::comparison::DistanceMetric;
use xenovision_core::curve_set::OpponentContrast;
use xenovision_core::pipeline::Pipeline;
use xenovision_core::{
    fixture_library, fixtures, CurveSet as CoreCurveSet, CurveType, QuantityKind, SpectralCurve,
};

use crate::dock::{DockAction, DockLayout};
use crate::plot_axis::AxisOrientation;
use crate::undo::UndoManager;
use crate::window_stimulus_editor::StimulusEditorState;

pub type CurveSetId = u64;
pub type CurveId = u64;

/// Which of a `CurveSet`'s two curve lists the Workspace window's left
/// rail / graph is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CurveListTab {
    #[default]
    Colorspace,
    Isolated,
}

/// Everything the per-tab undo/redo stack needs to restore - every
/// field a Workspace editing surface can touch, *except* `dirty`/
/// `revision`/`file_path`, which are bookkeeping, not user-visible data.
#[derive(Clone, PartialEq)]
pub struct EditableSnapshot {
    pub colorspace: Vec<(CurveId, SpectralCurve)>,
    pub isolated: Vec<(CurveId, SpectralCurve)>,
    pub opponent_contrasts: Vec<OpponentContrast>,
    pub metadata: BTreeMap<String, String>,
}

/// App-level Curve Set like `xenovision_core::CurveSet`, but curves
/// are referenced by stable `CurveId` rather than held inline,
/// so a tab/selection/Comparison-window reference survives an edit
/// elsewhere in the set.
pub struct AppCurveSet {
    pub id: CurveSetId,
    pub name: String,
    pub colorspace_curves: Vec<CurveId>,
    pub isolated_curves: Vec<CurveId>,
    pub curves: HashMap<CurveId, SpectralCurve>,
    pub opponent_contrasts: Vec<OpponentContrast>,
    pub metadata: BTreeMap<String, String>,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    /// Bumped on every edit affecting the receptor curves, opponent
    /// contrasts, or luminance weights - `TransformCache`'s invalidation
    /// key.
    pub revision: u64,
}

impl AppCurveSet {
    pub fn snapshot(&self) -> EditableSnapshot {
        EditableSnapshot {
            colorspace: self
                .colorspace_curves
                .iter()
                .map(|id| (*id, self.curves[id].clone()))
                .collect(),
            isolated: self
                .isolated_curves
                .iter()
                .map(|id| (*id, self.curves[id].clone()))
                .collect(),
            opponent_contrasts: self.opponent_contrasts.clone(),
            metadata: self.metadata.clone(),
        }
    }

    /// Restores every undo-tracked field from `snap`, preserving whatever
    /// `CurveId`s the snapshot itself carries (so undo/redo of a curve
    /// add/remove doesn't disturb the ids of curves that weren't touched).
    pub fn restore(&mut self, snap: &EditableSnapshot) {
        self.colorspace_curves = snap.colorspace.iter().map(|(id, _)| *id).collect();
        self.isolated_curves = snap.isolated.iter().map(|(id, _)| *id).collect();
        self.curves = snap
            .colorspace
            .iter()
            .chain(snap.isolated.iter())
            .cloned()
            .collect();
        self.opponent_contrasts = snap.opponent_contrasts.clone();
        self.metadata = snap.metadata.clone();
        self.revision += 1;
        self.dirty = true;
    }

    pub fn to_core(&self) -> CoreCurveSet {
        CoreCurveSet {
            name: self.name.clone(),
            colorspace_curves: self
                .colorspace_curves
                .iter()
                .map(|id| self.curves[id].clone())
                .collect(),
            metadata: self.metadata.clone(),
            opponent_contrasts: self.opponent_contrasts.clone(),
            isolated_curves: self
                .isolated_curves
                .iter()
                .map(|id| self.curves[id].clone())
                .collect(),
        }
    }

    pub fn has_partial_eta_coverage(&self) -> bool {
        self.to_core().has_partial_eta_coverage()
    }

    pub fn receptor_noise_complete(&self) -> bool {
        self.to_core().receptor_noise().is_some()
    }

    /// The integral-derived default luminance weight for the colorspace
    /// curve at `index` (i.e. what applies when that curve has no
    /// explicit `luminance_weight` override) - see
    /// `xenovision_core::CurveSet::luminance_weights`.
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
    let mut colorspace_curves = Vec::new();
    for c in core.colorspace_curves {
        let cid = *next_id;
        *next_id += 1;
        colorspace_curves.push(cid);
        curves.insert(cid, c);
    }
    let mut isolated_curves = Vec::new();
    for c in core.isolated_curves {
        let cid = *next_id;
        *next_id += 1;
        isolated_curves.push(cid);
        curves.insert(cid, c);
    }
    AppCurveSet {
        id,
        name: core.name,
        colorspace_curves,
        isolated_curves,
        curves,
        opponent_contrasts: core.opponent_contrasts,
        metadata: core.metadata,
        file_path: None,
        dirty: false,
        revision: 0,
    }
}

/// Per-tab UI state selection, undo/redo, and
/// transient editing-surface scratch buffers.
pub struct TabUiState {
    pub selected_curve: Option<CurveId>,
    pub selected_point: Option<usize>,
    pub curve_tab: CurveListTab,
    pub undo: UndoManager<EditableSnapshot>,
    pub noise_omega_buf: String,
    pub noise_eta_buf: String,
    pub noise_w_buf: String,
    pub sat_buf: String,
    pub sat_buf_for: Option<CurveId>,
    pub noise_buf_for: Option<CurveId>,
    pub save_path_buf: String,
    pub csv_path_buf: String,
    pub status: String,
}

impl Default for TabUiState {
    fn default() -> Self {
        TabUiState {
            selected_curve: None,
            selected_point: None,
            curve_tab: CurveListTab::default(),
            undo: UndoManager::default(),
            noise_omega_buf: String::new(),
            noise_eta_buf: String::new(),
            noise_w_buf: String::new(),
            sat_buf: String::new(),
            sat_buf_for: None,
            noise_buf_for: None,
            save_path_buf: "visual_system.json".to_string(),
            csv_path_buf: "curve.csv".to_string(),
            status: String::new(),
        }
    }
}

/// Workspace window state
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

/// Comparison window state. Coordinate tables and difference matrices
/// are deliberately *not* stored here - they're recomputed each frame
/// (via `TransformCache`) from the fields below, so there's no
/// derived-data copy that could drift from the underlying curves after
/// a Workspace edit.
pub struct ComparisonState {
    /// Ids into `AppState::stimulus_curves` currently included in this
    /// comparison run - a subset of the shared library, not a copy of
    /// any curve data.
    pub selected_stimuli: Vec<CurveId>,
    pub selected_species: Vec<CurveSetId>,
    pub reference_luminant: Option<CurveId>,
    pub metric: DistanceMetric,
    pub active_species_subtab: usize,
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
}

/// Cross-window-shared cache: keyed by (species set, reference luminant),
/// invalidated when the set's `revision` no longer matches what the cached
/// `Pipeline` was built from. `Pipeline` isn't `Clone`, so cache hits
/// share one `Rc` rather than re-deriving the adaptation matrix.
#[derive(Default)]
pub struct TransformCache {
    cache: HashMap<(CurveSetId, CurveId), (Rc<Pipeline>, u64)>,
}

impl TransformCache {
    pub fn get_or_build(
        &mut self,
        set: &AppCurveSet,
        luminant_id: CurveId,
        luminant: &SpectralCurve,
        step_nm: f64,
    ) -> Rc<Pipeline> {
        if let Some((pipeline, rev)) = self.cache.get(&(set.id, luminant_id)) {
            if *rev == set.revision {
                return Rc::clone(pipeline);
            }
        }
        let pipeline = Rc::new(Pipeline::build(set.to_core(), luminant, step_nm));
        self.cache
            .insert((set.id, luminant_id), (Rc::clone(&pipeline), set.revision));
        pipeline
    }

    /// Drops every cached `Pipeline` built against `luminant_id`. A
    /// cache entry's key is `(species, luminant)`, but its
    /// invalidation above only checks the *species'* revision - editing
    /// a luminant's own curve in place changes neither the species'
    /// `CurveSet` nor its revision, so without this, an edited
    /// luminant's stale `Pipeline` (built from its pre-edit curve)
    /// would keep being reused for anything that still matches on id.
    /// Call this whenever a luminant's curve content changes or it's
    /// removed.
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
        // A real default, not a "placeholder" - a perfectly flat
        // spectrum is a legitimate reference environment in its own
        // right (no wavelength-dependent bias at all), not just a
        // stand-in for a better luminant someone hasn't picked yet.
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
                luminant_selected_point: None,
                luminant_scale_factor: 1.0,
                luminant_target_power: 1.0,
                absorption_to_apply: None,
                absorption_weight: 1.0,
            },
            stimulus_curves,
            stimulus_order,
            stimulus_editor: StimulusEditorState::default(),
            dock: DockLayout::default(),
            pending_dock_actions: Vec::new(),
            transform_cache: TransformCache::default(),
            luminants,
            luminant_order,
            axis_orientation: AxisOrientation::default(),
        }
    }
}
