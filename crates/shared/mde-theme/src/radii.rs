//! Corner-radius tokens. Locks: Q41 (button radius = 8 px),
//! Q45 (modal radius = 16 px). Inputs use a smaller radius (6 px,
//! derived from § 7) so the eye reads them as data-entry vs
//! action.

/// Corner-radius token set in px. Apply via the consumer
/// widget's `border-radius` analogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Radii {
    /// 0 px — sharp.
    pub none: u16,
    /// 4 px — chips, badges.
    pub sm: u16,
    /// 6 px — text inputs.
    pub input: u16,
    /// 8 px — buttons, cards, panels (Q41 lock).
    pub md: u16,
    /// 12 px — secondary modal / sheet corners (now unused after
    /// Q45 raised modal radius; kept for backwards-compat in case
    /// a future surface wants this tier).
    pub lg: u16,
    /// 16 px — modals (Q45 lock).
    pub modal: u16,
    /// 9999 — full / pill (status badges).
    pub full: u16,
}

impl Radii {
    /// Defaults per Q41 + Q45.
    pub const fn defaults() -> Self {
        Self {
            none: 0,
            sm: 4,
            input: 6,
            md: 8,
            lg: 12,
            modal: 16,
            full: 9999,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_radius_is_8_per_q41() {
        assert_eq!(Radii::defaults().md, 8);
    }

    #[test]
    fn modal_radius_is_16_per_q45() {
        assert_eq!(Radii::defaults().modal, 16);
    }

    #[test]
    fn input_radius_is_smaller_than_button() {
        let r = Radii::defaults();
        assert!(r.input < r.md);
    }

    #[test]
    fn full_radius_is_pill_shaped() {
        assert!(Radii::defaults().full >= 999);
    }
}
