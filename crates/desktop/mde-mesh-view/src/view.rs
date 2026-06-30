//! The [`MeshView`] widget — a procedural, animated canvas of the current mesh
//! state, drawn entirely with [`egui::Painter`] primitives (`line_segment` /
//! `circle_filled` / `circle_stroke` / `text` — no pre-rendered images) and
//! themed exclusively through the shared [`Style`].
//!
//! Like the egui clock demo, it is repainted every frame off the frame clock
//! (`ui.input(|i| i.time)` + `request_repaint`), so links pulse and the leader
//! ring breathes continuously. The render path draws **only** the [`MeshState`]
//! it is handed — there is no embedded demo data here (the runnable sample lives
//! in `examples/mesh_view.rs`).

use std::collections::HashMap;
use std::f32::consts::TAU;

use mde_egui::egui::{
    self, Align2, Color32, FontFamily, FontId, Pos2, Response, Sense, Stroke, Ui, Vec2,
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
