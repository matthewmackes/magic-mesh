//! `notification_center` — WL-UX-006/U13: the Construct **Notification Center**
//! (PLATFORM-INTERFACES Q14 — a pull-down panel of grouped alert history with
//! clear-all, fed by the alerts rollup, over the existing KIRON toast plumbing).
//!
//! Mounted from `main.rs`'s reserved `mount_notification_center_slot` (the U09
//! contract): [`mount`] consumes the `NotificationCenter` [`ChromeIntent`],
//! owns the `notification_center_open` flag, and paints the panel — a
//! top-anchored pull-down (RADIUS_XL bottom corners, `SCRIM_THIN` backdrop,
//! `Spring::SHEET` drop) dismissed by a scrim click or Escape through the house
//! `egui::Modal` idiom (Q20/P5).
//!
//! ## Data honesty (what "history" really is here)
//!
//! The shell has **no persistent notification store** to read: the notify
//! daemon publishes only the *latest* rollup per segment
//! (`state/notify/segment/*`, [`crate::status`]), and KIRON alert visuals fold
//! into the Communications hub (the popup tier is retired —
//! [`crate::toast_bridge`]). So the Center groups exactly what IS retained,
//! and nothing more:
//!
//! * **STATUS** — the daemon's latest per-segment rollups (the Q14 "alerts
//!   rollup" feed): current posture, one row per live segment, deliberately
//!   NOT clearable (they are daemon state, and inventing a Bus-side ack is out
//!   of scope by design).
//! * **Per-topic alert groups** — an in-memory ring ([`NotificationRing`],
//!   honest cap [`RING_CAP`]) of the alerts this shell process decoded/raised
//!   since launch, fed by the toast bridge's admit tap. Clear-all / per-group
//!   clear empty only this shell-local ring.
//!
//! When neither source holds anything the panel says "No notifications" —
//! never a fabricated backlog.
//!
//! Row time stamps are absolute `HH:MM` through the one clock fold
//! (`crate::timers::hhmm`, §6) — the shell has no shared relative-time helper
//! to reuse (the only "N ago" formatter is private to `thisnode.rs`).

use std::collections::VecDeque;

use mde_egui::egui::{
    self, pos2, vec2, Align, CornerRadius, Frame, Layout, Rect, RichText, Sense, UiKind,
};
use mde_egui::motion::Spring;
use mde_egui::{paint_carbon, Elevation, Motion, Severity, Style};

use crate::construct::{ChromeIntent, ConstructChrome};
use crate::status::{SegmentRollup, StatusSegments};
use crate::timers::{hhmm, now_unix};
use crate::toast_bridge::ToastBridge;

/// Honest cap on the retained ring — the Center never pretends to a deeper
/// history than this shell process actually observed (~the last 50 alerts).
pub(crate) const RING_CAP: usize = 50;

/// The panel's widest reading; narrower screens inset by [`Style::SP_L`].
const PANEL_MAX_W: f32 = 560.0;
/// The panel's tallest reading.
const PANEL_MAX_H: f32 = 520.0;
/// The pull-down's height as a fraction of the screen, below the cap.
const PANEL_H_FRAC: f32 = 0.55;
/// A notification row's square severity-glyph cell, on the spacing ladder.
const GLYPH: f32 = Style::SP_M + Style::SP_XS;
/// The honest empty-state headline (asserted by name in the tests).
const EMPTY_HEADLINE: &str = "No notifications";

// ─────────────────────────────────────────────────────────────────────────────
// The retained ring
// ─────────────────────────────────────────────────────────────────────────────

/// One retained alert row (a decoded/raised KIRON alert's display fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotificationEntry {
    /// The alert severity — picks the row's Carbon glyph + tint.
    pub(crate) severity: Severity,
    /// The originating hostname (mesh identity). Empty for an anonymous raise.
    pub(crate) source_host: String,
    /// The category flag — `SECURITY` / `BUILD` / … — the primary grouping topic.
    pub(crate) flag: String,
    /// The alert headline (the row title).
    pub(crate) headline: String,
    /// Wall-clock arrival (Unix seconds) — the row's `HH:MM` stamp.
    pub(crate) at_unix: i64,
}

/// The Notification Center's bounded in-memory history (module doc: the shell
/// has no persistent notification store; this ring holds what THIS shell
/// process observed, capped at [`RING_CAP`], oldest rolling off first).
#[derive(Debug, Default)]
pub(crate) struct NotificationRing {
    /// Chronological entries, oldest first.
    entries: VecDeque<NotificationEntry>,
}

impl NotificationRing {
    /// Record one alert at the current wall clock (the bridge's admit tap).
    pub(crate) fn record(
        &mut self,
        severity: Severity,
        source_host: &str,
        flag: &str,
        headline: &str,
    ) {
        self.record_at(severity, source_host, flag, headline, now_unix());
    }

    /// [`Self::record`] with an injected arrival time (unit tests).
    pub(crate) fn record_at(
        &mut self,
        severity: Severity,
        source_host: &str,
        flag: &str,
        headline: &str,
        at_unix: i64,
    ) {
        self.entries.push_back(NotificationEntry {
            severity,
            source_host: source_host.to_owned(),
            flag: flag.to_owned(),
            headline: headline.to_owned(),
            at_unix,
        });
        while self.entries.len() > RING_CAP {
            self.entries.pop_front();
        }
    }

    /// Retained entry count.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is retained.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Global clear — the panel's "Clear all" (this shell-local ring only).
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Per-group clear: drop every entry whose grouping topic matches.
    pub(crate) fn clear_topic(&mut self, topic: &str) {
        self.entries.retain(|e| topic_of(e) != topic);
    }

    /// Chronological iteration, oldest first (double-ended so the grouping
    /// fold can walk newest-first).
    fn iter(&self) -> impl DoubleEndedIterator<Item = &NotificationEntry> {
        self.entries.iter()
    }
}

/// The grouping topic of an entry: its category flag, else its source host,
/// else the honest catch-all.
pub(crate) fn topic_of(entry: &NotificationEntry) -> &str {
    if !entry.flag.is_empty() {
        &entry.flag
    } else if !entry.source_host.is_empty() {
        &entry.source_host
    } else {
        "GENERAL"
    }
}

/// One rendered group: a topic header plus its rows, newest first.
pub(crate) struct TopicGroup<'a> {
    /// The `TYPE_FOOTNOTE` group header text.
    pub(crate) topic: &'a str,
    /// The group's entries, newest first.
    pub(crate) entries: Vec<&'a NotificationEntry>,
}

/// Fold the ring newest-first into per-topic groups; groups are ordered by
/// their newest entry (the most recently active topic leads).
pub(crate) fn grouped(ring: &NotificationRing) -> Vec<TopicGroup<'_>> {
    let mut groups: Vec<TopicGroup<'_>> = Vec::new();
    for entry in ring.iter().rev() {
        let topic = topic_of(entry);
        match groups.iter_mut().find(|g| g.topic == topic) {
            Some(group) => group.entries.push(entry),
            None => groups.push(TopicGroup {
                topic,
                entries: vec![entry],
            }),
        }
    }
    groups
}

// ─────────────────────────────────────────────────────────────────────────────
// The alerts-rollup feed (Q14)
// ─────────────────────────────────────────────────────────────────────────────

/// The daemon's `state/notify/segment/*` rollups present right now — the Q14
/// "alerts rollup" feed. Latest-per-segment posture, NOT history (module doc),
/// rendered as the Center's non-clearable STATUS group.
fn status_rows(segments: &StatusSegments) -> Vec<(&'static str, &SegmentRollup)> {
    [
        ("Alerts", segments.alerts.as_ref()),
        ("Device", segments.device.as_ref()),
        ("Mesh", segments.mesh.as_ref()),
        ("Power", segments.power.as_ref()),
    ]
    .into_iter()
    .filter_map(|(name, rollup)| rollup.map(|r| (name, r)))
    .collect()
}

/// Map a rollup's wire severity string onto the shared ladder. An unknown
/// string reads as Info — the Center never invents urgency.
fn rollup_severity(severity: &str) -> Severity {
    match severity {
        "critical" => Severity::Critical,
        "warning" => Severity::Warning,
        _ => Severity::Info,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The pull-down panel
// ─────────────────────────────────────────────────────────────────────────────

/// The panel's resting rect: top-anchored (flush with the screen edge — the
/// RADIUS_XL corners are the *bottom* pair), horizontally centered.
pub(crate) fn panel_rect(screen: Rect) -> Rect {
    let w = (screen.width() - 2.0 * Style::SP_L).clamp(0.0, PANEL_MAX_W);
    let h = (screen.height() * PANEL_H_FRAC).clamp(0.0, PANEL_MAX_H);
    Rect::from_min_size(pos2(screen.center().x - w / 2.0, screen.top()), vec2(w, h))
}

/// The panel chrome: SURFACE fill, hairline stroke, RADIUS_XL **bottom**
/// corners (the top pair sits flush against the screen edge), Overlay
/// elevation — the U04 material ladder throughout.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn panel_frame() -> Frame {
    let xl = Style::RADIUS_XL as u8;
    Frame::NONE
        .fill(Style::SURFACE)
        .stroke(Style::hairline())
        .corner_radius(CornerRadius {
            nw: 0,
            ne: 0,
            sw: xl,
            se: xl,
        })
        .inner_margin(Style::SP_M)
        .shadow(Elevation::Overlay.egui_shadow())
}

/// The U13 mount: consume the `NotificationCenter` intent, own the open flag,
/// and paint the pull-down while it is set (`main.rs` calls this once per
/// frame from the reserved U09 slot and never changes again).
// PLATFORM-INTERFACES Q14 — "Notification Center: top-left/center pull-down;
// grouped history over the toast plumbing; clear-all; scrim + Escape close."
pub(crate) fn mount(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    toasts: &mut ToastBridge,
    segments: &StatusSegments,
) {
    if construct.take_intent(ChromeIntent::NotificationCenter) {
        construct.notification_center_open = !construct.notification_center_open;
    }
    // Paint-safety gate: laying text out needs fonts, and a Context grows them
    // only once its first `run` pass begins — the U09 contract tests drive the
    // mount slots headlessly without ever running a frame. Nothing is lost:
    // the intent/flag state above is already applied, and paint resumes on the
    // next real frame.
    if ctx.cumulative_pass_nr() == 0 {
        return;
    }
    // Seed + drive the pull-down spring every frame, so the FIRST open springs
    // down from the edge (Motion::spring_to lands on target at first sight of
    // an id — an unseeded spring would pop the panel in place).
    let open = construct.notification_center_open;
    let t = Motion::spring_to(
        ctx,
        "construct-nc-spring",
        if open { 1.0 } else { 0.0 },
        Spring::SHEET,
    );
    if !open {
        // Exit is instant by design: egui's Modal swallows input beneath it,
        // so a lingering exit animation would eat the click after an Escape.
        return;
    }

    let screen = ctx.screen_rect();
    let rect = panel_rect(screen);
    // The drop: parked fully above the top edge at t = 0; the SHEET spring is
    // near-critical so the pull-down lands without sailing past its rect.
    let slide = -(rect.height() + Style::SP_L) * (1.0 - t);
    let id = egui::Id::new("construct-notification-center");
    let area = egui::Area::new(id)
        .kind(UiKind::Modal)
        .sense(Sense::hover())
        .order(egui::Order::Foreground)
        .interactable(true)
        .movable(false)
        .constrain(false)
        .fixed_pos(pos2(rect.left(), rect.top() + slide));

    // The house modality idiom (Q20/P5, same as the shared Sheet): the Modal
    // paints the scrim, reports the outside click, and traps focus above the
    // layers below.
    let modal = egui::Modal::new(id)
        .area(area)
        .backdrop_color(Style::SCRIM_THIN.gamma_multiply(t.clamp(0.0, 1.0)))
        .frame(panel_frame());
    let shown = modal.show(ctx, |ui| {
        let inner_w = (rect.width() - 2.0 * Style::SP_M).max(0.0);
        ui.set_min_width(inner_w);
        ui.set_max_width(inner_w);
        panel_contents(ui, toasts, segments, rect.height());
    });
    // Q14 + P5: the scrim click and the (top-modal, consumed) Escape both
    // close — clearing the ONE open flag the input contract reads.
    if shown.should_close() {
        construct.notification_center_open = false;
    }
}

/// The panel body: header (title + clear-all), then the STATUS group and the
/// per-topic alert groups in a scroll region — or the honest empty state.
fn panel_contents(
    ui: &mut egui::Ui,
    toasts: &mut ToastBridge,
    segments: &StatusSegments,
    panel_h: f32,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Notifications")
                .size(Style::TYPE_SUBHEADLINE)
                .color(Style::TEXT_STRONG)
                .strong(),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Clears only the shell-local ring — the daemon's STATUS rollups
            // are current posture, not clearable history (module doc).
            if !toasts.history().is_empty()
                && ui
                    .button(RichText::new("Clear all").size(Style::TYPE_FOOTNOTE))
                    .clicked()
            {
                toasts.history_mut().clear();
            }
        });
    });
    ui.add_space(Style::SP_S);

    let status = status_rows(segments);
    if status.is_empty() && toasts.history().is_empty() {
        // The honest empty state — nothing retained anywhere, say so.
        ui.add_space(Style::SP_XL);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(EMPTY_HEADLINE)
                    .size(Style::TYPE_BODY)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("Alerts raised while this shell runs appear here.")
                    .size(Style::TYPE_FOOTNOTE)
                    .color(Style::TEXT_DIM),
            );
        });
        ui.add_space(Style::SP_XL);
        return;
    }

    egui::ScrollArea::vertical()
        .max_height((panel_h - 2.0 * Style::SP_XL).max(0.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if !status.is_empty() {
                let _ = group_header(ui, "Status", false);
                for (name, rollup) in &status {
                    row(
                        ui,
                        rollup_severity(&rollup.severity),
                        &rollup.summary,
                        &format!(
                            "{name} · {} · {}",
                            rollup.host,
                            hhmm(rollup.ts_unix_ms / 1000)
                        ),
                    );
                }
                ui.add_space(Style::SP_S);
            }
            // Per-group clear is deferred past the loop: the groups borrow the
            // ring immutably while rendering.
            let mut clear_topic: Option<String> = None;
            for group in grouped(toasts.history()) {
                if group_header(ui, group.topic, true) {
                    clear_topic = Some(group.topic.to_owned());
                }
                for entry in &group.entries {
                    let footnote = if entry.source_host.is_empty() {
                        hhmm(entry.at_unix)
                    } else {
                        format!("{} · {}", entry.source_host, hhmm(entry.at_unix))
                    };
                    row(ui, entry.severity, &entry.headline, &footnote);
                }
                ui.add_space(Style::SP_S);
            }
            if let Some(topic) = clear_topic {
                toasts.history_mut().clear_topic(&topic);
            }
        });
}

/// A `TYPE_FOOTNOTE` group header, with an optional right-aligned per-group
/// "Clear". Returns whether the clear was clicked this frame.
fn group_header(ui: &mut egui::Ui, topic: &str, clearable: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(topic.to_uppercase())
                .size(Style::TYPE_FOOTNOTE)
                .color(Style::TEXT_DIM),
        );
        if clearable {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                clicked = ui
                    .button(RichText::new("Clear").size(Style::TYPE_FOOTNOTE))
                    .clicked();
            });
        }
    });
    clicked
}

/// One notification row: the Carbon severity glyph, a `TYPE_BODY` title, and a
/// `TYPE_FOOTNOTE` `source · HH:MM` line.
fn row(ui: &mut egui::Ui, severity: Severity, title: &str, footnote: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(GLYPH, GLYPH), Sense::hover());
        if !paint_carbon(ui.painter(), rect, severity.glyph_name(), severity.color()) {
            // Registry miss: an honest severity dot rather than a blank cell.
            ui.painter()
                .circle_filled(rect.center(), Style::SP_XS, severity.color());
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                RichText::new(title)
                    .size(Style::TYPE_BODY)
                    .color(Style::TEXT),
            );
            ui.label(
                RichText::new(footnote)
                    .size(Style::TYPE_FOOTNOTE)
                    .color(Style::TEXT_DIM),
            );
        });
    });
    ui.add_space(Style::SP_XS);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mde_egui::egui::{self, pos2, vec2, Pos2, Rect};
    use mde_egui::{Edge, Style};

    use super::*;
    use crate::construct::{ChromeInput, EdgeSwipe};

    const SCREEN: egui::Vec2 = egui::Vec2::new(1280.0, 720.0);

    fn run_frame(
        ctx: &egui::Context,
        t: f64,
        events: Vec<egui::Event>,
        construct: &mut ConstructChrome,
        toasts: &mut ToastBridge,
        segments: &StatusSegments,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(t),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| mount(ctx, construct, toasts, segments))
    }

    /// Warm frames: the pass gate (frame 1) + the modal area's first invisible
    /// layout (frame 2), so frame 3+ is the steady-state paint.
    fn warm(
        ctx: &egui::Context,
        construct: &mut ConstructChrome,
        toasts: &mut ToastBridge,
        segments: &StatusSegments,
    ) -> egui::FullOutput {
        let mut out = run_frame(ctx, 0.0, Vec::new(), construct, toasts, segments);
        for i in 1..4 {
            out = run_frame(
                ctx,
                f64::from(i) / 60.0,
                Vec::new(),
                construct,
                toasts,
                segments,
            );
        }
        out
    }

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx
    }

    /// Whether any painted shape carries text containing `needle`.
    fn shapes_contain_text(shapes: &[egui::epaint::ClippedShape], needle: &str) -> bool {
        fn walk(shape: &egui::epaint::Shape, needle: &str) -> bool {
            match shape {
                egui::epaint::Shape::Text(t) => t.galley.job.text.contains(needle),
                egui::epaint::Shape::Vec(v) => v.iter().any(|s| walk(s, needle)),
                _ => false,
            }
        }
        shapes.iter().any(|c| walk(&c.shape, needle))
    }

    fn press_events(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    fn release_events(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    // ── the ring (push → grouped → clear) ────────────────────────────────────

    #[test]
    fn the_ring_groups_by_topic_newest_first_and_clears() {
        let mut ring = NotificationRing::default();
        ring.record_at(Severity::Info, "nyc3", "CHAT", "hello", 100);
        ring.record_at(Severity::Warning, "lh1", "BUILD", "build red", 200);
        ring.record_at(Severity::Info, "nyc3", "CHAT", "again", 300);

        let groups = grouped(&ring);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].topic, "CHAT", "the freshest topic leads");
        assert_eq!(groups[0].entries[0].headline, "again", "newest first");
        assert_eq!(groups[0].entries[1].headline, "hello");
        assert_eq!(groups[1].topic, "BUILD");

        ring.clear_topic("CHAT");
        assert_eq!(ring.len(), 1, "per-group clear drops only its topic");
        ring.clear();
        assert!(ring.is_empty(), "clear-all empties the ring");
        assert!(grouped(&ring).is_empty());
    }

    #[test]
    fn the_ring_holds_an_honest_cap() {
        let mut ring = NotificationRing::default();
        for i in 0..(RING_CAP + 10) {
            ring.record_at(Severity::Info, "h", "F", &format!("n{i}"), i as i64);
        }
        assert_eq!(ring.len(), RING_CAP, "the oldest rolled off at the cap");
        let groups = grouped(&ring);
        assert_eq!(
            groups[0].entries[0].headline,
            format!("n{}", RING_CAP + 9),
            "the newest survived"
        );
    }

    #[test]
    fn topic_falls_back_from_flag_to_host_to_general() {
        let mut ring = NotificationRing::default();
        ring.record_at(Severity::Info, "nyc3", "", "no flag", 1);
        ring.record_at(Severity::Info, "", "", "anonymous", 2);
        let groups = grouped(&ring);
        assert_eq!(groups[0].topic, "GENERAL");
        assert_eq!(groups[1].topic, "nyc3");
    }

    #[test]
    fn rollup_severity_maps_the_wire_strings() {
        assert_eq!(rollup_severity("critical"), Severity::Critical);
        assert_eq!(rollup_severity("warning"), Severity::Warning);
        assert_eq!(rollup_severity("info"), Severity::Info);
        assert_eq!(rollup_severity("nonsense"), Severity::Info, "never invents");
    }

    // ── the mount contract (intent + paint-safety) ───────────────────────────

    #[test]
    fn mount_toggles_the_flag_from_the_intent_even_without_a_frame() {
        // Mirrors the U09 contract tests: the mount slot is driven on a Context
        // that never ran a frame — the intent/flag seam must work, and painting
        // must be safely gated (fonts do not exist yet).
        let ctx = egui::Context::default();
        let mut construct = ConstructChrome::default();
        let mut toasts = ToastBridge::default();
        let segments = StatusSegments::default();

        let pull = ChromeInput {
            super_tap: false,
            super_tab: false,
            app_expanded: false,
            remote_session_focused: false,
            edges: vec![EdgeSwipe {
                edge: Edge::Top,
                x_frac: Some(0.2),
            }],
            now: Duration::ZERO,
        };
        construct.dispatch(&pull);
        mount(&ctx, &mut construct, &mut toasts, &segments);
        assert!(construct.notification_center_open, "the pull opened it");
        construct.dispatch(&pull);
        mount(&ctx, &mut construct, &mut toasts, &segments);
        assert!(
            !construct.notification_center_open,
            "the pull toggles closed"
        );
    }

    // ── the panel (open renders · scrim/Escape close · honest empty) ─────────

    #[test]
    fn the_open_flag_renders_the_grouped_pull_down() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        construct.notification_center_open = true;
        let mut toasts = ToastBridge::default();
        toasts
            .history_mut()
            .record(Severity::Warning, "lh1", "BUILD", "farm went red");
        let segments = StatusSegments {
            alerts: Some(SegmentRollup {
                segment: "alerts".to_owned(),
                severity: "warning".to_owned(),
                source: "journal".to_owned(),
                summary: "3 warnings on eagle".to_owned(),
                host: "eagle".to_owned(),
                critical_policy: "remote-pip-chat".to_owned(),
                ts_unix_ms: 12 * 3_600 * 1_000,
            }),
            ..StatusSegments::default()
        };

        let out = warm(&ctx, &mut construct, &mut toasts, &segments);
        assert!(
            shapes_contain_text(&out.shapes, "farm went red"),
            "the ring alert's headline is on the panel"
        );
        assert!(
            shapes_contain_text(&out.shapes, "BUILD"),
            "grouped under its TYPE_FOOTNOTE topic header"
        );
        assert!(
            shapes_contain_text(&out.shapes, "3 warnings on eagle"),
            "the alerts rollup feeds the STATUS group"
        );
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "an open Center painted no geometry");
        assert!(construct.notification_center_open, "nothing dismissed it");
    }

    #[test]
    fn a_closed_center_paints_nothing() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        let mut toasts = ToastBridge::default();
        let segments = StatusSegments::default();
        let out = warm(&ctx, &mut construct, &mut toasts, &segments);
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(prims.is_empty(), "a closed Center still drew geometry");
    }

    #[test]
    fn a_scrim_click_clears_the_open_flag() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        construct.notification_center_open = true;
        let mut toasts = ToastBridge::default();
        let segments = StatusSegments::default();
        let _ = warm(&ctx, &mut construct, &mut toasts, &segments);

        // Click well outside the top-center panel: bottom-left of the screen.
        let outside = pos2(30.0, SCREEN.y - 30.0);
        let _ = run_frame(
            &ctx,
            0.1,
            press_events(outside),
            &mut construct,
            &mut toasts,
            &segments,
        );
        let _ = run_frame(
            &ctx,
            0.12,
            release_events(outside),
            &mut construct,
            &mut toasts,
            &segments,
        );
        assert!(
            !construct.notification_center_open,
            "the scrim click closed the Center (Q14/P5)"
        );
    }

    #[test]
    fn escape_clears_the_open_flag() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        construct.notification_center_open = true;
        let mut toasts = ToastBridge::default();
        let segments = StatusSegments::default();
        let _ = warm(&ctx, &mut construct, &mut toasts, &segments);

        let escape = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let _ = run_frame(&ctx, 0.1, escape, &mut construct, &mut toasts, &segments);
        assert!(
            !construct.notification_center_open,
            "Escape closed the Center (Q14/P5)"
        );
    }

    #[test]
    fn nothing_retained_shows_the_honest_empty_state() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        construct.notification_center_open = true;
        let mut toasts = ToastBridge::default();
        let segments = StatusSegments::default();
        assert!(toasts.history().is_empty());
        assert!(status_rows(&segments).is_empty());

        let out = warm(&ctx, &mut construct, &mut toasts, &segments);
        assert!(
            shapes_contain_text(&out.shapes, EMPTY_HEADLINE),
            "an empty Center says so — never a fabricated backlog"
        );
    }

    // ── geometry ─────────────────────────────────────────────────────────────

    #[test]
    fn the_panel_is_top_anchored_and_centered() {
        let screen = Rect::from_min_size(Pos2::ZERO, SCREEN);
        let rect = panel_rect(screen);
        assert!(
            (rect.top() - screen.top()).abs() < f32::EPSILON,
            "flush top"
        );
        assert!(
            (rect.center().x - screen.center().x).abs() < 0.5,
            "horizontally centered"
        );
        assert!(rect.width() <= PANEL_MAX_W);
        assert!(rect.height() <= PANEL_MAX_H);
        // A small screen insets rather than overflowing.
        let small = Rect::from_min_size(Pos2::ZERO, vec2(320.0, 240.0));
        assert!(panel_rect(small).width() <= small.width() - 2.0 * Style::SP_L);
    }
}
