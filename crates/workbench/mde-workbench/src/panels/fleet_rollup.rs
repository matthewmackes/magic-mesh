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


use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{empty_state, panel_container, status_badge, BadgeSeverity};
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
#[derive(Debug, Clone, Default)]
pub struct FleetRollupPanel {
    pub rollup: Rollup,
    /// W81 — peer rows feeding the live-map centerpiece.
    pub rows: Vec<PeerRow>,
    /// W81 — host→RTT for the map's edge labels/spring lengths.
    pub rtt: HashMap<String, Option<f64>>,
    /// W81 — this node, anchored at the map's center.
    pub self_hostname: String,
    pub loaded: bool,
    pub status: String,
    /// EFF-45 — set when `mackesd fleet-status --json` failed (I/O, non-zero
    /// exit). The view renders the error state instead of the misleading "No
    /// enrolled nodes yet" empty state.
    pub load_error: Option<String>,
    pub busy: bool,
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
                self.loaded = true;
                self.load_error = None;
                self.busy = false;
                self.status.clear();
                Task::none()
            }
            Message::LoadError(e) => {
                // EFF-45 — fleet-status failure is an error, not an empty
                // fleet.
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let refresh = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then_some(crate::Message::FleetRollup(Message::RefreshClicked)),
            palette,
        );

        // EFF-45 — a failed fleet-status run renders as failure, never as the
        // "No enrolled nodes yet" empty state.
        if let Some(err) = &self.load_error {
            return panel_container(
                crate::panel_chrome::error_state(err.clone(), palette, || {
                    crate::Message::FleetRollup(Message::RefreshClicked)
                }),
                density,
            );
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

        panel_container(
            column![
                row![
                    text(format!("Fleet — {} node(s)", self.rollup.total)).size(20),
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
    fn loaded_sets_rollup_and_clears_busy() {
        let mut p = FleetRollupPanel::new();
        p.busy = true;
        let _ = p.update(Message::Loaded {
            rollup: parse_rollup(
                r#"{"total":1,"groups":[{"role":"host","total":1,"worst_health":"healthy"}]}"#,
            ),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: "pine".into(),
        });
        assert!(p.loaded);
        assert!(!p.busy);
        assert_eq!(p.rollup.total, 1);
        assert_eq!(p.self_hostname, "pine");
    }
}
