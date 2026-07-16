//! NOTIF-3 — the dock's compact health strip.
//!
//! The daemon owns event severity and segment rollups (`state/notify/segment/*`);
//! the shell only renders the latest read-model: one local A-F grade pip plus the
//! four count-free health pips Device · Mesh · Power · Alerts plus the shell-fed
//! File operations segment. Missing state is an honest dim baseline, never
//! fabricated green. NOTIF-6's critical edge cue is also kept here so status has
//! one shell-side code path.

use std::time::{Duration, Instant};

use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;
use mde_egui::egui::{self, FontId};
use mde_egui::{
    operation_progress_value, paint_operation_progress_badge, GradeBand, OperationProgressView,
    Style,
};
use mde_seat::{Battery, BatteryState, Probe, SeatSnapshot};
use mde_theme::brand::icons::IconId;
use serde::Deserialize;

use crate::chrome::{GradeRow, NodeGrades};
use crate::dock::{icon_texture, Surface};

const REFRESH: Duration = Duration::from_secs(2);
const TOPIC_PREFIX: &str = "state/notify/segment/";
const BATTERY_LOW: f64 = 20.0;
const BATTERY_CRITICAL: f64 = 5.0;
const CRITICAL_POLICY_OWN_SEAT: &str = "own-seat-light-show";
const EDGE_PULSE_SECONDS: f32 = 2.4;
const EDGE_PULSE_HALF_CYCLE_SECONDS: f32 = 0.24;
const EDGE_HELD_W: f32 = 3.0;
const EDGE_PULSE_W: f32 = 14.0;

/// The bottom notification/status segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusSegment {
    /// Local device/platform health.
    Device,
    /// Mesh/fleet/cloud health.
    Mesh,
    /// Power and energy posture.
    Power,
    /// Shell-wide file-operation/download progress.
    FileOperations,
    /// Aggregate alert health.
    Alerts,
}

impl StatusSegment {
    /// Render order, top to bottom.
    pub const ALL: [Self; 5] = [
        Self::Device,
        Self::Mesh,
        Self::Power,
        Self::FileOperations,
        Self::Alerts,
    ];

    const DAEMON: [Self; 4] = [Self::Device, Self::Mesh, Self::Power, Self::Alerts];

    const fn key(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Mesh => "mesh",
            Self::Power => "power",
            Self::FileOperations => "file-operations",
            Self::Alerts => "alerts",
        }
    }

    const fn route(self) -> Surface {
        match self {
            Self::Device | Self::Power => Surface::System,
            Self::Mesh => Surface::MeshView,
            Self::FileOperations => Surface::Files,
            Self::Alerts => Surface::Chat,
        }
    }

    const fn icon(self) -> IconId {
        match self {
            Self::Device => IconId::Settings,
            Self::Mesh => IconId::MeshView,
            Self::Power => IconId::BatteryBolt,
            Self::FileOperations => IconId::Files,
            Self::Alerts => IconId::Chat,
        }
    }

    fn topic(self) -> String {
        format!("{TOPIC_PREFIX}{}", self.key())
    }
}

/// Compact shell-wide file-operation status carried by the notification fabric.
///
/// Files owns the detailed operation/transfer state. Browser downloads and other
/// producers feed this bounded projection so the bottom status segment can report
/// active count, known-progress average, and a primary label without learning any
/// per-surface job model.
#[derive(Debug, Clone, PartialEq)]
pub struct FileOperationStatus {
    /// Active file jobs represented by this summary.
    active: usize,
    /// Average known progress, `None` while all active jobs are still queued/starting.
    fraction: Option<f32>,
    /// Bounded display label for the status strip/panel.
    label: String,
}

impl FileOperationStatus {
    /// Construct a shell-wide file-operation summary.
    #[must_use]
    pub fn new(active: usize, fraction: Option<f32>, label: impl Into<String>) -> Self {
        Self {
            active,
            fraction: fraction.map(|f| f.clamp(0.0, 1.0)),
            label: truncate_file_operation_label(&label.into()),
        }
    }

    fn view(&self) -> OperationProgressView<'_> {
        OperationProgressView::new(self.active, self.fraction, &self.label)
    }
}

/// A segment's latest rollup as published by the notify worker.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SegmentRollup {
    /// The wire segment name.
    pub segment: String,
    /// `critical` / `warning` / `info`.
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

/// The pips the dock renders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusSegments {
    /// Latest Device rollup.
    pub device: Option<SegmentRollup>,
    /// Latest Mesh rollup.
    pub mesh: Option<SegmentRollup>,
    /// Latest Power rollup.
    pub power: Option<SegmentRollup>,
    /// Shell-fed active file-operation/download progress.
    pub file_operations: Option<FileOperationStatus>,
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
            StatusSegment::FileOperations => {}
            StatusSegment::Alerts => self.alerts = rollup,
        }
    }

    fn get(&self, segment: StatusSegment) -> Option<&SegmentRollup> {
        match segment {
            StatusSegment::Device => self.device.as_ref(),
            StatusSegment::Mesh => self.mesh.as_ref(),
            StatusSegment::Power => self.power.as_ref(),
            StatusSegment::FileOperations => None,
            StatusSegment::Alerts => self.alerts.as_ref(),
        }
    }
}

/// Polls `state/notify/segment/*` for the dock.
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
    /// Test seam.
    #[cfg(test)]
    fn with_bus_root(bus_root: std::path::PathBuf) -> Self {
        Self {
            bus_root: Some(bus_root),
            segments: StatusSegments::default(),
            last_poll: None,
        }
    }

    /// Poll the local Bus mirror, self-gated to a cheap cadence.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.last_poll.is_some_and(|t| t.elapsed() < REFRESH) {
            return;
        }
        self.last_poll = Some(Instant::now());
        // arch-11: open through the shared BusReader seam. A missing spool and an
        // unopenable one both fold to the same honest "seen but dim" state.
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
    for segment in StatusSegment::DAEMON {
        out.set(segment, latest_rollup(persist, segment));
    }
    out
}

fn truncate_file_operation_label(label: &str) -> String {
    const MAX_CHARS: usize = 34;
    let char_count = label.chars().count();
    if char_count <= MAX_CHARS {
        return label.to_owned();
    }
    let mut out = label
        .chars()
        .take(MAX_CHARS.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn latest_rollup(persist: &Persist, segment: StatusSegment) -> Option<SegmentRollup> {
    let topic = segment.topic();
    let msg = persist.list_since(&topic, None).ok()?.pop()?;
    serde_json::from_str(msg.body.as_deref()?).ok()
}

/// `pub(crate)` (not private) because WIN7-4's Start Menu System tile
/// (`start_menu.rs`) reuses this SAME fold for its own Device/Power segment
/// tint rather than re-deriving a second severity→colour mapping (the
/// `dock::response_activated` cross-module-reuse idiom, restated here).
pub(crate) fn severity_color(rollup: Option<&SegmentRollup>) -> egui::Color32 {
    match rollup.map(|r| r.severity.as_str()) {
        Some("critical" | "error" | "fatal" | "urgent") => Style::SUPPORT_ERROR,
        Some("warning" | "warn" | "high") => Style::SUPPORT_WARNING,
        Some("info" | "notice" | "debug") => Style::SUPPORT_INFO,
        Some("success" | "ok") => Style::SUPPORT_SUCCESS,
        _ => Style::TEXT_DIM,
    }
}

/// The color used for a compact status segment, including shell-owned synthetic
/// segments that do not have daemon rollups.
pub(crate) fn segment_color(segment: StatusSegment, segments: &StatusSegments) -> egui::Color32 {
    match segment {
        StatusSegment::FileOperations if segments.file_operations.is_some() => Style::ACCENT,
        StatusSegment::FileOperations => Style::TEXT_DIM,
        _ => severity_color(segments.get(segment)),
    }
}

/// `pub(crate)` for the SAME WIN7-4 reuse [`severity_color`] documents.
pub(crate) fn severity_label(rollup: Option<&SegmentRollup>) -> &'static str {
    match rollup.map(|r| r.severity.as_str()) {
        Some("critical" | "error" | "fatal" | "urgent") => "critical",
        Some("warning" | "warn" | "high") => "warning",
        Some("info" | "notice" | "debug") => "info",
        Some("success" | "ok") => "ok",
        Some(_) => "unknown",
        None => "unknown",
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

/// NOTIF-6 — ambient all-edges cue for an own-seat critical.
#[derive(Debug, Default)]
pub struct CriticalEdgeCue {
    active: Option<ActiveCriticalEdge>,
    acknowledged: Option<CriticalEdgeKey>,
    /// WIN7-6 (`docs/design/win7-desktop-survey.md` lock #9) — latched inside
    /// [`Self::update`] on a [`Self::visible`] hidden→visible edge (a fresh
    /// critical or one re-raised after an older one was acknowledged), drained
    /// once by [`Self::take_became_visible`]. `main.rs` uses the drain to
    /// auto-close an open Start Menu the instant the cue starts showing,
    /// without re-firing on every steady "still visible" frame afterward — the
    /// same one-shot `take_*` shape
    /// `start_menu::StartMenuState`'s own `take_tile_activation`/
    /// `HotkeyRouter::take_dock_toggle` already use.
    became_visible: bool,
}

impl CriticalEdgeCue {
    /// Fold the daemon segment rollups into this seat's edge-cue state.
    pub fn update(&mut self, segments: &StatusSegments, local_host: &str) {
        let was_visible = self.visible();
        let next = critical_edge_key(segments, local_host);
        match next {
            Some(key) => {
                if self.active.as_ref().map(|a| &a.key) != Some(&key) {
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
        // WIN7-6 (lock #9) — an edge, not a level check: `visible()` can go
        // false→true from a brand-new critical or a genuinely new one after an
        // old (different) one was acknowledged. Both are real "the cue just
        // started showing" moments and both should latch here. A steady
        // already-visible frame, and the same rollup staying acknowledged,
        // must NOT latch (see `Self::became_visible`'s own doc for why).
        if !was_visible && self.visible() {
            self.became_visible = true;
        }
    }

    /// Acknowledge the live cue without clearing the underlying daemon rollup.
    pub fn acknowledge(&mut self) {
        if let Some(active) = &self.active {
            self.acknowledged = Some(active.key.clone());
        }
    }

    /// Whether the cue should be visible right now.
    pub fn visible(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| self.acknowledged.as_ref() != Some(&active.key))
    }

    /// WIN7-6 — drain whether the last [`Self::update`] carried the cue
    /// across a hidden→visible edge; `false` on every other call, including
    /// every steady "still visible" frame while the SAME critical stays live
    /// and un-acknowledged, and every frame after a caller has already
    /// drained a `true`. `main.rs` calls this once per frame, right after
    /// `update`, to react to the cue *starting* to show (closing the Start
    /// Menu if it's open, lock #9) without fighting an operator who reopens
    /// the Start Menu afterward — whether that's while this same critical is
    /// still up (no further edge fires until something actually changes) or
    /// after they've acknowledged it via [`Self::acknowledge`].
    pub fn take_became_visible(&mut self) -> bool {
        std::mem::take(&mut self.became_visible)
    }

    /// Render the no-text all-edges cue and let an edge click acknowledge it.
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.visible() {
            return;
        }
        let Some(active) = &self.active else {
            return;
        };
        let elapsed = active.started_at.elapsed().as_secs_f32();
        let intensity = edge_cue_intensity(elapsed);
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
                let painter = ui.painter();
                for (i, edge) in edge_rects(rect, width).into_iter().enumerate() {
                    painter.rect_filled(edge, 0.0, color);
                    clicked |= ui
                        .interact(
                            edge,
                            egui::Id::new(("notif-critical-edge", i)),
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

/// Stable id for NOTIF-6's foreground all-edges cue.
pub fn critical_edge_cue_id() -> egui::Id {
    egui::Id::new("notif-critical-edge-cue")
}

/// Stable id for NOTIF-11's polite status live region.
pub fn status_live_region_id() -> egui::Id {
    egui::Id::new("notif-status-live-region")
}

/// Stable id for NOTIF-11's assertive critical alert node.
pub fn critical_edge_live_region_id() -> egui::Id {
    egui::Id::new("notif-critical-edge-live-region")
}

fn critical_edge_key(segments: &StatusSegments, local_host: &str) -> Option<CriticalEdgeKey> {
    for segment in StatusSegment::DAEMON {
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

#[cfg(test)]
fn local_grade_color(grades: &NodeGrades) -> egui::Color32 {
    grades
        .rows
        .iter()
        .find(|row| row.is_local)
        .map_or(Style::TEXT_DIM, |row| {
            if row.stale {
                Style::TEXT_DIM
            } else {
                GradeBand::from_score(f32::from(row.score)).color()
            }
        })
}

fn local_grade_label(grades: &NodeGrades) -> String {
    grades.rows.iter().find(|row| row.is_local).map_or_else(
        || "?".to_string(),
        |row| {
            if row.stale {
                "?".to_string()
            } else {
                GradeBand::from_score(f32::from(row.score))
                    .letter()
                    .to_string()
            }
        },
    )
}

fn status_panel_grade_text(row: &GradeRow) -> String {
    let grade = if row.stale {
        "?".to_string()
    } else {
        GradeBand::from_score(f32::from(row.score))
            .letter()
            .to_string()
    };
    format!("{}  {} {grade}", row.host, row.trend.arrow())
}

pub(crate) const fn segment_label(segment: StatusSegment) -> &'static str {
    match segment {
        StatusSegment::Device => "Device",
        StatusSegment::Mesh => "Mesh",
        StatusSegment::Power => "Power",
        StatusSegment::FileOperations => "File operations",
        StatusSegment::Alerts => "Alerts",
    }
}

pub(crate) fn segment_accessibility_value(
    segment: StatusSegment,
    segments: &StatusSegments,
) -> String {
    if segment == StatusSegment::FileOperations {
        return segments.file_operations.as_ref().map_or_else(
            || "File operations idle".to_string(),
            |progress| {
                format!(
                    "File operations active: {}",
                    operation_progress_value(progress.view())
                )
            },
        );
    }
    let rollup = segments.get(segment);
    let state = severity_label(rollup);
    rollup.map_or_else(
        || format!("{} status unknown", segment_label(segment)),
        |r| {
            format!(
                "{} {state}: {} from {} on {}",
                segment_label(segment),
                r.summary,
                r.source,
                r.host
            )
        },
    )
}

fn status_live_summary(grades: &NodeGrades, segments: &StatusSegments) -> String {
    let mut parts = vec![format!("Local grade {}", local_grade_label(grades))];
    for segment in StatusSegment::ALL {
        parts.push(segment_accessibility_value(segment, segments));
    }
    parts.join(". ")
}

fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

fn install_status_accessibility(
    ctx: &egui::Context,
    rect: egui::Rect,
    grades: &NodeGrades,
    segments: &StatusSegments,
) {
    let summary = status_live_summary(grades, segments);
    let _ = ctx.accesskit_node_builder(status_live_region_id(), |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label("Notification status");
        node.set_value(summary);
        node.set_bounds(accesskit_rect(rect));
    });
}

fn install_segment_accessibility(
    ctx: &egui::Context,
    segment: StatusSegment,
    segments: &StatusSegments,
    rect: egui::Rect,
) {
    let value = segment_accessibility_value(segment, segments);
    let _ = ctx.accesskit_node_builder(segment_pip_id(segment), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(format!("{} status", segment_label(segment)));
        node.set_value(value);
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
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
    let _ = ctx.accesskit_node_builder(critical_edge_live_region_id(), |node| {
        node.set_role(egui::accesskit::Role::Alert);
        node.set_live(egui::accesskit::Live::Assertive);
        node.set_label("Critical alert");
        node.set_value(value);
        node.set_bounds(accesskit_rect(rect));
    });
}

/// Stable id for the local grade pip.
#[cfg(test)]
pub fn local_grade_pip_id() -> egui::Id {
    egui::Id::new("notif-status-local-grade-pip")
}

/// Stable id for one segment pip.
pub fn segment_pip_id(segment: StatusSegment) -> egui::Id {
    egui::Id::new(("notif-status-segment-pip", segment.key()))
}

/// Stable id for NOTIF-4's expansion chevron.
#[cfg(test)]
pub fn status_chevron_id() -> egui::Id {
    egui::Id::new("notif-status-chevron")
}

/// Stable id for NOTIF-4's expansion panel.
pub fn status_panel_id() -> egui::Id {
    egui::Id::new("notif-status-panel")
}

/// Stable id for a peer row in the expansion panel.
pub fn status_panel_grade_id(host: &str) -> egui::Id {
    egui::Id::new(("notif-status-panel-grade", host))
}

/// Stable id for the expansion panel's device-control band.
pub fn status_panel_device_id() -> egui::Id {
    egui::Id::new("notif-status-panel-device")
}

/// Stable id for the expansion panel's file-operation row.
pub fn status_panel_file_operations_id() -> egui::Id {
    egui::Id::new("notif-status-panel-file-operations")
}

/// Output from rendering the compact row.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StatusBarOutcome {
    /// A pip or local grade routed to a surface.
    pub routed: bool,
    /// The specific segment that routed, when a segment pip was activated.
    pub routed_segment: Option<StatusSegment>,
    /// The chevron was clicked.
    pub toggle_panel: bool,
}

/// Output from rendering NOTIF-4's expansion panel.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StatusPanelOutcome {
    /// A peer grade row requested focus.
    pub node_focus: Option<String>,
    /// Device controls requested the full System surface.
    pub route_system: bool,
    /// A status segment row requested its owning surface.
    pub routed_segment: Option<StatusSegment>,
}

/// Render the notification/status micro-icons inside the bottom rail.
pub fn notification_rail(
    ui: &egui::Ui,
    active: &mut Surface,
    grades: &NodeGrades,
    segments: &StatusSegments,
    rect: egui::Rect,
    expanded: bool,
) -> StatusBarOutcome {
    install_status_accessibility(ui.ctx(), rect, grades, segments);
    let mut out = StatusBarOutcome::default();
    let rail_h = rect.height();
    let mut x = rect.left();
    for segment in StatusSegment::ALL {
        let pip_rect =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(rail_h, rail_h))
                .shrink(2.0);
        if segment_pip(ui, segment, segments, pip_rect) {
            *active = segment.route();
            out.routed = true;
            out.routed_segment = Some(segment);
        }
        x += rail_h;
    }
    let _ = expanded;
    out
}

/// Render one dock row containing `[local grade] + [Device · Mesh · Power · Alerts]`.
#[cfg(test)]
pub fn status_bar(
    ui: &egui::Ui,
    active: &mut Surface,
    grades: &NodeGrades,
    segments: &StatusSegments,
    rect: egui::Rect,
    expanded: bool,
) -> StatusBarOutcome {
    install_status_accessibility(ui.ctx(), rect, grades, segments);
    let painter = ui.painter().clone();
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );

    let grade_rect =
        egui::Rect::from_min_max(rect.left_top(), egui::pos2(rect.center().x, rect.bottom()))
            .shrink(Style::SP_XS);
    let resp = ui.interact(grade_rect, local_grade_pip_id(), egui::Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Local grade {}", local_grade_label(grades)),
        )
    });
    if resp.hovered() {
        painter.rect_filled(grade_rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let grade_color = local_grade_color(grades);
    painter.circle_filled(grade_rect.center(), Style::SP_S, grade_color);
    painter.text(
        grade_rect.center(),
        egui::Align2::CENTER_CENTER,
        local_grade_label(grades),
        FontId::proportional(Style::SMALL),
        Style::BG,
    );
    let mut out = StatusBarOutcome::default();
    if resp.clicked() {
        *active = Surface::MeshView;
        out.routed = true;
    }

    let chev_rect = egui::Rect::from_min_size(
        egui::pos2(grade_rect.left(), grade_rect.bottom() - Style::SP_M),
        egui::vec2(Style::SP_M, Style::SP_M),
    );
    let chev = ui.interact(chev_rect, status_chevron_id(), egui::Sense::click());
    let chev_tint = if expanded || chev.hovered() {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    painter.text(
        chev_rect.center(),
        egui::Align2::CENTER_CENTER,
        if expanded { "‹" } else { "›" },
        FontId::proportional(Style::BODY),
        chev_tint,
    );
    if chev.clicked() {
        out.toggle_panel = true;
    }

    let pip_left = rect.center().x;
    let pip_h = rect.height() / StatusSegment::ALL.len() as f32;
    for (i, segment) in StatusSegment::ALL.iter().copied().enumerate() {
        let pip_rect = egui::Rect::from_min_size(
            egui::pos2(pip_left, rect.top() + i as f32 * pip_h),
            egui::vec2(rect.width() / 2.0, pip_h),
        )
        .shrink(1.0);
        if segment_pip(ui, segment, segments, pip_rect) {
            *active = segment.route();
            out.routed = true;
            out.routed_segment = Some(segment);
        }
    }
    out
}

fn segment_pip(
    ui: &egui::Ui,
    segment: StatusSegment,
    segments: &StatusSegments,
    rect: egui::Rect,
) -> bool {
    install_segment_accessibility(ui.ctx(), segment, segments, rect);
    let resp = ui.interact(rect, segment_pip_id(segment), egui::Sense::click());
    let painter = ui.painter().clone();
    if resp.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let color = segment_color(segment, segments);
    let center = rect.center();
    if let Some(tex) = icon_texture(ui.ctx(), segment.icon(), Style::SP_M, color) {
        let icon = egui::Rect::from_center_size(center, egui::vec2(Style::SP_M, Style::SP_M));
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon, uv, egui::Color32::WHITE);
    } else {
        painter.circle_filled(center, Style::SP_XS, color);
    }
    resp.clicked()
}

/// Render NOTIF-4's slide-out detail panel to the right of the dock.
pub fn status_panel(
    ui: &egui::Ui,
    grades: &NodeGrades,
    segments: &StatusSegments,
    seat: Option<&SeatSnapshot>,
    rect: egui::Rect,
) -> StatusPanelOutcome {
    let painter = ui.painter().clone();
    painter.rect_filled(rect, Style::RADIUS, Style::SURFACE);
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    let _panel = ui.interact(rect, status_panel_id(), egui::Sense::hover());

    let mut out = StatusPanelOutcome::default();
    let mut y = rect.top() + Style::SP_S;
    let inner_left = rect.left() + Style::SP_S;
    let inner_right = rect.right() - Style::SP_S;
    let row_h = Style::SP_L;

    painter.text(
        egui::pos2(inner_left, y),
        egui::Align2::LEFT_TOP,
        "Health",
        FontId::proportional(Style::BODY),
        Style::TEXT,
    );
    y += row_h;

    let mut rows = grades.rows.clone();
    rows.sort_by_key(|row| {
        (
            !row.is_local,
            std::cmp::Reverse(if row.stale { 0 } else { row.score }),
        )
    });
    if rows.is_empty() {
        painter.text(
            egui::pos2(inner_left, y),
            egui::Align2::LEFT_TOP,
            "No grades yet",
            FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        );
        y += row_h;
    }
    let grade_limit = if segments.file_operations.is_some() {
        4
    } else {
        5
    };
    for row in rows.iter().take(grade_limit) {
        let r = egui::Rect::from_min_max(
            egui::pos2(inner_left, y),
            egui::pos2(inner_right, y + row_h),
        );
        let resp = ui.interact(r, status_panel_grade_id(&row.host), egui::Sense::click());
        if resp.hovered() {
            painter.rect_filled(r, Style::RADIUS, Style::SURFACE_HI);
        }
        let color = if row.stale {
            Style::TEXT_DIM
        } else {
            GradeBand::from_score(f32::from(row.score)).color()
        };
        painter.circle_filled(
            egui::pos2(r.left() + Style::SP_M, r.center().y),
            Style::SP_XS,
            color,
        );
        painter.text(
            egui::pos2(r.left() + Style::SP_L, r.center().y),
            egui::Align2::LEFT_CENTER,
            status_panel_grade_text(row),
            FontId::proportional(Style::SMALL),
            if row.is_local {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            },
        );
        if resp.clicked() {
            out.node_focus = Some(row.host.clone());
        }
        y += row_h;
    }

    y += Style::SP_XS;
    painter.hline(
        inner_left..=inner_right,
        y,
        egui::Stroke::new(1.0, Style::BORDER),
    );
    y += Style::SP_S;

    if let Some(progress) = segments.file_operations.as_ref() {
        let file_rect = egui::Rect::from_min_max(
            egui::pos2(inner_left, y),
            egui::pos2(inner_right, y + row_h + Style::SP_S),
        );
        if draw_file_operations_row(ui, file_rect, progress) {
            out.routed_segment = Some(StatusSegment::FileOperations);
        }
        y = file_rect.bottom() + Style::SP_XS;
        painter.hline(
            inner_left..=inner_right,
            y,
            egui::Stroke::new(1.0, Style::BORDER),
        );
        y += Style::SP_S;
    }

    let device_rect = egui::Rect::from_min_max(
        egui::pos2(inner_left, y),
        egui::pos2(inner_right, y + row_h * 3.0),
    );
    let resp = ui.interact(device_rect, status_panel_device_id(), egui::Sense::click());
    if resp.hovered() {
        painter.rect_filled(device_rect, Style::RADIUS, Style::SURFACE_HI);
    }
    if resp.clicked() {
        out.route_system = true;
    }
    draw_device_controls(ui, seat, device_rect);
    out
}

fn draw_file_operations_row(
    ui: &egui::Ui,
    rect: egui::Rect,
    progress: &FileOperationStatus,
) -> bool {
    let resp = ui.interact(
        rect,
        status_panel_file_operations_id(),
        egui::Sense::click(),
    );
    let badge_rect = rect.shrink2(egui::vec2(Style::SP_XS, Style::SP_XS));
    paint_operation_progress_badge(ui, badge_rect, progress.view(), false, resp.hovered());
    let _ = ui
        .ctx()
        .accesskit_node_builder(status_panel_file_operations_id(), |node| {
            node.set_role(egui::accesskit::Role::Button);
            node.set_label("File operations status");
            node.set_value(operation_progress_value(progress.view()));
            node.set_bounds(accesskit_rect(rect));
            node.add_action(egui::accesskit::Action::Click);
        });
    resp.clicked()
}

fn draw_device_controls(ui: &egui::Ui, seat: Option<&SeatSnapshot>, rect: egui::Rect) {
    let painter = ui.painter().clone();
    let left = rect.left() + Style::SP_S;
    let right = rect.right() - Style::SP_S;
    let volume = seat.and_then(|snap| match &snap.mixer {
        Probe::Present(m) => Some((m.master.volume, m.master.muted)),
        Probe::Absent { .. } => None,
    });
    let bt = seat.and_then(|snap| match &snap.bluetooth {
        Probe::Present(b) => Some((b.any_adapter_powered(), b.connected_devices())),
        Probe::Absent { .. } => None,
    });
    let battery = battery_status(seat);
    let vol_y = rect.top() + Style::SP_M;
    painter.text(
        egui::pos2(left, vol_y),
        egui::Align2::LEFT_CENTER,
        volume.map_or("Vol --".to_string(), |(v, muted)| {
            if muted {
                format!("Vol {v}% muted")
            } else {
                format!("Vol {v}%")
            }
        }),
        FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    draw_meter(
        &painter,
        egui::Rect::from_min_max(
            egui::pos2(left, vol_y + Style::SP_S),
            egui::pos2(right, vol_y + Style::SP_M),
        ),
        volume.map_or(0.0, |(v, _)| f32::from(v) / 100.0),
    );
    let bt_y = rect.top() + Style::SP_L + Style::SP_M;
    painter.text(
        egui::pos2(left, bt_y),
        egui::Align2::LEFT_CENTER,
        bt.map_or("BT --".to_string(), |(on, connected)| {
            bluetooth_status_text(on, connected)
        }),
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
    draw_meter(
        &painter,
        egui::Rect::from_min_max(
            egui::pos2(left, bt_y + Style::SP_S),
            egui::pos2(right, bt_y + Style::SP_M),
        ),
        bt.map_or(0.0, |(on, _)| if on { 1.0 } else { 0.0 }),
    );
    let battery_y = rect.top() + Style::SP_L * 2.0 + Style::SP_M;
    let (battery_text, battery_pct, battery_tone, charging) =
        battery.map_or(("Batt --".to_string(), 0.0, Style::TEXT_DIM, false), |b| {
            (
                format!(
                    "Batt {:.0}%{}",
                    b.percentage,
                    if battery_charging(b.state) {
                        " charging"
                    } else {
                        ""
                    }
                ),
                (b.percentage / 100.0) as f32,
                battery_tone(b),
                battery_charging(b.state),
            )
        });
    painter.text(
        egui::pos2(left, battery_y),
        egui::Align2::LEFT_CENTER,
        battery_text,
        FontId::proportional(Style::SMALL),
        if charging {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        },
    );
    let battery_icon = battery.map_or(IconId::BatteryEmpty, |b| battery_fill_icon(b.percentage));
    if let Some(tex) = icon_texture(ui.ctx(), battery_icon, Style::SP_M, battery_tone) {
        let icon = egui::Rect::from_center_size(
            egui::pos2(right - Style::SP_S, battery_y),
            egui::vec2(Style::SP_M, Style::SP_M),
        );
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon, uv, egui::Color32::WHITE);
    }
    draw_meter(
        &painter,
        egui::Rect::from_min_max(
            egui::pos2(left, battery_y + Style::SP_S),
            egui::pos2(right, battery_y + Style::SP_M),
        ),
        battery_pct,
    );
}

fn draw_meter(painter: &egui::Painter, rect: egui::Rect, pct: f32) {
    painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    let fill = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            rect.left() + rect.width() * pct.clamp(0.0, 1.0),
            rect.bottom(),
        ),
    );
    painter.rect_filled(fill, Style::RADIUS, Style::ACCENT);
}

fn bluetooth_status_text(on: bool, connected: usize) -> String {
    format!("BT {} - {connected}", if on { "on" } else { "off" })
}

fn battery_status(seat: Option<&SeatSnapshot>) -> Option<&Battery> {
    match seat.map(|s| &s.batteries) {
        Some(Probe::Present(cells)) => system_pack(cells),
        _ => None,
    }
}

fn battery_fill_icon(percentage: f64) -> IconId {
    if percentage < 12.5 {
        IconId::BatteryEmpty
    } else if percentage < 37.5 {
        IconId::BatteryQuarter
    } else if percentage < 62.5 {
        IconId::BatteryHalf
    } else if percentage < 87.5 {
        IconId::BatteryThreeQuarter
    } else {
        IconId::BatteryFull
    }
}

const fn battery_charging(state: BatteryState) -> bool {
    matches!(state, BatteryState::Charging | BatteryState::PendingCharge)
}

fn battery_tone(b: &Battery) -> egui::Color32 {
    match b.state {
        BatteryState::Charging | BatteryState::FullyCharged => Style::SUPPORT_SUCCESS,
        BatteryState::Empty => Style::SUPPORT_ERROR,
        BatteryState::Discharging | BatteryState::PendingDischarge => {
            if b.percentage <= BATTERY_CRITICAL {
                Style::SUPPORT_ERROR
            } else if b.percentage < BATTERY_LOW {
                Style::SUPPORT_WARNING
            } else {
                Style::TEXT_DIM
            }
        }
        BatteryState::PendingCharge | BatteryState::Unknown => Style::TEXT_DIM,
    }
}

fn system_pack(cells: &[Battery]) -> Option<&Battery> {
    cells.iter().find(|b| b.power_supply).or_else(|| {
        cells.iter().max_by(|a, b| {
            a.percentage
                .partial_cmp(&b.percentage)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::{GradeRow, GradeTrend};
    use mde_bus::hooks::config::Priority;
    use mde_seat::BatteryKind;

    fn grade(score: u8, stale: bool) -> NodeGrades {
        NodeGrades {
            rows: vec![GradeRow {
                host: "eagle".to_string(),
                score,
                trend: GradeTrend::Steady,
                is_local: true,
                stale,
            }],
            seen: true,
        }
    }

    fn battery(percentage: f64, state: BatteryState, power_supply: bool) -> Battery {
        Battery {
            model: "BAT".to_string(),
            kind: BatteryKind::Internal,
            percentage,
            state,
            power_supply,
            time_to_empty: None,
            time_to_full: None,
            energy_rate: None,
        }
    }

    fn rollup(segment: &str, severity: &str, host: &str, policy: &str, ts: i64) -> SegmentRollup {
        SegmentRollup {
            segment: segment.to_string(),
            severity: severity.to_string(),
            source: "test".to_string(),
            summary: "test".to_string(),
            host: host.to_string(),
            critical_policy: policy.to_string(),
            ts_unix_ms: ts,
        }
    }

    fn accesskit_nodes(
        out: &egui::FullOutput,
    ) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
        out.platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone()
    }

    #[test]
    fn segment_severity_maps_to_carbon_support_tokens() {
        let rollup = |severity: &str| SegmentRollup {
            segment: "alerts".to_string(),
            severity: severity.to_string(),
            source: "test".to_string(),
            summary: "test".to_string(),
            host: "eagle".to_string(),
            critical_policy: "own-seat-light-show".to_string(),
            ts_unix_ms: 1,
        };
        assert_eq!(
            severity_color(Some(&rollup("critical"))),
            Style::SUPPORT_ERROR
        );
        assert_eq!(
            severity_color(Some(&rollup("warning"))),
            Style::SUPPORT_WARNING
        );
        assert_eq!(severity_color(Some(&rollup("info"))), Style::SUPPORT_INFO);
        assert_eq!(severity_color(None), Style::TEXT_DIM);
    }

    #[test]
    fn local_grade_pip_uses_grade_band_or_dim_unknown() {
        assert_eq!(local_grade_label(&grade(95, false)), "A");
        assert_eq!(
            local_grade_color(&grade(95, false)),
            GradeBand::from_score(95.0).color()
        );
        assert_eq!(local_grade_label(&grade(20, true)), "?");
        assert_eq!(local_grade_color(&grade(20, true)), Style::TEXT_DIM);
        assert_eq!(local_grade_label(&NodeGrades::default()), "?");
    }

    #[test]
    fn status_panel_grade_rows_include_trend_arrows() {
        let mut rows = grade(95, false).rows;
        rows[0].trend = GradeTrend::Up;
        assert_eq!(status_panel_grade_text(&rows[0]), "eagle  ↑ A");
        rows[0].trend = GradeTrend::Down;
        rows[0].stale = true;
        assert_eq!(status_panel_grade_text(&rows[0]), "eagle  ↓ ?");
    }

    #[test]
    fn status_bar_exports_accesskit_live_region_and_named_pips() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut active = Surface::Workbench;
        let grades = grade(95, false);
        let segments = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                11,
            )),
            mesh: Some(rollup("mesh", "warning", "lh-1", "remote-pip-chat", 12)),
            file_operations: Some(FileOperationStatus::new(
                2,
                Some(0.5),
                "2 browser downloads",
            )),
            seen: true,
            ..StatusSegments::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(240.0, 160.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                status_bar(
                    ui,
                    &mut active,
                    &grades,
                    &segments,
                    egui::Rect::from_min_size(egui::pos2(8.0, 8.0), egui::vec2(48.0, 48.0)),
                    false,
                );
            });
        });
        let nodes = accesskit_nodes(&out);
        let status = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Notification status"))
            .expect("status live region node");
        assert_eq!(status.role(), egui::accesskit::Role::Status);
        assert_eq!(status.live(), Some(egui::accesskit::Live::Polite));
        let value = status.value().expect("status summary");
        assert!(value.contains("Local grade A"));
        assert!(value.contains("Alerts critical"));
        assert!(value.contains("Mesh warning"));
        assert!(value.contains("File operations active"));

        let alert_pip = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Alerts status"))
            .expect("alerts pip node");
        assert_eq!(alert_pip.role(), egui::accesskit::Role::Button);
        assert!(
            alert_pip
                .value()
                .is_some_and(|value| value.contains("Alerts critical")),
            "alert pip carries its severity summary"
        );
        let file_pip = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("File operations status"))
            .expect("file operations pip node");
        assert_eq!(file_pip.role(), egui::accesskit::Role::Button);
        assert!(
            file_pip
                .value()
                .is_some_and(|value| value.contains("50% average progress")),
            "file-operation pip carries progress through the notification status fabric"
        );
    }

    #[test]
    fn file_operation_status_labels_are_bounded_ascii_for_bottom_progress() {
        let status = FileOperationStatus::new(
            1,
            Some(0.5),
            "Copy very-long-platform-operation-report-final.txt",
        );
        let label = status.view().label;

        assert!(label.ends_with("..."), "truncated label = {label}");
        assert!(label.is_ascii(), "truncated label = {label}");
        assert!(
            label.chars().count() <= 34,
            "bottom-progress label should stay bounded: {label}"
        );
        assert!(!label.contains('…'), "label must not use Unicode ellipsis");
        assert!(
            !mde_egui::operation_progress_text(status.view()).contains('·'),
            "shared bottom-progress badge text must not use middle-dot separators"
        );
    }

    #[test]
    fn status_panel_device_copy_uses_ascii_separators() {
        let text = bluetooth_status_text(true, 2);
        assert_eq!(text, "BT on - 2");
        assert!(text.is_ascii());
        assert!(!text.contains('·'), "Bluetooth status text = {text}");
    }

    #[test]
    fn read_segments_pulls_latest_rollup_for_each_topic() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let body = serde_json::json!({
            "segment": "mesh",
            "severity": "critical",
            "source": "cloud",
            "summary": "cloud api down",
            "host": "lh-1",
            "critical_policy": "remote-pip-chat",
            "ts_unix_ms": 10
        })
        .to_string();
        persist
            .write(
                &StatusSegment::Mesh.topic(),
                Priority::Default,
                None,
                Some(&body),
            )
            .unwrap();

        let segments = read_segments(&persist);
        assert!(segments.seen);
        assert_eq!(
            segments.mesh.as_ref().map(|r| r.severity.as_str()),
            Some("critical")
        );
        assert!(segments.device.is_none(), "missing segments stay dim");
    }

    #[test]
    fn status_state_polls_the_bus_root() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        persist
            .write(
                &StatusSegment::Alerts.topic(),
                Priority::Default,
                None,
                Some(
                    r#"{"segment":"alerts","severity":"warning","source":"journal","summary":"warn","host":"eagle","critical_policy":"remote-pip-chat","ts_unix_ms":1}"#,
                ),
            )
            .unwrap();
        let ctx = egui::Context::default();
        let mut state = StatusState::with_bus_root(tmp.path().to_path_buf());
        state.poll(&ctx);
        assert_eq!(
            state.segments().alerts.as_ref().map(|r| r.source.as_str()),
            Some("journal")
        );
    }

    #[test]
    fn battery_fill_ladder_survives_tray_retirement() {
        assert_eq!(battery_fill_icon(0.0), IconId::BatteryEmpty);
        assert_eq!(battery_fill_icon(20.0), IconId::BatteryQuarter);
        assert_eq!(battery_fill_icon(50.0), IconId::BatteryHalf);
        assert_eq!(battery_fill_icon(80.0), IconId::BatteryThreeQuarter);
        assert_eq!(battery_fill_icon(95.0), IconId::BatteryFull);
    }

    #[test]
    fn system_pack_and_battery_tone_stay_honest() {
        let peripheral = battery(90.0, BatteryState::Discharging, false);
        let system = battery(35.0, BatteryState::Discharging, true);
        assert_eq!(
            system_pack(&[peripheral.clone(), system.clone()]),
            Some(&system)
        );
        assert_eq!(
            system_pack(std::slice::from_ref(&peripheral)),
            Some(&peripheral)
        );

        assert_eq!(
            battery_tone(&battery(80.0, BatteryState::Charging, true)),
            Style::SUPPORT_SUCCESS
        );
        assert_eq!(
            battery_tone(&battery(10.0, BatteryState::Discharging, true)),
            Style::SUPPORT_WARNING
        );
        assert_eq!(
            battery_tone(&battery(4.0, BatteryState::Discharging, true)),
            Style::SUPPORT_ERROR
        );
        assert_eq!(
            battery_tone(&battery(80.0, BatteryState::Discharging, true)),
            Style::TEXT_DIM
        );
    }

    #[test]
    fn critical_edge_cue_only_keys_own_seat_critical_rollups() {
        let mut segments = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                11,
            )),
            ..StatusSegments::default()
        };
        assert!(
            critical_edge_key(&segments, "eagle").is_some(),
            "own-seat criticals light the edge cue"
        );

        segments.alerts = Some(rollup(
            "alerts",
            "warning",
            "eagle",
            CRITICAL_POLICY_OWN_SEAT,
            12,
        ));
        assert!(
            critical_edge_key(&segments, "eagle").is_none(),
            "warnings stay pull-only"
        );

        segments.alerts = Some(rollup(
            "alerts",
            "critical",
            "oak",
            CRITICAL_POLICY_OWN_SEAT,
            13,
        ));
        assert!(
            critical_edge_key(&segments, "eagle").is_none(),
            "remote host criticals stay in the pip/chat path"
        );

        segments.alerts = Some(rollup("alerts", "critical", "eagle", "remote-pip-chat", 14));
        assert!(
            critical_edge_key(&segments, "eagle").is_none(),
            "the daemon policy must opt into the own-seat light-show"
        );
    }

    #[test]
    fn critical_edge_cue_acknowledges_and_clears_on_resolve() {
        let live = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                11,
            )),
            ..StatusSegments::default()
        };
        let mut cue = CriticalEdgeCue::default();
        cue.update(&live, "eagle");
        assert!(cue.visible(), "a live critical is visible");

        cue.acknowledge();
        assert!(
            !cue.visible(),
            "ack hides the cue while the same rollup is live"
        );

        cue.update(&live, "eagle");
        assert!(
            !cue.visible(),
            "the same acknowledged rollup does not immediately reappear"
        );

        let reraised = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                12,
            )),
            ..StatusSegments::default()
        };
        cue.update(&reraised, "eagle");
        assert!(cue.visible(), "a new live critical re-raises the cue");

        cue.update(&StatusSegments::default(), "eagle");
        assert!(!cue.visible(), "resolved rollups clear the cue");
    }

    #[test]
    fn critical_edge_cue_critical_breaks_through_push_suppression_policy() {
        let live = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                11,
            )),
            ..StatusSegments::default()
        };
        let mut cue = CriticalEdgeCue::default();
        cue.update(&live, "eagle");
        assert!(
            cue.visible(),
            "own-seat Critical edge cues break through DND/focus suppression; \
             non-critical ambient suppression belongs to the toast policy"
        );
        assert!(
            cue.take_became_visible(),
            "a break-through Critical still latches the one-shot visible edge"
        );
    }

    #[test]
    fn critical_edge_cue_take_became_visible_latches_once_per_real_edge() {
        // WIN7-6 (lock #9): `take_became_visible` must fire exactly once per
        // genuine hidden→visible transition — never on a steady "still
        // visible" frame, never on a re-affirmed acknowledged rollup — so
        // `main.rs`'s Start-Menu auto-close reacts to a firing without
        // fighting an operator who reopens the menu afterward.
        let live = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                11,
            )),
            ..StatusSegments::default()
        };
        let mut cue = CriticalEdgeCue::default();

        cue.update(&live, "eagle");
        assert!(
            cue.take_became_visible(),
            "a fresh critical is a hidden->visible edge"
        );
        assert!(
            !cue.take_became_visible(),
            "draining the edge clears it until the next real transition"
        );

        cue.update(&live, "eagle");
        assert!(
            !cue.take_became_visible(),
            "the SAME still-active critical is not a new edge"
        );

        cue.acknowledge();
        cue.update(&live, "eagle");
        assert!(
            !cue.take_became_visible(),
            "re-affirming an acknowledged rollup is not a new edge"
        );

        let reraised = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                12,
            )),
            ..StatusSegments::default()
        };
        cue.update(&reraised, "eagle");
        assert!(
            cue.take_became_visible(),
            "a genuinely new critical after an old one was acked is a fresh edge"
        );

        cue.update(&StatusSegments::default(), "eagle");
        assert!(
            !cue.take_became_visible(),
            "a resolved rollup clearing the cue is not itself a visible edge"
        );
    }

    #[test]
    fn critical_edge_pulse_settles_to_a_held_glow() {
        assert_eq!(edge_cue_intensity(0.0), 1.0);
        assert_eq!(
            edge_cue_intensity(EDGE_PULSE_HALF_CYCLE_SECONDS * 1.1),
            0.35
        );
        assert_eq!(edge_cue_intensity(EDGE_PULSE_SECONDS + 0.01), 0.0);
    }

    #[test]
    fn critical_edge_cue_tessellates_as_an_ambient_edge_overlay() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let live = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                11,
            )),
            ..StatusSegments::default()
        };
        let mut cue = CriticalEdgeCue::default();
        cue.update(&live, "eagle");
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| cue.show(ctx));
        let out = ctx.run(input(), |ctx| cue.show(ctx));
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "a live own-seat critical paints a real no-text edge overlay"
        );
    }

    #[test]
    fn critical_edge_cue_exports_an_assertive_accesskit_alert() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let live = StatusSegments {
            alerts: Some(rollup(
                "alerts",
                "critical",
                "eagle",
                CRITICAL_POLICY_OWN_SEAT,
                11,
            )),
            ..StatusSegments::default()
        };
        let mut cue = CriticalEdgeCue::default();
        cue.update(&live, "eagle");
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| cue.show(ctx));
        let nodes = accesskit_nodes(&out);
        let alert = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Critical alert"))
            .expect("critical alert live-region node");
        assert_eq!(alert.role(), egui::accesskit::Role::Alert);
        assert_eq!(alert.live(), Some(egui::accesskit::Live::Assertive));
        assert!(
            alert
                .value()
                .is_some_and(|value| value.contains("Critical Alerts alert")),
            "critical live region names the affected segment"
        );
    }
}
