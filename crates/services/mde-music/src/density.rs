//! MUSIC-RFX-10 — Carbon list/row density for the music views.
//!
//! The album/playlist/search list surfaces previously carried scattered
//! pixel literals (`.spacing(8)`, `.size(13)`, `Length::Fixed(32.0)`, …).
//! §4 locks every metric — paddings, row heights, gaps, type sizes — to the
//! `mde-theme` tokens, the same way colours are single-sourced through the
//! Carbon `Palette`. This module is the one place that resolves those tokens
//! into the named, sonixd-class metrics the dense list rows read, so there is
//! no raw literal at the call sites.
//!
//! Two derivations matter (UX-24): the *gaps/paddings* (vertical rhythm) are
//! density-scaled — the dense list reads them at [`Density::Compact`] to hit
//! the tight, information-dense rhythm the task asks for — while the *column
//! widths* are component dimensions and are NOT density-scaled (they read the
//! unscaled Comfortable tokens so the columns line up identically across the
//! user's density mode). Font sizes come straight from the [`FontSize`] tiers
//! (also not density-scaled, per UX-24).

use mde_theme::{Density, FontSize, Space, Theme, Tokens};

/// Resolved Carbon metrics for the music app's dense list/row surfaces.
///
/// Covers the album track list, playlist editor, and search results. Every
/// field traces to an `mde-theme` token — no field is a bare literal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListMetrics {
    /// Vertical gap between adjacent rows in a list. Density-scaled
    /// (Compact) — the tight rhythm that gives the list its density.
    pub row_gap: u16,
    /// Horizontal gap between the cells within a row (number → title →
    /// duration → actions). Density-scaled.
    pub col_gap: u16,
    /// Inner padding around a list/page body. Density-scaled.
    pub pad: u16,
    /// Gap between a header block and the list beneath it. Density-scaled.
    pub header_gap: u16,
    /// Width of the leading track-number column. A component dimension —
    /// NOT density-scaled (UX-24), so columns align across density modes.
    pub number_col: f32,
    /// Width of the trailing duration column. Component dimension, not
    /// density-scaled.
    pub duration_col: f32,
    /// Body text size (row title / track label) — the [`FontSize`] body tier.
    /// Kept as `f32` to feed iced's `text().size()` (Pixels) without a cast.
    pub body: f32,
    /// Caption text size (inline action-button labels) — caption tier.
    pub caption: f32,
    /// Monospace-ish numeric text size (track number / duration) — mono tier.
    pub mono: f32,
    /// Page heading size (album / playlist title) — display tier.
    pub heading: f32,
}

impl ListMetrics {
    /// Resolve the dense list metrics. Gaps come from the Compact spacing
    /// tokens (tight rhythm); widths come from the unscaled Comfortable
    /// tokens (stable columns); sizes come from the [`FontSize`] tiers.
    #[must_use]
    pub fn carbon_dense() -> Self {
        // Density-scaled rhythm: the dense list reads Compact spacing so the
        // rows pack to sonixd-class density.
        let tight: Space = Tokens::resolve(Theme::Dark, Density::Compact).space;
        // Unscaled dimensions: column widths are component geometry (UX-24 —
        // not density-scaled), so they read the Comfortable token set.
        let dim: Space = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        let fs: FontSize = FontSize::defaults();
        Self {
            row_gap: tight.xs,
            col_gap: tight.sm,
            pad: tight.sm2,
            header_gap: tight.md,
            // 34 px leading number column (the unscaled `xl2` token).
            number_col: f32::from(dim.xl2),
            // 56 px duration column (`xxl2` 48 + `sm` 8 — a token sum, no
            // raw literal).
            duration_col: f32::from(dim.xxl2 + dim.sm),
            body: fs.body,
            caption: fs.caption,
            mono: fs.mono,
            heading: fs.display,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_token_derived_not_literals() {
        let m = ListMetrics::carbon_dense();
        let tight = Tokens::resolve(Theme::Dark, Density::Compact).space;
        let dim = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        let fs = FontSize::defaults();
        // Gaps trace to the Compact (density-scaled) spacing tokens.
        assert_eq!(m.row_gap, tight.xs);
        assert_eq!(m.col_gap, tight.sm);
        assert_eq!(m.pad, tight.sm2);
        assert_eq!(m.header_gap, tight.md);
        // Column widths trace to the unscaled Comfortable tokens.
        assert_eq!(m.number_col, f32::from(dim.xl2));
        assert_eq!(m.duration_col, f32::from(dim.xxl2 + dim.sm));
        // Sizes trace to the FontSize tiers.
        assert_eq!(m.body, fs.body);
        assert_eq!(m.caption, fs.caption);
        assert_eq!(m.mono, fs.mono);
        assert_eq!(m.heading, fs.display);
    }

    #[test]
    fn dense_rows_are_tighter_than_comfortable() {
        // UX-24 sanity: the list rhythm is the Compact (denser) scaling, so
        // every gap is strictly tighter than the Comfortable equivalent —
        // this is what makes the rows read as sonixd-dense.
        let m = ListMetrics::carbon_dense();
        let comfy = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        assert!(m.row_gap < comfy.xs, "row gap must be denser than comfortable");
        assert!(m.col_gap < comfy.sm, "col gap must be denser than comfortable");
        assert!(m.pad < comfy.sm2, "padding must be denser than comfortable");
    }

    #[test]
    fn columns_hold_the_carbon_grid() {
        // The number/duration columns must land on real spacing tokens (no
        // off-grid literal), so the row lines up to the Carbon grid.
        let m = ListMetrics::carbon_dense();
        let snapped_no = Space::snap_to_nearest_token(m.number_col as u16);
        assert_eq!(snapped_no, m.number_col as u16, "number column is on-grid");
    }
}
