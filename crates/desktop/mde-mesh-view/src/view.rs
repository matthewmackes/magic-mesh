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
use crate::state::{Health, MeshState, Role};

// ── Canvas geometry (pixels) ────────────────────────────────────────────────
// Colours come exclusively from `Style`; these are the inherent *dimensions* of
// a painted canvas (disc radii, stroke widths, pulse sizes), kept as named,
// documented consts — the clock-demo idiom. Spacing-shaped values are sourced
// from `Style::SP_*`.

/// Node disc radius for a Lighthouse (the largest role).
const NODE_R_LIGHTHOUSE: f32 = 12.0;
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

impl Role {
    /// Drawn disc radius for this role.
    const fn radius(self) -> f32 {
        match self {
            Self::Lighthouse => NODE_R_LIGHTHOUSE,
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

        // No nodes ⇒ nothing to place or animate: draw the honest "waiting for
        // mesh" canvas instead of a blank rect or fabricated peers (§6/§7).
        if self.state.nodes.is_empty() {
            Self::paint_empty_state(&painter, area);
            return response;
        }

        let centres = layout::place(self.state, area, self.margin);
        let index: HashMap<&str, usize> = self
            .state
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();

        let time = ui.input(|i| i.time);

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
            painter.text(
                c + Vec2::new(0.0, r + Style::SP_XS),
                Align2::CENTER_TOP,
                &node.label,
                FontId::new(Style::SMALL, FontFamily::Monospace),
                label_color,
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
    fn paint_empty_state(painter: &egui::Painter, area: Rect) {
        // Centre the glyph + title + subtitle block: the glyph's full height
        // plus the two stacked text lines and the gaps between them.
        let block_h =
            EMPTY_GLYPH_R * 2.0 + Style::SP_M + Style::HEADING + Style::SP_XS + Style::BODY;
        let cx = area.center().x;
        let glyph_c = Pos2::new(cx, area.center().y - block_h * 0.5 + EMPTY_GLYPH_R);

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

        // Title + dim subtitle, centre-top-anchored beneath the glyph.
        let title = painter.text(
            Pos2::new(cx, glyph_c.y + EMPTY_GLYPH_R + Style::SP_M),
            Align2::CENTER_TOP,
            "Waiting for mesh",
            FontId::new(Style::HEADING, FontFamily::Monospace),
            Style::TEXT,
        );
        painter.text(
            Pos2::new(cx, title.bottom() + Style::SP_XS),
            Align2::CENTER_TOP,
            "Peers and links appear here as nodes join.",
            FontId::new(Style::BODY, FontFamily::Monospace),
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
    use super::MeshView;
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
    fn populated_state_paints_the_full_animated_path() {
        // Leader ring + Ok/Warn/Down health + active, idle and dangling links
        // exercise every branch of the animated paint path.
        let state = MeshState {
            nodes: vec![
                MeshNode::new("lh", "lighthouse", Role::Lighthouse, Health::Ok).leader(),
                MeshNode::new("a", "peer-a", Role::Workstation, Health::Warn),
                MeshNode::new("b", "peer-b", Role::Workstation, Health::Down),
            ],
            links: vec![
                MeshLink::new("lh", "a", 0.9),     // active: travelling pulses
                MeshLink::new("lh", "b", 0.0),     // idle: hairline only
                MeshLink::new("lh", "ghost", 0.5), // unknown endpoint → skipped
            ],
        };
        render(&state, false, 0.75);
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
