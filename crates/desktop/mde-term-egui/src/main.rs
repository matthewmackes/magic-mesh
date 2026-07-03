//! `mde-term-egui` binary — the Terminal surface on the shared harness.
//!
//! TERM-4/5: the window mounts a [`TabbedTerminal`] — a Carbon-token tab bar
//! over Terminator's arbitrarily nested H/V split tree, one real login shell
//! per pane. Each tab owns its own split tree; switching tabs preserves each
//! tab's whole layout + live shells. The update loop consumes the tab chords
//! (`Ctrl+Shift+T` new, `Ctrl+PageDown`/`PageUp` switch, `Ctrl+Shift+PageDown`/
//! `PageUp` reorder) and the split-surface chords (`Ctrl+Shift+O/E/W/X`,
//! `Alt+arrows`, and the `Ctrl+Shift+A`/`Ctrl+Shift+G` broadcast toggles)
//! **before** any pane widget clones the event stream, so a chord
//! never doubles as shell input; everything else reaches the focused shell
//! exactly as in TERM-3. When the last pane of the last tab closes (explicitly
//! or because its shell exited), the surface closes with it — the classic
//! terminal-window lifecycle. If the OS refuses the very first PTY, the honest
//! spawn error is all that renders (no fake shell, §7).
//!
//! TERM-8 adds the **remote** path: the tab bar's globe button (or `Ctrl+Shift+R`)
//! opens the "new terminal on → <peer>" picker — the mesh presence roster + a
//! manual host field — and a pick opens a new tab whose first pane is a shell on
//! that mesh node, driven over the TERM-7 broker. That wiring lives in
//! [`TabbedTerminal`]; the `TabbedTerminal::new` here resolves the live Bus +
//! roster from the environment ([`mde_term_egui::RemoteHub::from_env`]).
//!
//! TERM-10 adds **saved layouts**: the tab bar's split-pane button (or
//! `Ctrl+Shift+L`) opens the saved-layouts overlay — save the current tab/split
//! arrangement under a name, or launch any layout the mesh has synced here.
//! Layouts persist to the Syncthing-replicated workgroup root (the bookmarks
//! store idiom), so one saved on any node is launchable on another; the wiring
//! (capture + rebuild + the synced store) all lives in [`TabbedTerminal`].

use mde_egui::{eframe, egui, run_client, Style};
use mde_term_egui::{SpawnOptions, TabbedTerminal};

/// The Terminal surface app: a tabbed split-pane terminal filling the window —
/// or the honest spawn error if the OS refused the first PTY.
struct TermApp {
    term: Result<TabbedTerminal, String>,
}

impl TermApp {
    /// Spawn the first session with the design's lock-10 defaults (the user's
    /// `$SHELL` as a login shell, inherited cwd/env, via
    /// [`SpawnOptions::default`]); every split and every new tab reuses the
    /// same recipe.
    fn new() -> Self {
        let term = TabbedTerminal::new(SpawnOptions::default())
            .map_err(|err| format!("could not start the shell: {err}"));
        Self { term }
    }
}

impl eframe::App for TermApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(term) = &mut self.term {
            // One rebindable table (TERM-12) decodes every chord — tab + split
            // commands and the pane actions — before the panes read input this
            // frame, so a chord never doubles as shell input.
            term.dispatch_keys(ctx);
        }
        // Full-bleed: the tab bar caps the window, the active split tree owns
        // the rest, so the rect → cols/rows mapping is the window size (no
        // panel margins eating cells).
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Style::BG))
            .show(ctx, |ui| match &mut self.term {
                Ok(term) => {
                    term.show(ui);
                    if term.is_empty() {
                        // The last pane of the last tab closed — the window
                        // goes with it.
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Err(err) => {
                    ui.colored_label(Style::DANGER, err.as_str());
                }
            });
    }
}

fn main() -> eframe::Result<()> {
    run_client("org.magicmesh.Terminal", "MCNF Terminal", |_cc| {
        TermApp::new()
    })
}
