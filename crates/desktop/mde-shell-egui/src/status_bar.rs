//! `status_bar` — WL-UX-006/U11: Construct's responsive clock/status chrome.
//!
//! Authority: `docs/design/platform-interfaces.md` §2.3 (Q12): a ~24px
//! HIG-style side-rail strip — a centered clock, one health control, and
//! compact system-control glyphs on the right — plus a Windows-style bottom
//! clock/tray when the taskbar is in Bottom mode. Both are fed by the existing
//! [`crate::status`] `StatusSegments` read-model.
//! **This deliberately REVERSES the old NAVBAR-W10 "no top bar" decision**
//! (Q12 says so in as many words).
//!
//! ## Paint layer and reserved layout band
//!
//! The side strip paints as a foreground [`egui::Area`] pinned to the top edge,
//! while `main.rs::central_view` reserves the matching [`STATUS_BAR_H`] band
//! only in the side-rail phase. The bottom tray lives in the full-width taskbar
//! lane, so the two treatments never compete for the same clock or workspace
//! pixels.
//!
//! ## Auto-hide (Q12/Q28)
//!
//! Hidden while the curtain is engaged (CURTAIN-1: no chrome under the lock),
//! in the Car profile (Auto Mode owns its own instrument chrome), and over a
//! focused full-screen remote desktop or immersive Maps workspace (U24).
//! Visibility is a pure fold ([`status_bar_visible`]); appearing transitions
//! fade through [`Motion::animate`]. A hiding frame stops painting immediately:
//! `main.rs::central_view` removes the reserved band at the same target-state
//! boundary, so fading over the newly reclaimed workspace would reintroduce the
//! very overlap this strip is meant to prevent.
//!
//! ## Material (§2.6 doctrine)
//!
//! The strip is **persistent chrome, not an overlay**, so it takes a clean
//! opaque [`Style::BG`] band + hairline instead of a scrim: Q21's translucent
//! materials are for overlays that push back content beneath, and an opaque
//! band is the only way to *guarantee* the 4.5:1 text contrast over arbitrary
//! surface content (a translucent wash over a bright surface would wash out).
//!
//! ## Honest data (§7)
//!
//! The health control consumes the typed health snapshot. Its accessible label
//! and badge carry the exact unacknowledged actionable count; A–F is shown only
//! inside the centered modal.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use mackes_mesh_types::health::{
    GradeLetter, HealthSeverity, NodeGrade, RequirementClass, SystemMeshHealthSnapshot,
    HEALTH_SCHEMA_VERSION, MAX_HEALTH_ID_BYTES, MAX_NODE_HEALTH_CONDITIONS,
};
use mde_egui::egui;
use mde_egui::{Motion, Style, TypographyRole};
use mde_theme::brand::icons::IconId;

use crate::chrome::HealthStatus;
use crate::construct::ConstructChrome;
use crate::status::StatusSegments;
use crate::surfaces::{icon_texture, Surface, TOOL_TRAY_SURFACES};

/// The locked strip height (Q12: "~24px").
pub(crate) const STATUS_BAR_H: f32 = 24.0;
/// Width reserved by the bottom taskbar for the Windows-style system tray.
/// The navigation bar keeps this lane free of app pins so the clock and
/// controls remain visually stable while the center cluster changes.
pub(crate) const BOTTOM_TRAY_W: f32 = 480.0;
/// Clear space between the taskbar placement control and the tray.
pub(crate) const BOTTOM_TRAY_GAP: f32 = 8.8;

/// Construct-owned workspaces promoted into the persistent notification/tool
/// tray. The navigation rail remains intact in both placement modes.
pub(crate) const WORKSPACE_TRAY_SURFACES: [Surface; 5] = TOOL_TRAY_SURFACES;
const WORKSPACE_TRAY_ICON_W: f32 = STATUS_BAR_H;
const WORKSPACE_TRAY_GAP: f32 = Style::SP_XS;

/// One menu trigger replaces the old four-icon shortcut cluster. The existing
/// Control Center remains the source of truth for all live status values.
const STATUS_MENU_ICON: IconId = IconId::Menu;
const STATUS_MENU_LABEL: &str = "Open status menu — Control Center";

/// The status-menu icon gets one full rail-height hit target, matching the
/// compact macOS menu-bar rhythm while keeping the pointer target larger than
/// the glyph.
const STATUS_CONTROL_W: f32 = STATUS_BAR_H;
const STATUS_CONTROL_GAP: f32 = Style::SP_XS;
const STATUS_CONTROL_ICON: f32 = Style::ICON_M;
const BOTTOM_TRAY_STATUS_MENU_W: f32 = 40.0;
const NOTIFICATION_BELL_W: f32 = 32.0;
/// The health and Mesh Teams launchers share the notification bell's compact
/// hit target so the three adjacent controls read as one intentional group.
const STATUS_LAUNCHER_W: f32 = NOTIFICATION_BELL_W;
const BATTERY_STATUS_W: f32 = 58.0;
const WEATHER_STATUS_W: f32 = 64.0;
const WEATHER_STATUS_COMPACT_W: f32 = STATUS_BAR_H;
/// Keep a usable clock lane when a window is narrower than the normal menu
/// bar. The lane may shrink below this value on an extremely small surface,
/// but the controls must never consume the centered clock's hit target.
const STATUS_CLOCK_MIN_W: f32 = Style::SP_XL;
/// A single status cell is deliberately one line. This cap is generous for
/// the current fixed labels, but prevents a future daemon-provided label from
/// turning the rail into an unbounded layout job.
const MAX_STATUS_TEXT_CHARS: usize = 256;

const HEALTH_INDICATOR_AUTHORITY_ID: &str = "status-bar-health-indicator-authority-v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HealthIndicator {
    grade: Option<GradeLetter>,
    count: usize,
}

impl HealthIndicator {
    const fn fresh(self) -> bool {
        self.grade.is_some()
    }

    const fn severity(self) -> Option<HealthSeverity> {
        match self.grade {
            Some(GradeLetter::F) => Some(HealthSeverity::Critical),
            Some(GradeLetter::D | GradeLetter::E) => Some(HealthSeverity::Warning),
            Some(GradeLetter::A | GradeLetter::B | GradeLetter::C) | None => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct HealthIndicatorAuthority {
    watermark: Option<SystemMeshHealthSnapshot>,
    visible: HealthIndicator,
}

fn grade_from_summary(capability: u8, warnings: usize, critical: usize) -> GradeLetter {
    if critical > 0 {
        GradeLetter::F
    } else if warnings >= 2 {
        GradeLetter::E
    } else if warnings == 1 {
        GradeLetter::D
    } else {
        match capability {
            90..=u8::MAX => GradeLetter::A,
            80..=89 => GradeLetter::B,
            _ => GradeLetter::C,
        }
    }
}

/// Revalidate the complete UX-013 authority before its grade reaches persistent
/// chrome. This is intentionally a consumer-side admission check: a replaced
/// on-disk projection must not become trusted merely because it deserializes.
fn health_indicator_from_snapshot(
    snapshot: &SystemMeshHealthSnapshot,
    expected_observer: &str,
    now_ms: u64,
) -> Option<HealthIndicator> {
    if snapshot.schema_version != HEALTH_SCHEMA_VERSION
        || snapshot.observer != expected_observer
        || snapshot.generation == 0
        || !snapshot.is_fresh(now_ms)
        || snapshot.roster_revision.is_empty()
        || snapshot.roster_revision.len() > MAX_HEALTH_ID_BYTES
        || snapshot.roster_revision.trim() != snapshot.roster_revision
        || !snapshot.roster_revision.is_ascii()
        || snapshot.roster_revision.chars().any(char::is_control)
        || snapshot.current_node_grades.len() != snapshot.mesh_summary.fresh_nodes
        || snapshot.mesh_summary.fresh_nodes > snapshot.mesh_summary.canonical_nodes
        || snapshot.active_conditions.len() > MAX_NODE_HEALTH_CONDITIONS
    {
        return None;
    }

    let mut nodes = BTreeSet::new();
    for grade in &snapshot.current_node_grades {
        if grade.node.is_empty()
            || grade.node.len() > MAX_HEALTH_ID_BYTES
            || grade.node.trim() != grade.node
            || !grade.node.is_ascii()
            || !nodes.insert(grade.node.as_str())
            || grade.capability_score > 100
            || grade.evaluated_at_ms == 0
            || grade.evaluated_at_ms > snapshot.generated_at_ms
            || NodeGrade::evaluate(
                grade.node.clone(),
                grade.capability_score,
                grade.factors,
                &snapshot.active_conditions,
                grade.evaluated_at_ms,
            )
            .grade
                != grade.grade
        {
            return None;
        }
    }

    let mut strongest = BTreeMap::new();
    for condition in snapshot.active_conditions.iter().filter(|condition| {
        condition.is_active() && condition.requirement == RequirementClass::Required
    }) {
        if condition.source != condition.evidence.provider
            || condition.active_since_ms == 0
            || condition.active_since_ms > condition.last_observed_ms
            || condition.last_observed_ms > snapshot.generated_at_ms
            || condition.evidence.observed_at_ms > condition.last_observed_ms
        {
            return None;
        }
        strongest
            .entry((condition.scope.clone(), condition.id.as_str()))
            .and_modify(|severity: &mut HealthSeverity| {
                *severity = (*severity).max(condition.severity);
            })
            .or_insert(condition.severity);
    }
    let (warnings, critical) = strongest.values().fold(
        (0usize, 0usize),
        |(warnings, critical), severity| match severity {
            HealthSeverity::Warning => (warnings + 1, critical),
            HealthSeverity::Critical => (warnings, critical + 1),
        },
    );
    let count = snapshot.active_issue_count(now_ms);
    let capability = snapshot
        .current_node_grades
        .iter()
        .map(|grade| grade.capability_score)
        .min()
        .unwrap_or(70);
    let grade = grade_from_summary(capability, warnings, critical);
    if snapshot.mesh_summary.active_warnings != warnings
        || snapshot.mesh_summary.active_critical != critical
        || snapshot.mesh_summary.unacknowledged_actionable != count
        || snapshot.mesh_summary.grade != grade
    {
        return None;
    }
    Some(HealthIndicator {
        grade: Some(grade),
        count,
    })
}

fn reconcile_health_indicator(
    watermark: Option<&SystemMeshHealthSnapshot>,
    candidate: Option<&SystemMeshHealthSnapshot>,
    expected_observer: &str,
    now_ms: u64,
) -> HealthIndicatorAuthority {
    let Some(candidate) = candidate else {
        return HealthIndicatorAuthority {
            watermark: watermark.cloned(),
            visible: HealthIndicator::default(),
        };
    };
    let Some(candidate_indicator) =
        health_indicator_from_snapshot(candidate, expected_observer, now_ms)
    else {
        return HealthIndicatorAuthority {
            watermark: watermark.cloned(),
            visible: HealthIndicator::default(),
        };
    };
    let admitted = match watermark {
        None => candidate.clone(),
        Some(previous) if previous == candidate => previous.clone(),
        Some(previous)
            if previous.observer == candidate.observer
                && previous.generation < candidate.generation
                && previous.generated_at_ms < candidate.generated_at_ms =>
        {
            candidate.clone()
        }
        Some(previous) => previous.clone(),
    };
    let visible = if &admitted == candidate {
        candidate_indicator
    } else {
        health_indicator_from_snapshot(&admitted, expected_observer, now_ms).unwrap_or_default()
    };
    HealthIndicatorAuthority {
        watermark: Some(admitted),
        visible,
    }
}

fn sync_health_indicator(ctx: &egui::Context, health: &HealthStatus) {
    let id = egui::Id::new(HEALTH_INDICATOR_AUTHORITY_ID);
    let previous = ctx
        .data(|data| data.get_temp::<HealthIndicatorAuthority>(id))
        .unwrap_or_default();
    let now_ms = u64::try_from(crate::timers::now_unix())
        .unwrap_or(0)
        .saturating_mul(1_000);
    let next = reconcile_health_indicator(
        previous.watermark.as_ref(),
        health.snapshot(),
        &crate::explorer::local_hostname(),
        now_ms,
    );
    ctx.data_mut(|data| data.insert_temp(id, next));
}

fn health_indicator(ctx: &egui::Context) -> HealthIndicator {
    ctx.data(|data| {
        data.get_temp::<HealthIndicatorAuthority>(egui::Id::new(HEALTH_INDICATOR_AUTHORITY_ID))
            .map_or_else(HealthIndicator::default, |state| state.visible)
    })
}

/// Live primary-battery summary folded from the off-render UPower snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveBatteryStatus {
    percent: u8,
    state: mde_seat::BatteryState,
}

impl LiveBatteryStatus {
    pub(crate) fn from_batteries(batteries: &[mde_seat::Battery]) -> Option<Self> {
        let battery = batteries.iter().find(|battery| battery.power_supply)?;
        if !battery.percentage.is_finite() {
            return None;
        }
        Some(Self {
            percent: battery.percentage.clamp(0.0, 100.0).round() as u8,
            state: battery.state,
        })
    }

    const fn icon(self) -> IconId {
        if matches!(
            self.state,
            mde_seat::BatteryState::Charging | mde_seat::BatteryState::PendingCharge
        ) {
            return IconId::BatteryBolt;
        }
        match self.percent {
            0..=10 => IconId::BatteryEmpty,
            11..=35 => IconId::BatteryQuarter,
            36..=60 => IconId::BatteryHalf,
            61..=85 => IconId::BatteryThreeQuarter,
            _ => IconId::BatteryFull,
        }
    }
}

/// Render-ready weather launcher state admitted from the existing typed
/// effective-location and current-condition projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveWeatherStatus {
    icon: IconId,
    temperature: Option<String>,
    label: String,
    tone: WeatherTone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WeatherTone {
    Live,
    Stale,
    Unavailable,
}

impl WeatherTone {
    fn foreground(self, ctx: &egui::Context, normal: egui::Color32) -> egui::Color32 {
        match self {
            Self::Live => normal,
            Self::Stale => Style::resolve_color(ctx, Style::TEXT_DIM),
            Self::Unavailable => Style::resolve_color(ctx, Style::DISABLED),
        }
    }

    fn taskbar_foreground(self) -> egui::Color32 {
        match self {
            Self::Live => Style::NAV_BAR_ICON,
            Self::Stale => Style::NAV_BAR_ICON.gamma_multiply(0.68),
            Self::Unavailable => Style::NAV_BAR_ICON.gamma_multiply(0.48),
        }
    }
}

impl LiveWeatherStatus {
    pub(crate) fn unavailable() -> Self {
        Self {
            icon: IconId::WeatherUnavailable,
            temperature: None,
            label: "Weather unavailable — open Maps & Location".to_string(),
            tone: WeatherTone::Unavailable,
        }
    }

    pub(crate) fn from_projections(
        host: &str,
        location: Option<&mackes_mesh_types::location::EffectiveLocationSnapshot>,
        current: Option<&mackes_mesh_types::weather::CurrentWeatherSnapshot>,
        now_ms: i64,
    ) -> Self {
        use mackes_mesh_types::location::EffectiveLocationState;
        use mackes_mesh_types::weather::{TemperatureUnit, WeatherAvailability};

        let (Some(location), Some(current)) = (location, current) else {
            return Self::unavailable();
        };
        let effective_location = match &location.state {
            EffectiveLocationState::Available { location }
            | EffectiveLocationState::Stale { location, .. } => location,
            EffectiveLocationState::Unavailable { .. } => return Self::unavailable(),
        };
        if location.validate_at(now_ms).is_err()
            || current.validate_at(now_ms).is_err()
            || location.host != host
            || current.host != host
            || current.location_generation != location.generation
            || current.location_point.as_ref() != Some(&effective_location.point)
            || matches!(
                &current.availability,
                WeatherAvailability::Unavailable { .. }
            )
        {
            return Self::unavailable();
        }
        let Some(conditions) = current.conditions.as_ref() else {
            return Self::unavailable();
        };
        let temperature = conditions.temperature.map(|value| {
            let unit = match value.unit {
                TemperatureUnit::Celsius => "°C",
                TemperatureUnit::Fahrenheit => "°F",
            };
            format!("{:.0}{unit}", value.value)
        });
        let condition = conditions.provider_text.as_deref().unwrap_or("Weather");
        let (freshness, tone) = match &current.availability {
            WeatherAvailability::Fresh => ("live", WeatherTone::Live),
            WeatherAvailability::Stale { .. } => ("stale", WeatherTone::Stale),
            WeatherAvailability::Unavailable { .. } => unreachable!("filtered above"),
        };
        let temperature_label = temperature
            .as_deref()
            .map_or(String::new(), |value| format!(" · {value}"));
        Self {
            icon: weather_icon(conditions.condition),
            temperature,
            label: format!(
                "{condition}{temperature_label} · {freshness} — open Maps & Location Weather"
            ),
            tone,
        }
    }
}

const fn weather_icon(condition: mackes_mesh_types::weather::WeatherConditionKind) -> IconId {
    use mackes_mesh_types::weather::WeatherConditionKind;

    match condition {
        WeatherConditionKind::ClearDay => IconId::WeatherClearDay,
        WeatherConditionKind::ClearNight => IconId::WeatherClearNight,
        WeatherConditionKind::Clouds => IconId::WeatherClouds,
        WeatherConditionKind::Rain => IconId::WeatherRain,
        WeatherConditionKind::Wintry => IconId::WeatherWintry,
        WeatherConditionKind::Storm => IconId::WeatherStorm,
        WeatherConditionKind::Fog => IconId::WeatherFog,
        WeatherConditionKind::Wind => IconId::WeatherWind,
        WeatherConditionKind::Unavailable => IconId::WeatherUnavailable,
    }
}

fn is_status_format_control(ch: char) -> bool {
    ch.is_control() || matches!(ch, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Keep status text single-line and bounded before it reaches egui's font
/// shaper. The current status read model already folds severities to a
/// canonical vocabulary; this is a final presentation boundary for future
/// labels and hostile control/bidi characters. An ellipsis makes truncation
/// truthful instead of silently dropping the value.
fn safe_status_text(raw: &str) -> String {
    let limit = MAX_STATUS_TEXT_CHARS.saturating_sub(1);
    let mut out = String::new();
    let mut chars = raw.chars().filter(|ch| !is_status_format_control(*ch));
    for _ in 0..limit {
        let Some(ch) = chars.next() else {
            return out;
        };
        out.push(ch);
    }
    if chars.next().is_some() {
        out.push('\u{2026}');
    }
    out
}

/// Build a rail-safe, single-row galley. `max_rows = 1` is important here:
/// wrapping would increase the 24px strip's height and make the painted text
/// disagree with its hit-test band on a narrow surface.
fn status_text_job(
    raw: impl Into<String>,
    role: TypographyRole,
    color: egui::Color32,
    max_width: f32,
) -> egui::text::LayoutJob {
    let max_width = if max_width.is_finite() {
        max_width.max(1.0)
    } else {
        1.0
    };
    let mut job = Style::typography_job(safe_status_text(&raw.into()), role, color, max_width);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
    job.break_on_newline = false;
    job
}

fn finite_non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn status_controls_width() -> f32 {
    STATUS_CONTROL_W
}

/// Fit the status-menu trigger to the available rail without changing its
/// normal macOS-sized geometry on a real workstation. Tiny headless/windowed
/// surfaces still get a bounded hit target instead of extending past the
/// top-bar edge.
fn status_menu_width(bar: egui::Rect) -> f32 {
    // Scale the side inset down with the viewport. For a bar narrower than
    // both insets there is no honest room for a positive hit target; return
    // zero-sized targets at the center rather than placing them outside the
    // bar (which would steal clicks from the workspace).
    let width = finite_non_negative(bar.width());
    let inset = Style::SP_S.min(width / 2.0);
    // Leave a clock lane before allocating the right-hand controls. On a
    // narrow window the old calculation let the controls span the centered
    // clock; since those interactions are registered later, they stole clock
    // clicks even though the clock remained visibly centered.
    let available = (width - inset * 2.0 - STATUS_CONTROL_GAP - STATUS_CLOCK_MIN_W).max(0.0);
    STATUS_CONTROL_W.min(available)
}

fn status_menu_rect(bar: egui::Rect) -> egui::Rect {
    let width = finite_non_negative(bar.width());
    let inset = Style::SP_S.min(width / 2.0);
    let left = bar.left() + inset;
    let right = bar.right() - inset;
    let total = status_menu_width(bar);
    let controls_left = (right - total).max(left).min(right);
    egui::Rect::from_min_max(
        egui::pos2(controls_left, bar.top()),
        egui::pos2((controls_left + total).min(right), bar.bottom()),
    )
}

fn workspace_tray_width() -> f32 {
    WORKSPACE_TRAY_ICON_W * WORKSPACE_TRAY_SURFACES.len() as f32
        + WORKSPACE_TRAY_GAP * WORKSPACE_TRAY_SURFACES.len().saturating_sub(1) as f32
}

fn workspace_tray_rect(bar: egui::Rect, left_anchor: egui::Rect) -> egui::Rect {
    let menu = status_menu_rect(bar);
    let right = (menu.left() - WORKSPACE_TRAY_GAP).max(bar.left());
    let left = (right - workspace_tray_width())
        .max((left_anchor.right() + WORKSPACE_TRAY_GAP).min(right))
        .max(bar.left());
    egui::Rect::from_min_max(
        egui::pos2(left, bar.top()),
        egui::pos2(right.max(left), bar.bottom()),
    )
}

const fn workspace_tray_shortcut(surface: Surface) -> &'static str {
    match surface {
        Surface::InfraCode => "Status tray",
        Surface::Workers => "Super+1",
        Surface::FleetMesh => "Super+1",
        Surface::Music => "Super+4",
        Surface::Media => "Super+5",
        Surface::ThisNode => "Super+Shift+2",
        _ => "Front Door search",
    }
}

fn workspace_tray_status(surface: Surface, active: Option<Surface>) -> &'static str {
    if active == Some(surface) {
        "Active"
    } else {
        "Available"
    }
}

fn workspace_tray_tooltip(surface: Surface, active: Option<Surface>) -> String {
    format!(
        "{} — {} — {}",
        surface.label(),
        workspace_tray_status(surface, active),
        workspace_tray_shortcut(surface)
    )
}

fn status_menu_id() -> egui::Id {
    egui::Id::new(("construct-status-bar", "menu"))
}

fn centered_clock_rect(bar: egui::Rect, time_width: f32) -> egui::Rect {
    let bar_width = finite_non_negative(bar.width());
    let desired_width = if time_width.is_finite() {
        time_width.max(0.0) + Style::SP_S * 2.0
    } else {
        bar_width
    };
    egui::Rect::from_center_size(
        bar.center(),
        egui::vec2(desired_width.min(bar_width), bar.height()),
    )
}

/// Keep the clock target disjoint from the right-hand controls. At normal
/// desktop widths this is exactly the centered clock. If the controls would
/// collide on a narrow surface, the clock moves into the remaining left lane
/// and is clipped there; a slightly shifted, clickable clock is preferable to
/// a centered target that later controls steal.
fn clock_target_rect(bar: egui::Rect, time_width: f32, controls: egui::Rect) -> egui::Rect {
    let centered = centered_clock_rect(bar, time_width);
    if !centered.intersects(controls) {
        return centered;
    }

    let right = (controls.left() - STATUS_CONTROL_GAP).clamp(bar.left(), bar.right());
    let width = (right - bar.left())
        .max(0.0)
        .min(finite_non_negative(centered.width()));
    egui::Rect::from_center_size(
        egui::pos2((bar.left() + right) / 2.0, bar.center().y),
        egui::vec2(width, bar.height()),
    )
}

/// Keep the rollup cluster in the lane between the centered clock and the
/// right-hand controls. On a normal workstation this returns the same
/// right-aligned geometry as the Mac-like rail. On a narrow/headless surface,
/// the lower-priority rollups are clipped to the remaining lane instead of
/// stealing the clock's target or escaping the status bar.
fn bounded_cluster_rect(
    bar: egui::Rect,
    clock: egui::Rect,
    controls: egui::Rect,
    cluster_width: f32,
) -> egui::Rect {
    let bar_left = bar.left();
    let bar_right = bar.right().max(bar_left);
    let controls_left = if controls.left().is_finite() {
        controls.left()
    } else {
        bar_right
    };
    let clock_right = if clock.right().is_finite() {
        clock.right()
    } else {
        bar_left
    };
    let right = (controls_left - Style::SP_XS).clamp(bar_left, bar_right);
    let left = (clock_right + Style::SP_XS).clamp(bar_left, right);
    let available = (right - left).max(0.0);
    let width = finite_non_negative(cluster_width).min(available);
    egui::Rect::from_min_max(
        egui::pos2(right - width, bar.top()),
        egui::pos2(right, bar.bottom()),
    )
}

/// The shell state the strip's visibility folds over — read in `main.rs`'s
/// slot (the only place with the fields) and passed by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusBarEnv {
    /// The CURTAIN-1 lock curtain is engaged.
    pub curtain_engaged: bool,
    /// The seat is in the Car layout profile (Q42).
    pub car: bool,
    /// A focused full-screen VDI or immersive Maps workspace is in front.
    pub immersive_app: bool,
}

/// The ONE visibility rule (module doc) — pure, so the auto-hide contract is
/// unit-tested without a painter.
#[must_use]
pub(crate) const fn status_bar_visible(env: StatusBarEnv) -> bool {
    !env.curtain_engaged && !env.car && !env.immersive_app
}

/// Stable id of the strip's Area.
fn status_bar_area_id() -> egui::Id {
    egui::Id::new("construct-status-bar")
}

/// Stable id of the centered clock (the direct Clock-surface trigger).
pub(crate) fn status_bar_clock_id() -> egui::Id {
    egui::Id::new(("construct-status-bar", "clock"))
}

/// Stable id of the dedicated Notification Center bell.
pub(crate) fn notification_bell_id(placement: &'static str) -> egui::Id {
    egui::Id::new(("construct-notification-bell", placement))
}

fn unread_badge_label(unread: usize) -> Option<String> {
    match unread {
        0 => None,
        1..=99 => Some(unread.to_string()),
        _ => Some("99+".to_owned()),
    }
}

/// Stable id of the one health control in a taskbar placement.
fn health_status_id(placement: &'static str) -> egui::Id {
    egui::Id::new(("system-mesh-health", placement))
}

fn live_battery_id(placement: &'static str) -> egui::Id {
    egui::Id::new(("live-battery-status", placement))
}

fn live_weather_id(placement: &'static str) -> egui::Id {
    egui::Id::new(("live-weather-launcher", placement))
}

fn weather_status_width(status: &LiveWeatherStatus, available: f32) -> f32 {
    if status.temperature.is_some() && available >= WEATHER_STATUS_W {
        WEATHER_STATUS_W
    } else {
        WEATHER_STATUS_COMPACT_W.min(available.max(0.0))
    }
}

/// Mount the strip — called every frame from `main.rs`'s
/// `mount_status_bar_slot` (the U09 contract's reserved mount point).
pub fn mount(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    health: &HealthStatus,
    env: StatusBarEnv,
) {
    let _ = mount_top_with_active(ctx, construct, segments, health, env, 1.0, None, None, None);
}

/// Mount the current top-strip treatment with an explicit cross-fade weight.
/// The dock owns the eased transition between this strip and the bottom tray;
/// the legacy [`mount`] wrapper keeps standalone status-bar callers unchanged.
pub(crate) fn mount_top(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    health: &HealthStatus,
    env: StatusBarEnv,
    opacity: f32,
) {
    let _ = mount_top_with_active(
        ctx, construct, segments, health, env, opacity, None, None, None,
    );
}

pub(crate) fn mount_top_with_active(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    health: &HealthStatus,
    env: StatusBarEnv,
    opacity: f32,
    active_surface: Option<Surface>,
    battery: Option<LiveBatteryStatus>,
    weather: Option<LiveWeatherStatus>,
) -> bool {
    // Refresh authority even while Bottom placement has faded this strip out;
    // the bottom tray reads the same state and must not preserve pre-restart
    // mutable chrome fields when the current projection is absent or hostile.
    sync_health_indicator(ctx, health);
    let visible = status_bar_visible(env);
    // The U09 chrome-contract tests drive all mount slots on a bare Context to
    // prove intent routing without opening a frame. Keep this persistent
    // paint-only slot inert until egui has initialized its fonts; the real
    // frame path always has a positive pass number. This mirrors the overlay
    // guard in control_center.rs and notification_center.rs.
    if ctx.cumulative_pass_nr() == 0 {
        return false;
    }
    // The central workspace releases its reserved band when the target state
    // becomes hidden. Do not leave a fading foreground Area over those pixels;
    // the workspace must remain clear for the entire hidden state transition.
    if !visible || opacity <= 0.0 {
        return false;
    }
    let t = Motion::animate(ctx, "construct-status-bar-visible", visible, Motion::BASE)
        * opacity.clamp(0.0, 1.0);
    if t <= 0.0 {
        return false;
    }
    let screen = ctx.screen_rect();
    let bar =
        egui::Rect::from_min_size(screen.left_top(), egui::vec2(screen.width(), STATUS_BAR_H));
    egui::Area::new(status_bar_area_id())
        .order(egui::Order::Foreground)
        .fixed_pos(bar.min)
        // Motion owns the bar fade. egui's implicit Area fade can leave the
        // first rendered widgets non-interactable while this animation runs.
        .fade_in(false)
        // Persistent chrome must not expose egui Area's default drag/click
        // behavior. The strip itself only hovers; its two explicit child
        // interactions own clicks.
        .movable(false)
        .sense(egui::Sense::hover())
        .show(ctx, |ui| {
            ui.set_min_size(bar.size());
            // Area clips default to the full screen. The status bar is
            // persistent chrome, so its children must not paint or advertise
            // hit regions outside the reserved 24px band on narrow surfaces.
            ui.set_clip_rect(bar);
            ui.set_opacity(t);
            strip(
                ui,
                bar,
                construct,
                segments,
                health,
                active_surface,
                battery,
                weather.as_ref(),
            )
        })
        .inner
}

/// Mount the Windows-style bottom tray used while the taskbar is in its bottom
/// configuration. The shell normally calls [`paint_bottom_tray`] from inside
/// the taskbar's own Area so clock/tray and app controls are one bar; this Area
/// wrapper remains for focused status-bar tests and standalone callers.
pub(crate) fn mount_bottom(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    opacity: f32,
    env: StatusBarEnv,
) {
    if ctx.cumulative_pass_nr() == 0 || opacity <= 0.0 || !status_bar_visible(env) {
        return;
    }
    let tray = bottom_tray_rect(ctx.screen_rect());
    egui::Area::new(egui::Id::new("construct-bottom-system-tray"))
        .order(egui::Order::Foreground)
        .fixed_pos(tray.min)
        .default_size(tray.size())
        .movable(false)
        .sense(egui::Sense::hover())
        .show(ctx, |ui| {
            ui.set_min_size(tray.size());
            ui.set_clip_rect(tray);
            paint_bottom_tray(ui, ctx.screen_rect(), construct, segments, opacity, env);
        });
}

/// Paint the bottom clock/tray inside the taskbar's existing foreground area.
/// Keeping this as paint-only composition prevents a second foreground bar
/// from covering the taskbar when Bottom placement is active.
pub(crate) fn paint_bottom_tray(
    ui: &egui::Ui,
    screen: egui::Rect,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    opacity: f32,
    env: StatusBarEnv,
) {
    let _ = paint_bottom_tray_with_active(
        ui, screen, construct, segments, opacity, env, None, None, None,
    );
}

pub(crate) fn paint_bottom_tray_with_active(
    ui: &egui::Ui,
    screen: egui::Rect,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    opacity: f32,
    env: StatusBarEnv,
    active_surface: Option<Surface>,
    battery: Option<LiveBatteryStatus>,
    weather: Option<&LiveWeatherStatus>,
) -> bool {
    if opacity <= 0.0 || !status_bar_visible(env) {
        return false;
    }
    bottom_tray(
        ui,
        bottom_tray_rect(screen),
        construct,
        segments,
        opacity,
        active_surface,
        battery,
        weather,
    )
}

/// Return the tray's screen-space footprint. Keeping this in the status-bar
/// module gives the navigation geometry one source of truth for the reserved
/// right-side lane and prevents hit-target overlap during animation.
pub(crate) fn bottom_tray_rect(screen: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            (screen.right() - BOTTOM_TRAY_W - Style::SP_S).max(screen.left()),
            screen.bottom() - crate::nav_bar::TASKBAR_H,
        ),
        egui::pos2(screen.right() - Style::SP_S, screen.bottom()),
    )
}

fn paint_workspace_tray(
    ui: &egui::Ui,
    tray: egui::Rect,
    construct: &mut ConstructChrome,
    active_surface: Option<Surface>,
    id_prefix: &'static str,
    foreground: egui::Color32,
) {
    let painter = ui.painter().clone();
    let hover = Style::resolve_color(ui.ctx(), Style::SURFACE_HI);
    let icon_w = WORKSPACE_TRAY_ICON_W.min(tray.height()).max(0.0);
    if icon_w <= 0.0 || tray.width() <= 0.0 {
        return;
    }
    let gap = WORKSPACE_TRAY_GAP.min(icon_w / 4.0);
    for (index, surface) in WORKSPACE_TRAY_SURFACES.into_iter().enumerate() {
        let left = tray.left() + index as f32 * (icon_w + gap);
        let rect = egui::Rect::from_min_max(
            egui::pos2(left, tray.top()),
            egui::pos2((left + icon_w).min(tray.right()), tray.bottom()),
        );
        if rect.width() <= 0.0 {
            continue;
        }
        let response = ui.interact(
            rect,
            egui::Id::new((id_prefix, "workspace", index)),
            egui::Sense::click(),
        );
        let active = active_surface == Some(surface);
        let tooltip = workspace_tray_tooltip(surface, active_surface);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tooltip.clone())
        });
        if response.hovered() {
            painter.rect_filled(rect.shrink(2.0), Style::RADIUS_S, hover);
        }
        if let Some(texture) = icon_texture(ui.ctx(), surface.icon_id(), Style::ICON_M, foreground)
        {
            let draw = egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(Style::ICON_M, Style::ICON_M),
            );
            painter.image(
                texture.id(),
                draw,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                foreground,
            );
        }
        if active {
            let indicator = egui::Rect::from_min_max(
                egui::pos2(rect.center().x - 5.0, rect.bottom() - 3.0),
                egui::pos2(rect.center().x + 5.0, rect.bottom() - 1.0),
            );
            painter.rect_filled(indicator, egui::CornerRadius::same(1), Style::ACCENT);
        }
        let _response = response.clone().on_hover_ui(move |ui| {
            mde_egui::overlay()
                .corner_radius(mde_egui::corner(Style::RADIUS_S))
                .inner_margin(Style::tooltip_margin())
                .show(ui, |ui| {
                    ui.set_max_width(Style::SP_XL * 12.0);
                    ui.label(Style::typography_text(&tooltip, TypographyRole::Caption));
                });
        });
        if response.clicked() {
            construct.request_workspace_tray(surface);
        }
    }
}

fn bottom_tray(
    ui: &egui::Ui,
    tray: egui::Rect,
    construct: &mut ConstructChrome,
    _segments: &StatusSegments,
    opacity: f32,
    active_surface: Option<Surface>,
    battery: Option<LiveBatteryStatus>,
    weather: Option<&LiveWeatherStatus>,
) -> bool {
    let painter = ui.painter().clone();
    // This is a lane within the taskbar, not a second raised card layered over
    // it. The taskbar already paints the shared backing and top hairline.
    let panel = tray;
    let surface_hi = Style::resolve_color(ui.ctx(), Style::SURFACE_HI);
    // The taskbar is an intentionally opaque black surface in both Quazar
    // schemes. Resolving the page TEXT token in Light would produce dark ink
    // on black, so keep this foreground on the taskbar palette.
    let text = Style::NAV_BAR_ICON;
    let text_dim = Style::NAV_BAR_ICON.gamma_multiply(0.68);
    let opacity = opacity.clamp(0.0, 1.0);

    let (time, date) = match crate::timers::display_unix() {
        Ok(now) => {
            let (year, month, day) = crate::calendar::civil_from_days(now.div_euclid(86_400));
            (
                crate::timers::hhmm(now),
                format!("{month:02}/{day:02}/{year:04}"),
            )
        }
        Err(_) => ("Unavailable".to_owned(), String::new()),
    };
    let clock_width = 85.8_f32.min((panel.width() * 0.30).max(39.6));
    let bell = egui::Rect::from_min_max(
        egui::pos2(
            (panel.right() - BOTTOM_TRAY_STATUS_MENU_W - NOTIFICATION_BELL_W).max(panel.left()),
            panel.top(),
        ),
        egui::pos2(
            (panel.right() - BOTTOM_TRAY_STATUS_MENU_W).max(panel.left()),
            panel.bottom(),
        ),
    );
    let clock = egui::Rect::from_min_max(
        egui::pos2(
            (bell.left() - Style::SP_XS - clock_width).max(panel.left()),
            panel.top(),
        ),
        egui::pos2(
            (bell.left() - Style::SP_XS).max(panel.left()),
            panel.bottom(),
        ),
    );
    let battery_rect = battery.map(|_| {
        let right = clock.left();
        egui::Rect::from_min_max(
            egui::pos2((right - BATTERY_STATUS_W).max(panel.left()), panel.top()),
            egui::pos2(right, panel.bottom()),
        )
    });
    if let (Some(status), Some(rect)) = (battery, battery_rect) {
        paint_live_battery(
            ui,
            rect,
            status,
            text.gamma_multiply(opacity),
            opacity,
            "bottom",
        );
    }
    let weather_right = battery_rect.map_or(clock.left(), |rect| rect.left());
    let weather_available = (weather_right
        - panel.left()
        - workspace_tray_width()
        - 34.0
        - BOTTOM_TRAY_STATUS_MENU_W
        - Style::SP_S)
        .max(0.0);
    let weather_width = weather.map_or(0.0, |status| {
        weather_status_width(status, weather_available)
    });
    let weather_rect = weather.map(|_| {
        egui::Rect::from_min_max(
            egui::pos2(weather_right - weather_width, panel.top()),
            egui::pos2(weather_right, panel.bottom()),
        )
    });
    let weather_clicked = weather.zip(weather_rect).is_some_and(|(status, rect)| {
        paint_live_weather(
            ui,
            rect,
            status,
            text.gamma_multiply(opacity),
            opacity,
            "bottom",
        )
    });
    let clock_response = ui.interact(
        clock,
        egui::Id::new(("construct-bottom-system-tray", "clock")),
        egui::Sense::click(),
    );
    clock_response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Clock {time} — open Clock"),
        )
    });
    if clock_response.hovered() {
        painter.rect_filled(
            clock.shrink(2.0),
            Style::RADIUS_S,
            surface_hi.gamma_multiply(opacity),
        );
    }
    let time_galley = painter.layout_job(status_text_job(
        time.clone(),
        TypographyRole::Label,
        text.gamma_multiply(opacity),
        clock.width(),
    ));
    let date_galley = painter.layout_job(status_text_job(
        date,
        TypographyRole::Caption,
        text_dim.gamma_multiply(opacity),
        clock.width(),
    ));
    painter.galley(
        egui::pos2(
            clock.center().x - time_galley.size().x / 2.0,
            clock.center().y - time_galley.size().y - 1.0,
        ),
        time_galley,
        text.gamma_multiply(opacity),
    );
    painter.galley(
        egui::pos2(
            clock.center().x - date_galley.size().x / 2.0,
            clock.center().y + 1.0,
        ),
        date_galley,
        text_dim.gamma_multiply(opacity),
    );
    if clock_response.clicked() {
        construct.request_workspace_tray(Surface::Clock);
    }
    paint_notification_bell(
        ui,
        bell,
        construct,
        crate::toast_bridge::unread_count(ui.ctx()),
        text.gamma_multiply(opacity),
        surface_hi.gamma_multiply(opacity),
        "bottom",
    );

    let workspace_limit = weather_rect.map_or(weather_right, |rect| rect.left())
        - BOTTOM_TRAY_STATUS_MENU_W
        - 34.0
        - Style::SP_S;
    let workspace_rect = egui::Rect::from_min_max(
        panel.left_top(),
        egui::pos2(
            (panel.left() + workspace_tray_width())
                .min(workspace_limit)
                .max(panel.left()),
            panel.bottom(),
        ),
    );
    paint_workspace_tray(
        ui,
        workspace_rect,
        construct,
        active_surface,
        "construct-bottom-system-tray",
        Style::NAV_BAR_ICON,
    );

    let health_rect = egui::Rect::from_min_size(
        egui::pos2(panel.left() + workspace_rect.width(), panel.top()),
        egui::vec2(34.0, panel.height()),
    );
    paint_health_status(ui, health_rect, construct, opacity, "bottom");

    let menu_right = weather_rect.map_or(weather_right, |rect| rect.left()) - Style::SP_XS;
    let menu_left = (menu_right - BOTTOM_TRAY_STATUS_MENU_W)
        .max(health_rect.right() + Style::SP_XS)
        .min(menu_right);
    let menu = egui::Rect::from_min_max(
        egui::pos2(menu_left, panel.top()),
        egui::pos2(menu_right, panel.bottom()),
    );
    paint_status_menu(
        ui,
        menu,
        construct,
        egui::Id::new(("construct-bottom-system-tray", "menu")),
        text.gamma_multiply(opacity),
        surface_hi.gamma_multiply(opacity),
        opacity,
    );

    ui.ctx().request_repaint_after(Duration::from_secs(
        crate::timers::secs_to_next_minute(crate::timers::now_unix()).max(1),
    ));
    weather_clicked
}

/// Paint the same accessible System and Mesh Health icon in either taskbar
/// placement. The ring reads calm at zero; warning/critical states use the
/// shared semantic palette and carry the exact numeric badge.
fn paint_health_status(
    ui: &egui::Ui,
    rect: egui::Rect,
    construct: &mut ConstructChrome,
    opacity: f32,
    placement: &'static str,
) {
    let response = ui.interact(rect, health_status_id(placement), egui::Sense::click());
    let indicator = health_indicator(ui.ctx());
    let count = indicator.count;
    let label = health_status_label(indicator.fresh(), count);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label.clone())
    });
    let response = mde_egui::hover_text(response, label);
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(
            rect.shrink(2.0),
            Style::RADIUS_S,
            Style::SURFACE_HI.gamma_multiply(opacity),
        );
    }
    let color = match indicator.severity() {
        Some(mackes_mesh_types::health::HealthSeverity::Critical) => Style::SUPPORT_ERROR,
        Some(mackes_mesh_types::health::HealthSeverity::Warning) => Style::SUPPORT_WARNING,
        None if indicator.fresh() => Style::SUPPORT_SUCCESS,
        None => Style::TEXT_DIM,
    }
    .gamma_multiply(opacity);
    let center = rect.center();
    painter.circle_stroke(center, 6.2, egui::Stroke::new(1.8, color));
    painter.line_segment(
        [
            egui::pos2(center.x - 3.0, center.y),
            egui::pos2(center.x - 0.5, center.y + 2.5),
        ],
        egui::Stroke::new(1.6, color),
    );
    painter.line_segment(
        [
            egui::pos2(center.x - 0.5, center.y + 2.5),
            egui::pos2(center.x + 3.8, center.y - 3.0),
        ],
        egui::Stroke::new(1.6, color),
    );
    if count > 0 {
        let badge = egui::pos2(rect.right() - 6.0, rect.top() + 7.0);
        painter.circle_filled(badge, 6.0, color);
        painter.text(
            badge,
            egui::Align2::CENTER_CENTER,
            count.to_string(),
            Style::typography_font(TypographyRole::Caption),
            Style::BG,
        );
    }
    if response.clicked() {
        construct.open_health();
    }
}

fn health_status_label(fresh: bool, count: usize) -> String {
    if fresh {
        format!(
            "System and Mesh Health: {count} active unacknowledged {}",
            if count == 1 { "issue" } else { "issues" }
        )
    } else {
        "System and Mesh Health: evidence stale".to_string()
    }
}

/// Paint the one status-menu trigger shared by both taskbar placements.
/// Its Control Center owns the live status details and actions.
fn paint_status_menu(
    ui: &egui::Ui,
    rect: egui::Rect,
    construct: &mut ConstructChrome,
    id: egui::Id,
    foreground: egui::Color32,
    hover: egui::Color32,
    opacity: f32,
) {
    let response = ui.interact(rect, id, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), STATUS_MENU_LABEL)
    });
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect.shrink(2.0), Style::RADIUS_S, hover);
    }
    if let Some(texture) = icon_texture(ui.ctx(), STATUS_MENU_ICON, STATUS_CONTROL_ICON, foreground)
    {
        let draw = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(STATUS_CONTROL_ICON, STATUS_CONTROL_ICON),
        );
        painter.image(
            texture.id(),
            draw,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            foreground,
        );
    } else {
        // The bundled glyph loader fails soft; retain a visible, honest
        // interaction target rather than manufacturing a status value.
        painter.circle_filled(
            rect.center(),
            Style::SP_XS,
            Style::TEXT_DIM.gamma_multiply(opacity),
        );
    }
    if response.clicked() {
        construct.control_center_open = !construct.control_center_open;
    }
}

/// Paint the sole persistent Notification Center target. Its unread badge is
/// a bounded presentation of the existing in-memory notification ring.
fn paint_notification_bell(
    ui: &egui::Ui,
    rect: egui::Rect,
    construct: &mut ConstructChrome,
    unread: usize,
    foreground: egui::Color32,
    hover: egui::Color32,
    placement: &'static str,
) {
    let label = match unread_badge_label(unread) {
        Some(badge) => format!("Notifications, {badge} unread"),
        None => "Notifications, no unread alerts".to_owned(),
    };
    let response = ui.interact(rect, notification_bell_id(placement), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label.clone())
    });
    let response = mde_egui::hover_text(response, label);
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect.shrink(2.0), Style::RADIUS_S, hover);
    }
    if let Some(texture) = icon_texture(
        ui.ctx(),
        IconId::Notifications,
        STATUS_CONTROL_ICON,
        foreground,
    ) {
        let draw = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(STATUS_CONTROL_ICON, STATUS_CONTROL_ICON),
        );
        painter.image(
            texture.id(),
            draw,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            foreground,
        );
    }
    if let Some(badge) = unread_badge_label(unread) {
        let badge_width = if badge == "99+" { 24.0 } else { 14.0 };
        let badge_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - badge_width / 2.0, rect.top() + 7.0),
            egui::vec2(badge_width, 14.0),
        );
        painter.rect_filled(badge_rect, 7.0, Style::SUPPORT_ERROR);
        painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            badge,
            Style::typography_font(TypographyRole::Caption),
            Style::BG,
        );
    }
    if response.clicked() {
        construct.notification_center_open = true;
    }
}

/// Paint the Mesh Teams launcher beside the notification bell. Navigation is
/// delegated to the existing Communications surface; this control owns no
/// second Teams state or presentation.
fn paint_mesh_teams_launcher(
    ui: &egui::Ui,
    rect: egui::Rect,
    construct: &mut ConstructChrome,
    active: bool,
    foreground: egui::Color32,
    hover: egui::Color32,
) {
    let label = "Mesh Teams — open Mesh Teams";
    let response = ui.interact(
        rect,
        egui::Id::new(("construct-status-bar", "mesh-teams")),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let response = mde_egui::hover_text(response, label);
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect.shrink(2.0), Style::RADIUS_S, hover);
    }
    if let Some(texture) = icon_texture(
        ui.ctx(),
        Surface::Communications.icon_id(),
        STATUS_CONTROL_ICON,
        foreground,
    ) {
        let draw = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(STATUS_CONTROL_ICON, STATUS_CONTROL_ICON),
        );
        painter.image(
            texture.id(),
            draw,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            foreground,
        );
    }
    if active {
        let indicator = egui::Rect::from_min_max(
            egui::pos2(rect.center().x - 5.0, rect.bottom() - 3.0),
            egui::pos2(rect.center().x + 5.0, rect.bottom() - 1.0),
        );
        painter.rect_filled(indicator, egui::CornerRadius::same(1), Style::ACCENT);
    }
    if response.clicked() {
        construct.request_workspace_tray(Surface::Communications);
    }
}

/// Paint the latest primary UPower battery observation immediately before the
/// clock. No observation means no indicator; the shell never invents a charge.
fn paint_live_battery(
    ui: &egui::Ui,
    rect: egui::Rect,
    battery: LiveBatteryStatus,
    foreground: egui::Color32,
    opacity: f32,
    placement: &'static str,
) {
    let label = format!("Battery {}% — {}", battery.percent, battery.state.label());
    let response = ui.interact(rect, live_battery_id(placement), egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), label.clone())
    });
    let response = mde_egui::hover_text(response, label);
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(
            rect.shrink(2.0),
            Style::RADIUS_S,
            Style::SURFACE_HI.gamma_multiply(opacity),
        );
    }

    let icon_edge = Style::ICON_M.min(rect.height());
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + icon_edge / 2.0, rect.center().y),
        egui::vec2(icon_edge, icon_edge),
    );
    if let Some(texture) = icon_texture(ui.ctx(), battery.icon(), icon_edge, foreground) {
        painter.image(
            texture.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            foreground,
        );
    }
    let percent = format!("{}%", battery.percent);
    let galley = painter.layout_job(status_text_job(
        percent,
        TypographyRole::Caption,
        foreground,
        (rect.width() - icon_edge).max(0.0),
    ));
    painter.with_clip_rect(rect).galley(
        egui::pos2(icon_rect.right(), rect.center().y - galley.size().y / 2.0),
        galley,
        foreground,
    );
}

/// Paint the single launcher for Maps' existing Weather mode. The target owns
/// only navigation; it never opens a shell flyout or presents a second weather
/// surface.
fn paint_live_weather(
    ui: &egui::Ui,
    rect: egui::Rect,
    weather: &LiveWeatherStatus,
    foreground: egui::Color32,
    opacity: f32,
    placement: &'static str,
) -> bool {
    if rect.width() <= 0.0 {
        return false;
    }
    let response = ui.interact(rect, live_weather_id(placement), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            weather.label.clone(),
        )
    });
    let response = mde_egui::hover_text(response, weather.label.clone());
    let painter = ui.painter();
    let foreground = if placement == "bottom" {
        weather.tone.taskbar_foreground()
    } else {
        weather.tone.foreground(ui.ctx(), foreground)
    }
    .gamma_multiply(opacity);
    if response.hovered() {
        painter.rect_filled(
            rect.shrink(2.0),
            Style::RADIUS_S,
            Style::SURFACE_HI.gamma_multiply(opacity),
        );
    }
    let icon_edge = Style::ICON_M.min(rect.height());
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + WEATHER_STATUS_COMPACT_W / 2.0,
            rect.center().y,
        ),
        egui::vec2(icon_edge, icon_edge),
    );
    if let Some(texture) = icon_texture(ui.ctx(), weather.icon, icon_edge, foreground) {
        painter.image(
            texture.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            foreground,
        );
    }
    if rect.width() > WEATHER_STATUS_COMPACT_W {
        if let Some(temperature) = weather.temperature.as_ref() {
            let galley = painter.layout_job(status_text_job(
                temperature.clone(),
                TypographyRole::Caption,
                foreground,
                (rect.width() - WEATHER_STATUS_COMPACT_W).max(0.0),
            ));
            painter.with_clip_rect(rect).galley(
                egui::pos2(
                    rect.left() + WEATHER_STATUS_COMPACT_W,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                foreground,
            );
        }
    }
    response.clicked()
}

/// Paint + interact the strip body. Absolute screen-space rects throughout
/// (the dock's WIN7-DESKTOP-1 lesson: an Area's `fixed_pos` only seeds the Ui,
/// `ui.painter()`/`ui.interact` stay absolute).
fn strip(
    ui: &egui::Ui,
    bar: egui::Rect,
    construct: &mut ConstructChrome,
    _segments: &StatusSegments,
    _health: &HealthStatus,
    active_surface: Option<Surface>,
    battery: Option<LiveBatteryStatus>,
    weather: Option<&LiveWeatherStatus>,
) -> bool {
    let painter = ui.painter().clone();
    let background = Style::resolve_color(ui.ctx(), Style::BG);
    let border = Style::resolve_color(ui.ctx(), Style::BORDER);
    let text = Style::resolve_color(ui.ctx(), Style::TEXT);
    let surface_hi = Style::resolve_color(ui.ctx(), Style::SURFACE_HI);
    // The clean BG band + bottom hairline (module doc: persistent chrome, not
    // an overlay — no scrim, guaranteed contrast).
    painter.rect_filled(bar, egui::CornerRadius::ZERO, background);
    painter.hline(
        bar.left()..=bar.right(),
        bar.bottom(),
        egui::Stroke::new(1.0, border),
    );
    let cy = bar.center().y;
    // ── Center cluster: the one authoritative clock ────────────────────────
    let controls_rect = status_menu_rect(bar);
    let time = crate::timers::display_unix()
        .map(crate::timers::hhmm)
        .unwrap_or_else(|_| "Unavailable".to_owned());
    let time_galley = painter.layout_job(status_text_job(
        time.clone(),
        TypographyRole::Label,
        text,
        bar.width(),
    ));
    let time_w = time_galley.size().x;
    // Reserve the complete health/bell/Teams cluster when finding the clock
    // lane. Without this reservation the newly-adjacent health target could
    // steal the clock's hit region on narrow bars.
    let launcher_cluster_w = STATUS_LAUNCHER_W * 3.0 + STATUS_CONTROL_GAP * 4.0;
    let clock_controls = egui::Rect::from_min_max(
        egui::pos2(
            (controls_rect.left() - launcher_cluster_w).max(bar.left()),
            controls_rect.top(),
        ),
        controls_rect.max,
    );
    let clock_rect = clock_target_rect(bar, time_w, clock_controls);
    let bell_left =
        (clock_rect.right() + STATUS_CONTROL_GAP + STATUS_LAUNCHER_W + STATUS_CONTROL_GAP)
            .min(controls_rect.left());
    let bell_rect = egui::Rect::from_min_max(
        egui::pos2(bell_left, bar.top()),
        egui::pos2(
            (bell_left + NOTIFICATION_BELL_W).min(controls_rect.left()),
            bar.bottom(),
        ),
    );
    let battery_rect = battery.map(|_| {
        egui::Rect::from_min_max(
            egui::pos2(
                (clock_rect.left() - BATTERY_STATUS_W).max(bar.left()),
                bar.top(),
            ),
            egui::pos2(clock_rect.left(), bar.bottom()),
        )
    });
    if let (Some(status), Some(rect)) = (battery, battery_rect) {
        paint_live_battery(ui, rect, status, text, 1.0, "top");
    }
    let weather_right = battery_rect.map_or(clock_rect.left(), |rect| rect.left());
    let weather_available = (weather_right - bar.left()).max(0.0);
    let weather_width = weather.map_or(0.0, |status| {
        weather_status_width(status, weather_available)
    });
    let weather_rect = weather.map(|_| {
        egui::Rect::from_min_max(
            egui::pos2(weather_right - weather_width, bar.top()),
            egui::pos2(weather_right, bar.bottom()),
        )
    });
    let weather_clicked = weather
        .zip(weather_rect)
        .is_some_and(|(status, rect)| paint_live_weather(ui, rect, status, text, 1.0, "top"));
    let clock = ui.interact(clock_rect, status_bar_clock_id(), egui::Sense::click());
    clock.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Clock {time} — open Clock"),
        )
    });
    if clock.hovered() {
        painter.rect_filled(clock_rect.shrink(2.0), Style::RADIUS_S, surface_hi);
    }
    // A narrow fallback lane may be smaller than the clock text. Clip the
    // paint as well as the interaction so text cannot visually enter the
    // controls' lane or the workspace outside the reserved rail.
    painter.with_clip_rect(clock_rect).galley(
        egui::pos2(
            clock_rect.center().x - time_galley.size().x / 2.0,
            cy - time_galley.size().y / 2.0,
        ),
        time_galley,
        text,
    );
    if clock.clicked() {
        construct.request_workspace_tray(Surface::Clock);
    }
    paint_notification_bell(
        ui,
        bell_rect,
        construct,
        crate::toast_bridge::unread_count(ui.ctx()),
        text,
        surface_hi,
        "left",
    );

    // Keep the three launchers together in the requested order:
    // System and Mesh Health, Notification, Mesh Teams. Each uses the same
    // compact hit target; the health badge remains the exact live count.
    let health_rect = egui::Rect::from_min_max(
        egui::pos2(
            (bell_rect.left() - STATUS_CONTROL_GAP - STATUS_LAUNCHER_W).max(bar.left()),
            bar.top(),
        ),
        egui::pos2(
            (bell_rect.left() - STATUS_CONTROL_GAP).max(bar.left()),
            bar.bottom(),
        ),
    );
    paint_health_status(ui, health_rect, construct, 1.0, "top");

    let teams_rect = egui::Rect::from_min_max(
        egui::pos2(
            (bell_rect.right() + STATUS_CONTROL_GAP).min(controls_rect.left()),
            bar.top(),
        ),
        egui::pos2(
            (bell_rect.right() + STATUS_CONTROL_GAP + STATUS_LAUNCHER_W).min(controls_rect.left()),
            bar.bottom(),
        ),
    );
    paint_mesh_teams_launcher(
        ui,
        teams_rect,
        construct,
        active_surface == Some(Surface::Communications),
        text,
        surface_hi,
    );

    let workspace_rect = workspace_tray_rect(bar, teams_rect);

    paint_workspace_tray(
        ui,
        workspace_rect,
        construct,
        active_surface,
        "construct-status-bar",
        text,
    );

    paint_status_menu(
        ui,
        status_menu_rect(bar),
        construct,
        status_menu_id(),
        text,
        surface_hi,
        1.0,
    );

    // Wake at the next minute rollover so the painted minute is never stale
    // (the dock tray clock's idiom).
    ui.ctx().request_repaint_after(Duration::from_secs(
        crate::timers::secs_to_next_minute(crate::timers::now_unix()).max(1),
    ));
    weather_clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::HealthStatus;

    fn battery(
        percentage: f64,
        state: mde_seat::BatteryState,
        power_supply: bool,
    ) -> mde_seat::Battery {
        mde_seat::Battery {
            model: "test battery".to_owned(),
            kind: mde_seat::BatteryKind::Internal,
            percentage,
            state,
            power_supply,
            time_to_empty: None,
            time_to_full: None,
            energy_rate: None,
        }
    }

    fn visible_env() -> StatusBarEnv {
        StatusBarEnv {
            curtain_engaged: false,
            car: false,
            immersive_app: false,
        }
    }

    /// Drive ONE headless frame of the strip through the house `Context::run`
    /// harness, minus the stand-in surface.
    fn drive(
        ctx: &egui::Context,
        construct: &mut ConstructChrome,
        segments: &StatusSegments,
        grades: &HealthStatus,
        env: StatusBarEnv,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        drive_at(
            ctx,
            construct,
            segments,
            grades,
            env,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)),
            events,
        )
    }

    fn drive_at(
        ctx: &egui::Context,
        construct: &mut ConstructChrome,
        segments: &StatusSegments,
        grades: &HealthStatus,
        env: StatusBarEnv,
        screen: egui::Rect,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| mount(ctx, construct, segments, grades, env))
    }

    fn collect_texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
            egui::Shape::Vec(v) => {
                for s in v {
                    collect_texts(s, out);
                }
            }
            _ => {}
        }
    }

    fn frame_texts(out: &egui::FullOutput) -> Vec<String> {
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Press-then-release a primary click at `pos` (the dock's two-frame
    /// `click_rail_cell` idiom).
    fn click(
        ctx: &egui::Context,
        construct: &mut ConstructChrome,
        segments: &StatusSegments,
        grades: &HealthStatus,
        pos: egui::Pos2,
    ) {
        click_at(
            ctx,
            construct,
            segments,
            grades,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)),
            pos,
        );
    }

    fn click_at(
        ctx: &egui::Context,
        construct: &mut ConstructChrome,
        segments: &StatusSegments,
        grades: &HealthStatus,
        screen: egui::Rect,
        pos: egui::Pos2,
    ) {
        let press = egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        let release = egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        };
        let _ = drive_at(
            ctx,
            construct,
            segments,
            grades,
            visible_env(),
            screen,
            vec![egui::Event::PointerMoved(pos), press],
        );
        let _ = drive_at(
            ctx,
            construct,
            segments,
            grades,
            visible_env(),
            screen,
            vec![egui::Event::PointerMoved(pos), release],
        );
    }

    #[test]
    fn the_status_bar_is_the_locked_24px_strip() {
        // Q12 — "~24px". Pinned so a future change is a conscious edit here.
        assert!((STATUS_BAR_H - 24.0).abs() < f32::EPSILON);
    }

    #[test]
    fn bottom_tray_uses_clock_and_icon_health_without_a_grade_label() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let mut output = egui::FullOutput::default();
        for _ in 0..3 {
            output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| mount_bottom(ctx, &mut construct, &segments, 1.0, visible_env()),
            );
        }
        let texts = frame_texts(&output);
        assert!(texts.iter().any(|text| text.contains(':')));
        assert!(texts.iter().any(|text| text.contains('/')));
        assert!(
            !texts
                .iter()
                .any(|text| ["A", "B", "C", "D", "F"].contains(&text.as_str())),
            "the bottom tray must not list the node grade: {texts:?}"
        );
        assert!(
            ctx.read_response(egui::Id::new(("construct-bottom-system-tray", "clock")))
                .is_some(),
            "the Windows-style clock must remain a reachable tray target"
        );
        assert!(
            ctx.read_response(health_status_id("bottom")).is_some(),
            "mesh health must remain a keyboard/click reachable target"
        );
    }

    #[test]
    fn live_battery_uses_primary_upower_reading_and_charging_icon() {
        let batteries = [
            battery(7.0, mde_seat::BatteryState::Discharging, false),
            battery(72.6, mde_seat::BatteryState::Charging, true),
        ];
        let status = LiveBatteryStatus::from_batteries(&batteries).expect("primary battery");
        assert_eq!(status.percent, 73);
        assert_eq!(status.state, mde_seat::BatteryState::Charging);
        assert_eq!(status.icon(), IconId::BatteryBolt);
        assert!(LiveBatteryStatus::from_batteries(&[battery(
            7.0,
            mde_seat::BatteryState::Discharging,
            false,
        )])
        .is_none());
        assert!(LiveBatteryStatus::from_batteries(&[battery(
            f64::NAN,
            mde_seat::BatteryState::Unknown,
            true,
        )])
        .is_none());
    }

    #[test]
    fn weather_projection_is_generation_scoped_fresh_or_explicitly_stale() {
        use mackes_mesh_types::location::{
            EffectiveLocationProvenance, EffectiveLocationSnapshot, EffectiveLocationState,
            EffectiveWeatherLocation, WeatherCoverage, WeatherLocationMode,
            WEATHER_LOCATION_SCHEMA_VERSION,
        };
        use mackes_mesh_types::nws_alert::GeoPoint;
        use mackes_mesh_types::weather::{
            CurrentConditions, CurrentWeatherSnapshot, Temperature, TemperatureUnit,
            WeatherAttribution, WeatherAvailability, WeatherConditionKind, WeatherProvider,
            WeatherStaleReason, WEATHER_CONTRACT_SCHEMA_VERSION,
        };

        const NOW: i64 = 1_800_000_000_000;
        let point = GeoPoint {
            latitude: 42.36,
            longitude: -71.06,
        };
        let location = EffectiveLocationSnapshot {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            host: "seat".to_string(),
            generation: 7,
            mode: WeatherLocationMode::Manual,
            produced_at_ms: NOW - 30_000,
            state: EffectiveLocationState::Available {
                location: EffectiveWeatherLocation {
                    label: "Boston, MA".to_string(),
                    point: point.clone(),
                    time_zone: "America/New_York".to_string(),
                    coverage: WeatherCoverage::NwsUnitedStates,
                    provenance: EffectiveLocationProvenance::ManualVerifiedPlace {
                        place_id: "boston-ma".to_string(),
                    },
                    source_observed_at_ms: None,
                },
            },
        };
        let mut current = CurrentWeatherSnapshot {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: "seat".to_string(),
            location_generation: 7,
            location_point: Some(point),
            producer_at_ms: NOW - 30_000,
            fetched_at_ms: NOW - 30_000,
            availability: WeatherAvailability::Fresh,
            conditions: Some(CurrentConditions {
                observed_at_ms: NOW - 60_000,
                condition: WeatherConditionKind::ClearNight,
                provider_text: Some("Clear".to_string()),
                temperature: Some(Temperature {
                    value: 72.0,
                    unit: TemperatureUnit::Fahrenheit,
                }),
                apparent_temperature: None,
                relative_humidity_percent: None,
                precipitation_probability_percent: None,
                wind_speed: None,
                wind_direction_degrees: None,
                wind_gust: None,
                visibility: None,
                pressure: None,
            }),
            gaps: Vec::new(),
            attributions: vec![WeatherAttribution {
                provider: WeatherProvider::NationalWeatherService,
                source_id: "nws".to_string(),
                label: "National Weather Service".to_string(),
            }],
        };
        let fresh =
            LiveWeatherStatus::from_projections("seat", Some(&location), Some(&current), NOW);
        assert_eq!(fresh.icon, IconId::WeatherClearNight);
        assert_eq!(fresh.temperature.as_deref(), Some("72°F"));
        assert!(fresh.label.contains("live"));

        current.availability = WeatherAvailability::Stale {
            reason: WeatherStaleReason::RefreshFailed,
        };
        current
            .conditions
            .as_mut()
            .expect("conditions")
            .observed_at_ms = NOW - 2 * 60 * 60 * 1_000;
        let stale =
            LiveWeatherStatus::from_projections("seat", Some(&location), Some(&current), NOW);
        assert_eq!(stale.temperature.as_deref(), Some("72°F"));
        assert!(stale.label.contains("stale"));

        current.location_generation = 8;
        let unavailable =
            LiveWeatherStatus::from_projections("seat", Some(&location), Some(&current), NOW);
        assert_eq!(unavailable.icon, IconId::WeatherUnavailable);
        assert_eq!(unavailable.temperature, None);

        current.location_generation = location.generation;
        current.location_point = Some(GeoPoint {
            latitude: 41.82,
            longitude: -71.41,
        });
        let wrong_point =
            LiveWeatherStatus::from_projections("seat", Some(&location), Some(&current), NOW);
        assert_eq!(wrong_point, LiveWeatherStatus::unavailable());

        current.location_point = match &location.state {
            EffectiveLocationState::Available { location }
            | EffectiveLocationState::Stale { location, .. } => Some(location.point),
            EffectiveLocationState::Unavailable { .. } => unreachable!("fixture is available"),
        };
        let corrected_forward =
            LiveWeatherStatus::from_projections("seat", Some(&location), Some(&current), NOW);
        assert_eq!(corrected_forward.temperature.as_deref(), Some("72°F"));
        assert!(corrected_forward.label.contains("stale"));
    }

    #[test]
    fn weather_then_battery_then_time_is_disjoint_in_both_placements() {
        let battery = LiveBatteryStatus {
            percent: 64,
            state: mde_seat::BatteryState::Discharging,
        };
        let weather = LiveWeatherStatus {
            icon: IconId::DarkMode,
            temperature: Some("72°F".to_string()),
            label: "Clear · 72°F · live — open Maps & Location Weather".to_string(),
            tone: WeatherTone::Live,
        };
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));

        let top_ctx = egui::Context::default();
        Style::install(&top_ctx);
        let mut top_construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let health = HealthStatus::default();
        for _ in 0..3 {
            let _ = top_ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    mount_top_with_active(
                        ctx,
                        &mut top_construct,
                        &segments,
                        &health,
                        visible_env(),
                        1.0,
                        None,
                        Some(battery),
                        Some(weather.clone()),
                    );
                },
            );
        }
        let top_battery = top_ctx
            .read_response(live_battery_id("top"))
            .expect("top battery target");
        let top_clock = top_ctx
            .read_response(status_bar_clock_id())
            .expect("top clock target");
        let top_weather = top_ctx
            .read_response(live_weather_id("top"))
            .expect("top weather target");
        let top_bell = top_ctx
            .read_response(notification_bell_id("left"))
            .expect("left-placement bell target");
        assert_eq!(top_weather.rect.right(), top_battery.rect.left());
        assert_eq!(top_battery.rect.right(), top_clock.rect.left());
        assert!(top_weather.rect.intersect(top_battery.rect).width() <= f32::EPSILON);
        assert!(top_battery.rect.intersect(top_clock.rect).width() <= f32::EPSILON);
        assert!(top_clock.rect.right() <= top_bell.rect.left());
        assert!(top_clock.rect.intersect(top_bell.rect).width() <= f32::EPSILON);

        let bottom_ctx = egui::Context::default();
        Style::install(&bottom_ctx);
        let mut bottom_construct = ConstructChrome::default();
        for _ in 0..3 {
            let _ = bottom_ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        paint_bottom_tray_with_active(
                            ui,
                            screen,
                            &mut bottom_construct,
                            &segments,
                            1.0,
                            visible_env(),
                            None,
                            Some(battery),
                            Some(&weather),
                        );
                    });
                },
            );
        }
        let bottom_battery = bottom_ctx
            .read_response(live_battery_id("bottom"))
            .expect("bottom battery target");
        let bottom_clock = bottom_ctx
            .read_response(egui::Id::new(("construct-bottom-system-tray", "clock")))
            .expect("bottom clock target");
        let bottom_weather = bottom_ctx
            .read_response(live_weather_id("bottom"))
            .expect("bottom weather target");
        let bottom_bell = bottom_ctx
            .read_response(notification_bell_id("bottom"))
            .expect("bottom bell target");
        assert_eq!(bottom_weather.rect.right(), bottom_battery.rect.left());
        assert_eq!(bottom_battery.rect.right(), bottom_clock.rect.left());
        assert!(bottom_weather.rect.intersect(bottom_battery.rect).width() <= f32::EPSILON);
        assert!(bottom_battery.rect.intersect(bottom_clock.rect).width() <= f32::EPSILON);
        assert!(bottom_clock.rect.right() <= bottom_bell.rect.left());
        assert!(bottom_clock.rect.intersect(bottom_bell.rect).width() <= f32::EPSILON);
    }

    #[test]
    fn weather_collapses_to_icon_and_abuts_time_when_battery_is_absent() {
        let weather = LiveWeatherStatus {
            icon: IconId::Internet,
            temperature: Some("68°F".to_string()),
            label: "Cloudy · 68°F · live — open Maps & Location Weather".to_string(),
            tone: WeatherTone::Live,
        };
        assert_eq!(
            weather_status_width(&weather, WEATHER_STATUS_W),
            WEATHER_STATUS_W
        );
        assert_eq!(
            weather_status_width(&weather, WEATHER_STATUS_COMPACT_W),
            WEATHER_STATUS_COMPACT_W
        );

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 300.0));
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let health = HealthStatus::default();
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    mount_top_with_active(
                        ctx,
                        &mut construct,
                        &segments,
                        &health,
                        visible_env(),
                        1.0,
                        None,
                        None,
                        Some(weather.clone()),
                    );
                },
            );
        }
        let weather_rect = ctx
            .read_response(live_weather_id("top"))
            .expect("weather target")
            .rect;
        let clock_rect = ctx
            .read_response(status_bar_clock_id())
            .expect("clock target")
            .rect;
        assert_eq!(weather_rect.right(), clock_rect.left());
    }

    #[test]
    fn one_weather_click_emits_only_the_weather_navigation_action() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let weather = LiveWeatherStatus::unavailable();
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let health = HealthStatus::default();
        {
            let mut run = |events: Vec<egui::Event>| {
                let mut clicked = false;
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        clicked = mount_top_with_active(
                            ctx,
                            &mut construct,
                            &segments,
                            &health,
                            visible_env(),
                            1.0,
                            None,
                            None,
                            Some(weather.clone()),
                        );
                    },
                );
                clicked
            };
            for _ in 0..3 {
                assert!(!run(Vec::new()));
            }
            let pos = ctx
                .read_response(live_weather_id("top"))
                .expect("weather target")
                .rect
                .center();
            assert!(!run(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ]));
            assert!(run(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ]));
        }
        assert!(!construct.notification_center_open);
        assert!(!construct.control_center_open);
    }

    #[test]
    fn health_accessibility_copy_is_exact_and_never_calls_stale_data_zero() {
        assert_eq!(
            health_status_label(true, 0),
            "System and Mesh Health: 0 active unacknowledged issues"
        );
        assert_eq!(
            health_status_label(true, 1),
            "System and Mesh Health: 1 active unacknowledged issue"
        );
        assert_eq!(
            health_status_label(false, 0),
            "System and Mesh Health: evidence stale"
        );
    }

    #[test]
    fn restarted_status_bar_cannot_relabel_health_grade_from_foreign_or_rolled_back_generation() {
        use mackes_mesh_types::health::{
            GradeFactors, HealthComponent, HealthCondition, HealthEvidence, HealthScope,
            MeshHealthSummary,
        };

        const NOW: u64 = 1_800_000_000_000;
        fn snapshot(
            observer: &str,
            generation: u64,
            generated_at_ms: u64,
            grade: GradeLetter,
        ) -> SystemMeshHealthSnapshot {
            let conditions = if grade == GradeLetter::F {
                vec![HealthCondition {
                    id: "seat:storage".into(),
                    scope: HealthScope::Node {
                        node: "seat".into(),
                    },
                    component: HealthComponent::Resources,
                    source: "node-grade".into(),
                    severity: HealthSeverity::Critical,
                    requirement: RequirementClass::Required,
                    evidence: HealthEvidence {
                        provider: "node-grade".into(),
                        summary: "Storage evidence exceeded its governed threshold.".into(),
                        facts: BTreeMap::new(),
                        observed_at_ms: generated_at_ms,
                    },
                    active_since_ms: generated_at_ms,
                    last_observed_ms: generated_at_ms,
                    resolved_at_ms: None,
                    acknowledged_at_ms: None,
                    snoozed_until_ms: None,
                    remediation: Vec::new(),
                }]
            } else {
                Vec::new()
            };
            let node_grade = NodeGrade::evaluate(
                "seat",
                95,
                GradeFactors::default(),
                &conditions,
                generated_at_ms,
            );
            let critical = usize::from(grade == GradeLetter::F);
            SystemMeshHealthSnapshot {
                schema_version: HEALTH_SCHEMA_VERSION,
                observer: observer.into(),
                roster_revision: "roster-7".into(),
                generation,
                generated_at_ms,
                fresh_until_ms: NOW + 60_000,
                current_node_grades: vec![node_grade],
                active_conditions: conditions,
                resolved_conditions: Vec::new(),
                mesh_summary: MeshHealthSummary {
                    grade,
                    canonical_nodes: 1,
                    fresh_nodes: 1,
                    reachable_lighthouses: 1,
                    active_warnings: 0,
                    active_critical: critical,
                    unacknowledged_actionable: critical,
                },
            }
        }

        // A restarted shell may legitimately bootstrap from the latest retained
        // local projection, but that establishes a generation/provenance
        // watermark before any grade reaches persistent chrome.
        let critical = snapshot("seat", 42, NOW - 4_000, GradeLetter::F);
        let restarted = reconcile_health_indicator(None, Some(&critical), "seat", NOW);
        assert_eq!(restarted.visible.grade, Some(GradeLetter::F));

        let foreign = snapshot("other-seat", 43, NOW - 3_000, GradeLetter::A);
        let substituted =
            reconcile_health_indicator(restarted.watermark.as_ref(), Some(&foreign), "seat", NOW);
        assert_eq!(
            substituted.visible,
            HealthIndicator::default(),
            "foreign provenance must render stale, never a calm grade"
        );

        let rollback = snapshot("seat", 41, NOW - 2_000, GradeLetter::A);
        let rolled_back = reconcile_health_indicator(
            substituted.watermark.as_ref(),
            Some(&rollback),
            "seat",
            NOW,
        );
        assert_eq!(rolled_back.visible.grade, Some(GradeLetter::F));

        let equivocation = snapshot("seat", 42, NOW - 1_000, GradeLetter::A);
        let equivocated = reconcile_health_indicator(
            rolled_back.watermark.as_ref(),
            Some(&equivocation),
            "seat",
            NOW,
        );
        assert_eq!(equivocated.visible.grade, Some(GradeLetter::F));

        let corrected = snapshot("seat", 43, NOW, GradeLetter::A);
        let corrected_forward = reconcile_health_indicator(
            equivocated.watermark.as_ref(),
            Some(&corrected),
            "seat",
            NOW,
        );
        assert_eq!(corrected_forward.visible.grade, Some(GradeLetter::A));
    }

    #[test]
    fn bottom_taskbar_foreground_stays_white_when_shell_is_light() {
        // The bottom tray is painted over the shared black taskbar, so it must
        // not inherit the page's Light-mode dark text token.
        assert_eq!(
            Style::resolve_color_for_scheme(mde_egui::StyleColorScheme::Light, Style::NAV_BAR_ICON,),
            egui::Color32::WHITE
        );
        assert_eq!(Style::NAV_BAR_ICON, egui::Color32::WHITE);
    }

    #[test]
    fn stale_and_unavailable_weather_never_retain_the_live_status_tone() {
        for scheme in [
            mde_egui::StyleColorScheme::Dark,
            mde_egui::StyleColorScheme::Light,
        ] {
            let ctx = egui::Context::default();
            Style::install_color_scheme_with_density(&ctx, scheme, mde_egui::Density::Mouse);
            let live = Style::resolve_color(&ctx, Style::TEXT);
            let stale = WeatherTone::Stale.foreground(&ctx, live);
            let unavailable = WeatherTone::Unavailable.foreground(&ctx, live);

            assert_eq!(WeatherTone::Live.foreground(&ctx, live), live);
            assert_ne!(
                stale, live,
                "stale weather retained the live {scheme:?} tone"
            );
            assert_ne!(
                unavailable, live,
                "unavailable weather retained the live {scheme:?} tone"
            );
        }

        assert_ne!(
            WeatherTone::Stale.taskbar_foreground(),
            WeatherTone::Live.taskbar_foreground()
        );
        assert_ne!(
            WeatherTone::Unavailable.taskbar_foreground(),
            WeatherTone::Live.taskbar_foreground()
        );
    }

    #[test]
    fn bottom_tray_footprint_stays_inside_the_taskbar_at_small_widths() {
        for width in [96.0, 160.0, 320.0, 1280.0] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 240.0));
            let tray = bottom_tray_rect(screen);
            assert!(tray.left() >= screen.left());
            assert!(tray.right() <= screen.right());
            assert!(tray.top() >= screen.top());
            assert!(tray.bottom() <= screen.bottom());
            assert!(tray.width() <= screen.width());
            assert!(tray.height() >= 0.0);
        }
    }

    #[test]
    fn bottom_tray_clamps_narrow_layouts_and_honors_visibility() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        for size in [egui::vec2(320.0, 240.0), egui::vec2(96.0, 80.0)] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            for _ in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ctx| mount_bottom(ctx, &mut construct, &segments, 1.0, visible_env()),
                );
            }
            let clock = ctx
                .read_response(egui::Id::new(("construct-bottom-system-tray", "clock")))
                .expect("narrow tray keeps a clock target");
            assert!(clock.rect.width() >= 0.0 && clock.rect.height() >= 0.0);
            if let Some(response) =
                ctx.read_response(egui::Id::new(("construct-bottom-system-tray", "menu")))
            {
                assert!(response.rect.width() >= 0.0 && response.rect.height() >= 0.0);
            }
        }

        let hidden_ctx = egui::Context::default();
        Style::install(&hidden_ctx);
        let hidden_output = hidden_ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 240.0),
                )),
                ..Default::default()
            },
            |ctx| {
                mount_bottom(
                    ctx,
                    &mut construct,
                    &segments,
                    1.0,
                    StatusBarEnv {
                        curtain_engaged: true,
                        ..visible_env()
                    },
                );
            },
        );
        assert!(
            hidden_output.shapes.is_empty(),
            "hidden status chrome must not paint a bottom tray"
        );
    }

    #[test]
    fn visibility_is_a_pure_fold_of_curtain_car_and_immersive_apps() {
        assert!(status_bar_visible(visible_env()), "default Construct shows");
        assert!(
            !status_bar_visible(StatusBarEnv {
                curtain_engaged: true,
                ..visible_env()
            }),
            "no chrome under the lock (CURTAIN-1)"
        );
        assert!(
            !status_bar_visible(StatusBarEnv {
                car: true,
                ..visible_env()
            }),
            "Car profile owns its own chrome"
        );
        assert!(
            !status_bar_visible(StatusBarEnv {
                immersive_app: true,
                ..visible_env()
            }),
            "U24: VDI and Maps auto-hide the strip"
        );
    }

    #[test]
    fn the_strip_renders_clock_without_old_rollup_or_grade_text() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = HealthStatus::default();
        let _ = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        let _ = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        // Pass 1 initializes the Context and pass 2 is the Area's invisible
        // sizing pass; the following frame is the first painted one.
        let out = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        let texts = frame_texts(&out);
        // The centered clock. The farm can spend long enough compiling this
        // crate for the minute to roll between the before/after samples, so
        // assert the rendered HH:MM contract instead of a stale snapshot.
        assert!(
            texts.iter().any(|t| {
                t.len() == 5
                    && t.as_bytes().get(2) == Some(&b':')
                    && t.as_bytes()[..2].iter().all(u8::is_ascii_digit)
                    && t.as_bytes()[3..].iter().all(u8::is_ascii_digit)
            }),
            "no clock text painted: {texts:?}"
        );
        assert!(!texts.iter().any(|t| t == "Mesh warning"));
        assert!(!texts.iter().any(|t| t == "A"));
        // Non-empty tessellation — the strip reaches real draw primitives.
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the strip painted no draw primitives");
    }

    #[test]
    fn left_clock_opens_clock_and_dedicated_bell_opens_notifications() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = HealthStatus::default();
        let _ = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        let _ = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        // Pass 1 initializes the Context, pass 2 sizes the Area, and pass 3
        // registers the children for egui's previous-pass hit testing.
        let _ = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        let pos = ctx
            .read_response(status_bar_clock_id())
            .expect("clock cluster registered")
            .rect
            .center();
        click(&ctx, &mut construct, &segments, &grades, pos);
        assert_eq!(
            construct.take_workspace_tray_target(),
            Some(Surface::Clock),
            "clock click routes directly to Clock"
        );
        assert!(
            !construct.notification_center_open,
            "clock must not open Notification Center"
        );
        assert!(!construct.control_center_open, "CC untouched by the clock");

        let bell = ctx
            .read_response(notification_bell_id("left"))
            .expect("dedicated bell registered")
            .rect
            .center();
        click(&ctx, &mut construct, &segments, &grades, bell);
        assert!(
            construct.notification_center_open,
            "bell opens Notification Center"
        );
        assert_eq!(construct.take_workspace_tray_target(), None);
    }

    #[test]
    fn default_launcher_orders_health_notification_and_mesh_teams_with_equal_targets() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = HealthStatus::default();
        for _ in 0..3 {
            let _ = drive(
                &ctx,
                &mut construct,
                &segments,
                &grades,
                visible_env(),
                Vec::new(),
            );
        }

        let health = ctx
            .read_response(health_status_id("top"))
            .expect("health launcher registered")
            .rect;
        let bell = ctx
            .read_response(notification_bell_id("left"))
            .expect("notification launcher registered")
            .rect;
        let teams = ctx
            .read_response(egui::Id::new(("construct-status-bar", "mesh-teams")))
            .expect("Mesh Teams launcher registered")
            .rect;

        assert!(health.right() <= bell.left());
        assert!(bell.right() <= teams.left());
        assert!((health.width() - bell.width()).abs() < f32::EPSILON);
        assert!((bell.width() - teams.width()).abs() < f32::EPSILON);

        click(&ctx, &mut construct, &segments, &grades, teams.center());
        assert_eq!(
            construct.take_workspace_tray_target(),
            Some(Surface::Communications),
            "Mesh Teams launcher keeps its existing surface route"
        );
    }

    #[test]
    fn notification_badge_is_bounded_at_99_plus() {
        assert_eq!(unread_badge_label(0), None);
        assert_eq!(unread_badge_label(1).as_deref(), Some("1"));
        assert_eq!(unread_badge_label(99).as_deref(), Some("99"));
        assert_eq!(unread_badge_label(100).as_deref(), Some("99+"));
        assert_eq!(unread_badge_label(usize::MAX).as_deref(), Some("99+"));
    }

    #[test]
    fn bottom_clock_and_bell_have_distinct_routes() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let segments = StatusSegments::default();
        let mut construct = ConstructChrome::default();
        fn run(
            ctx: &egui::Context,
            construct: &mut ConstructChrome,
            segments: &StatusSegments,
            screen: egui::Rect,
            events: Vec<egui::Event>,
        ) {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| mount_bottom(ctx, construct, segments, 1.0, visible_env()),
            );
        }
        run(&ctx, &mut construct, &segments, screen, Vec::new());
        run(&ctx, &mut construct, &segments, screen, Vec::new());
        run(&ctx, &mut construct, &segments, screen, Vec::new());

        let clock = ctx
            .read_response(egui::Id::new(("construct-bottom-system-tray", "clock")))
            .expect("bottom clock registered")
            .rect
            .center();
        let press = egui::Event::PointerButton {
            pos: clock,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        let release = egui::Event::PointerButton {
            pos: clock,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        };
        run(
            &ctx,
            &mut construct,
            &segments,
            screen,
            vec![egui::Event::PointerMoved(clock), press],
        );
        run(
            &ctx,
            &mut construct,
            &segments,
            screen,
            vec![egui::Event::PointerMoved(clock), release],
        );
        assert_eq!(construct.take_workspace_tray_target(), Some(Surface::Clock));
        assert!(!construct.notification_center_open);

        let bell = ctx
            .read_response(notification_bell_id("bottom"))
            .expect("bottom bell registered")
            .rect
            .center();
        let press = egui::Event::PointerButton {
            pos: bell,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        let release = egui::Event::PointerButton {
            pos: bell,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        };
        run(
            &ctx,
            &mut construct,
            &segments,
            screen,
            vec![egui::Event::PointerMoved(bell), press],
        );
        run(
            &ctx,
            &mut construct,
            &segments,
            screen,
            vec![egui::Event::PointerMoved(bell), release],
        );
        assert!(construct.notification_center_open);
        assert_eq!(construct.take_workspace_tray_target(), None);
    }

    #[test]
    fn clicking_the_health_control_opens_the_centered_modal() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = HealthStatus::default();
        let _ = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        let _ = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        // Pass 1 initializes the Context, pass 2 sizes the Area, and pass 3
        // registers the children for egui's previous-pass hit testing.
        let _ = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            visible_env(),
            Vec::new(),
        );
        let pos = ctx
            .read_response(health_status_id("top"))
            .expect("health control registered")
            .rect
            .center();
        click(&ctx, &mut construct, &segments, &grades, pos);
        assert!(
            construct.health_modal_open,
            "health control opens the modal"
        );
        assert!(
            !construct.control_center_open,
            "health drilldown does not open Control Center"
        );
    }

    #[test]
    fn centered_clock_and_status_menu_have_deterministic_geometry() {
        let bar = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, STATUS_BAR_H));
        let clock = centered_clock_rect(bar, 48.0);
        assert!((clock.center().x - bar.center().x).abs() < f32::EPSILON);
        assert!((clock.center().y - bar.center().y).abs() < f32::EPSILON);

        let controls = status_menu_rect(bar);
        assert!((controls.right() - (bar.right() - Style::SP_S)).abs() < f32::EPSILON);
        assert!((status_controls_width() - STATUS_CONTROL_W).abs() < f32::EPSILON);
        let menu = status_menu_rect(bar);
        assert_eq!(menu, controls);
        assert_eq!(menu.top(), bar.top());
        assert_eq!(menu.bottom(), bar.bottom());
        assert!((menu.right() - controls.right()).abs() < f32::EPSILON);
    }

    #[test]
    fn narrow_clock_target_does_not_overlap_the_status_menu_target() {
        for width in [72.0, 128.0, 240.0] {
            let bar =
                egui::Rect::from_min_size(egui::pos2(73.0, 41.0), egui::vec2(width, STATUS_BAR_H));
            let controls = status_menu_rect(bar);
            let clock = clock_target_rect(bar, 48.0, controls);
            assert!(
                bar.contains_rect(clock),
                "clock escaped {width}px bar: {clock:?} vs {bar:?}"
            );
            let menu = status_menu_rect(bar);
            assert!(
                !clock.intersects(menu),
                "clock target {clock:?} overlaps status menu target {menu:?}"
            );
        }
    }

    #[test]
    fn rail_text_is_single_line_bounded_and_direction_safe() {
        let hostile = format!(
            "Device\n{}\u{202e}tail",
            "x".repeat(MAX_STATUS_TEXT_CHARS * 2)
        );
        let safe = safe_status_text(&hostile);
        assert!(safe.chars().count() <= MAX_STATUS_TEXT_CHARS);
        assert!(safe.ends_with('\u{2026}'));
        assert!(!safe.chars().any(is_status_format_control));

        let job = status_text_job(hostile, TypographyRole::Label, Style::TEXT, 32.0);
        assert_eq!(job.wrap.max_rows, 1);
        assert!(job.wrap.break_anywhere);
        assert_eq!(job.wrap.max_width, 32.0);
        assert!(!job.break_on_newline);
        assert!(!job.text.chars().any(is_status_format_control));
    }

    #[test]
    fn status_hit_targets_stay_inside_the_bar_below_the_inset_width() {
        for width in [0.0, 1.0, 7.0, 15.0, 31.0] {
            let bar =
                egui::Rect::from_min_size(egui::pos2(73.0, 41.0), egui::vec2(width, STATUS_BAR_H));
            let controls = status_menu_rect(bar);
            assert!(
                bar.contains_rect(controls),
                "control cluster escaped {width}px bar: {controls:?}"
            );
            let menu = status_menu_rect(bar);
            assert!(
                bar.contains_rect(menu),
                "status menu escaped {width}px bar: {menu:?}"
            );

            let clock = centered_clock_rect(bar, f32::INFINITY);
            let cluster = bounded_cluster_rect(bar, clock, controls, f32::MAX);
            assert!(bar.contains_rect(clock), "clock escaped {width}px bar");
            assert!(bar.contains_rect(cluster), "cluster escaped {width}px bar");
            assert!(cluster.width().is_finite());
        }
    }

    #[test]
    fn status_menu_remains_inside_a_narrow_top_bar() {
        let bar = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(72.0, STATUS_BAR_H));
        let controls = status_menu_rect(bar);
        assert!(
            bar.contains_rect(controls),
            "cluster escaped narrow bar: {controls:?}"
        );
        assert!(bar.contains_rect(status_menu_rect(bar)));
    }

    #[test]
    fn narrow_non_zero_origin_bounds_rollups_and_keeps_a_control_clickable() {
        let screen = egui::Rect::from_min_size(egui::pos2(73.0, 41.0), egui::vec2(72.0, 120.0));
        let bar =
            egui::Rect::from_min_size(screen.left_top(), egui::vec2(screen.width(), STATUS_BAR_H));
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = HealthStatus::default();

        for _ in 0..3 {
            let _ = drive_at(
                &ctx,
                &mut construct,
                &segments,
                &grades,
                visible_env(),
                screen,
                Vec::new(),
            );
        }

        for (label, id) in [
            ("clock", status_bar_clock_id()),
            ("health", health_status_id("top")),
        ] {
            let response = ctx
                .read_response(id)
                .unwrap_or_else(|| panic!("{label} target was not registered"));
            assert!(
                bar.contains_rect(response.rect),
                "{label} target escaped the narrow status bar: {:?} vs {bar:?}",
                response.rect
            );
        }
        let clock = ctx
            .read_response(status_bar_clock_id())
            .expect("clock target registered")
            .rect;
        let menu = ctx
            .read_response(status_menu_id())
            .expect("status menu target was not registered");
        assert!(
            bar.contains_rect(menu.rect),
            "status menu target escaped the narrow status bar: {:?} vs {bar:?}",
            menu.rect
        );
        assert!(
            !clock.intersects(menu.rect),
            "clock target must not steal the status menu on a narrow bar"
        );

        click_at(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            screen,
            clock.center(),
        );
        assert_eq!(
            construct.take_workspace_tray_target(),
            Some(Surface::Clock),
            "the centered clock remains directly routable beside the narrow status menu"
        );
        assert!(!construct.notification_center_open);

        click_at(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            screen,
            menu.rect.center(),
        );
        assert!(
            construct.control_center_open,
            "a non-zero-origin status menu target must remain clickable"
        );
    }

    #[test]
    fn clicking_the_status_menu_toggles_the_existing_control_center() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = HealthStatus::default();
        for _ in 0..3 {
            let _ = drive(
                &ctx,
                &mut construct,
                &segments,
                &grades,
                visible_env(),
                Vec::new(),
            );
        }
        let pos = ctx
            .read_response(status_menu_id())
            .expect("status menu registered")
            .rect
            .center();
        click(&ctx, &mut construct, &segments, &grades, pos);
        assert!(construct.control_center_open, "status menu opens CC");
        click(&ctx, &mut construct, &segments, &grades, pos);
        assert!(!construct.control_center_open, "second click closes CC");
    }

    #[test]
    fn status_menu_uses_the_existing_menu_icon() {
        assert_eq!(STATUS_MENU_ICON, IconId::Menu);
    }

    #[test]
    fn the_strip_hides_under_the_curtain_car_and_fullscreen_remote() {
        for env in [
            StatusBarEnv {
                curtain_engaged: true,
                ..visible_env()
            },
            StatusBarEnv {
                car: true,
                ..visible_env()
            },
            StatusBarEnv {
                immersive_app: true,
                ..visible_env()
            },
        ] {
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let mut construct = ConstructChrome::default();
            let segments = StatusSegments::default();
            let grades = HealthStatus::default();
            let out = drive(&ctx, &mut construct, &segments, &grades, env, Vec::new());
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(
                prims.is_empty(),
                "a hidden strip must draw nothing at all ({env:?})"
            );
        }
    }

    #[test]
    fn hiding_stops_paint_before_the_workspace_band_is_reclaimed() {
        // The central workspace removes its TopBottomPanel reservation as soon
        // as the target visibility becomes false. A prior visible frame leaves
        // Motion::animate with a non-zero fade-out value, so this same-context
        // transition catches any status-bar paint that would overlay the newly
        // available workspace pixels.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = HealthStatus::default();

        for _ in 0..3 {
            let _ = drive(
                &ctx,
                &mut construct,
                &segments,
                &grades,
                visible_env(),
                Vec::new(),
            );
        }

        let out = drive(
            &ctx,
            &mut construct,
            &segments,
            &grades,
            StatusBarEnv {
                curtain_engaged: true,
                ..visible_env()
            },
            Vec::new(),
        );
        assert!(
            out.shapes.is_empty(),
            "a hidden status bar must not fade over the reclaimed workspace"
        );
    }
}
