//! `mde-term-egui` binary — the Terminal surface on the shared harness.
//!
//! TERM-4: the window mounts a [`SplitTerminal`] — Terminator's arbitrarily
//! nested H/V split tree, one real login shell per pane. The update loop
//! consumes the split-surface chords (`Ctrl+Shift+O/E/W/X`, `Alt+arrows`)
//! **before** any pane widget clones the event stream, so a chord never
//! doubles as shell input; everything else reaches the focused shell exactly
//! as in TERM-3. When the last pane closes (explicitly or because its shell
//! exited), the surface closes with it — the classic terminal-window
//! lifecycle. If the OS refuses the very first PTY, the honest spawn error is
//! all that renders (no fake shell, §7).

use mde_egui::{eframe, egui, run_client, Style};
use mde_term_egui::{consume_commands, SpawnOptions, SplitTerminal};

/// The Terminal surface app: a split-pane terminal filling the window — or
/// the honest spawn error if the OS refused the first PTY.
struct TermApp {
    term: Result<SplitTerminal, String>,
}

impl TermApp {
    /// Spawn the first session with the design's lock-10 defaults (the user's
    /// `$SHELL` as a login shell, inherited cwd/env, via
    /// [`SpawnOptions::default`]); every split reuses the same recipe.
    fn new() -> Self {
        let term = SplitTerminal::new(SpawnOptions::default())
            .map_err(|err| format!("could not start the shell: {err}"));
        Self { term }
    }
}

impl eframe::App for TermApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(term) = &mut self.term {
            for cmd in consume_commands(ctx) {
                term.apply(cmd);
            }
        }
        // Full-bleed: the split tree owns the whole window, so the rect →
        // cols/rows mapping is the window size (no panel margins eating cells).
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Style::BG))
            .show(ctx, |ui| match &mut self.term {
                Ok(term) => {
                    term.show(ui);
                    if term.is_empty() {
                        // The last pane closed — the window goes with it.
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
