//! v4.0.1 WB-2.g — Maintain → Drift panel.
//!
//! Surfaces config drift surfaced by mackesd's reconciler.
//! Reads the SQLite audit-event chain via `mackesd events list
//! --json`, filters for the drift-flavoured payloads, and
//! renders each as a row with severity icon + peer + message.
//!
//! Empty-state messaging: "no drift detected" + a hint that
//! mackesd needs to be running for new events to land.
//!
//! Chrome influence: Win11 Event Viewer-as-applet — severity
//! pill + time + per-event detail.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftSeverity {
    Info,
    Warn,
    Error,
}

impl DriftSeverity {
    fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "warn" | "warning" => Self::Warn,
            "error" | "err" | "fatal" => Self::Error,
            _ => Self::Info,
        }
    }
    fn icon(self) -> Icon {
        match self {
            Self::Info => Icon::StatusOk,
            Self::Warn => Icon::StatusWarning,
            Self::Error => Icon::StatusError,
        }
    }
    fn color(self) -> Color {
        let palette = Palette::dark();
        match self {
            Self::Info => palette.accent.into_iced_color(),
            Self::Warn => palette.warning.into_iced_color(),
            Self::Error => palette.danger.into_iced_color(),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftRow {
    pub event_id: u64,
    pub timestamp_ms: i64,
    pub peer: String,
    pub severity: DriftSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct DriftPanel {
    pub rows: Vec<DriftRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<DriftRow>, String>),
    RefreshClicked,
}

impl DriftPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_drift_events() }, |result| {
            crate::Message::Drift(Message::Loaded(result))
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
        let palette = Palette::dark();
        let sizes = FontSize::defaults();

        let title = text("Drift")
            .size(TypeRole::Display.size_in(sizes))
            .color(palette.text.into_iced_color());

        let subtitle_text = if let Some(t) = self.last_run_at {
            format!(
                "{} event{} · last refresh {}",
                self.rows.len(),
                if self.rows.len() == 1 { "" } else { "s" },
                fmt_age(t)
            )
        } else {
            "click Refresh to load".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .color(palette.text_muted.into_iced_color());

        let refresh_btn = button(
            text(if self.busy { "Loading…" } else { "Refresh" })
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
        .on_press(crate::Message::Drift(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(drift_row(r, palette));
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

fn drift_row<'a>(r: &'a DriftRow, palette: Palette) -> Element<'a, crate::Message> {
    let resolved = mde_icon(r.severity.icon(), IconSize::Inline);
    let icon_color = r.severity.color();
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .style(
                move |_t: &Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(icon_color),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .color(icon_color)
            .into()
    };

    let head = row![
        icon_widget,
        text(r.severity.label()).size(10).color(icon_color),
        text(format!("#{}", r.event_id))
            .size(10)
            .color(palette.text_muted.into_iced_color()),
        text(r.peer.clone())
            .size(11)
            .color(palette.text.into_iced_color()),
        Space::new().width(Length::Fill),
        text(fmt_epoch_ms(r.timestamp_ms))
            .size(10)
            .color(palette.text_muted.into_iced_color()),
    ]
    .spacing(8)
    .align_y(iced::alignment::Vertical::Center);

    let body = text(r.message.clone())
        .size(12)
        .color(palette.text_muted.into_iced_color());

    let bg = palette.raised.into_iced_color();
    let border = palette.border.into_iced_color();
    container(column![head, body].spacing(4))
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
    let (icon_kind, icon_color, heading_text, body_text): (Icon, Color, String, String) =
        if let Some(err) = error {
            (
                Icon::StatusError,
                palette.danger.into_iced_color(),
                "Couldn't load drift events".to_string(),
                err.to_string(),
            )
        } else {
            (
                Icon::StatusOk,
                palette.success.into_iced_color(),
                "No drift detected".to_string(),
                "mackesd's reconciler has not surfaced any divergence between the locked TOML \
                 config and the live state. When mackesd is running, new events will appear here."
                    .to_string(),
            )
        };
    let resolved = mde_icon(icon_kind, IconSize::PanelHeader);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
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
            text(heading_text)
                .size(14)
                .color(palette.text.into_iced_color()),
            text(body_text)
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

/// Shell out to `mackesd events list --json`, parse the rows,
/// filter for the drift-flavoured ones.
pub fn fetch_drift_events() -> Result<Vec<DriftRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["events", "list", "--json"])
        .output()
        .map_err(|e| format!("mackesd events list failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd events list exited non-zero: {stderr}"));
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    Ok(parse_events(&raw))
}

/// Pure parser exposed for tests. Pulls the drift-flavoured
/// payloads out of `mackesd events list --json` output. The
/// CLI emits a JSON array; each entry is
/// `{event_id, timestamp_ms, payload (string), hash}`. The
/// payload is itself a JSON object — when its `kind` field is
/// "drift" (or its `severity` field is set) we surface it.
#[must_use]
pub fn parse_events(raw: &str) -> Vec<DriftRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in top {
        let event_id = entry.get("event_id").and_then(|v| v.as_u64()).unwrap_or(0);
        let timestamp_ms = entry
            .get("timestamp_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let payload_str = entry.get("payload").and_then(|v| v.as_str()).unwrap_or("");
        let payload: serde_json::Value =
            serde_json::from_str(payload_str).unwrap_or(serde_json::Value::Null);
        // Heuristic filter: surface anything that looks like
        // drift — a `kind` field with "drift" in it, OR a
        // `severity` field set to warn/error, OR a `peer` field
        // present alongside a `message`. Conservative; better
        // to show too much than too little until the reconciler
        // event schema settles.
        let kind = payload.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let severity_str = payload
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let peer = payload.get("peer").and_then(|v| v.as_str()).unwrap_or("");
        let message = payload
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let is_drift = kind.contains("drift") || !severity_str.is_empty();
        if !is_drift {
            continue;
        }
        out.push(DriftRow {
            event_id,
            timestamp_ms,
            peer: peer.to_string(),
            severity: DriftSeverity::from_str(severity_str),
            message,
        });
    }
    out
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
    } else {
        format!("{} h ago", secs / 3600)
    }
}

fn fmt_epoch_ms(ms: i64) -> String {
    if ms <= 0 {
        return "—".into();
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(ms);
    let delta = (now_ms - ms).max(0);
    let secs = delta / 1000;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_severity_from_str_handles_known_values() {
        assert_eq!(DriftSeverity::from_str("warn"), DriftSeverity::Warn);
        assert_eq!(DriftSeverity::from_str("WARNING"), DriftSeverity::Warn);
        assert_eq!(DriftSeverity::from_str("error"), DriftSeverity::Error);
        assert_eq!(DriftSeverity::from_str("info"), DriftSeverity::Info);
        assert_eq!(
            DriftSeverity::from_str("anything-else"),
            DriftSeverity::Info
        );
    }

    #[test]
    fn parse_events_returns_empty_for_garbage() {
        assert!(parse_events("not json").is_empty());
        assert!(parse_events("").is_empty());
    }

    #[test]
    fn parse_events_extracts_drift_kind_rows() {
        let raw = r#"[
            {"event_id": 7, "timestamp_ms": 1715000000000, "payload":
                "{\"kind\":\"drift\",\"peer\":\"pine\",\"severity\":\"warn\",\"message\":\"missed heartbeat\"}",
                "hash":"abcd"},
            {"event_id": 8, "timestamp_ms": 1715000001000, "payload":
                "{\"kind\":\"lifecycle\",\"peer\":\"oak\",\"message\":\"joined\"}",
                "hash":"efgh"}
        ]"#;
        let rows = parse_events(raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_id, 7);
        assert_eq!(rows[0].peer, "pine");
        assert_eq!(rows[0].severity, DriftSeverity::Warn);
        assert_eq!(rows[0].message, "missed heartbeat");
    }

    #[test]
    fn parse_events_also_surfaces_rows_with_severity_but_no_drift_kind() {
        let raw = r#"[
            {"event_id": 9, "timestamp_ms": 1715000002000, "payload":
                "{\"kind\":\"sync\",\"peer\":\"birch\",\"severity\":\"error\",\"message\":\"rsync failed\"}",
                "hash":"ijkl"}
        ]"#;
        let rows = parse_events(raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].severity, DriftSeverity::Error);
    }

    #[test]
    fn view_renders_empty_without_panic() {
        let p = DriftPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_rows_without_panic() {
        let mut p = DriftPanel::new();
        p.rows = vec![DriftRow {
            event_id: 7,
            timestamp_ms: 1_715_000_000_000,
            peer: "pine".into(),
            severity: DriftSeverity::Warn,
            message: "missed heartbeat".into(),
        }];
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
    }

    #[test]
    fn view_renders_error_state_without_panic() {
        let mut p = DriftPanel::new();
        p.error = Some("mackesd not running".into());
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
    }
}
