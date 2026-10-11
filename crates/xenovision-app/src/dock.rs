//! Docking layout for the top-level panels. The root OS window is
//! a dock host showing its docked panels as tabs; any panel can instead
//! float in its own OS window, and Comparison/Stimulus Editor/Image
//! Display can also be hidden. Panel content is unaffected - each window
//! module's `ui()` renders the same way whichever container it ends up in.
//!
//! A panel is identified by a [`PanelId`] (a [`PanelKind`] plus an instance
//! number). Singleton kinds always use instance `0`. Comparison, Stimulus
//! Editor and Image Display are *repeatable*
//! ([`PanelKind::is_multi_instance`]), so several independent panels can be
//! open (docked and/or floating) at the same time - each with its own
//! selection/views, all reading the same shared state (curve sets, stimulus
//! library, image library). The first instance (`0`) of Comparison and
//! Stimulus Editor is the long-lived "primary" - its state survives being
//! hidden and is what other panels' cross-references read - while extra
//! instances (and every Image Display panel) are discarded when closed.

use eframe::egui;

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

    /// Whether several instances of this panel can be open at once. Workspace
    /// is the only single-instance kind; Comparison, Stimulus Editor and
    /// Image Display are repeatable.
    pub fn is_multi_instance(self) -> bool {
        self != PanelKind::Workspace
    }

    /// Workspace holds every open visual system and has no meaningful
    /// "hidden" state, so it is always either docked or floating.
    pub fn can_hide(self) -> bool {
        self != PanelKind::Workspace
    }
}

/// Identifies one panel instance: a [`PanelKind`] plus an instance number.
/// Singleton kinds always use instance `0`; repeatable kinds are numbered
/// from `0` as they are created.
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
        egui::ViewportId::from_hash_of(("xenovision-panel", self.kind, self.instance))
    }

    /// Whether closing (hiding) this instance preserves its state for when it
    /// is reopened. The first instance of a repeatable Comparison/Stimulus
    /// Editor panel is the long-lived "primary" - its state lives in
    /// `AppState`'s dedicated fields, so hiding the panel keeps it - while
    /// every extra instance and every Image Display panel is discarded on
    /// close (a new one starts blank).
    pub fn keeps_state_when_closed(self) -> bool {
        match self.kind {
            PanelKind::ImageDisplay => false,
            _ => self.instance == 0,
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
    OpenDocked(PanelId),
    OpenFloating(PanelId),
    Dock(PanelId),
    Undock(PanelId),
    Close(PanelId),
    Activate(PanelId),
    /// Allocate and open a fresh instance of a repeatable panel kind. The
    /// layout always allows it; the owning state (which must create the new
    /// panel's data) handles it rather than [`DockLayout::apply`].
    NewInstance(PanelKind),
}

/// Invariants (kept by `apply`): `docked` is never empty, a panel id is in
/// at most one of `docked`/`floating`, and Workspace is always in one of
/// them.
pub struct DockLayout {
    pub docked: Vec<PanelId>,
    pub active: PanelId,
    pub floating: Vec<PanelId>,
}

impl Default for DockLayout {
    fn default() -> Self {
        DockLayout {
            docked: vec![PanelId::singleton(PanelKind::Workspace)],
            active: PanelId::singleton(PanelKind::Workspace),
            floating: Vec::new(),
        }
    }
}

impl DockLayout {
    pub fn placement(&self, id: PanelId) -> Placement {
        if self.docked.contains(&id) {
            Placement::Docked
        } else if self.floating.contains(&id) {
            Placement::Floating
        } else {
            Placement::Hidden
        }
    }

    /// Every id of `kind` currently docked or floating, docked first. Since
    /// each id is in exactly one list, the result has no duplicates.
    pub fn panels_of_kind(&self, kind: PanelKind) -> Vec<PanelId> {
        self.docked
            .iter()
            .chain(self.floating.iter())
            .filter(|id| id.kind == kind)
            .copied()
            .collect()
    }

    /// Whether `action` is allowed in the current layout - used both to
    /// enable/disable the buttons offering it and as a guard in `apply`.
    pub fn allows(&self, action: DockAction) -> bool {
        match action {
            DockAction::OpenDocked(id) | DockAction::OpenFloating(id) => {
                self.placement(id) == Placement::Hidden
            }
            DockAction::Dock(id) => self.placement(id) == Placement::Floating,
            // Undocking the last tab would leave the root window empty.
            DockAction::Undock(id) => {
                self.placement(id) == Placement::Docked && self.docked.len() > 1
            }
            DockAction::Close(id) => {
                id.kind.can_hide()
                    && match self.placement(id) {
                        Placement::Docked => self.docked.len() > 1,
                        Placement::Floating => true,
                        Placement::Hidden => false,
                    }
            }
            DockAction::Activate(id) => self.placement(id) == Placement::Docked,
            DockAction::NewInstance(_) => true,
        }
    }

    pub fn apply(&mut self, action: DockAction) {
        if !self.allows(action) {
            return;
        }
        match action {
            DockAction::OpenDocked(id) | DockAction::Dock(id) => {
                self.floating.retain(|&p| p != id);
                self.docked.push(id);
                self.active = id;
            }
            DockAction::OpenFloating(id) => self.floating.push(id),
            DockAction::Undock(id) => {
                self.remove_docked(id);
                self.floating.push(id);
            }
            DockAction::Close(id) => {
                self.remove_docked(id);
                self.floating.retain(|&p| p != id);
            }
            DockAction::Activate(id) => self.active = id,
            // Handled by the owning state, which allocates the new instance
            // and then opens it with OpenDocked/OpenFloating.
            DockAction::NewInstance(_) => {}
        }
    }

    /// The OS close button on a floating window. Hides panels that can be
    /// hidden; Workspace goes back into the dock instead.
    pub fn floating_window_closed(&mut self, id: PanelId) {
        if id.kind.can_hide() {
            self.apply(DockAction::Close(id));
        } else {
            self.apply(DockAction::Dock(id));
        }
    }

    fn remove_docked(&mut self, id: PanelId) {
        self.docked.retain(|&p| p != id);
        if self.active == id {
            self.active = self.docked[0];
        }
    }
}

/// One row for a panel instance with whichever actions apply to its current
/// placement - the shared body of the dock host's "Windows" menu and
/// Workspace's "Window" menu, so both offer exactly the same operations.
fn instance_row(ui: &mut egui::Ui, layout: &DockLayout, id: PanelId, actions: &mut Vec<DockAction>) {
    ui.horizontal(|ui| {
        let state = match layout.placement(id) {
            Placement::Docked => "tab",
            Placement::Floating => "window",
            Placement::Hidden => "closed",
        };
        ui.label(format!("{} ({state})", id.title()));
        let options = [
            (DockAction::OpenDocked(id), "Open as tab"),
            (DockAction::OpenFloating(id), "Open as window"),
            (DockAction::Dock(id), "Dock"),
            (DockAction::Undock(id), "Undock"),
            (DockAction::Close(id), "Close"),
        ];
        for (action, label) in options {
            // Hide actions that make no sense for this placement at all;
            // disable ones that exist but are blocked (e.g. undocking the
            // last tab).
            let relevant = match action {
                DockAction::OpenDocked(_) | DockAction::OpenFloating(_) => {
                    layout.placement(id) == Placement::Hidden
                }
                DockAction::Dock(_) => layout.placement(id) == Placement::Floating,
                DockAction::Undock(_) => layout.placement(id) == Placement::Docked,
                DockAction::Close(_) => {
                    id.kind.can_hide() && layout.placement(id) != Placement::Hidden
                }
                DockAction::Activate(_) | DockAction::NewInstance(_) => false,
            };
            if relevant
                && ui
                    .add_enabled(layout.allows(action), egui::Button::new(label))
                    .clicked()
            {
                actions.push(action);
                ui.close_menu();
            }
        }
    });
}

/// One row per panel kind for its existing instances, plus a "New ... window"
/// action for repeatable kinds - shared by the dock host's "Windows" menu and
/// Workspace's "Window" menu.
pub fn window_menu(ui: &mut egui::Ui, layout: &DockLayout, actions: &mut Vec<DockAction>) {
    for kind in PanelKind::ALL {
        if kind.is_multi_instance() {
            // The primary instance of Comparison/Stimulus Editor exists even
            // while hidden (its state persists), so it is always listed; extra
            // instances only exist while open. Image Display has no persistent
            // instance, so only open ones appear.
            let primary = PanelId::singleton(kind);
            let mut ids = Vec::new();
            if primary.keeps_state_when_closed() {
                ids.push(primary);
            }
            for id in layout.panels_of_kind(kind) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids.sort_unstable();
            for id in ids {
                instance_row(ui, layout, id, actions);
            }
            if ui
                .button(format!("＋ New {} window", kind.title()))
                .on_hover_text(
                    "Open another independent panel; every window shares the loaded data",
                )
                .clicked()
            {
                actions.push(DockAction::NewInstance(kind));
                ui.close_menu();
            }
        } else {
            instance_row(ui, layout, PanelId::singleton(kind), actions);
        }
    }
}

/// The dock host's tab strip: one tab per docked panel plus a per-tab
/// Undock/Close, and a "Windows" menu for opening hidden panels.
pub fn tab_strip(ui: &mut egui::Ui, layout: &DockLayout, actions: &mut Vec<DockAction>) {
    // Widgets go directly into the wrapping row (no nested group per tab):
    // a wrapping layout can only wrap individual widgets, so nested groups
    // would make the strip one unbreakable line wider than the window.
    ui.horizontal_wrapped(|ui| {
        for (i, &id) in layout.docked.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            if ui
                .selectable_label(layout.active == id, id.title())
                .clicked()
            {
                actions.push(DockAction::Activate(id));
            }
            if ui
                .add_enabled(
                    layout.allows(DockAction::Undock(id)),
                    egui::Button::new("⇱").small(),
                )
                .on_hover_text("Undock into its own window")
                .on_disabled_hover_text("The main window needs at least one tab")
                .clicked()
            {
                actions.push(DockAction::Undock(id));
            }
            if id.kind.can_hide()
                && ui
                    .add_enabled(
                        layout.allows(DockAction::Close(id)),
                        egui::Button::new("×").small(),
                    )
                    .on_hover_text(if id.keeps_state_when_closed() {
                        "Close (its contents are kept for next time)"
                    } else {
                        "Close (discards this panel; the shared data is kept)"
                    })
                    .clicked()
            {
                actions.push(DockAction::Close(id));
            }
        }
        ui.separator();
        ui.menu_button("Windows ▾", |ui| window_menu(ui, layout, actions));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> PanelId {
        PanelId::singleton(PanelKind::Workspace)
    }
    fn cmp() -> PanelId {
        PanelId::singleton(PanelKind::Comparison)
    }
    fn stim() -> PanelId {
        PanelId::singleton(PanelKind::StimulusEditor)
    }
    fn img(n: u32) -> PanelId {
        PanelId::new(PanelKind::ImageDisplay, n)
    }

    #[test]
    fn default_has_workspace_docked_only() {
        let l = DockLayout::default();
        assert_eq!(l.docked, vec![ws()]);
        assert_eq!(l.placement(cmp()), Placement::Hidden);
    }

    #[test]
    fn last_docked_tab_cannot_be_undocked_or_closed() {
        let mut l = DockLayout::default();
        assert!(!l.allows(DockAction::Undock(ws())));
        l.apply(DockAction::Undock(ws()));
        assert_eq!(l.docked, vec![ws()], "blocked action is a no-op");

        l.apply(DockAction::OpenDocked(cmp()));
        l.apply(DockAction::Undock(ws()));
        assert_eq!(l.docked, vec![cmp()]);
        assert!(
            !l.allows(DockAction::Close(cmp())),
            "would empty the dock"
        );
    }

    #[test]
    fn dock_undock_round_trip_and_active_tab_follows() {
        let mut l = DockLayout::default();
        l.apply(DockAction::OpenFloating(stim()));
        assert_eq!(l.placement(stim()), Placement::Floating);
        l.apply(DockAction::Dock(stim()));
        assert_eq!(l.placement(stim()), Placement::Docked);
        assert_eq!(l.active, stim());
        assert!(l.floating.is_empty());

        l.apply(DockAction::Undock(stim()));
        assert_eq!(l.placement(stim()), Placement::Floating);
        assert_eq!(
            l.active,
            ws(),
            "active falls back to a remaining tab"
        );
    }

    #[test]
    fn closing_floating_workspace_redocks_instead_of_hiding() {
        let mut l = DockLayout::default();
        l.apply(DockAction::OpenDocked(cmp()));
        l.apply(DockAction::Undock(ws()));
        l.floating_window_closed(ws());
        assert_eq!(l.placement(ws()), Placement::Docked);

        l.apply(DockAction::OpenFloating(stim()));
        l.floating_window_closed(stim());
        assert_eq!(l.placement(stim()), Placement::Hidden);
    }

    #[test]
    fn workspace_is_never_hideable() {
        let mut l = DockLayout::default();
        l.apply(DockAction::OpenDocked(cmp()));
        assert!(!l.allows(DockAction::Close(ws())));
        l.apply(DockAction::Close(ws()));
        assert_eq!(l.placement(ws()), Placement::Docked);
    }

    #[test]
    fn multiple_image_display_instances_are_independent() {
        let mut l = DockLayout::default();
        l.apply(DockAction::OpenDocked(img(0)));
        l.apply(DockAction::OpenFloating(img(1)));
        assert_eq!(l.placement(img(0)), Placement::Docked);
        assert_eq!(l.placement(img(1)), Placement::Floating);
        assert_eq!(
            l.panels_of_kind(PanelKind::ImageDisplay),
            vec![img(0), img(1)]
        );
        // Closing one leaves the other untouched.
        l.apply(DockAction::Close(img(0)));
        assert_eq!(l.placement(img(0)), Placement::Hidden);
        assert_eq!(l.placement(img(1)), Placement::Floating);
    }

    #[test]
    fn instance_titles_number_the_repeats() {
        assert_eq!(img(0).title(), "Image Display");
        assert_eq!(img(1).title(), "Image Display 2");
        assert_eq!(ws().title(), "Workspace");
    }

    #[test]
    fn comparison_and_stimulus_editor_are_repeatable() {
        let mut l = DockLayout::default();
        let cmp2 = PanelId::new(PanelKind::Comparison, 1);
        let stim2 = PanelId::new(PanelKind::StimulusEditor, 1);
        l.apply(DockAction::OpenDocked(cmp()));
        l.apply(DockAction::OpenFloating(cmp2));
        l.apply(DockAction::OpenFloating(stim2));
        assert_eq!(l.placement(cmp()), Placement::Docked);
        assert_eq!(l.placement(cmp2), Placement::Floating);
        assert_eq!(l.placement(stim()), Placement::Hidden);
        assert_eq!(l.placement(stim2), Placement::Floating);
        assert_eq!(
            l.panels_of_kind(PanelKind::Comparison),
            vec![cmp(), cmp2]
        );

        assert!(PanelKind::Comparison.is_multi_instance());
        assert!(PanelKind::StimulusEditor.is_multi_instance());
        assert!(PanelKind::ImageDisplay.is_multi_instance());
        assert!(!PanelKind::Workspace.is_multi_instance());
    }

    #[test]
    fn only_the_primary_instance_of_comparison_keeps_state_when_closed() {
        // The first Comparison/Stimulus Editor instance is the persistent
        // primary; extra instances and every Image Display panel are
        // discarded when closed.
        assert!(cmp().keeps_state_when_closed());
        assert!(!PanelId::new(PanelKind::Comparison, 1).keeps_state_when_closed());
        assert!(stim().keeps_state_when_closed());
        assert!(!PanelId::new(PanelKind::StimulusEditor, 2).keeps_state_when_closed());
        assert!(!img(0).keeps_state_when_closed());
        assert!(!img(3).keeps_state_when_closed());
    }
}
