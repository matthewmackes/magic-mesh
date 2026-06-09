//! Six render modes for the universal card subsystem (Portal-31).
//!
//! Each variant maps to a surface that consumes Cards:
//!
//! | Mode          | Surface(s)                                   |
//! | ------------- | -------------------------------------------- |
//! | `Segment`     | Dock breadcrumb (Portal-14)                  |
//! | `CascadeCard` | Hub / Library landing grids (Portal-17/19)   |
//! | `ListRow`     | Notification history, search results         |
//! | `MiniTreeCell`| Workspace cells in the Dock mini-tree        |
//! | `LockWidget`  | Lock screen widget row (Portal-25)           |
//! | `Hero`        | Full-detail page after enrichment (R5-Q17)   |
//!
//! The enum locks the set per R5-Q2; new modes require a schema-version
//! bump and a renderer in every consumer.

use serde::{Deserialize, Serialize};

/// All known render modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    /// Compact strip view — Dock breadcrumb segments.
    Segment,
    /// Card grid view — Hub / Library landing.
    CascadeCard,
    /// Single-row dense view — history lists.
    ListRow,
    /// Compact cell view — Dock mini-tree workspace cells.
    MiniTreeCell,
    /// Lock-screen widget — minimal status pip.
    LockWidget,
    /// Full-detail enriched view — landing page when a Card is opened.
    Hero,
}

impl RenderMode {
    /// Lower-bound canvas width in logical pixels the renderer should
    /// reserve for a card in this mode.  Layout engines consult this
    /// to size their cells; tests assert each mode reports a positive
    /// minimum so layout never collapses a card to zero width.
    pub fn min_width_px(self) -> u16 {
        match self {
            Self::Segment => 48,
            Self::CascadeCard => 220,
            Self::ListRow => 320,
            Self::MiniTreeCell => 24,
            Self::LockWidget => 96,
            Self::Hero => 480,
        }
    }

    /// Lower-bound canvas height.
    pub fn min_height_px(self) -> u16 {
        match self {
            Self::Segment => 28,
            Self::CascadeCard => 132,
            Self::ListRow => 48,
            Self::MiniTreeCell => 28,
            Self::LockWidget => 32,
            Self::Hero => 320,
        }
    }

    /// Iterate every mode in declaration order. Stable — consumers
    /// use this to build per-mode render tables.
    pub const ALL: &'static [RenderMode] = &[
        Self::Segment,
        Self::CascadeCard,
        Self::ListRow,
        Self::MiniTreeCell,
        Self::LockWidget,
        Self::Hero,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_render_modes_exist() {
        assert_eq!(RenderMode::ALL.len(), 6, "R5-Q2 locks 6 modes");
    }

    #[test]
    fn each_mode_has_positive_minimums() {
        for mode in RenderMode::ALL {
            assert!(mode.min_width_px() > 0, "{mode:?} width must be > 0");
            assert!(mode.min_height_px() > 0, "{mode:?} height must be > 0");
        }
    }

    #[test]
    fn hero_is_largest() {
        let hero_area = u32::from(RenderMode::Hero.min_width_px())
            * u32::from(RenderMode::Hero.min_height_px());
        for mode in RenderMode::ALL {
            if *mode == RenderMode::Hero {
                continue;
            }
            let other = u32::from(mode.min_width_px()) * u32::from(mode.min_height_px());
            assert!(hero_area > other, "Hero must be larger than {mode:?}");
        }
    }

    #[test]
    fn segment_and_mini_tree_cell_are_smallest_height_class() {
        // Both live in the 56 px Dock; widgets must fit within it.
        assert!(RenderMode::Segment.min_height_px() < 56);
        assert!(RenderMode::MiniTreeCell.min_height_px() < 56);
    }

    #[test]
    fn render_mode_round_trips_through_json() {
        for mode in RenderMode::ALL {
            let raw = serde_json::to_string(mode).unwrap();
            let back: RenderMode = serde_json::from_str(&raw).unwrap();
            assert_eq!(*mode, back);
        }
    }

    #[test]
    fn render_mode_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&RenderMode::CascadeCard).unwrap(),
            "\"cascade_card\""
        );
        assert_eq!(
            serde_json::to_string(&RenderMode::MiniTreeCell).unwrap(),
            "\"mini_tree_cell\""
        );
    }
}
