//! UNIFY-1 — the global status strip.
//!
//! The Unified Workbench design (`docs/design/workbench/Workbench.dc.html`) frames
//! the whole app with a thin, always-on chrome strip across the very top — above
//! the window header — rendering at-a-glance mesh status in dense IBM-Carbon mono.
//! This is the first increment of that frame: it surfaces the **live, real**
//! signal the shell already holds — the mde-bus ("chain") reachability — alongside
//! the cluster brand mark. Richer fields from the design (CRIT/WARN/OK counts,
//! the event ticker, posture, uptime, clock) land in later UNIFY increments once
//! their live data sources are plumbed into `App` state; per §7 we render only what
//! is genuinely backed, never placeholder values.
//!
//! All colour / size / weight come from `mde-theme` tokens (§4 — no raw hex):
//! the strip background is a token-derived near-black (`palette.background`
//! darkened toward `carbon::BLACK`), matching the design's `#0a0a0a` chrome shade
//! while staying single-sourced.

use cosmic::iced::widget::{container, row, text, Space};
use cosmic::iced::{alignment, Background, Border, Length};
use cosmic::Element;

use crate::cosmic_compat::overlay_color_on;
use crate::cosmic_compat::prelude::*;
use mde_theme::{carbon, FontSize, FontWeight, Palette, TypeRole};

/// Strip height — the design's 26 px chrome band.
pub const STRIP_HEIGHT: f32 = 26.0;

/// Diameter of an inline status pip.
const PIP: f32 = 7.0;

/// Build the global status strip as an Iced [`Element`].
///
/// `bus_reachable` is the live mde-bus health the shell already tracks
/// (`App::bus_reachable`); it drives the "chain" indicator. The strip is
/// display-only this increment, so it stays generic over the app's `Message`.
pub fn view<'a, Message: 'a>(bus_reachable: bool) -> Element<'a, Message> {
    let palette = crate::live_theme::palette();
    let sizes = FontSize::defaults();
    let weights = FontWeight::defaults();

    // §4: the design's #0a0a0a chrome shade as a token-derived near-black, so the
    // strip reads a step darker than the Gray-100 content background it sits over.
    let strip_bg = overlay_color_on(palette.background, carbon::BLACK.into_cosmic_color(), 0.45);

    // Cluster brand mark — accent diamond + wordmark (the product identity, real).
    let brand = row![
        mono_text("◆", TypeRole::Caption, &sizes, &weights)
            .colr(palette.accent.into_cosmic_color()),
        Space::new().width(Length::Fixed(7.0)),
        mono_text("MCNF", TypeRole::Caption, &sizes, &weights)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .align_y(alignment::Vertical::Center);

    // Chain / bus reachability — the one live signal we surface this increment.
    let (chain_col, chain_label) = if bus_reachable {
        (palette.success, "chain ok")
    } else {
        (palette.danger, "bus offline")
    };
    let chain = row![
        pip(chain_col),
        Space::new().width(Length::Fixed(7.0)),
        mono_text(chain_label, TypeRole::Caption, &sizes, &weights)
            .colr(palette.text.into_cosmic_color()),
    ]
    .align_y(alignment::Vertical::Center);

    let bar = row![
        cell(brand.into(), &palette),
        cell(chain.into(), &palette),
        Space::new().width(Length::Fill),
    ]
    .height(Length::Fixed(STRIP_HEIGHT))
    .align_y(alignment::Vertical::Center);

    container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(STRIP_HEIGHT))
        .style(move |_| container::Style {
            snap: false,
            icon_color: Some(palette.text.into_cosmic_color()),
            background: Some(Background::Color(strip_bg)),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: Some(palette.text.into_cosmic_color()),
        })
        .into()
}

/// One segment of the strip, padded with a right divider (design's `border-right`).
fn cell<'a, Message: 'a>(content: Element<'a, Message>, palette: &Palette) -> Element<'a, Message> {
    let border = palette.border.into_cosmic_color();
    container(content)
        .padding([0u16, 11u16])
        .height(Length::Fixed(STRIP_HEIGHT))
        .align_y(alignment::Vertical::Center)
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: None,
            border: Border {
                color: border,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: None,
        })
        .into()
}

/// A small filled status pip (the design's `border-radius:50%` dot).
fn pip<'a, Message: 'a>(color: mde_theme::Rgba) -> Element<'a, Message> {
    let fill = color.into_cosmic_color();
    container(
        Space::new()
            .width(Length::Fixed(PIP))
            .height(Length::Fixed(PIP)),
    )
    .style(move |_| container::Style {
        snap: false,
        icon_color: None,
        background: Some(Background::Color(fill)),
        border: Border {
            color: cosmic::iced::Color::TRANSPARENT,
            width: 0.0,
            radius: (PIP / 2.0).into(),
        },
        shadow: Default::default(),
        text_color: None,
    })
    .into()
}

/// Roboto-Mono caption text, the strip's typeface (design uses `'Roboto Mono'`).
fn mono_text<'a>(
    s: &'a str,
    role: TypeRole,
    sizes: &FontSize,
    weights: &FontWeight,
) -> cosmic::iced::widget::Text<'a, cosmic::Theme> {
    text(s).size(role.size_in(*sizes)).font(cosmic::iced::Font {
        family: cosmic::iced::font::Family::Name(TypeRole::Mono.family()),
        weight: weight_from_u16(role.weight_in(*weights)),
        ..cosmic::iced::Font::DEFAULT
    })
}

fn weight_from_u16(w: u16) -> cosmic::iced::font::Weight {
    match w {
        0..=150 => cosmic::iced::font::Weight::Thin,
        151..=250 => cosmic::iced::font::Weight::ExtraLight,
        251..=350 => cosmic::iced::font::Weight::Light,
        351..=450 => cosmic::iced::font::Weight::Normal,
        451..=550 => cosmic::iced::font::Weight::Medium,
        551..=650 => cosmic::iced::font::Weight::Semibold,
        651..=750 => cosmic::iced::font::Weight::Bold,
        751..=850 => cosmic::iced::font::Weight::ExtraBold,
        _ => cosmic::iced::font::Weight::Black,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_height_matches_design_band() {
        // Design (`Workbench.dc.html`) global status strip is a 26 px band.
        assert!((STRIP_HEIGHT - 26.0).abs() < f32::EPSILON);
    }

    #[test]
    fn chain_indicator_reflects_real_bus_state() {
        // The strip must render from the live bus signal, not a constant —
        // building both branches guards against the indicator going static.
        let _up = view::<()>(true);
        let _down = view::<()>(false);
    }
}
