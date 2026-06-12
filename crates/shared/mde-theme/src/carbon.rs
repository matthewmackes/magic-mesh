//! The IBM Carbon color ramp — the single source of the platform's hex values.
//!
//! §4 pins the look to IBM Carbon and requires the tokens be **single-sourced in
//! `mde-theme`**, with no raw hex scattered across surfaces. [`Palette`] exposes
//! the eleven *semantic* dark/light roles; this module exposes the underlying
//! *named ramp steps* (`carbondesignsystem.com/elements/color/tokens`) so a
//! surface that needs a specific on-dark step (e.g. the voice-HUD picking Green
//! 40 over the Palette's Green 50) references the step **by name from here**
//! rather than re-typing the hex. One hex, many semantic choices.
//!
//! Change a value only with a Carbon reference + the pinning test below.

use crate::color::Rgba;

// ---- Gray ramp ----
/// Gray 10 — primary text on dark / lightest surface on light.
pub const GRAY_10: Rgba = Rgba::rgb(0xf4, 0xf4, 0xf4);
/// Carbon `gray-10-hover` — the hover/overlay companion to Gray 10
/// (carbondesignsystem.com states #e8e8e8).
pub const GRAY_10_HOVER: Rgba = Rgba::rgb(0xe8, 0xe8, 0xe8);
/// Gray 20.
pub const GRAY_20: Rgba = Rgba::rgb(0xe0, 0xe0, 0xe0);
/// Gray 30 — secondary text on dark.
pub const GRAY_30: Rgba = Rgba::rgb(0xc6, 0xc6, 0xc6);
/// Gray 50 — muted / helper text.
pub const GRAY_50: Rgba = Rgba::rgb(0x8d, 0x8d, 0x8d);
/// Gray 60 — hierarchical accent surface.
pub const GRAY_60: Rgba = Rgba::rgb(0x6f, 0x6f, 0x6f);
/// Gray 70 — overlay tier / divider.
pub const GRAY_70: Rgba = Rgba::rgb(0x52, 0x52, 0x52);
/// Gray 80 — Layer-02 / border-subtle on dark.
pub const GRAY_80: Rgba = Rgba::rgb(0x39, 0x39, 0x39);
/// Gray 90 — Layer-01 (cards, panels).
pub const GRAY_90: Rgba = Rgba::rgb(0x26, 0x26, 0x26);
/// Gray 100 — base background (default dark).
pub const GRAY_100: Rgba = Rgba::rgb(0x16, 0x16, 0x16);

// ---- Blue ramp (interactive) ----
/// Blue 20 — on-container text.
pub const BLUE_20: Rgba = Rgba::rgb(0xd0, 0xe2, 0xff);
/// Blue 40 — on-dark info / on-call pip.
pub const BLUE_40: Rgba = Rgba::rgb(0x78, 0xa9, 0xff);
/// Blue 50 — on-dark interactive accent.
pub const BLUE_50: Rgba = Rgba::rgb(0x45, 0x89, 0xff);
/// Blue 60 — the canonical interactive accent ([`Palette::accent`]).
pub const BLUE_60: Rgba = Rgba::rgb(0x0f, 0x62, 0xfe);
/// Blue 70 — interactive container.
pub const BLUE_70: Rgba = Rgba::rgb(0x00, 0x43, 0xce);

// ---- Teal ramp (the "self / local node" status hue) ----
/// Teal 30 — the local-node / "this machine" status hue, kept distinct from
/// the Blue interactive accent. Used by mde-files' self-peer marker.
pub const TEAL_30: Rgba = Rgba::rgb(0x3d, 0xdb, 0xd9);

// ---- Support ramp ----
/// Green 30 — on-dark success glyph.
pub const GREEN_30: Rgba = Rgba::rgb(0x6f, 0xdc, 0x8c);
/// Green 40 — on-dark success.
pub const GREEN_40: Rgba = Rgba::rgb(0x42, 0xbe, 0x65);
/// Green 50 — support-success ([`Palette::success`]).
pub const GREEN_50: Rgba = Rgba::rgb(0x24, 0xa1, 0x48);
/// Green 60 — success container.
pub const GREEN_60: Rgba = Rgba::rgb(0x19, 0x80, 0x38);
/// Yellow 30 — support-warning ([`Palette::warning`]).
pub const YELLOW_30: Rgba = Rgba::rgb(0xf1, 0xc2, 0x1b);
/// Red 50 — on-dark error.
pub const RED_50: Rgba = Rgba::rgb(0xfa, 0x4d, 0x56);
/// Red 60 — support-error ([`Palette::danger`]).
pub const RED_60: Rgba = Rgba::rgb(0xda, 0x1e, 0x28);

// ---- Extremes ----
/// Pure white — text on a saturated fill.
pub const WHITE: Rgba = Rgba::rgb(0xff, 0xff, 0xff);
/// Pure black — scrims / shadows.
pub const BLACK: Rgba = Rgba::rgb(0x00, 0x00, 0x00);

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the ramp to the published IBM Carbon values. A drift here is caught
    /// before it reaches any surface that single-sources from this module (§4).
    #[test]
    fn ramp_matches_published_carbon() {
        for (step, (r, g, b)) in [
            (GRAY_10, (0xf4, 0xf4, 0xf4)),
            (GRAY_10_HOVER, (0xe8, 0xe8, 0xe8)),
            (GRAY_30, (0xc6, 0xc6, 0xc6)),
            (GRAY_50, (0x8d, 0x8d, 0x8d)),
            (GRAY_60, (0x6f, 0x6f, 0x6f)),
            (GRAY_70, (0x52, 0x52, 0x52)),
            (GRAY_80, (0x39, 0x39, 0x39)),
            (GRAY_90, (0x26, 0x26, 0x26)),
            (GRAY_100, (0x16, 0x16, 0x16)),
            (BLUE_20, (0xd0, 0xe2, 0xff)),
            (BLUE_40, (0x78, 0xa9, 0xff)),
            (BLUE_50, (0x45, 0x89, 0xff)),
            (BLUE_60, (0x0f, 0x62, 0xfe)),
            (BLUE_70, (0x00, 0x43, 0xce)),
            (TEAL_30, (0x3d, 0xdb, 0xd9)),
            (GREEN_30, (0x6f, 0xdc, 0x8c)),
            (GREEN_40, (0x42, 0xbe, 0x65)),
            (GREEN_50, (0x24, 0xa1, 0x48)),
            (GREEN_60, (0x19, 0x80, 0x38)),
            (YELLOW_30, (0xf1, 0xc2, 0x1b)),
            (RED_50, (0xfa, 0x4d, 0x56)),
            (RED_60, (0xda, 0x1e, 0x28)),
        ] {
            assert_eq!((step.r, step.g, step.b), (r, g, b));
            assert_eq!(step.a, 1.0, "ramp steps are fully opaque");
        }
    }
}
