//! The [`MeshView`] widget — a procedural, animated canvas of the current mesh
//! state, drawn entirely with [`egui::Painter`] primitives (`line_segment` /
//! `circle_filled` / `circle_stroke` / `text` — no pre-rendered images) and
//! themed exclusively through the shared [`Style`].
//!
//! Like the egui clock demo, it is repainted every frame off the frame clock
//! (`ui.input(|i| i.time)` + `request_repaint`), so links pulse and the leader
//! ring breathes continuously. The render path draws **only** the [`MeshState`]
//! it is handed — there is no embedded demo data here (the runnable sample lives
//! in `examples/mesh_view.rs`). When that state has no nodes — no mesh data yet,
//! or a mesh with no reachable peers — it paints an honest "waiting for mesh"
//! `EmptyState` rather than a blank canvas or fabricated peers (§6/§7).

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};

use mde_egui::egui::{
    self, Align2, Color32, FontFamily, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2,
};
use mde_egui::{Motion, Style};

use crate::layout;
use crate::state::{Health, MeshNode, MeshState, Role};

// ── Canvas geometry (pixels) ────────────────────────────────────────────────
// Colours come exclusively from `Style`; these are the inherent *dimensions* of
// a painted canvas (disc radii, stroke widths, pulse sizes), kept as named,
// documented consts — the clock-demo idiom. Spacing-shaped values are sourced
// from `Style::SP_*`.

/// Node disc radius for a Lighthouse (the largest role).
const NODE_R_LIGHTHOUSE: f32 = 12.0;
/// Node disc radius for a headless Server (between the anchor and a workstation).
const NODE_R_SERVER: f32 = 9.0;
/// Node disc radius for a Workstation peer (the smaller role).
const NODE_R_WORKSTATION: f32 = 6.5;
/// Stroke width of a node's health ring.
const NODE_STROKE_W: f32 = 1.5;
/// Gap between a node's edge and the steady leader ring.
const LEADER_RING_GAP: f32 = 4.0;
/// Travel distance of the expanding leader ripple.
const LEADER_RING_PULSE: f32 = 6.0;
/// One full leader heartbeat (seconds), derived from the shared Motion table so
/// the cadence stays on the harness timing scale rather than a bespoke literal.
const LEADER_PULSE_SECS: f64 = Motion::SLOW as f64 * 5.0;
/// Idle link hairline width.
const LINK_BASE_W: f32 = 1.0;
/// Extra width an active link gains at full activity.
const LINK_ACTIVE_W: f32 = 2.0;
/// Radius of a travelling activity pulse dot.
const PULSE_DOT_R: f32 = 3.0;
/// Gap between the hostname label and the version sub-label stacked beneath it.
const LABEL_LINE_GAP: f32 = 1.0;
/// Namespacing salt for a node's per-axis position-easing animation ids, so the
/// eased x/y a node glides along never collide with another surface's shared
/// `Motion::animate_value` keys.
const POS_ANIM_SALT: &str = "mde-mesh-view::node-pos";

// ── Empty-state glyph (the no-nodes canvas) ─────────────────────────────────
// A dim hub-and-spoke emblem shown when the mesh has no nodes. Sizes are
// spacing-shaped (`Style::SP_*`) so the glyph scales on the shared grid.
/// Satellite-ring radius of the empty-state glyph — icon-scale, so it reads as
/// an emblem of an idle mesh rather than as peer data.
const EMPTY_GLYPH_R: f32 = Style::SP_XL;
/// Satellites drawn around the empty-state glyph hub.
const EMPTY_GLYPH_SPOKES: usize = 5;
/// Hub disc radius of the empty-state glyph.
const EMPTY_GLYPH_HUB_R: f32 = Style::SP_S;
/// Satellite disc radius of the empty-state glyph.
const EMPTY_GLYPH_SAT_R: f32 = Style::SP_XS;
/// One full "listening" scan sweep of the empty-state glyph (seconds) — a calm
/// radar-like ripple breathing out from the hub. Derived from the shared Motion
/// table and deliberately slower than the leader heartbeat, so an idle canvas
/// reads as *quietly discovering* the mesh rather than as an alarm.
const EMPTY_SCAN_SECS: f64 = Motion::SLOW as f64 * 8.0;

impl Role {
    /// Drawn disc radius for this role. Public so a caller overlaying its own
    /// per-node adornment (e.g. the shell's brand role badge, QBRAND-8) can size
    /// and place it against the same disc the widget draws.
    #[must_use]
    pub const fn radius(self) -> f32 {
        match self {
            Self::Lighthouse => NODE_R_LIGHTHOUSE,
            Self::Server => NODE_R_SERVER,
            Self::Workstation => NODE_R_WORKSTATION,
        }
    }
}

impl Health {
    /// Status colour for this health, from the shared palette.
    const fn color(self) -> Color32 {
        match self {
            Self::Ok => Style::OK,
            Self::Warn => Style::WARN,
            Self::Down => Style::DANGER,
        }
    }
}

/// A concise, live **accessibility read-out** of the mesh the canvas is
/// painting, exposed to assistive tech (AccessKit) via the canvas
/// [`Response::widget_info`]. The map is otherwise a purely-painted graphic with
/// no text a screen reader can reach (WCAG 1.1.1 — *Non-text Content*), so it is
/// annotated with the same node total + per-tier health counts the menu bar's
/// status cluster shows. An empty mesh reads out the honest "waiting for mesh"
/// state, never fabricated peers (§7).
fn a11y_summary(state: &MeshState) -> String {
    if state.nodes.is_empty() {
        return "Mesh map: waiting for mesh — no peers have joined yet".to_owned();
    }
    let (mut up, mut degraded, mut down) = (0usize, 0usize, 0usize);
    for node in &state.nodes {
        match node.health {
            Health::Ok => up += 1,
            Health::Warn => degraded += 1,
            Health::Down => down += 1,
        }
    }
    let n = state.nodes.len();
    format!(
        "Mesh map: {n} node{s} — {up} up, {degraded} degraded, {down} down",
        s = if n == 1 { "" } else { "s" },
    )
}

/// The version sub-label text + colour for a node: the running build in
/// [`Style::TEXT_DIM`], an older build (`stale`) marked and drawn in
/// [`Style::WARN`] so it stands out, and an honest `—` in dim when the source
/// carries no version (never a fabricated build string, §7).
fn version_line(node: &MeshNode) -> (String, Color32) {
    match &node.version {
        Some(v) if node.stale => (format!("{v} · old"), Style::WARN),
        Some(v) => (v.clone(), Style::TEXT_DIM),
        None => ("—".to_string(), Style::TEXT_DIM),
    }
}

/// A reusable widget that paints a [`MeshState`] as a live, procedural canvas.
///
/// Build it per frame around the current state and call [`show`](Self::show):
///
/// ```no_run
/// # use mde_mesh_view::{MeshState, MeshView};
/// # fn frame(ui: &mut mde_egui::egui::Ui, state: &MeshState) {
/// MeshView::new(state).show(ui);
/// # }
/// ```
pub struct MeshView<'a> {
    state: &'a MeshState,
    reduce_motion: bool,
    margin: f32,
}

impl<'a> MeshView<'a> {
    /// Default inset (px) reserved around the node ring for discs + labels —
    /// `Style::SP_XL + Style::SP_M`.
    pub const DEFAULT_MARGIN: f32 = Style::SP_XL + Style::SP_M;
    /// Smallest canvas the widget will allocate, so a tiny container still draws.
    const MIN_CANVAS: f32 = Style::SP_XL * 6.0;

    /// Wrap a [`MeshState`] for rendering. The widget borrows the state for the
    /// frame; rebuild it each frame with fresh data.
    #[must_use]
    pub const fn new(state: &'a MeshState) -> Self {
        Self {
            state,
            reduce_motion: false,
            margin: Self::DEFAULT_MARGIN,
        }
    }

    /// Freeze animation when the session prefers reduced motion (WCAG 2.3.3):
    /// link pulses become a static mid-dot and the leader ring stops breathing.
    #[must_use]
    pub const fn reduce_motion(mut self, reduce: bool) -> Self {
        self.reduce_motion = reduce;
        self
    }

    /// Override the inset (px) reserved for discs + labels.
    #[must_use]
    pub const fn margin(mut self, margin: f32) -> Self {
        self.margin = margin;
        self
    }

    /// Allocate a canvas filling the available space and paint the mesh into it.
    /// Returns the canvas [`Response`].
    pub fn show(self, ui: &mut Ui) -> Response {
        let desired = ui.available_size().max(Vec2::splat(Self::MIN_CANVAS));
        let (response, painter) = ui.allocate_painter(desired, Sense::hover());
        let area = response.rect;
        let time = ui.input(|i| i.time);

        // The map is a text-free painted graphic: annotate the canvas with a live
        // read-out so assistive tech (AccessKit) can voice it (WCAG 1.1.1) — the
        // same node/health summary the menu-bar status cluster shows, built lazily
        // so it only costs a string on the frames a11y actually polls it.
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, a11y_summary(self.state))
        });

        // No nodes ⇒ nothing to place or animate: draw the honest "waiting for
        // mesh" canvas instead of a blank rect or fabricated peers (§6/§7). Its
        // scan ripple keeps repainting only while motion is allowed.
        if self.state.nodes.is_empty() {
            self.paint_empty_state(&painter, area, time);
            if !self.reduce_motion {
                ui.ctx().request_repaint();
            }
            return response;
        }

        // Freshly computed layout slots, then eased so a re-pack (peer join /
        // leave, a filter change) glides each node to its new slot on the SLOW
        // structural cadence instead of teleporting. egui seeds a newly seen id
        // at its target, so a just-appeared node lands in place and only a node
        // that actually moved animates; frozen to the raw slot under reduced
        // motion (WCAG 2.3.3). Links read these same centres, so an edge stays
        // pinned to its endpoints throughout the glide.
        let targets = layout::place(self.state, area, self.margin);
        let ctx = ui.ctx().clone();
        let centres: Vec<Pos2> = if self.reduce_motion {
            targets
        } else {
            self.state
                .nodes
                .iter()
                .zip(&targets)
                .map(|(n, t)| {
                    let id = n.id.as_str();
                    Pos2::new(
                        Motion::animate_value(&ctx, (POS_ANIM_SALT, id, 0u8), t.x, Motion::SLOW),
                        Motion::animate_value(&ctx, (POS_ANIM_SALT, id, 1u8), t.y, Motion::SLOW),
                    )
                })
                .collect()
        };
        let index: HashMap<&str, usize> = self
            .state
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();

        // 1) Links (and their activity pulses) under the nodes.
        for link in &self.state.links {
            let (Some(&ia), Some(&ib)) = (index.get(link.a.as_str()), index.get(link.b.as_str()))
            else {
                continue; // a link to an unknown node id is silently skipped
            };
            let (pa, pb) = (centres[ia], centres[ib]);
            let activity = link.activity.clamp(0.0, 1.0);

            painter.line_segment([pa, pb], Stroke::new(LINK_BASE_W, Style::BORDER));
            if activity > 0.0 {
                painter.line_segment(
                    [pa, pb],
                    Stroke::new(
                        LINK_BASE_W + LINK_ACTIVE_W * activity,
                        Style::ACCENT.gamma_multiply(0.18 + 0.5 * activity),
                    ),
                );
                self.paint_activity(&painter, pa, pb, activity, time);
            }
        }

        // 2) Nodes over the links.
        for (i, node) in self.state.nodes.iter().enumerate() {
            let c = centres[i];
            let r = node.role.radius();
            let hc = node.health.color();

            if node.is_leader {
                self.paint_leader_ring(&painter, c, r, time);
            }
            // Filled disc (dimmed health colour) + a solid health ring, so a
            // Down node reads as a red ring on a near-empty disc.
            painter.circle_filled(c, r, hc.gamma_multiply(0.45));
            painter.circle_stroke(c, r, Stroke::new(NODE_STROKE_W, hc));

            let label_color = if matches!(node.health, Health::Down) {
                Style::TEXT_DIM
            } else {
                Style::TEXT
            };
            let label = painter.text(
                c + Vec2::new(0.0, r + Style::SP_XS),
                Align2::CENTER_TOP,
                &node.label,
                FontId::new(Style::SMALL, FontFamily::Monospace),
                label_color,
            );

            // The node's running build, stacked under the hostname (QBRAND-8). A
            // node on an older build reads WARN so it stands out; an absent version
            // draws an honest `—`, never a fabricated build string (§7).
            let (version, version_color) = version_line(node);
            painter.text(
                Pos2::new(c.x, label.bottom() + LABEL_LINE_GAP),
                Align2::CENTER_TOP,
                version,
                FontId::new(Style::SMALL, FontFamily::Monospace),
                version_color,
            );
        }

        // 3) Keep repainting only while something actually moves (zero-CPU idle).
        let animating = !self.reduce_motion
            && (self.state.nodes.iter().any(|n| n.is_leader)
                || self.state.links.iter().any(|l| l.activity > 0.0));
        if animating {
            ui.ctx().request_repaint();
        }
        response
    }

    /// Paint the honest "waiting for mesh" `EmptyState` into `area`: a dim
    /// hub-and-spoke glyph over a title and subtitle, vertically centred. Drawn
    /// when the [`MeshState`] has no nodes, so an un-populated canvas reads as
    /// *waiting for the mesh* — never a blank rect, never fabricated peers
    /// (§6/§7). Every colour, space and type value comes from [`Style`].
    fn paint_empty_state(&self, painter: &egui::Painter, area: Rect, time: f64) {
        // Centre the glyph + title + subtitle block: the glyph's full height
        // plus the two stacked text lines and the gaps between them.
        let block_h =
            EMPTY_GLYPH_R * 2.0 + Style::SP_M + Style::HEADING + Style::SP_XS + Style::BODY;
        let cx = area.center().x;
        let glyph_c = Pos2::new(cx, area.center().y - block_h * 0.5 + EMPTY_GLYPH_R);

        // A calm "listening" scan ripple breathing out from the hub, under the
        // emblem: one expanding, fading ring on the shared scan cadence, so the
        // idle canvas reads as actively discovering the mesh rather than a dead
        // placeholder. Painted in the same accent the live view pulses its links
        // and leader ring with, so the empty and populated states share one
        // motion language. Frozen (no ripple) under reduced motion (WCAG 2.3.3).
        if !self.reduce_motion {
            let phase = (time / EMPTY_SCAN_SECS).fract() as f32;
            let ripple_r = EMPTY_GLYPH_HUB_R + phase * (EMPTY_GLYPH_R - EMPTY_GLYPH_HUB_R);
            painter.circle_stroke(
                glyph_c,
                ripple_r,
                Stroke::new(NODE_STROKE_W, Style::ACCENT.gamma_multiply(1.0 - phase)),
            );
        }

        // Dim hub-and-spoke emblem — hairline links out to unfilled discs in one
        // muted tone, so it reads as an icon of an idle mesh, not as peer data.
        for v in layout::ring(
            glyph_c.to_vec2(),
            EMPTY_GLYPH_R,
            EMPTY_GLYPH_SPOKES,
            -FRAC_PI_2,
        ) {
            let sat = v.to_pos2();
            painter.line_segment([glyph_c, sat], Stroke::new(LINK_BASE_W, Style::BORDER));
            painter.circle_stroke(
                sat,
                EMPTY_GLYPH_SAT_R,
                Stroke::new(NODE_STROKE_W, Style::TEXT_DIM),
            );
        }
        painter.circle_stroke(
            glyph_c,
            EMPTY_GLYPH_HUB_R,
            Stroke::new(NODE_STROKE_W, Style::TEXT_DIM),
        );

        // Title + dim subtitle, centre-top-anchored beneath the glyph. Both are
        // human-sentence prose, so they paint in the proportional Construct face
        // (Inter) per the platform type lock. The node hostname/version labels
        // above remain monospace because those are code-like identifiers.
        let title = painter.text(
            Pos2::new(cx, glyph_c.y + EMPTY_GLYPH_R + Style::SP_M),
            Align2::CENTER_TOP,
            "Waiting for mesh",
            FontId::new(Style::HEADING, FontFamily::Proportional),
            Style::TEXT,
        );
        painter.text(
            Pos2::new(cx, title.bottom() + Style::SP_XS),
            Align2::CENTER_TOP,
            "Peers and links appear here as nodes join.",
            FontId::new(Style::BODY, FontFamily::Proportional),
            Style::TEXT_DIM,
        );
    }

    /// Travelling activity dot(s) along `a → b`. When reduced motion is set, a
    /// single static mid-dot conveys "active" without movement.
    fn paint_activity(
        &self,
        painter: &egui::Painter,
        pa: Pos2,
        pb: Pos2,
        activity: f32,
        time: f64,
    ) {
        if self.reduce_motion {
            let mid = layout::pulse_pos(pa, pb, 0.5);
            painter.circle_filled(mid, PULSE_DOT_R, Style::ACCENT_HI.gamma_multiply(activity));
            return;
        }
        // Faster + denser the busier the link; dots evenly phase-staggered.
        let speed = 0.15 + 0.85 * f64::from(activity);
        let dots = 1 + (activity * 3.0) as usize; // 1..=4
        for k in 0..dots {
            let phase = (time * speed + k as f64 / dots as f64).fract() as f32;
            let p = layout::pulse_pos(pa, pb, phase);
            painter.circle_filled(p, PULSE_DOT_R * 2.0, Style::ACCENT.gamma_multiply(0.22));
            painter.circle_filled(p, PULSE_DOT_R, Style::ACCENT_HI);
        }
    }

    /// The elected-leader accent ring: a steady ring plus an expanding, fading
    /// ripple that breathes on the shared Motion cadence. Static when reduced
    /// motion is set.
    fn paint_leader_ring(&self, painter: &egui::Painter, c: Pos2, r: f32, time: f64) {
        let base = r + LEADER_RING_GAP;
        painter.circle_stroke(c, base, Stroke::new(NODE_STROKE_W, Style::ACCENT));
        if self.reduce_motion {
            return;
        }
        // Smooth 0 → 1 → 0 heartbeat off the Motion-derived period.
        let phase = (time / LEADER_PULSE_SECS).fract() as f32;
        let pulse = 0.5 - 0.5 * (phase * TAU).cos();
        painter.circle_stroke(
            c,
            base + pulse * LEADER_RING_PULSE,
            Stroke::new(NODE_STROKE_W, Style::ACCENT.gamma_multiply(1.0 - pulse)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{a11y_summary, version_line, MeshView};
    use crate::state::{Health, MeshLink, MeshNode, MeshState, Role};
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;

    /// Drive one headless egui frame that shows `state`, then tessellate the
    /// result on the CPU so any paint-path fault (bad shape/text/geometry)
    /// surfaces as a test failure. This is the same `Context::run` →
    /// `tessellate` path the DRM runner drives, minus the GPU — no window, no
    /// wgpu. It makes the paint code (untestable via the pure `layout` fns)
    /// runtime-reachable in `cargo test`.
    fn render(state: &MeshState, reduce_motion: bool, time: f64) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))),
            time: Some(time),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                MeshView::new(state).reduce_motion(reduce_motion).show(ui);
            });
        });
        // Tessellation is where a malformed shape or text call would blow up;
        // a non-empty primitive list confirms the frame actually drew something.
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "frame produced no draw primitives");
    }

    #[test]
    fn empty_state_paints_instead_of_a_blank_canvas() {
        // No nodes ⇒ the honest "waiting for mesh" EmptyState path (§6/§7),
        // exercised end-to-end (glyph geometry + both text lines).
        render(&MeshState::default(), false, 0.0);
    }

    #[test]
    fn empty_state_is_static_under_reduced_motion() {
        // No nodes + reduced motion ⇒ the honest "waiting for mesh" EmptyState
        // with its scan ripple frozen (the static empty branch), still a real
        // painted frame — the glyph + both text lines, no animation.
        render(&MeshState::default(), true, 2.0);
    }

    #[test]
    fn populated_state_paints_the_full_animated_path() {
        // Leader ring + Ok/Warn/Down health + Server role + active, idle and
        // dangling links exercise every branch of the animated paint path; the
        // version sub-labels cover current / older-build / absent-version lines.
        let state = MeshState {
            nodes: vec![
                MeshNode::new("lh", "lighthouse", Role::Lighthouse, Health::Ok)
                    .leader()
                    .version("12.0.0"),
                MeshNode::new("srv", "server-01", Role::Server, Health::Ok).version("12.0.0"),
                MeshNode::new("a", "peer-a", Role::Workstation, Health::Warn).version("11.4.1"), // older build
                MeshNode::new("b", "peer-b", Role::Workstation, Health::Down), // no version → "—"
            ],
            links: vec![
                MeshLink::new("lh", "a", 0.9),     // active: travelling pulses
                MeshLink::new("lh", "b", 0.0),     // idle: hairline only
                MeshLink::new("lh", "ghost", 0.5), // unknown endpoint → skipped
            ],
        };
        // Mark the older-build peer stale so the WARN version branch renders.
        let mut state = state;
        state.nodes[2].stale = true;
        render(&state, false, 0.75);
    }

    #[test]
    fn a11y_summary_voices_the_live_mesh_and_the_empty_state() {
        // Empty ⇒ the honest "waiting for mesh" read-out (no fabricated peers, §7),
        // so a screen reader hears the same state the EmptyState paints.
        let empty = a11y_summary(&MeshState::default());
        assert!(
            empty.contains("waiting for mesh"),
            "empty canvas voices the waiting state: {empty}"
        );

        // Populated ⇒ the node total + per-tier health counts, matching the menu
        // bar's status cluster so the read-out and the chips agree.
        let state = MeshState {
            nodes: vec![
                MeshNode::new("lh", "lighthouse", Role::Lighthouse, Health::Ok),
                MeshNode::new("a", "peer-a", Role::Workstation, Health::Warn),
                MeshNode::new("b", "peer-b", Role::Workstation, Health::Down),
            ],
            links: vec![],
        };
        let summary = a11y_summary(&state);
        assert!(summary.contains("3 nodes"), "node total: {summary}");
        assert!(summary.contains("1 up"), "up count: {summary}");
        assert!(summary.contains("1 degraded"), "degraded count: {summary}");
        assert!(summary.contains("1 down"), "down count: {summary}");

        // A single node reads out without the plural "s" (grammatical honesty).
        let one = MeshState {
            nodes: vec![MeshNode::new("lh", "lh", Role::Lighthouse, Health::Ok)],
            links: vec![],
        };
        assert!(
            a11y_summary(&one).contains("1 node —"),
            "singular reads '1 node'"
        );
    }

    #[test]
    fn version_line_renders_current_stale_and_absent_honestly() {
        // Current build → dim; older build → marked + WARN so it stands out;
        // absent version → an honest "—", never a fabricated build (§7).
        let current = MeshNode::new("a", "a", Role::Workstation, Health::Ok).version("12.0.0");
        assert_eq!(
            version_line(&current),
            ("12.0.0".to_string(), Style::TEXT_DIM)
        );

        let old = MeshNode::new("b", "b", Role::Workstation, Health::Warn)
            .version("11.4.1")
            .stale();
        let (text, color) = version_line(&old);
        assert!(text.starts_with("11.4.1"), "keeps the build string: {text}");
        assert!(text.contains("old"), "flags the older build: {text}");
        assert_eq!(color, Style::WARN, "an older build reads WARN");

        let none = MeshNode::new("c", "c", Role::Server, Health::Ok);
        assert_eq!(version_line(&none), ("—".to_string(), Style::TEXT_DIM));
    }

    #[test]
    fn reduce_motion_takes_the_static_branches() {
        let state = MeshState {
            nodes: vec![
                MeshNode::new("lh", "lighthouse", Role::Lighthouse, Health::Ok).leader(),
                MeshNode::new("a", "peer-a", Role::Workstation, Health::Ok),
            ],
            links: vec![MeshLink::new("lh", "a", 0.7)],
        };
        // Static mid-dot + static leader ring (the reduced-motion branches).
        render(&state, true, 1.5);
    }
}
