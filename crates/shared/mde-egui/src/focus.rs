//! Shared **keyboard-focus ring** (a11y — WCAG 2.4.7 *Focus Visible*).
//!
//! Raw-painted cells (`ui.interact(rect, id, Sense::click())`) get no default egui
//! focus visual, so every hand-rolled focusable widget across the shell drew its
//! own ring — the same `FOCUS_RING_W = 2.5` + `ACCENT_HI` stroke duplicated in
//! `dock/mod.rs`, `explorer/render.rs`, `console/mod.rs`. This module is the ONE
//! source of that indicator: a single **2px** token (design lock #5's "visible 2px
//! focus ring") and the two helpers surfaces call. The ring wears the lifted brand
//! accent [`Style::ACCENT_HI`] — the same rung egui's own `selection.stroke` derives
//! from — one step brighter than [`Style::ACCENT`] so it stays legible over the
//! selection wash a selected cell already wears.

use egui::{Painter, Rect, Stroke, StrokeKind};

use crate::style::Style;

/// Keyboard-focus-ring stroke width, in logical px. The design-lock #5 "visible
/// **2px** focus ring" — the one weight every focus indicator across the shell
/// shares, so a keyboard user reads focus at a consistent contrast on the
/// Quazar-dark ground.
pub const FOCUS_RING_W: f32 = 2.0;

/// The rect a focusable `cell`'s ring strokes when `focused`, or `None` when it
/// does not hold focus. Inset by half the stroke so the [`FOCUS_RING_W`]-wide ring
/// sits fully **inside** `cell` and never bleeds into a neighbour. A pure
/// geometry/decision seam — unit-testable without a live painter.
#[must_use]
pub fn focus_ring_rect(cell: Rect, focused: bool) -> Option<Rect> {
    focused.then(|| cell.shrink(FOCUS_RING_W / 2.0))
}

/// Paint the shared keyboard-focus ring on a raw-painted `cell` when `focused`: a
/// high-contrast [`Style::ACCENT_HI`] stroke, [`FOCUS_RING_W`] wide, drawn
/// [`StrokeKind::Inside`] with the tight [`Style::RADIUS_S`] corner so it hugs the
/// cell edge. A no-op when the cell is unfocused.
pub fn paint_focus_ring(painter: &Painter, cell: Rect, focused: bool) {
    if let Some(ring) = focus_ring_rect(cell, focused) {
        painter.rect_stroke(
            ring,
            Style::RADIUS_S,
            Stroke::new(FOCUS_RING_W, Style::ACCENT_HI),
            StrokeKind::Inside,
        );
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{focus_ring_rect, FOCUS_RING_W};
    use egui::{pos2, Rect};

    #[test]
    fn focus_ring_is_a_2px_token() {
        assert_eq!(FOCUS_RING_W, 2.0, "design lock #5 — the focus ring is 2px");
    }

    #[test]
    fn ring_sits_fully_inside_the_focused_cell_and_is_absent_when_unfocused() {
        let cell = Rect::from_min_max(pos2(10.0, 10.0), pos2(58.0, 58.0)); // a 48px cell
        assert_eq!(
            focus_ring_rect(cell, false),
            None,
            "unfocused cells ring nothing"
        );
        let ring = focus_ring_rect(cell, true).expect("focused cell rings");
        assert!(
            cell.contains_rect(ring),
            "the ring must not bleed past the cell edge"
        );
        // Inset by exactly half the stroke on every side.
        assert_eq!(ring.min.x - cell.min.x, FOCUS_RING_W / 2.0);
        assert_eq!(cell.max.y - ring.max.y, FOCUS_RING_W / 2.0);
    }
}
