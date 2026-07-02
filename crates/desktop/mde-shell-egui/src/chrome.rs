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

use mde_seat::{Probe, SeatSnapshot};

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

// ───────────────────── read-only seat status icons (E12-15) ─────────────────
//
// Lock 3: the chrome bar carries read-only iconic status only — Signal · Bluetooth
// · Volume — and no controls; ALL host-control interaction lives on
// `Surface::System`. These fold the SAME `mde-seat` snapshot the System surface
// renders (the shell polls one `Seat`), so the icon and the panel can't diverge.
// Each is the familiar dot+name+value slot, so the read-only status reads exactly
// like the mesh slots beside it.

/// The Signal slot: mesh reachability, reused from the chrome's own mesh summary
/// (the presence source lock 3 points the Signal icon at) — online when any peer
/// is reachable, an honest "isolated" when the directory is populated but nobody
/// answers, "no peers" when it's empty, and "Connecting…" before the first read.
fn signal_view(s: &MeshSummary) -> SlotView {
    if !s.seen {
        return SlotView::connecting();
    }
    if s.peers_total == 0 {
        return SlotView {
            dot: Style::TEXT_DIM,
            value: "no peers".to_string(),
            value_color: Style::TEXT_DIM,
            tooltip: Some("Signal — no peers in the mesh directory yet.".to_string()),
        };
    }
    if s.peers_online == 0 {
        return SlotView {
            dot: Style::WARN,
            value: "isolated".to_string(),
            value_color: Style::WARN,
            tooltip: Some("Signal — the directory is populated but no peer is online.".to_string()),
        };
    }
    SlotView {
        dot: Style::OK,
        value: "online".to_string(),
        value_color: Style::TEXT,
        tooltip: Some(format!(
            "Signal — {}/{} peers reachable on the mesh.",
            s.peers_online, s.peers_total
        )),
    }
}

/// The Bluetooth slot: adapter power + connected-device count from the seat
/// snapshot. `Absent` (no `BlueZ` / no bus — the build-host case) reads as a dim
/// "off" carrying the honest reason; never a fabricated radio state (§7).
fn bluetooth_view(seat: Option<&SeatSnapshot>) -> SlotView {
    match seat.map(|s| &s.bluetooth) {
        None => seat_reading("Bluetooth"),
        Some(Probe::Absent { reason, .. }) => seat_unavailable(reason),
        Some(Probe::Present(bt)) => {
            if bt.any_adapter_powered() {
                let connected = bt.connected_devices();
                let value = if connected > 0 {
                    format!("on \u{00B7} {connected}")
                } else {
                    "on".to_string()
                };
                SlotView {
                    dot: Style::OK,
                    value,
                    value_color: Style::TEXT,
                    tooltip: Some(format!(
                        "Bluetooth on — {connected} device(s) connected of {} known.",
                        bt.devices.len()
                    )),
                }
            } else {
                SlotView {
                    dot: Style::TEXT_DIM,
                    value: "off".to_string(),
                    value_color: Style::TEXT_DIM,
                    tooltip: Some("Bluetooth adapter powered off.".to_string()),
                }
            }
        }
    }
}

/// The Volume slot: master mute / level from the seat snapshot's mixer. `Absent`
/// (the `PipeWire` binding lands in E12-16, so this is the build-host state today)
/// reads as a dim "unavailable" carrying the honest reason; never a fake level.
fn volume_view(seat: Option<&SeatSnapshot>) -> SlotView {
    match seat.map(|s| &s.mixer) {
        None => seat_reading("Volume"),
        Some(Probe::Absent { reason, .. }) => seat_unavailable(reason),
        Some(Probe::Present(m)) => {
            if m.master.muted {
                SlotView {
                    dot: Style::WARN,
                    value: "muted".to_string(),
                    value_color: Style::WARN,
                    tooltip: Some("Master output muted.".to_string()),
                }
            } else {
                SlotView {
                    dot: Style::OK,
                    value: format!("{}%", m.master.volume),
                    value_color: Style::TEXT,
                    tooltip: Some(format!("Master output at {}%.", m.master.volume)),
                }
            }
        }
    }
}

/// The pre-first-snapshot seat state, shared by the Bluetooth + Volume icons.
fn seat_reading(what: &str) -> SlotView {
    SlotView {
        dot: Style::TEXT_DIM,
        value: "\u{2014}".to_string(),
        value_color: Style::TEXT_DIM,
        tooltip: Some(format!("Reading {what} status from the seat…")),
    }
}

/// A seat backend that is honestly absent on this host — a dim "unavailable"
/// carrying the typed reason (§7 / interlock 4), never a fabricated state.
fn seat_unavailable(reason: &str) -> SlotView {
    SlotView {
        dot: Style::TEXT_DIM,
        value: "unavailable".to_string(),
        value_color: Style::TEXT_DIM,
        tooltip: Some(reason.to_string()),
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

/// What the chrome bar reported this frame — the Expand/Collapse toggle and/or a
/// click on the unread indicator (NOTIFY-CHAT-6). The shell applies each; the
/// unread click routes through the one `shell/goto/chat` nav grammar so the chrome
/// has no second copy of the navigation.
#[derive(Default, Clone, Copy)]
pub(crate) struct ChromeOutcome {
    /// The Expand/Collapse toggle was clicked this frame.
    pub toggled: bool,
    /// The unread indicator was clicked — open the unified Chat surface.
    pub open_chat: bool,
}

/// Render the chrome bar's contents inside a top panel. Polls the live mesh
/// summary (self-gating on the shared cadence), draws the brand mark + the live
/// slots + the unread indicator, and reports the Expand toggle / unread click this
/// frame. `unread` is the whole-mesh Chat tally (folded alerts + clipboard clips +
/// human chat) — the ONE notification interface's badge.
pub(crate) fn show(
    ui: &mut egui::Ui,
    chrome: &mut ChromeState,
    seat: Option<&SeatSnapshot>,
    expanded: bool,
    unread: usize,
) -> ChromeOutcome {
    chrome.poll(ui.ctx());

    // The mesh slots, then the read-only seat status icons (lock 3: Signal ·
    // Bluetooth · Volume) folded from the same seat snapshot the System surface
    // renders.
    let slots = [
        ("Peers", peers_view(&chrome.summary)),
        ("Sessions", session_view()),
        ("Status", status_view(&chrome.summary)),
        ("Signal", signal_view(&chrome.summary)),
        ("BT", bluetooth_view(seat)),
        ("Vol", volume_view(seat)),
    ];

    let mut outcome = ChromeOutcome::default();
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

        // Expand / Collapse, pinned to the right edge, with the unread indicator
        // just inside it (the ONE notification interface's badge — NOTIFY-CHAT-6).
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if expanded { "Collapse" } else { "Expand" };
            if ui.button(label).clicked() {
                outcome.toggled = true;
            }
            ui.add_space(Style::SP_S);
            if chat_indicator(ui, unread).clicked() {
                outcome.open_chat = true;
            }
        });
    });
    outcome
}

/// The unread indicator — a filled dot plus the word "Chat" and, when the mesh has
/// unread notifications (folded alerts + clipboard clips + human chat, all one
/// Chat surface now), a count badge. Accent when something waits, dim when quiet;
/// always clickable so the operator can open Chat regardless. Reads the same
/// `Style` dot glyph as the status slots (no emoji-font tofu risk), returns the
/// click response (the caller routes it through `shell/goto/chat`). The tooltip is
/// honest about the count so a dim dot never reads as a fabricated "all clear".
fn chat_indicator(ui: &mut egui::Ui, unread: usize) -> egui::Response {
    let (color, tooltip) = if unread > 0 {
        (
            Style::ACCENT,
            format!("{unread} unread — open Chat (alerts, clipboard, messages)."),
        )
    } else {
        (
            Style::TEXT_DIM,
            "No unread — open Chat (alerts, clipboard, messages).".to_string(),
        )
    };
    let label = if unread > 0 {
        // Cap the badge so a firehose can't stretch the bar.
        let shown = if unread > 99 {
            "99+".to_string()
        } else {
            unread.to_string()
        };
        RichText::new(format!("{DOT} Chat {shown}"))
            .color(color)
            .size(Style::SMALL)
            .strong()
    } else {
        RichText::new(format!("{DOT} Chat"))
            .color(color)
            .size(Style::SMALL)
    };
    ui.add(egui::Button::new(label).frame(unread > 0))
        .on_hover_text(tooltip)
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
    fn chrome_outcome_defaults_to_no_action() {
        let o = ChromeOutcome::default();
        assert!(!o.toggled);
        assert!(!o.open_chat);
    }

    #[test]
    fn chrome_bar_with_the_unread_indicator_tessellates() {
        // The chrome bar (incl. the NOTIFY-CHAT-6 unread indicator) mounts + paints
        // headless with a live unread count — proving the new indicator draws real
        // geometry, the same CPU paint path the DRM runner drives.
        use mde_egui::egui::{pos2, vec2, Rect};
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut chrome = ChromeState::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 48.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("mcnf-chrome").show(ctx, |ui| {
                let outcome = show(ui, &mut chrome, None, false, 7);
                // No pointer events → neither affordance fires this frame.
                assert!(!outcome.toggled && !outcome.open_chat);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the chrome bar produced no draw primitives"
        );
    }

    #[test]
    fn chrome_state_defaults_to_the_snapshot_path_unseen() {
        let c = ChromeState::default();
        assert_eq!(c.snapshot_path, PathBuf::from(SNAPSHOT_PATH));
        assert!(!c.summary.seen);
        assert!(c.last_poll.is_none());
    }

    // ── the read-only seat status icons (E12-15, lock 3) ─────────────────────

    use mde_seat::{Backend, BtAdapter, BtDevice, BtStatus, MixerStatus};

    /// A typed-absent probe of any section (the honest build-host state).
    fn absent<T>() -> Probe<T> {
        Probe::Absent {
            backend: Backend::PipeWire,
            reason: "PipeWire is not available: test".to_string(),
        }
    }

    /// A snapshot with a chosen Bluetooth, every other section Absent. The mixer is
    /// always Absent here: its `MixerStatus` can't be constructed off the shell
    /// crate (the `StripOrigin` field type isn't re-exported), and Absent is the
    /// real build-host mixer state until E12-16 anyway — mde-seat's own tests cover
    /// a Present mixer fold.
    fn seat_snapshot(bluetooth: Probe<BtStatus>) -> SeatSnapshot {
        let mixer: Probe<MixerStatus> = absent();
        SeatSnapshot {
            bluetooth,
            batteries: absent(),
            power: absent(),
            displays: absent(),
            backlights: absent(),
            mixer,
            ddc: absent(),
        }
    }

    #[test]
    fn seat_icons_read_unavailable_when_the_backend_is_absent() {
        // The build-host reality: no PipeWire mixer, no BlueZ — the icons must read
        // an honest dim "unavailable", never a fabricated on/level (§7).
        let snap = seat_snapshot(absent());
        let bt = bluetooth_view(Some(&snap));
        assert_eq!(bt.value, "unavailable");
        assert_eq!(bt.dot, Style::TEXT_DIM);
        assert!(bt.tooltip.is_some_and(|t| t.contains("not available")));
        let vol = volume_view(Some(&snap));
        assert_eq!(vol.value, "unavailable");
        assert_eq!(vol.dot, Style::TEXT_DIM);
    }

    #[test]
    fn seat_icons_are_a_dim_dash_before_the_first_snapshot() {
        // No snapshot yet (pre-poll) is distinct from Absent — a dim placeholder,
        // never blank.
        assert_eq!(bluetooth_view(None).value, "\u{2014}");
        assert_eq!(volume_view(None).value, "\u{2014}");
    }

    #[test]
    fn bluetooth_icon_folds_powered_adapter_and_connected_count() {
        let bt = BtStatus {
            adapters: vec![BtAdapter {
                path: "/org/bluez/hci0".into(),
                name: "hci0".into(),
                powered: true,
                discovering: false,
            }],
            devices: vec![BtDevice {
                path: "/org/bluez/hci0/dev_x".into(),
                alias: "MX Keys".into(),
                paired: true,
                connected: true,
                trusted: true,
                battery_percent: Some(80),
                icon: None,
            }],
        };
        let v = bluetooth_view(Some(&seat_snapshot(Probe::Present(bt))));
        assert_eq!(v.value, "on \u{00B7} 1");
        assert_eq!(v.dot, Style::OK);

        // A powered-off adapter with nothing connected reads a dim "off".
        let bare = BtStatus {
            adapters: vec![BtAdapter {
                path: "/org/bluez/hci0".into(),
                name: "hci0".into(),
                powered: false,
                discovering: false,
            }],
            devices: Vec::new(),
        };
        let off = bluetooth_view(Some(&seat_snapshot(Probe::Present(bare))));
        assert_eq!(off.value, "off");
        assert_eq!(off.dot, Style::TEXT_DIM);
    }

    #[test]
    fn signal_icon_folds_mesh_reachability_from_the_summary() {
        // Any peer online → green "online".
        let up = MeshSummary::from_snapshot(&snapshot("online", "online", "offline"));
        let v = signal_view(&up);
        assert_eq!(v.value, "online");
        assert_eq!(v.dot, Style::OK);

        // A populated directory with nobody online → amber "isolated".
        let down = MeshSummary::from_snapshot(&snapshot("offline", "offline", "offline"));
        let v = signal_view(&down);
        assert_eq!(v.value, "isolated");
        assert_eq!(v.dot, Style::WARN);

        // Before the first snapshot → the shared connecting state.
        assert_eq!(signal_view(&MeshSummary::default()).value, "Connecting…");
    }
}
