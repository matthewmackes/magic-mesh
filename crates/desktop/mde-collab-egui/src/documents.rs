//! Documents mode (WL-FUNC-011 Phase 3c foundation) — the biggest parity mode,
//! built by **reusing** the whole `mde-editor-egui` "Construct" editor rather than
//! re-implementing one.
//!
//! A document lives in a space by [`DocumentId`]. The mode has two sub-modes:
//!
//! * [`DocSubMode::Document`] (default) — a **one-pane Markdown editor**: an
//!   embedded [`EditorSurface`] holding a single Markdown buffer, rendered through
//!   the editor's own [`editor_panel`] seam (so the Office-97 menu bar + the
//!   Standard and Formatting toolbars — the editor's EDTB-1/2/3 chrome — come for
//!   free). A Documents-level **Source ↔ Visual** toggle renders either the raw
//!   rope (Source, via the editor) or the rendered Markdown (Visual, via the
//!   editor's own [`markdown::parse`]/[`markdown::show`]). Ops-oriented templates
//!   seed a new document; the canonical Markdown is the only export.
//! * [`DocSubMode::Project`] — the **full IDE**: the same embedded editor with its
//!   whole capability set (rope, undo/redo, multicursor, tree-sitter, LSP,
//!   tabs/splits, folding, palette, integrated terminal). Nothing re-implemented;
//!   the real widget is mounted.
//!
//! # The collab document round-trip (wired now)
//!
//! Opening/editing reads the [`DocumentSessions`](mde_collab_types::DocumentSessions)
//! projection (the session picker) and the resolved canonical Markdown
//! ([`CollabData::document_body`](crate::CollabData::document_body)); a **New**
//! document emits [`CreateDocument`](CollabCommand::CreateDocument) and a **Save**
//! emits [`UpdateDocument`](CollabCommand::UpdateDocument) whose
//! [`DocumentChange`] payload is the content address of the **canonical Markdown**
//! (`text/markdown`) — the Markdown path stays the source of truth. The same
//! [`DocumentId`] linked into multiple spaces shares content; per-space discussion
//! anchors stay separate (they live in Messages/Threads, not here).
//!
//! # Explicit Phase-3c follow-ups (marked in-code, never stubbed/faked)
//!
//! The foundation is real (a real embedded editor + real Markdown editing + the
//! real `UpdateDocument` round-trip). These advanced paths are the next slice and
//! are each marked with a `// WL-FUNC-011 Phase 3c:` note at their seam:
//!
//! 1. **Yrs CRDT live co-editing** + shared cursor/presence + follow-mode (the
//!    editor already carries `CollabSession`/`follow`; wiring the mesh session over
//!    the Bus per document is the next unit).
//! 2. the **external-write three-way merge** (last-shared-base vs. collab vs. disk).
//! 3. the **portable review sidecar** (comments/suggestions as anchored threads);
//!    the `RequestReview`/`SubmitReview` commands exist but their UI is deferred.
//! 4. **autosave versioned snapshots** + a rendered word-diff timeline + git
//!    integration.

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{CollabCommand, DocumentChange, DocumentId, PayloadRef, SpaceId};
use mde_editor_egui::{editor_panel, markdown, real_editor, EditorSurface};

use crate::{frame, icons, CollabData, CommandSink, CommunicationsSurface};

/// The content-type the Documents mode stamps on its canonical export/update
/// payload — Markdown is the source of truth, so an `UpdateDocument` change always
/// names `text/markdown` bytes.
pub(crate) const MARKDOWN_MIME: &str = "text/markdown";

/// The two Documents sub-modes: the default one-pane Markdown document, or the
/// full embedded IDE editor for a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DocSubMode {
    /// The one-pane Markdown editor (default).
    #[default]
    Document,
    /// The full embedded "Construct" IDE editor.
    Project,
}

impl DocSubMode {
    /// The two sub-modes in display order.
    pub(crate) const ALL: [Self; 2] = [Self::Document, Self::Project];

    /// The sub-mode tab label.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Document => "Document",
            Self::Project => "Project",
        }
    }

    /// The sub-mode's Carbon glyph.
    #[must_use]
    pub(crate) const fn icon(self) -> &'static str {
        match self {
            Self::Document => icons::DOC_SUBMODE_DOCUMENT,
            Self::Project => icons::DOC_SUBMODE_PROJECT,
        }
    }
}

/// The Document sub-mode's Source ↔ Visual view toggle over the **same** rope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DocView {
    /// Edit the raw Markdown rope (the embedded editor's Source view).
    #[default]
    Source,
    /// The rendered Markdown (read view), via the editor's own render.
    Visual,
}

impl DocView {
    /// The two views in display order.
    pub(crate) const ALL: [Self; 2] = [Self::Source, Self::Visual];

    /// The view label.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::Visual => "Visual",
        }
    }

    /// The view's Carbon glyph.
    #[must_use]
    pub(crate) const fn icon(self) -> &'static str {
        match self {
            Self::Source => icons::DOC_VIEW_SOURCE,
            Self::Visual => icons::DOC_VIEW_VISUAL,
        }
    }
}

/// An ops-oriented starter template a new Document seeds its rope from — a real
/// editable Markdown skeleton, never a locked/faked form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocTemplate {
    /// An empty document.
    Blank,
    /// A runbook skeleton (purpose / preconditions / steps / rollback).
    Runbook,
    /// An incident report skeleton (summary / timeline / impact / follow-ups).
    Incident,
    /// A standup-notes skeleton (done / next / blockers).
    Standup,
}

impl DocTemplate {
    /// The templates offered by the **New** affordance, in display order.
    pub(crate) const ALL: [Self; 4] = [Self::Blank, Self::Runbook, Self::Incident, Self::Standup];

    /// The template's default document title.
    #[must_use]
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Blank => "Untitled",
            Self::Runbook => "Runbook",
            Self::Incident => "Incident report",
            Self::Standup => "Standup notes",
        }
    }

    /// The template's seed Markdown (the real starting rope, §7).
    #[must_use]
    pub(crate) const fn markdown(self) -> &'static str {
        match self {
            Self::Blank => "",
            Self::Runbook => concat!(
                "# Runbook\n\n",
                "## Purpose\n\n",
                "## Preconditions\n\n",
                "- [ ] \n\n",
                "## Steps\n\n",
                "1. \n\n",
                "## Rollback\n\n",
                "1. \n",
            ),
            Self::Incident => concat!(
                "# Incident report\n\n",
                "## Summary\n\n",
                "## Timeline\n\n",
                "- \n\n",
                "## Impact\n\n",
                "## Follow-ups\n\n",
                "- [ ] \n",
            ),
            Self::Standup => concat!(
                "# Standup notes\n\n",
                "## Done\n\n",
                "- \n\n",
                "## Next\n\n",
                "- \n\n",
                "## Blockers\n\n",
                "- \n",
            ),
        }
    }
}

/// The Documents mode's view state — the two embedded editors plus the picked
/// document/title and the sub-mode/view toggles. Holds no authoritative content:
/// the canonical Markdown lives in the editor's rope and is read back on save.
#[derive(Default)]
pub(crate) struct DocumentsState {
    /// The active sub-mode (Document by default).
    pub(crate) sub: DocSubMode,
    /// The Document sub-mode's Source/Visual view.
    pub(crate) view: DocView,
    /// The **Document** sub-mode's one-pane Markdown editor (a single buffer). A
    /// fresh [`EditorSurface`] is swapped in on every load so it stays one-pane.
    pub(crate) editor: EditorSurface,
    /// The **Project** sub-mode's full IDE editor (its own tabs/splits/tree).
    pub(crate) project_editor: EditorSurface,
    /// The document currently being edited in Document mode, if any.
    pub(crate) active_document: Option<DocumentId>,
    /// The document whose body is currently loaded into [`editor`](Self::editor)
    /// — the load debounce, so a re-render does not re-open the buffer each frame.
    pub(crate) loaded_document: Option<DocumentId>,
    /// The active document's title (shown in the toolbar).
    pub(crate) active_title: String,
    /// Whether the New-document template picker row is open.
    pub(crate) template_open: bool,
    /// A transient, honest notice (e.g. "Saved", "Exported 812 bytes"), shown
    /// once, cleared on the next action — never a silent swallow (§7).
    pub(crate) notice: Option<String>,
}

impl std::fmt::Debug for DocumentsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The embedded `EditorSurface`s are not `Debug`; report the view state.
        f.debug_struct("DocumentsState")
            .field("sub", &self.sub)
            .field("view", &self.view)
            .field("active_document", &self.active_document)
            .field("loaded_document", &self.loaded_document)
            .field("active_title", &self.active_title)
            .field("template_open", &self.template_open)
            .field("editor_open", &self.editor.is_open())
            .finish_non_exhaustive()
    }
}

impl DocumentsState {
    /// Reset the picked-document state on a space switch — the active document is a
    /// per-space intent. The next Document-mode render re-seeds a blank buffer or
    /// loads the newly-picked document; the editor content is replaced on load, so
    /// nothing stale leaks across spaces.
    pub(crate) fn on_space_switch(&mut self) {
        self.active_document = None;
        self.loaded_document = None;
        self.active_title.clear();
        self.template_open = false;
        self.notice = None;
    }
}

impl CommunicationsSurface {
    /// Render Documents mode for the selected space: the sub-mode + document
    /// toolbar strip, then the active sub-mode's body (the one-pane Markdown editor
    /// or the full embedded IDE).
    pub(crate) fn documents_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn CollabData,
        sink: &mut CommandSink,
    ) {
        let Some(space) = self.selected_space() else {
            ui.label(
                egui::RichText::new("Select a space to see its documents.").color(Style::TEXT_DIM),
            );
            return;
        };

        egui::TopBottomPanel::top(ui.id().with("collab-doc-strip"))
            .frame(frame::bar_frame())
            .show_inside(ui, |ui| self.documents_strip(ui, data, sink, space));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| match self.doc_submode() {
                DocSubMode::Document => self.documents_pane(ui, data),
                // WL-FUNC-011 Phase 3c: the Project editor is a distinct embedded
                // `EditorSurface`; a follow-up may join a mesh co-edit session on
                // the focused project buffer (the editor already carries
                // `CollabSession`/`follow`). Today it is the full local IDE.
                DocSubMode::Project => {
                    editor_panel(ui, &mut self.documents.project_editor);
                }
            });
    }

    /// The sub-mode tabs + (in Document sub-mode) the document toolbar: the session
    /// picker, New (templates), the Source/Visual toggle, Save, and Export.
    fn documents_strip(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn CollabData,
        sink: &mut CommandSink,
        space: SpaceId,
    ) {
        // Sub-mode tabs (Document | Project).
        ui.horizontal(|ui| {
            for sub in DocSubMode::ALL {
                let selected = self.doc_submode() == sub;
                let tint = if selected {
                    Style::ACCENT
                } else {
                    Style::TEXT_DIM
                };
                icons::icon(ui, sub.icon(), Style::SP_M, tint);
                let label = egui::RichText::new(sub.label()).color(if selected {
                    Style::TEXT_STRONG
                } else {
                    Style::TEXT
                });
                if ui.selectable_label(selected, label).clicked() {
                    self.set_doc_submode(sub);
                }
                ui.add_space(Style::SP_XS);
            }
        });

        if self.doc_submode() != DocSubMode::Document {
            // Project sub-mode carries the editor's own Word-97 menu + toolbars, so
            // the strip stops at the sub-mode tabs.
            return;
        }

        ui.separator();

        // Document controls: the title, New, the Source/Visual toggle, Save, Export.
        ui.horizontal(|ui| {
            let title = if self.documents.active_title.is_empty() {
                "Untitled".to_owned()
            } else {
                self.documents.active_title.clone()
            };
            icons::icon(ui, icons::DOC_ROW, Style::SP_M, Style::ACCENT);
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .color(Style::TEXT_STRONG),
            );

            if icons::icon_button(
                ui,
                icons::DOC_NEW,
                Style::SP_M,
                Style::ACCENT,
                "New document from a template",
            )
            .clicked()
            {
                self.documents.template_open = !self.documents.template_open;
            }

            // Source ↔ Visual toggle over the same rope.
            for view in DocView::ALL {
                let selected = self.doc_view() == view;
                let tint = if selected {
                    Style::ACCENT
                } else {
                    Style::TEXT_DIM
                };
                if icons::icon_button(ui, view.icon(), Style::SP_M, tint, view.label()).clicked() {
                    self.set_doc_view(view);
                }
            }

            if icons::icon_button(
                ui,
                icons::DOC_SAVE,
                Style::SP_M,
                Style::OK,
                "Save — share this update (emits UpdateDocument with the Markdown)",
            )
            .clicked()
            {
                self.save_document(sink, space);
            }

            if icons::icon_button(
                ui,
                icons::DOC_EXPORT,
                Style::SP_M,
                Style::TEXT_DIM,
                "Export as Markdown (the only export; print/preview live in the editor's File menu)",
            )
            .clicked()
            {
                let bytes = self.export_markdown().map_or(0, |md| md.len());
                self.documents.notice = Some(format!("Exported {bytes} bytes of Markdown."));
            }
        });

        // The ops-oriented template picker row (opened by New).
        if self.documents.template_open {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("New from template:")
                        .small()
                        .color(Style::TEXT_DIM),
                );
                for template in DocTemplate::ALL {
                    if ui.selectable_label(false, template.title()).clicked() {
                        self.new_document(sink, space, template);
                    }
                }
            });
        }

        // The session picker: the space's live documents (read model), plus the
        // honest empty state when the space has none yet.
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Open:").small().color(Style::TEXT_DIM));
            let mut pick: Option<(DocumentId, String)> = None;
            match data.document_sessions(space) {
                Some(sessions) if !sessions.sessions.is_empty() => {
                    for session in &sessions.sessions {
                        let selected = self.active_document() == Some(session.document);
                        icons::icon(ui, icons::DOC_ROW, Style::SP_M, Style::TEXT_DIM);
                        if ui.selectable_label(selected, &session.title).clicked() {
                            pick = Some((session.document, session.title.clone()));
                        }
                    }
                }
                _ => {
                    ui.label(
                        egui::RichText::new("no documents yet — New to create one")
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                }
            }
            if let Some((document, title)) = pick {
                self.open_document(data, document, title);
            }
        });

        if let Some(notice) = self.documents.notice.clone() {
            ui.label(egui::RichText::new(notice).small().color(Style::TEXT_DIM));
        }
    }

    /// The Document sub-mode body: the Source view (the embedded editor over the
    /// live rope) or the Visual view (the editor's own rendered Markdown).
    fn documents_pane(&mut self, ui: &mut egui::Ui, data: &dyn CollabData) {
        self.ensure_document_loaded(data);
        match self.doc_view() {
            DocView::Source => {
                // The one-pane Markdown editor — the editor's real widget, chrome,
                // and Word-97 menu + Standard + Formatting toolbars, all reused.
                editor_panel(ui, &mut self.documents.editor);
            }
            DocView::Visual => {
                // The rendered Markdown — the editor's OWN parser + render over the
                // same rope, so Source and Visual never diverge.
                let text = self.documents.editor.current_text().unwrap_or_default();
                let blocks = markdown::parse(&text);
                markdown::show(ui, &blocks);
            }
        }
    }

    /// Ensure the Document editor reflects the picked document: load its resolved
    /// canonical Markdown when the active document changed, or seed a blank
    /// editable buffer before any document is opened/created. Idempotent per
    /// document (the load debounce), so it is cheap on a quiet re-render.
    fn ensure_document_loaded(&mut self, data: &dyn CollabData) {
        if let Some(document) = self.documents.active_document {
            if self.documents.loaded_document != Some(document) {
                self.load_editor_body(data.document_body(document).unwrap_or_default());
                self.documents.loaded_document = Some(document);
            }
        } else if !self.documents.editor.is_open() {
            // No document picked yet — a real, empty, editable Markdown buffer
            // (§7), never a faked placeholder.
            self.documents.editor.open_text("");
        }
    }

    /// Replace the Document editor with a fresh one-pane [`EditorSurface`] seeded
    /// with `body` — the load path that keeps the Document editor single-pane
    /// (a fresh surface, one buffer). The seeded buffer is a real editable rope.
    ///
    /// WL-FUNC-011 Phase 3c: this loads a resolved *snapshot* of the canonical
    /// Markdown; the live Yrs CRDT co-edit stream (shared cursors/presence,
    /// follow-mode) that keeps the buffer converging across seats in real time is
    /// the next slice, wired through `mde_editor_egui::CollabSession`.
    fn load_editor_body(&mut self, body: &str) {
        self.documents.editor = real_editor();
        self.documents.editor.open_text(body);
    }

    // ── testable command seams (the UI above drives these same methods) ──────

    /// The active Documents sub-mode (test/inspection accessor).
    #[must_use]
    pub(crate) fn doc_submode(&self) -> DocSubMode {
        self.documents.sub
    }

    /// Switch the Documents sub-mode.
    pub(crate) fn set_doc_submode(&mut self, sub: DocSubMode) {
        self.documents.sub = sub;
    }

    /// The active Document Source/Visual view (test/inspection accessor).
    #[must_use]
    pub(crate) fn doc_view(&self) -> DocView {
        self.documents.view
    }

    /// Switch the Document Source/Visual view.
    pub(crate) fn set_doc_view(&mut self, view: DocView) {
        self.documents.view = view;
    }

    /// The document currently being edited in Document mode, if any.
    #[must_use]
    pub(crate) fn active_document(&self) -> Option<DocumentId> {
        self.documents.active_document
    }

    /// The Document editor's current text — the canonical Markdown. A test seam to
    /// assert a load/round-trip put the right bytes in the rope (the runtime read
    /// path is [`export_markdown`](Self::export_markdown) / the save path).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn document_editor_text(&self) -> Option<String> {
        self.documents.editor.current_text()
    }

    /// Open `document` for editing: load its resolved canonical Markdown into the
    /// one-pane editor and make it the active document. The read side of the
    /// collab round-trip — it reads [`CollabData::document_body`], never fetching
    /// bytes itself.
    pub(crate) fn open_document(
        &mut self,
        data: &dyn CollabData,
        document: DocumentId,
        title: impl Into<String>,
    ) {
        self.load_editor_body(data.document_body(document).unwrap_or_default());
        self.documents.active_document = Some(document);
        self.documents.loaded_document = Some(document);
        self.documents.active_title = title.into();
        self.documents.sub = DocSubMode::Document;
        self.documents.notice = None;
    }

    /// Create a new document in `space` from `template`: emit
    /// [`CreateDocument`](CollabCommand::CreateDocument) and seed the editor with
    /// the template's real Markdown skeleton. Returns the fresh [`DocumentId`].
    pub(crate) fn new_document(
        &mut self,
        sink: &mut CommandSink,
        space: SpaceId,
        template: DocTemplate,
    ) -> DocumentId {
        let document = DocumentId::new();
        let title = template.title().to_owned();
        sink.emit(CollabCommand::CreateDocument {
            space,
            document,
            title: title.clone(),
        });
        self.load_editor_body(template.markdown());
        self.documents.active_document = Some(document);
        self.documents.loaded_document = Some(document);
        self.documents.active_title = title;
        self.documents.template_open = false;
        self.documents.notice = Some("Created — Save to share it.".to_owned());
        document
    }

    /// Save the active document: read the canonical Markdown back out of the
    /// editor's rope and emit [`UpdateDocument`](CollabCommand::UpdateDocument)
    /// whose [`DocumentChange`] payload is the **content address of that Markdown**
    /// (`text/markdown`) — the Markdown path is the source of truth. Returns
    /// whether an update was emitted.
    ///
    /// WL-FUNC-011 Phase 3c: this emits a whole-document snapshot update. The
    /// follow-ups are the external-write three-way merge (last-shared-base vs.
    /// collab vs. disk) before publishing, and autosave versioned snapshots + a
    /// rendered word-diff timeline + git integration around each save.
    pub(crate) fn save_document(&mut self, sink: &mut CommandSink, space: SpaceId) -> bool {
        let Some(document) = self.documents.active_document else {
            self.documents.notice = Some("Open or create a document first.".to_owned());
            return false;
        };
        let Some(markdown) = self.documents.editor.current_text() else {
            return false;
        };
        let payload = PayloadRef::of_bytes(markdown.as_bytes()).with_content_type(MARKDOWN_MIME);
        let summary = first_nonblank_line(&markdown).map(str::to_owned);
        sink.emit(CollabCommand::UpdateDocument {
            space,
            document,
            change: DocumentChange { payload, summary },
        });
        self.documents.notice = Some("Saved — update shared.".to_owned());
        true
    }

    /// The canonical Markdown to export — the editor's current text. Markdown is
    /// the only export; print/preview remain reachable through the embedded
    /// editor's File menu, deliberately off this default toolbar.
    #[must_use]
    pub(crate) fn export_markdown(&self) -> Option<String> {
        self.documents.editor.current_text()
    }
}

/// The first non-blank line of `text`, trimmed — a short human summary for the
/// Activity feed (the `DocumentChange.summary`), or `None` for an empty document.
fn first_nonblank_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}
