//! PLANES-12 — the **Audit** panel (Controller plane), absorbing ENT-14.
//!
//! A timeline viewer + verify-chain over the tamper-evident audit log
//! (W43/W44): security, jobs/remediation, config/policy, and lifecycle
//! events, hash-chained in the daemon's `events` table. The panel shells
//! to `mackesd audit-verify --json` (the backend that walks the SHA-256
//! chain and emits the verdict + the **72 h rolling window** of events,
//! W45) and renders the chain status + the timeline. Re-verify re-runs it.
//!
//! Build-now-defer-visual: the JSON projection is pure + unit-tested; the
//! on-Cosmic `/preview` pass is the deferred tail.

use cosmic::iced::widget::{column, container, row, scrollable, text};
use cosmic::iced::{Length, Padding, Task};
// CUT-1: cosmic::Element bakes in cosmic::Theme, matching the theme the
// panel_chrome / controls helpers thread through the tree.
use cosmic::Element;
use mde_theme::{EmptyState, Icon};
use serde::Deserialize;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{empty_state, panel_container, status_badge, BadgeSeverity};
use crate::panels::fleet_settings::run_mackesd;

/// One audit event row (from the `audit-verify --json` timeline).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AuditEventRow {
    pub event_id: u64,
    pub timestamp_ms: i64,
    #[serde(default)]
    pub payload: String,
    #[serde(default)]
    pub hash: String,
}

/// The `mackesd audit-verify --json` document.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct AuditReport {
    /// `intact` | `break` | `empty`.
    pub verify: String,
    pub detail: String,
    pub total_events: usize,
    pub retained_72h: usize,
    pub timeline: Vec<AuditEventRow>,
}

/// Parse the backend JSON; tolerant of an empty/garbled body (→ a
/// best-effort empty report so the panel renders rather than panicking).
#[must_use]
pub fn parse_report(raw: &str) -> AuditReport {
    serde_json::from_str(raw).unwrap_or_default()
}

/// The Audit panel state.
#[derive(Debug, Clone, Default)]
pub struct AuditPanel {
    pub report: AuditReport,
    pub loaded: bool,
    pub status: String,
    /// EFF-45 — set when the audit-verify CLI FAILED (exit non-zero + no JSON).
    /// The view renders the error state instead of the misleading empty state.
    pub load_error: Option<String>,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(AuditReport),
    Error(String),
    VerifyClicked,
}

impl AuditPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                match run_mackesd(&["audit-verify".into(), "--json".into()]).await {
                    // A chain break makes the CLI exit 1 but still prints
                    // the JSON on stdout — parse it regardless of status.
                    Ok(out) | Err(out) if out.trim_start().starts_with('{') => {
                        Message::Loaded(parse_report(&out))
                    }
                    Ok(out) => Message::Loaded(parse_report(&out)),
                    Err(e) => Message::Error(e),
                }
            },
            crate::Message::Audit,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(report) => {
                self.report = report;
                self.loaded = true;
                self.load_error = None;
                self.busy = false;
                self.status.clear();
                Task::none()
            }
            Message::Error(e) => {
                // EFF-45 — a failed CLI run is an error state, not an empty
                // audit log.
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::VerifyClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Verifying chain…".into();
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let verify_btn = variant_button(
            "Verify chain",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::Audit(Message::VerifyClicked)),
            palette,
        );

        // EFF-45 — a failed CLI run renders as failure, never as the empty state.
        if let Some(err) = &self.load_error {
            return panel_container(
                crate::panel_chrome::error_state(err.clone(), palette, || {
                    crate::Message::Audit(Message::VerifyClicked)
                }),
                density,
            );
        }

        if !self.loaded || (self.report.timeline.is_empty() && self.report.verify == "empty") {
            let state = EmptyState::with_cta(
                "No audit events in the last 72 h",
                "Security, jobs, config, and lifecycle operations are hash-chained \
                 and appear here within the 72 h retention window (PLANES-12). \
                 Verify-chain confirms the log hasn't been tampered with.",
                "Verify chain",
            )
            .with_icon(Icon::Inventory);
            return panel_container(
                empty_state(state, palette, || {
                    crate::Message::Audit(Message::VerifyClicked)
                }),
                density,
            );
        }

        // Chain-status badge.
        let (badge_label, severity) = match self.report.verify.as_str() {
            "intact" => ("chain intact", BadgeSeverity::Success),
            "break" => ("CHAIN BREAK", BadgeSeverity::Warning),
            _ => ("empty", BadgeSeverity::Neutral),
        };
        let header: Element<'_, crate::Message> = row![
            verify_btn,
            status_badge(badge_label, severity, palette),
            text(format!(
                "{} event(s) in 72 h · {} total{}",
                self.report.retained_72h,
                self.report.total_events,
                if self.report.detail.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", self.report.detail)
                },
            ))
            .size(13),
        ]
        .spacing(12)
        .align_y(cosmic::iced::Alignment::Center)
        .into();

        let mut list = column![].spacing(6);
        for e in &self.report.timeline {
            let short_hash: String = e.hash.chars().take(12).collect();
            list = list.push(
                container(
                    column![
                        text(format!(
                            "#{}  ·  {}ms  ·  {}…",
                            e.event_id, e.timestamp_ms, short_hash
                        ))
                        .size(12),
                        text(e.payload.clone()).size(13),
                    ]
                    .spacing(2),
                )
                .padding(Padding::from(8)),
            );
        }

        panel_container(
            column![header, scrollable(list).height(Length::Fill)]
                .spacing(16)
                .width(Length::Fill)
                .into(),
            density,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_report_reads_verdict_and_timeline() {
        let raw = r#"{
            "verify": "intact", "detail": "3 events",
            "total_events": 3, "retained_72h": 2,
            "timeline": [
                {"event_id": 2, "timestamp_ms": 1000, "payload": "config.apply", "hash": "abcd"},
                {"event_id": 3, "timestamp_ms": 2000, "payload": "job.run", "hash": "ef01"}
            ]
        }"#;
        let r = parse_report(raw);
        assert_eq!(r.verify, "intact");
        assert_eq!(r.total_events, 3);
        assert_eq!(r.retained_72h, 2);
        assert_eq!(r.timeline.len(), 2);
        assert_eq!(r.timeline[0].payload, "config.apply");
    }

    #[test]
    fn parse_report_tolerates_garbage() {
        assert_eq!(parse_report("not json"), AuditReport::default());
        assert_eq!(parse_report(""), AuditReport::default());
    }

    #[test]
    fn update_loaded_sets_report_and_clears_busy() {
        let mut p = AuditPanel::new();
        p.busy = true;
        let _ = p.update(Message::Loaded(parse_report(
            r#"{"verify":"break","detail":"at event 5","total_events":5,"retained_72h":5,"timeline":[]}"#,
        )));
        assert!(p.loaded);
        assert!(!p.busy);
        assert_eq!(p.report.verify, "break");
    }
}
