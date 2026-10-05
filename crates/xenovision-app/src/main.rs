//! Xenovision GUI: three panels (Workspace, Comparison, Stimulus Editor;
//! design doc `docs/gui-design-doc.md` §2) sharing one `AppState`. The
//! main OS window hosts any docked panels as tabs; each other panel
//! floats in its own OS window or is hidden (see `dock`).

// Release builds on Windows are GUI programs, so no console window opens
// alongside the app. Debug builds keep the console for log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod batch_export;
mod dock;
mod gradient;
mod multi_curve_editor;
mod plot_axis;
mod state;
mod stimulus_picker;
mod undo;
mod window_comparison;
mod window_stimulus_editor;
mod window_workspace;

use dock::{DockAction, PanelKind};
use eframe::egui;

struct App {
    state: state::AppState,
}

/// Smallest content area each panel is laid out in. When its window is
/// smaller, the panel keeps this size and scrolls rather than squashing
/// its columns until they're unusable or clipping its controls.
fn min_panel_size(kind: PanelKind) -> egui::Vec2 {
    match kind {
        PanelKind::Workspace => egui::vec2(960.0, 640.0),
        PanelKind::Comparison => egui::vec2(900.0, 520.0),
        PanelKind::StimulusEditor => egui::vec2(800.0, 520.0),
    }
}

/// Draws a panel in whatever container it's in (a dock tab or its own
/// window): normally it fills the space, but never below its minimum
/// size - beyond that the whole panel scrolls in both directions.
fn panel_ui(kind: PanelKind, ui: &mut egui::Ui, state: &mut state::AppState) {
    let size = ui.available_size().max(min_panel_size(kind));
    egui::ScrollArea::both()
        .id_salt(("panel_scroll", kind))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| match kind {
                PanelKind::Workspace => window_workspace::ui(ui, state),
                PanelKind::Comparison => window_comparison::ui(ui, state),
                PanelKind::StimulusEditor => window_stimulus_editor::ui(ui, state),
            });
        });
}

impl Default for App {
    fn default() -> Self {
        App {
            state: state::AppState::new(),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

impl App {
    /// One frame of UI. Separate from `eframe::App::update` so it can be
    /// driven headlessly in tests (an `eframe::Frame` can't be built
    /// outside eframe).
    fn frame(&mut self, ctx: &egui::Context) {
        // Dock changes are collected during the frame and applied at the
        // end, so the set of docked/floating panels never changes while
        // they're being drawn.
        let mut actions: Vec<DockAction> = Vec::new();

        egui::TopBottomPanel::top("dock_tab_strip").show(ctx, |ui| {
            dock::tab_strip(ui, &self.state.dock, &mut actions);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            panel_ui(self.state.dock.active, ui, &mut self.state);
        });

        let mut closed: Vec<PanelKind> = Vec::new();
        for kind in self.state.dock.floating.clone() {
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("Xenovision - {}", kind.title()))
                .with_inner_size([980.0, 640.0]);
            ctx.show_viewport_immediate(kind.viewport_id(), builder, |ctx, _class| {
                egui::TopBottomPanel::top(egui::Id::new(("floating_dock_bar", kind))).show(
                    ctx,
                    |ui| {
                        if ui
                            .button("⇲ Dock into main window")
                            .on_hover_text("Merge this window into the main window as a tab")
                            .clicked()
                        {
                            actions.push(DockAction::Dock(kind));
                        }
                    },
                );
                egui::CentralPanel::default().show(ctx, |ui| {
                    panel_ui(kind, ui, &mut self.state);
                });
                if ctx.input(|i| i.viewport().close_requested()) {
                    closed.push(kind);
                }
            });
        }

        actions.append(&mut self.state.pending_dock_actions);
        for action in actions {
            self.state.dock.apply(action);
        }
        for kind in closed {
            self.state.dock.floating_window_closed(kind);
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        // Speculative mitigation for a window-resize crash reported under
        // WSLg ("Io error: Connection reset by peer" / WinitEventLoop
        // ExitFailure) - vsync's swap-interval handling is a plausible
        // interaction point with GL surface recreation on resize under
        // software/virtualized rendering.
        //
        // Still unconfirmed and now reported again on the three-window
        // build, with one "Connection reset by peer" per open viewport -
        // this crash is NOT fixed yet. A tried follow-up,
        // `hardware_acceleration: HardwareAcceleration::Off` (forcing
        // software rendering so there's no GPU/Zink surface-renegotiation
        // path to fail on resize), was reverted after this sandbox's own
        // smoke test showed it causes a *different*, immediate crash here
        // ("failed to find a matching configuration for creating glutin
        // config") - worse than the original bug in a GL-impaired
        // environment, so not safe to ship blind. This sandbox has no
        // resizable display, so the actual resize crash itself still
        // can't be reproduced or verified fixed from here.
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        "Xenovision",
        options,
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn run_frames(app: &mut App, ctx: &egui::Context, width: f32, height: f32, n: usize) {
        for _ in 0..n {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.frame(ctx));
        }
    }

    fn panel_rect(ctx: &egui::Context, id: &str) -> Option<egui::Rect> {
        egui::containers::panel::PanelState::load(ctx, egui::Id::new(id)).map(|s| s.rect)
    }

    /// Renders every panel both as the active docked tab and floating, so
    /// each arrangement is exercised end to end, not just the layout rules.
    #[test]
    fn every_panel_renders_docked_and_floating() {
        let mut app = App::default();
        // An absorption curve in the library makes the luminant editor
        // show its apply-absorption control; the custom-notch panel is
        // the Stimulus Editor's newest creation mode.
        let notch = xenovision_core::blackbody::builtin_default_notches().remove(0);
        let id = app.state.alloc_id();
        app.state.stimulus_order.push(id);
        app.state.stimulus_curves.insert(
            id,
            state::StimulusEntry {
                curve: notch.notch.absorption_curve(notch.name),
                file_path: None,
                dirty: false,
            },
        );
        app.state.stimulus_editor.creation_mode = window_stimulus_editor::CreationMode::Notch;
        let ctx = egui::Context::default();
        for kind in [PanelKind::Comparison, PanelKind::StimulusEditor] {
            app.state.dock.apply(DockAction::OpenDocked(kind));
            run_frames(&mut app, &ctx, 1200.0, 900.0, 3);
            assert_eq!(app.state.dock.active, kind);
        }
        // Float everything except one tab (the dock can't be emptied).
        app.state
            .dock
            .apply(DockAction::Undock(PanelKind::Workspace));
        app.state
            .dock
            .apply(DockAction::Undock(PanelKind::Comparison));
        assert_eq!(app.state.dock.docked, vec![PanelKind::StimulusEditor]);
        run_frames(&mut app, &ctx, 1200.0, 900.0, 3);
        // Requests queued from inside a panel are applied at frame end.
        app.state
            .pending_dock_actions
            .push(DockAction::Dock(PanelKind::Workspace));
        run_frames(&mut app, &ctx, 1200.0, 900.0, 1);
        assert_eq!(
            app.state.dock.placement(PanelKind::Workspace),
            dock::Placement::Docked
        );
    }

    /// With every species and stimulus selected, the cross-species table
    /// must not rebuild species pipelines per frame - after one warm-up
    /// frame fills the cache, a frame should cost milliseconds.
    #[test]
    fn comparison_frames_reuse_cached_pipelines() {
        let mut app = App::default();
        app.state
            .dock
            .apply(DockAction::OpenDocked(PanelKind::Comparison));
        app.state.comparison.selected_species = app.state.workspace.open_tabs.clone();
        app.state.comparison.selected_stimuli = app.state.stimulus_order.clone();
        let ctx = egui::Context::default();
        run_frames(&mut app, &ctx, 1400.0, 900.0, 2);
        let start = std::time::Instant::now();
        run_frames(&mut app, &ctx, 1400.0, 900.0, 5);
        let per_frame = start.elapsed() / 5;
        assert!(
            per_frame < std::time::Duration::from_millis(250),
            "{per_frame:?} per frame - pipelines are being rebuilt"
        );
    }

    /// In a window far smaller than any panel's minimum size, panels keep
    /// usable proportions (and scroll) instead of squashing.
    #[test]
    fn tiny_window_keeps_panels_usable() {
        let mut app = App::default();
        let set_id = app.state.workspace.open_tabs[0];
        let first = app.state.curve_sets[&set_id].colorspace_curves[0];
        app.state
            .workspace
            .per_tab
            .get_mut(&set_id)
            .unwrap()
            .selected_curve = Some(first);
        let ctx = egui::Context::default();
        run_frames(&mut app, &ctx, 400.0, 300.0, 4);

        let left = panel_rect(&ctx, "workspace_left_rail").expect("left rail laid out");
        let right = panel_rect(&ctx, "workspace_right_inspector").expect("inspector laid out");
        let graph = multi_curve_editor::last_graph_rect(&ctx).expect("graph drawn");
        assert!(left.width() >= 90.0, "left rail squashed: {left:?}");
        assert!(right.width() >= 200.0, "inspector squashed: {right:?}");
        assert!(graph.width() >= 300.0, "graph squashed: {graph:?}");
        assert!(
            right.max.x > 400.0,
            "layout should exceed the 400px window (and scroll), got {right:?}"
        );

        for kind in [PanelKind::Comparison, PanelKind::StimulusEditor] {
            app.state.dock.apply(DockAction::OpenDocked(kind));
            run_frames(&mut app, &ctx, 400.0, 300.0, 3);
        }
    }

    /// Lays out the Workspace at two window widths and checks the left
    /// rail is present and the colorspace graph tracks the width.
    #[test]
    fn workspace_left_rail_present_and_graph_tracks_window_width() {
        let mut app = App::default();
        // Select a curve so the inspector shows its full contents - the
        // widest state it has.
        let set_id = app.state.workspace.open_tabs[0];
        let first_curve = app.state.curve_sets[&set_id].colorspace_curves[0];
        app.state
            .workspace
            .per_tab
            .get_mut(&set_id)
            .unwrap()
            .selected_curve = Some(first_curve);

        let ctx = egui::Context::default();
        let mut report = Vec::new();
        for width in [1200.0, 1800.0] {
            run_frames(&mut app, &ctx, width, 900.0, 4);
            let left = panel_rect(&ctx, "workspace_left_rail");
            let right = panel_rect(&ctx, "workspace_right_inspector");
            let graph = multi_curve_editor::last_graph_rect(&ctx);
            report.push(format!(
                "window {width}: left={left:?} right={right:?} graph={graph:?}"
            ));
        }
        let msg = report.join("\n");
        eprintln!("{msg}");

        // Re-run at each width and assert, now with the report available
        // for diagnosis on failure.
        let mut graph_widths = Vec::new();
        for width in [1200.0, 1800.0] {
            run_frames(&mut app, &ctx, width, 900.0, 4);
            let left = panel_rect(&ctx, "workspace_left_rail")
                .unwrap_or_else(|| panic!("left rail never laid out\n{msg}"));
            assert!(left.width() >= 90.0, "left rail too narrow\n{msg}");
            assert!(left.min.x <= 16.0, "left rail not at the left edge\n{msg}");
            let right = panel_rect(&ctx, "workspace_right_inspector")
                .unwrap_or_else(|| panic!("inspector never laid out\n{msg}"));
            assert!(
                right.max.x <= width,
                "inspector extends past the {width}px window\n{msg}"
            );
            let graph = multi_curve_editor::last_graph_rect(&ctx)
                .unwrap_or_else(|| panic!("graph never drawn\n{msg}"));
            graph_widths.push(graph.width());
        }
        assert!(
            graph_widths[1] > graph_widths[0] + 400.0,
            "graph didn't grow with the window: {graph_widths:?}\n{msg}"
        );
    }
}
