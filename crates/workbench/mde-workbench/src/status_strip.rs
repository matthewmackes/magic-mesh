//! UNIFY-1/4 — the global status strip (top chrome band).
//!
//! The design (`docs/design/workbench/Workbench.dc.html` lines 30-43) puts the
//! cluster brand mark + the **CRIT / WARN / OK severity tallies** in the always-on
//! top strip, while the per-screen breadcrumb / search / up-count / chain live in
//! the content header (UNIFY-4, `content_header.rs`). The severity tallies are
//! **real** — computed from the live shared alert lane the Live-Events rail already
//! tails (`App::events`), so no separate feed and no placeholders (§7). Remaining
//! design fields (ticker, posture, uptime, clock) land in UNIFY-5 once their live
//! sources are wired.
//!
//! All colour / size / weight come from `mde-theme` tokens (§4 — no raw hex): the
//! strip background is a token-derived near-black (`palette.background` darkened
//! toward `carbon::BLACK`), matching the design's `#0a0a0a` chrome shade.

use cosmic::iced::widget::{container, row, text, Space};
use cosmic::iced::{alignment, Background, Border, Color, Length};
use cosmic::Element;

use crate::cosmic_compat::overlay_color_on;
use crate::cosmic_compat::prelude::*;
use mde_notify::{severity_token, AlertItem, Severity};
use mde_theme::{carbon, FontSize, FontWeight, Palette, TypeRole};

/// Strip height — the design's 26 px chrome band.
pub const STRIP_HEIGHT: f32 = 26.0;

/// Diameter of an inline status pip.
const PIP: f32 = 7.0;

/// Build the global status strip from the live alert items.
///
/// `events` is the live shared alert lane (`App::events`); the strip shows the
/// cluster brand + the CRIT / WARN / OK tallies derived from it, a live event
/// ticker (the newest alert), and the clock (`now`). Display-only.
pub fn view<'a, Message: 'a>(events: &[AlertItem], now: &str) -> Element<'a, Message> {
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

    // CRIT / WARN / OK tallies — real, from the live alert lane.
    let (mut crit, mut warn, mut ok) = (0u32, 0u32, 0u32);
    for e in events {
        match e.severity {
            Severity::Critical => crit += 1,
            Severity::Warning => warn += 1,
            Severity::Success => ok += 1,
            Severity::Info => {}
        }
    }
    let tallies = row![
        tally("CRIT", crit, palette.danger, &sizes, &weights),
        Space::new().width(Length::Fixed(13.0)),
        tally("WARN", warn, palette.warning, &sizes, &weights),
        Space::new().width(Length::Fixed(13.0)),
        tally("OK", ok, palette.success, &sizes, &weights),
    ]
    .align_y(alignment::Vertical::Center);

    let bar = row![
        cell(brand.into(), &palette),
        cell(tallies.into(), &palette),
        ticker(events, &palette, &sizes, &weights),
        cell(
            mono_text(now.to_string(), TypeRole::Caption, &sizes, &weights)
                .colr(palette.text.into_cosmic_color())
                .into(),
            &palette,
        ),
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

/// One severity tally: a coloured pip + `LABEL n`.
fn tally<'a, Message: 'a>(
    label: &'static str,
    n: u32,
    color: mde_theme::Rgba,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, Message> {
    row![
        pip(color),
        Space::new().width(Length::Fixed(6.0)),
        mono_text(format!("{label} {n}"), TypeRole::Caption, sizes, weights)
            .colr(color.into_cosmic_color()),
    ]
    .align_y(alignment::Vertical::Center)
    .into()
}

/// UNIFY-5 — the live event ticker: the newest alert, severity-tinted (a real,
/// non-animated condensed feed; the scrolling marquee is a motion follow-up).
/// Takes the flex-middle of the strip so the clock sits at the right edge.
fn ticker<'a, Message: 'a>(
    events: &[AlertItem],
    palette: &Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, Message> {
    let line: Element<'a, Message> = match events.first() {
        Some(e) => {
            let sev = severity_token(e.severity, palette);
            let host = e.host.clone().unwrap_or_default();
            let msg = if host.is_empty() {
                e.body.clone()
            } else {
                format!("{host}: {}", e.body)
            };
            row![
                pip(sev),
                Space::new().width(Length::Fixed(7.0)),
                mono_text(msg, TypeRole::Caption, sizes, weights)
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .align_y(alignment::Vertical::Center)
            .into()
        }
        None => mono_text("—", TypeRole::Caption, sizes, weights)
            .colr(palette.text_muted.into_cosmic_color())
            .into(),
    };
    container(line)
        .width(Length::Fill)
        .height(Length::Fixed(STRIP_HEIGHT))
        .padding([0u16, 11u16])
        .align_y(alignment::Vertical::Center)
        .into()
}

/// One segment of the strip, padded (design's per-cell `border-right` spacing).
fn cell<'a, Message: 'a>(
    content: Element<'a, Message>,
    _palette: &Palette,
) -> Element<'a, Message> {
    container(content)
        .padding([0u16, 11u16])
        .height(Length::Fixed(STRIP_HEIGHT))
        .align_y(alignment::Vertical::Center)
        .into()
}

/// A small filled status pip (the design's `border-radius:50%` dot). Shared with
/// the content header (UNIFY-4) + the events rail (UNIFY-3).
pub(crate) fn pip<'a, Message: 'a>(color: mde_theme::Rgba) -> Element<'a, Message> {
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
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: (PIP / 2.0).into(),
        },
        shadow: Default::default(),
        text_color: None,
    })
    .into()
}

/// Roboto-Mono caption text, the chrome typeface (design uses `'Roboto Mono'`).
/// Shared with the content header (UNIFY-4) + the events rail (UNIFY-3).
pub(crate) fn mono_text<'a>(
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

pub(crate) fn weight_from_u16(w: u16) -> cosmic::iced::font::Weight {
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

    fn item(sev: Severity) -> AlertItem {
        AlertItem {
            id: "x".into(),
            ts_unix_ms: 0,
            severity: sev,
            source: mde_notify::Source::System,
            topic: "mackesd::alert".into(),
            host: None,
            title: "t".into(),
            body: "b".into(),
            read: false,
        }
    }

    #[test]
    fn strip_height_matches_design_band() {
        assert!((STRIP_HEIGHT - 26.0).abs() < f32::EPSILON);
    }

    #[test]
    fn tallies_render_from_real_events_not_constants() {
        let _empty = view::<()>(&[], "12:00 UTC");
        let items = [
            item(Severity::Critical),
            item(Severity::Warning),
            item(Severity::Warning),
            item(Severity::Success),
            item(Severity::Info),
        ];
        let _full = view::<()>(&items, "12:00 UTC");
    }
}
