//! WL-FUNC-011 — the **Communications** surface, mounted live in the shell.
//!
//! `mde-collab-egui`'s [`CommunicationsSurface`] is a pure UI widget: it renders
//! the [`CollabReadModel`](mde_collab_types) projections through a
//! [`CollabData`] source it is handed and emits typed
//! [`CollabCommand`](mde_collab_types::CollabCommand)s into a [`CommandSink`] the
//! caller drains. This module is the shell-side mount that makes it real on the
//! mesh — the standalone crate carried only a [`FixtureData`](mde_collab_egui) and
//! left the Bus wiring "for a later shell-mount phase". That phase is here:
//!
//!   * [`LiveCollabData`] is the Bus-backed [`CollabData`]. Each refresh folds the
//!     collab worker's retained `state/collab/*` mirrors into the owned projection
//!     shapes the surface reads. The heavy per-space mirrors (Activity,
//!     conversation, threads, files, clipboard, and document sessions) are folded
//!     for the focused channel instead of every channel on first open; fleet-wide
//!     rollups and the call bar still fold globally. It is a **pure renderer** over
//!     the worker's read-model: the shell never depends on the mackesd collab
//!     worker crate — the Bus JSON is the seam (the same discipline as `chat.rs`).
//!   * [`CommunicationsState`] owns the surface + the data source, refreshes the
//!     fold on a poll cadence while in view, and drains the surface's emitted
//!     commands onto `action/collab/<verb>` ([`topics::command_topic_for`]) so the
//!     collab worker applies them.
//!
//! Activity + Messages are live (the surface implements them in full); the
//! labeled-for-later modes stay labeled — no faked data (§7). Live multi-node
//! delivery is the worker's job; this mount is the read-fold + command-publish
//! seam, headless-testable against a tempdir [`Persist`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(feature = "drm")]
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::clock::CLOCK_COMMAND_PREFIX;
use mackes_mesh_types::cloud::CLOUD_ACTION_SCHEMA_VERSION;
use mackes_mesh_types::vdi_clipboard::{
    ClipboardMaterialization, CLIPBOARD_MATERIALIZATION_MAX_AGE_SECS,
    CLIPBOARD_MATERIALIZATION_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::{egui, TextClipboard};
#[cfg(feature = "drm")]
use mde_egui::{ClipboardClientPoll, LocalClipboardOffer, RichClipboardClient};
use serde::de::DeserializeOwned;

use mde_collab_egui::{CollabData, CommandSink, CommunicationsSurface, Mode};
use mde_collab_types::topics::{self, projection as proj};
use mde_collab_types::{
    clipboard_clip_id, ActivityFeed, ActorId, AlertInbox, CallState, ChannelTasks,
    ClipboardClipBody, ClipboardLane, CollabCommand, ConversationTimeline, DocumentSessions,
    EventId, FileReferences, MessagePins, SavedMessages, SpaceDirectory, SpaceId, ThreadId,
    ThreadTimeline, TransferJobs, MAX_CLIPBOARD_TEXT_BYTES,
};
#[cfg(feature = "drm")]
use mde_collab_types::{ClipboardMimeKind, ClipboardMimeOfferV2, ClipboardPayloadV2};

use crate::bus_reader::BusReader;

/// Poll cadence — matches the collab worker's own 2 s tick so the rail +
/// conversations stay live without a cold-start wait (the `chat.rs` cadence).
const REFRESH: Duration = Duration::from_secs(2);

/// Defensive shell-side cap for retained Activity mirrors. The current collab
/// worker publishes the same 1,024-row cap, but a live seat can carry an older or
/// hand-authored Bus mirror; the UI boundary still must not paint or scan an
/// unbounded feed on low-end hardware.
const MAX_ACTIVITY_FEED_ENTRIES: usize = 1024;

/// Seat-local read cursors. This topic deliberately lives outside the
/// replicated `state/collab/*` namespace: read position is a UI preference for
/// this seat, not a collaboration event or a remote read receipt.
const LOCAL_READ_CURSORS_TOPIC: &str = "local/collab/read-cursors";

/// The canonical direct-seat capture/materialization lane. Its body is the
/// existing `{ id, text, source, time }` event contract; do not put an action
/// capability in this body because the event consumers require this exact
/// shape. Mutating action lanes below remain capability-gated.
const CLIPBOARD_CAPTURE_TOPIC: &str = "event/clipboard/clip";

/// The canonical mesh clipboard responder namespace. Communications still emits
/// typed `action/collab/*` commands for its signed projection, but row
/// pin/delete/clear controls must also hit this lane so Mesh Teams edits the
/// same clipboard history as the Clipboard Viewer.
const CLIPBOARD_ACTION_PREFIX: &str = "action/clipboard/";

/// The materialization lane is transient and target-seat scoped. Keep the
/// shell read bounded even if retention has not yet reclaimed old handoffs;
/// a missing match remains an explicit retryable unavailable state rather than
/// turning one clipboard read into a full retained-topic scan.
const MAX_MATERIALIZATION_TAIL: usize = 256;

/// The local seat's wall time in epoch milliseconds (the collab worker's
/// `now_unix_ms` shape). Injected into [`CollabData::now_unix_ms`] so the surface
/// evaluates the message edit/delete window + relative ages against a real clock.
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The newest (latest-wins) body retained on `topic`, decoded into `T`. `None`
/// when the topic carries no message or the body won't decode — the honest
/// pre-projection state, never a fake (§7).
fn read_state<T: DeserializeOwned>(persist: &Persist, topic: &str) -> Option<T> {
    let msg = persist.read_latest(topic).ok().flatten()?;
    serde_json::from_str(&msg.body?).ok()
}

/// Remove Clock mutation capabilities from the retained collaboration mirror.
///
/// Mesh Teams remains a truthful retained notification view, but it is not a
/// second Clock command authority.  In particular, a shell restart must not
/// revive a generic `RunAlertAction` carrying an old `action/clock/command/*`
/// verb after the daemon has replaced that occurrence generation.  Live Clock
/// controls are rebuilt from the current bounded Clock projection and signed by
/// `ClockState`; display-only alert history and unrelated alert actions remain.
fn strip_retained_clock_command_authority(inbox: &mut AlertInbox) {
    for row in &mut inbox.alerts {
        row.alert.actions.retain(|action| {
            !action
                .verb
                .as_deref()
                .is_some_and(|verb| verb.starts_with(CLOCK_COMMAND_PREFIX))
        });
    }
}

/// Normalize line endings and bound one native clipboard value at a UTF-8
/// character boundary. This mirrors the DRM provider's contract at the shell
/// boundary without making the shell depend on private mde-egui helpers.
fn normalize_clipboard_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut bounded = String::with_capacity(text.len().min(MAX_CLIPBOARD_TEXT_BYTES));
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let normalized = if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                let _ = chars.next();
            }
            '\n'
        } else {
            ch
        };
        if bounded.len() + normalized.len_utf8() > MAX_CLIPBOARD_TEXT_BYTES {
            break;
        }
        bounded.push(normalized);
    }
    bounded
}

/// Read and validate the newest canonical event, retaining its Bus ULID so a
/// local clear can suppress exactly that event without suppressing a later
/// event carrying the same text.
fn read_latest_clipboard_event(
    persist: &Persist,
) -> Result<Option<(String, ClipboardClipBody)>, String> {
    let Some(message) = persist
        .read_latest(CLIPBOARD_CAPTURE_TOPIC)
        .map_err(|error| format!("clipboard Bus read failed: {error}"))?
    else {
        return Ok(None);
    };
    let Some(body) = message.body.as_deref() else {
        return Err("clipboard event has no body".to_string());
    };
    let clip: ClipboardClipBody = serde_json::from_str(body)
        .map_err(|error| format!("malformed clipboard event body: {error}"))?;
    clip.validate()
        .map_err(|error| format!("clipboard event validation failed: {error:?}"))?;
    chrono::DateTime::parse_from_rfc3339(&clip.time)
        .map_err(|error| format!("clipboard event time is not RFC3339: {error}"))?;
    Ok(Some((message.ulid, clip)))
}

/// The shell-consumable state of the daemon-authorized target-seat handoff.
///
/// `Unavailable` is deliberately not folded into an empty clipboard: the DRM
/// provider is infallible, but its owner can use `retryable` to distinguish a
/// handoff that has not arrived (or whose Bus is temporarily unavailable) from
/// a successfully consumed materialization. No protocol support is implied by
/// this status; RDP CLIPRDR and SPICE vdagent remain outside this adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipboardMaterializationStatus {
    /// A fresh handoff was found for the exact target seat.
    Available { ulid: String },
    /// No handoff can currently be consumed by this seat.
    Unavailable { retryable: bool, reason: String },
}

impl ClipboardMaterializationStatus {
    fn unavailable(retryable: bool, reason: impl Into<String>) -> Self {
        Self::Unavailable {
            retryable,
            reason: reason.into(),
        }
    }
}

/// Operator-facing recovery guidance for clipboard mode. A missing or stale
/// handoff remains visibly unavailable; it is never presented as an empty
/// clipboard value.
fn clipboard_materialization_notice(status: &ClipboardMaterializationStatus) -> Option<String> {
    match status {
        ClipboardMaterializationStatus::Available { .. } => None,
        ClipboardMaterializationStatus::Unavailable { retryable, reason } => Some(if *retryable {
            format!("Clipboard delivery unavailable — retry: {reason}")
        } else {
            format!("Clipboard delivery unavailable: {reason}")
        }),
    }
}

/// Read the newest daemon-authorized target-seat handoff. This is a transient
/// local delivery record, not the replicated clipboard history event; keeping
/// it on a separate topic prevents a guest→seat paste from being sent back to
/// every attached VDI session.
///
/// The topic is shared by all seats on the node, so `read_latest` is not enough:
/// a newer handoff for another seat must not hide a still-fresh handoff for this
/// seat. Scan newest-to-oldest and stop at the newest matching seat record.
fn read_latest_clipboard_materialization(
    persist: &Persist,
    target_seat: &str,
) -> Result<(Option<(String, String)>, ClipboardMaterializationStatus), String> {
    let messages = persist
        .read_tail(CLIPBOARD_MATERIALIZATION_TOPIC, MAX_MATERIALIZATION_TAIL)
        .map_err(|error| format!("clipboard materialization read failed: {error}"))?;
    for message in messages.iter().rev() {
        let Some(body) = message.body.as_deref() else {
            continue;
        };
        let handoff: ClipboardMaterialization = serde_json::from_str(body)
            .map_err(|error| format!("malformed clipboard materialization: {error}"))?;
        handoff
            .validate()
            .map_err(|error| format!("clipboard materialization validation failed: {error}"))?;
        let target_matches = handoff.target_seat == target_seat
            || target_seat
                .strip_prefix("seat:")
                .is_some_and(|seat| handoff.target_seat == seat);
        if !target_matches {
            continue;
        }
        let issued = chrono::DateTime::parse_from_rfc3339(&handoff.time)
            .map_err(|error| format!("clipboard materialization time is invalid: {error}"))?;
        let age = chrono::Utc::now()
            .signed_duration_since(issued)
            .num_seconds();
        if !(0..=CLIPBOARD_MATERIALIZATION_MAX_AGE_SECS).contains(&age) {
            return Ok((
                None,
                ClipboardMaterializationStatus::unavailable(
                    true,
                    "target-seat clipboard materialization is stale",
                ),
            ));
        }
        let ulid = message.ulid.clone();
        return Ok((
            Some((ulid.clone(), handoff.text.into())),
            ClipboardMaterializationStatus::Available { ulid },
        ));
    }
    Ok((
        None,
        ClipboardMaterializationStatus::unavailable(
            true,
            "no fresh target-seat clipboard materialization is available",
        ),
    ))
}

/// The shell-owned text provider for the direct DRM runner.
///
/// A local `CopyText` output enters this provider through [`TextClipboard`], is
/// bounded and content-addressed, and is published to the canonical Bus event
/// lane. A later paste reads the newest validated event, so mesh-originated
/// text materializes into the same provider without `wl-copy`/`wl-paste`.
///
/// The provider keeps a small local pending cache only while the Bus is absent
/// or a write is failing; it never fabricates a successful mesh publication.
/// Row mutations remain on the existing authorized `action/clipboard/*` path.
#[derive(Debug, Clone)]
pub(crate) struct BusTextClipboard {
    bus_root: Option<PathBuf>,
    source: String,
    /// Session-scoped privacy gate for local → mesh publication. Reads are
    /// intentionally independent: turning this off never hides or deletes a
    /// remote event already retained on the canonical lane.
    local_publishing_enabled: bool,
    cached_text: Option<String>,
    local_write_pending: bool,
    suppressed_bus_ulid: Option<String>,
    consumed_materialization_ulid: Option<String>,
    materialization_status: ClipboardMaterializationStatus,
    last_error: Option<String>,
}

impl BusTextClipboard {
    /// Construct a provider with an explicit source identity (useful for
    /// deterministic tests and for a caller that already owns seat identity).
    pub(crate) fn new(bus_root: Option<PathBuf>, source: impl Into<String>) -> Self {
        Self {
            bus_root,
            source: source.into(),
            local_publishing_enabled: false,
            cached_text: None,
            local_write_pending: false,
            suppressed_bus_ulid: None,
            consumed_materialization_ulid: None,
            materialization_status: ClipboardMaterializationStatus::unavailable(
                true,
                "clipboard materialization has not been checked",
            ),
            last_error: None,
        }
    }

    /// Construct the production seat provider using the shell's canonical local
    /// hostname identity.
    pub(crate) fn for_shell(bus_root: Option<PathBuf>) -> Self {
        Self::new(
            bus_root,
            format!("seat:{}", crate::explorer::local_hostname()),
        )
    }

    /// The most recent provider-side error, if the mesh or event body was
    /// unavailable. The DRM trait is intentionally infallible, so callers use
    /// this for an honest diagnostic surface after a failed frame write/read.
    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// The last exact-target materialization result observed by the shell.
    /// `Unavailable { retryable: true, .. }` is the explicit contract for a
    /// later frame/Bus retry; it is not represented as a clipboard clear.
    pub(crate) fn materialization_status(&self) -> &ClipboardMaterializationStatus {
        &self.materialization_status
    }

    /// Whether this session has explicitly opted into local clipboard
    /// publication. The default is deliberately off for a new provider.
    pub(crate) fn local_publishing_enabled(&self) -> bool {
        self.local_publishing_enabled
    }

    /// Change the session-only local publication preference. This is a
    /// control-plane choice, not a clipboard event, so it does not replay the
    /// current event or create a second wire lane.
    pub(crate) fn set_local_publishing_enabled(&mut self, enabled: bool) {
        self.local_publishing_enabled = enabled;
        self.last_error = None;
    }

    /// Read and materialize the newest valid Bus event.
    fn read_text_checked(&mut self) -> Result<Option<String>, String> {
        let Some(root) = self.bus_root.as_deref() else {
            self.materialization_status = ClipboardMaterializationStatus::unavailable(
                true,
                "local clipboard Bus is unavailable",
            );
            return Ok(self
                .local_write_pending
                .then(|| self.cached_text.clone())
                .flatten());
        };
        let persist = match Persist::open(root.to_path_buf()) {
            Ok(persist) => persist,
            Err(error) => {
                self.materialization_status = ClipboardMaterializationStatus::unavailable(
                    true,
                    format!("could not open clipboard Bus: {error}"),
                );
                return Err(format!("could not open clipboard Bus: {error}"));
            }
        };

        let (materialization, status) =
            read_latest_clipboard_materialization(&persist, &self.source)?;
        self.materialization_status = status;
        if let Some((ulid, text)) = materialization {
            if self.consumed_materialization_ulid.as_deref() != Some(ulid.as_str()) {
                self.consumed_materialization_ulid = Some(ulid);
                self.cached_text = (!text.is_empty()).then_some(text.clone());
                self.local_write_pending = false;
                return Ok(Some(text));
            }
            // A signed handoff is the current clipboard value for this exact
            // seat, not a one-frame notification. Keep the authorized value
            // available to repeated DRM paste reads until a newer event or
            // handoff supersedes it.
            return Ok(self.cached_text.clone());
        }

        let Some((ulid, clip)) = read_latest_clipboard_event(&persist)? else {
            return Ok(self
                .local_write_pending
                .then(|| self.cached_text.clone())
                .flatten());
        };

        if self.suppressed_bus_ulid.as_deref() == Some(ulid.as_str()) {
            return Ok(None);
        }

        self.suppressed_bus_ulid = None;
        self.cached_text = Some(clip.text.clone());
        self.local_write_pending = false;
        Ok(Some(clip.text))
    }

    /// Publish one local copy, returning whether a new Bus event was written.
    /// The latest event is checked first so the provider itself participates in
    /// the existing content dedup/echo guard; the worker remains the mesh-wide
    /// move-to-top/debounce authority.
    fn write_text_checked(&mut self, text: &str) -> Result<bool, String> {
        let text = normalize_clipboard_text(text);
        if !self.local_publishing_enabled {
            // A disabled session must not publish, deduplicate against, or
            // otherwise rewrite the canonical event. Reads remain fully live,
            // so remote history is still materialized by `read_text_checked`.
            self.last_error = None;
            return Ok(false);
        }
        if text.is_empty() {
            let latest = self
                .bus_root
                .as_deref()
                .and_then(|root| Persist::open(root.to_path_buf()).ok())
                .and_then(|persist| read_latest_clipboard_event(&persist).ok().flatten());
            self.suppressed_bus_ulid = latest.map(|(ulid, _)| ulid);
            self.cached_text = None;
            self.local_write_pending = false;
            self.last_error = None;
            return Ok(false);
        }

        let clip = ClipboardClipBody::from_text(
            text.clone(),
            self.source.clone(),
            chrono::Utc::now().to_rfc3339(),
        );
        clip.validate()
            .map_err(|error| format!("clipboard copy refused: {error:?}"))?;
        let expected_id = clipboard_clip_id(&text);

        self.cached_text = Some(text);
        self.local_write_pending = true;

        let Some(root) = self.bus_root.as_deref() else {
            return Err("No local Bus — the mesh daemon may be down.".to_string());
        };
        let persist = Persist::open(root.to_path_buf())
            .map_err(|error| format!("could not open clipboard Bus: {error}"))?;
        if let Some((_ulid, latest)) = read_latest_clipboard_event(&persist)? {
            if latest.id == expected_id && latest.text == clip.text {
                // This is a content-deduplicated copy, not a clear. Keep the
                // current event materializable for the subsequent Paste.
                self.suppressed_bus_ulid = None;
                self.local_write_pending = false;
                return Ok(false);
            }
        }

        // This event lane's exact four-field body is the existing producer
        // contract. It is not an action request and must not be wrapped with an
        // `armed_token`; the existing authorized action lanes remain separate.
        let body = serde_json::to_string(&clip)
            .map_err(|error| format!("serialize clipboard event: {error}"))?;
        persist
            .write(
                CLIPBOARD_CAPTURE_TOPIC,
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(|error| format!("clipboard Bus write failed: {error}"))?;
        self.suppressed_bus_ulid = None;
        self.local_write_pending = false;
        self.last_error = None;
        Ok(true)
    }
}

impl TextClipboard for BusTextClipboard {
    fn read_text(&mut self) -> Option<String> {
        match self.read_text_checked() {
            Ok(text) => {
                self.last_error = None;
                text
            }
            Err(error) => {
                self.last_error = Some(error);
                None
            }
        }
    }

    fn write_text(&mut self, text: &str) {
        match self.write_text_checked(text) {
            Ok(_) => self.last_error = None,
            Err(error) => self.last_error = Some(error),
        }
    }
}

#[cfg(feature = "drm")]
enum AsyncClipboardCommand {
    Poll,
    Publish(String),
    Clear,
}

#[cfg(feature = "drm")]
enum AsyncClipboardResult {
    Poll(Result<Option<String>, String>),
}

/// Nonblocking direct-DRM client for the canonical Bus clipboard lane.
///
/// The worker owns all `Persist` access. Render frames only use bounded
/// `try_send`/`try_recv`, and the newest pending copy replaces an older pending
/// copy because clipboard ownership itself is latest-wins.
#[cfg(feature = "drm")]
pub(crate) struct AsyncBusClipboardClient {
    commands: SyncSender<AsyncClipboardCommand>,
    results: Receiver<AsyncClipboardResult>,
    poll_inflight: bool,
    observed_text: Option<Option<String>>,
    pending_publish: Option<Option<String>>,
}

#[cfg(feature = "drm")]
impl AsyncBusClipboardClient {
    pub(crate) fn for_shell(bus_root: Option<PathBuf>) -> Self {
        let (command_tx, command_rx) = sync_channel(1);
        let (result_tx, result_rx) = sync_channel(2);
        let mut provider = BusTextClipboard::for_shell(bus_root);
        let _ = std::thread::Builder::new()
            .name("mde-drm-clipboard".to_owned())
            .spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    match command {
                        AsyncClipboardCommand::Poll => {
                            let _ = result_tx
                                .try_send(AsyncClipboardResult::Poll(provider.read_text_checked()));
                        }
                        AsyncClipboardCommand::Publish(text) => {
                            let _ = provider.write_text_checked(&text);
                        }
                        AsyncClipboardCommand::Clear => {
                            let _ = provider.write_text_checked("");
                        }
                    }
                }
            });
        Self {
            commands: command_tx,
            results: result_rx,
            poll_inflight: false,
            observed_text: None,
            pending_publish: None,
        }
    }

    fn flush_pending(&mut self) {
        let Some(pending) = self.pending_publish.take() else {
            return;
        };
        let command = match pending.as_ref() {
            Some(text) => AsyncClipboardCommand::Publish(text.clone()),
            None => AsyncClipboardCommand::Clear,
        };
        if let Err(TrySendError::Full(command)) = self.commands.try_send(command) {
            self.pending_publish = Some(match command {
                AsyncClipboardCommand::Publish(text) => Some(text),
                AsyncClipboardCommand::Clear => None,
                AsyncClipboardCommand::Poll => return,
            });
        }
    }

    fn plain_text(offer: &LocalClipboardOffer) -> Option<String> {
        offer
            .offers()
            .iter()
            .find(|candidate| candidate.mime == ClipboardMimeKind::TextPlain)
            .and_then(|candidate| match &candidate.payload {
                ClipboardPayloadV2::InlineText { text } => Some(text.clone()),
                _ => None,
            })
    }
}

#[cfg(feature = "drm")]
impl RichClipboardClient for AsyncBusClipboardClient {
    fn poll_offer(&mut self) -> ClipboardClientPoll {
        self.flush_pending();
        let mut completed = None;
        loop {
            match self.results.try_recv() {
                Ok(AsyncClipboardResult::Poll(result)) => {
                    self.poll_inflight = false;
                    if let Ok(text) = result {
                        completed = Some(text);
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if !self.poll_inflight && self.pending_publish.is_none() {
            match self.commands.try_send(AsyncClipboardCommand::Poll) {
                Ok(()) => self.poll_inflight = true,
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {}
            }
        }
        let Some(text) = completed else {
            return ClipboardClientPoll::Unchanged;
        };
        if self.observed_text.as_ref() == Some(&text) {
            return ClipboardClientPoll::Unchanged;
        }
        self.observed_text = Some(text.clone());
        match text.and_then(|text| {
            ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextPlain, text).ok()
        }) {
            Some(offer) => ClipboardClientPoll::Offer(vec![offer]),
            None => ClipboardClientPoll::Cleared,
        }
    }

    fn publish_offer(&mut self, offer: &LocalClipboardOffer) {
        if let Some(text) = Self::plain_text(offer) {
            self.observed_text = Some(Some(text.clone()));
            self.pending_publish = Some(Some(text));
            self.flush_pending();
        }
    }

    fn clear_offer(&mut self) {
        self.observed_text = Some(None);
        self.pending_publish = Some(None);
        self.flush_pending();
    }
}

/// Keep only the newest Activity rows from a retained Bus mirror. Activity feeds
/// are newest-last by contract, so draining from the front preserves order and
/// keeps the cursor/virtualized renderer on the recent window.
fn bounded_activity_feed(mut feed: ActivityFeed) -> ActivityFeed {
    let overflow = feed.entries.len().saturating_sub(MAX_ACTIVITY_FEED_ENTRIES);
    if overflow > 0 {
        feed.entries.drain(0..overflow);
    }
    feed
}

/// The Bus-backed [`CollabData`] the Communications surface reads.
///
/// Owns the folded projection shapes (so the trait can hand out `&` references,
/// the same shape [`FixtureData`](mde_collab_egui) has) and rebuilds them from the
/// retained `state/collab/*` mirrors on [`refresh`](Self::refresh). The worker
/// publishes each projection latest-wins; this is the surface's window onto that
/// read side.
pub(crate) struct LiveCollabData {
    /// The shared fail-soft Bus-reader seam (holds the resolved spool path).
    reader: BusReader,
    /// This node's collaboration identity — the bare hostname, matching the
    /// collab worker's `self_host` (so "my message" alignment + the author-scoped
    /// edit affordance resolve against the same actor).
    me: ActorId,
    /// The injected wall time, refreshed each fold.
    now_unix_ms: i64,
    /// The rail directory (folded from `state/collab/directory`).
    directory: SpaceDirectory,
    /// Activity feeds currently folded for paint, keyed `Some(space)` to match
    /// the surface's `data.activity(self.selected_space())` read (folded from
    /// `state/collab/activity/<space>`). The shell intentionally keeps this to
    /// the focused channel so opening Mesh Teams does not deserialize every
    /// retained per-space Activity body on a modest seat.
    activity: HashMap<Option<SpaceId>, ActivityFeed>,
    /// Per-space conversation timelines (folded from
    /// `state/collab/conversation/<space>`).
    conversations: HashMap<SpaceId, ConversationTimeline>,
    /// Per-space shared message-pin projections.
    message_pins: HashMap<SpaceId, MessagePins>,
    /// The local actor's private saved-message projection.
    saved_messages: Option<SavedMessages>,
    /// Per-space channel task projections.
    channel_tasks: HashMap<SpaceId, ChannelTasks>,
    /// Retained thread timelines, keyed by their typed thread id. The worker's
    /// per-space thread topic carries one typed timeline, so the root index is
    /// built at the same time for the message-row reply affordance.
    threads: HashMap<ThreadId, ThreadTimeline>,
    /// Thread lookup by its owning space and root message event.
    thread_roots: HashMap<(SpaceId, EventId), ThreadId>,
    /// The aggregated active-call state — every space's `state/collab/call-state`
    /// concatenated into the one persistent call bar's read model.
    call_state: CallState,
    /// Per-space linked-file references (folded from
    /// `state/collab/file-references/<space>`).
    file_references: HashMap<SpaceId, FileReferences>,
    /// Fleet-wide transfer ledger mirror (folded from
    /// `state/collab/transfer-jobs`).
    transfer_jobs: Option<TransferJobs>,
    /// Fleet-wide alert inbox (folded from `state/collab/alert-inbox`).
    alert_inbox: Option<AlertInbox>,
    /// Per-space clipboard lanes (folded from
    /// `state/collab/clipboard-lane/<space>`).
    clipboard_lanes: HashMap<SpaceId, ClipboardLane>,
    /// Per-space live document-session lists (folded from
    /// `state/collab/document-sessions/<space>`).
    document_sessions: HashMap<SpaceId, DocumentSessions>,
    /// The local seat's durable read position for each space. Cursors are
    /// compared with the activity HLC, so a restart does not turn retained
    /// history into a new unread storm.
    read_cursors: HashMap<SpaceId, mde_collab_types::ActorClock>,
    /// The last fold time; the poll self-throttles to [`REFRESH`].
    last_poll: Option<Instant>,
}

impl LiveCollabData {
    /// A fresh source over `bus_root` (the desktop-client spool). No projections
    /// yet — the first [`refresh`](Self::refresh) folds them.
    fn new(bus_root: Option<PathBuf>) -> Self {
        Self {
            reader: BusReader::new(bus_root),
            me: ActorId::new(crate::explorer::local_hostname()),
            now_unix_ms: now_unix_ms(),
            directory: SpaceDirectory::default(),
            activity: HashMap::new(),
            conversations: HashMap::new(),
            message_pins: HashMap::new(),
            saved_messages: None,
            channel_tasks: HashMap::new(),
            threads: HashMap::new(),
            thread_roots: HashMap::new(),
            call_state: CallState::default(),
            file_references: HashMap::new(),
            transfer_jobs: None,
            alert_inbox: None,
            clipboard_lanes: HashMap::new(),
            document_sessions: HashMap::new(),
            read_cursors: HashMap::new(),
            last_poll: None,
        }
    }

    /// Re-fold on the [`REFRESH`] cadence while the surface is in view, and keep
    /// the frame loop ticking so a worker republish surfaces without operator
    /// input (the `chat.rs` poll shape).
    fn poll(&mut self, ctx: &egui::Context, focus_space: Option<SpaceId>) {
        if self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.last_poll = Some(Instant::now());
            self.refresh_for(focus_space);
            ctx.request_repaint_after(REFRESH);
        }
    }

    /// Fold the retained `state/collab/*` mirrors into the owned projections. Opens
    /// the spool fail-soft: no spool / an unopenable store clears to the honest
    /// off-mesh empty state (§7). The `directory` names the spaces and
    /// [`refresh_for`](Self::refresh_for) chooses which channel's heavy
    /// per-space projections are read from the one open handle.
    fn refresh(&mut self) {
        self.refresh_for(None);
    }

    /// Fold the retained `state/collab/*` mirrors into the owned projections for
    /// the currently focused channel. This is the seat .15 open-path guard: the
    /// directory and global rollups stay live, but expensive per-space bodies are
    /// read only for the selected channel (or the first directory row before the
    /// first UI frame has selected one).
    fn refresh_for(&mut self, focus_space: Option<SpaceId>) {
        self.now_unix_ms = now_unix_ms();
        let Some(persist) = self.reader.open() else {
            self.directory = SpaceDirectory::default();
            self.activity.clear();
            self.conversations.clear();
            self.message_pins.clear();
            self.saved_messages = None;
            self.channel_tasks.clear();
            self.threads.clear();
            self.thread_roots.clear();
            self.call_state = CallState::default();
            self.file_references.clear();
            self.transfer_jobs = None;
            self.alert_inbox = None;
            self.clipboard_lanes.clear();
            self.document_sessions.clear();
            self.read_cursors.clear();
            return;
        };

        self.directory =
            read_state(&persist, &topics::state_topic(proj::SPACE_DIRECTORY)).unwrap_or_default();
        self.read_cursors = read_state(&persist, LOCAL_READ_CURSORS_TOPIC).unwrap_or_default();
        let focus_space = focus_space
            .filter(|candidate| {
                self.directory
                    .spaces
                    .iter()
                    .any(|summary| summary.id == *candidate)
            })
            .or_else(|| self.directory.spaces.first().map(|summary| summary.id));

        let mut activity = HashMap::new();
        let mut conversations = HashMap::new();
        let mut message_pins = HashMap::new();
        let saved_messages =
            read_state::<SavedMessages>(&persist, &topics::state_topic(proj::SAVED_MESSAGES));
        let mut channel_tasks = HashMap::new();
        let mut threads = HashMap::new();
        let mut thread_roots = HashMap::new();
        let mut call_state = CallState::default();
        let mut file_references = HashMap::new();
        let mut clipboard_lanes = HashMap::new();
        let mut document_sessions = HashMap::new();
        for summary in &self.directory.spaces {
            let space = summary.id;
            if Some(space) == focus_space {
                if let Some(feed) = read_state::<ActivityFeed>(
                    &persist,
                    &topics::space_state_topic(proj::ACTIVITY, space),
                ) {
                    activity.insert(Some(space), bounded_activity_feed(feed));
                }
                if let Some(convo) = read_state::<ConversationTimeline>(
                    &persist,
                    &topics::space_state_topic(proj::CONVERSATION, space),
                ) {
                    conversations.insert(space, convo);
                }
                if let Some(pins) = read_state::<MessagePins>(
                    &persist,
                    &topics::space_state_topic(proj::MESSAGE_PINS, space),
                ) {
                    message_pins.insert(space, pins);
                }
                if let Some(tasks) = read_state::<ChannelTasks>(
                    &persist,
                    &topics::space_state_topic(proj::CHANNEL_TASKS, space),
                ) {
                    channel_tasks.insert(space, tasks);
                }
                if let Some(thread) = read_state::<ThreadTimeline>(
                    &persist,
                    &topics::space_state_topic(proj::THREAD, space),
                ) {
                    thread_roots.insert((thread.space, thread.root.event_id), thread.thread);
                    threads.insert(thread.thread, thread);
                }
                if let Some(files) = read_state::<FileReferences>(
                    &persist,
                    &topics::space_state_topic(proj::FILE_REFERENCES, space),
                ) {
                    file_references.insert(space, files);
                }
                if let Some(clipboard) = read_state::<ClipboardLane>(
                    &persist,
                    &topics::space_state_topic(proj::CLIPBOARD_LANE, space),
                ) {
                    clipboard_lanes.insert(space, clipboard);
                }
                if let Some(sessions) = read_state::<DocumentSessions>(
                    &persist,
                    &topics::space_state_topic(proj::DOCUMENT_SESSIONS, space),
                ) {
                    document_sessions.insert(space, sessions);
                }
            }
            if let Some(calls) = read_state::<CallState>(
                &persist,
                &topics::space_state_topic(proj::CALL_STATE, space),
            ) {
                // The trait exposes one aggregate CallState (the call bar's read
                // model); the worker publishes it per space, so concatenate.
                call_state.active.extend(calls.active);
            }
        }
        for summary in &mut self.directory.spaces {
            let cursor = self
                .read_cursors
                .get(&summary.id)
                .copied()
                .unwrap_or_default();
            summary.unread = activity
                .get(&Some(summary.id))
                .map(|feed| {
                    feed.entries
                        .iter()
                        .rev()
                        .take_while(|entry| entry.clock > cursor)
                        .count()
                        .min(u32::MAX as usize) as u32
                })
                .unwrap_or_else(|| if summary.last_activity > cursor { 1 } else { 0 });
        }
        let transfer_jobs =
            read_state::<TransferJobs>(&persist, &topics::state_topic(proj::TRANSFER_JOBS));
        let alert_inbox =
            read_state::<AlertInbox>(&persist, &topics::state_topic(proj::ALERT_INBOX)).map(
                |mut inbox| {
                    strip_retained_clock_command_authority(&mut inbox);
                    inbox
                },
            );
        self.activity = activity;
        self.conversations = conversations;
        self.message_pins = message_pins;
        self.saved_messages = saved_messages;
        self.channel_tasks = channel_tasks;
        self.threads = threads;
        self.thread_roots = thread_roots;
        self.call_state = call_state;
        self.file_references = file_references;
        self.transfer_jobs = transfer_jobs;
        self.alert_inbox = alert_inbox;
        self.clipboard_lanes = clipboard_lanes;
        self.document_sessions = document_sessions;
    }

    /// Advance a seat-local cursor to the newest activity currently visible in
    /// `space`. A failed write leaves the in-memory cursor unchanged, so the
    /// badge remains honest and the next render can retry the persistence.
    fn mark_space_read(&mut self, space: SpaceId) {
        let Some(latest) = self
            .activity
            .get(&Some(space))
            .and_then(|feed| feed.entries.last().map(|entry| entry.clock))
        else {
            return;
        };
        if self
            .read_cursors
            .get(&space)
            .is_some_and(|cursor| *cursor >= latest)
        {
            return;
        }

        let mut next = self.read_cursors.clone();
        next.insert(space, latest);
        let Ok(body) = serde_json::to_string(&next) else {
            tracing::debug!(target: "shell::communications", "failed to encode local read cursors");
            return;
        };
        let Some(persist) = self.reader.open() else {
            return;
        };
        if let Err(error) = persist.write(
            LOCAL_READ_CURSORS_TOPIC,
            Priority::Default,
            None,
            Some(&body),
        ) {
            tracing::debug!(
                target: "shell::communications",
                %error,
                "failed to persist local collaboration read cursor",
            );
            return;
        }

        self.read_cursors = next;
        if let Some(summary) = self.directory.spaces.iter_mut().find(|s| s.id == space) {
            summary.unread = 0;
        }
    }
}

impl CollabData for LiveCollabData {
    fn me(&self) -> &ActorId {
        &self.me
    }

    fn now_unix_ms(&self) -> i64 {
        self.now_unix_ms
    }

    fn space_directory(&self) -> &SpaceDirectory {
        &self.directory
    }

    fn activity(&self, space: Option<SpaceId>) -> Option<&ActivityFeed> {
        self.activity.get(&space)
    }

    fn conversation(&self, space: SpaceId) -> Option<&ConversationTimeline> {
        self.conversations.get(&space)
    }

    fn message_pinned(&self, space: SpaceId, message: EventId) -> bool {
        self.message_pins
            .get(&space)
            .is_some_and(|pins| pins.messages.contains(&message))
    }

    fn message_saved(&self, space: SpaceId, message: EventId) -> bool {
        self.saved_messages
            .as_ref()
            .filter(|saved| saved.actor == self.me)
            .is_some_and(|saved| {
                saved
                    .messages
                    .iter()
                    .any(|row| row.space == space && row.message == message)
            })
    }

    fn thread(&self, space: SpaceId, thread: ThreadId) -> Option<&ThreadTimeline> {
        self.threads
            .get(&thread)
            .filter(|timeline| timeline.space == space)
    }

    fn thread_for_root(&self, space: SpaceId, root: EventId) -> Option<ThreadId> {
        self.thread_roots.get(&(space, root)).copied()
    }

    fn channel_tasks(&self, space: SpaceId) -> Option<&ChannelTasks> {
        self.channel_tasks.get(&space)
    }

    fn call_state(&self) -> &CallState {
        &self.call_state
    }

    fn file_references(&self, space: SpaceId) -> Option<&FileReferences> {
        self.file_references.get(&space)
    }

    fn transfer_jobs(&self) -> Option<&TransferJobs> {
        self.transfer_jobs.as_ref()
    }

    fn alert_inbox(&self) -> Option<&AlertInbox> {
        self.alert_inbox.as_ref()
    }

    fn clipboard_lane(&self, space: SpaceId) -> Option<&ClipboardLane> {
        self.clipboard_lanes.get(&space)
    }

    fn document_sessions(&self, space: SpaceId) -> Option<&DocumentSessions> {
        self.document_sessions.get(&space)
    }
}

/// The shell-side mount of the Communications surface: the widget + its live data
/// source + the publish seam that routes emitted commands onto `action/collab/*`.
pub(crate) struct CommunicationsState {
    /// The pure `mde-collab-egui` widget (owns only view state).
    surface: CommunicationsSurface,
    /// The Bus-backed projection source the widget renders.
    data: LiveCollabData,
    /// The shell-owned native text clipboard seam. The direct DRM runner can
    /// borrow this provider; while its runner wiring remains outside this
    /// slice's permitted files, Communications still keeps remote Bus
    /// materialization live whenever the hub is open.
    clipboard: BusTextClipboard,
    /// The resolved spool path commands are published through (kept alongside the
    /// reader's copy because publishing needs the open/write error text; the
    /// fail-soft `BusReader` swallows it).
    bus_root: Option<PathBuf>,
}

impl Default for CommunicationsState {
    /// Resolve the desktop-client spool via the canonical GUI resolution
    /// ([`mde_bus::client_data_dir`]), exactly like `ChatState::default`.
    fn default() -> Self {
        Self::new(mde_bus::client_data_dir())
    }
}

impl CommunicationsState {
    /// A fresh mount over `bus_root`.
    fn new(bus_root: Option<PathBuf>) -> Self {
        Self {
            surface: CommunicationsSurface::new(),
            data: LiveCollabData::new(bus_root.clone()),
            clipboard: BusTextClipboard::for_shell(bus_root.clone()),
            bus_root,
        }
    }

    /// Return the shared alert projection for linked This Node health views.
    /// This is a read-only borrow of the existing Notification authority; no
    /// second alert store is created in the hardware center.
    pub(crate) fn alert_inbox(&self) -> Option<&AlertInbox> {
        self.data.alert_inbox()
    }

    /// Publish one typed alert command through the same signed collab action
    /// path used by Communications. This lets This Node acknowledge or snooze
    /// a linked alert without creating a parallel mutation authority.
    pub(crate) fn publish_alert_command(&self, command: CollabCommand) -> Result<(), String> {
        let topic = topics::command_topic_for(&command);
        publish_command(self.bus_root.as_deref(), &topic, &command)
    }

    /// Focus the embedded editor without creating a second editor surface. The
    /// taskbar's Editor icon is therefore a direct route to the real Documents
    /// mode and preserves the existing Communications ownership boundary.
    pub(crate) fn open_editor(&mut self) {
        self.surface.open_editor();
    }

    /// Open the canonical Files app inside Mesh Teams. Legacy `Surface::Files`
    /// deep links call this compatibility route; there is no second file app or
    /// state authority.
    pub(crate) fn open_files(&mut self) {
        self.surface.set_app(mde_collab_egui::MeshTeamsApp::Files);
        self.surface.set_mode(Mode::Files);
    }

    /// Re-fold the `state/collab/*` mirrors on the poll cadence (the shell calls
    /// this while Communications is the surface in view).
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        // Materialize the newest canonical event into the shell-owned provider
        // while Communications is live. This is deliberately read-only with
        // respect to the Bus; local publication occurs only on a DRM CopyText
        // write through `drm_clipboard`.
        let _ = self.clipboard.read_text();
        self.data.poll(ctx, self.surface.selected_space());
    }

    /// Borrow the production text provider for the direct DRM runner. The
    /// caller owns the runner lifetime; no compositor clipboard tools are
    /// involved.
    pub(crate) fn drm_clipboard(&mut self) -> &mut dyn TextClipboard {
        &mut self.clipboard
    }

    /// Expose the target-seat handoff result to the shell owner. A caller can
    /// render `Unavailable { retryable: true, .. }` as a retry affordance
    /// without treating the infallible [`TextClipboard`] read as a successful
    /// empty paste.
    pub(crate) fn clipboard_materialization_status(&self) -> &ClipboardMaterializationStatus {
        self.clipboard.materialization_status()
    }

    /// Read the session-scoped local clipboard publication preference.
    pub(crate) fn clipboard_publishing_enabled(&self) -> bool {
        self.clipboard.local_publishing_enabled()
    }

    /// Opt this session into or out of local clipboard publication. Disabling
    /// only gates future local writes; it never clears the remote read model.
    pub(crate) fn set_clipboard_publishing_enabled(&mut self, enabled: bool) {
        self.clipboard.set_local_publishing_enabled(enabled);
    }

    /// Render the surface and route the frame's emitted commands. The widget reads
    /// [`self.data`](LiveCollabData) and pushes intent into a per-frame
    /// [`CommandSink`]; this drains the sink and publishes each command onto
    /// `action/collab/<verb>` so the collab worker applies it.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let mut sink = CommandSink::new();
        let selected_before = self.surface.selected_space();
        self.surface
            .set_clipboard_publishing_enabled(self.clipboard_publishing_enabled());
        if self.surface.mode() == Mode::Clipboard {
            if let Some(notice) =
                clipboard_materialization_notice(self.clipboard_materialization_status())
            {
                ui.colored_label(mde_egui::Style::WARN, notice);
                ui.add_space(mde_egui::Style::SP_S);
            }
        }
        self.surface.ui(ui, &self.data, &mut sink);
        let surface_clipboard_preference = self.surface.clipboard_publishing_enabled();
        if surface_clipboard_preference != self.clipboard_publishing_enabled() {
            self.set_clipboard_publishing_enabled(surface_clipboard_preference);
        }
        let selected_after = self.surface.selected_space();
        if selected_after != selected_before {
            self.data.refresh_for(selected_after);
            ui.ctx().request_repaint();
        }
        if let Some(space) = self.surface.selected_space() {
            self.data.mark_space_read(space);
        }
        drain_to_bus(&mut sink, self.bus_root.as_deref(), &self.data);
    }
}

/// Drain every command the surface emitted this frame onto `action/collab/*`. A
/// publish failure is logged (visible) and dropped — never a silent swallow, and
/// never a faked local apply (the worker is the one authority).
fn drain_to_bus(sink: &mut CommandSink, bus_root: Option<&Path>, data: &dyn CollabData) {
    for command in sink.drain() {
        if let Err(e) = publish_canonical_clipboard_action(bus_root, data, &command) {
            tracing::debug!(
                target: "shell::communications",
                verb = command.verb(),
                error = %e,
                "canonical clipboard action publish failed",
            );
        }
        let topic = topics::command_topic_for(&command);
        if let Err(e) = publish_command(bus_root, &topic, &command) {
            tracing::debug!(
                target: "shell::communications",
                verb = command.verb(),
                error = %e,
                "collab command publish failed",
            );
        }
    }
}

/// Mirror Communications clipboard row mutations to the canonical
/// `action/clipboard/*` responder. The collab command remains the signed
/// projection authority; this companion request keeps the mesh-global
/// `clipboard/history.json` action semantics from drifting into a parallel
/// Communications-only store.
fn publish_canonical_clipboard_action(
    bus_root: Option<&Path>,
    data: &dyn CollabData,
    command: &CollabCommand,
) -> Result<(), String> {
    match command {
        CollabCommand::PinClipboard { space, clip } => {
            let id = clipboard_history_id_for(data, *space, *clip)?;
            publish_clipboard_action_request(bus_root, "pin", Some(&id))
        }
        CollabCommand::UnpinClipboard { space, clip } => {
            let id = clipboard_history_id_for(data, *space, *clip)?;
            publish_clipboard_action_request(bus_root, "unpin", Some(&id))
        }
        CollabCommand::DeleteClipboard { space, clip } => {
            let id = clipboard_history_id_for(data, *space, *clip)?;
            publish_clipboard_action_request(bus_root, "delete", Some(&id))
        }
        CollabCommand::ClearClipboard { .. } => {
            publish_clipboard_action_request(bus_root, "clear", None)
        }
        _ => Ok(()),
    }
}

fn clipboard_history_id_for(
    data: &dyn CollabData,
    space: SpaceId,
    clip: EventId,
) -> Result<String, String> {
    let item = data
        .clipboard_lane(space)
        .and_then(|lane| lane.items.iter().find(|item| item.event_id == clip))
        .ok_or_else(|| format!("clipboard item {clip} is not in the folded lane for {space}"))?;
    clipboard_history_id(&item.sha256_hex).ok_or_else(|| {
        format!(
            "clipboard item {clip} has an invalid content hash for canonical history addressing"
        )
    })
}

/// `clipboard_sync::clip_id` is the first 16 lower-hex chars of the full SHA-256.
/// The Communications read model carries the full content address, so the shell
/// can address canonical history rows without linking the daemon worker crate.
fn clipboard_history_id(sha256_hex: &str) -> Option<String> {
    let id = sha256_hex.get(..16)?;
    if id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(id.to_ascii_lowercase())
    } else {
        None
    }
}

fn clipboard_action_topic(verb: &str) -> String {
    format!("{CLIPBOARD_ACTION_PREFIX}{verb}")
}

fn publish_clipboard_action_request(
    bus_root: Option<&Path>,
    verb: &str,
    id: Option<&str>,
) -> Result<(), String> {
    let Some(root) = bus_root else {
        return Err("No local Bus — the mesh daemon may be down.".to_string());
    };
    let unsigned = match id {
        Some(id) => serde_json::json!({
            "id": id,
            "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        }),
        None => serde_json::json!({
            "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        }),
    }
    .to_string();
    let auth_verb = format!("clipboard-{verb}");
    let target = id
        .map(|id| format!("entry:{id}"))
        .unwrap_or_else(|| "all-unpinned".to_string());
    let authorized =
        crate::iac::authorize_root_mutation_body(&unsigned, &auth_verb, "clipboard", &target)?;
    let persist = Persist::open(root.to_path_buf())
        .map_err(|e| format!("Couldn't open the local Bus: {e}"))?;
    mde_bus::rpc::publish_request(
        &persist,
        &clipboard_action_topic(verb),
        Priority::Default,
        None,
        Some(&authorized),
    )
    .map_err(|e| format!("Bus write failed: {e}"))?;
    Ok(())
}

/// Publish one [`CollabCommand`] on `topic` (`action/collab/<verb>`) through the
/// persist-first Bus path. Mirrors `chat.rs`'s `publish`: the writer opens its own
/// `Persist` (not the fail-soft `BusReader`) because it needs the error text.
fn publish_command(
    bus_root: Option<&Path>,
    topic: &str,
    command: &CollabCommand,
) -> Result<(), String> {
    let Some(root) = bus_root else {
        return Err("No local Bus — the mesh daemon may be down.".to_string());
    };
    let mut envelope =
        serde_json::to_value(command).map_err(|e| format!("serialize collab command: {e}"))?;
    envelope["schema_version"] = serde_json::Value::from(CLOUD_ACTION_SCHEMA_VERSION);
    let body = serde_json::to_string(&envelope)
        .map_err(|e| format!("serialize collab command envelope: {e}"))?;
    let authorized = crate::iac::authorize_root_mutation_body(
        &body,
        "collab-command",
        &crate::explorer::local_hostname(),
        command.verb(),
    )?;
    let persist = Persist::open(root.to_path_buf())
        .map_err(|e| format!("Couldn't open the local Bus: {e}"))?;
    persist
        .write(topic, Priority::Default, None, Some(&authorized))
        .map_err(|e| format!("Bus write failed: {e}"))?;
    Ok(())
}

/// Headless acceptance seam that drives the exact signed command path used by
/// the visible Communications surface. It is intentionally crate-private: the
/// root shell's hidden acceptance CLI can exercise a five-seat matrix without
/// creating a second mint authority or an unauthenticated Bus shortcut.
pub(crate) fn publish_acceptance_command(command: &CollabCommand) -> Result<(), String> {
    let topic = mde_collab_types::topics::command_topic(command.verb());
    publish_command(mde_bus::client_data_dir().as_deref(), &topic, command)
}

/// Publish one clipboard acceptance value through the native DRM provider's
/// real opt-in lane. This covers the toggle gate as well as the canonical event
/// shape; it does not write directly around the provider.
pub(crate) fn publish_acceptance_clipboard(text: &str) -> Result<bool, String> {
    let mut clipboard = BusTextClipboard::for_shell(mde_bus::client_data_dir());
    clipboard.set_local_publishing_enabled(true);
    clipboard.write_text_checked(text)
}

/// Materialize the current native clipboard through the same DRM provider and
/// return only content evidence, never the clipboard text itself.
pub(crate) fn read_acceptance_clipboard() -> Result<Option<(String, usize)>, String> {
    let mut clipboard = BusTextClipboard::for_shell(mde_bus::client_data_dir());
    clipboard.read_text_checked().map(|text| {
        text.map(|text| {
            let len = text.len();
            (mde_collab_types::value::sha256_hex(text.as_bytes()), len)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use mde_collab_types::value::{
        sha256_hex, AlertActionKind, CallKind, ClipItemKind, DeliveryState, MessageBody, Severity,
    };
    use mde_collab_types::{
        ActivityEntry, ActorClock, AlertAction, AlertPayload, AlertView, CallParticipantState,
        CallParticipantView, CallView, ChannelTasks, ClipboardView, EventId, MessagePins,
        MessageView, SavedMessageView, SavedMessages, SpaceKind, SpaceRole, SpaceSummary, TaskView,
    };

    fn persist_at(root: &Path) -> Persist {
        Persist::open(root.to_path_buf()).expect("open persist")
    }

    /// Write a `state/collab/*` retained mirror as the worker would.
    fn write_state<T: serde::Serialize>(persist: &Persist, topic: &str, model: &T) {
        let body = serde_json::to_string(model).expect("serialize model");
        persist
            .write(topic, Priority::Default, None, Some(&body))
            .expect("write state");
    }

    fn space_summary(id: SpaceId, name: &str) -> SpaceSummary {
        SpaceSummary {
            id,
            kind: SpaceKind::Team,
            name: name.to_owned(),
            role: SpaceRole::Owner,
            unread: 0,
            members: 2,
            last_activity: ActorClock::at(1_000, 0),
        }
    }

    fn message(author: &ActorId, body: &str) -> MessageView {
        MessageView {
            event_id: EventId::new(),
            author: author.clone(),
            created_unix_ms: 1_000,
            body: body.to_owned(),
            edited: false,
            deleted: false,
            delivery: DeliveryState::Sent,
            reply_count: 0,
        }
    }

    fn activity_entry(space: SpaceId, actor: &ActorId, wall_ms: u64) -> ActivityEntry {
        ActivityEntry {
            event_id: EventId::new(),
            space,
            actor: actor.clone(),
            clock: ActorClock::at(wall_ms, 0),
            created_unix_ms: wall_ms as i64,
            kind_tag: "message_posted".to_owned(),
            summary: "posted a message".to_owned(),
        }
    }

    #[cfg(feature = "drm")]
    #[test]
    fn drm_bus_client_selects_exact_plain_text_without_mutating_rich_mime_offer() {
        let mut authority = mde_egui::LocalClipboardAuthority::new();
        authority.focus("editor").expect("focus editor");
        let html =
            ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextHtml, "<strong>rich</strong>")
                .expect("html offer");
        let plain = ClipboardMimeOfferV2::inline_text(ClipboardMimeKind::TextPlain, "rich")
            .expect("plain offer");
        authority
            .replace(vec![html.clone(), plain.clone()])
            .expect("rich offer");
        let current = authority.current().expect("current offer");

        assert_eq!(
            AsyncBusClipboardClient::plain_text(current).as_deref(),
            Some("rich")
        );
        assert_eq!(current.offers(), &[html, plain]);
    }

    #[test]
    fn live_collab_data_folds_state_collab_mirrors_into_the_projections() {
        // A fixture set of `state/collab/*` mirror rows — the directory plus one
        // space's Activity, conversation, and call-state — folds into the exact
        // projections the surface reads.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());

        let ops = SpaceId::new();
        let me = ActorId::new(crate::explorer::local_hostname());
        let peer = ActorId::new("falcon");

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![space_summary(ops, "Team Ops")],
            },
        );
        let first_message = message(&peer, "deploy is green");
        write_state(
            &persist,
            &topics::space_state_topic(proj::CONVERSATION, ops),
            &ConversationTimeline {
                space: ops,
                thread: None,
                messages: vec![first_message.clone(), message(&me, "shipped the rail")],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::MESSAGE_PINS, ops),
            &MessagePins {
                space: ops,
                messages: vec![first_message.event_id],
            },
        );
        write_state(
            &persist,
            &topics::state_topic(proj::SAVED_MESSAGES),
            &SavedMessages {
                actor: me.clone(),
                messages: vec![SavedMessageView {
                    space: ops,
                    message: first_message.event_id,
                    saved_unix_ms: 1_001,
                }],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CHANNEL_TASKS, ops),
            &ChannelTasks {
                space: ops,
                tasks: vec![TaskView {
                    task: EventId::new(),
                    space: ops,
                    title: "Review deployment".to_owned(),
                    created_by: me.clone(),
                    created_unix_ms: 1_002,
                    source: Some(first_message.event_id),
                    checked: false,
                    completed: false,
                    completed_by: None,
                    completed_unix_ms: None,
                }],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, ops),
            &ActivityFeed {
                space: Some(ops),
                entries: vec![ActivityEntry {
                    event_id: EventId::new(),
                    space: ops,
                    actor: peer.clone(),
                    clock: ActorClock::at(1_000, 0),
                    created_unix_ms: 1_000,
                    kind_tag: "message_posted".to_owned(),
                    summary: "posted a message".to_owned(),
                }],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CALL_STATE, ops),
            &CallState {
                active: vec![CallView {
                    call: mde_collab_types::CallId::new(),
                    space: ops,
                    kind: CallKind::Audio,
                    started_unix_ms: 1_000,
                    participants: vec![CallParticipantView {
                        actor: me.clone(),
                        state: CallParticipantState::Connected,
                        muted: false,
                    }],
                }],
            },
        );
        let thread_id = ThreadId::new();
        let thread_root = message(&peer, "thread root");
        write_state(
            &persist,
            &topics::space_state_topic(proj::THREAD, ops),
            &ThreadTimeline {
                space: ops,
                thread: thread_id,
                root: thread_root.clone(),
                replies: vec![message(&me, "thread reply")],
                resolved: false,
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::FILE_REFERENCES, ops),
            &FileReferences {
                space: ops,
                files: Vec::new(),
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CLIPBOARD_LANE, ops),
            &ClipboardLane {
                space: ops,
                items: Vec::new(),
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::DOCUMENT_SESSIONS, ops),
            &DocumentSessions::default(),
        );
        write_state(
            &persist,
            &topics::state_topic(proj::TRANSFER_JOBS),
            &TransferJobs::default(),
        );
        write_state(
            &persist,
            &topics::state_topic(proj::ALERT_INBOX),
            &AlertInbox::default(),
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh();

        // Directory folded — the rail row is present.
        assert_eq!(data.space_directory().spaces.len(), 1, "directory folded");
        assert_eq!(data.space_directory().spaces[0].id, ops);
        assert_eq!(data.space_directory().spaces[0].name, "Team Ops");

        // Conversation folded under its space, in order.
        let convo = data.conversation(ops).expect("conversation folded");
        assert_eq!(convo.messages.len(), 2);
        assert_eq!(convo.messages[0].body, "deploy is green");
        assert_eq!(convo.messages[1].author, me);
        assert!(data.message_pinned(ops, first_message.event_id));
        assert!(data.message_saved(ops, first_message.event_id));
        assert_eq!(
            data.channel_tasks(ops).expect("tasks folded").tasks.len(),
            1
        );

        // Activity folded, keyed Some(space) as the surface reads it.
        let feed = data.activity(Some(ops)).expect("activity folded");
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].kind_tag, "message_posted");

        // Per-space call-state aggregated into the one call-bar read model.
        assert_eq!(data.call_state().active.len(), 1, "call-state aggregated");
        assert_eq!(data.call_state().active[0].space, ops);

        let thread = data.thread(ops, thread_id).expect("thread folded");
        assert_eq!(thread.root.event_id, thread_root.event_id);
        assert_eq!(thread.replies.len(), 1);
        assert_eq!(
            data.thread_for_root(ops, thread_root.event_id),
            Some(thread_id),
            "thread root lookup folded"
        );
        assert!(data.file_references(ops).is_some(), "files folded");
        assert!(data.transfer_jobs().is_some(), "transfers folded");
        assert!(data.alert_inbox().is_some(), "alerts folded");
        assert!(data.clipboard_lane(ops).is_some(), "clipboard folded");
        assert!(data.document_sessions(ops).is_some(), "documents folded");
    }

    #[test]
    fn no_spool_folds_to_the_honest_empty_state() {
        // No configured spool → the honest off-mesh empty projections, never a
        // panic and never faked data (§7).
        let mut data = LiveCollabData::new(None);
        data.refresh();
        assert!(data.space_directory().spaces.is_empty());
        assert!(data.activity(None).is_none());
        assert!(data.call_state().active.is_empty());
        assert!(data.thread(SpaceId::new(), ThreadId::new()).is_none());
        assert!(data.transfer_jobs().is_none());
        assert!(data.alert_inbox().is_none());
    }

    #[test]
    fn restarted_shell_cannot_adopt_retained_generic_clock_command_authority() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let stale_clock_verb = format!("{CLOCK_COMMAND_PREFIX}recycled-seat");
        write_state(
            &persist,
            &topics::state_topic(proj::ALERT_INBOX),
            &AlertInbox {
                alerts: vec![AlertView {
                    event_id: EventId::new(),
                    space: SpaceId::new(),
                    alert: AlertPayload {
                        severity: Severity::Critical,
                        source: "former-clock-generation".to_owned(),
                        headline: "Retained ringing alarm".to_owned(),
                        fields: std::collections::BTreeMap::new(),
                        actions: vec![
                            AlertAction {
                                id: "stale-stop".to_owned(),
                                label: "Stop".to_owned(),
                                verb: Some(stale_clock_verb),
                                kind: AlertActionKind::Safe,
                            },
                            AlertAction {
                                id: "inspect".to_owned(),
                                label: "Inspect".to_owned(),
                                verb: Some("action/collab/inspect".to_owned()),
                                kind: AlertActionKind::Safe,
                            },
                        ],
                        goto: Some("clock".to_owned()),
                    },
                    acknowledged: false,
                    snoozed_until_unix_ms: None,
                }],
            },
        );

        // A new shell process reloads the worker-retained inbox.  The row stays
        // visible, but only a current Clock projection may mint Clock commands.
        let mut restarted = LiveCollabData::new(Some(dir.path().to_path_buf()));
        restarted.refresh();
        let inbox = restarted.alert_inbox().expect("retained inbox");
        assert_eq!(
            inbox.alerts.len(),
            1,
            "notification history remains visible"
        );
        assert_eq!(inbox.alerts[0].alert.headline, "Retained ringing alarm");
        assert_eq!(inbox.alerts[0].alert.goto.as_deref(), Some("clock"));
        assert_eq!(
            inbox.alerts[0].alert.actions,
            vec![AlertAction {
                id: "inspect".to_owned(),
                label: "Inspect".to_owned(),
                verb: Some("action/collab/inspect".to_owned()),
                kind: AlertActionKind::Safe,
            }],
            "the stale Clock verb is stripped without erasing unrelated alert actions"
        );
    }

    #[test]
    fn first_open_folds_only_the_focused_channel_activity_body() {
        // Seat .15 regression guard: opening Mesh Teams should not deserialize
        // every retained channel Activity body before the first frame can paint.
        // The focused channel still folds exactly; the non-focused channel keeps
        // an attention badge derived from the directory clock until selected.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let focused = SpaceId::new();
        let noisy = SpaceId::new();
        let peer = ActorId::new("falcon");

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![
                    space_summary(focused, "Focused Ops"),
                    space_summary(noisy, "Noisy Ops"),
                ],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, focused),
            &ActivityFeed {
                space: Some(focused),
                entries: vec![
                    activity_entry(focused, &peer, 1_000),
                    activity_entry(focused, &peer, 1_001),
                ],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, noisy),
            &ActivityFeed {
                space: Some(noisy),
                entries: (0..2_000)
                    .map(|index| activity_entry(noisy, &peer, 2_000 + index))
                    .collect(),
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CONVERSATION, noisy),
            &ConversationTimeline {
                space: noisy,
                thread: None,
                messages: vec![message(&peer, "not on the first-open path")],
            },
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh_for(Some(focused));

        assert_eq!(
            data.activity(Some(focused))
                .expect("focused activity folded")
                .entries
                .len(),
            2,
            "the focused channel keeps its exact unread/activity feed"
        );
        assert!(
            data.activity(Some(noisy)).is_none(),
            "a non-focused channel's retained Activity body must not be deserialized on open"
        );
        assert!(
            data.conversation(noisy).is_none(),
            "non-focused heavy per-space mirrors stay out of the first-open fold"
        );
        let focused_row = data
            .space_directory()
            .spaces
            .iter()
            .find(|summary| summary.id == focused)
            .expect("focused row");
        let noisy_row = data
            .space_directory()
            .spaces
            .iter()
            .find(|summary| summary.id == noisy)
            .expect("noisy row");
        assert_eq!(focused_row.unread, 2);
        assert_eq!(
            noisy_row.unread, 1,
            "unfocused rows keep a cheap attention badge from the directory clock"
        );
    }

    #[test]
    fn focused_activity_feed_is_clamped_and_read_cursor_uses_newest_row() {
        // Seat .15 regression guard: a stale or older worker-retained Activity
        // mirror can be larger than the current core projection cap. The shell
        // keeps the newest-last contract, clamps that mirror at its read boundary,
        // and marks read from the newest retained row without a per-frame max scan.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let space = SpaceId::new();
        let peer = ActorId::new("falcon");

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![space_summary(space, "Operations")],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, space),
            &ActivityFeed {
                space: Some(space),
                entries: (0..2_000)
                    .map(|index| activity_entry(space, &peer, index))
                    .collect(),
            },
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh_for(Some(space));

        let feed = data.activity(Some(space)).expect("focused activity folded");
        assert_eq!(
            feed.entries.len(),
            MAX_ACTIVITY_FEED_ENTRIES,
            "oversized retained mirrors must be clamped at the UI read boundary"
        );
        assert_eq!(
            feed.entries.first().expect("first retained").clock,
            ActorClock::at(976, 0)
        );
        assert_eq!(
            feed.entries.last().expect("newest retained").clock,
            ActorClock::at(1_999, 0),
            "clamping keeps newest-last order"
        );
        assert_eq!(
            data.space_directory().spaces[0].unread,
            MAX_ACTIVITY_FEED_ENTRIES as u32,
            "unread counting is bounded to the retained activity window"
        );

        data.mark_space_read(space);

        assert_eq!(
            data.read_cursors.get(&space).copied(),
            Some(ActorClock::at(1_999, 0)),
            "mark-read advances to the newest retained row"
        );
        assert_eq!(data.space_directory().spaces[0].unread, 0);
    }

    #[test]
    fn read_cursors_drive_unread_badges_and_survive_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let space = SpaceId::new();
        let peer = ActorId::new("falcon");
        let feed = |entries| ActivityFeed {
            space: Some(space),
            entries,
        };

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![space_summary(space, "Team Ops")],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, space),
            &feed(vec![
                activity_entry(space, &peer, 1_000),
                activity_entry(space, &peer, 1_001),
            ]),
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh();
        assert_eq!(data.space_directory().spaces[0].unread, 2);

        data.mark_space_read(space);
        assert_eq!(data.space_directory().spaces[0].unread, 0);

        let mut reloaded = LiveCollabData::new(Some(dir.path().to_path_buf()));
        reloaded.refresh();
        assert_eq!(
            reloaded.space_directory().spaces[0].unread,
            0,
            "the seat-local cursor is durable across a shell reload"
        );

        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, space),
            &feed(vec![
                activity_entry(space, &peer, 1_000),
                activity_entry(space, &peer, 1_001),
                activity_entry(space, &peer, 1_002),
            ]),
        );
        reloaded.refresh();
        assert_eq!(
            reloaded.space_directory().spaces[0].unread,
            1,
            "only activity after the stored cursor is unread"
        );
    }

    #[test]
    fn a_send_message_command_publishes_to_action_collab_send() {
        // A surface-emitted SendMessage (recorded in the CommandSink exactly as the
        // composer's Enter does) drains onto `action/collab/send` with a body that
        // round-trips back to the same typed command — the publish seam.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let ops = SpaceId::new();

        let mut sink = CommandSink::new();
        sink.emit(CollabCommand::SendMessage {
            space: ops,
            thread: None,
            body: MessageBody::new("hello **mesh**"),
        });

        let data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        drain_to_bus(&mut sink, Some(dir.path()), &data);
        assert!(sink.is_empty(), "the sink was drained");

        // The command landed on the canonical `action/collab/send` topic.
        let topic = topics::command_topic("send_message");
        assert_eq!(topic, "action/collab/send_message");
        let published = persist
            .read_latest(&topic)
            .expect("read command")
            .expect("command published");
        let envelope: serde_json::Value =
            serde_json::from_str(published.body.as_deref().expect("command body"))
                .expect("decode command envelope");
        assert_eq!(envelope["schema_version"], 1);
        assert!(
            envelope["armed_token"].as_str().is_some(),
            "mutable collab commands carry the root capability"
        );
        let mut command_value: serde_json::Value =
            serde_json::from_str(published.body.as_deref().expect("command body"))
                .expect("decode command envelope");
        let object = command_value
            .as_object_mut()
            .expect("command envelope object");
        object.remove("armed_token");
        object.remove("schema_version");
        let back: CollabCommand = serde_json::from_value(command_value).expect("decode command");
        assert_eq!(
            back,
            CollabCommand::SendMessage {
                space: ops,
                thread: None,
                body: MessageBody::new("hello **mesh**"),
            },
            "the published body is the emitted SendMessage",
        );
    }

    #[test]
    fn publish_without_a_spool_is_a_visible_error_not_a_panic() {
        // No spool → a typed Err (logged by the drain), never a panic or a faked
        // local apply.
        let err = publish_command(
            None,
            &topics::command_topic("send_message"),
            &CollabCommand::LeaveSpace {
                space: SpaceId::new(),
            },
        )
        .expect_err("no spool must be an error");
        assert!(err.contains("No local Bus"), "explains the down mesh");
    }

    #[test]
    fn clipboard_pin_publishes_collab_command_and_canonical_clipboard_action() {
        // Mesh Teams renders the collab read model, but a row pin must also hit
        // the canonical action/clipboard responder. The responder addresses rows
        // by clipboard_sync's 16-hex content id, not the collab EventId.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let ops = SpaceId::new();
        let clip = EventId::new();
        let text = b"canonical mesh clip";
        let full_hash = sha256_hex(text);
        let history_id = full_hash[..16].to_string();

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![space_summary(ops, "Team Ops")],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CLIPBOARD_LANE, ops),
            &ClipboardLane {
                space: ops,
                items: vec![ClipboardView {
                    event_id: clip,
                    kind: ClipItemKind::Text,
                    preview: "canonical mesh clip".to_string(),
                    sha256_hex: full_hash,
                    source: "falcon".to_string(),
                    at_unix_ms: 1_700_000_000_000,
                    pinned: false,
                }],
            },
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh_for(Some(ops));
        let mut sink = CommandSink::new();
        sink.emit(CollabCommand::PinClipboard { space: ops, clip });

        drain_to_bus(&mut sink, Some(dir.path()), &data);

        let collab = persist
            .read_latest(&topics::command_topic("pin_clipboard"))
            .expect("read collab command")
            .expect("collab command published");
        let collab_body: serde_json::Value =
            serde_json::from_str(collab.body.as_deref().expect("collab body"))
                .expect("decode collab command");
        assert_eq!(collab_body["schema_version"], 1);
        assert!(
            collab_body["armed_token"].as_str().is_some(),
            "collab projection command remains capability-gated"
        );

        let canonical = persist
            .read_latest(&clipboard_action_topic("pin"))
            .expect("read canonical clipboard action")
            .expect("canonical clipboard action published");
        let action_body: serde_json::Value =
            serde_json::from_str(canonical.body.as_deref().expect("action body"))
                .expect("decode canonical action");
        assert_eq!(action_body["schema_version"], 1);
        assert_eq!(action_body["id"], history_id);
        assert!(
            action_body["armed_token"].as_str().is_some(),
            "canonical clipboard mutation carries the responder capability"
        );
    }

    #[test]
    fn clear_clipboard_publishes_canonical_clear_request() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let ops = SpaceId::new();
        let data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        let mut sink = CommandSink::new();
        sink.emit(CollabCommand::ClearClipboard { space: ops });

        drain_to_bus(&mut sink, Some(dir.path()), &data);

        let canonical = persist
            .read_latest(&clipboard_action_topic("clear"))
            .expect("read canonical clipboard action")
            .expect("canonical clipboard clear published");
        let action_body: serde_json::Value =
            serde_json::from_str(canonical.body.as_deref().expect("action body"))
                .expect("decode canonical action");
        assert_eq!(action_body["schema_version"], 1);
        assert!(
            action_body.get("id").is_none(),
            "clear targets all unpinned history, not a row id"
        );
        assert!(
            action_body["armed_token"].as_str().is_some(),
            "canonical clear carries the responder capability"
        );
    }

    #[test]
    fn bus_clipboard_copy_publishes_exact_event_and_remote_paste_materializes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let mut local = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:dell");
        local.set_local_publishing_enabled(true);

        local.write_text("one\r\ntwo");
        assert!(local.last_error().is_none(), "local copy should publish");

        let message = persist
            .read_latest(CLIPBOARD_CAPTURE_TOPIC)
            .expect("read canonical clipboard event")
            .expect("clipboard event published");
        let body_text = message.body.as_deref().expect("event body");
        let body: serde_json::Value = serde_json::from_str(body_text).expect("event json");
        let keys = body
            .as_object()
            .expect("event object")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            ["id", "source", "text", "time"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "the shell must preserve the existing event wire contract"
        );
        assert_eq!(body["text"], "one\ntwo");
        assert_eq!(body["id"], mde_collab_types::clipboard_clip_id("one\ntwo"));
        assert_eq!(body["source"], "seat:dell");
        assert!(
            chrono::DateTime::parse_from_rfc3339(body["time"].as_str().unwrap()).is_ok(),
            "the event timestamp is RFC3339"
        );

        let mut remote_seat = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:other");
        assert_eq!(
            remote_seat.read_text().as_deref(),
            Some("one\ntwo"),
            "a valid mesh event materializes into the native provider"
        );
    }

    #[test]
    fn bus_clipboard_deduplicates_copies_and_clear_suppresses_only_current_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let mut clipboard = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:dell");
        clipboard.set_local_publishing_enabled(true);

        clipboard.write_text("same text");
        let first = persist
            .read_latest(CLIPBOARD_CAPTURE_TOPIC)
            .expect("read first event")
            .expect("first event");
        clipboard.write_text("same text");
        let all = persist
            .list_since(CLIPBOARD_CAPTURE_TOPIC, None)
            .expect("list events");
        assert_eq!(all.len(), 1, "same content does not echo onto the Bus");
        assert_eq!(clipboard.read_text().as_deref(), Some("same text"));

        clipboard.write_text("");
        assert!(
            clipboard.read_text().is_none(),
            "an explicit local clear must not paste the stale Bus event"
        );
        assert_eq!(
            persist
                .read_latest(CLIPBOARD_CAPTURE_TOPIC)
                .expect("read after clear")
                .expect("event retained")
                .ulid,
            first.ulid,
            "clear is local provider state, not a malformed canonical event"
        );

        let next =
            ClipboardClipBody::from_text("new remote text", "seat:other", "2026-07-30T12:00:00Z");
        persist
            .write(
                CLIPBOARD_CAPTURE_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&next).expect("encode next clip")),
            )
            .expect("write remote event");
        assert_eq!(
            clipboard.read_text().as_deref(),
            Some("new remote text"),
            "a later Bus event clears the suppression guard"
        );
    }

    #[test]
    fn bus_clipboard_bounds_utf8_without_splitting_and_rejects_bad_remote_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let prefix = "a".repeat(MAX_CLIPBOARD_TEXT_BYTES - 1);
        let mut clipboard = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:dell");
        clipboard.set_local_publishing_enabled(true);

        clipboard.write_text(&format!("{prefix}é"));
        let materialized = clipboard.read_text().expect("bounded local text");
        assert_eq!(materialized.len(), MAX_CLIPBOARD_TEXT_BYTES - 1);
        assert!(materialized.is_char_boundary(materialized.len()));

        let mut bad = ClipboardClipBody::from_text("remote", "seat:other", "2026-07-30T12:00:00Z");
        bad.id = "wrong".to_string();
        persist
            .write(
                CLIPBOARD_CAPTURE_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&bad).expect("encode bad event")),
            )
            .expect("write bad event");

        let mut fresh = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:fresh");
        assert!(
            fresh.read_text().is_none(),
            "malformed remote text is not materialized"
        );
        assert!(fresh.last_error().is_some());
    }

    #[test]
    fn clipboard_publishing_is_opt_in_and_does_not_replay_or_hide_remote_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let mut clipboard = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:dell");

        assert!(!clipboard.local_publishing_enabled());
        clipboard.write_text("local before opt-in");
        assert!(
            persist
                .read_latest(CLIPBOARD_CAPTURE_TOPIC)
                .expect("read disabled lane")
                .is_none(),
            "a new session must not publish local clipboard text by default"
        );

        let remote =
            ClipboardClipBody::from_text("remote history", "seat:other", "2026-07-30T12:00:00Z");
        persist
            .write(
                CLIPBOARD_CAPTURE_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&remote).expect("encode remote event")),
            )
            .expect("write remote event");
        assert_eq!(
            clipboard.read_text().as_deref(),
            Some("remote history"),
            "disabling local publication must not hide remote clipboard history"
        );

        clipboard.set_local_publishing_enabled(true);
        assert!(clipboard.local_publishing_enabled());
        clipboard.write_text("new local entry");
        let events = persist
            .list_since(CLIPBOARD_CAPTURE_TOPIC, None)
            .expect("list clipboard events");
        assert_eq!(events.len(), 2, "opt-in publishes only the new local entry");

        clipboard.set_local_publishing_enabled(false);
        clipboard.write_text("local after opt-out");
        let events_after_disable = persist
            .list_since(CLIPBOARD_CAPTURE_TOPIC, None)
            .expect("list events after opt-out");
        assert_eq!(
            events_after_disable.len(),
            2,
            "disabling stops future local publication without rewriting history"
        );
        assert_eq!(
            clipboard.read_text().as_deref(),
            Some("new local entry"),
            "the retained remote event remains readable after opt-out"
        );
    }

    #[test]
    fn daemon_materialization_is_targeted_bounded_and_retained_until_superseded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let handoff = ClipboardMaterialization::new(
            "eagle",
            mackes_mesh_types::vdi_clipboard::VdiClipboardText::new("guest→seat")
                .expect("bounded text"),
            "vdi-action:eagle",
            chrono::Utc::now().to_rfc3339(),
        );
        persist
            .write(
                CLIPBOARD_MATERIALIZATION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&handoff).expect("encode handoff")),
            )
            .expect("write handoff");

        let mut clipboard = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:eagle");
        assert_eq!(clipboard.read_text().as_deref(), Some("guest→seat"));
        assert!(matches!(
            clipboard.materialization_status(),
            ClipboardMaterializationStatus::Available { .. }
        ));
        assert_eq!(
            clipboard.read_text().as_deref(),
            Some("guest→seat"),
            "the authorized target-seat value remains the current clipboard across DRM frames"
        );
        let mut other = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:other");
        assert_eq!(
            other.read_text(),
            None,
            "handoff must not cross target seats"
        );
    }

    #[test]
    fn newer_other_seat_materialization_does_not_hide_target_seat_handoff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        for (seat, text) in [("eagle", "for eagle"), ("other", "for other")] {
            let handoff = ClipboardMaterialization::new(
                seat,
                mackes_mesh_types::vdi_clipboard::VdiClipboardText::new(text)
                    .expect("bounded text"),
                format!("vdi-action:{seat}"),
                chrono::Utc::now().to_rfc3339(),
            );
            persist
                .write(
                    CLIPBOARD_MATERIALIZATION_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&handoff).expect("encode handoff")),
                )
                .expect("write handoff");
        }

        let mut clipboard = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:eagle");
        assert_eq!(clipboard.read_text().as_deref(), Some("for eagle"));
        assert!(matches!(
            clipboard.materialization_status(),
            ClipboardMaterializationStatus::Available { .. }
        ));
    }

    #[test]
    fn missing_materialization_exposes_retryable_shell_status() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut clipboard = BusTextClipboard::new(Some(dir.path().to_path_buf()), "seat:eagle");
        assert_eq!(clipboard.read_text(), None);
        assert!(matches!(
            clipboard.materialization_status(),
            ClipboardMaterializationStatus::Unavailable {
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn materialization_notice_is_silent_when_available_and_actionable_when_missing() {
        assert_eq!(
            clipboard_materialization_notice(&ClipboardMaterializationStatus::Available {
                ulid: "01J00000000000000000000000".to_string(),
            }),
            None
        );
        assert_eq!(
            clipboard_materialization_notice(&ClipboardMaterializationStatus::Unavailable {
                retryable: true,
                reason: "no fresh target-seat clipboard materialization is available".to_string(),
            }),
            Some("Clipboard delivery unavailable — retry: no fresh target-seat clipboard materialization is available".to_string())
        );
    }
}
