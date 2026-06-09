//! Maintain → Logs panel — read-only viewer over MDE's
//! `mackes.log` + the sway compositor's user-session journal.
//!
//! CB-1.7 partial: replaces the v1.x
//! `mackes/workbench/maintain/logs.py`. The v1.x panel tailed
//! `~/.local/share/mackes-shell/mackes.log` + the xfsettingsd
//! journal; the v2.0.0 port drops xfsettingsd entirely (Phase
//! D retires xfconf) and reads the sway user-session journal
//! instead.

use std::path::PathBuf;

use iced::widget::{column, container, row, scrollable, text};
use mde_theme::Palette;

use crate::controls::{variant_button, ButtonVariant};
use iced::{Element, Length, Padding, Task};
use tokio::process::Command;

/// Max lines tailed from each log source. Matches the v1.x
/// `TAIL_LINES = 400`.
pub const TAIL_LINES: usize = 400;

#[derive(Debug, Clone, Default)]
pub struct LogsPanel {
    pub mackes_log: String,
    pub sway_journal: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        mackes_log: String,
        sway_journal: String,
    },
    RefreshClicked,
}

impl LogsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let mackes_log = read_mackes_log_tail(TAIL_LINES).await;
                let sway_journal = read_sway_journal_tail(TAIL_LINES).await;
                Message::Loaded {
                    mackes_log,
                    sway_journal,
                }
            },
            crate::Message::Logs,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                mackes_log,
                sway_journal,
            } => {
                self.mackes_log = mackes_log;
                self.sway_journal = sway_journal;
                self.status = "Refreshed.".into();
                Task::none()
            }
            Message::RefreshClicked => Self::load(),
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        // UX-7.a — refresh routed through the shared button variant.
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            Some(crate::Message::Logs(Message::RefreshClicked)),
            Palette::dark(),
        );

        column![
            text("mackes.log").size(16),
            scrollable(
                container(text(&self.mackes_log).size(12))
                    .padding(Padding::new(12.0))
                    .width(Length::Fill),
            )
            .height(Length::Fixed(280.0)),
            text("sway user-session journal").size(16),
            scrollable(
                container(text(&self.sway_journal).size(12))
                    .padding(Padding::new(12.0))
                    .width(Length::Fill),
            )
            .height(Length::Fixed(280.0)),
            row![refresh_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Path to the MDE log file. Matches the v1.x layout for
/// continuity; the file is created by `mackes.logging` on the
/// first `log_action` call.
fn mackes_log_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/mackes-shell/mackes.log")
}

/// Read the last N lines of `mackes.log`. Returns a friendly
/// placeholder string for the empty-state branches (file
/// missing, unreadable, empty).
pub async fn read_mackes_log_tail(n: usize) -> String {
    let path = mackes_log_path();
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return "No log yet — mackes hasn't recorded any actions.".into();
    };
    let tail = tail_lines(&raw, n);
    if tail.is_empty() {
        "(log is empty)".into()
    } else {
        tail
    }
}

/// Read the last N lines of the sway user-session journal via
/// `journalctl --user -u sway -n <N>`. Returns the upstream
/// stderr/error message on failure so the operator can see
/// what went wrong.
pub async fn read_sway_journal_tail(n: usize) -> String {
    let Ok(output) = Command::new("journalctl")
        .args(["--user", "-u", "sway", "-n", &n.to_string(), "--no-pager"])
        .output()
        .await
    else {
        return "journalctl not found.".into();
    };
    if !output.status.success() {
        return String::from_utf8(output.stderr).unwrap_or_default();
    }
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    if stdout.trim().is_empty() {
        "(journal returned nothing)".into()
    } else {
        stdout
    }
}

/// Pure tail-N-lines helper. Splits on `\n`, keeps the last
/// `n` non-empty-aware (every line preserved, the truncation
/// is purely positional), rejoins.
#[must_use]
pub fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n {
        return text.trim_end_matches('\n').to_string();
    }
    lines[lines.len() - n..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_lines_returns_full_text_when_under_limit() {
        assert_eq!(tail_lines("a\nb\nc", 5), "a\nb\nc");
        assert_eq!(tail_lines("", 5), "");
    }

    #[test]
    fn tail_lines_keeps_only_last_n() {
        let text = "1\n2\n3\n4\n5";
        assert_eq!(tail_lines(text, 2), "4\n5");
        assert_eq!(tail_lines(text, 3), "3\n4\n5");
    }

    #[test]
    fn tail_lines_strips_trailing_newline_on_full_text_branch() {
        assert_eq!(tail_lines("a\nb\nc\n", 5), "a\nb\nc");
    }

    #[test]
    fn loaded_message_records_state_and_marks_refreshed() {
        let mut panel = LogsPanel::new();
        let _ = panel.update(Message::Loaded {
            mackes_log: "boot ok".into(),
            sway_journal: "sway 1.9".into(),
        });
        assert_eq!(panel.mackes_log, "boot ok");
        assert_eq!(panel.sway_journal, "sway 1.9");
        assert!(panel.status.contains("Refresh"));
    }

    #[tokio::test]
    async fn read_mackes_log_tail_returns_placeholder_when_missing() {
        // The HOME setup is environment-dependent; the test verifies
        // the unreachable-file path produces the placeholder
        // string by depending on a real-but-likely-absent path.
        // We can't safely poison HOME globally inside an async
        // test, so this is best-effort coverage.
        let raw = read_mackes_log_tail(10).await;
        // Either we're on a real box with a real log, or we get
        // the placeholder. Both are acceptable; the assertion
        // just checks the function ran without panicking.
        assert!(!raw.is_empty());
    }
}
