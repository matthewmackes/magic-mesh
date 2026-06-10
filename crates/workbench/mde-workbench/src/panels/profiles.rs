//! PLANES-21 — Provisioning ▸ Install Profiles panel.
//!
//! An install profile is a named deployment template (W56): a role pin,
//! capability tags, the kickstart `%post` fragments it injects, and
//! whether the firstboot auto-join slot is baked in (W60). One image
//! carries every profile; the boot menu picks one at install (W57). This
//! panel shells `mackesd profiles --json` and renders the catalog (the
//! shipped per-role core pack + any TOML profiles on LizardFS).
//!
//! Read-only renderer (W88): profiles are authored as TOML; the write
//! side (form edit) + the actual image bake (PLANES-22) build on this.

use std::time::SystemTime;

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

/// One install profile, parsed from `mackesd profiles --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    pub name: String,
    pub description: String,
    pub role: String,
    pub tags: Vec<String>,
    pub ks_fragments: Vec<String>,
    pub auto_join: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ProfilesPanel {
    pub rows: Vec<ProfileRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<ProfileRow>, String>),
    RefreshClicked,
}

impl ProfilesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_profiles() }, |result| {
            crate::Message::Profiles(Message::Loaded(result))
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

        let title = text("Install Profiles")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());
        let subtitle_text = if self.last_run_at.is_some() {
            format!(
                "{} profile{} — one image carries them all (boot-menu choice at install)",
                self.rows.len(),
                if self.rows.len() == 1 { "" } else { "s" }
            )
        } else {
            "click Refresh to load".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let accent = palette.accent.into_iced_color();
        let refresh_btn = button(
            text(if self.busy { "Loading…" } else { "Refresh" })
                .size(13)
                .color(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .style(move |_t: &Theme, status: iced::widget::button::Status| {
            let bg = match status {
                iced::widget::button::Status::Hovered => Color {
                    r: accent.r * 1.10,
                    g: accent.g * 1.10,
                    b: accent.b * 1.10,
                    a: accent.a,
                },
                _ => accent,
            };
            iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: Color::WHITE,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 6.0.into(),
                },
                shadow: iced::Shadow::default(),
            }
        })
        .on_press(crate::Message::Profiles(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(profile_row(r, palette));
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

fn profile_row<'a>(r: &'a ProfileRow, palette: Palette) -> Element<'a, crate::Message> {
    let accent = palette.accent.into_iced_color();
    let resolved = mde_icon(Icon::Update, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(accent),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .color(accent)
            .into()
    };

    let head = row![
        icon_widget,
        text(r.name.clone())
            .size(12)
            .color(palette.text.into_iced_color()),
        text(format!("role: {}", r.role))
            .size(10)
            .color(palette.accent.into_iced_color()),
        Space::new().width(Length::Fill),
        text(if r.auto_join {
            "auto-join"
        } else {
            "manual enroll"
        })
        .size(10)
        .color(if r.auto_join {
            palette.success.into_iced_color()
        } else {
            palette.text_muted.into_iced_color()
        }),
    ]
    .spacing(8)
    .align_y(iced::alignment::Vertical::Center);

    let desc = text(r.description.clone())
        .size(11)
        .color(palette.text_muted.into_iced_color());
    let tags_line = text(format!(
        "tags: {} · ks: {}",
        if r.tags.is_empty() {
            "—".to_string()
        } else {
            r.tags.join(", ")
        },
        if r.ks_fragments.is_empty() {
            "—".to_string()
        } else {
            r.ks_fragments.join(", ")
        }
    ))
    .size(10)
    .color(palette.text_muted.into_iced_color());

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(column![head, desc, tags_line].spacing(3))
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
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
    let (icon_color, heading, body): (Color, String, String) = if let Some(err) = error {
        (
            palette.danger.into_iced_color(),
            "Couldn't read profiles".to_string(),
            err.to_string(),
        )
    } else {
        (
            palette.accent.into_iced_color(),
            "No install profiles".to_string(),
            "The core pack ships one profile per role (lighthouse / server / workstation). \
             Drop a TOML profile under the workgroup's profiles/ dir to add more."
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
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(icon_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(32.0)
            .color(icon_color)
            .into()
    };
    container(
        column![
            icon_widget,
            Space::new().height(Length::Fixed(8.0)),
            text(heading).size(14).color(palette.text.into_iced_color()),
            text(body)
                .size(11)
                .color(palette.text_muted.into_iced_color()),
        ]
        .spacing(2)
        .align_x(iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

// ---- I/O ------------------------------------------------------

/// Shell out to `mackesd profiles --json` and parse the catalog.
pub fn fetch_profiles() -> Result<Vec<ProfileRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["profiles", "--json"])
        .output()
        .map_err(|e| format!("mackesd profiles failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd profiles exited non-zero: {stderr}"));
    }
    Ok(parse_profiles(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `profiles --json` array.
#[must_use]
pub fn parse_profiles(raw: &str) -> Vec<ProfileRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    let str_vec = |v: Option<&serde_json::Value>| -> Vec<String> {
        v.and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    top.into_iter()
        .filter_map(|p| {
            let name = p.get("name").and_then(|v| v.as_str())?.to_string();
            Some(ProfileRow {
                name,
                description: p
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                role: p
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                tags: str_vec(p.get("tags")),
                ks_fragments: str_vec(p.get("ks_fragments")),
                auto_join: p
                    .get("auto_join")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_profiles_reads_the_catalog_shape() {
        let raw = r#"[
            {"name":"server","description":"d","role":"server",
             "tags":["execution","headless"],"ks_fragments":["role-server"],"auto_join":true}
        ]"#;
        let rows = parse_profiles(raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].role, "server");
        assert_eq!(
            rows[0].tags,
            vec!["execution".to_string(), "headless".to_string()]
        );
        assert!(rows[0].auto_join);
    }

    #[test]
    fn parse_profiles_returns_empty_for_garbage() {
        assert!(parse_profiles("not json").is_empty());
        assert!(parse_profiles("").is_empty());
    }

    #[test]
    fn view_renders_rows_and_empty_without_panic() {
        let mut p = ProfilesPanel::new();
        p.rows = parse_profiles(
            r#"[{"name":"workstation","description":"d","role":"workstation",
                "tags":["execution"],"ks_fragments":["cosmic"],"auto_join":true}]"#,
        );
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
        let mut empty = ProfilesPanel::new();
        empty.last_run_at = Some(SystemTime::now());
        let _ = empty.view();
        empty.error = Some("mackesd down".into());
        let _ = empty.view();
    }
}
