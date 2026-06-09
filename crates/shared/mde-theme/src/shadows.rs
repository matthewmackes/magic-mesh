//! Shadow tokens. Locks: Q20 (window — 1 px hairline ring +
//! 16 px ambient), Q44 modal backdrop blur is handled separately
//! at the modal surface (not a "shadow" in the elevation sense).
//!
//! The shadow tokens here describe a single drop-shadow + an
//! optional 1 px hairline ring layered on top (when adaptive
//! borders are active per Q7).

use crate::color::Rgba;

/// A drop-shadow spec — translated to the consumer's shadow API
/// (CSS, `iced::Shadow`, etc.).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// Horizontal offset (px). Positive = right.
    pub offset_x: f32,
    /// Vertical offset (px). Positive = down.
    pub offset_y: f32,
    /// Blur radius (px). Larger = softer.
    pub blur: f32,
    /// Spread radius (px). Rarely used; usually 0.
    pub spread: f32,
    /// Shadow color (typically black at low alpha for dark
    /// themes; black at slightly higher alpha for light).
    pub color: Rgba,
}

impl Shadow {
    /// `SHADOW_0` — no shadow. Sentinel.
    pub const fn none() -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 0.0,
            blur: 0.0,
            spread: 0.0,
            color: Rgba::rgba(0, 0, 0, 0.0),
        }
    }

    /// `SHADOW_1` — minimal lift. Cards on the surface tier.
    pub const fn lift() -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 1.0,
            blur: 2.0,
            spread: 0.0,
            color: Rgba::rgba(0, 0, 0, 0.10),
        }
    }

    /// `SHADOW_2` — raised. Popovers, sidebars over surface.
    pub const fn raised() -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 2.0,
            blur: 6.0,
            spread: 0.0,
            color: Rgba::rgba(0, 0, 0, 0.15),
        }
    }

    /// `SHADOW_2b` — floating overlay. OSDs, toasts, compact overlays
    /// (Q29 — between raised and modal in visual weight).
    pub const fn floating() -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 4.0,
            blur: 12.0,
            spread: 0.0,
            color: Rgba::rgba(0, 0, 0, 0.22),
        }
    }

    /// `SHADOW_3` — modal. Dialogs, command palette, Portal-full sheets.
    /// Q20 + Q29: the drop-shadow half of the layered spec (the 1 px
    /// hairline ring is rendered by the consumer via the adaptive-border
    /// palette token).
    pub const fn modal() -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 8.0,
            blur: 24.0,
            spread: 0.0,
            color: Rgba::rgba(0, 0, 0, 0.30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_transparent() {
        assert_eq!(Shadow::none().color.a, 0.0);
    }

    #[test]
    fn elevations_increase_in_blur() {
        assert!(Shadow::lift().blur < Shadow::raised().blur);
        assert!(Shadow::raised().blur < Shadow::floating().blur);
        assert!(Shadow::floating().blur < Shadow::modal().blur);
    }

    #[test]
    fn floating_shadow_is_between_raised_and_modal() {
        let f = Shadow::floating();
        assert!(f.blur > Shadow::raised().blur);
        assert!(f.blur < Shadow::modal().blur);
        // Q29: 22 % black, 4 px y, 12 px blur.
        assert_eq!(f.offset_y as i32, 4);
        assert_eq!(f.blur as i32, 12);
        assert!((f.color.a - 0.22).abs() < 0.001);
    }

    #[test]
    fn modal_shadow_matches_q20_spec() {
        // Q20 — modal-tier ambient shadow: 24 px blur, 30% black,
        // 8 px y-offset.
        let s = Shadow::modal();
        assert_eq!(s.blur as i32, 24);
        assert_eq!(s.offset_y as i32, 8);
        assert!((s.color.a - 0.30).abs() < 0.001);
    }
}
