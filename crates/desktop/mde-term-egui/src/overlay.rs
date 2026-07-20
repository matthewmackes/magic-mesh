//! Shared overlay-card **depth**: the one soft shadow every floating terminal
//! popover casts.
//!
//! The three foreground popovers over the terminal surface — the TERM-8 remote
//! picker ([`crate::picker`]), the TERM-10 saved-layouts overlay
//! ([`crate::layout_ui`]) and the TERM-11 appearance picker
//! ([`crate::appearance`]) — all paint the **same** `Area`/plate card
//! (`Order::Foreground`, anchored under the top edge). This module hands them the
//! single elevation the design ladder assigns a floating overlay so they read as
//! genuinely lifted off the grid, not flat on it.
//!
//! §4: it mints **no** colour of its own — the umbra comes straight from the
//! shared [`Elevation::Overlay`] token in `mde_egui`, cast into egui's shadow
//! type by the foundation's own [`Elevation::egui_shadow`] converter (design
//! lock #2 — "layered soft shadows", a translucent depth, never an opaque fill).
//! No surface hand-rolls the `Elevation → Shadow` field mapping anymore.

use mde_egui::egui::{self, Rect};
use mde_egui::style::Elevation;
use mde_egui::Style;

/// The soft-shadow shape a floating overlay card casts behind its `plate` — the
/// shared [`Elevation::Overlay`] token, cast to egui by the foundation's shared
/// [`Elevation::egui_shadow`] converter and shaped to the card's
/// [`Style::RADIUS`] corner. Reserve a `Shape::Noop` slot *before* the plate
/// fill and `set` it to this so the shadow paints behind the card without
/// changing a pixel of layout.
pub(crate) fn overlay_shadow(plate: Rect) -> egui::Shape {
    Elevation::Overlay
        .egui_shadow()
        .as_shape(plate, Style::RADIUS)
        .into()
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::overlay_shadow;
    use mde_egui::egui::{self, pos2, Rect};
    use mde_egui::style::Elevation;

    #[test]
    fn overlay_card_casts_the_shared_overlay_elevation_no_minted_colour() {
        let token = Elevation::Overlay.shadow();
        let shadow = Elevation::Overlay.egui_shadow();
        // Every field is the token's value, just cast onto epaint's small ints by
        // the shared foundation converter — nothing hand-tuned, and (crucially) no
        // minted Color32: the umbra is the token's own translucent black.
        assert_eq!(
            shadow.offset,
            [token.offset[0] as i8, token.offset[1] as i8]
        );
        assert_eq!(shadow.blur, token.blur as u8);
        assert_eq!(shadow.spread, token.spread as u8);
        assert_eq!(shadow.color, token.umbra);
        assert!(
            shadow.color.a() < 255,
            "depth is a translucent umbra, never opaque (lock #2)"
        );
    }

    #[test]
    fn overlay_shadow_shape_is_the_overlay_token_cast_at_the_plate() {
        let token = Elevation::Overlay.shadow();
        let plate = Rect::from_min_max(pos2(20.0, 20.0), pos2(340.0, 260.0));
        match overlay_shadow(plate) {
            egui::Shape::Rect(rect) => {
                assert_eq!(rect.blur_width, token.blur, "blur is the Overlay token's");
                assert_eq!(
                    rect.fill, token.umbra,
                    "fill is the token umbra — none minted"
                );
            }
            other => panic!("overlay shadow must be a blurred rect, got {other:?}"),
        }
    }
}
