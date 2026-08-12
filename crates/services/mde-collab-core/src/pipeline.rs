//! The command → signed-events pipeline: [`apply_command`].
//!
//! It validates a [`CollabCommand`] against the folded [`DomainState`]
//! (membership, Owner/Member permission, the 5-minute author edit/delete
//! window, entity existence, destructive-action arming), and on success mints,
//! HLC-stamps, and **signs** one or more [`CollabEventEnvelope`]s via the
//! injected [`EventSigner`] + [`IdSource`]. A rejected command returns a typed
//! [`CollabError`] — the denial is *visible*, never a silent no-op.
//!
//! [`apply_command`] itself performs no I/O and reads no wall clock: time, ids,
//! and signing are all supplied through [`ApplyCtx`], so the same command
//! replays to byte-identical events. [`ingest_and_register_file`] is the narrow
//! production orchestration boundary which couples verified CAS installation
//! to the normal authorized command path.

use std::io::Read;

use mde_collab_types::event::CollabEventKind;
use mde_collab_types::ids::{EventId, FileRefId, SpaceId};
use mde_collab_types::value::{
    AlertActionKind, CallParticipantState, FileRef, MessageBody, PayloadRef, TransferState,
};
use mde_collab_types::{
    ActorClock, CollabCommand, CollabEventEnvelope, FileReferenceView, PresenceState, SpaceRole,
    TransferControl, MAX_TRANSFER_CONTENT_BYTES,
};

use crate::blob::FsBlobStore;
use crate::domain::DomainState;
use crate::engine::CollabEngine;
use crate::error::{CollabError, Result};
use crate::signer::{EventSigner, IdSource};

/// The author edit/delete window: 5 minutes, in milliseconds.
pub const EDIT_WINDOW_MS: i64 = 5 * 60 * 1000;

/// The maximum UTF-8 byte length of an inline message body.
///
/// This mirrors the collaboration projection's existing 256 KiB message-body
/// contract. Enforcing it here keeps oversized user input from being signed
/// and materialized into an event that the read model would later reject.
pub const MAX_MESSAGE_BODY_BYTES: usize = 256 * 1024;

/// The maximum UTF-8 byte length of a basic channel task title.
pub const MAX_TASK_TITLE_BYTES: usize = 512;

/// The maximum UTF-8 byte length for document titles, change summaries, and
/// review comments stored in the collaboration read model.
pub const MAX_DOCUMENT_TEXT_BYTES: usize = 64 * 1024;

/// Maximum tombstones a `ClearClipboard` command may author.
///
/// The mesh clipboard history already retains at most 50 unpinned entries.
/// This keeps the command boundary aligned with the existing history contract
/// while preventing a malformed or legacy aggregate from creating an unbounded
/// event batch.
const MAX_CLEAR_CLIPBOARD_EVENTS: usize = 50;

/// The largest accepted AI sidecar request id.
///
/// The worker publishes this id in `state/collab/ai-requests`; keep it a small
/// single token so it never becomes layout, path, or topic material.
pub const MAX_AI_REQUEST_ID_BYTES: usize = 128;

/// The largest accepted inline alert action id. Action ids are signed into
///
/// invocation events and may be consumed by downstream verb/topic adapters;
/// keep them bounded and token-shaped at the authority boundary.
pub const MAX_ALERT_ACTION_ID_BYTES: usize = 128;

/// A Files identity returned only after its signed `LinkFile` event and exact
/// read-side row have committed successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredFile {
    /// The caller-provided stable Files identity.
    pub file: FileRefId,
    /// The exact durable projection confirmed after command application.
    pub projection: FileReferenceView,
    /// Signed events produced by the normal command path. The runtime worker
    /// durably appends these before publishing them on the live lane.
    pub events: Vec<CollabEventEnvelope>,
}

/// Verify and install bytes in CAS, then register their stable Files identity
/// through the engine's normal authenticated `LinkFile` command path.
///
/// `engine.actor()` is the authenticated actor. The operation accepts only an
/// existing space where that actor is a present member because authorization is
/// performed by [`CollabEngine::apply`], not by a parallel policy path. The CAS
/// commit guard remains armed across command application: authorization or
/// projection failure rolls back only an inode installed by this operation,
/// while an exact pre-existing/concurrent blob is never removed.
///
/// # Errors
///
/// Returns an error if staging, authorization, projection, or event persistence
/// fails. A newly-installed blob is rolled back when later command processing
/// fails.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::needless_pass_by_value)] // Public API owns the caller's `FileRef`.
pub fn ingest_and_register_file<R: Read, S: EventSigner, I: IdSource>(
    engine: &mut CollabEngine,
    blobs: &FsBlobStore,
    space: SpaceId,
    file: FileRefId,
    reference: FileRef,
    reader: R,
    signer: &S,
    ids: &mut I,
    now_unix_ms: i64,
) -> Result<RegisteredFile> {
    let payload = PayloadRef {
        sha256_hex: reference.sha256_hex.clone(),
        len: reference.size,
        content_type: reference.mime.clone(),
    };
    let stage = blobs.stage(reader, &payload)?;
    let commit = stage.commit()?;
    let linked_by = engine.actor().clone();
    let command = CollabCommand::LinkFile {
        space,
        file,
        reference: reference.clone(),
    };

    let events = match engine.apply(&command, signer, ids, now_unix_ms) {
        Ok(events) => events,
        Err(apply_error) => {
            return match commit.abort() {
                Ok(()) => Err(apply_error),
                Err(cleanup_error) => Err(CollabError::Io(format!(
                    "file registration failed ({apply_error}); CAS rollback also failed ({cleanup_error})"
                ))),
            };
        }
    };

    // `apply` returns only after the SQLite projection transaction commits.
    // Query that exact durable row before reporting a usable Files identity.
    let confirmation = engine.projection().file_references(space).and_then(|rows| {
        rows.files
            .into_iter()
            .find(|row| {
                row.file == file
                    && row.reference == reference
                    && row.linked_by == linked_by
                    && row.linked_unix_ms == now_unix_ms
            })
            .ok_or_else(|| {
                CollabError::Io(format!(
                    "LinkFile projection did not confirm exact file identity {file}"
                ))
            })
    });

    // A successful `apply` means the durable signed event and projection now
    // reference these bytes. Preserve CAS even if the confirmation read itself
    // encounters a later SQLite error; deleting it would strand durable state.
    let _ = commit.retain();
    let projection = confirmation?;
    Ok(RegisteredFile {
        file,
        projection,
        events,
    })
}

/// The injected authoring context for [`apply_command`]. Carries the local
/// actor, the injected wall time, the actor's running HLC (advanced per emitted
/// event), and the signer + id source.
///
/// Generic (not `dyn`) so a hot path stays
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
    pub const fn with_clock(mut self, clock: ActorClock) -> Self {
        self.clock = clock;
        self
    }

    /// Author one worker-adapted event directly, with no originating command:
    /// mint, HLC-stamp, and sign it. This is the fold path for event classes that
    /// have **no** authoring command — [`AlertRaised`](CollabEventKind::AlertRaised)
    /// and [`ClipboardPublished`](CollabEventKind::ClipboardPublished) adapted from
    /// external Bus lanes (the emitters keep publishing their truthful events; the
    /// collab worker adapts them into a signed collab fact here). It performs no
    /// validation — the caller vouches for the fact (it originates from a trusted
    /// local subsystem, not a peer command).
    pub fn author(&mut self, space: SpaceId, kind: CollabEventKind) -> CollabEventEnvelope {
        self.emit(space, kind)
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
/// convergent fact for the log): the ephemeral in-call media-plane signal
/// ([`SendDtmf`](CollabCommand::SendDtmf)), the local-seat notification
/// preferences ([`SetAlertMute`](CollabCommand::SetAlertMute),
/// [`SetSeverityThreshold`](CollabCommand::SetSeverityThreshold)), and the
/// AI-suggestion *request/cancel* sidecar verbs (the offer is emitted later by
/// the worker once the model answers — [`RequestAiSuggestion`](CollabCommand::RequestAiSuggestion)).
/// These still validate (membership/existence) and are documented Phase-1
/// follow-ups where a Phase-0 event class does not yet carry the fact.
///
/// # Errors
///
/// Returns a typed error when command validation, signing, or ID generation
/// cannot produce an accepted event sequence.
#[allow(clippy::too_many_lines)]
pub fn apply_command<S: EventSigner, I: IdSource>(
    state: &DomainState,
    cmd: &CollabCommand,
    ctx: &mut ApplyCtx<'_, S, I>,
) -> Result<Vec<CollabEventEnvelope>> {
    let events: Result<Vec<CollabEventEnvelope>> = match cmd {
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
            validate_message_body(body)?;
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
            // Authorship is not a standing capability: a departed author may
            // not mint another message mutation after leaving the space.
            require_member(state, *space, &ctx.actor)?;
            let msg = require_message(state, *space, *target)?;
            if msg.author != ctx.actor {
                return Err(CollabError::NotAuthor(*target));
            }
            if msg.deleted {
                return Err(CollabError::TargetDeleted(*target));
            }
            enforce_window(*target, msg.created_ms, ctx.now_unix_ms)?;
            validate_message_body(body)?;
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
            // Keep deletion under the same current-membership boundary as
            // editing; the author check below alone is historical identity.
            require_member(state, *space, &ctx.actor)?;
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
        CollabCommand::PinMessage { space, target } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            let message = require_message(state, *space, *target)?;
            if message.deleted {
                return Err(CollabError::TargetDeleted(*target));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessagePinned { target: *target },
            )])
        }
        CollabCommand::UnpinMessage { space, target } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_message(state, *space, *target)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessageUnpinned { target: *target },
            )])
        }
        CollabCommand::SaveMessage { space, target } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            let message = require_message(state, *space, *target)?;
            if message.deleted {
                return Err(CollabError::TargetDeleted(*target));
            }
            if state.is_message_saved(&ctx.actor, *target) {
                return Err(CollabError::MessageAlreadySaved(*target));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessageSaved { target: *target },
            )])
        }
        CollabCommand::UnsaveMessage { space, target } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_message(state, *space, *target)?;
            if !state.is_message_saved(&ctx.actor, *target) {
                return Err(CollabError::MessageNotSaved(*target));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessageUnsaved { target: *target },
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
            validate_message_body(body)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::MessagePosted {
                    body: body.clone(),
                    thread: Some(*thread),
                },
            )])
        }
        CollabCommand::ResolveThread { space, thread } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if state.threads.get(thread) != Some(space) {
                return Err(CollabError::ThreadNotFound(*thread));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ThreadResolved { thread: *thread },
            )])
        }
        CollabCommand::ReopenThread { space, thread } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            if state.threads.get(thread) != Some(space) {
                return Err(CollabError::ThreadNotFound(*thread));
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::ThreadReopened { thread: *thread },
            )])
        }
        CollabCommand::CreateTask {
            space,
            title,
            source,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            validate_task_title(title)?;
            if let Some(source) = source {
                let source_message = require_message(state, *space, *source)?;
                if source_message.deleted {
                    return Err(CollabError::TargetDeleted(*source));
                }
            }
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::TaskCreated {
                    title: title.trim().to_owned(),
                    source: *source,
                },
            )])
        }
        CollabCommand::UpdateTask { space, task, title } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_open_task(state, *space, *task)?;
            validate_task_title(title)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::TaskUpdated {
                    task: *task,
                    title: title.trim().to_owned(),
                },
            )])
        }
        CollabCommand::SetTaskChecked {
            space,
            task,
            checked,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_open_task(state, *space, *task)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::TaskChecked {
                    task: *task,
                    checked: *checked,
                },
            )])
        }
        CollabCommand::CompleteTask { space, task } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_open_task(state, *space, *task)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::TaskCompleted { task: *task },
            )])
        }
        CollabCommand::ReopenTask { space, task } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_completed_task(state, *space, *task)?;
            Ok(vec![ctx.emit(
                *space,
                CollabEventKind::TaskReopened { task: *task },
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
            require_alert_action_id(action_id)?;
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
        CollabCommand::PublishClipboard { space, item, .. } => {
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
                // Collect only one item over the permitted batch size. This
                // makes the refusal itself bounded even if a hostile aggregate
                // contains an arbitrarily large number of eligible clips.
                .take(MAX_CLEAR_CLIPBOARD_EVENTS + 1)
                .collect();
            if targets.len() > MAX_CLEAR_CLIPBOARD_EVENTS {
                return Err(CollabError::Serde(format!(
                    "clear_clipboard fan-out exceeds {MAX_CLEAR_CLIPBOARD_EVENTS} events"
                )));
            }
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
            validate_document_text(title, "document title")?;
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
            if let Some(summary) = &change.summary {
                validate_document_text(summary, "document change summary")?;
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
            if let Some(comment) = comment {
                validate_document_text(comment, "review comment")?;
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
        CollabCommand::CommitFileGeneration {
            space,
            file,
            expected_generation,
            expected_sha256_hex,
            expected_size,
            reference,
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            let current = state
                .files
                .get(file)
                .filter(|current| current.space == *space && current.present)
                .ok_or(CollabError::FileNotFound(*file))?;
            if current.generation != *expected_generation
                || current.reference.sha256_hex != *expected_sha256_hex
                || current.reference.size != *expected_size
            {
                return Err(CollabError::FileGenerationConflict {
                    file: *file,
                    expected_generation: *expected_generation,
                    current_generation: current.generation,
                });
            }
            if ctx.now_unix_ms <= current.generation {
                return Err(CollabError::FileGenerationDidNotAdvance {
                    file: *file,
                    current_generation: current.generation,
                    proposed_generation: ctx.now_unix_ms,
                });
            }
            if reference.name != current.reference.name || reference.mime != current.reference.mime
            {
                return Err(CollabError::FileGenerationMetadataMutation(*file));
            }
            if reference.size > MAX_TRANSFER_CONTENT_BYTES
                || !is_nonzero_lower_sha256(&reference.sha256_hex)
                || reference.sha256_hex == current.reference.sha256_hex
            {
                return Err(CollabError::InvalidFileGeneration(*file));
            }
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
            let current = state
                .transfers
                .get(transfer)
                .ok_or(CollabError::TransferNotFound(*transfer))?;
            let space = current.space;
            // Transfer controls are shared space facts.  Without this guard,
            // any actor who had learned a transfer id could mint a pause,
            // resume, or cancel event for a space they do not belong to.
            require_active_space(state, space)?;
            require_member(state, space, &ctx.actor)?;
            let new_state = next_transfer_state(*transfer, current.state, *control)?;
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
            // A call id is discoverable replicated state, not authority to
            // mint a participant lifecycle event.  Only current members of
            // the call's space may answer it.
            require_member(state, space, &ctx.actor)?;
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
            // Declining changes the shared participant read model just like
            // answering; apply the same space-membership boundary.
            require_member(state, space, &ctx.actor)?;
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
            // A hang-up changes shared participant state and may end the call;
            // knowing a call id is not sufficient authority to mint either fact.
            let space = require_active_call_participant(state, *call, &ctx.actor)?;
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
            let others_connected = state.calls.get(call).is_some_and(|c| {
                c.participants
                    .iter()
                    .any(|(a, s)| a != &ctx.actor && matches!(s, CallParticipantState::Connected))
            });
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
        // DTMF remains an ephemeral in-call media-plane signal. It still has to
        // pass the same connected-participant boundary as convergent media
        // controls, and the tone must be representable as an RFC 4733
        // telephone-event before a future SIP/WebRTC adapter ever sees it.
        // Mute is a signed, convergent call-state fact so every peer can render
        // the same participant read model after replay.
        CollabCommand::SendDtmf { call, digit } => {
            require_active_call_participant(state, *call, &ctx.actor)?;
            require_dtmf_digit(*digit)?;
            Ok(Vec::new())
        }
        CollabCommand::SetCallMuted { call, muted } => {
            let space = require_active_call_participant(state, *call, &ctx.actor)?;
            Ok(vec![ctx.emit(
                space,
                CollabEventKind::CallParticipantMuted {
                    call: *call,
                    actor: ctx.actor.clone(),
                    muted: *muted,
                },
            )])
        }

        // ---- AI --------------------------------------------------------
        // A request produces no event; the worker calls the model and emits the
        // AiSuggestionOffered event when the answer arrives.
        // WL-FUNC-011 Phase 1 follow-up: the model call + offer emission is the
        // Phase-2 worker's async flow.
        CollabCommand::RequestAiSuggestion {
            space,
            request_id,
            target,
            ..
        } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_ai_request_id(request_id)?;
            if let Some(target) = target {
                require_ai_target_in_space(state, *space, *target)?;
            }
            Ok(Vec::new())
        }
        CollabCommand::CancelAiSuggestion { space, request_id } => {
            require_active_space(state, *space)?;
            require_member(state, *space, &ctx.actor)?;
            require_ai_request_id(request_id)?;
            Ok(Vec::new())
        }
    };
    let events = events?;

    if let Some(invalid) = events.iter().find(|event| !event.verify()) {
        return Err(CollabError::InvalidEvent(invalid.event_id));
    }
    Ok(events)
}

fn is_nonzero_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().any(|byte| byte != b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Mirror the UI/read-model transfer control contract at the authoritative
/// command boundary. Queued transfers may only be canceled; active transfers
/// may be paused/canceled; paused transfers may be resumed/canceled; terminal
/// states carry no controls.
const fn next_transfer_state(
    transfer: mde_collab_types::ids::TransferId,
    state: TransferState,
    control: TransferControl,
) -> Result<TransferState> {
    let next = match (state, control) {
        (TransferState::Active, TransferControl::Pause) => TransferState::Paused,
        (TransferState::Paused, TransferControl::Resume) => TransferState::Active,
        (
            TransferState::Queued | TransferState::Active | TransferState::Paused,
            TransferControl::Cancel,
        ) => TransferState::Canceled,
        _ => {
            return Err(CollabError::InvalidTransferControl {
                transfer,
                state,
                control,
            });
        }
    };
    Ok(next)
}

/// The space must exist and not be deleted.
fn require_active_space(state: &DomainState, space: SpaceId) -> Result<()> {
    match state.space(space) {
        None => Err(CollabError::SpaceNotFound(space)),
        Some(s) if s.deleted => Err(CollabError::SpaceDeleted(space)),
        Some(_) => Ok(()),
    }
}
/// Reject an oversized inline body before [`ApplyCtx::emit`] can consume an id
/// or HLC tick. The projection has the same cap when it builds message views;
/// keeping the command boundary aligned avoids signing events that can never be
/// represented by the read model.
fn validate_message_body(body: &MessageBody) -> Result<()> {
    if body.as_str().len() > MAX_MESSAGE_BODY_BYTES {
        return Err(CollabError::Serde(format!(
            "message body exceeds {MAX_MESSAGE_BODY_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Reject empty or oversized task titles before an event id/HLC tick is consumed.
fn validate_task_title(title: &str) -> Result<()> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(CollabError::Serde("task title is empty".into()));
    }
    if trimmed.len() > MAX_TASK_TITLE_BYTES {
        return Err(CollabError::Serde(format!(
            "task title exceeds {MAX_TASK_TITLE_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Keep human-authored document metadata representable by the read model
/// before it consumes an event id, HLC tick, or signature.
fn validate_document_text(value: &str, field: &str) -> Result<()> {
    if value.len() > MAX_DOCUMENT_TEXT_BYTES {
        return Err(CollabError::Serde(format!(
            "{field} exceeds {MAX_DOCUMENT_TEXT_BYTES} bytes"
        )));
    }
    Ok(())
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

/// The task must exist in this space and still be open.
fn require_open_task(state: &DomainState, space: SpaceId, task: EventId) -> Result<()> {
    match state.tasks.get(&task) {
        Some(t) if t.space == space && !t.completed => Ok(()),
        Some(t) if t.space == space => Err(CollabError::TaskAlreadyCompleted(task)),
        _ => Err(CollabError::TaskNotFound(task)),
    }
}

/// The task must exist in this space and currently be complete.
fn require_completed_task(state: &DomainState, space: SpaceId, task: EventId) -> Result<()> {
    match state.tasks.get(&task) {
        Some(t) if t.space == space && t.completed => Ok(()),
        Some(t) if t.space == space => Err(CollabError::TaskAlreadyOpen(task)),
        _ => Err(CollabError::TaskNotFound(task)),
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
    state
        .files
        .get(&file)
        .is_some_and(|current| current.present && current.space == space)
}

/// The call must exist and still have a connected participant; returns its
/// space. This derived guard prevents a late answer/decline from resurrecting
/// a call whose independently-authored participant facts have all converged to
/// `left`/`declined`, even when no peer could safely mint `CallEnded`.
fn require_call(state: &DomainState, call: mde_collab_types::ids::CallId) -> Result<SpaceId> {
    state
        .calls
        .get(&call)
        .filter(|call| {
            !call.ended
                && call
                    .participants
                    .values()
                    .any(|state| matches!(state, CallParticipantState::Connected))
        })
        .map(|c| c.space)
        .ok_or(CollabError::CallNotFound(call))
}

/// An AI request may only target content already known to belong to the request
/// space. This is the core admission boundary for the "bounded context only"
/// `DigitalOcean` lock: a stale/cross-space event id cannot be smuggled into a
/// future provider prompt.
fn require_ai_target_in_space(state: &DomainState, space: SpaceId, target: EventId) -> Result<()> {
    if state
        .messages
        .get(&target)
        .is_some_and(|message| message.space == space && !message.deleted)
        || state
            .alerts
            .get(&target)
            .is_some_and(|(alert_space, _)| *alert_space == space)
        || state
            .clips
            .get(&target)
            .is_some_and(|clip| clip.space == space && !clip.deleted)
    {
        return Ok(());
    }

    Err(CollabError::AiTargetNotFound { space, target })
}

/// Validate the caller-minted id used for AI request/cancel sidecar state.
fn require_ai_request_id(request_id: &str) -> Result<()> {
    let valid = !request_id.is_empty()
        && request_id.len() <= MAX_AI_REQUEST_ID_BYTES
        && request_id.bytes().all(|b| {
            matches!(
                b,
                b'a'..=b'z'
                    | b'A'..=b'Z'
                    | b'0'..=b'9'
                    | b'-'
                    | b'_'
                    | b'.'
                    | b':'
            )
        });
    if valid {
        Ok(())
    } else {
        Err(CollabError::InvalidAiRequestId {
            request_id: request_id.to_string(),
            max_bytes: MAX_AI_REQUEST_ID_BYTES,
        })
    }
}

/// Validate the caller-supplied alert action key before it is looked up or
/// signed into an invocation event. Delimiters that can become path/topic
/// components are rejected rather than normalized, preserving exact identity.
fn require_alert_action_id(action_id: &str) -> Result<()> {
    let valid = !action_id.is_empty()
        && action_id.len() <= MAX_ALERT_ACTION_ID_BYTES
        && action_id
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'/' | b'\\'));
    if valid {
        Ok(())
    } else {
        Err(CollabError::InvalidAlertActionId {
            action_id: action_id.to_string(),
            max_bytes: MAX_ALERT_ACTION_ID_BYTES,
        })
    }
}

/// Validate the in-call DTMF tone alphabet accepted by RFC 4733
/// `telephone-event`: 0-9, `*`, `#`, and the A-D row. Lower-case A-D is
/// accepted so provider adapters can normalize without losing a valid keypad
/// event.
const fn require_dtmf_digit(digit: char) -> Result<()> {
    if matches!(digit, '0'..='9' | '*' | '#' | 'A'..='D' | 'a'..='d') {
        Ok(())
    } else {
        Err(CollabError::InvalidDtmfDigit { digit })
    }
}

/// The actor must be a present space member and a connected participant in an
/// active call. Call-scoped mutations that alter the participant read model
/// must not be authorized by a call id alone: that would let an unrelated
/// peer forge state for someone else's media session.
fn require_active_call_participant(
    state: &DomainState,
    call: mde_collab_types::ids::CallId,
    actor: &mde_collab_types::ActorId,
) -> Result<SpaceId> {
    let c = state
        .calls
        .get(&call)
        .ok_or(CollabError::CallNotFound(call))?;
    if c.ended
        || !matches!(
            c.participants.get(actor),
            Some(CallParticipantState::Connected)
        )
    {
        return Err(CollabError::CallNotFound(call));
    }
    require_member(state, c.space, actor)?;
    Ok(c.space)
}

/// The author edit/delete window guard.
const fn enforce_window(
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
    state.space(space).is_some_and(|s| {
        matches!(state.role(space, actor), Some(SpaceRole::Owner)) && s.present_owner_count() <= 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobStore, FsBlobStore};
    use crate::engine::CollabEngine;
    use crate::signer::{Ed25519Signer, IdSource};
    use mde_collab_types::ids::{CallId, EventId, FileRefId, ThreadId, TransferId};
    use mde_collab_types::value::{
        sha256_hex, CallKind, ClipItemKind, ClipboardItem, FileRef, MessageBody, TransferDirection,
        TransferMethod,
    };
    use mde_collab_types::{ActorClock, ActorId, CollabEventEnvelope, SpaceKind, SpaceRole};
    use std::io::Cursor;
    use uuid::Uuid;

    struct SeqIds(u128);

    impl IdSource for SeqIds {
        fn next_event_id(&mut self) -> EventId {
            let id = EventId::from_uuid(Uuid::from_u128(self.0));
            self.0 += 1;
            id
        }
    }

    struct ActorSubstitutingSigner(Ed25519Signer);

    impl EventSigner for ActorSubstitutingSigner {
        fn sign(&self, envelope: &mut CollabEventEnvelope) {
            self.0.sign(envelope);
            envelope.actor = ActorId::new("mallory");
        }
    }

    #[test]
    fn signer_actor_substitution_cannot_escape_the_authoring_pipeline() {
        let signer = ActorSubstitutingSigner(Ed25519Signer::from_seed([46; 32]));
        let mut ids = SeqIds(0x4600);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);

        let denied = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "signature boundary".into(),
            },
            &mut alice,
        );

        assert!(matches!(
            denied,
            Err(CollabError::InvalidEvent(event))
                if event == EventId::from_uuid(Uuid::from_u128(0x4600))
        ));
    }

    fn create_files_space(
        engine: &mut CollabEngine,
        signer: &Ed25519Signer,
        ids: &mut SeqIds,
    ) -> SpaceId {
        engine
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "files ingest".into(),
                },
                signer,
                ids,
                1_000,
            )
            .expect("create files space")[0]
            .space_id
    }

    fn file_reference(name: &str, bytes: &[u8]) -> FileRef {
        FileRef {
            name: name.into(),
            size: bytes.len() as u64,
            sha256_hex: sha256_hex(bytes),
            mime: Some("application/octet-stream".into()),
        }
    }

    fn staged_residue(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut residue = Vec::new();
        let Ok(shards) = std::fs::read_dir(root) else {
            return residue;
        };
        for shard in shards.flatten() {
            let Ok(entries) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            residue.extend(entries.flatten().filter_map(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
                    .then(|| entry.path())
            }));
        }
        residue
    }

    #[test]
    fn files_ingest_registers_verified_bytes_and_confirms_projection() {
        let root = tempfile::tempdir().expect("blob root");
        let store = FsBlobStore::new(root.path());
        let signer = Ed25519Signer::from_seed([41; 32]);
        let mut ids = SeqIds(0x4100);
        let mut engine = CollabEngine::in_memory("alice").expect("engine");
        let space = create_files_space(&mut engine, &signer, &mut ids);
        let file = FileRefId::from_uuid(Uuid::from_u128(0x4101));
        let bytes = b"authenticated Files payload";
        let reference = file_reference("payload.bin", bytes);

        let registered = ingest_and_register_file(
            &mut engine,
            &store,
            space,
            file,
            reference.clone(),
            Cursor::new(bytes),
            &signer,
            &mut ids,
            2_000,
        )
        .expect("ingest and register");

        assert_eq!(registered.file, file);
        assert_eq!(registered.projection.file, file);
        assert_eq!(registered.projection.reference, reference);
        assert_eq!(registered.projection.linked_by, ActorId::new("alice"));
        assert_eq!(registered.projection.linked_unix_ms, 2_000);
        assert!(store.contains(&registered.projection.reference.sha256_hex));
        assert!(staged_residue(root.path()).is_empty());
    }

    #[test]
    fn files_ingest_non_member_refusal_cleans_owned_install() {
        let root = tempfile::tempdir().expect("blob root");
        let store = FsBlobStore::new(root.path());
        let owner_signer = Ed25519Signer::from_seed([42; 32]);
        let mut owner_ids = SeqIds(0x4200);
        let mut owner = CollabEngine::in_memory("alice").expect("owner engine");
        let space = create_files_space(&mut owner, &owner_signer, &mut owner_ids);
        let mut intruder = CollabEngine::in_memory("mallory").expect("intruder engine");
        intruder
            .merge(owner.all_events())
            .expect("replicate existing space");
        let bytes = b"must be rolled back";
        let reference = file_reference("denied.bin", bytes);
        let digest = reference.sha256_hex.clone();

        let error = ingest_and_register_file(
            &mut intruder,
            &store,
            space,
            FileRefId::from_uuid(Uuid::from_u128(0x4201)),
            reference,
            Cursor::new(bytes),
            &Ed25519Signer::from_seed([43; 32]),
            &mut SeqIds(0x4300),
            2_000,
        )
        .expect_err("non-member must be refused");

        assert!(matches!(
            error,
            CollabError::NotMember { space: found, actor }
                if found == space && actor == ActorId::new("mallory")
        ));
        assert!(!store.contains(&digest));
        assert!(staged_residue(root.path()).is_empty());
    }

    #[test]
    fn files_ingest_reuses_exact_existing_blob_without_replacing_it() {
        let root = tempfile::tempdir().expect("blob root");
        let mut store = FsBlobStore::new(root.path());
        let bytes = b"already durable CAS payload";
        let existing = store.put(bytes).expect("prepopulate CAS");
        let canonical = root
            .path()
            .join(&existing.sha256_hex[..2])
            .join(&existing.sha256_hex);
        #[cfg(unix)]
        let existing_identity = {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&canonical).expect("existing metadata");
            (metadata.dev(), metadata.ino())
        };
        let signer = Ed25519Signer::from_seed([44; 32]);
        let mut ids = SeqIds(0x4400);
        let mut engine = CollabEngine::in_memory("alice").expect("engine");
        let space = create_files_space(&mut engine, &signer, &mut ids);
        let file = FileRefId::from_uuid(Uuid::from_u128(0x4401));

        let registered = ingest_and_register_file(
            &mut engine,
            &store,
            space,
            file,
            file_reference("existing.bin", bytes),
            Cursor::new(bytes),
            &signer,
            &mut ids,
            2_000,
        )
        .expect("register existing blob");

        assert_eq!(registered.file, file);
        assert!(store.contains(&existing.sha256_hex));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(canonical).expect("retained metadata");
            assert_eq!((metadata.dev(), metadata.ino()), existing_identity);
        }
        assert!(staged_residue(root.path()).is_empty());
    }

    #[test]
    fn files_ingest_projection_failure_rolls_back_owned_install() {
        let root = tempfile::tempdir().expect("blob root");
        let store = FsBlobStore::new(root.path());
        let signer = Ed25519Signer::from_seed([45; 32]);
        let mut ids = SeqIds(0x4500);
        let mut engine = CollabEngine::in_memory("alice").expect("engine");
        let space = create_files_space(&mut engine, &signer, &mut ids);
        engine
            .projection()
            .connection()
            .execute_batch("DROP TABLE file_refs")
            .expect("inject projection failure");
        let bytes = b"projection must reject";
        let reference = file_reference("rollback.bin", bytes);
        let digest = reference.sha256_hex.clone();
        let event_count = engine.all_events().len();

        ingest_and_register_file(
            &mut engine,
            &store,
            space,
            FileRefId::from_uuid(Uuid::from_u128(0x4501)),
            reference,
            Cursor::new(bytes),
            &signer,
            &mut ids,
            2_000,
        )
        .expect_err("projection failure must reject registration");

        assert_eq!(engine.all_events().len(), event_count);
        assert!(!store.contains(&digest));
        assert!(staged_residue(root.path()).is_empty());
    }

    #[test]
    fn file_generation_commit_is_optimistic_and_rejects_a_stale_retry() {
        let signer = Ed25519Signer::from_seed([9; 32]);
        let mut ids = SeqIds(0x700);
        let mut ctx = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);
        let created = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "files".into(),
            },
            &mut ctx,
        )
        .expect("create space");
        let space = created[0].space_id;
        let file = FileRefId::from_uuid(Uuid::from_u128(0x701));
        let old = FileRef {
            name: "report.bin".into(),
            size: 3,
            sha256_hex: "a".repeat(64),
            mime: Some("application/octet-stream".into()),
        };
        let linked = apply_command(
            &DomainState::from_events(&created),
            &CollabCommand::LinkFile {
                space,
                file,
                reference: old.clone(),
            },
            &mut ctx,
        )
        .expect("link file");
        let mut events = created;
        events.extend(linked);
        ctx.now_unix_ms = 2_000;
        let replacement = FileRef {
            name: old.name.clone(),
            size: 4,
            sha256_hex: "b".repeat(64),
            mime: old.mime.clone(),
        };
        let command_for = |reference| CollabCommand::CommitFileGeneration {
            space,
            file,
            expected_generation: 1_000,
            expected_sha256_hex: old.sha256_hex.clone(),
            expected_size: old.size,
            reference,
        };
        let state = DomainState::from_events(&events);
        for proposed_generation in [1_000, 999] {
            ctx.now_unix_ms = proposed_generation;
            assert!(matches!(
                apply_command(&state, &command_for(replacement.clone()), &mut ctx),
                Err(CollabError::FileGenerationDidNotAdvance {
                    file: found,
                    current_generation: 1_000,
                    proposed_generation: found_generation,
                }) if found == file && found_generation == proposed_generation
            ));
        }
        ctx.now_unix_ms = 2_000;
        let mut renamed = replacement.clone();
        renamed.name = "hostile-name.bin".into();
        assert!(matches!(
            apply_command(&state, &command_for(renamed), &mut ctx),
            Err(CollabError::FileGenerationMetadataMutation(found)) if found == file
        ));
        let mut remimed = replacement.clone();
        remimed.mime = Some("text/plain".into());
        assert!(matches!(
            apply_command(&state, &command_for(remimed), &mut ctx),
            Err(CollabError::FileGenerationMetadataMutation(found)) if found == file
        ));
        for hostile_hash in [
            "aB".repeat(32),
            "0".repeat(64),
            "g".repeat(64),
            "b".repeat(63),
            old.sha256_hex.clone(),
        ] {
            let mut hostile = replacement.clone();
            hostile.sha256_hex = hostile_hash;
            assert!(matches!(
                apply_command(&state, &command_for(hostile), &mut ctx),
                Err(CollabError::InvalidFileGeneration(found)) if found == file
            ));
        }
        let command = command_for(replacement.clone());
        let committed =
            apply_command(&state, &command, &mut ctx).expect("matching generation commits");
        assert!(matches!(
            &committed[0].kind,
            CollabEventKind::FileLinked { file: found, reference }
                if *found == file && *reference == replacement
        ));
        events.extend(committed);
        assert!(matches!(
            apply_command(&DomainState::from_events(&events), &command, &mut ctx),
            Err(CollabError::FileGenerationConflict { file: found, .. }) if found == file
        ));
    }

    #[test]
    fn control_transfer_requires_space_membership() {
        let signer = Ed25519Signer::from_seed([7; 32]);
        let mut ids = SeqIds(1);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);

        let created = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = created[0].space_id;
        let file = FileRefId::from_uuid(Uuid::from_u128(10));
        let transfer = TransferId::from_uuid(Uuid::from_u128(11));

        let state = DomainState::from_events(&created);
        let linked = apply_command(
            &state,
            &CollabCommand::LinkFile {
                space,
                file,
                reference: FileRef {
                    name: "report.txt".into(),
                    size: 0,
                    sha256_hex: "0".repeat(64),
                    mime: None,
                },
            },
            &mut alice,
        )
        .expect("link file");

        let mut events = created;
        events.extend(linked);
        let state = DomainState::from_events(&events);
        let started = apply_command(
            &state,
            &CollabCommand::StartTransfer {
                space,
                transfer,
                file,
                method: TransferMethod::Node,
                direction: TransferDirection::Inbound,
            },
            &mut alice,
        )
        .expect("start transfer");
        events.extend(started);
        let state = DomainState::from_events(&events);

        let mut intruder = ApplyCtx::new(ActorId::new("mallory"), 1_100, &signer, &mut ids);
        let denied = apply_command(
            &state,
            &CollabCommand::ControlTransfer {
                transfer,
                control: TransferControl::Cancel,
            },
            &mut intruder,
        );

        assert!(matches!(
            denied,
            Err(CollabError::NotMember {
                space: denied_space,
                actor
            }) if denied_space == space && actor == ActorId::new("mallory")
        ));
        assert!(state.is_owner(space, &ActorId::new("alice")));
        assert!(!state.is_member(space, &ActorId::new("mallory")));
    }

    #[test]
    fn hang_up_requires_a_connected_member_of_the_call_space() {
        let signer = Ed25519Signer::from_seed([8; 32]);
        let mut ids = SeqIds(20);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);

        let created = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = created[0].space_id;
        let state = DomainState::from_events(&created);
        let call = CallId::from_uuid(Uuid::from_u128(30));
        let started = apply_command(
            &state,
            &CollabCommand::StartCall {
                space,
                call,
                kind: CallKind::Audio,
            },
            &mut alice,
        )
        .expect("start call");

        // Model the hostile precondition: the actor has learned the call id
        // and even appears as a connected participant, but is not a space
        // member. The command boundary must still refuse a leave/end event.
        let forged_participant = CollabEventEnvelope::new(
            EventId::from_uuid(Uuid::from_u128(31)),
            space,
            ActorId::new("mallory"),
            ActorClock::at(2_000, 0),
            2_000,
            CollabEventKind::CallParticipantChanged {
                call,
                actor: ActorId::new("mallory"),
                state: CallParticipantState::Connected,
            },
        );
        let mut events = created;
        events.extend(started);
        events.push(forged_participant);
        let state = DomainState::from_events(&events);

        let mut mallory = ApplyCtx::new(ActorId::new("mallory"), 2_100, &signer, &mut ids);
        let denied = apply_command(&state, &CollabCommand::HangUpCall { call }, &mut mallory);

        assert!(matches!(
            denied,
            Err(CollabError::NotMember {
                space: denied_space,
                actor
            }) if denied_space == space && actor == ActorId::new("mallory")
        ));
    }

    #[test]
    fn answering_or_declining_requires_call_space_membership() {
        let signer = Ed25519Signer::from_seed([9; 32]);
        let mut ids = SeqIds(40);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);

        let created = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = created[0].space_id;
        let call = CallId::from_uuid(Uuid::from_u128(41));
        let started = apply_command(
            &DomainState::from_events(&created),
            &CollabCommand::StartCall {
                space,
                call,
                kind: CallKind::Audio,
            },
            &mut alice,
        )
        .expect("start call");
        let mut events = created;
        events.extend(started);
        let state = DomainState::from_events(&events);

        let mut mallory = ApplyCtx::new(ActorId::new("mallory"), 1_100, &signer, &mut ids);
        for command in [
            CollabCommand::AnswerCall { call },
            CollabCommand::DeclineCall { call },
        ] {
            let denied = apply_command(&state, &command, &mut mallory);
            assert!(matches!(
                denied,
                Err(CollabError::NotMember {
                    space: denied_space,
                    actor
                }) if denied_space == space && actor == ActorId::new("mallory")
            ));
        }
    }

    #[test]
    fn send_dtmf_requires_connected_participant_and_valid_tone() {
        let signer = Ed25519Signer::from_seed([10; 32]);
        let mut ids = SeqIds(60);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);

        let created = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = created[0].space_id;

        let state = DomainState::from_events(&created);
        let added_bob = apply_command(
            &state,
            &CollabCommand::AddMember {
                space,
                actor: ActorId::new("bob"),
                role: SpaceRole::Member,
            },
            &mut alice,
        )
        .expect("add bob");

        let mut events = created;
        events.extend(added_bob);
        let state = DomainState::from_events(&events);
        let call = CallId::from_uuid(Uuid::from_u128(61));
        let started = apply_command(
            &state,
            &CollabCommand::StartCall {
                space,
                call,
                kind: CallKind::Audio,
            },
            &mut alice,
        )
        .expect("start call");
        events.extend(started);
        let state = DomainState::from_events(&events);

        let invalid = apply_command(
            &state,
            &CollabCommand::SendDtmf { call, digit: '+' },
            &mut alice,
        );
        assert!(matches!(
            invalid,
            Err(CollabError::InvalidDtmfDigit { digit: '+' })
        ));

        let mut bob = ApplyCtx::new(ActorId::new("bob"), 1_100, &signer, &mut ids);
        let denied = apply_command(
            &state,
            &CollabCommand::SendDtmf { call, digit: '5' },
            &mut bob,
        );
        assert!(matches!(denied, Err(CollabError::CallNotFound(c)) if c == call));

        let answer = apply_command(&state, &CollabCommand::AnswerCall { call }, &mut bob)
            .expect("bob answers");
        events.extend(answer);
        let state = DomainState::from_events(&events);
        let accepted = apply_command(
            &state,
            &CollabCommand::SendDtmf { call, digit: '*' },
            &mut bob,
        )
        .expect("connected bob sends DTMF");
        assert!(
            accepted.is_empty(),
            "DTMF remains ephemeral after admission"
        );
    }

    #[test]
    fn message_mutations_require_current_space_membership_after_author_leaves() {
        let signer = Ed25519Signer::from_seed([10; 32]);
        let mut ids = SeqIds(60);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);

        let created = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = created[0].space_id;
        let mut events = created;

        let added = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::AddMember {
                space,
                actor: ActorId::new("bob"),
                role: SpaceRole::Member,
            },
            &mut alice,
        )
        .expect("add bob");
        events.extend(added);

        let mut bob = ApplyCtx::new(ActorId::new("bob"), 1_100, &signer, &mut ids);
        let posted = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("before departure"),
            },
            &mut bob,
        )
        .expect("post message");
        let target = posted[0].event_id;
        events.extend(posted);

        let left = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::LeaveSpace { space },
            &mut bob,
        )
        .expect("bob leaves");
        events.extend(left);
        let state = DomainState::from_events(&events);
        assert!(!state.is_member(space, &ActorId::new("bob")));

        let mut departed_bob = ApplyCtx::new(ActorId::new("bob"), 1_200, &signer, &mut ids);
        for command in [
            CollabCommand::EditMessage {
                space,
                target,
                body: MessageBody::new("stale edit"),
            },
            CollabCommand::DeleteMessage { space, target },
        ] {
            let denied = apply_command(&state, &command, &mut departed_bob);
            assert!(matches!(
                denied,
                Err(CollabError::NotMember {
                    space: denied_space,
                    actor
                }) if denied_space == space && actor == ActorId::new("bob")
            ));
        }
    }

    #[test]
    fn message_body_cap_is_enforced_before_event_materialization() {
        let signer = Ed25519Signer::from_seed([13; 32]);
        let mut ids = SeqIds(200);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);
        let created = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = created[0].space_id;
        let state = DomainState::from_events(&created);

        let accepted = apply_command(
            &state,
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("x".repeat(MAX_MESSAGE_BODY_BYTES)),
            },
            &mut alice,
        )
        .expect("the existing message-body boundary remains accepted");
        assert!(matches!(
            &accepted[0].kind,
            CollabEventKind::MessagePosted { body, thread: None }
                if body.as_str().len() == MAX_MESSAGE_BODY_BYTES
        ));

        let before_clock = alice.clock;
        let before_next_id = alice.ids.0;
        let denied = apply_command(
            &state,
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("x".repeat(MAX_MESSAGE_BODY_BYTES + 1)),
            },
            &mut alice,
        );

        assert!(matches!(
            denied,
            Err(CollabError::Serde(message))
                if message == format!("message body exceeds {MAX_MESSAGE_BODY_BYTES} bytes")
        ));
        assert_eq!(
            alice.clock, before_clock,
            "refusal must not mint an HLC tick"
        );
        assert_eq!(
            alice.ids.0, before_next_id,
            "refusal must not consume an event id"
        );
    }

    #[test]
    fn resolve_and_reopen_thread_emit_convergent_events() {
        let signer = Ed25519Signer::from_seed([14; 32]);
        let mut ids = SeqIds(300);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);
        let mut events = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = events[0].space_id;

        let posted = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("root"),
            },
            &mut alice,
        )
        .expect("post root");
        let root = posted[0].event_id;
        events.extend(posted);

        let started = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::StartThread {
                space,
                root,
                title: None,
            },
            &mut alice,
        )
        .expect("start thread");
        let thread = match started[0].kind {
            CollabEventKind::ThreadStarted { thread, .. } => thread,
            ref other => panic!("expected ThreadStarted, got {other:?}"),
        };
        events.extend(started);

        let resolved = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::ResolveThread { space, thread },
            &mut alice,
        )
        .expect("resolve thread");
        assert!(matches!(
            resolved[0].kind,
            CollabEventKind::ThreadResolved { thread: t } if t == thread
        ));
        events.extend(resolved);

        let reopened = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::ReopenThread { space, thread },
            &mut alice,
        )
        .expect("reopen thread");
        assert!(matches!(
            reopened[0].kind,
            CollabEventKind::ThreadReopened { thread: t } if t == thread
        ));

        let denied = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::ResolveThread {
                space,
                thread: ThreadId::from_uuid(Uuid::from_u128(999)),
            },
            &mut alice,
        );
        assert!(matches!(denied, Err(CollabError::ThreadNotFound(_))));
    }

    #[test]
    fn channel_task_commands_emit_convergent_lifecycle_events() {
        let signer = Ed25519Signer::from_seed([15; 32]);
        let mut ids = SeqIds(400);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);
        let mut events = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = events[0].space_id;

        let created = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::CreateTask {
                space,
                title: " check the gateway ".into(),
                source: None,
            },
            &mut alice,
        )
        .expect("create task");
        let task = created[0].event_id;
        assert!(matches!(
            &created[0].kind,
            CollabEventKind::TaskCreated { title, source: None } if title == "check the gateway"
        ));
        events.extend(created);

        let updated = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::UpdateTask {
                space,
                task,
                title: " inspect the gateway ".into(),
            },
            &mut alice,
        )
        .expect("update task");
        assert!(matches!(
            &updated[0].kind,
            CollabEventKind::TaskUpdated { task: t, title } if *t == task && title == "inspect the gateway"
        ));
        assert!(updated[0].verify(), "task updates remain signed events");
        events.extend(updated);

        let checked = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::SetTaskChecked {
                space,
                task,
                checked: true,
            },
            &mut alice,
        )
        .expect("check task");
        assert!(matches!(
            checked[0].kind,
            CollabEventKind::TaskChecked { task: t, checked: true } if t == task
        ));
        events.extend(checked);

        let completed = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::CompleteTask { space, task },
            &mut alice,
        )
        .expect("complete task");
        assert!(matches!(
            completed[0].kind,
            CollabEventKind::TaskCompleted { task: t } if t == task
        ));
        assert!(
            completed[0].verify(),
            "task completions remain signed events"
        );
        events.extend(completed);

        let denied = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::SetTaskChecked {
                space,
                task,
                checked: false,
            },
            &mut alice,
        );
        assert!(matches!(denied, Err(CollabError::TaskAlreadyCompleted(t)) if t == task));

        let reopened = apply_command(
            &DomainState::from_events(&events),
            &CollabCommand::ReopenTask { space, task },
            &mut alice,
        )
        .expect("reopen task");
        assert!(matches!(
            reopened[0].kind,
            CollabEventKind::TaskReopened { task: t } if t == task
        ));
        assert!(reopened[0].verify(), "task reopens remain signed events");

        let open_again = DomainState::from_events(
            &events
                .iter()
                .cloned()
                .chain(reopened.iter().cloned())
                .collect::<Vec<_>>(),
        );
        let already_open = apply_command(
            &open_again,
            &CollabCommand::ReopenTask { space, task },
            &mut alice,
        );
        assert!(matches!(already_open, Err(CollabError::TaskAlreadyOpen(t)) if t == task));
    }

    #[test]
    fn channel_task_title_rejection_does_not_consume_clock_or_id() {
        let signer = Ed25519Signer::from_seed([16; 32]);
        let mut ids = SeqIds(500);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);
        let events = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &mut alice,
        )
        .expect("create space");
        let space = events[0].space_id;
        let state = DomainState::from_events(&events);
        let before_clock = alice.clock;
        let before_next_id = alice.ids.0;

        let denied = apply_command(
            &state,
            &CollabCommand::CreateTask {
                space,
                title: " ".into(),
                source: None,
            },
            &mut alice,
        );

        assert!(matches!(
            denied,
            Err(CollabError::Serde(message)) if message == "task title is empty"
        ));
        assert_eq!(alice.clock, before_clock);
        assert_eq!(alice.ids.0, before_next_id);
    }

    fn state_with_unpinned_clips(
        count: usize,
        alice: &mut ApplyCtx<'_, Ed25519Signer, SeqIds>,
    ) -> (SpaceId, DomainState) {
        let created = apply_command(
            &DomainState::default(),
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            alice,
        )
        .expect("create space");
        let space = created[0].space_id;
        let mut events = created;
        for index in 0..count {
            let state = DomainState::from_events(&events);
            let published = apply_command(
                &state,
                &CollabCommand::PublishClipboard {
                    space,
                    text: format!("clip-{index}"),
                    item: ClipboardItem {
                        kind: ClipItemKind::Text,
                        preview: format!("clip-{index}"),
                        sha256_hex: "0".repeat(64),
                        len: index as u64,
                        source: "alice".into(),
                    },
                },
                alice,
            )
            .expect("publish clip");
            events.extend(published);
        }
        (space, DomainState::from_events(&events))
    }

    #[test]
    fn clear_clipboard_rejects_overlarge_fan_out_without_emitting() {
        let signer = Ed25519Signer::from_seed([11; 32]);
        let mut ids = SeqIds(80);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);
        let (space, state) = state_with_unpinned_clips(MAX_CLEAR_CLIPBOARD_EVENTS + 1, &mut alice);
        let before_clock = alice.clock;
        let before_next_id = alice.ids.0;

        let denied = apply_command(&state, &CollabCommand::ClearClipboard { space }, &mut alice);

        assert!(matches!(
            denied,
            Err(CollabError::Serde(message))
                if message.contains("clear_clipboard fan-out exceeds")
        ));
        assert_eq!(
            alice.clock, before_clock,
            "refusal must not mint an HLC tick"
        );
        assert_eq!(
            alice.ids.0, before_next_id,
            "refusal must not consume an id"
        );
    }

    #[test]
    fn clear_clipboard_allows_the_bounded_fan_out() {
        let signer = Ed25519Signer::from_seed([12; 32]);
        let mut ids = SeqIds(140);
        let mut alice = ApplyCtx::new(ActorId::new("alice"), 1_000, &signer, &mut ids);
        let (space, state) = state_with_unpinned_clips(MAX_CLEAR_CLIPBOARD_EVENTS, &mut alice);

        let cleared = apply_command(&state, &CollabCommand::ClearClipboard { space }, &mut alice)
            .expect("the existing 50-entry history remains clearable");

        assert_eq!(cleared.len(), MAX_CLEAR_CLIPBOARD_EVENTS);
        assert!(cleared
            .iter()
            .all(|event| matches!(event.kind, CollabEventKind::ClipboardDeleted { .. })));
    }
}
