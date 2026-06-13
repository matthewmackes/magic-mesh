//! PLANES-3 / W82 — Fleet ▸ Capability Tags panel.
//!
//! The fleet-wide capability-tag census: the v1 vocabulary is **hop /
//! execution / headless** (W82), and tags GATE duty — an untagged node
//! is refused the work the tag authorizes (W84). This panel shells
//! `mackesd tags --json` and shows, for each tag, the roster nodes that
//! carry it, so the operator can answer "who can run jobs?" / "who's a
//! hop?" at a glance. The per-node view/edit lives in Provisioning ▸
//! Node Roles (the `mackesd tag <host>` write side, W26/W58).
//!
//! Read-only renderer (W88): tags are replicated TOML/JSON on LizardFS;
//! this surface only shows the census.

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::Task;
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::Theme;
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// Elements in this panel thread libcosmic's `cosmic::Theme` (the cosmic_compat
/// style-closure traits resolve against it), so the alias pins that theme param.
type Element<'a, M> = cosmic::iced::Element<'a, M, Theme>;

/// One capability tag and the nodes carrying it, parsed from
/// `mackesd tags --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRow {
    pub tag: String,
    pub nodes: Vec<String>,
}

impl TagRow {
    /// One-line description of what carrying the tag authorizes (W82).
    fn purpose(&self) -> &'static str {
        match self.tag.as_str() {
            "hop" => "advertises subnets / serves as an overlay relay + exit node",
            "execution" => "runs fleet jobs + image builds locally",
            "headless" => "GUI units off; full agent, no desktop",
            _ => "capability gate",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TagsPanel {
    pub rows: Vec<TagRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<TagRow>, String>),
    RefreshClicked,
}

impl TagsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_tags() }, |result| {
            crate::Message::Tags(Message::Loaded(result))
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

        let title = text("Capability Tags")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let tagged: usize = self.rows.iter().map(|r| r.nodes.len()).sum();
        let subtitle_text = if self.last_run_at.is_some() {
            format!("{} tags · {tagged} node assignment(s)", self.rows.len())
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
        .on_press(crate::Message::Tags(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(tag_row(r, palette));
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

fn tag_row<'a>(r: &'a TagRow, palette: Palette) -> Element<'a, crate::Message> {
    let has_nodes = !r.nodes.is_empty();
    let accent = palette.accent.into_cosmic_color();
    let resolved = mde_icon(Icon::Fleet, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(accent),
            })
            .into()
    } else {
        text(resolved.fallback_glyph).size(16.0).colr(accent).into()
    };

    let count_color = if has_nodes {
        palette.accent.into_cosmic_color()
    } else {
        palette.text_muted.into_cosmic_color()
    };
    let head = row![
        icon_widget,
        text(r.tag.to_uppercase())
            .size(12)
            .colr(palette.text.into_cosmic_color()),
        Space::new().width(Length::Fill),
        text(format!("{} node(s)", r.nodes.len()))
            .size(11)
            .colr(count_color),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let purpose = text(r.purpose())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());
    let nodes_line = text(if has_nodes {
        r.nodes.join(", ")
    } else {
        "no nodes carry this tag yet".to_string()
    })
    .size(11)
    .colr(if has_nodes {
        palette.text.into_cosmic_color()
    } else {
        palette.text_muted.into_cosmic_color()
    });

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(column![head, purpose, nodes_line].spacing(3))
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
    let (icon_color, heading, body): (Color, String, String) = if let Some(err) = error {
        (
            palette.danger.into_cosmic_color(),
            "Couldn't read tags".to_string(),
            err.to_string(),
        )
    } else {
        (
            palette.accent.into_cosmic_color(),
            "No tag census".to_string(),
            "The v1 capability tags are hop, execution, and headless. Assign them per node \
             in Provisioning ▸ Node Roles; this census then shows which nodes carry each."
                .to_string(),
        )
    };
    let icon_kind = if error.is_some() {
        Icon::StatusError
    } else {
        Icon::Fleet
    };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
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

/// Shell out to `mackesd tags --json` and parse the census.
pub fn fetch_tags() -> Result<Vec<TagRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["tags", "--json"])
        .output()
        .map_err(|e| format!("mackesd tags failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd tags exited non-zero: {stderr}"));
    }
    Ok(parse_tags(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `tags --json` array.
#[must_use]
pub fn parse_tags(raw: &str) -> Vec<TagRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    top.into_iter()
        .filter_map(|t| {
            let tag = t.get("tag").and_then(|v| v.as_str())?.to_string();
            let nodes = t
                .get("nodes")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Some(TagRow { tag, nodes })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tags_reads_the_census_shape() {
        let raw = r#"[
            {"tag":"hop","nodes":["birch"]},
            {"tag":"execution","nodes":["pine","oak"]},
            {"tag":"headless","nodes":[]}
        ]"#;
        let rows = parse_tags(raw);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].tag, "execution");
        assert_eq!(rows[1].nodes, vec!["pine".to_string(), "oak".to_string()]);
        assert!(rows[2].nodes.is_empty());
        assert_eq!(rows[0].purpose().is_empty(), false);
    }

    #[test]
    fn parse_tags_returns_empty_for_garbage() {
        assert!(parse_tags("not json").is_empty());
        assert!(parse_tags("").is_empty());
    }

    #[test]
    fn view_renders_rows_and_empty_without_panic() {
        let mut p = TagsPanel::new();
        p.rows = parse_tags(r#"[{"tag":"execution","nodes":["pine"]},{"tag":"hop","nodes":[]}]"#);
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
        let mut empty = TagsPanel::new();
        empty.last_run_at = Some(SystemTime::now());
        let _ = empty.view();
        empty.error = Some("mackesd down".into());
        let _ = empty.view();
    }
}
