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

use cosmic::iced::widget::{column, row, scrollable, text};
use cosmic::iced::{Length, Task};
use cosmic::Element;
use mde_theme::{EmptyState, Icon};
use serde::Deserialize;

use mde_theme::LoadState;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{
    empty_state, load_state_chrome, load_state_pill, panel_container, selectable_card,
    staggered_reveal, status_badge, BadgeSeverity,
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
    /// MOTION-FEEDBACK-2 — the role card the operator has selected (its `role`
    /// token), painted with the Carbon -selected accent wash + selection rail.
    /// `None` = nothing selected. A click toggles it.
    pub selected_role: Option<String>,
    /// MOTION-FEEDBACK-2 — when the staggered card-reveal cascade began (set on
    /// each `Loaded`). `reveal_now - reveal_start` is the cascade's elapsed time;
    /// the `RevealTick` advances `reveal_now` only while the cascade is in flight
    /// (idle-gated — MOTION-PERF-1), then stops.
    pub reveal_start: std::time::Instant,
    pub reveal_now: std::time::Instant,
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
            selected_role: None,
            reveal_start: std::time::Instant::now(),
            reveal_now: std::time::Instant::now(),
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
    /// MOTION-FEEDBACK-2 — select (or toggle off) a role card. Paints the
    /// Carbon -selected accent wash + rail on the matching card.
    SelectRole(String),
    /// MOTION-FEEDBACK-2 — per-frame tick advancing the staggered card-reveal
    /// cascade. Registered only while the cascade is in flight (idle-gated).
    RevealTick,
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
                // MOTION-FEEDBACK-2 — kick off the staggered card reveal: the
                // cards cascade in over the next ≤500ms. App::subscription gates
                // the `RevealTick` on `revealing()`, so the loop stops the moment
                // the last card settles (no idle animation — MOTION-PERF-1).
                let now = std::time::Instant::now();
                self.reveal_start = now;
                self.reveal_now = now;
                // Drop a selection that no longer maps to a present role.
                if let Some(sel) = &self.selected_role {
                    if !self.rollup.groups.iter().any(|g| &g.role == sel) {
                        self.selected_role = None;
                    }
                }
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
            Message::SelectRole(role) => {
                // MOTION-FEEDBACK-2 — toggle selection: re-clicking the selected
                // card clears it. Selection is local view state (no reload).
                self.selected_role = if self.selected_role.as_deref() == Some(role.as_str()) {
                    None
                } else {
                    Some(role)
                };
                Task::none()
            }
            Message::RevealTick => {
                // MOTION-FEEDBACK-2 — advance the staggered reveal clock.
                self.reveal_now = std::time::Instant::now();
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

    /// MOTION-FEEDBACK-2 — elapsed ms since the staggered reveal cascade began.
    #[must_use]
    fn reveal_elapsed_ms(&self) -> u32 {
        u32::try_from(
            self.reveal_now
                .saturating_duration_since(self.reveal_start)
                .as_millis(),
        )
        .unwrap_or(u32::MAX)
    }

    /// MOTION-FEEDBACK-2 — the staggered card-reveal tick, registered by
    /// `App::subscription` ONLY while [`revealing`](Self::revealing) is true, so
    /// the cascade animates exactly while cards are arriving and the loop stops
    /// the instant the last card settles (no idle animation — MOTION-PERF-1).
    #[must_use]
    pub fn reveal_subscription() -> cosmic::iced::Subscription<crate::Message> {
        cosmic::iced::time::every(std::time::Duration::from_millis(60))
            .map(|_| crate::Message::FleetRollup(Message::RevealTick))
    }

    /// MOTION-FEEDBACK-2 — true while the card-reveal cascade is still in flight
    /// (some card hasn't finished revealing), so the App can gate the reveal
    /// tick on it. Always false under reduce-motion (the cascade collapses).
    #[must_use]
    pub fn revealing(&self) -> bool {
        // Only meaningful when content is actually on screen (not during the
        // skeleton/loading chrome).
        self.load.has_content()
            && mde_theme::stagger::is_animating(
                self.reveal_elapsed_ms(),
                crate::live_theme::reduce_motion(),
            )
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

        // MOTION-FEEDBACK-2 — selection + staggered reveal for the role-card
        // list. Each card is a `selectable_card` (Carbon -selected/-hover accent
        // wash + selection rail), wrapped in `staggered_reveal` so a freshly
        // loaded fleet cascades its cards in (capped, ≤500ms total) instead of
        // snapping. `reveal_elapsed_ms` drives the cascade; `reduce_motion` ⇒ the
        // whole list reveals at once (selection still works).
        let reveal_ms = self.reveal_elapsed_ms();
        let reduce_motion = crate::live_theme::reduce_motion();
        let mut cards = column![].spacing(10);
        for (i, g) in self.rollup.groups.iter().enumerate() {
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
            let body: Element<'_, crate::Message> = row![
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
            .align_y(cosmic::iced::Alignment::Center)
            .into();
            let selected = self.selected_role.as_deref() == Some(g.role.as_str());
            let card = selectable_card(
                body,
                selected,
                crate::Message::FleetRollup(Message::SelectRole(g.role.clone())),
                palette,
                density,
            );
            cards = cards.push(staggered_reveal(card, i, reveal_ms, reduce_motion));
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

    #[test]
    fn select_role_toggles_and_clears_on_reselect() {
        // MOTION-FEEDBACK-2 — clicking a role card selects it; re-clicking the
        // same card clears the selection; clicking another switches.
        let mut p = FleetRollupPanel::new();
        assert_eq!(p.selected_role, None);
        let _ = p.update(Message::SelectRole("host".into()));
        assert_eq!(p.selected_role.as_deref(), Some("host"));
        let _ = p.update(Message::SelectRole("lighthouse".into()));
        assert_eq!(p.selected_role.as_deref(), Some("lighthouse"));
        let _ = p.update(Message::SelectRole("lighthouse".into()));
        assert_eq!(p.selected_role, None, "re-click clears");
    }

    #[test]
    fn loaded_drops_a_selection_no_longer_present() {
        // MOTION-FEEDBACK-2 — a reload that no longer contains the selected role
        // must drop the stale selection (it can't paint a card that's gone).
        let mut p = FleetRollupPanel::new();
        p.selected_role = Some("gone".into());
        let _ = p.update(Message::Loaded {
            rollup: parse_rollup(
                r#"{"total":1,"groups":[{"role":"host","total":1,"worst_health":"healthy"}]}"#,
            ),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: "pine".into(),
        });
        assert_eq!(p.selected_role, None, "stale selection dropped");
        // A selection that still maps survives.
        p.selected_role = Some("host".into());
        let _ = p.update(Message::Loaded {
            rollup: parse_rollup(
                r#"{"total":1,"groups":[{"role":"host","total":1,"worst_health":"healthy"}]}"#,
            ),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: "pine".into(),
        });
        assert_eq!(p.selected_role.as_deref(), Some("host"));
    }

    #[test]
    fn loaded_arms_the_reveal_cascade_and_tick_advances_it() {
        // MOTION-FEEDBACK-2 — `Loaded` restarts the staggered-reveal clock so the
        // cards cascade in; the RevealTick advances the clock frame to frame.
        let mut p = FleetRollupPanel::new();
        let _ = p.update(Message::Loaded {
            rollup: parse_rollup(
                r#"{"total":1,"groups":[{"role":"host","total":1,"worst_health":"healthy"}]}"#,
            ),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: "pine".into(),
        });
        // Fresh cascade: reveal_now == reveal_start (≈0 elapsed).
        assert!(p.reveal_elapsed_ms() < 50, "cascade just started");
        let before = p.reveal_now;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = p.update(Message::RevealTick);
        assert!(p.reveal_now > before, "reveal tick advances the clock");
    }

    #[test]
    fn revealing_gates_on_cascade_in_flight_and_content() {
        // MOTION-FEEDBACK-2 / MOTION-PERF-1 — the reveal tick is gated on
        // `revealing()`: true only while content is on screen AND the cascade
        // hasn't finished, so the loop stops at rest (no idle animation).
        let mut p = FleetRollupPanel::new();
        // No content yet ⇒ never revealing (the skeleton owns the screen).
        assert!(!p.revealing(), "idle/loading ⇒ not revealing");
        let _ = p.update(Message::Loaded {
            rollup: parse_rollup(
                r#"{"total":1,"groups":[{"role":"host","total":1,"worst_health":"healthy"}]}"#,
            ),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: "pine".into(),
        });
        // Fresh load with content ⇒ cascade in flight (unless reduce-motion).
        if !crate::live_theme::reduce_motion() {
            assert!(p.revealing(), "fresh load ⇒ cascade animating");
        }
        // Past the whole cascade window ⇒ settled, tick stops.
        p.reveal_now = p.reveal_start + std::time::Duration::from_millis(1000);
        assert!(!p.revealing(), "cascade done ⇒ not revealing");
    }
}
