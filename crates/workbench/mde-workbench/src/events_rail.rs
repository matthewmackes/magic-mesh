//! UNIFY-3 — the Live Events rail.
//!
//! The right-hand collapsible rail of the Unified Workbench design
//! (`docs/design/workbench/Workbench.dc.html`): a live, always-on stream of mesh
//! alerts beside whatever panel is active. It renders the **real** shared alert
//! lane (`mde_notify::read_shared_alert_items` — the same source the notification
//! center tails, §6 reuse, not a new transport) with severity counts + a scrolling
//! event list. Empty until the first poll; no placeholder rows (§7). Colours come
//! from `mde-theme` tokens via `mde_notify::severity_token` (§4 — no raw hex). The
//! near-black rail background matches the design's `#0d0d0d` chrome shade as a
//! token-derived darken of `palette.background`.

use cosmic::iced::widget::{column, container, row, scrollable, Space};
use cosmic::iced::{alignment, Background, Border, Length};
use cosmic::Element;

use crate::cosmic_compat::overlay_color_on;
use crate::cosmic_compat::prelude::*;
use crate::status_strip::{mono_text, pip};
use mde_notify::{severity_token, AlertItem, Severity};
use mde_theme::{carbon, FontSize, FontWeight, Palette, TypeRole};

/// Rail width — the design's 284 px right column.
pub const RAIL_WIDTH: f32 = 284.0;

/// Build the Live Events rail from the live alert items (newest-first).
pub fn view<'a, Message: 'a>(events: &[AlertItem]) -> Element<'a, Message> {
    let palette = crate::live_theme::palette();
    let sizes = FontSize::defaults();
    let weights = FontWeight::defaults();
    let rail_bg = overlay_color_on(palette.background, carbon::BLACK.into_cosmic_color(), 0.40);

    // --- header ---
    let header = container(
        row![
            pip(palette.success),
            Space::new().width(Length::Fixed(8.0)),
            mono_text("LIVE EVENTS", TypeRole::Caption, &sizes, &weights)
                .colr(palette.text_muted.into_cosmic_color()),
            Space::new().width(Length::Fill),
            mono_text("follow · all nodes", TypeRole::Caption, &sizes, &weights)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .align_y(alignment::Vertical::Center),
    )
    .padding([7u16, 12u16])
    .width(Length::Fill);

    // --- severity counts (computed from the live items) ---
    let (mut crit, mut warn, mut info, mut ok) = (0u32, 0u32, 0u32, 0u32);
    for e in events {
        match e.severity {
            Severity::Critical => crit += 1,
            Severity::Warning => warn += 1,
            Severity::Info => info += 1,
            Severity::Success => ok += 1,
        }
    }
    let counts = container(row![
        count_cell(crit, "CRIT", palette.danger, &sizes, &weights, &palette),
        count_cell(warn, "WARN", palette.warning, &sizes, &weights, &palette),
        count_cell(info, "INFO", palette.accent, &sizes, &weights, &palette),
        count_cell(ok, "OK", palette.success, &sizes, &weights, &palette),
    ])
    .width(Length::Fill);

    // --- event list (or an honest empty state) ---
    let list: Element<'a, Message> = if events.is_empty() {
        container(
            mono_text("no events", TypeRole::Caption, &sizes, &weights)
                .colr(palette.text_muted.into_cosmic_color()),
        )
        .padding(14u16)
        .into()
    } else {
        let mut col = column![].width(Length::Fill);
        for e in events {
            col = col.push(event_row(e, &sizes, &weights, &palette));
            col = col.push(divider(&palette));
        }
        scrollable(col).height(Length::Fill).into()
    };

    let body = column![
        header,
        divider(&palette),
        counts,
        divider(&palette),
        container(list).height(Length::Fill),
    ]
    .height(Length::Fill)
    .width(Length::Fill);

    container(body)
        .width(Length::Fixed(RAIL_WIDTH))
        .height(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            icon_color: Some(palette.text.into_cosmic_color()),
            background: Some(Background::Color(rail_bg)),
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

/// One severity counter cell (big number + label), tinted by the severity token.
fn count_cell<'a, Message: 'a>(
    n: u32,
    label: &'static str,
    color: mde_theme::Rgba,
    sizes: &FontSize,
    weights: &FontWeight,
    palette: &Palette,
) -> Element<'a, Message> {
    container(
        column![
            mono_text(n.to_string(), TypeRole::Subheading, sizes, weights)
                .colr(color.into_cosmic_color()),
            mono_text(label, TypeRole::Caption, sizes, weights)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .align_x(alignment::Horizontal::Center)
        .spacing(2),
    )
    .width(Length::Fill)
    .padding([6u16, 0u16])
    .align_x(alignment::Horizontal::Center)
    .into()
}

/// One event row: timestamp · severity pip · title/host (severity-tinted) · body.
fn event_row<'a, Message: 'a>(
    e: &AlertItem,
    sizes: &FontSize,
    weights: &FontWeight,
    palette: &Palette,
) -> Element<'a, Message> {
    let sev = severity_token(e.severity, palette);
    let tagline = match &e.host {
        Some(h) if !h.is_empty() => format!("{} · {h}", e.title),
        _ => e.title.clone(),
    };
    container(
        column![
            row![
                mono_text(fmt_hms(e.ts_unix_ms), TypeRole::Caption, sizes, weights)
                    .colr(palette.text_muted.into_cosmic_color()),
                Space::new().width(Length::Fixed(8.0)),
                pip(sev),
                Space::new().width(Length::Fixed(6.0)),
                mono_text(tagline, TypeRole::Caption, sizes, weights).colr(sev.into_cosmic_color()),
            ]
            .align_y(alignment::Vertical::Center),
            mono_text(e.body.clone(), TypeRole::Caption, sizes, weights)
                .colr(palette.text.into_cosmic_color()),
        ]
        .spacing(2),
    )
    .padding([6u16, 11u16])
    .width(Length::Fill)
    .into()
}

/// A 1 px full-width hairline divider in the border token.
fn divider<'a, Message: 'a>(palette: &Palette) -> Element<'a, Message> {
    let color = palette.border.into_cosmic_color();
    container(Space::new().height(Length::Fixed(1.0)).width(Length::Fill))
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(color)),
            border: Border {
                color: cosmic::iced::Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: None,
        })
        .into()
}

/// Format an epoch-ms timestamp as `HH:MM:SS` (UTC; no chrono dep). Honest — the
/// real recorded time — matching the design's mono timestamps.
fn fmt_hms(ts_ms: i64) -> String {
    let secs = (ts_ms / 1000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(sev: Severity, ts: i64) -> AlertItem {
        AlertItem {
            id: format!("id{ts}"),
            ts_unix_ms: ts,
            severity: sev,
            source: mde_notify::Source::System,
            topic: "mackesd::alert".to_string(),
            host: Some("oak".to_string()),
            title: "test".to_string(),
            body: "body".to_string(),
            read: false,
        }
    }

    #[test]
    fn rail_width_matches_design() {
        assert!((RAIL_WIDTH - 284.0).abs() < f32::EPSILON);
    }

    #[test]
    fn fmt_hms_is_zero_padded_utc() {
        // 14:32:07 UTC = 52327 s into the day → 52_327_000 ms.
        assert_eq!(fmt_hms(52_327_000), "14:32:07");
        assert_eq!(fmt_hms(0), "00:00:00");
    }

    #[test]
    fn renders_empty_and_populated_from_real_items() {
        let _empty = view::<()>(&[]);
        let items = [
            item(Severity::Critical, 1),
            item(Severity::Warning, 2),
            item(Severity::Info, 3),
            item(Severity::Success, 4),
        ];
        let _full = view::<()>(&items);
    }
}
