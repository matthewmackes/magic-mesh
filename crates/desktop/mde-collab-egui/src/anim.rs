//! Shared **motion glue** for the Communications surface (Quasar-dark lock #4 —
//! "macOS-level motion: all of it, all shared"; lock #2 — subtle, not floaty).
//!
//! This crate mints **no** bespoke duration or easing of its own: every timing,
//! curve, and micro-interaction factor here reads the shared `mde_egui::motion`
//! table (`Motion::FAST`, the `MotionPreset` specs, and the
//! `hover_lift`/`press_scale`/`focus_glow` helpers). This module only *composes*
//! those primitives into the surface's chrome motion — the five interaction states
//! a spaces-rail row / mode tab wears, the mode-switch crossfade, and the staggered
//! list / call-bar entrances — so the Construct frame eases exactly like every other
//! surface. Every helper is reduce-motion-aware through the one shared
//! [`Motion::mode`](mde_egui::Motion::mode) flag (a11y-07).

use mde_egui::egui;
use mde_egui::{focus, Motion, MotionPreset, Style};

use crate::Mode;

/// How many rows deep a staggered entrance keeps offsetting before it caps — past
/// this the tail all enters together, so a long list never trickles in for seconds.
const ENTRANCE_STAGGER_CAP: usize = 6;

/// The opacity a mode body dips to at the **start** of a switch before it fades
/// back to full — a gentle crossfade wash-in (lock #2, subtle), never a blank
/// swap. Kept legible (not `0.0`) so the content is always readable mid-transition.
const MODE_FADE_FLOOR: f32 = 0.25;

/// The stable display index of a [`Mode`] (its position in [`Mode::TABS`]) — the
/// key the mode-switch crossfade tracks so it can tell a real change from a repaint.
#[must_use]
pub(crate) fn mode_index(mode: Mode) -> usize {
    Mode::TABS.iter().position(|m| *m == mode).unwrap_or(0)
}

/// Scale `rect` about its own centre by `scale` — the geometry a press squash uses
/// (feed it [`Motion::press_scale`], `0.97..=1.0`) so a pressed cell's wash contracts
/// a hair toward the pointer rather than snapping. Pure; identity at `scale == 1.0`.
#[must_use]
pub(crate) fn scale_about_center(rect: egui::Rect, scale: f32) -> egui::Rect {
    egui::Rect::from_center_size(rect.center(), rect.size() * scale)
}

/// Paint one **interactive chrome cell** — a spaces-rail row or a mode tab — wearing
/// the shared five interaction states, all on the `mde_egui::motion` table:
///
/// * **rest** — no wash;
/// * **hover** — a [`Style::SURFACE_HI`] wash lifted in on the FAST tier
///   ([`Motion::hover_lift`], the documented brightness-bump reading of the lift);
/// * **press** — the wash plate squashes toward its centre ([`Motion::press_scale`]);
/// * **selected** — the steady shared [`Style::selection_fill`] tint (immediate, so
///   keyboard navigation never trails a lagging wash);
/// * **focus** — the shared 2px keyboard focus ring (lock #5) eased in on
///   [`Motion::focus_glow`].
///
/// `content` paints the cell's glyph + label; it is laid out first (so the cell
/// self-measures) and the wash lands *behind* it via the shared reserve-then-set
/// idiom. Returns the click [`Response`](egui::Response) so the caller reads
/// `.clicked()`. `full_width` stretches the wash + hit area to the panel's right edge
/// (the rail rows), otherwise the cell hugs its content (the inline mode tabs).
pub(crate) fn interactive_cell(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash,
    selected: bool,
    full_width: bool,
    content: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    // Reserve a slot so the wash lands BEHIND the glyph + label, then lay the
    // content and measure it (the shared reserve-then-set row-background idiom).
    let bg = ui.painter().add(egui::Shape::Noop);
    let mut rect = ui.horizontal(content).response.rect;
    if full_width {
        rect.max.x = rect.max.x.max(ui.max_rect().right());
    }

    let id = ui.make_persistent_id(salt);
    let resp = ui.interact(rect, id, egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, ""));

    // Hover / press / focus all ease on the shared FAST micro-interaction tier —
    // never a bespoke literal — and each collapses to its endpoint under
    // reduce-motion (a11y-07), since `Motion::animate` honours the shared flag.
    let ctx = ui.ctx();
    let hover = Motion::animate(ctx, id.with("hover"), resp.hovered(), Motion::FAST);
    let press = Motion::animate(
        ctx,
        id.with("press"),
        resp.is_pointer_button_down_on(),
        Motion::FAST,
    );
    let focus = Motion::animate(ctx, id.with("focus"), resp.has_focus(), Motion::FAST);

    let plate = scale_about_center(rect, Motion::press_scale(press));
    let painter = ui.painter();
    if selected {
        painter.set(
            bg,
            egui::Shape::rect_filled(plate, Style::RADIUS_S, Style::selection_fill()),
        );
    } else {
        let lift = Motion::hover_lift(hover);
        if lift > 0.0 {
            painter.set(
                bg,
                egui::Shape::rect_filled(
                    plate,
                    Style::RADIUS_S,
                    Style::SURFACE_HI.gamma_multiply(lift),
                ),
            );
        }
    }

    let glow = Motion::focus_glow(focus);
    if glow > 0.0 {
        painter.rect_stroke(
            plate.shrink(focus::FOCUS_RING_W / 2.0),
            Style::RADIUS_S,
            egui::Stroke::new(focus::FOCUS_RING_W, Style::ACCENT_HI.gamma_multiply(glow)),
            egui::StrokeKind::Inside,
        );
    }
    resp
}

/// Fade the active mode **body** in on a switch — a crossfade wash, never an instant
/// swap (lock #4). On every change of `target_index` the fade clock restarts and the
/// body eases from [`MODE_FADE_FLOOR`] back to full on the shared
/// [`MotionPreset::Page`] tier; a repaint stays scheduled while it travels. The fade
/// is distance-independent (a jump across many tabs reads the same as a neighbour
/// switch) and reduce-motion-aware via the shared [`Motion::mode`]. `body` renders
/// the mode content into the faded scope, keeping its normal layout intact.
pub(crate) fn switch_body(
    ui: &mut egui::Ui,
    target_index: usize,
    body: impl FnOnce(&mut egui::Ui),
) {
    let mode_id = ui.id().with("collab-switch-mode");
    let at_id = ui.id().with("collab-switch-at");
    let now = ui.input(|i| i.time);
    let start = ui.ctx().data_mut(|d| {
        if d.get_temp::<usize>(mode_id) == Some(target_index) {
            d.get_temp::<f64>(at_id).unwrap_or(now)
        } else {
            d.insert_temp(mode_id, target_index);
            d.insert_temp(at_id, now);
            now
        }
    });
    let elapsed = (now - start) as f32;
    let progress = Motion::spec(MotionPreset::Page).progress_at(elapsed.max(0.0), Motion::mode());
    let fade = (1.0 - MODE_FADE_FLOOR).mul_add(progress, MODE_FADE_FLOOR);
    ui.scope(|ui| {
        ui.multiply_opacity(fade);
        body(ui);
    });
    if progress < 1.0 {
        ui.ctx().request_repaint();
    }
}

/// Give a newly-appearing list row a **staggered entrance** — a subtle fade-up on the
/// shared [`MotionPreset::Layout`] (list-insert) tier, offset one [`Motion::FAST`] tick
/// per row (capped at [`ENTRANCE_STAGGER_CAP`]) so a batch cascades rather than
/// popping in at once. The first frame a `(kind, key)` row is seen stamps its start
/// time in egui memory, so only genuinely new rows animate — a row already on screen
/// is settled at full opacity and never re-enters. Reduce-motion shortens (Reduced) or
/// removes (Disabled) the travel through the shared [`Motion::mode`]. `body` renders
/// the row into the faded scope, so the list layout is never reflowed.
pub(crate) fn entrance(
    ui: &mut egui::Ui,
    kind: &'static str,
    key: impl std::hash::Hash,
    index: usize,
    body: impl FnOnce(&mut egui::Ui),
) {
    let id = egui::Id::new(("collab-entrance", kind, key));
    let now = ui.input(|i| i.time);
    // Stamp the first frame this row is seen so only genuinely new rows enter; a
    // row already stamped reads its start back and stays settled.
    let first = ui.ctx().data_mut(|d| {
        let seen = d.get_temp::<f64>(id);
        let start = seen.unwrap_or(now);
        if seen.is_none() {
            d.insert_temp(id, start);
        }
        start
    });
    let stagger = (index.min(ENTRANCE_STAGGER_CAP) as f32) * Motion::FAST;
    let elapsed = (now - first) as f32 - stagger;
    let progress = Motion::spec(MotionPreset::Layout).progress_at(elapsed.max(0.0), Motion::mode());
    ui.scope(|ui| {
        ui.multiply_opacity(progress);
        body(ui);
    });
    if progress < 1.0 {
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::{mode_index, scale_about_center};
    use crate::Mode;
    use mde_egui::egui::{pos2, Rect};

    #[test]
    fn mode_index_covers_every_tab_uniquely() {
        // Every tab maps to its display slot, so the crossfade tracker can tell one
        // mode from another (and an unknown mode falls back to the first tab).
        for (i, mode) in Mode::TABS.iter().enumerate() {
            assert_eq!(mode_index(*mode), i, "{mode:?} must index to its TABS slot");
        }
    }

    #[test]
    fn press_squash_contracts_toward_centre_and_is_identity_at_rest() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 40.0));
        // At rest (scale 1.0 — Motion::press_scale at t=0) the plate is unchanged.
        assert_eq!(scale_about_center(rect, 1.0), rect);
        // A press squash keeps the centre fixed, shrinks both axes, and never grows
        // past the cell — a contained, restrained press cue (lock #2).
        let squashed = scale_about_center(rect, 0.97);
        assert_eq!(squashed.center(), rect.center());
        assert!(squashed.width() < rect.width());
        assert!(squashed.height() < rect.height());
        assert!(
            rect.contains_rect(squashed),
            "the squashed plate stays inside the cell"
        );
    }
}
