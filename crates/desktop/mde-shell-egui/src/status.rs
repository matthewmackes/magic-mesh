//! Daemon-owned notification rollups and the ambient own-seat critical cue.
//!
//! Presentation lives in the Construct status bar and notification center. This
//! module is intentionally only their shared read model plus the critical edge.

use std::time::{Duration, Instant};

use mde_bus::persist::Persist;
use mde_egui::egui;
use mde_egui::Style;
use serde::Deserialize;

use crate::bus_reader::BusReader;

const REFRESH: Duration = Duration::from_secs(2);
const TOPIC_PREFIX: &str = "state/notify/segment/";
const CRITICAL_POLICY_OWN_SEAT: &str = "own-seat-light-show";
const EDGE_PULSE_SECONDS: f32 = 2.4;
const EDGE_PULSE_HALF_CYCLE_SECONDS: f32 = 0.24;
const EDGE_HELD_W: f32 = 3.0;
const EDGE_PULSE_W: f32 = 14.0;

/// Daemon notification segments presented by Construct chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusSegment {
    /// Local device/platform health.
    Device,
    /// Mesh/fleet/cloud health.
    Mesh,
    /// Power and energy posture.
    Power,
    /// Aggregate alert health.
    Alerts,
}

impl StatusSegment {
    const ALL: [Self; 4] = [Self::Device, Self::Mesh, Self::Power, Self::Alerts];

    const fn key(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Mesh => "mesh",
            Self::Power => "power",
            Self::Alerts => "alerts",
        }
    }

    fn topic(self) -> String {
        format!("{TOPIC_PREFIX}{}", self.key())
    }
}

/// A segment's latest rollup as published by the notify worker.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SegmentRollup {
    /// The wire segment name.
    pub segment: String,
    /// `critical` / `warning` / `info` / `success`.
    pub severity: String,
    /// The source currently driving the segment's worst state.
    pub source: String,
    /// Human summary for expanded surfaces.
    pub summary: String,
    /// Host the worst event belongs to.
    pub host: String,
    /// Cross-node critical policy.
    pub critical_policy: String,
    /// Event timestamp.
    pub ts_unix_ms: i64,
}

/// Latest daemon-owned notification projection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusSegments {
    /// Latest Device rollup.
    pub device: Option<SegmentRollup>,
    /// Latest Mesh rollup.
    pub mesh: Option<SegmentRollup>,
    /// Latest Power rollup.
    pub power: Option<SegmentRollup>,
    /// Latest Alerts aggregate rollup.
    pub alerts: Option<SegmentRollup>,
    /// `true` once a poll was attempted.
    pub seen: bool,
}

impl StatusSegments {
    fn set(&mut self, segment: StatusSegment, rollup: Option<SegmentRollup>) {
        match segment {
            StatusSegment::Device => self.device = rollup,
            StatusSegment::Mesh => self.mesh = rollup,
            StatusSegment::Power => self.power = rollup,
            StatusSegment::Alerts => self.alerts = rollup,
        }
    }

    pub(crate) const fn get(&self, segment: StatusSegment) -> Option<&SegmentRollup> {
        match segment {
            StatusSegment::Device => self.device.as_ref(),
            StatusSegment::Mesh => self.mesh.as_ref(),
            StatusSegment::Power => self.power.as_ref(),
            StatusSegment::Alerts => self.alerts.as_ref(),
        }
    }
}

/// Self-gated Bus reader for the four daemon notification mirrors.
#[derive(Debug)]
pub struct StatusState {
    bus_root: Option<std::path::PathBuf>,
    segments: StatusSegments,
    last_poll: Option<Instant>,
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            segments: StatusSegments::default(),
            last_poll: None,
        }
    }
}

impl StatusState {
    /// Poll the local Bus mirror on its bounded cadence.
    pub fn poll(&mut self, ctx: &egui::Context, _local_host: &str) {
        if self.last_poll.is_some_and(|last| last.elapsed() < REFRESH) {
            return;
        }
        self.last_poll = Some(Instant::now());
        if let Some(persist) = BusReader::new(self.bus_root.clone()).open() {
            self.segments = read_segments(&persist);
        } else {
            self.segments.seen = true;
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Latest folded segment state.
    pub const fn segments(&self) -> &StatusSegments {
        &self.segments
    }

    /// Test seam for shell-level integration without touching the real Bus.
    #[cfg(test)]
    pub(crate) fn set_segments_for_test(&mut self, segments: StatusSegments) {
        self.segments = segments;
        self.last_poll = Some(Instant::now());
    }
}

fn read_segments(persist: &Persist) -> StatusSegments {
    let mut out = StatusSegments {
        seen: true,
        ..StatusSegments::default()
    };
    for segment in StatusSegment::ALL {
        out.set(segment, latest_rollup(persist, segment));
    }
    out
}

fn latest_rollup(persist: &Persist, segment: StatusSegment) -> Option<SegmentRollup> {
    let msg = persist.list_since(&segment.topic(), None).ok()?.pop()?;
    serde_json::from_str(msg.body.as_deref()?).ok()
}

/// Map a daemon severity to the shared support-token palette.
pub(crate) fn severity_color(rollup: Option<&SegmentRollup>) -> egui::Color32 {
    match rollup.map(|rollup| rollup.severity.as_str()) {
        Some("critical" | "error" | "fatal" | "urgent") => Style::SUPPORT_ERROR,
        Some("warning" | "warn" | "high") => Style::SUPPORT_WARNING,
        Some("info" | "notice" | "debug") => Style::SUPPORT_INFO,
        Some("success" | "ok") => Style::SUPPORT_SUCCESS,
        _ => Style::TEXT_DIM,
    }
}

/// Accessible severity label for one rollup.
pub(crate) fn severity_label(rollup: Option<&SegmentRollup>) -> &'static str {
    match rollup.map(|rollup| rollup.severity.as_str()) {
        Some("critical" | "error" | "fatal" | "urgent") => "critical",
        Some("warning" | "warn" | "high") => "warning",
        Some("info" | "notice" | "debug") => "info",
        Some("success" | "ok") => "ok",
        Some(_) | None => "unknown",
    }
}

/// Human label for one Construct status cell.
pub(crate) const fn segment_label(segment: StatusSegment) -> &'static str {
    match segment {
        StatusSegment::Device => "Device",
        StatusSegment::Mesh => "Mesh",
        StatusSegment::Power => "Power",
        StatusSegment::Alerts => "Alerts",
    }
}

fn is_critical_severity(severity: &str) -> bool {
    matches!(severity, "critical" | "error" | "fatal" | "urgent")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CriticalEdgeKey {
    segment: StatusSegment,
    host: String,
    source: String,
    ts_unix_ms: i64,
}

#[derive(Debug, Clone)]
struct ActiveCriticalEdge {
    key: CriticalEdgeKey,
    started_at: Instant,
}

/// Ambient all-edges cue for an own-seat critical.
#[derive(Debug, Default)]
pub struct CriticalEdgeCue {
    active: Option<ActiveCriticalEdge>,
    acknowledged: Option<CriticalEdgeKey>,
    became_visible: bool,
}

impl CriticalEdgeCue {
    /// Fold daemon segment rollups into this seat's edge-cue state.
    pub fn update(&mut self, segments: &StatusSegments, local_host: &str) {
        let was_visible = self.visible();
        match critical_edge_key(segments, local_host) {
            Some(key) => {
                if self.active.as_ref().map(|active| &active.key) != Some(&key) {
                    self.active = Some(ActiveCriticalEdge {
                        key,
                        started_at: Instant::now(),
                    });
                }
            }
            None => {
                self.active = None;
                self.acknowledged = None;
            }
        }
        if !was_visible && self.visible() {
            self.became_visible = true;
        }
    }

    /// Acknowledge the live cue without clearing the daemon rollup.
    pub fn acknowledge(&mut self) {
        if let Some(active) = &self.active {
            self.acknowledged = Some(active.key.clone());
        }
    }

    /// Whether the cue should be visible right now.
    #[must_use]
    pub fn visible(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| self.acknowledged.as_ref() != Some(&active.key))
    }

    /// Drain a hidden-to-visible edge exactly once.
    pub fn take_became_visible(&mut self) -> bool {
        std::mem::take(&mut self.became_visible)
    }

    /// Render the no-text all-edges cue and acknowledge it when clicked.
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.visible() {
            return;
        }
        let Some(active) = &self.active else {
            return;
        };
        let intensity = edge_cue_intensity(active.started_at.elapsed().as_secs_f32());
        let width = EDGE_HELD_W + (EDGE_PULSE_W - EDGE_HELD_W) * intensity;
        let alpha = 110.0 + 125.0 * intensity;
        let color = Style::SUPPORT_ERROR.linear_multiply((alpha / 255.0).clamp(0.0, 1.0));
        let mut clicked = false;
        egui::Area::new(critical_edge_cue_id())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_TOP, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                let rect = ui.ctx().screen_rect();
                install_critical_edge_accessibility(ui.ctx(), &active.key, rect);
                ui.set_min_size(rect.size());
                for (index, edge) in edge_rects(rect, width).into_iter().enumerate() {
                    ui.painter().rect_filled(edge, 0.0, color);
                    clicked |= ui
                        .interact(
                            edge,
                            egui::Id::new(("notif-critical-edge", index)),
                            egui::Sense::click(),
                        )
                        .clicked();
                }
            });
        ctx.request_repaint_after(Duration::from_secs_f32(EDGE_PULSE_HALF_CYCLE_SECONDS));
        if clicked {
            self.acknowledge();
        }
    }
}

/// Stable id for the foreground critical cue.
pub fn critical_edge_cue_id() -> egui::Id {
    egui::Id::new("notif-critical-edge-cue")
}

fn critical_edge_key(segments: &StatusSegments, local_host: &str) -> Option<CriticalEdgeKey> {
    for segment in StatusSegment::ALL {
        let Some(rollup) = segments.get(segment) else {
            continue;
        };
        if is_critical_severity(&rollup.severity)
            && rollup.critical_policy == CRITICAL_POLICY_OWN_SEAT
            && rollup.host == local_host
        {
            return Some(CriticalEdgeKey {
                segment,
                host: rollup.host.clone(),
                source: rollup.source.clone(),
                ts_unix_ms: rollup.ts_unix_ms,
            });
        }
    }
    None
}

fn edge_cue_intensity(elapsed: f32) -> f32 {
    if elapsed < 0.0 || elapsed >= EDGE_PULSE_SECONDS {
        return 0.0;
    }
    let phase = (elapsed / EDGE_PULSE_HALF_CYCLE_SECONDS).floor() as i32;
    if phase.rem_euclid(2) == 0 {
        1.0
    } else {
        0.35
    }
}

fn edge_rects(rect: egui::Rect, width: f32) -> [egui::Rect; 4] {
    [
        egui::Rect::from_min_max(
            rect.left_top(),
            egui::pos2(rect.right(), rect.top() + width),
        ),
        egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - width),
            rect.right_bottom(),
        ),
        egui::Rect::from_min_max(
            rect.left_top(),
            egui::pos2(rect.left() + width, rect.bottom()),
        ),
        egui::Rect::from_min_max(
            egui::pos2(rect.right() - width, rect.top()),
            rect.right_bottom(),
        ),
    ]
}

fn install_critical_edge_accessibility(
    ctx: &egui::Context,
    key: &CriticalEdgeKey,
    rect: egui::Rect,
) {
    let value = format!(
        "Critical {} alert from {} on {}",
        segment_label(key.segment),
        key.source,
        key.host
    );
    let _ = ctx.accesskit_node_builder(egui::Id::new("notif-critical-edge-live-region"), |node| {
        node.set_role(egui::accesskit::Role::Alert);
        node.set_live(egui::accesskit::Live::Assertive);
        node.set_label("Critical alert");
        node.set_value(value);
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rollup(severity: &str, host: &str) -> SegmentRollup {
        SegmentRollup {
            segment: "alerts".to_string(),
            severity: severity.to_string(),
            source: "test".to_string(),
            summary: "test".to_string(),
            host: host.to_string(),
            critical_policy: CRITICAL_POLICY_OWN_SEAT.to_string(),
            ts_unix_ms: 1,
        }
    }

    #[test]
    fn severity_uses_support_tokens() {
        assert_eq!(
            severity_color(Some(&rollup("critical", "eagle"))),
            Style::SUPPORT_ERROR
        );
        assert_eq!(
            severity_color(Some(&rollup("warning", "eagle"))),
            Style::SUPPORT_WARNING
        );
        assert_eq!(severity_color(None), Style::TEXT_DIM);
    }

    #[test]
    fn critical_cue_is_scoped_to_this_seat_and_latches_once() {
        let mut cue = CriticalEdgeCue::default();
        let peer = StatusSegments {
            alerts: Some(rollup("critical", "hawk")),
            ..StatusSegments::default()
        };
        cue.update(&peer, "eagle");
        assert!(!cue.visible());

        let own = StatusSegments {
            alerts: Some(rollup("critical", "eagle")),
            ..StatusSegments::default()
        };
        cue.update(&own, "eagle");
        assert!(cue.visible());
        assert!(cue.take_became_visible());
        assert!(!cue.take_became_visible());
        cue.acknowledge();
        assert!(!cue.visible());
    }
}
