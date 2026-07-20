//! The **integrated terminal dock** (EDITOR-10): `mde-term-egui`'s
//! [`TabbedTerminal`] embedded as a **toggleable bottom panel** in the editor.
//!
//! This is pure glue over `mde-term-egui` (§6) — it re-derives no terminal UI. It
//! mirrors the TERM-16 mount seam (`TerminalSurface` / `terminal_pump` /
//! `terminal_panel`) that mounts the terminal into the one Construct shell, driving
//! the reused [`TabbedTerminal`] with its own public [`dispatch_keys`] +
//! [`show`] + the bundled monospace [`fonts::install`]. The one difference from
//! `mde_term_egui::real_terminal()` is the **cwd**: the acceptance spawns the
//! login shell in the editor's open project root, and `real_terminal()`'s recipe
//! hardcodes [`SpawnOptions::default`] (the process cwd), so the dock holds the
//! [`TabbedTerminal`] directly and spawns it with a caller-supplied cwd.
//!
//! The session is spawned **lazily on the first open** and then kept across
//! toggles, so hiding the dock never loses the running shell (the acceptance).
//!
//! [`dispatch_keys`]: TabbedTerminal::dispatch_keys
//! [`show`]: TabbedTerminal::show
//! [`fonts::install`]: mde_term_egui::fonts::install

use std::path::Path;

use mde_egui::egui::{Context, RichText, Ui};
use mde_egui::Style;
use mde_term_egui::{SpawnOptions, TabbedTerminal};

/// The embedded integrated terminal (EDITOR-10).
///
/// Holds the live [`TabbedTerminal`] — the whole TERM-4/5/8 terminal (tabs /
/// splits / broadcast / a shell on any mesh peer) over a real local PTY — spawned
/// in the open project root, plus whether the dock is currently shown. The editor
/// analogue of `mde-term-egui`'s `TerminalSurface`, driven by [`pump`](Self::pump)
/// and [`show`](Self::show); the differences are the caller-supplied cwd and the
/// lazy first-open spawn (a project that never opens the terminal pays no PTY).
///
/// `pub` in a private module (the crate convention for `mde_files`/`menu_bar` and
/// friends) — the private `mod terminal` keeps it crate-internal, not public API.
#[derive(Default)]
pub struct TerminalDock {
    /// Whether the dock is shown at the bottom of the editor body.
    shown: bool,
    /// The live tabbed terminal, or the honest first-PTY spawn error (§7) — `None`
    /// until the dock is first opened. Kept across toggles so hiding the dock
    /// never loses the running shell (`Option::<Result<_>>::default()` is `None`).
    term: Option<Result<TabbedTerminal, String>>,
    /// Whether the bundled monospace face has been layered onto the shared font
    /// set yet — installed lazily on the first [`pump`](Self::pump) (there is no
    /// egui [`Context`] until the first frame), exactly as `terminal_pump` does.
    fonts_installed: bool,
}

impl TerminalDock {
    /// Whether the dock is currently shown (the editor is painting the terminal).
    pub const fn is_shown(&self) -> bool {
        self.shown
    }

    /// Toggle the dock's visibility — the seam every trigger drives (the
    /// Ctrl+Backtick chord, the View → Terminal menu item, the surface strip
    /// button, the palette
    /// command). Opening it lazily spawns the shell in `cwd` on the first open (see
    /// [`ensure_spawned`](Self::ensure_spawned)); closing only hides it — the PTY
    /// keeps running, so re-opening restores the same session (the acceptance).
    pub fn toggle(&mut self, cwd: Option<&Path>) {
        self.shown = !self.shown;
        if self.shown {
            self.ensure_spawned(cwd);
        }
    }

    /// Spawn the login shell in `cwd` (the open project root, else the process cwd
    /// when `None`) if it has not been spawned yet — the one construction path,
    /// mirroring `real_terminal()` but with the caller's cwd threaded into
    /// [`SpawnOptions`]. A refused first PTY is kept as the honest error the panel
    /// renders (§7), never a fabricated shell.
    fn ensure_spawned(&mut self, cwd: Option<&Path>) {
        if self.term.is_some() {
            return;
        }
        let opts = SpawnOptions {
            cwd: cwd.map(Path::to_path_buf),
            ..SpawnOptions::default()
        };
        self.term = Some(
            TabbedTerminal::new(opts).map_err(|err| format!("could not start the shell: {err}")),
        );
    }

    /// The per-frame pump — mirrors `terminal_pump`. Lands the bundled monospace
    /// face once (idempotent), then drains this frame's terminal chords through the
    /// terminal's own rebindable keymap **before** its panes read input, so a chord
    /// never doubles as shell input. Called only while the dock is shown — a hidden
    /// terminal never dispatches, so it can never eat the editor's own keystrokes;
    /// the editor's panel-level intercepts already ran this frame, so on any shared
    /// chord (e.g. `Alt+arrow`, `Ctrl+Shift+O`) the editor wins.
    pub fn pump(&mut self, ctx: &Context) {
        if !self.fonts_installed {
            mde_term_egui::fonts::install(ctx);
            self.fonts_installed = true;
        }
        if let Some(Ok(term)) = self.term.as_mut() {
            term.dispatch_keys(ctx);
        }
    }

    /// Render the dock's body — mirrors `terminal_panel`. Paints the
    /// [`TabbedTerminal`]'s own tab bar + the active tab's split tree (all through
    /// the shared Carbon [`Style`] tokens, §4), or the honest first-PTY spawn error
    /// (§7). If every tab has since closed it offers a fresh session rather than
    /// dead-ending (the embedded dock has no window to close). A no-op before the
    /// dock has ever been opened (nothing spawned yet).
    pub fn show(&mut self, ui: &mut Ui) {
        match self.term.as_mut() {
            Some(Ok(term)) => {
                term.show(ui);
                if term.is_empty() {
                    empty_state(ui, term);
                }
            }
            Some(Err(err)) => {
                ui.add_space(Style::SP_M);
                ui.horizontal(|ui| {
                    ui.add_space(Style::SP_M);
                    ui.colored_label(Style::DANGER, err.as_str());
                });
            }
            None => {}
        }
    }

    /// The active terminal tab's live working directory — read from the running
    /// shell (`/proc/<pid>/cwd`, via the TERM-10 layout capture). The test seam
    /// proving the PTY spawned in the project root; `None` before the dock has been
    /// opened, on a spawn error, or when the first tab is not a lone local pane.
    #[cfg(test)]
    pub fn active_cwd(&self) -> Option<std::path::PathBuf> {
        use mde_term_egui::LayoutPane;
        let Some(Ok(term)) = self.term.as_ref() else {
            return None;
        };
        let layout = term.capture_layout("probe", "editor");
        match &layout.tabs.first()?.root {
            LayoutPane::Leaf(spec) => spec.cwd.clone(),
            LayoutPane::Split { .. } => None,
        }
    }
}

/// The dock's "no session" face — the last tab closed. Offers a fresh shell so a
/// docked terminal never dead-ends (§7 honest empty state, not a blank surface);
/// mirrors `terminal_panel`'s own empty state.
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
    use super::TerminalDock;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A unique temp dir for a live terminal test, cleaned up on drop (the same
    /// idiom `panel`'s tests use — no `tempfile` dep needed for a bare dir).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "mde-editor-term-{tag}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&base).expect("create temp dir");
            Self(base)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// A fresh dock opens closed, holds no PTY, and paints nothing — the honest
    /// default (a project that never opens the terminal pays no shell).
    #[test]
    fn a_fresh_dock_is_closed_and_unspawned() {
        let dock = TerminalDock::default();
        assert!(!dock.is_shown(), "a fresh dock is hidden");
        assert!(dock.active_cwd().is_none(), "a fresh dock spawned no shell");
    }

    /// Opening the dock spawns a **real PTY** whose shell runs in the supplied
    /// project cwd (the acceptance) — read back live from `/proc/<pid>/cwd`.
    #[test]
    fn opening_spawns_a_real_pty_in_the_project_cwd() {
        let tmp = TempDir::new("cwd");
        let want = tmp.0.canonicalize().expect("canonical project root");
        let mut dock = TerminalDock::default();
        dock.toggle(Some(&want));
        assert!(dock.is_shown(), "toggling on shows the dock");
        let got = dock
            .active_cwd()
            .expect("the dock spawned a live local shell")
            .canonicalize()
            .expect("canonical shell cwd");
        assert_eq!(got, want, "the shell spawned in the project root");
    }

    /// Toggling the dock off then on again keeps the SAME session — hiding never
    /// tears the shell down (the acceptance), so the cwd is unchanged and no
    /// second PTY was spawned.
    #[test]
    fn toggling_off_and_on_keeps_the_session() {
        let tmp = TempDir::new("keep");
        let want = tmp.0.canonicalize().expect("canonical project root");
        let mut dock = TerminalDock::default();
        dock.toggle(Some(&want)); // open — spawns
        let first = dock.active_cwd().expect("spawned");
        dock.toggle(None); // hide — must NOT tear the shell down
        assert!(!dock.is_shown(), "toggling off hides the dock");
        assert!(
            dock.active_cwd().is_some(),
            "the hidden dock kept its live session"
        );
        dock.toggle(None); // show again — must reuse, not respawn in the None cwd
        let second = dock.active_cwd().expect("still alive");
        assert_eq!(first, second, "re-opening restored the same session");
    }

    /// The dock mounts + paints headlessly once shown — the same `Context::run` →
    /// `tessellate` path the panel's mount test drives, minus the GPU.
    #[test]
    fn a_shown_dock_mounts_and_renders_headless() {
        let tmp = TempDir::new("paint");
        let mut dock = TerminalDock::default();
        dock.toggle(Some(&tmp.0));
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 320.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            dock.pump(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                dock.show(ui);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mounted terminal dock produced no draw primitives"
        );
    }
}
