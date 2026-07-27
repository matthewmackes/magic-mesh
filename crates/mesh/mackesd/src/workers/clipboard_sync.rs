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

use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

pub mod session;

use super::{ShutdownToken, Worker};

/// Non-pinned entries kept in the shared history (O7: pins are exempt +
/// unlimited, so the real file can be longer than this).
pub const HISTORY_CAP: usize = 50;

/// Bus topic every text clip is broadcast on. The viewer + any tailing
/// consumer subscribe here for real-time updates; the durable record is
/// the history file.
pub const CLIP_TOPIC: &str = "event/clipboard/clip";

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
    if clip.text.trim().is_empty() {
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
    /// Bus root override (tests). `None` ⇒ [`crate::bus_publish::default_bus_root`].
    bus_root_override: Option<PathBuf>,
    /// Bus drain cadence.
    poll: Duration,
}

impl ClipboardSyncWorker {
    /// Build the worker rooted at the replicated workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            bus_root_override: None,
            poll: CLIP_EVENT_POLL_INTERVAL,
        }
    }

    /// Override the Bus root (tests).
    #[cfg(test)]
    #[must_use]
    fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override
            .clone()
            .or_else(crate::bus_publish::default_bus_root)
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

    /// Drain new canonical `event/clipboard/clip` messages since `cursor`.
    fn drain_clip_events(&self, persist: &mut Persist, cursor: &mut Option<String>) -> usize {
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
            *cursor = Some(msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            let clip = match parse_clip_event_body(body) {
                Ok(clip) => clip,
                Err(e) => {
                    warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %e, "bad clipboard event body");
                    continue;
                }
            };
            match self.handle_clip_event(&clip) {
                Ok(true) => {
                    applied += 1;
                    debug!(
                        target: "clipboard_sync",
                        source = %clip.source,
                        "folded clipboard event ({} bytes)",
                        clip.text.len()
                    );
                }
                Ok(false) => {}
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
        let mut persist = match Persist::open(bus_root) {
            Ok(persist) => persist,
            Err(e) => {
                warn!(target: "clipboard_sync", error = %e, "bus open failed; worker idle");
                return Ok(());
            }
        };
        // Existing retained clipboard events may predate this daemon instance and
        // could resurrect a user-deleted/cleared history row. Start at the tail and
        // consume newly published lane events from here.
        let mut cursor = persist.latest_ulid(CLIP_TOPIC).ok().flatten();
        info!(target: "clipboard_sync", "watching canonical clipboard bus lane");
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.drain_clip_events(&mut persist, &mut cursor);
                }
                () = shutdown.wait() => return Ok(()),
            }
        }
    }
}

/// Build the supervisor-ready worker (call site in `run_serve`).
#[must_use]
pub fn build(workgroup_root: PathBuf) -> ClipboardSyncWorker {
    ClipboardSyncWorker::new(workgroup_root)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(w.drain_clip_events(&mut persist, &mut cursor), 3);
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
