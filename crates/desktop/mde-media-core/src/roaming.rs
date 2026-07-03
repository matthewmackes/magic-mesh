//! MEDIA-16: **playback session roaming** — the media session that follows an
//! operator between seats.
//!
//! A [`SessionRecord`] snapshots the live player (title / position / queue /
//! tracks / [`PlayerState`]) bound to the mesh **identity**, and a [`RoamingStore`]
//! syncs it **exactly the way mesh bookmarks + peer records sync**: one
//! append-target JSON file *per seat* under the Syncthing-replicated workgroup root
//! (`<root>/media-sessions/<identity>/<seat>.json`), written atomically (temp +
//! rename) and read back by folding every seat file — the same single-writer
//! per-file idiom [`mackes_mesh_types::peers::write_peer_record`] /
//! [`read_peers`](mackes_mesh_types::peers::read_peers) use, so Syncthing never sees
//! a write conflict and needs **no new transport**. The root itself comes from the
//! canonical [`mackes_mesh_types::peers::default_workgroup_root`] (never a hardcoded
//! `/mnt/mesh-storage`, which would reintroduce the documented split-brain).
//!
//! # The single owned lease (no double-play)
//!
//! Each record carries a monotonic [`SessionRecord::lease_gen`]. The **owner** is
//! the seat whose record holds the highest `(lease_gen, updated_ms, seat)` —
//! [`resolve_owner`]. When a user logs in at a new seat ([`RoamingSession::login`])
//! it **acquires** the lease by writing its own record at [`next_lease_gen`] (the
//! global max + 1), so it immediately becomes the sole owner. The seat it roamed
//! *away from* discovers on its next [`RoamingSession::poll`] that it no longer
//! holds the top lease and **releases** — it pauses playback and stops re-asserting
//! — so exactly one seat is ever playing. A momentary two-seat acquire race is
//! resolved deterministically by the `(updated_ms, seat)` tiebreak on the next poll
//! (eventual convergence, like the bookmarks CRDT fold) — never a permanent
//! double-play.
//!
//! # §6 / §7 posture — nothing faked
//!
//! The whole model is pure file I/O against a directory, so it runs unchanged on a
//! headless farm box and is fully unit-tested (including the two-seat resume
//! below). The one environmental condition is whether the workgroup root is
//! actually provisioned: [`RoamingStore`] writes only under an **already-present**
//! root ([`RoamingStore::is_ready`]) — a seat with no mesh volume is a silent
//! no-op ([`LoginOutcome::Offline`]), never a fabricated resume and never a write
//! into a bare unprovisioned mount.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use mackes_mesh_types::peers::default_workgroup_root;

use crate::engine::{MediaEngine, Track};
use crate::player::{Player, PlayerState};
use crate::playlist::Playlist;

/// The share subdirectory the per-identity session records live under
/// (`<root>/media-sessions/<identity>/<seat>.json`).
pub const SESSIONS_SUBDIR: &str = "media-sessions";

// ── the synced session record ────────────────────────────────────────────────

/// A snapshot of an operator's playback, bound to their mesh identity and synced
/// between seats.
///
/// Carries everything the acceptance needs to resume elsewhere — the `title`, the
/// `position_secs`, the `queue` ([`Playlist`], which folds the ordered items +
/// cursor + repeat/shuffle), the enumerated `tracks`, and the [`PlayerState`] —
/// plus the roaming bookkeeping (`identity` / `seat` / `lease_gen` / `updated_ms`).
/// Plain serde, persisted one-file-per-seat by [`RoamingStore`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The mesh identity this session belongs to — the roaming key.
    pub identity: String,
    /// The seat that wrote this record (the single writer of its own file).
    pub seat: String,
    /// The monotonic lease generation; the highest across seats owns the session.
    pub lease_gen: u64,
    /// Wall-clock epoch millis of the last update (freshness + lease tiebreak).
    pub updated_ms: u64,
    /// The now-playing display title, if any.
    pub title: Option<String>,
    /// The loaded media URL/path — the resume target (also the resume key).
    pub media: Option<String>,
    /// The last playback position in seconds (where a resume continues from).
    pub position_secs: f64,
    /// The media duration in seconds, if known.
    pub duration_secs: Option<f64>,
    /// The playback state captured at write time.
    pub state: PlayerState,
    /// The playback queue (ordered items + cursor + repeat/shuffle).
    pub queue: Playlist,
    /// The enumerated tracks of the loaded media.
    pub tracks: Vec<Track>,
}

impl SessionRecord {
    /// Capture the live [`Player`] into a record for `identity` / `seat` at
    /// `lease_gen`, stamped `now_ms`.
    #[must_use]
    pub fn capture<E: MediaEngine>(
        player: &Player<E>,
        identity: impl Into<String>,
        seat: impl Into<String>,
        lease_gen: u64,
        now_ms: u64,
    ) -> Self {
        Self {
            identity: identity.into(),
            seat: seat.into(),
            lease_gen,
            updated_ms: now_ms,
            title: title_for(player),
            media: player.media().map(ToOwned::to_owned),
            position_secs: player.position(),
            duration_secs: player.duration(),
            state: player.state(),
            queue: player.playlist().clone(),
            tracks: player.tracks().to_vec(),
        }
    }

    /// Whether this session is worth resuming at a new seat: it has loaded media and
    /// was actively playing or paused (a `Stopped`/`Idle`/`Ended` session has
    /// nothing to continue).
    #[must_use]
    pub const fn is_resumable(&self) -> bool {
        self.media.is_some() && matches!(self.state, PlayerState::Playing | PlayerState::Paused)
    }

    /// Re-key this snapshot onto `seat` at `lease_gen`, stamped `now_ms` — the
    /// arriving seat's owning record when it acquires the lease (the playback
    /// payload is carried verbatim so the surface reflects the roamed session at
    /// once).
    #[must_use]
    pub fn reseat(&self, seat: impl Into<String>, lease_gen: u64, now_ms: u64) -> Self {
        Self {
            seat: seat.into(),
            lease_gen,
            updated_ms: now_ms,
            ..self.clone()
        }
    }
}

/// The owning record of `records` — the highest `(lease_gen, updated_ms, seat)`.
///
/// This is the single-owner lease resolution: the seat whose record wins is the one
/// allowed to play. Deterministic (the `(updated_ms, seat)` tiebreak settles a
/// same-generation acquire race). [`None`] for an empty set.
#[must_use]
pub fn resolve_owner(records: &[SessionRecord]) -> Option<&SessionRecord> {
    records.iter().max_by(|a, b| {
        a.lease_gen
            .cmp(&b.lease_gen)
            .then_with(|| a.updated_ms.cmp(&b.updated_ms))
            .then_with(|| a.seat.cmp(&b.seat))
    })
}

/// The next lease generation to acquire over `records`: one past the current max
/// (so a fresh acquire always becomes the sole owner). `1` for an empty set.
#[must_use]
pub fn next_lease_gen(records: &[SessionRecord]) -> u64 {
    records
        .iter()
        .map(|r| r.lease_gen)
        .max()
        .map_or(1, |max| max.saturating_add(1))
}

/// The now-playing title: the current queue item's title, else the media file name.
fn title_for<E: MediaEngine>(player: &Player<E>) -> Option<String> {
    if let Some(item) = player.playlist().current() {
        if let Some(title) = &item.title {
            if !title.trim().is_empty() {
                return Some(title.clone());
            }
        }
    }
    player.media().map(title_from_path)
}

/// The display title derived from a media URL/path — its final path component.
fn title_from_path(media: &str) -> String {
    media
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(media)
        .to_owned()
}

// ── the per-seat synced store ──────────────────────────────────────────────────

/// The mesh-synced session store — one JSON file per seat under the workgroup root,
/// the same single-writer-per-file idiom mesh peer records + bookmarks use.
///
/// Every seat writes only its own `<root>/media-sessions/<identity>/<seat>.json`
/// (atomic temp + rename, so a reader never sees a half-write) and reads a session
/// by folding every seat file for that identity — Syncthing replicates the files
/// out of band, so there is no new transport.
#[derive(Debug, Clone)]
pub struct RoamingStore {
    /// The Syncthing-replicated workgroup root the session files live under.
    root: PathBuf,
}

impl RoamingStore {
    /// A store rooted at `root` (tests point this at a tempdir).
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// A store over the canonical workgroup root
    /// ([`mackes_mesh_types::peers::default_workgroup_root`]).
    #[must_use]
    pub fn open_default() -> Self {
        Self::new(default_workgroup_root())
    }

    /// The workgroup root this store writes under.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether the workgroup root is actually present. The store writes only under an
    /// existing root — never creating a bare unprovisioned mount — so a seat with no
    /// mesh volume is a silent no-op rather than a fabricated local session.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.root.is_dir()
    }

    /// The `<root>/media-sessions/<identity>/` directory.
    fn identity_dir(&self, identity: &str) -> PathBuf {
        self.root.join(SESSIONS_SUBDIR).join(sanitize(identity))
    }

    /// The `<root>/media-sessions/<identity>/<seat>.json` path.
    fn seat_path(&self, identity: &str, seat: &str) -> PathBuf {
        self.identity_dir(identity)
            .join(format!("{}.json", sanitize(seat)))
    }

    /// Publish `rec` into this seat's file (atomic temp + rename). A silent no-op
    /// when the root is not provisioned ([`is_ready`](Self::is_ready)).
    ///
    /// # Errors
    /// The [`io::Error`] if the directory cannot be created or the file cannot be
    /// written / renamed.
    pub fn publish(&self, rec: &SessionRecord) -> io::Result<()> {
        if !self.is_ready() {
            return Ok(());
        }
        let dir = self.identity_dir(&rec.identity);
        fs::create_dir_all(&dir)?;
        let seat = sanitize(&rec.seat);
        let final_path = dir.join(format!("{seat}.json"));
        let tmp_path = dir.join(format!(".{seat}.json.tmp"));
        let json = serde_json::to_string_pretty(rec)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }

    /// Fold every seat file for `identity` into a record list (one per seat).
    /// Malformed / half-written / temp files are skipped (never fatal), and a
    /// missing directory yields an empty list — exactly like
    /// [`read_peers`](mackes_mesh_types::peers::read_peers).
    #[must_use]
    pub fn records(&self, identity: &str) -> Vec<SessionRecord> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.identity_dir(identity)) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue; // an in-flight atomic-write temp file
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(rec) = serde_json::from_str::<SessionRecord>(&data) {
                    out.push(rec);
                }
            }
        }
        out.sort_by(|a, b| a.seat.cmp(&b.seat));
        out
    }

    /// The current owning session for `identity`, if any ([`resolve_owner`] over the
    /// folded records).
    #[must_use]
    pub fn current(&self, identity: &str) -> Option<SessionRecord> {
        resolve_owner(&self.records(identity)).cloned()
    }

    /// The seat that currently owns `identity`'s session, if any.
    #[must_use]
    pub fn owner_seat(&self, identity: &str) -> Option<String> {
        resolve_owner(&self.records(identity)).map(|r| r.seat.clone())
    }

    /// The next lease generation to acquire for `identity` (one past the current
    /// max across seats).
    #[must_use]
    pub fn next_lease_gen(&self, identity: &str) -> u64 {
        next_lease_gen(&self.records(identity))
    }

    /// Remove this seat's record for `identity` (an explicit leave). Absent is not an
    /// error.
    ///
    /// # Errors
    /// The [`io::Error`] if the file exists but cannot be removed.
    pub fn release(&self, identity: &str, seat: &str) -> io::Result<()> {
        match fs::remove_file(self.seat_path(identity, seat)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Reduce an identity / seat id to a safe single path component
/// (`[A-Za-z0-9_-]`, everything else → `_`; never empty, never `.`/`..`).
fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

// ── the roaming session (login / poll / publish orchestration) ──────────────────

/// A deferred resume seek — applied once the arriving seat's engine has the file
/// open (the roamed position can't be sought until the media is loaded).
#[derive(Debug, Clone, Copy)]
struct PendingResume {
    /// The position (seconds) to resume at.
    position: f64,
    /// Whether the roamed session was paused (so the arriving seat opens paused).
    was_paused: bool,
}

/// What a [`RoamingSession::login`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginOutcome {
    /// A prior session was picked up — playback resumes where it was left.
    Resumed {
        /// The resumed title, if any.
        title: Option<String>,
        /// The position (seconds) playback resumes from.
        position_secs: f64,
    },
    /// No resumable session existed; this seat took a fresh lease so a later seat
    /// can roam from here.
    FreshLease,
    /// The workgroup root is not provisioned — roaming is inert (honest offline).
    Offline,
}

/// What a [`RoamingSession::poll`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    /// This seat still owns the session (its live position was checkpointed).
    Owner,
    /// Another seat acquired the lease — playback was released here (no double-play).
    Released,
    /// The workgroup root is not provisioned — roaming is inert.
    Offline,
}

/// The per-seat roaming controller: owns the [`RoamingStore`] seam, this seat's held
/// lease generation, and the deferred resume seek.
///
/// The surface drives it: [`login`](Self::login) on start-up (resume + acquire the
/// lease), [`apply_pending`](Self::apply_pending) each pump (land the resume seek
/// once loaded), [`publish`](Self::publish) on a playback change (checkpoint), and
/// [`poll`](Self::poll) on an interval (checkpoint if owner, release if not).
#[derive(Debug)]
pub struct RoamingSession {
    /// The mesh-synced store seam.
    store: RoamingStore,
    /// The mesh identity this session roams under.
    identity: String,
    /// This seat's id.
    seat: String,
    /// The lease generation this seat currently holds (`0` before login / offline).
    held_gen: u64,
    /// A resume seek awaiting the engine opening the file.
    pending: Option<PendingResume>,
    /// Whether playback has been released here after losing the lease (so the
    /// release fires once, not every poll).
    released: bool,
}

impl RoamingSession {
    /// A roaming session over `store` for `identity` at `seat`.
    #[must_use]
    pub fn new(store: RoamingStore, identity: impl Into<String>, seat: impl Into<String>) -> Self {
        Self {
            store,
            identity: identity.into(),
            seat: seat.into(),
            held_gen: 0,
            pending: None,
            released: false,
        }
    }

    /// A roaming session over the canonical workgroup root, with the mesh identity +
    /// seat resolved from the environment ([`resolve_identity`] / [`resolve_seat`]).
    #[must_use]
    pub fn open_default() -> Self {
        Self::new(
            RoamingStore::open_default(),
            resolve_identity(),
            resolve_seat(),
        )
    }

    /// The mesh identity this session roams under.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// This seat's id.
    #[must_use]
    pub fn seat(&self) -> &str {
        &self.seat
    }

    /// The lease generation this seat currently holds (`0` before login / offline).
    #[must_use]
    pub const fn held_gen(&self) -> u64 {
        self.held_gen
    }

    /// The store seam (read-only) — the surface reads
    /// [`owner_seat`](RoamingStore::owner_seat) etc. from it.
    #[must_use]
    pub const fn store(&self) -> &RoamingStore {
        &self.store
    }

    /// Log in at this seat: acquire the single owned lease and, when a resumable
    /// session exists, restore its queue + arrange a resume seek so playback picks up
    /// where it was left.
    ///
    /// Acquiring writes this seat's record at [`next_lease_gen`], so this seat is at
    /// once the sole owner and the seat it roamed from will release on its next
    /// [`poll`](Self::poll). A no-op ([`LoginOutcome::Offline`]) when the workgroup
    /// root is not provisioned.
    pub fn login<E: MediaEngine>(&mut self, player: &mut Player<E>, now_ms: u64) -> LoginOutcome {
        if !self.store.is_ready() {
            return LoginOutcome::Offline;
        }
        let records = self.store.records(&self.identity);
        self.held_gen = next_lease_gen(&records);
        self.released = false;
        if let Some(current) = resolve_owner(&records) {
            if current.is_resumable() {
                let owning = current.reseat(&self.seat, self.held_gen, now_ms);
                let title = owning.title.clone();
                let position = owning.position_secs;
                let was_paused = matches!(current.state, PlayerState::Paused);
                player.set_playlist(owning.queue.clone());
                if let Some(media) = &owning.media {
                    // Best-effort load; the pending seek lands once the file opens.
                    let _ = player.load(media.clone());
                    self.pending = Some(PendingResume {
                        position,
                        was_paused,
                    });
                }
                // Take the lease immediately (carry the roamed playback payload).
                let _ = self.store.publish(&owning);
                return LoginOutcome::Resumed {
                    title,
                    position_secs: position,
                };
            }
        }
        // Nothing to resume — still claim a lease so a later seat roams from here.
        let idle =
            SessionRecord::capture(player, &self.identity, &self.seat, self.held_gen, now_ms);
        let _ = self.store.publish(&idle);
        LoginOutcome::FreshLease
    }

    /// Land a pending resume seek once the engine has the file open (`Playing` /
    /// `Paused` after `FileLoaded`). Cheap + I/O-free — call every pump.
    pub fn apply_pending<E: MediaEngine>(&mut self, player: &mut Player<E>) {
        let Some(pending) = self.pending else {
            return;
        };
        if matches!(player.state(), PlayerState::Playing | PlayerState::Paused) {
            let _ = player.seek(pending.position);
            if pending.was_paused {
                let _ = player.pause();
            }
            self.pending = None;
        }
    }

    /// Checkpoint the live player into this seat's record. A no-op before login /
    /// offline.
    pub fn publish<E: MediaEngine>(&mut self, player: &Player<E>, now_ms: u64) {
        if self.held_gen == 0 || !self.store.is_ready() {
            return;
        }
        let rec = SessionRecord::capture(player, &self.identity, &self.seat, self.held_gen, now_ms);
        let _ = self.store.publish(&rec);
    }

    /// Converge with the shared plane: if this seat still owns the lease, checkpoint
    /// its live position; if another seat has acquired it, **release** — pause
    /// playback so only the new owner plays (no double-play).
    pub fn poll<E: MediaEngine>(&mut self, player: &mut Player<E>, now_ms: u64) -> PollOutcome {
        if self.held_gen == 0 || !self.store.is_ready() {
            return PollOutcome::Offline;
        }
        match self.store.owner_seat(&self.identity) {
            Some(seat) if seat != self.seat => {
                if !self.released {
                    let _ = player.pause();
                    self.released = true;
                }
                PollOutcome::Released
            }
            _ => {
                self.released = false;
                self.publish(player, now_ms);
                PollOutcome::Owner
            }
        }
    }
}

// ── environment resolution (mirrors the bookmarks worker) ───────────────────────

/// Resolve the mesh identity playback roams under: `$MDE_MESH_USER` → `$USER` /
/// `$LOGNAME` → a stable `operator` fallback (the same precedence the mesh
/// bookmarks worker attributes ops to).
#[must_use]
pub fn resolve_identity() -> String {
    for key in ["MDE_MESH_USER", "USER", "LOGNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    "operator".to_owned()
}

/// Resolve this seat's id: `$MDE_MESH_SEAT` → `$HOSTNAME` → `/etc/hostname` → a
/// stable `seat` fallback (the seat is the per-file writer, like a peer record's
/// hostname).
#[must_use]
pub fn resolve_seat() -> String {
    for key in ["MDE_MESH_SEAT", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    if let Ok(host) = fs::read_to_string("/etc/hostname") {
        let host = host.trim();
        if !host.is_empty() {
            return host.to_owned();
        }
    }
    "seat".to_owned()
}

/// Wall-clock epoch millis — the record timestamp / lease tiebreak source the
/// surface stamps writes with.
#[must_use]
pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeMpv;
    use crate::playlist::PlaylistItem;

    fn player() -> Player<FakeMpv> {
        Player::new(FakeMpv::new().with_duration(120.0))
    }

    fn record(seat: &str, gen: u64, updated: u64) -> SessionRecord {
        SessionRecord {
            identity: "matthew".to_owned(),
            seat: seat.to_owned(),
            lease_gen: gen,
            updated_ms: updated,
            title: None,
            media: Some("movie.mkv".to_owned()),
            position_secs: 10.0,
            duration_secs: Some(120.0),
            state: PlayerState::Paused,
            queue: Playlist::new(),
            tracks: Vec::new(),
        }
    }

    // ── pure lease resolution ──────────────────────────────────────────────────

    #[test]
    fn owner_is_the_highest_lease_generation() {
        let records = vec![
            record("a", 1, 100),
            record("b", 3, 100),
            record("c", 2, 500),
        ];
        assert_eq!(resolve_owner(&records).expect("owner").seat, "b");
        // Acquiring is always strictly above the current max.
        assert_eq!(next_lease_gen(&records), 4);
        assert_eq!(next_lease_gen(&[]), 1);
    }

    #[test]
    fn same_generation_acquire_race_resolves_deterministically() {
        // Two seats both wrote gen 2 (a race) — the (updated_ms, seat) tiebreak
        // settles a single winner, so the next poll converges (no permanent
        // double-play).
        let records = vec![record("seat-a", 2, 200), record("seat-b", 2, 500)];
        assert_eq!(resolve_owner(&records).expect("owner").seat, "seat-b");
        let tie = vec![record("seat-a", 2, 200), record("seat-b", 2, 200)];
        assert_eq!(resolve_owner(&tie).expect("owner").seat, "seat-b");
    }

    #[test]
    fn is_resumable_only_for_loaded_playing_or_paused() {
        let mut rec = record("a", 1, 0);
        rec.state = PlayerState::Paused;
        assert!(rec.is_resumable());
        rec.state = PlayerState::Playing;
        assert!(rec.is_resumable());
        rec.state = PlayerState::Stopped;
        assert!(!rec.is_resumable());
        rec.state = PlayerState::Paused;
        rec.media = None;
        assert!(!rec.is_resumable());
    }

    // ── the store round-trips + skips corruption ───────────────────────────────

    #[test]
    fn store_round_trips_and_folds_across_seats() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = RoamingStore::new(dir.path().to_path_buf());
        store.publish(&record("seat-a", 1, 100)).expect("publish a");
        store.publish(&record("seat-b", 2, 200)).expect("publish b");
        let records = store.records("matthew");
        assert_eq!(records.len(), 2, "one file per seat, folded");
        assert_eq!(store.owner_seat("matthew").as_deref(), Some("seat-b"));
        assert_eq!(store.next_lease_gen("matthew"), 3);

        // A corrupt file is skipped, not fatal.
        let corrupt = dir
            .path()
            .join(SESSIONS_SUBDIR)
            .join("matthew")
            .join("seat-c.json");
        std::fs::write(&corrupt, "{ not json").expect("write corrupt");
        assert_eq!(store.records("matthew").len(), 2, "corrupt file skipped");

        // Release removes only this seat's file.
        store.release("matthew", "seat-a").expect("release");
        assert_eq!(store.owner_seat("matthew").as_deref(), Some("seat-b"));
        assert_eq!(store.records("matthew").len(), 1);
    }

    #[test]
    fn store_is_inert_when_the_root_is_unprovisioned() {
        let store = RoamingStore::new(PathBuf::from("/no/such/mesh/root"));
        assert!(!store.is_ready());
        // Publishing is a silent no-op; reading yields nothing — never a panic.
        store
            .publish(&record("seat-a", 1, 0))
            .expect("no-op publish");
        assert!(store.records("matthew").is_empty());
        assert_eq!(store.owner_seat("matthew"), None);
    }

    #[test]
    fn login_offline_when_root_missing() {
        let mut session = RoamingSession::new(
            RoamingStore::new(PathBuf::from("/no/such/mesh/root")),
            "matthew",
            "seat-a",
        );
        let mut p = player();
        assert_eq!(session.login(&mut p, 1000), LoginOutcome::Offline);
        assert_eq!(session.held_gen(), 0);
    }

    // ── THE CRUX: two-seat resume with a single owned lease ────────────────────

    #[test]
    fn two_seats_roam_playback_with_a_single_owned_lease() {
        // One shared workgroup root = the Syncthing-replicated dir both seats see.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();

        // ── Seat A: play a queued title, then pause at 45s. ──
        let mut pa = player();
        let mut sa = RoamingSession::new(RoamingStore::new(root.clone()), "matthew", "seat-a");
        assert_eq!(sa.login(&mut pa, 1_000), LoginOutcome::FreshLease);
        pa.playlist_mut()
            .push(PlaylistItem::titled("movie.mkv", "The Movie"));
        pa.playlist_mut().push(PlaylistItem::new("next.mkv"));
        pa.load("movie.mkv").expect("load");
        pa.pump(); // → Playing
        pa.seek(45.0).expect("seek");
        pa.pause().expect("pause"); // Paused @45
        sa.publish(&pa, 2_000);

        // ── Seat B logs in at a NEW seat: resume where paused + take the lease. ──
        let mut pb = player();
        let mut sb = RoamingSession::new(RoamingStore::new(root.clone()), "matthew", "seat-b");
        let outcome = sb.login(&mut pb, 3_000);
        assert_eq!(
            outcome,
            LoginOutcome::Resumed {
                title: Some("The Movie".to_owned()),
                position_secs: 45.0,
            }
        );
        // The queue roamed too (2 items).
        assert_eq!(pb.playlist().items().len(), 2);
        // Land the deferred resume seek once the file is open.
        pb.pump(); // Loading → Playing
        sb.apply_pending(&mut pb);
        assert_eq!(pb.state(), PlayerState::Paused, "resumed paused");
        assert!(
            (pb.position() - 45.0).abs() < f64::EPSILON,
            "resume continues from the paused position"
        );
        // B is now the sole owner (single owned lease).
        let store = RoamingStore::new(root);
        assert_eq!(store.owner_seat("matthew").as_deref(), Some("seat-b"));
        assert!(sb.held_gen() > sa.held_gen(), "B acquired a higher lease");

        // ── Seat A converges: it lost the lease → releases (no double-play). ──
        pa.play().expect("A resumes locally"); // pretend A kept playing
        assert_eq!(pa.state(), PlayerState::Playing);
        assert_eq!(sa.poll(&mut pa, 4_000), PollOutcome::Released);
        assert_eq!(
            pa.state(),
            PlayerState::Paused,
            "the old seat is released — only one seat plays"
        );
        // B still owns on its own poll.
        assert_eq!(sb.poll(&mut pb, 5_000), PollOutcome::Owner);
        assert_eq!(store.owner_seat("matthew").as_deref(), Some("seat-b"));
    }
}
