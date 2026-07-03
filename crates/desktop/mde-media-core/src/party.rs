//! MEDIA-17: **sync-play party mode** — a shared playback session several seats
//! join, where **play / pause / seek propagate in sync** across every joined seat.
//!
//! Where MEDIA-16 [`roaming`](crate::roaming) makes *one* operator's playback follow
//! them between seats under a **single owned lease** (exactly one seat ever plays),
//! party mode is the mirror image: **several** seats join one shared session and all
//! play **together**, each transport control on any seat propagating to the rest. It
//! reuses the exact same mesh sync seam — one atomic JSON file *per seat* under the
//! Syncthing-replicated workgroup root, folded on read (no new transport, no
//! Syncthing write-conflict) — so nothing is re-derived (§6).
//!
//! # The propagation model (pure + unit-tested)
//!
//! A transport control is a [`SyncCommand`] (`Open` / `Play` / `Pause` / `Seek`). When
//! a seat issues one, it is **projected** onto a [`TransportSnapshot`] (the resulting
//! media / position / playing intent) and stamped into a [`SyncEvent`] carrying a
//! **party-global monotonic** [`SyncEvent::seq`] (the same max-plus-one idiom
//! [`roaming`](crate::roaming)'s lease uses). Every seat folds the per-seat event
//! files, elects the highest-`seq` event as the authoritative **head**
//! ([`resolve_head`]), and — via the pure [`plan_sync`] fold — computes the concrete
//! [`SyncApply`] transport ops that bring its local player into agreement. `plan_sync`
//! is the load-bearing, engine-free core the acceptance asks for: the
//! play/pause/seek → sync-event → apply logic, pure and exhaustively tested below.
//!
//! # §6 / §7 posture — nothing faked
//!
//! Like [`RoamingStore`](crate::roaming::RoamingStore), the whole plane is file I/O
//! against a directory, so it runs unchanged on a headless farm box and is fully
//! unit-tested (including the two-seat sync below, driven against
//! [`FakeMpv`](crate::FakeMpv) with a tempdir root). The one environmental condition
//! is whether the workgroup root is provisioned: a seat with no mesh volume is a
//! silent no-op ([`JoinOutcome::Offline`] / [`PartyPoll::Offline`]) — never a
//! fabricated party and never a write into a bare unprovisioned mount.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use mackes_mesh_types::peers::default_workgroup_root;

use crate::engine::MediaEngine;
use crate::player::{Player, PlayerState};

/// The share subdirectory the per-party, per-seat event files live under
/// (`<root>/media-parties/<party>/<seat>.json`).
pub const PARTY_SUBDIR: &str = "media-parties";

/// The seek tolerance (seconds) [`plan_sync`] applies before re-seeking to converge.
///
/// A propagated seek within this window of the local position is left alone, so a
/// steady stream of position checkpoints does not turn into a seek storm — only a
/// deliberate jump (a real `Seek`, or drift past the window) re-seeks.
pub const SYNC_SEEK_EPSILON_SECS: f64 = 0.75;

// ── the shared transport plane ─────────────────────────────────────────────────

/// A minimal snapshot of a seat's transport — what a party converges on.
///
/// Pure data (no engine), so the [`plan_sync`] fold is testable with no [`Player`]:
/// the loaded `media`, the `position_secs`, and whether it is `playing`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TransportSnapshot {
    /// The loaded media URL/path, if any — the shared "now playing" of the party.
    pub media: Option<String>,
    /// The playback position in seconds.
    pub position_secs: f64,
    /// Whether playback is running (`true`) or paused (`false`).
    pub playing: bool,
}

impl TransportSnapshot {
    /// Capture the live [`Player`]'s transport into a snapshot.
    #[must_use]
    pub fn from_player<E: MediaEngine>(player: &Player<E>) -> Self {
        Self {
            media: player.media().map(ToOwned::to_owned),
            position_secs: player.position(),
            playing: matches!(player.state(), PlayerState::Playing),
        }
    }
}

/// A transport control that propagates across the party.
///
/// This is the *intent* a seat broadcasts; every joined seat applies it via
/// [`plan_sync`]. Serde so it rides inside a [`SyncEvent`] in the per-seat file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SyncCommand {
    /// Open a title for the whole party (everyone loads it and starts from the top).
    Open {
        /// The media URL/path to open on every seat.
        media: String,
        /// The display title, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// Resume playback everywhere.
    Play,
    /// Pause playback everywhere (position held).
    Pause,
    /// Seek every seat to an absolute position.
    Seek {
        /// The absolute position in seconds to seek to.
        position_secs: f64,
    },
}

impl SyncCommand {
    /// Project this command onto `base`, yielding the transport intent that results
    /// once it is applied — the payload a [`SyncEvent`] carries so a lagging or newly
    /// joined seat converges to the *full* state, not just a replayed keystroke.
    ///
    /// `Open` resets to the new title at the top, playing; `Play`/`Pause` flip the
    /// run state; `Seek` moves the position — each leaving the rest of `base` intact.
    #[must_use]
    pub fn project(&self, base: &TransportSnapshot) -> TransportSnapshot {
        match self {
            Self::Open { media, .. } => TransportSnapshot {
                media: Some(media.clone()),
                position_secs: 0.0,
                playing: true,
            },
            Self::Play => TransportSnapshot {
                playing: true,
                ..base.clone()
            },
            Self::Pause => TransportSnapshot {
                playing: false,
                ..base.clone()
            },
            Self::Seek { position_secs } => TransportSnapshot {
                position_secs: *position_secs,
                ..base.clone()
            },
        }
    }
}

/// A concrete transport op [`plan_sync`] emits to converge a local player onto the
/// party head — the interpreted result of the pure fold.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncApply {
    /// Load this media (the party opened a new title).
    Open(String),
    /// Seek to this absolute position (seconds).
    Seek(f64),
    /// Resume playback.
    Play,
    /// Pause playback.
    Pause,
}

/// **The propagation fold** (pure, engine-free): the ordered transport ops that bring
/// `current` into agreement with the party's `target` state.
///
/// This is the load-bearing core of the acceptance — the play/pause/seek → sync
/// logic — with no [`Player`] in sight, so it is exhaustively unit-tested:
///
/// * a different `target.media` ⇒ `Open` it, then `Seek` to its position (when past
///   the start) and `Play`/`Pause` per the target run-state — the newcomer / new-title
///   path;
/// * the same media ⇒ `Seek` only when the positions differ by more than
///   [`SYNC_SEEK_EPSILON_SECS`] (no seek storm), and `Play`/`Pause` only when the run
///   state actually differs.
///
/// An empty result means the seat is already in sync — nothing to do.
#[must_use]
pub fn plan_sync(current: &TransportSnapshot, target: &TransportSnapshot) -> Vec<SyncApply> {
    let mut ops = Vec::new();
    if target.media != current.media {
        let Some(media) = &target.media else {
            // The party has no media loaded — nothing to converge toward.
            return ops;
        };
        ops.push(SyncApply::Open(media.clone()));
        if target.position_secs > SYNC_SEEK_EPSILON_SECS {
            ops.push(SyncApply::Seek(target.position_secs));
        }
        ops.push(if target.playing {
            SyncApply::Play
        } else {
            SyncApply::Pause
        });
        return ops;
    }
    if (target.position_secs - current.position_secs).abs() > SYNC_SEEK_EPSILON_SECS {
        ops.push(SyncApply::Seek(target.position_secs));
    }
    if target.playing != current.playing {
        ops.push(if target.playing {
            SyncApply::Play
        } else {
            SyncApply::Pause
        });
    }
    ops
}

// ── the synced event record ──────────────────────────────────────────────────

/// One propagated transport control — the [`SyncCommand`] a seat issued, stamped with
/// a party-global monotonic [`seq`](Self::seq) and the resulting transport intent.
///
/// The seat is the sole writer of its own file; the party's authoritative state is the
/// highest-`seq` event across every seat's file ([`resolve_head`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncEvent {
    /// The party (room) this event belongs to.
    pub party: String,
    /// The party-global monotonic sequence — the highest across seats is the head.
    pub seq: u64,
    /// The seat that issued the command (the sole writer of its file).
    pub origin_seat: String,
    /// Wall-clock epoch millis the command was issued (freshness + seq tiebreak).
    pub issued_ms: u64,
    /// The transport control that propagated.
    pub command: SyncCommand,
    /// The resulting media (part of the projected intent), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    /// The resulting position in seconds (projected intent).
    pub position_secs: f64,
    /// The resulting run state (projected intent).
    pub playing: bool,
}

impl SyncEvent {
    /// Build an event for `command` at `seq`, projecting its intent onto `base`.
    #[must_use]
    pub fn new(
        party: impl Into<String>,
        seq: u64,
        origin_seat: impl Into<String>,
        issued_ms: u64,
        command: SyncCommand,
        base: &TransportSnapshot,
    ) -> Self {
        let intent = command.project(base);
        Self {
            party: party.into(),
            seq,
            origin_seat: origin_seat.into(),
            issued_ms,
            command,
            media: intent.media,
            position_secs: intent.position_secs,
            playing: intent.playing,
        }
    }

    /// The transport intent this event converges seats onto.
    #[must_use]
    pub fn intent(&self) -> TransportSnapshot {
        TransportSnapshot {
            media: self.media.clone(),
            position_secs: self.position_secs,
            playing: self.playing,
        }
    }
}

/// The authoritative head of `events` — the highest `(seq, issued_ms, origin_seat)`.
///
/// The single-head election: the seat whose event wins is the one the party follows.
/// Deterministic (the `(issued_ms, origin_seat)` tiebreak settles a same-`seq` race,
/// so two simultaneous controls converge — never a permanent split). [`None`] for an
/// empty set.
#[must_use]
pub fn resolve_head(events: &[SyncEvent]) -> Option<&SyncEvent> {
    events.iter().max_by(|a, b| {
        a.seq
            .cmp(&b.seq)
            .then_with(|| a.issued_ms.cmp(&b.issued_ms))
            .then_with(|| a.origin_seat.cmp(&b.origin_seat))
    })
}

/// The next party-global sequence to issue over `events`: one past the current max
/// (so a fresh control always becomes the head). `1` for an empty set.
#[must_use]
pub fn next_seq(events: &[SyncEvent]) -> u64 {
    events
        .iter()
        .map(|e| e.seq)
        .max()
        .map_or(1, |max| max.saturating_add(1))
}

// ── the per-seat synced store ──────────────────────────────────────────────────

/// One seat's row in a party — its membership plus the latest transport event it
/// originated.
///
/// The file `<party>/<seat>.json` IS the row; the seat is its sole writer (the same
/// single-writer-per-file idiom mesh peer records + [`roaming`](crate::roaming) use).
/// Each seat keeps only its *most recent* event; because the sequence is party-global
/// and monotonic, the max across every seat's file is always the true head.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartySeatRecord {
    /// The party (room) this seat is in.
    pub party: String,
    /// This seat's id (the file writer).
    pub seat: String,
    /// Wall-clock epoch millis this seat joined.
    pub joined_ms: u64,
    /// Wall-clock epoch millis of the last refresh (liveness).
    pub updated_ms: u64,
    /// The latest transport event this seat originated, if any (a presence-only seat
    /// that has issued nothing carries [`None`] and never disturbs the head fold).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<SyncEvent>,
}

/// The mesh-synced party store — one JSON file per seat under the workgroup root.
///
/// The same single-writer-per-file idiom [`RoamingStore`](crate::roaming::RoamingStore)
/// uses: Syncthing replicates the files out of band, so there is no new transport.
#[derive(Debug, Clone)]
pub struct PartyStore {
    /// The Syncthing-replicated workgroup root the party files live under.
    root: PathBuf,
}

impl PartyStore {
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
    /// mesh volume is a silent no-op rather than a fabricated party.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.root.is_dir()
    }

    /// The `<root>/media-parties/<party>/` directory.
    fn party_dir(&self, party: &str) -> PathBuf {
        self.root.join(PARTY_SUBDIR).join(sanitize(party))
    }

    /// The `<root>/media-parties/<party>/<seat>.json` path.
    fn seat_path(&self, party: &str, seat: &str) -> PathBuf {
        self.party_dir(party)
            .join(format!("{}.json", sanitize(seat)))
    }

    /// Publish `rec` into this seat's file (atomic temp + rename). A silent no-op when
    /// the root is not provisioned ([`is_ready`](Self::is_ready)).
    ///
    /// # Errors
    /// The [`io::Error`] if the directory cannot be created or the file cannot be
    /// written / renamed.
    pub fn publish(&self, rec: &PartySeatRecord) -> io::Result<()> {
        if !self.is_ready() {
            return Ok(());
        }
        let dir = self.party_dir(&rec.party);
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

    /// Fold every seat file for `party` into a record list (one per seat). Malformed /
    /// half-written / temp files are skipped (never fatal); a missing directory yields
    /// an empty list — exactly like [`read_peers`](mackes_mesh_types::peers::read_peers).
    #[must_use]
    pub fn records(&self, party: &str) -> Vec<PartySeatRecord> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.party_dir(party)) else {
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
                if let Ok(rec) = serde_json::from_str::<PartySeatRecord>(&data) {
                    out.push(rec);
                }
            }
        }
        out.sort_by(|a, b| a.seat.cmp(&b.seat));
        out
    }

    /// The transport events across every seat (the presence-only seats contribute
    /// none).
    #[must_use]
    pub fn events(&self, party: &str) -> Vec<SyncEvent> {
        self.records(party)
            .into_iter()
            .filter_map(|r| r.event)
            .collect()
    }

    /// The authoritative head event for `party`, if any ([`resolve_head`] over the
    /// folded events).
    #[must_use]
    pub fn head(&self, party: &str) -> Option<SyncEvent> {
        resolve_head(&self.events(party)).cloned()
    }

    /// The next party-global sequence to issue for `party` (one past the current max).
    #[must_use]
    pub fn next_seq(&self, party: &str) -> u64 {
        next_seq(&self.events(party))
    }

    /// The seats currently in `party`, sorted (every folded record's seat).
    #[must_use]
    pub fn members(&self, party: &str) -> Vec<String> {
        self.records(party).into_iter().map(|r| r.seat).collect()
    }

    /// Remove this seat's record for `party` (an explicit leave). Absent is not an
    /// error.
    ///
    /// # Errors
    /// The [`io::Error`] if the file exists but cannot be removed.
    pub fn leave(&self, party: &str, seat: &str) -> io::Result<()> {
        match fs::remove_file(self.seat_path(party, seat)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Reduce a party / seat id to a safe single path component (`[A-Za-z0-9_-]`,
/// everything else → `_`; never empty). Mirrors [`crate::roaming`]'s sanitizer.
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

// ── the party session (join / broadcast / poll orchestration) ───────────────────

/// A deferred sync — applied once the arriving seat's engine has an opened title (the
/// propagated position + run-state can't be set until the media is loaded). Mirrors
/// [`roaming`](crate::roaming)'s `PendingResume`.
#[derive(Debug, Clone, Copy)]
struct PendingSync {
    /// The position (seconds) to seek to once loaded.
    position: f64,
    /// Whether the party is playing (so the seat opens playing or paused).
    playing: bool,
}

/// What a [`PartySession::join`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinOutcome {
    /// Joined the party; `synced` is whether an in-progress title was picked up.
    Joined {
        /// Whether a head event was present and this seat synced to it.
        synced: bool,
    },
    /// The workgroup root is not provisioned — party mode is inert (honest offline).
    Offline,
}

/// What a [`PartySession::poll`] observed.
#[derive(Debug, Clone, PartialEq)]
pub enum PartyPoll {
    /// A newer control from another seat was applied here (playback stayed in sync).
    Applied(SyncCommand),
    /// This seat is already in sync with the party head — nothing to apply.
    InSync,
    /// The workgroup root is not provisioned — party mode is inert.
    Offline,
}

/// The per-seat party controller: owns the [`PartyStore`] seam, this seat's latest
/// originated event, the highest sequence it has applied, and any deferred sync.
///
/// The surface drives it: [`join`](Self::join) on entering a party (sync to the head),
/// [`apply_pending`](Self::apply_pending) each pump (land a deferred open-sync once
/// loaded), [`broadcast`](Self::broadcast) after a local transport change (propagate
/// it), and [`poll`](Self::poll) on an interval (apply any newer control from another
/// seat — the play/pause/seek in-sync propagation).
#[derive(Debug)]
pub struct PartySession {
    /// The mesh-synced store seam.
    store: PartyStore,
    /// The party (room) id this seat is in.
    party: String,
    /// This seat's id.
    seat: String,
    /// Whether this seat has joined (a store write happened / would have).
    joined: bool,
    /// The highest sequence this seat has issued or applied (so it never re-applies
    /// its own control or an already-seen one).
    applied_seq: u64,
    /// This seat's latest originated event, re-published on each presence refresh so
    /// it stays in the head fold (a poll that applies another seat's control must not
    /// clobber it).
    last_event: Option<SyncEvent>,
    /// A deferred sync awaiting the engine opening a propagated title.
    pending: Option<PendingSync>,
}

impl PartySession {
    /// A party session over `store` for `party` at `seat`.
    #[must_use]
    pub fn new(store: PartyStore, party: impl Into<String>, seat: impl Into<String>) -> Self {
        Self {
            store,
            party: party.into(),
            seat: seat.into(),
            joined: false,
            applied_seq: 0,
            last_event: None,
            pending: None,
        }
    }

    /// A party session over the canonical workgroup root, with the seat resolved from
    /// the environment ([`resolve_seat`](crate::roaming::resolve_seat)).
    #[must_use]
    pub fn open_default(party: impl Into<String>) -> Self {
        Self::new(
            PartyStore::open_default(),
            party,
            crate::roaming::resolve_seat(),
        )
    }

    /// The party (room) id.
    #[must_use]
    pub fn party(&self) -> &str {
        &self.party
    }

    /// This seat's id.
    #[must_use]
    pub fn seat(&self) -> &str {
        &self.seat
    }

    /// Whether this seat has joined a party.
    #[must_use]
    pub const fn joined(&self) -> bool {
        self.joined
    }

    /// The store seam (read-only) — the surface reads
    /// [`members`](PartyStore::members) / [`head`](PartyStore::head) from it.
    #[must_use]
    pub const fn store(&self) -> &PartyStore {
        &self.store
    }

    /// The seats currently in this party (a live [`PartyStore::members`] fold).
    #[must_use]
    pub fn members(&self) -> Vec<String> {
        self.store.members(&self.party)
    }

    /// Join the party at this seat: publish a presence record and, when a title is
    /// already in progress, sync this seat's player to the party head so a latecomer
    /// picks up mid-watch. A no-op ([`JoinOutcome::Offline`]) when the workgroup root
    /// is not provisioned.
    pub fn join<E: MediaEngine>(&mut self, player: &mut Player<E>, now_ms: u64) -> JoinOutcome {
        if !self.store.is_ready() {
            return JoinOutcome::Offline;
        }
        self.joined = true;
        self.last_event = None;
        let head = self.store.head(&self.party);
        let synced = if let Some(head) = &head {
            self.applied_seq = head.seq;
            self.converge(player, &head.intent());
            true
        } else {
            self.applied_seq = 0;
            false
        };
        self.publish_presence(now_ms);
        JoinOutcome::Joined { synced }
    }

    /// Land a deferred open-sync once the engine has the propagated title open
    /// (`Playing` / `Paused` after `FileLoaded`). Cheap + I/O-free — call every pump,
    /// exactly like [`roaming`](crate::roaming)'s `apply_pending`.
    pub fn apply_pending<E: MediaEngine>(&mut self, player: &mut Player<E>) {
        let Some(pending) = self.pending else {
            return;
        };
        if matches!(player.state(), PlayerState::Playing | PlayerState::Paused) {
            if pending.position > SYNC_SEEK_EPSILON_SECS {
                let _ = player.seek(pending.position);
            }
            if pending.playing {
                let _ = player.play();
            } else {
                let _ = player.pause();
            }
            self.pending = None;
        }
    }

    /// Broadcast a local transport `command` to the party: stamp it at the next
    /// party-global sequence (projecting its intent onto the live player) and publish
    /// it, so every other seat applies it on their next [`poll`](Self::poll). The
    /// local player is assumed already changed — this only announces it. A no-op when
    /// not joined / offline.
    pub fn broadcast<E: MediaEngine>(
        &mut self,
        command: SyncCommand,
        player: &Player<E>,
        now_ms: u64,
    ) {
        if !self.joined || !self.store.is_ready() {
            return;
        }
        let seq = self.store.next_seq(&self.party);
        let base = TransportSnapshot::from_player(player);
        let event = SyncEvent::new(&self.party, seq, &self.seat, now_ms, command, &base);
        self.applied_seq = seq;
        self.last_event = Some(event);
        self.publish_presence(now_ms);
    }

    /// Converge with the shared plane: if another seat has issued a newer control,
    /// apply it to the local player — the play/pause/seek in-sync propagation — and
    /// refresh this seat's presence. A no-op ([`PartyPoll::Offline`]) when not joined /
    /// offline.
    pub fn poll<E: MediaEngine>(&mut self, player: &mut Player<E>, now_ms: u64) -> PartyPoll {
        if !self.joined || !self.store.is_ready() {
            return PartyPoll::Offline;
        }
        let head = self.store.head(&self.party);
        let outcome = match head {
            Some(head) if head.seq > self.applied_seq && head.origin_seat != self.seat => {
                self.applied_seq = head.seq;
                self.converge(player, &head.intent());
                PartyPoll::Applied(head.command)
            }
            _ => PartyPoll::InSync,
        };
        self.publish_presence(now_ms);
        outcome
    }

    /// Leave the party — remove this seat's record. Absent is not an error.
    ///
    /// # Errors
    /// The [`io::Error`] if the file exists but cannot be removed.
    pub fn leave(&mut self) -> io::Result<()> {
        self.joined = false;
        self.last_event = None;
        self.pending = None;
        self.store.leave(&self.party, &self.seat)
    }

    /// Drive the local player toward `target` via the pure [`plan_sync`] fold. A
    /// propagated `Open` defers its seek + run-state to [`apply_pending`](Self::apply_pending)
    /// (the title must load first), exactly like [`roaming`](crate::roaming)'s resume.
    fn converge<E: MediaEngine>(&mut self, player: &mut Player<E>, target: &TransportSnapshot) {
        let plan = plan_sync(&TransportSnapshot::from_player(player), target);
        if let Some(SyncApply::Open(media)) = plan.first() {
            let _ = player.load(media.clone());
            self.pending = Some(PendingSync {
                position: target.position_secs,
                playing: target.playing,
            });
            return;
        }
        for op in plan {
            let _ = match op {
                SyncApply::Seek(pos) => player.seek(pos),
                SyncApply::Play => player.play(),
                SyncApply::Pause => player.pause(),
                // The Open case is handled above (it is always the first op).
                SyncApply::Open(_) => Ok(()),
            };
        }
    }

    /// Re-publish this seat's presence + its latest originated event (freshening
    /// `updated_ms`), keeping it in the head fold without clobbering another seat's
    /// control.
    fn publish_presence(&self, now_ms: u64) {
        let rec = PartySeatRecord {
            party: self.party.clone(),
            seat: self.seat.clone(),
            joined_ms: now_ms,
            updated_ms: now_ms,
            event: self.last_event.clone(),
        };
        let _ = self.store.publish(&rec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeMpv;

    fn player() -> Player<FakeMpv> {
        Player::new(FakeMpv::new().with_duration(120.0))
    }

    fn snapshot(media: Option<&str>, pos: f64, playing: bool) -> TransportSnapshot {
        TransportSnapshot {
            media: media.map(ToOwned::to_owned),
            position_secs: pos,
            playing,
        }
    }

    // ── the pure propagation fold ───────────────────────────────────────────────

    #[test]
    fn project_maps_each_command_to_its_intent() {
        let base = snapshot(Some("movie.mkv"), 30.0, true);
        assert_eq!(
            SyncCommand::Pause.project(&base),
            snapshot(Some("movie.mkv"), 30.0, false)
        );
        assert_eq!(
            SyncCommand::Play.project(&snapshot(Some("m"), 5.0, false)),
            snapshot(Some("m"), 5.0, true)
        );
        assert_eq!(
            SyncCommand::Seek {
                position_secs: 88.0
            }
            .project(&base),
            snapshot(Some("movie.mkv"), 88.0, true)
        );
        assert_eq!(
            SyncCommand::Open {
                media: "next.mkv".to_owned(),
                title: None,
            }
            .project(&base),
            snapshot(Some("next.mkv"), 0.0, true)
        );
    }

    #[test]
    fn plan_sync_is_empty_when_already_in_agreement() {
        let s = snapshot(Some("movie.mkv"), 30.0, true);
        assert!(plan_sync(&s, &s).is_empty());
        // A sub-epsilon position drift does not re-seek.
        let drifted = snapshot(Some("movie.mkv"), 30.0 + SYNC_SEEK_EPSILON_SECS / 2.0, true);
        assert!(plan_sync(&drifted, &s).is_empty());
    }

    #[test]
    fn plan_sync_propagates_pause_play_and_seek_on_the_same_media() {
        let playing = snapshot(Some("movie.mkv"), 30.0, true);
        // A pause propagates as a single Pause.
        assert_eq!(
            plan_sync(&playing, &snapshot(Some("movie.mkv"), 30.0, false)),
            vec![SyncApply::Pause]
        );
        // A play propagates as a single Play.
        let paused = snapshot(Some("movie.mkv"), 30.0, false);
        assert_eq!(
            plan_sync(&paused, &snapshot(Some("movie.mkv"), 30.0, true)),
            vec![SyncApply::Play]
        );
        // A deliberate seek propagates (past the epsilon).
        assert_eq!(
            plan_sync(&playing, &snapshot(Some("movie.mkv"), 75.0, true)),
            vec![SyncApply::Seek(75.0)]
        );
        // A seek that also flips the run state emits both, seek first.
        assert_eq!(
            plan_sync(&playing, &snapshot(Some("movie.mkv"), 75.0, false)),
            vec![SyncApply::Seek(75.0), SyncApply::Pause]
        );
    }

    #[test]
    fn plan_sync_opens_a_new_title_then_seeks_and_sets_run_state() {
        let current = snapshot(Some("movie.mkv"), 30.0, true);
        // Opening a fresh title mid-watch: Open, Seek (past start), then Play/Pause.
        assert_eq!(
            plan_sync(&current, &snapshot(Some("other.mkv"), 12.0, false)),
            vec![
                SyncApply::Open("other.mkv".to_owned()),
                SyncApply::Seek(12.0),
                SyncApply::Pause,
            ]
        );
        // From nothing loaded, a title at the top: Open + Play, no needless seek.
        assert_eq!(
            plan_sync(
                &snapshot(None, 0.0, false),
                &snapshot(Some("other.mkv"), 0.0, true)
            ),
            vec![SyncApply::Open("other.mkv".to_owned()), SyncApply::Play]
        );
    }

    // ── the head election ───────────────────────────────────────────────────────

    fn event(seat: &str, seq: u64, issued: u64) -> SyncEvent {
        SyncEvent::new(
            "movie-night",
            seq,
            seat,
            issued,
            SyncCommand::Pause,
            &snapshot(Some("movie.mkv"), 30.0, true),
        )
    }

    #[test]
    fn head_is_the_highest_sequence_and_next_is_one_past() {
        let events = vec![event("a", 1, 100), event("b", 3, 100), event("c", 2, 500)];
        assert_eq!(resolve_head(&events).expect("head").origin_seat, "b");
        assert_eq!(next_seq(&events), 4);
        assert_eq!(next_seq(&[]), 1);
        assert!(resolve_head(&[]).is_none());
    }

    #[test]
    fn same_sequence_race_resolves_deterministically() {
        // Two seats both wrote seq 2 — the (issued_ms, origin_seat) tiebreak settles a
        // single head, so every seat converges (no permanent split).
        let events = vec![event("seat-a", 2, 200), event("seat-b", 2, 500)];
        assert_eq!(resolve_head(&events).expect("head").origin_seat, "seat-b");
        let tie = vec![event("seat-a", 2, 200), event("seat-b", 2, 200)];
        assert_eq!(resolve_head(&tie).expect("head").origin_seat, "seat-b");
    }

    // ── the store round-trips + skips corruption ───────────────────────────────

    #[test]
    fn store_folds_events_across_seats_and_skips_corruption() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PartyStore::new(dir.path().to_path_buf());
        let rec = |seat: &str, seq: u64| PartySeatRecord {
            party: "movie-night".to_owned(),
            seat: seat.to_owned(),
            joined_ms: 1,
            updated_ms: 1,
            event: Some(event(seat, seq, seq * 10)),
        };
        store.publish(&rec("seat-a", 1)).expect("publish a");
        store.publish(&rec("seat-b", 2)).expect("publish b");
        assert_eq!(store.members("movie-night"), vec!["seat-a", "seat-b"]);
        assert_eq!(
            store.head("movie-night").expect("head").origin_seat,
            "seat-b"
        );
        assert_eq!(store.next_seq("movie-night"), 3);

        // A corrupt file is skipped, not fatal.
        let corrupt = dir
            .path()
            .join(PARTY_SUBDIR)
            .join("movie-night")
            .join("seat-c.json");
        std::fs::write(&corrupt, "{ not json").expect("write corrupt");
        assert_eq!(
            store.members("movie-night").len(),
            2,
            "corrupt file skipped"
        );

        // Leave removes only this seat's file.
        store.leave("movie-night", "seat-a").expect("leave");
        assert_eq!(store.members("movie-night"), vec!["seat-b"]);
    }

    #[test]
    fn store_is_inert_when_the_root_is_unprovisioned() {
        let store = PartyStore::new(PathBuf::from("/no/such/mesh/root"));
        assert!(!store.is_ready());
        let rec = PartySeatRecord {
            party: "p".to_owned(),
            seat: "s".to_owned(),
            joined_ms: 0,
            updated_ms: 0,
            event: None,
        };
        store.publish(&rec).expect("no-op publish");
        assert!(store.members("p").is_empty());
        assert!(store.head("p").is_none());
    }

    #[test]
    fn join_is_offline_when_root_missing() {
        let mut session = PartySession::new(
            PartyStore::new(PathBuf::from("/no/such/mesh/root")),
            "p",
            "seat-a",
        );
        let mut p = player();
        assert_eq!(session.join(&mut p, 1_000), JoinOutcome::Offline);
        assert!(!session.joined());
        assert_eq!(session.poll(&mut p, 2_000), PartyPoll::Offline);
    }

    // ── THE CRUX: several seats watch together, transport propagates in sync ────

    #[test]
    fn two_seats_watch_together_with_play_pause_seek_in_sync() {
        // One shared workgroup root = the Syncthing-replicated dir both seats see.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();

        // ── Seat A hosts: joins, opens a title, and starts it playing. ──
        let mut pa = player();
        let mut sa = PartySession::new(PartyStore::new(root.clone()), "movie-night", "seat-a");
        assert_eq!(
            sa.join(&mut pa, 1_000),
            JoinOutcome::Joined { synced: false }
        );
        pa.load("movie.mkv").expect("load");
        pa.pump(); // → Playing
        sa.broadcast(
            SyncCommand::Open {
                media: "movie.mkv".to_owned(),
                title: Some("The Movie".to_owned()),
            },
            &pa,
            1_100,
        );

        // ── Seat B joins mid-watch: it syncs to the in-progress title. ──
        let mut pb = player();
        let mut sb = PartySession::new(PartyStore::new(root), "movie-night", "seat-b");
        assert_eq!(
            sb.join(&mut pb, 2_000),
            JoinOutcome::Joined { synced: true }
        );
        pb.pump(); // Loading → Playing
        sb.apply_pending(&mut pb);
        assert_eq!(pb.media(), Some("movie.mkv"), "B opened the party title");
        assert_eq!(pb.state(), PlayerState::Playing, "B is playing along");
        // Both seats are members.
        assert_eq!(sa.members(), vec!["seat-a", "seat-b"]);

        // ── Seat A pauses + seeks; B stays in sync on its next poll. ──
        pa.seek(45.0).expect("seek");
        sa.broadcast(
            SyncCommand::Seek {
                position_secs: 45.0,
            },
            &pa,
            3_000,
        );
        pa.pause().expect("pause");
        sa.broadcast(SyncCommand::Pause, &pa, 3_100);

        assert!(
            matches!(
                sb.poll(&mut pb, 3_200),
                PartyPoll::Applied(SyncCommand::Pause)
            ),
            "B should have applied A's pause"
        );
        assert_eq!(pb.state(), PlayerState::Paused, "B paused in sync with A");
        assert!(
            (pb.position() - 45.0).abs() < 1.0,
            "B seeked in sync with A (pos {})",
            pb.position()
        );

        // ── Seat B resumes; A picks it up (control flows both ways). ──
        pb.play().expect("B resumes");
        sb.broadcast(SyncCommand::Play, &pb, 4_000);
        assert!(
            matches!(
                sa.poll(&mut pa, 4_100),
                PartyPoll::Applied(SyncCommand::Play)
            ),
            "A should have applied B's play"
        );
        assert_eq!(pa.state(), PlayerState::Playing, "A resumed in sync with B");

        // A seat that originated the head sees nothing new to apply.
        assert_eq!(sb.poll(&mut pb, 4_200), PartyPoll::InSync);
    }

    #[test]
    fn a_seat_never_reapplies_its_own_control() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut pa = player();
        let mut sa = PartySession::new(
            PartyStore::new(dir.path().to_path_buf()),
            "movie-night",
            "seat-a",
        );
        sa.join(&mut pa, 1_000);
        pa.load("movie.mkv").expect("load");
        pa.pump();
        pa.pause().expect("pause");
        sa.broadcast(SyncCommand::Pause, &pa, 2_000);
        // Its own event is the head, but poll must not re-apply it (origin == self).
        assert_eq!(sa.poll(&mut pa, 2_100), PartyPoll::InSync);
    }
}
