//! CHOOSER-9 — the operator's Desktop Chooser preferences that FOLLOW the mesh
//! identity between seats: pinned **favorites**, recently-used **recents**, and
//! **manual** (operator-added) sources.
//!
//! The transport is the MEDIA-16 roaming seam reused verbatim in shape: one JSON
//! file *per seat* under the Syncthing-replicated workgroup root
//! (`<root>/chooser-prefs/<identity>/<seat>.json`), written atomically (temp +
//! rename) and folded back by unioning every seat file — the single-writer-per-
//! file idiom [`mackes_mesh_types::peers::write_peer_record`] /
//! [`read_peers`](mackes_mesh_types::peers::read_peers) use, so Syncthing never
//! sees a write conflict and needs **no new transport**. The root is the canonical
//! [`mackes_mesh_types::peers::default_workgroup_root`], never a hardcoded
//! `/mnt/mesh-storage` (which would reintroduce the documented split-brain).
//!
//! # Merge, not a single lease
//!
//! Unlike the roaming session's single owned lease, prefs **merge**: every seat's
//! record contributes. Each favorite / manual entry is a last-writer-wins register
//! keyed by id (a monotonic `updated_ms`, `seat` tiebreak) so a pin OR an un-pin on
//! any seat converges — an un-pin is a `pinned: false` tombstone, a remove a
//! `present: false` one, never a silent grow-only set that can't shrink fleet-wide.
//! Recents merge by the newest `used_ms` per id, sorted most-recent-first and
//! capped. The whole model is pure directory I/O, so it runs unchanged on a
//! headless farm box and is fully unit-tested (including the two-seat pin below).
//!
//! # Honest offline
//!
//! A seat with no mesh volume ([`ChooserPrefsStore::is_ready`] false) is a silent
//! no-op: writes are dropped, reads fold nothing from disk — but the session's own
//! in-memory record is always folded, so local pins still work session-scoped, and
//! nothing is faked into an unprovisioned mount.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use mackes_mesh_types::peers::default_workgroup_root;

/// The share subdirectory the per-identity prefs records live under
/// (`<root>/chooser-prefs/<identity>/<seat>.json`).
pub const PREFS_SUBDIR: &str = "chooser-prefs";

/// The most-recent desktops kept in the synced recents list — a modest cap (the
/// recents are a convenience, not an archive) so the record never grows without
/// bound; older uses fall off the tail.
const RECENTS_CAP: usize = 24;

// ── the synced entry types ───────────────────────────────────────────────────

/// A favorite/pin as a last-writer-wins register: `pinned` false is the un-pin
/// tombstone, so a toggle on any seat converges (never a grow-only set).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoriteEntry {
    /// The desktop source id this pin is keyed on.
    pub id: String,
    /// Whether the source is currently pinned (false = un-pinned tombstone).
    pub pinned: bool,
    /// Wall-clock epoch millis of the last flip (the LWW ordering key).
    pub updated_ms: u64,
}

/// A recently-used desktop, keyed by source id and carrying its display name so
/// the recents list reads even before the roster reloads that id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEntry {
    /// The desktop source id.
    pub id: String,
    /// The display name captured at use time.
    pub name: String,
    /// Wall-clock epoch millis of the last use (the recency ordering key).
    pub used_ms: u64,
}

/// A manual (operator-added) desktop endpoint as a last-writer-wins register:
/// `present` false is the remove tombstone, so a remove on any seat converges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualEntry {
    /// The `manual:<host>:<port>:<proto>` id.
    pub id: String,
    /// Whether the source is currently present (false = removed tombstone).
    pub present: bool,
    /// The host/IP to dial.
    pub host: String,
    /// The port to dial.
    pub port: u16,
    /// The protocol wire tag (`rdp` / `vnc` / `spice`).
    pub protocol: String,
    /// The operator's display name, or `None` (defaults to `host:port` worker-side).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Wall-clock epoch millis of the last add/edit/remove (the LWW ordering key).
    pub updated_ms: u64,
}

/// One seat's contribution to the operator's prefs — the single record it writes
/// to its own file, bound to the mesh `identity`. Additive per-id registers, so
/// folding every seat's record converges.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SeatPrefs {
    /// The mesh identity these prefs belong to — the roaming key.
    pub identity: String,
    /// The seat that wrote this record (the single writer of its own file).
    pub seat: String,
    /// Wall-clock epoch millis of the last write (freshness only; per-entry
    /// registers carry their own ordering key).
    pub updated_ms: u64,
    /// The favorite registers this seat has touched.
    #[serde(default)]
    pub favorites: Vec<FavoriteEntry>,
    /// The recently-used desktops this seat has recorded.
    #[serde(default)]
    pub recents: Vec<RecentEntry>,
    /// The manual-source registers this seat has touched.
    #[serde(default)]
    pub manual: Vec<ManualEntry>,
}

/// The folded, merged view across every seat — what the surface renders.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergedPrefs {
    /// The pinned source ids (winning register per id is `pinned`).
    pub favorites: HashSet<String>,
    /// The recently-used desktops, most-recent-first, capped.
    pub recents: Vec<RecentEntry>,
    /// The present manual sources (winning register per id is `present`).
    pub manual: Vec<ManualEntry>,
}

// ── the pure merge folds (unit-tested without any I/O) ───────────────────────

/// The merged favorite set: the last-writer-wins register per id
/// (`(updated_ms, seat)`), kept only when the winner is `pinned`.
#[must_use]
pub fn merge_favorites(records: &[SeatPrefs]) -> HashSet<String> {
    let mut best: HashMap<&str, (&FavoriteEntry, &str)> = HashMap::new();
    for rec in records {
        for entry in &rec.favorites {
            let take = best.get(entry.id.as_str()).is_none_or(|(cur, cur_seat)| {
                (entry.updated_ms, rec.seat.as_str()) > (cur.updated_ms, *cur_seat)
            });
            if take {
                best.insert(entry.id.as_str(), (entry, rec.seat.as_str()));
            }
        }
    }
    best.into_iter()
        .filter(|(_, (entry, _))| entry.pinned)
        .map(|(id, _)| id.to_owned())
        .collect()
}

/// The merged manual sources: the last-writer-wins register per id, kept only when
/// the winner is `present`. Sorted by id for a stable render.
#[must_use]
pub fn merge_manual(records: &[SeatPrefs]) -> Vec<ManualEntry> {
    let mut best: HashMap<&str, (&ManualEntry, &str)> = HashMap::new();
    for rec in records {
        for entry in &rec.manual {
            let take = best.get(entry.id.as_str()).is_none_or(|(cur, cur_seat)| {
                (entry.updated_ms, rec.seat.as_str()) > (cur.updated_ms, *cur_seat)
            });
            if take {
                best.insert(entry.id.as_str(), (entry, rec.seat.as_str()));
            }
        }
    }
    let mut out: Vec<ManualEntry> = best
        .into_values()
        .filter(|(entry, _)| entry.present)
        .map(|(entry, _)| entry.clone())
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// The merged recents: the newest `used_ms` per id, most-recent-first (id
/// tiebreak), capped to [`RECENTS_CAP`].
#[must_use]
pub fn merge_recents(records: &[SeatPrefs]) -> Vec<RecentEntry> {
    let mut best: HashMap<&str, &RecentEntry> = HashMap::new();
    for rec in records {
        for entry in &rec.recents {
            let take = best
                .get(entry.id.as_str())
                .is_none_or(|cur| entry.used_ms > cur.used_ms);
            if take {
                best.insert(entry.id.as_str(), entry);
            }
        }
    }
    let mut out: Vec<RecentEntry> = best.into_values().cloned().collect();
    out.sort_by(|a, b| b.used_ms.cmp(&a.used_ms).then_with(|| a.id.cmp(&b.id)));
    out.truncate(RECENTS_CAP);
    out
}

// ── the per-seat synced store ────────────────────────────────────────────────

/// The mesh-synced prefs store — one JSON file per seat under the workgroup root,
/// the same single-writer-per-file idiom mesh peer records + the media-session
/// roaming store use.
#[derive(Debug, Clone)]
pub struct ChooserPrefsStore {
    /// The Syncthing-replicated workgroup root the prefs files live under.
    root: PathBuf,
}

impl ChooserPrefsStore {
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

    /// Whether the workgroup root is actually present. The store writes only under
    /// an existing root — never creating a bare unprovisioned mount — so a seat with
    /// no mesh volume is a silent no-op rather than a fabricated synced record.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.root.is_dir()
    }

    /// The `<root>/chooser-prefs/<identity>/` directory.
    fn identity_dir(&self, identity: &str) -> PathBuf {
        self.root.join(PREFS_SUBDIR).join(sanitize(identity))
    }

    /// The `<root>/chooser-prefs/<identity>/<seat>.json` path.
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
    pub fn publish(&self, rec: &SeatPrefs) -> io::Result<()> {
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
    pub fn records(&self, identity: &str) -> Vec<SeatPrefs> {
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
                if let Ok(rec) = serde_json::from_str::<SeatPrefs>(&data) {
                    out.push(rec);
                }
            }
        }
        out.sort_by(|a, b| a.seat.cmp(&b.seat));
        out
    }

    /// Read only `seat`'s own persisted record for `identity`, if any (used to
    /// resume a seat's tombstones on start-up).
    #[must_use]
    fn seat_record(&self, identity: &str, seat: &str) -> Option<SeatPrefs> {
        let data = fs::read_to_string(self.seat_path(identity, seat)).ok()?;
        serde_json::from_str::<SeatPrefs>(&data).ok()
    }
}

// ── the per-seat prefs session (mutate this seat, merge every seat) ───────────

/// The per-seat prefs controller: owns the [`ChooserPrefsStore`] seam, the mesh
/// `identity` these prefs roam under, this `seat`'s id, and this seat's in-memory
/// record (loaded on open so prior tombstones survive a restart).
///
/// The surface drives it: [`toggle_favorite`](Self::toggle_favorite) on a pin,
/// [`record_recent`](Self::record_recent) on a connect, [`set_manual`](Self::set_manual)
/// / [`remove_manual`](Self::remove_manual) on a manual add/edit/remove, and
/// [`merged`](Self::merged) to render the folded view every refresh.
#[derive(Debug)]
pub struct ChooserPrefs {
    /// The mesh-synced store seam.
    store: ChooserPrefsStore,
    /// The mesh identity these prefs roam under.
    identity: String,
    /// This seat's id (the per-file writer).
    seat: String,
    /// This seat's own record — the single file it writes, mutated in place.
    local: SeatPrefs,
}

impl ChooserPrefs {
    /// A prefs session over `store` for `identity` at `seat`, resuming this seat's
    /// persisted record (its tombstones) if one is present.
    #[must_use]
    pub fn new(
        store: ChooserPrefsStore,
        identity: impl Into<String>,
        seat: impl Into<String>,
    ) -> Self {
        let identity = identity.into();
        let seat = seat.into();
        let local = store
            .seat_record(&identity, &seat)
            .unwrap_or_else(|| SeatPrefs {
                identity: identity.clone(),
                seat: seat.clone(),
                ..SeatPrefs::default()
            });
        Self {
            store,
            identity,
            seat,
            local,
        }
    }

    /// A prefs session over the canonical workgroup root, with the mesh identity +
    /// seat resolved from the environment ([`resolve_identity`] / [`resolve_seat`]).
    #[must_use]
    pub fn open_default() -> Self {
        Self::new(
            ChooserPrefsStore::open_default(),
            resolve_identity(),
            resolve_seat(),
        )
    }

    /// Whether the workgroup root is provisioned — when false the store is inert and
    /// prefs stay session-local (honest offline).
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.store.is_ready()
    }

    /// Fold the store's seat files with this seat's live in-memory record (which is
    /// authoritative for its own seat, so an unpersisted local edit still counts —
    /// the offline path).
    fn records(&self) -> Vec<SeatPrefs> {
        let mut recs = self.store.records(&self.identity);
        match recs.iter_mut().find(|r| r.seat == self.seat) {
            Some(slot) => slot.clone_from(&self.local),
            None => recs.push(self.local.clone()),
        }
        recs
    }

    /// The merged view across every seat + this one — the render source of truth.
    #[must_use]
    pub fn merged(&self) -> MergedPrefs {
        let recs = self.records();
        MergedPrefs {
            favorites: merge_favorites(&recs),
            recents: merge_recents(&recs),
            manual: merge_manual(&recs),
        }
    }

    /// Whether the merged view currently pins `id`.
    #[must_use]
    pub fn is_favorite(&self, id: &str) -> bool {
        self.merged().favorites.contains(id)
    }

    /// Whether ANY seat's record carries `id` at all — present OR tombstoned. Used
    /// to gate the roster capture so a removed manual source (a lingering tombstone)
    /// is never resurrected by a seat whose roster still shows it; only genuinely
    /// new endpoints are captured.
    #[must_use]
    pub fn knows_manual(&self, id: &str) -> bool {
        self.records()
            .iter()
            .any(|r| r.manual.iter().any(|m| m.id == id))
    }

    /// Persist this seat's record (a no-op when the store is inert).
    fn persist(&mut self, now_ms: u64) {
        self.local.updated_ms = now_ms;
        let _ = self.store.publish(&self.local);
    }

    /// Set `id`'s pin register to `pinned`, stamped `now_ms`, and persist.
    pub fn set_favorite(&mut self, id: &str, pinned: bool, now_ms: u64) {
        match self.local.favorites.iter_mut().find(|e| e.id == id) {
            Some(entry) => {
                entry.pinned = pinned;
                entry.updated_ms = now_ms;
            }
            None => self.local.favorites.push(FavoriteEntry {
                id: id.to_owned(),
                pinned,
                updated_ms: now_ms,
            }),
        }
        self.persist(now_ms);
    }

    /// Flip `id`'s pin relative to the current MERGED state (so a cross-seat pin is
    /// respected), persist, and return the new pinned state.
    pub fn toggle_favorite(&mut self, id: &str, now_ms: u64) -> bool {
        let next = !self.is_favorite(id);
        self.set_favorite(id, next, now_ms);
        next
    }

    /// Record `id` (`name`) as used at `now_ms`, trimming to the newest
    /// [`RECENTS_CAP`], and persist.
    pub fn record_recent(&mut self, id: &str, name: &str, now_ms: u64) {
        match self.local.recents.iter_mut().find(|e| e.id == id) {
            Some(entry) => {
                name.clone_into(&mut entry.name);
                entry.used_ms = now_ms;
            }
            None => self.local.recents.push(RecentEntry {
                id: id.to_owned(),
                name: name.to_owned(),
                used_ms: now_ms,
            }),
        }
        self.local
            .recents
            .sort_by(|a, b| b.used_ms.cmp(&a.used_ms).then_with(|| a.id.cmp(&b.id)));
        self.local.recents.truncate(RECENTS_CAP);
        self.persist(now_ms);
    }

    /// Set `id`'s manual register present with its endpoint fields, stamped
    /// `now_ms`, and persist.
    pub fn set_manual(
        &mut self,
        id: &str,
        host: &str,
        port: u16,
        protocol: &str,
        name: Option<String>,
        now_ms: u64,
    ) {
        match self.local.manual.iter_mut().find(|e| e.id == id) {
            Some(entry) => {
                entry.present = true;
                host.clone_into(&mut entry.host);
                entry.port = port;
                protocol.clone_into(&mut entry.protocol);
                entry.name = name;
                entry.updated_ms = now_ms;
            }
            None => self.local.manual.push(ManualEntry {
                id: id.to_owned(),
                present: true,
                host: host.to_owned(),
                port,
                protocol: protocol.to_owned(),
                name,
                updated_ms: now_ms,
            }),
        }
        self.persist(now_ms);
    }

    /// Tombstone `id`'s manual register (`present: false`), stamped `now_ms`, and
    /// persist — so the remove converges across seats.
    pub fn remove_manual(&mut self, id: &str, now_ms: u64) {
        if let Some(entry) = self.local.manual.iter_mut().find(|e| e.id == id) {
            entry.present = false;
            entry.updated_ms = now_ms;
        } else {
            self.local.manual.push(ManualEntry {
                id: id.to_owned(),
                present: false,
                host: String::new(),
                port: 0,
                protocol: String::new(),
                name: None,
                updated_ms: now_ms,
            });
        }
        self.persist(now_ms);
    }
}

// ── environment resolution (mirrors the roaming session + bookmarks worker) ──

/// Reduce an identity / seat id to a safe single path component
/// (`[A-Za-z0-9_-]`, everything else → `_`; never empty).
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

/// Resolve the mesh identity the prefs roam under: `$MDE_MESH_USER` → `$USER` /
/// `$LOGNAME` → a stable `operator` fallback (the same precedence MEDIA-16 roaming
/// + the mesh bookmarks worker use, so every identity-bound record agrees).
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

/// Wall-clock epoch millis — the record timestamp / register ordering key the
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

    fn seat(id: &str) -> SeatPrefs {
        SeatPrefs {
            identity: "matthew".to_owned(),
            seat: id.to_owned(),
            ..SeatPrefs::default()
        }
    }

    // ── the pure merge folds ──

    #[test]
    fn favorites_merge_lww_so_an_unpin_beats_an_older_pin() {
        // seat-a pinned at t=100; seat-b un-pinned the SAME id at t=200 → the newer
        // register wins, so the merged set drops it (a grow-only union could not).
        let mut a = seat("seat-a");
        a.favorites.push(FavoriteEntry {
            id: "peer:oak".to_owned(),
            pinned: true,
            updated_ms: 100,
        });
        let mut b = seat("seat-b");
        b.favorites.push(FavoriteEntry {
            id: "peer:oak".to_owned(),
            pinned: false,
            updated_ms: 200,
        });
        assert!(
            merge_favorites(&[a.clone(), b.clone()]).is_empty(),
            "the newer un-pin wins"
        );
        // Flip the timestamps: now the pin is newer and survives.
        a.favorites[0].updated_ms = 300;
        assert_eq!(
            merge_favorites(&[a, b]),
            std::iter::once("peer:oak".to_owned()).collect()
        );
    }

    #[test]
    fn favorites_from_different_seats_union_when_distinct() {
        let mut a = seat("seat-a");
        a.favorites.push(FavoriteEntry {
            id: "peer:oak".to_owned(),
            pinned: true,
            updated_ms: 1,
        });
        let mut b = seat("seat-b");
        b.favorites.push(FavoriteEntry {
            id: "vm:elm:dev".to_owned(),
            pinned: true,
            updated_ms: 1,
        });
        let merged = merge_favorites(&[a, b]);
        assert_eq!(merged.len(), 2, "distinct pins from two seats both survive");
    }

    #[test]
    fn manual_merge_drops_a_tombstoned_id() {
        let mut a = seat("seat-a");
        a.manual.push(ManualEntry {
            id: "manual:h:1:rdp".to_owned(),
            present: true,
            host: "h".to_owned(),
            port: 1,
            protocol: "rdp".to_owned(),
            name: None,
            updated_ms: 10,
        });
        // Present alone → one source.
        assert_eq!(merge_manual(&[a.clone()]).len(), 1);
        // A newer remove on another seat tombstones it.
        let mut b = seat("seat-b");
        b.manual.push(ManualEntry {
            id: "manual:h:1:rdp".to_owned(),
            present: false,
            host: "h".to_owned(),
            port: 1,
            protocol: "rdp".to_owned(),
            name: None,
            updated_ms: 20,
        });
        assert!(merge_manual(&[a, b]).is_empty(), "the newer remove wins");
    }

    #[test]
    fn recents_merge_newest_per_id_capped_and_ordered() {
        let mut a = seat("seat-a");
        a.recents.push(RecentEntry {
            id: "peer:oak".to_owned(),
            name: "oak".to_owned(),
            used_ms: 100,
        });
        let mut b = seat("seat-b");
        b.recents.push(RecentEntry {
            id: "peer:oak".to_owned(),
            name: "oak".to_owned(),
            used_ms: 500,
        });
        b.recents.push(RecentEntry {
            id: "vm:elm:dev".to_owned(),
            name: "dev".to_owned(),
            used_ms: 300,
        });
        let merged = merge_recents(&[a, b]);
        assert_eq!(merged.len(), 2, "one entry per id");
        assert_eq!(merged[0].id, "peer:oak", "most-recent-first");
        assert_eq!(merged[0].used_ms, 500, "the newer use wins");
        assert_eq!(merged[1].id, "vm:elm:dev");
    }

    // ── the store round-trip ──

    fn temp_root(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("mde-chooser9-{tag}-{n}"))
    }

    #[test]
    fn store_round_trips_and_folds_across_seats() {
        let dir = temp_root("store");
        std::fs::create_dir_all(&dir).expect("mkroot");
        let store = ChooserPrefsStore::new(dir.clone());
        assert!(store.is_ready());

        let mut a = seat("seat-a");
        a.favorites.push(FavoriteEntry {
            id: "peer:oak".to_owned(),
            pinned: true,
            updated_ms: 1,
        });
        store.publish(&a).expect("publish a");
        let mut b = seat("seat-b");
        b.favorites.push(FavoriteEntry {
            id: "vm:elm:dev".to_owned(),
            pinned: true,
            updated_ms: 1,
        });
        store.publish(&b).expect("publish b");

        let recs = store.records("matthew");
        assert_eq!(recs.len(), 2, "one file per seat, folded");
        assert_eq!(merge_favorites(&recs).len(), 2);

        // A corrupt file is skipped, never fatal.
        let corrupt = dir.join(PREFS_SUBDIR).join("matthew").join("seat-c.json");
        std::fs::write(&corrupt, "{ not json").expect("write corrupt");
        assert_eq!(store.records("matthew").len(), 2, "corrupt skipped");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_is_inert_when_the_root_is_unprovisioned() {
        let store = ChooserPrefsStore::new(PathBuf::from("/no/such/mesh/root"));
        assert!(!store.is_ready());
        store.publish(&seat("seat-a")).expect("no-op publish");
        assert!(store.records("matthew").is_empty());
    }

    // ── the session: offline still tracks locally ──

    #[test]
    fn an_inert_session_still_pins_locally_but_persists_nothing() {
        let mut prefs = ChooserPrefs::new(
            ChooserPrefsStore::new(PathBuf::from("/no/such/mesh/root")),
            "matthew",
            "seat-a",
        );
        assert!(!prefs.is_ready());
        assert!(prefs.toggle_favorite("peer:oak", 1_000), "pins locally");
        assert!(prefs.is_favorite("peer:oak"), "the local record folds in");
        assert!(
            !prefs.toggle_favorite("peer:oak", 2_000),
            "a second toggle un-pins"
        );
        assert!(!prefs.is_favorite("peer:oak"));
    }

    // ── THE CRUX: two seats, one workgroup root — a pin roams ──

    #[test]
    fn two_seats_roam_a_pin_a_recent_and_a_manual_source() {
        // One shared workgroup root = the Syncthing-replicated dir both seats see.
        let dir = temp_root("twoseat");
        std::fs::create_dir_all(&dir).expect("mkroot");

        // ── Seat A pins a desktop, records a use, adds a manual source. ──
        let mut a = ChooserPrefs::new(ChooserPrefsStore::new(dir.clone()), "matthew", "seat-a");
        assert!(a.is_ready());
        assert!(a.toggle_favorite("peer:oak", 1_000));
        a.record_recent("peer:oak", "oak", 1_100);
        a.set_manual(
            "manual:10.0.0.5:3389:rdp",
            "10.0.0.5",
            3389,
            "rdp",
            Some("OfficePC".to_owned()),
            1_200,
        );

        // ── Seat B opens fresh over the SAME root and folds A's record. ──
        let b = ChooserPrefs::new(ChooserPrefsStore::new(dir.clone()), "matthew", "seat-b");
        let merged = b.merged();
        assert!(
            merged.favorites.contains("peer:oak"),
            "seat A's pin roamed to seat B"
        );
        assert_eq!(
            merged.recents.first().map(|r| r.id.as_str()),
            Some("peer:oak"),
            "seat A's recent roamed to seat B"
        );
        assert_eq!(
            merged
                .manual
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["manual:10.0.0.5:3389:rdp"],
            "seat A's manual source roamed to seat B"
        );

        // ── Seat B un-pins; seat A folds the un-pin back (LWW converges). ──
        let mut b = b;
        assert!(!b.toggle_favorite("peer:oak", 2_000));
        let a_view = ChooserPrefs::new(ChooserPrefsStore::new(dir.clone()), "matthew", "seat-a");
        assert!(
            !a_view.is_favorite("peer:oak"),
            "seat B's newer un-pin roamed back to seat A"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
