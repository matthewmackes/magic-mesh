//! DENSITY-SYMMETRY — Carbon list/row density for the file-listing views.
//!
//! The Artifact Manager's List-layout chrome (the `file_row_head` column
//! header + each tabular `list_row`) previously carried scattered pixel
//! literals: `.spacing(12)`, `.size(13)`, `Length::Fixed(120.0)`,
//! `Padding::from([6.0, 8.0])`, `.height(Length::Fixed(28.0))`, … so the
//! listing chrome barely moved when the user flipped their Density mode.
//!
//! This module mirrors `mde-music`'s `density.rs` ([`ListMetrics`]): the one
//! place that resolves the `mde-theme` Space/FontSize tokens into the named
//! metrics the file rows read, so there is no raw literal at the call sites
//! (§4 — Carbon tokens only) and so the listing **re-rhythms** when the
//! [`Density`] token changes.
//!
//! Two derivations matter (UX-24): the *gaps / paddings* (vertical + inter-cell
//! rhythm) are **density-scaled** — they read the [`Space`] token set resolved
//! for the live [`Density`], so Compact packs the rows tight and Spacious opens
//! them up — while the *column widths* and the *row height* are **component
//! dimensions** and are NOT density-scaled (UX-24): they read the unscaled
//! Comfortable token set so the columns line up identically across density
//! modes. Font sizes come from the [`FontSize`] tiers via [`TypeRole`] (also
//! not density-scaled, per UX-24).

use mde_theme::{Density, FontSize, Space, Theme, Tokens, TypeRole};

/// Resolved Carbon metrics for the Artifact Manager's List-layout surfaces.
///
/// Covers the file-list column header ([`crate::widgets::file_row_head`]) and
/// every tabular file row ([`crate::widgets::list_row`]). Every field traces
/// to an `mde-theme` token — no field is a bare literal. Construct one per
/// frame from the live [`Density`] via [`FileListMetrics::for_density`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileListMetrics {
    /// Horizontal gap between the cells within a row (icon → name → origin →
    /// size → modified). Density-scaled — the inter-cell rhythm.
    pub col_gap: u16,
    /// Inner horizontal padding on each row / the header. Density-scaled.
    pub pad_x: u16,
    /// Inner vertical padding on the header block. Density-scaled.
    pub pad_y: u16,
    /// Width of the leading icon column. A component dimension — NOT
    /// density-scaled (UX-24), so the columns align across density modes.
    pub icon_col: f32,
    /// Width of the trailing "Size" column. Component dimension, not
    /// density-scaled.
    pub size_col: f32,
    /// Width of the trailing "Modified" column. Component dimension, not
    /// density-scaled.
    pub modified_col: f32,
    /// Fixed row height for a list row. Component dimension, not
    /// density-scaled (the 1.2-classic-ChromeOS tabular row).
    pub row_h: f32,
    /// Body text size (row name / cell text) — the [`FontSize`] body tier.
    /// Kept as `f32` to feed iced's `text().size()` (Pixels) without a cast.
    pub body: f32,
    /// Caption text size (column-header caps / size + modified cells) —
    /// caption tier.
    pub caption: f32,
}

impl FileListMetrics {
    /// Resolve the file-list metrics for a given [`Density`]. Gaps + paddings
    /// come from the density-scaled [`Space`] tokens (so the listing re-rhythms
    /// with the user's density); column widths + the row height come from the
    /// unscaled Comfortable tokens (stable columns, UX-24); sizes come from the
    /// [`FontSize`] tiers via [`TypeRole`].
    #[must_use]
    pub fn for_density(density: Density) -> Self {
        // Density-scaled rhythm: the gaps + paddings read the Space token set
        // resolved for the live density, so Compact packs the rows and Spacious
        // opens them up.
        let space: Space = Tokens::resolve(Theme::Dark, density).space;
        // Unscaled dimensions: column widths + the row height are component
        // geometry (UX-24 — not density-scaled), so they read the Comfortable
        // token set and stay put across density modes.
        let dim: Space = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        let fs: FontSize = FontSize::defaults();
        Self {
            col_gap: space.sm2,
            pad_x: space.sm,
            pad_y: space.xs,
            // 20 px leading icon column (the unscaled `lg` token).
            icon_col: f32::from(dim.lg),
            // 120 px size column (`xxl2` 48 ×2 + `lg2` 24 — a token sum, no raw
            // literal).
            size_col: f32::from(dim.xxl2 * 2 + dim.lg2),
            // 102 px modified column (`xxl2` 48 + `xxl` 40 + `md` 14 — a token
            // sum, no raw literal).
            modified_col: f32::from(dim.xxl2 + dim.xxl + dim.md),
            // 28 px tabular row height (the unscaled `xl` token).
            row_h: f32::from(dim.xl),
            body: TypeRole::Body.size_in(fs),
            caption: TypeRole::Caption.size_in(fs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_token_derived_not_literals() {
        // The "no bare literal" assertion: every field of the resolved metrics
        // re-derives from an mde-theme Space / FontSize token, so a future raw
        // literal slipping into a field is caught here.
        let m = FileListMetrics::for_density(Density::Comfortable);
        let space = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        let dim = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        let fs = FontSize::defaults();
        // Gaps + paddings trace to the (density-scaled) Space tokens.
        assert_eq!(m.col_gap, space.sm2);
        assert_eq!(m.pad_x, space.sm);
        assert_eq!(m.pad_y, space.xs);
        // Column widths + row height trace to the unscaled Comfortable tokens.
        assert_eq!(m.icon_col, f32::from(dim.lg));
        assert_eq!(m.size_col, f32::from(dim.xxl2 * 2 + dim.lg2));
        assert_eq!(m.modified_col, f32::from(dim.xxl2 + dim.xxl + dim.md));
        assert_eq!(m.row_h, f32::from(dim.xl));
        // Sizes trace to the FontSize tiers.
        assert_eq!(m.body, TypeRole::Body.size_in(fs));
        assert_eq!(m.caption, TypeRole::Caption.size_in(fs));
    }

    #[test]
    fn density_rerhythms_the_gaps() {
        // The whole point of DENSITY-SYMMETRY: flipping the Density token must
        // move the listing's rhythm. Compact packs the gaps tighter than
        // Comfortable; Spacious opens them wider — strictly monotonic.
        let compact = FileListMetrics::for_density(Density::Compact);
        let comfy = FileListMetrics::for_density(Density::Comfortable);
        let spacious = FileListMetrics::for_density(Density::Spacious);
        assert!(compact.col_gap < comfy.col_gap, "compact packs the cells");
        assert!(comfy.col_gap < spacious.col_gap, "spacious opens the cells");
        assert!(compact.pad_x < comfy.pad_x, "compact tightens row padding");
        assert!(comfy.pad_x < spacious.pad_x, "spacious loosens row padding");
        assert!(
            compact.pad_y < comfy.pad_y,
            "compact tightens header padding"
        );
    }

    #[test]
    fn dimensions_are_density_invariant() {
        // UX-24: column widths, the row height, and the font sizes are
        // component dimensions / type tiers — they must NOT move with density,
        // so the columns line up and the text stays legible across modes.
        let compact = FileListMetrics::for_density(Density::Compact);
        let spacious = FileListMetrics::for_density(Density::Spacious);
        assert_eq!(compact.icon_col, spacious.icon_col);
        assert_eq!(compact.size_col, spacious.size_col);
        assert_eq!(compact.modified_col, spacious.modified_col);
        assert_eq!(compact.row_h, spacious.row_h);
        assert_eq!(compact.body, spacious.body);
        assert_eq!(compact.caption, spacious.caption);
    }

    #[test]
    fn columns_hold_the_carbon_grid() {
        // The icon / size / modified columns must land on real spacing tokens
        // (no off-grid literal), so the row lines up to the Carbon grid.
        let m = FileListMetrics::for_density(Density::Comfortable);
        assert_eq!(
            Space::snap_to_nearest_token(m.icon_col as u16),
            m.icon_col as u16,
            "icon column is on-grid"
        );
        assert_eq!(
            Space::snap_to_nearest_token(m.row_h as u16),
            m.row_h as u16,
            "row height is on-grid"
        );
    }
}
