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

/// Hard ceiling for one replicated per-seat session record. A session can carry
/// a queue and a track list, but a hostile replicated row must not become an
/// unbounded allocation before it is parsed.
const MAX_SESSION_RECORD_BYTES: usize = 16 * 1024 * 1024;
/// Maximum queue entries admitted from one replicated session row.
const MAX_SESSION_QUEUE_ITEMS: usize = 4_096;
/// Maximum decoder tracks carried by one replicated session row.
const MAX_SESSION_TRACKS: usize = 256;
/// Maximum identity, seat, media, and display strings in one session row.
const MAX_SESSION_TEXT_BYTES: usize = 4_096;

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
            let Some(seat_stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if let Some(data) = read_bounded_session_record(&path) {
                if let Ok(rec) = serde_json::from_str::<SessionRecord>(&data) {
                    if valid_session_record(&rec, identity, seat_stem) {
                        out.push(rec);
                    }
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

/// Admit only a session row that belongs to the requested identity and its
/// single-writer seat file, and whose playback payload stays within the
/// roaming contract. The byte cap protects parsing; these semantic checks
/// protect lease selection and resume from a forged replicated row.
fn valid_session_record(record: &SessionRecord, identity: &str, seat_stem: &str) -> bool {
    let bounded_text = |value: &str| {
        !value.is_empty()
            && value.len() <= MAX_SESSION_TEXT_BYTES
            && !value.chars().any(char::is_control)
    };
    if record.identity != identity
        || !bounded_text(&record.identity)
        || !bounded_text(&record.seat)
        || sanitize(&record.seat) != seat_stem
        || record.lease_gen == 0
        || record.lease_gen == u64::MAX
        || record.updated_ms == 0
        || !record.position_secs.is_finite()
        || record.position_secs < 0.0
        || record.duration_secs.is_some_and(|duration| {
            !duration.is_finite() || duration < 0.0 || record.position_secs > duration
        })
        || record
            .media
            .as_deref()
            .is_some_and(|media| !bounded_text(media))
        || record
            .title
            .as_deref()
            .is_some_and(|title| !bounded_text(title))
        || record.queue.len() > MAX_SESSION_QUEUE_ITEMS
        || record
            .queue
            .current_index()
            .is_some_and(|index| index >= record.queue.len())
        || record.queue.items().iter().any(|item| {
            !bounded_text(&item.url)
                || item
                    .title
                    .as_deref()
                    .is_some_and(|title| !bounded_text(title))
        })
        || record.tracks.len() > MAX_SESSION_TRACKS
        || record.tracks.iter().any(|track| {
            track.id <= 0
                || track
                    .title
                    .as_deref()
                    .is_some_and(|title| !bounded_text(title))
                || track
                    .lang
                    .as_deref()
                    .is_some_and(|lang| !bounded_text(lang))
                || track
                    .codec
                    .as_deref()
                    .is_some_and(|codec| !bounded_text(codec))
        })
    {
        return false;
    }
    true
}

/// Read one replicated session record through a bounded descriptor.
///
/// The final path component is opened without following a symlink and with
/// non-blocking semantics, so a FIFO or another special file cannot stall a
/// roaming poll. Descriptor metadata admits only regular files; reading one
/// byte beyond the cap and comparing the descriptor length before and after
/// consumption rejects oversized or growing rows. UTF-8 is validated before
/// the caller materializes JSON.
fn read_bounded_session_record(path: &Path) -> Option<String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400_000 | 0o4000 | 0o2_000_000); // O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK

        // Unsupported Unix targets still fail closed for a final symlink when
        // their standard library does not expose O_NOFOLLOW here.
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_file())
        {
            return None;
        }
    }
    #[cfg(not(unix))]
    if !fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_file())
    {
        return None;
    }

    let file = options.open(path).ok()?;
    let before = file.metadata().ok()?;
    if !before.file_type().is_file() || before.len() > MAX_SESSION_RECORD_BYTES as u64 {
        return None;
    }
    read_bounded_session_record_file(&file, &before)
}

/// Consume an already-open session-record descriptor and reject metadata
/// changes observed during the read. Keeping this boundary separate makes the
/// growth check directly testable without introducing timing-sensitive sleeps.
fn read_bounded_session_record_file(file: &fs::File, before: &fs::Metadata) -> Option<String> {
    use std::io::Read as _;

    let capacity = usize::try_from(before.len())
        .unwrap_or(MAX_SESSION_RECORD_BYTES)
        .min(MAX_SESSION_RECORD_BYTES)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    file.take((MAX_SESSION_RECORD_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    let after = file.metadata().ok()?;
    if !after.file_type().is_file()
        || after.len() != before.len()
        || bytes.len() > MAX_SESSION_RECORD_BYTES
        || bytes.len() as u64 != before.len()
    {
        return None;
    }
    String::from_utf8(bytes).ok()
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
#[derive(Debug, Clone)]
struct PendingResume {
    /// The exact media URL/path that must be open before the seek is applied.
    media: String,
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
    /// The replicated lease-generation space is exhausted or the exact lease
    /// claim could not be durably published and confirmed, so this seat did not
    /// resume playback.
    LeaseUnavailable,
    /// The prior owner's media could not be opened here, so the existing owner
    /// keeps the lease and this seat does not claim or alter it.
    ResumeUnavailable,
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
    /// The workgroup root was not provisioned before login — roaming is inert.
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
    /// root is not provisioned, or [`LoginOutcome::LeaseUnavailable`] when no
    /// admissible generation remains.
    pub fn login<E: MediaEngine>(&mut self, player: &mut Player<E>, now_ms: u64) -> LoginOutcome {
        if !self.store.is_ready() {
            return LoginOutcome::Offline;
        }
        let records = self.store.records(&self.identity);
        self.held_gen = next_lease_gen(&records);
        self.pending = None;
        self.released = false;
        if self.held_gen == u64::MAX {
            self.held_gen = 0;
            self.released = true;
            return LoginOutcome::LeaseUnavailable;
        }
        if let Some(current) = resolve_owner(&records) {
            if current.is_resumable() {
                let owning = current.reseat(&self.seat, self.held_gen, now_ms);
                let title = owning.title.clone();
                let position = owning.position_secs;
                let was_paused = matches!(current.state, PlayerState::Paused);
                if let Some(media) = &owning.media {
                    // Do not displace the current owner until this seat can at
                    // least hand the target to its engine. A rejected load must
                    // leave the target untouched and the source lease intact.
                    if player.load(media.clone()).is_err() {
                        self.held_gen = 0;
                        self.released = true;
                        return LoginOutcome::ResumeUnavailable;
                    }
                }
                // Take and confirm the exact lease before arming any playback
                // payload. A failed rename or a concurrent higher claim leaves
                // the source as owner; reporting a resume here would let this
                // target play without authority until the next poll.
                if self.store.publish(&owning).is_err() || !self.holds_current_lease() {
                    self.held_gen = 0;
                    self.release_local(player);
                    return LoginOutcome::LeaseUnavailable;
                }
                player.set_playlist(owning.queue.clone());
                // The pending seek lands once the file opens.
                self.pending = owning.media.map(|media| PendingResume {
                    media,
                    position,
                    was_paused,
                });
                return LoginOutcome::Resumed {
                    title,
                    position_secs: position,
                };
            }
        }
        // Nothing to resume — still claim a lease so a later seat roams from here.
        let idle =
            SessionRecord::capture(player, &self.identity, &self.seat, self.held_gen, now_ms);
        if self.store.publish(&idle).is_err() || !self.holds_current_lease() {
            self.held_gen = 0;
            self.release_local(player);
            return LoginOutcome::LeaseUnavailable;
        }
        LoginOutcome::FreshLease
    }

    /// Land a pending resume seek once the engine has the file open (`Playing` /
    /// `Paused` after `FileLoaded`). Cheap + I/O-free — call every pump.
    pub fn apply_pending<E: MediaEngine>(&mut self, player: &mut Player<E>) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if !self.holds_current_lease() {
            self.release_local(player);
            return;
        }
        if player.media() != Some(pending.media.as_str()) {
            // The handoff load failed or the user replaced it before it became
            // ready. Never seek a stale handoff position into another item.
            self.pending = None;
            return;
        }
        if matches!(player.state(), PlayerState::Playing | PlayerState::Paused) {
            if player.seek(pending.position).is_err() {
                // Do not let a target that loaded at position zero keep playing
                // when the handoff seek was rejected. Retain the request so a
                // later pump can retry after the engine becomes seekable.
                let _ = player.pause();
                return;
            }
            if pending.was_paused && player.pause().is_err() {
                // The position landed, but the source's paused intent did not;
                // retain the request rather than claiming a complete handoff.
                return;
            }
            self.pending = None;
        }
    }

    /// Checkpoint the live player into this seat's record. A no-op before login /
    /// offline.
    pub fn publish<E: MediaEngine>(&mut self, player: &Player<E>, now_ms: u64) {
        if !self.holds_current_lease() {
            return;
        }
        let rec = SessionRecord::capture(player, &self.identity, &self.seat, self.held_gen, now_ms);
        let _ = self.store.publish(&rec);
    }

    /// Converge with the shared plane: if this seat still owns the lease, checkpoint
    /// its live position; if another seat has acquired it, **release** — pause
    /// playback so only the new owner plays (no double-play). Losing an already-held
    /// workgroup root is also a release: an active seat must not keep playing while
    /// its lease/control plane is unavailable.
    pub fn poll<E: MediaEngine>(&mut self, player: &mut Player<E>, now_ms: u64) -> PollOutcome {
        if self.held_gen == 0 {
            return PollOutcome::Offline;
        }
        if !self.store.is_ready() {
            if !self.released {
                self.release_local(player);
            }
            return PollOutcome::Released;
        }
        if !self.holds_current_lease() {
            if !self.released {
                self.release_local(player);
            }
            return PollOutcome::Released;
        }
        self.released = false;
        self.publish(player, now_ms);
        PollOutcome::Owner
    }

    /// Stop this seat from producing playback while it no longer has a usable
    /// shared lease. Clearing the deferred resume is essential: a later pump must
    /// not seek into media after the seat has yielded ownership.
    fn release_local<E: MediaEngine>(&mut self, player: &mut Player<E>) {
        self.pending = None;
        if matches!(player.state(), PlayerState::Loading | PlayerState::Playing) {
            let _ = player.pause();
        }
        self.released = true;
    }

    /// Whether the shared plane still names this exact seat and generation as
    /// owner. A missing, replaced, or otherwise invalid row is a yielded lease;
    /// treating it as ownership would let a seat reassert after replication loss.
    fn holds_current_lease(&self) -> bool {
        let held_generation = self.held_gen;
        self.store
            .current(&self.identity)
            .is_some_and(|record| record.seat == self.seat && record.lease_gen == held_generation)
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
    use crate::audio::AudioConfig;
    use crate::controls::{PlaybackControls, ScreenshotMode};
    use crate::engine::{EngineError, EngineSignal, MediaEngine, VideoFrame};
    use crate::fake::FakeMpv;
    use crate::playlist::PlaylistItem;
    use crate::subtitle::{SubtitleConfig, TrackSelection};
    use crate::video::VideoConfig;

    #[derive(Debug)]
    struct SeekRejectingEngine {
        inner: FakeMpv,
        reject_seek: bool,
    }

    impl SeekRejectingEngine {
        fn new() -> Self {
            Self {
                inner: FakeMpv::new().with_duration(120.0),
                reject_seek: true,
            }
        }

        fn allow_seek(&mut self) {
            self.reject_seek = false;
        }
    }

    impl MediaEngine for SeekRejectingEngine {
        fn load_file(&mut self, url: &str) -> Result<(), EngineError> {
            self.inner.load_file(url)
        }

        fn set_paused(&mut self, paused: bool) -> Result<(), EngineError> {
            self.inner.set_paused(paused)
        }

        fn seek_absolute(&mut self, position_secs: f64) -> Result<(), EngineError> {
            if self.reject_seek {
                return Err(EngineError::Backend("fixture seek rejected".to_owned()));
            }
            self.inner.seek_absolute(position_secs)
        }

        fn stop(&mut self) -> Result<(), EngineError> {
            self.inner.stop()
        }

        fn position(&self) -> Option<f64> {
            self.inner.position()
        }

        fn duration(&self) -> Option<f64> {
            self.inner.duration()
        }

        fn tracks(&self) -> Vec<Track> {
            self.inner.tracks()
        }

        fn poll(&mut self) -> Vec<EngineSignal> {
            self.inner.poll()
        }

        fn apply_audio_config(&mut self, config: &AudioConfig) -> Result<(), EngineError> {
            self.inner.apply_audio_config(config)
        }

        fn apply_video_config(&mut self, config: &VideoConfig) -> Result<(), EngineError> {
            self.inner.apply_video_config(config)
        }

        fn apply_track_selection(&mut self, selection: &TrackSelection) -> Result<(), EngineError> {
            self.inner.apply_track_selection(selection)
        }

        fn apply_subtitle_config(&mut self, config: &SubtitleConfig) -> Result<(), EngineError> {
            self.inner.apply_subtitle_config(config)
        }

        fn apply_playback_controls(
            &mut self,
            controls: &PlaybackControls,
        ) -> Result<(), EngineError> {
            self.inner.apply_playback_controls(controls)
        }

        fn frame_step(&mut self, forward: bool) -> Result<(), EngineError> {
            self.inner.frame_step(forward)
        }

        fn screenshot(&mut self, mode: ScreenshotMode) -> Result<(), EngineError> {
            self.inner.screenshot(mode)
        }

        fn chapter(&self) -> Option<i64> {
            self.inner.chapter()
        }

        fn chapter_count(&self) -> Option<i64> {
            self.inner.chapter_count()
        }

        fn set_chapter(&mut self, chapter: i64) -> Result<(), EngineError> {
            self.inner.set_chapter(chapter)
        }

        fn latest_frame(&mut self) -> Option<VideoFrame> {
            self.inner.latest_frame()
        }
    }

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

        let invalid_utf8 = dir
            .path()
            .join(SESSIONS_SUBDIR)
            .join("matthew")
            .join("seat-d.json");
        std::fs::write(&invalid_utf8, [0xff, 0xfe]).expect("write invalid UTF-8");
        let oversized = dir
            .path()
            .join(SESSIONS_SUBDIR)
            .join("matthew")
            .join("seat-e.json");
        std::fs::write(&oversized, vec![b'x'; MAX_SESSION_RECORD_BYTES + 1])
            .expect("write oversized record");
        let non_regular = dir
            .path()
            .join(SESSIONS_SUBDIR)
            .join("matthew")
            .join("seat-f.json");
        std::fs::create_dir(&non_regular).expect("create non-regular record");
        assert_eq!(
            store.records("matthew").len(),
            2,
            "hostile rows are skipped without losing valid seats"
        );

        // Release removes only this seat's file.
        store.release("matthew", "seat-a").expect("release");
        assert_eq!(store.owner_seat("matthew").as_deref(), Some("seat-b"));
        assert_eq!(store.records("matthew").len(), 1);
    }

    #[test]
    fn store_rejects_cross_identity_seat_and_semantic_tampering() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = RoamingStore::new(dir.path().to_path_buf());
        store
            .publish(&record("seat-a", 1, 100))
            .expect("publish valid");
        let identity_dir = dir.path().join(SESSIONS_SUBDIR).join("matthew");

        let mut wrong_identity = record("seat-b", 2, 200);
        wrong_identity.identity = "another-user".into();
        std::fs::write(
            identity_dir.join("seat-b.json"),
            serde_json::to_string(&wrong_identity).expect("serialize identity tamper"),
        )
        .expect("write identity tamper");

        let mut wrong_seat = record("seat-c", 3, 300);
        std::fs::write(
            identity_dir.join("seat-b-claimed.json"),
            serde_json::to_string(&wrong_seat).expect("serialize seat tamper"),
        )
        .expect("write seat tamper");

        wrong_seat.lease_gen = u64::MAX;
        std::fs::write(
            identity_dir.join("seat-c.json"),
            serde_json::to_string(&wrong_seat).expect("serialize lease tamper"),
        )
        .expect("write lease tamper");

        let mut oversized_title = record("seat-d", 4, 400);
        oversized_title.title = Some("x".repeat(MAX_SESSION_TEXT_BYTES + 1));
        std::fs::write(
            identity_dir.join("seat-d.json"),
            serde_json::to_string(&oversized_title).expect("serialize title tamper"),
        )
        .expect("write title tamper");

        let mut negative_position = record("seat-e", 5, 500);
        negative_position.position_secs = -1.0;
        std::fs::write(
            identity_dir.join("seat-e.json"),
            serde_json::to_string(&negative_position).expect("serialize position tamper"),
        )
        .expect("write position tamper");

        assert_eq!(store.records("matthew").len(), 1);
        assert_eq!(store.owner_seat("matthew").as_deref(), Some("seat-a"));
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
    fn session_record_reader_rejects_invalid_utf8_and_oversized_rows() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let valid = tmp.path().join("valid.json");
        std::fs::write(&valid, "{\"ok\":true}").expect("write valid record");
        assert_eq!(
            read_bounded_session_record(&valid).as_deref(),
            Some("{\"ok\":true}")
        );

        let invalid_utf8 = tmp.path().join("invalid-utf8.json");
        std::fs::write(&invalid_utf8, [0xff, 0xfe, 0xfd]).expect("write invalid UTF-8");
        assert!(read_bounded_session_record(&invalid_utf8).is_none());

        let oversized = tmp.path().join("oversized.json");
        std::fs::write(&oversized, vec![b'x'; MAX_SESSION_RECORD_BYTES + 1])
            .expect("write oversized record");
        assert!(read_bounded_session_record(&oversized).is_none());

        let non_regular = tmp.path().join("directory.json");
        std::fs::create_dir(&non_regular).expect("create directory row");
        assert!(read_bounded_session_record(&non_regular).is_none());
    }

    #[test]
    fn session_record_reader_rejects_a_row_that_grows_after_metadata_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("growing.json");
        std::fs::write(&path, "{}").expect("write initial row");

        let file = fs::OpenOptions::new()
            .read(true)
            .open(&path)
            .expect("open row");
        let before = file.metadata().expect("read initial metadata");
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open append handle")
            .set_len(before.len() + 2)
            .expect("grow row");

        assert!(read_bounded_session_record_file(&file, &before).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn session_record_reader_rejects_final_symlinks_and_special_files() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tmp.path().join("outside.json");
        std::fs::write(&outside, "{\"outside\":true}").expect("write target");

        let symlinked = tmp.path().join("linked.json");
        symlink(&outside, &symlinked).expect("create symlink");
        assert!(read_bounded_session_record(&symlinked).is_none());

        let fifo = tmp.path().join("handoff.json");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("run mkfifo")
                .success(),
            "mkfifo must create the special-file fixture"
        );
        assert!(read_bounded_session_record(&fifo).is_none());
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

    #[test]
    fn login_refuses_an_exhausted_lease_generation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = RoamingStore::new(dir.path().to_path_buf());
        store
            .publish(&record("seat-a", u64::MAX - 1, 1_000))
            .expect("publish final admissible lease");

        let mut session = RoamingSession::new(store.clone(), "matthew", "seat-b");
        let mut p = player();
        assert_eq!(session.login(&mut p, 2_000), LoginOutcome::LeaseUnavailable);
        assert_eq!(session.held_gen(), 0);
        assert_eq!(store.owner_seat("matthew").as_deref(), Some("seat-a"));
        assert_eq!(store.records("matthew").len(), 1);
    }

    #[test]
    fn owner_yield_fails_closed_when_its_record_disappears() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = RoamingStore::new(root.clone());
        let mut session = RoamingSession::new(store.clone(), "matthew", "seat-a");
        let mut p = player();
        assert_eq!(session.login(&mut p, 1_000), LoginOutcome::FreshLease);
        p.load("movie.mkv").expect("load");
        p.pump();
        assert_eq!(p.state(), PlayerState::Playing);

        store.release("matthew", "seat-a").expect("remove lease");
        session.publish(&p, 2_000);
        assert!(
            store.records("matthew").is_empty(),
            "yielded seat cannot reassert"
        );
        assert_eq!(session.poll(&mut p, 3_000), PollOutcome::Released);
        assert_eq!(p.state(), PlayerState::Paused);
        assert_eq!(session.poll(&mut p, 4_000), PollOutcome::Released);
    }

    #[test]
    fn active_owner_yields_when_the_workgroup_root_disappears() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = RoamingStore::new(root.clone());
        let mut session = RoamingSession::new(store, "matthew", "seat-a");
        let mut p = player();
        assert_eq!(session.login(&mut p, 1_000), LoginOutcome::FreshLease);
        p.load("movie.mkv").expect("load");
        p.pump();
        assert_eq!(p.state(), PlayerState::Playing);

        std::fs::remove_dir_all(&root).expect("unmount workgroup root");
        assert_eq!(session.poll(&mut p, 2_000), PollOutcome::Released);
        assert_eq!(p.state(), PlayerState::Paused);
        assert_eq!(session.poll(&mut p, 3_000), PollOutcome::Released);
    }

    #[test]
    fn pending_resume_is_cancelled_after_a_new_owner_yields_this_seat() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();

        let mut pa = player();
        let mut sa = RoamingSession::new(RoamingStore::new(root.clone()), "matthew", "seat-a");
        assert_eq!(sa.login(&mut pa, 1_000), LoginOutcome::FreshLease);
        pa.load("movie.mkv").expect("load");
        pa.pump();
        pa.seek(45.0).expect("seek");
        pa.pause().expect("pause");
        sa.publish(&pa, 2_000);

        let mut pb = player();
        let mut sb = RoamingSession::new(RoamingStore::new(root.clone()), "matthew", "seat-b");
        assert!(matches!(
            sb.login(&mut pb, 3_000),
            LoginOutcome::Resumed { .. }
        ));

        let mut pc = player();
        let mut sc = RoamingSession::new(RoamingStore::new(root), "matthew", "seat-c");
        assert!(matches!(
            sc.login(&mut pc, 4_000),
            LoginOutcome::Resumed { .. }
        ));

        sb.apply_pending(&mut pb);
        assert!(sb.pending.is_none(), "yielded resume must be discarded");
        pb.pump();
        assert_eq!(pb.state(), PlayerState::Paused);
        assert_eq!(
            pb.position(),
            0.0,
            "yielded seat must not seek into resumed media"
        );
    }

    #[test]
    fn pending_resume_is_cancelled_when_the_user_replaces_media() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();

        let mut pa = player();
        let mut sa = RoamingSession::new(RoamingStore::new(root.clone()), "matthew", "seat-a");
        assert_eq!(sa.login(&mut pa, 1_000), LoginOutcome::FreshLease);
        pa.load("movie.mkv").expect("load");
        pa.pump();
        pa.seek(45.0).expect("seek");
        pa.pause().expect("pause");
        sa.publish(&pa, 2_000);

        let mut pb = player();
        let mut sb = RoamingSession::new(RoamingStore::new(root), "matthew", "seat-b");
        assert!(matches!(
            sb.login(&mut pb, 3_000),
            LoginOutcome::Resumed { .. }
        ));
        assert!(sb.pending.is_some());

        pb.load("different.mkv").expect("replace handoff media");
        pb.pump();
        sb.apply_pending(&mut pb);

        assert!(sb.pending.is_none());
        assert_eq!(pb.media(), Some("different.mkv"));
        assert_eq!(pb.position(), 0.0, "stale handoff position must not leak");
    }

    #[test]
    fn failed_handoff_load_does_not_arm_a_resume_seek() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();

        let mut pa = player();
        let mut sa = RoamingSession::new(RoamingStore::new(root.clone()), "matthew", "seat-a");
        assert_eq!(sa.login(&mut pa, 1_000), LoginOutcome::FreshLease);
        pa.playlist_mut().push(PlaylistItem::new("movie.mkv"));
        pa.load("movie.mkv").expect("load");
        pa.pump();
        pa.seek(45.0).expect("seek");
        pa.pause().expect("pause");
        sa.publish(&pa, 2_000);

        let mut pb = Player::new(FakeMpv::new().with_duration(120.0).failing_load());
        let mut sb = RoamingSession::new(RoamingStore::new(root), "matthew", "seat-b");
        assert_eq!(sb.login(&mut pb, 3_000), LoginOutcome::ResumeUnavailable);
        assert!(sb.pending.is_none(), "a rejected load cannot arm a resume");
        assert_eq!(sb.held_gen(), 0, "failed target must not claim a lease");
        assert_eq!(
            sb.store().owner_seat("matthew").as_deref(),
            Some("seat-a"),
            "the source keeps ownership when the target cannot open media"
        );
        assert_eq!(
            pb.media(),
            None,
            "rejected load leaves the target untouched"
        );
        assert!(
            pb.playlist().items().is_empty(),
            "rejected load must not copy the source queue into the target"
        );
    }

    #[test]
    fn failed_lease_publication_cannot_resume_on_non_owner_after_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = RoamingStore::new(root.clone());
        let mut source = record("seat-a", 7, 2_000);
        source.position_secs = 45.0;
        source
            .queue
            .push(PlaylistItem::titled("movie.mkv", "Source queue"));
        store
            .publish(&source)
            .expect("persist source before restart");

        // A durable obstruction at the arriving seat's final pathname makes
        // the atomic rename fail. The old implementation ignored that failure,
        // returned Resumed, and left the non-owner target armed to play.
        std::fs::create_dir(store.seat_path("matthew", "seat-b"))
            .expect("obstruct target lease pathname");
        let mut target = player();
        let mut restarted = RoamingSession::new(store.clone(), "matthew", "seat-b");

        assert_eq!(
            restarted.login(&mut target, 3_000),
            LoginOutcome::LeaseUnavailable
        );
        assert_eq!(restarted.held_gen(), 0);
        assert!(restarted.pending.is_none());
        assert_eq!(store.owner_seat("matthew").as_deref(), Some("seat-a"));
        assert!(
            target.playlist().items().is_empty(),
            "a target without the lease must not adopt the source queue"
        );

        target.pump();
        assert_eq!(
            target.state(),
            PlayerState::Paused,
            "a target without the lease must remain unable to play"
        );
    }

    #[test]
    fn failed_target_seek_stays_paused_and_retries_the_resume() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();

        let mut pa = player();
        let mut sa = RoamingSession::new(RoamingStore::new(root.clone()), "matthew", "seat-a");
        assert_eq!(sa.login(&mut pa, 1_000), LoginOutcome::FreshLease);
        pa.load("movie.mkv").expect("load source");
        pa.pump();
        pa.seek(45.0).expect("seek source");
        pa.pause().expect("pause source");
        sa.publish(&pa, 2_000);

        let mut pb = Player::new(SeekRejectingEngine::new());
        let mut sb = RoamingSession::new(RoamingStore::new(root), "matthew", "seat-b");
        assert!(matches!(
            sb.login(&mut pb, 3_000),
            LoginOutcome::Resumed {
                position_secs: 45.0,
                ..
            }
        ));

        pb.pump();
        sb.apply_pending(&mut pb);
        assert_eq!(pb.state(), PlayerState::Paused);
        assert_eq!(pb.position(), 0.0, "rejected seek must not claim a resume");
        assert!(sb.pending.is_some(), "failed seek must remain retryable");

        pb.engine_mut().allow_seek();
        sb.apply_pending(&mut pb);
        assert_eq!(pb.state(), PlayerState::Paused);
        assert_eq!(pb.position(), 45.0);
        assert!(
            sb.pending.is_none(),
            "successful retry completes the handoff"
        );
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
