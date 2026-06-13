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

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

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
        let palette = crate::live_theme::palette();
        match self {
            Self::Info => palette.accent.into_cosmic_color(),
            Self::Warn => palette.warning.into_cosmic_color(),
            Self::Error => palette.danger.into_cosmic_color(),
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

/// PLANES-11 — one live policy violation paired with its remediation
/// plan, parsed from `mackesd remediate match --json` (the `MatchedDrift`
/// shape). The Fire button enqueues the matched plan's job bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationMatch {
    pub peer: String,
    pub policy: String,
    pub severity: DriftSeverity,
    pub detail: String,
    /// The matched plan name, or `None` when no plan covers the drift.
    pub plan: Option<String>,
    /// W42 — whether the matched plan auto-fires on the leader sweep.
    pub auto: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DriftPanel {
    pub rows: Vec<DriftRow>,
    /// PLANES-11 — live drift→plan matches (the remediation list).
    pub matches: Vec<RemediationMatch>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    pub error: Option<String>,
    /// Result strip from the most recent Fire (the loud launch reply).
    pub fire_result: Option<Result<String, String>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Loaded, String>),
    RefreshClicked,
    /// PLANES-11 (W41) — fire `plan` against the drifted `peer`.
    Fire {
        plan: String,
        peer: String,
    },
    Fired(Result<String, String>),
}

/// The combined load: audit-chain drift events + live remediation
/// matches (PLANES-11).
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub events: Vec<DriftRow>,
    pub matches: Vec<RemediationMatch>,
}

impl DriftPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_drift_and_matches() }, |result| {
            crate::Message::Drift(Message::Loaded(result))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(loaded)) => {
                self.rows = loaded.events;
                self.matches = loaded.matches;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.rows = Vec::new();
                self.matches = Vec::new();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                self.fire_result = None;
                Self::load()
            }
            Message::Fire { plan, peer } => {
                self.busy = true;
                Task::perform(async move { fire_plan(&plan, &peer) }, |result| {
                    crate::Message::Drift(Message::Fired(result))
                })
            }
            Message::Fired(result) => {
                self.fire_result = Some(result);
                self.busy = false;
                // Reload so the matched-drift list reflects the fire.
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Remediation")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if let Some(t) = self.last_run_at {
            format!(
                "{} live drift match{} · {} audit event{} · last refresh {}",
                self.matches.len(),
                if self.matches.len() == 1 { "" } else { "es" },
                self.rows.len(),
                if self.rows.len() == 1 { "" } else { "s" },
                fmt_age(t)
            )
        } else {
            "click Refresh to load".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let refresh_btn = button(
            text(if self.busy { "Loading…" } else { "Refresh" })
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
        .on_press(crate::Message::Drift(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut rows_col = column![].spacing(6);

        // PLANES-11 — the loud result strip from the last Fire.
        if let Some(res) = &self.fire_result {
            rows_col = rows_col.push(fire_result_strip(res, palette));
        }

        // PLANES-11 — the live drift→plan matches with Fire buttons.
        if !self.matches.is_empty() {
            rows_col = rows_col.push(section_heading("Drift → remediation plan", palette));
            for m in &self.matches {
                rows_col = rows_col.push(remediation_row(m, palette, self.busy));
            }
            rows_col = rows_col.push(Space::new().height(Length::Fixed(14.0)));
        }

        // The audit-chain drift events (historical view).
        if !self.rows.is_empty() {
            rows_col = rows_col.push(section_heading("Recent drift events", palette));
        }
        for r in &self.rows {
            rows_col = rows_col.push(drift_row(r, palette));
        }
        if self.rows.is_empty() && self.matches.is_empty() && self.last_run_at.is_some() {
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

/// A small uppercase section divider label.
fn section_heading<'a>(label: &'static str, palette: Palette) -> Element<'a, crate::Message> {
    container(
        text(label)
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    )
    .padding(Padding::from([4u16, 2u16]))
    .into()
}

/// PLANES-11 — one drift→plan row: severity + peer + policy, the
/// matched plan (or "no plan"), an AUTO badge (W42), and a Fire button
/// (enabled only when a plan matched and the panel isn't busy).
fn remediation_row<'a>(
    m: &'a RemediationMatch,
    palette: Palette,
    busy: bool,
) -> Element<'a, crate::Message> {
    let icon_color = m.severity.color();
    let resolved = mde_icon(m.severity.icon(), IconSize::Inline);
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(icon_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .colr(icon_color)
            .into()
    };

    let plan_label: Element<'a, crate::Message> = match &m.plan {
        Some(plan) => text(format!("→ {plan}"))
            .size(11)
            .colr(palette.accent.into_cosmic_color())
            .into(),
        None => text("→ no remediation plan")
            .size(11)
            .colr(palette.text_muted.into_cosmic_color())
            .into(),
    };

    let mut head = row![
        icon_widget,
        text(m.severity.label()).size(10).colr(icon_color),
        text(m.peer.clone())
            .size(11)
            .colr(palette.text.into_cosmic_color()),
        text(m.policy.clone())
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
        plan_label,
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    // W42 — surface the auto flag so the operator sees which plans the
    // leader sweep will fire on its own.
    if m.auto {
        head = head.push(text("AUTO").size(9).colr(palette.warning.into_cosmic_color()));
    }
    head = head.push(Space::new().width(Length::Fill));

    // Fire button — only when a plan matched (W41). Disabled while busy.
    if let Some(plan) = &m.plan {
        let accent = palette.accent.into_cosmic_color();
        let mut btn = button(text("Fire").size(12).colr(Color::WHITE))
            .padding(Padding::from([4u16, 12u16]))
            .sty(
                move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                    let bg = match status {
                        cosmic::iced::widget::button::Status::Hovered => Color {
                            r: accent.r * 1.10,
                            g: accent.g * 1.10,
                            b: accent.b * 1.10,
                            a: accent.a,
                        },
                        cosmic::iced::widget::button::Status::Disabled => {
                            palette.raised.into_cosmic_color()
                        }
                        _ => accent,
                    };
                    cosmic::iced::widget::button::Style {
                        snap: false,
                        background: Some(Background::Color(bg)),
                        text_color: Color::WHITE,
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: 5.0.into(),
                        },
                        shadow: cosmic::iced::Shadow::default(),
                        ..cosmic::iced::widget::button::Style::default()
                    }
                },
            );
        if !busy {
            btn = btn.on_press(crate::Message::Drift(Message::Fire {
                plan: plan.clone(),
                peer: m.peer.clone(),
            }));
        }
        head = head.push(btn);
    }

    let detail = text(m.detail.clone())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color());

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(column![head, detail].spacing(4))
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

/// PLANES-11 — the loud result strip from the most recent Fire: the
/// launch reply (run id + targets) on success, the error on failure.
fn fire_result_strip<'a>(
    res: &'a Result<String, String>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let (accent, label): (Color, String) = match res {
        Ok(reply) => (
            palette.success.into_cosmic_color(),
            format!("Fired — {reply}"),
        ),
        Err(e) => (
            palette.danger.into_cosmic_color(),
            format!("Fire failed — {e}"),
        ),
    };
    let bg = palette.raised.into_cosmic_color();
    container(text(label).size(11).colr(accent))
        .padding(Padding::from([8u16, 14u16]))
        .width(Length::Fill)
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: accent,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn drift_row<'a>(r: &'a DriftRow, palette: Palette) -> Element<'a, crate::Message> {
    let resolved = mde_icon(r.severity.icon(), IconSize::Inline);
    let icon_color = r.severity.color();
    let icon_widget: Element<'a, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t: &Theme| widget_svg::Style {
                color: Some(icon_color),
            })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .colr(icon_color)
            .into()
    };

    let head = row![
        icon_widget,
        text(r.severity.label()).size(10).colr(icon_color),
        text(format!("#{}", r.event_id))
            .size(10)
            .colr(palette.text_muted.into_cosmic_color()),
        text(r.peer.clone())
            .size(11)
            .colr(palette.text.into_cosmic_color()),
        Space::new().width(Length::Fill),
        text(fmt_epoch_ms(r.timestamp_ms))
            .size(10)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let body = text(r.message.clone())
        .size(12)
        .colr(palette.text_muted.into_cosmic_color());

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
    let (icon_kind, icon_color, heading_text, body_text): (Icon, Color, String, String) =
        if let Some(err) = error {
            (
                Icon::StatusError,
                palette.danger.into_cosmic_color(),
                "Couldn't load drift events".to_string(),
                err.to_string(),
            )
        } else {
            (
                Icon::StatusOk,
                palette.success.into_cosmic_color(),
                "No drift detected".to_string(),
                "mackesd's reconciler has not surfaced any divergence between the locked TOML \
                 config and the live state. When mackesd is running, new events will appear here."
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
            text(heading_text)
                .size(14)
                .colr(palette.text.into_cosmic_color()),
            text(body_text)
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

/// PLANES-11 — load both the audit-chain drift events and the live
/// remediation matches. The matches drive the Fire UI; a failure to
/// fetch matches is non-fatal (the events still render).
pub fn fetch_drift_and_matches() -> Result<Loaded, String> {
    let events = fetch_drift_events()?;
    let matches = fetch_matches().unwrap_or_default();
    Ok(Loaded { events, matches })
}

/// Shell out to `mackesd remediate match --json` and parse the
/// `MatchedDrift` array into [`RemediationMatch`] rows.
pub fn fetch_matches() -> Result<Vec<RemediationMatch>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["remediate", "match", "--json"])
        .output()
        .map_err(|e| format!("mackesd remediate match failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd remediate match exited non-zero: {stderr}"));
    }
    Ok(parse_matches(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for the `MatchedDrift` JSON array
/// (`[{violation:{peer,policy,severity,detail}, plan, template, auto}]`).
#[must_use]
pub fn parse_matches(raw: &str) -> Vec<RemediationMatch> {
    let Ok(top) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
        return Vec::new();
    };
    top.into_iter()
        .map(|m| {
            let v = m
                .get("violation")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let sev = v.get("severity").and_then(|x| x.as_str()).unwrap_or("");
            RemediationMatch {
                peer: v
                    .get("peer")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                policy: v
                    .get("policy")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                severity: DriftSeverity::from_str(sev),
                detail: v
                    .get("detail")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                plan: m.get("plan").and_then(|x| x.as_str()).map(str::to_string),
                auto: m
                    .get("auto")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            }
        })
        .collect()
}

/// Shell out to `mackesd remediate fire --plan <p> --peer <h>`;
/// returns the launch reply (run id + targets) on success.
pub fn fire_plan(plan: &str, peer: &str) -> Result<String, String> {
    let out = std::process::Command::new("mackesd")
        .args(["remediate", "fire", "--plan", plan, "--peer", peer])
        .output()
        .map_err(|e| format!("mackesd remediate fire failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd remediate fire exited non-zero: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
    fn parse_matches_reads_the_matched_drift_shape() {
        // The `mackesd remediate match --json` (MatchedDrift) shape.
        let raw = r#"[
            {"violation":{"policy":"all-nodes-current","peer":"pine","severity":"warn","detail":"behind"},
             "plan":"resync-behind-node","template":"reconcile-config","auto":false,"vars":{}},
            {"violation":{"policy":"unknown","peer":"oak","severity":"crit","detail":"x"},
             "plan":null,"template":null,"auto":false,"vars":{}}
        ]"#;
        let rows = parse_matches(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].peer, "pine");
        assert_eq!(rows[0].plan.as_deref(), Some("resync-behind-node"));
        assert_eq!(rows[0].severity, DriftSeverity::Warn);
        // Unmatched drift surfaces with no plan (Fire button hidden).
        assert!(rows[1].plan.is_none());
    }

    #[test]
    fn parse_matches_returns_empty_for_garbage() {
        assert!(parse_matches("not json").is_empty());
        assert!(parse_matches("").is_empty());
    }

    #[test]
    fn view_renders_with_matches_and_fire_strip_without_panic() {
        let mut p = DriftPanel::new();
        p.matches = vec![RemediationMatch {
            peer: "pine".into(),
            policy: "all-nodes-current".into(),
            severity: DriftSeverity::Warn,
            detail: "behind".into(),
            plan: Some("resync-behind-node".into()),
            auto: true,
        }];
        p.fire_result = Some(Ok("{\"ok\":true,\"run_id\":\"rem-1\"}".into()));
        p.last_run_at = Some(SystemTime::now());
        let _ = p.view();
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
