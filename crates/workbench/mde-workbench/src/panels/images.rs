//! PLANES-22 — Provisioning ▸ Images panel.
//!
//! The image catalog (W53/W55): the mesh builds four kinds of image —
//! install ISO, VM golden, container, USB writer — each as a job on an
//! execution-tagged node (W54), landing as a versioned dir + TOML
//! manifest on LizardFS (W55). This panel shells `mackesd images --json`
//! and renders all four kinds (so the catalog shows what *can* be built)
//! each with the versioned builds present.
//!
//! Read-only renderer (W88): the actual build is a typed job on an
//! execution node; this surface only shows the catalog.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

/// One built image under a kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBuild {
    pub name: String,
    pub version: String,
    pub built_at_ms: Option<u64>,
    pub size_bytes: Option<u64>,
    pub profile: Option<String>,
}

/// One image kind + its builds, parsed from `mackesd images --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageKindRow {
    pub kind: String,
    pub label: String,
    pub description: String,
    pub builds: Vec<ImageBuild>,
}

#[derive(Debug, Clone, Default)]
pub struct ImagesPanel {
    pub rows: Vec<ImageKindRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
    /// W54 — last build-launch outcome line.
    pub build_msg: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<ImageKindRow>, String>),
    RefreshClicked,
    /// W54 — launch a build of `kind` as a job on an execution-tagged node.
    BuildClicked(String),
    BuildLaunched(String),
}

impl ImagesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_images() }, |result| {
            crate::Message::Images(Message::Loaded(result))
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
            Message::BuildClicked(kind) => {
                // W54 — launch the build playbook as a job on the
                // execution-tagged nodes; the target node runs `mackesd
                // images --build` (which records the manifest on success).
                self.build_msg = format!("launching {kind} build job…");
                let body = serde_json::json!({
                    "playbook": "playbooks/build-image.yml",
                    "targets": { "tags": ["execution"] },
                    "vars": {
                        "image_kind": kind,
                        "image_name": format!("magic-{kind}"),
                        "image_version": "dev",
                    },
                })
                .to_string();
                Task::perform(
                    async move {
                        let reply = tokio::task::spawn_blocking(move || {
                            crate::dbus::action_request_with_body(
                                "action/jobs/launch",
                                Some(&body),
                                std::time::Duration::from_secs(3),
                            )
                        })
                        .await
                        .ok()
                        .flatten();
                        match reply
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                        {
                            Some(v) if v["ok"] == true => {
                                let n = v["targets"].as_array().map_or(0, Vec::len);
                                format!("build job launched on {n} execution node(s) — Refresh for the build")
                            }
                            Some(v) => {
                                format!(
                                    "build launch failed: {}",
                                    v["error"].as_str().unwrap_or("unknown")
                                )
                            }
                            None => "build launch failed: mackesd not answering on the Bus".into(),
                        }
                    },
                    |msg| crate::Message::Images(Message::BuildLaunched(msg)),
                )
            }
            Message::BuildLaunched(msg) => {
                self.build_msg = msg;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Images")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());
        let total: usize = self.rows.iter().map(|r| r.builds.len()).sum();
        let subtitle_text = if self.last_run_at.is_some() {
            format!(
                "{} image kinds · {total} build{} — built by jobs on execution-tagged nodes",
                self.rows.len(),
                if total == 1 { "" } else { "s" }
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
        .on_press(crate::Message::Images(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(kind_row(r, palette));
        }
        if self.rows.is_empty() && self.last_run_at.is_some() {
            rows_col = rows_col.push(empty_state_card(palette, self.error.as_deref()));
        }

        let build_line: Element<'_, crate::Message> = if self.build_msg.is_empty() {
            Space::new().height(Length::Fixed(0.0)).into()
        } else {
            text(self.build_msg.clone())
                .size(12)
                .color(palette.text_muted.into_iced_color())
                .into()
        };

        container(
            column![
                header,
                Space::new().height(Length::Fixed(8.0)),
                build_line,
                Space::new().height(Length::Fixed(12.0)),
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

fn kind_row<'a>(r: &'a ImageKindRow, palette: Palette) -> Element<'a, crate::Message> {
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

    let has_builds = !r.builds.is_empty();
    let count_color = if has_builds {
        palette.success.into_iced_color()
    } else {
        palette.text_muted.into_iced_color()
    };
    let head = row![
        icon_widget,
        text(r.label.clone())
            .size(12)
            .color(palette.text.into_iced_color()),
        Space::new().width(Length::Fill),
        text(format!("{} build(s)", r.builds.len()))
            .size(11)
            .color(count_color),
        // W54 — launch a build of this kind as a job on execution nodes.
        crate::controls::variant_button(
            "Build",
            crate::controls::ButtonVariant::Secondary,
            Some(crate::Message::Images(Message::BuildClicked(
                r.kind.clone()
            ))),
            palette,
        ),
    ]
    .spacing(8)
    .align_y(iced::alignment::Vertical::Center);

    let desc = text(r.description.clone())
        .size(11)
        .color(palette.text_muted.into_iced_color());

    let mut body = column![head, desc].spacing(3);
    if has_builds {
        for b in &r.builds {
            let when = b.built_at_ms.map_or_else(|| "—".to_string(), fmt_age_ms);
            let size = b
                .size_bytes
                .map_or_else(String::new, |s| format!(" · {}", fmt_size(s)));
            body = body.push(
                text(format!("  • {} v{} ({when}{size})", b.name, b.version))
                    .size(10)
                    .color(palette.accent.into_iced_color()),
            );
        }
    } else {
        body = body.push(
            text("  no builds yet — run an image-build job on an execution node")
                .size(10)
                .color(palette.text_muted.into_iced_color()),
        );
    }

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(body)
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

fn fmt_age_ms(ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ms);
    let secs = now.saturating_sub(ms) / 1000;
    if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

fn fmt_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if bytes >= 1 << 30 {
        format!("{:.1} GiB", b / (1u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.0} MiB", b / (1u64 << 20) as f64)
    } else {
        format!("{bytes} B")
    }
}

fn empty_state_card<'a>(palette: Palette, error: Option<&'a str>) -> Element<'a, crate::Message> {
    let (icon_color, heading, body): (Color, String, String) = if let Some(err) = error {
        (
            palette.danger.into_iced_color(),
            "Couldn't read images".to_string(),
            err.to_string(),
        )
    } else {
        (
            palette.accent.into_iced_color(),
            "No image catalog".to_string(),
            "The mesh builds four kinds of image (ISO / VM / container / USB) as jobs on \
             execution-tagged nodes; built versions land on LizardFS and appear here."
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

/// Shell out to `mackesd images --json` and parse the catalog.
pub fn fetch_images() -> Result<Vec<ImageKindRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["images", "--json"])
        .output()
        .map_err(|e| format!("mackesd images failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd images exited non-zero: {stderr}"));
    }
    Ok(parse_images(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `images --json` array.
#[must_use]
pub fn parse_images(raw: &str) -> Vec<ImageKindRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    let s = |v: &serde_json::Value, k: &str| {
        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    top.into_iter()
        .filter_map(|k| {
            let kind = k.get("kind").and_then(|v| v.as_str())?.to_string();
            let builds = k
                .get("builds")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|b| {
                            Some(ImageBuild {
                                name: b.get("name")?.as_str()?.to_string(),
                                version: s(b, "version"),
                                built_at_ms: b
                                    .get("built_at_ms")
                                    .and_then(serde_json::Value::as_u64),
                                size_bytes: b.get("size_bytes").and_then(serde_json::Value::as_u64),
                                profile: b
                                    .get("profile")
                                    .and_then(|x| x.as_str())
                                    .map(str::to_string),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(ImageKindRow {
                kind,
                label: s(&k, "label"),
                description: s(&k, "description"),
                builds,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_images_reads_kinds_and_builds() {
        let raw = r#"[
            {"kind":"iso","label":"Install ISO","description":"d","builds":[
                {"name":"cosmic-iso","version":"2.0","built_at_ms":1700000000000,"size_bytes":2147483648,"profile":"workstation"}
            ]},
            {"kind":"vm","label":"VM golden image","description":"d","builds":[]}
        ]"#;
        let rows = parse_images(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, "iso");
        assert_eq!(rows[0].builds.len(), 1);
        assert_eq!(rows[0].builds[0].name, "cosmic-iso");
        assert_eq!(rows[0].builds[0].profile.as_deref(), Some("workstation"));
        assert!(rows[1].builds.is_empty());
    }

    #[test]
    fn parse_images_returns_empty_for_garbage() {
        assert!(parse_images("not json").is_empty());
        assert!(parse_images("").is_empty());
    }

    #[test]
    fn view_renders_rows_and_empty_without_panic() {
        let mut p = ImagesPanel::new();
        p.rows = parse_images(
            r#"[{"kind":"iso","label":"Install ISO","description":"d","builds":[
                {"name":"x","version":"1","built_at_ms":null,"size_bytes":null,"profile":null}]},
               {"kind":"usb","label":"USB writer","description":"d","builds":[]}]"#,
        );
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
        let mut empty = ImagesPanel::new();
        empty.last_run_at = Some(SystemTime::now());
        empty.error = Some("down".into());
        let _ = empty.view();
    }
}
