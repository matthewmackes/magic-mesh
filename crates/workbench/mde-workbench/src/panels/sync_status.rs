//! v4.0.1 — panel.toml sync-status surface (Look & Feel →
//! Panel Sync Status).
//!
//! Reads two sources to render a one-card view of the panel
//! TOML's mesh-sync state:
//!   * `~/.config/mde/panel.toml` mtime + size (proof the file
//!     exists locally, when it last changed).
//!   * `mackesd healthz` JSON (parsed for `node_id` /
//!     `revision` / `drift_count` when present; until mackesd
//!     populates those fields the panel honestly says
//!     "not collected yet").
//!
//! Closes the worklist item at line ≈2723 (v4.0.1 panel.toml
//! sync-status). Mirrors the Win11 Settings → Sync your
//! settings status surface.

use std::path::PathBuf;
use std::time::SystemTime;

use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

#[derive(Debug, Clone, Default)]
pub struct SyncSnapshot {
    /// `panel.toml` absolute path (None when $HOME unset).
    pub panel_toml_path: Option<PathBuf>,
    /// True when the file exists at the resolved path.
    pub file_exists: bool,
    /// Bytes on disk (0 when missing).
    pub size_bytes: u64,
    /// mtime of the file (None when missing).
    pub mtime: Option<SystemTime>,
    /// Parsed `node_id` from `mackesd healthz`. Empty when not
    /// populated yet.
    pub healthz_node: String,
    /// Parsed `revision` from healthz (best-effort). Empty when
    /// the field isn't in the JSON.
    pub healthz_revision: String,
    /// Parsed `drift_count` from healthz. None when not present.
    pub drift_count: Option<u64>,
    /// Raw healthz JSON for the "see full" disclosure.
    pub healthz_raw: String,
}

#[derive(Debug, Clone, Default)]
pub struct SyncStatusPanel {
    pub snapshot: SyncSnapshot,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(SyncSnapshot),
    RefreshClicked,
}

impl SyncStatusPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { probe() }, |snap| {
            crate::Message::SyncStatus(Message::Loaded(snap))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(snap) => {
                self.snapshot = snap;
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
        let palette = Palette::dark();
        let sizes = FontSize::defaults();

        let title = text("Panel Sync Status")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());
        let subtitle = text(if let Some(t) = self.last_run_at {
            format!("last probe {}", fmt_age(t))
        } else {
            "click Refresh to probe".into()
        })
        .size(TypeRole::Body.size_in(sizes))
        .color(palette.text_muted.into_iced_color());

        let refresh_btn = button(
            text(if self.busy { "Probing…" } else { "Refresh" })
                .size(13)
                .color(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .style({
            let accent = palette.accent.into_iced_color();
            move |_t: &Theme, status: iced::widget::button::Status| {
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
            }
        })
        .on_press(crate::Message::SyncStatus(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let local_card = file_status_card(&self.snapshot, palette);
        let healthz_card = healthz_status_card(&self.snapshot, palette);

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                local_card,
                Space::new().height(Length::Fixed(12.0)),
                healthz_card,
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn file_status_card<'a>(snap: &'a SyncSnapshot, palette: Palette) -> Element<'a, crate::Message> {
    let (status_icon, status_color, status_label) = if snap.file_exists {
        (Icon::StatusOk, palette.success.into_iced_color(), "PRESENT")
    } else {
        (
            Icon::StatusWarning,
            palette.warning.into_iced_color(),
            "ABSENT",
        )
    };
    let resolved = mde_icon(status_icon, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(28.0))
            .height(Length::Fixed(28.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(status_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(28.0)
            .color(status_color)
            .into()
    };
    let path_text = text(
        snap.panel_toml_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no $HOME)".into()),
    )
    .size(12)
    .color(palette.text_muted.into_iced_color());
    let detail_text = text(if snap.file_exists {
        format!(
            "{} bytes · changed {}",
            snap.size_bytes,
            snap.mtime.map(fmt_age).unwrap_or_else(|| "—".into())
        )
    } else {
        "no local panel.toml yet — Workbench Apps→Panel Apps writes one on first save".to_string()
    })
    .size(11)
    .color(palette.text_muted.into_iced_color());

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(
        row![
            icon_widget,
            column![
                row![
                    text("Local panel.toml")
                        .size(13)
                        .color(palette.text.into_iced_color()),
                    text(status_label).size(11).color(status_color),
                ]
                .spacing(10)
                .align_y(iced::alignment::Vertical::Center),
                path_text,
                detail_text,
            ]
            .spacing(3),
        ]
        .spacing(12)
        .align_y(iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([14u16, 18u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn healthz_status_card<'a>(
    snap: &'a SyncSnapshot,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let (status_icon, status_color, summary): (Icon, Color, String) =
        if !snap.healthz_node.is_empty()
            && (!snap.healthz_revision.is_empty() || snap.drift_count.is_some())
        {
            let rev = if snap.healthz_revision.is_empty() {
                "(no revision in healthz)".into()
            } else {
                snap.healthz_revision.clone()
            };
            let drift = snap
                .drift_count
                .map(|n| format!(" · drift={n}"))
                .unwrap_or_default();
            (
                Icon::StatusOk,
                palette.success.into_iced_color(),
                format!("synced to revision {rev} on {}{drift}", snap.healthz_node),
            )
        } else if !snap.healthz_raw.is_empty() {
            (
                Icon::StatusWarning,
                palette.warning.into_iced_color(),
                "mackesd healthz returned data but no revision/drift fields populated yet".into(),
            )
        } else {
            (
                Icon::StatusUnknown,
                palette.text_muted.into_iced_color(),
                "mackesd healthz not reachable — is the daemon installed?".into(),
            )
        };
    let resolved = mde_icon(status_icon, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(18.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(status_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(18.0)
            .color(status_color)
            .into()
    };

    let body_text = if snap.healthz_raw.trim().is_empty() {
        "no JSON body".to_string()
    } else {
        snap.healthz_raw.clone()
    };

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    let raw_bg = Color {
        r: 0.06,
        g: 0.06,
        b: 0.07,
        a: 1.0,
    };
    container(
        column![
            row![
                icon_widget,
                text("Mesh sync state")
                    .size(13)
                    .color(palette.text.into_iced_color()),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
            text(summary)
                .size(12)
                .color(palette.text_muted.into_iced_color()),
            container(
                text(body_text)
                    .size(10)
                    .color(palette.text_muted.into_iced_color())
            )
            .padding(Padding::from([8u16, 12u16]))
            .width(Length::Fill)
            .style(move |_| container::Style {
                snap: false,
                background: Some(Background::Color(raw_bg)),
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            }),
        ]
        .spacing(8),
    )
    .padding(Padding::from([14u16, 18u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

// ---- I/O ------------------------------------------------------

#[must_use]
pub fn probe() -> SyncSnapshot {
    let panel_toml_path = panel_toml_path();
    let (file_exists, size_bytes, mtime) = panel_toml_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| (true, m.len(), m.modified().ok()))
        .unwrap_or((false, 0, None));
    let healthz_raw = run_mackesd_healthz();
    let (healthz_node, healthz_revision, drift_count) = parse_healthz(&healthz_raw);
    SyncSnapshot {
        panel_toml_path,
        file_exists,
        size_bytes,
        mtime,
        healthz_node,
        healthz_revision,
        drift_count,
        healthz_raw,
    }
}

fn panel_toml_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("mde").join("panel.toml"))
}

fn run_mackesd_healthz() -> String {
    let out = std::process::Command::new("mackesd")
        .arg("healthz")
        .output();
    let Ok(out) = out else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Pure parser — extracts `(node_id, revision, drift_count)`
/// from `mackesd healthz` JSON. Missing fields return empty
/// strings / None.
#[must_use]
pub fn parse_healthz(raw: &str) -> (String, String, Option<u64>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (String::new(), String::new(), None);
    };
    let node = v
        .get("node_id")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let rev = v
        .get("revision")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("config_version").and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string();
    let drift = v
        .get("drift_count")
        .and_then(|x| x.as_u64())
        .or_else(|| v.get("drift").and_then(|x| x.as_u64()));
    (node, rev, drift)
}

fn fmt_age(t: SystemTime) -> String {
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs} s ago")
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_healthz_extracts_known_shape() {
        let raw = r#"{"node_id":"peer:anvil","revision":"r42","drift_count":3}"#;
        let (node, rev, drift) = parse_healthz(raw);
        assert_eq!(node, "peer:anvil");
        assert_eq!(rev, "r42");
        assert_eq!(drift, Some(3));
    }

    #[test]
    fn parse_healthz_falls_back_to_config_version() {
        let raw = r#"{"node_id":"peer:anvil","config_version":"v9"}"#;
        let (_, rev, _) = parse_healthz(raw);
        assert_eq!(rev, "v9");
    }

    #[test]
    fn parse_healthz_returns_empties_for_garbage() {
        let (n, r, d) = parse_healthz("not json");
        assert!(n.is_empty());
        assert!(r.is_empty());
        assert!(d.is_none());
    }

    #[test]
    fn view_renders_empty_without_panic() {
        let p = SyncStatusPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_loaded_data_without_panic() {
        let mut p = SyncStatusPanel::new();
        p.snapshot = SyncSnapshot {
            panel_toml_path: Some(PathBuf::from("/tmp/panel.toml")),
            file_exists: true,
            size_bytes: 256,
            mtime: Some(SystemTime::now()),
            healthz_node: "peer:anvil".into(),
            healthz_revision: "r42".into(),
            drift_count: Some(0),
            healthz_raw: r#"{"node_id":"peer:anvil","revision":"r42"}"#.into(),
        };
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
    }
}
