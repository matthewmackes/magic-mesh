//! The **surface panel seam** (EDITOR-1 → EDITOR-3): the code-editor surface the
//! one Quasar shell (`mde-shell-egui`) embeds as `Surface::Editor`.
//!
//! Under E12 "Quasar" the mesh surfaces are **panels in the one shell**, not
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

use std::io;
use std::path::{Path, PathBuf};

use mde_egui::egui::{self, Align2, Key, Modifiers, RichText, Ui};
use mde_egui::Style;

use crate::buffer::Buffer;
use crate::finder::{self, FileFinder};
use crate::format_bar;
use crate::highlight::{Highlighter, Language};
use crate::lsp::LspClient;
use crate::lsp_ui::{lsp_status, DiagnosticsOverlay};
use crate::md_actions::{self, ListKind};
use crate::menu_bar::{self, ListStyle, MenuAction, MenuContext};
use crate::palette::{self, CommandPalette, PaletteCommand};
use crate::project_tree::{self, ProjectTree};
use crate::toolbar;
use crate::widget::{editor_widget, EditorView};

/// The project-tree side panel's default width — six shared spacing units (§4).
const TREE_WIDTH: f32 = Style::SP_XL * 6.0;

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

/// The About dialog's title (Help → About the Editor, EDTB-1).
const ABOUT_TITLE: &str = "About the Editor";

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
}

impl Doc {
    /// Wrap a freshly built buffer in a new view, picking the highlighter by
    /// the buffer's file extension (none for scratch/unknown — plain text). The
    /// language server is attached separately by [`start_lsp`](Self::start_lsp)
    /// (it needs the resolved project root, which the surface owns).
    fn new(buffer: Buffer) -> Self {
        let highlight = buffer.path().and_then(Highlighter::for_path);
        Self {
            buffer,
            view: EditorView::new(),
            highlight,
            lsp: None,
            diagnostics: DiagnosticsOverlay::default(),
            lsp_synced_gen: 0,
        }
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

/// The code-editor surface the E12 shell embeds.
///
/// The surface state the shell holds directly and drives with [`editor_panel`],
/// mirroring `mde-files-egui`'s `FileBrowser` seam. It owns the open document as
/// an `Option<Doc>`: `None` renders the honest empty state (§7); `Some` renders
/// the live text widget over the real rope. EDITOR-7/9 add fuzzy-open / Files
/// send by calling [`open_path`](Self::open_path) / [`open_text`](Self::open_text).
#[derive(Default)]
pub struct EditorSurface {
    /// The currently open document, or `None` for the empty state.
    doc: Option<Doc>,
    /// The project-tree panel over the open project root (EDITOR-9), or `None`
    /// before a folder is opened (via [`open_folder`](Self::open_folder) — the
    /// Files "Send-to-Editor" send, a folder picker, or the tree toggle default).
    project: Option<ProjectTree>,
    /// Whether the project-tree side panel is shown beside the editor body.
    show_tree: bool,
    /// The fuzzy file-finder overlay (EDITOR-7, `Cmd`/`Ctrl-P`).
    finder: FileFinder,
    /// The command-palette overlay (EDITOR-7, `Cmd`/`Ctrl-Shift-P`).
    palette: CommandPalette,
    /// The Save As… path dialog (EDTB-1, File → Save As).
    save_as: SaveAsDialog,
    /// Whether the About dialog is shown (EDTB-1, Help → About the Editor).
    about_open: bool,
    /// The Insert Table grid-picker dialog (EDTB-3, Insert → Table…).
    table_picker: TablePicker,
}

/// The Insert Table grid-picker's dialog state (EDTB-3): whether the Word
/// drag-grid overlay is shown. The hover selection is transient per frame
/// ([`format_bar::table_grid`]), so nothing else persists here.
#[derive(Default)]
struct TablePicker {
    /// Whether the picker window is shown.
    open: bool,
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

impl EditorSurface {
    /// Whether a document is currently open (the surface is showing the editor,
    /// not the empty state).
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.doc.is_some()
    }

    /// The path of the open document, if it has one (a scratch buffer has none).
    /// Exposes the active file for the chrome / a future tab title.
    #[must_use]
    pub fn current_path(&self) -> Option<&Path> {
        self.doc.as_ref().and_then(|doc| doc.buffer.path())
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

    /// Open an in-memory document seeded with `text` (no path). The open-a-buffer
    /// seam the finder / Files send drive; also the scratch affordance's backing.
    /// A pathless buffer starts no language server (§7 — nothing to serve).
    pub fn open_text(&mut self, text: &str) {
        self.set_doc(Some(Doc::new(Buffer::from_text(text))));
    }

    /// Open `path` from disk into the surface, replacing any open document and
    /// starting a language-server session for it (EDITOR-LSP-2 `didOpen`), rooted
    /// at the open project root when there is one, else the file's directory.
    ///
    /// # Errors
    /// Returns any [`io::Error`] from reading `path` (missing file, permissions).
    pub fn open_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let path = path.as_ref();
        let mut doc = Doc::new(Buffer::open(path)?);
        doc.start_lsp(&self.lsp_root_for(path));
        self.set_doc(Some(doc));
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

    /// Replace the open document, tearing down the outgoing document's language
    /// server first (EDITOR-LSP-2 `didClose`) so no session leaks across an
    /// open/close/switch.
    fn set_doc(&mut self, doc: Option<Doc>) {
        if let Some(existing) = self.doc.as_ref() {
            existing.close_lsp();
        }
        self.doc = doc;
    }

    /// Open a fresh scratch buffer (the temporary EDITOR-3 exercise affordance).
    pub fn open_scratch(&mut self) {
        self.open_text(SCRATCH_SEED);
    }

    /// Close the open document, returning the surface to the empty state and
    /// tearing down its language-server session (EDITOR-LSP-2 `didClose`).
    pub fn close(&mut self) {
        self.set_doc(None);
    }

    // ── EDITOR-7: the fuzzy finder + command palette ─────────────────────────

    /// Whether a modal overlay (the fuzzy file-finder, the command palette, or
    /// an EDTB-1 dialog — Save As / About) is currently up. The shell can key
    /// off this to suppress its own global chords while the editor surface is
    /// capturing the keyboard for an overlay.
    #[must_use]
    pub const fn overlay_active(&self) -> bool {
        self.finder.is_open()
            || self.palette.is_open()
            || self.save_as.open
            || self.about_open
            || self.table_picker.open
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
            self.palette.close(); // only one overlay at a time
            self.finder.open_at(&root);
        }
    }

    /// Toggle the command palette (the `Cmd`/`Ctrl-Shift-P` seam), closing the
    /// finder so only one overlay is up at a time.
    pub(crate) fn toggle_palette(&mut self) {
        self.finder.close();
        self.palette.toggle();
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
                if let Some(doc) = self.doc.as_mut() {
                    // A scratch buffer (no path) can't save; that's an honest no-op,
                    // surfaced by the dirty marker staying lit — not a panic.
                    let _ = doc.buffer.save();
                }
            }
            PaletteCommand::OpenScratch => self.open_scratch(),
            PaletteCommand::ToggleTree => self.show_tree = !self.show_tree,
            PaletteCommand::ToggleWrap => {
                if let Some(doc) = self.doc.as_mut() {
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

    // ── EDTB-1: the Word-97 menu bar + Standard toolbar ─────────────────────

    /// Snapshot the enablement + toggle/zoom state the menu bar and toolbar
    /// render from this frame — the bars' one read seam into the surface.
    pub(crate) fn menu_context(&self) -> MenuContext {
        let doc = self.doc.as_ref();
        MenuContext {
            has_doc: doc.is_some(),
            has_selection: doc.is_some_and(|d| d.view.has_selection()),
            can_undo: doc.is_some_and(|d| d.view.can_undo()),
            can_redo: doc.is_some_and(|d| d.view.can_redo()),
            tree_shown: self.show_tree,
            wrap_on: doc.is_some_and(|d| d.view.wrap()),
            zoom_percent: doc.map(|d| d.view.zoom_percent()),
            // The Format strip Style dropdown reads the primary caret line's
            // heading level (EDTB-3).
            heading_level: doc.map(|d| {
                let (line, _) = d.view.line_col(&d.buffer);
                heading_level_of(&d.buffer, line - 1)
            }),
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
            MenuAction::Undo => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.view.undo(&mut doc.buffer);
                }
            }
            MenuAction::Redo => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.view.redo(&mut doc.buffer);
                }
            }
            MenuAction::Cut => {
                // The widget's Ctrl-X arm exactly: copy the selection, then
                // delete it — the same two seams, menu-driven.
                self.copy_selection(ctx);
                if let Some(doc) = self.doc.as_mut() {
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
                if let Some(doc) = self.doc.as_mut() {
                    doc.view.select_all(&doc.buffer);
                }
            }
            MenuAction::ToggleTree => self.run_command(PaletteCommand::ToggleTree),
            MenuAction::ToggleWrap => self.run_command(PaletteCommand::ToggleWrap),
            MenuAction::CommandPalette => self.toggle_palette(),
            MenuAction::About => self.about_open = true,
            MenuAction::Zoom(percent) => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.view.set_zoom_percent(percent);
                }
            }
            // ── EDTB-3: the Formatting strip / Insert & Format menus ─────────
            // Each drives the landed `md_actions` engine on the live buffer as
            // ONE operator undo step (the widget's `apply_md` records the
            // engine's undo-group count). A no-op with no document (§7).
            MenuAction::Heading(level) => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.view.apply_md(&mut doc.buffer, |b, spans| {
                        md_actions::set_heading(b, spans, level)
                    });
                }
            }
            MenuAction::Wrap(marker) => {
                if let Some(doc) = self.doc.as_mut() {
                    let m = marker.marker();
                    doc.view.apply_md(&mut doc.buffer, |b, spans| {
                        md_actions::toggle_wrap(b, spans, m)
                    });
                }
            }
            MenuAction::List(style) => {
                if let Some(doc) = self.doc.as_mut() {
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
                if let Some(doc) = self.doc.as_mut() {
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
        if let Some(doc) = self.doc.as_mut() {
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
        if let Some(doc) = self.doc.as_ref() {
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
        let Some(doc) = self.doc.as_mut() else {
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
                        RichText::new("Quasar Editor")
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
        if open_palette {
            self.toggle_palette();
        }
        if open_finder {
            self.open_finder();
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
    }
}

/// The markdown ATX heading level of `line` (0-based) — the leading `#`-run
/// (1-6) when it is a real heading (followed by a space or the line end), else 0
/// (Normal body text). The Format strip's Style dropdown read-back (EDTB-3); a
/// cheap, allocation-light mirror of the engine's own `#`-run detection.
fn heading_level_of(buffer: &Buffer, line: usize) -> u8 {
    if line >= buffer.len_lines() {
        return 0;
    }
    let text = buffer.line(line);
    let hashes = text.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && matches!(text.chars().nth(hashes), Some(' ' | '\n') | None) {
        u8::try_from(hashes).unwrap_or(0)
    } else {
        0
    }
}

/// Render the editor surface into the shell body — mirrors `files_panel`.
///
/// When a document is open: a compact chrome strip (name, dirty marker, wrap
/// toggle, caret position) over the real text widget, which edits the live rope
/// (§7 — runtime-reachable, not a mockup). When none is open: the honest empty
/// state plus a temporary "open a scratch buffer" affordance so the surface is
/// exercisable before fuzzy-open lands. `surface` is the mount seam the shell
/// wires (the analogue of `files_panel`'s `&mut FileBrowser`).
pub fn editor_panel(ui: &mut Ui, surface: &mut EditorSurface) {
    // EDITOR-7 — panel-level keybind intercept. Consume the overlay trigger chords
    // FIRST, before the tree/central body render (the text widget clones this
    // frame's `ui.input` events during its own render below), so `Cmd`/`Ctrl-P`
    // and `Cmd`/`Ctrl-Shift-P` open the overlays instead of typing into the buffer.
    surface.handle_overlay_triggers(ui);

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

    egui::CentralPanel::default().show_inside(ui, |ui| {
        // No open document → the honest empty state + the scratch affordance. The
        // diverging `else` frees the `doc` borrow, so `open_scratch` can mutate.
        let Some(doc) = surface.doc.as_mut() else {
            editor_chrome(ui, &mut surface.show_tree);
            if empty_state(ui) {
                surface.open_scratch();
            }
            return;
        };
        doc_chrome(ui, doc, &mut surface.show_tree);
        ui.separator();
        // EDITOR-LSP-2 — refresh the diagnostics overlay from the server's store
        // (epoch-gated: a quiet frame skips it) so the widget paints the current
        // gutter markers + underlines this frame.
        doc.refresh_diagnostics();
        // Disjoint field borrows: the widget edits `&mut view` + `&mut buffer`,
        // syncs `&mut highlight` with this frame's edits (EDITOR-5), and reads
        // the shared `&diagnostics` overlay to paint them (EDITOR-LSP-2).
        editor_widget(
            ui,
            &mut doc.view,
            &mut doc.buffer,
            doc.highlight.as_mut(),
            &doc.diagnostics,
        );
        // EDITOR-LSP-2 — push this frame's edits to the server (one throttled
        // `didChange`, full-text sync) now the buffer has settled.
        doc.sync_lsp();
    });

    // EDITOR-7 — the finder + palette overlays float above the body (rendered last
    // so they paint on top); each returns the operator's pick, routed to its seam.
    surface.render_overlays(ui);

    // EDTB-1 — the Save As / About dialogs, rendered above everything.
    surface.render_dialogs(ui.ctx());
}

/// The project tree's "no folder open" face (§7) — an honest note plus a reachable
/// affordance to root the tree at the current working directory, so the tree is
/// exercisable before a Files send / folder picker lands. Sets `open_cwd` on click.
fn no_folder_face(ui: &mut Ui, open_cwd: &mut bool) {
    ui.add_space(Style::SP_M);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("No folder open")
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        *open_cwd = ui
            .button(RichText::new("Open current folder").size(Style::SMALL))
            .clicked();
    });
}

/// The left-edge toggle that shows/hides the project-tree side panel, shared by the
/// open-document and empty-state chromes. A token-styled (§4) `selectable_label`
/// bound to `show_tree`.
fn tree_toggle(ui: &mut Ui, show_tree: &mut bool) {
    if ui
        .selectable_label(*show_tree, RichText::new("\u{2630}").size(Style::BODY))
        .on_hover_text("Toggle the project tree")
        .clicked()
    {
        *show_tree = !*show_tree;
    }
}

/// The open-document chrome strip: the file name (or "scratch"), a dirty marker,
/// a soft-wrap toggle, and the caret's line:col — all through [`Style`] tokens
/// (§4). Honest chrome: it reflects the real buffer/view, no faked state.
fn doc_chrome(ui: &mut Ui, doc: &mut Doc, show_tree: &mut bool) {
    // Precompute every displayed value up front (owned), so the render closures
    // capture no borrow of `doc`; the one mutation (wrap toggle) is deferred to a
    // flag and applied after the strip.
    let name = doc.buffer.path().and_then(Path::file_name).map_or_else(
        || "scratch".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let dirty = if doc.buffer.is_dirty() {
        " \u{2022}"
    } else {
        ""
    };
    let lossy = doc.buffer.is_lossy();
    let (line, col) = doc.view.line_col(&doc.buffer);
    let wrap_on = doc.view.wrap();
    // The detected language (EDITOR-5) — honest chrome: shown only when a real
    // grammar is highlighting this document, absent for plain text.
    let lang = doc.highlight.as_ref().map(|hl| hl.language().name());
    // The language-server status (EDITOR-LSP-2) — honest chrome: an absent
    // server binary reads "<cmd>: not found", never a faked session (§7); a
    // serverless / stopped session shows nothing.
    let lsp = doc.lsp.as_ref().and_then(lsp_status);
    let mut toggle_wrap = false;

    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        tree_toggle(ui, show_tree);
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(format!("{name}{dirty}"))
                .size(Style::BODY)
                .color(Style::TEXT)
                .strong(),
        );
        if lossy {
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new("lossy decode")
                    .size(Style::SMALL)
                    .color(Style::WARN),
            );
        }
        // Right-aligned: the wrap toggle + the caret position + the language.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
        });
    });
    ui.add_space(Style::SP_XS);

    if toggle_wrap {
        doc.view.toggle_wrap();
    }
}

/// The editor's identity strip for the empty state — a compact header naming the
/// surface, drawn through [`Style`] tokens (§4).
fn editor_chrome(ui: &mut Ui, show_tree: &mut bool) {
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        tree_toggle(ui, show_tree);
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("Editor")
                .size(Style::BODY)
                .color(Style::TEXT)
                .strong(),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.separator();
}

/// The "no document" face — the honest empty state (§7), plus a temporary button
/// to open a scratch buffer so the surface is exercisable before fuzzy-open
/// lands. Returns `true` when the operator clicked the button (the caller opens
/// the buffer). Every value is a shared [`Style`] token (§4), no raw hex/metric.
fn empty_state(ui: &mut Ui) -> bool {
    let mut open = false;
    ui.vertical_centered(|ui| {
        ui.add_space(Style::SP_XL);
        ui.label(
            RichText::new(NO_FILE_TITLE)
                .size(Style::HEADING)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(NO_FILE_HINT)
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_L);
        open = ui
            .button(RichText::new("Open a scratch buffer").size(Style::BODY))
            .clicked();
    });
    open
}

#[cfg(test)]
mod tests {
    use super::{editor_panel, EditorSurface, NO_FILE_HINT, NO_FILE_TITLE, SCRATCH_SEED};
    use crate::menu_bar::MenuAction;
    use crate::palette::PaletteCommand;
    use crate::real_editor;
    use mde_egui::egui::{self, pos2, vec2, Event, Key, Modifiers, Rect};
    use mde_egui::Style;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Build a key-press event for the headless driver (mirrors `widget.rs`'s test
    /// helper): pressed, non-repeat, with the given modifiers.
    fn key_press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Drive one real `editor_panel` frame on a *persistent* `ctx` with injected
    /// `events`, so a multi-frame interaction (open an overlay, then act on it)
    /// exercises the true render + routing path — not a mocked seam.
    fn run_frame(ctx: &egui::Context, surface: &mut EditorSurface, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_panel(ui, surface);
            });
        });
    }

    /// A unique temp dir for a live editor test, cleaned up on drop (the crate has
    /// no `tempfile` dev-dep — the same idiom `project_tree`'s tests use).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "mde-editor-panel-{tag}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&base).expect("create temp dir");
            Self(base)
        }
        fn join(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// Drive one headless frame through the editor panel, tessellating on the CPU
    /// so any paint-path fault surfaces — the same `Context::run` → `tessellate`
    /// path the shell's mount test drives, minus the GPU. Returns the primitive
    /// count so callers can assert the surface actually paints.
    fn tessellate_panel(surface: &mut EditorSurface) -> usize {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_panel(ui, surface);
            });
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    /// Like [`tessellate_panel`] but at a caller-chosen panel width, so a test can
    /// drive the EDTB-4 wide (full) vs narrow (compact) bar layouts. Returns the
    /// primitive count so callers can assert the surface actually paints.
    fn tessellate_panel_at(surface: &mut EditorSurface, width: f32) -> usize {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_panel(ui, surface);
            });
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    #[test]
    fn empty_state_panel_mounts_and_renders_headless() {
        let mut surface = real_editor();
        assert!(!surface.is_open(), "a fresh surface opens no document");
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the empty-state editor surface produced no draw primitives"
        );
    }

    #[test]
    fn opening_a_document_renders_the_widget() {
        let mut surface = real_editor();
        surface.open_text("fn main() {}\n");
        assert!(surface.is_open(), "open_text opens a document");
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the open-document editor surface produced no draw primitives"
        );
        surface.close();
        assert!(!surface.is_open(), "close returns to the empty state");
    }

    #[test]
    fn open_scratch_seeds_a_real_editable_buffer() {
        let mut surface = EditorSurface::default();
        surface.open_scratch();
        assert!(surface.is_open());
        // The scratch seed is real, reachable text (not empty, not a stub).
        assert!(!SCRATCH_SEED.is_empty());
    }

    #[test]
    fn empty_state_copy_is_honest_and_reachable() {
        assert!(!NO_FILE_TITLE.is_empty(), "empty-state title is blank");
        assert!(!NO_FILE_HINT.is_empty(), "empty-state hint is blank");
        assert!(
            NO_FILE_TITLE.to_lowercase().contains("file"),
            "the headline should name the missing file"
        );
        let hint = NO_FILE_HINT.to_lowercase();
        assert!(
            hint.contains("open") && hint.contains("edit"),
            "the hint should tell the operator to open a file to edit"
        );
    }

    #[test]
    fn opening_a_source_file_attaches_its_highlighter() {
        // EDITOR-5 — the open-path seam picks the grammar by extension; a
        // pathless scratch buffer honestly stays plain.
        let d = TempDir::new("hl");
        let file = d.join("lib.rs");
        std::fs::write(&file, b"fn f() {}\n").expect("write");

        let mut surface = real_editor();
        surface.open_path(&file).expect("open");
        assert!(
            surface.doc.as_ref().expect("doc").highlight.is_some(),
            "a .rs file gets the rust highlighter"
        );
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the highlighted document renders"
        );

        surface.open_scratch();
        assert!(
            surface.doc.as_ref().expect("doc").highlight.is_none(),
            "a pathless scratch buffer renders plain (no guessed grammar)"
        );
    }

    #[test]
    fn open_folder_sets_the_project_root_and_shows_the_tree() {
        let d = TempDir::new("folder");
        std::fs::write(d.join("a.rs"), b"fn main() {}").expect("write");
        let mut surface = real_editor();
        assert!(!surface.has_project(), "a fresh surface has no project");
        surface.open_folder(d.0.clone());
        assert!(surface.has_project(), "open_folder roots the project tree");
    }

    #[test]
    fn a_tree_file_click_routes_through_open_path() {
        // The exact routing a project-tree file click drives: `show` yields the
        // clicked path, `editor_panel` hands it to `open_selected` → `open_path`.
        let d = TempDir::new("click");
        let file = d.join("hello.rs");
        std::fs::write(&file, b"fn hello() {}\n").expect("write");
        let mut surface = real_editor();
        surface.open_folder(d.0.clone());
        assert!(
            !surface.is_open(),
            "no document open until a file is picked"
        );

        surface.open_selected(&file);
        assert!(surface.is_open(), "the picked file opened a document");
        assert_eq!(
            surface.current_path(),
            Some(file.as_path()),
            "open-on-click opened the exact clicked file"
        );
    }

    #[test]
    fn project_tree_panel_tessellates_over_a_real_dir() {
        // The whole editor body with the tree side panel shown paints real
        // primitives over a real directory listing (§7 — a reachable render path).
        let d = TempDir::new("render");
        std::fs::create_dir(d.join("src")).expect("mkdir src");
        std::fs::write(d.join("Cargo.toml"), b"[package]").expect("write");
        let mut surface = real_editor();
        surface.open_folder(d.0.clone());
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the editor + project tree produced no draw primitives"
        );
    }

    // ── EDITOR-7: the fuzzy finder + command palette ─────────────────────────

    #[test]
    fn cmd_p_then_enter_opens_the_selected_file_through_open_path() {
        // The full select→open routing, driven end-to-end through the real panel:
        // Cmd+P opens the finder (rooted at the project), Enter opens the
        // highlighted file via `open_path`. No mocked seam — real frames + events.
        let d = TempDir::new("finder-route");
        let file = d.join("routing_target.rs");
        std::fs::write(&file, b"fn go() {}\n").expect("write");

        let mut surface = real_editor();
        surface.open_folder(d.0.clone());

        let ctx = egui::Context::default();
        Style::install(&ctx);

        // Frame 1: Cmd+P opens the finder over the one seeded file (empty query
        // lists it, selection at row 0).
        run_frame(
            &ctx,
            &mut surface,
            vec![key_press(Key::P, Modifiers::COMMAND)],
        );
        assert!(surface.finder.is_open(), "Cmd+P opened the file finder");
        assert!(!surface.is_open(), "no document is open yet");

        // Frame 2: Enter opens the highlighted result, routed to `open_path`.
        run_frame(
            &ctx,
            &mut surface,
            vec![key_press(Key::Enter, Modifiers::NONE)],
        );
        assert!(surface.is_open(), "Enter opened a document");
        assert_eq!(
            surface.current_path(),
            Some(file.as_path()),
            "the finder opened the exact file through open_path"
        );
        assert!(
            !surface.finder.is_open(),
            "the finder closed after the pick"
        );
    }

    #[test]
    fn cmd_shift_p_toggles_the_palette_at_the_panel_level() {
        // The palette chord is intercepted at the panel level (not the widget), so
        // pressing it opens the overlay; pressing it again closes it.
        let mut surface = real_editor();
        surface.open_text("fn main() {}\n");

        let ctx = egui::Context::default();
        Style::install(&ctx);

        let chord = || {
            key_press(
                Key::P,
                Modifiers {
                    shift: true,
                    ..Modifiers::COMMAND
                },
            )
        };
        run_frame(&ctx, &mut surface, vec![chord()]);
        assert!(surface.palette.is_open(), "Cmd+Shift+P opened the palette");
        run_frame(&ctx, &mut surface, vec![chord()]);
        assert!(
            !surface.palette.is_open(),
            "Cmd+Shift+P again closed the palette"
        );
    }

    #[test]
    fn palette_command_save_writes_the_buffer_to_disk() {
        let d = TempDir::new("cmd-save");
        let file = d.join("save.txt");
        std::fs::write(&file, b"abc").expect("seed file");
        let mut surface = real_editor();
        surface.open_path(&file).expect("open");
        // Dirty the buffer, then dispatch Save through the palette seam.
        surface.doc.as_mut().expect("doc").buffer.insert(3, "DEF");
        assert!(surface.doc.as_ref().expect("doc").buffer.is_dirty());

        surface.run_command(PaletteCommand::Save);

        assert!(
            !surface.doc.as_ref().expect("doc").buffer.is_dirty(),
            "Save cleared the dirty flag"
        );
        assert_eq!(
            std::fs::read(&file).expect("read back"),
            b"abcDEF",
            "the Save command wrote the on-disk bytes (buffer.save)"
        );
    }

    #[test]
    fn palette_command_open_scratch_opens_a_document() {
        let mut surface = real_editor();
        assert!(!surface.is_open());
        surface.run_command(PaletteCommand::OpenScratch);
        assert!(surface.is_open(), "Open Scratch opened a document");
    }

    #[test]
    fn palette_command_toggle_tree_flips_the_side_panel() {
        let mut surface = real_editor();
        let before = surface.show_tree;
        surface.run_command(PaletteCommand::ToggleTree);
        assert_ne!(
            surface.show_tree, before,
            "Toggle Project Tree flipped the side panel"
        );
    }

    #[test]
    fn palette_command_toggle_wrap_flips_the_editor_wrap() {
        let mut surface = real_editor();
        surface.open_text("a long line\n");
        let before = surface.doc.as_ref().expect("doc").view.wrap();
        surface.run_command(PaletteCommand::ToggleWrap);
        assert_ne!(
            surface.doc.as_ref().expect("doc").view.wrap(),
            before,
            "Toggle Soft-Wrap flipped the view's wrap"
        );
    }

    #[test]
    fn palette_command_close_document_returns_to_empty_state() {
        let mut surface = real_editor();
        surface.open_text("x\n");
        assert!(surface.is_open());
        surface.run_command(PaletteCommand::CloseDoc);
        assert!(
            !surface.is_open(),
            "Close Document returned to the empty state"
        );
    }

    #[test]
    fn palette_command_open_folder_roots_the_tree() {
        let mut surface = real_editor();
        assert!(!surface.has_project());
        surface.run_command(PaletteCommand::OpenFolderCwd);
        assert!(
            surface.has_project(),
            "Open Folder rooted the project tree at the cwd"
        );
    }

    #[test]
    fn the_finder_overlay_paints_when_open() {
        let d = TempDir::new("finder-paint");
        std::fs::write(d.join("a.rs"), b"a").expect("write");
        let mut surface = real_editor();
        surface.open_folder(d.0.clone());
        surface.open_finder();
        assert!(surface.finder.is_open());
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the open finder overlay produced no draw primitives"
        );
    }

    #[test]
    fn the_palette_overlay_paints_when_open() {
        let mut surface = real_editor();
        surface.toggle_palette();
        assert!(surface.palette.is_open());
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the open palette overlay produced no draw primitives"
        );
    }

    // ── EDTB-1: the menu bar + Standard toolbar dispatch ────────────────────

    /// Run `action` through the real dispatch inside a live frame, returning
    /// the frame's [`egui::FullOutput`] so tests can observe the platform
    /// effects (clipboard commands, viewport commands).
    fn run_action_in_frame(surface: &mut EditorSurface, action: MenuAction) -> egui::FullOutput {
        let ctx = egui::Context::default();
        ctx.run(egui::RawInput::default(), |ctx| {
            surface.run_action(ctx, action);
        })
    }

    #[test]
    fn menu_new_scratch_opens_a_document() {
        let mut surface = real_editor();
        run_action_in_frame(&mut surface, MenuAction::NewScratch);
        assert!(surface.is_open(), "File > New opened a scratch document");
    }

    #[test]
    fn menu_open_routes_to_the_finder() {
        let mut surface = real_editor();
        run_action_in_frame(&mut surface, MenuAction::OpenFinder);
        assert!(
            surface.finder.is_open(),
            "File > Open… opened the Ctrl-P finder"
        );
    }

    #[test]
    fn menu_open_folder_roots_the_project_tree() {
        let mut surface = real_editor();
        assert!(!surface.has_project());
        run_action_in_frame(&mut surface, MenuAction::OpenFolderCwd);
        assert!(
            surface.has_project(),
            "File > Open Folder rooted the tree at the cwd"
        );
    }

    #[test]
    fn menu_save_writes_the_buffer_through_the_palette_seam() {
        let d = TempDir::new("menu-save");
        let file = d.join("menu.txt");
        std::fs::write(&file, b"abc").expect("seed");
        let mut surface = real_editor();
        surface.open_path(&file).expect("open");
        surface.doc.as_mut().expect("doc").buffer.insert(3, "!");
        run_action_in_frame(&mut surface, MenuAction::Save);
        assert_eq!(
            std::fs::read(&file).expect("read back"),
            b"abc!",
            "File > Save wrote the bytes (the same run_command(Save) seam)"
        );
    }

    #[test]
    fn menu_save_as_commits_to_a_new_path_and_repicks_the_highlighter() {
        let d = TempDir::new("menu-save-as");
        let mut surface = real_editor();
        surface.open_text("fn f() {}\n");
        run_action_in_frame(&mut surface, MenuAction::SaveAs);
        assert!(surface.save_as.open, "File > Save As… opened the dialog");
        assert!(
            surface.overlay_active(),
            "the open dialog reports as an active overlay"
        );
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the Save As dialog produced no draw primitives"
        );

        let target = d.join("adopted.rs");
        surface.save_as.path = target.display().to_string();
        surface.save_as_commit();

        assert!(!surface.save_as.open, "a successful save closes the dialog");
        assert_eq!(
            std::fs::read(&target).expect("read back"),
            b"fn f() {}\n",
            "Save As wrote the buffer to the new path"
        );
        assert_eq!(
            surface.current_path(),
            Some(target.as_path()),
            "the buffer adopted the new path"
        );
        assert!(
            surface.doc.as_ref().expect("doc").highlight.is_some(),
            "the .rs path re-picked a highlighter for the renamed document"
        );
    }

    #[test]
    fn save_as_failure_keeps_the_dialog_open_with_the_error() {
        let mut surface = real_editor();
        surface.open_text("x");
        run_action_in_frame(&mut surface, MenuAction::SaveAs);
        surface.save_as.path = "/nonexistent-dir-mde-editor/x.txt".to_owned();
        surface.save_as_commit();
        assert!(surface.save_as.open, "a failed write keeps the dialog open");
        assert!(
            surface.save_as.error.is_some(),
            "the write error is shown, not swallowed"
        );
    }

    #[test]
    fn menu_close_returns_to_the_empty_state() {
        let mut surface = real_editor();
        surface.open_text("x");
        run_action_in_frame(&mut surface, MenuAction::CloseDoc);
        assert!(!surface.is_open(), "File > Close closed the document");
    }

    #[test]
    fn menu_undo_and_redo_unwind_a_typed_edit() {
        // Type through a REAL widget frame (the same path the keyboard takes),
        // then unwind/redo through the menu seams.
        let mut surface = real_editor();
        surface.open_text("abc");
        let ctx = egui::Context::default();
        Style::install(&ctx);
        run_frame(&ctx, &mut surface, vec![Event::Text("X".to_owned())]);
        assert_eq!(
            surface.doc.as_ref().expect("doc").buffer.rope().to_string(),
            "Xabc",
            "the frame's Text event inserted at the caret"
        );
        assert!(surface.menu_context().can_undo, "the edit armed Undo");

        surface.run_action(&ctx, MenuAction::Undo);
        assert_eq!(
            surface.doc.as_ref().expect("doc").buffer.rope().to_string(),
            "abc",
            "Edit > Undo unwound the typed edit"
        );
        assert!(surface.menu_context().can_redo, "undo armed Redo");

        surface.run_action(&ctx, MenuAction::Redo);
        assert_eq!(
            surface.doc.as_ref().expect("doc").buffer.rope().to_string(),
            "Xabc",
            "Edit > Redo re-applied the edit"
        );
    }

    #[test]
    fn menu_select_all_selects_the_whole_buffer() {
        let mut surface = real_editor();
        surface.open_text("hello");
        run_action_in_frame(&mut surface, MenuAction::SelectAll);
        assert_eq!(
            surface.doc.as_ref().expect("doc").view.selection(),
            Some(0..5),
            "Edit > Select All selected the whole document"
        );
        assert!(surface.menu_context().has_selection);
    }

    #[test]
    fn menu_copy_emits_the_clipboard_command_with_the_selection() {
        let mut surface = real_editor();
        surface.open_text("hello");
        run_action_in_frame(&mut surface, MenuAction::SelectAll);
        let out = run_action_in_frame(&mut surface, MenuAction::Copy);
        let copied = out
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == "hello"));
        assert!(copied, "Edit > Copy put the selection on the clipboard");
        assert_eq!(
            surface.doc.as_ref().expect("doc").buffer.rope().to_string(),
            "hello",
            "Copy leaves the buffer untouched"
        );
    }

    #[test]
    fn menu_cut_copies_then_deletes_the_selection() {
        let mut surface = real_editor();
        surface.open_text("hello");
        run_action_in_frame(&mut surface, MenuAction::SelectAll);
        let out = run_action_in_frame(&mut surface, MenuAction::Cut);
        let copied = out
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == "hello"));
        assert!(copied, "Edit > Cut copied the selection first");
        assert_eq!(
            surface.doc.as_ref().expect("doc").buffer.rope().to_string(),
            "",
            "Edit > Cut deleted the selection"
        );
    }

    #[test]
    fn menu_paste_requests_the_platform_clipboard() {
        // The dispatch asks the backend for its clipboard; the backend answers
        // with the same `Event::Paste` the widget's Ctrl-V inserts (that insert
        // path is covered by the widget's own paste tests).
        let mut surface = real_editor();
        surface.open_text("x");
        let out = run_action_in_frame(&mut surface, MenuAction::Paste);
        let requested = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| {
                v.commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::RequestPaste))
            });
        assert!(requested, "Edit > Paste sent ViewportCommand::RequestPaste");
    }

    #[test]
    fn menu_toggles_and_palette_route_to_their_seams() {
        let mut surface = real_editor();
        surface.open_text("a long line\n");
        let tree_before = surface.show_tree;
        run_action_in_frame(&mut surface, MenuAction::ToggleTree);
        assert_ne!(surface.show_tree, tree_before, "View > Project Tree flips");

        let wrap_before = surface.doc.as_ref().expect("doc").view.wrap();
        run_action_in_frame(&mut surface, MenuAction::ToggleWrap);
        assert_ne!(
            surface.doc.as_ref().expect("doc").view.wrap(),
            wrap_before,
            "View > Soft-Wrap flips"
        );

        run_action_in_frame(&mut surface, MenuAction::CommandPalette);
        assert!(
            surface.palette.is_open(),
            "Tools > Command Palette… opened the overlay"
        );
    }

    #[test]
    fn menu_about_opens_the_dialog_and_it_paints() {
        let mut surface = real_editor();
        run_action_in_frame(&mut surface, MenuAction::About);
        assert!(surface.about_open, "Help > About opened the dialog");
        assert!(
            surface.overlay_active(),
            "the About dialog reports as an active overlay"
        );
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the About dialog produced no draw primitives"
        );
        assert!(
            !super::ABOUT_VERSION_LINE.is_empty()
                && super::ABOUT_VERSION_LINE.contains("mde-editor-egui"),
            "the About line names the crate + version"
        );
    }

    #[test]
    fn toolbar_zoom_sets_the_editor_view_scale() {
        let mut surface = real_editor();
        surface.open_text("x");
        assert_eq!(
            surface.menu_context().zoom_percent,
            Some(100),
            "a fresh document opens at 100%"
        );
        run_action_in_frame(&mut surface, MenuAction::Zoom(150));
        assert_eq!(
            surface.menu_context().zoom_percent,
            Some(150),
            "the Zoom dropdown set the view's font scale"
        );
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the zoomed editor still paints"
        );
    }

    #[test]
    fn zoom_without_a_document_is_a_genuine_no_op() {
        let mut surface = real_editor();
        assert_eq!(
            surface.menu_context().zoom_percent,
            None,
            "no document, no zoom value (the dropdown is omitted)"
        );
        run_action_in_frame(&mut surface, MenuAction::Zoom(150));
        assert!(!surface.is_open(), "Zoom with no document changed nothing");
    }

    #[test]
    fn doc_gated_actions_are_no_ops_on_the_empty_surface() {
        // Every doc-gated dispatch arm survives the empty state as a genuine
        // no-op (§7 — never a panic); the bars also grey these out.
        let mut surface = real_editor();
        for action in [
            MenuAction::Save,
            MenuAction::CloseDoc,
            MenuAction::Undo,
            MenuAction::Redo,
            MenuAction::Cut,
            MenuAction::Copy,
            MenuAction::SelectAll,
            MenuAction::ToggleWrap,
            MenuAction::Zoom(200),
        ] {
            run_action_in_frame(&mut surface, action);
        }
        assert!(!surface.is_open(), "the surface stayed in the empty state");
    }

    // ── EDTB-3: the Formatting strip + Insert/Table + Format menus ───────────

    use crate::menu_bar::{ListStyle, WrapMarker};

    /// The current buffer text of the open document (test helper).
    fn text_of(surface: &EditorSurface) -> String {
        surface.doc.as_ref().expect("doc").buffer.rope().to_string()
    }

    #[test]
    fn format_bold_wraps_the_selection_as_one_undo_step() {
        // The exact seam the strip's B button (and the Format → Bold menu twin)
        // emit — dispatched through the real `run_action`, observed on the bytes.
        let mut surface = real_editor();
        surface.open_text("word");
        run_action_in_frame(&mut surface, MenuAction::SelectAll);
        run_action_in_frame(&mut surface, MenuAction::Wrap(WrapMarker::Bold));
        assert_eq!(text_of(&surface), "**word**", "Bold wrapped the selection");
        assert!(surface.menu_context().can_undo, "the format op armed Undo");

        // ONE operator undo step reverts the whole wrap.
        run_action_in_frame(&mut surface, MenuAction::Undo);
        assert_eq!(text_of(&surface), "word", "one Undo reverts the wrap");
        assert!(
            !surface.menu_context().can_undo,
            "the wrap was exactly one step"
        );
    }

    #[test]
    fn format_italic_underline_and_strike_wrap_with_their_markers() {
        for (marker, wrapped) in [
            (WrapMarker::Italic, "*word*"),
            (WrapMarker::Underline, "<u>word</u>"),
            (WrapMarker::Strike, "~~word~~"),
        ] {
            let mut surface = real_editor();
            surface.open_text("word");
            run_action_in_frame(&mut surface, MenuAction::SelectAll);
            run_action_in_frame(&mut surface, MenuAction::Wrap(marker));
            assert_eq!(text_of(&surface), wrapped, "{marker:?} wraps its markup");
        }
    }

    #[test]
    fn format_heading_sets_the_hash_prefix_at_the_caret_line() {
        let mut surface = real_editor();
        surface.open_text("title\nbody\n");
        // Caret opens at line 0; the Style dropdown → Heading 2 hashes that line.
        run_action_in_frame(&mut surface, MenuAction::Heading(2));
        assert_eq!(text_of(&surface), "## title\nbody\n");
        // Normal Text (level 0) strips it back.
        run_action_in_frame(&mut surface, MenuAction::Heading(0));
        assert_eq!(text_of(&surface), "title\nbody\n");
    }

    #[test]
    fn format_bullet_and_numbered_lists_toggle_the_selected_lines() {
        let mut surface = real_editor();
        surface.open_text("a\nb\n");
        run_action_in_frame(&mut surface, MenuAction::SelectAll);
        run_action_in_frame(&mut surface, MenuAction::List(ListStyle::Bullet));
        assert_eq!(text_of(&surface), "- a\n- b\n", "bullets on both lines");

        run_action_in_frame(&mut surface, MenuAction::SelectAll);
        run_action_in_frame(&mut surface, MenuAction::List(ListStyle::Numbered));
        assert_eq!(text_of(&surface), "1. a\n2. b\n", "converted to numbers");
    }

    #[test]
    fn format_indent_shifts_the_caret_line_and_round_trips() {
        let mut surface = real_editor();
        surface.open_text("x\n");
        run_action_in_frame(&mut surface, MenuAction::Indent(1));
        assert_eq!(
            text_of(&surface),
            "  x\n",
            "increase indent adds two spaces"
        );
        run_action_in_frame(&mut surface, MenuAction::Indent(-1));
        assert_eq!(text_of(&surface), "x\n", "decrease indent removes them");
    }

    #[test]
    fn insert_table_action_drops_a_skeleton_as_one_undo_step() {
        let mut surface = real_editor();
        surface.open_text("");
        run_action_in_frame(&mut surface, MenuAction::InsertTable { rows: 2, cols: 3 });
        assert_eq!(
            text_of(&surface),
            "| Col 1 | Col 2 | Col 3 |\n\
             | ----- | ----- | ----- |\n\
             |       |       |       |\n\
             |       |       |       |\n",
            "the grid-picker inserts a markdown table skeleton"
        );
        assert!(surface.menu_context().can_undo, "the insert armed Undo");
        run_action_in_frame(&mut surface, MenuAction::Undo);
        assert_eq!(text_of(&surface), "", "one Undo removes the whole table");
    }

    #[test]
    fn insert_table_picker_opens_and_paints() {
        let mut surface = real_editor();
        surface.open_text("x");
        run_action_in_frame(&mut surface, MenuAction::InsertTablePicker);
        assert!(
            surface.table_picker.open,
            "Insert → Table… opened the picker"
        );
        assert!(
            surface.overlay_active(),
            "the open picker reports as an active overlay"
        );
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the grid-picker dialog produced no draw primitives"
        );
    }

    #[test]
    fn the_style_dropdown_reads_back_the_caret_heading_level() {
        let mut surface = real_editor();
        surface.open_text("## heading\nbody\n");
        assert_eq!(
            surface.menu_context().heading_level,
            Some(2),
            "the Style box reflects the caret line's `##`"
        );
        // A non-heading line reads back Normal (0).
        surface.open_text("plain\n");
        assert_eq!(surface.menu_context().heading_level, Some(0));
        // No document → no read-back (the strip greys out).
        surface.close();
        assert_eq!(surface.menu_context().heading_level, None);
    }

    #[test]
    fn format_actions_are_no_ops_on_the_empty_surface() {
        // Every EDTB-3 dispatch arm survives the empty state as a genuine no-op
        // (§7); the Formatting strip also greys these out (Gate::Doc).
        let mut surface = real_editor();
        for action in [
            MenuAction::Heading(3),
            MenuAction::Wrap(WrapMarker::Bold),
            MenuAction::List(ListStyle::Numbered),
            MenuAction::Indent(1),
            MenuAction::InsertTable { rows: 2, cols: 2 },
        ] {
            run_action_in_frame(&mut surface, action);
        }
        assert!(!surface.is_open(), "the surface stayed in the empty state");
        // The picker toggle is harmless with no document (grid-picker acts at
        // the caret only once a document is open).
        run_action_in_frame(&mut surface, MenuAction::InsertTablePicker);
        run_action_in_frame(&mut surface, MenuAction::InsertTable { rows: 1, cols: 1 });
        assert!(!surface.is_open());
    }

    #[test]
    fn the_format_strip_paints_over_an_open_document() {
        // The whole three-bar chrome (menu + Standard + Formatting) tessellates
        // real primitives — the Formatting strip is mounted + reachable (§7).
        let mut surface = real_editor();
        surface.open_text("# title\n\n- item\n");
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the editor with the Formatting strip produced no draw primitives"
        );
    }

    // ── EDITOR-LSP-2: the language-server lifecycle wiring ───────────────────
    //
    // The suite never spawns a real OS language server (see `build_lsp_client`
    // under `cfg(test)`): a serverless language (`.md` → NoServer) yields a
    // gated client with no process, so the open/change/close *wiring* is
    // exercised — the doc-sync calls are honest no-ops on the gated client. The
    // honest gated *statuses* + the diagnostics paint are covered by `lsp_ui`'s
    // and `widget`'s own tests.

    #[test]
    fn opening_a_recognized_file_starts_a_language_client() {
        let d = TempDir::new("lsp-open");
        let file = d.join("notes.md");
        std::fs::write(&file, b"# Title\n\nbody\n").expect("write");
        let mut surface = real_editor();
        surface.open_path(&file).expect("open");
        assert!(
            surface.doc.as_ref().expect("doc").lsp.is_some(),
            "opening a file with a known language attaches a client (didOpen)"
        );
    }

    #[test]
    fn a_scratch_buffer_starts_no_language_client() {
        let mut surface = real_editor();
        surface.open_scratch();
        assert!(
            surface.doc.as_ref().expect("doc").lsp.is_none(),
            "a pathless scratch buffer has nothing to serve"
        );
    }

    #[test]
    fn a_plain_text_file_starts_no_language_client() {
        let d = TempDir::new("lsp-plain");
        let file = d.join("readme.txt");
        std::fs::write(&file, b"just prose\n").expect("write");
        let mut surface = real_editor();
        surface.open_path(&file).expect("open");
        assert!(
            surface.doc.as_ref().expect("doc").lsp.is_none(),
            "an unknown extension has no language — no server"
        );
    }

    #[test]
    fn editing_an_open_file_pushes_a_didchange_each_frame() {
        // The per-frame `didChange` wiring: a real typed frame bumps the edit
        // generation and the panel's sync point advances `lsp_synced_gen` to
        // match — proof `on_change` fired for the settled buffer.
        let d = TempDir::new("lsp-change");
        let file = d.join("doc.md");
        std::fs::write(&file, b"body\n").expect("write");
        let mut surface = real_editor();
        surface.open_path(&file).expect("open");
        assert_eq!(
            surface.doc.as_ref().expect("doc").lsp_synced_gen,
            0,
            "a freshly opened doc is synced at generation 0"
        );

        let ctx = egui::Context::default();
        Style::install(&ctx);
        run_frame(&ctx, &mut surface, vec![Event::Text("X".to_owned())]);

        let doc = surface.doc.as_ref().expect("doc");
        assert!(
            doc.view.edit_generation() >= 1,
            "the typed frame recorded a real edit"
        );
        assert_eq!(
            doc.lsp_synced_gen,
            doc.view.edit_generation(),
            "the panel pushed the settled buffer to the server (didChange)"
        );
    }

    #[test]
    fn a_caret_only_frame_sends_no_didchange() {
        // The throttle: an arrow-key frame moves the caret but does not change
        // the buffer, so the sync generation must not advance.
        let d = TempDir::new("lsp-quiet");
        let file = d.join("doc.md");
        std::fs::write(&file, b"abc\n").expect("write");
        let mut surface = real_editor();
        surface.open_path(&file).expect("open");

        let ctx = egui::Context::default();
        Style::install(&ctx);
        run_frame(
            &ctx,
            &mut surface,
            vec![key_press(Key::ArrowRight, Modifiers::NONE)],
        );
        let doc = surface.doc.as_ref().expect("doc");
        assert_eq!(
            doc.lsp_synced_gen, 0,
            "a caret-only frame is not resent to the server"
        );
    }

    #[test]
    fn closing_a_document_tears_down_the_client_without_panic() {
        // Close fires didClose + shutdown then drops the client — the graceful
        // teardown path, exercised end to end.
        let d = TempDir::new("lsp-close");
        let file = d.join("x.md");
        std::fs::write(&file, b"hi\n").expect("write");
        let mut surface = real_editor();
        surface.open_path(&file).expect("open");
        assert!(surface.is_open());
        surface.close();
        assert!(!surface.is_open(), "close returned to the empty state");
    }

    #[test]
    fn switching_documents_closes_the_previous_client() {
        // Opening a second file replaces the doc through `set_doc`, which closes
        // the outgoing document's server first — no leaked session.
        let d = TempDir::new("lsp-switch");
        let first = d.join("a.md");
        let second = d.join("b.md");
        std::fs::write(&first, b"a\n").expect("write");
        std::fs::write(&second, b"b\n").expect("write");
        let mut surface = real_editor();
        surface.open_path(&first).expect("open first");
        surface.open_path(&second).expect("open second");
        assert_eq!(
            surface.current_path(),
            Some(second.as_path()),
            "the second file is now the open document"
        );
        assert!(
            surface.doc.as_ref().expect("doc").lsp.is_some(),
            "the second document has its own client"
        );
    }

    // ── EDTB-4: the compact-aware bars ───────────────────────────────────────

    #[test]
    fn the_compact_threshold_switches_at_the_token_width() {
        // The layout decision is a pure fn of the panel's available width: at or
        // above the token-derived threshold the full bars render; just below it
        // the strips lean out (compact).
        assert!(
            !super::is_compact(super::COMPACT_WIDTH),
            "at the threshold the full bars render"
        );
        assert!(!super::is_compact(super::COMPACT_WIDTH + 1.0));
        assert!(
            super::is_compact(super::COMPACT_WIDTH - 1.0),
            "just under the threshold is compact"
        );
        assert!(super::is_compact(320.0), "a phone-narrow panel is compact");
        assert!(!super::is_compact(1280.0), "a desktop-wide panel is full");
    }

    #[test]
    fn the_full_bars_render_at_a_wide_panel() {
        // §7 — at a wide panel the full three-bar chrome (menu + Standard +
        // Formatting, the Style + Zoom dropdowns inline) paints real primitives.
        let mut surface = real_editor();
        surface.open_text("# title\n\nbody\n");
        assert!(!super::is_compact(1200.0), "1200px is the full layout");
        assert!(
            tessellate_panel_at(&mut surface, 1200.0) > 0,
            "the full bars produced no draw primitives"
        );
    }

    #[test]
    fn the_bars_lean_out_and_still_paint_at_a_narrow_panel() {
        // §7 — at a narrow panel the strips go compact (the wide dropdowns fold
        // into the `»` overflow) and still paint real primitives over an open
        // document (Zoom + Style present, so both overflows render).
        let mut surface = real_editor();
        surface.open_text("# title\n\nbody\n");
        assert!(super::is_compact(400.0), "400px is the compact layout");
        assert!(
            tessellate_panel_at(&mut surface, 400.0) > 0,
            "the compact bars produced no draw primitives"
        );
    }

    #[test]
    fn compact_bars_keep_every_command_reachable_and_dispatching() {
        // §7 — the folded controls are relocated into the `»` overflow, never
        // lost: the SAME MenuAction they emit still dispatches. Render the whole
        // panel narrow (compact), then drive the two overflowed controls' actions
        // through the real seam — Zoom (Standard-strip overflow) and Heading
        // (Format-strip overflow) both act on the live document at compact width.
        let mut surface = real_editor();
        surface.open_text("title\n");
        assert!(
            super::is_compact(400.0),
            "the panel is in the compact layout at this width"
        );
        assert!(
            tessellate_panel_at(&mut surface, 400.0) > 0,
            "the compact bars paint"
        );
        // The overflowed Zoom still zooms…
        run_action_in_frame(&mut surface, MenuAction::Zoom(150));
        assert_eq!(
            surface.menu_context().zoom_percent,
            Some(150),
            "the overflow Zoom still sets the view scale"
        );
        // …and the overflowed paragraph Style still sets the heading.
        run_action_in_frame(&mut surface, MenuAction::Heading(2));
        assert_eq!(
            text_of(&surface),
            "## title\n",
            "the overflow Style still sets the caret-line heading"
        );
    }
}
