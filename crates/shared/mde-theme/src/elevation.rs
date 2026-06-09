//! Elevation token bundles (ANIM-8.c.1 — Q29 shadows + Q30 mixed-radius).
//!
//! Ties the corner-radius scale to the shadow scale so every floating
//! MDE surface uses a consistent radius + shadow pair without callers
//! having to hand-roll the values from `Radii` and `Shadow` separately.
//!
//! Q30 radius scale: inline 4 px / menus-popovers 8 px / modal 12 px.
//! Q29 shadow scale: inline none / menus-popovers raised / OSDs floating / modal modal.

use crate::shadows::Shadow;

/// Per-elevation visual token bundle (Q29 + Q30).
///
/// Each variant names a surface tier:
/// - `Inline` — inline controls, rows, badges. No shadow; 4 px radius.
/// - `PopoverMenu` — dropdown menus, notification chips, popovers.
///   Raised shadow; 8 px radius.
/// - `Floating` — OSDs, toasts, compact overlays (Q29 floating tier).
///   Floating shadow; 8 px radius.
/// - `Modal` — dialogs, Portal-full sheets, command palette.
///   Modal shadow; 12 px radius.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// Inline elements — rows, badges, chips. 4 px, no shadow.
    Inline,
    /// Dropdown menus + popovers. 8 px, raised shadow.
    PopoverMenu,
    /// OSDs, toasts, compact overlays. 8 px, floating shadow (Q29).
    Floating,
    /// Dialogs, sheets, command palette. 12 px, modal shadow.
    Modal,
}

impl Elevation {
    /// Corner radius (px) for this elevation tier (Q30).
    ///
    /// Maps to: `Radii::sm` (4) / `Radii::md` (8) / `Radii::lg` (12).
    pub const fn radius(self) -> u16 {
        match self {
            Elevation::Inline => 4,
            Elevation::PopoverMenu | Elevation::Floating => 8,
            Elevation::Modal => 12,
        }
    }

    /// Drop-shadow spec for this elevation tier (Q29).
    pub const fn shadow(self) -> Shadow {
        match self {
            Elevation::Inline => Shadow::none(),
            Elevation::PopoverMenu => Shadow::raised(),
            Elevation::Floating => Shadow::floating(),
            Elevation::Modal => Shadow::modal(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_has_no_shadow_and_4px_radius() {
        assert_eq!(Elevation::Inline.radius(), 4);
        assert_eq!(Elevation::Inline.shadow().blur as i32, 0);
    }

    #[test]
    fn popover_has_8px_and_raised_shadow() {
        assert_eq!(Elevation::PopoverMenu.radius(), 8);
        assert!(Elevation::PopoverMenu.shadow().blur > 0.0);
    }

    #[test]
    fn floating_has_8px_and_between_raised_modal_shadow() {
        assert_eq!(Elevation::Floating.radius(), 8);
        let f = Elevation::Floating.shadow();
        assert!(f.blur > Elevation::PopoverMenu.shadow().blur);
        assert!(f.blur < Elevation::Modal.shadow().blur);
    }

    #[test]
    fn modal_has_12px_and_modal_shadow() {
        assert_eq!(Elevation::Modal.radius(), 12);
        assert!(Elevation::Modal.shadow().blur >= 24.0);
    }

    #[test]
    fn shadow_blur_increases_with_elevation() {
        assert!(Elevation::Inline.shadow().blur < Elevation::PopoverMenu.shadow().blur);
        assert!(Elevation::PopoverMenu.shadow().blur < Elevation::Floating.shadow().blur);
        assert!(Elevation::Floating.shadow().blur < Elevation::Modal.shadow().blur);
    }

    #[test]
    fn radius_scale_matches_q30_spec() {
        // Q30: inline 4 / menus-popovers 8 / modal 12.
        assert_eq!(Elevation::Inline.radius(), 4);
        assert_eq!(Elevation::PopoverMenu.radius(), 8);
        assert_eq!(Elevation::Floating.radius(), 8);
        assert_eq!(Elevation::Modal.radius(), 12);
    }
}
