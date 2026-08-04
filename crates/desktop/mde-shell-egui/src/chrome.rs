//! The shell's live **mesh-status fold** — the world-readable snapshot poll
//! plus the pure [`MeshSummary`] projection the status chrome renders.
//!
//! Until NAVBAR-W10-2 this module also rendered the top chrome strip
//! (brand/version · Peers · Sessions · Status · Signal · BT · Vol · Batt ·
//! Chat · Collapse); lock W1 removed that bar outright. What remains here is
//! the strip's pure heart:
//!
//! * **[`MeshSummary`]** folds the world-readable mesh-status snapshot the
//!   root timer writes (`/run/mde/mesh-status.json`) — the same source the
//!   panel client reads (the desktop user can't read the root-only peer
//!   directory). It carries operational peer counts only; platform health is
//!   exclusively the typed authority below.
//! * **[`ChromeState::poll`]** is the ONE self-gating snapshot read + repaint
//!   heartbeat — `main.rs` drives it each frame. The taskbar consumes only the
//!   centralized health count; no second poll or score exists.
//!
//! The projection is pure (no egui `Context`, no IO, no GPU), so it's
//! unit-tested directly; the only IO is the snapshot read in `poll`. The
//! seat-side folds the strip carried (battery pack pick + tone) live in
//! `status.rs` with the panel they feed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::health::SystemMeshHealthSnapshot;
use mackes_mesh_types::peers::default_workgroup_root;
use mde_egui::egui;

/// The world-readable mesh-status snapshot the root timer writes. The shell
/// reads operational peer presence from it exactly like the panel client — the
/// desktop user can't read the root-only replicated peer directory, so this
/// JSON is the read path.
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a peer join/leave surfaces within
/// this window (and the tray clock's minute flip rides the same heartbeat).
/// Matches the panel client + the Fleet datacenter poll; the read is a cheap
/// local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// Both status-chrome inputs are compact JSON projections. Bound materialized
/// bytes before serde sees a world-readable or replicated file.
// Health history and structured evidence can legitimately exceed the compact
// mesh-status projection; keep both reads bounded while allowing a busy fleet.
const MAX_CHROME_RECORD_BYTES: usize = 2 * 1024 * 1024;

/// Read one status-chrome record through the descriptor that will be parsed.
/// Reject final symlinks, non-regular or blocking files, oversized input,
/// invalid UTF-8, and size changes while the record is materialized.
fn read_bounded_chrome_record(path: &Path) -> Option<String> {
    use std::io::Read as _;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400000 | 0o4000); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK

        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_file())
        {
            return None;
        }
    }
    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_file())
    {
        return None;
    }

    let file = options.open(path).ok()?;
    let before = file.metadata().ok()?;
    if !before.file_type().is_file() || before.len() > MAX_CHROME_RECORD_BYTES as u64 {
        return None;
    }

    let capacity = usize::try_from(before.len())
        .unwrap_or(MAX_CHROME_RECORD_BYTES)
        .min(MAX_CHROME_RECORD_BYTES)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    (&file)
        .take((MAX_CHROME_RECORD_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    let after = file.metadata().ok()?;
    if !after.file_type().is_file()
        || after.len() != before.len()
        || bytes.len() > MAX_CHROME_RECORD_BYTES
        || bytes.len() as u64 != before.len()
    {
        return None;
    }
    String::from_utf8(bytes).ok()
}

// ──────────────────────────── projected view ────────────────────────────

/// The shell's live mesh summary, folded from the mesh-status snapshot — the
/// source behind operational peer counts. Pure data — parsed
/// without egui/IO/GPU, so it's unit-tested directly. (`pub`, not `pub(crate)`,
/// is the `clippy::redundant_pub_crate` form for crate-visible items in a
/// private module.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshSummary {
    /// Peers in the directory (every node the snapshot names).
    pub peers_total: usize,
    /// Peers currently `presence == "online"`.
    pub peers_online: usize,
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the honest dim pre-read state) from a parsed-but-empty mesh.
    pub seen: bool,
}

impl Default for MeshSummary {
    /// The pre-first-read state: nothing seen yet (the tray renders dim dots).
    /// This is hand-rolled to preserve the honest unseen state.
    fn default() -> Self {
        Self {
            peers_total: 0,
            peers_online: 0,
            seen: false,
        }
    }
}

impl MeshSummary {
    /// Fold the mesh-status snapshot JSON into the summary. A missing / garbage
    /// snapshot yields the honest unseen summary (the tray's dim dots), never a
    /// panic — mirroring the panel client's tolerance.
    pub(crate) fn from_snapshot(snapshot: &str) -> Self {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(snapshot) else {
            return Self::default();
        };
        let Some(nodes) = v.get("nodes").and_then(serde_json::Value::as_array) else {
            return Self::default();
        };
        let peers_total = nodes.len();
        let peers_online = nodes
            .iter()
            .filter(|n| n.get("presence").and_then(serde_json::Value::as_str) == Some("online"))
            .count();
        Self {
            peers_total,
            peers_online,
            seen: true,
        }
    }
}

// ──────────────────────── NODE-GRADE-2 the grade fold ───────────────────────
/// The shell projection of the daemon-owned snapshot. No grade wire shape is
/// mirrored here: consumers read the shared contract directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthStatus {
    snapshot: Option<SystemMeshHealthSnapshot>,
    pub seen: bool,
}

impl HealthStatus {
    #[must_use]
    pub const fn snapshot(&self) -> Option<&SystemMeshHealthSnapshot> {
        self.snapshot.as_ref()
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        self.snapshot
            .as_ref()
            .filter(|snapshot| snapshot.is_fresh(now_ms()))
            .map_or(0, |snapshot| snapshot.active_issue_count(now_ms()))
    }
}

fn read_health(path: &Path) -> HealthStatus {
    HealthStatus {
        snapshot: read_bounded_chrome_record(path)
            .and_then(|body| serde_json::from_str(&body).ok()),
        seen: true,
    }
}

/// Wall-clock ms since the epoch — the read clock grade staleness folds against (a
/// monotonic `Instant` can't compare to the worker's wall-clock publish stamp).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

// ──────────────────────────── the chrome state ────────────────────────────

/// The live mesh-fold state: the projected summary plus the small IO context to
/// refresh it on the shared cadence.
pub struct ChromeState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// The latest projection. Unseen until the first snapshot lands (the tray
    /// renders dim dots).
    summary: MeshSummary,
    /// The observer-owned, roster-folded health snapshot.
    health_path: PathBuf,
    /// Latest centralized health authority projection.
    health: HealthStatus,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ChromeState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            summary: MeshSummary::default(),
            health_path: default_workgroup_root()
                .join("system-mesh-health")
                .join("snapshots")
                .join(format!("{}.json", crate::explorer::local_hostname())),
            health: HealthStatus::default(),
            last_poll: None,
        }
    }
}

impl ChromeState {
    /// The poll seam: refresh the projection from the snapshot when the cadence
    /// has elapsed, then keep the repaint heartbeat alive so a peer join/leave,
    /// a peer-presence flip or the tray clock's minute change surfaces without
    /// input. Cheap enough to call every frame — it self-gates. A missing /
    /// unreadable snapshot yields the unseen summary (honest dim dots), never a
    /// panic. `pub(crate)` so the QBRAND-4 boot-splash can bank its "first mesh
    /// snapshot poll" milestone by running THIS real fold (the first dock frame
    /// then opens with a live tray).
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = read_bounded_chrome_record(&self.snapshot_path).unwrap_or_default();
            self.summary = MeshSummary::from_snapshot(&snapshot);
            self.health = read_health(&self.health_path);
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// The latest operational peer projection consumed by Construct chrome.
    pub(crate) const fn summary(&self) -> &MeshSummary {
        &self.summary
    }

    /// The one health authority consumed by the tray and modal.
    pub(crate) const fn health(&self) -> &HealthStatus {
        &self.health
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot with one lighthouse (by role) + one (by overlay-IP membership) +
    /// one ordinary workstation, each at a chosen presence — the same shape the
    /// applet/panel models are tested against.
    fn snapshot(lh_role: &str, lh_ip: &str, peer: &str) -> String {
        format!(
            r#"{{"nodes":[
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"{lh_role}","role":"lighthouse"}},
                {{"hostname":"lh-02","overlay_ip":"10.42.0.2","presence":"{lh_ip}","role":"server"}},
                {{"hostname":"ws-1","overlay_ip":"10.42.0.50","presence":"{peer}","role":"workstation"}}
            ],"network":{{"lighthouse_ips":["10.42.0.1","10.42.0.2"]}}}}"#
        )
    }

    #[test]
    fn unseen_before_the_first_snapshot() {
        let s = MeshSummary::default();
        assert!(!s.seen);
        assert_eq!((s.peers_online, s.peers_total), (0, 0));
    }

    #[test]
    fn garbage_or_missing_snapshot_stays_unseen() {
        for bad in ["", "not json", "{}", r#"{"network":{}}"#] {
            let s = MeshSummary::from_snapshot(bad);
            assert!(!s.seen, "{bad:?} must not read as a live mesh");
        }
    }

    #[test]
    fn peers_count_folds_total_and_online() {
        // Two lighthouses online + the workstation offline → 2/3 online.
        let s = MeshSummary::from_snapshot(&snapshot("online", "online", "offline"));
        assert!(s.seen);
        assert_eq!((s.peers_online, s.peers_total), (2, 3));
        // All three up → 3/3.
        let s = MeshSummary::from_snapshot(&snapshot("online", "online", "online"));
        assert_eq!((s.peers_online, s.peers_total), (3, 3));
    }

    #[test]
    fn empty_directory_is_seen_not_pre_read() {
        // A parsed snapshot with an empty node list is "seen" → the tray's
        // honest empty state, distinct from the pre-read dim state.
        let s = MeshSummary::from_snapshot(r#"{"nodes":[],"network":{"lighthouse_ips":[]}}"#);
        assert!(s.seen);
        assert_eq!(s.peers_total, 0);
    }

    #[test]
    fn chrome_state_defaults_to_the_snapshot_path_unseen() {
        let c = ChromeState::default();
        assert_eq!(c.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!c.summary().seen);
        assert!(!c.health().seen, "health unseen before the first poll");
        assert!(c.last_poll.is_none());
    }

    #[test]
    fn chrome_record_reader_rejects_hostile_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.json");
        std::fs::write(&path, "{}\n").unwrap();
        assert_eq!(read_bounded_chrome_record(&path).as_deref(), Some("{}\n"));

        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(read_bounded_chrome_record(&path).is_none());

        std::fs::write(&path, vec![b'x'; MAX_CHROME_RECORD_BYTES + 1]).unwrap();
        assert!(read_bounded_chrome_record(&path).is_none());

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(read_bounded_chrome_record(&path).is_none());

        #[cfg(unix)]
        {
            std::fs::remove_dir(&path).unwrap();
            let target = dir.path().join("target.json");
            std::fs::write(&target, "{}\n").unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            assert!(read_bounded_chrome_record(&path).is_none());

            std::fs::remove_file(&path).unwrap();
            let fifo = dir.path().join("record.fifo");
            if std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .is_ok_and(|status| status.success())
            {
                assert!(read_bounded_chrome_record(&fifo).is_none());
            }
        }
    }

    #[test]
    fn malformed_health_snapshot_is_honestly_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("health.json");
        std::fs::write(&path, "not-json").unwrap();
        let health = read_health(&path);
        assert!(health.seen);
        assert!(health.snapshot().is_none());
        assert_eq!(health.active_count(), 0);
    }
}
