//! CLIP-SYNC-1 — mesh clipboard history worker.
//!
//! Consumes canonical text-clipboard events from the Mackes Bus
//! (`event/clipboard/clip`) and appends them to ONE mesh-global history file on
//! the QNM-Shared replicated root (`<root>/clipboard/history.json`). Every peer
//! runs this worker; the single shared file is the mesh-global clipboard (no
//! per-user/per-node partition — the single-operator model, design lock O8).
//!
//! The canonical event body is `{ id, text, source, time }`. `id` is the stable
//! content fingerprint, `source` is the producer node/lane, and `time` is an
//! RFC3339 timestamp. The worker deliberately does not read the OS clipboard or
//! shell out to compositor-specific tools; seat, browser, KDC/mobile, and VDI
//! producers publish the shared lane and this worker folds that lane into
//! durable history.
//!
//! Operator locks (design `docs/design/notify-hub-redesign.md`, survey round 1,
//! 2026-06-18):
//!   * O2 echo-loop — **debounce identical content**: a copy whose text
//!     equals the most-recent applied clip is dropped. This is what kills
//!     the click-to-load echo without origin-tagging the selection.
//!   * O3 dedup — **move-to-top**: re-copying existing text bumps the one
//!     entry to the front instead of duplicating.
//!   * O4 no size cap — any text length syncs (the bus-retention worker
//!     bounds the bus; the history stays at 50 + pinned).
//!   * O6 stamp — each entry carries its source node + an RFC3339 time so
//!     the viewer renders "from <node> · <age>".
//!   * O7 pins — pinned entries are **exempt from the 50-cap and
//!     unlimited**; only unpinned entries are trimmed.
//!
//! The history mutations (`apply_clip` / `apply_clip_event`) are pure + fully
//! unit-tested; the worker body is the I/O glue (tail the Bus lane and
//! read/merge/write the shared file under the shared-root guard). The
//! `action/clipboard/*` IPC responder (`ipc::clipboard`) edits the same file for
//! the viewer's delete/pin/clear verbs.
//!
//! **Concurrency.** Each writer (this worker, the IPC responder, every
//! peer) does an unlocked read → mutate → atomic-`rename` write of the one
//! shared `history.json` — the same last-writer-wins shape the sibling
//! shared-state responders (`ipc::connect`, the peer directory) use against
//! the replicated root. The atomic rename prevents a torn read; a rare
//! concurrent pin-vs-capture can lose one update, self-healing on the next
//! capture. A real clipboard never sustains the write rate where this
//! matters, so a cross-node lock is deliberately not taken here (it would
//! add a Syncthing round-trip to every copy).

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmSigner, CloudArmedToken};
use mackes_mesh_types::vdi_clipboard::{
    ClipboardMaterialization, VdiClipboardText, CLIPBOARD_MATERIALIZATION_TOPIC,
};
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

pub mod session;

use super::clipboard_bridge::{ClipDirection, ClipPayload, ClipboardEvent};
use super::session_broker::{EtcdSessionStore, MeshSessionStore, SessionState, SessionStore};
use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{production_action_signer, ACTION_SCHEMA_VERSION, MAX_AUTH_TTL_MS};

/// The daemon-owned VNC-to-seat handoff is deliberately narrower than the
/// general clipboard action lane: only the shell's truthful VNC source form is
/// eligible for conversion, and only an active session record can supply the
/// destination seat.
const VNC_SOURCE_PREFIX: &str = "vnc:";

/// The shared text-only clipboard ceiling. Keep mesh history on the same
/// bounded contract as the VDI bridge; oversized payloads must never become
/// durable replicated state.
const MAX_CLIP_BYTES: usize = super::clipboard_bridge::MAX_CLIP_BYTES;

/// Non-pinned entries kept in the shared history (O7: pins are exempt +
/// unlimited, so the real file can be longer than this).
pub const HISTORY_CAP: usize = 50;

/// Bus topic every text clip is broadcast on. The viewer + any tailing
/// consumer subscribe here for real-time updates; the durable record is
/// the history file.
pub const CLIP_TOPIC: &str = "event/clipboard/clip";

/// Per-daemon cursor file kept beside the local Bus log. It is deliberately
/// not stored under the replicated workgroup root: each daemon must acknowledge
/// the canonical lane independently, while the history itself is mesh-global.
const CURSOR_FILE_NAME: &str = "clipboard-sync.cursor.json";

/// Bus-drain cadence for the canonical clipboard event lane.
pub const CLIP_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// One clipboard entry in the mesh-global history. `id` is a stable
/// content fingerprint so the viewer/IPC can address an entry (pin/delete)
/// without shipping the full text back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipEntry {
    /// Stable id (content fingerprint) — addresses the entry for pin/delete.
    pub id: String,
    /// The clip text (verbatim; O4 — no size cap, no secret filtering).
    pub text: String,
    /// Node that captured the clip (O6 source attribution).
    pub source: String,
    /// RFC3339 capture timestamp (O6 — the viewer renders relative age).
    pub time: String,
    /// O7 — pinned entries survive the cap + a mesh-wide clear.
    #[serde(default)]
    pub pinned: bool,
}

/// Canonical `event/clipboard/clip` Bus body.
///
/// Keep this shape compatible with existing producers and consumers: exactly the
/// public clipboard event fields `{ id, text, source, time }`. Durable history
/// adds `pinned` locally, but event producers cannot set it through this body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipEventBody {
    /// Stable id (content fingerprint) — addresses the entry for pin/delete.
    pub id: String,
    /// The clip text (verbatim; O4 — no size cap, no secret filtering).
    pub text: String,
    /// Producer node/lane that emitted the event.
    pub source: String,
    /// RFC3339 capture timestamp.
    pub time: String,
}

impl ClipEventBody {
    /// Build the canonical event body from local text/source/time inputs.
    #[must_use]
    pub fn from_text(text: &str, source: &str, time: &str) -> Self {
        Self {
            id: clip_id(text),
            text: text.to_string(),
            source: source.to_string(),
            time: time.to_string(),
        }
    }
}

/// The mesh-global clipboard history (newest first). Serialized as the
/// whole `clipboard/history.json` document so a tailing node reads one
/// stable shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// Entries, newest first (index 0 is the current clipboard top).
    #[serde(default)]
    pub entries: Vec<ClipEntry>,
}

/// Content fingerprint for an entry id — a short hex SHA-256 prefix of the
/// text. Stable across nodes so the same clip dedups to one id mesh-wide.
#[must_use]
pub fn clip_id(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    // 16 hex chars (64 bits) is ample to avoid collisions across a 50+pin
    // history while staying short in the JSON + the bus body.
    let mut s = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Apply a freshly captured clip to the history (pure — the whole O2/O3/O7
/// policy lives here, unit-tested without any I/O).
///
/// Returns `true` when the history changed (the caller then persists);
/// `false` when the clip was debounced away (O2) and nothing should be
/// written.
///
///   * **O2 debounce** — if `text` equals the current top entry's text, it
///     is a no-op (drops the click-to-load echo + a redundant re-copy of
///     the same already-top clip).
///   * **O3 dedup move-to-top** — if `text` matches a *lower* existing
///     entry, that entry is moved to the front (its pinned flag preserved)
///     rather than duplicated.
///   * **new** — otherwise a fresh entry is pushed to the front.
///   * **O7 cap** — after insertion, unpinned entries beyond
///     [`HISTORY_CAP`] are trimmed (oldest first); pinned entries are
///     never counted nor trimmed.
pub fn apply_clip(history: &mut History, text: &str, source: &str, now: &str) -> bool {
    let clip = ClipEventBody::from_text(text, source, now);
    apply_clip_event(history, &clip)
}

/// Apply a canonical `event/clipboard/clip` body to the history.
///
/// Preserves the event's `{ id, text, source, time }` fields and keeps `pinned`
/// as durable history-only state: moving an existing entry preserves its pin,
/// while a new event is always inserted unpinned.
#[must_use]
pub fn apply_clip_event(history: &mut History, clip: &ClipEventBody) -> bool {
    if clip.text.trim().is_empty() || clip.text.len() > MAX_CLIP_BYTES {
        return false;
    }
    // O2 — identical to the current top → debounce (no change, no echo).
    if history.entries.first().is_some_and(|e| e.text == clip.text) {
        return false;
    }
    // O3 — same text lower in the list → move it to the top, keeping its
    // pin + id, refreshing source/time to the capture that re-surfaced it.
    if let Some(pos) = history
        .entries
        .iter()
        .position(|e| e.id == clip.id || e.text == clip.text)
    {
        let mut existing = history.entries.remove(pos);
        existing.id = clip.id.clone();
        existing.text = clip.text.clone();
        existing.source = clip.source.clone();
        existing.time = clip.time.clone();
        history.entries.insert(0, existing);
    } else {
        history.entries.insert(
            0,
            ClipEntry {
                id: clip.id.clone(),
                text: clip.text.clone(),
                source: clip.source.clone(),
                time: clip.time.clone(),
                pinned: false,
            },
        );
    }
    trim_unpinned(history, HISTORY_CAP);
    true
}

/// Parse the canonical `event/clipboard/clip` Bus body.
///
/// # Errors
/// Human-readable validation error for malformed JSON, missing required fields,
/// or a non-RFC3339 timestamp.
pub fn parse_clip_event_body(body: &str) -> Result<ClipEventBody, String> {
    let clip: ClipEventBody =
        serde_json::from_str(body).map_err(|e| format!("malformed clipboard clip body: {e}"))?;
    if clip.id.trim().is_empty() {
        return Err("clipboard clip body missing `id`".to_string());
    }
    if clip.text.trim().is_empty() {
        return Err("clipboard clip body missing non-blank `text`".to_string());
    }
    if clip.source.trim().is_empty() {
        return Err("clipboard clip body missing `source`".to_string());
    }
    if clip.text.len() > MAX_CLIP_BYTES {
        return Err(format!(
            "clipboard clip body `text` exceeds {MAX_CLIP_BYTES} byte limit"
        ));
    }
    let expected_id = clip_id(&clip.text);
    if clip.id != expected_id {
        return Err(format!(
            "clipboard clip body `id` must match content fingerprint {expected_id}"
        ));
    }
    chrono::DateTime::parse_from_rfc3339(&clip.time)
        .map_err(|e| format!("clipboard clip body `time` must be RFC3339: {e}"))?;
    Ok(clip)
}

/// O7 — keep at most `cap` unpinned entries (oldest unpinned trimmed
/// first); pinned entries are exempt + unlimited. Preserves order.
pub fn trim_unpinned(history: &mut History, cap: usize) {
    // Entries are stored newest→oldest, so the *oldest* unpinned entries are
    // the last unpinned indices. Collect them in one pass, then drop the
    // oldest (tail) overflow — removing from the highest index first keeps
    // the earlier indices valid.
    let unpinned_idx: Vec<usize> = history
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.pinned)
        .map(|(i, _)| i)
        .collect();
    if unpinned_idx.len() <= cap {
        return;
    }
    for &idx in unpinned_idx[cap..].iter().rev() {
        history.entries.remove(idx);
    }
}

/// RFC3339 (UTC) timestamp for "now" — the stamp written into each entry.
#[must_use]
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// O6 — render a stored RFC3339 stamp as a short relative age ("just now",
/// "2m", "3h", "5d") for the viewer's "from <node> · <age>" label. Pure so
/// both the worker's logging and any consumer share one format; unknown /
/// future stamps fall back to "now".
#[must_use]
pub fn age_label(stamp: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(then) = chrono::DateTime::parse_from_rfc3339(stamp) else {
        return "now".to_string();
    };
    let secs = (now - then.with_timezone(&chrono::Utc)).num_seconds();
    if secs < 5 {
        "now".to_string()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// The mesh-global history file under the replicated root.
#[must_use]
pub fn history_path(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("clipboard").join("history.json")
}

/// Read the shared history (an empty/missing/corrupt file → empty history,
/// never an error — a tailing node degrades gracefully pre-sync).
#[must_use]
pub fn read_history(path: &Path) -> History {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => History::default(),
    }
}

/// Atomic write-through of the history (tmp + rename), creating the
/// `clipboard/` dir as needed.
pub fn write_history(path: &Path, history: &History) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(history).map_err(|e| format!("encode: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

/// The durable acknowledgement for one daemon's clipboard event lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorRecord {
    topic: String,
    ulid: String,
}

/// Locate the local cursor, next to the Bus data rather than in shared history.
#[must_use]
fn cursor_path(bus_root: &Path) -> PathBuf {
    bus_root.join(CURSOR_FILE_NAME)
}

/// Read a cursor only when it is for this exact lane. A malformed or foreign
/// record is treated as absent so a damaged local cursor cannot suppress new
/// events or make the worker consume an unrelated topic.
#[must_use]
fn read_cursor(path: &Path) -> Option<String> {
    let record: CursorRecord = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (record.topic == CLIP_TOPIC && !record.ulid.trim().is_empty()).then_some(record.ulid)
}

/// Atomically persist the lane acknowledgement after its history mutation has
/// succeeded. A failed checkpoint is reported to the caller; the in-memory
/// cursor is intentionally not advanced, so the event is safely replayable.
fn write_cursor(path: &Path, ulid: &str) -> Result<(), String> {
    let record = CursorRecord {
        topic: CLIP_TOPIC.to_string(),
        ulid: ulid.to_string(),
    };
    let body = serde_json::to_vec(&record).map_err(|e| format!("encode cursor: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

/// Writability for the shared clipboard history.
///
/// Pure core — `root_is_dir` is injected so it unit-tests without touching the
/// filesystem. See [`ClipboardSyncWorker::share_writable`] for the why.
///
/// Under SUBSTRATE-V2 `/mnt/mesh-storage` is a plain Syncthing directory.
/// Writable **iff the canonical root actually exists as a directory**: a present
/// plain dir is fine, but a missing/unprovisioned share (early boot before
/// Syncthing creates it) is NOT written into — that avoids a per-clip write error
/// landing on a bare local dir. Any non-canonical root (dev tree / tempdir) is
/// always writable.
#[must_use]
pub fn clip_share_writable_core(workgroup_root: &Path, root_is_dir: bool) -> bool {
    crate::shared_root_writable_core(workgroup_root, root_is_dir)
}

/// Writability for the shared clipboard history, reading the shared root's
/// directory state. Thin I/O wrapper over [`clip_share_writable_core`].
#[must_use]
pub fn clip_share_writable(workgroup_root: &Path) -> bool {
    clip_share_writable_core(workgroup_root, workgroup_root.is_dir())
}

/// The clipboard-sync worker. Holds the replicated root and folds canonical
/// `event/clipboard/clip` Bus bodies through [`apply_clip_event`].
pub struct ClipboardSyncWorker {
    workgroup_root: PathBuf,
    /// Exact direct-seat identity that may consume a replicated history
    /// materialization on this node.
    target_seat: String,
    /// Bus root override (tests). `None` ⇒ [`crate::bus_publish::default_bus_root`].
    bus_root_override: Option<PathBuf>,
    /// Bus drain cadence.
    poll: Duration,
    /// Root-only signer for the daemon-authored VNC guest→seat handoff. A
    /// missing credential disables this handoff honestly; it never publishes
    /// an unsigned action body.
    vnc_action_signer: Option<CloudArmSigner>,
}

impl ClipboardSyncWorker {
    /// Build the worker rooted at the replicated workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            target_seat: local_target_seat(),
            bus_root_override: None,
            poll: CLIP_EVENT_POLL_INTERVAL,
            vnc_action_signer: production_action_signer().ok(),
        }
    }

    /// Override the Bus root (tests).
    #[cfg(test)]
    #[must_use]
    fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the direct-seat identity in deterministic tests.
    #[cfg(test)]
    #[must_use]
    fn with_target_seat(mut self, target_seat: impl Into<String>) -> Self {
        self.target_seat = target_seat.into();
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override
            .clone()
            .or_else(crate::bus_publish::default_bus_root)
    }

    /// Inject the daemon action signer in unit tests. Production obtains it
    /// from the root-only systemd credential in [`Self::new`].
    #[cfg(test)]
    #[must_use]
    fn with_vnc_action_signer(mut self, signer: CloudArmSigner) -> Self {
        self.vnc_action_signer = Some(signer);
        self
    }

    /// Decode the shell VNC source identity without trusting it as a route.
    /// The returned `(serving_peer, session_id)` is checked against the
    /// authoritative session records before it can become a target seat.
    fn parse_vnc_source(source: &str) -> Option<(&str, &str)> {
        let rest = source.strip_prefix(VNC_SOURCE_PREFIX)?;
        let (serving_peer, session_id) = rest.split_once(':')?;
        if serving_peer.trim().is_empty() || session_id.trim().is_empty() {
            return None;
        }
        Some((serving_peer, session_id))
    }

    /// Select the same etcd-first / replicated-file fallback store as the
    /// session broker. Clipboard routing must not read a different authority
    /// when the lease-backed session plane is enabled.
    fn session_store(&self) -> Box<dyn SessionStore + Send + Sync> {
        let endpoints = crate::substrate::etcd::default_endpoints();
        if endpoints.is_empty() {
            Box::new(MeshSessionStore::new(self.workgroup_root.clone()))
        } else {
            Box::new(EtcdSessionStore::new(endpoints))
        }
    }

    /// Resolve a VNC guest event to the active client's exact local target.
    /// The canonical event is not itself an authorization envelope, so the
    /// source is only a lookup hint; the active session roster is the authority.
    fn vnc_target_seat(&self, clip: &ClipEventBody) -> Result<Option<(String, String)>, String> {
        let Some((serving_peer, session_id)) = Self::parse_vnc_source(&clip.source) else {
            return Ok(None);
        };
        let sessions = self
            .session_store()
            .list()
            .map_err(|error| format!("read VDI session roster for VNC clipboard: {error}"))?;
        let Some(session) = sessions.into_iter().find(|session| {
            session.id == session_id
                && session.serving_peer == serving_peer
                && session.state == SessionState::Active
        }) else {
            // A stale/disconnected VNC event must never be guessed onto a seat.
            return Ok(Some((String::new(), String::new())));
        };
        super::clipboard_bridge::validate_target_seat(&session.client_peer).map_err(|error| {
            format!("VNC session client peer is not a safe target seat: {error}")
        })?;
        Ok(Some((session.id, session.client_peer)))
    }

    /// Mint the exact-body capability consumed by `clipboard_bridge` for one
    /// VNC guest→client event. Each publication attempt gets a fresh nonce:
    /// authorization consumes a nonce before the adapter write, so retrying a
    /// failed adapter must be able to obtain a new capability. The bridge's
    /// session/payload echo guard handles duplicate successful publications.
    fn signed_vnc_action(
        clip: &ClipEventBody,
        session_id: &str,
        target_seat: &str,
        signer: &CloudArmSigner,
    ) -> Result<String, String> {
        let event = ClipboardEvent {
            session_id: session_id.to_owned(),
            target_seat: target_seat.to_owned(),
            direction: ClipDirection::GuestToClient,
            payload: ClipPayload::checked(
                super::clipboard_bridge::ClipFormat::Text,
                clip.text.clone(),
            )
            .map_err(|error| format!("VNC clipboard payload rejected: {error}"))?,
            source: Some(clip.source.clone()),
        };
        let mut document = serde_json::to_value(event)
            .map_err(|error| format!("serialize VNC clipboard action: {error}"))?;
        document
            .as_object_mut()
            .ok_or_else(|| "VNC clipboard action is not a JSON object".to_string())?
            .insert(
                "schema_version".to_string(),
                serde_json::Value::from(ACTION_SCHEMA_VERSION),
            );
        let unsigned = document.to_string();
        let target = format!("session:{session_id}:seat:{target_seat}");
        let nonce = uuid::Uuid::new_v4().to_string();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "system clock is before the Unix epoch".to_string())
            .and_then(|duration| {
                i64::try_from(duration.as_millis())
                    .map_err(|_| "system clock is beyond the capability range".to_string())
            })?;
        let token = CloudArmedToken::mint(
            signer,
            &nonce,
            now_ms.saturating_add(MAX_AUTH_TTL_MS),
            super::clipboard_bridge::ACTION_AUTH_VERB,
            super::clipboard_bridge::ACTION_AUTH_NODE_SCOPE,
            &target,
            &cloud_request_digest(&unsigned).map_err(str::to_string)?,
        )
        .encode();
        document
            .as_object_mut()
            .ok_or_else(|| "VNC clipboard action is not a JSON object".to_string())?
            .insert("armed_token".to_string(), serde_json::Value::String(token));
        serde_json::to_string(&document)
            .map_err(|error| format!("serialize signed VNC clipboard action: {error}"))
    }

    /// Convert one accepted canonical VNC event into the signed action lane.
    /// `Ok(true)` means the event may be acknowledged; `Ok(false)` is reserved
    /// for a future deferred route. A missing/stale session is acknowledged but
    /// never guessed onto another seat.
    fn publish_vnc_action(
        &self,
        persist: &mut Persist,
        clip: &ClipEventBody,
    ) -> Result<bool, String> {
        let Some((session_id, target_seat)) = self.vnc_target_seat(clip)? else {
            return Ok(true);
        };
        if session_id.is_empty() {
            warn!(source = %clip.source, "discarding VNC clipboard event without an active matching session");
            return Ok(true);
        }
        let Some(signer) = self.vnc_action_signer.as_ref() else {
            return Err("VNC clipboard action signer is unavailable".to_string());
        };
        let body = Self::signed_vnc_action(clip, &session_id, &target_seat, signer)?;
        persist
            .write(
                super::clipboard_bridge::ACTION_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .map(|_| true)
            .map_err(|error| format!("publish signed VNC clipboard action: {error}"))
    }

    /// Whether it is safe to write `clipboard/history.json` under the shared
    /// root, **substrate-aware** (mirrors the boot_readiness SUBSTRATE-10
    /// probe).
    ///
    /// Post-SUBSTRATE-V2 `/mnt/mesh-storage` is a **plain Syncthing directory,
    /// not a FUSE mount** (design `substrate-v2.md` Q3/Q8: "now a plain local
    /// dir (NO FUSE)"), so a guard that gates the canonical path on a real
    /// `/proc/mounts` entry ([`crate::shared_root_writable`]) returns `false`
    /// for it and the worker would silently drop **every** clip —
    /// `history.json` is never written and the Hub's Clipboard Viewer reads an
    /// always-empty `action/clipboard/list`. When the etcd coordination plane
    /// is provisioned (the SUBSTRATE-1 endpoints file is present) the node is
    /// on SUBSTRATE-V2, the shared root is a plain dir, and there is no
    /// mountpoint to check — so it is writable. Absent the endpoints file we
    /// fall back to the dir-exists guard.
    fn share_writable(&self) -> bool {
        clip_share_writable(&self.workgroup_root)
    }

    /// Fold one canonical Bus clip into the shared history.
    fn handle_clip_event(&self, clip: &ClipEventBody) -> Result<bool, String> {
        if !self.share_writable() {
            return Ok(false);
        }
        let path = history_path(&self.workgroup_root);
        let mut history = read_history(&path);
        if !apply_clip_event(&mut history, clip) {
            return Ok(false);
        }
        write_history(&path, &history)?;
        Ok(true)
    }

    /// Publish a newly-observed replicated history head into this node's
    /// target-seat materialization lane. The shared history is the durable
    /// mesh transport; this local handoff is what lets the compositor-less DRM
    /// provider consume the value without fabricating a local capture event.
    fn materialize_replicated_head(
        &self,
        persist: &mut Persist,
        observed: &mut Option<ClipEventBody>,
    ) -> Result<bool, String> {
        let latest = read_history(&history_path(&self.workgroup_root))
            .entries
            .first()
            .map(|entry| ClipEventBody {
                id: entry.id.clone(),
                text: entry.text.clone(),
                source: entry.source.clone(),
                time: entry.time.clone(),
            });
        if latest == *observed {
            return Ok(false);
        }
        let Some(clip) = latest else {
            *observed = None;
            return Ok(false);
        };

        // Treat a malformed replicated row as observed so it is reported once,
        // never retried as a 400 ms log storm. A later valid head still differs
        // and will be delivered normally.
        if let Err(error) = parse_clip_event_body(
            &serde_json::to_string(&clip)
                .map_err(|encode| format!("encode replicated clipboard head: {encode}"))?,
        ) {
            *observed = Some(clip);
            return Err(format!(
                "refused malformed replicated clipboard head: {error}"
            ));
        }
        let text = VdiClipboardText::new(clip.text.clone())
            .map_err(|error| format!("replicated clipboard text rejected: {error}"))?;
        let handoff = ClipboardMaterialization::new(
            self.target_seat.clone(),
            text,
            clip.source.clone(),
            clip.time.clone(),
        );
        handoff
            .validate()
            .map_err(|error| format!("replicated clipboard handoff rejected: {error}"))?;
        let body = serde_json::to_string(&handoff)
            .map_err(|error| format!("encode replicated clipboard handoff: {error}"))?;
        persist
            .write(
                CLIPBOARD_MATERIALIZATION_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .map_err(|error| format!("publish replicated clipboard handoff: {error}"))?;
        *observed = Some(clip);
        Ok(true)
    }

    /// Drain new canonical `event/clipboard/clip` messages since `cursor`.
    fn drain_clip_events(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        checkpoint: Option<&Path>,
    ) -> usize {
        persist.reopen_if_index_changed();
        let msgs = match persist.list_since(CLIP_TOPIC, cursor.as_deref()) {
            Ok(msgs) => msgs,
            Err(e) => {
                debug!(target: "clipboard_sync", error = %e, "clipboard event drain failed");
                return 0;
            }
        };
        let mut applied = 0;
        for msg in msgs {
            let body = msg.body.as_deref().unwrap_or("");
            let clip = match parse_clip_event_body(body) {
                Ok(clip) => clip,
                Err(e) => {
                    warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %e, "bad clipboard event body");
                    if let Some(path) = checkpoint {
                        if let Err(checkpoint_error) = write_cursor(path, &msg.ulid) {
                            warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %checkpoint_error, "clipboard cursor checkpoint failed");
                            continue;
                        }
                    }
                    *cursor = Some(msg.ulid.clone());
                    continue;
                }
            };
            // VNC guest copies are canonical history events first, then become
            // signed target-seat mutations. Do the conversion before the
            // cursor can acknowledge the source event so a publish failure is
            // retryable rather than silently losing the guest copy.
            match self.publish_vnc_action(persist, &clip) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    warn!(target: "clipboard_sync", ulid = %msg.ulid, %error, "VNC clipboard action publish deferred");
                    continue;
                }
            }
            match self.handle_clip_event(&clip) {
                Ok(true) => {
                    if let Some(path) = checkpoint {
                        if let Err(checkpoint_error) = write_cursor(path, &msg.ulid) {
                            warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %checkpoint_error, "clipboard cursor checkpoint failed; event will be replayed");
                            continue;
                        }
                    }
                    *cursor = Some(msg.ulid.clone());
                    applied += 1;
                    debug!(
                        target: "clipboard_sync",
                        source = %clip.source,
                        "folded clipboard event ({} bytes)",
                        clip.text.len()
                    );
                }
                Ok(false) => {
                    // A non-applied valid event is either the O2 debounce or a
                    // non-writable shared root. Only the former is safe to
                    // acknowledge; handle_clip_event returns false for both,
                    // so leave the cursor unchanged and let the next tick
                    // retry until the shared history is writable.
                    if self.share_writable() {
                        if let Some(path) = checkpoint {
                            if let Err(checkpoint_error) = write_cursor(path, &msg.ulid) {
                                warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %checkpoint_error, "clipboard cursor checkpoint failed");
                                continue;
                            }
                        }
                        *cursor = Some(msg.ulid.clone());
                    }
                }
                Err(e) => {
                    warn!(target: "clipboard_sync", ulid = %msg.ulid, "history write failed: {e}");
                }
            }
        }
        applied
    }
}

#[async_trait::async_trait]
impl Worker for ClipboardSyncWorker {
    fn name(&self) -> &'static str {
        "clipboard_sync"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root() else {
            debug!("clipboard_sync: no bus root; worker idle");
            return Ok(());
        };
        let mut persist = match Persist::open(bus_root.clone()) {
            Ok(persist) => persist,
            Err(e) => {
                warn!(target: "clipboard_sync", error = %e, "bus open failed; worker idle");
                return Ok(());
            }
        };
        // Existing retained clipboard events may predate this daemon instance and
        // could resurrect a user-deleted/cleared history row. Start at the tail and
        // consume newly published lane events from here.
        let checkpoint = cursor_path(&bus_root);
        let mut cursor = read_cursor(&checkpoint);
        if cursor.is_none() {
            // First boot is intentionally forward-only: retained pre-daemon
            // events must not resurrect a user's deleted/cleared history.
            cursor = persist.latest_ulid(CLIP_TOPIC).ok().flatten();
            if let Some(ulid) = cursor.as_deref() {
                if let Err(e) = write_cursor(&checkpoint, ulid) {
                    warn!(target: "clipboard_sync", error = %e, "initial clipboard cursor checkpoint failed");
                }
            }
        }
        // Do not resurrect a retained clipboard at daemon start. Only a head
        // that changes while this worker is alive is a fresh mesh delivery.
        let mut observed_history_head = read_history(&history_path(&self.workgroup_root))
            .entries
            .first()
            .map(|entry| ClipEventBody {
                id: entry.id.clone(),
                text: entry.text.clone(),
                source: entry.source.clone(),
                time: entry.time.clone(),
            });
        info!(target: "clipboard_sync", "watching canonical clipboard bus lane");
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.drain_clip_events(&mut persist, &mut cursor, Some(&checkpoint));
                    if let Err(error) = self.materialize_replicated_head(
                        &mut persist,
                        &mut observed_history_head,
                    ) {
                        warn!(target: "clipboard_sync", %error, "replicated clipboard materialization failed");
                    }
                }
                () = shutdown.wait() => return Ok(()),
            }
        }
    }
}

fn local_target_seat() -> String {
    let hostname = std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown".to_owned());
    format!("seat:{}", hostname.trim())
}

/// Build the supervisor-ready worker (call site in `run_serve`).
#[must_use]
pub fn build(workgroup_root: PathBuf) -> ClipboardSyncWorker {
    ClipboardSyncWorker::new(workgroup_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
    use mackes_mesh_types::cloud::CloudArmSigner;
    use mde_bus::hooks::config::Priority;

    fn entry(text: &str, pinned: bool) -> ClipEntry {
        ClipEntry {
            id: clip_id(text),
            text: text.to_string(),
            source: "n".into(),
            time: "2026-06-21T00:00:00+00:00".into(),
            pinned,
        }
    }

    #[test]
    fn worker_name_is_stable() {
        let w = ClipboardSyncWorker::new(PathBuf::from("/tmp"));
        assert_eq!(w.name(), "clipboard_sync");
    }

    #[test]
    fn vnc_promotion_is_bounded_attributed_and_signed_for_exact_seat() {
        let key = b"clipboard-sync-vnc-action-test-key";
        let signer = CloudArmSigner::new(key.to_vec()).expect("test signer");
        let auth_root = tempfile::tempdir().expect("auth root");
        let clip = ClipEventBody::from_text(
            "guest copy",
            "vnc:serving-peer:session-1",
            "2026-07-31T12:00:00Z",
        );
        let body = ClipboardSyncWorker::signed_vnc_action(&clip, "session-1", "seat:dell", &signer)
            .expect("signed VNC action");
        let event: ClipboardEvent = serde_json::from_str(&body).expect("action event");
        assert_eq!(event.direction, ClipDirection::GuestToClient);
        assert_eq!(event.target_seat, "seat:dell");
        assert_eq!(event.source.as_deref(), Some("vnc:serving-peer:session-1"));
        assert!(event.payload.len() <= MAX_CLIP_BYTES);

        let signed_now = CloudArmedToken::parse(
            serde_json::from_str::<serde_json::Value>(&body).expect("action JSON")["armed_token"]
                .as_str()
                .expect("armed token"),
        )
        .expect("parse armed token")
        .expires_at_ms
        .saturating_sub(MAX_AUTH_TTL_MS);
        let authorizer =
            ActionAuthorizer::for_test(key, auth_root.path().to_path_buf(), signed_now);
        authorizer
            .authorize(
                &body,
                MutationContext {
                    verb: super::super::clipboard_bridge::ACTION_AUTH_VERB,
                    node: super::super::clipboard_bridge::ACTION_AUTH_NODE_SCOPE,
                    target: "session:session-1:seat:seat:dell",
                },
            )
            .expect("exact target-seat capability verifies");
        assert!(authorizer
            .authorize(
                &body,
                MutationContext {
                    verb: super::super::clipboard_bridge::ACTION_AUTH_VERB,
                    node: super::super::clipboard_bridge::ACTION_AUTH_NODE_SCOPE,
                    target: "session:session-1:seat:seat:other",
                },
            )
            .is_err());

        // Authorization is consumed before the adapter write. A second
        // publication therefore needs a fresh nonce so a failed first write
        // remains retryable; the bridge's payload echo guard handles the
        // duplicate if the first write actually succeeded.
        let retry_body =
            ClipboardSyncWorker::signed_vnc_action(&clip, "session-1", "seat:dell", &signer)
                .expect("retry action is signed");
        let retry_now_ms = CloudArmedToken::parse(
            serde_json::from_str::<serde_json::Value>(&retry_body).expect("retry action JSON")
                ["armed_token"]
                .as_str()
                .expect("retry armed token"),
        )
        .expect("parse retry armed token")
        .expires_at_ms
        .saturating_sub(MAX_AUTH_TTL_MS);
        ActionAuthorizer::for_test(key, auth_root.path().to_path_buf(), retry_now_ms)
            .authorize(
                &retry_body,
                MutationContext {
                    verb: super::super::clipboard_bridge::ACTION_AUTH_VERB,
                    node: super::super::clipboard_bridge::ACTION_AUTH_NODE_SCOPE,
                    target: "session:session-1:seat:seat:dell",
                },
            )
            .expect("retry gets a fresh capability nonce");
    }

    #[test]
    fn vnc_source_routes_only_through_matching_active_session() {
        let root = tempfile::tempdir().expect("session root");
        let requested = super::super::session_broker::open_session(
            "session-1".to_owned(),
            "serving-peer".to_owned(),
            "vm-1".to_owned(),
            "seat:dell".to_owned(),
            1,
        );
        let active =
            super::super::session_broker::mark_active(&requested, 2).expect("active session");
        MeshSessionStore::new(root.path().to_path_buf())
            .publish(&active)
            .expect("persist active session");
        let worker = ClipboardSyncWorker::new(root.path().to_path_buf());
        let clip = ClipEventBody::from_text(
            "guest copy",
            "vnc:serving-peer:session-1",
            "2026-07-31T12:00:00Z",
        );
        assert_eq!(
            worker.vnc_target_seat(&clip).expect("route lookup"),
            Some(("session-1".to_owned(), "seat:dell".to_owned()))
        );
        let stale = ClipEventBody::from_text(
            "guest copy",
            "vnc:other-peer:session-1",
            "2026-07-31T12:00:00Z",
        );
        assert_eq!(
            worker.vnc_target_seat(&stale).expect("stale lookup"),
            Some((String::new(), String::new()))
        );
    }

    #[test]
    fn apply_pushes_new_clip_to_front_and_stamps_it() {
        let mut h = History::default();
        assert!(apply_clip(
            &mut h,
            "hello",
            "alpha",
            "2026-06-21T10:00:00+00:00"
        ));
        assert_eq!(h.entries.len(), 1);
        let e = &h.entries[0];
        assert_eq!(e.text, "hello");
        assert_eq!(e.source, "alpha"); // O6 source stamp
        assert_eq!(e.time, "2026-06-21T10:00:00+00:00"); // O6 time stamp
        assert!(!e.pinned);
        assert_eq!(e.id, clip_id("hello"));
    }

    #[test]
    fn o2_debounce_drops_identical_top_clip() {
        // Re-copying / the viewer echoing the SAME top clip is a no-op.
        let mut h = History::default();
        assert!(apply_clip(&mut h, "x", "a", "t1"));
        assert!(
            !apply_clip(&mut h, "x", "a", "t2"),
            "identical top → debounced"
        );
        assert!(
            !apply_clip(&mut h, "x", "b", "t3"),
            "even from a different source"
        );
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].time, "t1", "no rewrite on debounce");
    }

    #[test]
    fn o3_dedup_moves_existing_entry_to_top() {
        let mut h = History::default();
        apply_clip(&mut h, "a", "n", "t1");
        apply_clip(&mut h, "b", "n", "t2");
        apply_clip(&mut h, "c", "n", "t3");
        // Re-copy "a" (now at the bottom) — it must move to the top, NOT dup.
        assert!(apply_clip(&mut h, "a", "host2", "t4"));
        assert_eq!(
            h.entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "c", "b"]
        );
        assert_eq!(h.entries.len(), 3, "no duplicate");
        assert_eq!(
            h.entries[0].source, "host2",
            "source refreshed on re-surface"
        );
        assert_eq!(h.entries[0].time, "t4");
    }

    #[test]
    fn o3_dedup_preserves_pin_on_resurface() {
        let mut h = History {
            entries: vec![entry("top", false), entry("pinned-old", true)],
        };
        // Re-copy the pinned entry's text → moves to top, stays pinned.
        assert!(apply_clip(&mut h, "pinned-old", "n", "t"));
        assert_eq!(h.entries[0].text, "pinned-old");
        assert!(h.entries[0].pinned, "pin survives a move-to-top");
    }

    #[test]
    fn o7_cap_trims_to_50_unpinned_oldest_first() {
        let mut h = History::default();
        for i in 0..60 {
            apply_clip(&mut h, &format!("clip-{i}"), "n", "t");
        }
        assert_eq!(h.entries.len(), HISTORY_CAP, "trimmed to 50 unpinned");
        // Newest first; the 10 oldest (clip-0..clip-9) were dropped.
        assert_eq!(h.entries[0].text, "clip-59");
        assert_eq!(h.entries[HISTORY_CAP - 1].text, "clip-10");
        assert!(!h.entries.iter().any(|e| e.text == "clip-0"));
    }

    #[test]
    fn o7_pins_are_exempt_from_the_cap_and_unlimited() {
        // 50 pinned + 50 unpinned → file holds all 100; only unpinned capped.
        let mut h = History::default();
        for i in 0..50 {
            h.entries.push(entry(&format!("pin-{i}"), true));
        }
        for i in 0..60 {
            apply_clip(&mut h, &format!("clip-{i}"), "n", "t");
        }
        let pinned = h.entries.iter().filter(|e| e.pinned).count();
        let unpinned = h.entries.iter().filter(|e| !e.pinned).count();
        assert_eq!(pinned, 50, "every pin survives — unlimited");
        assert_eq!(unpinned, HISTORY_CAP, "unpinned still capped at 50");
        assert!(h.entries.len() > HISTORY_CAP, "file longer than the cap");
    }

    #[test]
    fn trim_unpinned_drops_oldest_unpinned_keeps_pins_in_place() {
        // newest→oldest: u3, p, u2, u1  (cap 2 unpinned → drop u1, the oldest)
        let mut h = History {
            entries: vec![
                entry("u3", false),
                entry("p", true),
                entry("u2", false),
                entry("u1", false),
            ],
        };
        trim_unpinned(&mut h, 2);
        assert_eq!(
            h.entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["u3", "p", "u2"]
        );
    }

    #[test]
    fn clip_id_is_stable_and_content_addressed() {
        assert_eq!(clip_id("hello"), clip_id("hello"));
        assert_ne!(clip_id("hello"), clip_id("world"));
        assert_eq!(clip_id("hello").len(), 16);
    }

    #[test]
    fn canonical_clip_event_body_shape_is_locked() {
        let body = ClipEventBody::from_text("from bus", "seat/node-a", "2026-07-26T10:30:00Z");
        let encoded = serde_json::to_value(&body).unwrap();
        let obj = encoded.as_object().unwrap();
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["id", "source", "text", "time"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "event/clipboard/clip stays compatible with {{ id, text, source, time }}"
        );
        assert_eq!(body.id, clip_id("from bus"));
        assert_eq!(parse_clip_event_body(&encoded.to_string()).unwrap(), body);
    }

    #[test]
    fn canonical_event_body_does_not_grant_pin_state() {
        let id = clip_id("pinned?");
        let parsed = parse_clip_event_body(
            &format!(
                r#"{{"id":"{id}","text":"pinned?","source":"remote","time":"2026-07-26T10:30:00Z","pinned":true}}"#
            ),
        )
        .unwrap();
        let mut h = History::default();
        assert!(apply_clip_event(&mut h, &parsed));
        assert_eq!(h.entries[0].id, id);
        assert!(!h.entries[0].pinned, "pin state is history-only");
    }

    #[test]
    fn malformed_clip_event_bodies_are_rejected() {
        for body in [
            "not json",
            r#"{"id":"","text":"x","source":"n","time":"2026-07-26T10:30:00Z"}"#,
            r#"{"id":"i","text":"   ","source":"n","time":"2026-07-26T10:30:00Z"}"#,
            r#"{"id":"i","text":"x","source":"","time":"2026-07-26T10:30:00Z"}"#,
            r#"{"id":"wrong","text":"x","source":"n","time":"2026-07-26T10:30:00Z"}"#,
            r#"{"id":"i","text":"x","source":"n","time":"today"}"#,
        ] {
            assert!(
                parse_clip_event_body(body).is_err(),
                "body should be rejected: {body}"
            );
        }
    }

    #[test]
    fn oversized_clip_event_is_rejected_before_history_persistence() {
        let text = "x".repeat(MAX_CLIP_BYTES + 1);
        let body = ClipEventBody::from_text(&text, "remote", "2026-07-26T10:30:00Z");
        let encoded = serde_json::to_string(&body).unwrap();
        assert!(parse_clip_event_body(&encoded)
            .expect_err("oversized text must be rejected")
            .contains("byte limit"));

        let mut history = History::default();
        assert!(!apply_clip_event(&mut history, &body));
        assert!(history.entries.is_empty());
    }

    #[test]
    fn read_history_tolerates_missing_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("clipboard/history.json");
        assert_eq!(read_history(&p), History::default()); // missing
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(read_history(&p), History::default()); // corrupt → empty
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = history_path(dir.path());
        let mut h = History::default();
        apply_clip(&mut h, "round-trip", "src", "2026-06-21T10:00:00+00:00");
        write_history(&p, &h).unwrap();
        assert!(p.is_file());
        assert_eq!(read_history(&p), h);
    }

    #[test]
    fn history_path_is_clipboard_history_json() {
        assert_eq!(
            history_path(Path::new("/mnt/mesh")),
            PathBuf::from("/mnt/mesh/clipboard/history.json")
        );
    }

    #[test]
    fn age_label_buckets() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-21T12:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let at = |s: &str| {
            let t = now - chrono::Duration::seconds(s.parse::<i64>().unwrap());
            age_label(&t.to_rfc3339(), now)
        };
        assert_eq!(at("2"), "now");
        assert_eq!(at("30"), "30s");
        assert_eq!(at("120"), "2m");
        assert_eq!(at("7200"), "2h");
        assert_eq!(at("172800"), "2d");
        assert_eq!(age_label("garbage", now), "now"); // unparseable → now
    }

    #[test]
    fn bus_clip_events_write_and_dedup_history_end_to_end() {
        let history_dir = tempfile::tempdir().unwrap();
        let bus_dir = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).unwrap();
        let bodies = [
            ClipEventBody::from_text("first", "nodeA", "2026-07-26T10:00:00Z"),
            ClipEventBody::from_text("second", "nodeB", "2026-07-26T10:01:00Z"),
            ClipEventBody::from_text("first", "nodeA", "2026-07-26T10:02:00Z"),
        ];
        for body in &bodies {
            persist
                .write(
                    CLIP_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(body).unwrap()),
                )
                .unwrap();
        }

        let w = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf());
        let mut cursor = None;
        assert_eq!(w.drain_clip_events(&mut persist, &mut cursor, None), 3);
        let h = read_history(&history_path(history_dir.path()));
        assert_eq!(
            h.entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(h.entries[0].source, "nodeA");
        assert_eq!(h.entries[0].time, "2026-07-26T10:02:00Z");
    }

    #[test]
    fn durable_cursor_resumes_after_restart_without_replaying_retained_lane() {
        let history_dir = tempfile::tempdir().unwrap();
        let bus_dir = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).unwrap();
        let first = ClipEventBody::from_text("first", "nodeA", "2026-07-26T10:00:00Z");
        persist
            .write(
                CLIP_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&first).unwrap()),
            )
            .unwrap();

        let checkpoint = cursor_path(bus_dir.path());
        let w = ClipboardSyncWorker::new(history_dir.path().to_path_buf());
        let mut cursor = None;
        assert_eq!(
            w.drain_clip_events(&mut persist, &mut cursor, Some(&checkpoint)),
            1
        );
        let saved = read_cursor(&checkpoint).expect("successful fold is checkpointed");
        assert_eq!(cursor.as_deref(), Some(saved.as_str()));

        // A restarted daemon loads the durable acknowledgement and consumes
        // only the event published after it, while the retained first event is
        // not folded a second time.
        let second = ClipEventBody::from_text("second", "nodeB", "2026-07-26T10:01:00Z");
        persist
            .write(
                CLIP_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&second).unwrap()),
            )
            .unwrap();
        let mut restarted_cursor = read_cursor(&checkpoint);
        assert_eq!(
            w.drain_clip_events(&mut persist, &mut restarted_cursor, Some(&checkpoint)),
            1
        );
        assert_eq!(
            read_history(&history_path(history_dir.path()))
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "first"]
        );
    }

    #[test]
    fn replicated_history_head_materializes_once_for_the_exact_local_seat() {
        let history_dir = tempfile::tempdir().expect("history root");
        let bus_dir = tempfile::tempdir().expect("bus root");
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).expect("open bus");
        let old = ClipEventBody::from_text("old clipboard", "seat:source", "2026-08-03T12:00:00Z");
        let mut history = History::default();
        assert!(apply_clip_event(&mut history, &old));
        write_history(&history_path(history_dir.path()), &history).expect("write old history");
        let mut observed = Some(old);

        let fresh = ClipEventBody::from_text("fresh clipboard", "seat:remote", &now_rfc3339());
        assert!(apply_clip_event(&mut history, &fresh));
        write_history(&history_path(history_dir.path()), &history).expect("write fresh history");
        let worker = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf())
            .with_target_seat("seat:eagle");

        assert!(worker
            .materialize_replicated_head(&mut persist, &mut observed)
            .expect("materialize changed head"));
        let message = persist
            .read_latest(CLIPBOARD_MATERIALIZATION_TOPIC)
            .expect("read materialization")
            .expect("materialization exists");
        let handoff: ClipboardMaterialization =
            serde_json::from_str(message.body.as_deref().expect("materialization body"))
                .expect("decode materialization");
        assert_eq!(handoff.target_seat, "seat:eagle");
        assert_eq!(String::from(handoff.text), "fresh clipboard");
        assert_eq!(handoff.source, "seat:remote");
        assert_eq!(handoff.time, fresh.time);
        assert!(
            persist
                .read_latest(CLIP_TOPIC)
                .expect("read capture lane")
                .is_none(),
            "a replicated handoff must not fabricate a local capture event"
        );

        assert!(!worker
            .materialize_replicated_head(&mut persist, &mut observed)
            .expect("unchanged head is a no-op"));
        assert_eq!(
            persist
                .list_since(CLIPBOARD_MATERIALIZATION_TOPIC, None)
                .expect("list handoffs")
                .len(),
            1,
            "one replicated head produces exactly one local handoff",
        );
    }

    #[test]
    fn failed_history_write_does_not_acknowledge_event_and_retries() {
        let history_dir = tempfile::tempdir().unwrap();
        let bus_dir = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).unwrap();
        let body = ClipEventBody::from_text("retry me", "nodeA", "2026-07-26T10:00:00Z");
        persist
            .write(
                CLIP_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&body).unwrap()),
            )
            .unwrap();

        // Make the history parent unusable. The event must remain unacked.
        std::fs::write(history_dir.path().join("clipboard"), b"not a directory").unwrap();
        let checkpoint = cursor_path(bus_dir.path());
        let w = ClipboardSyncWorker::new(history_dir.path().to_path_buf());
        let mut cursor = None;
        assert_eq!(
            w.drain_clip_events(&mut persist, &mut cursor, Some(&checkpoint)),
            0
        );
        assert!(cursor.is_none());
        assert!(!checkpoint.exists());

        // Repair the destination and drain again: the same retained event is
        // now applied because the failed attempt never advanced the cursor.
        std::fs::remove_file(history_dir.path().join("clipboard")).unwrap();
        std::fs::create_dir(history_dir.path().join("clipboard")).unwrap();
        assert_eq!(
            w.drain_clip_events(&mut persist, &mut cursor, Some(&checkpoint)),
            1
        );
        assert_eq!(
            read_history(&history_path(history_dir.path())).entries[0].text,
            "retry me"
        );
        assert!(read_cursor(&checkpoint).is_some());
    }

    #[test]
    fn multi_line_clip_is_one_verbatim_entry() {
        let mut h = History::default();
        let snippet = "line one\nline two\nline three";
        let body = ClipEventBody::from_text(snippet, "n", "2026-07-26T10:30:00Z");
        assert!(apply_clip_event(&mut h, &body));
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].text, snippet, "newlines preserved, one entry");
    }

    #[test]
    fn clip_share_writable_core_writes_when_root_exists() {
        // SUBSTRATE-V2: the canonical path is a plain Syncthing dir, so the
        // clipboard worker MUST treat an EXISTING dir as writable — otherwise
        // every clip was dropped, leaving the Hub's Clipboard Viewer empty.
        let canonical = Path::new(crate::CANONICAL_QNM_MOUNT);
        assert!(
            clip_share_writable_core(canonical, /* root_is_dir = */ true),
            "present plain dir → writable"
        );
    }

    #[test]
    fn clip_share_writable_core_skips_missing_root() {
        // The shared dir doesn't exist yet (early boot, before Syncthing
        // provisions it): NOT writable, so we don't error per-clip writing into a
        // missing path that would land on a bare local dir.
        let canonical = Path::new(crate::CANONICAL_QNM_MOUNT);
        assert!(!clip_share_writable_core(
            canonical, /* root_is_dir = */ false
        ));
    }

    #[test]
    fn clip_share_writable_core_allows_non_canonical_roots() {
        // A non-canonical root (dev tree / tempdir) is always writable.
        let dir = tempfile::tempdir().unwrap();
        assert!(clip_share_writable_core(dir.path(), true));
        assert!(clip_share_writable_core(dir.path(), false));
    }

    #[test]
    fn whitespace_only_clip_is_skipped() {
        let mut h = History::default();
        let body = ClipEventBody {
            id: "blank".into(),
            text: "   ".into(),
            source: "n".into(),
            time: "2026-07-26T10:30:00Z".into(),
        };
        assert!(!apply_clip_event(&mut h, &body));
        assert!(h.entries.is_empty(), "blank/whitespace selections skipped");
    }
}
