//! Xenovision GUI: four panel kinds (Workspace, Comparison, Stimulus
//! Editor, Image Display; design doc `docs/design-doc.md`) sharing one
//! `AppState`. The main OS window hosts any docked panels as tabs; each
//! other panel floats in its own OS window or is hidden (see `dock`).
//! Workspace is single-instance, but Comparison, Stimulus Editor and Image
//! Display are repeatable, so several of each can be open at once.

// Release builds on Windows are GUI programs, so no console window opens
// alongside the app. Debug builds keep the console for log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod batch_export;
mod colormap;
mod dock;
mod gradient;
mod image_library;
mod multi_curve_editor;
mod plot_axis;
mod region_selector;
mod render_worker;
mod state;
mod stimulus_picker;
mod undo;
mod window_comparison;
mod window_image_display;
mod window_stimulus_editor;
mod window_workspace;

use dock::{DockAction, PanelId, PanelKind};
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
        PanelKind::ImageDisplay => egui::vec2(900.0, 560.0),
    }
}

/// Draws a panel in whatever container it's in (a dock tab or its own
/// window): normally it fills the space, but never below its minimum
/// size - beyond that the whole panel scrolls in both directions.
fn panel_ui(id: PanelId, ui: &mut egui::Ui, state: &mut state::AppState) {
    let size = ui.available_size().max(min_panel_size(id.kind));
    egui::ScrollArea::both()
        .id_salt(("panel_scroll", id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| match id.kind {
                PanelKind::Workspace => window_workspace::ui(ui, state),
                // The repeatable panels: swap the instance's state into the
                // `AppState` slot each window module reads, so the module itself
                // needs no notion of instance numbers.
                PanelKind::Comparison => {
                    state.with_comparison(id, |state| window_comparison::ui(ui, state))
                }
                PanelKind::StimulusEditor => state.with_stimulus_editor(id, |state| {
                    window_stimulus_editor::ui(ui, state, id)
                }),
                PanelKind::ImageDisplay => window_image_display::ui(ui, state, id),
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

        let mut closed: Vec<PanelId> = Vec::new();
        for id in self.state.dock.floating.clone() {
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("Xenovision - {}", id.title()))
                .with_inner_size([980.0, 640.0]);
            ctx.show_viewport_immediate(id.viewport_id(), builder, |ctx, _class| {
                egui::TopBottomPanel::top(egui::Id::new(("floating_dock_bar", id))).show(
                    ctx,
                    |ui| {
                        if ui
                            .button("⇲ Dock into main window")
                            .on_hover_text("Merge this window into the main window as a tab")
                            .clicked()
                        {
                            actions.push(DockAction::Dock(id));
                        }
                    },
                );
                egui::CentralPanel::default().show(ctx, |ui| {
                    panel_ui(id, ui, &mut self.state);
                });
                if ctx.input(|i| i.viewport().close_requested()) {
                    closed.push(id);
                }
            });
        }

        actions.append(&mut self.state.pending_dock_actions);
        for action in actions {
            self.state.dispatch_dock_action(action);
        }
        for id in closed {
            self.state.dismiss_floating(id);
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
        // A freshly opened Stimulus Editor inherits the parked state, so this
        // mode reaches the live instance.
        app.state.stimulus_editor.creation_mode = window_stimulus_editor::CreationMode::Notch;
        let ctx = egui::Context::default();
        for kind in [
            PanelKind::Comparison,
            PanelKind::StimulusEditor,
            PanelKind::ImageDisplay,
        ] {
            let panel = app.state.new_panel(kind, false);
            assert_eq!(
                panel,
                PanelId::singleton(kind),
                "the first panel of a kind is always instance 0"
            );
            run_frames(&mut app, &ctx, 1200.0, 900.0, 3);
            assert_eq!(app.state.dock.active, panel);
        }
        // Float everything except one tab (the dock can't be emptied).
        let ws = PanelId::singleton(PanelKind::Workspace);
        app.state.dock.apply(DockAction::Undock(ws));
        app.state
            .dock
            .apply(DockAction::Undock(PanelId::singleton(PanelKind::Comparison)));
        assert_eq!(
            app.state.dock.docked,
            vec![
                PanelId::singleton(PanelKind::StimulusEditor),
                PanelId::singleton(PanelKind::ImageDisplay)
            ]
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 3);
        // Requests queued from inside a panel are applied at frame end.
        app.state.pending_dock_actions.push(DockAction::Dock(ws));
        run_frames(&mut app, &ctx, 1200.0, 900.0, 1);
        assert_eq!(app.state.dock.placement(ws), dock::Placement::Docked);
    }

    /// The repeatable kind: two Image Display panels can be open at the same
    /// time (here both floating, in their own OS windows) with independent
    /// view lists, reading the one shared image library.
    #[test]
    fn two_image_display_panels_open_simultaneously() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let a = app.state.new_image_display(false);
        let b = app.state.new_image_display(true);
        assert_ne!(a, b);
        app.state.dock.apply(DockAction::Undock(a));
        run_frames(&mut app, &ctx, 1000.0, 700.0, 3);
        assert_eq!(app.state.dock.placement(a), dock::Placement::Floating);
        assert_eq!(app.state.dock.placement(b), dock::Placement::Floating);
        assert_ne!(
            a.viewport_id(),
            b.viewport_id(),
            "each panel needs its own OS viewport id"
        );
        assert!(app.state.image_displays.contains_key(&a));
        assert!(app.state.image_displays.contains_key(&b));
    }

    /// Comparison and Stimulus Editor are repeatable: each instance keeps
    /// independent state (swapped into the module's `AppState` slot only while
    /// that instance draws), and closing a panel *parks* its setup so the next
    /// window of that kind inherits the most recently closed one - but only
    /// when no other window of that kind is open.
    #[test]
    fn repeatable_panels_park_the_last_closed_setup() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let set_a = app.state.workspace.open_tabs[0];
        let set_b = app.state.workspace.open_tabs[1];

        let a = app.state.new_panel(PanelKind::Comparison, false);
        assert_eq!(a, PanelId::singleton(PanelKind::Comparison));
        let b = app.state.new_panel(PanelKind::Comparison, true);
        assert_ne!(a, b);
        assert_eq!(b.instance, 1);
        app.state
            .with_comparison(a, |s| s.comparison.selected_species = vec![set_a]);
        app.state
            .with_comparison(b, |s| s.comparison.selected_species = vec![set_b]);
        app.state
            .with_comparison(a, |s| assert_eq!(s.comparison.selected_species, vec![set_a]));
        app.state
            .with_comparison(b, |s| assert_eq!(s.comparison.selected_species, vec![set_b]));

        // Drawing the panels (state swapped in per instance) must not disturb
        // either instance's own state.
        run_frames(&mut app, &ctx, 1000.0, 700.0, 2);
        app.state
            .with_comparison(a, |s| assert_eq!(s.comparison.selected_species, vec![set_a]));
        app.state
            .with_comparison(b, |s| assert_eq!(s.comparison.selected_species, vec![set_b]));

        // Closing b parks b's setup; a keeps its own.
        app.state.dispatch_dock_action(DockAction::Close(b));
        assert!(!app.state.comparisons.contains_key(&b));
        assert!(app.state.comparisons.contains_key(&a));
        assert_eq!(app.state.comparison.selected_species, vec![set_b]);

        // Close a too: last-closed wins, so a newly opened Comparison inherits
        // a's setup; a second window opened while one is already open starts
        // blank.
        app.state.dispatch_dock_action(DockAction::Close(a));
        let c = app.state.new_panel(PanelKind::Comparison, false);
        app.state
            .with_comparison(c, |s| assert_eq!(s.comparison.selected_species, vec![set_a]));
        let d = app.state.new_panel(PanelKind::Comparison, true);
        app.state
            .with_comparison(d, |s| assert!(s.comparison.selected_species.is_empty()));

        // Stimulus Editor behaves the same way, reusing the freed instance
        // number when it is the only window of its kind.
        let stim_a = app.state.stimulus_order[0];
        let e1 = app.state.new_panel(PanelKind::StimulusEditor, false);
        app.state
            .with_stimulus_editor(e1, |s| s.stimulus_editor.selected = Some(stim_a));
        app.state.dispatch_dock_action(DockAction::Close(e1));
        let e2 = app.state.new_panel(PanelKind::StimulusEditor, false);
        assert_eq!(e2, e1, "reopening reuses the freed instance number");
        app.state.with_stimulus_editor(e2, |s| {
            assert_eq!(s.stimulus_editor.selected, Some(stim_a))
        });
        let e3 = app.state.new_panel(PanelKind::StimulusEditor, true);
        app.state
            .with_stimulus_editor(e3, |s| assert_eq!(s.stimulus_editor.selected, None));
    }

    /// With every species and stimulus selected, the cross-species table
    /// must not rebuild species pipelines per frame - after one warm-up
    /// frame fills the cache, a frame should cost milliseconds.
    #[test]
    fn comparison_frames_reuse_cached_pipelines() {
        let mut app = App::default();
        let id = app.state.new_panel(PanelKind::Comparison, false);
        app.state.with_comparison(id, |s| {
            s.comparison.selected_species = s.workspace.open_tabs.clone();
            s.comparison.selected_stimuli = s.stimulus_order.clone();
        });
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
        let first = app.state.curve_sets[&set_id].curve_order[0];
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

        for kind in [
            PanelKind::Comparison,
            PanelKind::StimulusEditor,
            PanelKind::ImageDisplay,
        ] {
            app.state.new_panel(kind, false);
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
        let first_curve = app.state.curve_sets[&set_id].curve_order[0];
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
