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

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mde_egui::egui;
use serde::Deserialize;

use mackes_mesh_types::peers::default_workgroup_root;
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

// ──────────────────────── NODE-GRADE-2 the grade fold ───────────────────────
//
// Each node self-grades + publishes `<workgroup_root>/node-grade/<host>.json`
// (NODE-GRADE-1); the dock's grade mini-list is a thin renderer over the SAME
// status poll that feeds the mesh summary. The shell mirrors the worker's payload
// with its own serde structs rather than depending on the mackesd daemon crate —
// the established "keep the shell in the desktop-shell tier" pattern (§6, the Fleet
// plane's Bus mirrors do the same).

/// A node grade older than this (ms) reads as **stale** — the observer can no longer
/// see it, so its row shows a greyed "?" rather than a stale letter (design #17, §7).
/// The `node_grade` worker republishes every ~10 s and heartbeats every 60 s, so
/// 90 s of silence is a real gap, not a missed tick.
const GRADE_STALE_MS: u64 = 90_000;

/// The recent-slope trend arrow a grade row carries (NODE-GRADE-2 #14) — a local
/// mirror of the `node_grade` worker's `trend` enum. Its serde shape matches the
/// worker's lowercase wire tokens (`"up"`/`"steady"`/`"down"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GradeTrend {
    /// Score rising over the recent window.
    Up,
    /// Score roughly flat (the honest default for a malformed/absent trend).
    #[default]
    Steady,
    /// Score falling.
    Down,
}

impl GradeTrend {
    /// The arrow glyph the dock row renders (#14).
    pub(crate) const fn arrow(self) -> &'static str {
        match self {
            Self::Up => "↑",
            Self::Steady => "→",
            Self::Down => "↓",
        }
    }
}

/// The published grade JSON as the dock needs it — a local serde mirror of the
/// `node_grade` worker's `<workgroup_root>/node-grade/<host>.json` body. Only the
/// fields the dock renders are decoded: the `score` (→ the band via
/// [`mde_egui::GradeBand`] + the load bar), the trend arrow, and the freshness
/// stamp. The worker's own `grade` letter + `factors` block are ignored so "which
/// score is which grade" stays defined ONCE, in `mde_egui` (§4).
#[derive(Debug, Clone, Deserialize)]
struct RawGrade {
    host: String,
    score: u8,
    #[serde(default)]
    trend: GradeTrend,
    #[serde(default)]
    published_at_ms: u64,
}

/// One node's grade as the dock's mini-list renders it — folded from a [`RawGrade`]
/// with the local node flagged (pinned first, #18) and staleness resolved against
/// the read clock (#17/§7). Pure data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GradeRow {
    /// The publishing node's hostname (the row key + the tap-route target, #7).
    pub host: String,
    /// The published 0–100 capability score (the load bar + the derived band).
    pub score: u8,
    /// The recent-slope trend arrow.
    pub trend: GradeTrend,
    /// Whether this is the local node — pinned first with the "you are here"
    /// marker (#18).
    pub is_local: bool,
    /// Whether the row is stale (the node stopped publishing) — rendered as a greyed
    /// "?", never a fake letter (#17/§7).
    pub stale: bool,
}

/// The folded per-node grade set the dock's mini-list renders (NODE-GRADE-2) —
/// local pinned first, then peers worst-grade-first (#18/#19). `seen` distinguishes
/// the honest pre-poll state (no rows, the band vanishes) from a polled-but-empty
/// grade directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeGrades {
    /// The rows in render order (local first, then worst-first among peers).
    pub rows: Vec<GradeRow>,
    /// `true` once the grade directory has been read at least once.
    pub seen: bool,
}

impl NodeGrades {
    /// Fold the raw published grades into the dock's render order: the local node
    /// pinned first (#18), then peers **worst-first** (#19). A stale/unobservable
    /// node ranks with the worst (the design's "unreachable is itself an F", #17)
    /// though it renders honestly as "?". Pure over its inputs (the read clock + the
    /// local hostname are the caller's), so the sort + staleness are unit-tested.
    fn fold(published: Vec<RawGrade>, local_host: &str, now_ms: u64) -> Self {
        let mut rows: Vec<GradeRow> = published
            .into_iter()
            .map(|r| GradeRow {
                is_local: r.host == local_host,
                stale: now_ms.saturating_sub(r.published_at_ms) > GRADE_STALE_MS,
                host: r.host,
                score: r.score,
                trend: r.trend,
            })
            .collect();
        rows.sort_by(|a, b| {
            // Local pinned first, then ascending effective score (worst first), then
            // a stable tie-break by host so the list never jitters between polls.
            b.is_local
                .cmp(&a.is_local)
                .then_with(|| sort_rank(a).cmp(&sort_rank(b)))
                .then_with(|| a.host.cmp(&b.host))
        });
        Self { rows, seen: true }
    }
}

/// The worst-first sort key for a grade row: a stale node sinks to `0` (ranked with
/// the failing, #17); a live node sorts by its score.
const fn sort_rank(row: &GradeRow) -> u8 {
    if row.stale {
        0
    } else {
        row.score
    }
}

/// Read + fold every published node grade under `dir` (`<root>/node-grade/*.json`)
/// into the dock's render order. A missing directory / junk / half-replicated file
/// yields the honest empty (but `seen`) set — never a panic (mirrors
/// [`MeshSummary::from_snapshot`]'s tolerance). Dotfiles (the atomic temp writes)
/// are skipped.
fn read_grades(dir: &Path, local_host: &str) -> NodeGrades {
    let mut raws = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
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
                continue;
            }
            if let Ok(body) = std::fs::read_to_string(&path) {
                if let Ok(raw) = serde_json::from_str::<RawGrade>(&body) {
                    raws.push(raw);
                }
            }
        }
    }
    NodeGrades::fold(raws, local_host, now_ms())
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
    /// The replicated per-node grade directory (`<workgroup_root>/node-grade`),
    /// resolved once — the same substrate mount the Explorer + roaming read from.
    grade_dir: PathBuf,
    /// This node's hostname — pins the local grade row first (#18), off the SAME
    /// resolution the Explorer uses (no third copy of the idiom).
    local_host: String,
    /// The latest folded grade set (NODE-GRADE-2). Unseen until the first read (the
    /// dock renders no fake rows pre-poll, §7).
    grades: NodeGrades,
    /// When the snapshot was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ChromeState {
    fn default() -> Self {
        Self {
            snapshot_path: PathBuf::from(SNAPSHOT_PATH),
            summary: MeshSummary::default(),
            grade_dir: default_workgroup_root().join("node-grade"),
            local_host: crate::explorer::local_hostname(),
            grades: NodeGrades::default(),
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
            // The NODE-GRADE-2 grade fold rides the SAME status poll (no second
            // reader / cadence) — a cheap local scan of the replicated grade dir.
            self.grades = read_grades(&self.grade_dir, &self.local_host);
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// The latest projection — what the taskbar tray folds its Peers / Status /
    /// Signal dots from each frame.
    pub(crate) const fn summary(&self) -> &MeshSummary {
        &self.summary
    }

    /// The latest folded grade set — what the dock's NODE-GRADE-2 mini-list renders
    /// (local pinned first, peers worst-first). Fed into `set_status_inputs` beside
    /// the mesh summary each frame.
    pub(crate) const fn grades(&self) -> &NodeGrades {
        &self.grades
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
        assert!(!c.grades().seen, "grades unseen before the first poll");
        assert!(c.last_poll.is_none());
    }

    // ── NODE-GRADE-2: the grade fold (sort / pin / staleness) ─────────────────

    /// A raw published grade at a chosen score + publish stamp (steady trend).
    fn raw(host: &str, score: u8, published_at_ms: u64) -> RawGrade {
        RawGrade {
            host: host.to_string(),
            score,
            trend: GradeTrend::Steady,
            published_at_ms,
        }
    }

    #[test]
    fn grades_fold_pins_local_first_then_worst_peers() {
        // #18/#19 — the local node leads regardless of its own score, then the peers
        // sort worst-grade-first (the F/blinking end near the top).
        let now = 1_000_000;
        let raws = vec![
            raw("oak", 92, now), // a healthy peer
            raw("me", 55, now),  // local, itself failing
            raw("elm", 30, now), // the worst peer
        ];
        let g = NodeGrades::fold(raws, "me", now);
        assert!(g.seen);
        assert_eq!(g.rows[0].host, "me", "local pinned first (#18)");
        assert!(g.rows[0].is_local);
        assert_eq!(g.rows[1].host, "elm", "then the worst peer (#19)");
        assert_eq!(g.rows[2].host, "oak");
        assert!(!g.rows[1].is_local && !g.rows[2].is_local);
    }

    #[test]
    fn a_stale_grade_reads_as_worst_and_flagged() {
        // #17/§7 — a node that stopped publishing is stale (renders "?"), and ranks
        // with the worst even though its last-seen score was high.
        let now = 10_000_000_u64;
        let raws = vec![
            raw("fresh", 95, now),
            raw("gone", 88, now - GRADE_STALE_MS - 1),
        ];
        let g = NodeGrades::fold(raws, "solo", now);
        let gone = g.rows.iter().find(|r| r.host == "gone").expect("gone row");
        assert!(gone.stale, "a silent node is stale");
        assert!(
            !g.rows
                .iter()
                .find(|r| r.host == "fresh")
                .expect("row")
                .stale
        );
        assert_eq!(g.rows[0].host, "gone", "stale ranks with the worst (#17)");
        assert_eq!(g.rows[1].host, "fresh");
    }
}
