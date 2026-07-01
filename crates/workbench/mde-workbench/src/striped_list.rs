//! CTRLSURF-7 — the shared zebra-striped list helper.
//!
//! Generalizes the `mde-notify-center` alternating base-shade row idiom
//! (NOTIFY-HUB-2) so every list / table panel stripes its rows from one
//! place instead of each hand-rolling an `if idx % 2` shade pick. The two
//! shades come from [`mde_theme::Palette::zebra_row`] — the two lowest
//! Carbon layer tokens alternated by row parity — so no panel mints a raw
//! colour for its stripe (§4). Two entry points:
//!
//!   * [`row_shade`] — the zebra [`Background`] for a row at `index`, to drop
//!     into a panel's own `container::Style` (keeps the panel's border /
//!     radius, swaps only the fill).
//!   * [`striped_row`] — wraps a bare `row![…]` in a zebra container + the
//!     standard row padding, for panels that push naked rows into a column.

use cosmic::iced::widget::container;
use cosmic::iced::{Background, Length, Padding};
use cosmic::Element;

use mde_theme::{Density, Palette, Space as MdeSpace};

use crate::cosmic_compat::prelude::*;

/// CTRLSURF-7 — the zebra background for a list row at `index` (0-based),
/// ready to drop into a `container::Style { background: Some(..) }`. Reads
/// the [`Palette::zebra_row`] token, so a panel that already builds its own
/// bordered row container swaps only its fixed fill for this (keeping its
/// border / radius) and inherits the shared stripe.
#[must_use]
pub fn row_shade(palette: Palette, index: usize) -> Background {
    Background::Color(palette.zebra_row(index).into_cosmic_color())
}

/// CTRLSURF-7 — wrap a bare list row in its zebra-striped container. For
/// panels that push naked `row![…]`s into a column (no per-row surface of
/// their own); this gives each row the alternating shade plus the standard
/// row padding so the stripe reads as a full-width band. Interactive rows
/// keep threading their own button / link widgets inside `inner`.
///
/// The padding is the Comfortable-density `Space` token (the app default);
/// the shade itself is density-independent, so a compact-density panel loses
/// nothing load-bearing by the fixed padding.
#[must_use]
pub fn striped_row<'a, Message: 'a>(
    inner: Element<'a, Message>,
    index: usize,
    palette: Palette,
) -> Element<'a, Message> {
    let shade = row_shade(palette, index);
    let space = MdeSpace::for_density(Density::Comfortable);
    let pad = Padding {
        top: f32::from(space.xs),
        right: f32::from(space.sm),
        bottom: f32::from(space.xs),
        left: f32::from(space.sm),
    };
    container(inner)
        .width(Length::Fill)
        .padding(pad)
        .sty(move |_| container::Style {
            background: Some(shade),
            ..container::Style::default()
        })
        .into()
}
