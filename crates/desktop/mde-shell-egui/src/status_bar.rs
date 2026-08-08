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

use std::time::Duration;

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
pub(crate) const BOTTOM_TRAY_W: f32 = 376.0;
/// Clear space between the taskbar placement control and the tray.
pub(crate) const BOTTOM_TRAY_GAP: f32 = 8.8;

/// Construct-owned workspaces promoted into the persistent notification/tool
/// tray. The navigation rail remains intact in both placement modes.
pub(crate) const WORKSPACE_TRAY_SURFACES: [Surface; 4] = TOOL_TRAY_SURFACES;
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
/// Keep a usable clock lane when a window is narrower than the normal menu
/// bar. The lane may shrink below this value on an extremely small surface,
/// but the controls must never consume the centered clock's hit target.
const STATUS_CLOCK_MIN_W: f32 = Style::SP_XL;
/// A single status cell is deliberately one line. This cap is generous for
/// the current fixed labels, but prevents a future daemon-provided label from
/// turning the rail into an unbounded layout job.
const MAX_STATUS_TEXT_CHARS: usize = 256;

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

fn workspace_tray_rect(bar: egui::Rect, clock: egui::Rect) -> egui::Rect {
    let menu = status_menu_rect(bar);
    let right = (menu.left() - WORKSPACE_TRAY_GAP).max(bar.left());
    let left = (right - workspace_tray_width())
        .max((clock.right() + WORKSPACE_TRAY_GAP).min(right))
        .max(bar.left());
    egui::Rect::from_min_max(
        egui::pos2(left, bar.top()),
        egui::pos2(right.max(left), bar.bottom()),
    )
}

const fn workspace_tray_shortcut(surface: Surface) -> &'static str {
    match surface {
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

/// Stable id of the centered clock (the Notification Center trigger).
pub(crate) fn status_bar_clock_id() -> egui::Id {
    egui::Id::new(("construct-status-bar", "clock"))
}

/// Stable id of the one health control in a taskbar placement.
fn health_status_id(placement: &'static str) -> egui::Id {
    egui::Id::new(("system-mesh-health", placement))
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
    mount_top_with_active(ctx, construct, segments, health, env, 1.0, None);
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
    mount_top_with_active(ctx, construct, segments, health, env, opacity, None);
}

pub(crate) fn mount_top_with_active(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    health: &HealthStatus,
    env: StatusBarEnv,
    opacity: f32,
    active_surface: Option<Surface>,
) {
    let visible = status_bar_visible(env);
    // The U09 chrome-contract tests drive all mount slots on a bare Context to
    // prove intent routing without opening a frame. Keep this persistent
    // paint-only slot inert until egui has initialized its fonts; the real
    // frame path always has a positive pass number. This mirrors the overlay
    // guard in control_center.rs and notification_center.rs.
    if ctx.cumulative_pass_nr() == 0 {
        return;
    }
    // The central workspace releases its reserved band when the target state
    // becomes hidden. Do not leave a fading foreground Area over those pixels;
    // the workspace must remain clear for the entire hidden state transition.
    if !visible || opacity <= 0.0 {
        return;
    }
    let t = Motion::animate(ctx, "construct-status-bar-visible", visible, Motion::BASE)
        * opacity.clamp(0.0, 1.0);
    if t <= 0.0 {
        return;
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
            strip(ui, bar, construct, segments, health, active_surface);
        });
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
    paint_bottom_tray_with_active(ui, screen, construct, segments, opacity, env, None);
}

pub(crate) fn paint_bottom_tray_with_active(
    ui: &egui::Ui,
    screen: egui::Rect,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    opacity: f32,
    env: StatusBarEnv,
    active_surface: Option<Surface>,
) {
    if opacity <= 0.0 || !status_bar_visible(env) {
        return;
    }
    bottom_tray(
        ui,
        bottom_tray_rect(screen),
        construct,
        segments,
        opacity,
        active_surface,
    );
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
) {
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

    let now = crate::timers::display_unix();
    let time = crate::timers::hhmm(now);
    let (year, month, day) = crate::chat::civil_from_days(now.div_euclid(86_400));
    let date = format!("{month:02}/{day:02}/{year:04}");
    let clock_width = 85.8_f32.min((panel.width() * 0.30).max(39.6));
    let pin_left = panel.right() - BOTTOM_TRAY_STATUS_MENU_W;
    let clock = egui::Rect::from_min_max(
        egui::pos2(
            (pin_left - Style::SP_XS - clock_width).max(panel.left()),
            panel.top(),
        ),
        egui::pos2((pin_left - Style::SP_XS).max(panel.left()), panel.bottom()),
    );
    let clock_response = ui.interact(
        clock,
        egui::Id::new(("construct-bottom-system-tray", "clock")),
        egui::Sense::click(),
    );
    clock_response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Clock {time} — Notification Center"),
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
        construct.notification_center_open = !construct.notification_center_open;
    }

    let workspace_rect = egui::Rect::from_min_max(
        panel.left_top(),
        egui::pos2(
            (panel.left() + workspace_tray_width()).min(clock.left()),
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

    let menu_right = clock.left() - Style::SP_XS;
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
        crate::timers::secs_to_next_minute(now).max(1),
    ));
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
    let count = construct.health_count;
    let label = health_status_label(construct.health_fresh, count);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label.clone())
    });
    let response = response.on_hover_text(label);
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(
            rect.shrink(2.0),
            Style::RADIUS_S,
            Style::SURFACE_HI.gamma_multiply(opacity),
        );
    }
    let color = match construct.health_severity {
        Some(mackes_mesh_types::health::HealthSeverity::Critical) => Style::SUPPORT_ERROR,
        Some(mackes_mesh_types::health::HealthSeverity::Warning) => Style::SUPPORT_WARNING,
        None if construct.health_fresh => Style::SUPPORT_SUCCESS,
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
) {
    let painter = ui.painter().clone();
    // The clean BG band + bottom hairline (module doc: persistent chrome, not
    // an overlay — no scrim, guaranteed contrast).
    painter.rect_filled(bar, egui::CornerRadius::ZERO, Style::BG);
    painter.hline(
        bar.left()..=bar.right(),
        bar.bottom(),
        egui::Stroke::new(1.0, Style::BORDER),
    );
    let cy = bar.center().y;
    // ── Center cluster: the one authoritative clock ────────────────────────
    let controls_rect = status_menu_rect(bar);
    let now = crate::timers::display_unix();
    let time = crate::timers::hhmm(now);
    let time_galley = painter.layout_job(status_text_job(
        time.clone(),
        TypographyRole::Label,
        Style::TEXT,
        bar.width(),
    ));
    let time_w = time_galley.size().x;
    let clock_rect = clock_target_rect(bar, time_w, controls_rect);
    let clock = ui.interact(clock_rect, status_bar_clock_id(), egui::Sense::click());
    clock.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Clock {time} — Notification Center"),
        )
    });
    if clock.hovered() {
        painter.rect_filled(clock_rect.shrink(2.0), Style::RADIUS_S, Style::SURFACE_HI);
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
        Style::TEXT,
    );
    if clock.clicked() {
        // PLATFORM-INTERFACES §2.3 — "Notification Center | click status-bar
        // clock": the pub open flag IS the sanctioned seam.
        construct.notification_center_open = !construct.notification_center_open;
    }

    // One logical health affordance replaces the grade/status cluster. The
    // grade stays available in the modal, while the badge is the exact active,
    // unacknowledged actionable count.
    let workspace_rect = workspace_tray_rect(bar, clock_rect);
    let cluster_rect = bounded_cluster_rect(
        bar,
        clock_rect,
        egui::Rect::from_min_max(
            egui::pos2(bar.left(), bar.top()),
            egui::pos2(workspace_rect.left(), bar.bottom()),
        ),
        36.0,
    );
    paint_health_status(ui, cluster_rect, construct, 1.0, "top");

    paint_workspace_tray(
        ui,
        workspace_rect,
        construct,
        active_surface,
        "construct-status-bar",
        Style::resolve_color(ui.ctx(), Style::TEXT),
    );

    paint_status_menu(
        ui,
        status_menu_rect(bar),
        construct,
        status_menu_id(),
        Style::TEXT,
        Style::SURFACE_HI,
        1.0,
    );

    // Wake at the next minute rollover so the painted minute is never stale
    // (the dock tray clock's idiom).
    ui.ctx().request_repaint_after(Duration::from_secs(
        crate::timers::secs_to_next_minute(now).max(1),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::HealthStatus;

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
    fn clicking_the_clock_toggles_the_notification_center() {
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
        // PLATFORM-INTERFACES §2.3 — clock click = Notification Center.
        assert!(construct.notification_center_open, "clock click opens NC");
        assert!(!construct.control_center_open, "CC untouched by the clock");
        click(&ctx, &mut construct, &segments, &grades, pos);
        assert!(!construct.notification_center_open, "second click closes");
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
        assert!(
            construct.notification_center_open,
            "the centered clock remains clickable beside the narrow status menu"
        );

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
