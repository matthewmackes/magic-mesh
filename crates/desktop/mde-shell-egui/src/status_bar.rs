//! `status_bar` — WL-UX-006/U11: the **Construct slim top status bar**.
//!
//! Authority: `docs/design/platform-interfaces.md` §2.3 (Q12): a ~24px
//! HIG-style strip — clock + date on the left; the mesh/system rollups on the
//! right — fed by the existing [`crate::status`] `StatusSegments` read-model.
//! **This deliberately REVERSES the old NAVBAR-W10 "no top bar" decision**
//! (Q12 says so in as many words).
//!
//! ## Why a floating overlay, not a reserved panel
//!
//! The strip renders as a foreground [`egui::Area`] pinned to the top edge and
//! does **not** reserve layout space. The Q28 full-native-resolution guarantee
//! therefore holds for every app canvas.
//!
//! ## Auto-hide (Q12/Q28)
//!
//! Hidden while the curtain is engaged (CURTAIN-1: no chrome under the lock),
//! in the Car profile (Auto Mode owns its own instrument chrome), and over a
//! focused full-screen remote desktop or immersive Maps workspace (U24).
//! Visibility is a pure fold ([`status_bar_visible`]); the transition fades
//! through [`Motion::animate`].
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

use mde_egui::egui::{self, FontId};
use mde_egui::{GradeBand, Motion, Style};

use crate::chrome::NodeGrades;
use crate::construct::ConstructChrome;
use crate::status::{segment_label, severity_color, severity_label, StatusSegment, StatusSegments};

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

/// The date line — the dock tray clock's civil `M/D/YYYY` fold
/// (`dock::clock_date_text`, private there; the *call* is replicated citing
/// that origin so the date math itself stays in the crate's ONE calendar,
/// [`crate::chat::civil_from_days`], §6).
#[must_use]
pub(crate) fn date_text(now_unix: i64) -> String {
    let (year, month, day) = crate::chat::civil_from_days(now_unix.div_euclid(86_400));
    format!("{month}/{day}/{year}")
}

/// Stable id of the strip's Area.
fn status_bar_area_id() -> egui::Id {
    egui::Id::new("construct-status-bar")
}

/// Stable id of the left clock cluster (the Notification Center trigger).
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
    let time_font = FontId::proportional(Style::TYPE_FOOTNOTE);
    let date_font = FontId::proportional(Style::TYPE_CAPTION);

    // ── Left cluster: clock + date (the crate's one clock/calendar) ────────
    let now = crate::timers::now_unix();
    let time = crate::timers::hhmm(now);
    let date = date_text(now);
    let time_w = painter
        .layout_no_wrap(time.clone(), time_font.clone(), Style::TEXT)
        .size()
        .x;
    let date_w = painter
        .layout_no_wrap(date.clone(), date_font.clone(), Style::TEXT_DIM)
        .size()
        .x;
    let clock_rect = egui::Rect::from_min_max(
        bar.left_top(),
        egui::pos2(
            bar.left() + Style::SP_S + time_w + Style::SP_S + date_w + Style::SP_S,
            bar.bottom(),
        ),
    );
    let clock = ui.interact(clock_rect, status_bar_clock_id(), egui::Sense::click());
    clock.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Clock {time}, {date} — Notification Center"),
        )
    });
    if clock.hovered() {
        painter.rect_filled(clock_rect.shrink(2.0), Style::RADIUS_S, Style::SURFACE_HI);
    }
    painter.text(
        egui::pos2(bar.left() + Style::SP_S, cy),
        egui::Align2::LEFT_CENTER,
        &time,
        time_font.clone(),
        Style::TEXT,
    );
    painter.text(
        egui::pos2(bar.left() + Style::SP_S + time_w + Style::SP_S, cy),
        egui::Align2::LEFT_CENTER,
        &date,
        date_font.clone(),
        Style::TEXT_DIM,
    );
    if clock.clicked() {
        // PLATFORM-INTERFACES §2.3 — "Notification Center | click status-bar
        // clock": the pub open flag IS the sanctioned seam.
        construct.notification_center_open = !construct.notification_center_open;
    }

    // ── Right cluster: mesh grade + the four rollup cells ──────────────────
    let (grade_text, grade_color) = mesh_grade_cell(grades);
    let cells = right_cells(segments);
    let dot_r = Style::SP_XS;
    let grade_w = Style::SP_S * 2.0;
    let mut cluster_w = grade_w;
    let cell_widths: Vec<f32> = cells
        .iter()
        .map(|cell| {
            let text_w = painter
                .layout_no_wrap(cell.text.clone(), time_font.clone(), Style::TEXT)
                .size()
                .x;
            let w = dot_r * 2.0 + Style::SP_XS + text_w;
            cluster_w += Style::SP_S + w;
            w
        })
        .collect();
    let cluster_rect = egui::Rect::from_min_max(
        egui::pos2(
            bar.right() - Style::SP_S - cluster_w - Style::SP_XS,
            bar.top(),
        ),
        bar.right_bottom(),
    );
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
    let mut x = bar.right() - Style::SP_S - cluster_w;
    // The grade glyph — the letter over its band-coloured pip (the dock's
    // local-grade idiom, shrunk to the strip).
    let grade_center = egui::pos2(x + Style::SP_S, cy);
    painter.circle_filled(grade_center, Style::SP_S, grade_color);
    painter.text(
        grade_center,
        egui::Align2::CENTER_CENTER,
        &grade_text,
        date_font,
        Style::BG,
    );
    x += grade_w;
    for (cell, w) in cells.iter().zip(cell_widths) {
        x += Style::SP_S;
        painter.circle_filled(egui::pos2(x + dot_r, cy), dot_r, cell.dot);
        painter.text(
            egui::pos2(x + dot_r * 2.0 + Style::SP_XS, cy),
            egui::Align2::LEFT_CENTER,
            &cell.text,
            time_font.clone(),
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
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
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
        let _ = drive(
            ctx,
            construct,
            segments,
            grades,
            visible_env(),
            vec![egui::Event::PointerMoved(pos), press],
        );
        let _ = drive(
            ctx,
            construct,
            segments,
            grades,
            visible_env(),
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
        // The clock (bracketed against a minute rollover mid-test) + date.
        assert!(
            texts
                .iter()
                .any(|t| *t == crate::timers::hhmm(before) || *t == crate::timers::hhmm(after)),
            "no clock text painted: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| *t == date_text(before) || *t == date_text(after)),
            "no date text painted: {texts:?}"
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
}
