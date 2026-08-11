//! [`CollabReadModel`] — the retained read-side projections the egui surface
//! consumes off `state/collab/*`.
//!
//! These are **shapes only** — pure struct/enum definitions with no logic. The
//! collab worker folds the signed event log into them and publishes them
//! latest-wins; the surface renders them. Nothing here computes anything.

use serde::{Deserialize, Serialize};

use crate::clock::ActorClock;
use crate::ids::{CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId};
use crate::space::{SpaceKind, SpaceRole};
use crate::value::{
    AiSuggestionKind, AlertPayload, CallKind, CallParticipantState, ClipItemKind, DeliveryState,
    FileRef, PresenceState, Severity, TransferDirection, TransferMethod, TransferState,
};
use crate::ActorId;

/// The full set of read-side projections, so a caller can name each shape by
/// one type. Each variant is an independently-published `state/collab/*` model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollabReadModel {
    /// The left-rail directory of spaces.
    SpaceDirectory(SpaceDirectory),
    /// A space's action-oriented Activity feed.
    Activity(ActivityFeed),
    /// A conversation or thread timeline.
    ConversationTimeline(ConversationTimeline),
    /// Shared message pins for one space.
    MessagePins(MessagePins),
    /// Private saved messages for one local actor.
    SavedMessages(SavedMessages),
    /// A thread timeline (root + replies).
    ThreadTimeline(ThreadTimeline),
    /// Basic channel tasks/action items.
    ChannelTasks(ChannelTasks),
    /// The live document co-edit sessions.
    DocumentSessions(DocumentSessions),
    /// The files linked into a space.
    FileReferences(FileReferences),
    /// The transfer jobs (mirror of the WL-FUNC-006 ledger, read-side).
    TransferJobs(TransferJobs),
    /// The global alert inbox.
    AlertInbox(AlertInbox),
    /// A space's clipboard lane.
    ClipboardLane(ClipboardLane),
    /// The presence board.
    Presence(PresenceBoard),
    /// The active call state.
    CallState(CallState),
    /// Local media-adapter readiness for active calls.
    CallMediaReadiness(CallMediaReadiness),
    /// Local media-adapter live-proof results for active calls.
    CallMediaVerification(CallMediaVerification),
    /// `DigitalOcean` AI suggestion request state.
    AiSuggestionRequests(AiSuggestionRequests),
    /// External Discord bridge worker status.
    DiscordBridgeBoard(DiscordBridgeBoard),
}

/// The rail directory of spaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SpaceDirectory {
    /// One row per space the seat is a member of.
    pub spaces: Vec<SpaceSummary>,
}

/// A single rail row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSummary {
    /// The space id.
    pub id: SpaceId,
    /// Its kind.
    pub kind: SpaceKind,
    /// Its name.
    pub name: String,
    /// The seat's role in it.
    pub role: SpaceRole,
    /// Unread event count feeding the badge (zero paints nothing).
    pub unread: u32,
    /// Member count.
    pub members: u32,
    /// The most recent activity clock (rail sort key).
    pub last_activity: ActorClock,
}

/// A space's chronological, action-oriented Activity feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ActivityFeed {
    /// The space this feed is for (`None` for the cross-space Activity).
    #[serde(default)]
    pub space: Option<SpaceId>,
    /// Newest-last feed entries.
    pub entries: Vec<ActivityEntry>,
}

/// One Activity row — a projected summary of an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    /// The event this summarizes.
    pub event_id: EventId,
    /// The space it happened in.
    pub space: SpaceId,
    /// The actor.
    pub actor: ActorId,
    /// The event's clock.
    pub clock: ActorClock,
    /// The event's creation time (epoch ms).
    pub created_unix_ms: i64,
    /// The event-kind discriminant (matches `CollabEventKind::tag`), so the
    /// feed can filter by band without re-parsing the whole event.
    pub kind_tag: String,
    /// A short human summary line.
    pub summary: String,
}

/// A conversation (or in-thread) timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTimeline {
    /// The space.
    pub space: SpaceId,
    /// The thread, when this is a thread view.
    #[serde(default)]
    pub thread: Option<ThreadId>,
    /// Ordered messages.
    pub messages: Vec<MessageView>,
}

/// A rendered message row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageView {
    /// The message event id.
    pub event_id: EventId,
    /// The author.
    pub author: ActorId,
    /// Creation time (epoch ms).
    pub created_unix_ms: i64,
    /// The (possibly edited) Markdown body.
    pub body: String,
    /// Whether the message was edited.
    pub edited: bool,
    /// Whether the message was deleted (tombstone; body may be redacted).
    pub deleted: bool,
    /// Honest delivery state (never a faked read receipt).
    pub delivery: DeliveryState,
    /// Reply count, when this message roots a thread.
    #[serde(default)]
    pub reply_count: u32,
}

/// The currently pinned messages in one space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePins {
    /// The space whose shared pins are listed.
    pub space: SpaceId,
    /// Pinned message ids in canonical message order.
    pub messages: Vec<EventId>,
}

/// The private saved-message projection for one actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMessages {
    /// The actor whose private marks are represented.
    pub actor: ActorId,
    /// Saved messages in canonical save order.
    pub messages: Vec<SavedMessageView>,
}

/// One actor-scoped private saved-message row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMessageView {
    /// The space containing the saved message.
    pub space: SpaceId,
    /// The saved message id.
    pub message: EventId,
    /// When the current save mark was authored.
    pub saved_unix_ms: i64,
}

/// A thread's root + replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTimeline {
    /// The space.
    pub space: SpaceId,
    /// The thread id.
    pub thread: ThreadId,
    /// The root message.
    pub root: MessageView,
    /// The replies, ordered.
    pub replies: Vec<MessageView>,
    /// Whether the thread is resolved.
    pub resolved: bool,
}

/// Basic channel tasks/action items for one space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelTasks {
    /// The space/channel these tasks belong to.
    pub space: SpaceId,
    /// Newest-last task rows.
    pub tasks: Vec<TaskView>,
}

/// One channel task/action-item row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    /// The task creation event id.
    pub task: EventId,
    /// The space/channel that owns the task.
    pub space: SpaceId,
    /// Short operator-authored title.
    pub title: String,
    /// Who created the task.
    pub created_by: ActorId,
    /// When the task was created (epoch ms).
    pub created_unix_ms: i64,
    /// Optional message that originated the action item.
    #[serde(default)]
    pub source: Option<EventId>,
    /// Lightweight checked state. Completion is a separate terminal state.
    pub checked: bool,
    /// Whether the task is complete.
    pub completed: bool,
    /// Who completed the task, when complete.
    #[serde(default)]
    pub completed_by: Option<ActorId>,
    /// When the task was completed, when complete.
    #[serde(default)]
    pub completed_unix_ms: Option<i64>,
}

/// The live document co-edit sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DocumentSessions {
    /// One row per open session.
    pub sessions: Vec<DocumentSession>,
}

/// One document session view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSession {
    /// The document.
    pub document: DocumentId,
    /// The space it lives in.
    pub space: SpaceId,
    /// Title.
    pub title: String,
    /// Current participants.
    pub participants: Vec<ActorId>,
    /// The call backing the live session, if one is open.
    #[serde(default)]
    pub call: Option<CallId>,
}

/// The files linked into a space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReferences {
    /// The space.
    pub space: SpaceId,
    /// The linked files.
    pub files: Vec<FileReferenceView>,
}

/// One linked-file row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReferenceView {
    /// The stable file-reference id.
    pub file: FileRefId,
    /// The file metadata.
    pub reference: FileRef,
    /// Who linked it.
    pub linked_by: ActorId,
    /// When it was linked (epoch ms).
    pub linked_unix_ms: i64,
}

/// The transfer jobs — a read-side mirror of the WL-FUNC-006 progress ledger
/// (this crate never owns a second progress authority).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TransferJobs {
    /// One row per job.
    pub jobs: Vec<TransferJobView>,
}

/// One transfer-job row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferJobView {
    /// The transfer's control id.
    pub transfer: TransferId,
    /// The file being moved.
    pub file: FileRefId,
    /// Transport.
    pub method: TransferMethod,
    /// Direction.
    pub direction: TransferDirection,
    /// State.
    pub state: TransferState,
    /// Bytes moved so far (mirrored from the ledger).
    pub moved: u64,
    /// Total bytes (mirrored from the ledger; `0` if unknown).
    pub total: u64,
}

/// The global alert inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AlertInbox {
    /// Newest-first alert rows.
    pub alerts: Vec<AlertView>,
}

/// One alert-inbox row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertView {
    /// The alert event id.
    pub event_id: EventId,
    /// The space it was projected into.
    pub space: SpaceId,
    /// The alert substance.
    pub alert: AlertPayload,
    /// Whether it has been acknowledged.
    pub acknowledged: bool,
    /// The snooze expiry (epoch ms), when snoozed.
    #[serde(default)]
    pub snoozed_until_unix_ms: Option<i64>,
}

/// A space's clipboard lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardLane {
    /// The space.
    pub space: SpaceId,
    /// Newest-first clip rows.
    pub items: Vec<ClipboardView>,
}

/// One clipboard-lane row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardView {
    /// The clip event id.
    pub event_id: EventId,
    /// Text vs URI.
    pub kind: ClipItemKind,
    /// A short preview.
    pub preview: String,
    /// SHA-256 (lower-hex) of the full content.
    pub sha256_hex: String,
    /// The source node.
    pub source: String,
    /// When captured (epoch ms).
    pub at_unix_ms: i64,
    /// Whether pinned.
    pub pinned: bool,
}

/// The presence board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PresenceBoard {
    /// One row per known member.
    pub members: Vec<PresenceView>,
}

/// One presence row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceView {
    /// The member.
    pub actor: ActorId,
    /// Their presence.
    pub presence: PresenceState,
    /// Their free-text status, if any.
    #[serde(default)]
    pub status: Option<String>,
    /// Their node role badge, if any.
    #[serde(default)]
    pub role_badge: Option<String>,
}

/// The active call state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CallState {
    /// One row per active call.
    pub active: Vec<CallView>,
}

/// One active-call row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallView {
    /// The call id.
    pub call: CallId,
    /// The space it is in.
    pub space: SpaceId,
    /// What the call carries.
    pub kind: CallKind,
    /// When it started (epoch ms).
    pub started_unix_ms: i64,
    /// The participants and their states.
    pub participants: Vec<CallParticipantView>,
}

/// One call-participant row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallParticipantView {
    /// The participant.
    pub actor: ActorId,
    /// Their call state.
    pub state: CallParticipantState,
    /// Whether they are muted.
    pub muted: bool,
}

/// The adapter-facing media readiness projection for one local actor.
///
/// This is not proof that live media is connected. It is the bounded signed-state
/// hand-off a WebRTC/SIP/LiveKit/VDI worker can consume before touching provider
/// APIs: only non-ended calls where `local_actor` is already a connected
/// participant are included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallMediaReadiness {
    /// The local actor this view was built for.
    pub local_actor: ActorId,
    /// Calls whose local signed state should be surfaced to a media adapter,
    /// either as adapter-ready or as an honest degraded/waiting state.
    pub sessions: Vec<CallMediaSession>,
}

/// One active call that a future media adapter may evaluate or attempt to bind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallMediaSession {
    /// The call id.
    pub call: CallId,
    /// The space it belongs to.
    pub space: SpaceId,
    /// The declared collaboration/call kind.
    pub kind: CallKind,
    /// When the call started (epoch ms).
    pub started_unix_ms: i64,
    /// Required local capabilities/devices for this call kind.
    pub requirements: Vec<CallMediaRequirement>,
    /// Candidate adapter classes. These are not a selected route and do not
    /// claim the adapter is reachable.
    pub candidate_adapters: Vec<CallMediaAdapter>,
    /// Signed-state admission status for a media adapter attempt. This is still
    /// not provider health or proof of advancing media frames.
    pub admission: CallMediaAdmission,
    /// Connected participants to offer to the adapter.
    pub connected_participants: Vec<ActorId>,
    /// The local actor's signed mute bit.
    pub local_muted: bool,
}

/// Whether the signed call state is sufficient for a media adapter attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallMediaAdmission {
    /// A local connected participant and at least one connected remote peer exist.
    AdapterReady,
    /// The local actor is in the call, but no other participant is connected yet.
    WaitingForConnectedPeer,
}

/// Local capability/device requirements implied by a call kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallMediaRequirement {
    /// Microphone/audio capture or playback.
    Microphone,
    /// Camera capture.
    Camera,
    /// Screen capture.
    ScreenCapture,
    /// Shared document/session state.
    DocumentSync,
    /// Remote desktop stream decode/input.
    RemoteDesktopStream,
}

/// Adapter families a future worker may evaluate for a ready call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallMediaAdapter {
    /// Mesh peer-to-peer WebRTC.
    WebRtcP2p,
    /// Mesh-reachable `LiveKit` SFU/SIP bridge.
    LiveKitSfu,
    /// SIP/PSTN gateway path.
    SipGateway,
    /// Document co-edit adapter.
    DocumentCollab,
    /// VDI remote-desktop adapter.
    VdiRemoteDesktop,
}

/// Worker-owned media verification rows derived from retained
/// [`CallMediaReadiness`].
///
/// This is not signed collaboration history and is not a route authority. It is
/// the honest sidecar board a SIP/WebRTC/LiveKit/VDI verifier publishes after
/// consuming readiness: a row may only use [`CallMediaVerificationStatus::LiveMediaVerified`]
/// when a concrete adapter reports advancing frames/data for the session's
/// declared requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallMediaVerification {
    /// The local actor the consumed readiness board was built for.
    pub local_actor: ActorId,
    /// One bounded result row per readiness session and candidate adapter.
    pub rows: Vec<CallMediaVerificationRow>,
}

/// One media-verifier result for a candidate adapter on a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallMediaVerificationRow {
    /// The call id.
    pub call: CallId,
    /// The collaboration space.
    pub space: SpaceId,
    /// The declared collaboration/call kind.
    pub kind: CallKind,
    /// The candidate adapter that was evaluated.
    pub adapter: CallMediaAdapter,
    /// The verifier outcome.
    pub status: CallMediaVerificationStatus,
    /// Frame/data counters supplied by a concrete verifier. Absent for blocked
    /// or unproven rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<CallMediaFrameEvidence>,
    /// Bounded human-readable detail for an honest blocked/unproven row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Whether a media adapter actually proved live media for a readiness row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallMediaVerificationStatus {
    /// The signed call state is still waiting for a connected remote peer.
    WaitingForConnectedPeer,
    /// No local media transport/verifier is registered for this adapter.
    TransportUnavailable,
    /// The adapter exists, but its external provider/gateway is unavailable.
    ProviderUnavailable,
    /// A verifier ran, but did not prove the required advancing frames/data.
    MediaNotProven,
    /// A verifier proved advancing frames/data for the call requirements.
    LiveMediaVerified,
}

/// Counters from a concrete live-media verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CallMediaFrameEvidence {
    /// Advancing audio frames/packets observed.
    pub audio_frames: u64,
    /// Advancing camera/video frames observed.
    pub video_frames: u64,
    /// Advancing screen-share frames observed.
    pub screen_frames: u64,
    /// Advancing collaboration/data-channel messages observed.
    pub data_messages: u64,
}

/// The bounded `DigitalOcean` AI suggestion request board.
///
/// This is worker-owned sidecar state, not signed collaboration history: it lets
/// surfaces render honest pending/canceled/failed provider state while the
/// eventual accepted suggestion remains an explicit user edit carrying
/// provenance in the normal event history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AiSuggestionRequests {
    /// One row per recent request.
    pub requests: Vec<AiSuggestionRequestView>,
}

/// One `DigitalOcean` AI request row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiSuggestionRequestView {
    /// Caller-minted opaque request id.
    pub request_id: String,
    /// The space whose bounded context was requested.
    pub space: SpaceId,
    /// The local actor who requested the suggestion.
    pub requested_by: ActorId,
    /// The assistance kind.
    pub kind: AiSuggestionKind,
    /// The scoped event target, if any.
    #[serde(default)]
    pub target: Option<EventId>,
    /// Current sidecar status.
    pub status: AiSuggestionRequestStatus,
    /// The only hosted provider permitted by the Communications lock.
    pub provider: String,
    /// Provider model, once known.
    #[serde(default)]
    pub model: Option<String>,
    /// Retryable/nonfatal failure reason, if the request did not reach an offer.
    #[serde(default)]
    pub error: Option<String>,
    /// Last state transition time (epoch ms).
    pub updated_unix_ms: i64,
}

/// Sidecar state for a `DigitalOcean` AI suggestion request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSuggestionRequestStatus {
    /// Admitted to the worker sidecar and waiting on the provider adapter.
    Pending,
    /// Canceled by the user before an offer was accepted.
    Canceled,
    /// Failed without impairing local collaboration.
    Failed,
}

/// Worker-owned status for the explicit Discord bridge.
///
/// This is not a Discord client and not collaboration history. It is the honest
/// read-side board a future bridge worker publishes after consuming operator
/// config/provider state: no row means the surface must render "unconfigured",
/// while rows distinguish configured bridges from unavailable/degraded provider
/// state without inventing servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DiscordBridgeBoard {
    /// One row per known Discord bridge binding.
    pub bridges: Vec<DiscordBridgeView>,
}

/// One Discord bridge binding or degraded provider row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordBridgeView {
    /// Worker-stable opaque bridge id.
    pub bridge_id: String,
    /// Mesh channel/space this bridge is scoped to, when configured.
    #[serde(default)]
    pub space: Option<SpaceId>,
    /// Human label supplied by the bridge worker/operator config.
    pub label: String,
    /// Overall configuration/provider state.
    pub status: DiscordBridgeConfigStatus,
    /// Discord-to-Mesh delivery status.
    pub inbound: DiscordBridgeFlowStatus,
    /// Mesh-to-Discord delivery status.
    pub outbound: DiscordBridgeFlowStatus,
    /// Where this row came from.
    pub provenance: DiscordBridgeProvenance,
    /// Bounded detail for degraded/unavailable rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Last worker observation/update time (epoch ms).
    pub updated_unix_ms: i64,
}

/// Overall status for a Discord bridge row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscordBridgeConfigStatus {
    /// No operator bridge config exists.
    Unconfigured,
    /// Config exists or was expected, but the Discord/provider adapter is unavailable.
    ProviderUnavailable,
    /// The worker has a configured bridge row.
    Configured,
}

/// Directional bridge health for the two-way contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscordBridgeFlowStatus {
    /// No mapping/config exists for this direction.
    NotConfigured,
    /// The worker cannot evaluate this direction because the provider is unavailable.
    ProviderUnavailable,
    /// The direction is configured but degraded.
    Degraded,
    /// The direction is configured and ready according to worker state.
    Ready,
}

/// Provenance for a Discord bridge row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordBridgeProvenance {
    /// The source class that produced the row.
    pub source: DiscordBridgeProvenanceSource,
    /// Operator/config authority, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    /// Worker/node that observed or published this row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_by: Option<String>,
    /// Config digest/revision, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_digest: Option<String>,
}

/// Source class for a Discord bridge status row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscordBridgeProvenanceSource {
    /// The row is the UI's honest "no projection/no config" fallback.
    None,
    /// The row came from retained operator configuration.
    OperatorConfig,
    /// The row came from bridge worker cached state.
    WorkerState,
    /// The row came from a concrete provider adapter observation.
    ProviderAdapter,
}

/// The unread/alert badge counters the shell reads for the launcher tile + dock
/// cell (bounded dimensions; a read-side rollup, not a second authority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CommsBadges {
    /// Total unread across spaces.
    pub unread: u32,
    /// Unacknowledged alerts.
    pub alerts: u32,
    /// The most severe unacknowledged alert.
    #[serde(default)]
    pub top_severity: Option<Severity>,
    /// Active transfer count.
    pub active_transfers: u32,
    /// Active call count.
    pub active_calls: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_model_variants_round_trip() {
        let models = [
            CollabReadModel::SpaceDirectory(SpaceDirectory::default()),
            CollabReadModel::Activity(ActivityFeed::default()),
            CollabReadModel::ConversationTimeline(ConversationTimeline {
                space: SpaceId::new(),
                thread: None,
                messages: Vec::new(),
            }),
            CollabReadModel::ChannelTasks(ChannelTasks {
                space: SpaceId::new(),
                tasks: Vec::new(),
            }),
            CollabReadModel::DocumentSessions(DocumentSessions::default()),
            CollabReadModel::TransferJobs(TransferJobs::default()),
            CollabReadModel::AlertInbox(AlertInbox::default()),
            CollabReadModel::Presence(PresenceBoard::default()),
            CollabReadModel::CallState(CallState::default()),
            CollabReadModel::CallMediaReadiness(CallMediaReadiness {
                local_actor: ActorId::new("alice"),
                sessions: Vec::new(),
            }),
            CollabReadModel::CallMediaVerification(CallMediaVerification {
                local_actor: ActorId::new("alice"),
                rows: Vec::new(),
            }),
            CollabReadModel::AiSuggestionRequests(AiSuggestionRequests::default()),
            CollabReadModel::DiscordBridgeBoard(DiscordBridgeBoard {
                bridges: vec![DiscordBridgeView {
                    bridge_id: "bridge-1".to_owned(),
                    space: Some(SpaceId::new()),
                    label: "Ops bridge".to_owned(),
                    status: DiscordBridgeConfigStatus::Configured,
                    inbound: DiscordBridgeFlowStatus::Ready,
                    outbound: DiscordBridgeFlowStatus::Ready,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::OperatorConfig,
                        authority: Some("mesh-team-revision:42".to_owned()),
                        observed_by: Some("seat-15".to_owned()),
                        config_digest: Some("sha256:bridge".to_owned()),
                    },
                    detail: None,
                    updated_unix_ms: 1_000,
                }],
            }),
        ];
        for m in models {
            let json = serde_json::to_string(&m).expect("serialize");
            let back: CollabReadModel = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(m, back);
        }
    }

    #[test]
    fn badges_default_is_all_zero() {
        let b = CommsBadges::default();
        assert_eq!(b.unread, 0);
        assert_eq!(b.alerts, 0);
        assert!(b.top_severity.is_none());
    }

    #[test]
    fn discord_bridge_board_round_trips_status_provenance_and_directional_flows() {
        let model = CollabReadModel::DiscordBridgeBoard(DiscordBridgeBoard {
            bridges: vec![
                DiscordBridgeView {
                    bridge_id: "unconfigured".to_owned(),
                    space: None,
                    label: "Discord bridge".to_owned(),
                    status: DiscordBridgeConfigStatus::Unconfigured,
                    inbound: DiscordBridgeFlowStatus::NotConfigured,
                    outbound: DiscordBridgeFlowStatus::NotConfigured,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::None,
                        authority: None,
                        observed_by: None,
                        config_digest: None,
                    },
                    detail: Some("No operator mapping exists.".to_owned()),
                    updated_unix_ms: 1_000,
                },
                DiscordBridgeView {
                    bridge_id: "configured".to_owned(),
                    space: Some(SpaceId::new()),
                    label: "Ops Discord bridge".to_owned(),
                    status: DiscordBridgeConfigStatus::Configured,
                    inbound: DiscordBridgeFlowStatus::Ready,
                    outbound: DiscordBridgeFlowStatus::Ready,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::OperatorConfig,
                        authority: Some("mesh-team-revision:44".to_owned()),
                        observed_by: Some("seat-15".to_owned()),
                        config_digest: Some("sha256:configured".to_owned()),
                    },
                    detail: None,
                    updated_unix_ms: 2_000,
                },
                DiscordBridgeView {
                    bridge_id: "provider-unavailable".to_owned(),
                    space: Some(SpaceId::new()),
                    label: "Provider unavailable".to_owned(),
                    status: DiscordBridgeConfigStatus::ProviderUnavailable,
                    inbound: DiscordBridgeFlowStatus::ProviderUnavailable,
                    outbound: DiscordBridgeFlowStatus::Degraded,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::WorkerState,
                        authority: Some("mesh-team-revision:45".to_owned()),
                        observed_by: Some("seat-15".to_owned()),
                        config_digest: Some("sha256:provider".to_owned()),
                    },
                    detail: Some("Discord provider adapter unavailable.".to_owned()),
                    updated_unix_ms: 3_000,
                },
            ],
        });

        let json = serde_json::to_string(&model).expect("serialize");
        let back: CollabReadModel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(model, back);
    }
}
