//! PLANES-20 — the **Fleet rollup** dashboard (Fleet plane), absorbing
//! OBS-6 + ENT-8.
//!
//! Role-grouped fleet cards over `mackesd fleet-status --json` (W86): each
//! card is a role with its member count, per-state health breakdown, and
//! the group's worst health as the headline badge. CLI parity with
//! `mackesd fleet-status`.
//!
//! Build-now-defer-visual: the JSON projection is pure + unit-tested; the
//! live-map centerpiece (W81) + drill-down-into-Peers (W87) + the on-Cosmic
//! `/preview` are the deferred tail.

use std::collections::HashMap;

use cosmic::iced::widget::{column, container, row, scrollable, text};
use cosmic::iced::{Length, Padding, Task};
use cosmic::Element;
use mde_theme::{EmptyState, Icon};
use serde::Deserialize;

use mde_theme::LoadState;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{
    empty_state, load_state_chrome, load_state_pill, panel_container, status_badge, BadgeSeverity,
};
use crate::panels::fleet_settings::run_mackesd;
use crate::panels::peers::{parse_directory, PeerRow};
use crate::panels::peers_map::{layout, read_latency_cache, MapNode, MapProgram};

/// One role group (mirrors `mackesd_core::fleet_rollup::RoleRollup`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RoleGroup {
    pub role: String,
    pub total: usize,
    pub healthy: usize,
    pub degraded: usize,
    pub unreachable: usize,
    pub unknown: usize,
    pub worst_health: String,
}

/// The `mackesd fleet-status --json` document.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Rollup {
    pub total: usize,
    pub groups: Vec<RoleGroup>,
}

/// Parse the backend JSON, tolerant of an empty/garbled body.
#[must_use]
pub fn parse_rollup(raw: &str) -> Rollup {
    serde_json::from_str(raw).unwrap_or_default()
}

/// This node's hostname (anchors the live map; "localhost" on failure).
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

/// Map a worst-health string to its badge severity.
#[must_use]
pub fn health_severity(worst: &str) -> BadgeSeverity {
    match worst {
        "healthy" => BadgeSeverity::Success,
        "unreachable" | "degraded" => BadgeSeverity::Warning,
        _ => BadgeSeverity::Neutral,
    }
}

/// The Fleet-rollup panel state.
#[derive(Debug, Clone)]
pub struct FleetRollupPanel {
    pub rollup: Rollup,
    /// W81 — peer rows feeding the live-map centerpiece.
    pub rows: Vec<PeerRow>,
    /// W81 — host→RTT for the map's edge labels/spring lengths.
    pub rtt: HashMap<String, Option<f64>>,
    /// W81 — this node, anchored at the map's center.
    pub self_hostname: String,
    /// MOTION-NET-1 — the canonical async lifecycle state, replacing the old
    /// `loaded`/`load_error`/`busy` flag triple. A first load shows the
    /// `Loading` chrome; a refresh keeps the existing cards visible
    /// (`Refreshing{stale}`); a `mackesd fleet-status --json` failure renders
    /// `Failed` (never the misleading "No enrolled nodes yet" empty state).
    pub load: LoadState,
    pub status: String,
    /// MOTION-NET-2 — the clock the first-load skeleton shimmer reads. A
    /// `ShimmerTick` (registered only while the load is busy) refreshes it each
    /// frame so the shimmer sweep advances; at rest it's never ticked (no idle
    /// animation — MOTION-PERF-1).
    pub shimmer_now: std::time::Instant,
}

impl Default for FleetRollupPanel {
    fn default() -> Self {
        Self {
            rollup: Rollup::default(),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: String::new(),
            load: LoadState::default(),
            status: String::new(),
            shimmer_now: std::time::Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        rollup: Rollup,
        rows: Vec<PeerRow>,
        rtt: HashMap<String, Option<f64>>,
        self_hostname: String,
    },
    /// EFF-45 — emitted when `mackesd fleet-status --json` fails (I/O /
    /// non-zero exit / parse error) so the view can render the error state.
    LoadError(String),
    RefreshClicked,
    /// MOTION-NET-2 — per-frame tick that advances the first-load skeleton
    /// shimmer. Registered only while the load is busy ([`subscription`]).
    ShimmerTick,
}

impl FleetRollupPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                // EFF-45: distinguish a real fleet-status failure from an
                // empty fleet. `run_mackesd` returns Err on non-zero exit or
                // I/O error; parse failure on non-empty output is also an
                // error (the source exists but is unreadable).
                let rollup_result = run_mackesd(&["fleet-status".into(), "--json".into()])
                    .await
                    .and_then(|out| {
                        let t = out.trim();
                        if t.is_empty() {
                            // Empty output = no nodes yet, legitimately empty.
                            Ok(Rollup::default())
                        } else {
                            serde_json::from_str::<Rollup>(t)
                                .map_err(|e| format!("parse fleet-status: {e}"))
                        }
                    });
                let rollup = match rollup_result {
                    Ok(r) => r,
                    Err(e) => return Message::LoadError(e),
                };
                // W81 — the live-map data: the same directory the Peers
                // Front Door reads, plus the mesh-latency cache.
                // Junk-tolerant: a missing or unparseable peers list degrades
                // to an empty map overlay, not an error.
                let rows = run_mackesd(&["peers".into(), "--json".into()])
                    .await
                    .ok()
                    .and_then(|out| parse_directory(&out).ok())
                    .unwrap_or_default();
                let rtt = read_latency_cache();
                let self_hostname = hostname();
                Message::Loaded {
                    rollup,
                    rows,
                    rtt,
                    self_hostname,
                }
            },
            crate::Message::FleetRollup,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                rollup,
                rows,
                rtt,
                self_hostname,
            } => {
                self.rollup = rollup;
                self.rows = rows;
                self.rtt = rtt;
                self.self_hostname = self_hostname;
                self.load = self.load.clone().on_loaded();
                self.status.clear();
                Task::none()
            }
            Message::LoadError(e) => {
                // EFF-45 / MOTION-NET-1 — fleet-status failure is a `Failed`
                // load, not an empty fleet.
                self.load = self.load.clone().on_error(e);
                Task::none()
            }
            Message::RefreshClicked => {
                if self.load.is_busy() {
                    return Task::none();
                }
                // MOTION-NET-1 — a reload over existing cards is a refresh
                // (keeps them visible); a first load shows the Loading chrome.
                self.load = self.load.clone().begin_load();
                self.status = "Refreshing…".into();
                Self::load()
            }
            Message::ShimmerTick => {
                // MOTION-NET-2 — advance the first-load skeleton's shimmer clock.
                self.shimmer_now = std::time::Instant::now();
                Task::none()
            }
        }
    }

    /// MOTION-NET-2 — the skeleton-shimmer animation tick, registered by
    /// `App::subscription` ONLY while a first load is in flight with no content
    /// to show (so the shimmer animates exactly when the skeleton is visible;
    /// an idle or content-bearing panel runs no loop — MOTION-PERF-1). One tick
    /// per ~60 ms frame keeps the sweep smooth.
    #[must_use]
    pub fn shimmer_subscription() -> cosmic::iced::Subscription<crate::Message> {
        cosmic::iced::time::every(std::time::Duration::from_millis(60))
            .map(|_| crate::Message::FleetRollup(Message::ShimmerTick))
    }

    /// MOTION-NET-2 — true while the first-load skeleton is on screen (a load is
    /// in flight and there's nothing to show yet), so the App can gate the
    /// shimmer tick on it.
    #[must_use]
    pub fn skeleton_visible(&self) -> bool {
        self.load.is_busy() && !self.load.has_content()
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let refresh = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.load.is_busy()).then_some(crate::Message::FleetRollup(Message::RefreshClicked)),
            palette,
        );

        // MOTION-NET-1 — the canonical async chrome: a first `Loading`, an
        // `Offline`, and a `Failed` fleet-status run each render their distinct
        // non-content state (Failed → error+retry, never the misleading "No
        // enrolled nodes yet" empty state). Content-bearing states (Loaded /
        // Degraded / Refreshing-over-stale) fall through to the data view.
        // MOTION-NET-2 — a first `Loading` (nothing to show yet) paints the
        // shared skeleton + shimmer placeholder, so a slow fleet-status run shows
        // layout-shaped structure instead of a blank panel. `shimmer_now` is
        // advanced by `ShimmerTick` while busy; `reduce_motion` ⇒ static grey.
        if let Some(chrome) = load_state_chrome(
            &self.load,
            palette,
            density,
            self.shimmer_now,
            crate::live_theme::reduce_motion(),
            || crate::Message::FleetRollup(Message::RefreshClicked),
        ) {
            return panel_container(chrome, density);
        }

        if self.rollup.groups.is_empty() {
            let state = EmptyState::with_cta(
                "No enrolled nodes yet",
                "Once peers enroll, this dashboard groups them by role with each \
                 group's member count and worst health (PLANES-20).",
                "Refresh",
            )
            .with_icon(Icon::Fleet);
            return panel_container(
                empty_state(state, palette, || {
                    crate::Message::FleetRollup(Message::RefreshClicked)
                }),
                density,
            );
        }

        let mut cards = column![].spacing(10);
        for g in &self.rollup.groups {
            let breakdown = text(format!(
                "{} healthy · {} degraded · {} unreachable · {} unknown",
                g.healthy, g.degraded, g.unreachable, g.unknown
            ))
            .size(12);
            // W87 — drill-down: open the Peers Front Door filtered to this
            // role. The directory filter matches the role token.
            let drill = variant_button(
                "View peers ›",
                ButtonVariant::Ghost,
                Some(crate::Message::DrillToPeers(g.role.clone())),
                palette,
            );
            cards = cards.push(
                container(
                    row![
                        column![
                            text(format!(
                                "{}  ({} member{})",
                                g.role,
                                g.total,
                                if g.total == 1 { "" } else { "s" }
                            ))
                            .size(16),
                            breakdown,
                        ]
                        .spacing(2),
                        status_badge(
                            g.worst_health.clone(),
                            health_severity(&g.worst_health),
                            palette
                        ),
                        cosmic::iced::widget::Space::new().width(Length::Fill),
                        drill,
                    ]
                    .spacing(12)
                    .align_y(cosmic::iced::Alignment::Center),
                )
                .padding(Padding::from(12)),
            );
        }

        // W81 — the live-map centerpiece: the same PD-7 force-graph the
        // Peers panel + wallpaper render, fed by the directory + RTT cache.
        // Sits above the role cards as the dashboard's focal point. (Node
        // click pre-selects the peer; the cards' "View peers ›" navigates.)
        let centerpiece: Element<'_, crate::Message> = if self.rows.is_empty() {
            cosmic::iced::widget::Space::new()
                .height(Length::Fixed(0.0))
                .into()
        } else {
            let nodes: Vec<MapNode> = self
                .rows
                .iter()
                .map(|r| MapNode {
                    hostname: r.hostname.clone(),
                    presence: r.presence.clone(),
                    rtt_ms: self.rtt.get(&r.hostname).copied().flatten(),
                    is_self: r.hostname == self.self_hostname,
                    // PD-7/L18 — the rollup map is a static overview; no live
                    // flow particles here (the Peers Map drives those).
                    flow: 0.0,
                })
                .collect();
            let positions = layout(&nodes);
            // `MapProgram` implements `canvas::Program` for the stock
            // `cosmic::iced::Theme`, so the canvas is a stock-themed element;
            // `themer` bridges it into the surrounding `cosmic::Theme` tree.
            // The program ignores the passed theme (it paints from `palette`),
            // so `None` (Base default) carries no styling decision.
            let canvas: cosmic::iced::Element<'_, crate::Message, cosmic::iced::Theme> =
                cosmic::iced::widget::canvas(MapProgram {
                    nodes,
                    positions,
                    palette,
                    flow_phase: 0.0,
                })
                .width(Length::Fill)
                .height(Length::Fixed(260.0))
                .into();
            cosmic::iced::widget::themer(None, canvas).into()
        };

        // MOTION-NET-1 — the non-motion status affordance: a refresh over
        // existing cards reads as a "Refreshing…" pill in the header, legible
        // with animation disabled. (`load_state_pill` covers every state; the
        // settled `Loaded` pill confirms the data is current.)
        let status_pill = load_state_pill(&self.load, palette);

        panel_container(
            column![
                row![
                    text(format!("Fleet — {} node(s)", self.rollup.total)).size(20),
                    status_pill,
                    cosmic::iced::widget::Space::new().width(Length::Fill),
                    refresh
                ]
                .spacing(12)
                .align_y(cosmic::iced::Alignment::Center),
                centerpiece,
                scrollable(cards).height(Length::Fill),
            ]
            .spacing(16)
            .width(Length::Fill)
            .into(),
            density,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rollup_reads_groups() {
        let raw = r#"{
            "total": 4,
            "groups": [
                {"role":"host","total":1,"healthy":1,"degraded":0,"unreachable":0,"unknown":0,"worst_health":"healthy"},
                {"role":"peer","total":3,"healthy":1,"degraded":1,"unreachable":1,"unknown":0,"worst_health":"unreachable"}
            ]
        }"#;
        let r = parse_rollup(raw);
        assert_eq!(r.total, 4);
        assert_eq!(r.groups.len(), 2);
        assert_eq!(r.groups[1].role, "peer");
        assert_eq!(r.groups[1].worst_health, "unreachable");
    }

    #[test]
    fn parse_rollup_tolerates_garbage() {
        assert_eq!(parse_rollup("nope"), Rollup::default());
        assert_eq!(parse_rollup(""), Rollup::default());
    }

    #[test]
    fn health_severity_maps_states() {
        assert_eq!(health_severity("healthy"), BadgeSeverity::Success);
        assert_eq!(health_severity("degraded"), BadgeSeverity::Warning);
        assert_eq!(health_severity("unreachable"), BadgeSeverity::Warning);
        assert_eq!(health_severity("unknown"), BadgeSeverity::Neutral);
    }

    #[test]
    fn loaded_sets_rollup_and_settles_load_state() {
        // MOTION-NET-1 — a refresh keeps the prior data visible
        // (Refreshing{stale}), then `Loaded` settles it back to current.
        let mut p = FleetRollupPanel::new();
        p.load = LoadState::Loaded; // already had data
        let _ = p.update(Message::RefreshClicked);
        assert_eq!(
            p.load,
            LoadState::Refreshing { stale: true },
            "refresh keeps cards visible"
        );
        let _ = p.update(Message::Loaded {
            rollup: parse_rollup(
                r#"{"total":1,"groups":[{"role":"host","total":1,"worst_health":"healthy"}]}"#,
            ),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: "pine".into(),
        });
        assert_eq!(p.load, LoadState::Loaded);
        assert!(!p.load.is_busy());
        assert_eq!(p.rollup.total, 1);
        assert_eq!(p.self_hostname, "pine");
    }

    #[test]
    fn first_load_then_error_renders_failed_not_empty() {
        // MOTION-NET-1 / EFF-45 — a fleet-status failure is a Failed load, so
        // the view paints the error+retry chrome instead of "No nodes yet".
        let mut p = FleetRollupPanel::new();
        assert_eq!(p.load, LoadState::Idle);
        let _ = p.update(Message::RefreshClicked);
        assert_eq!(p.load, LoadState::Loading, "first load with no prior data");
        let _ = p.update(Message::LoadError("fleet-status exit 1".into()));
        assert!(p.load.is_failed());
        assert_eq!(p.load.error(), Some("fleet-status exit 1"));
    }

    #[test]
    fn skeleton_visible_only_during_a_first_load() {
        // MOTION-NET-2 — the shimmer tick is gated on this: the skeleton shows
        // exactly when a load is in flight with nothing to display yet.
        let mut p = FleetRollupPanel::new();
        assert!(!p.skeleton_visible(), "idle ⇒ no skeleton");
        let _ = p.update(Message::RefreshClicked); // Idle → Loading
        assert!(p.skeleton_visible(), "first load ⇒ skeleton");
        // A refresh over existing data keeps the cards (stale), not a skeleton.
        p.load = LoadState::Loaded;
        let _ = p.update(Message::RefreshClicked); // Loaded → Refreshing{stale}
        assert_eq!(p.load, LoadState::Refreshing { stale: true });
        assert!(
            !p.skeleton_visible(),
            "stale-refresh keeps content, no skeleton"
        );
        // Settled.
        p.load = LoadState::Loaded;
        assert!(!p.skeleton_visible());
    }

    #[test]
    fn shimmer_tick_advances_the_clock() {
        // MOTION-NET-2 — a ShimmerTick refreshes the skeleton's animation clock
        // so the sweep progresses frame to frame.
        let mut p = FleetRollupPanel::new();
        let before = p.shimmer_now;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = p.update(Message::ShimmerTick);
        assert!(p.shimmer_now > before, "tick must advance the clock");
    }
}
