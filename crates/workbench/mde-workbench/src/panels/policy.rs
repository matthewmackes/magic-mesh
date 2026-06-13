//! PLANES-13 — Controller ▸ Policy panel.
//!
//! Surfaces the declarative policy core pack (W46–W51): each loaded
//! policy is a TOML assertion (`field op expected`) over the replicated
//! directory record. The panel shells `mackesd policy list --json`,
//! which evaluates every policy against the live directory and reports
//! the peers currently violating it — a violation is a drift event
//! (W49) the Remediation panel can then fire a plan against.
//!
//! Read-only: policies are authored as TOML on LizardFS (W88); this
//! surface is the renderer (W88 — GUIs are renderers, not editors).

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// One policy + its current compliance, parsed from
/// `mackesd policy list --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRow {
    pub name: String,
    pub description: String,
    pub field: String,
    pub op: String,
    pub expected: String,
    pub severity: String,
    /// Hostnames currently violating this policy (empty = compliant).
    pub violated_peers: Vec<String>,
}

impl PolicyRow {
    fn is_compliant(&self) -> bool {
        self.violated_peers.is_empty()
    }
    /// The assertion in human form: `revision.currency = synced`.
    fn assertion(&self) -> String {
        let sym = match self.op.as_str() {
            "eq" => "=",
            "ne" => "≠",
            "le" => "≤",
            "ge" => "≥",
            other => other,
        };
        format!("{} {} {}", self.field, sym, self.expected)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PolicyPanel {
    pub rows: Vec<PolicyRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<PolicyRow>, String>),
    RefreshClicked,
}

impl PolicyPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_policies() }, |result| {
            crate::Message::Policy(Message::Loaded(result))
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

        let title = text("Policy")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let violating = self.rows.iter().filter(|r| !r.is_compliant()).count();
        let subtitle_text = if self.last_run_at.is_some() {
            format!(
                "{} polic{} · {} violating",
                self.rows.len(),
                if self.rows.len() == 1 { "y" } else { "ies" },
                violating
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
                    text_color: Color::WHITE,
                    icon_color: None,
                    border_radius: 6.0.into(),
                    border_width: 0.0,
                    border_color: Color::TRANSPARENT,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                }
            },
        )
        .on_press(crate::Message::Policy(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);
        for r in &self.rows {
            rows_col = rows_col.push(policy_row(r, palette));
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

fn severity_color(sev: &str, palette: Palette) -> Color {
    match sev {
        "crit" => palette.danger.into_cosmic_color(),
        "warn" => palette.warning.into_cosmic_color(),
        _ => palette.accent.into_cosmic_color(),
    }
}

fn policy_row<'a>(r: &'a PolicyRow, palette: Palette) -> Element<'a, crate::Message> {
    let compliant = r.is_compliant();
    let (status_icon, status_color, status_text) = if compliant {
        (
            Icon::StatusOk,
            palette.success.into_cosmic_color(),
            "compliant".to_string(),
        )
    } else {
        (
            Icon::StatusWarning,
            severity_color(&r.severity, palette),
            format!("{} violating", r.violated_peers.len()),
        )
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
        text(r.severity.to_uppercase())
            .size(9)
            .colr(severity_color(&r.severity, palette)),
        Space::new().width(Length::Fill),
        text(status_text).size(11).colr(status_color),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let assertion = text(r.assertion())
        .size(11)
        .colr(palette.accent.into_cosmic_color());
    let desc = text(r.description.clone())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());

    // When violated, name the offending peers so the operator can act
    // (the Remediation panel fires a plan against them).
    let mut body = column![assertion, desc].spacing(2);
    if !compliant {
        body = body.push(
            text(format!("violating: {}", r.violated_peers.join(", ")))
                .size(10)
                .colr(severity_color(&r.severity, palette)),
        );
    }

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
                "Couldn't load policies".to_string(),
                err.to_string(),
            )
        } else {
            (
                Icon::StatusOk,
                palette.success.into_cosmic_color(),
                "No policies loaded".to_string(),
                "The core pack ships enabled (all-nodes-current, no-critical-alarms). \
                 Drop a TOML policy under the workgroup's policies/ dir to add more."
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
            text(heading)
                .size(14)
                .colr(palette.text.into_cosmic_color()),
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

/// Shell out to `mackesd policy list --json` and parse the rows.
pub fn fetch_policies() -> Result<Vec<PolicyRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["policy", "list", "--json"])
        .output()
        .map_err(|e| format!("mackesd policy list failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd policy list exited non-zero: {stderr}"));
    }
    Ok(parse_policies(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `policy list --json` array.
#[must_use]
pub fn parse_policies(raw: &str) -> Vec<PolicyRow> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    top.into_iter()
        .map(|p| {
            let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            PolicyRow {
                name: s("name"),
                description: s("description"),
                field: s("field"),
                op: s("op"),
                expected: s("expected"),
                severity: s("severity"),
                violated_peers: p
                    .get("violated_peers")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_policies_reads_the_list_shape() {
        let raw = r#"[
            {"name":"all-nodes-current","description":"d","field":"revision.currency",
             "op":"eq","expected":"synced","severity":"warn","violated_peers":["fedora"]},
            {"name":"no-critical-alarms","description":"d2","field":"health",
             "op":"ne","expected":"critical","severity":"crit","violated_peers":[]}
        ]"#;
        let rows = parse_policies(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "all-nodes-current");
        assert_eq!(rows[0].violated_peers, vec!["fedora".to_string()]);
        assert!(!rows[0].is_compliant());
        assert!(rows[1].is_compliant());
        assert_eq!(rows[0].assertion(), "revision.currency = synced");
    }

    #[test]
    fn parse_policies_returns_empty_for_garbage() {
        assert!(parse_policies("not json").is_empty());
        assert!(parse_policies("").is_empty());
    }

    #[test]
    fn view_renders_without_panic() {
        let mut p = PolicyPanel::new();
        p.rows = parse_policies(
            r#"[{"name":"all-nodes-current","description":"d","field":"revision.currency",
                "op":"eq","expected":"synced","severity":"warn","violated_peers":["fedora"]}]"#,
        );
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
    }

    #[test]
    fn view_renders_empty_and_error_without_panic() {
        let mut p = PolicyPanel::new();
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
        p.error = Some("mackesd down".into());
        let _ = p.view();
    }
}
