//! PLANES-24 — Provisioning ▸ Mirrors panel.
//!
//! The package-mirror catalog (W61/W62/W63): each mirror pulls the
//! magic-mesh GitHub-RPM channel into LizardFS, so every node serves
//! itself via a `file://` baseurl with the upstream as fallback. This
//! panel shells `mackesd mirrors --json` and renders each mirror's
//! upstream, the `file://` baseurl it serves, and how fresh its last
//! sync is.
//!
//! Read-only renderer (W88): mirrors are TOML configs + a scheduled
//! one-puller sync job; this surface only shows them.

use std::time::{SystemTime, UNIX_EPOCH};

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::Task;
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// One mirror, parsed from `mackesd mirrors --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorRow {
    pub name: String,
    pub description: String,
    pub upstream: String,
    pub file_baseurl: String,
    pub enabled: bool,
    pub last_sync_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct MirrorsPanel {
    pub rows: Vec<MirrorRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<MirrorRow>, String>),
    RefreshClicked,
}

impl MirrorsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_mirrors() }, |result| {
            crate::Message::Mirrors(Message::Loaded(result))
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

        let title = text("Mirrors")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle_text = if self.last_run_at.is_some() {
            format!(
                "{} mirror{} — every node serves itself via file://",
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
                    background: Some(Background::Color(bg)),
                    text_color: Color::WHITE,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            },
        )
        .on_press(crate::Message::Mirrors(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(mirror_row(r, palette));
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

fn mirror_row<'a>(r: &'a MirrorRow, palette: Palette) -> Element<'a, crate::Message> {
    let accent = palette.accent.into_cosmic_color();
    let resolved = mde_icon(Icon::Update, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        let svg_widget: cosmic::iced::widget::Svg<'a, Theme> =
            widget_svg(widget_svg::Handle::from_memory(b))
                .width(Length::Fixed(16.0))
                .height(Length::Fixed(16.0));
        svg_widget
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(accent),
            })
            .into()
    } else {
        text(resolved.fallback_glyph).size(16.0).colr(accent).into()
    };

    let (sync_color, sync_text) = match r.last_sync_ms {
        Some(ms) => (palette.success.into_cosmic_color(), fmt_age_ms(ms)),
        None => (
            palette.warning.into_cosmic_color(),
            "never synced".to_string(),
        ),
    };
    let (state_color, state_text) = if r.enabled {
        (palette.success.into_cosmic_color(), "enabled")
    } else {
        (palette.text_muted.into_cosmic_color(), "disabled")
    };

    let head = row![
        icon_widget,
        text(r.name.clone())
            .size(12)
            .colr(palette.text.into_cosmic_color()),
        text(state_text).size(10).colr(state_color),
        Space::new().width(Length::Fill),
        text(sync_text).size(10).colr(sync_color),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let desc = text(r.description.clone())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());
    let upstream = text(format!("upstream: {}", r.upstream))
        .size(10)
        .colr(palette.text_muted.into_cosmic_color());
    let serves = text(format!("serves: {}", r.file_baseurl))
        .size(10)
        .colr(palette.accent.into_cosmic_color());

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(column![head, desc, upstream, serves].spacing(3))
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .sty(move |_t: &Theme| container::Style {
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

fn fmt_age_ms(ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ms);
    let secs = now.saturating_sub(ms) / 1000;
    if secs < 60 {
        format!("synced {secs}s ago")
    } else if secs < 3600 {
        format!("synced {}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("synced {}h ago", secs / 3600)
    } else {
        format!("synced {}d ago", secs / 86_400)
    }
}

fn empty_state_card<'a>(palette: Palette, error: Option<&'a str>) -> Element<'a, crate::Message> {
    let (icon_color, heading, body): (Color, String, String) = if let Some(err) = error {
        (
            palette.danger.into_cosmic_color(),
            "Couldn't read mirrors".to_string(),
            err.to_string(),
        )
    } else {
        (
            palette.accent.into_cosmic_color(),
            "No mirrors".to_string(),
            "The core pack ships the magic-mesh GitHub-RPM mirror. Drop a TOML mirror under \
             the workgroup's mirrors/ dir to add more."
                .to_string(),
        )
    };
    let icon_kind = if error.is_some() {
        Icon::StatusError
    } else {
        Icon::Update
    };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        let svg_widget: cosmic::iced::widget::Svg<'a, Theme> =
            widget_svg(widget_svg::Handle::from_memory(b))
                .width(Length::Fixed(32.0))
                .height(Length::Fixed(32.0));
        svg_widget
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

/// Shell out to `mackesd mirrors --json` and parse the catalog.
pub fn fetch_mirrors() -> Result<Vec<MirrorRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["mirrors", "--json"])
        .output()
        .map_err(|e| format!("mackesd mirrors failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd mirrors exited non-zero: {stderr}"));
    }
    Ok(parse_mirrors(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `mirrors --json` array.
#[must_use]
pub fn parse_mirrors(raw: &str) -> Vec<MirrorRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    let s = |v: &serde_json::Value, k: &str| {
        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    top.into_iter()
        .filter_map(|m| {
            let name = m.get("name").and_then(|v| v.as_str())?.to_string();
            Some(MirrorRow {
                name,
                description: s(&m, "description"),
                upstream: s(&m, "upstream"),
                file_baseurl: s(&m, "file_baseurl"),
                enabled: m
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                last_sync_ms: m.get("last_sync_ms").and_then(serde_json::Value::as_u64),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mirrors_reads_the_catalog_shape() {
        let raw = r#"[
            {"name":"magic-mesh","description":"d","upstream":"https://up/repo/",
             "file_baseurl":"file:///wg/mirrors/magic-mesh","enabled":true,"last_sync_ms":null}
        ]"#;
        let rows = parse_mirrors(raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "magic-mesh");
        assert_eq!(rows[0].file_baseurl, "file:///wg/mirrors/magic-mesh");
        assert!(rows[0].enabled);
        assert!(rows[0].last_sync_ms.is_none());
    }

    #[test]
    fn parse_mirrors_reads_a_synced_disabled_mirror() {
        let raw = r#"[{"name":"x","description":"","upstream":"u","file_baseurl":"f",
            "enabled":false,"last_sync_ms":1700000000000}]"#;
        let rows = parse_mirrors(raw);
        assert!(!rows[0].enabled);
        assert_eq!(rows[0].last_sync_ms, Some(1_700_000_000_000));
    }

    #[test]
    fn parse_mirrors_returns_empty_for_garbage() {
        assert!(parse_mirrors("not json").is_empty());
        assert!(parse_mirrors("").is_empty());
    }

    #[test]
    fn view_renders_rows_and_empty_without_panic() {
        let mut p = MirrorsPanel::new();
        p.rows = parse_mirrors(
            r#"[{"name":"magic-mesh","description":"d","upstream":"u","file_baseurl":"f",
                "enabled":true,"last_sync_ms":null}]"#,
        );
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
        let mut empty = MirrorsPanel::new();
        empty.last_run_at = Some(SystemTime::now());
        let _ = empty.view();
        empty.error = Some("down".into());
        let _ = empty.view();
    }
}
