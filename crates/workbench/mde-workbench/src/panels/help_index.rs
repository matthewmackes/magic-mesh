//! v4.0.1 WB-2.c — Help group root: topics list.
//!
//! Routes to when the operator clicks the "Help" sidebar group
//! header. Reads `/usr/share/mde/help/*.md` (RPM-installed) or
//! `docs/help/*.md` (repo) and renders one clickable row per
//! topic. Click opens the .md file via `xdg-open`.
//!
//! Chrome influence (per Phase 0.8): Win11 Settings → Help &
//! Support topic list.

use std::path::{Path, PathBuf};

use cosmic::iced::widget::{button, column, container, row, text, Space};
use cosmic::iced::{Background, Border, Color, Element, Length, Padding};
use cosmic::Theme;
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

#[derive(Debug, Clone, Default)]
pub struct HelpIndexPanel {
    topics: Vec<HelpTopic>,
}

#[derive(Debug, Clone)]
pub struct HelpTopic {
    pub path: PathBuf,
    pub title: String,
}

impl HelpIndexPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            topics: discover_topics(),
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, Theme> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();
        let title = text("Help")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let body = if self.topics.is_empty() {
            text(
                "No help topics shipped with this install. The mde RPM \
                 normally installs Markdown topics under \
                 /usr/share/mde/help/. Open `docs/help/` in the source \
                 tree for the upstream copies.",
            )
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color())
        } else {
            text(format!("{} topic(s) available.", self.topics.len()))
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
        };
        let mut col = column![
            title,
            Space::new().height(Length::Fixed(6.0)),
            body,
            Space::new().height(Length::Fixed(20.0)),
        ];
        for topic in &self.topics {
            col = col.push(topic_row(topic, palette));
        }
        container(col.spacing(0))
            .padding(Padding::from([24u16, 32u16]))
            .width(Length::Fill)
            .into()
    }
}

fn topic_row<'a>(topic: &'a HelpTopic, palette: Palette) -> Element<'a, crate::Message, Theme> {
    let resolved = mde_icon(Icon::Help, IconSize::Nav);
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
    let title_text = text(topic.title.clone())
        .size(14)
        .colr(palette.text.into_cosmic_color());
    let path_text = text(topic.path.display().to_string())
        .size(10)
        .colr(palette.text_muted.into_cosmic_color());
    let inner = row![
        icon_widget,
        Space::new().width(Length::Fixed(12.0)),
        column![title_text, path_text].spacing(2),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    let path = topic.path.clone();
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    let muted_text = palette.text_muted.into_cosmic_color();
    button(inner)
        .width(Length::Fill)
        .padding(Padding::from([12u16, 16u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                let hover_bg = Color {
                    r: bg.r * 1.08,
                    g: bg.g * 1.08,
                    b: bg.b * 1.08,
                    a: bg.a,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(match status {
                        cosmic::iced::widget::button::Status::Hovered => hover_bg,
                        _ => bg,
                    })),
                    text_color: muted_text,
                    icon_color: None,
                    border_color: border,
                    border_width: 1.0,
                    border_radius: 6.0.into(),
                    border: Border {
                        color: border,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                }
            },
        )
        .on_press(crate::Message::HelpTopicOpened(path))
        .into()
}

/// Walks the RPM-installed help dir + the repo's `docs/help/`
/// dir, returning one `HelpTopic` per `.md` file. Title is the
/// stem with `-` → ` ` + title-case capitalisation.
fn discover_topics() -> Vec<HelpTopic> {
    let mut out = Vec::new();
    for dir in [
        Path::new("/usr/share/mde/help"),
        Path::new("/usr/share/mde/help"),
        Path::new("docs/help"),
    ] {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("(untitled)")
                .replace('-', " ");
            // Tiny title-case: capitalise first letter of each word.
            let title = stem
                .split_whitespace()
                .map(|w| {
                    let mut chars = w.chars();
                    match chars.next() {
                        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            out.push(HelpTopic { path, title });
        }
        if !out.is_empty() {
            break; // first dir that yielded topics wins
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

/// Spawn `xdg-open <path>` detached. Fired when the operator
/// clicks a topic row.
pub fn spawn_xdg_open(path: &Path) {
    let _ = std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_renders_without_panic_empty() {
        let panel = HelpIndexPanel { topics: vec![] };
        let _ = panel.view();
    }

    #[test]
    fn view_renders_without_panic_populated() {
        let panel = HelpIndexPanel {
            topics: vec![HelpTopic {
                path: PathBuf::from("/usr/share/mde/help/getting-started.md"),
                title: "Getting Started".into(),
            }],
        };
        let _ = panel.view();
    }
}
