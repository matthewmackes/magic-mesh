//! Documents mode — the biggest parity mode, built by **reusing** the whole
//! `mde-editor-egui` "Construct" editor rather than re-implementing one.
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
//!   the real widget is mounted. Live mesh share-sessions attach to the Document
//!   sub-mode's one-pane buffer.
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
//! # Mesh share-session (WL-FUNC-031)
//!
//! A **Share** control on the focused document starts a [`CollabSession`] into a
//! chosen member space. Peers join from the space's live-session picker, the
//! participant roster offers a follow-mode toggle (wired through the editor's
//! existing `follow` / [`follow_banner`] APIs), and the owner can close the
//! session — which detaches every follower. Non-members and closed sessions
//! refuse honestly. CollabCommand has no share/join/follow/close variants, so
//! this UI crate emits a local [`DocumentShareCommand`] through
//! [`DocumentShareSink`].
//!
//! External file changes merge against the **last shared base** instead of
//! overwriting the live CRDT buffer; a concurrent write that cannot merge
//! surfaces a typed [`ExternalWriteConflict`] and never drops the in-flight edit.
//!
//! Remaining follow-ups (not faked): the portable review sidecar (anchored
//! comments) and autosave versioned snapshots + a rendered word-diff timeline.

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    CollabCommand, DocumentChange, DocumentId, DocumentSession, PayloadRef, ReviewVerdict, SpaceId,
};
use mde_editor_egui::{
    editor_panel, follow_banner, markdown, real_editor, BusTransport, CollabSession,
    CollabTransport, EditorSurface, FakeBus, FollowUpdate, Role, SessionId,
};

use crate::{frame, icons, CollabData, CommandSink, CommunicationsSurface};

/// The content-type the Documents mode stamps on its canonical export/update
/// payload — Markdown is the source of truth, so an `UpdateDocument` change always
/// names `text/markdown` bytes.
pub(crate) const MARKDOWN_MIME: &str = "text/markdown";

/// Presentation limits for values that can arrive from another seat. These are
/// display/read-boundary limits; the editor and Markdown export keep the complete
/// canonical document untouched.
const MAX_DOCUMENT_TITLE_CHARS: usize = 96;
const MAX_DOCUMENT_SUMMARY_CHARS: usize = 160;
const MAX_DOCUMENT_PREVIEW_CHARS: usize = 64 * 1024;
const MAX_REVIEW_COMMENT_CHARS: usize = 4096;

/// Local share-session intent. [`CollabCommand`] has no share / join / follow /
/// close variants, so Documents mode records these here for the mount to drain
/// the same way it drains [`CommandSink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentShareCommand {
    /// Start hosting a share session for `document` in `space`.
    Start {
        /// Space the session is shared into.
        space: SpaceId,
        /// Document being shared.
        document: DocumentId,
        /// Mesh collab session id (the Bus topic segment).
        session: String,
    },
    /// Join an existing live share session as a guest.
    Join {
        /// Space whose live-session picker listed the session.
        space: SpaceId,
        /// Document to join.
        document: DocumentId,
        /// Mesh collab session id.
        session: String,
    },
    /// Follow `peer` in the live session (view tracks their caret/viewport).
    Follow {
        /// Document whose share session is being followed.
        document: DocumentId,
        /// Peer identity to follow.
        peer: String,
    },
    /// Stop following in the live session.
    Unfollow {
        /// Document whose follow is being cleared.
        document: DocumentId,
    },
    /// Owner closes the session; every follower must detach.
    Close {
        /// Space the session was shared into.
        space: SpaceId,
        /// Document whose session is closing.
        document: DocumentId,
        /// Mesh collab session id.
        session: String,
    },
}

/// Sink Documents mode pushes [`DocumentShareCommand`]s into.
#[derive(Debug, Default, Clone)]
pub struct DocumentShareSink {
    queued: Vec<DocumentShareCommand>,
}

impl DocumentShareSink {
    /// A fresh, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `command` as intent.
    pub fn emit(&mut self, command: DocumentShareCommand) {
        self.queued.push(command);
    }

    /// Take every queued command, leaving the sink empty.
    #[must_use = "the drained share commands must be routed by the caller"]
    pub fn drain(&mut self) -> Vec<DocumentShareCommand> {
        std::mem::take(&mut self.queued)
    }

    /// The queued commands without draining.
    #[must_use]
    pub fn queued(&self) -> &[DocumentShareCommand] {
        &self.queued
    }
}

/// An external write that could not be merged into the live buffer without
/// dropping an in-flight edit. The live rope is left untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalWriteConflict {
    /// Last shared / loaded snapshot the merge used as the ancestor.
    pub base: String,
    /// The live editor (or CRDT) text that would have been lost by a clobber.
    pub local: String,
    /// The incoming external snapshot.
    pub remote: String,
}

/// How an external write was reconciled against the last shared base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalWriteMerge {
    /// The three sides agreed on `0` — use this text.
    Clean(String),
    /// Concurrent edits overlap; keep the live buffer and surface this.
    Conflict(ExternalWriteConflict),
}

/// Transport a Documents-mode share session rides. Production uses the editor's
/// Bus-backed transport; tests inject a shared [`FakeBus`].
enum DocumentShareTransport {
    /// Live Mackes Bus (`collab/session/<id>`).
    Bus(BusTransport),
    /// In-process bus so two [`CommunicationsSurface`]s can co-edit in tests.
    Fake(FakeBus),
}

impl Default for DocumentShareTransport {
    fn default() -> Self {
        Self::Bus(BusTransport::from_env())
    }
}

impl CollabTransport for DocumentShareTransport {
    fn publish(&self, topic: &str, body: &str) {
        match self {
            Self::Bus(transport) => transport.publish(topic, body),
            Self::Fake(transport) => transport.publish(topic, body),
        }
    }

    fn poll(&self, topic: &str, cursor: &mut Option<String>) -> Vec<String> {
        match self {
            Self::Bus(transport) => transport.poll(topic, cursor),
            Self::Fake(transport) => transport.poll(topic, cursor),
        }
    }

    fn tail(&self, topic: &str) -> Option<String> {
        match self {
            Self::Bus(transport) => transport.tail(topic),
            Self::Fake(transport) => transport.tail(topic),
        }
    }
}

/// Opaque `CollabMessage` JSON: a host `Leave` is `{"from":"<host>","kind":{"t":"leave"},…}`.
/// Parsed here as text so this crate does not take a second protocol store or a
/// new `serde_json` edge — the live [`CollabSession`] remains the only decoder.
fn frame_is_from_host(body: &str, host: &str) -> bool {
    !host.is_empty() && body.contains(&format!(r#""from":"{host}""#))
}

fn frame_is_leave(body: &str) -> bool {
    body.contains(r#""t":"leave""#)
}

/// Watches the same transport the live session polls and notes a host `Leave`
/// even when that peer was never added to the CRDT roster. `join` tails past
/// history, so an immediate owner-close must still detach every follower.
struct LeaveWatch<'a> {
    inner: &'a DocumentShareTransport,
    host: String,
    left: std::cell::Cell<bool>,
}

impl CollabTransport for LeaveWatch<'_> {
    fn publish(&self, topic: &str, body: &str) {
        self.inner.publish(topic, body);
    }

    fn poll(&self, topic: &str, cursor: &mut Option<String>) -> Vec<String> {
        let bodies = self.inner.poll(topic, cursor);
        if !self.host.is_empty() {
            for body in &bodies {
                if frame_is_from_host(body, &self.host) && frame_is_leave(body) {
                    self.left.set(true);
                }
            }
        }
        bodies
    }

    fn tail(&self, topic: &str) -> Option<String> {
        self.inner.tail(topic)
    }
}

/// Whether `host` has already published `Leave` on this session topic.
///
/// Uses an independent cursor so it does not steal frames from the live
/// [`CollabSession`]. A later join against a stale session row must still
/// refuse after owner-close.
fn host_has_left_on_wire(
    transport: &DocumentShareTransport,
    session: &SessionId,
    host: &str,
) -> bool {
    if host.is_empty() {
        return false;
    }
    let mut cursor = None;
    let mut left = false;
    for body in transport.poll(&session.topic(), &mut cursor) {
        if frame_is_from_host(&body, host) {
            left = frame_is_leave(&body);
        }
    }
    left
}

/// One locally attached mesh share-session (host or guest).
struct LiveShare {
    /// Space the session was started or joined in.
    space: SpaceId,
    /// Document being co-edited.
    document: DocumentId,
    /// The editor crate's live CRDT session.
    session: CollabSession,
    /// Whether this seat hosted the session (owner-close authority).
    owner: bool,
    /// Session-row owner (first participant that is not this seat). May not be
    /// in the CRDT roster yet; do not treat this as a Leave until observed.
    expected_host: String,
    /// Host peer identity once seen in the roster — guests detach on its Leave.
    host_peer: String,
}

/// Derive the mesh [`SessionId`] for one document *in one space*.
///
/// A document can be linked into multiple spaces. Scoping the Bus topic with
/// both ids prevents edits in one space from leaking into another share
/// session while keeping the id deterministic for a picker join.
fn session_id_for(space: SpaceId, document: DocumentId) -> Option<SessionId> {
    SessionId::new(format!("{}-{}", space.as_uuid(), document.as_uuid())).ok()
}

/// Whether `space` is in the seat's directory (membership).
fn is_space_member(data: &dyn CollabData, space: SpaceId) -> bool {
    data.space_directory()
        .spaces
        .iter()
        .any(|summary| summary.id == space)
}

/// The live document session row for `document` in `space`, if the projection
/// still lists it (a missing row is a closed session).
fn live_document_session<'a>(
    data: &'a dyn CollabData,
    space: SpaceId,
    document: DocumentId,
) -> Option<&'a DocumentSession> {
    data.document_sessions(space)?
        .sessions
        .iter()
        .find(|session| session.document == document)
}

/// The share owner advertised by the live session row.
///
/// The first participant that is not this seat is the session starter (the
/// mount lists the owner first when it records `Start`). Guests pin that
/// identity so an owner `Leave` detaches every follower even when another
/// guest's id sorts first in the CRDT roster.
fn host_peer_from_session(data: &dyn CollabData, space: SpaceId, document: DocumentId) -> String {
    live_document_session(data, space, document)
        .and_then(|session| {
            session
                .participants
                .iter()
                .map(|actor| actor.as_str())
                .find(|peer| *peer != data.me().as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// Characters that can change the visual direction or structure of a Documents
/// label. They are replaced before text reaches egui's shaper; ordinary
/// whitespace is handled separately so every presentation remains one line.
fn is_document_format_control(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
        )
}

/// Make untrusted document metadata safe for a bounded, single-line
/// presentation. The full source value remains in the read model; this helper
/// only produces the copy sent to egui labels and the bounded activity summary.
fn bounded_document_display(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut chars = value.chars().map(|character| {
        if character.is_whitespace() {
            ' '
        } else if is_document_format_control(character) {
            '\u{fffd}'
        } else {
            character
        }
    });
    let mut bounded: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_none() {
        return bounded;
    }

    // Leave room for a visible truncation marker without ever splitting UTF-8.
    let keep_bytes = bounded
        .char_indices()
        .nth(max_chars.saturating_sub(1))
        .map_or(0, |(byte, _)| byte);
    bounded.truncate(keep_bytes);
    bounded.push('\u{2026}');
    bounded
}

/// Cap a local review buffer before egui measures it. `TextEdit::char_limit`
/// handles new keystrokes/pastes; this in-place guard also protects the first
/// frame when a caller or restored state seeded an oversized value.
fn cap_review_comment(value: &mut String) {
    if let Some((byte, _)) = value.char_indices().nth(MAX_REVIEW_COMMENT_CHARS) {
        value.truncate(byte);
    }
}

/// Return a UTF-8-safe prefix for the Visual Markdown view. Source, save, and
/// export continue to use the complete editor rope; only the potentially
/// expensive preview parser receives this bounded presentation excerpt.
fn markdown_preview(text: &str) -> (&str, bool) {
    text.char_indices()
        .nth(MAX_DOCUMENT_PREVIEW_CHARS)
        .map_or((text, false), |(byte, _)| (&text[..byte], true))
}

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
    /// Optional review comment, kept local until the seat explicitly submits a
    /// verdict. The durable review event carries the bounded snapshot.
    pub(crate) review_comment: String,
    /// Whether the Share space-picker row is open.
    share_picker_open: bool,
    /// Last snapshot this seat loaded or saved — the ancestor for external-write
    /// three-way merge.
    last_shared_base: Option<String>,
    /// Last `document_body` this seat already merged or loaded, so a quiet frame
    /// does not re-merge the same external snapshot.
    last_seen_external: Option<String>,
    /// A typed external-write conflict, if a concurrent disk/collab write could
    /// not merge. The live rope is left as-is.
    external_conflict: Option<ExternalWriteConflict>,
    /// Local share-session commands (CollabCommand has no share/join/follow/close).
    share_commands: DocumentShareSink,
    /// In-process or Bus transport the live [`CollabSession`] rides.
    share_transport: DocumentShareTransport,
    /// Attached mesh share-session, if this seat is hosting or has joined.
    share: Option<LiveShare>,
}

impl std::fmt::Debug for DocumentsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The embedded `EditorSurface`s and `CollabSession` are not `Debug`.
        f.debug_struct("DocumentsState")
            .field("sub", &self.sub)
            .field("view", &self.view)
            .field("active_document", &self.active_document)
            .field("loaded_document", &self.loaded_document)
            .field("active_title", &self.active_title)
            .field("template_open", &self.template_open)
            .field("share_picker_open", &self.share_picker_open)
            .field("review_comment_len", &self.review_comment.len())
            .field("editor_open", &self.editor.is_open())
            .field("share_attached", &self.share.is_some())
            .field("external_conflict", &self.external_conflict.is_some())
            .finish_non_exhaustive()
    }
}

impl DocumentsState {
    /// Drain share-session intents for the shell mount.
    pub(crate) fn drain_share_commands(&mut self) -> Vec<DocumentShareCommand> {
        self.share_commands.drain()
    }

    /// Prepare both embedded editor routes for a direct Editor launch. The
    /// Project editor and the one-pane document editor retain their documents,
    /// but optional sidebars never steal the initial canvas width.
    pub(crate) fn prepare_direct_entry(&mut self) {
        self.editor.collapse_sidebars();
        self.project_editor.collapse_sidebars();
    }

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
        self.review_comment.clear();
        self.share_picker_open = false;
        self.last_shared_base = None;
        self.last_seen_external = None;
        self.external_conflict = None;
        self.detach_share("Left the previous space's share session.");
        // The previous space's editor is a loaded view, not durable state. Drop
        // it with the per-space selection so the no-selection path cannot keep
        // rendering the old space's Markdown while the new session projection
        // is still arriving (or has disappeared after membership loss).
        self.editor = real_editor();
    }

    /// Drop the live share-session, announcing Leave so followers detach.
    fn detach_share(&mut self, notice: &str) {
        if let Some(live) = self.share.take() {
            live.session.leave(&self.share_transport);
            if !notice.is_empty() {
                self.notice = Some(notice.to_owned());
            }
        }
    }

    /// Record the snapshot that future external writes merge against.
    fn remember_shared_base(&mut self, body: &str) {
        self.last_shared_base = Some(body.to_owned());
        self.last_seen_external = Some(body.to_owned());
        self.external_conflict = None;
    }

    /// Pump the attached share-session against the Document editor.
    fn pump_share(&mut self) -> SharePumpOutcome {
        let DocumentsState {
            share,
            share_transport,
            editor,
            ..
        } = self;
        let Some(live) = share.as_mut() else {
            return SharePumpOutcome::default();
        };
        if let Some(text) = editor.current_text() {
            mirror_local_text_into_session(&mut live.session, &text);
        }
        live.session.set_cursor(editor.current_cursor());
        live.session.set_viewport(editor.current_viewport());
        live.session.flush(share_transport);
        live.session.publish_presence(share_transport);
        let watch_host = if !live.expected_host.is_empty() {
            live.expected_host.clone()
        } else {
            live.host_peer.clone()
        };
        let watcher = LeaveWatch {
            inner: share_transport,
            host: watch_host,
            left: std::cell::Cell::new(false),
        };
        let outcome = live.session.poll(&watcher);
        let host_left_on_wire = watcher.left.get();
        // Pin the host only after they appear in the roster. Setting
        // `host_peer` from the session row at join time made the first pump
        // treat a not-yet-visible owner as already gone.
        if live.host_peer.is_empty() {
            if !live.expected_host.is_empty()
                && live.session.peers().contains_key(&live.expected_host)
            {
                live.host_peer = live.expected_host.clone();
            } else if live.expected_host.is_empty() && live.session.peers().len() == 1 {
                if let Some(peer) = live.session.peers().keys().next() {
                    live.host_peer = peer.clone();
                }
            }
        }
        let host_left = !live.owner
            && ((host_left_on_wire && !live.expected_host.is_empty())
                || (!live.host_peer.is_empty()
                    && !live.session.peers().contains_key(&live.host_peer)));
        SharePumpOutcome {
            crdt_text: Some(live.session.doc().to_text()),
            follow: outcome.follow,
            host_left,
            follow_ended: outcome.follow_ended,
        }
    }

    /// Apply merged text to the live CRDT without clobbering a different document.
    fn mirror_merged_into_share(&mut self, document: DocumentId, text: &str) {
        let DocumentsState {
            share,
            share_transport,
            ..
        } = self;
        if let Some(live) = share.as_mut() {
            if live.document == document {
                mirror_local_text_into_session(&mut live.session, text);
                live.session.flush(share_transport);
            }
        }
    }
}

/// Result of one share-session pump, applied to the editor after the session
/// borrow ends.
#[derive(Default)]
struct SharePumpOutcome {
    crdt_text: Option<String>,
    follow: Option<FollowUpdate>,
    host_left: bool,
    follow_ended: bool,
}

impl CommunicationsSurface {
    /// Drain Documents share-session intents for the owning shell mount.
    ///
    /// These lifecycle intents are distinct from [`CommandSink`], because
    /// `CollabCommand` has no share-session variants. The mount drains this
    /// seam once per frame and routes the typed intents through its Bus lane,
    /// paralleling the shape of [`drain_sync_pair_commands`],
    /// [`drain_voice_admin_commands`], and [`drain_gateway_commands`].
    ///
    /// [`drain_sync_pair_commands`]: Self::drain_sync_pair_commands
    /// [`drain_voice_admin_commands`]: Self::drain_voice_admin_commands
    /// [`drain_gateway_commands`]: Self::drain_gateway_commands
    #[must_use]
    pub fn drain_document_share_commands(&mut self) -> Vec<DocumentShareCommand> {
        self.documents.drain_share_commands()
    }

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

        self.sync_document_share(data);
        frame::bar_frame().show(ui, |ui| self.documents_strip(ui, data, sink, space));

        egui::Frame::NONE.show(ui, |ui| match self.doc_submode() {
            DocSubMode::Document => self.documents_pane(ui, data),
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
                bounded_document_display(&self.documents.active_title, MAX_DOCUMENT_TITLE_CHARS)
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

            if icons::icon_button(
                ui,
                icons::CLIP_ATTACH,
                Style::SP_M,
                Style::ACCENT,
                "Share this document into a space",
            )
            .clicked()
            {
                self.documents.share_picker_open = !self.documents.share_picker_open;
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

        if self.documents.share_picker_open {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("Share into space:")
                        .small()
                        .color(Style::TEXT_DIM),
                );
                let mut chosen: Option<SpaceId> = None;
                for summary in &data.space_directory().spaces {
                    let label = bounded_document_display(&summary.name, MAX_DOCUMENT_TITLE_CHARS);
                    if ui.selectable_label(summary.id == space, &label).clicked() {
                        chosen = Some(summary.id);
                    }
                }
                if data.space_directory().spaces.is_empty() {
                    ui.label(
                        egui::RichText::new("no member spaces to share into")
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                }
                if let Some(target) = chosen {
                    let _ = self.share_document(data, target);
                    self.documents.share_picker_open = false;
                }
            });
        }

        // The session picker: the space's live documents (read model), plus the
        // honest empty state when the space has none yet. Clicking a live session
        // opens it and joins the mesh share-session when this seat is a member.
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Open:").small().color(Style::TEXT_DIM));
            let mut pick: Option<(DocumentId, String)> = None;
            match data.document_sessions(space) {
                Some(sessions) if !sessions.sessions.is_empty() => {
                    for session in &sessions.sessions {
                        let selected = self.active_document() == Some(session.document);
                        icons::icon(ui, icons::DOC_ROW, Style::SP_M, Style::TEXT_DIM);
                        let title =
                            bounded_document_display(&session.title, MAX_DOCUMENT_TITLE_CHARS);
                        if ui.selectable_label(selected, &title).clicked() {
                            // Title is display-only state, not part of the canonical
                            // Markdown command payload, so keep the click path bounded.
                            pick = Some((session.document, title));
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
                let _ = self.join_document_share(data, space, document);
            }
        });

        self.share_session_strip(ui, data, space);

        // Review actions are explicit commands over the selected document. The
        // current session participants are the honest reviewer candidates; the
        // local seat is excluded and an empty candidate set stays visible as a
        // truthful notice instead of emitting a review request to nobody.
        if self.active_document().is_some() {
            cap_review_comment(&mut self.documents.review_comment);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Review:")
                        .small()
                        .strong()
                        .color(Style::TEXT_STRONG),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.documents.review_comment)
                        .desired_width(180.0)
                        .char_limit(MAX_REVIEW_COMMENT_CHARS)
                        .hint_text("optional comment"),
                );
                if ui.button("Request review").clicked() {
                    self.request_review(data, sink, space);
                }
                if ui.button("Approve").clicked() {
                    self.submit_review(sink, space, ReviewVerdict::Approved);
                }
                if ui.button("Changes").clicked() {
                    self.submit_review(sink, space, ReviewVerdict::ChangesRequested);
                }
                if ui.button("Comment").clicked() {
                    self.submit_review(sink, space, ReviewVerdict::Commented);
                }
            });
        }

        if let Some(notice) = self.documents.notice.clone() {
            ui.label(egui::RichText::new(notice).small().color(Style::TEXT_DIM));
        }
        if self.documents.external_conflict.is_some() {
            ui.label(
                egui::RichText::new(
                    "External write conflict — live edits kept; incoming file was not applied.",
                )
                .small()
                .color(Style::WARN),
            );
        }
    }

    /// Participant roster + follow-mode toggle + owner close for the live share.
    fn share_session_strip(&mut self, ui: &mut egui::Ui, data: &dyn CollabData, _space: SpaceId) {
        self.sync_document_share(data);
        let Some(live) = self.documents.share.as_ref() else {
            return;
        };
        let me = data.me().as_str().to_owned();
        let owner = live.session.role() == Role::Host;
        let following = live.session.following().map(str::to_owned);
        let follow_name = following.as_ref().and_then(|peer| {
            live.session
                .peers()
                .get(peer)
                .map(|remote| remote.presence.name.clone())
                .or_else(|| Some(peer.clone()))
        });
        let mut peers: Vec<(String, String)> = live
            .session
            .peers()
            .iter()
            .map(|(id, remote)| (id.clone(), remote.presence.name.clone()))
            .collect();
        peers.sort_by(|a, b| a.0.cmp(&b.0));
        drop(live);

        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Sharing:")
                    .small()
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new(format!("{me} (you)"))
                    .small()
                    .color(Style::TEXT),
            );
            let mut follow_peer: Option<String> = None;
            let mut unfollow = false;
            for (id, name) in &peers {
                let label = bounded_document_display(name, MAX_DOCUMENT_TITLE_CHARS);
                ui.label(egui::RichText::new(&label).small().color(Style::TEXT));
                let is_following = following.as_deref() == Some(id.as_str());
                let hint = if is_following {
                    "Stop following this participant"
                } else {
                    "Follow this participant"
                };
                let tint = if is_following {
                    Style::ACCENT
                } else {
                    Style::TEXT_DIM
                };
                if icons::icon_button(ui, icons::THREAD, Style::SP_M, tint, hint).clicked() {
                    if is_following {
                        unfollow = true;
                    } else {
                        follow_peer = Some(id.clone());
                    }
                }
            }
            if owner
                && icons::icon_button(
                    ui,
                    icons::CALL_DECLINE,
                    Style::SP_M,
                    Style::DANGER,
                    "Close share session (detaches every follower)",
                )
                .clicked()
            {
                let _ = self.close_document_share();
            }
            if let Some(peer) = follow_peer {
                let _ = self.follow_share_peer(&peer);
            }
            if unfollow {
                let _ = self.unfollow_share_peer();
            }
        });

        if let Some(name) = follow_name {
            if follow_banner(ui, &name) {
                let _ = self.unfollow_share_peer();
            }
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
                let (preview, truncated) = markdown_preview(&text);
                let blocks = markdown::parse(preview);
                markdown::show(ui, &blocks);
                if truncated {
                    ui.label(
                        egui::RichText::new(
                            "Visual preview truncated; Source and Export retain the full Markdown.",
                        )
                        .small()
                        .color(Style::TEXT_DIM),
                    );
                }
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
                let body = data.document_body(document).unwrap_or_default();
                self.load_editor_body(body);
                self.documents.remember_shared_base(body);
                self.documents.loaded_document = Some(document);
            } else {
                self.merge_external_document(data, document);
            }
        } else if !self.documents.editor.is_open() {
            // No document picked yet — a real, empty, editable Markdown buffer
            // (§7), never a faked placeholder.
            self.documents.editor.open_text("");
        }
        self.sync_document_share(data);
    }

    /// Replace the Document editor with a fresh one-pane [`EditorSurface`] seeded
    /// with `body` — the load path that keeps the Document editor single-pane
    /// (a fresh surface, one buffer). The seeded buffer is a real editable rope.
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
        if self
            .documents
            .share
            .as_ref()
            .is_some_and(|live| live.document != document)
        {
            self.documents
                .detach_share("Left the previous document's share session.");
        }
        self.load_editor_body(data.document_body(document).unwrap_or_default());
        self.documents.active_document = Some(document);
        self.documents.loaded_document = Some(document);
        let title = title.into();
        // Keep the title canonical in state; the toolbar applies the bounded
        // presentation transform at the egui boundary below.
        self.documents.active_title = title;
        self.documents.sub = DocSubMode::Document;
        self.documents.review_comment.clear();
        self.documents.notice = None;
        let body = self.documents.editor.current_text().unwrap_or_default();
        self.documents.remember_shared_base(&body);
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
        self.documents.review_comment.clear();
        self.documents.notice = Some("Created — Save to share it.".to_owned());
        self.documents.remember_shared_base(template.markdown());
        document
    }

    /// Save the active document: read the canonical Markdown back out of the
    /// editor's rope and emit [`UpdateDocument`](CollabCommand::UpdateDocument)
    /// whose [`DocumentChange`] payload is the **content address of that Markdown**
    /// (`text/markdown`) — the Markdown path is the source of truth. Returns
    /// whether an update was emitted. External writes merge against this saved
    /// snapshot as the last shared base instead of clobbering the live buffer.
    pub(crate) fn save_document(&mut self, sink: &mut CommandSink, space: SpaceId) -> bool {
        let Some(document) = self.documents.active_document else {
            self.documents.notice = Some("Open or create a document first.".to_owned());
            return false;
        };
        let Some(markdown) = self.documents.editor.current_text() else {
            return false;
        };
        let payload = PayloadRef::of_bytes(markdown.as_bytes()).with_content_type(MARKDOWN_MIME);
        let summary = first_nonblank_line(&markdown);
        sink.emit(CollabCommand::UpdateDocument {
            space,
            document,
            change: DocumentChange { payload, summary },
        });
        self.documents.remember_shared_base(&markdown);
        self.documents.notice = Some("Saved — update shared.".to_owned());
        true
    }

    /// Request review from every current peer in the selected document session.
    /// The peer list comes from the retained read model, is bounded, and excludes
    /// the local seat so a request cannot accidentally target the requester.
    pub(crate) fn request_review(
        &mut self,
        data: &dyn CollabData,
        sink: &mut CommandSink,
        space: SpaceId,
    ) -> bool {
        let Some(document) = self.documents.active_document else {
            self.documents.notice = Some("Open a document before requesting review.".to_owned());
            return false;
        };
        let reviewers = data
            .document_sessions(space)
            .and_then(|sessions| sessions.sessions.iter().find(|s| s.document == document))
            .map(|session| {
                session
                    .participants
                    .iter()
                    .filter(|actor| *actor != data.me())
                    .take(32)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if reviewers.is_empty() {
            self.documents.notice =
                Some("No other document participants are available to review it.".to_owned());
            return false;
        }
        sink.emit(CollabCommand::RequestReview {
            space,
            document,
            reviewers,
        });
        self.documents.notice = Some("Review requested from current document peers.".to_owned());
        true
    }

    /// Submit an explicit review verdict for the selected document. Comments are
    /// copied from the local field and bounded before they cross the command
    /// boundary; an empty comment remains `None`.
    pub(crate) fn submit_review(
        &mut self,
        sink: &mut CommandSink,
        space: SpaceId,
        verdict: ReviewVerdict,
    ) -> bool {
        let Some(document) = self.documents.active_document else {
            self.documents.notice = Some("Open a document before submitting review.".to_owned());
            return false;
        };
        let trimmed = self.documents.review_comment.trim();
        let comment = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.chars().take(MAX_REVIEW_COMMENT_CHARS).collect())
        };
        sink.emit(CollabCommand::SubmitReview {
            space,
            document,
            verdict,
            comment,
        });
        self.documents.notice = Some("Review submitted.".to_owned());
        true
    }

    /// The canonical Markdown to export — the editor's current text. Markdown is
    /// the only export; print/preview remain reachable through the embedded
    /// editor's File menu, deliberately off this default toolbar.
    #[must_use]
    pub(crate) fn export_markdown(&self) -> Option<String> {
        self.documents.editor.current_text()
    }

    /// Inject an in-process [`FakeBus`] so two surfaces can co-edit in tests.
    #[cfg(test)]
    pub(crate) fn bind_document_share_bus(&mut self, bus: FakeBus) {
        self.documents.share_transport = DocumentShareTransport::Fake(bus);
    }

    /// Queued local share-session commands (test/inspection accessor).
    #[must_use]
    pub(crate) fn document_share_commands(&self) -> &[DocumentShareCommand] {
        self.documents.share_commands.queued()
    }

    /// Whether this seat currently has a live share-session attached.
    #[must_use]
    pub(crate) fn has_live_document_share(&self) -> bool {
        self.documents.share.is_some()
    }

    /// Whether this seat hosted the attached share-session.
    #[must_use]
    pub(crate) fn is_document_share_owner(&self) -> bool {
        self.documents.share.as_ref().is_some_and(|live| live.owner)
    }

    /// Peer currently being followed in the live share-session, if any.
    #[must_use]
    pub(crate) fn following_share_peer(&self) -> Option<&str> {
        self.documents
            .share
            .as_ref()
            .and_then(|live| live.session.following())
    }

    /// The live mesh [`CollabSession`] this seat attached, if any.
    ///
    /// Follow / Unfollow / Close already apply on this session when
    /// [`follow_share_peer`], [`unfollow_share_peer`], and
    /// [`close_document_share`] emit. The shell mount drains those
    /// intents with this borrow so Follow / Unfollow / Close hit the
    /// same CRDT, never a second one.
    #[must_use]
    pub fn live_document_share_session(&mut self) -> Option<&mut CollabSession> {
        self.documents.share.as_mut().map(|live| &mut live.session)
    }

    /// Honest Documents-mode notice (share refuse, merge, save), if any.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn document_notice(&self) -> Option<&str> {
        self.documents.notice.as_deref()
    }

    /// Pinned share-session host identity (test/inspection accessor).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn document_share_host_peer(&self) -> Option<&str> {
        self.documents
            .share
            .as_ref()
            .map(|live| live.host_peer.as_str())
    }

    /// Remote peer identities currently in the live share roster.
    #[must_use]
    pub(crate) fn document_share_peers(&self) -> Vec<String> {
        self.documents
            .share
            .as_ref()
            .map(|live| live.session.peers().keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Typed external-write conflict, if a concurrent write could not merge.
    #[must_use]
    pub(crate) fn external_write_conflict(&self) -> Option<&ExternalWriteConflict> {
        self.documents.external_conflict.as_ref()
    }

    /// Last shared / loaded snapshot used as the three-way merge ancestor.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn last_shared_base(&self) -> Option<&str> {
        self.documents.last_shared_base.as_deref()
    }

    /// Test seam: replace the Document editor rope without opening a new tab.
    #[cfg(test)]
    pub(crate) fn set_document_editor_text(&mut self, text: &str) {
        self.documents.editor.replace_text(text);
    }

    /// Test seam: the Document editor's collab caret/selection.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn document_editor_cursor(&self) -> Option<mde_editor_egui::CursorPos> {
        self.documents.editor.current_cursor()
    }

    /// Test seam: place the Document editor caret without typing.
    #[cfg(test)]
    pub(crate) fn set_document_editor_cursor(&mut self, idx: usize) {
        self.documents.editor.place_cursor(idx);
    }

    /// Test seam: run the external-write merge against the current `document_body`.
    #[cfg(test)]
    pub(crate) fn apply_external_document_body(&mut self, data: &dyn CollabData) {
        if let Some(document) = self.documents.active_document {
            self.merge_external_document(data, document);
        }
    }

    /// Start hosting a share session for the focused document into `space`.
    /// Non-members are refused honestly.
    pub(crate) fn share_document(&mut self, data: &dyn CollabData, space: SpaceId) -> bool {
        let Some(document) = self.documents.active_document else {
            self.documents.notice = Some("Open or create a document first.".to_owned());
            return false;
        };
        if !is_space_member(data, space) {
            self.documents.notice =
                Some("Cannot share: this seat is not a member of that space.".to_owned());
            return false;
        }
        let Some(session_id) = session_id_for(space, document) else {
            self.documents.notice =
                Some("Cannot share: document id is not a valid session.".to_owned());
            return false;
        };
        if let Some(live) = &self.documents.share {
            if live.document == document && live.owner {
                self.documents.notice = Some("Already sharing this document.".to_owned());
                return true;
            }
            self.documents.detach_share("");
        }
        let text = self.documents.editor.current_text().unwrap_or_default();
        let mut session = CollabSession::host(session_id.clone(), data.me().as_str(), &text);
        session.join(&self.documents.share_transport);
        let me = data.me().as_str().to_owned();
        self.documents.share = Some(LiveShare {
            space,
            document,
            session,
            owner: true,
            expected_host: me.clone(),
            host_peer: me,
        });
        self.documents
            .share_commands
            .emit(DocumentShareCommand::Start {
                space,
                document,
                session: session_id.to_string(),
            });
        self.documents.notice =
            Some("Sharing — peers can join from this space's session picker.".to_owned());
        true
    }

    /// Join the live share-session for `document` in `space` as a guest. Closed
    /// sessions and non-members refuse honestly.
    pub(crate) fn join_document_share(
        &mut self,
        data: &dyn CollabData,
        space: SpaceId,
        document: DocumentId,
    ) -> bool {
        if !is_space_member(data, space) {
            self.documents.notice =
                Some("Cannot join: this seat is not a member of that space.".to_owned());
            return false;
        }
        if live_document_session(data, space, document).is_none() {
            self.documents.notice = Some("Cannot join: that share session is closed.".to_owned());
            return false;
        }
        let Some(session_id) = session_id_for(space, document) else {
            self.documents.notice =
                Some("Cannot join: document id is not a valid session.".to_owned());
            return false;
        };
        let expected_host = host_peer_from_session(data, space, document);
        if host_has_left_on_wire(&self.documents.share_transport, &session_id, &expected_host) {
            self.documents.detach_share("");
            self.documents.notice = Some("Cannot join: that share session is closed.".to_owned());
            return false;
        }
        if let Some(live) = &self.documents.share {
            if live.document == document {
                return true;
            }
            self.documents.detach_share("");
        }
        let mut session = CollabSession::guest(session_id.clone(), data.me().as_str());
        session.join(&self.documents.share_transport);
        self.documents.share = Some(LiveShare {
            space,
            document,
            session,
            owner: false,
            expected_host,
            host_peer: String::new(),
        });
        self.documents
            .share_commands
            .emit(DocumentShareCommand::Join {
                space,
                document,
                session: session_id.to_string(),
            });
        self.pump_document_share();
        if self.documents.share.is_none() {
            return false;
        }
        self.documents.notice = Some("Joined share session.".to_owned());
        true
    }

    /// Follow `peer` in the live share-session. Returns false when the peer is
    /// not in the roster (you can only follow a collaborator you can see).
    pub(crate) fn follow_share_peer(&mut self, peer: &str) -> bool {
        let Some(live) = self.documents.share.as_mut() else {
            self.documents.notice = Some("Join a share session before following.".to_owned());
            return false;
        };
        if !live.session.follow(peer) {
            self.documents.notice =
                Some("Cannot follow: that participant is not in the share roster.".to_owned());
            return false;
        }
        let document = live.document;
        self.documents
            .share_commands
            .emit(DocumentShareCommand::Follow {
                document,
                peer: peer.to_owned(),
            });
        self.documents.notice = Some(format!("Following {peer}."));
        true
    }

    /// Stop following in the live share-session.
    pub(crate) fn unfollow_share_peer(&mut self) -> bool {
        let Some(live) = self.documents.share.as_mut() else {
            return false;
        };
        let document = live.document;
        live.session.unfollow();
        self.documents
            .share_commands
            .emit(DocumentShareCommand::Unfollow { document });
        self.documents.notice = Some("Stopped following.".to_owned());
        true
    }

    /// Owner closes the live share-session. Every follower detaches on the next
    /// pump (the host Leave frame). Non-owners are refused.
    pub(crate) fn close_document_share(&mut self) -> bool {
        let (space, document, session, owner) = match self.documents.share.as_ref() {
            None => {
                self.documents.notice = Some("No share session to close.".to_owned());
                return false;
            }
            Some(live) => (
                live.space,
                live.document,
                live.session.session_id().to_string(),
                live.owner,
            ),
        };
        if !owner {
            self.documents.notice = Some("Only the share owner can close this session.".to_owned());
            return false;
        }
        if let Some(live) = self.documents.share.take() {
            live.session.leave(&self.documents.share_transport);
        }
        self.documents
            .share_commands
            .emit(DocumentShareCommand::Close {
                space,
                document,
                session,
            });
        self.documents.notice = Some("Share session closed — followers detached.".to_owned());
        true
    }

    /// Detach an attached share when this seat is no longer a member or the
    /// projection has closed the session. Owners stay attached until they
    /// close even if the mount has not yet projected `Start`.
    fn refuse_stale_share(&mut self, data: &dyn CollabData) {
        let Some(live) = self.documents.share.as_ref() else {
            return;
        };
        let space = live.space;
        let document = live.document;
        let owner = live.owner;
        if !is_space_member(data, space) {
            self.documents
                .detach_share("Cannot stay shared: this seat is not a member of that space.");
            return;
        }
        if !owner && live_document_session(data, space, document).is_none() {
            self.documents
                .detach_share("Cannot stay shared: that share session is closed.");
        }
    }

    /// Reconcile membership / closed-session honesty, then pump the CRDT.
    pub(crate) fn sync_document_share(&mut self, data: &dyn CollabData) {
        self.refuse_stale_share(data);
        self.pump_document_share();
    }

    /// Pump the live share-session: mirror local edits into the CRDT, apply
    /// remote updates onto the editor, replay follow, and detach if the owner
    /// closed.
    pub(crate) fn pump_document_share(&mut self) {
        let outcome = self.documents.pump_share();
        if let Some(crdt_text) = outcome.crdt_text.as_ref() {
            if self.documents.editor.current_text().as_deref() != Some(crdt_text.as_str()) {
                self.documents.editor.replace_text(crdt_text);
            }
        }
        if let Some(update) = outcome.follow.as_ref() {
            let _ = self.documents.editor.apply_follow_update(update);
        }
        if outcome.host_left {
            self.documents
                .detach_share("Share session closed by its owner — followers detached.");
            return;
        }
        if outcome.follow_ended {
            self.documents.notice = Some("Stopped following — that participant left.".to_owned());
        }
    }

    /// Merge an external `document_body` against the last shared base instead of
    /// overwriting the live CRDT / editor buffer.
    ///
    /// The live side is the editor rope (in-flight keystrokes), never a stale
    /// CRDT that still equals `last_shared_base` — that path treated
    /// `local == base` and took remote wholesale. After a clean merge, the
    /// consumed disk snapshot is recorded in `last_seen_external` so the next
    /// frame cannot re-enter as `live == new_base` and clobber the merged local
    /// lines.
    fn merge_external_document(&mut self, data: &dyn CollabData, document: DocumentId) {
        let Some(external) = data.document_body(document) else {
            return;
        };
        if self.documents.last_seen_external.as_deref() == Some(external) {
            return;
        }
        let live = self.documents.editor.current_text().unwrap_or_default();
        let Some(base) = self.documents.last_shared_base.clone() else {
            if live != external {
                self.documents.last_seen_external = Some(external.to_owned());
                self.documents.external_conflict = Some(ExternalWriteConflict {
                    base: String::new(),
                    local: live,
                    remote: external.to_owned(),
                });
                self.documents.notice = Some(
                    "External write conflict — live edits kept; incoming file was not applied."
                        .to_owned(),
                );
            }
            return;
        };
        match merge_external_write(&base, &live, external) {
            ExternalWriteMerge::Clean(merged) => {
                if merged != live {
                    self.documents.editor.replace_text(&merged);
                    self.documents.mirror_merged_into_share(document, &merged);
                    self.documents.notice = Some("Merged external file changes.".to_owned());
                }
                // Ancestor for the *next* write is the merged live buffer.
                // The snapshot we already reconciled is `external` — do not
                // point `last_seen_external` at `merged` or the still-present
                // disk body looks like a fresh write against that ancestor.
                self.documents.last_shared_base = Some(merged);
                self.documents.last_seen_external = Some(external.to_owned());
                self.documents.external_conflict = None;
            }
            ExternalWriteMerge::Conflict(conflict) => {
                self.documents.last_seen_external = Some(external.to_owned());
                self.documents.external_conflict = Some(conflict);
                self.documents.notice = Some(
                    "External write conflict — live edits kept; incoming file was not applied."
                        .to_owned(),
                );
            }
        }
    }
}

/// Mirror local editor text into the CRDT as a prefix/suffix splice so concurrent
/// remote edits still merge instead of a whole-document replace.
fn mirror_local_text_into_session(session: &mut CollabSession, text: &str) {
    let current = session.doc().to_text();
    if current == text {
        return;
    }
    let prefix = char_prefix_len(&current, text);
    let suffix = char_suffix_len(&current, text, prefix);
    let current_len = current.chars().count();
    let text_len = text.chars().count();
    let remove_end = current_len.saturating_sub(suffix);
    if prefix < remove_end {
        let _ = session.local_remove(prefix..remove_end);
    }
    let insert: String = text
        .chars()
        .skip(prefix)
        .take(text_len.saturating_sub(prefix + suffix))
        .collect();
    if !insert.is_empty() {
        let _ = session.local_insert(prefix, &insert);
    }
}

fn char_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn char_suffix_len(a: &str, b: &str, prefix: usize) -> usize {
    let a_len = a.chars().count();
    let b_len = b.chars().count();
    let max = a_len.min(b_len).saturating_sub(prefix);
    a.chars()
        .rev()
        .zip(b.chars().rev())
        .take(max)
        .take_while(|(x, y)| x == y)
        .count()
}

/// Three-way merge of an external snapshot against the last shared base and the
/// live buffer. Overlapping concurrent edits surface a typed conflict; the live
/// side is never silently overwritten.
fn merge_external_write(base: &str, local: &str, remote: &str) -> ExternalWriteMerge {
    if local == remote {
        return ExternalWriteMerge::Clean(local.to_owned());
    }
    if local == base {
        return ExternalWriteMerge::Clean(remote.to_owned());
    }
    if remote == base {
        return ExternalWriteMerge::Clean(local.to_owned());
    }
    let base_lines = split_lines_keep_nl(base);
    let local_lines = split_lines_keep_nl(local);
    let remote_lines = split_lines_keep_nl(remote);
    match merge_line_lists(&base_lines, &local_lines, &remote_lines) {
        Some(merged) => ExternalWriteMerge::Clean(merged.concat()),
        None => ExternalWriteMerge::Conflict(ExternalWriteConflict {
            base: base.to_owned(),
            local: local.to_owned(),
            remote: remote.to_owned(),
        }),
    }
}

fn split_lines_keep_nl(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('\n') {
        lines.push(&rest[..=at]);
        rest = &rest[at + 1..];
    }
    if !rest.is_empty() {
        lines.push(rest);
    }
    lines
}

fn lcs_pairs(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

fn merge_line_lists(base: &[&str], local: &[&str], remote: &[&str]) -> Option<Vec<String>> {
    let local_matches = lcs_pairs(base, local);
    let remote_matches = lcs_pairs(base, remote);
    let remote_by_base: std::collections::BTreeMap<usize, usize> =
        remote_matches.into_iter().collect();
    let mut anchors: Vec<(usize, usize, usize)> = Vec::new();
    for (base_i, local_i) in local_matches {
        if let Some(&remote_i) = remote_by_base.get(&base_i) {
            anchors.push((base_i, local_i, remote_i));
        }
    }

    let mut merged = Vec::new();
    let mut prev_base = 0usize;
    let mut prev_local = 0usize;
    let mut prev_remote = 0usize;
    for (base_i, local_i, remote_i) in anchors {
        merge_gap(
            &mut merged,
            &base[prev_base..base_i],
            &local[prev_local..local_i],
            &remote[prev_remote..remote_i],
        )?;
        merged.push(local[local_i].to_owned());
        prev_base = base_i + 1;
        prev_local = local_i + 1;
        prev_remote = remote_i + 1;
    }
    merge_gap(
        &mut merged,
        &base[prev_base..],
        &local[prev_local..],
        &remote[prev_remote..],
    )?;
    Some(merged)
}

fn merge_gap(
    merged: &mut Vec<String>,
    base: &[&str],
    local: &[&str],
    remote: &[&str],
) -> Option<()> {
    if local == remote {
        merged.extend(local.iter().map(|line| (*line).to_owned()));
        return Some(());
    }
    if local == base {
        merged.extend(remote.iter().map(|line| (*line).to_owned()));
        return Some(());
    }
    if remote == base {
        merged.extend(local.iter().map(|line| (*line).to_owned()));
        return Some(());
    }
    None
}

/// The first non-blank line of `text`, trimmed and bounded — a short human
/// summary for the Activity feed (the `DocumentChange.summary`), or `None` for
/// an empty document. The canonical Markdown itself is never passed through
/// this presentation excerpt.
fn first_nonblank_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| bounded_document_display(line, MAX_DOCUMENT_SUMMARY_CHARS))
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_document_display, cap_review_comment, first_nonblank_line, markdown_preview,
        MAX_DOCUMENT_PREVIEW_CHARS, MAX_DOCUMENT_SUMMARY_CHARS, MAX_DOCUMENT_TITLE_CHARS,
        MAX_REVIEW_COMMENT_CHARS,
    };

    #[test]
    fn hostile_document_title_is_single_line_bidi_safe_and_bounded() {
        let hostile = format!(
            "Runbook\n\t\u{202e}ops\u{200b}{}",
            "x".repeat(MAX_DOCUMENT_TITLE_CHARS)
        );
        let display = bounded_document_display(&hostile, MAX_DOCUMENT_TITLE_CHARS);

        assert_eq!(display.chars().count(), MAX_DOCUMENT_TITLE_CHARS);
        assert!(display.ends_with('\u{2026}'));
        assert!(!display.contains(['\n', '\r', '\t', '\u{202e}', '\u{200b}']));
    }

    #[test]
    fn review_comment_cap_keeps_oversized_unicode_input_out_of_layout() {
        let mut comment = format!("🙂{}", "x".repeat(MAX_REVIEW_COMMENT_CHARS + 32));
        cap_review_comment(&mut comment);

        assert_eq!(comment.chars().count(), MAX_REVIEW_COMMENT_CHARS);
        assert!(comment.starts_with('🙂'));
        assert!(comment.is_char_boundary(comment.len()));
    }

    #[test]
    fn markdown_preview_is_a_utf8_safe_bounded_excerpt() {
        let source = format!(
            "{}é🦀{}",
            "# hostile\n".repeat(MAX_DOCUMENT_PREVIEW_CHARS),
            "tail"
        );
        let (preview, truncated) = markdown_preview(&source);

        assert!(truncated);
        assert_eq!(preview.chars().count(), MAX_DOCUMENT_PREVIEW_CHARS);
        assert!(preview.is_char_boundary(preview.len()));
        assert!(source.starts_with(preview));
    }

    #[test]
    fn markdown_summary_is_bounded_without_rewriting_the_source_body() {
        let markdown = format!("# \u{202e}{}\n\nbody", "x".repeat(512));
        let summary = first_nonblank_line(&markdown).expect("the heading is non-blank");

        assert_eq!(summary.chars().count(), MAX_DOCUMENT_SUMMARY_CHARS);
        assert!(summary.ends_with('\u{2026}'));
        assert!(!summary.contains('\u{202e}'));
        assert!(markdown.contains("\n\nbody"));
    }

    #[test]
    fn short_document_titles_remain_truthful() {
        assert_eq!(
            bounded_document_display("Runbook — ops", MAX_DOCUMENT_TITLE_CHARS),
            "Runbook — ops"
        );
        let (preview, truncated) = markdown_preview("# Runbook\n");
        assert_eq!(preview, "# Runbook\n");
        assert!(!truncated);
    }

    #[test]
    fn external_write_merges_non_overlapping_line_edits() {
        let merged = match super::merge_external_write("base\n", "base\nlocal\n", "remote\nbase\n")
        {
            super::ExternalWriteMerge::Clean(text) => text,
            super::ExternalWriteMerge::Conflict(_) => panic!("expected a clean merge"),
        };
        assert_eq!(merged, "remote\nbase\nlocal\n");
    }

    #[test]
    fn overlapping_external_write_is_a_typed_conflict_not_a_clobber() {
        match super::merge_external_write("hello\n", "hello world\n", "hello mesh\n") {
            super::ExternalWriteMerge::Conflict(conflict) => {
                assert_eq!(conflict.local, "hello world\n");
                assert_eq!(conflict.remote, "hello mesh\n");
            }
            super::ExternalWriteMerge::Clean(text) => {
                panic!("overlapping edits must not silently merge, got {text:?}")
            }
        }
    }

    #[test]
    fn attached_share_external_write_merges_or_conflicts_without_losing_unpumped_edits() {
        use crate::CommunicationsSurface;
        use mde_collab_types::{DocumentId, SpaceId};

        // The CRDT is pinned at last_shared_base until pump. An unpumped editor
        // edit must still be the live merge side, and the same disk snapshot
        // must not re-enter on the next apply as live==new_base (silent clobber).
        let space = SpaceId::new();
        let document = DocumentId::new();
        let data = share_fixture("zebra", space, document, &["zebra"]);
        let bus = mde_editor_egui::FakeBus::new();

        let mut surface = CommunicationsSurface::new();
        surface.bind_document_share_bus(bus);
        surface.select_space(space);
        surface.open_document(&data, document, "Runbook");
        assert!(surface.share_document(&data, space));
        assert_eq!(surface.last_shared_base(), Some("# Runbook\n"));

        surface.set_document_editor_text("# Runbook\nlocal\n");
        let remote = share_fixture("zebra", space, document, &["zebra"])
            .with_document_body(document, "remote\n# Runbook\n");
        surface.apply_external_document_body(&remote);
        assert_eq!(
            surface.document_editor_text().as_deref(),
            Some("remote\n# Runbook\nlocal\n"),
            "non-overlapping unpumped edit must merge, not yield to the stale CRDT"
        );
        assert!(surface.external_write_conflict().is_none());

        surface.apply_external_document_body(&remote);
        assert_eq!(
            surface.document_editor_text().as_deref(),
            Some("remote\n# Runbook\nlocal\n"),
            "re-applying the same external snapshot must not clobber the merged local line"
        );
        assert_eq!(
            surface.last_shared_base(),
            Some("remote\n# Runbook\nlocal\n")
        );

        surface.set_document_editor_text("hello world\n");
        let overlapping = share_fixture("zebra", space, document, &["zebra"])
            .with_document_body(document, "hello mesh\n");
        surface.apply_external_document_body(&overlapping);
        assert_eq!(
            surface.document_editor_text().as_deref(),
            Some("hello world\n"),
            "overlapping external write must keep the live edit"
        );
        let conflict = surface
            .external_write_conflict()
            .expect("overlapping external write must surface ExternalWriteConflict");
        assert_eq!(conflict.local, "hello world\n");
        assert_eq!(conflict.remote, "hello mesh\n");

        surface.apply_external_document_body(&overlapping);
        assert_eq!(
            surface.document_editor_text().as_deref(),
            Some("hello world\n"),
            "re-applying a conflicted snapshot must not clobber the kept live edit"
        );
        assert!(surface.external_write_conflict().is_some());
    }

    fn share_fixture(
        me: &str,
        space: mde_collab_types::SpaceId,
        document: mde_collab_types::DocumentId,
        participants: &[&str],
    ) -> crate::fixture::FixtureData {
        crate::fixture::FixtureData::document_share(
            me,
            space,
            document,
            participants,
            "# Runbook\n",
        )
    }

    #[test]
    fn owner_close_detaches_every_follower_even_when_another_guest_sorts_first() {
        use crate::CommunicationsSurface;
        use mde_collab_types::{DocumentId, SpaceId, SpaceKind, SpaceRole};

        // Host id sorts *after* both guests. The previous first-roster-key pin
        // would have treated "alpha" as the host and then ignored zebra's Leave.
        let space = SpaceId::new();
        let document = DocumentId::new();
        let host_data = share_fixture("zebra", space, document, &["zebra", "alpha", "beta"]);
        let alpha_data = share_fixture("alpha", space, document, &["zebra", "alpha", "beta"]);
        let beta_data = share_fixture("beta", space, document, &["zebra", "alpha", "beta"]);
        let bus = mde_editor_egui::FakeBus::new();

        let mut host = CommunicationsSurface::new();
        host.bind_document_share_bus(bus.clone());
        host.select_space(space);
        host.open_document(&host_data, document, "Runbook");
        assert!(host.share_document(&host_data, space));

        let mut alpha = CommunicationsSurface::new();
        alpha.bind_document_share_bus(bus.clone());
        alpha.select_space(space);
        alpha.open_document(&alpha_data, document, "Runbook");
        assert!(alpha.join_document_share(&alpha_data, space, document));

        let mut beta = CommunicationsSurface::new();
        beta.bind_document_share_bus(bus);
        beta.select_space(space);
        beta.open_document(&beta_data, document, "Runbook");
        assert!(beta.join_document_share(&beta_data, space, document));

        for _ in 0..6 {
            host.pump_document_share();
            alpha.pump_document_share();
            beta.pump_document_share();
        }

        assert_eq!(alpha.document_share_host_peer(), Some("zebra"));
        assert_eq!(beta.document_share_host_peer(), Some("zebra"));
        assert!(
            !alpha.close_document_share(),
            "only the owner may close the session"
        );
        assert!(
            alpha.has_live_document_share(),
            "a refused guest close must leave the follower attached"
        );
        assert!(
            alpha
                .document_notice()
                .is_some_and(|notice| notice.contains("Only the share owner")),
            "guest close must refuse honestly, got {:?}",
            alpha.document_notice()
        );

        assert!(host.close_document_share());
        alpha.pump_document_share();
        beta.pump_document_share();
        assert!(
            !alpha.has_live_document_share(),
            "owner close must detach the guest whose id sorts first"
        );
        assert!(
            !beta.has_live_document_share(),
            "owner close must detach every follower"
        );

        // Closed-session honesty after attach: a later projection with no row
        // (and a non-member directory) must refuse, not stay silently joined.
        let closed = crate::fixture::FixtureData::new("alpha", 1_000).with_space(
            crate::fixture::space_summary(
                space,
                SpaceKind::Project,
                "Docs",
                SpaceRole::Member,
                0,
                1,
                1_000,
            ),
        );
        alpha.open_document(&alpha_data, document, "Runbook");
        assert!(
            !alpha.join_document_share(&closed, space, document),
            "a closed session must refuse a later join"
        );
        assert!(!alpha.has_live_document_share());
        assert!(
            alpha
                .document_notice()
                .is_some_and(|notice| notice.contains("closed")),
            "closed-session refuse must be honest, got {:?}",
            alpha.document_notice()
        );

        let outsider = crate::fixture::FixtureData::new("osprey", 1_000);
        let mut stranger = CommunicationsSurface::new();
        stranger.select_space(space);
        stranger.open_document(&outsider, document, "Runbook");
        assert!(
            !stranger.share_document(&outsider, space),
            "a non-member must be refused at share"
        );
        assert!(!stranger.has_live_document_share());
        assert!(
            stranger
                .document_notice()
                .is_some_and(|notice| notice.contains("not a member")),
            "non-member share refuse must be honest, got {:?}",
            stranger.document_notice()
        );
    }

    #[test]
    fn attached_share_refuses_honestly_when_session_closes_or_membership_is_lost() {
        use crate::CommunicationsSurface;
        use mde_collab_types::{DocumentId, SpaceId, SpaceKind, SpaceRole};

        let space = SpaceId::new();
        let document = DocumentId::new();
        let host_data = share_fixture("zebra", space, document, &["zebra", "alpha"]);
        let guest_data = share_fixture("alpha", space, document, &["zebra", "alpha"]);
        let bus = mde_editor_egui::FakeBus::new();

        let mut host = CommunicationsSurface::new();
        host.bind_document_share_bus(bus.clone());
        host.select_space(space);
        host.open_document(&host_data, document, "Runbook");
        assert!(host.share_document(&host_data, space));

        let mut guest = CommunicationsSurface::new();
        guest.bind_document_share_bus(bus);
        guest.select_space(space);
        guest.open_document(&guest_data, document, "Runbook");
        assert!(guest.join_document_share(&guest_data, space, document));
        assert!(guest.has_live_document_share());

        let closed = crate::fixture::FixtureData::new("alpha", 1_000).with_space(
            crate::fixture::space_summary(
                space,
                SpaceKind::Project,
                "Docs",
                SpaceRole::Member,
                0,
                1,
                1_000,
            ),
        );
        guest.sync_document_share(&closed);
        assert!(
            !guest.has_live_document_share(),
            "an already-attached guest must detach when the session row disappears"
        );
        assert!(
            guest
                .document_notice()
                .is_some_and(|notice| notice.contains("closed")),
            "closed-session refuse after attach must be honest, got {:?}",
            guest.document_notice()
        );

        assert!(guest.join_document_share(&guest_data, space, document));
        guest.sync_document_share(&crate::fixture::FixtureData::new("alpha", 1_000));
        assert!(
            !guest.has_live_document_share(),
            "an already-attached guest must detach after membership loss"
        );
        assert!(
            guest
                .document_notice()
                .is_some_and(|notice| notice.contains("not a member")),
            "non-member refuse after attach must be honest, got {:?}",
            guest.document_notice()
        );
    }

    #[test]
    fn owner_close_detaches_unpinned_followers_and_refuses_stale_rejoin() {
        use crate::CommunicationsSurface;
        use mde_collab_types::{DocumentId, SpaceId};

        // Guest join tails past the host Hello. If the owner closes before the
        // host answers, the follower never pins the host — Leave on the same
        // transport must still detach, and a later join against the still-
        // projected session row must refuse as closed.
        let space = SpaceId::new();
        let document = DocumentId::new();
        let host_data = share_fixture("zebra", space, document, &["zebra", "alpha"]);
        let guest_data = share_fixture("alpha", space, document, &["zebra", "alpha"]);
        let bus = mde_editor_egui::FakeBus::new();

        let mut host = CommunicationsSurface::new();
        host.bind_document_share_bus(bus.clone());
        host.select_space(space);
        host.open_document(&host_data, document, "Runbook");
        assert!(host.share_document(&host_data, space));

        let mut guest = CommunicationsSurface::new();
        guest.bind_document_share_bus(bus.clone());
        guest.select_space(space);
        guest.open_document(&guest_data, document, "Runbook");
        assert!(guest.join_document_share(&guest_data, space, document));
        assert_eq!(
            guest.document_share_host_peer(),
            Some(""),
            "host must stay unpinned when it has not answered the join"
        );
        assert!(
            !guest.close_document_share(),
            "only the owner may close the session"
        );
        assert!(
            guest.has_live_document_share(),
            "a refused guest close must leave the unpinned follower attached"
        );
        assert!(
            guest
                .document_notice()
                .is_some_and(|notice| notice.contains("Only the share owner")),
            "guest close must refuse honestly, got {:?}",
            guest.document_notice()
        );

        assert!(host.close_document_share());
        guest.pump_document_share();
        assert!(
            !guest.has_live_document_share(),
            "owner Leave must detach a follower who never pinned the host"
        );
        assert!(
            guest
                .document_notice()
                .is_some_and(|notice| { notice.contains("closed") || notice.contains("detached") }),
            "owner-close detach must be honest, got {:?}",
            guest.document_notice()
        );

        let mut late = CommunicationsSurface::new();
        late.bind_document_share_bus(bus);
        late.select_space(space);
        late.open_document(&guest_data, document, "Runbook");
        assert!(
            !late.join_document_share(&guest_data, space, document),
            "a stale session row after owner-close must refuse the join"
        );
        assert!(!late.has_live_document_share());
        assert!(
            late.document_notice()
                .is_some_and(|notice| notice.contains("closed")),
            "closed-session rejoin must refuse honestly, got {:?}",
            late.document_notice()
        );

        let outsider = crate::fixture::FixtureData::new("osprey", 1_000);
        let mut stranger = CommunicationsSurface::new();
        stranger.select_space(space);
        stranger.open_document(&outsider, document, "Runbook");
        assert!(
            !stranger.join_document_share(&outsider, space, document),
            "a non-member must be refused at join"
        );
        assert!(!stranger.has_live_document_share());
        assert!(
            stranger
                .document_notice()
                .is_some_and(|notice| notice.contains("not a member")),
            "non-member join refuse must be honest, got {:?}",
            stranger.document_notice()
        );
    }

    #[test]
    fn follow_unfollow_close_hit_the_live_attached_session() {
        use crate::CommunicationsSurface;
        use mde_collab_types::{DocumentId, SpaceId};

        // The shell mount drains DocumentShareCommand with this live
        // session so Follow / Unfollow / Close land on the attached
        // CRDT share_document / join created — never a second one.
        let space = SpaceId::new();
        let document = DocumentId::new();
        let host_data = share_fixture("eagle", space, document, &["eagle", "falcon"]);
        let guest_data = share_fixture("falcon", space, document, &["eagle", "falcon"]);
        let bus = mde_editor_egui::FakeBus::new();

        let mut host = CommunicationsSurface::new();
        host.bind_document_share_bus(bus.clone());
        host.select_space(space);
        host.open_document(&host_data, document, "Runbook");
        assert!(host.share_document(&host_data, space));
        assert!(
            host.live_document_share_session().is_some(),
            "Share must attach the live CollabSession, not a second CRDT"
        );

        let mut guest = CommunicationsSurface::new();
        guest.bind_document_share_bus(bus);
        guest.select_space(space);
        guest.open_document(&guest_data, document, "Runbook");
        assert!(guest.join_document_share(&guest_data, space, document));
        for _ in 0..4 {
            host.pump_document_share();
            guest.pump_document_share();
        }

        assert!(guest.follow_share_peer("eagle"));
        assert_eq!(
            guest
                .live_document_share_session()
                .and_then(|session| session.following().map(str::to_owned))
                .as_deref(),
            Some("eagle"),
            "Follow must apply on the attached session, not only the drain queue"
        );

        assert!(guest.unfollow_share_peer());
        assert_eq!(
            guest
                .live_document_share_session()
                .and_then(|session| session.following().map(str::to_owned)),
            None,
            "Unfollow must clear follow on the attached session"
        );

        assert!(host.close_document_share());
        assert!(
            host.live_document_share_session().is_none(),
            "owner Close must drop the attached session"
        );
        guest.pump_document_share();
        assert!(
            guest.live_document_share_session().is_none(),
            "owner Close must detach the follower's attached session"
        );
    }

    #[test]
    fn two_seats_co_edit_without_clobbering_and_follow_replays_the_host_caret() {
        use crate::CommunicationsSurface;
        use mde_collab_types::{DocumentId, SpaceId};

        let space = SpaceId::new();
        let document = DocumentId::new();
        let host_data = share_fixture("eagle", space, document, &["eagle", "falcon"]);
        let guest_data = share_fixture("falcon", space, document, &["eagle", "falcon"]);
        let bus = mde_editor_egui::FakeBus::new();

        let mut host = CommunicationsSurface::new();
        host.bind_document_share_bus(bus.clone());
        host.select_space(space);
        host.open_document(&host_data, document, "Runbook");
        assert!(host.share_document(&host_data, space));

        let mut guest = CommunicationsSurface::new();
        guest.bind_document_share_bus(bus);
        guest.select_space(space);
        guest.open_document(&guest_data, document, "Runbook");
        assert!(guest.join_document_share(&guest_data, space, document));
        for _ in 0..4 {
            host.pump_document_share();
            guest.pump_document_share();
        }

        guest.set_document_editor_cursor(2);
        host.set_document_editor_text("# Runbook\nhost\n");
        for _ in 0..6 {
            host.pump_document_share();
            guest.pump_document_share();
        }
        assert_eq!(
            guest.document_editor_text().as_deref(),
            Some("# Runbook\nhost\n"),
            "guest must receive the host line"
        );
        assert_eq!(
            guest.document_editor_cursor(),
            Some(mde_editor_egui::CursorPos::caret(2)),
            "applying a remote CRDT snapshot must not jump the guest caret to the end"
        );

        guest.set_document_editor_text("# Runbook\nhost\nguest\n");
        for _ in 0..6 {
            host.pump_document_share();
            guest.pump_document_share();
        }
        assert_eq!(
            host.document_editor_text().as_deref(),
            Some("# Runbook\nhost\nguest\n"),
            "host must receive the guest line"
        );
        assert_eq!(
            host.document_editor_text().as_deref(),
            guest.document_editor_text().as_deref(),
            "two seats must converge after sequential co-edits"
        );

        host.set_document_editor_cursor(4);
        for _ in 0..4 {
            host.pump_document_share();
            guest.pump_document_share();
        }
        assert!(
            !guest.follow_share_peer("osprey"),
            "follow of a peer not in the roster must refuse"
        );
        assert!(
            guest
                .document_notice()
                .is_some_and(|notice| notice.contains("not in the share roster")),
            "unknown-peer follow must refuse honestly, got {:?}",
            guest.document_notice()
        );
        assert!(guest.follow_share_peer("eagle"));
        for _ in 0..4 {
            host.pump_document_share();
            guest.pump_document_share();
        }
        assert_eq!(
            guest.document_editor_cursor(),
            Some(mde_editor_egui::CursorPos::caret(4)),
            "follow must replay the host caret onto the guest editor"
        );
    }

    #[test]
    fn concurrent_suffix_edits_from_a_shared_base_keep_both_lines() {
        use crate::CommunicationsSurface;
        use mde_collab_types::{DocumentId, SpaceId};

        let space = SpaceId::new();
        let document = DocumentId::new();
        let host_data = share_fixture("eagle", space, document, &["eagle", "falcon"]);
        let guest_data = share_fixture("falcon", space, document, &["eagle", "falcon"]);
        let bus = mde_editor_egui::FakeBus::new();

        let mut host = CommunicationsSurface::new();
        host.bind_document_share_bus(bus.clone());
        host.select_space(space);
        host.open_document(&host_data, document, "Runbook");
        assert!(host.share_document(&host_data, space));

        let mut guest = CommunicationsSurface::new();
        guest.bind_document_share_bus(bus);
        guest.select_space(space);
        guest.open_document(&guest_data, document, "Runbook");
        assert!(guest.join_document_share(&guest_data, space, document));
        for _ in 0..4 {
            host.pump_document_share();
            guest.pump_document_share();
        }

        host.set_document_editor_text("# Runbook\nhost\n");
        guest.set_document_editor_text("# Runbook\nguest\n");
        for _ in 0..8 {
            host.pump_document_share();
            guest.pump_document_share();
        }
        let host_text = host.document_editor_text().expect("host buffer");
        let guest_text = guest.document_editor_text().expect("guest buffer");
        assert_eq!(
            host_text, guest_text,
            "concurrent suffix inserts must converge"
        );
        assert!(
            host_text.contains("host") && host_text.contains("guest"),
            "neither seat's line may be silently dropped, got {host_text:?}"
        );
    }
}
