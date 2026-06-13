//! PLANES-18 — Network ▸ Mesh DNS panel.
//!
//! The flat mesh name service (W74/W75): the `mesh_dns` worker turns
//! the replicated roster's overlay IPs into `<host>.mesh` records and
//! feeds them to systemd-resolved per-link (no server, no center). This
//! panel shells `mackesd dns list --json` and renders the same record
//! set the worker publishes, so the operator can see exactly which
//! names resolve and to which overlay IP.
//!
//! Read-only renderer (W88): the records derive from the roster; this
//! surface only shows them.

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Element, Length, Padding, Task};
use cosmic::Theme;
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// One `<host>.mesh → overlay-ip` record, parsed from
/// `mackesd dns list --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsRow {
    pub fqdn: String,
    pub overlay_ip: String,
}

#[derive(Debug, Clone, Default)]
pub struct DnsPanel {
    pub rows: Vec<DnsRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<DnsRow>, String>),
    RefreshClicked,
}

impl DnsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_records() }, |result| {
            crate::Message::Dns(Message::Loaded(result))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(rows)) => {
                self.rows = rows;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.rows = Vec::new();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, Theme> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Mesh DNS")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if self.last_run_at.is_some() {
            format!(
                "{} name{} resolve under .mesh",
                self.rows.len(),
                if self.rows.len() == 1 { "" } else { "s" }
            )
        } else {
            "click Refresh to load".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let accent = palette.accent.into_cosmic_color();
        let refresh_btn = button(
            text(if self.busy { "Loading…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                let bg = match status {
                    cosmic::iced::widget::button::Status::Hovered => Color {
                        r: accent.r * 1.10,
                        g: accent.g * 1.10,
                        b: accent.b * 1.10,
                        a: accent.a,
                    },
                    _ => accent,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: Color::WHITE,
                    icon_color: None,
                    border_color: Color::TRANSPARENT,
                    border_width: 0.0,
                    border_radius: 6.0.into(),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                }
            },
        )
        .on_press(crate::Message::Dns(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(dns_row(r, palette));
        }
        if self.rows.is_empty() && self.last_run_at.is_some() {
            rows_col = rows_col.push(empty_state_card(palette, self.error.as_deref()));
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                scrollable(rows_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn dns_row<'a>(r: &'a DnsRow, palette: Palette) -> Element<'a, crate::Message, Theme> {
    let resolved = mde_icon(Icon::Network, IconSize::Inline);
    let accent = palette.accent.into_cosmic_color();
    let icon_widget: Element<'a, crate::Message, Theme> =
        if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(16.0))
                .height(Length::Fixed(16.0))
                .sty(move |_t: &Theme| widget_svg::Style {
                    color: Some(accent),
                })
                .into()
        } else {
            text(resolved.fallback_glyph).size(16.0).colr(accent).into()
        };

    let line = row![
        icon_widget,
        text(r.fqdn.clone())
            .size(12)
            .colr(palette.text.into_cosmic_color()),
        Space::new().width(Length::Fill),
        text(r.overlay_ip.clone())
            .size(12)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(line)
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn empty_state_card<'a>(
    palette: Palette,
    error: Option<&'a str>,
) -> Element<'a, crate::Message, Theme> {
    let (icon_color, heading, body): (Color, String, String) = if let Some(err) = error {
        (
            palette.danger.into_cosmic_color(),
            "Couldn't read mesh DNS".to_string(),
            err.to_string(),
        )
    } else {
        (
            palette.accent.into_cosmic_color(),
            "No mesh names yet".to_string(),
            "Mesh DNS publishes <host>.mesh for every roster peer with a known overlay IP. \
             Once peers enrol and their overlay addresses are known, their names resolve here \
             (and on every box via systemd-resolved) with no DNS server or center."
                .to_string(),
        )
    };
    let icon_kind = if error.is_some() {
        Icon::StatusError
    } else {
        Icon::Network
    };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message, Theme> =
        if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(32.0))
                .height(Length::Fixed(32.0))
                .sty(move |_t: &Theme| widget_svg::Style {
                    color: Some(icon_color),
                })
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(32.0)
                .colr(icon_color)
                .into()
        };
    container(
        column![
            icon_widget,
            Space::new().height(Length::Fixed(8.0)),
            text(heading)
                .size(14)
                .colr(palette.text.into_cosmic_color()),
            text(body)
                .size(11)
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2)
        .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

// ---- I/O ------------------------------------------------------

/// Shell out to `mackesd dns list --json` and parse the records.
pub fn fetch_records() -> Result<Vec<DnsRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["dns", "list", "--json"])
        .output()
        .map_err(|e| format!("mackesd dns list failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd dns list exited non-zero: {stderr}"));
    }
    Ok(parse_records(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `dns list --json` array.
#[must_use]
pub fn parse_records(raw: &str) -> Vec<DnsRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    top.into_iter()
        .filter_map(|r| {
            let fqdn = r.get("fqdn").and_then(|v| v.as_str())?.to_string();
            let overlay_ip = r
                .get("overlay_ip")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(DnsRow { fqdn, overlay_ip })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_records_reads_the_list_shape() {
        let raw = r#"[
            {"fqdn":"pine.mesh","overlay_ip":"10.42.0.2"},
            {"fqdn":"oak.mesh","overlay_ip":"10.42.0.3"}
        ]"#;
        let rows = parse_records(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].fqdn, "pine.mesh");
        assert_eq!(rows[0].overlay_ip, "10.42.0.2");
    }

    #[test]
    fn parse_records_returns_empty_for_garbage() {
        assert!(parse_records("not json").is_empty());
        assert!(parse_records("").is_empty());
    }

    #[test]
    fn view_renders_rows_and_empty_without_panic() {
        let mut p = DnsPanel::new();
        p.rows = parse_records(r#"[{"fqdn":"pine.mesh","overlay_ip":"10.42.0.2"}]"#);
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
        let mut empty = DnsPanel::new();
        empty.last_run_at = Some(SystemTime::now());
        let _ = empty.view();
        empty.error = Some("mackesd down".into());
        let _ = empty.view();
    }
}
