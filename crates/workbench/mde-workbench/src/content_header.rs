//! UNIFY-4 — the content header.
//!
//! The Unified Workbench design's per-screen header bar
//! (`docs/design/workbench/Workbench.dc.html` lines 68-81): the **breadcrumb**
//! (left), a **search** box (centre, opens the real app launcher — not a dead
//! box), the live **`N/M up` + chain** status chips (right, from `App::mesh_health`
//! + `App::bus_reachable`), and the **Live-Events rail toggle** (far right, moved
//! here from the status strip). All real data (§7); all `mde-theme` tokens (§4).

use cosmic::iced::widget::button::{self, Status as ButtonStatus};
use cosmic::iced::widget::{container, row, Space};
use cosmic::iced::{alignment, Background, Border, Color, Length};
use cosmic::Element;

use crate::cosmic_compat::overlay_white_on;
use crate::cosmic_compat::prelude::*;
use crate::mesh_directory::HealthSummary;
use crate::status_strip::{mono_text, pip};
use mde_theme::{FontSize, FontWeight, Palette, TypeRole};

/// Content-header height — the design's 38 px bar.
pub const HEADER_HEIGHT: f32 = 38.0;

/// Search placeholder (design line 73).
const SEARCH_HINT: &str = "search · peers, services, panels…";
const SEARCH_WIDTH: f32 = 320.0;

/// Build the content header. `on_search` opens the real launcher (the app passes
/// `Message::FocusRequest("launcher")`); `on_toggle_events` toggles the rail.
pub fn view<'a, Message: Clone + 'a>(
    crumbs: String,
    health: Option<&HealthSummary>,
    bus_reachable: bool,
    events_open: bool,
    on_search: Message,
    on_toggle_events: Message,
) -> Element<'a, Message> {
    let palette = crate::live_theme::palette();
    let sizes = FontSize::defaults();
    let weights = FontWeight::defaults();

    let crumb = mono_text(crumbs, TypeRole::Caption, &sizes, &weights)
        .colr(palette.text_muted.into_cosmic_color());

    let search = search_box(on_search, &palette, &sizes, &weights);

    // Right cluster: live N/M up · chain · events toggle.
    let mut right = row![].align_y(alignment::Vertical::Center);
    if let Some(h) = health {
        right = right.push(chip(
            palette.success,
            format!("{}/{} up", h.healthy_nodes, h.node_count),
            palette.text,
            &sizes,
            &weights,
        ));
        right = right.push(div_v(&palette));
    }
    let (chain_col, chain_label) = if bus_reachable {
        (palette.success, "chain ok")
    } else {
        (palette.danger, "bus offline")
    };
    right = right.push(chip(
        chain_col,
        chain_label.to_string(),
        palette.text,
        &sizes,
        &weights,
    ));
    right = right.push(div_v(&palette));
    right = right.push(events_toggle(
        events_open,
        on_toggle_events,
        &palette,
        &sizes,
        &weights,
    ));

    let bar = row![
        crumb,
        Space::new().width(Length::Fill),
        search,
        Space::new().width(Length::Fill),
        right,
    ]
    .height(Length::Fixed(HEADER_HEIGHT))
    .align_y(alignment::Vertical::Center);

    container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(HEADER_HEIGHT))
        .padding([0u16, 14u16])
        .style(move |_| container::Style {
            snap: false,
            icon_color: Some(palette.text.into_cosmic_color()),
            background: Some(Background::Color(palette.background.into_cosmic_color())),
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

/// The search box — a button styled as an input that opens the real launcher.
fn search_box<'a, Message: Clone + 'a>(
    on_search: Message,
    palette: &Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, Message> {
    let pal = *palette;
    let content = row![
        mono_text("⌕", TypeRole::Caption, sizes, weights).colr(pal.text_muted.into_cosmic_color()),
        Space::new().width(Length::Fixed(8.0)),
        mono_text(SEARCH_HINT, TypeRole::Caption, sizes, weights)
            .colr(pal.text_muted.into_cosmic_color()),
    ]
    .align_y(alignment::Vertical::Center);
    cosmic::iced::widget::button(content)
        .width(Length::Fixed(SEARCH_WIDTH))
        .padding([4u16, 10u16])
        .on_press(on_search)
        .sty(move |_t: &cosmic::Theme, status: ButtonStatus| {
            let bg = match status {
                ButtonStatus::Hovered | ButtonStatus::Pressed => {
                    overlay_white_on(pal.surface, 0.10)
                }
                _ => pal.surface.into_cosmic_color(),
            };
            let border = Border {
                color: pal.border.into_cosmic_color(),
                width: 1.0,
                radius: 0.0.into(),
            };
            button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: pal.text_muted.into_cosmic_color(),
                icon_color: Some(pal.text_muted.into_cosmic_color()),
                border_color: border.color,
                border_width: border.width,
                border_radius: border.radius,
                border,
                shadow: cosmic::iced::Shadow::default(),
            }
        })
        .into()
}

/// A status chip: coloured pip + label.
fn chip<'a, Message: 'a>(
    color: mde_theme::Rgba,
    label: String,
    text_color: mde_theme::Rgba,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, Message> {
    container(
        row![
            pip(color),
            Space::new().width(Length::Fixed(6.0)),
            mono_text(label, TypeRole::Caption, sizes, weights)
                .colr(text_color.into_cosmic_color()),
        ]
        .align_y(alignment::Vertical::Center),
    )
    .padding([0u16, 10u16])
    .into()
}

/// A 1×16 px vertical divider in the border token (design's header dividers).
fn div_v<'a, Message: 'a>(palette: &Palette) -> Element<'a, Message> {
    let color = palette.border.into_cosmic_color();
    container(
        Space::new()
            .width(Length::Fixed(1.0))
            .height(Length::Fixed(16.0)),
    )
    .style(move |_| container::Style {
        snap: false,
        icon_color: None,
        background: Some(Background::Color(color)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        shadow: Default::default(),
        text_color: None,
    })
    .into()
}

/// The Live-Events rail toggle (design's `⟨/⟩ events`).
fn events_toggle<'a, Message: Clone + 'a>(
    open: bool,
    on_toggle: Message,
    palette: &Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, Message> {
    let pal = *palette;
    let label = if open { "⟨ events" } else { "⟩ events" };
    let txt = if open { pal.text } else { pal.text_muted }.into_cosmic_color();
    let border_col = if open { pal.overlay } else { pal.border }.into_cosmic_color();
    let content = mono_text(label, TypeRole::Caption, sizes, weights).colr(txt);
    cosmic::iced::widget::button(content)
        .padding([4u16, 9u16])
        .on_press(on_toggle)
        .sty(move |_t: &cosmic::Theme, status: ButtonStatus| {
            let bg = match status {
                ButtonStatus::Hovered | ButtonStatus::Pressed => {
                    Some(Background::Color(overlay_white_on(pal.surface, 0.08)))
                }
                _ => None,
            };
            let border = Border {
                color: border_col,
                width: 1.0,
                radius: 0.0.into(),
            };
            button::Style {
                snap: false,
                background: bg,
                text_color: txt,
                icon_color: Some(txt),
                border_color: border.color,
                border_width: border.width,
                border_radius: border.radius,
                border,
                shadow: cosmic::iced::Shadow::default(),
            }
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_height_matches_design() {
        assert!((HEADER_HEIGHT - 38.0).abs() < f32::EPSILON);
    }

    #[test]
    fn renders_with_and_without_health() {
        let _none = view::<()>("Mesh / Peers".into(), None, true, true, (), ());
        let h = HealthSummary {
            node_count: 8,
            healthy_nodes: 7,
            lighthouse_count: 3,
            ha_ok: true,
        };
        let _some = view::<()>("Mesh / Peers".into(), Some(&h), false, false, (), ());
    }
}
