//! PLANES-14 — the **Fleet logs search** panel (Controller plane),
//! absorbing OBS-5.
//!
//! Controller-side search over the OBS-5 mesh-replicated structured logs
//! (W15): every node appends its records to `<root>/logs/<host>.jsonl`
//! (the magic_fleet::structured_log engine); this panel reads them all,
//! filters by a free-text query + a minimum level, and lists matches
//! newest-first. Reads the store directly (local row + filter — the
//! established panel pattern, no new cross-crate dep).
//!
//! Build-now-defer-visual: the load + filter are pure + unit-tested; the
//! on-Cosmic `/preview` is the deferred tail.

use std::path::{Path, PathBuf};

use iced::widget::{column, row, scrollable, text, text_input};
use iced::{Element, Length, Task};
use serde::Deserialize;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{panel_container, status_badge, BadgeSeverity};

/// One structured log record (mirrors magic_fleet::structured_log::LogRecord).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LogRow {
    #[serde(default)]
    pub ts_ms: u64,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub message: String,
}

#[must_use]
fn level_rank(level: &str) -> u8 {
    match level.to_ascii_lowercase().as_str() {
        "error" => 4,
        "warn" | "warning" => 3,
        "info" => 2,
        "debug" => 1,
        "trace" => 0,
        _ => 2,
    }
}

/// `MDE_WORKGROUP_ROOT`-or-`/mnt/mesh-storage`.
#[must_use]
pub fn workgroup_root() -> PathBuf {
    std::env::var_os("MDE_WORKGROUP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/mesh-storage"))
}

/// Read every `logs/<host>.jsonl` record under `root` (junk-tolerant).
#[must_use]
pub fn load_all(root: &Path) -> Vec<LogRow> {
    let dir = root.join("logs");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .flat_map(|raw| {
            raw.lines()
                .filter_map(|l| serde_json::from_str::<LogRow>(l).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Filter `rows` by a min-level + a case-insensitive query over message OR
/// target, newest-first, capped at `limit`. Pure.
#[must_use]
pub fn filter_rows(rows: &[LogRow], min_level: &str, query: &str, limit: usize) -> Vec<LogRow> {
    let min = level_rank(min_level);
    let q = query.to_ascii_lowercase();
    let mut out: Vec<LogRow> = rows
        .iter()
        .filter(|r| level_rank(&r.level) >= min)
        .filter(|r| {
            q.is_empty()
                || r.message.to_ascii_lowercase().contains(&q)
                || r.target.to_ascii_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms));
    out.truncate(limit);
    out
}

/// Max rows rendered.
pub const MAX_ROWS: usize = 500;

/// Min-level filter options.
const LEVELS: [&str; 4] = ["trace", "info", "warn", "error"];

/// The Fleet-logs panel state.
#[derive(Debug, Clone)]
pub struct FleetLogsPanel {
    all: Vec<LogRow>,
    pub query: String,
    pub min_level: String,
    pub loaded: bool,
    pub busy: bool,
}

impl Default for FleetLogsPanel {
    fn default() -> Self {
        Self {
            all: Vec::new(),
            query: String::new(),
            min_level: "info".into(),
            loaded: false,
            busy: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<LogRow>),
    QueryChanged(String),
    SetLevel(&'static str),
    RefreshClicked,
}

impl FleetLogsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move { Message::Loaded(load_all(&workgroup_root())) },
            crate::Message::FleetLogs,
        )
    }

    /// The currently-visible (filtered) rows.
    #[must_use]
    pub fn visible(&self) -> Vec<LogRow> {
        filter_rows(&self.all, &self.min_level, &self.query, MAX_ROWS)
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(rows) => {
                self.all = rows;
                self.loaded = true;
                self.busy = false;
                Task::none()
            }
            Message::QueryChanged(q) => {
                self.query = q;
                Task::none()
            }
            Message::SetLevel(l) => {
                self.min_level = l.to_string();
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;

        let search = text_input("search message or target…", &self.query)
            .on_input(|q| crate::Message::FleetLogs(Message::QueryChanged(q)))
            .padding(8)
            .width(Length::Fixed(360.0));

        let mut levels = row![].spacing(6).align_y(iced::Alignment::Center);
        for l in LEVELS {
            let active = self.min_level == l;
            levels = levels.push(variant_button(
                l,
                if active {
                    ButtonVariant::Secondary
                } else {
                    ButtonVariant::Ghost
                },
                (!active).then(|| crate::Message::FleetLogs(Message::SetLevel(l))),
                palette,
            ));
        }
        let refresh = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::FleetLogs(Message::RefreshClicked)),
            palette,
        );

        let visible = self.visible();
        let mut list = column![].spacing(4);
        if visible.is_empty() {
            list = list.push(
                text(if self.loaded {
                    "No matching log records (peers append to <root>/logs/<host>.jsonl — OBS-5)."
                } else {
                    "Loading…"
                })
                .size(13),
            );
        }
        for r in &visible {
            let sev = match level_rank(&r.level) {
                4 => BadgeSeverity::Warning,
                3 => BadgeSeverity::Warning,
                _ => BadgeSeverity::Neutral,
            };
            list = list.push(
                row![
                    status_badge(r.level.clone(), sev, palette),
                    text(r.host.clone()).width(Length::Fixed(120.0)).size(12),
                    text(r.target.clone()).width(Length::Fixed(200.0)).size(12),
                    text(r.message.clone()).size(13),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            );
        }

        panel_container(
            column![
                text(format!(
                    "Fleet logs — {} record(s), {} shown",
                    self.all.len(),
                    visible.len()
                ))
                .size(20),
                row![search, levels, refresh]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                scrollable(list).height(Length::Fill),
            ]
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

    fn row(host: &str, ts: u64, level: &str, target: &str, msg: &str) -> LogRow {
        LogRow {
            ts_ms: ts,
            host: host.into(),
            level: level.into(),
            target: target.into(),
            message: msg.into(),
        }
    }

    #[test]
    fn load_all_reads_jsonl_across_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("pine.jsonl"),
            "{\"ts_ms\":1,\"host\":\"pine\",\"level\":\"info\",\"message\":\"a\"}\nbad line\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("oak.jsonl"),
            "{\"ts_ms\":2,\"host\":\"oak\",\"level\":\"error\",\"message\":\"boom\"}\n",
        )
        .unwrap();
        let rows = load_all(tmp.path());
        assert_eq!(rows.len(), 2, "two valid records, junk skipped");
    }

    #[test]
    fn filter_by_level_query_and_newest_first() {
        let rows = vec![
            row("pine", 100, "info", "net", "hello world"),
            row("pine", 300, "error", "net", "disk FULL"),
            row("oak", 200, "warn", "fw", "zone set"),
        ];
        // min_level=warn drops the info row.
        let warns = filter_rows(&rows, "warn", "", 500);
        assert_eq!(warns.len(), 2);
        assert_eq!(warns[0].ts_ms, 300, "newest first");
        // query (case-insensitive, msg or target).
        let full = filter_rows(&rows, "trace", "full", 500);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].message, "disk FULL");
        // limit.
        assert_eq!(filter_rows(&rows, "trace", "", 1).len(), 1);
    }

    #[test]
    fn query_changed_and_set_level_update_state() {
        let mut p = FleetLogsPanel::new();
        let _ = p.update(Message::QueryChanged("nebula".into()));
        assert_eq!(p.query, "nebula");
        let _ = p.update(Message::SetLevel("error"));
        assert_eq!(p.min_level, "error");
    }
}
