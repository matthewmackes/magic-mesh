//! Density mode. Locks: Q26 (Comfortable default), Q27 (user
//! toggle in Settings > Appearance), UX-24 (density scales
//! spacing tokens only, never component dimensions).

/// User-selectable density. Default is Comfortable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Density {
    /// 0.75× spacing. Information-dense; matches Linear's default
    /// rhythm. Power-user mode.
    Compact,
    /// 1.0× spacing. Balanced rhythm; matches Apple System
    /// Settings. The default per Q26.
    Comfortable,
    /// 1.25× spacing. Generous padding. Best for low-DPI displays
    /// or accessibility-prioritized setups.
    Spacious,
}

impl Default for Density {
    fn default() -> Self {
        Self::Comfortable
    }
}

impl Density {
    /// Multiplier applied to spacing tokens. UX-24: spacing only;
    /// never component dimensions.
    pub fn spacing_multiplier(self) -> f32 {
        match self {
            Density::Compact => 0.75,
            Density::Comfortable => 1.00,
            Density::Spacious => 1.25,
        }
    }

    /// Stable identifier for persistence to
    /// `~/.config/mde/preferences.toml`.
    pub fn id(self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Comfortable => "comfortable",
            Density::Spacious => "spacious",
        }
    }

    /// Parse from the persisted identifier. Returns `None` on
    /// unknown input.
    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "compact" => Some(Density::Compact),
            "comfortable" => Some(Density::Comfortable),
            "spacious" => Some(Density::Spacious),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_comfortable_per_q26() {
        assert_eq!(Density::default(), Density::Comfortable);
    }

    #[test]
    fn multipliers_match_locks() {
        assert!((Density::Compact.spacing_multiplier() - 0.75).abs() < 0.001);
        assert!((Density::Comfortable.spacing_multiplier() - 1.00).abs() < 0.001);
        assert!((Density::Spacious.spacing_multiplier() - 1.25).abs() < 0.001);
    }

    #[test]
    fn id_round_trips() {
        for d in [Density::Compact, Density::Comfortable, Density::Spacious] {
            assert_eq!(Some(d), Density::from_id(d.id()));
        }
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(Density::from_id("ultra-compact").is_none());
    }
}
