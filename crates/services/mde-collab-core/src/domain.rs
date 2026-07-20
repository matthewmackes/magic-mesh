//! The folded **domain aggregate** — the authoritative facts
//! [`apply_command`](crate::apply_command) validates against (membership, roles,
//! message authorship + age + tombstone, entity existence, alert action kinds).
//!
//! It is a pure fold of the signed event set in the **canonical convergent
//! order** — `(clock.wall_ms, clock.counter, event_id)`. Folding the same set
//! in canonical order on every node yields the same aggregate, so a validation
//! decision is deterministic across the mesh. This aggregate carries only what
//! validation needs; the rendered read-models are the SQLite projection's job.

use std::collections::BTreeMap;

use mde_collab_types::event::CollabEventKind;
use mde_collab_types::ids::{
    CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId,
};
use mde_collab_types::value::{AlertActionKind, CallParticipantState};
use mde_collab_types::{ActorId, CollabEventEnvelope, SpaceKind, SpaceRole};

/// The total, deterministic sort key that orders a merged multi-node log:
/// causal clock first, then the opaque [`EventId`] as a stable tiebreak.
#[must_use]
pub fn sort_key(env: &CollabEventEnvelope) -> (u64, u32, EventId) {
    (env.clock.wall_ms, env.clock.counter, env.event_id)
}

/// Sort `events` into the canonical convergent order in place.
pub fn canonical_sort(events: &mut [CollabEventEnvelope]) {
    events.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
}

/// A member's standing in a space.
#[derive(Debug, Clone)]
pub struct MemberAgg {
    /// Their role.
    pub role: SpaceRole,
    /// Whether they are currently present (a left member is retained as absent).
    pub present: bool,
}

/// A space's validation-relevant facts.
#[derive(Debug, Clone)]
pub struct SpaceAgg {
    /// The space kind.
    pub kind: SpaceKind,
    /// The current name.
    pub name: String,
    /// Whether the space has been deleted (tombstoned).
    pub deleted: bool,
    /// Members, present or historically-left.
    pub members: BTreeMap<ActorId, MemberAgg>,
}

impl SpaceAgg {
    /// The count of currently-present Owners.
    #[must_use]
    pub fn present_owner_count(&self) -> usize {
        self.members
            .values()
            .filter(|m| m.present && matches!(m.role, SpaceRole::Owner))
            .count()
    }
}

/// A message's validation facts.
#[derive(Debug, Clone)]
pub struct MessageAgg {
    /// The space it belongs to.
    pub space: SpaceId,
    /// The author.
    pub author: ActorId,
    /// Its injected creation time (epoch ms).
    pub created_ms: i64,
    /// Whether it has been deleted (tombstoned).
    pub deleted: bool,
}

/// A clipboard item's validation facts.
#[derive(Debug, Clone)]
pub struct ClipAgg {
    /// The space it lives in.
    pub space: SpaceId,
    /// Whether it has been deleted (tombstoned).
    pub deleted: bool,
    /// Whether it is pinned (LWW-folded; survives a `ClearClipboard`).
    pub pinned: bool,
}

/// A call's validation facts.
#[derive(Debug, Clone)]
pub struct CallAgg {
    /// The space it is in.
    pub space: SpaceId,
    /// Whether it has ended.
    pub ended: bool,
    /// Participant states, LWW-folded.
    pub participants: BTreeMap<ActorId, CallParticipantState>,
}

/// The folded aggregate.
#[derive(Debug, Default, Clone)]
pub struct DomainState {
    /// Spaces by id.
    pub spaces: BTreeMap<SpaceId, SpaceAgg>,
    /// Messages by their event id.
    pub messages: BTreeMap<EventId, MessageAgg>,
    /// Threads → their space.
    pub threads: BTreeMap<ThreadId, SpaceId>,
    /// Documents → their space.
    pub documents: BTreeMap<DocumentId, SpaceId>,
    /// File references → (space, currently-present).
    pub files: BTreeMap<FileRefId, (SpaceId, bool)>,
    /// Transfers → their space.
    pub transfers: BTreeMap<TransferId, SpaceId>,
    /// Calls by id.
    pub calls: BTreeMap<CallId, CallAgg>,
    /// Clipboard items by their publish event id.
    pub clips: BTreeMap<EventId, ClipAgg>,
    /// Alert events → (space, action-id → kind).
    pub alerts: BTreeMap<EventId, (SpaceId, BTreeMap<String, AlertActionKind>)>,
}

impl DomainState {
    /// Fold a set of envelopes (in any order) into the aggregate. The set is
    /// sorted into canonical order first, so the result is order-independent.
    #[must_use]
    pub fn from_events(events: &[CollabEventEnvelope]) -> Self {
        let mut sorted = events.to_vec();
        canonical_sort(&mut sorted);
        let mut state = Self::default();
        for env in &sorted {
            state.apply(env);
        }
        state
    }

    /// Fold one already-canonically-ordered event into the aggregate.
    fn apply(&mut self, env: &CollabEventEnvelope) {
        let space_id = env.space_id;
        match &env.kind {
            CollabEventKind::SpaceCreated { kind, name } => {
                self.spaces.entry(space_id).or_insert_with(|| SpaceAgg {
                    kind: *kind,
                    name: name.clone(),
                    deleted: false,
                    members: BTreeMap::new(),
                });
            }
            CollabEventKind::SpaceRenamed { name } => {
                if let Some(s) = self.spaces.get_mut(&space_id) {
                    s.name = name.clone();
                }
            }
            CollabEventKind::SpaceArchived => {}
            CollabEventKind::SpaceDeleted => {
                if let Some(s) = self.spaces.get_mut(&space_id) {
                    s.deleted = true;
                }
            }
            CollabEventKind::MemberJoined { actor, role } => {
                if let Some(s) = self.spaces.get_mut(&space_id) {
                    s.members.insert(
                        actor.clone(),
                        MemberAgg {
                            role: *role,
                            present: true,
                        },
                    );
                }
            }
            CollabEventKind::MemberLeft { actor } => {
                if let Some(s) = self.spaces.get_mut(&space_id) {
                    if let Some(m) = s.members.get_mut(actor) {
                        m.present = false;
                    }
                }
            }
            CollabEventKind::MemberRoleChanged { actor, role } => {
                if let Some(s) = self.spaces.get_mut(&space_id) {
                    if let Some(m) = s.members.get_mut(actor) {
                        m.role = *role;
                    }
                }
            }
            CollabEventKind::PresenceChanged { .. } => {}
            CollabEventKind::MessagePosted { thread, .. } => {
                let _ = thread;
                self.messages.insert(
                    env.event_id,
                    MessageAgg {
                        space: space_id,
                        author: env.actor.clone(),
                        created_ms: env.created_unix_ms,
                        deleted: false,
                    },
                );
            }
            CollabEventKind::MessageEdited { .. } => {}
            CollabEventKind::MessageDeleted { target } => {
                // Only the author's own delete tombstones the message.
                if let Some(m) = self.messages.get_mut(target) {
                    if m.author == env.actor {
                        m.deleted = true;
                    }
                }
            }
            CollabEventKind::ThreadStarted { thread, .. } => {
                self.threads.insert(*thread, space_id);
            }
            CollabEventKind::ThreadResolved { .. } => {}
            CollabEventKind::AlertRaised { alert } => {
                let actions = alert
                    .actions
                    .iter()
                    .map(|a| (a.id.clone(), a.kind))
                    .collect();
                self.alerts.insert(env.event_id, (space_id, actions));
            }
            CollabEventKind::AlertAcknowledged { .. }
            | CollabEventKind::AlertSnoozed { .. }
            | CollabEventKind::AlertActionInvoked { .. } => {}
            CollabEventKind::ClipboardPublished { .. } => {
                self.clips.insert(
                    env.event_id,
                    ClipAgg {
                        space: space_id,
                        deleted: false,
                        pinned: false,
                    },
                );
            }
            CollabEventKind::ClipboardDeleted { target } => {
                if let Some(c) = self.clips.get_mut(target) {
                    c.deleted = true;
                }
            }
            CollabEventKind::ClipboardPinned { target } => {
                if let Some(c) = self.clips.get_mut(target) {
                    c.pinned = true;
                }
            }
            CollabEventKind::ClipboardUnpinned { target } => {
                if let Some(c) = self.clips.get_mut(target) {
                    c.pinned = false;
                }
            }
            CollabEventKind::DocumentCreated { document, .. } => {
                self.documents.insert(*document, space_id);
            }
            CollabEventKind::DocumentUpdated { .. }
            | CollabEventKind::ReviewRequested { .. }
            | CollabEventKind::ReviewSubmitted { .. } => {}
            CollabEventKind::FileLinked { file, .. } => {
                self.files.insert(*file, (space_id, true));
            }
            CollabEventKind::FileUnlinked { file } => {
                if let Some(f) = self.files.get_mut(file) {
                    f.1 = false;
                }
            }
            CollabEventKind::TransferStarted { transfer, .. } => {
                self.transfers.insert(*transfer, space_id);
            }
            CollabEventKind::TransferStateChanged { .. } => {}
            CollabEventKind::CallStarted {
                call, initiator, ..
            } => {
                let mut participants = BTreeMap::new();
                participants.insert(initiator.clone(), CallParticipantState::Connected);
                self.calls.insert(
                    *call,
                    CallAgg {
                        space: space_id,
                        ended: false,
                        participants,
                    },
                );
            }
            CollabEventKind::CallParticipantChanged { call, actor, state } => {
                if let Some(c) = self.calls.get_mut(call) {
                    c.participants.insert(actor.clone(), *state);
                }
            }
            CollabEventKind::CallEnded { call, .. } => {
                if let Some(c) = self.calls.get_mut(call) {
                    c.ended = true;
                }
            }
            CollabEventKind::AiSuggestionOffered { .. }
            | CollabEventKind::AiSuggestionResolved { .. } => {}
        }
    }

    /// The space aggregate, if it exists.
    #[must_use]
    pub fn space(&self, space: SpaceId) -> Option<&SpaceAgg> {
        self.spaces.get(&space)
    }

    /// Whether `actor` is a currently-present member of `space`.
    #[must_use]
    pub fn is_member(&self, space: SpaceId, actor: &ActorId) -> bool {
        self.spaces
            .get(&space)
            .and_then(|s| s.members.get(actor))
            .is_some_and(|m| m.present)
    }

    /// `actor`'s role in `space`, if present.
    #[must_use]
    pub fn role(&self, space: SpaceId, actor: &ActorId) -> Option<SpaceRole> {
        self.spaces
            .get(&space)
            .and_then(|s| s.members.get(actor))
            .filter(|m| m.present)
            .map(|m| m.role)
    }

    /// Whether `actor` is a present Owner of `space`.
    #[must_use]
    pub fn is_owner(&self, space: SpaceId, actor: &ActorId) -> bool {
        matches!(self.role(space, actor), Some(SpaceRole::Owner))
    }
}
