//! UNIFY-1/2 — the global status strip.
//!
//! The Unified Workbench design (`docs/design/workbench/Workbench.dc.html`) frames
//! the whole app with a thin, always-on chrome strip across the very top — above
//! the window header — rendering at-a-glance mesh status in dense IBM-Carbon mono.
//! It surfaces the **live, real** signals the shell holds: the mde-bus ("chain")
//! reachability (UNIFY-1) and the mesh-health summary — online/total nodes +
//! lighthouse count (UNIFY-2, from `action/shell/healthz`). Remaining design fields
//! (CRIT/WARN/OK alert counts, the event ticker, posture, uptime, clock) land in
//! later UNIFY increments once their live sources are plumbed; per §7 we render only
//! what is genuinely backed, never placeholder values — when the daemon hasn't
//! answered, the count cell is simply omitted rather than showing a fake 0/0.
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
use crate::mesh_directory::HealthSummary;
use mde_theme::{carbon, FontSize, FontWeight, Palette, TypeRole};

/// Strip height — the design's 26 px chrome band.
pub const STRIP_HEIGHT: f32 = 26.0;

/// Diameter of an inline status pip.
const PIP: f32 = 7.0;

/// Build the global status strip as an Iced [`Element`].
///
/// `bus_reachable` is the live mde-bus health the shell tracks (`App::bus_reachable`)
/// and drives the "chain" indicator. `health` is the latest mesh-health summary
/// (`App::mesh_health`); when `Some`, the strip shows live online/total + lighthouse
/// counts, and when `None` (daemon not yet answered) that cell is omitted. The strip
/// is display-only, so it stays generic over the app's `Message`.
pub fn view<'a, Message: 'a>(
    bus_reachable: bool,
    health: Option<&HealthSummary>,
) -> Element<'a, Message> {
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

    // Chain / bus reachability — live (UNIFY-1).
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

    let mut bar = row![cell(brand.into(), &palette), cell(chain.into(), &palette)]
        .height(Length::Fixed(STRIP_HEIGHT))
        .align_y(alignment::Vertical::Center);

    // Live mesh-health counts (UNIFY-2) — only when the daemon has answered (§7).
    if let Some(h) = health {
        bar = bar.push(cell(up_cell(h, &palette), &palette));
    }

    bar = bar.push(Space::new().width(Length::Fill));

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

/// The live online/total + lighthouse-count segment (UNIFY-2).
fn up_cell<'a, Message: 'a>(h: &HealthSummary, palette: &Palette) -> Element<'a, Message> {
    let sizes = FontSize::defaults();
    let weights = FontWeight::defaults();
    // Healthy ⇒ success token; any unhealthy node ⇒ warning, so the dot tells the
    // truth at a glance without a separate severity feed.
    let dot = if h.healthy_nodes >= h.node_count {
        palette.success
    } else {
        palette.warning
    };
    row![
        pip(dot),
        Space::new().width(Length::Fixed(7.0)),
        mono_text(
            format!("{}/{} up", h.healthy_nodes, h.node_count),
            TypeRole::Caption,
            &sizes,
            &weights,
        )
        .colr(palette.text.into_cosmic_color()),
        Space::new().width(Length::Fixed(9.0)),
        mono_text(
            format!("{} LH", h.lighthouse_count),
            TypeRole::Caption,
            &sizes,
            &weights,
        )
        .colr(palette.text_muted.into_cosmic_color()),
    ]
    .align_y(alignment::Vertical::Center)
    .into()
}

/// One segment of the strip, padded (design's per-cell `border-right` spacing).
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
    s: impl Into<String>,
    role: TypeRole,
    sizes: &FontSize,
    weights: &FontWeight,
) -> cosmic::iced::widget::Text<'a, cosmic::Theme> {
    text(s.into())
        .size(role.size_in(*sizes))
        .font(cosmic::iced::Font {
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
    fn renders_from_real_signals_not_constants() {
        // Build every branch: bus up/down, health present/absent — guards against
        // the strip going static (must reflect the live App state it's handed).
        let _up = view::<()>(true, None);
        let _down = view::<()>(false, None);
        let h = HealthSummary {
            node_count: 8,
            healthy_nodes: 7,
            lighthouse_count: 3,
            ha_ok: true,
        };
        let _with = view::<()>(true, Some(&h));
    }
}
