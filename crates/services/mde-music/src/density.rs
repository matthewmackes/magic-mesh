//! MUSIC-RFX-10 — Carbon list/row + grid/card density for the music views.
//!
//! The album/playlist/search list surfaces previously carried scattered
//! pixel literals (`.spacing(8)`, `.size(13)`, `Length::Fixed(32.0)`, …) and
//! the library card grid likewise scattered them (`Length::Fixed(160.0)` card
//! width, `Length::Fixed(150.0)` art tile, `.spacing(8)` gutter, `.size(30)`
//! placeholder glyph, a `168.0` row-pitch constant). §4 locks every metric —
//! paddings, row heights, gaps, type sizes, component dimensions — to the
//! `mde-theme` tokens, the same way colours are single-sourced through the
//! Carbon `Palette`. This module is the one place that resolves those tokens
//! into the named, sonixd-class metrics the dense list rows ([`ListMetrics`])
//! and the card grid ([`GridMetrics`]) read, so there is no raw literal at the
//! call sites.
//!
//! Two derivations matter (UX-24): the *gaps/paddings* (vertical rhythm) are
//! density-scaled — the dense list reads them at [`Density::Compact`] to hit
//! the tight, information-dense rhythm the task asks for — while the *column
//! widths* (and the card/art tile dimensions) are component dimensions and are
//! NOT density-scaled (they read the unscaled Comfortable tokens so the columns
//! / cards line up identically across the user's density mode). Font sizes come
//! straight from the [`FontSize`] tiers (also not density-scaled, per UX-24).

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

/// MUSIC-RFX-10 — resolved Carbon metrics for the library **card grid**
/// (Albums / Artists / Genres / Podcasts / Playlists / Recents). Mirrors
/// [`ListMetrics`] for the row lists: every field traces to an `mde-theme`
/// token so the grid carries no bare pixel literal (§4).
///
/// The card / art-tile **dimensions** AND the inter-card **gutter** are
/// component geometry and are NOT density-scaled (UX-24 — same rule as
/// [`ListMetrics::number_col`]); they read the unscaled Comfortable token set so
/// a card — and the [`GridMetrics::row_pitch`] the virtualization math derives
/// from `card_width + gap` — is identical in every density mode (a
/// density-scaled gutter would shift the pitch and desync the
/// MUSIC-RESPONSIVE-9 spacer heights from the rendered rows). The
/// placeholder-glyph size is a glyph dimension (token sum), not a [`FontSize`]
/// tier, since it scales the empty art tile rather than running text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridMetrics {
    /// Card width (px). Component dimension — not density-scaled.
    pub card_width: f32,
    /// Square cover-art tile height (px). Component dimension.
    pub art_height: f32,
    /// Gutter between adjacent cards (both axes). Component dimension (it sets
    /// the [`GridMetrics::row_pitch`] the virtualization depends on), so NOT
    /// density-scaled — it reads the Carbon `sm` (8 px) gutter token.
    pub gap: u16,
    /// Card title text size — the [`FontSize`] body tier (kept `f32` for
    /// iced's `text().size()`).
    pub title: f32,
    /// The ♪ placeholder-glyph size for an art tile that hasn't loaded yet —
    /// a glyph dimension (token sum), sized to fill the tile.
    pub placeholder_glyph: f32,
}

impl GridMetrics {
    /// Resolve the library card-grid metrics. The card / art / gutter
    /// dimensions all come from the unscaled Comfortable spacing tokens (stable
    /// cards + a stable [`GridMetrics::row_pitch`]); the title size is the body
    /// [`FontSize`] tier.
    #[must_use]
    pub fn carbon_dense() -> Self {
        // Unscaled dimensions: card + art-tile geometry AND the gutter are
        // component geometry (UX-24 — not density-scaled; the gutter feeds the
        // row pitch the virtualization depends on), so they read the Comfortable
        // token set.
        let dim: Space = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        let fs: FontSize = FontSize::defaults();
        Self {
            // 160 px card (`xxl2` 48 × 3 + `sm` 8 × 2 — a token sum, no raw
            // literal; on the Carbon grid).
            card_width: f32::from(dim.xxl2 * 3 + dim.sm * 2),
            // 150 px square art tile (`xxl2` 48 × 3 + `xs` 6).
            art_height: f32::from(dim.xxl2 * 3 + dim.xs),
            // 8 px inter-card gutter (the Carbon `sm` token).
            gap: dim.sm,
            title: fs.body,
            // 30 px placeholder glyph (`display` 22 + `sm` 8) — fills the empty
            // tile until cover art resolves.
            placeholder_glyph: fs.display + f32::from(dim.sm),
        }
    }

    /// The vertical row pitch (px): one card row plus its gutter. The library
    /// grid's adaptive column count and the MUSIC-RESPONSIVE-9 virtualization
    /// spacers both derive from this, so it lives beside the card dimensions
    /// rather than as a scattered constant. Equals `card_width + gap` — the
    /// exact arithmetic the column-count math (`(width + gap) / pitch`) and the
    /// off-window spacer heights rely on.
    #[must_use]
    pub fn row_pitch(self) -> f32 {
        self.card_width + f32::from(self.gap)
    }

    /// Adaptive column count for a viewport of `available_width` px: how many
    /// `row_pitch`-wide cards fit (at least one). Single-sources the grid's
    /// reflow math so the rendered layout and the virtualization window agree.
    #[must_use]
    pub fn columns_for_width(self, available_width: f32) -> usize {
        let pitch = self.row_pitch();
        ((available_width + f32::from(self.gap)) / pitch)
            .floor()
            .max(1.0) as usize
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
        assert!(
            m.row_gap < comfy.xs,
            "row gap must be denser than comfortable"
        );
        assert!(
            m.col_gap < comfy.sm,
            "col gap must be denser than comfortable"
        );
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

    #[test]
    fn grid_metrics_are_token_derived_not_literals() {
        // MUSIC-RFX-10 — every card-grid metric must trace to an mde-theme
        // token (§4), exactly like the list rows above.
        let g = GridMetrics::carbon_dense();
        let dim = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        let fs = FontSize::defaults();
        // Card + art dimensions trace to the unscaled Comfortable tokens.
        assert_eq!(g.card_width, f32::from(dim.xxl2 * 3 + dim.sm * 2));
        assert_eq!(g.art_height, f32::from(dim.xxl2 * 3 + dim.xs));
        // The gutter is the (unscaled) Carbon `sm` token — a component metric
        // that feeds the row pitch, so it is NOT density-scaled.
        assert_eq!(g.gap, dim.sm);
        // Title is the body FontSize tier; the placeholder glyph is a token sum.
        assert_eq!(g.title, fs.body);
        assert_eq!(g.placeholder_glyph, fs.display + f32::from(dim.sm));
    }

    #[test]
    fn grid_metrics_preserve_the_card_geometry() {
        // The token sums must resolve to the exact card geometry the grid view
        // and the MUSIC-RESPONSIVE-9 virtualization were tuned for (160 px card,
        // 150 px art tile, 8 px gutter, 168 px row pitch), so tokenizing changed
        // no pixel — only the source of the numbers.
        let g = GridMetrics::carbon_dense();
        assert_eq!(g.card_width, 160.0);
        assert_eq!(g.art_height, 150.0);
        assert_eq!(g.gap, 8);
        assert_eq!(g.placeholder_glyph, 30.0);
        assert_eq!(g.row_pitch(), 168.0);
    }

    #[test]
    fn grid_columns_reflow_with_width() {
        // The adaptive column count is monotonic in the viewport width and
        // always yields at least one column (a zero-column grid would divide by
        // zero downstream). At the legacy default width (1100) it matches the
        // previous hand-rolled `(w + 8) / 168` arithmetic.
        let g = GridMetrics::carbon_dense();
        assert_eq!(g.columns_for_width(0.0), 1, "never zero columns");
        assert!(g.columns_for_width(2000.0) > g.columns_for_width(500.0));
        assert_eq!(
            g.columns_for_width(1100.0),
            ((1100.0 + 8.0) / 168.0_f32).floor() as usize
        );
    }

    #[test]
    fn grid_dimensions_hold_the_carbon_grid() {
        // The card + art dimensions are SUMS of real spacing tokens (no off-grid
        // literal): each summand must itself be a base token, so the composite
        // dimension lands on the Carbon grid. (`snap_to_nearest_token` only maps
        // to a single token, so it can't validate a composite — we assert the
        // summands directly.)
        let dim = Tokens::resolve(Theme::Dark, Density::Comfortable).space;
        for token in [dim.xxl2, dim.sm, dim.xs] {
            assert_eq!(
                Space::snap_to_nearest_token(token),
                token,
                "each card-dimension summand is itself a base token"
            );
        }
    }
}
