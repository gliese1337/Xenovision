//! Docking layout for the top-level panels. Every [`PanelKind`] is a
//! **container**: the root OS window hosts one container per kind as a
//! top-level tab, and each container holds its **instances** as its own tab
//! bar. Any instance tab can be popped out into its own OS window, and a
//! whole container can be floated too (moving all of its in-container tabs
//! together). Workspace's instances are its Curve Sets; the other kinds'
//! instances are repeatable panels identified by a [`PanelId`].
//!
//! Both kinds of tab tear off the same way: a `⇱` button, the Windows menu, or
//! a drag gesture (drag the tab out of its strip and release - see
//! [`DragTabState`] and the shared `dragged_tab` widget in `main.rs`). The
//! gesture is reversible: a floating window's own title tab can be dragged back
//! over the root window to re-dock it (see [`tab_release`]).
//!
//! [`DockLayout`] is pure *placement* bookkeeping - which kind containers are
//! docked/floated, and which individual instances are popped out. Instance
//! order, active selection, creation and state parking live in `AppState`,
//! which keeps this layout consistent (see `AppState::dispatch_dock_action`).

use eframe::egui;

use crate::state::CurveSetId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PanelKind {
    Workspace,
    Comparison,
    StimulusEditor,
    ImageDisplay,
}

impl PanelKind {
    pub const ALL: [PanelKind; 4] = [
        PanelKind::Workspace,
        PanelKind::Comparison,
        PanelKind::StimulusEditor,
        PanelKind::ImageDisplay,
    ];

    pub fn title(self) -> &'static str {
        match self {
            PanelKind::Workspace => "Workspace",
            PanelKind::Comparison => "Comparison",
            PanelKind::StimulusEditor => "Stimulus Editor",
            PanelKind::ImageDisplay => "Image Display",
        }
    }

    /// Whether this kind's instances are Workspace Curve Sets - shared,
    /// *discarded* when closed - rather than per-panel states that are
    /// *parked* for the next instance of the kind.
    pub fn instances_are_curve_sets(self) -> bool {
        self == PanelKind::Workspace
    }
}

/// Identifies one instance of a repeatable panel kind: a [`PanelKind`] plus
/// an instance number, numbered lowest-free as instances are created. The
/// first instance of a kind is always `0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PanelId {
    pub kind: PanelKind,
    pub instance: u32,
}

impl PanelId {
    pub const fn new(kind: PanelKind, instance: u32) -> Self {
        PanelId { kind, instance }
    }

    pub const fn singleton(kind: PanelKind) -> Self {
        PanelId::new(kind, 0)
    }

    /// The tab/window title. The first instance is the bare kind name;
    /// repeats get a numeric suffix ("Image Display 2", ...).
    pub fn title(self) -> String {
        if self.instance == 0 {
            self.kind.title().to_string()
        } else {
            format!("{} {}", self.kind.title(), self.instance + 1)
        }
    }

    pub fn viewport_id(self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("xenovision-instance", self.kind, self.instance))
    }
}

/// One instance tab: a Workspace Curve Set (its instances are Curve Sets) or
/// a repeatable panel instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelInstance {
    Workspace(CurveSetId),
    Panel(PanelId),
}

impl PanelInstance {
    pub fn kind(self) -> PanelKind {
        match self {
            PanelInstance::Workspace(_) => PanelKind::Workspace,
            PanelInstance::Panel(id) => id.kind,
        }
    }

    pub fn viewport_id(self) -> egui::ViewportId {
        match self {
            PanelInstance::Workspace(set_id) => {
                egui::ViewportId::from_hash_of(("xenovision-instance", "workspace", set_id))
            }
            PanelInstance::Panel(id) => id.viewport_id(),
        }
    }
}

/// The OS viewport id of a whole container floated into its own window.
pub fn container_viewport_id(kind: PanelKind) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("xenovision-container", kind))
}

/// Which tab a drag gesture is currently grabbing (the shared `dragged_tab`
/// widget in `main.rs` drives both tear-off and drag-in from this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DragTabSource {
    /// An instance tab: popped out into its own OS window when torn off.
    Instance(PanelInstance),
    /// A container tab: floated as a whole OS window when torn off, or docked
    /// back when a floating container is dragged over the root window.
    Container(PanelKind),
}

/// The in-progress tear-off / drag-in gesture, kept in [`AppState::drag_tab`]
/// while the pointer is down. `pulling` flips once the pointer has travelled
/// far enough from the press that releasing does something (float or dock).
#[derive(Debug, Clone, Copy)]
pub struct DragTabState {
    pub source: DragTabSource,
    /// Pointer position at the press, in the source viewport's own coordinates.
    pub press_pos: egui::Pos2,
    /// Whether releasing should float/dock rather than just select.
    pub pulling: bool,
    /// Latest pointer position in global (monitor) coordinates, maintained by
    /// the viewport that owns the gesture. `None` when the backend reports no
    /// viewport geometry (headless, embedded, or a platform without it) - the
    /// gesture then falls back to same-window behaviour.
    pub global_pos: Option<egui::Pos2>,
}

/// How far the pointer must travel from its press origin before the tab is
/// considered "pulled out" of its strip (releasing then floats or docks it).
pub const PULL_OUT_DISTANCE: f32 = 20.0;

/// Whether a drag from `press_pos` to `current_pos` counts as tearing the tab
/// out of its strip.
pub fn is_pulled_out(press_pos: egui::Pos2, current_pos: egui::Pos2) -> bool {
    current_pos.distance(press_pos) > PULL_OUT_DISTANCE
}

/// Where a dragged tab currently lives, which decides whether releasing it
/// pops it out, re-docks it, or does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragTabHome {
    /// In the main window's dock: a container tab in the strip, or an
    /// in-container instance tab.
    Docked,
    /// An instance tab inside a container that has itself been floated.
    FloatingContainer,
    /// A floating window's own title tab.
    FloatingWindow,
}

/// What releasing a dragged tab over the main window area should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabRelease {
    /// Pop the tab out into its own OS window.
    Float,
    /// Move the tab (or its whole floating group) back into the main window.
    Dock,
    /// Leave it where it already is.
    Nothing,
}

/// Whether releasing a tab that lives at `home` while the pointer is (or is
/// assumed to be) over the main window should float, dock, or do nothing.
///
/// Tabs docked in the main window always pop *out*; tabs inside a floating
/// container always tear *out* into their own window (re-docking the group is
/// the container's own title tab's job); only a floating window's title tab
/// re-docks, and only when dropped back over the main window.
pub fn tab_release(home: DragTabHome, over_main: bool) -> TabRelease {
    match home {
        DragTabHome::Docked | DragTabHome::FloatingContainer => TabRelease::Float,
        DragTabHome::FloatingWindow => {
            if over_main {
                TabRelease::Dock
            } else {
                TabRelease::Nothing
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Docked,
    Floating,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockAction {
    /// Select a docked container tab.
    ActivateContainer(PanelKind),
    /// Float a whole container (all its in-container tabs) into one OS window.
    FloatContainer(PanelKind),
    /// Re-dock a floated container.
    DockContainer(PanelKind),
    /// Pop an instance tab out into its own OS window.
    FloatInstance(PanelInstance),
    /// Merge a popped-out instance back into its container.
    DockInstance(PanelInstance),
    /// Remove an instance entirely (park for repeatable kinds; discard a
    /// Curve Set after the unsaved-changes prompt). The owning state handles
    /// it, including instance order and parking.
    CloseInstance(PanelInstance),
    /// Select an in-container instance tab.
    ActivateInstance(PanelInstance),
    /// Allocate and open a fresh instance of a kind. The owning state creates
    /// the new instance's data rather than [`DockLayout::apply`].
    NewInstance(PanelKind),
}

/// Placement state kept by `AppState`: `docked` never contains a kind twice,
/// a kind is in at most one of `docked`/`floating_containers`, and a
/// `PanelInstance` appears in `floating_instances` at most once.
#[derive(Debug, Clone, Default)]
pub struct DockLayout {
    /// Container tabs, in the order shown in the dock strip.
    pub docked: Vec<PanelKind>,
    /// The active docked container.
    pub active: Option<PanelKind>,
    /// Whole containers floated into their own OS windows.
    pub floating_containers: Vec<PanelKind>,
    /// Individual instances popped out into their own OS windows.
    pub floating_instances: Vec<PanelInstance>,
}

impl DockLayout {
    /// Placement of a kind's container tab.
    pub fn container_placement(&self, kind: PanelKind) -> Placement {
        if self.docked.contains(&kind) {
            Placement::Docked
        } else if self.floating_containers.contains(&kind) {
            Placement::Floating
        } else {
            Placement::Hidden
        }
    }

    /// Whether a container tab for `kind` is currently shown at all.
    pub fn container_is_shown(&self, kind: PanelKind) -> bool {
        self.container_placement(kind) != Placement::Hidden
    }

    /// Whether `inst` is popped out into its own OS window.
    pub fn instance_is_floating(&self, inst: PanelInstance) -> bool {
        self.floating_instances.contains(&inst)
    }

    /// Where a dragged tab of `source` currently lives, used to decide whether
    /// releasing it should float, dock, or do nothing.
    pub fn drag_tab_home(&self, source: DragTabSource) -> DragTabHome {
        match source {
            DragTabSource::Container(kind) => {
                if self.floating_containers.contains(&kind) {
                    DragTabHome::FloatingWindow
                } else {
                    DragTabHome::Docked
                }
            }
            DragTabSource::Instance(inst) => {
                if self.instance_is_floating(inst) {
                    DragTabHome::FloatingWindow
                } else if self.container_placement(inst.kind()) == Placement::Floating {
                    DragTabHome::FloatingContainer
                } else {
                    DragTabHome::Docked
                }
            }
        }
    }

    /// Applies a placement-only action. Instance creation/removal and container
    /// presence are reconciled by `AppState` afterwards.
    pub fn apply(&mut self, action: DockAction) {
        match action {
            DockAction::ActivateContainer(kind) => {
                if self.docked.contains(&kind) {
                    self.active = Some(kind);
                }
            }
            DockAction::FloatContainer(kind) => {
                if self.docked.contains(&kind) {
                    self.docked.retain(|&k| k != kind);
                    if !self.floating_containers.contains(&kind) {
                        self.floating_containers.push(kind);
                    }
                    self.fix_active();
                }
            }
            DockAction::DockContainer(kind) => {
                self.floating_containers.retain(|&k| k != kind);
                if !self.docked.contains(&kind) {
                    self.docked.push(kind);
                }
                self.active = Some(kind);
            }
            DockAction::FloatInstance(inst) => {
                if !self.floating_instances.contains(&inst) {
                    self.floating_instances.push(inst);
                }
            }
            DockAction::DockInstance(inst) | DockAction::CloseInstance(inst) => {
                self.floating_instances.retain(|&i| i != inst);
            }
            DockAction::ActivateInstance(_) | DockAction::NewInstance(_) => {}
        }
    }

    /// Keeps `active` pointing at a docked container (or `None` when the dock
    /// strip is empty). Call after `docked` changes.
    pub fn fix_active(&mut self) {
        match self.active {
            Some(kind) if self.docked.contains(&kind) => {}
            _ => self.active = self.docked.first().copied(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(kind: PanelKind, n: u32) -> PanelInstance {
        PanelInstance::Panel(PanelId::new(kind, n))
    }

    #[test]
    fn default_has_workspace_container_docked() {
        let l = DockLayout::default();
        assert_eq!(
            l.container_placement(PanelKind::Workspace),
            Placement::Hidden
        );
        // `AppState` reconciles the default docked Workspace container; the
        // bare layout starts empty.
    }

    #[test]
    fn container_float_and_dock_round_trip() {
        let mut l = DockLayout::default();
        l.apply(DockAction::DockContainer(PanelKind::Workspace));
        assert_eq!(
            l.container_placement(PanelKind::Workspace),
            Placement::Docked
        );
        assert_eq!(l.active, Some(PanelKind::Workspace));

        l.apply(DockAction::DockContainer(PanelKind::Comparison));
        l.apply(DockAction::FloatContainer(PanelKind::Workspace));
        assert_eq!(
            l.container_placement(PanelKind::Workspace),
            Placement::Floating
        );
        assert_eq!(l.active, Some(PanelKind::Comparison), "active falls back");
    }

    #[test]
    fn container_activation_only_while_docked() {
        let mut l = DockLayout::default();
        l.apply(DockAction::ActivateContainer(PanelKind::Workspace));
        assert_eq!(l.active, None, "a hidden container cannot be activated");

        l.apply(DockAction::DockContainer(PanelKind::Workspace));
        l.apply(DockAction::DockContainer(PanelKind::Comparison));
        l.apply(DockAction::ActivateContainer(PanelKind::Workspace));
        assert_eq!(l.active, Some(PanelKind::Workspace));
    }

    #[test]
    fn instance_float_and_dock_are_independent() {
        let mut l = DockLayout::default();
        let a = inst(PanelKind::ImageDisplay, 0);
        let b = inst(PanelKind::ImageDisplay, 1);
        l.apply(DockAction::FloatInstance(a));
        l.apply(DockAction::FloatInstance(b));
        assert!(l.instance_is_floating(a));
        assert!(l.instance_is_floating(b));

        l.apply(DockAction::DockInstance(a));
        assert!(!l.instance_is_floating(a));
        assert!(l.instance_is_floating(b));
    }

    #[test]
    fn closing_an_instance_clears_its_float() {
        let mut l = DockLayout::default();
        let a = inst(PanelKind::Comparison, 0);
        l.apply(DockAction::FloatInstance(a));
        l.apply(DockAction::CloseInstance(a));
        assert!(!l.instance_is_floating(a));
    }

    #[test]
    fn viewport_ids_are_distinct_per_instance_and_container() {
        let a = inst(PanelKind::ImageDisplay, 0);
        let b = inst(PanelKind::ImageDisplay, 1);
        assert_ne!(a.viewport_id(), b.viewport_id());
        assert_ne!(a.viewport_id(), PanelInstance::Workspace(7).viewport_id());
        assert_ne!(
            PanelInstance::Workspace(7).viewport_id(),
            PanelInstance::Workspace(8).viewport_id()
        );
        assert_ne!(
            container_viewport_id(PanelKind::Workspace),
            container_viewport_id(PanelKind::Comparison)
        );
        assert_ne!(
            container_viewport_id(PanelKind::Workspace),
            PanelInstance::Workspace(0).viewport_id()
        );
    }

    #[test]
    fn instance_kind_matches_variant() {
        assert_eq!(PanelInstance::Workspace(3).kind(), PanelKind::Workspace);
        assert_eq!(
            inst(PanelKind::StimulusEditor, 4).kind(),
            PanelKind::StimulusEditor
        );
    }

    #[test]
    fn instance_titles_number_the_repeats() {
        assert_eq!(
            PanelId::singleton(PanelKind::ImageDisplay).title(),
            "Image Display"
        );
        assert_eq!(
            PanelId::new(PanelKind::ImageDisplay, 1).title(),
            "Image Display 2"
        );
        assert!(PanelKind::Workspace.instances_are_curve_sets());
        assert!(!PanelKind::Comparison.instances_are_curve_sets());
    }

    #[test]
    fn drag_tab_home_follows_container_and_instance_placement() {
        let mut dock = DockLayout::default();
        let a = inst(PanelKind::ImageDisplay, 0);

        // Fully docked: both the container tab and its instance tab are in the
        // main window.
        dock.apply(DockAction::DockContainer(PanelKind::ImageDisplay));
        assert_eq!(
            dock.drag_tab_home(DragTabSource::Container(PanelKind::ImageDisplay)),
            DragTabHome::Docked
        );
        assert_eq!(
            dock.drag_tab_home(DragTabSource::Instance(a)),
            DragTabHome::Docked
        );

        // Float the whole group: the container's own tab is now its window's
        // title tab, while the instance tab lives inside a floating container.
        dock.apply(DockAction::FloatContainer(PanelKind::ImageDisplay));
        assert_eq!(
            dock.drag_tab_home(DragTabSource::Container(PanelKind::ImageDisplay)),
            DragTabHome::FloatingWindow
        );
        assert_eq!(
            dock.drag_tab_home(DragTabSource::Instance(a)),
            DragTabHome::FloatingContainer
        );

        // Popping the instance out on its own makes it the window's title tab.
        dock.apply(DockAction::FloatInstance(a));
        assert_eq!(
            dock.drag_tab_home(DragTabSource::Instance(a)),
            DragTabHome::FloatingWindow
        );
    }

    #[test]
    fn tab_release_floats_docked_and_docks_floating_windows() {
        // Docked tabs always pop out, wherever the pointer is.
        assert_eq!(tab_release(DragTabHome::Docked, true), TabRelease::Float);
        assert_eq!(tab_release(DragTabHome::Docked, false), TabRelease::Float);

        // Instance tabs inside a floating container always tear out into their
        // own window; re-docking the group is the container tab's job.
        assert_eq!(
            tab_release(DragTabHome::FloatingContainer, true),
            TabRelease::Float
        );
        assert_eq!(
            tab_release(DragTabHome::FloatingContainer, false),
            TabRelease::Float
        );

        // A floating window's title tab docks only when dropped over the main
        // window, and is left alone otherwise.
        assert_eq!(
            tab_release(DragTabHome::FloatingWindow, true),
            TabRelease::Dock
        );
        assert_eq!(
            tab_release(DragTabHome::FloatingWindow, false),
            TabRelease::Nothing
        );
    }
}
