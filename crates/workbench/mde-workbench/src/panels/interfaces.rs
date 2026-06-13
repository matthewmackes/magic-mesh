//! PLANES-15 — Network ▸ Interfaces panel.
//!
//! The desired-vs-actual nmstate diff (W68): the elected fleet
//! revision declares the interfaces it manages (W67 BaselineSpec); this
//! panel shells `mackesd netstate diff --json`, which reads that desired
//! state and the box's live interfaces (via nmstatectl) and reports each
//! interface's sync status. A managed interface that has drifted from
//! its desired state shows DRIFT; unmanaged interfaces render as
//! informational context (nmstate declares only what it manages).
//!
//! Read-only renderer (W88): the desired state is authored as fleet
//! revisions, applied by the netstate engine with checkpoint auto-revert
//! (W77/W78) — this surface only shows the diff.

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// One interface's desired-vs-actual row, parsed from
/// `mackesd netstate diff --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceRow {
    pub name: String,
    pub managed: bool,
    pub desired_state: Option<String>,
    pub desired_ipv4: Option<String>,
    pub actual_state: Option<String>,
    pub actual_ipv4: Option<String>,
    /// `Some(true)` in sync, `Some(false)` drifted, `None` unmanaged.
    pub in_sync: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct InterfacesPanel {
    pub rows: Vec<InterfaceRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<InterfaceRow>, String>),
    RefreshClicked,
}

impl InterfacesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_interfaces() }, |result| {
            crate::Message::Interfaces(Message::Loaded(result))
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

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Interfaces")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let managed = self.rows.iter().filter(|r| r.managed).count();
        let drifted = self
            .rows
            .iter()
            .filter(|r| r.in_sync == Some(false))
            .count();
        let subtitle_text = if self.last_run_at.is_some() {
            format!(
                "{} interface{} · {managed} managed · {drifted} drifted",
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
                    icon_color: None,
                    text_color: Color::WHITE,
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
        .on_press(crate::Message::Interfaces(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(interface_row(r, palette));
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

fn interface_row<'a>(r: &'a InterfaceRow, palette: Palette) -> Element<'a, crate::Message> {
    let (status_icon, status_color, status_text) = match r.in_sync {
        Some(true) => (
            Icon::StatusOk,
            palette.success.into_cosmic_color(),
            "in sync".to_string(),
        ),
        Some(false) => (
            Icon::StatusWarning,
            palette.warning.into_cosmic_color(),
            "drift".to_string(),
        ),
        None => (
            Icon::Network,
            palette.text_muted.into_cosmic_color(),
            "unmanaged".to_string(),
        ),
    };

    let resolved = mde_icon(status_icon, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(status_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .colr(status_color)
            .into()
    };

    let head = row![
        icon_widget,
        text(r.name.clone())
            .size(12)
            .colr(palette.text.into_cosmic_color()),
        Space::new().width(Length::Fill),
        text(status_text).size(11).colr(status_color),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    // Desired and actual columns; "—" where a side isn't present.
    let desired = format!(
        "desired: {} / {}",
        r.desired_state.as_deref().unwrap_or("—"),
        r.desired_ipv4.as_deref().unwrap_or("—"),
    );
    let actual = format!(
        "actual: {} / {}",
        r.actual_state.as_deref().unwrap_or("—"),
        r.actual_ipv4.as_deref().unwrap_or("—"),
    );
    let desired_color = if r.managed {
        palette.accent.into_cosmic_color()
    } else {
        palette.text_muted.into_cosmic_color()
    };
    let body = column![
        text(desired).size(11).colr(desired_color),
        text(actual)
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(2);

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(column![head, body].spacing(4))
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

fn empty_state_card<'a>(palette: Palette, error: Option<&'a str>) -> Element<'a, crate::Message> {
    let (icon_kind, icon_color, heading, body): (Icon, Color, String, String) =
        if let Some(err) = error {
            (
                Icon::StatusError,
                palette.danger.into_cosmic_color(),
                "Couldn't read interfaces".to_string(),
                err.to_string(),
            )
        } else {
            (
                Icon::Network,
                palette.accent.into_cosmic_color(),
                "No interfaces to show".to_string(),
                "No fleet revision declares a managed nmstate, and the live nmstate \
                 reader returned nothing (NetworkManager / nmstate tooling may not be \
                 installed on this node). On a managed node, desired and actual \
                 interfaces appear here with their sync status."
                    .to_string(),
            )
        };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
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
            text(heading).size(14).colr(palette.text.into_cosmic_color()),
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

/// Shell out to `mackesd netstate diff --json` and parse the rows.
pub fn fetch_interfaces() -> Result<Vec<InterfaceRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["netstate", "diff", "--json"])
        .output()
        .map_err(|e| format!("mackesd netstate diff failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd netstate diff exited non-zero: {stderr}"));
    }
    Ok(parse_interfaces(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `netstate diff --json` array.
#[must_use]
pub fn parse_interfaces(raw: &str) -> Vec<InterfaceRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    top.into_iter()
        .map(|p| {
            let os = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::to_string);
            InterfaceRow {
                name: p
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                managed: p
                    .get("managed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                desired_state: os("desired_state"),
                desired_ipv4: os("desired_ipv4"),
                actual_state: os("actual_state"),
                actual_ipv4: os("actual_ipv4"),
                in_sync: p.get("in_sync").and_then(serde_json::Value::as_bool),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interfaces_reads_the_diff_shape() {
        let raw = r#"[
            {"name":"nebula1","managed":true,"desired_state":"up","desired_ipv4":"10.42.0.7/24",
             "actual_state":"up","actual_ipv4":"10.42.0.7/24","in_sync":true},
            {"name":"eth0","managed":true,"desired_state":"up","desired_ipv4":"dhcp",
             "actual_state":"down","actual_ipv4":"—","in_sync":false},
            {"name":"lo","managed":false,"desired_state":null,"desired_ipv4":null,
             "actual_state":"up","actual_ipv4":"127.0.0.1/8","in_sync":null}
        ]"#;
        let rows = parse_interfaces(raw);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].managed && rows[0].in_sync == Some(true));
        assert_eq!(rows[1].in_sync, Some(false)); // drift
        assert!(!rows[2].managed && rows[2].in_sync.is_none()); // unmanaged
        assert_eq!(rows[0].actual_ipv4.as_deref(), Some("10.42.0.7/24"));
    }

    #[test]
    fn parse_interfaces_returns_empty_for_garbage() {
        assert!(parse_interfaces("not json").is_empty());
        assert!(parse_interfaces("").is_empty());
    }

    #[test]
    fn view_renders_rows_and_empty_without_panic() {
        let mut p = InterfacesPanel::new();
        p.rows = parse_interfaces(
            r#"[{"name":"nebula1","managed":true,"desired_state":"up","desired_ipv4":"10.42.0.7/24",
                "actual_state":"down","actual_ipv4":"—","in_sync":false}]"#,
        );
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
        let mut empty = InterfacesPanel::new();
        empty.last_run_at = Some(SystemTime::now());
        let _ = empty.view();
        empty.error = Some("nmstatectl missing".into());
        let _ = empty.view();
    }
}
