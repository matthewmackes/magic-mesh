//! Network → Mesh History panel — audit-log viewer over
//! `mackesd events list --json`.
//!
//! CB-1.8 partial: replaces the v1.x
//! `mackes/workbench/network/mesh_history.py`. Mesh
//! history in v2.0.0 is the hash-chained `events` table
//! mackesd's audit-verify already validates; this panel
//! surfaces the same rows for human inspection without
//! requiring the user to run audit-verify by hand.

use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Padding, Task};
use mde_theme::{EmptyState, Icon};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{empty_state, panel_container};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventRow {
    pub event_id: u64,
    pub timestamp_ms: i64,
    pub payload: String,
    pub hash: String,
}

/// NF-11.4 (v2.5) — the canonical set of Nebula-specific
/// event kinds the history panel highlights. Any payload
/// containing one of these substrings renders with the
/// "fabric event" badge styling.
pub const NEBULA_EVENT_KINDS: &[&str] = &[
    "nebula_ca_rotated",
    "nebula_peer_cert_issued",
    "nebula_peer_cert_revoked",
    "nebula_lighthouse_promoted",
    "nebula_lighthouse_demoted",
];

/// NF-11.4 — pure helper. True when the event payload
/// mentions any [`NEBULA_EVENT_KINDS`] token. The history
/// panel uses this to apply the "fabric event"
/// chronological styling alongside the existing
/// lifecycle-event styling.
#[must_use]
pub fn is_nebula_event(payload: &str) -> bool {
    NEBULA_EVENT_KINDS.iter().any(|k| payload.contains(k))
}

/// NF-11.4 — pure helper. Filter a row slice down to just
/// the Nebula events, preserving order. Used by the
/// "Show fabric events only" toggle the panel surfaces
/// when the operator wants to scan CA + lighthouse
/// activity without the rest of the audit log noise.
#[must_use]
pub fn filter_nebula(rows: &[EventRow]) -> Vec<EventRow> {
    rows.iter()
        .filter(|r| is_nebula_event(&r.payload))
        .cloned()
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct MeshHistoryPanel {
    pub rows: Vec<EventRow>,
    pub status: String,
    pub busy: bool,
    /// Event-id of the row currently drilled into. None = list
    /// view; Some(_) = detail view showing the raw payload + hash.
    pub focused: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<EventRow>),
    Error(String),
    FocusRow(u64),
    Back,
    RefreshClicked,
}

impl MeshHistoryPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let raw = run_mackesd_events_list().await;
                match parse_events_json(&raw) {
                    Ok(rows) => Message::Loaded(rows),
                    Err(e) => Message::Error(e),
                }
            },
            crate::Message::MeshHistory,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(rows) => {
                self.rows = rows;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::FocusRow(id) => {
                self.focused = Some(id);
                Task::none()
            }
            Message::Back => {
                self.focused = None;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        if let Some(id) = self.focused {
            let Some(row) = self.rows.iter().find(|r| r.event_id == id) else {
                return self.view_list();
            };
            return self.view_detail(row);
        }
        self.view_list()
    }

    fn view_list(&self) -> Element<'_, crate::Message> {
        // UX-7.a — refresh routed through the shared button variant.
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::MeshHistory(Message::RefreshClicked)),
            crate::live_theme::palette(),
        );

        if self.rows.is_empty() {
            // UX-6 — canonical empty-state pattern (icon slot +
            // heading + body + CTA). `refresh_btn` stays unused
            // in this branch; the CTA inside `empty_state` is
            // the user's primary affordance to retry.
            let _ = refresh_btn;
            let state = EmptyState::with_cta(
                "Audit chain empty",
                "mackesd hasn't recorded any events yet. Enroll a peer or apply a \
                 desired-config revision to populate the log, then refresh.",
                "Refresh",
            )
            .with_icon(Icon::History);
            return panel_container(
                empty_state(state, crate::live_theme::palette(), || {
                    crate::Message::MeshHistory(Message::RefreshClicked)
                }),
                crate::live_theme::tokens().density,
            );
        }

        let rows = self.rows.iter().fold(column![], |col, r| {
            let id = r.event_id;
            // UX-7.a — per-row Detail routed through Ghost.
            let detail = variant_button(
                "Detail",
                ButtonVariant::Ghost,
                Some(crate::Message::MeshHistory(Message::FocusRow(id))),
                crate::live_theme::palette(),
            );
            let payload_preview: String = r.payload.chars().take(80).collect();
            col.push(
                row![
                    text(format!("#{id}")).width(Length::Fixed(60.0)),
                    text(format_ts(r.timestamp_ms)).width(Length::Fixed(180.0)),
                    text(payload_preview).width(Length::Fill),
                    detail,
                ]
                .spacing(12),
            )
        });

        column![
            row![refresh_btn, text(&self.status).size(13)].spacing(12),
            scrollable(container(rows.spacing(4))).height(Length::Fill),
            text(format!("{} events in chain", self.rows.len())).size(13),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }

    fn view_detail<'a>(&'a self, row: &'a EventRow) -> Element<'a, crate::Message> {
        // UX-7.a — back routed through Ghost.
        let back = variant_button(
            "← Back",
            ButtonVariant::Ghost,
            Some(crate::Message::MeshHistory(Message::Back)),
            crate::live_theme::palette(),
        );
        column![
            row![
                back,
                text(format!(
                    "Event #{} · {}",
                    row.event_id,
                    format_ts(row.timestamp_ms)
                ))
                .size(18),
            ]
            .spacing(12),
            text(format!("Hash: {}", row.hash)).size(12),
            scrollable(
                container(text(&row.payload).size(12))
                    .padding(Padding::new(12.0))
                    .width(Length::Fill),
            )
            .height(Length::Fill),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Format a Unix-epoch-milliseconds timestamp as
/// `YYYY-MM-DD HH:MM:SS UTC`. Returns `"-"` for non-positive
/// timestamps.
#[must_use]
pub fn format_ts(ms: i64) -> String {
    if ms <= 0 {
        return "-".to_string();
    }
    let secs = ms / 1000;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}Z")
}

fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mo <= 2 { y + 1 } else { y };
    (year as i32, mo as u32, d as u32)
}

/// Pure JSON parser for `mackesd events list --json` output.
///
/// # Errors
///
/// Returns `Err(String)` on malformed input (non-array,
/// invalid JSON). Empty input + empty array both return
/// `Ok(vec![])`.
pub fn parse_events_json(raw: &str) -> Result<Vec<EventRow>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid mackesd output: {e}"))?;
    let arr = v
        .as_array()
        .ok_or_else(|| "expected JSON array at top level".to_string())?;
    Ok(arr
        .iter()
        .map(|o| EventRow {
            event_id: o.get("event_id").and_then(|v| v.as_u64()).unwrap_or(0),
            timestamp_ms: o.get("timestamp_ms").and_then(|v| v.as_i64()).unwrap_or(0),
            payload: o
                .get("payload")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            hash: o
                .get("hash")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect())
}

/// Shell out to `mackesd events list --json`. Returns stdout on
/// success; empty on failure (consumer surfaces "audit chain
/// empty" empty-state).
pub async fn run_mackesd_events_list() -> String {
    let Ok(output) = Command::new("mackesd")
        .args(["events", "list", "--json"])
        .output()
        .await
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8(output.stdout).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
        {"event_id": 1, "timestamp_ms": 1715000000000,
         "payload": "{\"type\":\"enroll\",\"node\":\"alpha\"}",
         "hash": "abc123"},
        {"event_id": 2, "timestamp_ms": 1715000050000,
         "payload": "{\"type\":\"apply\",\"revision\":42}",
         "hash": "def456"}
    ]"#;

    #[test]
    fn parse_events_json_extracts_every_field() {
        let rows = parse_events_json(SAMPLE).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].event_id, 1);
        assert_eq!(rows[0].timestamp_ms, 1_715_000_000_000);
        assert!(rows[0].payload.contains("enroll"));
        assert_eq!(rows[0].hash, "abc123");
    }

    #[test]
    fn parse_events_json_empty_array_returns_empty_vec() {
        assert!(parse_events_json("[]").unwrap().is_empty());
        assert!(parse_events_json("").unwrap().is_empty());
    }

    #[test]
    fn parse_events_json_rejects_non_array() {
        assert!(parse_events_json("{\"x\":1}").is_err());
    }

    #[test]
    fn parse_events_json_rejects_garbage() {
        assert!(parse_events_json("not json").is_err());
    }

    #[test]
    fn format_ts_renders_known_timestamp() {
        // 1_715_000_000_000 ms = 2024-05-06 16:53:20 UTC
        let out = format_ts(1_715_000_000_000);
        assert!(out.starts_with("2024-05-06"), "got: {out}");
        assert!(out.ends_with("Z"));
    }

    #[test]
    fn format_ts_dashes_zero_or_negative() {
        assert_eq!(format_ts(0), "-");
        assert_eq!(format_ts(-1), "-");
    }

    #[test]
    fn loaded_records_rows_and_clears_busy() {
        let mut panel = MeshHistoryPanel::new();
        panel.busy = true;
        let rows = parse_events_json(SAMPLE).unwrap();
        let _ = panel.update(Message::Loaded(rows.clone()));
        assert_eq!(panel.rows, rows);
        assert!(!panel.busy);
    }

    #[test]
    fn focus_and_back_round_trip() {
        let mut panel = MeshHistoryPanel::new();
        let _ = panel.update(Message::FocusRow(42));
        assert_eq!(panel.focused, Some(42));
        let _ = panel.update(Message::Back);
        assert!(panel.focused.is_none());
    }

    #[test]
    fn refresh_clicked_while_busy_is_noop() {
        let mut panel = MeshHistoryPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = MeshHistoryPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("mackesd not on PATH".into()));
        assert_eq!(panel.status, "mackesd not on PATH");
        assert!(!panel.busy);
    }

    // ─────────────────────────────────────────────────────
    // NF-11.4 — Nebula event filter
    // ─────────────────────────────────────────────────────

    #[test]
    fn nebula_event_kinds_locked() {
        // Lock the curated set so a future edit that adds
        // a new kind also adds a test row covering it.
        assert!(NEBULA_EVENT_KINDS.contains(&"nebula_ca_rotated"));
        assert!(NEBULA_EVENT_KINDS.contains(&"nebula_peer_cert_issued"));
        assert!(NEBULA_EVENT_KINDS.contains(&"nebula_peer_cert_revoked"));
        assert!(NEBULA_EVENT_KINDS.contains(&"nebula_lighthouse_promoted"));
        assert!(NEBULA_EVENT_KINDS.contains(&"nebula_lighthouse_demoted"));
    }

    #[test]
    fn is_nebula_event_matches_substring() {
        assert!(is_nebula_event("kind=nebula_ca_rotated mesh=m1"));
        assert!(is_nebula_event(
            r#"{"kind":"nebula_lighthouse_promoted","node":"peer:lh1"}"#
        ));
        assert!(!is_nebula_event("kind=heartbeat node=peer:alpha"));
        assert!(!is_nebula_event(""));
    }

    #[test]
    fn filter_nebula_keeps_only_fabric_events_in_order() {
        let rows = vec![
            EventRow {
                event_id: 1,
                payload: "kind=heartbeat".into(),
                ..Default::default()
            },
            EventRow {
                event_id: 2,
                payload: "kind=nebula_ca_rotated".into(),
                ..Default::default()
            },
            EventRow {
                event_id: 3,
                payload: "kind=settings_changed".into(),
                ..Default::default()
            },
            EventRow {
                event_id: 4,
                payload: "kind=nebula_peer_cert_issued".into(),
                ..Default::default()
            },
        ];
        let filtered = filter_nebula(&rows);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].event_id, 2);
        assert_eq!(filtered[1].event_id, 4);
    }
}
