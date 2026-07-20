//! The command → signed-events pipeline: [`apply_command`].
//!
//! It validates a [`CollabCommand`] against the folded [`DomainState`]
//! (membership, Owner/Member permission, the 5-minute author edit/delete
//! window, entity existence, destructive-action arming), and on success mints,
//! HLC-stamps, and **signs** one or more [`CollabEventEnvelope`]s via the
//! injected [`EventSigner`] + [`IdSource`]. A rejected command returns a typed
//! [`CollabError`] — the denial is *visible*, never a silent no-op.
//!
//! The pipeline itself performs no I/O and reads no wall clock: time, ids, and
//! signing are all supplied through [`ApplyCtx`], so the same command replays to
//! byte-identical events.

use mde_collab_types::event::CollabEventKind;
use mde_collab_types::ids::SpaceId;
use mde_collab_types::value::{AlertActionKind, CallParticipantState, MessageBody, TransferState};
use mde_collab_types::{
    ActorClock, CollabCommand, CollabEventEnvelope, PresenceState, SpaceRole, TransferControl,
};

use crate::domain::DomainState;
use crate::error::{CollabError, Result};
use crate::signer::{EventSigner, IdSource};

/// The author edit/delete window: 5 minutes, in milliseconds.
pub const EDIT_WINDOW_MS: i64 = 5 * 60 * 1000;

/// The injected authoring context for [`apply_command`]. Carries the local
/// actor, the injected wall time, the actor's running HLC (advanced per emitted
/// event), and the signer + id source. Generic (not `dyn`) so a hot path stays
/// monomorphized.
pub struct ApplyCtx<'a, S: EventSigner, I: IdSource> {
    /// The local actor authoring the command.
    pub actor: mde_collab_types::ActorId,
    /// The injected creation time for the events, epoch ms.
    pub now_unix_ms: i64,
    /// The actor's current HLC. Read on entry, advanced (`tick`) once per emitted
    /// event; on return it holds the last event's clock.
    pub clock: ActorClock,
    /// The signer for the authored events.
    pub signer: &'a S,
    /// The id source for the authored events.
    pub ids: &'a mut I,
}

impl<'a, S: EventSigner, I: IdSource> ApplyCtx<'a, S, I> {
    /// A context for `actor` at injected time `now_unix_ms`, starting from the
    /// zero clock. Use [`with_clock`](Self::with_clock) to resume an existing
    /// actor clock.
    pub fn new(
        actor: impl Into<mde_collab_types::ActorId>,
        now_unix_ms: i64,
        signer: &'a S,
        ids: &'a mut I,
    ) -> Self {
        Self {
            actor: actor.into(),
            now_unix_ms,
            clock: ActorClock::zero(),
            signer,
            ids,
        }
    }

    /// Resume from an existing actor clock (the engine's stored high-water).
    #[must_use]
    pub fn with_clock(mut self, clock: ActorClock) -> Self {
        self.clock = clock;
        self
    }

    /// Mint, HLC-stamp, and sign one envelope for `kind` in `space`.
    fn emit(&mut self, space: SpaceId, kind: CollabEventKind) -> CollabEventEnvelope {
        let now = u64::try_from(self.now_unix_ms).unwrap_or(0);
        self.clock = self.clock.tick(now);
        let id = self.ids.next_event_id();
        let mut env = CollabEventEnvelope::new(
            id,
            space,
            self.actor.clone(),
            self.clock,
            self.now_unix_ms,
            kind,
        );
        self.signer.sign(&mut env);
        env
    }
}

/// Validate `cmd` against `state` and, on success, return the signed
/// event(s) it produces. A rejected command returns a typed [`CollabError`].
///
/// A few commands intentionally produce **zero** events (they carry no
/// convergent fact for the log): the ephemeral in-call media-plane signals
/// ([`SendDtmf`](CollabCommand::SendDtmf),
/// [`SetCallMuted`](CollabCommand::SetCallMuted)), the local-seat notification
/// preferences ([`SetAlertMute`](CollabCommand::SetAlertMute),
/// [`SetSeverityThreshold`](CollabCommand::SetSeverityThreshold)), and the
/// AI-suggestion *request* (the offer is emitted later by the worker once the
/// model answers — [`RequestAiSuggestion`](CollabCommand::RequestAiSuggestion)).
/// These still validate (membership/existence) and are documented Phase-1
/// follow-ups where a Phase-0 event class does not yet carry the fact.
#[allow(clippy::too_many_lines)]
pub fn apply_command<S: EventSigner, I: IdSource>(
    state: &DomainState,
    cmd: &CollabCommand,
    ctx: &mut ApplyCtx<'_, S, I>,
) -> Result<Vec<CollabEventEnvelope>> {
    match cmd {
        // ---- Space lifecycle -------------------------------------------
        CollabCommand::CreateSpace { kind, name } => {
            // A fresh space; the creator becomes its first Owner. Two events.
            let space = SpaceId::new();
            let created = ctx.emit(
                space,
                CollabEventKind::SpaceCreated {
                    kind: *kind,
                    name: name.clone(),
                },
            );
            let joined = ctx.emit(
                space,
                CollabEventKind::MemberJoined {
                    actor: ctx.actor.clone(),
                    role: SpaceRole::Owner,
                },
            );
            Ok(vec![created, joined])
        }
        CollabCommand::RenameSpace { space, name } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::SpaceRenamed { name: name.clone() },
            )])
        }
        CollabCommand::DeleteSpace { space } => {
            require_active_space(state, *space)?;
            require_owner(state, *space, &ctx.actor, "delete_space")?;
            // Direct deletion (not archive-first): a convergent space tombstone.
            Ok(vec![ctx.emit(*space, CollabEventKind::SpaceDeleted)])
        }

        // ---- Membership + presence -------------------------------------
        CollabCommand::AddMember { space, actor, role } => {
            require_active_space(state, *space)?;
            require_owner(state, *space, &ctx.actor, "add_member")?;
            if state.is_member(*space, actor) {
                return Err(CollabError::AlreadyMember {
                    space: *space,
                    actor: actor.clone(),
                });
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MemberJoined {
                    actor: actor.clone(),
                    role: *role,
                },
            )])
        }
        CollabCommand::RemoveMember { space, actor } => {
            require_active_space(state, *space)?;
            require_owner(state, *space, &ctx.actor, "remove_member")?;
            if !state.is_member(*space, actor) {
                return Err(CollabError::NotPresent {
                    space: *space,
                    actor: actor.clone(),
                });
            }
            // Never orphan a space: removing the last present Owner is denied.
            if would_orphan(state, *space, actor) {
                return Err(CollabError::LastOwner {
                    space: *space,
                    action: "remove_member",
                });
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MemberLeft {
                    actor: actor.clone(),
                },
            )])
        }
        CollabCommand::SetMemberRole { space, actor, role } => {
            require_active_space(state, *space)?;
            require_owner(state, *space, &ctx.actor, "set_member_role")?;
            if !state.is_member(*space, actor) {
                return Err(CollabError::NotPresent {
                    space: *space,
                    actor: actor.clone(),
                });
            }
            // Demoting the last Owner would orphan the space.
            if matches!(role, SpaceRole::Member) && would_orphan(state, *space, actor) {
                return Err(CollabError::LastOwner {
                    space: *space,
                    action: "set_member_role",
                });
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MemberRoleChanged {
                    actor: actor.clone(),
                    role: *role,
                },
            )])
        }
        CollabCommand::JoinSpace { space } => {
            require_active_space(state, *space)?;
            if state.is_member(*space, &ctx.actor) {
                return Err(CollabError::AlreadyMember {
                    space: *space,
                    actor: ctx.actor.clone(),
                });
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MemberJoined {
                    actor: ctx.actor.clone(),
                    role: SpaceRole::Member,
                },
            )])
        }
        CollabCommand::LeaveSpace { space } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if would_orphan(state, *space, &ctx.actor) {
                return Err(CollabError::LastOwner {
                    space: *space,
                    action: "leave_space",
                });
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MemberLeft {
                    actor: ctx.actor.clone(),
                },
            )])
        }
        CollabCommand::SetPresence { presence, status } => {
            // Presence is fleet-global — carried on the nil "global" space lane.
            Ok(vec![ctx.emit(
                SpaceId::nil(),
                CollabEventKind::PresenceChanged {
                    actor: ctx.actor.clone(),
                    presence: *presence,
                    status: status.clone(),
                },
            )])
        }
        CollabCommand::SetDoNotDisturb { enabled } => {
            let presence = if *enabled {
                PresenceState::Dnd
            } else {
                PresenceState::Online
            };
            Ok(vec![ctx.emit(
                SpaceId::nil(),
                CollabEventKind::PresenceChanged {
                    actor: ctx.actor.clone(),
                    presence,
                    status: None,
                },
            )])
        }

        // ---- Messages + threads ----------------------------------------
        CollabCommand::SendMessage {
            space,
            thread,
            body,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if let Some(t) = thread {
                if state.threads.get(t) != Some(space) {
                    return Err(CollabError::ThreadNotFound(*t));
                }
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessagePosted {
                    body: body.clone(),
                    thread: *thread,
                },
            )])
        }
        CollabCommand::EditMessage {
            space,
            target,
            body,
        } => {
            require_active_space(state, *space)?;
            let msg = require_message(state, *space, *target)?;
            if msg.author != ctx.actor {
                return Err(CollabError::NotAuthor(*target));
            }
            if msg.deleted {
                return Err(CollabError::TargetDeleted(*target));
            }
            enforce_window(*target, msg.created_ms, ctx.now_unix_ms)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessageEdited {
                    target: *target,
                    body: body.clone(),
                },
            )])
        }
        CollabCommand::DeleteMessage { space, target } => {
            require_active_space(state, *space)?;
            let msg = require_message(state, *space, *target)?;
            if msg.author != ctx.actor {
                return Err(CollabError::NotAuthor(*target));
            }
            if msg.deleted {
                // Deleting an already-deleted message is a visible no-op error.
                return Err(CollabError::TargetDeleted(*target));
            }
            enforce_window(*target, msg.created_ms, ctx.now_unix_ms)?;
            // A convergent message tombstone.
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessageDeleted { target: *target },
            )])
        }
        CollabCommand::StartThread { space, root, title } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_message(state, *space, *root)?;
            let thread = mde_collab_types::ids::ThreadId::new();
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ThreadStarted {
                    thread,
                    root: *root,
                    title: title.clone(),
                },
            )])
        }
        CollabCommand::ReplyInThread {
            space,
            thread,
            body,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if state.threads.get(thread) != Some(space) {
                return Err(CollabError::ThreadNotFound(*thread));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessagePosted {
                    body: body.clone(),
                    thread: Some(*thread),
                },
            )])
        }

        // ---- Alerts ----------------------------------------------------
        CollabCommand::AckAlert { space, alert } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_alert(state, *space, *alert)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::AlertAcknowledged { target: *alert },
            )])
        }
        CollabCommand::SnoozeAlert {
            space,
            alert,
            until_unix_ms,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_alert(state, *space, *alert)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::AlertSnoozed {
                    target: *alert,
                    until_unix_ms: *until_unix_ms,
                },
            )])
        }
        CollabCommand::RunAlertAction {
            space,
            alert,
            action_id,
            armed,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            let entry = state
                .alerts
                .get(alert)
                .ok_or(CollabError::AlertNotFound(*alert))?;
            if entry.0 != *space {
                return Err(CollabError::AlertNotFound(*alert));
            }
            let kind = entry
                .1
                .get(action_id)
                .ok_or_else(|| CollabError::ActionNotFound {
                    alert: *alert,
                    action_id: action_id.clone(),
                })?;
            if matches!(kind, AlertActionKind::Destructive) && !*armed {
                return Err(CollabError::DestructiveNotArmed {
                    alert: *alert,
                    action_id: action_id.clone(),
                });
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::AlertActionInvoked {
                    target: *alert,
                    action_id: action_id.clone(),
                    armed: *armed,
                    outcome: Some("fired".to_string()),
                },
            )])
        }
        // Local-seat notification preferences — not convergent log facts.
        // WL-FUNC-011 Phase 1 follow-up: persist these in a per-seat local
        // settings store (not the replicated space log) in the worker.
        CollabCommand::SetAlertMute { .. } | CollabCommand::SetSeverityThreshold { .. } => {
            Ok(Vec::new())
        }

        // ---- Clipboard -------------------------------------------------
        CollabCommand::PublishClipboard { space, item } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ClipboardPublished { item: item.clone() },
            )])
        }
        CollabCommand::AttachClipboard { space, clip } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_clip(state, *space, *clip)?;
            // Re-share the clip as a message referencing it.
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessagePosted {
                    body: MessageBody::new(format!("shared clipboard item `{clip}`")),
                    thread: None,
                },
            )])
        }
        CollabCommand::PinClipboard { space, clip } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_clip(state, *space, *clip)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ClipboardPinned { target: *clip },
            )])
        }
        CollabCommand::UnpinClipboard { space, clip } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_clip(state, *space, *clip)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ClipboardUnpinned { target: *clip },
            )])
        }
        CollabCommand::DeleteClipboard { space, clip } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_clip(state, *space, *clip)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ClipboardDeleted { target: *clip },
            )])
        }
        CollabCommand::ClearClipboard { space } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            // Clear every unpinned, not-already-deleted clip in the space — one
            // convergent tombstone each. Pinned clips survive.
            let mut targets: Vec<mde_collab_types::ids::EventId> = state
                .clips
                .iter()
                .filter(|(_, c)| c.space == *space && !c.deleted && !c.pinned)
                .map(|(id, _)| *id)
                .collect();
            targets.sort();
            Ok(targets
                .into_iter()
                .map(|clip| ctx.emit(*space, CollabEventKind::ClipboardDeleted { target: clip }))
                .collect())
        }

        // ---- Documents + reviews ---------------------------------------
        CollabCommand::CreateDocument {
            space,
            document,
            title,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::DocumentCreated {
                    document: *document,
                    title: title.clone(),
                },
            )])
        }
        CollabCommand::UpdateDocument {
            space,
            document,
            change,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if state.documents.get(document) != Some(space) {
                return Err(CollabError::DocumentNotFound(*document));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::DocumentUpdated {
                    document: *document,
                    change: change.clone(),
                },
            )])
        }
        CollabCommand::RequestReview {
            space,
            document,
            reviewers,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if state.documents.get(document) != Some(space) {
                return Err(CollabError::DocumentNotFound(*document));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ReviewRequested {
                    document: *document,
                    reviewers: reviewers.clone(),
                },
            )])
        }
        CollabCommand::SubmitReview {
            space,
            document,
            verdict,
            comment,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if state.documents.get(document) != Some(space) {
                return Err(CollabError::DocumentNotFound(*document));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ReviewSubmitted {
                    document: *document,
                    verdict: *verdict,
                    comment: comment.clone(),
                },
            )])
        }

        // ---- File references -------------------------------------------
        CollabCommand::LinkFile {
            space,
            file,
            reference,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::FileLinked {
                    file: *file,
                    reference: reference.clone(),
                },
            )])
        }
        CollabCommand::UnlinkFile { space, file } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if !file_present(state, *space, *file) {
                return Err(CollabError::FileNotFound(*file));
            }
            // Unlinks the reference (a link tombstone); the canonical file's
            // content-addressed bytes are NOT purged by this.
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::FileUnlinked { file: *file },
            )])
        }

        // ---- Transfers -------------------------------------------------
        CollabCommand::StartTransfer {
            space,
            transfer,
            file,
            method,
            direction,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if !file_present(state, *space, *file) {
                return Err(CollabError::FileNotFound(*file));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::TransferStarted {
                    transfer: *transfer,
                    file: *file,
                    method: *method,
                    direction: *direction,
                },
            )])
        }
        CollabCommand::ControlTransfer { transfer, control } => {
            let space = *state
                .transfers
                .get(transfer)
                .ok_or(CollabError::TransferNotFound(*transfer))?;
            let new_state = match control {
                TransferControl::Pause => TransferState::Paused,
                TransferControl::Resume => TransferState::Active,
                TransferControl::Cancel => TransferState::Canceled,
            };
            Ok(vec![ctx.emit(
                space,
                CollabEventKind::TransferStateChanged {
                    transfer: *transfer,
                    state: new_state,
                },
            )])
        }

        // ---- Calls -----------------------------------------------------
        CollabCommand::StartCall { space, call, kind } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::CallStarted {
                    call: *call,
                    kind: *kind,
                    initiator: ctx.actor.clone(),
                },
            )])
        }
        CollabCommand::AnswerCall { call } => {
            let space = require_call(state, *call)?;
            Ok(vec![ctx.emit(
                space,
                CollabEventKind::CallParticipantChanged {
                    call: *call,
                    actor: ctx.actor.clone(),
                    state: CallParticipantState::Connected,
                },
            )])
        }
        CollabCommand::DeclineCall { call } => {
            let space = require_call(state, *call)?;
            Ok(vec![ctx.emit(
                space,
                CollabEventKind::CallParticipantChanged {
                    call: *call,
                    actor: ctx.actor.clone(),
                    state: CallParticipantState::Declined,
                },
            )])
        }
        CollabCommand::HangUpCall { call } => {
            let space = require_call(state, *call)?;
            let left = ctx.emit(
                space,
                CollabEventKind::CallParticipantChanged {
                    call: *call,
                    actor: ctx.actor.clone(),
                    state: CallParticipantState::Left,
                },
            );
            let mut events = vec![left];
            // If no other participant remains Connected, the call ends.
            let others_connected = state
                .calls
                .get(call)
                .map(|c| {
                    c.participants.iter().any(|(a, s)| {
                        a != &ctx.actor && matches!(s, CallParticipantState::Connected)
                    })
                })
                .unwrap_or(false);
            if !others_connected {
                events.push(ctx.emit(
                    space,
                    CollabEventKind::CallEnded {
                        call: *call,
                        reason: Some("hung_up".to_string()),
                    },
                ));
            }
            Ok(events)
        }
        // Ephemeral in-call media-plane signals — no convergent log fact.
        // WL-FUNC-011 Phase 1 follow-up: the worker forwards DTMF + mute over
        // the live RTP/media plane; the Phase-0 event taxonomy carries neither
        // (CallParticipantChanged has no muted/dtmf field).
        CollabCommand::SendDtmf { call, .. } | CollabCommand::SetCallMuted { call, .. } => {
            require_call(state, *call)?;
            Ok(Vec::new())
        }

        // ---- AI --------------------------------------------------------
        // A request produces no event; the worker calls the model and emits the
        // AiSuggestionOffered event when the answer arrives.
        // WL-FUNC-011 Phase 1 follow-up: the model call + offer emission is the
        // Phase-2 worker's async flow.
        CollabCommand::RequestAiSuggestion { space, .. } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            Ok(Vec::new())
        }
    }
}

/// The space must exist and not be deleted.
fn require_active_space(state: &DomainState, space: SpaceId) -> Result<()> {
    match state.space(space) {
        None => Err(CollabError::SpaceNotFound(space)),
        Some(s) if s.deleted => Err(CollabError::SpaceDeleted(space)),
        Some(_) => Ok(()),
    }
}

/// The actor must be a present member of the space.
fn require_member(
    state: &DomainState,
    space: SpaceId,
    actor: &mde_collab_types::ActorId,
) -> Result<()> {
    if state.is_member(space, actor) {
        Ok(())
    } else {
        Err(CollabError::NotMember {
            space,
            actor: actor.clone(),
        })
    }
}

/// The actor must be a present Owner of the space.
fn require_owner(
    state: &DomainState,
    space: SpaceId,
    actor: &mde_collab_types::ActorId,
    action: &'static str,
) -> Result<()> {
    // A non-member surfaces as NotMember; a member-but-not-Owner as OwnerRequired.
    require_member(state, space, actor)?;
    if state.is_owner(space, actor) {
        Ok(())
    } else {
        Err(CollabError::OwnerRequired { space, action })
    }
}

/// The message must exist in this space.
fn require_message(
    state: &DomainState,
    space: SpaceId,
    target: mde_collab_types::ids::EventId,
) -> Result<crate::domain::MessageAgg> {
    match state.messages.get(&target) {
        Some(m) if m.space == space => Ok(m.clone()),
        _ => Err(CollabError::MessageNotFound(target)),
    }
}

/// The alert must exist in this space.
fn require_alert(
    state: &DomainState,
    space: SpaceId,
    alert: mde_collab_types::ids::EventId,
) -> Result<()> {
    match state.alerts.get(&alert) {
        Some((s, _)) if *s == space => Ok(()),
        _ => Err(CollabError::AlertNotFound(alert)),
    }
}

/// The clip must exist (and not be deleted) in this space.
fn require_clip(
    state: &DomainState,
    space: SpaceId,
    clip: mde_collab_types::ids::EventId,
) -> Result<()> {
    match state.clips.get(&clip) {
        Some(c) if c.space == space && !c.deleted => Ok(()),
        _ => Err(CollabError::ClipNotFound(clip)),
    }
}

/// Whether `file` is a currently-present (linked, not unlinked) reference in
/// `space`.
fn file_present(
    state: &DomainState,
    space: SpaceId,
    file: mde_collab_types::ids::FileRefId,
) -> bool {
    match state.files.get(&file) {
        Some((s, present)) => *present && *s == space,
        None => false,
    }
}

/// The call must exist; returns its space.
fn require_call(state: &DomainState, call: mde_collab_types::ids::CallId) -> Result<SpaceId> {
    state
        .calls
        .get(&call)
        .map(|c| c.space)
        .ok_or(CollabError::CallNotFound(call))
}

/// The author edit/delete window guard.
fn enforce_window(
    target: mde_collab_types::ids::EventId,
    created_ms: i64,
    now_ms: i64,
) -> Result<()> {
    let age = now_ms.saturating_sub(created_ms);
    if age > EDIT_WINDOW_MS {
        Err(CollabError::EditWindowExpired {
            target,
            age_ms: age,
            window_ms: EDIT_WINDOW_MS,
        })
    } else {
        Ok(())
    }
}

/// Whether removing/demoting `actor` from `space` would leave it Owner-less.
fn would_orphan(state: &DomainState, space: SpaceId, actor: &mde_collab_types::ActorId) -> bool {
    match state.space(space) {
        Some(s) => {
            matches!(state.role(space, actor), Some(SpaceRole::Owner))
                && s.present_owner_count() <= 1
        }
        None => false,
    }
}
