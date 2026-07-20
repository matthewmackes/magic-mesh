//! The read/write seams the [`CommunicationsSurface`](crate::CommunicationsSurface)
//! is handed: the [`CollabData`] projection source it renders, the
//! [`CommandSink`] it pushes intent into, and the pure rules those two are
//! coupled by (the message edit/delete window).
//!
//! This crate is a **pure UI** crate: it never owns authoritative state and never
//! calls a provider. It READS the retained read-side projections through
//! [`CollabData`] and EMITS [`CollabCommand`]s through [`CommandSink`]; the real
//! shell mount hands over a Bus-backed [`CollabData`] and drains the sink onto
//! `action/collab/*`, a later phase. Tests hand over
//! [`FixtureData`](crate::FixtureData).

use mde_collab_types::{
    ActivityFeed, ActorId, CallState, CollabCommand, ConversationTimeline, EventId, FileReferences,
    MessageView, SpaceDirectory, SpaceId, ThreadId, ThreadTimeline, TransferJobs,
};

/// The message **edit/delete window** in milliseconds: an author may amend
/// (edit or delete) their own message for five minutes after it was posted. A
/// later attempt is refused by the worker, so the UI reflects the same rule —
/// the affordance is shown *denied*, never silently hidden, once the window has
/// passed (spec §3). The single source of the window so the surface and its
/// tests never re-decide "how long is the edit window".
pub const EDIT_WINDOW_MS: i64 = 5 * 60 * 1_000;

/// Read-side access to the Communications projections the surface renders.
///
/// The worker folds the signed event log into the [`CollabReadModel`] variants
/// and publishes them latest-wins; this trait is the surface's window onto that
/// retained read side plus the two ambient facts an author-scoped affordance
/// needs — *who am I* ([`me`](Self::me)) and *what time is it now*
/// ([`now_unix_ms`](Self::now_unix_ms), injected, never a wall-clock read in this
/// pure crate). Every accessor is read-only; the surface mutates nothing here.
///
/// [`CollabReadModel`]: mde_collab_types::CollabReadModel
pub trait CollabData {
    /// The local seat's actor identity — the author-ownership key the edit/delete
    /// affordance and the "my message" alignment read.
    fn me(&self) -> &ActorId;

    /// The injected wall time in epoch milliseconds. Used only to evaluate the
    /// [`EDIT_WINDOW_MS`] amend window and to render relative message ages; this
    /// crate never reads a real clock (governance: the surface is deterministic
    /// under a fixture time).
    fn now_unix_ms(&self) -> i64;

    /// The left-rail directory of spaces the seat is a member of.
    fn space_directory(&self) -> &SpaceDirectory;

    /// A space's Activity feed, or the cross-space feed when `space` is `None`.
    /// `None` if no feed has been projected yet.
    fn activity(&self, space: Option<SpaceId>) -> Option<&ActivityFeed>;

    /// A space's main conversation timeline. `None` until one is projected.
    fn conversation(&self, space: SpaceId) -> Option<&ConversationTimeline>;

    /// A thread's root + replies, addressed by `thread` within `space`. `None`
    /// until the thread has been projected.
    fn thread(&self, space: SpaceId, thread: ThreadId) -> Option<&ThreadTimeline>;

    /// The thread rooted at message `root` in `space`, if one has been started —
    /// the reverse lookup the "N replies" affordance opens. Defaults to `None`
    /// for a data source that does not index threads by their root.
    #[must_use]
    fn thread_for_root(&self, space: SpaceId, root: EventId) -> Option<ThreadId> {
        let _ = (space, root);
        None
    }

    /// The active call state — the persistent call bar's read model. Defaults to
    /// an empty (no active call) state, so a source that has not wired calls yet
    /// still renders the honest "no active call" bar.
    fn call_state(&self) -> &CallState;

    /// A space's linked-file references (the
    /// [`FileReferences`](mde_collab_types::FileReferences) projection the Files
    /// mode renders). `None` until the worker has projected any for the space —
    /// the honest "no files linked yet" empty state, never faked. Defaults to
    /// `None` so a data source that has not folded the `file_references` mirror
    /// yet still compiles + renders the empty state.
    #[must_use]
    fn file_references(&self, space: SpaceId) -> Option<&FileReferences> {
        let _ = space;
        None
    }

    /// The read-side mirror of the shared transfer ledger (WL-FUNC-006), scoped to
    /// the whole seat — the Files mode looks a linked file's transfer up in here by
    /// its `FileRefId`. This crate never owns a second progress authority: byte
    /// progress (`moved`/`total`) is *mirrored* from the ledger, never recomputed.
    /// Defaults to `None` so a source that has not folded `transfer_jobs` yet still
    /// renders the honest "not shared yet" state.
    #[must_use]
    fn transfer_jobs(&self) -> Option<&TransferJobs> {
        None
    }
}

/// The sink the surface pushes emitted [`CollabCommand`]s into for the caller to
/// drain and route onto `action/collab/*`.
///
/// The surface NEVER routes a command itself — it only records intent here, and
/// the shell mount (a later phase) drains the queue each frame and publishes it
/// through the persist-first Bus path. Keeping the queue explicit (rather than a
/// bare callback) is what lets the headless tests assert *exactly* which command
/// a gesture emitted.
#[derive(Debug, Default, Clone)]
pub struct CommandSink {
    queued: Vec<CollabCommand>,
}

impl CommandSink {
    /// A fresh, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `command` as intent for the caller to route.
    pub fn emit(&mut self, command: CollabCommand) {
        self.queued.push(command);
    }

    /// Take every queued command, leaving the sink empty — the caller drains this
    /// once per frame and publishes the batch.
    #[must_use = "the drained commands must be routed onto action/collab/*"]
    pub fn drain(&mut self) -> Vec<CollabCommand> {
        std::mem::take(&mut self.queued)
    }

    /// The queued commands without draining (test assertions read this).
    #[must_use]
    pub fn queued(&self) -> &[CollabCommand] {
        &self.queued
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// How many commands are queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queued.len()
    }
}

/// Whether — and how — the local seat may amend (edit/delete) a message, the
/// decision the Messages timeline reflects in its affordances (spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmendAffordance {
    /// The seat authored this message and is still inside the edit window — the
    /// edit + delete controls are shown enabled.
    Allowed,
    /// The seat authored this message but the [`EDIT_WINDOW_MS`] window has
    /// passed — the controls are shown *denied* (disabled, explained), never
    /// hidden, because a later attempt is a refused action, not an absent one.
    DeniedExpired,
    /// Not the seat's message (or already deleted) — no amend affordance at all.
    Hidden,
}

impl AmendAffordance {
    /// Whether the edit/delete controls appear at all (enabled or denied).
    #[must_use]
    pub const fn is_visible(self) -> bool {
        matches!(self, Self::Allowed | Self::DeniedExpired)
    }

    /// Whether the edit/delete controls are actionable (inside the window).
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// Evaluate the amend affordance for `message`, given the local seat `me` and the
/// injected `now_unix_ms`. This is the pure rule the Messages timeline drives its
/// edit/delete controls from: the author, within [`EDIT_WINDOW_MS`], and not
/// already a tombstone → [`Allowed`](AmendAffordance::Allowed); the author but
/// past the window → [`DeniedExpired`](AmendAffordance::DeniedExpired); anyone
/// else (or a deleted message) → [`Hidden`](AmendAffordance::Hidden).
#[must_use]
pub fn amend_affordance(me: &ActorId, now_unix_ms: i64, message: &MessageView) -> AmendAffordance {
    if message.deleted || &message.author != me {
        return AmendAffordance::Hidden;
    }
    let age = now_unix_ms.saturating_sub(message.created_unix_ms);
    if (0..=EDIT_WINDOW_MS).contains(&age) {
        AmendAffordance::Allowed
    } else {
        AmendAffordance::DeniedExpired
    }
}

/// A short, dependency-free relative age (`"now"`, `"5m"`, `"3h"`, `"2d"`) for
/// `then_unix_ms` as seen at `now_unix_ms`. Pure integer math — the surface reads
/// no date library and no wall clock, so it renders identically under a fixture
/// time. Times in the future (a clock-skewed peer) clamp to `"now"`.
#[must_use]
pub fn relative_age(now_unix_ms: i64, then_unix_ms: i64) -> String {
    let secs = now_unix_ms.saturating_sub(then_unix_ms).max(0) / 1_000;
    if secs < 45 {
        return "now".to_owned();
    }
    let mins = secs / 60;
    if mins < 1 {
        return "1m".to_owned();
    }
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}
