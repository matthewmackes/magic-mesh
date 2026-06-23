//! Density mode. Locks: Q26 (Comfortable default), Q27 (user
//! toggle in Settings > Appearance), UX-24 (density scales
//! spacing tokens only, never component dimensions).
//!
//! Two layers live here:
//!
//! * [`Density`] — the persisted user preference (Compact /
//!   Comfortable / Spacious, Q26/Q27). This is what
//!   [`Preferences`](crate::Preferences) stores and what
//!   [`Tokens`](crate::Tokens) resolves the spacing scale against.
//! * [`DensityScale`] (BEAUT-THEME) — the *presentation* spacing
//!   scale a single surface can opt into for a specific mode
//!   (Comfortable / Compact / **Presentation**), independent of
//!   the global preference. A dashboard rendered to a projector
//!   wants `Presentation` (generous rhythm, legible from across
//!   the room) without the user flipping their whole desktop to a
//!   looser density; a data-dense table wants `Compact` locally.
//!   It layers over the same [`spacing`](crate::spacing) metric
//!   tokens — UX-24 still holds: it scales spacing only, never
//!   component dimensions.

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

/// BEAUT-THEME — a **presentation** spacing scale a single surface opts into.
///
/// Layered over the [`spacing`](crate::spacing) metric tokens and distinct from
/// the global [`Density`] preference: this is a *per-surface* density mode (a
/// projector dashboard, a dense table) that does not touch the user's desktop
/// setting. UX-24 still holds — it scales spacing tokens only, never component
/// dimensions.
///
/// `Comfortable` is the neutral identity (1.0×) so a surface that picks the
/// default scale renders identically to one that scales the base tokens directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DensityScale {
    /// 0.80× spacing — information-dense local rhythm (a data table, a log
    /// pane) without flipping the whole desktop to [`Density::Compact`].
    Compact,
    /// 1.0× spacing — the neutral identity; matches the un-scaled metric
    /// tokens. The default per Q26.
    Comfortable,
    /// 1.50× spacing — generous rhythm for a surface meant to be read from a
    /// distance (a projected dashboard, a kiosk/“ambient” view). Larger than
    /// any global density step so presentation mode reads as deliberately
    /// roomy, not merely “spacious”.
    Presentation,
}

impl Default for DensityScale {
    /// The neutral identity (1.0×) — a surface that doesn't opt into a scale
    /// renders at the base metric tokens.
    fn default() -> Self {
        Self::Comfortable
    }
}

impl DensityScale {
    /// Multiplier applied to the [`spacing`](crate::spacing) tokens. UX-24:
    /// spacing only, never component dimensions.
    #[must_use]
    pub const fn spacing_multiplier(self) -> f32 {
        match self {
            DensityScale::Compact => 0.80,
            DensityScale::Comfortable => 1.00,
            DensityScale::Presentation => 1.50,
        }
    }

    /// Resolve the [`Space`](crate::spacing::Space) token set for this
    /// presentation scale, layered over the base metric tokens. Glue over the
    /// existing scaler — never a second copy of the scale math.
    #[must_use]
    pub fn space(self) -> crate::spacing::Space {
        crate::spacing::Space::scaled(self.spacing_multiplier())
    }

    /// Scale one base spacing value (px) by this presentation scale, rounding to
    /// the nearest whole px. Floors at 1 px so a token never collapses to 0
    /// under `Compact`.
    #[must_use]
    pub fn scale_px(self, base_px: u16) -> u16 {
        let v = ((base_px as f32) * self.spacing_multiplier()).round() as u16;
        v.max(1)
    }

    /// Stable identifier for persistence / config — e.g. a per-surface
    /// `[surface] density_scale = "presentation"` override.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            DensityScale::Compact => "compact",
            DensityScale::Comfortable => "comfortable",
            DensityScale::Presentation => "presentation",
        }
    }

    /// Parse from the persisted identifier. Returns `None` on unknown input.
    #[must_use]
    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "compact" => Some(DensityScale::Compact),
            "comfortable" => Some(DensityScale::Comfortable),
            "presentation" => Some(DensityScale::Presentation),
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

    // ── DensityScale (BEAUT-THEME presentation scale) ─────────────────────────

    #[test]
    fn density_scale_default_is_neutral_identity() {
        // Comfortable is the 1.0× identity — a surface that opts into the default
        // scale renders at the base metric tokens.
        assert_eq!(DensityScale::default(), DensityScale::Comfortable);
        assert!((DensityScale::Comfortable.spacing_multiplier() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn density_scale_multipliers_order_compact_lt_comfortable_lt_presentation() {
        let c = DensityScale::Compact.spacing_multiplier();
        let m = DensityScale::Comfortable.spacing_multiplier();
        let p = DensityScale::Presentation.spacing_multiplier();
        assert!(c < m, "compact tighter than comfortable");
        assert!(m < p, "presentation looser than comfortable");
        // Presentation is deliberately roomier than any global density step
        // (Spacious is 1.25×) so it reads as a distinct, projector-grade mode.
        assert!(p > Density::Spacious.spacing_multiplier());
    }

    #[test]
    fn density_scale_comfortable_matches_base_metric_tokens() {
        // UX-24 / single-source: the Comfortable scale must equal the un-scaled
        // base tokens exactly (it routes through the same scaler at 1.0×).
        let s = DensityScale::Comfortable.space();
        assert_eq!(s.sm, crate::spacing::BASE[2]);
        assert_eq!(s.lg2, crate::spacing::BASE[7]);
        assert_eq!(s, crate::spacing::Space::scaled(1.0));
    }

    #[test]
    fn density_scale_compact_shrinks_and_presentation_grows() {
        let c = DensityScale::Compact.space();
        let m = DensityScale::Comfortable.space();
        let p = DensityScale::Presentation.space();
        assert!(c.lg2 < m.lg2, "compact shrinks spacing");
        assert!(p.lg2 > m.lg2, "presentation grows spacing");
    }

    #[test]
    fn density_scale_scale_px_rounds_and_never_collapses_to_zero() {
        // 8 px × 0.80 = 6.4 → 6 (rounded).
        assert_eq!(DensityScale::Compact.scale_px(8), 6);
        // Identity at Comfortable.
        assert_eq!(DensityScale::Comfortable.scale_px(14), 14);
        // 14 px × 1.50 = 21.
        assert_eq!(DensityScale::Presentation.scale_px(14), 21);
        // A tiny token under Compact floors at 1 px, never 0.
        assert_eq!(DensityScale::Compact.scale_px(1), 1);
    }

    #[test]
    fn density_scale_id_round_trips() {
        for d in [
            DensityScale::Compact,
            DensityScale::Comfortable,
            DensityScale::Presentation,
        ] {
            assert_eq!(Some(d), DensityScale::from_id(d.id()));
        }
        assert!(DensityScale::from_id("spacious").is_none());
    }
}
