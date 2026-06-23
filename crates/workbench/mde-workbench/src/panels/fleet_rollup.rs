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
use std::time::Instant;

use cosmic::iced::widget::{column, container, row, scrollable, stack, text};
use cosmic::iced::{Length, Padding, Task};
use cosmic::Element;
use mde_theme::{EmptyState, Icon, LoadState, Palette};
use serde::Deserialize;

use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::{
    empty_state, load_state_indicator, panel_container, status_badge, BadgeSeverity,
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

/// MOTION-NET-3 — the old→new crossfade's `(outgoing_alpha, complete)` at `now`,
/// driven from `mde_theme::animation::crossfade` (the shared `dialog_mount`
/// 240 ms dissolve). `outgoing_alpha` is the opacity of a panel-background scrim
/// stacked over the freshly-loaded content: full at the swap → clear once
/// revealed, so the replacement reads as one dissolve, never a hard cut.
fn crossfade_sample(start: Instant, now: Instant) -> (f32, bool) {
    // Panel-local crossfades only arm with full motion (reduce-motion takes the
    // instant-swap branch), so `reduce_motion: false` here.
    let (outgoing, incoming) = mde_theme::animation::crossfade(start, now, false);
    (outgoing.alpha, incoming.alpha >= 1.0)
}

/// MOTION-NET-3 — has the crossfade settled at `now`? Drives the self-tick stop.
fn crossfade_complete(start: Instant, now: Instant) -> bool {
    crossfade_sample(start, now).1
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
    /// MOTION-NET-3 — when a fresh [`Message::Loaded`] replaced stale data, the
    /// instant the old→new crossfade began (panel-local, no central tick). `None`
    /// at rest; cleared once the crossfade settles. Under reduce-motion the swap
    /// is instant and this stays `None` (no tween, but still never a blank).
    crossfade_start: Option<Instant>,
    /// MOTION-NET-3 — true while a self-tick [`Message::CrossfadeTick`] loop is
    /// outstanding. Guarantees exactly ONE loop regardless of message
    /// interleaving: a back-to-back `Loaded` re-points `crossfade_start` at a
    /// fresh instant but does NOT spawn a second loop, so rapid refreshes never
    /// multiply the timer wakeups.
    crossfade_ticking: bool,
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
    /// MOTION-NET-3 — one frame of the panel-local old→new crossfade. The panel
    /// self-drives these (a timer-delayed [`Task`] re-arms the next frame) while a
    /// crossfade is in flight, so the dissolve animates WITHOUT a central tick in
    /// `app.rs`; the loop stops the moment the tween settles (no idle wakeups).
    CrossfadeTick,
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
                // MOTION-NET-3 — if this load is REPLACING prior on-screen data
                // (a stale-while-refreshing swap), crossfade old→new rather than
                // hard-cutting. A first load (nothing prior) just appears; under
                // reduce-motion the swap is instant (no tween) — either way the
                // panel never blanks (the stale data was kept dimmed until now).
                let had_prior = self.loaded && !self.rollup.groups.is_empty();
                let reduce_motion = crate::live_theme::reduce_motion();
                self.rollup = rollup;
                self.rows = rows;
                self.rtt = rtt;
                self.self_hostname = self_hostname;
                self.loaded = true;
                self.load_error = None;
                self.busy = false;
                self.status.clear();
                if had_prior && !reduce_motion {
                    self.crossfade_start = Some(Instant::now());
                    self.arm_crossfade_tick()
                } else {
                    // Reduce-motion / first load: instant swap, no crossfade.
                    self.crossfade_start = None;
                    Task::none()
                }
            }
            Message::LoadError(e) => {
                // EFF-45 — fleet-status failure is an error, not an empty
                // fleet.
                self.load_error = Some(e);
                self.busy = false;
                self.crossfade_start = None;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                // MOTION-NET-3 — mark the refresh in flight but KEEP the prior
                // rollup on screen (the view dims it + shows the Refreshing
                // indicator); it's never cleared, so the panel doesn't blank.
                self.busy = true;
                self.crossfade_start = None;
                self.status = "Refreshing…".into();
                Self::load()
            }
            Message::CrossfadeTick => {
                // MOTION-NET-3 — advance the panel-local crossfade. While still in
                // flight, re-arm the next frame; once settled (or `crossfade_start`
                // was cleared by a refresh/error), stop the loop so there are no
                // idle wakeups. `crossfade_ticking` stays true only while a frame
                // is genuinely pending.
                match self.crossfade_start {
                    Some(start) if !crossfade_complete(start, Instant::now()) => {
                        Self::crossfade_tick_task()
                    }
                    _ => {
                        self.crossfade_start = None;
                        self.crossfade_ticking = false;
                        Task::none()
                    }
                }
            }
        }
    }

    /// MOTION-NET-3 — start the self-tick loop **iff** one isn't already
    /// outstanding. The single `crossfade_ticking` flag makes this idempotent:
    /// a back-to-back `Loaded` re-points `crossfade_start` but reuses the live
    /// loop, so the timer never fans out into multiple concurrent loops.
    fn arm_crossfade_tick(&mut self) -> Task<crate::Message> {
        if self.crossfade_ticking {
            return Task::none();
        }
        self.crossfade_ticking = true;
        Self::crossfade_tick_task()
    }

    /// MOTION-NET-3 — schedule the next crossfade frame ≈60 fps later as a
    /// panel-local [`Task`] (a timer-delayed [`Message::CrossfadeTick`]). This is
    /// how the dissolve animates without a central `app.rs` subscription tick.
    fn crossfade_tick_task() -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::time::sleep(std::time::Duration::from_millis(16)).await;
            },
            |()| crate::Message::FleetRollup(Message::CrossfadeTick),
        )
    }

    /// MOTION-NET-3 — the canonical async state this panel is in, derived from
    /// its existing data/refresh flags. A refresh that still has prior groups to
    /// show is `Refreshing { stale: true }` (keep them dimmed, never blank); a
    /// first load with nothing yet is `Loading`.
    #[must_use]
    fn load_state(&self) -> LoadState {
        if self.load_error.is_some() {
            LoadState::Failed
        } else if self.busy {
            if self.rollup.groups.is_empty() {
                LoadState::Loading
            } else {
                LoadState::Refreshing { stale: true }
            }
        } else if self.loaded {
            LoadState::Loaded
        } else {
            LoadState::Idle
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let load = self.load_state();
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

        // MOTION-NET-3 — during a stale refresh the kept-on-screen data renders
        // DIMMED (foreground alpha scaled by the state's `content_alpha`; surfaces
        // stay opaque so only the content fades, never the panel). At rest it's
        // full opacity. The crossfade swap on `Loaded` then dissolves old→new.
        let content_palette = palette.dimmed(load.content_alpha());

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
                content_palette,
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
                            content_palette
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
                    palette: content_palette,
                    flow_phase: 0.0,
                })
                .width(Length::Fill)
                .height(Length::Fixed(260.0))
                .into();
            cosmic::iced::widget::themer(None, canvas).into()
        };

        // MOTION-NET-3 — the header carries the canonical Refreshing indicator
        // (icon + "Refreshing…", a non-motion cue) so a background refresh is
        // legible even under reduce-motion. The header itself never dims — only
        // the data below it does.
        let header = row![
            text(format!("Fleet — {} node(s)", self.rollup.total)).size(20),
            cosmic::iced::widget::Space::new().width(Length::Fill),
            load_state_indicator(load, palette),
            refresh,
        ]
        .spacing(12)
        .align_y(cosmic::iced::Alignment::Center);

        // The live data section (centerpiece + role cards) — the part that dims
        // while refreshing and crossfades on swap.
        let content: Element<'_, crate::Message> = column![
            centerpiece,
            scrollable(cards).height(Length::Fill),
        ]
        .spacing(16)
        .width(Length::Fill)
        .into();

        panel_container(
            column![header, self.crossfaded(content, palette)]
                .spacing(16)
                .width(Length::Fill)
                .into(),
            density,
        )
    }

    /// MOTION-NET-3 — wrap the freshly-loaded data `content` in the old→new
    /// crossfade while one is in flight: a panel-background scrim stacked over the
    /// new content at the outgoing alpha (full at the swap → clear when revealed),
    /// so the replacement dissolves rather than hard-cuts. iced 0.13 has no
    /// opacity widget for an arbitrary subtree, so — like `app.rs`'s route
    /// crossfade — the new body dissolves *through* the panel background. The
    /// scrim's content is an inert `Space`, so clicks/scrolls reach the live new
    /// content even mid-fade (no input delay). At rest the content is returned
    /// bare (zero extra widgets, no reflow).
    fn crossfaded<'a>(
        &self,
        content: Element<'a, crate::Message>,
        palette: Palette,
    ) -> Element<'a, crate::Message> {
        let Some(start) = self.crossfade_start else {
            return content;
        };
        let (scrim_alpha, complete) = crossfade_sample(start, Instant::now());
        if complete {
            return content;
        }
        let bg = palette.background;
        let scrim = container(cosmic::iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_theme| cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(
                    crate::cosmic_compat::with_alpha(bg.into_cosmic_color(), scrim_alpha),
                )),
                ..cosmic::iced::widget::container::Style::default()
            });
        stack![content, scrim].into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_theme::motion::Motion;

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

    /// Build a panel already showing one role group (the stale-while-refreshing
    /// prerequisite: there is prior on-screen data to keep + crossfade).
    fn loaded_panel() -> FleetRollupPanel {
        let mut p = FleetRollupPanel::new();
        let _ = p.update(Message::Loaded {
            rollup: parse_rollup(
                r#"{"total":1,"groups":[{"role":"host","total":1,"worst_health":"healthy"}]}"#,
            ),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: "pine".into(),
        });
        p
    }

    #[test]
    fn load_state_tracks_refresh_with_stale_data() {
        // MOTION-NET-3 — a refresh that still has prior groups reads as
        // Refreshing { stale: true } (keep-and-dim), not Loading (blank).
        let mut p = loaded_panel();
        assert_eq!(p.load_state(), LoadState::Loaded);
        // A first load with nothing on screen is Loading, not stale-refresh.
        let mut fresh = FleetRollupPanel::new();
        fresh.busy = true;
        assert_eq!(fresh.load_state(), LoadState::Loading);
        // Once data exists, a refresh is stale-refresh.
        p.busy = true;
        assert_eq!(p.load_state(), LoadState::Refreshing { stale: true });
        // The stale-refresh state dims the kept content (never blanks it).
        assert!(p.load_state().content_alpha() < 1.0);
        assert!(p.load_state().shows_content());
    }

    #[test]
    fn refresh_keeps_prior_groups_on_screen() {
        // MOTION-NET-3 — RefreshClicked must NOT clear the rollup; the data is
        // kept (the view dims it) so the panel never blanks during a refresh.
        let mut p = loaded_panel();
        let _ = p.update(Message::RefreshClicked);
        assert!(p.busy, "refresh is in flight");
        assert!(
            !p.rollup.groups.is_empty(),
            "prior groups stay on screen during the refresh"
        );
        assert_eq!(p.load_state(), LoadState::Refreshing { stale: true });
    }

    /// Drive a stale-refresh swap into `p`, replacing its groups with new data.
    fn swap_in_new_data(p: &mut FleetRollupPanel) {
        let _ = p.update(Message::RefreshClicked);
        let _ = p.update(Message::Loaded {
            rollup: parse_rollup(
                r#"{"total":2,"groups":[{"role":"host","total":2,"worst_health":"degraded"}]}"#,
            ),
            rows: Vec::new(),
            rtt: HashMap::new(),
            self_hostname: "pine".into(),
        });
    }

    #[test]
    fn motion_pref_decides_crossfade_vs_instant_swap() {
        // MOTION-NET-3 — full motion crossfades old→new; reduce-motion swaps
        // instantly (no tween). Driven in ONE test (serialized) since both mutate
        // the process-global `MDE_REDUCE_MOTION`, so they can't race each other.

        // Full motion: replacing on-screen data arms a crossfade.
        std::env::set_var("MDE_REDUCE_MOTION", "0");
        let mut full = loaded_panel();
        swap_in_new_data(&mut full);
        let start = full
            .crossfade_start
            .expect("full motion: replacing prior data arms a crossfade");
        // The crossfade starts in flight (scrim ~opaque at t=start), not complete.
        let (alpha0, complete0) = crossfade_sample(start, start);
        assert!(alpha0 > 0.9, "scrim starts ~opaque, got {alpha0}");
        assert!(!complete0, "crossfade is in flight at t=start");
        // After the dialog_mount duration it has settled.
        let done = start + Motion::dialog_mount().duration;
        assert!(crossfade_complete(start, done));

        // Reduce-motion: instant swap, no crossfade — but data still isn't blanked
        // (stale was kept until the swap).
        std::env::set_var("MDE_REDUCE_MOTION", "1");
        let mut reduced = loaded_panel();
        swap_in_new_data(&mut reduced);
        assert!(
            reduced.crossfade_start.is_none(),
            "reduce-motion takes the instant-swap branch — no crossfade"
        );
        assert_eq!(reduced.rollup.total, 2, "new data is shown immediately");

        std::env::remove_var("MDE_REDUCE_MOTION");
    }

    #[test]
    fn arm_crossfade_tick_is_idempotent() {
        // MOTION-NET-3 — the single-loop guard: arming while a loop is already
        // outstanding is a no-op, so back-to-back swaps never multiply the timer
        // wakeups into concurrent loops. (Env-free: drives the guard directly so
        // it can't race the process-global reduce-motion tests.)
        let mut p = loaded_panel();
        assert!(!p.crossfade_ticking, "starts at rest");
        // First arm starts a loop.
        let _ = p.arm_crossfade_tick();
        assert!(p.crossfade_ticking, "first arm starts the loop");
        // Arming again while ticking does NOT start a second loop.
        let _ = p.arm_crossfade_tick();
        assert!(p.crossfade_ticking, "still exactly one loop");
        // Once the loop reports nothing in flight, a CrossfadeTick stops it and
        // clears the flag — the next arm may start a fresh loop.
        p.crossfade_start = None;
        let _ = p.update(Message::CrossfadeTick);
        assert!(
            !p.crossfade_ticking,
            "loop stops + flag clears when the crossfade is gone"
        );
    }

    #[test]
    fn crossfade_tick_clears_once_settled() {
        // MOTION-NET-3 — the self-tick loop stops when the crossfade settles
        // (no idle wakeups): once `crossfade_start` is in the past beyond the
        // dialog_mount duration, a tick clears it.
        let mut p = loaded_panel();
        p.crossfade_start =
            Some(Instant::now() - Motion::dialog_mount().duration - std::time::Duration::from_secs(1));
        let _ = p.update(Message::CrossfadeTick);
        assert!(
            p.crossfade_start.is_none(),
            "a settled crossfade is dropped, stopping the tick loop"
        );
    }
}
