//! **Session management chrome** (TMUX-FC-2).
//!
//! The session / window / pane sidebar tree, the full session ops (create ·
//! attach · detach · kill · rename), and the all-sessions picker — all **glue
//! over TMUX-FC-1's** [`TmuxController`] (`crate::tmux`).
//!
//! Design: `docs/design/tmux-first-class.md` (lock #4 chrome, #5 sessions). The
//! discipline the design's risk section demands is preserved verbatim here: a GUI
//! op **never mutates the tree directly** — it emits a [`TmuxIntent`], which
//! [`command_for`] turns into the exact `tmux` command line
//! ([`crate::tmux::commands`]), the controller writes it, and the resulting
//! `%`-event reconciles [`crate::tmux::TmuxModel`]. The next frame's tree render
//! is the round-trip's visible half.
//!
//! What this module carries:
//! * [`TmuxIntent`] + [`command_for`] — the pure GUI-intent → tmux-command map
//!   (the one place a chrome click becomes a command; unit-tested).
//! * [`render_tree`] — the sessions→windows→panes sidebar over a live
//!   [`TmuxModel`], with the session op affordances; emits intents.
//! * [`render_picker_contents`] — the picker listing **all** sessions (attached
//!   AND detached, from [`TmuxController::all_sessions`]) so a detached session
//!   can be re-attached.
//! * [`TmuxChrome`] — the surface-held state: the optional live controller (tmux
//!   is opt-in, lock #16 — no auto-attach) + the UI-only bits, wiring pump →
//!   render → dispatch. [`crate::panel`] holds one and mounts its [`TmuxChrome::sidebar`].
//! * [`TmuxMenuChoice`] — the top-menu-bar (`crate::menubar`) tmux entries route
//!   OUT to the surface, which owns the controller the menu toggles.

use mde_egui::egui::{self, Button, RichText, ScrollArea, Ui};
use mde_egui::Style;

use crate::tmux::{commands, SessionInfo, Status, TmuxController, TmuxLaunch, TmuxModel};

/// One GUI intent from the chrome — every session/window/pane op the tree or the
/// picker can raise. Owns its strings so it outlives the render borrow.
///
/// Each maps through [`command_for`] to a real `tmux` command line (the two
/// out-of-band ones — [`Self::StartClient`], [`Self::RefreshSessions`] — are
/// handled by [`TmuxChrome::dispatch`] since they act on the channel itself, not
/// as a command). Never a direct tree mutation (the design's round-trip rule).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TmuxIntent {
    /// Start a `tmux -CC` control client (attach `main`, creating it if absent).
    StartClient,
    /// Create + switch to a new named session (`new-session -s`).
    NewSession(String),
    /// Re-attach a (detached) session onto this client (`switch-client -t`).
    AttachSession(String),
    /// Detach this control client (`detach-client`); the session keeps running.
    Detach,
    /// Kill a session by name (`kill-session -t`).
    KillSession(String),
    /// Rename a session (`rename-session -t <target> <new>`).
    RenameSession(String, String),
    /// Make a window the active one (`select-window -t @`).
    SelectWindow(u32),
    /// Make a pane the active one (`select-pane -t %`).
    SelectPane(u32),
    /// Re-request the full session enumeration (the picker's refresh).
    RefreshSessions,
}

/// Turn a [`TmuxIntent`] into the exact `tmux` command line, or [`None`] for the
/// two intents [`TmuxChrome::dispatch`] handles on the channel directly.
#[must_use]
pub fn command_for(intent: &TmuxIntent) -> Option<String> {
    match intent {
        TmuxIntent::NewSession(name) => Some(commands::new_session(name)),
        TmuxIntent::AttachSession(name) => Some(commands::attach_session(name)),
        TmuxIntent::Detach => Some(commands::detach_client()),
        TmuxIntent::KillSession(name) => Some(commands::kill_session(name)),
        TmuxIntent::RenameSession(target, name) => Some(commands::rename_session(target, name)),
        TmuxIntent::SelectWindow(w) => Some(commands::select_window(*w)),
        TmuxIntent::SelectPane(p) => Some(commands::select_pane(*p)),
        TmuxIntent::StartClient | TmuxIntent::RefreshSessions => None,
    }
}

/// A tmux top-menu choice (`crate::menubar`) the surface applies.
///
/// These toggle the surface-held [`TmuxChrome`] (which owns the optional live
/// controller), so they route OUT of the bar rather than into its `apply` (which
/// only touches the [`crate::TabbedTerminal`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TmuxMenuChoice {
    /// Start a tmux control client and reveal the tree.
    NewSession,
    /// Open the all-sessions picker (starting a client first if needed).
    ShowPicker,
    /// Detach the control client.
    Detach,
    /// Show/hide the sidebar tree.
    ToggleTree,
}

/// An in-progress inline rename (only the session target is renamed by FC-2; the
/// window/pane title rename is TMUX-FC-3's job).
#[derive(Clone, PartialEq, Eq, Debug)]
struct SessionRename {
    /// The session name being renamed (the `rename-session` target).
    target: String,
    /// The edit buffer.
    buffer: String,
}

/// The UI-only state of the chrome (everything that is NOT the live controller).
#[derive(Default)]
struct ChromeUi {
    /// Whether the sidebar tree is mounted.
    tree_open: bool,
    /// Whether the all-sessions picker window is open.
    picker_open: bool,
    /// Whether the inline "new session" name field is revealed.
    new_open: bool,
    /// The new-session name buffer.
    new_name: String,
    /// The in-progress session rename, if any.
    rename: Option<SessionRename>,
}

/// The surface-held tmux chrome.
///
/// The optional live [`TmuxController`] (tmux is **opt-in**, lock #16 — nothing
/// attaches until the user asks) plus the UI-only state, wiring the per-frame
/// pump → render → dispatch cycle.
#[derive(Default)]
pub struct TmuxChrome {
    /// The live control connection, once the user starts one (`None` = no tmux).
    controller: Option<TmuxController>,
    /// The UI-only state.
    ui: ChromeUi,
}

impl TmuxChrome {
    /// A fresh chrome with no tmux session (the sidebar hidden).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drain the control channel into the model — call once per frame, before the
    /// tree renders (the [`crate::panel::terminal_pump`] slot).
    pub fn pump(&mut self) {
        if let Some(ctrl) = self.controller.as_mut() {
            ctrl.pump();
        }
    }

    /// Whether a live control channel is up (attached or attaching) — the gate for
    /// the tree, the session ops, and the context-sensitive menu items.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.controller
            .as_ref()
            .is_some_and(|c| matches!(c.status(), Status::Connecting | Status::Attached))
    }

    /// Whether the sidebar tree is currently mounted (the menu's toggle state).
    #[must_use]
    pub const fn tree_open(&self) -> bool {
        self.ui.tree_open
    }

    /// Apply a top-menu choice (`crate::menubar`), if one was made this frame.
    pub fn apply_menu(&mut self, choice: Option<TmuxMenuChoice>) {
        match choice {
            Some(TmuxMenuChoice::NewSession) => {
                self.ensure_client();
                self.ui.tree_open = true;
            }
            Some(TmuxMenuChoice::ShowPicker) => {
                self.ensure_client();
                self.ui.tree_open = true;
                self.ui.picker_open = true;
                self.refresh_sessions();
            }
            Some(TmuxMenuChoice::Detach) => {
                if let Some(ctrl) = self.controller.as_ref() {
                    let _ = ctrl.send(&commands::detach_client());
                }
            }
            Some(TmuxMenuChoice::ToggleTree) => self.ui.tree_open = !self.ui.tree_open,
            None => {}
        }
    }

    /// Mount the sidebar tree (a left [`egui::SidePanel`]) + the picker window,
    /// dispatching whatever ops the user raised this frame. A no-op when the tree
    /// is hidden, so tmux truly costs nothing until opened.
    pub fn sidebar(&mut self, ui: &mut Ui) {
        if !self.ui.tree_open {
            return;
        }
        let mut intents: Vec<TmuxIntent> = Vec::new();
        {
            let Self {
                controller,
                ui: state,
            } = self;
            let ctrl = controller.as_ref();
            egui::SidePanel::left("tmux-tree")
                .resizable(true)
                .default_width(Style::SP_XL * 6.0)
                .show_inside(ui, |ui| {
                    render_tree(ui, ctrl, state, &mut intents);
                });
            if state.picker_open {
                render_picker(ui, ctrl, state, &mut intents);
            }
        }
        self.dispatch(intents);
    }

    /// Start a control client if none is live (idempotent) — the opt-in attach.
    fn ensure_client(&mut self) {
        if !self.is_active() {
            self.controller = Some(TmuxController::connect(&TmuxLaunch::default()));
        }
    }

    /// Ask the server for the full session list (feeds the picker).
    fn refresh_sessions(&self) {
        if let Some(ctrl) = self.controller.as_ref() {
            let _ = ctrl.request_sessions();
        }
    }

    /// Issue each raised intent as its tmux command (the round-trip's first half);
    /// the two out-of-band intents act on the channel directly.
    fn dispatch(&mut self, intents: Vec<TmuxIntent>) {
        for intent in intents {
            match &intent {
                TmuxIntent::StartClient => {
                    self.ensure_client();
                    self.ui.tree_open = true;
                }
                TmuxIntent::RefreshSessions => self.refresh_sessions(),
                other => {
                    if let (Some(cmd), Some(ctrl)) = (command_for(other), self.controller.as_ref())
                    {
                        let _ = ctrl.send(&cmd);
                    }
                }
            }
        }
    }
}

/// Render the sidebar content: the session op toolbar + the current session's
/// windows→panes tree (control mode streams the *attached* session's detail).
/// Emits [`TmuxIntent`]s; never mutates the model.
fn render_tree(
    ui: &mut Ui,
    controller: Option<&TmuxController>,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
) {
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("TMUX")
                .size(Style::SMALL)
                .color(Style::ACCENT_TERMINALS)
                .strong(),
        );
    });
    ui.add_space(Style::SP_XS);

    let Some(ctrl) =
        controller.filter(|c| matches!(c.status(), Status::Connecting | Status::Attached))
    else {
        no_session(ui, controller, intents);
        return;
    };

    let model = ctrl.model();
    // The session op row (create · picker · detach) — the always-available ops.
    ui.horizontal_wrapped(|ui| {
        if ui.button("+ Session").clicked() {
            state.new_open = !state.new_open;
        }
        if ui.button("Sessions\u{2026}").clicked() {
            state.picker_open = true;
            intents.push(TmuxIntent::RefreshSessions);
        }
        if ui.button("Detach").clicked() {
            intents.push(TmuxIntent::Detach);
        }
    });

    // The inline "new session" name field (create op).
    if state.new_open {
        ui.horizontal(|ui| {
            let resp = ui.text_edit_singleline(&mut state.new_name);
            let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Create").clicked() || submit) && !state.new_name.trim().is_empty() {
                intents.push(TmuxIntent::NewSession(state.new_name.trim().to_owned()));
                state.new_name.clear();
                state.new_open = false;
            }
        });
    }

    ui.add_space(Style::SP_XS);
    ui.separator();

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            session_node(ui, model, state, intents);
        });
}

/// The honest "no tmux session" state — a single start affordance (the opt-in
/// attach), plus the last channel's exit reason if it just detached/died (§7).
fn no_session(ui: &mut Ui, controller: Option<&TmuxController>, intents: &mut Vec<TmuxIntent>) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("No tmux session")
            .size(Style::BODY)
            .color(Style::TEXT_DIM),
    );
    if let Some(Status::Exited(reason)) = controller.map(TmuxController::status) {
        if !reason.is_empty() {
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(reason.as_str())
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        }
    }
    ui.add_space(Style::SP_S);
    if ui.button("New tmux session").clicked() {
        intents.push(TmuxIntent::StartClient);
    }
}

/// The current session header (with the inline rename + kill) and its windows.
fn session_node(
    ui: &mut Ui,
    model: &TmuxModel,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
) {
    let Some(sid) = model.current_session() else {
        return;
    };
    let name = model
        .session(sid)
        .map_or("", crate::tmux::TmuxSession::name)
        .to_owned();

    // The session row: name + rename + kill. A rename in progress swaps in an edit.
    let renaming_this = state.rename.as_ref().is_some_and(|r| r.target == name);
    if renaming_this {
        ui.horizontal(|ui| {
            let (submit, cancel) = {
                let rename = state.rename.as_mut().expect("renaming_this");
                let resp = ui.text_edit_singleline(&mut rename.buffer);
                let submit = ui.button("Rename").clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                let cancel = ui.button("Cancel").clicked();
                (submit, cancel)
            };
            if submit {
                if let Some(rename) = state.rename.take() {
                    let new = rename.buffer.trim();
                    if !new.is_empty() {
                        intents.push(TmuxIntent::RenameSession(rename.target, new.to_owned()));
                    }
                }
            } else if cancel {
                state.rename = None;
            }
        });
    } else {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("\u{25CF} {name}"))
                    .size(Style::BODY)
                    .color(Style::TEXT)
                    .strong(),
            );
            if icon_button(ui, "rename").clicked() {
                state.rename = Some(SessionRename {
                    target: name.clone(),
                    buffer: name.clone(),
                });
            }
            if icon_button(ui, "kill").clicked() {
                intents.push(TmuxIntent::KillSession(name.clone()));
            }
        });
    }

    // The windows of the attached session → each expandable to its panes.
    for window in model.windows_in_order() {
        window_node(ui, model, window, intents);
    }
}

/// One window row + its panes. Clicking selects (round-trips `select-window` /
/// `select-pane`); FC-3 owns the split/close/resize mutations.
fn window_node(ui: &mut Ui, model: &TmuxModel, window: u32, intents: &mut Vec<TmuxIntent>) {
    let Some(win) = model.window(window) else {
        return;
    };
    let active_pane = win.active_pane();
    let label = format!("@{window}  {}", win.name());
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_M);
        if ui
            .selectable_label(false, RichText::new(label).color(Style::TEXT))
            .clicked()
        {
            intents.push(TmuxIntent::SelectWindow(window));
        }
    });
    for pane in model.panes_of_window(window) {
        let title = model.pane(pane).map_or("", crate::tmux::TmuxPane::title);
        let is_active = active_pane == Some(pane);
        let text = if title.is_empty() {
            format!("%{pane}")
        } else {
            format!("%{pane}  {title}")
        };
        let color = if is_active {
            Style::ACCENT
        } else {
            Style::TEXT_DIM
        };
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_XL);
            if ui
                .selectable_label(
                    is_active,
                    RichText::new(text).size(Style::SMALL).color(color),
                )
                .clicked()
            {
                intents.push(TmuxIntent::SelectPane(pane));
            }
        });
    }
}

/// The all-sessions picker window (attached AND detached) — a detached row offers
/// **Attach** (re-attach it), any row offers **Kill**. Its source is the
/// control-channel `list-sessions` reply ([`TmuxController::all_sessions`]).
fn render_picker(
    ui: &Ui,
    controller: Option<&TmuxController>,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
) {
    let mut open = true;
    egui::Window::new("tmux sessions")
        .collapsible(false)
        .resizable(true)
        .show(ui.ctx(), |ui| {
            open = !render_picker_contents(ui, controller, intents);
        });
    if !open {
        state.picker_open = false;
    }
}

/// The picker's body (its own fn so it mounts headlessly). Returns `true` when the
/// user asked to close it.
fn render_picker_contents(
    ui: &mut Ui,
    controller: Option<&TmuxController>,
    intents: &mut Vec<TmuxIntent>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("All sessions")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM)
                .strong(),
        );
        if ui.button("Refresh").clicked() {
            intents.push(TmuxIntent::RefreshSessions);
        }
    });
    ui.add_space(Style::SP_XS);

    let sessions = controller.map_or(&[][..], TmuxController::all_sessions);
    if sessions.is_empty() {
        ui.label(
            RichText::new("No sessions reported yet")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    }
    for info in sessions {
        session_row(ui, info, intents);
    }

    ui.add_space(Style::SP_S);
    ui.separator();
    ui.button("Close").clicked()
}

/// One picker row: name, an attached/detached chip, and the Attach/Kill ops.
fn session_row(ui: &mut Ui, info: &SessionInfo, intents: &mut Vec<TmuxIntent>) {
    ui.horizontal(|ui| {
        let (chip, color) = if info.attached {
            ("attached", Style::OK)
        } else {
            ("detached", Style::TEXT_DIM)
        };
        ui.label(
            RichText::new(info.name.as_str())
                .size(Style::BODY)
                .color(Style::TEXT),
        );
        ui.label(RichText::new(chip).size(Style::SMALL).color(color));
        ui.label(
            RichText::new(format!("{} win", info.windows))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        // A detached session can be (re-)attached; the current one already is.
        if !info.attached && ui.button("Attach").clicked() {
            intents.push(TmuxIntent::AttachSession(info.name.clone()));
        }
        if ui.button("Kill").clicked() {
            intents.push(TmuxIntent::KillSession(info.name.clone()));
        }
    });
}

/// A small frameless text button for the row-inline ops (rename/kill), so the
/// tree rows stay compact and read as chrome rather than form buttons.
fn icon_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        Button::new(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        )
        .frame(false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{Parser, TmuxModel};
    use mde_egui::egui::{pos2, vec2, Context, Rect};

    // ── the intent → command map (the round-trip's first half) ───────────────

    #[test]
    fn command_for_maps_each_op_to_its_tmux_line() {
        assert_eq!(
            command_for(&TmuxIntent::NewSession("dev".to_owned())).as_deref(),
            Some("new-session -s 'dev'")
        );
        assert_eq!(
            command_for(&TmuxIntent::AttachSession("dev".to_owned())).as_deref(),
            Some("switch-client -t 'dev'")
        );
        assert_eq!(
            command_for(&TmuxIntent::Detach).as_deref(),
            Some("detach-client")
        );
        assert_eq!(
            command_for(&TmuxIntent::KillSession("dev".to_owned())).as_deref(),
            Some("kill-session -t 'dev'")
        );
        assert_eq!(
            command_for(&TmuxIntent::RenameSession(
                "dev".to_owned(),
                "prod".to_owned()
            ))
            .as_deref(),
            Some("rename-session -t 'dev' 'prod'")
        );
        assert_eq!(
            command_for(&TmuxIntent::SelectWindow(2)).as_deref(),
            Some("select-window -t @2")
        );
        assert_eq!(
            command_for(&TmuxIntent::SelectPane(5)).as_deref(),
            Some("select-pane -t %5")
        );
        // The two channel-level intents carry no command line (dispatch handles them).
        assert_eq!(command_for(&TmuxIntent::StartClient), None);
        assert_eq!(command_for(&TmuxIntent::RefreshSessions), None);
    }

    fn model_from(stream: &[u8]) -> TmuxModel {
        let mut model = TmuxModel::new();
        let mut parser = Parser::new();
        for note in parser.feed(stream) {
            model.apply(note);
        }
        model
    }

    fn headless(mut add: impl FnMut(&mut Ui)) -> usize {
        let ctx = Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| add(ui));
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    // The render smoke tests assert the tessellate path completes without a
    // layout/paint panic (a non-empty primitive set), the same idiom the sibling
    // `panel`/`widget` mount tests use — egui batches shapes into one clipped
    // primitive per clip/texture, so a *count* can't prove "more content".

    // ── the tree folds a fixture model (the acceptance's "tree from a model") ─

    #[test]
    fn tree_renders_a_fixture_session_headless() {
        // A session with one window (two panes) — the live control-mode shape.
        let model = model_from(
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n\
              %window-renamed @0 editor\n\
              %window-pane-changed @0 %1\n",
        );
        assert_eq!(model.current_session(), Some(0));
        assert_eq!(model.panes_of_window(0), vec![1, 2]);

        let prims = headless(|ui| {
            let mut state = ChromeUi::default();
            let mut intents = Vec::new();
            // No controller (no live channel), but the tree render reads the model
            // directly — drive the session-node folder over the fixture.
            session_node(ui, &model, &mut state, &mut intents);
        });
        assert!(prims > 0, "the fixture session tree did not tessellate");
    }

    #[test]
    fn no_session_state_offers_a_start_affordance_headless() {
        // The empty state raises `StartClient` when its one button is "clicked" —
        // exercised here by rendering; the button existing proves the opt-in path.
        let prims = headless(|ui| {
            let mut intents = Vec::new();
            no_session(ui, None, &mut intents);
        });
        assert!(prims > 0, "the empty tmux state did not tessellate");
    }

    // ── the picker lists attached + detached (the acceptance's picker) ───────

    #[test]
    fn picker_lists_attached_and_detached_rows_headless() {
        let attached = SessionInfo {
            name: "main".to_owned(),
            attached: true,
            windows: 3,
        };
        let detached = SessionInfo {
            name: "build".to_owned(),
            attached: false,
            windows: 1,
        };
        // The picker source is the controller's all_sessions; here render the rows
        // directly (the same fn the picker body calls) to prove both shapes paint.
        let mut intents = Vec::new();
        let prims = headless(|ui| {
            session_row(ui, &attached, &mut intents);
            session_row(ui, &detached, &mut intents);
        });
        assert!(prims > 0, "the picker rows did not tessellate");
    }

    // ── the chrome is opt-in: nothing attaches until asked (lock #16) ────────

    #[test]
    fn a_fresh_chrome_is_inactive_and_the_hidden_tree_is_a_no_op() {
        let mut chrome = TmuxChrome::new();
        assert!(
            !chrome.is_active(),
            "a fresh chrome must not auto-attach tmux"
        );
        assert!(!chrome.tree_open(), "the tree starts hidden (opt-in)");
        // Rendering the hidden tree is a no-op that attaches nothing — the surface
        // pays nothing for tmux until the user opts in.
        let _ = headless(|ui| chrome.sidebar(ui));
        assert!(
            !chrome.is_active(),
            "rendering a hidden tree must not have attached anything"
        );
        assert!(!chrome.tree_open(), "a hidden tree stays hidden");
    }
}
