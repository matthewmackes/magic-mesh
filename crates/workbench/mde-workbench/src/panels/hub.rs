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

use cosmic::iced::widget::{button, column, container, row, text, Space};
use cosmic::iced::{Background, Border, Color, Element, Length, Padding};
use cosmic::Theme;
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::model::Group;

#[derive(Debug, Clone, Default)]
pub struct HubPanel;

impl HubPanel {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub fn view(&self) -> Element<'_, crate::Message, Theme> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();
        let title = text("Maintain")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let body = text(
            "Keep MDE healthy. Capture snapshots before risky changes, \
             prune unused packages, probe system state, and reverse \
             drift before it compounds.",
        )
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

        let r1 = row![
            tile(
                "Snapshots",
                "Capture / restore the live config",
                Icon::Snapshot,
                Group::System,
                "snapshots",
                palette
            ),
            Space::new().width(Length::Fixed(12.0)),
            tile(
                "Debloat",
                "Remove apps you don't use",
                Icon::Delete,
                Group::System,
                "debloat",
                palette
            ),
            Space::new().width(Length::Fixed(12.0)),
            // PLANES-1 — Health re-homed to the This Node plane.
            tile(
                "Health Check",
                "Probe daemons and services",
                Icon::StatusOk,
                Group::Monitoring,
                "health_check",
                palette
            ),
        ];
        let r2 = row![
            tile(
                "Repair",
                "Reset broken settings",
                Icon::Repair,
                Group::System,
                "repair",
                palette
            ),
            Space::new().width(Length::Fixed(12.0)),
            // PLANES-1 — Drift folds into Controller/Remediation.
            tile(
                "Drift",
                "Find config divergence",
                Icon::History,
                Group::Fleet,
                "drift",
                palette
            ),
            Space::new().width(Length::Fixed(12.0)),
            tile(
                "Logs",
                "Recent daemon + worker output",
                Icon::Logs,
                Group::Monitoring,
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
    group: Group,
    panel_slug: &'static str,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let resolved = mde_icon(icon, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message, Theme> =
        if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            let muted = palette.text_muted.into_cosmic_color();
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(resolved.size_px()))
                .height(Length::Fixed(resolved.size_px()))
                .sty(move |_t: &Theme| widget_svg::Style { color: Some(muted) })
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(resolved.size_px())
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        };
    let name_text = text(name.to_string())
        .size(16)
        .colr(palette.text.into_cosmic_color());
    let desc_text = text(description.to_string())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());
    let inner = column![
        icon_widget,
        Space::new().height(Length::Fixed(8.0)),
        name_text,
        Space::new().height(Length::Fixed(2.0)),
        desc_text,
    ]
    .spacing(0);
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let muted_text = palette.text_muted.into_cosmic_color();
    button(inner)
        .width(Length::Fill)
        .padding(Padding::from([16u16, 16u16]))
        .sty(move |_t: &Theme, status: button::Status| {
            let hover_bg = Color {
                r: bg.r * 1.08,
                g: bg.g * 1.08,
                b: bg.b * 1.08,
                a: bg.a,
            };
            button::Style {
                snap: false,
                background: Some(Background::Color(match status {
                    button::Status::Hovered => hover_bg,
                    _ => bg,
                })),
                text_color: muted_text,
                icon_color: Some(muted_text),
                border_color: border,
                border_width: 1.0,
                border_radius: 8.0.into(),
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 8.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
            }
        })
        .on_press(crate::Message::SelectPanel {
            group,
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
