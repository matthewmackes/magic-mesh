//! The persistent chrome bar — the thin top strip that frames every session and
//! carries the Expand toggle into the Workbench.
//!
//! The bar reads **live** mesh state on a self-gating poll cadence and renders
//! honest empty / loading / degraded states through the shared `Style` (§4/§7):
//!
//! * **Peers** and **Status** fold the world-readable mesh-status snapshot the
//!   root timer writes (`/run/mde/mesh-status.json`) — the same source the panel
//!   client reads (the desktop user can't read the root-only peer directory). The
//!   worst-of lighthouse verdict behind Status is the reused LIGHTHOUSE-7 model
//!   (`lighthouse_health_from_snapshot`), so the bar can't diverge from the rest
//!   of the fleet's health verdict.
//! * **Sessions** honestly reads "No session" until `mde-vdi` connects a live VM
//!   desktop (a later unit) — the same truth the collapsed session empty state
//!   shows. Nothing here fabricates a count.
//!
//! The projection (`MeshSummary`) + the model→slot mapping are pure (no egui
//! `Context`, no IO, no GPU), so they're unit-tested directly; the only IO is the
//! snapshot read in [`ChromeState::poll`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Align, Color32, Layout, RichText};
use mde_egui::Style;

use mde_cosmic_applet::{lighthouse_health_from_snapshot, LighthouseHealth};

/// The world-readable mesh-status snapshot the root timer writes. The shell reads
/// peers + lighthouse health from it exactly like the panel client — the desktop
/// user can't read the root-only replicated peer directory, so this JSON is the
/// read path.
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// Poll cadence — a peer join/leave or a lighthouse health flip surfaces within
/// this window. Matches the panel client + the Fleet datacenter poll; the read is
/// a cheap local file scan, so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status dot, drawn as the shared glyph the rest of the platform
/// uses (the datacenter rows, the panel pip) rather than a hand-rolled painter
/// circle with literal metrics — so the dot reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

// ──────────────────────────── projected view ────────────────────────────

/// The chrome bar's live mesh summary, folded from the mesh-status snapshot.
/// Pure data — parsed without egui/IO/GPU, so it's unit-tested directly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MeshSummary {
    /// Peers in the directory (every node the snapshot names).
    peers_total: usize,
    /// Peers currently `presence == "online"`.
    peers_online: usize,
    /// Worst-of lighthouse health (the mesh "Status" verdict) — reused from the
    /// panel/applet model so the bar can't diverge from the fleet's verdict.
    health: LighthouseHealth,
    /// `healthy` lighthouses behind `health` (for the Status tooltip).
    lh_healthy: usize,
    /// `total` lighthouses behind `health` (for the Status tooltip).
    lh_total: usize,
    /// `true` once a snapshot has been parsed — distinguishes "no snapshot yet"
    /// (the connecting/loading state) from a parsed-but-empty mesh.
    seen: bool,
}

impl Default for MeshSummary {
    /// The pre-first-read state: nothing seen yet (drives the "Connecting…"
    /// loading state). `LighthouseHealth` has no `Default`, so this is hand-rolled.
    fn default() -> Self {
        Self {
            peers_total: 0,
            peers_online: 0,
            health: LighthouseHealth::None,
            lh_healthy: 0,
            lh_total: 0,
            seen: false,
        }
    }
}

impl MeshSummary {
    /// Fold the mesh-status snapshot JSON into the bar's summary. A missing /
    /// garbage snapshot yields the honest unseen summary (drives "Connecting…"),
    /// never a panic — mirroring the panel client's tolerance.
    fn from_snapshot(snapshot: &str) -> Self {
        // The worst-of lighthouse verdict is the reused LIGHTHOUSE-7 parser.
        let (health, lh_healthy, lh_total) = lighthouse_health_from_snapshot(snapshot);
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
            lh_healthy,
            lh_total,
            seen: true,
        }
    }
}

// ──────────────────────────── slot view mapping ───────────────────────────

/// One rendered slot: a tone dot, a value string, its colour, and a hover
/// tooltip. The model→slot mapping is pure so it's unit-tested (a degraded mesh
/// must map to a `DANGER` dot + "Degraded", etc.).
struct SlotView {
    dot: Color32,
    value: String,
    value_color: Color32,
    tooltip: Option<String>,
}

impl SlotView {
    /// The pre-first-snapshot loading state, shared by every live slot.
    fn connecting() -> Self {
        Self {
            dot: Style::TEXT_DIM,
            value: "Connecting…".to_string(),
            value_color: Style::TEXT_DIM,
            tooltip: Some("Reading mesh status…".to_string()),
        }
    }
}

/// The Peers slot: `online/total` with a tone (green all-online, amber some-away),
/// an honest "No peers" when the directory is empty, and "Connecting…" before the
/// first snapshot.
fn peers_view(s: &MeshSummary) -> SlotView {
    if !s.seen {
        return SlotView::connecting();
    }
    if s.peers_total == 0 {
        return SlotView {
            dot: Style::TEXT_DIM,
            value: "No peers".to_string(),
            value_color: Style::TEXT_DIM,
            tooltip: Some("No peers in the directory yet.".to_string()),
        };
    }
    let all_online = s.peers_online == s.peers_total;
    let away = s.peers_total.saturating_sub(s.peers_online);
    SlotView {
        dot: if all_online { Style::OK } else { Style::WARN },
        value: format!("{}/{}", s.peers_online, s.peers_total),
        value_color: if all_online { Style::TEXT } else { Style::WARN },
        tooltip: Some(if all_online {
            format!("Peers: all {} online.", s.peers_total)
        } else {
            format!(
                "Peers: {}/{} online — {away} away.",
                s.peers_online, s.peers_total
            )
        }),
    }
}

/// The Status slot: the worst-of lighthouse verdict — green "Healthy", red
/// "Degraded", a dim "No lighthouses" when none are in view, and "Connecting…"
/// before the first snapshot.
fn status_view(s: &MeshSummary) -> SlotView {
    if !s.seen {
        return SlotView::connecting();
    }
    match s.health {
        LighthouseHealth::AllHealthy => SlotView {
            dot: Style::OK,
            value: "Healthy".to_string(),
            value_color: Style::TEXT,
            tooltip: s.health.tooltip(s.lh_healthy, s.lh_total),
        },
        LighthouseHealth::Degraded => SlotView {
            dot: Style::DANGER,
            value: "Degraded".to_string(),
            value_color: Style::DANGER,
            tooltip: s.health.tooltip(s.lh_healthy, s.lh_total),
        },
        LighthouseHealth::None => SlotView {
            dot: Style::TEXT_DIM,
            value: "No lighthouses".to_string(),
            value_color: Style::TEXT_DIM,
            tooltip: Some("No lighthouses in view.".to_string()),
        },
    }
}

/// The Sessions slot: honest "No session" until `mde-vdi` connects a live VM
/// desktop (a later unit) and drives this slot. Never a fabricated count (§7) —
/// the same truth the collapsed session empty state shows.
fn session_view() -> SlotView {
    SlotView {
        dot: Style::TEXT_DIM,
        value: "No session".to_string(),
        value_color: Style::TEXT_DIM,
        tooltip: Some(
            "Connect a desktop — your VM session renders here (mde-vdi, a later unit).".to_string(),
        ),
    }
}

// ──────────────────────────── the chrome state ────────────────────────────

/// The chrome bar's live state: the projected mesh summary plus the small IO
/// context to refresh it on the shared cadence.
pub(crate) struct ChromeState {
    /// The world-readable snapshot path (resolved once).
    snapshot_path: PathBuf,
    /// The latest projection. Unseen until the first snapshot lands (drives the
    /// "Connecting…" state).
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
    /// has elapsed, then keep the repaint heartbeat alive so a peer join/leave or a
    /// lighthouse flip surfaces without input. Cheap enough to call every frame —
    /// it self-gates. A missing / unreadable snapshot yields the unseen summary
    /// (honest "Connecting…"), never a panic.
    fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snapshot = std::fs::read_to_string(&self.snapshot_path).unwrap_or_default();
            self.summary = MeshSummary::from_snapshot(&snapshot);
        }
        ctx.request_repaint_after(REFRESH);
    }
}

/// Render the chrome bar's contents inside a top panel. Polls the live mesh
/// summary (self-gating on the shared cadence), draws the brand mark + the three
/// live slots, and returns `true` when the Expand/Collapse toggle was clicked this
/// frame.
pub(crate) fn show(ui: &mut egui::Ui, chrome: &mut ChromeState, expanded: bool) -> bool {
    chrome.poll(ui.ctx());

    let slots = [
        ("Peers", peers_view(&chrome.summary)),
        ("Sessions", session_view()),
        ("Status", status_view(&chrome.summary)),
    ];

    let mut toggled = false;
    ui.horizontal_centered(|ui| {
        // Brand mark — keeps the bar identifiable when a session is fullscreen.
        ui.label(
            RichText::new("MCNF")
                .color(Style::ACCENT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_M);

        for (i, (name, view)) in slots.iter().enumerate() {
            if i > 0 {
                ui.add_space(Style::SP_S);
                ui.label(RichText::new("·").color(Style::BORDER).size(Style::BODY));
                ui.add_space(Style::SP_S);
            }
            draw_slot(ui, name, view);
        }

        // Expand / Collapse, pinned to the right edge.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if expanded { "Collapse" } else { "Expand" };
            if ui.button(label).clicked() {
                toggled = true;
            }
        });
    });
    toggled
}

/// One status slot: a tone dot, the slot name, and its live value (with a hover
/// tooltip). All colour/size read `Style`.
fn draw_slot(ui: &mut egui::Ui, name: &str, view: &SlotView) {
    ui.label(RichText::new(DOT).color(view.dot).size(Style::SMALL));
    ui.add_space(Style::SP_XS);
    ui.label(RichText::new(name).color(Style::TEXT).size(Style::SMALL));
    ui.add_space(Style::SP_XS);
    let resp = ui.label(
        RichText::new(&view.value)
            .color(view.value_color)
            .size(Style::SMALL),
    );
    if let Some(tip) = &view.tooltip {
        let _ = resp.on_hover_text(tip.clone());
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
        // Every live slot loads honestly, never blank.
        for v in [peers_view(&s), status_view(&s)] {
            assert_eq!(v.value, "Connecting…");
            assert_eq!(v.dot, Style::TEXT_DIM);
        }
    }

    #[test]
    fn garbage_or_missing_snapshot_stays_unseen() {
        for bad in ["", "not json", "{}", r#"{"network":{}}"#] {
            let s = MeshSummary::from_snapshot(bad);
            assert!(!s.seen, "{bad:?} must not read as a live mesh");
            assert_eq!(peers_view(&s).value, "Connecting…");
        }
    }

    #[test]
    fn peers_count_folds_total_and_online() {
        // Two lighthouses online + the workstation offline → 2/3 online.
        let s = MeshSummary::from_snapshot(&snapshot("online", "online", "offline"));
        assert!(s.seen);
        assert_eq!((s.peers_online, s.peers_total), (2, 3));
        let v = peers_view(&s);
        assert_eq!(v.value, "2/3");
        // Some away → amber, not green and not red.
        assert_eq!(v.dot, Style::WARN);
        assert!(v.tooltip.is_some_and(|t| t.contains("away")));
    }

    #[test]
    fn peers_all_online_is_green() {
        let s = MeshSummary::from_snapshot(&snapshot("online", "online", "online"));
        assert_eq!((s.peers_online, s.peers_total), (3, 3));
        let v = peers_view(&s);
        assert_eq!(v.value, "3/3");
        assert_eq!(v.dot, Style::OK);
        assert_eq!(v.value_color, Style::TEXT);
    }

    #[test]
    fn empty_directory_is_no_peers_not_connecting() {
        // A parsed snapshot with an empty node list is "seen" → honest empty state,
        // distinct from the pre-read "Connecting…".
        let s = MeshSummary::from_snapshot(r#"{"nodes":[],"network":{"lighthouse_ips":[]}}"#);
        assert!(s.seen);
        let v = peers_view(&s);
        assert_eq!(v.value, "No peers");
        assert_eq!(v.dot, Style::TEXT_DIM);
    }

    #[test]
    fn status_maps_worst_of_lighthouse_health() {
        // All lighthouses up → green "Healthy".
        let up = MeshSummary::from_snapshot(&snapshot("online", "online", "offline"));
        assert_eq!(up.health, LighthouseHealth::AllHealthy);
        let v = status_view(&up);
        assert_eq!(v.value, "Healthy");
        assert_eq!(v.dot, Style::OK);
        assert!(v.tooltip.is_some_and(|t| t.contains("2/2")));

        // Any lighthouse down → red "Degraded" (worst-of).
        let deg = MeshSummary::from_snapshot(&snapshot("online", "idle", "online"));
        assert_eq!(deg.health, LighthouseHealth::Degraded);
        let v = status_view(&deg);
        assert_eq!(v.value, "Degraded");
        assert_eq!(v.dot, Style::DANGER);
        assert_eq!(v.value_color, Style::DANGER);
    }

    #[test]
    fn status_with_no_lighthouses_is_dim() {
        let s = MeshSummary::from_snapshot(
            r#"{"nodes":[{"hostname":"ws","overlay_ip":"10.42.0.50","presence":"online","role":"workstation"}],"network":{"lighthouse_ips":[]}}"#,
        );
        assert!(s.seen);
        assert_eq!(s.health, LighthouseHealth::None);
        let v = status_view(&s);
        assert_eq!(v.value, "No lighthouses");
        assert_eq!(v.dot, Style::TEXT_DIM);
    }

    #[test]
    fn session_slot_is_an_honest_empty_state() {
        // No live session signal yet — never a fabricated count (§7).
        let v = session_view();
        assert_eq!(v.value, "No session");
        assert_eq!(v.dot, Style::TEXT_DIM);
        assert!(v.tooltip.is_some_and(|t| t.contains("mde-vdi")));
    }

    #[test]
    fn chrome_state_defaults_to_the_snapshot_path_unseen() {
        let c = ChromeState::default();
        assert_eq!(c.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!c.summary.seen);
        assert!(c.last_poll.is_none());
    }
}
