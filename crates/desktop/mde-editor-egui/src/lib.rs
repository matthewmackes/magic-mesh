//! `mde-editor-egui` — the MCNF **"Quasar"** native code-editor surface.
//!
//! A Zed-style, keyboard-driven Rust code editor adapted to the DRM-native egui
//! shell (design: `docs/design/editor.md`). It exposes an [`EditorSurface`] state
//! struct + the [`editor_panel`] seam, mounted in the one Quasar shell
//! (`mde-shell-egui`) as `Surface::Editor` through the exact seam `mde-files-egui`
//! gives the shell (`files_panel` / `real_browser`): the shell owns the surface
//! state and drives it per-frame with the `*_panel` fn.
//!
//! What has landed:
//!
//! * EDITOR-1 — the mountable surface + the honest "No file open" empty state (§7).
//! * EDITOR-2 — the rope [`Buffer`]: the real editable document model.
//! * EDITOR-3 — the custom text [`widget`]: viewport-culled render + gutter,
//!   caret + selection, mouse hit-testing (click / drag / word / line), keyboard
//!   editing + motion, undo/redo, scroll, and a soft-wrap toggle. The widget
//!   edits the live rope (§7 — runtime-reachable, not a mockup).
//! * EDITOR-4 — multi-cursor + column selection: the single caret generalizes to
//!   a `Vec` of carets (add above/below, add-at-next-match, Alt-click toggle,
//!   Alt-drag box select); every edit fans out across all of them, overlapping
//!   carets merge, and `Esc` collapses to the single primary caret.
//! * EDITOR-7 — the **fuzzy file finder** ([`finder`], `Cmd`/`Ctrl-P`) + the
//!   **command palette** ([`palette`], `Cmd`/`Ctrl-Shift-P`): two overlays whose
//!   trigger chords are intercepted at the panel level (before the widget reads
//!   this frame's events) and which drive the existing surface seams over a small
//!   self-contained [`fuzzy`] matcher.
//! * EDITOR-5 — **tree-sitter syntax highlighting** ([`highlight`]): per-open-
//!   buffer grammars picked by file extension (rust / python / js / ts / json /
//!   toml / markdown / bash), re-parsed **incrementally** on edit (the buffer's
//!   edit deltas splice the old tree — no full-file reparse per keystroke) and
//!   painted through the shared Carbon code-theme tokens ([`mde_egui::code`],
//!   §4). Unknown extensions honestly render plain (§7).
//! * EDTB-1 — the **Word-97 menu bar + Standard toolbar**
//!   ([`menu_bar`] / [`toolbar`]; design:
//!   `docs/design/editor-toolbar.md`): File/Edit/View/Tools/Help menus and the
//!   New·Open·Save | Cut·Copy·Paste | Undo·Redo | Zoom strip, every control
//!   routed through the same palette / `EditorView` seams (§6) — no dead
//!   entries (lock #4). Zoom is a real per-view font scale on [`EditorView`].
//! * EDTB-2/3 — the **markdown formatting engine + Formatting toolbar**
//!   ([`md_actions`] / [`format_bar`]): the Style dropdown, B/I/U/S, list, and
//!   indent controls (Word's second toolbar row) plus the **Insert** (Table
//!   grid-picker) and **Format** menus, each driving `md_actions` on the live
//!   buffer as one operator undo step. Only the standalone Table menu (cell ops)
//!   and Print/Spell/Find await later phases.
//! * EDITOR-LSP-1/2/3 — the **LSP client** ([`lsp`]) wired into the surface: the
//!   panel owns a per-document [`LspClient`], driving `didOpen`/`didChange`/
//!   `didClose` off the real buffer's open/edit/close lifecycle, and the widget
//!   paints the published diagnostics (`lsp_ui`) as gutter severity markers +
//!   inline underlines with a hover message. LSP-3 adds cross-file navigation
//!   ([`lsp_nav`]): goto-definition (`F12`) + find-references (`Shift-F12`,
//!   a results list) jump through the shared open+jump seam, rename (`F2`)
//!   applies the server's cross-file `WorkspaceEdit` (undoable in open buffers,
//!   rewritten on disk when closed), and format (`Shift-Alt-F`) applies the
//!   returned edits. An absent server binary surfaces the honest
//!   [`LspState::Unavailable`] status, and every navigation action on a gated
//!   server is an honest no-op with a status — never a faked session (§7).
//! * EDITOR-COLLAB-1/2 — **mesh co-editing**: the conflict-free replicated
//!   document ([`crdt`], yrs) and the **share-session** ([`collab_session`]) that
//!   carries it over the Mackes Bus as a P2P editing session — local edits
//!   broadcast, remote merges applied, the y-sync state-vector handshake on join,
//!   remote cursors/presence, and a host/guest permission model. No cloud: every
//!   frame rides the local Bus spool the broker federates over Nebula. Real
//!   convergence is proven over a transport seam ([`collab_session::CollabTransport`])
//!   with an in-process fake bus; the panel wiring + live smoke land in COLLAB-3.
//!
//! * EDITOR-6 — **tabs + splittable panes** ([`panes`]): the surface grows from
//!   one open document into a pane registry (each pane a strip of open-buffer
//!   **tabs**) arranged in a binary **split tree** (H/V splits to any depth,
//!   draggable dividers), mirroring `mde-term-egui`'s pure split model (§6). The
//!   focused pane's active tab drives the shared text widget; `Ctrl-T` opens a
//!   tab, `Ctrl-W` closes one (collapsing an emptied pane), and `Ctrl-\` /
//!   `Ctrl-Shift-\` split the focused pane. Every tab is a real rope [`Buffer`]
//!   (§7 — no mock panes).
//!
//! * EDITOR-8 — **project + in-buffer search** (`search`): a find/replace bar over
//!   the active buffer (`Ctrl-F` / `Ctrl-H`) with case / whole-word / regex
//!   toggles, next/prev cycling, replace + replace-all, and every match live-
//!   highlighted in the widget (viewport-culled, layered over the tree-sitter
//!   paint like the LSP diagnostics); plus a project-wide search (`Ctrl-Shift-F`)
//!   whose honest backend is **ripgrep** when present, else a bounded in-Rust
//!   walk — a picked hit opens the file + jumps to the line (§7 — real rope
//!   edits + real files, no mockups).
//!
//! * EDITOR-10 — the **integrated terminal dock** ([`terminal`]): `mde-term-egui`'s
//!   `TabbedTerminal` embedded as a toggleable bottom panel (Ctrl+Backtick / View →
//!   Terminal / a surface-strip button / the palette), a real login shell on a
//!   real PTY spawned in the open project root, kept across toggles (§6 glue over
//!   the TERM-16 mount seam — no re-implemented terminal).
//!
//! * EDITOR-12 — **code folding + symbol outline** ([`fold`] / [`outline`]): both
//!   reuse the SAME EDITOR-5 tree-sitter tree (no second parser). Folding derives
//!   fold regions (functions / blocks / impls) from the tree, collapses them from
//!   a gutter chevron or the `Ctrl-Shift-[` / `Ctrl-Shift-]` chords (per-buffer
//!   state), and genuinely hides the folded lines in the widget render. The
//!   outline is a toggleable panel (`Ctrl-Shift-O`) listing the file's symbols;
//!   clicking one jumps the caret through the shared EDITOR-8/LSP jump seam. A
//!   language with no grammar shows an honest empty state (§7).
//!
//! Layering (§6): the surface state + render seam live in [`panel`], the widget in
//! [`widget`], the document model in [`buffer`]; the only in-workspace edge points
//! inward to [`mde_egui`] (the harness + the shared Carbon `Style`).

pub mod buffer;
pub mod collab_session;
pub mod crdt;
mod finder;
pub mod fold;
mod format_bar;
mod fuzzy;
pub mod highlight;
pub mod lsp;
pub mod lsp_nav;
pub mod lsp_ui;
pub mod md_actions;
mod menu_bar;
pub mod outline;
mod palette;
pub mod panel;
pub mod panes;
pub mod project_tree;
mod search;
mod terminal;
mod toolbar;
pub mod widget;

use mde_egui::{eframe, egui};

pub use buffer::Buffer;
pub use collab_session::{
    Access, BusTransport, CollabError, CollabMessage, CollabSession, CollabTransport, CursorPos,
    FakeBus, FrameKind, PollOutcome, Presence, RemotePeer, Role, SessionId,
};
pub use crdt::{CollabDoc, CrdtError, EditSink, TextEdit};
pub use fold::{FoldRegion, Folds};
pub use highlight::{Highlighter, Language};
pub use lsp::{Diagnostic, LspClient, LspState, Severity};
pub use lsp_ui::DiagnosticsOverlay;
pub use outline::{Symbol, SymbolKind};
pub use panel::{editor_panel, EditorSurface};
pub use project_tree::ProjectTree;
pub use widget::{editor_widget, EditorView};

/// Build the production [`EditorSurface`] the E12 shell owns and mounts with
/// [`editor_panel`] — the editor analogue of `mde_files_egui::real_browser()`.
///
/// The surface opens to the honest empty state with no document (§7); the
/// operator opens one through the scratch affordance or, once they land, the
/// fuzzy-open / Files send (EDITOR-7/9 call [`EditorSurface::open_path`] /
/// [`EditorSurface::open_text`]). Factored out so the shell mounts the surface
/// through one named constructor, exactly as it builds Files via `real_browser()`
/// and Terminal via `real_terminal()`.
#[must_use]
pub fn real_editor() -> EditorSurface {
    EditorSurface::default()
}

/// The eframe application: the [`EditorSurface`] rendered each frame.
///
/// The standalone binary's app wrapper; it owns the window `CentralPanel` and
/// renders the surface through the shared [`editor_panel`] fn — the exact call
/// the E12 shell makes to mount the editor as an embedded panel, so standalone
/// and embedded are identical (E12 §5 EMBED).
pub struct EditorApp {
    /// The one editor surface this window renders.
    editor: EditorSurface,
}

impl EditorApp {
    /// Build the surface over a fresh [`EditorSurface`] (`real_editor()`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            editor: real_editor(),
        }
    }
}

impl Default for EditorApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Thin frame wrapper (E12 §5 EMBED): the binary only owns the window
        // `CentralPanel`; the surface itself renders through the shared
        // [`editor_panel`] fn — the exact same call the E12 shell makes to mount
        // the editor as an embedded panel, so standalone and embedded are
        // identical.
        egui::CentralPanel::default().show(ctx, |ui| {
            editor_panel(ui, &mut self.editor);
        });
    }
}
