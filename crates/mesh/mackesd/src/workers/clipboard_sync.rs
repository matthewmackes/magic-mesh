//! CLIP-SYNC-1 — mesh clipboard sync worker.
//!
//! Watches the local Wayland clipboard for text changes, broadcasts every
//! clip on the Mackes Bus (`event/clipboard/clip`), and appends it to ONE
//! mesh-global history file on the QNM-Shared replicated root
//! (`<root>/clipboard/history.json`). Every peer runs this worker; the
//! single shared file is the mesh-global clipboard (no per-user/per-node
//! partition — the single-operator model, design lock O8).
//!
//! Operator locks (design `docs/design/notify-hub-redesign.md`, survey
//! round 1, 2026-06-18):
//!   * O1 capture — the Cosmic clipboard-manager exposes the wlroots
//!     data-control protocol, so `wl-paste --watch` IS the integration
//!     hook on Cosmic; it is also the explicit fallback elsewhere. One
//!     subprocess streams a fresh copy of the selection on every change.
//!   * O2 echo-loop — **debounce identical content**: a copy whose text
//!     equals the most-recent applied clip is dropped. This is what kills
//!     the click-to-load echo (the viewer `wl-copy`s an entry back onto
//!     this node, which `wl-paste --watch` re-emits — we drop it) without
//!     origin-tagging the selection.
//!   * O3 dedup — **move-to-top**: re-copying existing text bumps the one
//!     entry to the front instead of duplicating.
//!   * O4 no size cap — any text length syncs (the bus-retention worker
//!     bounds the bus; the history stays at 50 + pinned).
//!   * O6 stamp — each entry carries its source node + an RFC3339 time so
//!     the viewer renders "from <node> · <age>".
//!   * O7 pins — pinned entries are **exempt from the 50-cap and
//!     unlimited**; only unpinned entries are trimmed.
//!
//! The history mutations (`apply_clip`) are pure + fully unit-tested; the
//! worker body is the I/O glue (spawn `wl-paste`, read/merge/write the
//! shared file under the meshfs-mount guard, publish to the bus). The
//! `action/clipboard/*` IPC responder (`ipc::clipboard`) edits the same
//! file for the viewer's delete/pin/clear verbs.
//!
//! **Concurrency.** Each writer (this worker, the IPC responder, every
//! peer) does an unlocked read → mutate → atomic-`rename` write of the one
//! shared `history.json` — the same last-writer-wins shape the sibling
//! shared-state responders (`ipc::connect`, the peer directory) use against
//! the replicated root. The atomic rename prevents a torn read; a rare
//! concurrent pin-vs-capture can lose one update, self-healing on the next
//! capture. A real clipboard never sustains the write rate where this
//! matters, so a cross-node lock is deliberately not taken here (it would
//! add a LizardFS round-trip to every copy).

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, info, warn};

/// The capture command: `wl-paste --watch` runs the given command on every
/// selection change. We frame each selection with a trailing NUL
/// (`cat; printf '\0'`) so a clip containing embedded newlines is read back
/// as ONE record (`read_until(0)`), not split per line — the multi-line
/// fidelity the history needs.
const WATCH_ARGS: &[&str] = &["--watch", "sh", "-c", "cat; printf '\\0'"];

/// The capture binary. `wl-paste` is wlroots/Cosmic's data-control client;
/// it is the Cosmic clipboard-manager hook (O1) and the explicit fallback.
const WL_PASTE: &str = "wl-paste";

/// NUL byte — the per-selection frame delimiter (see [`WATCH_ARGS`]).
const NUL: u8 = 0;

use super::{ShutdownToken, Worker};

/// Non-pinned entries kept in the shared history (O7: pins are exempt +
/// unlimited, so the real file can be longer than this).
pub const HISTORY_CAP: usize = 50;

/// Bus topic every text clip is broadcast on. The viewer + any tailing
/// consumer subscribe here for real-time updates; the durable record is
/// the history file.
pub const CLIP_TOPIC: &str = "event/clipboard/clip";

/// How long to wait before re-spawning `wl-paste --watch` after it exits
/// (compositor restart, display coming up late). Paced so a missing
/// display doesn't busy-loop.
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(3);

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
/// Returns `true` when the history changed (the caller then persists +
/// publishes); `false` when the clip was debounced away (O2) and nothing
/// should be written or broadcast.
///
///   * **O2 debounce** — if `text` equals the current top entry's text, it
///     is a no-op (drops the click-to-load echo + a redundant re-copy of
///     the same already-top clip).
///   * **O3 dedup move-to-top** — if `text` matches a *lower* existing
///     entry, that entry is moved to the front (its pinned flag + original
///     id preserved) rather than duplicated.
///   * **new** — otherwise a fresh entry is pushed to the front.
///   * **O7 cap** — after insertion, unpinned entries beyond
///     [`HISTORY_CAP`] are trimmed (oldest first); pinned entries are
///     never counted nor trimmed.
pub fn apply_clip(history: &mut History, text: &str, source: &str, now: &str) -> bool {
    // O2 — identical to the current top → debounce (no change, no echo).
    if history.entries.first().is_some_and(|e| e.text == text) {
        return false;
    }
    // O3 — same text lower in the list → move it to the top, keeping its
    // pin + id, refreshing source/time to the capture that re-surfaced it.
    if let Some(pos) = history.entries.iter().position(|e| e.text == text) {
        let mut existing = history.entries.remove(pos);
        existing.source = source.to_string();
        existing.time = now.to_string();
        history.entries.insert(0, existing);
    } else {
        history.entries.insert(
            0,
            ClipEntry {
                id: clip_id(text),
                text: text.to_string(),
                source: source.to_string(),
                time: now.to_string(),
                pinned: false,
            },
        );
    }
    trim_unpinned(history, HISTORY_CAP);
    true
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

/// Substrate-aware writability for the shared clipboard history.
///
/// Pure core — `on_etcd` + `root_is_dir` are injected so it unit-tests without
/// touching `/etc` or the filesystem. See
/// [`ClipboardSyncWorker::share_writable`] for the why.
///
///   * `on_etcd == true` (the SUBSTRATE-1 endpoints file exists → this node is
///     on the etcd/Syncthing substrate) — `/mnt/mesh-storage` is a plain
///     Syncthing directory, **not** a FUSE mount, so there is no mountpoint to
///     poison. Writable **iff the root actually exists as a directory**
///     (`root_is_dir`): a present plain dir is fine, but a missing/unprovisioned
///     share (early boot before Syncthing creates it) or a bare unmounted
///     `LizardFS` mountpoint left mid-cutover is NOT written into — that both
///     avoids a per-clip write error and keeps the ONBOARD-6/XPA-10 poison
///     invariant (never fill a bare canonical mountpoint). This mirrors the
///     `boot_readiness` SUBSTRATE-10 probe's shape (a real liveness check when
///     on etcd, not an unconditional yes).
///   * `on_etcd == false` (pre-cutover, legacy `LizardFS`) — defer to the
///     ONBOARD-6/XPA-10 [`crate::shared_root_writable`] guard so we never fill
///     a bare canonical mountpoint and block `mfsmount`.
#[must_use]
pub fn clip_share_writable_core(workgroup_root: &Path, on_etcd: bool, root_is_dir: bool) -> bool {
    if on_etcd {
        root_is_dir
    } else {
        crate::shared_root_writable(workgroup_root)
    }
}

/// Substrate-aware writability for the shared clipboard history, reading the
/// live SUBSTRATE-1 etcd endpoints file + the shared root's directory state.
/// Thin I/O wrapper over [`clip_share_writable_core`].
#[must_use]
pub fn clip_share_writable(workgroup_root: &Path) -> bool {
    let on_etcd = !crate::substrate::etcd::default_endpoints().is_empty();
    clip_share_writable_core(workgroup_root, on_etcd, workgroup_root.is_dir())
}

/// This node's short hostname — the O6 source stamp.
fn local_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().split('.').next().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// Broadcast one clip on the bus (best-effort, fire-and-forget shell-out —
/// same `mde-bus publish` bridge shape as `ipc::bus_bridge`). The durable
/// record is the history file; the bus event is the real-time nudge.
fn publish_clip(entry: &ClipEntry) {
    let body = serde_json::json!({
        "id": entry.id,
        "text": entry.text,
        "source": entry.source,
        "time": entry.time,
    })
    .to_string();
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.arg("publish")
        .arg(CLIP_TOPIC)
        .arg("--body-flag")
        .arg(&body)
        // Persist + audit without needing the broker up (pre-enrollment).
        .arg("--no-broker");
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// The clipboard-sync worker. Holds the replicated root + this node's
/// source stamp; the run loop spawns `wl-paste --watch` and folds every
/// emitted clip through [`apply_clip`].
pub struct ClipboardSyncWorker {
    workgroup_root: PathBuf,
    source: String,
}

impl ClipboardSyncWorker {
    /// Build the worker rooted at the replicated workgroup root, stamping
    /// captures with this node's hostname.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            source: local_hostname(),
        }
    }

    /// Test seam — pin the source node label explicitly.
    #[must_use]
    pub fn with_source(mut self, source: String) -> Self {
        self.source = source;
        self
    }

    /// Whether it is safe to write `clipboard/history.json` under the shared
    /// root, **substrate-aware** (mirrors the boot_readiness SUBSTRATE-10
    /// probe).
    ///
    /// Post-SUBSTRATE-V2 `/mnt/mesh-storage` is a **plain Syncthing directory,
    /// not a FUSE mount** (design `substrate-v2.md` Q3/Q8: "now a plain local
    /// dir (NO FUSE)"), so the legacy LizardFS poison guard
    /// ([`crate::shared_root_writable`], which gates the canonical path on a
    /// real `/proc/mounts` entry) returns `false` for it and the worker would
    /// silently drop **every** clip — `history.json` is never written and the
    /// Hub's Clipboard Viewer reads an always-empty `action/clipboard/list`.
    /// When the etcd coordination plane is provisioned (the SUBSTRATE-1
    /// endpoints file is present) the node is on SUBSTRATE-V2, the shared root
    /// is a plain dir, and there is no mountpoint to poison — so it is
    /// writable. Pre-cutover (no endpoints file) we keep the LizardFS
    /// mount-poison guard.
    fn share_writable(&self) -> bool {
        clip_share_writable(&self.workgroup_root)
    }

    /// Fold one captured clip into the shared history + broadcast it. Skips
    /// blank captures and debounced echoes (O2); persists + publishes only
    /// on a real change. Best-effort + logged so a transient write/probe
    /// failure never kills the capture stream.
    fn handle_clip(&self, text: &str) {
        // Skip empty / whitespace-only selections (a cleared clipboard, or
        // the blank middle of a framing artifact) — they'd otherwise consume
        // a 50-cap slot and broadcast noise. The stored text stays VERBATIM
        // (we only trim for the keep/skip decision, not the content).
        if text.trim().is_empty() {
            return;
        }
        if !self.share_writable() {
            debug!(target: "clipboard_sync", "shared root not writable; dropping clip");
            return;
        }
        let path = history_path(&self.workgroup_root);
        let mut history = read_history(&path);
        if !apply_clip(&mut history, text, &self.source, &now_rfc3339()) {
            // O2 — debounced echo; nothing changed.
            return;
        }
        if let Err(e) = write_history(&path, &history) {
            warn!(target: "clipboard_sync", "history write failed: {e}");
            return;
        }
        // The just-applied clip is the front entry — broadcast it.
        if let Some(top) = history.entries.first() {
            publish_clip(top);
            debug!(target: "clipboard_sync", source = %self.source, "synced clip ({} bytes)", text.len());
        }
    }
}

#[async_trait::async_trait]
impl Worker for ClipboardSyncWorker {
    fn name(&self) -> &'static str {
        "clipboard_sync"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // `wl-paste --watch <cmd>` runs <cmd> once per selection change. We
        // frame each selection with a trailing NUL (WATCH_ARGS) and read it
        // back with `read_until(NUL)`, so a multi-line clip arrives as ONE
        // record rather than being split per line (the fidelity bug a naive
        // `--watch cat` + line reader would have).
        loop {
            // A Wayland display is required for any clipboard to exist.
            if std::env::var("WAYLAND_DISPLAY").is_err() {
                debug!(target: "clipboard_sync", "$WAYLAND_DISPLAY unset; idling");
                tokio::select! {
                    () = tokio::time::sleep(RESPAWN_COOLDOWN) => continue,
                    () = shutdown.wait() => return Ok(()),
                }
            }
            let mut child = match Command::new(WL_PASTE)
                .args(WATCH_ARGS)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    debug!(target: "clipboard_sync", "{WL_PASTE} unavailable: {e}; retrying");
                    tokio::select! {
                        () = tokio::time::sleep(RESPAWN_COOLDOWN) => continue,
                        () = shutdown.wait() => return Ok(()),
                    }
                }
            };
            info!(target: "clipboard_sync", source = %self.source, "watching clipboard via {WL_PASTE} --watch");
            // stdout is configured `Stdio::piped()` above, so `take()` is
            // Some on the first read; tolerate None defensively (respawn)
            // rather than panic the worker.
            let Some(stdout) = child.stdout.take() else {
                warn!(target: "clipboard_sync", "no piped stdout; respawning");
                let _ = child.kill().await;
                continue;
            };
            let mut reader = BufReader::new(stdout);
            let mut buf: Vec<u8> = Vec::new();
            loop {
                buf.clear();
                tokio::select! {
                    read = reader.read_until(NUL, &mut buf) => {
                        match read {
                            Ok(0) => break, // EOF — child closed stdout → respawn
                            Ok(_) => {
                                // Drop the trailing NUL frame byte, then decode.
                                if buf.last() == Some(&NUL) {
                                    buf.pop();
                                }
                                match std::str::from_utf8(&buf) {
                                    Ok(text) => self.handle_clip(text),
                                    // Non-UTF-8 selection (an image / binary
                                    // target) — clipboard sync is text-only, skip.
                                    Err(_) => debug!(target: "clipboard_sync", "non-utf8 selection; skipped"),
                                }
                            }
                            Err(e) => {
                                warn!(target: "clipboard_sync", "read error: {e}");
                                break;
                            }
                        }
                    }
                    () = shutdown.wait() => {
                        let _ = child.kill().await;
                        return Ok(());
                    }
                }
            }
            // Child exited / stdout closed — reap + pace the respawn.
            let _ = child.wait().await;
            tokio::select! {
                () = tokio::time::sleep(RESPAWN_COOLDOWN) => {}
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
        let w = ClipboardSyncWorker::new(PathBuf::from("/tmp")).with_source("box".into());
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
    fn handle_clip_writes_and_dedups_end_to_end() {
        // Drive the worker's fold path against a tempdir root (writable → the
        // share guard passes for a non-canonical path).
        let dir = tempfile::tempdir().unwrap();
        let w = ClipboardSyncWorker::new(dir.path().to_path_buf()).with_source("nodeA".into());
        w.handle_clip("first");
        w.handle_clip("second");
        w.handle_clip("first"); // re-copy → move-to-top, no dup
        w.handle_clip(""); // blank → ignored
        let h = read_history(&history_path(dir.path()));
        assert_eq!(
            h.entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(h.entries[0].source, "nodeA");
    }

    #[test]
    fn multi_line_clip_is_one_verbatim_entry() {
        // The NUL-framed capture path delivers a multi-line selection as ONE
        // string; it must store as ONE entry with the newlines intact (not
        // split per line). This guards the framing contract at the fold layer.
        let dir = tempfile::tempdir().unwrap();
        let w = ClipboardSyncWorker::new(dir.path().to_path_buf()).with_source("n".into());
        let snippet = "line one\nline two\nline three";
        w.handle_clip(snippet);
        let h = read_history(&history_path(dir.path()));
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].text, snippet, "newlines preserved, one entry");
    }

    #[test]
    fn clip_share_writable_core_writes_on_etcd_when_root_exists() {
        // SUBSTRATE-V2: the canonical path is a plain Syncthing dir (no FUSE
        // mount), so the legacy `shared_root_writable` mount guard says false —
        // but on the etcd substrate there's no mountpoint to poison, so the
        // clipboard worker MUST treat an EXISTING dir as writable. This is the
        // bug fix: with the old guard, `on_etcd` was ignored and every clip was
        // dropped on a cut-over node, leaving the Hub's Clipboard Viewer empty.
        let canonical = Path::new(crate::CANONICAL_QNM_MOUNT);
        assert!(
            clip_share_writable_core(canonical, /* on_etcd = */ true, /* root_is_dir = */ true),
            "etcd substrate + present plain dir → writable"
        );
    }

    #[test]
    fn clip_share_writable_core_skips_missing_root_on_etcd() {
        // On etcd but the shared dir doesn't exist yet (early boot, before
        // Syncthing provisions it): NOT writable, so we don't error per-clip
        // writing into a missing path — mirrors boot_readiness's real liveness
        // check rather than an unconditional yes.
        let canonical = Path::new(crate::CANONICAL_QNM_MOUNT);
        assert!(!clip_share_writable_core(
            canonical, true, /* root_is_dir = */ false
        ));
    }

    #[test]
    fn clip_share_writable_core_defers_to_mount_guard_pre_cutover() {
        // Pre-cutover (no etcd endpoints) the canonical mount must still gate on
        // a real FUSE mount (ONBOARD-6/XPA-10 poison guard) — the value mirrors
        // `shared_root_writable` exactly, regardless of the dir-exists flag.
        let canonical = Path::new(crate::CANONICAL_QNM_MOUNT);
        assert_eq!(
            clip_share_writable_core(canonical, false, true),
            crate::shared_root_writable(canonical),
        );
        // A non-canonical root (dev tree / tempdir) is always writable
        // pre-cutover, and writable on etcd when it exists.
        let dir = tempfile::tempdir().unwrap();
        assert!(clip_share_writable_core(dir.path(), false, true));
        assert!(clip_share_writable_core(dir.path(), true, true));
    }

    #[test]
    fn whitespace_only_clip_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let w = ClipboardSyncWorker::new(dir.path().to_path_buf());
        w.handle_clip("   ");
        w.handle_clip("\n\t\n");
        let h = read_history(&history_path(dir.path()));
        assert!(h.entries.is_empty(), "blank/whitespace selections skipped");
    }
}
