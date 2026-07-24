//! `status_bar` — WL-UX-006/U11: the **Construct slim top status bar**.
//!
//! Authority: `docs/design/platform-interfaces.md` §2.3 (Q12): a ~24px
//! HIG-style strip — a centered clock, the mesh/system rollups, and compact
//! system-control glyphs on the right — fed by the existing
//! [`crate::status`] `StatusSegments` read-model.
//! **This deliberately REVERSES the old NAVBAR-W10 "no top bar" decision**
//! (Q12 says so in as many words).
//!
//! ## Paint layer and reserved layout band
//!
//! The strip paints as a foreground [`egui::Area`] pinned to the top edge, while
//! `main.rs::central_view` reserves the matching [`STATUS_BAR_H`] band before
//! laying out every workspace. This keeps the strip's fixed chrome interaction
//! model without allowing app content to slide underneath it.
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
//! Right-cluster cells surface exactly what the rollups carry: each daemon
//! segment's folded severity word, plus the local node's A–F mesh grade. The
//! Q12 sketch's "battery % / unacked alert count" are NOT in the
//! `StatusSegments` read-model (rollups carry severity + summary, no numeric
//! battery or count fields), so no number is fabricated: an absent rollup
//! renders as a dim "—", never a made-up value (the NOTIF-3 rule, restated).

use std::time::Duration;

use mde_egui::egui;
use mde_egui::{GradeBand, Motion, Style, TypographyRole};
use mde_theme::brand::icons::IconId;

use crate::chrome::NodeGrades;
use crate::construct::ConstructChrome;
use crate::status::{segment_label, severity_color, severity_label, StatusSegment, StatusSegments};
use crate::surfaces::icon_texture;

/// The locked strip height (Q12: "~24px").
pub(crate) const STATUS_BAR_H: f32 = 24.0;

/// The daemon rollup segments the right cluster surfaces, left→right —
/// Q12's "mesh grade, network, power, alert count" mapped onto what the
/// `StatusSegments` read-model actually carries (module doc: Device platform
/// health · Mesh = the mesh/fleet *network* rollup · Power · Alerts).
pub(crate) const RIGHT_SEGMENTS: [StatusSegment; 4] = [
    StatusSegment::Device,
    StatusSegment::Mesh,
    StatusSegment::Power,
    StatusSegment::Alerts,
];

/// Compact right-rail controls. These are intentionally action-only: the
/// existing Control Center remains the source of truth for their live values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusControl {
    /// Open the Control Center's volume controls.
    Volume,
    /// Open the Control Center's network controls.
    Network,
    /// Open the Control Center's display/brightness controls.
    Brightness,
}

impl StatusControl {
    /// The fixed, deterministic order used by the top rail.
    pub(crate) const ALL: [Self; 3] = [Self::Volume, Self::Network, Self::Brightness];

    const fn index(self) -> usize {
        match self {
            Self::Volume => 0,
            Self::Network => 1,
            Self::Brightness => 2,
        }
    }

    /// Existing YAMIS glyph selected for this status/control-center affordance.
    pub(crate) const fn icon(self) -> IconId {
        match self {
            Self::Volume => IconId::Volume,
            Self::Network => IconId::Signal,
            Self::Brightness => IconId::DisplaySettings,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Volume => "Volume",
            Self::Network => "Network",
            Self::Brightness => "Screen brightness",
        }
    }
}

/// Each icon gets one full rail-height hit target, matching the compact macOS
/// menu-bar rhythm while keeping the pointer target larger than the glyph.
const STATUS_CONTROL_W: f32 = STATUS_BAR_H;
const STATUS_CONTROL_GAP: f32 = Style::SP_XS;
const STATUS_CONTROL_ICON: f32 = Style::ICON_M;

fn status_controls_width() -> f32 {
    STATUS_CONTROL_W * StatusControl::ALL.len() as f32
        + STATUS_CONTROL_GAP * (StatusControl::ALL.len().saturating_sub(1) as f32)
}

/// Fit the right-control cluster to the available rail without changing its
/// normal macOS-sized geometry on a real workstation. Tiny headless/windowed
/// surfaces still get bounded hit targets instead of controls extending past
/// the top-bar edge.
fn status_controls_metrics(bar: egui::Rect) -> (f32, f32) {
    let available = (bar.width() - Style::SP_S * 2.0).max(1.0);
    let count = StatusControl::ALL.len() as f32;
    let normal_width = STATUS_CONTROL_W;
    let normal_gap = STATUS_CONTROL_GAP;
    let min_gap = normal_gap.min((available / (count * 4.0)).max(0.0));
    let control_width = normal_width.min(((available - min_gap * (count - 1.0)) / count).max(1.0));
    (control_width, min_gap)
}

fn status_controls_rect(bar: egui::Rect) -> egui::Rect {
    let right = bar.right() - Style::SP_S;
    let (control_width, gap) = status_controls_metrics(bar);
    egui::Rect::from_min_max(
        egui::pos2(
            right
                - control_width * StatusControl::ALL.len() as f32
                - gap * (StatusControl::ALL.len().saturating_sub(1) as f32),
            bar.top(),
        ),
        egui::pos2(right, bar.bottom()),
    )
}

fn status_control_rect(bar: egui::Rect, control: StatusControl) -> egui::Rect {
    let controls = status_controls_rect(bar);
    let (control_width, gap) = status_controls_metrics(bar);
    let x = controls.left() + control.index() as f32 * (control_width + gap);
    egui::Rect::from_min_size(
        egui::pos2(x, controls.top()),
        egui::vec2(control_width, controls.height()),
    )
}

fn status_control_id(control: StatusControl) -> egui::Id {
    egui::Id::new((
        "construct-status-bar",
        match control {
            StatusControl::Volume => "volume",
            StatusControl::Network => "network",
            StatusControl::Brightness => "brightness",
        },
    ))
}

fn centered_clock_rect(bar: egui::Rect, time_width: f32) -> egui::Rect {
    egui::Rect::from_center_size(
        bar.center(),
        egui::vec2(
            (time_width + Style::SP_S * 2.0).min(bar.width()),
            bar.height(),
        ),
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
    let right = (controls.left() - Style::SP_XS).clamp(bar.left(), bar.right());
    let left = (clock.right() + Style::SP_XS).clamp(bar.left(), right);
    let available = (right - left).max(0.0);
    let width = cluster_width.max(0.0).min(available);
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

/// One right-cluster segment cell, folded pure from the rollups (§7).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RightCell {
    /// The daemon segment this cell reads.
    pub segment: StatusSegment,
    /// `"{label} {severity-word}"` when the rollup exists, `"{label} —"` when
    /// it honestly does not.
    pub text: String,
    /// The severity dot tint ([`severity_color`]; dim when absent).
    pub dot: egui::Color32,
    /// Whether a rollup backs the cell (drives the text tint).
    pub present: bool,
}

/// Fold the four daemon rollups into their compact cells. Absent rollup →
/// a dim "—" cell, never a fabricated state (module doc).
#[must_use]
pub(crate) fn right_cells(segments: &StatusSegments) -> Vec<RightCell> {
    RIGHT_SEGMENTS
        .into_iter()
        .map(|segment| {
            let rollup = segments.get(segment);
            let value = rollup.map_or("—", |r| severity_label(Some(r)));
            RightCell {
                segment,
                text: format!("{} {value}", segment_label(segment)),
                dot: severity_color(rollup),
                present: rollup.is_some(),
            }
        })
        .collect()
}

/// The local node's A–F mesh grade glyph `(letter, band colour)` — the same
/// fold as `status::local_grade_label`/`local_grade_color` (private there;
/// replicated over the shared [`GradeBand`] so "which score is which grade"
/// still lives ONCE in `mde_egui`). Missing or stale local row → a dim "—",
/// never a fake letter (the NODE-GRADE-2 #17 rule).
#[must_use]
pub(crate) fn mesh_grade_cell(grades: &NodeGrades) -> (String, egui::Color32) {
    grades
        .rows
        .iter()
        .find(|row| row.is_local)
        .filter(|row| !row.stale)
        .map_or_else(
            || ("—".to_string(), Style::TEXT_DIM),
            |row| {
                let band = GradeBand::from_score(f32::from(row.score));
                (band.letter().to_string(), band.color())
            },
        )
}

/// Stable id of the strip's Area.
fn status_bar_area_id() -> egui::Id {
    egui::Id::new("construct-status-bar")
}

/// Stable id of the centered clock (the Notification Center trigger).
pub(crate) fn status_bar_clock_id() -> egui::Id {
    egui::Id::new(("construct-status-bar", "clock"))
}

/// Stable id of the right rollup cluster (the Control Center trigger).
pub(crate) fn status_bar_right_cluster_id() -> egui::Id {
    egui::Id::new(("construct-status-bar", "right-cluster"))
}

/// Mount the strip — called every frame from `main.rs`'s
/// `mount_status_bar_slot` (the U09 contract's reserved mount point).
pub fn mount(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    grades: &NodeGrades,
    env: StatusBarEnv,
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
    if !visible {
        return;
    }
    let t = Motion::animate(ctx, "construct-status-bar-visible", visible, Motion::BASE);
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
            strip(ui, bar, construct, segments, grades);
        });
}

/// Paint + interact the strip body. Absolute screen-space rects throughout
/// (the dock's WIN7-DESKTOP-1 lesson: an Area's `fixed_pos` only seeds the Ui,
/// `ui.painter()`/`ui.interact` stay absolute).
fn strip(
    ui: &egui::Ui,
    bar: egui::Rect,
    construct: &mut ConstructChrome,
    segments: &StatusSegments,
    grades: &NodeGrades,
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
    let time_role = TypographyRole::Label;
    // ── Center cluster: the one authoritative clock ────────────────────────
    let now = crate::timers::now_unix();
    let time = crate::timers::hhmm(now);
    let time_galley = painter.layout_job(Style::typography_job(
        time.clone(),
        TypographyRole::Label,
        Style::TEXT,
        f32::INFINITY,
    ));
    let time_w = time_galley.size().x;
    let clock_rect = centered_clock_rect(bar, time_w);
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
    painter.galley(
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

    // ── Right cluster: rollups, then the three compact system controls ─────
    let controls_rect = status_controls_rect(bar);
    let (grade_text, grade_color) = mesh_grade_cell(grades);
    let cells = right_cells(segments);
    let dot_r = Style::SP_XS;
    let grade_w = Style::SP_S * 2.0;
    let mut cluster_w = grade_w;
    let cell_widths: Vec<f32> = cells
        .iter()
        .map(|cell| {
            let text_w = painter
                .layout_job(Style::typography_job(
                    cell.text.clone(),
                    time_role,
                    Style::TEXT,
                    f32::INFINITY,
                ))
                .size()
                .x;
            let w = dot_r * 2.0 + Style::SP_XS + text_w;
            cluster_w += Style::SP_S + w;
            w
        })
        .collect();
    let cluster_rect = bounded_cluster_rect(bar, clock_rect, controls_rect, cluster_w);
    let cluster = ui.interact(
        cluster_rect,
        status_bar_right_cluster_id(),
        egui::Sense::click(),
    );
    cluster.widget_info(|| {
        let summary: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!(
                "System status: grade {grade_text}, {} — Control Center",
                summary.join(", ")
            ),
        )
    });
    if cluster.hovered() {
        painter.rect_filled(cluster_rect.shrink(2.0), Style::RADIUS_S, Style::SURFACE_HI);
    }
    let cluster_painter = painter.with_clip_rect(cluster_rect);
    let mut x = cluster_rect.left();
    // The grade glyph — the letter over its band-coloured pip (the dock's
    // local-grade idiom, shrunk to the strip).
    let grade_center = egui::pos2(x + Style::SP_S, cy);
    cluster_painter.circle_filled(grade_center, Style::SP_S, grade_color);
    cluster_painter.text(
        grade_center,
        egui::Align2::CENTER_CENTER,
        &grade_text,
        Style::typography_font(TypographyRole::Caption),
        Style::BG,
    );
    x += grade_w;
    for (cell, w) in cells.iter().zip(cell_widths) {
        x += Style::SP_S;
        cluster_painter.circle_filled(egui::pos2(x + dot_r, cy), dot_r, cell.dot);
        let cell_galley = cluster_painter.layout_job(Style::typography_job(
            cell.text.clone(),
            time_role,
            if cell.present {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            },
            f32::INFINITY,
        ));
        cluster_painter.galley(
            egui::pos2(
                x + dot_r * 2.0 + Style::SP_XS,
                cy - cell_galley.size().y / 2.0,
            ),
            cell_galley,
            if cell.present {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            },
        );
        x += w;
    }
    if cluster.clicked() {
        // PLATFORM-INTERFACES §2.3 — "Control Center | click status-bar right
        // cluster": the pub open flag IS the sanctioned seam.
        construct.control_center_open = !construct.control_center_open;
    }

    for control in StatusControl::ALL {
        let rect = status_control_rect(bar, control);
        let response = ui.interact(rect, status_control_id(control), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!("{} — Control Center", control.label()),
            )
        });
        if response.hovered() {
            painter.rect_filled(rect.shrink(2.0), Style::RADIUS_S, Style::SURFACE_HI);
        }
        if let Some(texture) =
            icon_texture(ui.ctx(), control.icon(), STATUS_CONTROL_ICON, Style::TEXT)
        {
            let draw = egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(STATUS_CONTROL_ICON, STATUS_CONTROL_ICON),
            );
            painter.image(
                texture.id(),
                draw,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            // The bundled glyph loader fails soft; retain a visible, honest
            // interaction target rather than manufacturing a status value.
            painter.circle_filled(rect.center(), Style::SP_XS, Style::TEXT_DIM);
        }
        if response.clicked() {
            // The three glyphs are shortcuts into the existing Control Center;
            // the panel remains the sole source of truth for live values.
            construct.control_center_open = !construct.control_center_open;
        }
    }

    // Wake at the next minute rollover so the painted minute is never stale
    // (the dock tray clock's idiom).
    ui.ctx().request_repaint_after(Duration::from_secs(
        crate::timers::secs_to_next_minute(now).max(1),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::{GradeRow, GradeTrend};
    use crate::status::SegmentRollup;

    fn visible_env() -> StatusBarEnv {
        StatusBarEnv {
            curtain_engaged: false,
            car: false,
            immersive_app: false,
        }
    }

    fn rollup(segment: &str, severity: &str) -> SegmentRollup {
        SegmentRollup {
            segment: segment.to_string(),
            severity: severity.to_string(),
            source: "unit".to_string(),
            summary: "unit summary".to_string(),
            host: "local".to_string(),
            critical_policy: "none".to_string(),
            ts_unix_ms: 0,
        }
    }

    fn local_grade(score: u8, stale: bool) -> NodeGrades {
        NodeGrades {
            rows: vec![GradeRow {
                host: "local".to_string(),
                score,
                trend: GradeTrend::Steady,
                is_local: true,
                stale,
            }],
            seen: true,
        }
    }

    /// Drive ONE headless frame of the strip through the house `Context::run`
    /// harness, minus the stand-in surface.
    fn drive(
        ctx: &egui::Context,
        construct: &mut ConstructChrome,
        segments: &StatusSegments,
        grades: &NodeGrades,
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
        grades: &NodeGrades,
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
        grades: &NodeGrades,
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
        grades: &NodeGrades,
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
    fn right_cells_render_rollups_honestly() {
        // Absent rollups → dim "—" cells, never a fabricated state (§7).
        let empty = right_cells(&StatusSegments::default());
        assert_eq!(empty.len(), RIGHT_SEGMENTS.len());
        for cell in &empty {
            assert!(cell.text.ends_with('—'), "{}", cell.text);
            assert!(!cell.present);
            assert_eq!(cell.dot, Style::TEXT_DIM);
        }
        // A live rollup folds to its severity word + tone.
        let segments = StatusSegments {
            mesh: Some(rollup("mesh", "warning")),
            seen: true,
            ..StatusSegments::default()
        };
        let mesh = right_cells(&segments)
            .into_iter()
            .find(|c| c.segment == StatusSegment::Mesh)
            .expect("mesh cell folded");
        assert_eq!(mesh.text, "Mesh warning");
        assert!(mesh.present);
        assert_eq!(mesh.dot, Style::SUPPORT_WARNING);
    }

    #[test]
    fn the_mesh_grade_cell_folds_the_local_row_honestly() {
        let (letter, color) = mesh_grade_cell(&local_grade(95, false));
        assert_eq!(letter, "A");
        assert_eq!(color, GradeBand::A.color());
        // Stale or missing local rows show a dim "—", never a fake letter.
        assert_eq!(
            mesh_grade_cell(&local_grade(95, true)),
            ("—".to_string(), Style::TEXT_DIM)
        );
        assert_eq!(
            mesh_grade_cell(&NodeGrades::default()),
            ("—".to_string(), Style::TEXT_DIM)
        );
    }

    #[test]
    fn the_strip_renders_the_clock_and_a_rollup_cell() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments {
            mesh: Some(rollup("mesh", "warning")),
            seen: true,
            ..StatusSegments::default()
        };
        let grades = local_grade(95, false);
        let before = crate::timers::now_unix();
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
        let after = crate::timers::now_unix();
        let texts = frame_texts(&out);
        // The centered clock (bracketed against a minute rollover mid-test).
        assert!(
            texts
                .iter()
                .any(|t| *t == crate::timers::hhmm(before) || *t == crate::timers::hhmm(after)),
            "no clock text painted: {texts:?}"
        );
        // At least one rollup cell + the grade letter.
        assert!(
            texts.iter().any(|t| t == "Mesh warning"),
            "no mesh rollup cell painted: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "A"),
            "no mesh grade glyph painted: {texts:?}"
        );
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
        let grades = NodeGrades::default();
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
    fn clicking_the_right_cluster_toggles_the_control_center() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = NodeGrades::default();
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
            .read_response(status_bar_right_cluster_id())
            .expect("right cluster registered")
            .rect
            .center();
        click(&ctx, &mut construct, &segments, &grades, pos);
        // PLATFORM-INTERFACES §2.3 — right cluster click = Control Center.
        assert!(construct.control_center_open, "cluster click opens CC");
        assert!(
            !construct.notification_center_open,
            "NC untouched by the cluster"
        );
        click(&ctx, &mut construct, &segments, &grades, pos);
        assert!(!construct.control_center_open, "second click closes");
    }

    #[test]
    fn centered_clock_and_right_controls_have_deterministic_geometry() {
        let bar = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, STATUS_BAR_H));
        let clock = centered_clock_rect(bar, 48.0);
        assert!((clock.center().x - bar.center().x).abs() < f32::EPSILON);
        assert!((clock.center().y - bar.center().y).abs() < f32::EPSILON);

        let controls = status_controls_rect(bar);
        assert!((controls.right() - (bar.right() - Style::SP_S)).abs() < f32::EPSILON);
        assert!(
            (status_controls_width() - (STATUS_CONTROL_W * 3.0 + STATUS_CONTROL_GAP * 2.0)).abs()
                < f32::EPSILON
        );
        for (index, control) in StatusControl::ALL.into_iter().enumerate() {
            let rect = status_control_rect(bar, control);
            assert_eq!(rect.top(), bar.top());
            assert_eq!(rect.bottom(), bar.bottom());
            assert!(
                (rect.left()
                    - (controls.left() + index as f32 * (STATUS_CONTROL_W + STATUS_CONTROL_GAP)))
                    .abs()
                    < f32::EPSILON
            );
            assert!(rect.right() <= controls.right());
        }
        let last = status_control_rect(bar, StatusControl::Brightness);
        assert!((last.right() - controls.right()).abs() < f32::EPSILON);
    }

    #[test]
    fn right_controls_remain_inside_a_narrow_top_bar() {
        let bar = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(72.0, STATUS_BAR_H));
        let controls = status_controls_rect(bar);
        assert!(
            bar.contains_rect(controls),
            "cluster escaped narrow bar: {controls:?}"
        );
        let rects: Vec<_> = StatusControl::ALL
            .into_iter()
            .map(|control| status_control_rect(bar, control))
            .collect();
        assert!(rects.iter().all(|rect| bar.contains_rect(*rect)));
        assert!(rects
            .windows(2)
            .all(|pair| pair[0].right() <= pair[1].left()));
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
        let grades = NodeGrades::default();

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
            ("rollups", status_bar_right_cluster_id()),
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
        for control in StatusControl::ALL {
            let response = ctx
                .read_response(status_control_id(control))
                .unwrap_or_else(|| panic!("{control:?} target was not registered"));
            assert!(
                bar.contains_rect(response.rect),
                "{control:?} target escaped the narrow status bar: {:?} vs {bar:?}",
                response.rect
            );
        }

        let brightness = ctx
            .read_response(status_control_id(StatusControl::Brightness))
            .expect("brightness control registered")
            .rect
            .center();
        click_at(&ctx, &mut construct, &segments, &grades, screen, brightness);
        assert!(
            construct.control_center_open,
            "a non-zero-origin control target must remain clickable"
        );
    }

    #[test]
    fn clicking_a_status_control_toggles_the_existing_control_center() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let grades = NodeGrades::default();
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
            .read_response(status_control_id(StatusControl::Volume))
            .expect("volume control registered")
            .rect
            .center();
        click(&ctx, &mut construct, &segments, &grades, pos);
        assert!(construct.control_center_open, "volume control opens CC");
        click(&ctx, &mut construct, &segments, &grades, pos);
        assert!(!construct.control_center_open, "second click closes CC");
    }

    #[test]
    fn status_controls_use_the_requested_existing_icon_identities() {
        assert_eq!(StatusControl::Volume.icon(), IconId::Volume);
        assert_eq!(StatusControl::Network.icon(), IconId::Signal);
        assert_eq!(StatusControl::Brightness.icon(), IconId::DisplaySettings);
        assert_eq!(StatusControl::ALL.len(), 3);
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
            let grades = NodeGrades::default();
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
        let grades = NodeGrades::default();

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
