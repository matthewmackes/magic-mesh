//! Fleet → Run history panel — table of every ansible-pull
//! run across the mesh, sourced from
//! `~/QNM-Shared/.qnm-sync/ansible-runs/<peer>/<ts>.json`.
//!
//! CB-1.5.c: replaces the v1.x
//! `mackes/workbench/fleet/run_history.py`. Same filesystem
//! source the Python panel reads through `mackes.fleet.list_runs`
//! — the JSON shape is a fixture written by ansible-pull's
//! `--callback-plugin mackes_runlog`. Click a row → modal body
//! with the raw payload for the operator to inspect.

use std::path::PathBuf;

use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Padding, Task};
use mde_theme::{Density, EmptyState, Icon, Palette};

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{empty_state, panel_container};

/// One row in the run-history table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunRow {
    pub peer: String,
    pub playbook: String,
    pub timestamp: f64,
    pub exit_code: i32,
    pub changed: u32,
    pub ok_count: u32,
    pub failed: u32,
    pub triggered_by: String,
    /// Raw JSON payload from the run-log file — surfaced on
    /// drill-in so the operator can inspect the full record.
    pub raw_json: String,
    /// Source path on disk (used for the drill-in title bar
    /// + as the row id for `FocusRow`).
    pub path: String,
}

#[derive(Debug, Clone, Default)]
pub struct RunHistoryPanel {
    pub rows: Vec<RunRow>,
    pub status: String,
    /// Path of the row currently drilled into; `None` = list
    /// view; `Some(_)` = detail view.
    pub focused_path: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<RunRow>),
    Error(String),
    FocusRow(String),
    Back,
    RefreshClicked,
}

impl RunHistoryPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let dir = ansible_runs_root();
                match collect_runs(&dir).await {
                    Ok(rows) => Message::Loaded(rows),
                    Err(e) => Message::Error(e),
                }
            },
            crate::Message::RunHistory,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(rows) => {
                self.rows = rows;
                self.status.clear();
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                Task::none()
            }
            Message::FocusRow(path) => {
                self.focused_path = Some(path);
                Task::none()
            }
            Message::Back => {
                self.focused_path = None;
                Task::none()
            }
            Message::RefreshClicked => Self::load(),
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        if let Some(path) = &self.focused_path {
            let Some(row) = self.rows.iter().find(|r| r.path == *path) else {
                // The row disappeared (refresh during drill-in);
                // bail back to the list view rather than rendering
                // a phantom detail.
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
            Some(crate::Message::RunHistory(Message::RefreshClicked)),
            Palette::dark(),
        );

        if self.rows.is_empty() {
            // UX-6.b — empty-state with refresh CTA.
            let _ = refresh_btn;
            let state = EmptyState::with_cta(
                "No runs recorded",
                "MDE reads ansible-pull history from \
                 ~/QNM-Shared/.qnm-sync/ansible-runs/<peer>/. Run a playbook \
                 from Fleet → Playbooks and refresh.",
                "Refresh",
            )
            .with_icon(Icon::History);
            return panel_container(
                empty_state(state, Palette::dark(), || {
                    crate::Message::RunHistory(Message::RefreshClicked)
                }),
                Density::Comfortable,
            );
        }

        let header = row![
            text("peer").width(Length::Fixed(140.0)),
            text("playbook").width(Length::Fixed(220.0)),
            text("when").width(Length::Fixed(180.0)),
            text("exit").width(Length::Fixed(60.0)),
            text("changed").width(Length::Fixed(80.0)),
            text("trigger"),
        ]
        .spacing(12);

        let rows = self.rows.iter().fold(column![], |col, run| {
            // UX-7.a — per-row Detail routed through Ghost.
            let detail = {
                let path = run.path.clone();
                variant_button(
                    "Detail",
                    ButtonVariant::Ghost,
                    Some(crate::Message::RunHistory(Message::FocusRow(path))),
                    Palette::dark(),
                )
            };
            col.push(
                row![
                    text(&run.peer).width(Length::Fixed(140.0)),
                    text(&run.playbook).width(Length::Fixed(220.0)),
                    text(format_ts(run.timestamp)).width(Length::Fixed(180.0)),
                    text(format!("{}", run.exit_code)).width(Length::Fixed(60.0)),
                    text(format!("{}", run.changed)).width(Length::Fixed(80.0)),
                    text(&run.triggered_by).width(Length::Fixed(80.0)),
                    detail,
                ]
                .spacing(12),
            )
        });

        column![
            header,
            scrollable(container(rows.spacing(6))).height(Length::Fill),
            row![
                refresh_btn,
                text(&self.status).size(13),
                text(format!("Runs: {}", self.rows.len())).size(13),
            ]
            .spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }

    fn view_detail<'a>(&'a self, row: &'a RunRow) -> Element<'a, crate::Message> {
        // UX-7.a — back routed through Ghost.
        let back_btn = variant_button(
            "← Back to history",
            ButtonVariant::Ghost,
            Some(crate::Message::RunHistory(Message::Back)),
            Palette::dark(),
        );
        column![
            row![
                back_btn,
                text(format!(
                    "{} · {} · {}",
                    row.peer,
                    row.playbook,
                    format_ts(row.timestamp)
                ))
                .size(18),
            ]
            .spacing(12),
            text(format!(
                "exit={}  changed={}  ok={}  failed={}  trigger={}",
                row.exit_code, row.changed, row.ok_count, row.failed, row.triggered_by,
            ))
            .size(13),
            scrollable(
                container(text(&row.raw_json).size(13))
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

/// Resolve the ansible-runs root. Matches the v1.x Python
/// panel's `peer_runs_dir` resolver:
/// `$QNM_SHARED_ROOT/.qnm-sync/ansible-runs/`, falling back
/// to `~/QNM-Shared/.qnm-sync/ansible-runs/`.
fn ansible_runs_root() -> PathBuf {
    let base = std::env::var("QNM_SHARED_ROOT").map(PathBuf::from).ok();
    let base = base.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("QNM-Shared"))
            .unwrap_or_else(|_| PathBuf::from("/var/empty"))
    });
    base.join(".qnm-sync").join("ansible-runs")
}

/// Walk `<root>/<peer>/*.json` files, parse each, and return
/// the resulting rows sorted by timestamp descending (newest
/// first). Errors propagate as Err so the panel can lay the
/// reason into its status row.
///
/// # Errors
///
/// Returns `Err(String)` when the root directory is present
/// but unreadable. Returns `Ok(vec![])` when the root simply
/// doesn't exist (matches the panel's empty-state branch —
/// fresh-install boxes shouldn't surface a scary error here).
pub async fn collect_runs(root: &std::path::Path) -> Result<Vec<RunRow>, String> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut peers = match tokio::fs::read_dir(root).await {
        Ok(rd) => rd,
        Err(e) => return Err(format!("reading {}: {e}", root.display())),
    };
    let mut rows = Vec::new();
    while let Ok(Some(peer_entry)) = peers.next_entry().await {
        if !peer_entry
            .file_type()
            .await
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let peer_name = peer_entry
            .file_name()
            .to_str()
            .map(str::to_string)
            .unwrap_or_default();
        if peer_name.is_empty() {
            continue;
        }
        let peer_dir = peer_entry.path();
        let Ok(mut run_files) = tokio::fs::read_dir(&peer_dir).await else {
            continue;
        };
        while let Ok(Some(file)) = run_files.next_entry().await {
            let path = file.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            if let Some(row) = parse_run_record(&peer_name, &path.to_string_lossy(), &raw) {
                rows.push(row);
            }
        }
    }
    rows.sort_by(|a, b| {
        b.timestamp
            .partial_cmp(&a.timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(rows)
}

/// Parse one ansible-pull run record into a [`RunRow`].
///
/// The JSON shape matches the v1.x Python `RunRecord` dataclass:
/// `{ timestamp, playbook, exit_code, changed, ok, failed,
///   duration_s, log_tail, triggered_by }`. Returns `None`
/// when the input isn't a JSON object (so corrupt files don't
/// crash the panel) — the row count just falls by one.
#[must_use]
pub fn parse_run_record(peer: &str, path: &str, raw: &str) -> Option<RunRow> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = v.as_object()?;
    Some(RunRow {
        peer: peer.to_string(),
        playbook: obj
            .get("playbook")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)")
            .to_string(),
        timestamp: obj.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0),
        exit_code: obj
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .min(i64::from(i32::MAX)) as i32,
        changed: obj
            .get("changed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32,
        ok_count: obj
            .get("ok")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32,
        failed: obj
            .get("failed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32,
        triggered_by: obj
            .get("triggered_by")
            .and_then(|v| v.as_str())
            .unwrap_or("pull")
            .to_string(),
        raw_json: raw.to_string(),
        path: path.to_string(),
    })
}

/// Format a unix timestamp as `YYYY-MM-DD HH:MM` UTC. Returns
/// `"-"` for zero / negative timestamps so the table doesn't
/// render the unix epoch as a "real" run time.
fn format_ts(ts: f64) -> String {
    if ts <= 0.0 {
        return "-".to_string();
    }
    let secs = ts as i64;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    // Naïve epoch-to-date — good enough for a sort-by-time
    // panel. The full chrono dep brings in tz data we don't
    // otherwise consume; the panel renders the operator's
    // own runlog so the absolute calendar accuracy isn't
    // load-bearing.
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}Z")
}

/// Convert a count of days since Unix epoch (1970-01-01) into
/// `(year, month, day)`. Civil-from-days algorithm by Howard
/// Hinnant — handles the full proleptic Gregorian range.
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "timestamp": 1715000000.0,
        "playbook": "apps-install",
        "exit_code": 0,
        "changed": 7,
        "ok": 42,
        "failed": 0,
        "duration_s": 38.4,
        "log_tail": "TASK [apps : install] ok",
        "triggered_by": "pull"
    }"#;

    #[test]
    fn parse_run_record_extracts_every_field() {
        let row = parse_run_record("alpha", "/tmp/a.json", SAMPLE).unwrap();
        assert_eq!(row.peer, "alpha");
        assert_eq!(row.playbook, "apps-install");
        assert_eq!(row.exit_code, 0);
        assert_eq!(row.changed, 7);
        assert_eq!(row.ok_count, 42);
        assert_eq!(row.failed, 0);
        assert_eq!(row.triggered_by, "pull");
        assert!(row.raw_json.contains("apps-install"));
        assert_eq!(row.path, "/tmp/a.json");
    }

    #[test]
    fn parse_run_record_handles_missing_optional_fields() {
        let row = parse_run_record("alpha", "/tmp/x.json", "{}").unwrap();
        assert_eq!(row.playbook, "(unknown)");
        assert_eq!(row.exit_code, 0);
        assert_eq!(row.triggered_by, "pull");
    }

    #[test]
    fn parse_run_record_rejects_non_object() {
        assert!(parse_run_record("a", "x", "[]").is_none());
        assert!(parse_run_record("a", "x", "not json").is_none());
        assert!(parse_run_record("a", "x", "42").is_none());
    }

    #[test]
    fn format_ts_renders_epoch_zero_as_dash() {
        assert_eq!(format_ts(0.0), "-");
        assert_eq!(format_ts(-1.0), "-");
    }

    #[test]
    fn format_ts_renders_known_timestamp() {
        // 1715000000 → 2024-05-06 16:53 UTC (approximately;
        // the algorithm rounds to minute boundaries).
        let out = format_ts(1_715_000_000.0);
        assert!(out.starts_with("2024-05-06"), "got: {out}");
        assert!(out.ends_with("Z"));
    }

    #[test]
    fn days_to_ymd_known_anchor_dates() {
        // 0 days since epoch = 1970-01-01.
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        // 19_723 = 2024-01-01.
        assert_eq!(days_to_ymd(19_723), (2024, 1, 1));
    }

    #[test]
    fn loaded_message_records_rows_and_clears_status() {
        let mut panel = RunHistoryPanel::new();
        panel.status = "stale".into();
        let rows = vec![parse_run_record("a", "x", SAMPLE).unwrap()];
        let _ = panel.update(Message::Loaded(rows.clone()));
        assert_eq!(panel.rows, rows);
        assert!(panel.status.is_empty());
    }

    #[test]
    fn focus_row_sets_focused_path() {
        let mut panel = RunHistoryPanel::new();
        let _ = panel.update(Message::FocusRow("/tmp/a.json".into()));
        assert_eq!(panel.focused_path.as_deref(), Some("/tmp/a.json"));
    }

    #[test]
    fn back_clears_focused_path() {
        let mut panel = RunHistoryPanel::new();
        panel.focused_path = Some("/tmp/a.json".into());
        let _ = panel.update(Message::Back);
        assert!(panel.focused_path.is_none());
    }

    #[test]
    fn error_message_stores_status() {
        let mut panel = RunHistoryPanel::new();
        let _ = panel.update(Message::Error("dir unreadable".into()));
        assert_eq!(panel.status, "dir unreadable");
    }

    #[tokio::test]
    async fn collect_runs_missing_dir_returns_ok_empty() {
        let bogus = PathBuf::from("/nonexistent-run-history-7234923");
        let rows = collect_runs(&bogus).await.unwrap();
        assert!(rows.is_empty());
    }
}
