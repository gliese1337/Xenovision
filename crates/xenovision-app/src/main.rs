//! Xenovision GUI: four panel kinds (Workspace, Comparison, Stimulus
//! Editor, Image Display; design doc `docs/design-doc.md`) sharing one
//! `AppState`. Every kind is a *container* shown as a single top-level tab in
//! the main OS window, holding its instances as its own tab bar; any instance
//! can be popped out into its own OS window, and a whole container can be
//! floated too (see `dock`). Workspace's instances are its Curve Sets; the
//! other kinds' are repeatable panels.

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
mod shots;
mod state;
mod stimulus_picker;
mod undo;
mod window_comparison;
mod window_image_display;
mod window_stimulus_editor;
mod window_workspace;

use dock::{DockAction, DragTabSource, PanelId, PanelInstance, PanelKind, Placement};
use eframe::egui;

struct App {
    state: state::AppState,
    /// When launched with `--screenshot`, the scriptable documentation
    /// screenshot driver; `None` in normal use (and in tests).
    shots: Option<shots::ShotSession>,
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

/// Draws one instance (a repeatable panel, or a single Workspace Curve Set) in
/// whatever container it's in (a container tab or its own OS window): normally
/// it fills the space, but never below its kind's minimum size - beyond that
/// the whole instance scrolls in both directions.
fn instance_ui(inst: PanelInstance, ui: &mut egui::Ui, state: &mut state::AppState) {
    let kind = inst.kind();
    let size = ui.available_size().max(min_panel_size(kind));
    egui::ScrollArea::both()
        .id_salt(("instance_scroll", inst))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| match inst {
                PanelInstance::Workspace(set_id) => {
                    window_workspace::instance_ui(ui, state, set_id)
                }
                // The repeatable panels: swap the instance's state into the
                // `AppState` slot each window module reads, so the module itself
                // needs no notion of instance numbers.
                PanelInstance::Panel(id) => match id.kind {
                    PanelKind::Comparison => {
                        state.with_comparison(id, |state| window_comparison::ui(ui, state))
                    }
                    PanelKind::StimulusEditor => state.with_stimulus_editor(id, |state| {
                        window_stimulus_editor::ui(ui, state, id)
                    }),
                    PanelKind::ImageDisplay => window_image_display::ui(ui, state, id),
                    PanelKind::Workspace => unreachable!("Workspace has no PanelId instance"),
                },
            });
        });
}

/// Draws a whole container: its instance tab bar plus the active in-container
/// instance. Workspace has its own richer tab bar (curve naming, unsaved-close
/// confirmation) drawn as one unit under the same min-size scroll wrapper every
/// panel used to get; the other kinds share [`repeatable_tab_bar`].
fn container_ui(kind: PanelKind, ui: &mut egui::Ui, state: &mut state::AppState) {
    match kind {
        PanelKind::Workspace => {
            let size = ui.available_size().max(min_panel_size(kind));
            egui::ScrollArea::both()
                .id_salt(("container_scroll", kind))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                        window_workspace::ui(ui, state)
                    });
                });
        }
        _ => {
            repeatable_tab_bar(kind, ui, state);
            ui.separator();
            match state.active_instance(kind) {
                Some(inst) => instance_ui(inst, ui, state),
                None => {
                    ui.label(format!(
                        "No {} open - use the \"＋ New\" button above.",
                        kind.title()
                    ));
                }
            }
        }
    }
}

/// The instance tab bar shared by Comparison, Stimulus Editor and Image
/// Display: one tab per *in-container* instance (popped-out instances live in
/// their own windows and are reached from the Windows menu), plus a "New"
/// button.
fn repeatable_tab_bar(kind: PanelKind, ui: &mut egui::Ui, state: &mut state::AppState) {
    ui.horizontal_wrapped(|ui| {
        let tabs: Vec<PanelId> = state
            .repeatable_tabs
            .get(&kind)
            .map(|tabs| tabs.order.clone())
            .unwrap_or_default();
        for id in tabs {
            if state.dock.instance_is_floating(PanelInstance::Panel(id)) {
                continue;
            }
            dragged_tab(
                ui,
                state,
                DragTabSource::Instance(PanelInstance::Panel(id)),
                id.title(),
                state.active_instance(kind) == Some(PanelInstance::Panel(id)),
                |state| state.set_active_instance(PanelInstance::Panel(id)),
            );
            if ui
                .small_button("⇱")
                .on_hover_text("Pop out into its own window")
                .clicked()
            {
                state
                    .pending_dock_actions
                    .push(DockAction::FloatInstance(PanelInstance::Panel(id)));
            }
            if ui
                .small_button("×")
                .on_hover_text("Close (its setup is remembered for the next window)")
                .clicked()
            {
                state
                    .pending_dock_actions
                    .push(DockAction::CloseInstance(PanelInstance::Panel(id)));
            }
        }
        ui.separator();
        if ui
            .button(format!("＋ New {}", kind.title()))
            .on_hover_text(
                "Add another independent panel; every window shares the loaded data. \
                 When no window of this kind is open, the new one inherits the most \
                 recently closed one's setup.",
            )
            .clicked()
        {
            state
                .pending_dock_actions
                .push(DockAction::NewInstance(kind));
        }
    });
}

/// A single tab label that doubles as a drag handle. Clicking selects the tab
/// (via `on_click`); dragging it past [`dock::PULL_OUT_DISTANCE`] and releasing
/// pops it out (a docked tab) or re-docks it when dropped over the main window
/// (a floating window's title tab) - see [`dock::tab_release`]. While the
/// pointer is pulling, a ghost chip follows it across OS windows.
///
/// Shared by every tab bar (dock strip, repeatable instance bars, Workspace's
/// Curve Set bar, and floating windows' title bars) so the gesture is uniform.
pub fn dragged_tab(
    ui: &mut egui::Ui,
    state: &mut state::AppState,
    source: DragTabSource,
    text: String,
    selected: bool,
    mut on_click: impl FnMut(&mut state::AppState),
) {
    let response = ui.add(
        egui::Button::new(text)
            .selected(selected)
            .sense(egui::Sense::click_and_drag()),
    );
    #[cfg(test)]
    TAB_RECTS.with(|rects| {
        rects.borrow_mut().insert(source, response.rect);
    });
    if response.clicked() {
        on_click(state);
    }
    handle_tab_drag(ui, state, source, &response, on_click);
}

// Test-only registry of the last frame's tab rects, keyed by source, so
// interaction tests can target a tab without hard-coding coordinates.
#[cfg(test)]
thread_local! {
    static TAB_RECTS: std::cell::RefCell<
        std::collections::HashMap<DragTabSource, egui::Rect>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Drives the "drag the tab out of its strip / back into the main window"
/// gesture for one [`dragged_tab`]. The gesture runs within the widget's own
/// context, so it works the same whether the tab bar lives in the root window,
/// a floated container, or a popped-out instance's window. Cross-window
/// hit-testing uses global pointer coordinates (see [`draw_drag_ghost`]).
/// Releasing a tab that barely moved is treated as a selection, not a no-op.
fn handle_tab_drag(
    ui: &mut egui::Ui,
    state: &mut state::AppState,
    source: DragTabSource,
    response: &egui::Response,
    mut on_click: impl FnMut(&mut state::AppState),
) {
    if response.drag_started() {
        let press_pos = ui
            .ctx()
            .input(|i| i.pointer.press_origin())
            .unwrap_or_else(|| response.interact_pointer_pos().unwrap_or_default());
        state.drag_tab = Some(dock::DragTabState {
            source,
            press_pos,
            pulling: false,
            global_pos: pointer_global(ui.ctx()),
        });
    }

    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let local = ui
            .ctx()
            .input(|i| i.pointer.latest_pos().or_else(|| i.pointer.interact_pos()));
        let global = pointer_global(ui.ctx());
        if let Some(drag) = &mut state.drag_tab {
            if drag.source == source {
                if let Some(local) = local {
                    if !drag.pulling {
                        drag.pulling = dock::is_pulled_out(drag.press_pos, local);
                    }
                }
                if let Some(global) = global {
                    drag.global_pos = Some(global);
                }
            }
        }
    }

    if response.drag_stopped() {
        if matches!(&state.drag_tab, Some(d) if d.source == source) {
            let drag = state.drag_tab.take().expect("checked just above");
            if drag.pulling {
                let over_main = pointer_over_main(ui.ctx(), drag.global_pos);
                apply_tab_release(state, source, over_main);
            } else {
                // Released without pulling far enough: a (slightly wiggly)
                // click after all.
                on_click(state);
            }
        }
    } else if let Some(drag) = &state.drag_tab {
        // Clean up a gesture whose release was never seen by this widget
        // (e.g. the pointer was released outside the OS window): otherwise the
        // ghost chip would linger until the next press. The next real
        // press/click on the tab starts fresh.
        if drag.source == source && !response.is_pointer_button_down_on() && !response.dragged() {
            state.drag_tab = None;
        }
    }
}

/// The global (monitor-space, points) pointer position, or `None` when the
/// backend reports no viewport geometry (headless/embedded, or a platform
/// without window positions).
fn pointer_global(ctx: &egui::Context) -> Option<egui::Pos2> {
    let rect = ctx.input(|i| i.viewport().inner_rect)?;
    let local = ctx.input(|i| i.pointer.latest_pos().or_else(|| i.pointer.interact_pos()))?;
    Some(rect.min + local.to_vec2())
}

/// Whether a release at `global_pos` lands on the main (root) window, i.e.
/// drag-in is possible. Without any window geometry there is nothing to test
/// against, so assume the main window - which keeps drag-in working on
/// backends that don't report viewport rects, including the embedded test
/// backend.
fn pointer_over_main(ctx: &egui::Context, global_pos: Option<egui::Pos2>) -> bool {
    let root_rect = ctx.input(|i| {
        i.raw
            .viewports
            .get(&egui::ViewportId::ROOT)
            .and_then(|v| v.inner_rect)
    });
    match (global_pos, root_rect) {
        (Some(pos), Some(rect)) => rect.contains(pos),
        _ => true,
    }
}

/// Queues the action a released tab should perform, given where it lived and
/// whether it was dropped over the main window.
fn apply_tab_release(state: &mut state::AppState, source: DragTabSource, over_main: bool) {
    let action = match (
        dock::tab_release(state.dock.drag_tab_home(source), over_main),
        source,
    ) {
        (dock::TabRelease::Nothing, _) => return,
        (dock::TabRelease::Float, DragTabSource::Container(kind)) => {
            DockAction::FloatContainer(kind)
        }
        (dock::TabRelease::Float, DragTabSource::Instance(inst)) => DockAction::FloatInstance(inst),
        (dock::TabRelease::Dock, DragTabSource::Container(kind)) => DockAction::DockContainer(kind),
        (dock::TabRelease::Dock, DragTabSource::Instance(inst)) => DockAction::DockInstance(inst),
    };
    state.pending_dock_actions.push(action);
}

/// Which pass is drawing the ghost, so it appears exactly once per frame even
/// though embedded/headless viewports share the root context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhostPass {
    Root,
    Floating,
}

/// Draws the tear-off ghost chip in whichever viewport the pointer is over,
/// plus a highlight around the main window when releasing would dock here.
/// Called once per viewport pass (the root `frame`, and each floating window).
fn draw_drag_ghost(ctx: &egui::Context, state: &state::AppState, pass: GhostPass) {
    let Some(drag) = &state.drag_tab else {
        return;
    };
    if !drag.pulling {
        return;
    }
    // In embedded/headless mode the floating passes share the root context;
    // let the root pass draw so we don't stack two ghosts.
    if pass == GhostPass::Floating && ctx.input(|i| i.raw.viewport_id) == egui::ViewportId::ROOT {
        return;
    }

    let own_rect = ctx.input(|i| i.viewport().inner_rect);
    let local_pos = match (own_rect, drag.global_pos) {
        // Cross-window: only the viewport containing the pointer draws.
        (Some(rect), Some(global)) => {
            if !rect.contains(global) {
                return;
            }
            global - rect.min.to_vec2()
        }
        // No geometry to map: the root pass falls back to its local pointer.
        _ if pass == GhostPass::Root => {
            match ctx.input(|i| i.pointer.latest_pos().or_else(|| i.pointer.interact_pos())) {
                Some(pos) => pos,
                None => return,
            }
        }
        _ => return,
    };

    let over_main = pointer_over_main(ctx, drag.global_pos);
    let release = dock::tab_release(state.dock.drag_tab_home(drag.source), over_main);
    let (hint, color) = match release {
        dock::TabRelease::Dock => (
            "⇲ release to dock into the main window",
            egui::Color32::from_rgb(90, 200, 120),
        ),
        dock::TabRelease::Float => (
            "⇱ release to pop out",
            egui::Color32::from_rgb(150, 170, 210),
        ),
        dock::TabRelease::Nothing => ("⇲ drag over the main window to dock", egui::Color32::GRAY),
    };
    let title = match drag.source {
        DragTabSource::Instance(inst) => state.instance_title(inst),
        DragTabSource::Container(kind) => kind.title().to_string(),
    };

    if release == dock::TabRelease::Dock && pass == GhostPass::Root {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("dock_drop_highlight"),
        ));
        painter.rect_stroke(
            ctx.screen_rect().shrink(2.0),
            egui::Rounding::same(4.0),
            egui::Stroke::new(3.0_f32, color),
        );
    }

    egui::Area::new(egui::Id::new(("tab_drag_ghost", drag.source)))
        .order(egui::Order::Foreground)
        .fixed_pos(local_pos + egui::vec2(10.0, 14.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new(title).strong());
                ui.label(egui::RichText::new(hint).small().color(color));
            });
        });
}

/// The dock host's top-level tab strip: one tab per kind container, each with a
/// "float the whole group" button, plus the always-present "Windows" menu.
fn dock_tab_strip(ui: &mut egui::Ui, state: &mut state::AppState) {
    ui.horizontal_wrapped(|ui| {
        let docked = state.dock.docked.clone();
        for (i, &kind) in docked.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            dragged_tab(
                ui,
                state,
                DragTabSource::Container(kind),
                kind.title().to_string(),
                state.dock.active == Some(kind),
                |state| {
                    state
                        .pending_dock_actions
                        .push(DockAction::ActivateContainer(kind));
                },
            );
            if ui
                .add(egui::Button::new("⇱").small())
                .on_hover_text("Float this whole window group into its own window")
                .clicked()
            {
                state
                    .pending_dock_actions
                    .push(DockAction::FloatContainer(kind));
            }
        }
        ui.separator();
        ui.menu_button("Windows ▾", |ui| window_menu(ui, state));
    });
}

/// The shared body of the dock host's "Windows" menu (and Workspace's "Window"
/// menu): each kind's container placement, every live instance, and a "New"
/// action for the repeatable kinds.
pub fn window_menu(ui: &mut egui::Ui, state: &mut state::AppState) {
    for kind in PanelKind::ALL {
        let container = state.dock.container_placement(kind);
        if container != Placement::Hidden {
            ui.horizontal(|ui| {
                let where_ = if container == Placement::Docked {
                    "tab"
                } else {
                    "window"
                };
                ui.label(format!("{} ({where_})", kind.title()));
                if container == Placement::Docked {
                    if ui.button("Float group").clicked() {
                        state
                            .pending_dock_actions
                            .push(DockAction::FloatContainer(kind));
                        ui.close_menu();
                    }
                } else if ui.button("Dock group").clicked() {
                    state
                        .pending_dock_actions
                        .push(DockAction::DockContainer(kind));
                    ui.close_menu();
                }
            });
        }
        for inst in state.live_instances(kind) {
            let floating = state.dock.instance_is_floating(inst);
            ui.horizontal(|ui| {
                let where_ = if floating { "window" } else { "tab" };
                ui.label(format!("{} ({where_})", state.instance_title(inst)));
                if floating {
                    if ui.button("Dock").clicked() {
                        state
                            .pending_dock_actions
                            .push(DockAction::DockInstance(inst));
                        ui.close_menu();
                    }
                } else if ui.button("Pop out").clicked() {
                    state
                        .pending_dock_actions
                        .push(DockAction::FloatInstance(inst));
                    ui.close_menu();
                }
                if ui.button("Close").clicked() {
                    state
                        .pending_dock_actions
                        .push(DockAction::CloseInstance(inst));
                    ui.close_menu();
                }
            });
        }
        if !kind.instances_are_curve_sets()
            && ui
                .button(format!("＋ New {} window", kind.title()))
                .clicked()
        {
            state
                .pending_dock_actions
                .push(DockAction::NewInstance(kind));
            ui.close_menu();
        }
    }
}

impl Default for App {
    fn default() -> Self {
        App {
            state: state::AppState::new(),
            shots: None,
        }
    }
}

impl App {
    /// Builds the app with the documentation-screenshot driver active: the
    /// demo data is seeded immediately (before the first frame) and `frame`
    /// then walks the capture scenes and exits when done.
    fn with_shots(cfg: shots::Config) -> App {
        let mut app = App::default();
        app.shots = Some(shots::ShotSession::start(&mut app.state, cfg));
        app
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
        // The screenshot driver (when active) is taken out of `self` for the
        // duration of the frame so its hooks don't fight the `&mut self.state`
        // borrows below; it's put back at the end.
        let mut shots = self.shots.take();
        if let Some(s) = shots.as_mut() {
            s.begin_frame(ctx);
        }

        // Dock changes are collected during the frame and applied at the
        // end, so the set of docked/floating containers/instances never
        // changes while they're being drawn.
        let mut actions: Vec<DockAction> = Vec::new();

        egui::TopBottomPanel::top("dock_tab_strip").show(ctx, |ui| {
            dock_tab_strip(ui, &mut self.state);
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.state.dock.active {
            Some(kind) => container_ui(kind, ui, &mut self.state),
            None => {
                ui.label("No windows open - use the \"Windows ▾\" menu above.");
            }
        });

        // Whole containers floated into their own OS windows.
        let mut closed_containers: Vec<PanelKind> = Vec::new();
        for kind in self.state.dock.floating_containers.clone() {
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("Xenovision - {}", kind.title()))
                .with_inner_size([1050.0, 700.0]);
            ctx.show_viewport_immediate(
                dock::container_viewport_id(kind),
                builder,
                |ctx, _class| {
                    egui::TopBottomPanel::top(egui::Id::new(("floating_dock_bar", kind))).show(
                        ctx,
                        |ui| {
                            ui.horizontal(|ui| {
                                dragged_tab(
                                    ui,
                                    &mut self.state,
                                    DragTabSource::Container(kind),
                                    kind.title().to_string(),
                                    true,
                                    |_| {},
                                );
                                if ui
                                    .button("⇲ Dock into main window")
                                    .on_hover_text(
                                        "Merge this whole window group into the main window",
                                    )
                                    .clicked()
                                {
                                    actions.push(DockAction::DockContainer(kind));
                                }
                            });
                        },
                    );
                    egui::CentralPanel::default().show(ctx, |ui| {
                        container_ui(kind, ui, &mut self.state);
                    });
                    draw_drag_ghost(ctx, &self.state, GhostPass::Floating);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        closed_containers.push(kind);
                    }
                },
            );
        }

        // Individual instances popped out into their own OS windows.
        let mut closed_instances: Vec<PanelInstance> = Vec::new();
        for inst in self.state.dock.floating_instances.clone() {
            let title = self.state.instance_title(inst);
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("Xenovision - {title}"))
                .with_inner_size([980.0, 640.0]);
            ctx.show_viewport_immediate(inst.viewport_id(), builder, |ctx, _class| {
                egui::TopBottomPanel::top(egui::Id::new(("floating_dock_bar", inst))).show(
                    ctx,
                    |ui| {
                        ui.horizontal(|ui| {
                            dragged_tab(
                                ui,
                                &mut self.state,
                                DragTabSource::Instance(inst),
                                title.clone(),
                                true,
                                |_| {},
                            );
                            if ui
                                .button("⇲ Dock into main window")
                                .on_hover_text("Merge this window into its group as a tab")
                                .clicked()
                            {
                                actions.push(DockAction::DockInstance(inst));
                            }
                        });
                    },
                );
                egui::CentralPanel::default().show(ctx, |ui| {
                    instance_ui(inst, ui, &mut self.state);
                });
                draw_drag_ghost(ctx, &self.state, GhostPass::Floating);
                if ctx.input(|i| i.viewport().close_requested()) {
                    closed_instances.push(inst);
                }
            });
        }

        // Ghost chip / drop highlight in the main window. Drawn last so it uses
        // the freshest global pointer position the floating windows reported.
        draw_drag_ghost(ctx, &self.state, GhostPass::Root);

        actions.append(&mut self.state.pending_dock_actions);
        for action in actions {
            self.state.dispatch_dock_action(action);
        }
        for inst in closed_instances {
            self.state
                .dispatch_dock_action(DockAction::CloseInstance(inst));
        }
        for kind in closed_containers {
            self.state
                .dispatch_dock_action(DockAction::DockContainer(kind));
        }

        if let Some(s) = shots.as_mut() {
            s.end_frame(ctx, &mut self.state);
        }
        self.shots = shots;
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let shots_cfg = shots::Config::from_args(&args);

    let mut options = eframe::NativeOptions {
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
    if shots_cfg.is_some() {
        // A fixed, roomy window so the documentation captures are
        // deterministic and every panel fits without its scroll wrapper
        // kicking in.
        options.viewport = options.viewport.with_inner_size([1440.0, 900.0]);
    }

    eframe::run_native(
        "Xenovision",
        options,
        Box::new(move |_cc| {
            let app = match shots_cfg {
                Some(cfg) => App::with_shots(cfg),
                None => App::default(),
            };
            Ok(Box::new(app))
        }),
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

    /// One frame fed a specific list of input events (for pointer-gesture
    /// tests), on a roomy 1200x900 screen.
    fn pointer_press(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn pointer_release(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// Drives a tear-off drag: press at `start`, move away in a couple of
    /// frames (so egui registers a drag), then release at `end`.
    fn drag_from_to(app: &mut App, ctx: &egui::Context, start: egui::Pos2, end: egui::Pos2) {
        let mid = start + (end - start) * 0.5;
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(start), pointer_press(start)]),
            |ctx| app.frame(ctx),
        );
        let _ = ctx.run(input(vec![egui::Event::PointerMoved(mid)]), |ctx| {
            app.frame(ctx)
        });
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(end), pointer_release(end)]),
            |ctx| app.frame(ctx),
        );
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
            assert_eq!(app.state.dock.active, Some(kind));
            assert_eq!(
                app.state.dock.container_placement(kind),
                Placement::Docked,
                "opening an instance shows the kind's container tab"
            );
        }

        // Pop the Image Display instance tab out into its own OS window; with
        // no in-container instance left, its container tab disappears.
        let img = PanelId::singleton(PanelKind::ImageDisplay);
        app.state
            .dispatch_dock_action(DockAction::FloatInstance(PanelInstance::Panel(img)));
        assert!(app
            .state
            .dock
            .instance_is_floating(PanelInstance::Panel(img)));
        run_frames(&mut app, &ctx, 1200.0, 900.0, 3);
        assert_eq!(
            app.state.dock.container_placement(PanelKind::ImageDisplay),
            Placement::Hidden
        );

        // Float the whole Workspace container (it still has in-container Curve
        // Sets); the other containers stay docked.
        app.state
            .dispatch_dock_action(DockAction::FloatContainer(PanelKind::Workspace));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Floating
        );
        assert!(app.state.dock.docked.contains(&PanelKind::Comparison));
        run_frames(&mut app, &ctx, 1200.0, 900.0, 3);

        // Requests queued from inside a panel are applied at frame end.
        app.state
            .pending_dock_actions
            .push(DockAction::DockContainer(PanelKind::Workspace));
        app.state
            .pending_dock_actions
            .push(DockAction::DockInstance(PanelInstance::Panel(img)));
        run_frames(&mut app, &ctx, 1200.0, 900.0, 1);
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Docked
        );
        assert_eq!(
            app.state.dock.container_placement(PanelKind::ImageDisplay),
            Placement::Docked,
            "docking the popped-out instance restores its container"
        );
    }

    /// The repeatable kind: two Image Display panels can be open at the same
    /// time (here one in-container and one popped out, each in its own OS
    /// window) with independent view lists, reading the one shared image
    /// library.
    #[test]
    fn two_image_display_panels_open_simultaneously() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let a = app.state.new_image_display(false);
        let b = app.state.new_image_display(true);
        assert_ne!(a, b);
        run_frames(&mut app, &ctx, 1000.0, 700.0, 3);
        assert!(!app.state.dock.instance_is_floating(PanelInstance::Panel(a)));
        assert!(app.state.dock.instance_is_floating(PanelInstance::Panel(b)));
        assert_eq!(
            app.state.live_instances(PanelKind::ImageDisplay),
            vec![PanelInstance::Panel(a), PanelInstance::Panel(b)],
            "both instances share the one Image Display container"
        );
        assert_ne!(
            a.viewport_id(),
            b.viewport_id(),
            "each instance needs its own OS viewport id"
        );
        assert!(app.state.image_displays.contains_key(&a));
        assert!(app.state.image_displays.contains_key(&b));
    }

    /// The Workspace's Curve Set tabs can pop out into their own OS windows
    /// just like any repeatable instance, and dock back into the container.
    #[test]
    fn workspace_curve_set_tab_pops_out_and_redocks() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let set_a = app.state.workspace.open_tabs[0];
        let set_b = app.state.workspace.open_tabs[1];
        let inst_a = PanelInstance::Workspace(set_a);
        assert_ne!(set_a, set_b);

        // Pop the first Curve Set out into its own OS window.
        app.state
            .dispatch_dock_action(DockAction::FloatInstance(inst_a));
        assert!(app.state.dock.instance_is_floating(inst_a));
        // The Workspace container stays because other sets remain in-container.
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Docked
        );
        assert_eq!(
            app.state.live_instances(PanelKind::Workspace).len(),
            app.state.workspace.open_tabs.len(),
            "popping out does not remove the Curve Set from the kind's instances"
        );
        run_frames(&mut app, &ctx, 1000.0, 700.0, 3);

        // Dock it back; the container is unaffected.
        app.state
            .dispatch_dock_action(DockAction::DockInstance(inst_a));
        assert!(!app.state.dock.instance_is_floating(inst_a));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Docked
        );
        assert!(app.state.workspace.open_tabs.contains(&set_a));
    }

    /// Popping out the last in-container Curve Set hides the whole Workspace
    /// container tab; docking it back restores the container.
    #[test]
    fn container_hides_when_last_instance_pops_out() {
        let mut app = App::default();
        while app.state.workspace.open_tabs.len() > 1 {
            let set = app.state.workspace.open_tabs[0];
            app.state.close_curve_set(set);
        }
        let set = app.state.workspace.open_tabs[0];
        let inst = PanelInstance::Workspace(set);

        app.state
            .dispatch_dock_action(DockAction::FloatInstance(inst));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Hidden,
            "no in-container Curve Set left, so no Workspace tab"
        );

        app.state
            .dispatch_dock_action(DockAction::DockInstance(inst));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Docked
        );
    }

    /// Closing a clean popped-out Curve Set's window discards it; closing a
    /// dirty one brings it back into the container for the unsaved-changes
    /// prompt instead of discarding silently.
    #[test]
    fn closing_popped_out_curve_set_discards_or_brings_back_dirty() {
        let mut app = App::default();
        let set_id = app.state.workspace.open_tabs[0];
        let inst = PanelInstance::Workspace(set_id);
        app.state
            .dispatch_dock_action(DockAction::FloatInstance(inst));

        let n = app.state.workspace.open_tabs.len();
        app.state
            .dispatch_dock_action(DockAction::CloseInstance(inst));
        assert!(
            !app.state.workspace.open_tabs.contains(&set_id),
            "a clean Curve Set is closed, not parked"
        );
        assert_eq!(app.state.workspace.open_tabs.len(), n - 1);
        assert!(!app.state.curve_sets.contains_key(&set_id));

        // Now a dirty one: closing its popped-out window docks it back and
        // queues the unsaved-changes confirmation, and brings the Workspace
        // container forward so the prompt is visible.
        app.state.new_panel(PanelKind::Comparison, false);
        assert_eq!(app.state.dock.active, Some(PanelKind::Comparison));
        let set2 = app.state.workspace.open_tabs[0];
        app.state.curve_sets.get_mut(&set2).unwrap().dirty = true;
        let inst2 = PanelInstance::Workspace(set2);
        app.state
            .dispatch_dock_action(DockAction::FloatInstance(inst2));
        app.state
            .dispatch_dock_action(DockAction::CloseInstance(inst2));
        assert!(
            app.state.workspace.open_tabs.contains(&set2),
            "a dirty Curve Set is brought back into the container"
        );
        assert!(!app.state.dock.instance_is_floating(inst2));
        assert_eq!(app.state.workspace.pending_close, Some(set2));
        assert!(app.state.curve_sets.contains_key(&set2));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Docked
        );
        assert_eq!(
            app.state.dock.active,
            Some(PanelKind::Workspace),
            "the Workspace container comes forward so the prompt is visible"
        );
    }

    fn tab_rect(source: DragTabSource) -> egui::Rect {
        TAB_RECTS.with(|rects| {
            *rects
                .borrow()
                .get(&source)
                .expect("that tab should have been drawn")
        })
    }

    /// The pull-out threshold is distance-from-press based.
    #[test]
    fn pull_out_threshold_is_distance_based() {
        let p = egui::pos2(100.0, 100.0);
        assert!(!dock::is_pulled_out(p, egui::pos2(105.0, 105.0)));
        assert!(dock::is_pulled_out(p, egui::pos2(160.0, 100.0)));
        assert!(dock::is_pulled_out(p, egui::pos2(100.0, 140.0)));
    }

    /// Dragging a container tab out of the dock strip floats the whole
    /// container into its own OS window.
    #[test]
    fn dragging_a_container_tab_out_floats_the_whole_container() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        let rect = tab_rect(DragTabSource::Container(PanelKind::Workspace));
        drag_from_to(
            &mut app,
            &ctx,
            rect.center(),
            rect.center() + egui::vec2(400.0, 300.0),
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        assert!(app.state.drag_tab.is_none(), "the gesture ends cleanly");
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Floating
        );
    }

    /// Dragging an instance tab out pops that instance into its own OS window;
    /// with no in-container instance left, its container tab disappears.
    #[test]
    fn dragging_an_instance_tab_out_pops_it_into_its_own_window() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let panel = app.state.new_image_display(false);
        let inst = PanelInstance::Panel(panel);
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        let rect = tab_rect(DragTabSource::Instance(inst));
        drag_from_to(
            &mut app,
            &ctx,
            rect.center(),
            rect.center() + egui::vec2(360.0, 260.0),
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        assert!(app.state.dock.instance_is_floating(inst));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::ImageDisplay),
            Placement::Hidden
        );
    }

    /// Dragging a Workspace Curve Set tab out pops just that set out, leaving
    /// the Workspace container (and its other sets) in place.
    #[test]
    fn dragging_a_workspace_curve_set_tab_pops_it_out() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let set_id = app.state.workspace.open_tabs[0];
        let inst = PanelInstance::Workspace(set_id);
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        let rect = tab_rect(DragTabSource::Instance(inst));
        drag_from_to(
            &mut app,
            &ctx,
            rect.center(),
            rect.center() + egui::vec2(360.0, 240.0),
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        assert!(app.state.dock.instance_is_floating(inst));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Docked
        );
    }

    /// Dragging a floating window's title tab back over the main window
    /// re-docks it into its group - the inverse of the tear-off gesture.
    #[test]
    fn dragging_a_floating_instance_title_back_docks_it() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let panel = app.state.new_image_display(false);
        let inst = PanelInstance::Panel(panel);
        app.state
            .dispatch_dock_action(DockAction::FloatInstance(inst));
        assert!(app.state.dock.instance_is_floating(inst));
        run_frames(&mut app, &ctx, 1200.0, 900.0, 3);
        let rect = tab_rect(DragTabSource::Instance(inst));
        drag_from_to(
            &mut app,
            &ctx,
            rect.center(),
            rect.center() + egui::vec2(360.0, 260.0),
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        assert!(app.state.drag_tab.is_none(), "the gesture ends cleanly");
        assert!(
            !app.state.dock.instance_is_floating(inst),
            "dropping the floating title over the main window re-docks it"
        );
        assert_eq!(
            app.state.dock.container_placement(PanelKind::ImageDisplay),
            Placement::Docked
        );
    }

    /// Dragging a floating whole-container title back over the main window
    /// re-docks the group.
    #[test]
    fn dragging_a_floating_container_title_back_docks_it() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        app.state
            .dispatch_dock_action(DockAction::FloatContainer(PanelKind::Workspace));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Floating
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 3);
        let rect = tab_rect(DragTabSource::Container(PanelKind::Workspace));
        drag_from_to(
            &mut app,
            &ctx,
            rect.center(),
            rect.center() + egui::vec2(360.0, 260.0),
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        assert_eq!(
            app.state.dock.container_placement(PanelKind::Workspace),
            Placement::Docked
        );
    }

    /// Runs a frame whose pointer events are in a coordinate space where the
    /// main window is only a tiny box, mimicking a real multi-window setup in
    /// which the pointer can be away from the main window.
    fn drag_frame_with_tiny_main(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        input.viewports = std::iter::once((
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                inner_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(10.0, 10.0),
                )),
                ..Default::default()
            },
        ))
        .collect();
        let _ = ctx.run(input, |ctx| app.frame(ctx));
    }

    /// A floating window's title tab dropped *away* from the main window stays
    /// floating (it has nowhere to dock to).
    #[test]
    fn dragging_a_floating_title_away_from_main_leaves_it_floating() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let panel = app.state.new_image_display(false);
        let inst = PanelInstance::Panel(panel);
        app.state
            .dispatch_dock_action(DockAction::FloatInstance(inst));
        run_frames(&mut app, &ctx, 1200.0, 900.0, 3);
        let rect = tab_rect(DragTabSource::Instance(inst));
        let start = rect.center();
        let end = start + egui::vec2(360.0, 260.0);
        let mid = start + (end - start) * 0.5;
        drag_frame_with_tiny_main(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(start), pointer_press(start)],
        );
        drag_frame_with_tiny_main(&mut app, &ctx, vec![egui::Event::PointerMoved(mid)]);
        drag_frame_with_tiny_main(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(end), pointer_release(end)],
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        assert!(
            app.state.dock.instance_is_floating(inst),
            "released away from the main window, so it stays floating"
        );
    }

    /// Releasing a tab back inside its strip (only a small nudge) selects it
    /// rather than tearing it out.
    #[test]
    fn releasing_a_tab_back_in_its_strip_does_not_float_it() {
        let mut app = App::default();
        let ctx = egui::Context::default();
        let panel = app.state.new_image_display(false);
        let inst = PanelInstance::Panel(panel);
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        let rect = tab_rect(DragTabSource::Instance(inst));
        drag_from_to(
            &mut app,
            &ctx,
            rect.center(),
            rect.center() + egui::vec2(6.0, 2.0),
        );
        run_frames(&mut app, &ctx, 1200.0, 900.0, 2);
        assert!(!app.state.dock.instance_is_floating(inst));
        assert_eq!(
            app.state.dock.container_placement(PanelKind::ImageDisplay),
            Placement::Docked
        );
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
        app.state.with_comparison(a, |s| {
            assert_eq!(s.comparison.selected_species, vec![set_a])
        });
        app.state.with_comparison(b, |s| {
            assert_eq!(s.comparison.selected_species, vec![set_b])
        });

        // Drawing the panels (state swapped in per instance) must not disturb
        // either instance's own state.
        run_frames(&mut app, &ctx, 1000.0, 700.0, 2);
        app.state.with_comparison(a, |s| {
            assert_eq!(s.comparison.selected_species, vec![set_a])
        });
        app.state.with_comparison(b, |s| {
            assert_eq!(s.comparison.selected_species, vec![set_b])
        });

        // Closing b parks b's setup; a keeps its own.
        app.state
            .dispatch_dock_action(DockAction::CloseInstance(PanelInstance::Panel(b)));
        assert!(!app.state.comparisons.contains_key(&b));
        assert!(app.state.comparisons.contains_key(&a));
        assert_eq!(app.state.comparison.selected_species, vec![set_b]);

        // Close a too: last-closed wins, so a newly opened Comparison inherits
        // a's setup; a second window opened while one is already open starts
        // blank.
        app.state
            .dispatch_dock_action(DockAction::CloseInstance(PanelInstance::Panel(a)));
        let c = app.state.new_panel(PanelKind::Comparison, false);
        app.state.with_comparison(c, |s| {
            assert_eq!(s.comparison.selected_species, vec![set_a])
        });
        let d = app.state.new_panel(PanelKind::Comparison, true);
        app.state
            .with_comparison(d, |s| assert!(s.comparison.selected_species.is_empty()));

        // Stimulus Editor behaves the same way, reusing the freed instance
        // number when it is the only window of its kind.
        let stim_a = app.state.stimulus_order[0];
        let e1 = app.state.new_panel(PanelKind::StimulusEditor, false);
        app.state
            .with_stimulus_editor(e1, |s| s.stimulus_editor.selected = Some(stim_a));
        app.state
            .dispatch_dock_action(DockAction::CloseInstance(PanelInstance::Panel(e1)));
        let e2 = app.state.new_panel(PanelKind::StimulusEditor, false);
        assert_eq!(e2, e1, "reopening reuses the freed instance number");
        app.state
            .with_stimulus_editor(e2, |s| assert_eq!(s.stimulus_editor.selected, Some(stim_a)));
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
