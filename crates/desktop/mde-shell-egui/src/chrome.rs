//! The shell's live **mesh-status fold** — the world-readable snapshot poll
//! plus the pure [`MeshSummary`] projection the taskbar tray renders.
//!
//! Until NAVBAR-W10-2 this module also rendered the top chrome strip
//! (brand/version · Peers · Sessions · Status · Signal · BT · Vol · Batt ·
//! Chat · Collapse); lock W1 removed that bar outright — the shell has ONE
//! bar, the bottom taskbar, and the tray IS the status surface. What remains
//! here is the strip's pure heart:
//!
//! * **[`MeshSummary`]** folds the world-readable mesh-status snapshot the
//!   root timer writes (`/run/mde/mesh-status.json`) — the same source the
//!   panel client reads (the desktop user can't read the root-only peer
//!   directory). The worst-of lighthouse verdict is the reused LIGHTHOUSE-7
//!   model (`lighthouse_health_from_snapshot`), so the tray's Status dot
//!   can't diverge from the rest of the fleet's health verdict.
//! * **[`ChromeState::poll`]** is the ONE self-gating snapshot read + repaint
//!   heartbeat — `main.rs` drives it each frame and the tray consumes the
//!   product (`tray::TrayInputs.mesh`); no second poll, no second reader.
//!
//! The projection is pure (no egui `Context`, no IO, no GPU), so it's
//! unit-tested directly; the only IO is the snapshot read in `poll`. The
//! seat-side folds the strip carried (battery pack pick + tone) moved to
//! `tray.rs` with the icons they feed.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui;

use mde_cosmic_applet::{lighthouse_health_from_snapshot, LighthouseHealth};

/// The world-readable mesh-status snapshot the root timer writes. The shell
/// reads peers + lighthouse health from it exactly like the panel client — the
/// desktop user can't read the root-only replicated peer directory, so this
/// JSON is the read path.
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a peer join/leave or a lighthouse health flip surfaces within
/// this window (and the tray clock's minute flip rides the same heartbeat).
/// Matches the panel client + the Fleet datacenter poll; the read is a cheap
/// local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

// ──────────────────────────── projected view ────────────────────────────

/// The shell's live mesh summary, folded from the mesh-status snapshot — the
/// source behind the tray's Peers / Status / Signal dots. Pure data — parsed
/// without egui/IO/GPU, so it's unit-tested directly. (`pub`, not `pub(crate)`,
/// is the `clippy::redundant_pub_crate` form for crate-visible items in a
/// private module, like `dock::TASKBAR_H`.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshSummary {
    /// Peers in the directory (every node the snapshot names).
    pub peers_total: usize,
    /// Peers currently `presence == "online"`.
    pub peers_online: usize,
    /// Worst-of lighthouse health (the mesh "Status" verdict) — reused from the
    /// panel/applet model so the tray can't diverge from the fleet's verdict.
    pub health: LighthouseHealth,
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the honest dim pre-read state) from a parsed-but-empty mesh.
    pub seen: bool,
}

impl Default for MeshSummary {
    /// The pre-first-read state: nothing seen yet (the tray renders dim dots).
    /// `LighthouseHealth` has no `Default`, so this is hand-rolled.
    fn default() -> Self {
        Self {
            peers_total: 0,
            peers_online: 0,
            health: LighthouseHealth::None,
            seen: false,
        }
    }
}

impl MeshSummary {
    /// Fold the mesh-status snapshot JSON into the summary. A missing / garbage
    /// snapshot yields the honest unseen summary (the tray's dim dots), never a
    /// panic — mirroring the panel client's tolerance.
    pub(crate) fn from_snapshot(snapshot: &str) -> Self {
        // The worst-of lighthouse verdict is the reused LIGHTHOUSE-7 parser.
        let (health, _, _) = lighthouse_health_from_snapshot(snapshot);
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
            health,
            seen: true,
        }
    }
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
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ChromeState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            summary: MeshSummary::default(),
            last_poll: None,
        }
    }
}

impl ChromeState {
    /// The poll seam: refresh the projection from the snapshot when the cadence
    /// has elapsed, then keep the repaint heartbeat alive so a peer join/leave,
    /// a lighthouse flip, or the tray clock's minute change surfaces without
    /// input. Cheap enough to call every frame — it self-gates. A missing /
    /// unreadable snapshot yields the unseen summary (honest dim dots), never a
    /// panic. `pub(crate)` so the QBRAND-4 boot-splash can bank its "first mesh
    /// snapshot poll" milestone by running THIS real fold (the first dock frame
    /// then opens with a live tray).
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = std::fs::read_to_string(&self.snapshot_path).unwrap_or_default();
            self.summary = MeshSummary::from_snapshot(&snapshot);
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// The latest projection — what the taskbar tray folds its Peers / Status /
    /// Signal dots from each frame.
    pub(crate) const fn summary(&self) -> &MeshSummary {
        &self.summary
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
        assert_eq!(s.health, LighthouseHealth::None);
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
    fn health_folds_the_worst_of_lighthouse_verdict() {
        // All lighthouses up → AllHealthy.
        let up = MeshSummary::from_snapshot(&snapshot("online", "online", "offline"));
        assert_eq!(up.health, LighthouseHealth::AllHealthy);
        // Any lighthouse down → Degraded (worst-of).
        let deg = MeshSummary::from_snapshot(&snapshot("online", "idle", "online"));
        assert_eq!(deg.health, LighthouseHealth::Degraded);
        // No lighthouses in view → None.
        let none = MeshSummary::from_snapshot(
            r#"{"nodes":[{"hostname":"ws","overlay_ip":"10.42.0.50","presence":"online","role":"workstation"}],"network":{"lighthouse_ips":[]}}"#,
        );
        assert_eq!(none.health, LighthouseHealth::None);
    }

    #[test]
    fn chrome_state_defaults_to_the_snapshot_path_unseen() {
        let c = ChromeState::default();
        assert_eq!(c.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!c.summary().seen);
        assert!(c.last_poll.is_none());
    }
}
