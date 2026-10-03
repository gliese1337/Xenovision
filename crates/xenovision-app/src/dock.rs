//! Docking layout for the three top-level panels. The root OS window is
//! a dock host showing its docked panels as tabs; any panel can instead
//! float in its own OS window, and Comparison/Stimulus Editor can also
//! be hidden. Panel content is unaffected - each window module's `ui()`
//! renders the same way whichever container it ends up in.

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelKind {
    Workspace,
    Comparison,
    StimulusEditor,
}

impl PanelKind {
    pub const ALL: [PanelKind; 3] = [
        PanelKind::Workspace,
        PanelKind::Comparison,
        PanelKind::StimulusEditor,
    ];

    pub fn title(self) -> &'static str {
        match self {
            PanelKind::Workspace => "Workspace",
            PanelKind::Comparison => "Comparison",
            PanelKind::StimulusEditor => "Stimulus Editor",
        }
    }

    /// Workspace holds every open visual system and has no meaningful
    /// "hidden" state, so it is always either docked or floating.
    pub fn can_hide(self) -> bool {
        self != PanelKind::Workspace
    }

    pub fn viewport_id(self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("xenovision-panel", self))
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
    OpenDocked(PanelKind),
    OpenFloating(PanelKind),
    Dock(PanelKind),
    Undock(PanelKind),
    Close(PanelKind),
    Activate(PanelKind),
}

/// Invariants (kept by `apply`): `docked` is never empty, a panel is in
/// at most one of `docked`/`floating`, and Workspace is always in one of
/// them.
pub struct DockLayout {
    pub docked: Vec<PanelKind>,
    pub active: PanelKind,
    pub floating: Vec<PanelKind>,
}

impl Default for DockLayout {
    fn default() -> Self {
        DockLayout {
            docked: vec![PanelKind::Workspace],
            active: PanelKind::Workspace,
            floating: Vec::new(),
        }
    }
}

impl DockLayout {
    pub fn placement(&self, kind: PanelKind) -> Placement {
        if self.docked.contains(&kind) {
            Placement::Docked
        } else if self.floating.contains(&kind) {
            Placement::Floating
        } else {
            Placement::Hidden
        }
    }

    /// Whether `action` is allowed in the current layout - used both to
    /// enable/disable the buttons offering it and as a guard in `apply`.
    pub fn allows(&self, action: DockAction) -> bool {
        match action {
            DockAction::OpenDocked(k) | DockAction::OpenFloating(k) => {
                self.placement(k) == Placement::Hidden
            }
            DockAction::Dock(k) => self.placement(k) == Placement::Floating,
            // Undocking the last tab would leave the root window empty.
            DockAction::Undock(k) => {
                self.placement(k) == Placement::Docked && self.docked.len() > 1
            }
            DockAction::Close(k) => {
                k.can_hide()
                    && match self.placement(k) {
                        Placement::Docked => self.docked.len() > 1,
                        Placement::Floating => true,
                        Placement::Hidden => false,
                    }
            }
            DockAction::Activate(k) => self.placement(k) == Placement::Docked,
        }
    }

    pub fn apply(&mut self, action: DockAction) {
        if !self.allows(action) {
            return;
        }
        match action {
            DockAction::OpenDocked(k) | DockAction::Dock(k) => {
                self.floating.retain(|&p| p != k);
                self.docked.push(k);
                self.active = k;
            }
            DockAction::OpenFloating(k) => self.floating.push(k),
            DockAction::Undock(k) => {
                self.remove_docked(k);
                self.floating.push(k);
            }
            DockAction::Close(k) => {
                self.remove_docked(k);
                self.floating.retain(|&p| p != k);
            }
            DockAction::Activate(k) => self.active = k,
        }
    }

    /// The OS close button on a floating window. Hides panels that can be
    /// hidden; Workspace goes back into the dock instead.
    pub fn floating_window_closed(&mut self, kind: PanelKind) {
        if kind.can_hide() {
            self.apply(DockAction::Close(kind));
        } else {
            self.apply(DockAction::Dock(kind));
        }
    }

    fn remove_docked(&mut self, kind: PanelKind) {
        self.docked.retain(|&p| p != kind);
        if self.active == kind {
            self.active = self.docked[0];
        }
    }
}

/// One row per panel with whichever actions apply to its current
/// placement - shared by the dock host's "Windows" menu and Workspace's
/// "Window" menu so both offer exactly the same operations.
pub fn window_menu(ui: &mut egui::Ui, layout: &DockLayout, actions: &mut Vec<DockAction>) {
    for kind in PanelKind::ALL {
        ui.horizontal(|ui| {
            let state = match layout.placement(kind) {
                Placement::Docked => "tab",
                Placement::Floating => "window",
                Placement::Hidden => "closed",
            };
            ui.label(format!("{} ({state})", kind.title()));
            let options = [
                (DockAction::OpenDocked(kind), "Open as tab"),
                (DockAction::OpenFloating(kind), "Open as window"),
                (DockAction::Dock(kind), "Dock"),
                (DockAction::Undock(kind), "Undock"),
                (DockAction::Close(kind), "Close"),
            ];
            for (action, label) in options {
                // Hide actions that make no sense for this placement at
                // all; disable ones that exist but are blocked (e.g.
                // undocking the last tab).
                let relevant = match action {
                    DockAction::OpenDocked(_) | DockAction::OpenFloating(_) => {
                        layout.placement(kind) == Placement::Hidden
                    }
                    DockAction::Dock(_) => layout.placement(kind) == Placement::Floating,
                    DockAction::Undock(_) => layout.placement(kind) == Placement::Docked,
                    DockAction::Close(_) => {
                        kind.can_hide() && layout.placement(kind) != Placement::Hidden
                    }
                    DockAction::Activate(_) => false,
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
}

/// The dock host's tab strip: one tab per docked panel plus a per-tab
/// Undock/Close, and a "Windows" menu for opening hidden panels.
pub fn tab_strip(ui: &mut egui::Ui, layout: &DockLayout, actions: &mut Vec<DockAction>) {
    // Widgets go directly into the wrapping row (no nested group per tab):
    // a wrapping layout can only wrap individual widgets, so nested groups
    // would make the strip one unbreakable line wider than the window.
    ui.horizontal_wrapped(|ui| {
        for (i, &kind) in layout.docked.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            if ui
                .selectable_label(layout.active == kind, kind.title())
                .clicked()
            {
                actions.push(DockAction::Activate(kind));
            }
            if ui
                .add_enabled(
                    layout.allows(DockAction::Undock(kind)),
                    egui::Button::new("⇱").small(),
                )
                .on_hover_text("Undock into its own window")
                .on_disabled_hover_text("The main window needs at least one tab")
                .clicked()
            {
                actions.push(DockAction::Undock(kind));
            }
            if kind.can_hide()
                && ui
                    .add_enabled(
                        layout.allows(DockAction::Close(kind)),
                        egui::Button::new("×").small(),
                    )
                    .on_hover_text("Close (its contents are kept for next time)")
                    .clicked()
            {
                actions.push(DockAction::Close(kind));
            }
        }
        ui.separator();
        ui.menu_button("Windows ▾", |ui| window_menu(ui, layout, actions));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_workspace_docked_only() {
        let l = DockLayout::default();
        assert_eq!(l.docked, vec![PanelKind::Workspace]);
        assert_eq!(l.placement(PanelKind::Comparison), Placement::Hidden);
    }

    #[test]
    fn last_docked_tab_cannot_be_undocked_or_closed() {
        let mut l = DockLayout::default();
        assert!(!l.allows(DockAction::Undock(PanelKind::Workspace)));
        l.apply(DockAction::Undock(PanelKind::Workspace));
        assert_eq!(
            l.docked,
            vec![PanelKind::Workspace],
            "blocked action is a no-op"
        );

        l.apply(DockAction::OpenDocked(PanelKind::Comparison));
        l.apply(DockAction::Undock(PanelKind::Workspace));
        assert_eq!(l.docked, vec![PanelKind::Comparison]);
        assert!(
            !l.allows(DockAction::Close(PanelKind::Comparison)),
            "would empty the dock"
        );
    }

    #[test]
    fn dock_undock_round_trip_and_active_tab_follows() {
        let mut l = DockLayout::default();
        l.apply(DockAction::OpenFloating(PanelKind::StimulusEditor));
        assert_eq!(l.placement(PanelKind::StimulusEditor), Placement::Floating);
        l.apply(DockAction::Dock(PanelKind::StimulusEditor));
        assert_eq!(l.placement(PanelKind::StimulusEditor), Placement::Docked);
        assert_eq!(l.active, PanelKind::StimulusEditor);
        assert!(l.floating.is_empty());

        l.apply(DockAction::Undock(PanelKind::StimulusEditor));
        assert_eq!(l.placement(PanelKind::StimulusEditor), Placement::Floating);
        assert_eq!(
            l.active,
            PanelKind::Workspace,
            "active falls back to a remaining tab"
        );
    }

    #[test]
    fn closing_floating_workspace_redocks_instead_of_hiding() {
        let mut l = DockLayout::default();
        l.apply(DockAction::OpenDocked(PanelKind::Comparison));
        l.apply(DockAction::Undock(PanelKind::Workspace));
        l.floating_window_closed(PanelKind::Workspace);
        assert_eq!(l.placement(PanelKind::Workspace), Placement::Docked);

        l.apply(DockAction::OpenFloating(PanelKind::StimulusEditor));
        l.floating_window_closed(PanelKind::StimulusEditor);
        assert_eq!(l.placement(PanelKind::StimulusEditor), Placement::Hidden);
    }

    #[test]
    fn workspace_is_never_hideable() {
        let mut l = DockLayout::default();
        l.apply(DockAction::OpenDocked(PanelKind::Comparison));
        assert!(!l.allows(DockAction::Close(PanelKind::Workspace)));
        l.apply(DockAction::Close(PanelKind::Workspace));
        assert_eq!(l.placement(PanelKind::Workspace), Placement::Docked);
    }
}
