//! WIN7-8 — Console's Custom-group entries (lock #35, the operator's own
//! named command entries), synced mesh-wide per operator identity (lock #21:
//! "Pin layout / Custom-group arrangement syncs mesh-wide per user (like
//! other synced settings)").
//!
//! **Investigation before this unit** (the design doc's own flagged open
//! item, `docs/design/win7-desktop-survey.md`'s "Open items" section):
//! Console's Custom entries were persisted ONLY to a local JSON file
//! (`<client-data-dir>/console-custom.json`, `console.rs`'s `CustomFile`) —
//! no mesh-sync mechanism at all, confirmed by reading `console.rs`'s
//! `CustomFile`/`persist_custom` in full before writing a line here. This
//! module ADDS the mesh-wide sync layer; it does not replace the local file
//! — `console.rs`'s existing `CustomFile`/`CUSTOM_FILE` continue completely
//! unchanged (same shape, same round-trip behavior), so an unmeshed seat —
//! or this crate's existing headless test suite, which never provisions a
//! workgroup root — keeps working exactly as it did before this unit.
//!
//! Also investigated and confirmed NOT in scope: `start_menu.rs`'s live-tile
//! grid has no operator-arrangeable concept to sync at all — `TILE_GROUPS`
//! (WIN7-3) is a compile-time `const [TileGroup; 7]`, not per-operator
//! state, so lock #21's "pin layout" half refers to Console's Custom group,
//! not the tile grid (WIN7-3/4's own module docs confirm the grid is fixed,
//! non-reorderable by design, per locks #6/#7).
//!
//! The mechanism reused verbatim in shape is CHOOSER-9's
//! (`crate::chooser::chooser_prefs`) — this project's own established
//! "operator preferences that follow the mesh identity between seats"
//! pattern (favorites/recents/manual sources), not a new one invented for
//! this unit: one JSON file **per seat** under the Syncthing-replicated
//! workgroup root (`<root>/console-custom-sync/<identity>/<seat>.json`),
//! written atomically (temp + rename); a missing/unmounted root is a
//! silent, honest no-op (§7) — never a fabricated synced record. Identity/
//! seat resolution
//! ([`resolve_identity`](crate::chooser::chooser_prefs::resolve_identity) /
//! [`resolve_seat`](crate::chooser::chooser_prefs::resolve_seat)) is reused
//! directly (not reimplemented) from `chooser_prefs`, so every
//! identity-bound record in this crate agrees on the same precedence —
//! `chooser.rs`'s `mod chooser_prefs;` is widened `pub(crate)` for this
//! reuse (the `dock::response_activated`/`status::severity_color`
//! cross-module-widening idiom already established this epic).
//!
//! # Merge, not a single lease
//!
//! Like CHOOSER-9's `manual` sources, a Custom entry is a last-writer-wins
//! register keyed by its own **content** (`name` + `command` — this domain
//! has no separate id/rename concept: CONSOLE-4 only ever adds or removes a
//! whole entry, never edits one in place, so the content pair IS the stable
//! identity). An add on any seat converges; a remove on ANY seat —
//! including of an entry that seat never itself added — converges too, by
//! writing a fresh tombstone register into the removing seat's OWN file
//! (the exact `ChooserPrefs::remove_manual` idiom: a tombstone is pushed
//! even when this seat has never locally registered that entry before).
//! The one honestly-flagged simplification content-keying introduces: two
//! entries sharing BOTH the identical name AND the identical command
//! collapse to one register once synced (nothing stopped registering the
//! literal same pair twice locally before this unit; the merge now treats
//! true duplicates as one). A random/synthetic id was considered and
//! rejected — it would need a durable id threaded through the plain local
//! file / UI / accesskit code that never needed one before, to guard a
//! near-zero-stakes edge case.
//!
//! # Honest offline
//!
//! A seat with no mesh volume ([`CustomSync::is_ready`] false) never writes
//! here; `console.rs`'s pre-existing local file remains the sole source of
//! truth for that seat, exactly as before this unit — but the in-memory
//! session still folds its own not-yet-mirrored edits, so a single
//! unmeshed seat's own add/remove still works session-scoped (the
//! `ChooserPrefs` "inert session still pins locally" idiom restated).

use std::collections::HashMap;
use std::path::PathBuf;
use std::{fs, io};

use serde::{Deserialize, Serialize};

use super::CustomEntry;
use crate::chooser::chooser_prefs::{resolve_identity, resolve_seat};

/// The share subdirectory the per-identity Custom-entry sync records live
/// under (`<root>/console-custom-sync/<identity>/<seat>.json`) — distinct
/// from `console.rs`'s own `console-custom.json` local-file name (a
/// different root entirely: the Syncthing workgroup root, never the local
/// client data dir), so the two never collide.
pub(crate) const CUSTOM_SYNC_SUBDIR: &str = "console-custom-sync";

/// One Custom entry as a last-writer-wins register (see the module doc for
/// why content, not a synthetic id, is the key): `present: false` is the
/// remove tombstone, so a remove on any seat converges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SyncedCustomEntry {
    /// The operator-visible entry — the SAME type `console.rs`'s local file
    /// and UI already use (flattened, so the JSON stays one flat object).
    #[serde(flatten)]
    pub(crate) entry: CustomEntry,
    /// Whether this register is present (false = removed tombstone).
    pub(crate) present: bool,
    /// Wall-clock epoch millis of the last add/remove (the LWW ordering
    /// key).
    pub(crate) updated_ms: u64,
}

/// One seat's contribution to the operator's synced Custom entries — the
/// single record it writes to its own file (the `PeerRecord`/`SeatPrefs`
/// single-writer-per-file idiom).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct SeatCustomFile {
    /// The mesh identity these entries belong to.
    pub(crate) identity: String,
    /// The seat that wrote this record.
    pub(crate) seat: String,
    /// Wall-clock epoch millis of the last write (freshness only; each
    /// register carries its own ordering key).
    pub(crate) updated_ms: u64,
    /// The registers this seat has touched (adds AND tombstones).
    #[serde(default)]
    pub(crate) entries: Vec<SyncedCustomEntry>,
}

/// The merged, LWW-folded Custom entries across every seat, in stable
/// oldest-add-first order (the closest honest analogue to the pre-sync
/// local behavior's plain append order) — `console.rs`'s render/activation
/// code consumes this unchanged, since it's the same [`CustomEntry`] shape
/// it already used.
#[must_use]
pub(crate) fn merge_custom_entries(records: &[SeatCustomFile]) -> Vec<CustomEntry> {
    let mut best: HashMap<(&str, &str), (&SyncedCustomEntry, &str)> = HashMap::new();
    for rec in records {
        for reg in &rec.entries {
            let key = (reg.entry.name.as_str(), reg.entry.command.as_str());
            let take = best.get(&key).is_none_or(|(cur, cur_seat)| {
                (reg.updated_ms, rec.seat.as_str()) > (cur.updated_ms, *cur_seat)
            });
            if take {
                best.insert(key, (reg, rec.seat.as_str()));
            }
        }
    }
    let mut out: Vec<&SyncedCustomEntry> = best
        .into_values()
        .filter(|(reg, _)| reg.present)
        .map(|(reg, _)| reg)
        .collect();
    out.sort_by(|a, b| {
        a.updated_ms
            .cmp(&b.updated_ms)
            .then_with(|| a.entry.name.cmp(&b.entry.name))
    });
    out.into_iter().map(|reg| reg.entry.clone()).collect()
}

/// The mesh-synced Custom-entry store — one JSON file per seat under the
/// Syncthing-replicated workgroup root (the `ChooserPrefsStore`/
/// `write_peer_record` single-writer-per-file idiom).
#[derive(Debug, Clone)]
pub(crate) struct CustomSyncStore {
    root: PathBuf,
}

impl CustomSyncStore {
    /// A store rooted at `root` (tests point this at a tempdir or a
    /// deliberately nonexistent path; production uses
    /// [`Self::open_default`]).
    #[must_use]
    pub(crate) const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// A store over the canonical workgroup root
    /// ([`mackes_mesh_types::peers::default_workgroup_root`]).
    #[must_use]
    pub(crate) fn open_default() -> Self {
        Self::new(mackes_mesh_types::peers::default_workgroup_root())
    }

    /// Whether the workgroup root is actually present. The store writes
    /// only under an existing root — never creating a bare unprovisioned
    /// mount — so a seat with no mesh volume is a silent no-op rather than
    /// a fabricated synced record (the `ChooserPrefsStore::is_ready`
    /// idiom).
    #[must_use]
    pub(crate) fn is_ready(&self) -> bool {
        self.root.is_dir()
    }

    /// The `<root>/console-custom-sync/<identity>/` directory.
    fn identity_dir(&self, identity: &str) -> PathBuf {
        self.root.join(CUSTOM_SYNC_SUBDIR).join(sanitize(identity))
    }

    /// The `<root>/console-custom-sync/<identity>/<seat>.json` path.
    fn seat_path(&self, identity: &str, seat: &str) -> PathBuf {
        self.identity_dir(identity)
            .join(format!("{}.json", sanitize(seat)))
    }

    /// Publish `rec` into this seat's file (atomic temp + rename). A silent
    /// no-op when the root is not provisioned ([`Self::is_ready`]).
    ///
    /// # Errors
    /// The [`io::Error`] if the directory cannot be created or the file
    /// cannot be written / renamed.
    pub(crate) fn publish(&self, rec: &SeatCustomFile) -> io::Result<()> {
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

    /// Fold every seat file for `identity` into a record list (one per
    /// seat). Malformed / half-written / temp files are skipped (never
    /// fatal), and a missing directory yields an empty list.
    #[must_use]
    pub(crate) fn records(&self, identity: &str) -> Vec<SeatCustomFile> {
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
                if let Ok(rec) = serde_json::from_str::<SeatCustomFile>(&data) {
                    out.push(rec);
                }
            }
        }
        out.sort_by(|a, b| a.seat.cmp(&b.seat));
        out
    }

    /// Read only `seat`'s own persisted record for `identity`, if any
    /// (used to resume a seat's tombstones on start-up).
    #[must_use]
    fn seat_record(&self, identity: &str, seat: &str) -> Option<SeatCustomFile> {
        let data = fs::read_to_string(self.seat_path(identity, seat)).ok()?;
        serde_json::from_str::<SeatCustomFile>(&data).ok()
    }
}

/// The per-seat Custom-sync session: owns the [`CustomSyncStore`] seam, the
/// mesh `identity` these entries roam under, this `seat`'s id, and this
/// seat's in-memory record (loaded on open so prior tombstones survive a
/// restart) — the `ChooserPrefs` idiom restated for Console's Custom
/// entries.
#[derive(Debug)]
pub(crate) struct CustomSync {
    store: CustomSyncStore,
    identity: String,
    seat: String,
    local: SeatCustomFile,
}

impl CustomSync {
    /// A sync session over `store` for `identity` at `seat`, resuming this
    /// seat's persisted record (its tombstones) if one is present.
    #[must_use]
    pub(crate) fn new(
        store: CustomSyncStore,
        identity: impl Into<String>,
        seat: impl Into<String>,
    ) -> Self {
        let identity = identity.into();
        let seat = seat.into();
        let local = store
            .seat_record(&identity, &seat)
            .unwrap_or_else(|| SeatCustomFile {
                identity: identity.clone(),
                seat: seat.clone(),
                ..SeatCustomFile::default()
            });
        Self {
            store,
            identity,
            seat,
            local,
        }
    }

    /// A sync session over the canonical workgroup root, with the mesh
    /// identity + seat resolved from the environment (reusing
    /// `chooser_prefs`'s own resolution — see the module doc for why).
    #[must_use]
    pub(crate) fn open_default() -> Self {
        Self::new(
            CustomSyncStore::open_default(),
            resolve_identity(),
            resolve_seat(),
        )
    }

    /// Whether the workgroup root is provisioned — when false, the session
    /// is inert and `console.rs` keeps its pre-existing local-file-only
    /// behavior untouched (honest offline).
    #[must_use]
    pub(crate) fn is_ready(&self) -> bool {
        self.store.is_ready()
    }

    /// This seat's own record, unioned with every other seat's file freshly
    /// read from disk (this seat's in-memory record is authoritative for
    /// its own seat, so an unpersisted local edit still counts — the
    /// offline path).
    fn records(&self) -> Vec<SeatCustomFile> {
        let mut recs = self.store.records(&self.identity);
        match recs.iter_mut().find(|r| r.seat == self.seat) {
            Some(slot) => slot.clone_from(&self.local),
            None => recs.push(self.local.clone()),
        }
        recs
    }

    /// The merged view across every seat + this one — the render source of
    /// truth once meshed.
    #[must_use]
    pub(crate) fn merged(&self) -> Vec<CustomEntry> {
        merge_custom_entries(&self.records())
    }

    /// Persist this seat's record (a no-op when the store is inert).
    fn persist(&mut self, now_ms: u64) {
        self.local.updated_ms = now_ms;
        let _ = self.store.publish(&self.local);
    }

    /// Register `entry` present (an add), stamped `now_ms`, and persist.
    pub(crate) fn add(&mut self, entry: CustomEntry, now_ms: u64) {
        match self.local.entries.iter_mut().find(|reg| reg.entry == entry) {
            Some(reg) => {
                reg.present = true;
                reg.updated_ms = now_ms;
            }
            None => self.local.entries.push(SyncedCustomEntry {
                entry,
                present: true,
                updated_ms: now_ms,
            }),
        }
        self.persist(now_ms);
    }

    /// Tombstone `entry` (`present: false`), stamped `now_ms`, and persist
    /// — so the remove converges across seats even when THIS seat never
    /// locally registered `entry` before (the `ChooserPrefs::remove_manual`
    /// idiom, see the module doc).
    pub(crate) fn remove(&mut self, entry: &CustomEntry, now_ms: u64) {
        match self
            .local
            .entries
            .iter_mut()
            .find(|reg| &reg.entry == entry)
        {
            Some(reg) => {
                reg.present = false;
                reg.updated_ms = now_ms;
            }
            None => self.local.entries.push(SyncedCustomEntry {
                entry: entry.clone(),
                present: false,
                updated_ms: now_ms,
            }),
        }
        self.persist(now_ms);
    }
}

/// Reduce an identity / seat id to a safe single path component — the exact
/// `chooser_prefs::sanitize` shape restated (a small private per-module
/// helper already duplicated more than once in this crate's mesh-sync
/// modules, e.g. `browser_session_sync::sanitize_host` in `mackesd`; small
/// and pure enough that one more copy here stays consistent with that
/// established convention rather than forcing a new shared-crate seam for
/// ~10 lines).
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, command: &str) -> CustomEntry {
        CustomEntry {
            name: name.to_owned(),
            command: command.to_owned(),
        }
    }

    fn seat_rec(seat: &str) -> SeatCustomFile {
        SeatCustomFile {
            identity: "matthew".to_owned(),
            seat: seat.to_owned(),
            ..SeatCustomFile::default()
        }
    }

    // ── the pure merge fold ──────────────────────────────────────────────

    #[test]
    fn merge_lww_so_a_remove_beats_an_older_add() {
        let mut a = seat_rec("seat-a");
        a.entries.push(SyncedCustomEntry {
            entry: entry("Fleet status", "meshctl fleet status"),
            present: true,
            updated_ms: 100,
        });
        let mut b = seat_rec("seat-b");
        b.entries.push(SyncedCustomEntry {
            entry: entry("Fleet status", "meshctl fleet status"),
            present: false,
            updated_ms: 200,
        });
        assert!(
            merge_custom_entries(&[a.clone(), b.clone()]).is_empty(),
            "the newer remove wins"
        );
        // Flip the timestamps: now the add is newer and survives.
        a.entries[0].updated_ms = 300;
        assert_eq!(
            merge_custom_entries(&[a, b]),
            vec![entry("Fleet status", "meshctl fleet status")]
        );
    }

    #[test]
    fn distinct_entries_from_different_seats_union() {
        let mut a = seat_rec("seat-a");
        a.entries.push(SyncedCustomEntry {
            entry: entry("Fleet status", "meshctl fleet status"),
            present: true,
            updated_ms: 1,
        });
        let mut b = seat_rec("seat-b");
        b.entries.push(SyncedCustomEntry {
            entry: entry("Farm top", "ssh mm@bigboy btop"),
            present: true,
            updated_ms: 1,
        });
        assert_eq!(
            merge_custom_entries(&[a, b]).len(),
            2,
            "distinct entries from two seats both survive"
        );
    }

    #[test]
    fn merge_orders_oldest_add_first() {
        let mut a = seat_rec("seat-a");
        a.entries.push(SyncedCustomEntry {
            entry: entry("Second", "cmd-2"),
            present: true,
            updated_ms: 200,
        });
        a.entries.push(SyncedCustomEntry {
            entry: entry("First", "cmd-1"),
            present: true,
            updated_ms: 100,
        });
        assert_eq!(
            merge_custom_entries(&[a]),
            vec![entry("First", "cmd-1"), entry("Second", "cmd-2")],
            "the merged view reads oldest-registered-first, like a plain local append"
        );
    }

    #[test]
    fn exact_duplicate_entries_collapse_to_one_register() {
        // Honestly-flagged simplification (module doc): content IS the key,
        // so two seats registering the literal same (name, command) fold to
        // one register, not two.
        let mut a = seat_rec("seat-a");
        a.entries.push(SyncedCustomEntry {
            entry: entry("Fleet status", "meshctl fleet status"),
            present: true,
            updated_ms: 1,
        });
        let mut b = seat_rec("seat-b");
        b.entries.push(SyncedCustomEntry {
            entry: entry("Fleet status", "meshctl fleet status"),
            present: true,
            updated_ms: 2,
        });
        assert_eq!(merge_custom_entries(&[a, b]).len(), 1);
    }

    // ── the store round trip ─────────────────────────────────────────────

    #[test]
    fn store_round_trips_and_folds_across_seats() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CustomSyncStore::new(dir.path().to_path_buf());
        assert!(store.is_ready());

        let mut a = seat_rec("seat-a");
        a.entries.push(SyncedCustomEntry {
            entry: entry("Fleet status", "meshctl fleet status"),
            present: true,
            updated_ms: 1,
        });
        store.publish(&a).expect("publish a");
        let mut b = seat_rec("seat-b");
        b.entries.push(SyncedCustomEntry {
            entry: entry("Farm top", "ssh mm@bigboy btop"),
            present: true,
            updated_ms: 1,
        });
        store.publish(&b).expect("publish b");

        let recs = store.records("matthew");
        assert_eq!(recs.len(), 2, "one file per seat, folded");
        assert_eq!(merge_custom_entries(&recs).len(), 2);

        // A corrupt file is skipped, never fatal.
        let corrupt = dir
            .path()
            .join(CUSTOM_SYNC_SUBDIR)
            .join("matthew")
            .join("seat-c.json");
        std::fs::write(&corrupt, "{ not json").expect("write corrupt");
        assert_eq!(store.records("matthew").len(), 2, "corrupt skipped");
    }

    #[test]
    fn store_is_inert_when_the_root_is_unprovisioned() {
        let store = CustomSyncStore::new(PathBuf::from("/no/such/mesh/root"));
        assert!(!store.is_ready());
        store.publish(&seat_rec("seat-a")).expect("no-op publish");
        assert!(store.records("matthew").is_empty());
    }

    // ── the session: offline still tracks locally ───────────────────────

    #[test]
    fn an_inert_session_still_folds_its_own_edit_but_persists_nothing() {
        let mut sync = CustomSync::new(
            CustomSyncStore::new(PathBuf::from("/no/such/mesh/root")),
            "matthew",
            "seat-a",
        );
        assert!(!sync.is_ready());
        sync.add(entry("Fleet status", "meshctl fleet status"), 1_000);
        // The in-memory session still folds its own not-yet-mirrored add (the
        // single-seat offline path)...
        assert_eq!(
            sync.merged(),
            vec![entry("Fleet status", "meshctl fleet status")]
        );
        // ...but nothing hit disk: `is_ready()` gates every
        // `CustomSyncStore::publish` call (proven independently above).
    }

    // ── THE CRUX: two seats, one workgroup root ─────────────────────────

    #[test]
    fn two_seats_converge_an_add_and_a_remove_of_the_others_entry() {
        // One shared workgroup root = the Syncthing-replicated dir both
        // seats see.
        let dir = tempfile::tempdir().expect("tempdir");

        // ── Seat A adds a Custom entry. ──
        let mut a = CustomSync::new(
            CustomSyncStore::new(dir.path().to_path_buf()),
            "matthew",
            "seat-a",
        );
        assert!(a.is_ready());
        a.add(entry("Fleet status", "meshctl fleet status"), 1_000);

        // ── Seat B, fresh over the SAME root, sees seat A's add. ──
        let b = CustomSync::new(
            CustomSyncStore::new(dir.path().to_path_buf()),
            "matthew",
            "seat-b",
        );
        assert_eq!(
            b.merged(),
            vec![entry("Fleet status", "meshctl fleet status")],
            "seat A's add roamed to seat B"
        );

        // ── Seat B removes an entry it never itself added. ──
        let mut b = b;
        b.remove(&entry("Fleet status", "meshctl fleet status"), 2_000);
        assert!(
            b.merged().is_empty(),
            "seat B's remove took effect in its own view"
        );

        // ── Seat A folds the remove back (LWW converges). ──
        let a_view = CustomSync::new(
            CustomSyncStore::new(dir.path().to_path_buf()),
            "matthew",
            "seat-a",
        );
        assert!(
            a_view.merged().is_empty(),
            "seat B's remove of seat A's entry roamed back to seat A"
        );
    }
}
