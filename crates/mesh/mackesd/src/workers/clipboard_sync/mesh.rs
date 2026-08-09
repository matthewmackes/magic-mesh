//! Authenticated, bounded mesh transport for the canonical rich clipboard V2
//! contract.
//!
//! The transport never invents a second clipboard authority. Producers submit
//! one signed [`ClipboardEnvelopeV2`] to [`MESH_SEND_TOPIC`]. This adapter binds
//! its self-carried Ed25519 key to the enrolled node directory, writes a strict
//! target-specific frame, and the receiving adapter repeats the peer/key,
//! target, expiry, generation, and quota checks before forwarding the original
//! envelope to `clipboard_sync`'s canonical collaboration lane.
//!
//! No raw path or payload-byte field exists here. Inline UTF-8 remains bounded
//! by the shared contract; images and file lists cross only as opaque Files CAS
//! references plus finite metadata and digests.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_collab_types::{
    reject_duplicate_json_keys, ClipboardDenialReasonV2, ClipboardEnvelopeV2,
    ClipboardEnvelopeV2DecodeError, ClipboardEnvelopeV2ValidationError, ClipboardPayloadV2,
    FileReferences, MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC;

/// Local, signed producer intake. A producer does not choose a network address;
/// the enrolled target node is resolved by the adapter.
pub const MESH_SEND_TOPIC: &str = "action/clipboard/mesh-send-v2";
/// Prefix for target-specific authenticated mesh frames.
pub const MESH_FRAME_TOPIC_PREFIX: &str = "event/clipboard/mesh-v2";
/// Stable typed outcome lane consumed by shell/audit renderers.
pub const MESH_RESULT_TOPIC: &str = "state/clipboard/mesh-result-v2";
/// Transport schema discriminator. The nested clipboard schema remains V2.
pub const MESH_FRAME_SCHEMA_VERSION: u16 = 1;
/// Hard cap before JSON decode. The wrapper contributes only bounded identities.
pub const MAX_MESH_FRAME_BYTES: usize = MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES + 1024;
/// Maximum logical bytes represented by one cross-node clipboard generation.
/// Large Files transfers belong to the Files workflow rather than clipboard.
pub const MAX_MESH_CLIP_LOGICAL_BYTES: u64 = 256 * 1024 * 1024;
/// Bound work and allocation per poll even if a hostile peer floods the topic.
pub const MAX_MESH_FRAMES_PER_TICK: usize = 32;
/// Maximum source/session replay lanes retained by one worker.
pub const MAX_MESH_REPLAY_LANES: usize = 256;
/// Bound the retained Files projections inspected for one CAS identity.
const MAX_FILES_IDENTITY_TOPICS: usize = 256;
/// Bound one retained Files projection before JSON decode.
const MAX_FILES_IDENTITY_BODY_BYTES: usize = 1024 * 1024;
/// Canonical collaboration Files projection prefix.
const FILE_REFERENCES_TOPIC_PREFIX: &str = "state/collab/file-references/";

/// One enrolled node usable by clipboard transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshClipboardPeer {
    /// Bare node name used by the clipboard contract.
    pub node: String,
    /// Enrollment-pinned Ed25519 verifying key, lower-case hex.
    pub public_key_hex: String,
    /// Honest reachability derived from the authoritative node health row.
    pub available: bool,
}

/// Read-only peer lookup seam. Production reads the enrollment DB read-only;
/// tests use an in-memory map and never need the store writer.
pub trait MeshClipboardPeerDirectory: Send + Sync {
    /// Resolve an enrolled peer by exact bare node identity.
    fn peer(&self, node: &str) -> Result<Option<MeshClipboardPeer>, String>;
}

/// Read-only SQLite projection of the canonical enrolled-node table.
#[derive(Debug, Clone)]
pub struct SqliteMeshClipboardPeerDirectory {
    db_path: PathBuf,
}

impl SqliteMeshClipboardPeerDirectory {
    /// Construct without opening or migrating the database.
    #[must_use]
    pub const fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn open_read_only(&self) -> Result<Connection, String> {
        Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| {
            format!(
                "open enrolled peer directory {} read-only: {error}",
                self.db_path.display()
            )
        })
    }
}

impl MeshClipboardPeerDirectory for SqliteMeshClipboardPeerDirectory {
    fn peer(&self, node: &str) -> Result<Option<MeshClipboardPeer>, String> {
        if !safe_node(node) {
            return Ok(None);
        }
        let conn = self.open_read_only()?;
        let mut statement = conn
            .prepare(
                "SELECT name, public_key, role, health FROM nodes \
                 WHERE node_id = ?1 OR name = ?2 LIMIT 1",
            )
            .map_err(|error| format!("prepare enrolled peer lookup: {error}"))?;
        let mut rows = statement
            .query((format!("peer:{node}"), node))
            .map_err(|error| format!("query enrolled peer: {error}"))?;
        let Some(row) = rows
            .next()
            .map_err(|error| format!("read enrolled peer row: {error}"))?
        else {
            return Ok(None);
        };
        let name: String = row
            .get(0)
            .map_err(|error| format!("decode enrolled peer name: {error}"))?;
        let public_key_hex: String = row
            .get(1)
            .map_err(|error| format!("decode enrolled peer public key: {error}"))?;
        let role: String = row
            .get(2)
            .map_err(|error| format!("decode enrolled peer role: {error}"))?;
        let health: String = row
            .get(3)
            .map_err(|error| format!("decode enrolled peer health: {error}"))?;
        if name != node || !lower_hex(&public_key_hex, 64) {
            return Ok(None);
        }
        Ok(Some(MeshClipboardPeer {
            node: name,
            public_key_hex,
            available: role != "decommissioned"
                && !matches!(health.as_str(), "unreachable" | "critical"),
        }))
    }
}

/// Strict transport wrapper. Every security-sensitive value is duplicated only
/// so the Bus router can reject a mismatch before forwarding the nested body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardMeshFrameV1 {
    /// Transport schema discriminator.
    pub schema_version: u16,
    /// Authenticated sending peer; must equal the envelope source.
    pub source_peer: String,
    /// Exact target peer; must equal the envelope target and topic suffix.
    pub target_peer: String,
    /// The sole canonical rich clipboard authority object.
    pub envelope: ClipboardEnvelopeV2,
}

impl ClipboardMeshFrameV1 {
    /// Build a strict frame around an already signed canonical envelope.
    pub fn new(envelope: ClipboardEnvelopeV2) -> Result<Self, ClipboardMeshRefusal> {
        let frame = Self {
            schema_version: MESH_FRAME_SCHEMA_VERSION,
            source_peer: envelope.source.node.to_string(),
            target_peer: envelope.target.node.to_string(),
            envelope,
        };
        frame.validate_shape()?;
        Ok(frame)
    }

    /// Decode with a pre-allocation cap and duplicate/unknown-field rejection
    /// inherited from serde and the nested canonical contract.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, ClipboardMeshRefusal> {
        if body.len() > MAX_MESH_FRAME_BYTES {
            return Err(ClipboardMeshRefusal::Oversized);
        }
        reject_duplicate_json_keys(body).map_err(|_| ClipboardMeshRefusal::InvalidPayload)?;
        let frame: Self =
            serde_json::from_slice(body).map_err(|_| ClipboardMeshRefusal::InvalidPayload)?;
        frame.validate_shape()?;
        Ok(frame)
    }

    fn validate_shape(&self) -> Result<(), ClipboardMeshRefusal> {
        if self.schema_version != MESH_FRAME_SCHEMA_VERSION
            || !safe_node(&self.source_peer)
            || !safe_node(&self.target_peer)
            || self.source_peer != self.envelope.source.node.as_str()
            || self.target_peer != self.envelope.target.node.as_str()
        {
            return Err(ClipboardMeshRefusal::InvalidPayload);
        }
        validate_payload_budget(&self.envelope)
    }
}

/// Stable transport-specific refusal vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardMeshRefusal {
    /// Source is absent from enrollment or its key does not match.
    UnauthorizedPeer,
    /// Target is not enrolled or is currently unavailable.
    UnavailablePeer,
    /// Frame or represented logical payload exceeded a transport bound.
    Oversized,
    /// Envelope expired or is not yet valid.
    Stale,
    /// Source/session generation was already admitted.
    Replayed,
    /// Per-tick or replay-lane capacity was exhausted.
    FloodLimited,
    /// Frame, signature, source/session binding, digest, or target was invalid.
    InvalidPayload,
    /// A Files-backed offer is valid but its canonical CAS bytes have not arrived.
    CasUnavailable,
    /// A Files projection or canonical object disagreed with the signed size/hash.
    CasMismatch,
}

impl ClipboardMeshRefusal {
    fn from_validation(error: &ClipboardEnvelopeV2ValidationError) -> Self {
        match error.denial_reason() {
            ClipboardDenialReasonV2::Oversized => Self::Oversized,
            ClipboardDenialReasonV2::Stale => Self::Stale,
            ClipboardDenialReasonV2::Replayed => Self::Replayed,
            ClipboardDenialReasonV2::UnknownVersion
            | ClipboardDenialReasonV2::SecretBearing
            | ClipboardDenialReasonV2::Unsupported
            | ClipboardDenialReasonV2::InvalidPayload => Self::InvalidPayload,
        }
    }

    fn from_decode(error: &ClipboardEnvelopeV2DecodeError) -> Self {
        match error {
            ClipboardEnvelopeV2DecodeError::BodyTooLarge { .. } => Self::Oversized,
            ClipboardEnvelopeV2DecodeError::Validation(error) => Self::from_validation(error),
            ClipboardEnvelopeV2DecodeError::Json(_) => Self::InvalidPayload,
        }
    }
}

impl std::fmt::Display for ClipboardMeshRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}", self)
    }
}

impl std::error::Error for ClipboardMeshRefusal {}

/// Result emitted for every terminal sender or receiver decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", deny_unknown_fields)]
pub enum ClipboardMeshResultV1 {
    /// Frame reached the canonical receiving authority.
    Accepted {
        /// Source peer.
        source_peer: String,
        /// Target peer.
        target_peer: String,
        /// Source session identity.
        session: String,
        /// Exact admitted generation.
        generation: u64,
    },
    /// Frame failed closed with a stable transport reason.
    Refused {
        /// Source peer when safely decoded, otherwise empty.
        source_peer: String,
        /// Target peer when safely decoded, otherwise empty.
        target_peer: String,
        /// Typed refusal.
        reason: ClipboardMeshRefusal,
    },
}

/// Payload-free replay marker.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplayMarker {
    generation: u64,
    expires_unix_ms: u64,
    /// Transport leases only. Canonical CAS objects remain owned by the Files
    /// purge gate and must never be deleted by clipboard expiry cleanup.
    cas_digests: BTreeSet<String>,
}

/// Bounded receiver replay/expiry state. Cleanup drops expired generations and
/// never touches Files objects because the transport does not own that CAS.
#[derive(Debug, Default)]
pub struct ClipboardMeshReplayLedger {
    latest: BTreeMap<(String, String), ReplayMarker>,
}

impl ClipboardMeshReplayLedger {
    /// Rebuild payload-free replay high-water marks from the canonical
    /// collaboration lane after restart or cursor loss.
    pub fn seed_from_retained(&mut self, persist: &Persist, now_ms: u64) {
        let Ok(messages) =
            persist.read_tail(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC, MAX_MESH_REPLAY_LANES)
        else {
            return;
        };
        for message in messages {
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            let Ok(envelope) = ClipboardEnvelopeV2::from_json_bytes(body.as_bytes()) else {
                continue;
            };
            if envelope.expires_unix_ms <= now_ms {
                continue;
            }
            let key = (
                envelope.source.node.to_string(),
                envelope.session.to_string(),
            );
            let marker = ReplayMarker {
                generation: envelope.sequence,
                expires_unix_ms: envelope.expires_unix_ms,
                cas_digests: cas_digests(&envelope),
            };
            if self
                .latest
                .get(&key)
                .is_none_or(|current| current.generation < marker.generation)
            {
                self.latest.insert(key, marker);
            }
        }
    }

    /// Remove every expired source/session marker.
    pub fn cleanup(&mut self, now_ms: u64) -> usize {
        let before = self.latest.len();
        self.latest
            .retain(|_, marker| marker.expires_unix_ms > now_ms);
        before - self.latest.len()
    }

    fn previous(&self, frame: &ClipboardMeshFrameV1) -> Option<u64> {
        self.latest
            .get(&(
                frame.source_peer.clone(),
                frame.envelope.session.to_string(),
            ))
            .map(|marker| marker.generation)
    }

    fn record(&mut self, frame: &ClipboardMeshFrameV1) -> Result<(), ClipboardMeshRefusal> {
        let key = (
            frame.source_peer.clone(),
            frame.envelope.session.to_string(),
        );
        if !self.latest.contains_key(&key) && self.latest.len() >= MAX_MESH_REPLAY_LANES {
            return Err(ClipboardMeshRefusal::FloodLimited);
        }
        self.latest.insert(
            key,
            ReplayMarker {
                generation: frame.envelope.sequence,
                expires_unix_ms: frame.envelope.expires_unix_ms,
                cas_digests: cas_digests(&frame.envelope),
            },
        );
        Ok(())
    }
}

/// Sender adapter: authenticate both endpoints and emit a target-specific frame.
pub fn send_envelope(
    persist: &Persist,
    directory: &dyn MeshClipboardPeerDirectory,
    content_root: &Path,
    local_node: &str,
    envelope_body: &[u8],
    now_ms: u64,
) -> Result<(), ClipboardMeshRefusal> {
    let envelope = ClipboardEnvelopeV2::from_json_bytes(envelope_body)
        .map_err(|error| ClipboardMeshRefusal::from_decode(&error))?;
    if envelope.source.node.as_str() != local_node {
        return Err(ClipboardMeshRefusal::UnauthorizedPeer);
    }
    authenticate_peer(
        directory,
        local_node,
        &envelope.attribution.pubkey_hex,
        true,
    )?;
    authenticate_peer(
        directory,
        envelope.target.node.as_str(),
        &directory
            .peer(envelope.target.node.as_str())
            .map_err(|_| ClipboardMeshRefusal::UnavailablePeer)?
            .ok_or(ClipboardMeshRefusal::UnavailablePeer)?
            .public_key_hex,
        true,
    )?;
    envelope
        .validate_at(now_ms, None)
        .map_err(|error| ClipboardMeshRefusal::from_validation(&error))?;
    validate_payload_budget(&envelope)?;
    validate_cas_offers(persist, content_root, local_node, &envelope)?;
    let frame = ClipboardMeshFrameV1::new(envelope)?;
    let body = serde_json::to_string(&frame).map_err(|_| ClipboardMeshRefusal::InvalidPayload)?;
    if body.len() > MAX_MESH_FRAME_BYTES {
        return Err(ClipboardMeshRefusal::Oversized);
    }
    persist
        .write(
            &mesh_frame_topic(&frame.target_peer),
            Priority::Default,
            None,
            Some(&body),
        )
        .map_err(|_| ClipboardMeshRefusal::UnavailablePeer)?;
    Ok(())
}

/// Receiver adapter: verify peer/key/target/session/generation and forward the
/// unchanged canonical envelope to the sole clipboard authority lane.
pub fn receive_frame(
    persist: &Persist,
    directory: &dyn MeshClipboardPeerDirectory,
    content_root: &Path,
    local_node: &str,
    frame_body: &[u8],
    ledger: &mut ClipboardMeshReplayLedger,
    now_ms: u64,
) -> Result<ClipboardMeshResultV1, ClipboardMeshRefusal> {
    let frame = ClipboardMeshFrameV1::from_json_bytes(frame_body)?;
    if frame.target_peer != local_node {
        return Err(ClipboardMeshRefusal::InvalidPayload);
    }
    authenticate_peer(
        directory,
        &frame.source_peer,
        &frame.envelope.attribution.pubkey_hex,
        true,
    )?;
    // Expired sessions no longer own a replay lane. Drop their high-water
    // marks before deriving `previous`, otherwise an expired hostile/high
    // generation can reject a fresh generation that legitimately reuses the
    // same bounded source/session identity.
    ledger.cleanup(now_ms);
    let previous = ledger.previous(&frame);
    frame
        .envelope
        .validate_at(now_ms, previous)
        .map_err(|error| ClipboardMeshRefusal::from_validation(&error))?;
    validate_payload_budget(&frame.envelope)?;
    validate_cas_offers(persist, content_root, &frame.source_peer, &frame.envelope)?;
    if ledger.previous(&frame).is_none() && ledger.latest.len() >= MAX_MESH_REPLAY_LANES {
        return Err(ClipboardMeshRefusal::FloodLimited);
    }
    let envelope_body =
        serde_json::to_string(&frame.envelope).map_err(|_| ClipboardMeshRefusal::InvalidPayload)?;
    persist
        .write(
            COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC,
            Priority::Default,
            None,
            Some(&envelope_body),
        )
        .map_err(|_| ClipboardMeshRefusal::UnavailablePeer)?;
    // Commit the replay generation only after the canonical authority accepted
    // the exact envelope. A transient Bus write failure must remain retryable.
    ledger.record(&frame)?;
    Ok(ClipboardMeshResultV1::Accepted {
        source_peer: frame.source_peer,
        target_peer: frame.target_peer,
        session: frame.envelope.session.to_string(),
        generation: frame.envelope.sequence,
    })
}

/// Target-specific topic consumed only by the named peer.
#[must_use]
pub fn mesh_frame_topic(target_peer: &str) -> String {
    format!("{MESH_FRAME_TOPIC_PREFIX}/{target_peer}")
}

/// Publish a bounded typed result. Result publication is best-effort and never
/// changes whether a clipboard generation was admitted.
pub fn publish_result(persist: &Persist, result: &ClipboardMeshResultV1) {
    if let Ok(body) = serde_json::to_string(result) {
        let _ = persist.write(MESH_RESULT_TOPIC, Priority::Min, None, Some(&body));
    }
}

fn authenticate_peer(
    directory: &dyn MeshClipboardPeerDirectory,
    node: &str,
    presented_key: &str,
    require_available: bool,
) -> Result<(), ClipboardMeshRefusal> {
    let peer = directory
        .peer(node)
        .map_err(|_| ClipboardMeshRefusal::UnavailablePeer)?
        .ok_or(ClipboardMeshRefusal::UnauthorizedPeer)?;
    if peer.node != node || peer.public_key_hex != presented_key {
        return Err(ClipboardMeshRefusal::UnauthorizedPeer);
    }
    if require_available && !peer.available {
        return Err(ClipboardMeshRefusal::UnavailablePeer);
    }
    Ok(())
}

fn validate_payload_budget(envelope: &ClipboardEnvelopeV2) -> Result<(), ClipboardMeshRefusal> {
    let mut logical_bytes = 0_u64;
    for offer in &envelope.offers {
        logical_bytes = logical_bytes
            .checked_add(offer.byte_count)
            .ok_or(ClipboardMeshRefusal::Oversized)?;
        match &offer.payload {
            ClipboardPayloadV2::InlineText { .. }
            | ClipboardPayloadV2::FilesReference { .. }
            | ClipboardPayloadV2::Unsupported { .. }
            | ClipboardPayloadV2::Unavailable { .. } => {}
        }
    }
    if logical_bytes > MAX_MESH_CLIP_LOGICAL_BYTES {
        return Err(ClipboardMeshRefusal::Oversized);
    }
    Ok(())
}

fn cas_digests(envelope: &ClipboardEnvelopeV2) -> BTreeSet<String> {
    envelope
        .offers
        .iter()
        .filter_map(|offer| match offer.payload {
            ClipboardPayloadV2::FilesReference { .. } => offer.content_sha256_hex.clone(),
            _ => None,
        })
        .collect()
}

/// Bind every Files-backed offer to the real collaboration Files projection
/// and exact canonical bytes. No caller-controlled path participates: the
/// object path is derived solely from the admitted lower-case SHA-256 digest.
fn validate_cas_offers(
    persist: &Persist,
    content_root: &Path,
    source_peer: &str,
    envelope: &ClipboardEnvelopeV2,
) -> Result<(), ClipboardMeshRefusal> {
    let files_offers: Vec<_> = envelope
        .offers
        .iter()
        .filter_map(|offer| match &offer.payload {
            ClipboardPayloadV2::FilesReference { file_ref } => Some((offer, file_ref)),
            _ => None,
        })
        .collect();
    if files_offers.is_empty() {
        return Ok(());
    }

    let topics = persist
        .list_topics()
        .map_err(|_| ClipboardMeshRefusal::CasUnavailable)?;
    let files_topics: Vec<_> = topics
        .into_iter()
        .filter(|topic| topic.starts_with(FILE_REFERENCES_TOPIC_PREFIX))
        .take(MAX_FILES_IDENTITY_TOPICS + 1)
        .collect();
    if files_topics.len() > MAX_FILES_IDENTITY_TOPICS {
        return Err(ClipboardMeshRefusal::FloodLimited);
    }
    for (offer, file_ref) in files_offers {
        let expected_digest = offer
            .content_sha256_hex
            .as_deref()
            .ok_or(ClipboardMeshRefusal::CasMismatch)?;
        let mut matched = false;
        for topic in &files_topics {
            let Some(message) = persist
                .read_latest(topic)
                .map_err(|_| ClipboardMeshRefusal::CasUnavailable)?
            else {
                continue;
            };
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            if body.len() > MAX_FILES_IDENTITY_BODY_BYTES {
                return Err(ClipboardMeshRefusal::CasMismatch);
            }
            reject_duplicate_json_keys(body.as_bytes())
                .map_err(|_| ClipboardMeshRefusal::CasMismatch)?;
            let references: FileReferences =
                serde_json::from_str(body).map_err(|_| ClipboardMeshRefusal::CasMismatch)?;
            for row in references
                .files
                .iter()
                .filter(|row| row.file == *file_ref && row.linked_by.as_str() == source_peer)
            {
                if row.reference.sha256_hex != expected_digest
                    || row.reference.size != offer.byte_count
                {
                    return Err(ClipboardMeshRefusal::CasMismatch);
                }
                matched = true;
            }
        }
        if !matched {
            return Err(ClipboardMeshRefusal::CasUnavailable);
        }
        verify_canonical_object(content_root, expected_digest, offer.byte_count)?;
    }
    Ok(())
}

fn verify_canonical_object(
    content_root: &Path,
    digest: &str,
    expected_size: u64,
) -> Result<(), ClipboardMeshRefusal> {
    if !lower_hex(digest, 64) {
        return Err(ClipboardMeshRefusal::CasMismatch);
    }
    let shard = content_root.join(&digest[..2]);
    let path = shard.join(digest);
    for directory in [content_root, shard.as_path()] {
        let metadata =
            fs::symlink_metadata(directory).map_err(|_| ClipboardMeshRefusal::CasUnavailable)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ClipboardMeshRefusal::CasMismatch);
        }
    }
    let metadata = fs::symlink_metadata(&path).map_err(|_| ClipboardMeshRefusal::CasUnavailable)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_size {
        return Err(ClipboardMeshRefusal::CasMismatch);
    }
    #[cfg(target_os = "linux")]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        OpenOptions::new()
            .read(true)
            .custom_flags(0o400000)
            .open(&path)
            .map_err(|_| ClipboardMeshRefusal::CasMismatch)?
    };
    #[cfg(not(target_os = "linux"))]
    let mut file = std::fs::File::open(&path).map_err(|_| ClipboardMeshRefusal::CasMismatch)?;
    let mut hash = Sha256::new();
    let mut observed = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ClipboardMeshRefusal::CasMismatch)?;
        if read == 0 {
            break;
        }
        observed = observed
            .checked_add(read as u64)
            .ok_or(ClipboardMeshRefusal::CasMismatch)?;
        if observed > expected_size {
            return Err(ClipboardMeshRefusal::CasMismatch);
        }
        hash.update(&buffer[..read]);
    }
    if observed != expected_size || format!("{:x}", hash.finalize()) != digest {
        return Err(ClipboardMeshRefusal::CasMismatch);
    }
    Ok(())
}

fn safe_node(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

fn lower_hex(value: &str, exact: usize) -> bool {
    value.len() == exact
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Read one cursor record without accepting a foreign topic.
pub fn read_mesh_cursor(path: &Path, topic: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let stored_topic = value.get("topic")?.as_str()?;
    let ulid = value.get("ulid")?.as_str()?;
    (stored_topic == topic && !ulid.is_empty()).then(|| ulid.to_owned())
}

/// Atomically checkpoint a payload-free topic/ULID cursor.
pub fn write_mesh_cursor(path: &Path, topic: &str, ulid: &str) -> Result<(), String> {
    if !safe_node(ulid) || topic.is_empty() {
        return Err("invalid clipboard mesh cursor".to_owned());
    }
    let body = serde_json::to_vec(&serde_json::json!({"topic": topic, "ulid": ulid}))
        .map_err(|error| format!("encode clipboard mesh cursor: {error}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create clipboard mesh cursor parent: {error}"))?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, body)
        .map_err(|error| format!("write clipboard mesh cursor: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("commit clipboard mesh cursor: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use mde_collab_types::{
        value::sha256_hex, ActorId, ClipboardClipId, ClipboardMimeKind, ClipboardMimeOfferV2,
        ClipboardNodeId, ClipboardSessionId, ClipboardSourceV2, ClipboardTargetV2, FileRef,
        FileRefId, FileReferenceView, SpaceId,
    };

    const XPROC_ROLE_ENV: &str = "MCNF_CLIPBOARD_MESH_XPROC_ROLE";
    const XPROC_ROOT_ENV: &str = "MCNF_CLIPBOARD_MESH_XPROC_ROOT";
    const XPROC_TEST_FILTER: &str =
        "mesh_cross_process_persist_sqlite_preserves_rich_payload_and_security_state";
    const XPROC_RICH_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\0mesh-rich-clipboard\xff\x10exact";

    #[derive(Default)]
    struct TestDirectory(BTreeMap<String, MeshClipboardPeer>);

    impl TestDirectory {
        fn insert(&mut self, node: &str, key: &SigningKey, available: bool) {
            self.0.insert(
                node.to_owned(),
                MeshClipboardPeer {
                    node: node.to_owned(),
                    public_key_hex: hex(key.verifying_key().as_bytes()),
                    available,
                },
            );
        }
    }

    impl MeshClipboardPeerDirectory for TestDirectory {
        fn peer(&self, node: &str) -> Result<Option<MeshClipboardPeer>, String> {
            Ok(self.0.get(node).cloned())
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn envelope(key: &SigningKey, sequence: u64, created: u64) -> ClipboardEnvelopeV2 {
        ClipboardEnvelopeV2::new(
            ClipboardClipId::new(),
            ClipboardSourceV2::new(
                ClipboardNodeId::new("source").unwrap(),
                mde_collab_types::ClipboardSeatId::new("seat0").unwrap(),
            ),
            ClipboardTargetV2::new(
                ClipboardNodeId::new("target").unwrap(),
                mde_collab_types::ClipboardSeatId::new("seat0").unwrap(),
            ),
            ClipboardSessionId::new(),
            sequence,
            created,
            created + 10_000,
            vec![ClipboardMimeOfferV2::inline_text(
                ClipboardMimeKind::TextPlain,
                "exact mesh bytes",
            )
            .unwrap()],
        )
        .unwrap()
        .signed(key)
    }

    fn fixture() -> (
        tempfile::TempDir,
        Persist,
        TestDirectory,
        SigningKey,
        ClipboardEnvelopeV2,
    ) {
        let root = tempfile::tempdir().unwrap();
        let persist = Persist::open(root.path().to_path_buf()).unwrap();
        let source = SigningKey::from_bytes(&[7; 32]);
        let target = SigningKey::from_bytes(&[8; 32]);
        let mut directory = TestDirectory::default();
        directory.insert("source", &source, true);
        directory.insert("target", &target, true);
        let envelope = envelope(&source, 1, 1_000);
        (root, persist, directory, source, envelope)
    }

    fn publish_cas_fixture(
        root: &Path,
        persist: &Persist,
        bytes: &[u8],
    ) -> (PathBuf, FileRefId, String) {
        let digest = sha256_hex(bytes);
        let content_root = root.join("collab/content");
        let shard = content_root.join(&digest[..2]);
        fs::create_dir_all(&shard).unwrap();
        let object = shard.join(&digest);
        fs::write(&object, bytes).unwrap();
        let file = FileRefId::from_uuid(uuid::Uuid::from_u128(0xcafe));
        let space = SpaceId::from_uuid(uuid::Uuid::from_u128(0xbeef));
        let references = FileReferences {
            space,
            files: vec![FileReferenceView {
                file,
                reference: FileRef {
                    name: "bounded-rich.png".to_owned(),
                    size: bytes.len() as u64,
                    sha256_hex: digest.clone(),
                    mime: Some("image/png".to_owned()),
                },
                linked_by: ActorId::new("source"),
                linked_unix_ms: 900,
            }],
        };
        persist
            .write(
                &format!("{FILE_REFERENCES_TOPIC_PREFIX}{space}"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&references).unwrap()),
            )
            .unwrap();
        (content_root, file, digest)
    }

    fn cas_envelope(
        key: &SigningKey,
        file: FileRefId,
        digest: &str,
        byte_count: u64,
        sequence: u64,
    ) -> ClipboardEnvelopeV2 {
        ClipboardEnvelopeV2::new(
            ClipboardClipId::new(),
            ClipboardSourceV2::new(
                ClipboardNodeId::new("source").unwrap(),
                mde_collab_types::ClipboardSeatId::new("seat0").unwrap(),
            ),
            ClipboardTargetV2::new(
                ClipboardNodeId::new("target").unwrap(),
                mde_collab_types::ClipboardSeatId::new("seat0").unwrap(),
            ),
            ClipboardSessionId::from_uuid(uuid::Uuid::from_u128(0x1234)),
            sequence,
            1_000,
            11_000,
            vec![ClipboardMimeOfferV2::files_reference(
                ClipboardMimeKind::ImagePng,
                file,
                byte_count,
                digest,
            )
            .unwrap()],
        )
        .unwrap()
        .signed(key)
    }

    fn xproc_rich_envelope(
        key: &SigningKey,
        sequence: u64,
        created_unix_ms: u64,
        expires_unix_ms: u64,
    ) -> ClipboardEnvelopeV2 {
        let digest = sha256_hex(XPROC_RICH_BYTES);
        ClipboardEnvelopeV2::new(
            ClipboardClipId::from_uuid(uuid::Uuid::from_u128(0xfeed)),
            ClipboardSourceV2::new(
                ClipboardNodeId::new("source").unwrap(),
                mde_collab_types::ClipboardSeatId::new("seat0").unwrap(),
            ),
            ClipboardTargetV2::new(
                ClipboardNodeId::new("target").unwrap(),
                mde_collab_types::ClipboardSeatId::new("seat0").unwrap(),
            ),
            ClipboardSessionId::from_uuid(uuid::Uuid::from_u128(0xf00d)),
            sequence,
            created_unix_ms,
            expires_unix_ms,
            vec![
                ClipboardMimeOfferV2::inline_text(
                    ClipboardMimeKind::TextPlain,
                    "exact mesh text\r\nwith unicode Ω",
                )
                .unwrap(),
                ClipboardMimeOfferV2::inline_text(
                    ClipboardMimeKind::TextHtml,
                    "<p>exact <strong>rich</strong> mesh Ω</p>",
                )
                .unwrap(),
                ClipboardMimeOfferV2::files_reference(
                    ClipboardMimeKind::ImagePng,
                    FileRefId::from_uuid(uuid::Uuid::from_u128(0xcafe)),
                    XPROC_RICH_BYTES.len() as u64,
                    digest,
                )
                .unwrap(),
            ],
        )
        .unwrap()
        .signed(key)
    }

    fn xproc_paths() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = PathBuf::from(std::env::var_os(XPROC_ROOT_ENV).unwrap());
        (
            root.clone(),
            root.join("peers.sqlite"),
            root.join("receiver-cursor.json"),
            root.join("collab/content"),
        )
    }

    fn run_xproc_child(role: &str) {
        let (bus_root, peers_db, cursor_path, content_root) = xproc_paths();
        let persist = Persist::open(bus_root).unwrap();
        let directory = SqliteMeshClipboardPeerDirectory::new(peers_db);
        let topic = mesh_frame_topic("target");
        match role {
            "sender" => {
                let request = persist.read_latest(MESH_SEND_TOPIC).unwrap().unwrap();
                send_envelope(
                    &persist,
                    &directory,
                    &content_root,
                    "source",
                    request.body.unwrap().as_bytes(),
                    1_001,
                )
                .unwrap();
            }
            "receiver" => {
                assert!(read_mesh_cursor(&cursor_path, &topic).is_none());
                let frames = persist.list_since(&topic, None).unwrap();
                assert_eq!(frames.len(), 1);
                let mut ledger = ClipboardMeshReplayLedger::default();
                ledger.seed_from_retained(&persist, 1_002);
                let result = receive_frame(
                    &persist,
                    &directory,
                    &content_root,
                    "target",
                    frames[0].body.as_deref().unwrap().as_bytes(),
                    &mut ledger,
                    1_002,
                )
                .unwrap();
                assert_eq!(
                    result,
                    ClipboardMeshResultV1::Accepted {
                        source_peer: "source".to_owned(),
                        target_peer: "target".to_owned(),
                        session: ClipboardSessionId::from_uuid(uuid::Uuid::from_u128(0xf00d))
                            .to_string(),
                        generation: 1,
                    }
                );
                assert_eq!(
                    fs::read(
                        content_root
                            .join(&sha256_hex(XPROC_RICH_BYTES)[..2])
                            .join(sha256_hex(XPROC_RICH_BYTES)),
                    )
                    .unwrap(),
                    XPROC_RICH_BYTES
                );
                write_mesh_cursor(&cursor_path, &topic, &frames[0].ulid).unwrap();
            }
            "replay" => {
                let cursor = read_mesh_cursor(&cursor_path, &topic).unwrap();
                assert!(persist
                    .list_since(&topic, Some(&cursor))
                    .unwrap()
                    .is_empty());
                let retained = persist.read_latest(&topic).unwrap().unwrap();
                let mut ledger = ClipboardMeshReplayLedger::default();
                ledger.seed_from_retained(&persist, 1_003);
                assert_eq!(
                    receive_frame(
                        &persist,
                        &directory,
                        &content_root,
                        "target",
                        retained.body.as_deref().unwrap().as_bytes(),
                        &mut ledger,
                        1_003,
                    ),
                    Err(ClipboardMeshRefusal::Replayed)
                );
            }
            "expired" | "identity" => {
                let cursor = read_mesh_cursor(&cursor_path, &topic).unwrap();
                let frames = persist.list_since(&topic, Some(&cursor)).unwrap();
                assert_eq!(frames.len(), 1);
                let mut ledger = ClipboardMeshReplayLedger::default();
                ledger.seed_from_retained(&persist, 20_000);
                let expected = if role == "expired" {
                    ClipboardMeshRefusal::Stale
                } else {
                    ClipboardMeshRefusal::UnauthorizedPeer
                };
                assert_eq!(
                    receive_frame(
                        &persist,
                        &directory,
                        &content_root,
                        "target",
                        frames[0].body.as_deref().unwrap().as_bytes(),
                        &mut ledger,
                        20_000,
                    ),
                    Err(expected)
                );
                write_mesh_cursor(&cursor_path, &topic, &frames[0].ulid).unwrap();
            }
            "forward-only" => {
                let cursor = read_mesh_cursor(&cursor_path, &topic).unwrap();
                assert!(persist
                    .list_since(&topic, Some(&cursor))
                    .unwrap()
                    .is_empty());
                let delivered = persist
                    .list_since(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC, None)
                    .unwrap();
                assert_eq!(delivered.len(), 1);
                let expected =
                    xproc_rich_envelope(&SigningKey::from_bytes(&[7; 32]), 1, 1_000, 11_000);
                assert_eq!(
                    ClipboardEnvelopeV2::from_json_bytes(
                        delivered[0].body.as_deref().unwrap().as_bytes()
                    )
                    .unwrap(),
                    expected
                );
            }
            other => panic!("unknown cross-process clipboard role {other}"),
        }
    }

    fn spawn_xproc_role(root: &Path, role: &str) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(XPROC_TEST_FILTER)
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(XPROC_ROLE_ENV, role)
            .env(XPROC_ROOT_ENV, root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cross-process role {role} failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn mesh_cross_process_persist_sqlite_preserves_rich_payload_and_security_state() {
        if let Ok(role) = std::env::var(XPROC_ROLE_ENV) {
            run_xproc_child(&role);
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let persist = Persist::open(root.path().to_path_buf()).unwrap();
        let source = SigningKey::from_bytes(&[7; 32]);
        let target = SigningKey::from_bytes(&[8; 32]);
        let peers_db = root.path().join("peers.sqlite");
        let peers = Connection::open(&peers_db).unwrap();
        peers
            .execute_batch(
                "CREATE TABLE nodes (\
                    node_id TEXT PRIMARY KEY, name TEXT NOT NULL, public_key TEXT NOT NULL, \
                    role TEXT NOT NULL, health TEXT NOT NULL\
                 );",
            )
            .unwrap();
        for (node, key) in [("source", &source), ("target", &target)] {
            peers
                .execute(
                    "INSERT INTO nodes (node_id, name, public_key, role, health) \
                     VALUES (?1, ?2, ?3, 'peer', 'healthy')",
                    (
                        format!("peer:{node}"),
                        node,
                        hex(key.verifying_key().as_bytes()),
                    ),
                )
                .unwrap();
        }
        drop(peers);
        let (_content_root, file, digest) =
            publish_cas_fixture(root.path(), &persist, XPROC_RICH_BYTES);
        assert_eq!(file, FileRefId::from_uuid(uuid::Uuid::from_u128(0xcafe)));
        assert_eq!(digest, sha256_hex(XPROC_RICH_BYTES));
        let fresh = xproc_rich_envelope(&source, 1, 1_000, 11_000);
        persist
            .write(
                MESH_SEND_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&fresh).unwrap()),
            )
            .unwrap();
        drop(persist);

        spawn_xproc_role(root.path(), "sender");
        spawn_xproc_role(root.path(), "receiver");
        spawn_xproc_role(root.path(), "replay");

        let persist = Persist::open(root.path().to_path_buf()).unwrap();
        let expired =
            ClipboardMeshFrameV1::new(xproc_rich_envelope(&source, 2, 1_100, 1_200)).unwrap();
        persist
            .write(
                &mesh_frame_topic("target"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&expired).unwrap()),
            )
            .unwrap();
        drop(persist);
        spawn_xproc_role(root.path(), "expired");

        let persist = Persist::open(root.path().to_path_buf()).unwrap();
        let attacker = SigningKey::from_bytes(&[9; 32]);
        let forged =
            ClipboardMeshFrameV1::new(xproc_rich_envelope(&attacker, 3, 19_000, 21_000)).unwrap();
        persist
            .write(
                &mesh_frame_topic("target"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&forged).unwrap()),
            )
            .unwrap();
        drop(persist);
        spawn_xproc_role(root.path(), "identity");
        spawn_xproc_role(root.path(), "forward-only");
    }

    #[test]
    fn authenticated_sender_receiver_preserves_exact_canonical_bytes() {
        let (root, persist, directory, _source, envelope) = fixture();
        let body = serde_json::to_vec(&envelope).unwrap();
        send_envelope(&persist, &directory, root.path(), "source", &body, 1_001).unwrap();
        let frame_row = persist
            .read_latest(&mesh_frame_topic("target"))
            .unwrap()
            .unwrap();
        let mut ledger = ClipboardMeshReplayLedger::default();
        let result = receive_frame(
            &persist,
            &directory,
            root.path(),
            "target",
            frame_row.body.unwrap().as_bytes(),
            &mut ledger,
            1_002,
        )
        .unwrap();
        assert!(matches!(
            result,
            ClipboardMeshResultV1::Accepted { generation: 1, .. }
        ));
        let delivered = persist
            .read_latest(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC)
            .unwrap()
            .unwrap()
            .body
            .unwrap();
        assert_eq!(
            serde_json::from_str::<ClipboardEnvelopeV2>(&delivered).unwrap(),
            envelope
        );
    }

    #[test]
    fn files_cas_bytes_identity_dedupe_denial_and_lease_cleanup_are_bound() {
        let (root, persist, directory, source, _inline) = fixture();
        let bytes = b"\x89PNG\r\n\x1a\nnon-secret bounded rich clipboard fixture";
        let (content_root, file, digest) = publish_cas_fixture(root.path(), &persist, bytes);
        for index in 0..=MAX_FILES_IDENTITY_TOPICS {
            persist
                .write(
                    &format!("state/unrelated/busy-bus/{index}"),
                    Priority::Min,
                    None,
                    Some("unrelated"),
                )
                .unwrap();
        }
        let envelope = cas_envelope(&source, file, &digest, bytes.len() as u64, 1);
        let body = serde_json::to_vec(&envelope).unwrap();

        send_envelope(&persist, &directory, &content_root, "source", &body, 1_001).unwrap();
        let frame = persist
            .read_latest(&mesh_frame_topic("target"))
            .unwrap()
            .unwrap()
            .body
            .unwrap();
        let mut ledger = ClipboardMeshReplayLedger::default();
        receive_frame(
            &persist,
            &directory,
            &content_root,
            "target",
            frame.as_bytes(),
            &mut ledger,
            1_002,
        )
        .unwrap();
        let marker = ledger.latest.values().next().unwrap();
        assert_eq!(marker.cas_digests, BTreeSet::from([digest.clone()]));
        assert_eq!(
            receive_frame(
                &persist,
                &directory,
                &content_root,
                "target",
                frame.as_bytes(),
                &mut ledger,
                1_003,
            ),
            Err(ClipboardMeshRefusal::Replayed)
        );

        let object = content_root.join(&digest[..2]).join(&digest);
        fs::write(&object, vec![b'x'; bytes.len()]).unwrap();
        let changed = cas_envelope(&source, file, &digest, bytes.len() as u64, 2);
        assert_eq!(
            send_envelope(
                &persist,
                &directory,
                &content_root,
                "source",
                &serde_json::to_vec(&changed).unwrap(),
                1_004,
            ),
            Err(ClipboardMeshRefusal::CasMismatch)
        );
        fs::write(&object, bytes).unwrap();
        fs::remove_file(&object).unwrap();
        assert_eq!(
            send_envelope(
                &persist,
                &directory,
                &content_root,
                "source",
                &serde_json::to_vec(&changed).unwrap(),
                1_005,
            ),
            Err(ClipboardMeshRefusal::CasUnavailable)
        );
        fs::write(&object, bytes).unwrap();
        assert_eq!(ledger.cleanup(11_000), 1);
        assert!(ledger.latest.is_empty());
        assert_eq!(fs::read(object).unwrap(), bytes);
    }

    #[test]
    fn files_cas_projection_with_duplicate_field_is_refused_before_authority_use() {
        let (root, persist, directory, source, _inline) = fixture();
        let bytes = b"bounded duplicate-key CAS projection fixture";
        let (content_root, file, digest) = publish_cas_fixture(root.path(), &persist, bytes);
        let topic = persist
            .list_topics()
            .unwrap()
            .into_iter()
            .find(|topic| topic.starts_with(FILE_REFERENCES_TOPIC_PREFIX))
            .unwrap();
        let body = persist.read_latest(&topic).unwrap().unwrap().body.unwrap();
        let duplicate = body.replacen("{", "{\"space\":\"duplicate-authority\",", 1);
        persist
            .write(&topic, Priority::Default, None, Some(&duplicate))
            .unwrap();
        let envelope = cas_envelope(&source, file, &digest, bytes.len() as u64, 1);
        assert_eq!(
            send_envelope(
                &persist,
                &directory,
                &content_root,
                "source",
                &serde_json::to_vec(&envelope).unwrap(),
                1_001,
            ),
            Err(ClipboardMeshRefusal::CasMismatch)
        );
        assert!(persist
            .read_latest(&mesh_frame_topic("target"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn hostile_unauthorized_peer_and_source_key_mismatch_fail_closed() {
        let (root, persist, mut directory, _source, envelope) = fixture();
        directory.0.remove("source");
        assert_eq!(
            send_envelope(
                &persist,
                &directory,
                root.path(),
                "source",
                &serde_json::to_vec(&envelope).unwrap(),
                1_001,
            ),
            Err(ClipboardMeshRefusal::UnauthorizedPeer)
        );
        let attacker = SigningKey::from_bytes(&[9; 32]);
        directory.insert("source", &attacker, true);
        assert_eq!(
            send_envelope(
                &persist,
                &directory,
                root.path(),
                "source",
                &serde_json::to_vec(&envelope).unwrap(),
                1_001,
            ),
            Err(ClipboardMeshRefusal::UnauthorizedPeer)
        );
    }

    #[test]
    fn hostile_replay_is_typed_and_never_forwards_twice() {
        let (root, persist, directory, _source, admitted_envelope) = fixture();
        let frame = ClipboardMeshFrameV1::new(admitted_envelope).unwrap();
        let body = serde_json::to_vec(&frame).unwrap();
        let mut ledger = ClipboardMeshReplayLedger::default();
        receive_frame(
            &persist,
            &directory,
            root.path(),
            "target",
            &body,
            &mut ledger,
            1_001,
        )
        .unwrap();
        assert_eq!(
            receive_frame(
                &persist,
                &directory,
                root.path(),
                "target",
                &body,
                &mut ledger,
                1_002,
            ),
            Err(ClipboardMeshRefusal::Replayed)
        );
        assert_eq!(
            persist
                .list_since(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC, None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn hostile_oversize_flood_and_raw_path_fields_are_refused() {
        let (_root, persist, directory, _source, admitted_envelope) = fixture();
        assert_eq!(
            ClipboardMeshFrameV1::from_json_bytes(&vec![b'x'; MAX_MESH_FRAME_BYTES + 1]),
            Err(ClipboardMeshRefusal::Oversized)
        );
        let mut value =
            serde_json::to_value(ClipboardMeshFrameV1::new(admitted_envelope).unwrap()).unwrap();
        value.as_object_mut().unwrap().insert(
            "raw_host_path".to_owned(),
            serde_json::Value::String("/home/operator/secret".to_owned()),
        );
        assert_eq!(
            ClipboardMeshFrameV1::from_json_bytes(value.to_string().as_bytes()),
            Err(ClipboardMeshRefusal::InvalidPayload)
        );

        let valid = serde_json::to_string(
            &ClipboardMeshFrameV1::new(envelope(&SigningKey::from_bytes(&[7; 32]), 3, 1_000))
                .unwrap(),
        )
        .unwrap();
        let duplicate = valid.replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
            1,
        );
        assert_eq!(
            ClipboardMeshFrameV1::from_json_bytes(duplicate.as_bytes()),
            Err(ClipboardMeshRefusal::InvalidPayload)
        );

        let mut ledger = ClipboardMeshReplayLedger::default();
        for index in 0..MAX_MESH_REPLAY_LANES {
            ledger.latest.insert(
                (format!("peer-{index}"), format!("session-{index}")),
                ReplayMarker {
                    generation: 1,
                    expires_unix_ms: 20_000,
                    cas_digests: BTreeSet::new(),
                },
            );
        }
        let source = SigningKey::from_bytes(&[7; 32]);
        let new_frame = ClipboardMeshFrameV1::new(envelope(&source, 2, 1_000)).unwrap();
        assert_eq!(
            ledger.record(&new_frame),
            Err(ClipboardMeshRefusal::FloodLimited)
        );
        assert!(persist
            .list_since(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC, None)
            .unwrap()
            .is_empty());
        drop(directory);
    }

    #[test]
    fn hostile_stale_and_unavailable_peer_are_distinct() {
        let (root, persist, mut directory, _source, envelope) = fixture();
        let body = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(
            send_envelope(&persist, &directory, root.path(), "source", &body, 11_000),
            Err(ClipboardMeshRefusal::Stale)
        );
        directory.0.get_mut("target").unwrap().available = false;
        assert_eq!(
            send_envelope(&persist, &directory, root.path(), "source", &body, 1_001),
            Err(ClipboardMeshRefusal::UnavailablePeer)
        );
    }

    #[test]
    fn cleanup_releases_only_expired_payload_free_replay_markers() {
        let mut ledger = ClipboardMeshReplayLedger::default();
        ledger.latest.insert(
            ("old".to_owned(), "session-old".to_owned()),
            ReplayMarker {
                generation: 7,
                expires_unix_ms: 1_000,
                cas_digests: BTreeSet::new(),
            },
        );
        ledger.latest.insert(
            ("fresh".to_owned(), "session-fresh".to_owned()),
            ReplayMarker {
                generation: 8,
                expires_unix_ms: 2_000,
                cas_digests: BTreeSet::new(),
            },
        );
        assert_eq!(ledger.cleanup(1_000), 1);
        assert_eq!(ledger.latest.len(), 1);
        assert!(ledger
            .latest
            .contains_key(&("fresh".to_owned(), "session-fresh".to_owned())));
    }

    #[test]
    fn expired_hostile_generation_cannot_block_fresh_session_reuse() {
        let (root, persist, directory, _source, envelope) = fixture();
        let frame = ClipboardMeshFrameV1::new(envelope).unwrap();
        let lane = (
            frame.source_peer.clone(),
            frame.envelope.session.to_string(),
        );
        let mut ledger = ClipboardMeshReplayLedger::default();
        ledger.latest.insert(
            lane.clone(),
            ReplayMarker {
                generation: u64::MAX,
                expires_unix_ms: 1_000,
                cas_digests: BTreeSet::new(),
            },
        );

        let result = receive_frame(
            &persist,
            &directory,
            root.path(),
            "target",
            &serde_json::to_vec(&frame).unwrap(),
            &mut ledger,
            1_001,
        )
        .unwrap();

        assert!(matches!(
            result,
            ClipboardMeshResultV1::Accepted { generation: 1, .. }
        ));
        assert_eq!(ledger.previous(&frame), Some(1));
        assert_eq!(
            persist
                .list_since(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC, None)
                .unwrap()
                .len(),
            1
        );
        assert!(ledger.latest.contains_key(&lane));
    }

    #[test]
    fn restart_seed_rejects_a_generation_already_forwarded_to_canonical_authority() {
        let (root, persist, directory, _source, envelope) = fixture();
        persist
            .write(
                COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&envelope).unwrap()),
            )
            .unwrap();
        let mut ledger = ClipboardMeshReplayLedger::default();
        ledger.seed_from_retained(&persist, 1_001);
        let frame = ClipboardMeshFrameV1::new(envelope).unwrap();
        assert_eq!(
            receive_frame(
                &persist,
                &directory,
                root.path(),
                "target",
                &serde_json::to_vec(&frame).unwrap(),
                &mut ledger,
                1_002,
            ),
            Err(ClipboardMeshRefusal::Replayed)
        );
    }
}
