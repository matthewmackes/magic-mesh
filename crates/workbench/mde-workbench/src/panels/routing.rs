//! PLANES-19 — Network ▸ Routing panel.
//!
//! The overlay-reachability validation surface (W79/W80): the
//! validation suite probes every directed edge between participants over
//! the Nebula overlay; an edge that never came back reachable is a
//! failure that feeds the drift pipeline (W80). This panel shells
//! `mackesd validate status --json` to show the newest run's verdict and
//! `mackesd validate run` to request a fresh one (the FPG leader mints
//! it). Routing itself stays display-only (W76) — what the operator acts
//! on here is the reachability health.

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::Task;
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// One directed `from → to` edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

/// The newest validation run's verdict, parsed from
/// `mackesd validate status --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationStatus {
    /// `None` when no run has been minted yet.
    pub run_id: Option<String>,
    pub passed: bool,
    pub reachable: usize,
    pub failed_edges: Vec<Edge>,
    pub missing_reporters: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RoutingPanel {
    pub status: ValidationStatus,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
    pub run_result: Option<Result<String, String>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<ValidationStatus, String>),
    RefreshClicked,
    RunNow,
    RunRequested(Result<String, String>),
}

impl RoutingPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_status() }, |result| {
            crate::Message::Routing(Message::Loaded(result))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(status)) => {
                self.status = status;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.status = ValidationStatus::default();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                self.run_result = None;
                Self::load()
            }
            Message::RunNow => {
                self.busy = true;
                Task::perform(async { request_run() }, |result| {
                    crate::Message::Routing(Message::RunRequested(result))
                })
            }
            Message::RunRequested(result) => {
                self.run_result = Some(result);
                self.busy = false;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Routing")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text("overlay-reachability validation")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let accent = palette.accent.into_cosmic_color();
        let style_btn = move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
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
        };
        let run_btn = button(text("Run validation now").size(13).colr(Color::WHITE))
            .padding(Padding::from([6u16, 14u16]))
            .sty(style_btn)
            .on_press(crate::Message::Routing(Message::RunNow));
        let refresh_btn = button(
            text(if self.busy { "…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty(style_btn)
        .on_press(crate::Message::Routing(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            run_btn,
            Space::new().width(Length::Fixed(8.0)),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut body_col = column![].spacing(6);
        if let Some(res) = &self.run_result {
            body_col = body_col.push(result_strip(res, palette));
        }
        if self.last_run_at.is_some() {
            if self.status.run_id.is_some() {
                body_col = body_col.push(verdict_card(&self.status, palette));
                for e in &self.status.failed_edges {
                    body_col = body_col.push(failed_edge_row(e, palette));
                }
            } else {
                body_col = body_col.push(empty_state_card(palette, self.error.as_deref()));
            }
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                scrollable(body_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn verdict_card<'a>(s: &ValidationStatus, palette: Palette) -> Element<'a, crate::Message> {
    let (icon, color, label) = if s.passed {
        (
            Icon::StatusOk,
            palette.success.into_cosmic_color(),
            "PASS — every overlay edge reachable".to_string(),
        )
    } else {
        (
            Icon::StatusError,
            palette.danger.into_cosmic_color(),
            format!(
                "FAIL — {} unreachable edge{}, {} missing reporter{}",
                s.failed_edges.len(),
                if s.failed_edges.len() == 1 { "" } else { "s" },
                s.missing_reporters.len(),
                if s.missing_reporters.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        )
    };
    let resolved = mde_icon(icon, IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style { color: Some(color) })
            .into()
    } else {
        text(resolved.fallback_glyph).size(16.0).colr(color).into()
    };
    let head = row![
        icon_widget,
        text(label).size(12).colr(color),
        Space::new().width(Length::Fill),
        text(format!("{} reachable", s.reachable))
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);
    let rid = s.run_id.clone().unwrap_or_default();
    card(
        column![
            head,
            text(format!("run {rid}"))
                .size(10)
                .colr(palette.text_muted.into_cosmic_color())
        ]
        .spacing(4),
        palette,
    )
}

fn failed_edge_row<'a>(e: &Edge, palette: Palette) -> Element<'a, crate::Message> {
    let danger = palette.danger.into_cosmic_color();
    card(
        row![
            text(format!("{} → {}", e.from, e.to))
                .size(12)
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fill),
            text("unreachable").size(11).colr(danger),
        ]
        .spacing(8)
        .align_y(cosmic::iced::alignment::Vertical::Center),
        palette,
    )
}

fn result_strip<'a>(res: &Result<String, String>, palette: Palette) -> Element<'a, crate::Message> {
    let (color, label) = match res {
        Ok(msg) => (palette.success.into_cosmic_color(), msg.clone()),
        Err(e) => (palette.danger.into_cosmic_color(), format!("error — {e}")),
    };
    let bg = palette.raised.into_cosmic_color();
    container(text(label).size(11).colr(color))
        .padding(Padding::from([8u16, 14u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color,
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
                "Couldn't read validation".to_string(),
                err.to_string(),
            )
        } else {
            (
                Icon::Network,
                palette.accent.into_cosmic_color(),
                "No validation run yet".to_string(),
                "The overlay-reachability suite probes every directed edge between \
                 participants. Click \"Run validation now\" to request a run — the FPG \
                 leader mints it, every node reports what it could reach, and the verdict \
                 (with any unreachable edges) appears here."
                    .to_string(),
            )
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

fn card<'a>(
    inner: impl Into<Element<'a, crate::Message>>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(inner)
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

// ---- I/O ------------------------------------------------------

/// Shell out to `mackesd validate status --json`.
pub fn fetch_status() -> Result<ValidationStatus, String> {
    let out = std::process::Command::new("mackesd")
        .args(["validate", "status", "--json"])
        .output()
        .map_err(|e| format!("mackesd validate status failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd validate status exited non-zero: {stderr}"));
    }
    Ok(parse_status(&String::from_utf8_lossy(&out.stdout)))
}

/// Shell out to `mackesd validate run` (request a fresh run).
pub fn request_run() -> Result<String, String> {
    let out = std::process::Command::new("mackesd")
        .args(["validate", "run"])
        .output()
        .map_err(|e| format!("mackesd validate run failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd validate run exited non-zero: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Pure parser for the `validate status --json` object.
#[must_use]
pub fn parse_status(raw: &str) -> ValidationStatus {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return ValidationStatus::default();
    };
    let run_id = v.get("run_id").and_then(|x| x.as_str()).map(str::to_string);
    let edges = |key: &str| -> Vec<Edge> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        Some(Edge {
                            from: e.get("from")?.as_str()?.to_string(),
                            to: e.get("to")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    ValidationStatus {
        run_id,
        passed: v
            .get("passed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        reachable: v
            .get("reachable")
            .and_then(|x| x.as_array())
            .map_or(0, Vec::len),
        failed_edges: edges("failed"),
        missing_reporters: v
            .get("missing_reporters")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_reads_a_pass_verdict() {
        let raw = r#"{"run_id":"v-1","passed":true,"reachable":[{"from":"a","to":"b"}],
            "failed":[],"missing_reporters":[]}"#;
        let s = parse_status(raw);
        assert_eq!(s.run_id.as_deref(), Some("v-1"));
        assert!(s.passed);
        assert_eq!(s.reachable, 1);
        assert!(s.failed_edges.is_empty());
    }

    #[test]
    fn parse_status_reads_a_fail_verdict_with_edges() {
        let raw = r#"{"run_id":"v-2","passed":false,"reachable":[],
            "failed":[{"from":"pine","to":"oak"}],"missing_reporters":["birch"]}"#;
        let s = parse_status(raw);
        assert!(!s.passed);
        assert_eq!(s.failed_edges.len(), 1);
        assert_eq!(s.failed_edges[0].from, "pine");
        assert_eq!(s.missing_reporters, vec!["birch".to_string()]);
    }

    #[test]
    fn parse_status_handles_no_run_and_garbage() {
        assert!(parse_status(r#"{"run_id":null}"#).run_id.is_none());
        assert!(parse_status("not json").run_id.is_none());
    }

    #[test]
    fn view_renders_all_states_without_panic() {
        let mut p = RoutingPanel::new();
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view(); // empty
        p.status = parse_status(
            r#"{"run_id":"v-2","passed":false,"reachable":[],
               "failed":[{"from":"pine","to":"oak"}],"missing_reporters":["birch"]}"#,
        );
        p.run_result = Some(Ok("requested".into()));
        let _ = p.view(); // fail verdict + strip
    }
}
