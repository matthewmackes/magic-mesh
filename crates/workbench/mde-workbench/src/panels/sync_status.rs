//! v4.0.1 — panel.toml sync-status surface (Look & Feel →
//! Panel Sync Status).
//!
//! Reads two sources to render a one-card view of the panel
//! TOML's mesh-sync state:
//!   * `~/.config/mde/panel.toml` mtime + size (proof the file
//!     exists locally, when it last changed).
//!   * `mackesd healthz` JSON (the `HealthReport` shape from
//!     `mackesd::health`: `applied_revision`, `is_leader`, and the
//!     `node_count` / `healthy_nodes` / `degraded_nodes` /
//!     `unreachable_nodes` rollup). An unreachable daemon yields an
//!     honest "not reachable", never a fake value.
//!
//! Closes the worklist item at line ≈2723 (v4.0.1 panel.toml
//! sync-status). Mirrors the Win11 Settings → Sync your
//! settings status surface.

use std::path::PathBuf;
use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, text, Space};
use cosmic::iced::{Background, Border, Color, Element, Length, Padding, Task};
use cosmic::Theme;
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

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
    /// `applied_revision` from `mackesd healthz` — the most recent
    /// applied config revision. `None` when the store has never
    /// accepted a deploy (a fresh / follower node).
    pub applied_revision: Option<String>,
    /// `is_leader` from healthz.
    pub is_leader: bool,
    /// Mesh size from healthz (`node_count`).
    pub node_count: u64,
    /// Healthy / degraded / unreachable node counts from healthz.
    pub healthy_nodes: u64,
    pub degraded_nodes: u64,
    pub unreachable_nodes: u64,
    /// True when healthz returned a body that parsed as JSON.
    pub healthz_ok: bool,
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

    pub fn view(&self) -> Element<'_, crate::Message, Theme> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Panel Sync Status")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text(if let Some(t) = self.last_run_at {
            format!("last probe {}", fmt_age(t))
        } else {
            "click Refresh to probe".into()
        })
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

        let refresh_btn = button(
            text(if self.busy { "Probing…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty({
            let accent = palette.accent.into_cosmic_color();
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
            }
        })
        .on_press(crate::Message::SyncStatus(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

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

fn file_status_card<'a>(
    snap: &'a SyncSnapshot,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let (status_icon, status_color, status_label) = if snap.file_exists {
        (
            Icon::StatusOk,
            palette.success.into_cosmic_color(),
            "PRESENT",
        )
    } else {
        (
            Icon::StatusWarning,
            palette.warning.into_cosmic_color(),
            "ABSENT",
        )
    };
    let resolved = mde_icon(status_icon, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message, Theme> =
        if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(28.0))
                .height(Length::Fixed(28.0))
                .sty(move |_t: &Theme| widget_svg::Style {
                    color: Some(status_color),
                })
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(28.0)
                .colr(status_color)
                .into()
        };
    let path_text = text(
        snap.panel_toml_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no $HOME)".into()),
    )
    .size(12)
    .colr(palette.text_muted.into_cosmic_color());
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
    .colr(palette.text_muted.into_cosmic_color());

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(
        row![
            icon_widget,
            column![
                row![
                    text("Local panel.toml")
                        .size(13)
                        .colr(palette.text.into_cosmic_color()),
                    text(status_label).size(11).colr(status_color),
                ]
                .spacing(10)
                .align_y(cosmic::iced::alignment::Vertical::Center),
                path_text,
                detail_text,
            ]
            .spacing(3),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([14u16, 18u16]))
    .width(Length::Fill)
    .sty(move |_| container::Style {
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
) -> Element<'a, crate::Message, Theme> {
    let healthy = snap.degraded_nodes == 0 && snap.unreachable_nodes == 0;
    let (status_icon, status_color, summary): (Icon, Color, String) = if snap.healthz_ok {
        let role = if snap.is_leader { "leader" } else { "follower" };
        let rollup = format!(
            "{} node{} · {} healthy · {} degraded · {} unreachable",
            snap.node_count,
            if snap.node_count == 1 { "" } else { "s" },
            snap.healthy_nodes,
            snap.degraded_nodes,
            snap.unreachable_nodes,
        );
        match &snap.applied_revision {
            Some(rev) => (
                if healthy {
                    Icon::StatusOk
                } else {
                    Icon::StatusWarning
                },
                if healthy {
                    palette.success.into_cosmic_color()
                } else {
                    palette.warning.into_cosmic_color()
                },
                format!("applied revision {rev} · {role} · {rollup}"),
            ),
            None => (
                Icon::StatusWarning,
                palette.warning.into_cosmic_color(),
                format!("no revision applied yet · {role} · {rollup}"),
            ),
        }
    } else if !snap.healthz_raw.trim().is_empty() {
        (
            Icon::StatusWarning,
            palette.warning.into_cosmic_color(),
            "mackesd healthz returned an unparseable body".into(),
        )
    } else {
        (
            Icon::StatusUnknown,
            palette.text_muted.into_cosmic_color(),
            "mackesd healthz not reachable — is the daemon installed?".into(),
        )
    };
    let resolved = mde_icon(status_icon, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message, Theme> =
        if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(18.0))
                .height(Length::Fixed(18.0))
                .sty(move |_t: &Theme| widget_svg::Style {
                    color: Some(status_color),
                })
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(18.0)
                .colr(status_color)
                .into()
        };

    let body_text = if snap.healthz_raw.trim().is_empty() {
        "no JSON body".to_string()
    } else {
        snap.healthz_raw.clone()
    };

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    // Recessed raw-output inset: the darkest surface token (Carbon Gray 100).
    let raw_bg = palette.background.into_cosmic_color();
    container(
        column![
            row![
                icon_widget,
                text("Mesh sync state")
                    .size(13)
                    .colr(palette.text.into_cosmic_color()),
            ]
            .spacing(8)
            .align_y(cosmic::iced::alignment::Vertical::Center),
            text(summary)
                .size(12)
                .colr(palette.text_muted.into_cosmic_color()),
            container(
                text(body_text)
                    .size(10)
                    .colr(palette.text_muted.into_cosmic_color())
            )
            .padding(Padding::from([8u16, 12u16]))
            .width(Length::Fill)
            .sty(move |_| container::Style {
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
    .sty(move |_| container::Style {
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
    let hz = parse_healthz(&healthz_raw);
    SyncSnapshot {
        panel_toml_path,
        file_exists,
        size_bytes,
        mtime,
        applied_revision: hz.applied_revision,
        is_leader: hz.is_leader,
        node_count: hz.node_count,
        healthy_nodes: hz.healthy_nodes,
        degraded_nodes: hz.degraded_nodes,
        unreachable_nodes: hz.unreachable_nodes,
        healthz_ok: hz.ok,
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

/// Parsed subset of `mackesd healthz` (the `HealthReport` shape in
/// `mackesd::health`). `ok` is false when the body did not parse as
/// JSON (e.g. the daemon is absent and stdout was empty).
#[derive(Debug, Clone, Default)]
pub struct HealthzView {
    pub ok: bool,
    pub is_leader: bool,
    pub applied_revision: Option<String>,
    pub node_count: u64,
    pub healthy_nodes: u64,
    pub degraded_nodes: u64,
    pub unreachable_nodes: u64,
}

/// Pure parser — projects the `HealthReport` JSON `mackesd healthz`
/// prints into the fields the panel renders. Missing numeric fields
/// default to 0; a JSON `null` / absent / empty `applied_revision`
/// is `None`.
#[must_use]
pub fn parse_healthz(raw: &str) -> HealthzView {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return HealthzView::default();
    };
    let u64f = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    HealthzView {
        ok: true,
        is_leader: v
            .get("is_leader")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        applied_revision: v
            .get("applied_revision")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        node_count: u64f("node_count"),
        healthy_nodes: u64f("healthy_nodes"),
        degraded_nodes: u64f("degraded_nodes"),
        unreachable_nodes: u64f("unreachable_nodes"),
    }
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
    fn parse_healthz_extracts_health_report_shape() {
        let raw = r#"{"schema":1,"is_leader":true,"applied_revision":"r-2026-06-29-0007","node_count":3,"healthy_nodes":3,"degraded_nodes":0,"unreachable_nodes":0}"#;
        let hz = parse_healthz(raw);
        assert!(hz.ok);
        assert!(hz.is_leader);
        assert_eq!(hz.applied_revision.as_deref(), Some("r-2026-06-29-0007"));
        assert_eq!(hz.node_count, 3);
        assert_eq!(hz.healthy_nodes, 3);
    }

    #[test]
    fn parse_healthz_null_applied_revision_is_none() {
        // The live follower shape from the operator's screenshot:
        // applied_revision is JSON null, the rollup is still present.
        let raw = r#"{"schema":1,"is_leader":false,"applied_revision":null,"node_count":3,"healthy_nodes":3,"degraded_nodes":0,"unreachable_nodes":0}"#;
        let hz = parse_healthz(raw);
        assert!(hz.ok);
        assert!(!hz.is_leader);
        assert!(hz.applied_revision.is_none());
        assert_eq!(hz.node_count, 3);
    }

    #[test]
    fn parse_healthz_marks_not_ok_for_garbage() {
        let hz = parse_healthz("not json");
        assert!(!hz.ok);
        assert!(hz.applied_revision.is_none());
        assert_eq!(hz.node_count, 0);
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
            applied_revision: Some("r-2026-06-29-0007".into()),
            is_leader: false,
            node_count: 3,
            healthy_nodes: 3,
            degraded_nodes: 0,
            unreachable_nodes: 0,
            healthz_ok: true,
            healthz_raw: r#"{"schema":1,"applied_revision":"r-2026-06-29-0007","node_count":3}"#
                .into(),
        };
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
    }
}
