//! `mde-term-egui` binary — stands the Terminal surface up as a client on the
//! shared harness (TERM-3, §7 runtime-reachable end-to-end): one
//! [`TerminalWidget`] over a [`LocalPty`] login shell, mounted exactly as the
//! sibling surface binaries mount theirs.

use mde_egui::{eframe, egui, run_client, Style};
use mde_term_egui::{LocalPty, SpawnOptions, TerminalWidget};

/// The Terminal surface app: a full-bleed terminal pane — or the honest spawn
/// error if the OS refused a PTY (no fake shell, §7).
struct TermApp {
    session: Result<TerminalWidget, String>,
}

impl TermApp {
    /// Spawn the user's login shell (`$SHELL`, fallback `/bin/sh`) on a fresh
    /// PTY — the design's lock 10 defaults, via [`SpawnOptions::default`].
    fn new() -> Self {
        let session = LocalPty::spawn(SpawnOptions::default())
            .map(TerminalWidget::new)
            .map_err(|err| format!("could not start the shell: {err}"));
        Self { session }
    }
}

impl eframe::App for TermApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Full-bleed: the terminal grid owns the whole window, so the rect →
        // cols/rows mapping is the window size (no panel margins eating cells).
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Style::BG))
            .show(ctx, |ui| match &mut self.session {
                Ok(widget) => {
                    widget.show(ui);
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
