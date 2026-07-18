//! The **surface panel seam** (EDITOR-1 → EDITOR-3): the code-editor surface the
//! one Construct shell (`mde-shell-egui`) embeds as `Surface::Editor`.
//!
//! Under E12 "Construct" the mesh surfaces are **panels in the one shell**, not
//! separate clients (§5 EMBED — there is no compositor). This module exposes the
//! editor surface through the exact seam `mde-files-egui` gives the shell:
//!
//! * [`EditorSurface`] is the surface state the shell holds directly (the
//!   analogue of `mde_files_egui::FileBrowser`), built by
//!   [`real_editor`](crate::real_editor). It now owns the **open document** — a
//!   rope [`Buffer`] plus its [`EditorView`] widget state (EDITOR-2/3) — as an
//!   `Option`: `None` is the honest empty state (§7), `Some` is a live editable
//!   document.
//! * [`editor_panel`] renders the surface into the shell body (the analogue of
//!   `files_panel`): when a document is open it draws a compact chrome strip and
//!   the real text widget ([`editor_widget`]); when none is open it keeps the
//!   honest "No file open" empty state, with a temporary affordance to open a
//!   scratch buffer so the surface is exercisable before the fuzzy-open / Files
//!   send land (EDITOR-7/9).
//!
//! All chrome resolves through the shared Carbon [`Style`] tokens (§4). The
//! `&mut EditorSurface` seam matches the mount contract the shell wires for
//! Files/Terminal, so this grows the surface without re-wiring the shell.

use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use mde_egui::egui::{
    self, pos2, vec2, Align, Align2, CursorIcon, FontId, Id, Key, Layout, Modifiers, Rect,
    Response, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder,
};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

use crate::buffer::{Buffer, DiskStamp, DiskState};
use crate::finder::{self, FileFinder};
use crate::format_bar;
use crate::highlight::{Highlighter, Language};
use crate::lsp::{Location, LspClient, LspReply, TextEdit, WorkspaceEdit};
use crate::lsp_nav::{self, RefRow, ReferencesPanel, RenameBox};
use crate::lsp_ui::{lsp_status, DiagnosticsOverlay};
use crate::markdown;
use crate::md_actions::{self, ListKind};
use crate::menu_bar::{self, ListStyle, MenuAction, MenuContext};
use crate::outline;
use crate::palette::{self, CommandPalette, PaletteCommand};
use crate::panes::{self, NavDir, Pane, PaneId, SplitDir};
use crate::print::{self, PageLayout, PrintOptions};
use crate::project_tree::{self, ProjectTree};
use crate::search::{self, FindEvent, FindState, ProjectSearch};
use crate::spell::{self, SpellChecker, SpellMiss, SpellState};
use crate::terminal::TerminalDock;
use crate::toolbar;
use crate::widget::{editor_widget, EditorView, MatchHighlights};

mod autosave;
mod faces;
use autosave::*;
use faces::*;

/// The project-tree side panel's default width — six shared spacing units (§4).
const TREE_WIDTH: f32 = Style::SP_XL * 6.0;

/// The symbol-outline side panel's default width (EDITOR-12) — six shared
/// spacing units (§4), the tree's twin on the opposite edge.
const OUTLINE_WIDTH: f32 = Style::SP_XL * 6.0;

/// The split markdown-preview pane's default width (EDTB-7) — eleven shared
/// spacing units (§4), roughly the editor's own half so the two read side by
/// side; resizable, so this is only the opening size.
const PREVIEW_WIDTH: f32 = Style::SP_XL * 11.0;

/// Below this panel width the Word toolbars lean out (EDTB-4, design lock #9):
/// each strip's width-heavy dropdown folds into a `»` overflow so the Standard +
/// Formatting bars stay usable on a narrow (compact-shell) editor panel. Derived
/// from the shared spacing grid — a fourteen-column span (§4); the two strips
/// each want roughly this to lay out their groups without crowding, so below it
/// the wide dropdowns fold. The menu bar is already compact (dropdown buttons),
/// so it is width-invariant.
const COMPACT_WIDTH: f32 = Style::SP_XL * 14.0;

/// Whether `available_width` puts the editor panel in the compact (narrow) layout
/// — the EDTB-4 threshold decision, taken from the egui available width (the
/// shell mounts the panel with no size signal, so width is the honest proxy).
/// Pure, so it is unit-testable at both widths.
fn is_compact(available_width: f32) -> bool {
    available_width < COMPACT_WIDTH
}

/// The Save As / About dialog plate width — ten shared spacing units (§4).
const DIALOG_W: f32 = Style::SP_XL * 10.0;

/// The Print Preview overlay's default plate width (EDTB-5) — twenty shared
/// spacing units (§4), wide enough for the 80-column monospace page at the
/// preview's `SMALL` face; the window is resizable + scrollable beyond it.
const PREVIEW_W: f32 = Style::SP_XL * 20.0;

/// The Print Preview scroll viewport's max height (EDTB-5) — fourteen shared
/// spacing units (§4), so a long document scrolls inside a bounded dialog.
const PREVIEW_MAX_H: f32 = Style::SP_XL * 14.0;

/// The integrated terminal dock's default height (EDITOR-10) — eight shared
/// spacing units (§4); resizable, so this is only the opening size.
const TERMINAL_HEIGHT: f32 = Style::SP_XL * 8.0;

/// The terminal dock's minimum drag height — three spacing units (§4), so a
/// dragged-down dock still shows a usable row or two of the shell.
const TERMINAL_MIN_HEIGHT: f32 = Style::SP_XL * 3.0;

/// The About dialog's title (Help → About the Editor, EDTB-1).
const ABOUT_TITLE: &str = "About the Editor";
/// The user-facing product line shown in the About dialog.
const ABOUT_PRODUCT_LINE: &str = "Construct Editor";

/// The crate + version line the About dialog shows. `mde-egui` carries no
/// brand/build module (and the iced-era `mde-theme` brand is deliberately not a
/// dependency of this surface), so the workspace-inherited crate version — the
/// platform version — is the honest source.
const ABOUT_VERSION_LINE: &str = concat!("mde-editor-egui ", env!("CARGO_PKG_VERSION"));

/// The honest empty-state headline the surface renders when no document is open
/// (§7) — a real, reachable message, never a `todo!()`/stub.
const NO_FILE_TITLE: &str = "No file open";
/// The honest empty-state hint paired with [`NO_FILE_TITLE`] — the §7 copy the
/// design doc locks: "open a file to start editing".
const NO_FILE_HINT: &str = "Open a file to start editing.";

/// The seed text a fresh scratch buffer opens with — a tiny reachable document so
/// the operator can immediately type, move the caret, and select (the temporary
/// EDITOR-3 open affordance; the fuzzy-open / Files send land in EDITOR-7/9).
const SCRATCH_SEED: &str = "// Scratch buffer — type here.\n// The editor edits a real rope: every keystroke\n// mutates the buffer and re-renders.\n\n";

/// One open document in the editor surface: the editable rope [`Buffer`]
/// (EDITOR-2) paired with the [`EditorView`] widget state (EDITOR-3, the caret /
/// selection / scroll / wrap) and — when the file's extension maps to a
/// vendored grammar — its tree-sitter [`Highlighter`] (EDITOR-5).
struct Doc {
    /// The editable document model.
    buffer: Buffer,
    /// The widget state rendering + editing it.
    view: EditorView,
    /// The syntax highlighter, or `None` for plain text (unknown extension /
    /// a pathless scratch buffer) — the honest no-highlight render (§7).
    highlight: Option<Highlighter>,
    /// The per-document language-server session (EDITOR-LSP-2), or `None` for a
    /// pathless / unrecognized buffer. A recognized language with no registered
    /// or installed server parks in an honest gated [`LspState`](crate::lsp::LspState)
    /// (`NoServer` / `Unavailable`), so this is `Some` whenever the file has a
    /// language — the doc-sync calls are honest no-ops there (§7).
    lsp: Option<LspClient>,
    /// The diagnostics the widget paints, rebuilt from `lsp`'s store only when
    /// its epoch moves (the §7 epoch-gate). Empty for a doc with no server.
    diagnostics: DiagnosticsOverlay,
    /// The [`EditorView`] edit generation last pushed to `lsp` via `didChange` —
    /// the per-frame throttle so an unchanged buffer is not re-sent.
    lsp_synced_gen: u64,
    /// The last [`LspClient::replies_epoch`](crate::lsp::LspClient::replies_epoch)
    /// this doc drained (EDITOR-LSP-3) — the poll gate so quiet frames skip the
    /// reply inbox.
    lsp_reply_epoch: u64,
    /// The buffer revision observed on the previous autosave tick (EDITOR-11) —
    /// the debounce spots a fresh keystroke by the revision moving and re-arms
    /// [`idle_since`](Self::idle_since).
    autosave_rev: u64,
    /// App-time (seconds) the buffer last changed — autosave writes a dirty
    /// buffer only once it has been idle past the configured window (EDITOR-11).
    idle_since: f64,
    /// The per-document spell-check state (EDTB-6): the resolved misses the widget
    /// underlines + the `F7` walk steps, and the background-check debounce. Empty
    /// for a code / non-spell-checkable buffer.
    spell: SpellChecker,
    /// The EDTB-7 split-preview parse cache: the parsed markdown [`Block`](markdown::Block)s
    /// and the buffer revision they were parsed at, so the preview re-parses only
    /// when the buffer changed (the debounce, mirroring the spell pass) — never a
    /// per-frame full-document parse. `None` revision = not yet parsed.
    preview: PreviewCache,
}

/// One document's markdown-preview parse cache (EDTB-7): the parsed blocks + the
/// buffer revision they reflect, so [`Doc::preview_blocks`] re-parses only on a
/// real edit.
#[derive(Default)]
struct PreviewCache {
    /// The buffer revision the cached blocks were parsed from, or `None` before
    /// the first parse.
    rev: Option<u64>,
    /// The parsed markdown blocks the preview pane renders.
    blocks: Vec<markdown::Block>,
}

impl Doc {
    /// Wrap a freshly built buffer in a new view, picking the highlighter by
    /// the buffer's file extension (none for scratch/unknown — plain text). The
    /// language server is attached separately by [`start_lsp`](Self::start_lsp)
    /// (it needs the resolved project root, which the surface owns).
    fn new(buffer: Buffer) -> Self {
        let highlight = buffer.path().and_then(Highlighter::for_path);
        let autosave_rev = buffer.revision();
        Self {
            buffer,
            view: EditorView::new(),
            highlight,
            lsp: None,
            diagnostics: DiagnosticsOverlay::default(),
            lsp_synced_gen: 0,
            lsp_reply_epoch: 0,
            autosave_rev,
            idle_since: 0.0,
            spell: SpellChecker::default(),
            preview: PreviewCache::default(),
        }
    }

    /// Whether this document is spell-checked (EDTB-6, md/text first).
    fn spellcheckable(&self) -> bool {
        spell::is_spellcheckable(self.buffer.path())
    }

    /// Whether this document gets the split markdown preview (EDTB-7, md/text
    /// first) — a code buffer greys the toggle out (§7).
    fn previewable(&self) -> bool {
        markdown::is_previewable(self.buffer.path())
    }

    /// The parsed markdown blocks for the split preview (EDTB-7), re-parsed only
    /// when the buffer revision moved since the last parse (the debounce, so live
    /// typing reflects without a per-frame full parse). Stringifies the rope only
    /// on a real edit; a quiet frame returns the cached blocks untouched.
    fn preview_blocks(&mut self) -> &[markdown::Block] {
        let rev = self.buffer.revision();
        if self.preview.rev != Some(rev) {
            self.preview.blocks = markdown::parse(&self.buffer.rope().to_string());
            self.preview.rev = Some(rev);
        }
        &self.preview.blocks
    }

    /// Drive the per-document spell pass (EDTB-6) — the spell analogue of
    /// [`refresh_diagnostics`](Self::refresh_diagnostics) + [`sync_lsp`](Self::sync_lsp):
    /// drain a finished background check into the overlay, then launch a fresh one
    /// when the buffer changed (debounced on the revision, at most one in flight).
    /// A non-spell-checkable buffer or an unavailable `hunspell` disables the
    /// checker cleanly (no stale squiggles, §7). Stringifies the rope only when it
    /// actually spawns, so a quiet frame is free. Returns whether a check is
    /// pending, so the panel can nudge a repaint to pick up the result.
    fn pump_spell(&mut self, program: &str, state: SpellState) -> bool {
        self.spell.poll();
        if !self.spellcheckable() || !state.is_ready() {
            self.spell.disable();
            return false;
        }
        let rev = self.buffer.revision();
        if self.spell.wants_check(rev) {
            let text = self.buffer.rope().to_string();
            self.spell.spawn(program, text, rev);
        }
        self.spell.is_pending()
    }

    /// Attach a language-server session for this document's file, rooted at
    /// `root` (EDITOR-LSP-2 lifecycle: `didOpen`). A no-op for a pathless
    /// buffer or an extension with no known language. Recognized languages
    /// always attach a client — a missing server binary is the honest gated
    /// [`LspState::Unavailable`](crate::lsp::LspState) the chrome surfaces, not
    /// a fake session (§7).
    fn start_lsp(&mut self, root: &Path) {
        let Some(path) = self.buffer.path().map(Path::to_path_buf) else {
            return;
        };
        let Some(language) = Language::from_path(&path) else {
            return;
        };
        let Some(client) = build_lsp_client(language, root) else {
            return;
        };
        // didOpen with the current full text (full-text sync, v1).
        client.on_open(&path, &self.buffer.rope().to_string());
        self.lsp_synced_gen = self.view.edit_generation();
        self.lsp = Some(client);
    }

    /// Push this frame's edits to the server as one `didChange` (full-text sync)
    /// — the per-frame sync point. Throttled on the [`EditorView`] edit
    /// generation so a caret-only / unchanged frame sends nothing; multiple
    /// keystrokes coalesced into one frame send exactly one `didChange`.
    fn sync_lsp(&mut self) {
        let Some(client) = self.lsp.as_ref() else {
            return;
        };
        let Some(path) = self.buffer.path() else {
            return;
        };
        let generation = self.view.edit_generation();
        if generation == self.lsp_synced_gen {
            return;
        }
        self.lsp_synced_gen = generation;
        client.on_change(path, &self.buffer.rope().to_string());
    }

    /// Refresh the diagnostics overlay from the server's store — the §7
    /// epoch-gate: the fetch + position recompute run only when
    /// [`LspClient::diagnostics_epoch`](crate::lsp::LspClient::diagnostics_epoch)
    /// has moved, so a quiet frame does nothing.
    fn refresh_diagnostics(&mut self) {
        let Some(client) = self.lsp.as_ref() else {
            return;
        };
        let Some(path) = self.buffer.path() else {
            return;
        };
        let epoch = client.diagnostics_epoch();
        if !self.diagnostics.needs_refresh(epoch) {
            return;
        }
        let diags = client.diagnostics_for(path);
        self.diagnostics.rebuild(epoch, &self.buffer, &diags);
    }

    /// Tear down the language-server session (EDITOR-LSP-2 lifecycle:
    /// `didClose` + graceful `shutdown`) before the doc is dropped. `Drop` on
    /// [`LspClient`] hard-kills whatever remains, so this is the clean path.
    fn close_lsp(&self) {
        let Some(client) = self.lsp.as_ref() else {
            return;
        };
        if let Some(path) = self.buffer.path() {
            client.on_close(path);
        }
        client.shutdown();
    }

    /// Reload this document's content from disk (EDITOR-11 reload-theirs): replace
    /// the buffer with the on-disk copy, re-pick the highlighter for a full parse
    /// of the new text (its old incremental tree no longer matches), re-arm the
    /// autosave anchor, and resync the language server with the reloaded text
    /// (full-text `didChange`) so diagnostics track disk, not the pre-reload buffer.
    ///
    /// # Errors
    /// Returns any [`io::Error`] from re-reading the file.
    fn reload(&mut self) -> io::Result<()> {
        self.buffer.reload_from_disk()?;
        self.highlight = self.buffer.path().and_then(Highlighter::for_path);
        self.autosave_rev = self.buffer.revision();
        if let (Some(client), Some(path)) = (self.lsp.as_ref(), self.buffer.path()) {
            client.on_change(path, &self.buffer.rope().to_string());
            self.lsp_synced_gen = self.view.edit_generation();
        }
        Ok(())
    }
}

/// Build the language client for `language` rooted at `root`.
///
/// Production starts the registered server, spawning its binary when present —
/// the real live-diagnostics path. Under the crate's own `cfg(test)` the suite
/// must never launch a real OS process (there is no rust-analyzer on the
/// airgapped build host, and a real server would make the tests heavy + flaky),
/// so only the **serverless** languages get a client — the honest
/// [`LspState::NoServer`](crate::lsp::LspState) gated shape, which spawns
/// nothing. The open/change/close wiring is exercised through that gated client,
/// and the honest `Unavailable`/`Running` statuses through `lsp_ui`'s
/// `status_of` unit tests.
#[cfg(not(test))]
#[allow(clippy::unnecessary_wraps)] // the cfg(test) twin returns None; keep one signature
fn build_lsp_client(language: Language, root: &Path) -> Option<LspClient> {
    Some(LspClient::start(language, root))
}

#[cfg(test)]
fn build_lsp_client(language: Language, root: &Path) -> Option<LspClient> {
    if crate::lsp::server_spec(language).is_some() {
        return None; // never spawn a real language server in the suite
    }
    Some(LspClient::start(language, root)) // NoServer — spawns no process
}

/// One pane's strip of open-buffer **tabs** (EDITOR-6): the [`Doc`]s open in a
/// single leaf of the split tree, plus which one is active. An empty strip is
/// the honest per-pane empty state (§7).
#[derive(Default)]
struct PaneTabs {
    /// The open documents in this pane, left-to-right in the tab bar.
    tabs: Vec<Doc>,
    /// The active tab's index — only meaningful when `tabs` is non-empty; kept
    /// in range by every mutator.
    active: usize,
}

impl PaneTabs {
    /// An empty pane (no open tabs) — the per-pane empty state.
    fn empty() -> Self {
        Self::default()
    }

    /// Whether this pane holds no tabs.
    fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The active tab's index, or `None` when the pane is empty.
    fn active_index(&self) -> Option<usize> {
        (!self.tabs.is_empty()).then_some(self.active)
    }

    /// The active tab's document, or `None` when the pane is empty.
    fn active_doc(&self) -> Option<&Doc> {
        self.tabs.get(self.active)
    }

    /// The active tab's document (mut), or `None` when the pane is empty.
    fn active_doc_mut(&mut self) -> Option<&mut Doc> {
        self.tabs.get_mut(self.active)
    }

    /// Append `doc` as a new tab and make it active (the open-a-buffer path).
    fn push(&mut self, doc: Doc) {
        self.tabs.push(doc);
        self.active = self.tabs.len() - 1;
    }

    /// Focus the tab at `idx` (a no-op if out of range).
    fn focus(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active = idx;
        }
    }

    /// The index of an already-open tab showing `path`, if any — so re-opening a
    /// file focuses its tab instead of stacking a duplicate.
    fn find_path(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|d| d.buffer.path() == Some(path))
    }

    /// Close the tab at `idx`, returning the removed [`Doc`] (the caller tears
    /// down its language server). The active index is kept valid: closing a tab
    /// at or before the active one shifts focus left toward the neighbour.
    fn close(&mut self, idx: usize) -> Option<Doc> {
        if idx >= self.tabs.len() {
            return None;
        }
        let doc = self.tabs.remove(idx);
        if self.active > idx || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        Some(doc)
    }

    /// Move the tab at `from` to sit at index `to` (drag-reorder), keeping the
    /// moved tab active. Out-of-range indices are clamped; a no-op when equal.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() {
            return;
        }
        let to = to.min(self.tabs.len() - 1);
        if from == to {
            self.active = to;
            return;
        }
        let doc = self.tabs.remove(from);
        self.tabs.insert(to, doc);
        self.active = to;
    }
}

/// A per-frame tab-bar / pane interaction, applied by
/// [`EditorSurface::apply_leaf_action`] after every pane has rendered (so the
/// tree/registry mutations happen outside the pane's own borrow).
#[derive(Clone, Copy)]
enum LeafAction {
    /// Give this pane the keyboard focus (a click landed in it).
    Focus,
    /// Open a fresh scratch tab in this pane (the `+` affordance).
    NewTab,
    /// Make the tab at this index active.
    SelectTab(usize),
    /// Close the tab at this index.
    CloseTab(usize),
    /// Drag-reorder: move the tab at `from` to `to`.
    MoveTab { from: usize, to: usize },
}

/// The code-editor surface the E12 shell embeds.
///
/// The surface state the shell holds directly and drives with [`editor_panel`],
/// mirroring `mde-files-egui`'s `FileBrowser` seam. Under EDITOR-6 it owns a
/// **pane registry** ([`PaneId`] → its [`PaneTabs`]) arranged in a binary split
/// [`Pane`] tree: the focused pane's active tab is the "current document" every
/// existing seam ([`open_path`](Self::open_path), the menu bar, the palette)
/// reads through [`doc`](Self::doc) / [`doc_mut`](Self::doc_mut). A pane with no
/// tabs renders the honest empty state (§7); each tab is a real rope [`Buffer`].
// `struct_excessive_bools`: the panel toggles (tree / outline / About / the
// spell probe-latch) are independent per-surface flags, not a disguised state
// machine — an enum would misrepresent flags that vary independently.
#[allow(clippy::struct_excessive_bools)]
pub struct EditorSurface {
    /// The pane registry: every split-tree leaf's open tabs.
    panes: HashMap<PaneId, PaneTabs>,
    /// The split tree of pane ids (always at least one leaf; `focus` names one).
    tree: Pane,
    /// The focused pane — the one whose active tab drives the widget + seams.
    focus: PaneId,
    /// Monotonic id source for [`PaneId`]s (never reused within a surface).
    next_pane_id: u64,
    /// Last frame's egui widget id per pane, so [`prefocus`](Self::prefocus) can
    /// hand the keyboard to the focused pane's text widget before it renders
    /// (the term-style focus-follows-pane machinery, EDITOR-6).
    pane_widget_ids: HashMap<PaneId, Id>,
    /// The project-tree panel over the open project root (EDITOR-9), or `None`
    /// before a folder is opened (via [`open_folder`](Self::open_folder) — the
    /// Files "Send-to-Editor" send, a folder picker, or the tree toggle default).
    project: Option<ProjectTree>,
    /// Whether the project-tree side panel is shown beside the editor body.
    show_tree: bool,
    /// Whether the symbol-outline side panel is shown (EDITOR-12).
    show_outline: bool,
    /// Whether the split markdown-preview pane is shown (EDTB-7). Only rendered
    /// while the focused document is previewable (md/text); the View → Preview /
    /// toolbar toggle greys out on a code buffer.
    show_preview: bool,
    /// The fuzzy file-finder overlay (EDITOR-7, `Cmd`/`Ctrl-P`).
    finder: FileFinder,
    /// The command-palette overlay (EDITOR-7, `Cmd`/`Ctrl-Shift-P`).
    palette: CommandPalette,
    /// The in-buffer find/replace bar (EDITOR-8, `Ctrl-F` / `Ctrl-H`).
    find: FindState,
    /// The project-wide search overlay (EDITOR-8, `Ctrl-Shift-F`).
    project_search: ProjectSearch,
    /// The Save As… path dialog (EDTB-1, File → Save As).
    save_as: SaveAsDialog,
    /// Whether the About dialog is shown (EDTB-1, Help → About the Editor).
    about_open: bool,
    /// The Insert Table grid-picker dialog (EDTB-3, Insert → Table…).
    table_picker: TablePicker,
    /// The EDTB-5 print state (Print Preview overlay + last print outcome).
    print: PrintUi,
    /// The find-references results overlay (EDITOR-LSP-3, `Shift-F12`).
    references: ReferencesPanel,
    /// The rename new-name input (EDITOR-LSP-3, `F2`).
    rename: RenameBox,
    /// A short honest status line for the last LSP navigation action (§7) — e.g.
    /// "No definition found" / "Renamed 3 files" / "No language server" — shown
    /// in the status bar until the next action replaces it.
    lsp_notice: Option<String>,
    /// The persisted autosave preference (EDITOR-11) — off by default, loaded
    /// from the editor config on construction, re-saved when the operator toggles
    /// it from the status bar.
    autosave: AutosavePrefs,
    /// The external-change watch (EDITOR-11): a pending reload prompt + the poll
    /// debounce clock.
    reload: ReloadWatch,
    /// A short honest status line for the last save / autosave / reload action
    /// (§7) — e.g. "Saved" / "Autosaved" / "Reloaded from disk" / a write error —
    /// shown in the status bar until the next action replaces it.
    notice: Option<String>,
    /// The EDITOR-10 integrated terminal dock — `mde-term-egui`'s `TabbedTerminal`
    /// embedded as a toggleable bottom panel over a real PTY in the project root.
    terminal: TerminalDock,
    /// The spell-check program (EDTB-6) — `hunspell` — probed once on the first
    /// frame; the resolved availability is [`spell_state`](Self::spell_state).
    spell_program: String,
    /// Whether `hunspell` is installed (EDTB-6), probed lazily once. Drives the
    /// honest greyed control + "hunspell not installed" note (§7).
    spell_state: SpellState,
    /// Whether the one-time [`spell_program`](Self::spell_program) probe has run.
    spell_probed: bool,
    /// The `F7` spelling-walk dialog state (open flag + the current miss index).
    spell_walk: SpellWalk,
    /// A short honest status line for the last spelling action (§7) — e.g.
    /// "hunspell not installed" / "Spell-check is for text or markdown files" —
    /// shown in the status bar until the next action replaces it.
    spell_notice: Option<String>,
}

impl Default for EditorSurface {
    fn default() -> Self {
        // Seed one empty pane so the surface always has a focused leaf to open
        // documents into (the honest empty state until a buffer is opened).
        let first = PaneId(0);
        let mut panes = HashMap::new();
        panes.insert(first, PaneTabs::empty());
        Self {
            panes,
            tree: Pane::leaf(first),
            focus: first,
            next_pane_id: 1,
            pane_widget_ids: HashMap::new(),
            project: None,
            show_tree: false,
            show_outline: false,
            show_preview: false,
            finder: FileFinder::default(),
            palette: CommandPalette::default(),
            find: FindState::default(),
            project_search: ProjectSearch::default(),
            save_as: SaveAsDialog::default(),
            about_open: false,
            table_picker: TablePicker::default(),
            print: PrintUi::default(),
            references: ReferencesPanel::default(),
            rename: RenameBox::default(),
            lsp_notice: None,
            autosave: load_autosave_prefs(),
            reload: ReloadWatch::default(),
            notice: None,
            terminal: TerminalDock::default(),
            spell_program: spell::HUNSPELL.to_owned(),
            // Unavailable until the first-frame probe proves otherwise — an honest
            // pessimistic default (no fake underlines before the probe).
            spell_state: SpellState::Unavailable,
            spell_probed: false,
            spell_walk: SpellWalk::default(),
            spell_notice: None,
        }
    }
}

/// The `F7` spelling-walk dialog state (EDTB-6): whether it is shown and which
/// visible miss it is parked on. The misses themselves live on the focused
/// [`Doc`]'s [`SpellChecker`]; this holds only the modal's own cursor.
#[derive(Default)]
struct SpellWalk {
    /// Whether the walk dialog is shown.
    open: bool,
    /// The index into the focused document's visible misses — clamped each frame.
    index: usize,
}

/// The Insert Table grid-picker's dialog state (EDTB-3): whether the Word
/// drag-grid overlay is shown. The hover selection is transient per frame
/// ([`format_bar::table_grid`]), so nothing else persists here.
#[derive(Default)]
struct TablePicker {
    /// Whether the picker window is shown.
    open: bool,
}

/// The EDTB-5 print state: the Print Preview overlay's open flag and the last
/// print attempt's honest outcome (surfaced in the preview + the status bar).
#[derive(Default)]
struct PrintUi {
    /// Whether the Print Preview overlay is shown.
    preview_open: bool,
    /// The last print attempt's outcome, or `None` before any attempt (§7 — a
    /// named success/failure, shown until the next attempt).
    outcome: Option<PrintOutcome>,
}

/// One print attempt's human-readable result (§7 — a success message or an honest
/// failure notice), with the flag that colors it (OK vs. DANGER).
struct PrintOutcome {
    /// The message text shown to the operator.
    text: String,
    /// True for a job sent to the queue; false for an honest failure.
    ok: bool,
}

/// The small Save As… path-input dialog (EDTB-1): a path field prefilled with
/// the document's current path, Save/Cancel buttons, and an inline error line
/// when the write fails (§7 — the failure is shown, never swallowed).
#[derive(Default)]
struct SaveAsDialog {
    /// Whether the dialog is shown.
    open: bool,
    /// The live path field text.
    path: String,
    /// The last failed write's message, shown inline until the next attempt.
    error: Option<String>,
    /// Set on open so the path field grabs the keyboard for one frame.
    focus_field: bool,
}

impl SaveAsDialog {
    /// Open the dialog, prefilled with the document's current path (empty for a
    /// scratch buffer) and cleared of any stale error.
    fn open_for(&mut self, current: Option<&Path>) {
        self.open = true;
        self.path = current.map_or_else(String::new, |p| p.display().to_string());
        self.error = None;
        self.focus_field = true;
    }
}

/// How often (app-time seconds) the surface re-stats the focused buffer's file
/// for an external change — a debounce so the poll is not a per-frame `stat`.
const EXTERNAL_POLL_SECS: f64 = 1.5;

/// The operator's decision on the external-change reload prompt (EDITOR-11).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReloadChoice {
    /// Discard the in-memory buffer and take the on-disk copy.
    Reload,
    /// Keep the in-memory buffer; acknowledge the disk change so it stops
    /// prompting (the keep-mine side of a conflict).
    Keep,
}

/// One pending external-change reload prompt (EDITOR-11): the file changed under
/// the focused buffer and the operator has not yet decided.
struct ReloadPrompt {
    /// The file that changed — guards the decision against a tab switch under the
    /// prompt and labels the dialog.
    path: PathBuf,
    /// The buffer also held unsaved edits when the change was detected — the
    /// keep-mine / reload-theirs conflict path (never silently clobbered).
    conflict: bool,
    /// The fresh on-disk stamp to adopt on keep-mine, so the same change does not
    /// re-fire on the next poll.
    stamp: DiskStamp,
}

/// The external-change watch state (EDITOR-11): the pending prompt (if any) and
/// the debounce clock for the next disk poll.
#[derive(Default)]
struct ReloadWatch {
    /// The prompt awaiting the operator's decision, or `None` when nothing pends.
    prompt: Option<ReloadPrompt>,
    /// App-time (seconds) of the next external-change poll — the stat debounce.
    next_poll: f64,
}

impl EditorSurface {
    /// Whether a document is currently open (the focused pane has an active tab)
    /// — the surface is showing the editor, not the empty state.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.doc().is_some()
    }

    /// The path of the active document, if it has one (a scratch buffer has none).
    #[must_use]
    pub fn current_path(&self) -> Option<&Path> {
        self.doc().and_then(|doc| doc.buffer.path())
    }

    // ── EDITOR-6: the focused pane's active tab is the "current document" ─────

    /// The focused pane's active document — the "current document" every seam
    /// reads (the menu bar, the palette, the widget). `None` when the focused
    /// pane has no open tab (the empty state).
    fn doc(&self) -> Option<&Doc> {
        self.panes.get(&self.focus).and_then(PaneTabs::active_doc)
    }

    /// The focused pane's active document (mut) — the write seam every editing
    /// action drives. `None` when the focused pane is empty.
    fn doc_mut(&mut self) -> Option<&mut Doc> {
        let focus = self.focus;
        self.panes
            .get_mut(&focus)
            .and_then(PaneTabs::active_doc_mut)
    }

    /// Allocate a fresh, never-reused [`PaneId`].
    const fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }

    /// Append `doc` as a new tab in the focused pane and make it active — the
    /// one open-a-buffer path every `open_*` seam funnels through.
    fn push_doc(&mut self, doc: Doc) {
        let focus = self.focus;
        if let Some(pane) = self.panes.get_mut(&focus) {
            pane.push(doc);
        }
    }

    /// The number of open panes (split-tree leaves) — one unless the surface has
    /// been split.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        self.tree.leaf_count()
    }

    /// Whether a project root is open (the tree has a folder to render).
    #[must_use]
    pub const fn has_project(&self) -> bool {
        self.project.is_some()
    }

    /// Open `root` as the project folder + show the tree — the EDITOR-9 seam the
    /// Files send / a folder picker drive. Reads the top level immediately (§7).
    pub fn open_folder<P: Into<PathBuf>>(&mut self, root: P) {
        self.project = Some(ProjectTree::new(root));
        self.show_tree = true;
    }

    /// Open the file a project-tree click yielded — the tree→widget routing
    /// ([`open_path`](Self::open_path) via the EDITOR-3 seam). Best-effort: a read
    /// failure (a vanished file / permission) is a silent no-op, never a panic.
    pub fn open_selected(&mut self, path: &Path) {
        let _ = self.open_path(path);
    }

    /// Open an in-memory document seeded with `text` (no path) as a new tab in
    /// the focused pane. The open-a-buffer seam the finder / Files send drive;
    /// also the scratch affordance's backing. A pathless buffer starts no
    /// language server (§7 — nothing to serve).
    pub fn open_text(&mut self, text: &str) {
        self.push_doc(Doc::new(Buffer::from_text(text)));
    }

    /// Open `path` from disk as a tab in the focused pane, starting a
    /// language-server session for it (EDITOR-LSP-2 `didOpen`), rooted at the
    /// open project root when there is one, else the file's directory. If the
    /// file is already open in the focused pane its existing tab is focused
    /// instead of stacking a duplicate.
    ///
    /// # Errors
    /// Returns any [`io::Error`] from reading `path` (missing file, permissions).
    pub fn open_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let path = path.as_ref();
        let focus = self.focus;
        if let Some(pane) = self.panes.get_mut(&focus) {
            if let Some(idx) = pane.find_path(path) {
                pane.focus(idx);
                return Ok(());
            }
        }
        let mut doc = Doc::new(Buffer::open(path)?);
        doc.start_lsp(&self.lsp_root_for(path));
        self.push_doc(doc);
        Ok(())
    }

    /// The workspace root to root a language server at for `path`: the open
    /// project root, else the file's parent directory, else the cwd — a server
    /// always gets an absolute-ish root to index from.
    fn lsp_root_for(&self, path: &Path) -> PathBuf {
        self.project
            .as_ref()
            .map(|tree| tree.root().to_path_buf())
            .or_else(|| {
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(Path::to_path_buf)
            })
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Open a fresh scratch buffer as a new tab in the focused pane.
    pub fn open_scratch(&mut self) {
        self.open_text(SCRATCH_SEED);
    }

    /// Close the focused pane's active document/tab, tearing down its
    /// language-server session (EDITOR-LSP-2 `didClose`). When it was the pane's
    /// last tab and the surface is split, the emptied pane collapses to its
    /// sibling; the last pane simply returns to the empty state.
    pub fn close(&mut self) {
        let focus = self.focus;
        if let Some(idx) = self.panes.get(&focus).and_then(PaneTabs::active_index) {
            self.close_tab_at(focus, idx);
        }
    }

    // ── EDITOR-6: tab + pane operations ──────────────────────────────────────

    /// Close the tab at `idx` in pane `pid`, gracefully shutting its language
    /// server. An emptied non-root pane collapses (its split folds to the
    /// sibling); an emptied lone pane stays as the empty state.
    fn close_tab_at(&mut self, pid: PaneId, idx: usize) {
        let removed = self.panes.get_mut(&pid).and_then(|pane| pane.close(idx));
        if let Some(doc) = removed {
            doc.close_lsp();
        }
        let emptied = self.panes.get(&pid).is_some_and(PaneTabs::is_empty);
        if emptied && self.tree.leaf_count() > 1 {
            self.close_pane(pid);
        }
    }

    /// Split the focused pane along `dir`, seeding the new pane with a duplicate
    /// of the focused pane's active document (so a split immediately shows two
    /// live buffers) and moving focus to it. A no-op if the focus is somehow not
    /// in the tree.
    fn split_focused(&mut self, dir: SplitDir) {
        let focus = self.focus;
        let new = self.alloc_pane_id();
        if !self.tree.split(focus, dir, new) {
            self.next_pane_id -= 1; // hand the unused id back
            return;
        }
        let mut pane = PaneTabs::empty();
        if let Some(doc) = self.duplicate_active() {
            pane.push(doc);
        }
        self.panes.insert(new, pane);
        self.focus = new;
    }

    /// A fresh [`Doc`] mirroring the focused pane's active document — a file's
    /// current on-disk contents re-opened as an independent buffer (its own
    /// language server), or, for a pathless / unreadable buffer, a scratch
    /// seeded with the live text. `None` when the focused pane is empty. Every
    /// copy is a real rope (§7) — the two panes edit independently.
    fn duplicate_active(&self) -> Option<Doc> {
        let src = self.doc()?;
        if let Some(path) = src.buffer.path() {
            let path = path.to_path_buf();
            if let Ok(buffer) = Buffer::open(&path) {
                let mut doc = Doc::new(buffer);
                doc.start_lsp(&self.lsp_root_for(&path));
                return Some(doc);
            }
        }
        Some(Doc::new(Buffer::from_text(&src.buffer.rope().to_string())))
    }

    /// Close the whole pane `pid` (every tab's language server torn down) and
    /// collapse its split to the sibling. Re-focuses the sibling that absorbs
    /// the freed space; if `pid` was the last pane, the surface reseeds one
    /// empty pane (the empty state) so a focused leaf always exists.
    fn close_pane(&mut self, pid: PaneId) {
        let Some(pane) = self.panes.remove(&pid) else {
            return;
        };
        for doc in &pane.tabs {
            doc.close_lsp();
        }
        let fallback = panes::sibling_first_leaf(&self.tree, pid);
        let tree = std::mem::replace(&mut self.tree, Pane::leaf(pid));
        let (rest, _found) = tree.close(pid);
        if let Some(rest) = rest {
            self.tree = rest;
            if self.focus == pid {
                self.focus = fallback.unwrap_or_else(|| self.tree.first_leaf());
            }
        } else {
            // The surface emptied entirely — reseed one empty pane.
            let fresh = self.alloc_pane_id();
            self.panes.insert(fresh, PaneTabs::empty());
            self.tree = Pane::leaf(fresh);
            self.focus = fresh;
        }
    }

    /// Give pane `pid` the keyboard focus (a no-op if it is gone).
    fn focus_pane(&mut self, pid: PaneId) {
        if self.panes.contains_key(&pid) {
            self.focus = pid;
        }
    }

    /// Move the focus to the geometrically adjacent pane (`Alt+arrows`), if any.
    fn navigate_focus(&mut self, dir: NavDir) {
        if let Some(target) = panes::navigate(&self.tree, self.focus, dir) {
            self.focus = target;
        }
    }

    /// Apply one deferred [`LeafAction`] from a pane's tab bar / body — run after
    /// every pane has rendered so tree/registry edits sit outside the pane borrow.
    fn apply_leaf_action(&mut self, pid: PaneId, action: LeafAction) {
        match action {
            LeafAction::Focus => self.focus_pane(pid),
            LeafAction::NewTab => {
                self.focus_pane(pid);
                self.open_scratch();
            }
            LeafAction::SelectTab(idx) => {
                self.focus_pane(pid);
                if let Some(pane) = self.panes.get_mut(&pid) {
                    pane.focus(idx);
                }
            }
            LeafAction::CloseTab(idx) => {
                self.focus_pane(pid);
                self.close_tab_at(pid, idx);
            }
            LeafAction::MoveTab { from, to } => {
                if let Some(pane) = self.panes.get_mut(&pid) {
                    pane.move_tab(from, to);
                }
            }
        }
    }

    // ── EDITOR-7: the fuzzy finder + command palette ─────────────────────────

    /// Whether a modal overlay (the fuzzy file-finder, the command palette, or a
    /// dialog — Save As / About / Insert Table / Print Preview) is currently up.
    /// The shell can key
    /// off this to suppress its own global chords while the editor surface is
    /// capturing the keyboard for an overlay.
    #[must_use]
    pub const fn overlay_active(&self) -> bool {
        self.finder.is_open()
            || self.palette.is_open()
            || self.find.is_open()
            || self.project_search.is_open()
            || self.save_as.open
            || self.about_open
            || self.table_picker.open
            || self.print.preview_open
            || self.references.is_open()
            || self.rename.is_open()
            || self.reload.prompt.is_some()
            || self.spell_walk.open
    }

    /// Open the fuzzy file-finder (the `Cmd`/`Ctrl-P` seam), rooted at the open
    /// project root if there is one, else the current working directory — so
    /// `Cmd`/`Ctrl-P` is always reachable. A silent no-op if neither root resolves.
    pub(crate) fn open_finder(&mut self) {
        let root = self
            .project
            .as_ref()
            .map(|tree| tree.root().to_path_buf())
            .or_else(|| std::env::current_dir().ok());
        if let Some(root) = root {
            self.close_search_overlays(); // only one overlay at a time
            self.finder.open_at(&root);
        }
    }

    /// Toggle the command palette (the `Cmd`/`Ctrl-Shift-P` seam), closing the
    /// finder / search overlays so only one overlay is up at a time.
    pub(crate) fn toggle_palette(&mut self) {
        self.finder.close();
        self.find.close();
        self.project_search.close();
        self.palette.toggle();
    }

    // ── EDITOR-8: the in-buffer find/replace + project search seams ──────────

    /// Close every non-palette overlay (finder / palette / find bar / project
    /// search) so only the one being opened is up — the "one overlay at a time"
    /// invariant every `open_*` seam re-asserts.
    fn close_search_overlays(&mut self) {
        self.finder.close();
        self.palette.close();
        self.find.close();
        self.project_search.close();
    }

    /// Open the in-buffer find bar (`Ctrl-F`).
    pub(crate) fn open_find(&mut self) {
        self.close_search_overlays();
        self.find.open_find();
    }

    /// Open the in-buffer find + replace bar (`Ctrl-H`).
    pub(crate) fn open_replace(&mut self) {
        self.close_search_overlays();
        self.find.open_replace();
    }

    /// Open the project-wide search (`Ctrl-Shift-F`), rooted at the open project
    /// root if there is one, else the current working directory — so it is always
    /// reachable. A silent no-op if neither root resolves.
    pub(crate) fn open_project_search(&mut self) {
        let root = self
            .project
            .as_ref()
            .map(|tree| tree.root().to_path_buf())
            .or_else(|| std::env::current_dir().ok());
        if let Some(root) = root {
            self.close_search_overlays();
            self.project_search.open_at(root);
        }
    }

    /// Recompute the in-buffer find matches against the active buffer while the
    /// find bar is open — so the widget paints this frame's matches and the bar's
    /// counter tracks edits + toggle flips. A no-op while the bar is closed; the
    /// matches are dropped when no document is open.
    fn refresh_find_matches(&mut self) {
        if !self.find.is_open() {
            return;
        }
        match self.doc().map(|doc| doc.buffer.rope().to_string()) {
            Some(text) => self.find.recompute(&text),
            None => self.find.clear_matches(),
        }
    }

    /// The find-match bands to paint this frame: the resolved ranges + the current
    /// index, or empty when the bar is closed. Owned so the caller can pass them
    /// past the `&mut self` render borrow.
    fn find_paint_bands(&self) -> (Vec<Range<usize>>, Option<usize>) {
        if self.find.is_open() && !self.find.matches().is_empty() {
            (
                self.find.matches().to_vec(),
                Some(self.find.current_index()),
            )
        } else {
            (Vec::new(), None)
        }
    }

    /// Reveal the current find match — place the caret at its start (which scrolls
    /// it into view through the existing [`EditorView::place_cursor`] reveal).
    fn reveal_current_match(&mut self) {
        let Some(range) = self.find.current_range() else {
            return;
        };
        if let Some(doc) = self.doc_mut() {
            doc.view.place_cursor(&doc.buffer, range.start);
        }
    }

    /// Replace the current find match with the replacement text — a real,
    /// undoable rope edit through [`EditorView::replace_range`] (§7). The matches
    /// recompute next frame; the current index then slides onto the following
    /// match (the standard replace-then-advance behaviour).
    fn replace_current_match(&mut self) {
        let Some(range) = self.find.current_range() else {
            return;
        };
        let replacement = self.find.replacement().to_owned();
        if let Some(doc) = self.doc_mut() {
            doc.view.replace_range(&mut doc.buffer, range, &replacement);
        }
    }

    /// Replace every find match with the replacement text as ONE undo step
    /// ([`EditorView::replace_all`]) and record the count for the bar to show.
    fn replace_all_matches(&mut self) {
        let ranges = self.find.matches().to_vec();
        if ranges.is_empty() {
            return;
        }
        let replacement = self.find.replacement().to_owned();
        if let Some(doc) = self.doc_mut() {
            let n = doc.view.replace_all(&mut doc.buffer, &ranges, &replacement);
            self.find.set_last_replaced(n);
        }
    }

    /// Open a project-search hit: open the file, then jump the caret onto the
    /// match — re-finding the query on the opened line for an exact,
    /// backend-independent char column (else the line start).
    fn open_hit(&mut self, hit: &search::Hit) {
        if self.open_path(&hit.path).is_err() {
            return;
        }
        let query = self.project_search.query().to_owned();
        let opts = self.project_search.options();
        if let Some(doc) = self.doc_mut() {
            let last_line = doc.buffer.len_lines().saturating_sub(1);
            let line = hit.line.saturating_sub(1).min(last_line);
            let line_start = doc.buffer.line_to_char(line);
            let line_text = doc.buffer.line(line);
            let col = search::find_matches(&line_text, &query, opts)
                .first()
                .map_or(0, |r| r.start);
            doc.view.place_cursor(&doc.buffer, line_start + col);
        }
    }

    // ── EDITOR-LSP-3: goto-definition / references / rename / format ──────────

    /// The focused document's `(path, line, character)` at the caret — the LSP
    /// wire position (zero-based line, UTF-16 column) a navigation request needs.
    /// `None` for a pathless / empty document.
    fn lsp_cursor_pos(&self) -> Option<(PathBuf, u32, u32)> {
        let doc = self.doc()?;
        let path = doc.buffer.path()?.to_path_buf();
        let (line, character) = lsp_nav::char_to_lsp_pos(&doc.buffer, doc.view.cursor());
        Some((path, line, character))
    }

    /// The honest gated-state notice for the focused document's server (§7) —
    /// names the missing command where the [`LspState`](crate::lsp::LspState)
    /// knows it (e.g. `rust-analyzer: not found`), else "No language server".
    fn lsp_unavailable_notice(&self) -> String {
        self.doc()
            .and_then(|d| d.lsp.as_ref())
            .and_then(lsp_status)
            .map_or_else(|| "No language server".to_owned(), |s| s.text)
    }

    /// Whether the focused document's server accepts a request — the live gate the
    /// action methods share: `f` is the client call (`goto_definition`, …),
    /// returning whether it dispatched.
    fn lsp_request(&self, f: impl FnOnce(&LspClient) -> bool) -> bool {
        self.doc().and_then(|d| d.lsp.as_ref()).is_some_and(f)
    }

    /// Goto-definition (`F12`): request `textDocument/definition` for the caret.
    /// The reply jumps to the target (see [`Self::route_reply`]); a gated server
    /// is an honest no-op with a status (§7).
    fn lsp_goto_definition(&mut self) {
        let Some((path, line, character)) = self.lsp_cursor_pos() else {
            return;
        };
        let sent = self.lsp_request(|c| c.goto_definition(&path, line, character));
        self.lsp_notice = Some(if sent {
            "Go to definition\u{2026}".to_owned()
        } else {
            self.lsp_unavailable_notice()
        });
    }

    /// Find-references (`Shift-F12`): request `textDocument/references` for the
    /// caret. The reply opens the results list (see [`Self::route_reply`]).
    fn lsp_find_references(&mut self) {
        let Some((path, line, character)) = self.lsp_cursor_pos() else {
            return;
        };
        let sent = self.lsp_request(|c| c.find_references(&path, line, character));
        self.lsp_notice = Some(if sent {
            "Finding references\u{2026}".to_owned()
        } else {
            self.lsp_unavailable_notice()
        });
    }

    /// Rename (`F2`): fire `prepareRename` and open the rename box prefilled with
    /// the word under the cursor. A gated server is an honest no-op — no box, just
    /// the status (§7), since a rename cannot happen without a server.
    fn lsp_start_rename(&mut self) {
        let Some((path, line, character)) = self.lsp_cursor_pos() else {
            return;
        };
        if !self.lsp_request(|c| c.prepare_rename(&path, line, character)) {
            self.lsp_notice = Some(self.lsp_unavailable_notice());
            return;
        }
        let prefill = self
            .doc()
            .and_then(|d| lsp_nav::word_under_cursor(&d.buffer, d.view.cursor()))
            .map(|(_, word)| word)
            .unwrap_or_default();
        self.close_search_overlays();
        self.rename.open_for(path, line, character, &prefill);
    }

    /// Fire `textDocument/rename` with the submitted `new_name` for the box's
    /// stored symbol position; the reply applies the workspace edit.
    fn lsp_fire_rename(&mut self, new_name: &str) {
        let (path, line, character) = self.rename.target();
        let sent = self.lsp_request(|c| c.rename(&path, line, character, new_name));
        self.lsp_notice = Some(if sent {
            "Renaming\u{2026}".to_owned()
        } else {
            self.lsp_unavailable_notice()
        });
    }

    /// Format (`Shift-Alt-F`): request `textDocument/formatting` for the whole
    /// document; the reply applies the edits to the buffer.
    fn lsp_format_document(&mut self) {
        let Some(path) = self
            .doc()
            .and_then(|d| d.buffer.path())
            .map(Path::to_path_buf)
        else {
            return;
        };
        let sent = self.lsp_request(|c| c.format(&path));
        self.lsp_notice = Some(if sent {
            "Formatting\u{2026}".to_owned()
        } else {
            self.lsp_unavailable_notice()
        });
    }

    /// Poll the focused document's server for completed navigation replies
    /// (EDITOR-LSP-3) and route each — gated on
    /// [`LspClient::replies_epoch`](crate::lsp::LspClient::replies_epoch) so a
    /// quiet frame does nothing. Called once per frame from [`editor_panel`].
    fn pump_lsp_replies(&mut self) {
        let drained = {
            let Some(doc) = self.doc() else { return };
            let Some(client) = doc.lsp.as_ref() else {
                return;
            };
            let epoch = client.replies_epoch();
            if epoch == doc.lsp_reply_epoch {
                return;
            }
            (epoch, client.take_replies())
        };
        let (epoch, replies) = drained;
        if let Some(doc) = self.doc_mut() {
            doc.lsp_reply_epoch = epoch;
        }
        for reply in replies {
            self.route_reply(reply);
        }
    }

    /// Route one completed [`LspReply`] to its seam: definition → jump,
    /// references → list, prepareRename → refine the box, rename → apply the
    /// workspace edit, format → apply the edits.
    fn route_reply(&mut self, reply: LspReply) {
        match reply {
            LspReply::Definition(locs) => self.route_definition(&locs),
            LspReply::References(locs) => self.route_references(&locs),
            LspReply::PrepareRename(prepare) => {
                if let Some(placeholder) = prepare.and_then(|p| p.placeholder) {
                    self.rename.set_placeholder(&placeholder);
                }
            }
            LspReply::Rename(edit) => self.route_rename(&edit),
            LspReply::Format(edits) => self.route_format(&edits),
        }
    }

    /// A definition reply: jump to the first target through the shared open+jump
    /// seam, or an honest "no definition found" notice.
    fn route_definition(&mut self, locs: &[Location]) {
        let Some(loc) = locs.first() else {
            self.lsp_notice = Some("No definition found".to_owned());
            return;
        };
        let path = loc.path.clone();
        self.jump_to_location(&path, loc.range.start_line, loc.range.start_character);
        self.lsp_notice = None;
    }

    /// A references reply: build the results list (each row's source-line preview
    /// read from the open buffer or disk) and open the overlay, or an honest "no
    /// references found" notice.
    fn route_references(&mut self, locs: &[Location]) {
        if locs.is_empty() {
            self.lsp_notice = Some("No references found".to_owned());
            return;
        }
        let rows: Vec<RefRow> = locs
            .iter()
            .map(|loc| {
                let preview = self.reference_preview(&loc.path, loc.range.start_line);
                RefRow::from_location(loc, &preview)
            })
            .collect();
        self.close_search_overlays();
        self.lsp_notice = Some(format!("{} references", rows.len()));
        self.references.open_with(rows);
    }

    /// A rename reply: apply the cross-file [`WorkspaceEdit`] — each file to its
    /// open buffer (undoable) or on disk (closed) — and report the file count.
    fn route_rename(&mut self, edit: &WorkspaceEdit) {
        if edit.is_empty() {
            self.lsp_notice = Some("Nothing to rename".to_owned());
            return;
        }
        let mut applied = 0usize;
        for (path, edits) in &edit.changes {
            if self.apply_edits_open_or_disk(path, edits) {
                applied += 1;
            }
        }
        self.lsp_notice = Some(if applied == 1 {
            "Renamed 1 file".to_owned()
        } else {
            format!("Renamed {applied} files")
        });
    }

    /// A format reply: apply the edits to the focused buffer (undoable), or an
    /// honest "no formatting changes" notice.
    fn route_format(&mut self, edits: &[TextEdit]) {
        if edits.is_empty() {
            self.lsp_notice = Some("No formatting changes".to_owned());
            return;
        }
        if let Some(doc) = self.doc_mut() {
            lsp_nav::apply_edits_to_open_buffer(&mut doc.view, &mut doc.buffer, edits);
        }
        self.lsp_notice = Some("Formatted".to_owned());
    }

    /// Apply one file's `edits` to its open buffer (undoable, preferred) when it
    /// is open in any pane, else rewrite it on disk. Returns whether the file was
    /// touched — the count that drives the rename notice.
    fn apply_edits_open_or_disk(&mut self, path: &Path, edits: &[TextEdit]) -> bool {
        for pane in self.panes.values_mut() {
            for doc in &mut pane.tabs {
                if doc.buffer.path() == Some(path) {
                    lsp_nav::apply_edits_to_open_buffer(&mut doc.view, &mut doc.buffer, edits);
                    return true;
                }
            }
        }
        lsp_nav::apply_edits_on_disk(path, edits).is_ok()
    }

    /// The trimmed source line for a reference preview: the live open buffer's
    /// line when the file is open, else the on-disk line, else empty.
    fn reference_preview(&self, path: &Path, line0: u32) -> String {
        for pane in self.panes.values() {
            for doc in &pane.tabs {
                if doc.buffer.path() == Some(path) {
                    let last = doc.buffer.len_lines().saturating_sub(1);
                    return doc.buffer.line((line0 as usize).min(last));
                }
            }
        }
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.lines().nth(line0 as usize).map(str::to_owned))
            .unwrap_or_default()
    }

    // ── EDITOR-12: the symbol outline ────────────────────────────────────────

    /// The focused document's outline symbols (functions / types / impls), derived
    /// from its tree-sitter tree — empty for a plain-text / pathless buffer (the
    /// honest empty state the panel then shows). Owned so the caller can render past
    /// the `&mut self` render borrow.
    fn active_symbols(&self) -> Vec<outline::Symbol> {
        self.doc()
            .and_then(|doc| {
                doc.highlight
                    .as_ref()
                    .map(|hl| hl.symbols(doc.buffer.rope()))
            })
            .unwrap_or_default()
    }

    /// Whether the focused document has a syntax grammar (a highlighter) — the
    /// outline uses it to tell "no outline for this file type" from "no symbols".
    fn active_has_grammar(&self) -> bool {
        self.doc().is_some_and(|doc| doc.highlight.is_some())
    }

    /// Jump the focused document's caret to rope char offset `idx` — the outline
    /// row click seam, reusing the SAME [`EditorView::place_cursor`] reveal the
    /// EDITOR-8 / LSP jumps use (§6). A no-op when no document is open.
    fn jump_caret(&mut self, idx: usize) {
        if let Some(doc) = self.doc_mut() {
            doc.view.place_cursor(&doc.buffer, idx);
        }
    }

    /// Open `path` and place the caret at the LSP `(line, character)` — the shared
    /// jump seam reused by goto-definition and a reference pick (the EDITOR-8
    /// open+jump idiom). A read failure is a silent no-op (a vanished file).
    fn jump_to_location(&mut self, path: &Path, line: u32, character: u32) {
        if self.open_path(path).is_err() {
            return;
        }
        if let Some(doc) = self.doc_mut() {
            let idx = lsp_nav::char_of(&doc.buffer, line, character);
            doc.view.place_cursor(&doc.buffer, idx);
        }
    }

    // ── EDTB-6: spell-check (hunspell) ───────────────────────────────────────

    /// Probe `hunspell` once, on the first frame (EDTB-6). Deferred out of the
    /// constructor so building an [`EditorSurface`] never spawns a subprocess; the
    /// result gates the squiggles + the honest "not installed" chrome. Under the
    /// crate's own tests the probe is skipped and the checker stays
    /// [`SpellState::Unavailable`], so the suite never launches `hunspell` (the LSP
    /// `build_lsp_client` idiom — deterministic + subprocess-free).
    // Not `const`: the production body calls `spell::probe` (spawns a subprocess);
    // only the `cfg(test)` body (which elides that) looks const to clippy.
    #[allow(clippy::missing_const_for_fn)]
    fn ensure_spell_probe(&mut self) {
        if self.spell_probed {
            return;
        }
        self.spell_probed = true;
        #[cfg(not(test))]
        {
            self.spell_state = spell::probe(&self.spell_program);
        }
    }

    /// Open the `F7` spelling-walk dialog (EDTB-6) — or, when it can't run, set the
    /// honest reason as the status note instead of a dead dialog (§7): no document,
    /// a code buffer, or a missing `hunspell` each name themselves.
    fn open_spell_walk(&mut self) {
        let Some(doc) = self.doc() else {
            self.spell_notice = Some("Open a document to spell-check".to_owned());
            return;
        };
        if !doc.spellcheckable() {
            self.spell_notice = Some("Spell-check is for text or markdown files".to_owned());
            return;
        }
        if !self.spell_state.is_ready() {
            self.spell_notice = Some(SpellState::Unavailable.notice().to_owned());
            return;
        }
        self.close_search_overlays();
        self.spell_walk.open = true;
        self.spell_walk.index = 0;
        self.spell_notice = None;
    }

    /// The focused document's current visible miss for the walk (clamped to the
    /// walk index), or `None` when the list is empty / there is no document.
    fn spell_current_miss(&self) -> Option<SpellMiss> {
        let doc = self.doc()?;
        let misses = doc.spell.misses();
        misses
            .get(self.spell_walk.index.min(misses.len().saturating_sub(1)))
            .cloned()
    }

    /// Replace the walk's current miss with `replacement` as a real, undoable rope
    /// edit (EDTB-6) — the same [`EditorView::replace_range`] seam the find/replace
    /// bar uses (§6). Guarded: if the stored span no longer holds the missed word
    /// (an edit shifted it since the last check), the replace is skipped rather
    /// than corrupting unrelated text (§7). The next background pass re-resolves
    /// the misses against the edited buffer.
    fn spell_replace_current(&mut self, miss: &SpellMiss, replacement: &str) {
        if let Some(doc) = self.doc_mut() {
            let end = miss.chars.end.min(doc.buffer.len_chars());
            if end <= miss.chars.start {
                return;
            }
            let here = doc.buffer.rope().slice(miss.chars.start..end).to_string();
            if here != miss.word {
                return; // stale span — an edit moved the word; skip, don't corrupt
            }
            doc.view
                .replace_range(&mut doc.buffer, miss.chars.start..end, replacement);
        }
    }

    /// Ignore every occurrence of the walk's current word this session
    /// (Ignore-All) — filtered out of the squiggles at once (EDTB-6).
    fn spell_ignore_all(&mut self, word: &str) {
        if let Some(doc) = self.doc_mut() {
            doc.spell.ignore_all(word);
        }
    }

    /// Add the walk's current word to the session personal dictionary
    /// (Add-to-dictionary) — un-squiggled at once (EDTB-6).
    fn spell_add_word(&mut self, word: &str) {
        if let Some(doc) = self.doc_mut() {
            doc.spell.add_to_dictionary(word);
        }
    }

    /// Run one [`PaletteCommand`] against the live surface seams — the dispatch the
    /// palette's Enter/click routes to. Each arm invokes a real seam (§7 — no dead
    /// entries): Save writes the buffer, the toggles flip real view/panel state,
    /// Close/Open act on the document, Open Folder roots the tree at the cwd. A
    /// command whose precondition is absent (Save/Toggle-Wrap with no open
    /// document) is a genuine no-op, never a panic.
    pub(crate) fn run_command(&mut self, cmd: PaletteCommand) {
        match cmd {
            PaletteCommand::Save => {
                if let Some(doc) = self.doc_mut() {
                    // A scratch buffer (no path) can't save; that's an honest no-op,
                    // surfaced by the dirty marker staying lit — not a panic.
                    let _ = doc.buffer.save();
                }
            }
            PaletteCommand::OpenScratch => self.open_scratch(),
            PaletteCommand::ToggleTree => self.show_tree = !self.show_tree,
            PaletteCommand::ToggleOutline => self.show_outline = !self.show_outline,
            PaletteCommand::ToggleTerminal => self.toggle_terminal(),
            PaletteCommand::ToggleWrap => {
                if let Some(doc) = self.doc_mut() {
                    doc.view.toggle_wrap();
                }
            }
            PaletteCommand::CloseDoc => self.close(),
            PaletteCommand::OpenFolderCwd => {
                if let Ok(cwd) = std::env::current_dir() {
                    self.open_folder(cwd);
                }
            }
        }
    }

    // ── EDITOR-10: the integrated terminal dock ─────────────────────────────

    /// The working directory a freshly opened terminal spawns its shell in: the
    /// open project root, else the current working directory — the same root
    /// resolution [`open_finder`](Self::open_finder) /
    /// [`open_project_search`](Self::open_project_search) use, so the dock is
    /// always reachable.
    fn terminal_cwd(&self) -> Option<PathBuf> {
        self.project
            .as_ref()
            .map(|tree| tree.root().to_path_buf())
            .or_else(|| std::env::current_dir().ok())
    }

    /// Toggle the integrated terminal dock (Ctrl+Backtick / View → Terminal / the
    /// surface-strip button / the palette). Opening it lazily spawns a real login
    /// shell in the project cwd on the first open; toggling only shows/hides
    /// thereafter, so the session is never lost (EDITOR-10).
    pub(crate) fn toggle_terminal(&mut self) {
        let cwd = self.terminal_cwd();
        self.terminal.toggle(cwd.as_deref());
    }

    /// Intercept the terminal-toggle chord (Ctrl+Backtick) at the panel level —
    /// consumed BEFORE the text widget clones this frame's events (mirroring the
    /// other panel-level chord intercepts) so it flips the dock instead of typing
    /// a backtick into the buffer.
    fn handle_terminal_chord(&mut self, ui: &Ui) {
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Backtick)) {
            self.toggle_terminal();
        }
    }

    // ── EDTB-7: the split markdown preview ──────────────────────────────────

    /// Toggle the split markdown-preview pane (View → Preview / the toolbar
    /// toggle). Opening it is honest-gated on a previewable (md/text) focused
    /// document: on a code buffer this is a genuine no-op (and the menu/toolbar
    /// grey the control out), never a pane of nonsense (§7). Closing always works.
    pub(crate) fn toggle_preview(&mut self) {
        if self.show_preview {
            self.show_preview = false;
        } else if self.doc().is_some_and(Doc::previewable) {
            self.show_preview = true;
        }
    }

    // ── EDTB-1: the Word-97 menu bar + Standard toolbar ─────────────────────

    /// Snapshot the enablement + toggle/zoom state the menu bar and toolbar
    /// render from this frame — the bars' one read seam into the surface.
    pub(crate) fn menu_context(&self) -> MenuContext {
        let doc = self.doc();
        MenuContext {
            has_doc: doc.is_some(),
            has_selection: doc.is_some_and(|d| d.view.has_selection()),
            can_undo: doc.is_some_and(|d| d.view.can_undo()),
            can_redo: doc.is_some_and(|d| d.view.can_redo()),
            tree_shown: self.show_tree,
            terminal_shown: self.terminal.is_shown(),
            wrap_on: doc.is_some_and(|d| d.view.wrap()),
            zoom_percent: doc.map(|d| d.view.zoom_percent()),
            // The Format strip Style dropdown reads the primary caret line's
            // heading level (EDTB-3).
            heading_level: doc.map(|d| {
                let (line, _) = d.view.line_col(&d.buffer);
                heading_level_of(&d.buffer, line - 1)
            }),
            // EDTB-6 — the Tools/toolbar Spelling control greys out unless
            // `hunspell` is installed and the buffer is a spell-checkable type.
            spell_available: self.spell_state.is_ready(),
            spellcheckable: doc.is_some_and(Doc::spellcheckable),
            // EDTB-7 — the View → Preview / toolbar toggle greys out on a code
            // buffer and reads back the pane's shown state.
            preview_available: doc.is_some_and(Doc::previewable),
            preview_shown: self.show_preview,
        }
    }

    /// Dispatch one menu-bar / toolbar action (EDTB-1). The Word chrome drives
    /// the SAME seams the palette and the widget's chords drive: where a
    /// [`PaletteCommand`] already names the operation, the arm delegates to
    /// [`run_command`](Self::run_command) (§6 — one implementation); the editing
    /// arms call the same `EditorView` fns the keyboard runs. An action whose
    /// precondition is absent (Undo with nothing to undo, Zoom with no document)
    /// is a genuine no-op — the bars also grey those items out.
    pub(crate) fn run_action(&mut self, ctx: &egui::Context, action: MenuAction) {
        match action {
            MenuAction::NewScratch => self.run_command(PaletteCommand::OpenScratch),
            MenuAction::OpenFinder => self.open_finder(),
            MenuAction::OpenFolderCwd => self.run_command(PaletteCommand::OpenFolderCwd),
            MenuAction::Save => self.run_command(PaletteCommand::Save),
            MenuAction::SaveAs => {
                let current = self.current_path().map(Path::to_path_buf);
                self.save_as.open_for(current.as_deref());
            }
            MenuAction::CloseDoc => self.run_command(PaletteCommand::CloseDoc),
            // ── EDTB-5: Print via CUPS ───────────────────────────────────────
            MenuAction::Print => self.do_print(),
            MenuAction::PrintPreview => {
                // A fresh preview drops the stale outcome from the last attempt.
                self.print.outcome = None;
                self.print.preview_open = true;
            }
            // ── EDTB-6: Spelling (hunspell) ──────────────────────────────────
            MenuAction::SpellCheck => self.open_spell_walk(),
            MenuAction::Undo => {
                if let Some(doc) = self.doc_mut() {
                    doc.view.undo(&mut doc.buffer);
                }
            }
            MenuAction::Redo => {
                if let Some(doc) = self.doc_mut() {
                    doc.view.redo(&mut doc.buffer);
                }
            }
            MenuAction::Cut => {
                // The widget's Ctrl-X arm exactly: copy the selection, then
                // delete it — the same two seams, menu-driven.
                self.copy_selection(ctx);
                if let Some(doc) = self.doc_mut() {
                    doc.view.delete_selections(&mut doc.buffer);
                }
            }
            MenuAction::Copy => self.copy_selection(ctx),
            MenuAction::Paste => {
                // Ask the platform for its clipboard: the backend answers with
                // the same `Event::Paste` the widget's Ctrl-V path inserts —
                // one insert seam. (The DRM backend has no clipboard yet; there
                // this is the same honest no-op the Ctrl-V chord is today.)
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            MenuAction::SelectAll => {
                if let Some(doc) = self.doc_mut() {
                    doc.view.select_all(&doc.buffer);
                }
            }
            MenuAction::ToggleTree => self.run_command(PaletteCommand::ToggleTree),
            MenuAction::ToggleTerminal => self.run_command(PaletteCommand::ToggleTerminal),
            // ── EDTB-7: the split markdown preview ───────────────────────────
            MenuAction::TogglePreview => self.toggle_preview(),
            MenuAction::ToggleWrap => self.run_command(PaletteCommand::ToggleWrap),
            MenuAction::CommandPalette => self.toggle_palette(),
            MenuAction::About => self.about_open = true,
            MenuAction::Zoom(percent) => {
                if let Some(doc) = self.doc_mut() {
                    doc.view.set_zoom_percent(percent);
                }
            }
            // ── EDTB-3: the Formatting strip / Insert & Format menus ─────────
            // Each drives the landed `md_actions` engine on the live buffer as
            // ONE operator undo step (the widget's `apply_md` records the
            // engine's undo-group count). A no-op with no document (§7).
            MenuAction::Heading(level) => {
                if let Some(doc) = self.doc_mut() {
                    doc.view.apply_md(&mut doc.buffer, |b, spans| {
                        md_actions::set_heading(b, spans, level)
                    });
                }
            }
            MenuAction::Wrap(marker) => {
                if let Some(doc) = self.doc_mut() {
                    let m = marker.marker();
                    doc.view.apply_md(&mut doc.buffer, |b, spans| {
                        md_actions::toggle_wrap(b, spans, m)
                    });
                }
            }
            MenuAction::List(style) => {
                if let Some(doc) = self.doc_mut() {
                    let kind = match style {
                        ListStyle::Bullet => ListKind::Bullet,
                        ListStyle::Numbered => ListKind::Numbered,
                    };
                    doc.view.apply_md(&mut doc.buffer, |b, spans| {
                        md_actions::toggle_line_prefix(b, spans, kind)
                    });
                }
            }
            MenuAction::Indent(delta) => {
                if let Some(doc) = self.doc_mut() {
                    let d = isize::from(delta);
                    doc.view.apply_md(&mut doc.buffer, |b, spans| {
                        md_actions::shift_indent(b, spans, d)
                    });
                }
            }
            MenuAction::InsertTablePicker => self.table_picker.open = true,
            MenuAction::InsertTable { rows, cols } => self.insert_table_at_caret(rows, cols),
        }
    }

    /// Insert a `rows`×`cols` markdown table skeleton at the primary caret —
    /// what the grid-picker (EDTB-3) commits, and the seam
    /// [`MenuAction::InsertTable`] dispatches (menu/test parity). One undo step;
    /// a no-op with no open document.
    fn insert_table_at_caret(&mut self, rows: u8, cols: u8) {
        if let Some(doc) = self.doc_mut() {
            let caret = doc.view.cursor();
            let (rows, cols) = (usize::from(rows), usize::from(cols));
            doc.view.apply_md(&mut doc.buffer, |b, _spans| {
                md_actions::insert_table(b, caret, rows, cols)
            });
        }
    }

    /// Copy every caret's selection to the platform clipboard — the Edit-menu /
    /// toolbar Copy, reading the exact same `EditorView::selected_text` the
    /// widget's `Ctrl-C` arm reads (§6). A no-op with no selection.
    fn copy_selection(&self, ctx: &egui::Context) {
        if let Some(doc) = self.doc() {
            if let Some(text) = doc.view.selected_text(&doc.buffer) {
                ctx.copy_text(text);
            }
        }
    }

    /// Commit the Save As dialog: write the buffer to the entered path
    /// ([`Buffer::save_as`], which adopts the path), re-pick the syntax
    /// highlighter for the new extension (exactly as `Doc::new` does on open),
    /// and close the dialog. A failed write keeps the dialog open with the
    /// error shown inline (§7).
    fn save_as_commit(&mut self) {
        let path = self.save_as.path.trim().to_owned();
        if path.is_empty() {
            self.save_as.error = Some("Enter a path to save to.".to_owned());
            return;
        }
        let Some(doc) = self.doc_mut() else {
            // The document vanished under the dialog — nothing left to save.
            self.save_as.open = false;
            return;
        };
        match doc.buffer.save_as(&path) {
            Ok(()) => {
                doc.highlight = doc.buffer.path().and_then(Highlighter::for_path);
                self.save_as.open = false;
            }
            Err(err) => self.save_as.error = Some(err.to_string()),
        }
    }

    // ── EDITOR-11: save / autosave / external-change reload ──────────────────

    /// Ctrl-S — write the focused buffer to its path (the EDITOR-2 save seam),
    /// clearing dirty. A pathless (scratch) buffer has nowhere to write, so this
    /// opens the Save As overlay to prompt for a path (the honest idiom, §7); a
    /// write failure is surfaced as a status notice, never swallowed. A no-op with
    /// no open document.
    fn save_focused(&mut self) {
        let has_path = match self.doc() {
            None => return,
            Some(doc) => doc.buffer.path().is_some(),
        };
        if !has_path {
            self.save_as.open_for(None);
            return;
        }
        let result = self.doc_mut().map(|doc| doc.buffer.save());
        self.notice = match result {
            Some(Ok(())) => Some("Saved".to_owned()),
            Some(Err(err)) => Some(format!("Save failed: {err}")),
            None => None,
        };
    }

    /// Flip the persisted autosave toggle (the status-bar control) and write the
    /// preference back to the editor config (EDITOR-11). A config path that does
    /// not resolve / fails to write is a silent no-op — the toggle still holds for
    /// this session.
    fn set_autosave(&mut self, enabled: bool) {
        self.autosave.enabled = enabled;
        if let Some(path) = autosave_config_path() {
            let _ = write_autosave_prefs_at(&path, self.autosave);
        }
        self.notice = Some(
            if enabled {
                "Autosave on"
            } else {
                "Autosave off"
            }
            .to_owned(),
        );
    }

    /// The per-frame autosave debounce (EDITOR-11). Every open buffer's idle time
    /// is tracked — re-armed whenever its revision moves, so a mid-keystroke burst
    /// never triggers a write — and, only when autosave is enabled, a dirty
    /// path-backed buffer is written once it has been idle past the configured
    /// window. `now` is egui's app-time in seconds (deterministic in headless
    /// tests). Own writes re-baseline the buffer, so nothing re-saves until the
    /// next edit.
    fn tick_autosave(&mut self, now: f64) {
        let enabled = self.autosave.enabled;
        let idle = self.autosave.idle_secs;
        let mut saved_any = false;
        for pane in self.panes.values_mut() {
            for doc in &mut pane.tabs {
                let rev = doc.buffer.revision();
                if rev != doc.autosave_rev {
                    doc.autosave_rev = rev;
                    doc.idle_since = now;
                    continue;
                }
                let due = enabled
                    && doc.buffer.is_dirty()
                    && doc.buffer.path().is_some()
                    && now - doc.idle_since >= idle;
                if due && doc.buffer.save().is_ok() {
                    saved_any = true;
                }
            }
        }
        if saved_any {
            self.notice = Some("Autosaved".to_owned());
        }
    }

    /// Poll the focused buffer's file for an external change (EDITOR-11), debounced
    /// to at most once per [`EXTERNAL_POLL_SECS`]. On a detected change it opens the
    /// reload prompt (flagging the dirty-conflict case); it never reloads on its
    /// own — the operator decides. Skipped while any overlay/dialog (including a
    /// prompt already up) holds the keyboard, so it never interrupts a live gesture.
    fn poll_external_change(&mut self, now: f64) {
        if self.overlay_active() || now < self.reload.next_poll {
            return;
        }
        self.reload.next_poll = now + EXTERNAL_POLL_SECS;
        let prompt = {
            let Some(doc) = self.doc() else {
                return;
            };
            let Some(path) = doc.buffer.path().map(Path::to_path_buf) else {
                return;
            };
            match doc.buffer.disk_state() {
                DiskState::Changed(stamp) => Some(ReloadPrompt {
                    path,
                    conflict: doc.buffer.is_dirty(),
                    stamp,
                }),
                DiskState::Unchanged | DiskState::Unknown => None,
            }
        };
        if prompt.is_some() {
            self.reload.prompt = prompt;
        }
    }

    /// Reload-theirs (EDITOR-11): if the focused doc still shows `path`, replace
    /// its buffer with the on-disk copy; otherwise the tab moved under the prompt
    /// and the decision is dropped. Surfaces the outcome as a status notice.
    fn reload_focused(&mut self, path: &Path) {
        let outcome = {
            let Some(doc) = self.doc_mut() else {
                return;
            };
            if doc.buffer.path() != Some(path) {
                return;
            }
            doc.reload()
        };
        self.notice = Some(match outcome {
            Ok(()) => "Reloaded from disk".to_owned(),
            Err(err) => format!("Reload failed: {err}"),
        });
    }

    /// Keep-mine (EDITOR-11): adopt the new on-disk `stamp` on the focused doc (if
    /// it still shows `path`) so the same external change stops prompting, leaving
    /// the operator's in-memory edits untouched.
    fn acknowledge_disk(&mut self, path: &Path, stamp: DiskStamp) {
        if let Some(doc) = self.doc_mut() {
            if doc.buffer.path() == Some(path) {
                doc.buffer.adopt_disk_stamp(stamp);
            }
        }
        self.notice = Some("Kept your version".to_owned());
    }

    /// Render the EDITOR-11 external-change reload prompt and route the operator's
    /// decision. Never clobbers: a dirty buffer's conflict offers keep-mine vs.
    /// reload-theirs; a clean buffer offers reload vs. ignore. Escape keeps the
    /// in-memory version (the safe default). Token-styled (§4).
    fn render_reload_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.reload.prompt.take() else {
            return;
        };
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let mut decision: Option<ReloadChoice> = esc.then_some(ReloadChoice::Keep);
        egui::Window::new("File changed on disk")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(DIALOG_W);
                ui.label(
                    RichText::new(prompt.path.display().to_string())
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_XS);
                let (body, color) = if prompt.conflict {
                    (
                        "This file changed on disk and you have unsaved edits here.",
                        Style::WARN,
                    )
                } else {
                    (
                        "This file changed on disk since you opened it.",
                        Style::TEXT,
                    )
                };
                ui.label(RichText::new(body).size(Style::BODY).color(color));
                ui.add_space(Style::SP_S);
                ui.horizontal(|ui| {
                    let (reload_label, keep_label) = if prompt.conflict {
                        ("Reload (discard mine)", "Keep mine")
                    } else {
                        ("Reload", "Ignore")
                    };
                    if ui.button(keep_label).clicked() {
                        decision = Some(ReloadChoice::Keep);
                    }
                    if ui.button(reload_label).clicked() {
                        decision = Some(ReloadChoice::Reload);
                    }
                });
            });
        match decision {
            Some(ReloadChoice::Reload) => self.reload_focused(&prompt.path),
            Some(ReloadChoice::Keep) => self.acknowledge_disk(&prompt.path, prompt.stamp),
            None => self.reload.prompt = Some(prompt),
        }
    }

    /// Intercept Ctrl-S at the panel level (EDITOR-11) — consumed before the text
    /// widget clones this frame's events so it saves instead of typing `s`. Skipped
    /// while an overlay/dialog holds the keyboard (so Ctrl-S in a field never fires
    /// a background save).
    fn handle_save_chord(&mut self, ui: &Ui) {
        if self.overlay_active() {
            return;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
            self.save_focused();
        }
    }

    /// Render the EDTB-1 dialogs (Save As…, About) above the editor body and
    /// route their outcomes. Escape cancels an open dialog (consumed, mirroring
    /// the finder/palette Esc handling). Token-styled (§4).
    fn render_dialogs(&mut self, ctx: &egui::Context) {
        if self.save_as.open {
            let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
            let mut save = false;
            let mut cancel = esc;
            egui::Window::new("Save As")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_min_width(DIALOG_W);
                    ui.label(
                        RichText::new("Save the document as")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    ui.add_space(Style::SP_XS);
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.save_as.path)
                            .hint_text("/path/to/file")
                            .desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut self.save_as.focus_field) {
                        field.request_focus();
                    }
                    // Enter in the path field saves (the field drops focus on
                    // Enter — the standard egui commit idiom).
                    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        save = true;
                    }
                    if let Some(error) = &self.save_as.error {
                        ui.add_space(Style::SP_XS);
                        ui.label(RichText::new(error).size(Style::SMALL).color(Style::WARN));
                    }
                    ui.add_space(Style::SP_XS);
                    ui.horizontal(|ui| {
                        save |= ui.button("Save").clicked();
                        cancel |= ui.button("Cancel").clicked();
                    });
                });
            if save {
                self.save_as_commit();
            } else if cancel {
                self.save_as.open = false;
            }
        }

        if self.about_open {
            let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
            egui::Window::new(ABOUT_TITLE)
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_min_width(DIALOG_W);
                    ui.label(
                        RichText::new(ABOUT_PRODUCT_LINE)
                            .size(Style::HEADING)
                            .color(Style::TEXT)
                            .strong(),
                    );
                    ui.add_space(Style::SP_XS);
                    ui.label(
                        RichText::new(ABOUT_VERSION_LINE)
                            .size(Style::BODY)
                            .color(Style::TEXT_DIM),
                    );
                    ui.label(
                        RichText::new("The MCNF native code-editor surface.")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    ui.add_space(Style::SP_S);
                    close |= ui.button("Close").clicked();
                });
            if close {
                self.about_open = false;
            }
        }

        self.render_table_picker(ctx);
        self.render_print_preview(ctx);
    }

    /// The filename shown in the print header / job title (EDTB-5) — the open
    /// document's file name, or `Untitled` for a pathless scratch buffer.
    fn print_filename(&self) -> String {
        self.current_path().and_then(Path::file_name).map_or_else(
            || "Untitled".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    /// Paginate the open buffer and submit it to CUPS `lp` (EDTB-5). Stores an
    /// honest outcome — a sent-N-pages message or a named [`print::PrintError`]
    /// notice — surfaced in the Print Preview overlay and mirrored to the status
    /// bar so a direct toolbar/menu Print (no preview open) is never a silent
    /// no-op or a faked success (§7). A no-op with no open document.
    fn do_print(&mut self) {
        let filename = self.print_filename();
        let Some(text) = self.doc().map(|d| d.buffer.rope().to_string()) else {
            return;
        };
        let layout = PageLayout::default();
        let pages = print::paginate(&text, &layout);
        let count = pages.len();
        let job = print::render_print_job(&pages, &filename, &layout);
        let opts = PrintOptions {
            printer: None,
            title: filename,
        };
        let message = match print::submit(print::LP, &job, &opts) {
            Ok(()) => (format!("Sent {count} page(s) to the print queue"), true),
            Err(err) => (err.notice(), false),
        };
        self.print.outcome = Some(PrintOutcome {
            text: message.0.clone(),
            ok: message.1,
        });
        self.notice = Some(message.0);
    }

    /// Render the EDTB-5 Print Preview overlay: the paginated pages (the same
    /// [`print::paginate`] the print job renders, so the preview is honest) in the
    /// Construct style, with a Print button that submits to CUPS and the last
    /// attempt's outcome shown inline. Escape / Close dismisses. Token-styled (§4).
    fn render_print_preview(&mut self, ctx: &egui::Context) {
        if !self.print.preview_open {
            return;
        }
        let filename = self.print_filename();
        let Some(text) = self.doc().map(|d| d.buffer.rope().to_string()) else {
            // The document vanished under the overlay — nothing left to preview.
            self.print.preview_open = false;
            return;
        };
        let layout = PageLayout::default();
        let pages = print::paginate(&text, &layout);
        let total = pages.len();
        let mut want_print = false;
        let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        egui::Window::new("Print Preview")
            .collapsible(false)
            .resizable(true)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(PREVIEW_W);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&filename)
                            .size(Style::BODY)
                            .color(Style::TEXT)
                            .strong(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{total} page(s)"))
                                .size(Style::SMALL)
                                .color(Style::TEXT_DIM),
                        );
                    });
                });
                ui.add_space(Style::SP_XS);
                egui::ScrollArea::both()
                    .max_height(PREVIEW_MAX_H)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for page in &pages {
                            print::draw_page(ui, page, &filename, total);
                            ui.add_space(Style::SP_S);
                        }
                    });
                if let Some(outcome) = &self.print.outcome {
                    ui.add_space(Style::SP_XS);
                    let color = if outcome.ok { Style::OK } else { Style::DANGER };
                    ui.label(RichText::new(&outcome.text).size(Style::SMALL).color(color));
                }
                ui.add_space(Style::SP_S);
                ui.horizontal(|ui| {
                    want_print = ui.button("Print").clicked();
                    close |= ui.button("Close").clicked();
                });
            });
        if want_print {
            self.do_print();
        }
        if close {
            self.print.preview_open = false;
            self.print.outcome = None;
        }
    }

    /// The in-line context around a miss (EDTB-6) — the text before + after the
    /// word on its own line, each clamped to a readable window so a very long line
    /// never blows the dialog. The word itself is [`SpellMiss::word`].
    fn spell_context(&self, miss: &SpellMiss) -> (String, String) {
        // A bounded window on each side of the word (with a leading/trailing
        // ellipsis) so a very long line never blows the dialog.
        const CTX: usize = 32;
        let Some(doc) = self.doc() else {
            return (String::new(), String::new());
        };
        let buffer = &doc.buffer;
        let line_idx = buffer.char_to_line(miss.chars.start);
        let line_start = buffer.line_to_char(line_idx);
        let line: Vec<char> = buffer
            .line(line_idx)
            .trim_end_matches('\n')
            .chars()
            .collect();
        let ws = miss.chars.start.saturating_sub(line_start).min(line.len());
        let we = miss
            .chars
            .end
            .saturating_sub(line_start)
            .min(line.len())
            .max(ws);
        let before: String = if ws > CTX {
            format!("\u{2026}{}", line[ws - CTX..ws].iter().collect::<String>())
        } else {
            line[..ws].iter().collect()
        };
        let after: String = if line.len().saturating_sub(we) > CTX {
            format!("{}\u{2026}", line[we..we + CTX].iter().collect::<String>())
        } else {
            line[we..].iter().collect()
        };
        (before, after)
    }

    /// Render the EDTB-6 `F7` spelling-walk dialog: step the buffer's misspellings,
    /// each shown in its line context with the [`hunspell`](crate::spell)
    /// suggestions, and the Suggest / Replace / Ignore / Ignore-All /
    /// Add-to-dictionary actions. Replace is a real undoable rope edit through the
    /// shared seam (§6/§7); Ignore-All / Add filter the squiggles at once. An empty
    /// list shows the honest "no misspellings" state, never a fake walk. Escape /
    /// Done dismisses. Token-styled (§4).
    #[allow(clippy::too_many_lines)]
    fn render_spell_walk(&mut self, ctx: &egui::Context) {
        if !self.spell_walk.open {
            return;
        }
        // The document vanished under the dialog — nothing left to walk.
        let Some(total) = self.doc().map(|d| d.spell.misses().len()) else {
            self.spell_walk.open = false;
            return;
        };
        // Keep the cursor in range as the visible list shrinks under it.
        if total > 0 {
            self.spell_walk.index = self.spell_walk.index.min(total - 1);
        }
        let miss = self.spell_current_miss();
        let checking = self.doc().is_some_and(|d| d.spell.is_pending());
        let (before, after) = miss
            .as_ref()
            .map_or_else(|| (String::new(), String::new()), |m| self.spell_context(m));

        let mut esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let mut replace_with: Option<String> = None;
        let mut ignore = false;
        let mut ignore_all = false;
        let mut add = false;
        let mut prev = false;
        let mut next = false;

        egui::Window::new("Spelling")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(DIALOG_W);
                match &miss {
                    None => {
                        // The honest clean / working state (§7) — never a fake walk.
                        ui.label(
                            RichText::new(if checking {
                                "Checking spelling\u{2026}"
                            } else {
                                "No misspellings found"
                            })
                            .size(Style::BODY)
                            .color(Style::TEXT_DIM),
                        );
                        ui.add_space(Style::SP_S);
                        esc |= ui.button("Done").clicked();
                    }
                    Some(m) => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Misspelling {} of {total}",
                                    self.spell_walk.index + 1
                                ))
                                .size(Style::SMALL)
                                .color(Style::TEXT_DIM),
                            );
                        });
                        ui.add_space(Style::SP_XS);
                        // The word in its line context — the miss painted in the
                        // shared spell token, the surrounds dimmed.
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(RichText::new(&before).monospace().color(Style::TEXT_DIM));
                            ui.label(
                                RichText::new(&m.word)
                                    .monospace()
                                    .strong()
                                    .color(Style::SPELL),
                            );
                            ui.label(RichText::new(&after).monospace().color(Style::TEXT_DIM));
                        });
                        ui.add_space(Style::SP_S);
                        // Suggest — the clickable suggestion list; a click Replaces.
                        if m.suggestions.is_empty() {
                            ui.label(
                                RichText::new("No suggestions")
                                    .size(Style::SMALL)
                                    .color(Style::TEXT_DIM),
                            );
                        } else {
                            ui.label(
                                RichText::new("Suggestions")
                                    .size(Style::SMALL)
                                    .color(Style::TEXT_DIM),
                            );
                            ui.add_space(Style::SP_XS);
                            ui.horizontal_wrapped(|ui| {
                                for sug in &m.suggestions {
                                    if ui.button(sug).clicked() {
                                        replace_with = Some(sug.clone());
                                    }
                                }
                            });
                        }
                        ui.add_space(Style::SP_S);
                        ui.separator();
                        ui.horizontal(|ui| {
                            // Replace with the top suggestion (greyed with none).
                            let top = m.suggestions.first().cloned();
                            if ui
                                .add_enabled(top.is_some(), egui::Button::new("Replace"))
                                .on_hover_text("Replace with the top suggestion")
                                .clicked()
                            {
                                replace_with = top;
                            }
                            ignore |= ui.button("Ignore").clicked();
                            ignore_all |= ui.button("Ignore All").clicked();
                            add |= ui
                                .button("Add to Dictionary")
                                .on_hover_text("Add to the session dictionary")
                                .clicked();
                        });
                        ui.add_space(Style::SP_XS);
                        ui.horizontal(|ui| {
                            prev |= ui.button("\u{2039} Prev").clicked();
                            next |= ui.button("Next \u{203A}").clicked();
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                esc |= ui.button("Done").clicked();
                            });
                        });
                    }
                }
            });

        // Apply the picked action after the window's borrow releases (the print /
        // save-as idiom). Replace edits the buffer; the next background check
        // re-resolves the misses, so the walk naturally advances off the fixed
        // word. Ignore-All / Add shrink the visible list immediately.
        if let (Some(replacement), Some(m)) = (replace_with, miss.as_ref()) {
            self.spell_replace_current(m, &replacement);
        } else if ignore {
            self.spell_walk.index += 1;
        } else if ignore_all {
            if let Some(m) = miss.as_ref() {
                self.spell_ignore_all(&m.word);
            }
        } else if add {
            if let Some(m) = miss.as_ref() {
                self.spell_add_word(&m.word);
            }
        }
        if prev {
            self.spell_walk.index = self.spell_walk.index.saturating_sub(1);
        }
        if next {
            self.spell_walk.index += 1;
        }
        // Re-clamp against the (possibly shrunk) list so the next frame is valid.
        let now = self.doc().map_or(0, |d| d.spell.misses().len());
        self.spell_walk.index = self.spell_walk.index.min(now.saturating_sub(1));
        if esc {
            self.spell_walk.open = false;
        }
        // A pending check should repaint so the walk fills in when it lands.
        if checking {
            ctx.request_repaint();
        }
    }

    /// Render the EDTB-3 Insert Table grid-picker (Word's drag-grid): hover to
    /// size, click to insert the markdown skeleton at the caret. Escape / Cancel
    /// dismisses; a click routes through the same seam `MenuAction::InsertTable`
    /// drives (§6 — one undo step). Token-styled (§4).
    fn render_table_picker(&mut self, ctx: &egui::Context) {
        if !self.table_picker.open {
            return;
        }
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let mut chosen: Option<(u8, u8)> = None;
        let mut cancel = esc;
        egui::Window::new("Insert Table")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Table size")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_XS);
                chosen = format_bar::table_grid(ui);
                ui.add_space(Style::SP_S);
                cancel |= ui.button("Cancel").clicked();
            });
        if let Some((rows, cols)) = chosen {
            self.table_picker.open = false;
            self.run_action(ctx, MenuAction::InsertTable { rows, cols });
        } else if cancel {
            self.table_picker.open = false;
        }
    }

    /// Intercept the overlay trigger chords at the panel level — consumed BEFORE
    /// the text widget reads this frame's events (it clones `ui.input` events
    /// during its own render), so `Cmd`/`Ctrl-P` opens the finder and
    /// `Cmd`/`Ctrl-Shift-P` toggles the palette instead of typing `p` into the
    /// document. The more-specific Shift chord is consumed first.
    fn handle_overlay_triggers(&mut self, ui: &Ui) {
        let open_palette =
            ui.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::P));
        let open_finder = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::P));
        // EDITOR-8 — the search chords, consumed at the same panel level so they
        // open the overlays instead of typing into the buffer. The more-specific
        // Shift chord (project search) is consumed before the plain `Ctrl-F`.
        let open_project =
            ui.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::F));
        let open_find = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F));
        let open_replace = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::H));
        // EDITOR-12 — the symbol-outline side-panel toggle, consumed here so it
        // never types an `o` into the buffer.
        let toggle_outline =
            ui.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::O));
        if open_palette {
            self.toggle_palette();
        }
        if open_finder {
            self.open_finder();
        }
        if open_project {
            self.open_project_search();
        }
        if open_find {
            self.open_find();
        }
        if open_replace {
            self.open_replace();
        }
        if toggle_outline {
            self.show_outline = !self.show_outline;
        }
    }

    /// Render the EDITOR-7 overlays on top of the editor body and route the
    /// operator's pick to its seam: a finder pick opens the file
    /// ([`open_path`](Self::open_path)); a palette pick runs its command.
    fn render_overlays(&mut self, ui: &Ui) {
        let ctx = ui.ctx();
        if let Some(path) = finder::show(ctx, &mut self.finder) {
            self.open_selected(&path);
        }
        if let Some(cmd) = palette::show(ctx, &mut self.palette) {
            self.run_command(cmd);
        }
        // EDITOR-8 — the in-buffer find/replace bar: route its frame event to the
        // live buffer / view (next/prev jump the caret, replace mutates the rope).
        match search::show_find(ctx, &mut self.find) {
            FindEvent::Idle => {}
            FindEvent::Next => {
                self.find.cycle(true);
                self.reveal_current_match();
            }
            FindEvent::Prev => {
                self.find.cycle(false);
                self.reveal_current_match();
            }
            FindEvent::ReplaceCurrent => self.replace_current_match(),
            FindEvent::ReplaceAll => self.replace_all_matches(),
        }
        // EDITOR-8 — the project-wide search: a picked hit opens the file + jumps.
        if let Some(hit) = search::show_project(ctx, &mut self.project_search) {
            self.open_hit(&hit);
        }
        // EDITOR-LSP-3 — the find-references results list: a picked row opens the
        // file + jumps through the same open+jump seam.
        if let Some(row) = lsp_nav::show_references(ctx, &mut self.references) {
            self.jump_to_location(&row.path, row.line0, row.char0);
        }
        // EDITOR-LSP-3 — the rename box: a submitted name fires the rename request
        // (the reply applies the cross-file workspace edit).
        if let Some(new_name) = lsp_nav::show_rename(ctx, &mut self.rename) {
            self.rename.close();
            self.lsp_fire_rename(&new_name);
        }
    }

    // ── EDITOR-6: tabs + split chords, chrome, and the pane tree render ───────

    /// Intercept the EDITOR-6 tab / split / pane-focus chords at the panel level,
    /// consumed BEFORE the text widget clones this frame's events (mirroring
    /// [`handle_overlay_triggers`](Self::handle_overlay_triggers)) so they never
    /// type into the buffer. `Ctrl-T` opens a tab, `Ctrl-W` closes one, `Ctrl-\`
    /// / `Ctrl-Shift-\` split the focused pane, and `Alt+arrows` move the focus.
    /// The more-specific Shift chord is consumed first.
    fn handle_pane_chords(&mut self, ui: &Ui) {
        let split_h =
            ui.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Backslash));
        let split_v = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Backslash));
        let new_tab = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::T));
        let close_tab = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W));
        if split_h {
            self.split_focused(SplitDir::H);
        }
        if split_v {
            self.split_focused(SplitDir::V);
        }
        if new_tab {
            self.open_scratch();
        }
        if close_tab {
            self.close();
        }
        for (key, dir) in [
            (Key::ArrowLeft, NavDir::Left),
            (Key::ArrowRight, NavDir::Right),
            (Key::ArrowUp, NavDir::Up),
            (Key::ArrowDown, NavDir::Down),
        ] {
            if ui.input_mut(|i| i.consume_key(Modifiers::ALT, key)) {
                self.navigate_focus(dir);
            }
        }
    }

    /// Intercept the EDITOR-LSP-3 navigation chords at the panel level (before the
    /// text widget clones this frame's events), so the F-keys drive the language
    /// server instead of falling through to the buffer: `F12` goto-definition,
    /// `Shift-F12` find-references, `F2` rename, `Shift-Alt-F` format. Skipped
    /// while an overlay/dialog holds the keyboard so a rename field can type an
    /// `F` or the references list can arrow without re-triggering.
    fn handle_lsp_chords(&mut self, ui: &Ui) {
        if self.overlay_active() {
            return;
        }
        let goto = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F12));
        let references = ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::F12));
        let rename = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F2));
        let format = ui.input_mut(|i| i.consume_key(Modifiers::SHIFT | Modifiers::ALT, Key::F));
        let fired = goto || references || rename || format;
        if references {
            self.lsp_find_references();
        } else if goto {
            self.lsp_goto_definition();
        }
        if rename {
            self.lsp_start_rename();
        }
        if format {
            self.lsp_format_document();
        }
        // A live server replies asynchronously; nudge the next frame so the reply
        // is drained promptly (the widget's 0.5 s heartbeat is the slow fallback).
        if fired {
            ui.ctx().request_repaint();
        }
    }

    /// Intercept the EDTB-6 spelling chord (`F7`) at the panel level (before the
    /// widget clones this frame's events, so it never types into the buffer),
    /// opening the walk dialog. Skipped while an overlay/dialog holds the
    /// keyboard (the walk owns its own keys once up).
    fn handle_spell_chord(&mut self, ui: &Ui) {
        if self.overlay_active() {
            return;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F7)) {
            self.open_spell_walk();
        }
    }

    /// The surface chrome strip above the pane tree: the project-tree toggle and
    /// the split controls (the mouse-reachable twins of the split chords, §7).
    /// Token-styled (§4). Per-buffer identity lives in each pane's tab bar, so
    /// this strip stays surface-global.
    fn top_strip(&mut self, ui: &mut Ui) {
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_S);
            tree_toggle(ui, &mut self.show_tree);
            // EDITOR-12 — the symbol-outline toggle (mouse twin of Ctrl+Shift+O).
            if ui
                .selectable_label(
                    self.show_outline,
                    RichText::new("\u{2263}").size(Style::BODY),
                )
                .on_hover_text("Toggle the symbol outline (Ctrl+Shift+O)")
                .clicked()
            {
                self.show_outline = !self.show_outline;
            }
            // EDITOR-10 — the integrated-terminal toggle (mouse twin of Ctrl+`).
            if ui
                .selectable_label(
                    self.terminal.is_shown(),
                    RichText::new("\u{2328}").size(Style::BODY),
                )
                .on_hover_text("Toggle the integrated terminal (Ctrl+`)")
                .clicked()
            {
                self.toggle_terminal();
            }
            ui.add_space(Style::SP_M);
            if ui
                .selectable_label(false, RichText::new("\u{2503}").size(Style::BODY))
                .on_hover_text("Split right (Ctrl+\\)")
                .clicked()
            {
                self.split_focused(SplitDir::V);
            }
            if ui
                .selectable_label(false, RichText::new("\u{2501}").size(Style::BODY))
                .on_hover_text("Split down (Ctrl+Shift+\\)")
                .clicked()
            {
                self.split_focused(SplitDir::H);
            }
            if self.tree.leaf_count() > 1 {
                ui.add_space(Style::SP_M);
                if ui
                    .selectable_label(false, RichText::new("Close pane").size(Style::SMALL))
                    .on_hover_text("Close the focused pane")
                    .clicked()
                {
                    let focus = self.focus;
                    self.close_pane(focus);
                }
            }
        });
        ui.add_space(Style::SP_XS);
        ui.separator();
    }

    /// The focused document's status line (caret position, language, LSP + spell
    /// status, soft-wrap toggle, lossy-decode note) — a single surface-global strip
    /// since the tab bars already carry each buffer's name + dirty marker. Honest
    /// chrome: every value reflects the real buffer/view (§7); token-styled (§4).
    #[allow(clippy::too_many_lines)]
    fn status_bar(&mut self, ui: &mut Ui) {
        // Snapshot every displayed value, ending the `&Doc` borrow before the
        // deferred wrap toggle needs `&mut Doc`.
        let Some((lossy, line, col, wrap_on, lang, lsp)) = self.doc().map(|doc| {
            let (line, col) = doc.view.line_col(&doc.buffer);
            (
                doc.buffer.is_lossy(),
                line,
                col,
                doc.view.wrap(),
                doc.highlight.as_ref().map(|hl| hl.language().name()),
                doc.lsp.as_ref().and_then(lsp_status),
            )
        }) else {
            return;
        };
        // EDITOR-LSP-3 — the last navigation action's honest status (§7).
        let notice = self.lsp_notice.clone();
        // EDTB-6 — the last spelling-action notice (§7) + the ambient spell status
        // (an honest "hunspell not installed" for a text buffer, else a quiet miss
        // count). Snapshotted here so the `&Doc` borrow ends before the toggles.
        let spell_notice = self.spell_notice.clone();
        let spell_status: Option<(String, egui::Color32)> = self.doc().and_then(|doc| {
            if !doc.spellcheckable() {
                return None;
            }
            if !self.spell_state.is_ready() {
                return Some((SpellState::Unavailable.notice().to_owned(), Style::WARN));
            }
            let n = doc.spell.misses().len();
            (n > 0).then(|| (format!("{n} spelling"), Style::TEXT_DIM))
        });
        // EDITOR-11 — the last save/autosave/reload status + the autosave toggle.
        let save_notice = self.notice.clone();
        let autosave_on = self.autosave.enabled;
        let mut toggle_wrap = false;
        let mut toggle_autosave = false;
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_S);
            if lossy {
                ui.label(
                    RichText::new("lossy decode")
                        .size(Style::SMALL)
                        .color(Style::WARN),
                );
            }
            if let Some(notice) = &notice {
                ui.add_space(Style::SP_M);
                ui.label(
                    RichText::new(notice)
                        .size(Style::SMALL)
                        .color(Style::ACCENT),
                );
            }
            if let Some(save_notice) = &save_notice {
                ui.add_space(Style::SP_M);
                ui.label(
                    RichText::new(save_notice)
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            }
            if let Some(spell_notice) = &spell_notice {
                ui.add_space(Style::SP_M);
                ui.label(
                    RichText::new(spell_notice)
                        .size(Style::SMALL)
                        .color(Style::ACCENT),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(Style::SP_S);
                ui.label(
                    RichText::new(format!("Ln {line}, Col {col}"))
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                if let Some(status) = &lsp {
                    ui.add_space(Style::SP_M);
                    ui.label(
                        RichText::new(&status.text)
                            .size(Style::SMALL)
                            .color(status.color),
                    );
                }
                if let Some((text, color)) = &spell_status {
                    ui.add_space(Style::SP_M);
                    ui.label(RichText::new(text).size(Style::SMALL).color(*color));
                }
                if let Some(lang) = lang {
                    ui.add_space(Style::SP_M);
                    ui.label(
                        RichText::new(lang)
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                }
                ui.add_space(Style::SP_M);
                if ui
                    .selectable_label(wrap_on, RichText::new("Wrap").size(Style::SMALL))
                    .clicked()
                {
                    toggle_wrap = true;
                }
                ui.add_space(Style::SP_M);
                if ui
                    .selectable_label(autosave_on, RichText::new("Autosave").size(Style::SMALL))
                    .on_hover_text("Save dirty buffers automatically after a short idle")
                    .clicked()
                {
                    toggle_autosave = true;
                }
            });
        });
        if toggle_wrap {
            if let Some(doc) = self.doc_mut() {
                doc.view.toggle_wrap();
            }
        }
        if toggle_autosave {
            self.set_autosave(!autosave_on);
        }
    }

    /// Hand the keyboard to the focused pane's text widget BEFORE the widgets
    /// render (mirrors term's `prefocus`): each widget only reads keys while it
    /// `has_focus`, so without this the first-rendered pane would capture typing.
    /// A no-op with a single pane (the lone widget self-manages focus) or while
    /// an overlay/dialog field holds the keyboard.
    fn prefocus(&self, ui: &Ui) {
        if self.tree.leaf_count() <= 1 {
            return;
        }
        let Some(&want) = self.pane_widget_ids.get(&self.focus) else {
            return;
        };
        let current = ui.memory(egui::Memory::focused);
        if current == Some(want) {
            return;
        }
        // Only steal focus from nothing or from another editor pane — never from
        // an open overlay/dialog text field.
        let ours = current.is_none_or(|id| self.pane_widget_ids.values().any(|w| *w == id));
        if ours {
            ui.memory_mut(|m| m.request_focus(want));
        }
    }

    /// Lay the split tree out into `rect` and render every leaf (its tab bar +
    /// active tab's widget), the draggable dividers, and the focus ring. Deferred
    /// tab/pane actions are applied after all borrows release.
    /// Render the EDTB-7 split markdown-preview pane on the right edge (the
    /// editor's side-by-side rendered half). Re-parses the focused document only
    /// on a real edit ([`Doc::preview_blocks`]'s debounce) so live typing tracks;
    /// an empty document shows an honest hint (§7). Only called while the focused
    /// document is previewable, so the pane never renders nonsense for code.
    fn render_preview(&mut self, ui: &mut Ui) {
        egui::SidePanel::right("editor-preview")
            .resizable(true)
            .default_width(PREVIEW_WIDTH)
            .frame(egui::Frame::default().fill(Style::BG))
            .show_inside(ui, |ui| {
                ui.add_space(Style::SP_XS);
                ui.horizontal(|ui| {
                    ui.add_space(Style::SP_S);
                    ui.label(
                        RichText::new("Preview")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                });
                ui.separator();
                if let Some(doc) = self.doc_mut() {
                    let blocks = doc.preview_blocks();
                    if blocks.is_empty() {
                        preview_empty(ui);
                    } else {
                        markdown::show(ui, blocks);
                    }
                }
            });
    }

    fn render_panes(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        match_ranges: &[Range<usize>],
        match_current: Option<usize>,
    ) {
        self.prefocus(ui);
        let lay = panes::layout(&self.tree, rect);
        let multi = lay.leaves.len() > 1;
        let mut outcomes: Vec<(PaneId, LeafAction)> = Vec::new();
        let mut ids: HashMap<PaneId, Id> = HashMap::new();
        // Snapshot the surface-global spell env once (a String clone + a Copy
        // state) so each leaf's per-doc pump reads it without re-borrowing self.
        let spell_program = self.spell_program.clone();
        let spell_state = self.spell_state;
        for (pid, prect) in &lay.leaves {
            self.render_leaf(
                ui,
                *pid,
                *prect,
                multi,
                &mut outcomes,
                &mut ids,
                match_ranges,
                match_current,
                &spell_program,
                spell_state,
            );
        }
        self.pane_widget_ids = ids;
        self.render_dividers(ui, &lay);
        if multi {
            self.paint_focus_ring(ui, &lay);
        }
        for (pid, action) in outcomes {
            self.apply_leaf_action(pid, action);
        }
    }

    /// Render one leaf pane into `rect`: its tab bar, then the active tab's text
    /// widget (the live rope, §7) or the honest empty-pane face. Collects the
    /// tab-bar action + the widget's egui id for [`prefocus`](Self::prefocus).
    #[allow(clippy::too_many_arguments)]
    fn render_leaf(
        &mut self,
        ui: &mut Ui,
        pid: PaneId,
        rect: Rect,
        multi: bool,
        outcomes: &mut Vec<(PaneId, LeafAction)>,
        ids: &mut HashMap<PaneId, Id>,
        match_ranges: &[Range<usize>],
        match_current: Option<usize>,
        spell_program: &str,
        spell_state: SpellState,
    ) {
        let is_focused = pid == self.focus;
        let Some(pane) = self.panes.get_mut(&pid) else {
            return;
        };
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .id_salt(("editor-pane", pid.0))
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(rect);

        // The per-pane tab bar (open buffers, active tab, close, new).
        if let Some(action) = tab_bar(&mut child, pane, is_focused && multi) {
            outcomes.push((pid, action));
        }
        child.separator();

        if let Some(doc) = pane.active_doc_mut() {
            // EDITOR-LSP-2 — refresh, render the live widget, then sync edits.
            doc.refresh_diagnostics();
            // EDTB-6 — drive the background spell pass (drain a finished check +
            // launch one for a fresh edit); a pending check nudges a repaint so
            // its squiggles land promptly.
            if doc.pump_spell(spell_program, spell_state) {
                child.ctx().request_repaint();
            }
            let spell = doc.spell.marks();
            // EDITOR-8 — the find-match highlights only paint in the focused pane
            // (the search targets its active document); other panes paint plain.
            let matches = if is_focused {
                MatchHighlights {
                    ranges: match_ranges,
                    current: match_current,
                }
            } else {
                MatchHighlights::default()
            };
            let resp = editor_widget(
                &mut child,
                &mut doc.view,
                &mut doc.buffer,
                doc.highlight.as_mut(),
                &doc.diagnostics,
                matches,
                spell,
            );
            doc.sync_lsp();
            ids.insert(pid, resp.id);
            if !is_focused && (resp.clicked() || resp.gained_focus()) {
                outcomes.push((pid, LeafAction::Focus));
            }
        } else if empty_state(&mut child) {
            outcomes.push((pid, LeafAction::NewTab));
        }
    }

    /// Divider strips between sibling panes: dragging one adjusts the split ratio
    /// (clamped so no pane collapses); hover/drag recolour the hairline through
    /// the shared `Style` tokens (§4).
    fn render_dividers(&mut self, ui: &Ui, lay: &panes::Layout) {
        for div in &lay.dividers {
            let (hit, icon, line_size) = match div.dir {
                SplitDir::V => (
                    div.rect.expand2(vec2(DIVIDER_HIT_SLOP, 0.0)),
                    CursorIcon::ResizeHorizontal,
                    vec2(1.0, div.rect.height()),
                ),
                SplitDir::H => (
                    div.rect.expand2(vec2(0.0, DIVIDER_HIT_SLOP)),
                    CursorIcon::ResizeVertical,
                    vec2(div.rect.width(), 1.0),
                ),
            };
            let resp = ui
                .interact(
                    hit,
                    ui.id().with(("editor-splitter", div.path)),
                    Sense::drag(),
                )
                .on_hover_cursor(icon);
            if resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    if let Some(ratio) = self.tree.ratio_mut(div.path) {
                        *ratio = panes::pointer_ratio(div, pos);
                    }
                }
            }
            let color = if resp.dragged() {
                Style::ACCENT
            } else if resp.hovered() {
                Style::ACCENT_HI
            } else {
                Style::BORDER
            };
            ui.painter().rect_filled(
                Rect::from_center_size(div.rect.center(), line_size),
                0.0,
                color,
            );
        }
    }

    /// The hairline focus ring on the focused pane — only drawn once there is
    /// more than one pane to tell apart (a lone pane stays full-bleed).
    fn paint_focus_ring(&self, ui: &Ui, lay: &panes::Layout) {
        if let Some((_, rect)) = lay.leaves.iter().find(|(pid, _)| *pid == self.focus) {
            ui.painter().rect_stroke(
                *rect,
                Style::RADIUS,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
        }
    }
}

/// Extra grab slop on each side of a divider strip (the strip is thin; the hit
/// area overlaps the panes slightly and, registered after them, wins the pointer).
const DIVIDER_HIT_SLOP: f32 = 2.0;

/// Render the editor surface into the shell body — mirrors `files_panel`.
///
/// When a document is open: a compact chrome strip (name, dirty marker, wrap
/// toggle, caret position) over the real text widget, which edits the live rope
/// (§7 — runtime-reachable, not a mockup). When none is open: the honest empty
/// state plus a temporary "open a scratch buffer" affordance so the surface is
/// exercisable before fuzzy-open lands. `surface` is the mount seam the shell
/// wires (the analogue of `files_panel`'s `&mut FileBrowser`).
#[allow(clippy::too_many_lines)] // one linear mount sequence; splitting it hides the order
pub fn editor_panel(ui: &mut Ui, surface: &mut EditorSurface) {
    // EDITOR-7 — panel-level keybind intercept. Consume the overlay trigger chords
    // FIRST, before the tree/central body render (the text widget clones this
    // frame's `ui.input` events during its own render below), so `Cmd`/`Ctrl-P`
    // and `Cmd`/`Ctrl-Shift-P` open the overlays instead of typing into the buffer.
    surface.handle_overlay_triggers(ui);
    // EDITOR-6 — the tab / split / pane-focus chords, consumed at the same panel
    // level (before the widget clones this frame's events) for the same reason.
    surface.handle_pane_chords(ui);
    // EDITOR-LSP-3 — the navigation chords (F12 / Shift-F12 / F2 / Shift-Alt-F),
    // consumed here for the same reason, then drain any completed async replies
    // from the language server so this frame reflects a jump / list / edit.
    surface.handle_lsp_chords(ui);
    // EDTB-6 — probe hunspell once, then the F7 spelling chord (consumed at the
    // panel level for the same reason) opens the walk dialog.
    surface.ensure_spell_probe();
    surface.handle_spell_chord(ui);
    // EDITOR-11 — Ctrl-S save (consumed at the panel level, before the widget
    // clones this frame's events), then the autosave debounce tick + the
    // external-change poll. `now` is egui's app-time in seconds; both the tick and
    // the poll are debounced so neither fights a live edit gesture.
    surface.handle_save_chord(ui);
    // EDITOR-10 — the integrated-terminal toggle chord (`Ctrl+``), consumed at the
    // panel level for the same reason (before the widget clones this frame's
    // events) so it flips the dock instead of typing a backtick into the buffer.
    surface.handle_terminal_chord(ui);
    let now = ui.input(|i| i.time);
    surface.tick_autosave(now);
    surface.poll_external_change(now);
    surface.pump_lsp_replies();

    // EDTB-4 — the compact decision, taken ONCE from the panel's available width
    // before the bars mount so all three read one consistent layout. Each bar's
    // `TopBottomPanel::top` spans this same full width, so this is their width
    // too. Below the threshold the Standard/Formatting strips fold their
    // width-heavy dropdowns into a `»` overflow (still reachable, §7); the menu
    // bar is already compact, so it renders the same at every width.
    let compact = is_compact(ui.available_width());

    // EDTB-1 — the Word-97 menu bar + Standard toolbar across the top of the
    // panel (design: `editor-toolbar.md`), drawn before the tree/body so the
    // whole surface sits under them. Both render from one state snapshot and
    // return at most one picked action, dispatched through the same seams the
    // palette drives (`run_action` → `run_command`/`EditorView`).
    let cx = surface.menu_context();
    let mut action = None;
    egui::TopBottomPanel::top("editor-menu-bar")
        .frame(
            egui::Frame::default()
                .fill(Style::SURFACE)
                .inner_margin(Style::SP_XS),
        )
        .show_inside(ui, |ui| {
            action = menu_bar::show(ui, &cx);
        });
    egui::TopBottomPanel::top("editor-toolbar")
        .frame(
            egui::Frame::default()
                .fill(Style::SURFACE)
                .inner_margin(Style::SP_XS),
        )
        .show_inside(ui, |ui| {
            if let Some(picked) = toolbar::show(ui, &cx, compact) {
                action = Some(picked);
            }
        });
    // EDTB-3 — the Word Formatting strip (Style/B/I/U/S/lists/indent), the
    // second toolbar row, mounted below the Standard strip. Drives `md_actions`
    // through the same `run_action` dispatch (§6). Greyed with no document.
    egui::TopBottomPanel::top("editor-format-bar")
        .frame(
            egui::Frame::default()
                .fill(Style::SURFACE)
                .inner_margin(Style::SP_XS),
        )
        .show_inside(ui, |ui| {
            if let Some(picked) = format_bar::show(ui, &cx, compact) {
                action = Some(picked);
            }
        });
    if let Some(action) = action {
        surface.run_action(ui.ctx(), action);
    }

    // EDITOR-8 — recompute the in-buffer find matches (while the find bar is open)
    // against the now-settled buffer, then snapshot the bands to paint. Taken here
    // — after the menu/toolbar action ran, before the body renders — so the widget
    // paints this frame's matches and the counter reflects the live buffer (§7).
    surface.refresh_find_matches();
    let (find_bands, find_current) = surface.find_paint_bands();

    // EDITOR-9 — the toggleable project-tree side panel, drawn BEFORE the central
    // body so the editor fills the area to its right (the `files_panel` idiom). A
    // file click routes through the EDITOR-3 open seam; a "no folder" state offers
    // a reachable affordance to open the current working directory.
    if surface.show_tree {
        let mut open_cwd = false;
        let picked = egui::SidePanel::left("editor-project-tree")
            .resizable(true)
            .default_width(TREE_WIDTH)
            .frame(egui::Frame::default().fill(Style::SURFACE))
            .show_inside(ui, |ui| {
                let Some(tree) = surface.project.as_mut() else {
                    no_folder_face(ui, &mut open_cwd);
                    return None;
                };
                project_tree::show(ui, tree)
            })
            .inner;
        if open_cwd {
            if let Ok(cwd) = std::env::current_dir() {
                surface.open_folder(cwd);
            }
        }
        if let Some(path) = picked {
            surface.open_selected(&path);
        }
    }

    // EDITOR-12 — the toggleable symbol-outline side panel, drawn on the RIGHT edge
    // (the project tree's twin) so the editor fills the space between them. A row
    // click jumps the focused caret through the shared `place_cursor` seam; a
    // language with no grammar / no symbols shows an honest empty state.
    if surface.show_outline {
        let symbols = surface.active_symbols();
        let has_grammar = surface.active_has_grammar();
        let has_doc = surface.doc().is_some();
        let jump = egui::SidePanel::right("editor-outline")
            .resizable(true)
            .default_width(OUTLINE_WIDTH)
            .frame(egui::Frame::default().fill(Style::SURFACE))
            .show_inside(ui, |ui| outline::show(ui, &symbols, has_grammar, has_doc))
            .inner;
        if let Some(idx) = jump {
            surface.jump_caret(idx);
        }
    }

    // EDTB-7 — the split markdown-preview pane, a right-edge rendered view of the
    // focused markdown/text buffer (the outline's neighbour). Only mounted while
    // the focused document is previewable, so switching to a code tab honestly
    // hides it (its toggle greys out) instead of rendering nonsense (§7).
    if surface.show_preview && surface.doc().is_some_and(Doc::previewable) {
        surface.render_preview(ui);
    }

    egui::CentralPanel::default().show_inside(ui, |ui| {
        // EDITOR-6 — the surface chrome strip (tree toggle + split controls),
        // then the focused document's status line pinned to the bottom, then the
        // split-pane tree filling the body. The tab bars + the widget over the
        // live rope render inside the tree (§7 — runtime-reachable, not a mockup).
        surface.top_strip(ui);
        if surface.doc().is_some() {
            egui::TopBottomPanel::bottom("editor-status")
                .frame(
                    egui::Frame::default()
                        .fill(Style::SURFACE)
                        .inner_margin(Style::SP_XS),
                )
                .show_inside(ui, |ui| surface.status_bar(ui));
        }
        // EDITOR-10 — the integrated terminal dock, a toggleable, resizable bottom
        // panel above the status strip (its bottom panel was added first, so this
        // one sits over it). Pump its chords + live font first, then paint the
        // reused `TabbedTerminal`; the session persists across toggles.
        if surface.terminal.is_shown() {
            egui::TopBottomPanel::bottom("editor-terminal")
                .resizable(true)
                .default_height(TERMINAL_HEIGHT)
                .min_height(TERMINAL_MIN_HEIGHT)
                .frame(egui::Frame::default().fill(Style::BG))
                .show_inside(ui, |ui| {
                    surface.terminal.pump(ui.ctx());
                    surface.terminal.show(ui);
                });
        }
        let body = ui.available_rect_before_wrap();
        surface.render_panes(ui, body, &find_bands, find_current);
    });

    // EDITOR-7 — the finder + palette overlays float above the body (rendered last
    // so they paint on top); each returns the operator's pick, routed to its seam.
    surface.render_overlays(ui);

    // EDTB-1 — the Save As / About dialogs, rendered above everything.
    surface.render_dialogs(ui.ctx());
    // EDTB-6 — the F7 spelling-walk dialog, above the body (the same overlay
    // layer as Print Preview / Save As).
    surface.render_spell_walk(ui.ctx());
    // EDITOR-11 — the external-change reload prompt, above everything (never a
    // silent clobber; the operator chooses keep-mine vs. reload-theirs).
    surface.render_reload_prompt(ui.ctx());
}

#[cfg(test)]
mod tests;
