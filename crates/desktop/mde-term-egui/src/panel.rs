//! The **lib panel seam** (TERM-16): the production terminal surface the one
//! Quazar shell (`mde-shell-egui`) embeds as `Surface::Terminal`.
//!
//! Under E12 "Quazar" the mesh surfaces are **panels in the one shell**, not
//! separate clients (§5 EMBED — there is no compositor). This module exposes the
//! full TERM-4/5/8 terminal — the [`TabbedTerminal`](crate::TabbedTerminal) tab
//! bar over Terminator's split tree, one real login shell per pane — through the
//! exact seam `mde-media-egui` gives the shell for the media player:
//!
//! * [`real_terminal`] builds the production [`TerminalSurface`] (the analogue of
//!   `mde_media_egui::real_media`), owning the live [`TabbedTerminal`] over a real
//!   local PTY — no demo data (§7). The shell holds one directly.
//! * [`terminal_pump`] is the per-frame state pump (the analogue of `media_pump`):
//!   it lazily lands the bundled Intel One Mono face and drains the chord keymap
//!   *before* the panes read input, exactly as the standalone binary's update loop
//!   does — so a chord never doubles as shell input.
//! * [`terminal_panel`] renders the surface into the shell body (the analogue of
//!   `media_panel`): the tab bar (its own header) + the active split tree, all
//!   through the shared Carbon [`Style`] tokens (§4).
//!
//! It REUSES the TERM-4/5 [`TabbedTerminal`] verbatim — it re-derives no terminal
//! UI; it is the mount seam the shell drives, the same way the standalone
//! `mde-term-egui` binary drives it in its own window.

use mde_egui::egui::{Context, RichText, Ui};
use mde_egui::Style;

use crate::{MenuBar, SpawnOptions, TabbedTerminal, TmuxChrome};

/// The production terminal surface the E12 shell embeds (TERM-16).
///
/// Owns the live [`TabbedTerminal`] — the whole TERM-4/5/8 terminal over a real
/// local PTY — or the honest first-PTY spawn error if the OS refused it (§7, never
/// a faked shell). The terminal analogue of `mde-media-egui`'s `MediaSurface`: the
/// render-agnostic surface state the shell holds directly and drives with
/// [`terminal_pump`] / [`terminal_panel`], the exact seam the standalone
/// `mde-term-egui` binary renders into its own window.
pub struct TerminalSurface {
    /// The tabbed split-pane terminal, or the honest first-PTY spawn error the
    /// panel renders instead (§7).
    term: Result<TabbedTerminal, String>,
    /// Whether the bundled Intel One Mono face has been layered onto the
    /// shell's shared font set yet. Installed lazily on the first [`terminal_pump`]
    /// — the standalone binary installs it at creation from its `CreationContext`,
    /// but the embed has no egui `Context` until its first frame.
    fonts_installed: bool,
    /// The TERM-MENUBAR-1 top menu bar (its only state is the shortcuts-window
    /// toggle; every feature it drives lives in the [`TabbedTerminal`]).
    menubar: MenuBar,
    /// The TMUX-FC-2 session management chrome — the optional live `tmux -CC`
    /// control client + the sidebar tree. Opt-in (lock #16): nothing attaches
    /// until the Tmux menu's "New tmux session", so a terminal that never touches
    /// tmux pays nothing.
    tmux: TmuxChrome,
}

impl TerminalSurface {
    /// CONSOLE-2 — the Console front door's entrypoint on the embedded surface:
    /// open a **named tab running `argv`** (a typed program+args array, §9) and
    /// focus it ([`TabbedTerminal::spawn_tab`]). The tab stays open when the
    /// command exits, showing the exit status with a key/click-to-close prompt.
    /// Root ops pass a [`crate::tabs::sudo_argv`]-wrapped argv — sudo prompts
    /// for the password interactively in the tab's PTY (no caching tricks).
    ///
    /// Returns whether the tab opened: `false` when the surface has no live
    /// terminal (the first PTY was refused — the panel already shows why) or
    /// the spawn failed (the surface's error chip carries the OS refusal, §7).
    pub fn spawn_tab(&mut self, name: impl Into<String>, argv: &[String]) -> bool {
        self.term
            .as_mut()
            .is_ok_and(|term| term.spawn_tab(name, argv))
    }

    /// WIN7-4 — the open-tab count, the SAME already-`pub`
    /// [`TabbedTerminal::tab_count`] the standalone binary's own tab strip
    /// already calls (no second read, §7). `None` when the surface has no
    /// live terminal (the first PTY was refused — matching
    /// [`Self::spawn_tab`]'s own honest-absence shape). `mde-shell-egui`
    /// holds this `TerminalSurface` directly and reuses it for the Start
    /// Menu Terminal tile's live fact.
    #[must_use]
    pub fn tab_count(&self) -> Option<usize> {
        self.term.as_ref().ok().map(TabbedTerminal::tab_count)
    }
}

/// Build the production [`TerminalSurface`] over a real local login shell.
///
/// The one construction path for a live terminal surface the shell owns, mirroring
/// `mde_media_egui::real_media()`. It spawns the design's lock-10 default recipe —
/// the user's `$SHELL` as a login shell (fallback `/bin/sh`), inheriting cwd + env
/// ([`SpawnOptions::default`]) — the same recipe the standalone binary uses. A
/// refused first PTY is kept as the honest error the panel renders (§7), never a
/// fabricated shell.
#[must_use]
pub fn real_terminal() -> TerminalSurface {
    TerminalSurface {
        term: TabbedTerminal::new(SpawnOptions::default())
            .map_err(|err| format!("could not start the shell: {err}")),
        fonts_installed: false,
        menubar: MenuBar::new(),
        tmux: TmuxChrome::new(),
    }
}

/// The per-frame state pump — mirrors the sibling surfaces' `*_pump`.
///
/// Lands the bundled Intel One Mono face once (idempotent), then drains this
/// frame's chords through the rebindable keymap and applies each — tab commands to
/// the surface, split commands + pane actions to the active tab — **before** the
/// panes read input in [`terminal_panel`], so a chord never doubles as shell input
/// (the standalone binary's `dispatch_keys` step, moved to the shell's pump slot).
pub fn terminal_pump(surface: &mut TerminalSurface, ctx: &Context) {
    if !surface.fonts_installed {
        crate::fonts::install(ctx);
        surface.fonts_installed = true;
    }
    // Drain any `tmux -CC` control traffic into the model before the tree renders
    // (a no-op until the user opts into a tmux session).
    surface.tmux.pump();
    if let Ok(term) = &mut surface.term {
        term.dispatch_keys(ctx);
    }
}

/// Render the terminal surface into the shell body — mirrors `media_panel`.
///
/// Paints the [`TabbedTerminal`]'s own tab-bar header + the active tab's split
/// tree (all through [`Style`] tokens, §4), or the honest spawn error if the OS
/// refused the first PTY (§7). If every tab has since closed, it offers a fresh
/// session rather than dead-ending — the embedded surface has no window to close,
/// unlike the standalone binary.
pub fn terminal_panel(ui: &mut Ui, surface: &mut TerminalSurface) {
    // Split the surface's fields so the menu bar (its own state), the tmux chrome,
    // and the terminal can be driven together this frame without aliasing.
    let TerminalSurface {
        term,
        menubar,
        tmux,
        ..
    } = surface;
    match term {
        Ok(term) => {
            // TERM-MENUBAR-1: the top menu-bar strip caps the surface, above the
            // TERM-5 tab bar + the active split tree. It reads a snapshot of the
            // terminal, renders the drop-downs, and applies the chosen action. The
            // Tmux menu's choice (TMUX-FC-2) routes to the surface-held chrome.
            let ctx = ui.ctx().clone();
            let tmux_choice = menubar.ui(ui, term, &ctx, tmux.is_active());
            tmux.apply_menu(tmux_choice);
            // The tmux session/window/pane tree mounts as a left panel (only when
            // opened), reserving its width before the terminal fills the rest.
            tmux.sidebar(ui);
            // TMUX-FC-3: while a control client is live, the body is the mounted
            // tmux window view (tab strip + the window's panes as live TERM-3
            // widgets); otherwise the native tabbed terminal owns it (lock #3
            // coexistence — tmux stays opt-in, detach returns the native shell).
            if !tmux.window_body(ui) {
                term.show(ui);
                if term.is_empty() {
                    empty_state(ui, term);
                }
            }
            // TMUX-FC-5: the templates ("projects") window + its editor mount as
            // floating overlays here, so a template can start a session from cold
            // (they render whether or not a control client is live).
            tmux.overlays(ui);
        }
        Err(err) => {
            ui.add_space(Style::SP_M);
            ui.horizontal(|ui| {
                ui.add_space(Style::SP_M);
                ui.colored_label(Style::DANGER, err.as_str());
            });
        }
    }
}

/// The "no session" face — the last tab closed. Offers a fresh shell so a docked
/// terminal never dead-ends (§7 honest empty state, not a blank surface).
fn empty_state(ui: &mut Ui, term: &mut TabbedTerminal) {
    ui.vertical_centered(|ui| {
        ui.add_space(Style::SP_XL);
        ui.label(
            RichText::new("No terminal sessions")
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        if ui.button("New terminal").clicked() {
            term.new_tab();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{real_terminal, terminal_panel, terminal_pump};
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;

    /// The panel seam mounts headlessly: build the real surface (a live local PTY,
    /// or the honest spawn error), pump it, and tessellate one frame on the CPU so
    /// any paint-path fault surfaces as a failure — the same `Context::run` →
    /// `tessellate` path the shell's mount test drives, minus the GPU.
    #[test]
    fn terminal_panel_mounts_and_renders_headless() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut surface = real_terminal();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            terminal_pump(&mut surface, ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                terminal_panel(ui, &mut surface);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mounted terminal surface produced no draw primitives"
        );
    }

    /// CONSOLE-2 — the shell-facing spawn-tab seam opens a named command tab on
    /// the embedded surface (the entrypoint `mde-shell-egui`'s Console panel
    /// will call), and a refused spawn reports `false` without a fake tab (§7).
    #[test]
    fn surface_spawn_tab_opens_a_named_command_tab() {
        let mut surface = real_terminal();
        let argv = vec!["/bin/echo".to_owned(), "console-seam".to_owned()];
        assert!(
            surface.spawn_tab("Ops", &argv),
            "the live surface opens a command tab"
        );
        assert!(
            !surface.spawn_tab("Ghost", &["/no/such/console-binary".to_owned()]),
            "a missing binary reports false"
        );
    }
}
