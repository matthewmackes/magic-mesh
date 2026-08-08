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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_collab_types::{
    reject_duplicate_json_keys, ClipboardDenialReasonV2, ClipboardEnvelopeV2,
    ClipboardEnvelopeV2DecodeError, ClipboardEnvelopeV2ValidationError, ClipboardPayloadV2,
    MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

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
            },
        );
        Ok(())
    }
}

/// Sender adapter: authenticate both endpoints and emit a target-specific frame.
pub fn send_envelope(
    persist: &Persist,
    directory: &dyn MeshClipboardPeerDirectory,
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
    let previous = ledger.previous(&frame);
    frame
        .envelope
        .validate_at(now_ms, previous)
        .map_err(|error| ClipboardMeshRefusal::from_validation(&error))?;
    validate_payload_budget(&frame.envelope)?;
    ledger.cleanup(now_ms);
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
        ClipboardClipId, ClipboardMimeKind, ClipboardMimeOfferV2, ClipboardNodeId,
        ClipboardSessionId, ClipboardSourceV2, ClipboardTargetV2,
    };

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

    #[test]
    fn authenticated_sender_receiver_preserves_exact_canonical_bytes() {
        let (_root, persist, directory, _source, envelope) = fixture();
        let body = serde_json::to_vec(&envelope).unwrap();
        send_envelope(&persist, &directory, "source", &body, 1_001).unwrap();
        let frame_row = persist
            .read_latest(&mesh_frame_topic("target"))
            .unwrap()
            .unwrap();
        let mut ledger = ClipboardMeshReplayLedger::default();
        let result = receive_frame(
            &persist,
            &directory,
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
    fn hostile_unauthorized_peer_and_source_key_mismatch_fail_closed() {
        let (_root, persist, mut directory, _source, envelope) = fixture();
        directory.0.remove("source");
        assert_eq!(
            send_envelope(
                &persist,
                &directory,
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
                "source",
                &serde_json::to_vec(&envelope).unwrap(),
                1_001,
            ),
            Err(ClipboardMeshRefusal::UnauthorizedPeer)
        );
    }

    #[test]
    fn hostile_replay_is_typed_and_never_forwards_twice() {
        let (_root, persist, directory, _source, admitted_envelope) = fixture();
        let frame = ClipboardMeshFrameV1::new(admitted_envelope).unwrap();
        let body = serde_json::to_vec(&frame).unwrap();
        let mut ledger = ClipboardMeshReplayLedger::default();
        receive_frame(&persist, &directory, "target", &body, &mut ledger, 1_001).unwrap();
        assert_eq!(
            receive_frame(&persist, &directory, "target", &body, &mut ledger, 1_002),
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
        let (_root, persist, mut directory, _source, envelope) = fixture();
        let body = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(
            send_envelope(&persist, &directory, "source", &body, 11_000),
            Err(ClipboardMeshRefusal::Stale)
        );
        directory.0.get_mut("target").unwrap().available = false;
        assert_eq!(
            send_envelope(&persist, &directory, "source", &body, 1_001),
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
            },
        );
        ledger.latest.insert(
            ("fresh".to_owned(), "session-fresh".to_owned()),
            ReplayMarker {
                generation: 8,
                expires_unix_ms: 2_000,
            },
        );
        assert_eq!(ledger.cleanup(1_000), 1);
        assert_eq!(ledger.latest.len(), 1);
        assert!(ledger
            .latest
            .contains_key(&("fresh".to_owned(), "session-fresh".to_owned())));
    }

    #[test]
    fn restart_seed_rejects_a_generation_already_forwarded_to_canonical_authority() {
        let (_root, persist, directory, _source, envelope) = fixture();
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
                "target",
                &serde_json::to_vec(&frame).unwrap(),
                &mut ledger,
                1_002,
            ),
            Err(ClipboardMeshRefusal::Replayed)
        );
    }
}
