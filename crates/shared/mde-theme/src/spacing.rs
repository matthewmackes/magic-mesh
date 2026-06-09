//! Spacing tokens. Locks: NFU-1 (12-step modular scale derived
//! from the 1.2 minor third type scale). UX-12's grid lint
//! enforces that the workspace uses only these values; UX-24
//! sub-lock requires that density modifiers scale these tokens
//! only — never component dimensions.

use crate::density::Density;

/// The 12 base spacing values, in px, before density scaling.
/// Source of truth for the lint table and the runtime resolver.
pub const BASE: [u16; 12] = [4, 6, 8, 10, 14, 17, 20, 24, 28, 34, 40, 48];

/// Named accessors over [`BASE`]. Match these to consumers'
/// vocabulary so call sites read as `space.lg` rather than
/// `space.idx(8)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space {
    /// `4 × density` — micro padding inside icon-glyph buttons.
    pub xs2: u16,
    /// `6 × density` — tight gap (e.g., between an icon and its
    /// inline label).
    pub xs: u16,
    /// `8 × density` — standard inner padding for controls.
    pub sm: u16,
    /// `10 × density` — slightly looser inner padding.
    pub sm2: u16,
    /// `14 × density` — gap between adjacent rows in a list.
    pub md: u16,
    /// `17 × density` — gap between adjacent sections.
    pub md2: u16,
    /// `20 × density` — panel inner padding (compact).
    pub lg: u16,
    /// `24 × density` — panel inner padding (comfortable).
    pub lg2: u16,
    /// `28 × density` — section header bottom gap.
    pub xl: u16,
    /// `34 × density` — top-of-panel margin to first row.
    pub xl2: u16,
    /// `40 × density` — wizard inner padding.
    pub xxl: u16,
    /// `48 × density` — wizard hero block padding.
    pub xxl2: u16,
}

impl Space {
    /// Resolve the spacing token set for a given density mode.
    /// UX-24: density scales tokens, not component dimensions.
    pub fn for_density(d: Density) -> Self {
        let mult = d.spacing_multiplier();
        let scale = |i: usize| ((BASE[i] as f32) * mult).round() as u16;
        Self {
            xs2: scale(0),
            xs: scale(1),
            sm: scale(2),
            sm2: scale(3),
            md: scale(4),
            md2: scale(5),
            lg: scale(6),
            lg2: scale(7),
            xl: scale(8),
            xl2: scale(9),
            xxl: scale(10),
            xxl2: scale(11),
        }
    }

    /// Snap an arbitrary value to the nearest token. Useful for
    /// UX-12 grid lint's "did you mean…" suggestions.
    pub fn snap_to_nearest_token(value: u16) -> u16 {
        let mut best = BASE[0];
        let mut best_d = (value as i32 - BASE[0] as i32).abs();
        for &b in &BASE[1..] {
            let d = (value as i32 - b as i32).abs();
            if d < best_d {
                best = b;
                best_d = d;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_is_12_steps() {
        assert_eq!(BASE.len(), 12);
    }

    #[test]
    fn base_first_and_last_match_nfu1_lock() {
        // NFU-1: 4 / 6 / 8 / 10 / 14 / 17 / 20 / 24 / 28 / 34 / 40 / 48
        assert_eq!(BASE[0], 4);
        assert_eq!(*BASE.last().unwrap(), 48);
    }

    #[test]
    fn base_is_monotonically_increasing() {
        for w in BASE.windows(2) {
            assert!(
                w[0] < w[1],
                "spacing scale must be monotonically increasing: {:?}",
                w
            );
        }
    }

    #[test]
    fn comfortable_density_does_not_scale() {
        let s = Space::for_density(Density::Comfortable);
        assert_eq!(s.sm, 8);
        assert_eq!(s.lg2, 24);
    }

    #[test]
    fn compact_shrinks_spacings_below_comfortable() {
        let c = Space::for_density(Density::Compact);
        let m = Space::for_density(Density::Comfortable);
        assert!(c.lg2 < m.lg2);
    }

    #[test]
    fn spacious_grows_spacings_above_comfortable() {
        let m = Space::for_density(Density::Comfortable);
        let s = Space::for_density(Density::Spacious);
        assert!(s.lg2 > m.lg2);
    }

    #[test]
    fn snap_picks_nearest() {
        // Unambiguous-nearest cases:
        assert_eq!(Space::snap_to_nearest_token(13), 14);
        assert_eq!(Space::snap_to_nearest_token(23), 24);
        assert_eq!(Space::snap_to_nearest_token(45), 48);
        // Tie-breaker: smaller token wins (stable ordering — the
        // first equally-distant candidate is kept). 7 is equidistant
        // from 6 and 8, snaps to 6; 5 from 4 and 6, snaps to 4;
        // 31 from 28 and 34, snaps to 28.
        assert_eq!(Space::snap_to_nearest_token(7), 6);
        assert_eq!(Space::snap_to_nearest_token(5), 4);
        assert_eq!(Space::snap_to_nearest_token(31), 28);
    }
}
