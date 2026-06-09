//! v4.0.1 WB-2.b — Maintain group root grid.
//!
//! Routes to when the operator clicks the "Maintain" sidebar
//! group header without picking a specific panel. Renders a 2×3
//! tile grid: Snapshots / Debloat / Health Check / Repair /
//! Drift / Logs. Each tile is a clickable card with a Material
//! Symbols glyph + the panel name + a short description.
//!
//! Chrome influence (per Phase 0.8): Win11 Settings landing
//! grid — square tiles, single accent per zone, 12 px gap.

use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::model::Group;

#[derive(Debug, Clone, Default)]
pub struct HubPanel;

impl HubPanel {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = Palette::dark();
        let sizes = FontSize::defaults();
        let title = text("Maintain")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());
        let body = text(
            "Keep MDE healthy. Capture snapshots before risky changes, \
             prune unused packages, probe system state, and reverse \
             drift before it compounds.",
        )
        .size(TypeRole::Body.size_in(sizes))
        .color(palette.text_muted.into_iced_color());

        let r1 = row![
            tile(
                "Snapshots",
                "Capture / restore the live config",
                Icon::Snapshot,
                "snapshots",
                palette
            ),
            Space::new().width(Length::Fixed(12.0)),
            tile(
                "Debloat",
                "Remove apps you don't use",
                Icon::Delete,
                "debloat",
                palette
            ),
            Space::new().width(Length::Fixed(12.0)),
            tile(
                "Health Check",
                "Probe daemons and services",
                Icon::StatusOk,
                "health_check",
                palette
            ),
        ];
        let r2 = row![
            tile(
                "Repair",
                "Reset broken settings",
                Icon::Repair,
                "repair",
                palette
            ),
            Space::new().width(Length::Fixed(12.0)),
            tile(
                "Drift",
                "Find config divergence",
                Icon::History,
                "drift",
                palette
            ),
            Space::new().width(Length::Fixed(12.0)),
            tile(
                "Logs",
                "Recent daemon + worker output",
                Icon::Logs,
                "logs",
                palette
            ),
        ];

        container(
            column![
                title,
                Space::new().height(Length::Fixed(6.0)),
                body,
                Space::new().height(Length::Fixed(24.0)),
                r1,
                Space::new().height(Length::Fixed(12.0)),
                r2,
            ]
            .spacing(0),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .into()
    }
}

fn tile<'a>(
    name: &'a str,
    description: &'a str,
    icon: Icon,
    panel_slug: &'static str,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let resolved = mde_icon(icon, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        let muted = palette.text_muted.into_iced_color();
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(resolved.size_px()))
            .height(Length::Fixed(resolved.size_px()))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style { color: Some(muted) },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(resolved.size_px())
            .color(palette.text_muted.into_iced_color())
            .into()
    };
    let name_text = text(name.to_string())
        .size(16)
        .color(palette.text.into_iced_color());
    let desc_text = text(description.to_string())
        .size(11)
        .color(palette.text_muted.into_iced_color());
    let inner = column![
        icon_widget,
        Space::new().height(Length::Fixed(8.0)),
        name_text,
        Space::new().height(Length::Fixed(2.0)),
        desc_text,
    ]
    .spacing(0);
    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    let muted_text = palette.text_muted.into_iced_color();
    button(inner)
        .width(Length::Fill)
        .padding(Padding::from([16u16, 16u16]))
        .style(move |_t: &Theme, status: iced::widget::button::Status| {
            let hover_bg = Color {
                r: bg.r * 1.08,
                g: bg.g * 1.08,
                b: bg.b * 1.08,
                a: bg.a,
            };
            iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(match status {
                    iced::widget::button::Status::Hovered => hover_bg,
                    _ => bg,
                })),
                text_color: muted_text,
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 8.0.into(),
                },
                shadow: iced::Shadow::default(),
            }
        })
        .on_press(crate::Message::SelectPanel {
            group: Group::Maintain,
            panel: panel_slug,
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_renders_without_panic() {
        let _ = HubPanel::new().view();
    }
}
