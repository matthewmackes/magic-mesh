//! AIR-8 (v6.1) — mesh playback state + exclusive-playback handoff.
//!
//! Music plays on **one** peer at a time across the workgroup. The
//! playing peer writes its authoritative state to
//! `~/.local/share/mde/music-state.json` every 5 s, plus a per-peer
//! activity snapshot at `music-state-by-peer/<host>.json` (the Peers
//! tab, Q26). When another peer wants to take over, it drops a
//! `music-handoff-intent/<ulid>.json`; the current peer reads it,
//! pauses, surfaces an "Operator-Mac took over" notification, and
//! writes a completion. The requesting peer deletes the intent only after
//! it has consumed the matching completion.
//!
//! All the coordination decisions are pure functions
//! (`is_claimed_by_other`, `pending_takeover_for`, `latest_intent`) so
//! the conflict resolution is fully unit-testable; the daemon (AIR-2)
//! drives the 5 s write + the pause/notify side effects, and
//! `mde-musicd state {show,by-peer,takeover}` is the reachable entry
//! point exercising the files.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::queue::Queue;

/// A music-state record is considered stale (the playing peer went
/// away without clearing it) after 15 s — three missed 5 s writes.
pub const STATE_STALE_MS: u64 = 15_000;
/// Version of the durable playback-state envelope.
pub const STATE_SCHEMA_VERSION: u16 = 1;
/// Maximum number of retained handoff intents admitted by one state read.
pub const MAX_HANDOFF_INTENTS: usize = 64;
/// Maximum number of peer heartbeat snapshots admitted by one roster read.
pub const MAX_PEER_STATE_SNAPSHOTS: usize = 64;
/// Maximum number of handoff completions admitted by one target-side read.
pub const MAX_HANDOFF_COMPLETIONS: usize = 64;
/// A yielded transfer must be acknowledged before the source may reclaim it.
/// The deadline is deliberately the same three-heartbeat window used to
/// decide that a playback owner disappeared.
pub const HANDOFF_ACK_TIMEOUT_MS: u64 = STATE_STALE_MS;

const MAX_STATE_RECORD_BYTES: u64 = 16 * 1024;
const MAX_PEER_NAME_BYTES: usize = 128;
const MAX_SONG_ID_BYTES: usize = 256;
const MAX_INTENT_ID_BYTES: usize = 64;

/// Authoritative "who is playing what" record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicState {
    /// Hostname of the peer that owns playback.
    pub peer: String,
    /// Whether that peer is actively playing (vs paused/idle).
    pub playing: bool,
    /// Currently-loaded song id (empty when idle).
    #[serde(default)]
    pub song_id: String,
    /// Playhead position in ms.
    #[serde(default)]
    pub position_ms: u64,
    /// Epoch-ms of this record's last write (freshness / staleness).
    pub updated_ms: u64,
}

/// A take-over request: `from_peer` asks the current owner to yield.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffIntent {
    /// ULID — also the intent file's basename.
    pub intent_id: String,
    /// Peer requesting playback.
    pub from_peer: String,
    /// Current owner being asked to pause (`None` = claim an idle mesh).
    #[serde(default)]
    pub to_peer: Option<String>,
    /// Epoch-ms the intent was issued (conflict tiebreak: latest wins).
    pub issued_ms: u64,
}

/// Durable completion written by the yielding owner after it has persisted its
/// paused state. The requesting peer consumes this record to resume the same
/// queue song at the owner's exact position; it is coordination metadata, not
/// a second playback state authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffCompletion {
    /// The intent that caused the yield.
    pub intent_id: String,
    /// Peer that requested the handoff and must consume this completion.
    pub from_peer: String,
    /// Peer that yielded and wrote the paused state.
    pub owner_peer: String,
    /// Queue song preserved by the yielding owner.
    pub song_id: String,
    /// Exact admitted queue identity at the yield boundary. The queue is
    /// transferred rather than inferred from a target-local song lookup, so a
    /// seat with a different ordering or duplicate song cannot fabricate
    /// continuity.
    #[serde(default)]
    pub queue: Queue,
    /// Exact position within `song_id` at the yield boundary.
    pub position_ms: u64,
    /// Epoch-ms when the completion was recorded.
    pub completed_ms: u64,
    /// Last epoch-ms at which the target may start playback from this record.
    /// After this deadline the source is allowed to reclaim authority.
    #[serde(default)]
    pub expires_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateFileV1 {
    schema_version: u16,
    #[serde(flatten)]
    state: MusicState,
}

fn valid_component(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .chars()
            .all(|character| !character.is_control() && character != '/' && character != '\\')
}

fn valid_song_id(value: &str) -> bool {
    value.len() <= MAX_SONG_ID_BYTES && value.chars().all(|character| !character.is_control())
}

fn valid_state_record(state: &MusicState) -> bool {
    valid_component(&state.peer, MAX_PEER_NAME_BYTES) && valid_song_id(&state.song_id)
}

fn valid_intent_record(intent: &HandoffIntent) -> bool {
    valid_component(&intent.intent_id, MAX_INTENT_ID_BYTES)
        && valid_component(&intent.from_peer, MAX_PEER_NAME_BYTES)
        && intent
            .to_peer
            .as_deref()
            .is_none_or(|peer| valid_component(peer, MAX_PEER_NAME_BYTES))
}

fn valid_completion_record(completion: &HandoffCompletion) -> bool {
    valid_component(&completion.intent_id, MAX_INTENT_ID_BYTES)
        && valid_component(&completion.from_peer, MAX_PEER_NAME_BYTES)
        && valid_component(&completion.owner_peer, MAX_PEER_NAME_BYTES)
        && !completion.song_id.is_empty()
        && valid_song_id(&completion.song_id)
        && !completion.queue.songs.is_empty()
        && completion.queue.current < completion.queue.songs.len()
        && completion.queue.current() == Some(completion.song_id.as_str())
        && completion
            .queue
            .songs
            .iter()
            .all(|song| valid_song_id(song))
        && completion.expires_ms
            == completion
                .completed_ms
                .saturating_add(HANDOFF_ACK_TIMEOUT_MS)
}

fn canonical_handoff_path(path: &Path, intent_id: &str) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem == intent_id)
}

fn canonical_peer_state_path(path: &Path, peer: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == format!("{peer}.json"))
}

fn read_bounded_bytes(path: &Path) -> Option<Vec<u8>> {
    let length = std::fs::metadata(path).ok()?.len();
    if length > MAX_STATE_RECORD_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(length as usize);
    let mut file = std::fs::File::open(path).ok()?;
    file.by_ref()
        .take(MAX_STATE_RECORD_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_STATE_RECORD_BYTES).then_some(bytes)
}

fn decode_bounded<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    serde_json::from_slice(bytes).ok()
}

// ───────────────────────── pure decisions ─────────────────────────

/// Is the mesh currently claimed by a peer **other than** `my_host`?
/// Returns that peer's hostname when the state is fresh + playing +
/// owned elsewhere; `None` when idle, stale, or owned by us.
#[must_use]
pub fn is_claimed_by_other(
    state: Option<&MusicState>,
    my_host: &str,
    now_ms: u64,
) -> Option<String> {
    let s = state?;
    if !s.playing {
        return None;
    }
    if s.peer == my_host {
        return None;
    }
    if now_ms.saturating_sub(s.updated_ms) > STATE_STALE_MS {
        return None; // owner went away — mesh is effectively free.
    }
    Some(s.peer.clone())
}

/// The handoff intent (if any) that `my_host` must honour by pausing:
/// an intent whose `to_peer` is `my_host` (or unset, a general claim
/// while we own playback). When several target us, the **latest** wins.
#[must_use]
pub fn pending_takeover_for(intents: &[HandoffIntent], my_host: &str) -> Option<HandoffIntent> {
    intents
        .iter()
        .filter(|i| {
            i.to_peer.as_deref().is_none_or(|target| target == my_host) && i.from_peer != my_host
        })
        .max_by(|a, b| {
            a.issued_ms
                .cmp(&b.issued_ms)
                .then_with(|| a.intent_id.cmp(&b.intent_id))
        })
        .cloned()
}

/// Of a set of competing intents, the one that wins (latest `issued_ms`;
/// ties broken by `intent_id` for determinism).
#[must_use]
pub fn latest_intent(intents: &[HandoffIntent]) -> Option<HandoffIntent> {
    intents
        .iter()
        .max_by(|a, b| {
            a.issued_ms
                .cmp(&b.issued_ms)
                .then_with(|| a.intent_id.cmp(&b.intent_id))
        })
        .cloned()
}

/// Whether a completion is authorized by the target's newest still-pending
/// request. Binding the completion to the current request prevents a stale
/// file (or a writer that merely spoofs the target in `from_peer`) from
/// starting playback without a live, target-owned intent. A targeted intent
/// also binds the completion to the owner that was asked to yield; general
/// claims still require a distinct owner. The owner timestamp must not
/// precede the request.
#[must_use]
pub fn completion_matches_intent(
    completion: &HandoffCompletion,
    intents: &[HandoffIntent],
    target_peer: &str,
    now_ms: u64,
) -> bool {
    let Some(current_intent) = intents
        .iter()
        .filter(|intent| valid_intent_record(intent) && intent.from_peer == target_peer)
        .max_by(|left, right| {
            left.issued_ms
                .cmp(&right.issued_ms)
                .then_with(|| left.intent_id.cmp(&right.intent_id))
        })
    else {
        return false;
    };

    valid_completion_record(completion)
        && current_intent.intent_id == completion.intent_id
        && completion.from_peer == current_intent.from_peer
        && completion.owner_peer != target_peer
        && current_intent
            .to_peer
            .as_deref()
            .is_none_or(|owner| owner == completion.owner_peer)
        && completion.completed_ms >= current_intent.issued_ms
        && now_ms <= completion.expires_ms
}

// ───────────────────────── file layout ─────────────────────────

/// `$HOME/.local/share/mde/` — the mesh-shared music data root.
#[must_use]
pub fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    Path::new(&home).join(".local/share/mde")
}

/// Syncthing-replicated ownership and handoff root.
///
/// Catalogs, credentials, replay nonces, downloads, and the playback queue
/// remain seat-local under [`data_dir`]. Only the bounded per-peer heartbeat,
/// handoff-intent, and handoff-completion records belong on the mesh file
/// plane. Keeping this resolver separate prevents a shared-root setting from
/// accidentally turning local credentials or mutable queue state into
/// workgroup-wide authority.
#[must_use]
pub fn coordination_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("MDE_MUSIC_COORDINATION_DIR") {
        return PathBuf::from(path);
    }
    std::env::var_os("MDE_WORKGROUP_ROOT")
        .map_or_else(data_dir, |root| PathBuf::from(root).join("music/session"))
}

/// This peer's hostname — the `peer` field on every [`MusicState`] this
/// host writes.
///
/// Falls back to `localhost` when the `hostname` command is unavailable. The
/// result is cached because this helper is used by state, handoff, MPRIS, and
/// catalog paths; repeated process spawning would amplify across seats.
#[must_use]
pub fn local_host() -> String {
    static LOCAL_HOST: OnceLock<String> = OnceLock::new();
    LOCAL_HOST
        .get_or_init(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "localhost".to_string())
        })
        .clone()
}

/// Epoch-ms now (the `updated_ms` / `issued_ms` timestamp source).
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Path of the authoritative `music-state.json` within `dir`.
#[must_use]
pub fn state_path(dir: &Path) -> PathBuf {
    dir.join("music-state.json")
}

/// Per-peer snapshot path: `music-state-by-peer/<host>.json`.
#[must_use]
pub fn by_peer_path(dir: &Path, host: &str) -> PathBuf {
    dir.join("music-state-by-peer").join(format!("{host}.json"))
}

/// The handoff-intent directory within `dir`.
#[must_use]
pub fn intents_dir(dir: &Path) -> PathBuf {
    dir.join("music-handoff-intent")
}

/// Durable handoff-completion directory shared by the music peers.
#[must_use]
pub fn completions_dir(dir: &Path) -> PathBuf {
    dir.join("music-handoff-complete")
}

/// Read `music-state.json` (None when absent/malformed).
#[must_use]
pub fn read_state(dir: &Path) -> Option<MusicState> {
    let bytes = read_bounded_bytes(&state_path(dir))?;
    decode_bounded::<StateFileV1>(&bytes)
        .filter(|file| file.schema_version == STATE_SCHEMA_VERSION)
        .map(|file| file.state)
        .filter(valid_state_record)
        .or_else(|| decode_bounded::<MusicState>(&bytes).filter(valid_state_record))
}

/// Replace one coordination record without exposing a partially written JSON
/// document to the local daemon or Syncthing. The sibling temporary file is not
/// a `.json` record, so bounded directory readers ignore it until the synced
/// rename commits the complete record.
fn write_record_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_record_atomically_with(path, bytes, |temporary, target| {
        std::fs::rename(temporary, target)
    })
}

fn write_record_atomically_with<F>(path: &Path, bytes: &[u8], replace: F) -> std::io::Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "music coordination record has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let target_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("music-state.json");
    let temporary = parent.join(format!(".{target_name}.{}.tmp", ulid::Ulid::new()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Write `music-state.json` (+ this peer's by-peer snapshot).
///
/// # Errors
/// IO / serialization failures, or an unsafe state identity.
pub fn write_state(dir: &Path, state: &MusicState) -> std::io::Result<()> {
    if !valid_state_record(state) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "music state identity or song id exceeds its contract",
        ));
    }
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(&StateFileV1 {
        schema_version: STATE_SCHEMA_VERSION,
        state: state.clone(),
    })
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let bp = by_peer_path(dir, &state.peer);
    // These two records are independently atomic, not a filesystem transaction.
    // Commit authority first: if the derived snapshot then fails, remote roster
    // readers remain safely stale instead of observing state this owner never
    // committed.
    write_record_atomically(&state_path(dir), json.as_bytes())?;
    write_record_atomically(&bp, json.as_bytes())
}

/// Read the newest bounded handoff-intent projection from the intents dir.
/// Malformed, oversized, and unsafe records are ignored. The return order is
/// newest first so callers can inspect the winning request without depending
/// on filesystem directory order.
#[must_use]
pub fn read_intents(dir: &Path) -> Vec<HandoffIntent> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(intents_dir(dir)) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Some(intent) = read_bounded_bytes(&p)
                    .and_then(|bytes| decode_bounded::<HandoffIntent>(&bytes))
                    .filter(|intent| {
                        valid_intent_record(intent) && canonical_handoff_path(&p, &intent.intent_id)
                    })
                {
                    out.push(intent);
                }
            }
        }
    }
    out.sort_unstable_by(|a, b| {
        b.issued_ms
            .cmp(&a.issued_ms)
            .then_with(|| b.intent_id.cmp(&a.intent_id))
    });
    out.truncate(MAX_HANDOFF_INTENTS);
    out
}

/// Read newest bounded handoff completions for target-side resume. Malformed,
/// oversized, and unsafe records are ignored; filesystem ordering is never
/// used as a conflict decision.
#[must_use]
pub fn read_completions(dir: &Path) -> Vec<HandoffCompletion> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(completions_dir(dir)) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                if let Some(completion) = read_bounded_bytes(&path)
                    .and_then(|bytes| decode_bounded::<HandoffCompletion>(&bytes))
                    .filter(|completion| {
                        valid_completion_record(completion)
                            && canonical_handoff_path(&path, &completion.intent_id)
                    })
                {
                    out.push(completion);
                }
            }
        }
    }
    out.sort_unstable_by(|a, b| {
        b.completed_ms
            .cmp(&a.completed_ms)
            .then_with(|| b.intent_id.cmp(&a.intent_id))
    });
    out.truncate(MAX_HANDOFF_COMPLETIONS);
    out
}

/// Read the newest bounded peer heartbeat projection from
/// `music-state-by-peer/`. Malformed, oversized, and unsafe records are
/// ignored; the final roster remains sorted by peer for stable UI output.
#[must_use]
pub fn read_all_peer_states(dir: &Path) -> Vec<MusicState> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir.join("music-state-by-peer")) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Some(state) = read_bounded_bytes(&p)
                    .and_then(|bytes| {
                        decode_bounded::<StateFileV1>(&bytes)
                            .filter(|file| file.schema_version == STATE_SCHEMA_VERSION)
                            .map(|file| file.state)
                            .or_else(|| decode_bounded::<MusicState>(&bytes))
                    })
                    .filter(|state| {
                        valid_state_record(state) && canonical_peer_state_path(&p, &state.peer)
                    })
                {
                    out.push(state);
                    if out.len() > MAX_PEER_STATE_SNAPSHOTS {
                        out.sort_unstable_by(|a, b| {
                            b.updated_ms
                                .cmp(&a.updated_ms)
                                .then_with(|| b.peer.cmp(&a.peer))
                        });
                        out.truncate(MAX_PEER_STATE_SNAPSHOTS);
                    }
                }
            }
        }
    }
    out.sort_unstable_by(|a, b| a.peer.cmp(&b.peer));
    out
}

/// Drop a take-over intent from `from_peer` targeting `to_peer`. Returns
/// the written intent (its `intent_id` is the file basename).
///
/// # Errors
/// IO / serialization failures.
pub fn post_takeover(
    dir: &Path,
    from_peer: &str,
    to_peer: Option<String>,
    now_ms: u64,
) -> std::io::Result<HandoffIntent> {
    if !valid_component(from_peer, MAX_PEER_NAME_BYTES)
        || to_peer
            .as_deref()
            .is_some_and(|peer| !valid_component(peer, MAX_PEER_NAME_BYTES))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "handoff peer identity exceeds its contract",
        ));
    }
    let id = ulid::Ulid::new().to_string();
    let intent = HandoffIntent {
        intent_id: id.clone(),
        from_peer: from_peer.to_string(),
        to_peer,
        issued_ms: now_ms,
    };
    let d = intents_dir(dir);
    std::fs::create_dir_all(&d)?;
    let json = serde_json::to_string_pretty(&intent)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_record_atomically(&d.join(format!("{id}.json")), json.as_bytes())?;
    Ok(intent)
}

/// Persist a completion after the owner has durably written its paused state.
/// The target can safely retry this record until it has started playback.
pub fn write_completion(dir: &Path, completion: &HandoffCompletion) -> std::io::Result<()> {
    if !valid_completion_record(completion) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "handoff completion identity or song id exceeds its contract",
        ));
    }
    let directory = completions_dir(dir);
    std::fs::create_dir_all(&directory)?;
    let body = serde_json::to_vec_pretty(completion)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if body.len() as u64 > MAX_STATE_RECORD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "handoff completion exceeds its bounded record contract",
        ));
    }
    write_record_atomically(
        &directory.join(format!("{}.json", completion.intent_id)),
        &body,
    )
}

/// Delete a consumed handoff completion by its validated intent id.
pub fn clear_completion(dir: &Path, intent_id: &str) {
    if !valid_component(intent_id, MAX_INTENT_ID_BYTES) {
        return;
    }
    let _ = std::fs::remove_file(completions_dir(dir).join(format!("{intent_id}.json")));
}

/// Delete a handoff intent by id after the requester has consumed its
/// matching completion. Best-effort.
pub fn clear_intent(dir: &Path, intent_id: &str) {
    if !valid_component(intent_id, MAX_INTENT_ID_BYTES) {
        return;
    }
    let _ = std::fs::remove_file(intents_dir(dir).join(format!("{intent_id}.json")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_all_peer_states_collects_and_sorts_snapshots() {
        let dir = tempfile::TempDir::new().unwrap();
        write_state(
            dir.path(),
            &MusicState {
                peer: "forge".into(),
                playing: false,
                song_id: String::new(),
                position_ms: 0,
                updated_ms: 1000,
            },
        )
        .unwrap();
        write_state(
            dir.path(),
            &MusicState {
                peer: "anvil".into(),
                playing: true,
                song_id: "s1".into(),
                position_ms: 0,
                updated_ms: 1000,
            },
        )
        .unwrap();
        let all = read_all_peer_states(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].peer, "anvil");
        assert_eq!(all[1].peer, "forge");
        assert!(read_all_peer_states(tempfile::TempDir::new().unwrap().path()).is_empty());
    }

    #[test]
    fn peer_state_reader_rejects_snapshot_with_mismatched_peer_filename() {
        let dir = tempdir().unwrap();
        let peers = dir.path().join("music-state-by-peer");
        std::fs::create_dir_all(&peers).unwrap();
        let snapshot = serde_json::to_vec(&StateFileV1 {
            schema_version: STATE_SCHEMA_VERSION,
            state: MusicState {
                peer: "seat-15".into(),
                playing: false,
                song_id: String::new(),
                position_ms: 0,
                updated_ms: 100,
            },
        })
        .unwrap();
        std::fs::write(peers.join("seat-16.json"), &snapshot).unwrap();
        assert!(read_all_peer_states(dir.path()).is_empty());

        std::fs::write(peers.join("seat-15.json"), snapshot).unwrap();
        assert_eq!(read_all_peer_states(dir.path()).len(), 1);
    }
    use tempfile::tempdir;

    fn state(peer: &str, playing: bool, updated: u64) -> MusicState {
        MusicState {
            peer: peer.into(),
            playing,
            song_id: "s1".into(),
            position_ms: 0,
            updated_ms: updated,
        }
    }

    fn intent(id: &str, from: &str, to: Option<&str>, issued: u64) -> HandoffIntent {
        HandoffIntent {
            intent_id: id.into(),
            from_peer: from.into(),
            to_peer: to.map(ToString::to_string),
            issued_ms: issued,
        }
    }

    fn completion(
        id: &str,
        from: &str,
        owner: &str,
        song: &str,
        position_ms: u64,
        completed_ms: u64,
    ) -> HandoffCompletion {
        HandoffCompletion {
            intent_id: id.into(),
            from_peer: from.into(),
            owner_peer: owner.into(),
            song_id: song.into(),
            queue: Queue {
                songs: vec!["queue-before".into(), song.into(), "queue-after".into()],
                current: 1,
                preferred_source: None,
            },
            position_ms,
            completed_ms,
            expires_ms: completed_ms.saturating_add(HANDOFF_ACK_TIMEOUT_MS),
        }
    }

    #[test]
    fn claimed_by_other_when_fresh_playing_elsewhere() {
        let s = state("anvil", true, 1000);
        assert_eq!(
            is_claimed_by_other(Some(&s), "forge", 2000),
            Some("anvil".into())
        );
    }

    #[test]
    fn not_claimed_when_ours_idle_or_stale() {
        // Owned by us.
        assert_eq!(
            is_claimed_by_other(Some(&state("forge", true, 1000)), "forge", 2000),
            None
        );
        // Not playing.
        assert_eq!(
            is_claimed_by_other(Some(&state("anvil", false, 1000)), "forge", 2000),
            None
        );
        // Stale (owner went away).
        assert_eq!(
            is_claimed_by_other(
                Some(&state("anvil", true, 1000)),
                "forge",
                1000 + STATE_STALE_MS + 1
            ),
            None
        );
        // No state at all.
        assert_eq!(is_claimed_by_other(None, "forge", 2000), None);
    }

    #[test]
    fn pending_takeover_targets_me_latest_wins() {
        let intents = vec![
            intent("a", "forge", Some("anvil"), 10),
            intent("b", "beacon", Some("anvil"), 30), // latest targeting anvil
            intent("c", "forge", Some("other"), 99),  // not for anvil
        ];
        let got = pending_takeover_for(&intents, "anvil").unwrap();
        assert_eq!(got.intent_id, "b");
        assert_eq!(got.from_peer, "beacon");
        let general = vec![intent("general", "beacon", None, 40)];
        assert_eq!(
            pending_takeover_for(&general, "anvil")
                .expect("general takeover applies to the local owner")
                .intent_id,
            "general"
        );
        // A peer never yields to its own intent.
        let self_only = vec![intent("x", "anvil", Some("anvil"), 5)];
        assert!(pending_takeover_for(&self_only, "anvil").is_none());
    }

    #[test]
    fn latest_intent_breaks_ties_by_id() {
        let intents = vec![
            intent("z", "a", None, 50),
            intent("a", "b", None, 50), // same ts → id tiebreak picks "z" (greater)
            intent("m", "c", None, 10),
        ];
        assert_eq!(latest_intent(&intents).unwrap().intent_id, "z");
        assert!(latest_intent(&[]).is_none());
    }

    #[test]
    fn handoff_completion_requires_a_live_request_and_matching_owner() {
        let request = intent("handoff-1", "forge", Some("anvil"), 100);
        let completion = completion("handoff-1", "forge", "anvil", "song-7", 42_500, 101);
        assert!(completion_matches_intent(
            &completion,
            std::slice::from_ref(&request),
            "forge",
            101,
        ));

        let mut spoofed_requester = completion.clone();
        spoofed_requester.from_peer = "intruder".into();
        assert!(!completion_matches_intent(
            &spoofed_requester,
            std::slice::from_ref(&request),
            "forge",
            101,
        ));

        // A replay after the request has been consumed has no authorization.
        assert!(!completion_matches_intent(&completion, &[], "forge", 101));

        let mut wrong_owner = completion.clone();
        wrong_owner.owner_peer = "beacon".into();
        assert!(!completion_matches_intent(
            &wrong_owner,
            std::slice::from_ref(&request),
            "forge",
            101,
        ));

        let mut stale = completion;
        stale.completed_ms = 99;
        stale.expires_ms = 99 + HANDOFF_ACK_TIMEOUT_MS;
        assert!(!completion_matches_intent(
            &stale,
            std::slice::from_ref(&request),
            "forge",
            101,
        ));
    }

    #[test]
    fn handoff_completion_rejects_a_superseded_request_for_the_target() {
        let old_request = intent("handoff-old", "forge", Some("anvil"), 100);
        let current_request = intent("handoff-current", "forge", Some("beacon"), 200);
        let old_completion = completion("handoff-old", "forge", "anvil", "song-7", 42_500, 101);
        let current_completion =
            completion("handoff-current", "forge", "beacon", "song-9", 12_000, 201);
        let intents = [old_request, current_request];

        assert!(!completion_matches_intent(
            &old_completion,
            &intents,
            "forge",
            201,
        ));
        assert!(completion_matches_intent(
            &current_completion,
            &intents,
            "forge",
            201,
        ));
    }

    #[test]
    fn state_writes_authoritative_plus_by_peer_snapshot() {
        let dir = tempdir().unwrap();
        let s = state("anvil", true, 1234);
        write_state(dir.path(), &s).unwrap();
        assert_eq!(read_state(dir.path()), Some(s.clone()));
        // Per-peer snapshot also written.
        let bp = by_peer_path(dir.path(), "anvil");
        assert!(bp.exists());
        let snap: MusicState = serde_json::from_str(&std::fs::read_to_string(bp).unwrap()).unwrap();
        assert_eq!(snap, s);
    }

    #[test]
    fn post_read_and_clear_intent_round_trip() {
        let dir = tempdir().unwrap();
        let posted = post_takeover(dir.path(), "forge", Some("anvil".into()), 77).unwrap();
        let read = read_intents(dir.path());
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].from_peer, "forge");
        assert_eq!(read[0].issued_ms, 77);
        clear_intent(dir.path(), &posted.intent_id);
        assert!(read_intents(dir.path()).is_empty());
    }

    #[test]
    fn handoff_completion_round_trips_and_is_cleared_by_intent_id() {
        let dir = tempdir().unwrap();
        let completion = completion("intent-1", "forge", "anvil", "song-7", 42_500, 88);
        write_completion(dir.path(), &completion).unwrap();
        assert_eq!(read_completions(dir.path()), vec![completion]);
        clear_completion(dir.path(), "intent-1");
        assert!(read_completions(dir.path()).is_empty());
    }

    #[test]
    fn failed_handoff_record_replace_preserves_last_good_and_cleans_temporary() {
        let dir = tempdir().unwrap();
        let path = completions_dir(dir.path()).join("intent-1.json");
        let last_good = completion("intent-1", "forge", "anvil", "song-7", 42_500, 88);
        write_completion(dir.path(), &last_good).unwrap();

        let replacement = serde_json::to_vec_pretty(&completion(
            "intent-1", "forge", "anvil", "song-8", 51_000, 99,
        ))
        .unwrap();
        let error = write_record_atomically_with(&path, &replacement, |_temporary, _target| {
            Err(std::io::Error::other("injected handoff replace failure"))
        })
        .expect_err("failed replacement must be reported");
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert_eq!(read_completions(dir.path()), vec![last_good]);
        assert!(
            std::fs::read_dir(completions_dir(dir.path()))
                .unwrap()
                .flatten()
                .all(|entry| entry.path().extension().is_none_or(|ext| ext != "tmp"))
        );
    }

    #[test]
    fn handoff_readers_reject_alias_files_that_can_replay_same_handoff() {
        let dir = tempdir().unwrap();
        let intent = intent("intent-1", "forge", Some("anvil"), 77);
        let intents = intents_dir(dir.path());
        std::fs::create_dir_all(&intents).unwrap();
        let intent_json = serde_json::to_vec(&intent).unwrap();
        std::fs::write(intents.join("intent-1.json"), &intent_json).unwrap();
        std::fs::write(intents.join("replay-alias.json"), intent_json).unwrap();
        assert_eq!(read_intents(dir.path()), vec![intent]);

        let completion = completion("intent-1", "forge", "anvil", "song-7", 42_500, 88);
        let completions = completions_dir(dir.path());
        std::fs::create_dir_all(&completions).unwrap();
        let completion_json = serde_json::to_vec(&completion).unwrap();
        std::fs::write(completions.join("intent-1.json"), &completion_json).unwrap();
        std::fs::write(completions.join("replay-alias.json"), completion_json).unwrap();
        assert_eq!(read_completions(dir.path()), vec![completion]);
    }

    #[test]
    fn handoff_completion_reader_keeps_newest_bounded_backlog() {
        let dir = tempdir().unwrap();
        let completions = completions_dir(dir.path());
        std::fs::create_dir_all(&completions).unwrap();
        for index in 0..=MAX_HANDOFF_COMPLETIONS {
            let completion = completion(
                &format!("intent-{index:03}"),
                "forge",
                "anvil",
                &format!("song-{index:03}"),
                index as u64,
                index as u64,
            );
            std::fs::write(
                completions.join(format!("{}.json", completion.intent_id)),
                serde_json::to_vec(&completion).unwrap(),
            )
            .unwrap();
        }
        let retained = read_completions(dir.path());
        assert_eq!(retained.len(), MAX_HANDOFF_COMPLETIONS);
        assert_eq!(retained.first().unwrap().completed_ms, 64);
        assert!(!retained.iter().any(|item| item.intent_id == "intent-000"));
    }

    #[test]
    fn handoff_intent_reader_keeps_newest_bounded_backlog() {
        let dir = tempdir().unwrap();
        let intents = intents_dir(dir.path());
        std::fs::create_dir_all(&intents).unwrap();
        for index in 0..=MAX_HANDOFF_INTENTS {
            let intent = intent(
                &format!("intent-{index:03}"),
                &format!("peer-{index:03}"),
                Some("anvil"),
                index as u64,
            );
            std::fs::write(
                intents.join(format!("{}.json", intent.intent_id)),
                serde_json::to_vec(&intent).unwrap(),
            )
            .unwrap();
        }

        let retained = read_intents(dir.path());
        assert_eq!(retained.len(), MAX_HANDOFF_INTENTS);
        assert_eq!(
            retained.first().unwrap().issued_ms,
            MAX_HANDOFF_INTENTS as u64
        );
        assert_eq!(retained.last().unwrap().issued_ms, 1);
        assert!(retained.iter().all(|item| item.issued_ms > 0));
        assert_eq!(
            latest_intent(&retained).unwrap().issued_ms,
            MAX_HANDOFF_INTENTS as u64
        );
    }

    #[test]
    fn peer_state_reader_keeps_newest_bounded_backlog() {
        let dir = tempdir().unwrap();
        for index in 0..=MAX_PEER_STATE_SNAPSHOTS {
            write_state(
                dir.path(),
                &state(&format!("peer-{index:03}"), index % 2 == 0, index as u64),
            )
            .unwrap();
        }

        let retained = read_all_peer_states(dir.path());
        assert_eq!(retained.len(), MAX_PEER_STATE_SNAPSHOTS);
        assert!(!retained.iter().any(|item| item.peer == "peer-000"));
        assert!(retained.iter().any(|item| item.peer == "peer-064"));
        assert!(
            retained
                .windows(2)
                .all(|items| items[0].peer <= items[1].peer)
        );
    }

    #[test]
    fn oversized_handoff_state_records_are_ignored() {
        let dir = tempdir().unwrap();
        let intents = intents_dir(dir.path());
        std::fs::create_dir_all(&intents).unwrap();
        std::fs::write(
            intents.join("oversized.json"),
            vec![b'x'; MAX_STATE_RECORD_BYTES as usize + 1],
        )
        .unwrap();
        std::fs::write(
            state_path(dir.path()),
            vec![b'x'; MAX_STATE_RECORD_BYTES as usize + 1],
        )
        .unwrap();
        assert!(read_intents(dir.path()).is_empty());
        assert!(read_completions(dir.path()).is_empty());
        assert!(read_state(dir.path()).is_none());
    }

    #[test]
    fn read_state_absent_is_none() {
        let dir = tempdir().unwrap();
        assert_eq!(read_state(dir.path()), None);
    }

    #[test]
    fn local_host_is_stable_across_repeated_state_calls() {
        let first = local_host();
        assert!(!first.is_empty());
        assert_eq!(first, local_host());
    }
}
