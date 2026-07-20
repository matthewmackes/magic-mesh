//! The shared leaf value types the events, commands, and read models are built
//! from. Pure data — no logic beyond a SHA-256 constructor for content refs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::EventId;

/// The Markdown body of a message (Communications composes in Markdown).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct MessageBody(pub String);

impl MessageBody {
    /// Wrap raw Markdown source.
    #[must_use]
    pub fn new(markdown: impl Into<String>) -> Self {
        Self(markdown.into())
    }

    /// The Markdown source.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A content-addressed reference to a payload stored **out of band**, identified
/// by the SHA-256 of its bytes.
///
/// The signed envelope stays small: a document snapshot, a CRDT update blob, or
/// a file's bytes are replicated separately and named here by digest, so the
/// signature still covers *which* bytes without carrying them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PayloadRef {
    /// Lower-hex SHA-256 digest of the referenced bytes (64 chars).
    pub sha256_hex: String,
    /// The referenced payload's length in bytes (lets a reader budget/verify
    /// before fetching).
    pub len: u64,
    /// Optional MIME/content-type hint for the referenced bytes.
    #[serde(default)]
    pub content_type: Option<String>,
}

impl PayloadRef {
    /// Compute a reference for `bytes` (hashes them; does not store them).
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self {
            sha256_hex: sha256_hex(bytes),
            len: bytes.len() as u64,
            content_type: None,
        }
    }

    /// Builder: attach a content-type hint.
    #[must_use]
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }
}

/// Lower-hex SHA-256 of `bytes` (64 chars). The platform's content-address
/// convention (same `sha2` 0.10 digest mackesd + bookmarks use).
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    use std::fmt::Write;
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A member's presence, unifying the automatic (Online/Away/Offline) and manual
/// (DND/Invisible/Free-for-Chat) states surveyed in the chat roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    /// Reachable and active.
    Online,
    /// Reachable but idle/away.
    Away,
    /// Do Not Disturb — reachable, but only Critical alerts break through.
    Dnd,
    /// Manually free-for-chat (actively available).
    FreeForChat,
    /// Manually hidden (appears offline to peers).
    Invisible,
    /// Not reachable.
    #[default]
    Offline,
}

/// Alert severity (the chat `Severity` band).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational.
    #[default]
    Info,
    /// Warning — attention advised.
    Warning,
    /// Critical — breaks Do-Not-Disturb.
    Critical,
}

/// The honest, non-faked delivery state of a message, derived from recipient
/// presence (never a fabricated read receipt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Signed and published; recipient reachability unknown.
    #[default]
    Sent,
    /// Recipient was reachable when sent.
    Delivered,
    /// Recipient was offline; queued for their return.
    Queued,
}

/// What kind of inline alert action a button drives (mirrors the chat
/// `AlertActionKind`, including destructive arming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlertActionKind {
    /// A non-destructive verb that may fire immediately.
    #[default]
    Safe,
    /// A destructive verb; requires an armed confirmation before it runs.
    Destructive,
    /// Mark the alert handled for the local seat.
    Ack,
    /// Temporarily hush the alert for the local seat.
    Snooze,
}

/// One configured action button on a folded alert.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AlertAction {
    /// Stable action id within the alert.
    pub id: String,
    /// Button label.
    pub label: String,
    /// The Bus verb this action drives, when it drives an external verb.
    #[serde(default)]
    pub verb: Option<String>,
    /// Action semantics (including destructive arming).
    #[serde(default)]
    pub kind: AlertActionKind,
}

/// The substance of an alert, folded from any truthful Bus alert lane (the
/// `fold_alert` successor). Emitters keep publishing their events unchanged; the
/// collab worker adapts them into this shape at ingest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertPayload {
    /// Severity band.
    pub severity: Severity,
    /// Originating node/host.
    pub source: String,
    /// A short, human-readable headline.
    pub headline: String,
    /// Structured detail fields. A `BTreeMap` so serialization is deterministic
    /// (the bytes fall under the envelope signature).
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// The typed inline actions offered on the alert card.
    #[serde(default)]
    pub actions: Vec<AlertAction>,
    /// Optional "go to" navigation target (a shell goto verb / object ref).
    #[serde(default)]
    pub goto: Option<String>,
}

/// What a clipboard item carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClipItemKind {
    /// Plain text.
    #[default]
    Text,
    /// A URI/link (e.g. a shared browser page).
    Uri,
}

/// A mesh-clipboard item published into the clipboard lane (the
/// `MessageKind::Clipboard` successor), with source attribution + a content
/// hash so the same clip is de-duplicated across nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClipboardItem {
    /// Text vs. URI.
    pub kind: ClipItemKind,
    /// A short preview for the lane row.
    pub preview: String,
    /// SHA-256 (lower-hex) of the full clip content (de-dup + integrity).
    pub sha256_hex: String,
    /// Total content length in bytes.
    pub len: u64,
    /// The node the clip was captured on.
    pub source: String,
}

/// A file reference linked into a space (the File-offer / `FileRefId` successor).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileRef {
    /// Display name.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 (lower-hex) of the file's bytes (integrity + de-dup).
    pub sha256_hex: String,
    /// Optional MIME type.
    #[serde(default)]
    pub mime: Option<String>,
}

/// The transport a transfer uses (the surveyed transfer lanes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransferMethod {
    /// Mesh node-to-node staging (the default mesh share).
    #[default]
    Node,
    /// SFTP lane.
    Sftp,
    /// HTTP/wget lane.
    Http,
    /// rsync lane.
    Rsync,
    /// Browser-download lane (media/HLS/DASH/scrape).
    BrowserDownload,
    /// Music-library lane.
    MusicLibrary,
}

/// Which way bytes flow relative to this seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    /// Bytes arriving at this seat.
    #[default]
    Inbound,
    /// Bytes leaving this seat.
    Outbound,
}

/// The transfer 5-state machine (the surveyed `TransferJob` states) plus the
/// pre-run `Queued` state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Accepted, not yet running.
    #[default]
    Queued,
    /// Bytes moving.
    Active,
    /// Paused by the operator.
    Paused,
    /// Finished successfully (hash-verified).
    Completed,
    /// Ended in error.
    Failed,
    /// Cancelled by the operator.
    Canceled,
}

/// What a call carries (audio SIP call, screen share, live co-edit, remote
/// desktop) — the Voice/Calls + Editor-co-edit + VDI hand-off successors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    /// A SIP/audio call.
    #[default]
    Audio,
    /// An audio + video call.
    Video,
    /// A screen-share session.
    Screen,
    /// A live shared-document co-editing session.
    CoEdit,
    /// A remote-desktop (VDI) hand-off session.
    RemoteDesktop,
}

/// A participant's state within a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CallParticipantState {
    /// Invited, phone ringing.
    #[default]
    Ringing,
    /// Connected and in the call.
    Connected,
    /// Declined the invitation.
    Declined,
    /// Was connected, has left.
    Left,
}

/// The outcome of a document review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    /// Approved as-is.
    Approved,
    /// Changes requested before approval.
    ChangesRequested,
    /// A non-blocking comment.
    #[default]
    Commented,
}

/// A change to a collaboratively-edited document. The actual edit bytes (a CRDT
/// update / yrs delta) are content-addressed via [`PayloadRef`], so a large
/// change never bloats the signed envelope; the base clock records what the
/// change was applied against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentChange {
    /// The content-addressed edit payload (the CRDT update blob).
    pub payload: PayloadRef,
    /// A human summary of the change (for the Activity feed), if any.
    #[serde(default)]
    pub summary: Option<String>,
}

/// What kind of AI assistance a suggestion is (metadata only — the model text
/// and provenance are recorded, but no inference happens in this crate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AiSuggestionKind {
    /// A suggested reply to a message.
    #[default]
    SmartReply,
    /// A summary of a conversation/thread/document.
    Summary,
    /// An extracted action item.
    ActionItem,
    /// A drafted document edit.
    DraftEdit,
}

/// AI suggestion **metadata**: what was suggested, about what, and where it came
/// from. Deliberately provenance-bearing so a suggestion is never mistaken for a
/// human-authored fact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AiSuggestion {
    /// Stable suggestion id (opaque, model-assigned).
    pub id: String,
    /// The assistance kind.
    pub kind: AiSuggestionKind,
    /// The event this suggestion is about, if it targets a specific one.
    #[serde(default)]
    pub target: Option<EventId>,
    /// A short human summary of the suggestion.
    pub summary: String,
    /// Confidence as an integer percent (0..=100), if the model reports one.
    #[serde(default)]
    pub confidence_pct: Option<u8>,
    /// Provenance string (which model/source produced it) — always recorded.
    pub provenance: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_stable_and_lowercase_64() {
        let h = sha256_hex(b"abc");
        assert_eq!(
            h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "known SHA-256('abc')"
        );
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn payload_ref_of_bytes_records_len_and_digest() {
        let r = PayloadRef::of_bytes(b"abc").with_content_type("text/plain");
        assert_eq!(r.len, 3);
        assert_eq!(r.sha256_hex, sha256_hex(b"abc"));
        assert_eq!(r.content_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn severity_orders_info_lt_warning_lt_critical() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Critical);
    }
}
