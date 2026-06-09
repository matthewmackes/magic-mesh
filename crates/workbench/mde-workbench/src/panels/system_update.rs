//! Maintain → System Update panel — `dnf upgrade` wrapper.
//!
//! CB-1.7 (shipped 2026-05-21): live-streams dnf stdout into
//! the panel via `iced::Task::stream` + an `async_stream!`
//! macro. The Check / Install actions now emit per-line
//! `Message::OutputLine(line)` events as dnf prints them, and
//! a terminal `Message::Finished` event when the subprocess
//! exits. The v1.x GLib io-watch pattern ports to Iced's
//! native Stream-of-Messages abstraction without needing a
//! separate channel + Subscription poll.

use std::process::Stdio;

use async_stream::stream;
use futures::stream::{Stream, StreamExt};
use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Padding, Task};
use mde_theme::Palette;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default)]
pub struct SystemUpdatePanel {
    pub summary: String,
    pub output: String,
    pub busy: bool,
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    SummaryLoaded(String),
    CheckClicked,
    InstallClicked,
    /// One line of streamed subprocess output — appended to the
    /// panel's `output` buffer.
    OutputLine(String),
    /// Terminal event for a streamed run — fires once the
    /// subprocess exits.
    Finished {
        argv: String,
        success: bool,
        output: String,
    },
    Error(String),
}

impl SystemUpdatePanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            summary: "(checking…)".into(),
            ..Self::default()
        }
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move { Message::SummaryLoaded(read_summary().await) },
            crate::Message::SystemUpdate,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::SummaryLoaded(s) => {
                self.summary = s;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::CheckClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Checking for updates…".into();
                self.output.clear();
                Task::stream(stream_subprocess(
                    "dnf check-update".to_string(),
                    vec!["dnf".into(), "check-update".into()],
                ))
                .map(crate::Message::SystemUpdate)
            }
            Message::InstallClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Installing updates (polkit will prompt for your password)…".into();
                self.output.clear();
                Task::stream(stream_subprocess(
                    "pkexec dnf upgrade -y --refresh".to_string(),
                    vec![
                        "pkexec".into(),
                        "dnf".into(),
                        "upgrade".into(),
                        "-y".into(),
                        "--refresh".into(),
                    ],
                ))
                .map(crate::Message::SystemUpdate)
            }
            Message::OutputLine(line) => {
                self.output.push_str(&line);
                self.output.push('\n');
                Task::none()
            }
            Message::Finished {
                argv,
                success,
                output,
            } => {
                self.busy = false;
                self.output = output;
                self.status = if success {
                    format!("{argv}: ok")
                } else {
                    format!("{argv}: failed (see output)")
                };
                // Refresh the summary line so a successful upgrade
                // shows "(up to date)" without a manual reload.
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        // UX-7.a — Check (Ghost) + Install (Primary) routed
        // through the shared button variants.
        let palette = Palette::dark();
        let check_btn = variant_button(
            "Check for updates",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::SystemUpdate(Message::CheckClicked)),
            palette,
        );
        let install_btn = variant_button(
            "Install all updates",
            ButtonVariant::Primary,
            (!self.busy).then(|| crate::Message::SystemUpdate(Message::InstallClicked)),
            palette,
        );

        column![
            text("System Update").size(20),
            text(
                "Install the latest fixes and updates for your machine. \
                 This may take a few minutes."
            )
            .size(13),
            text(&self.summary).size(13),
            row![check_btn, install_btn].spacing(12),
            text("Output").size(16),
            scrollable(
                container(text(&self.output).size(12))
                    .padding(Padding::new(12.0))
                    .width(Length::Fill),
            )
            .height(Length::Fixed(320.0)),
            text(&self.status).size(13),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Cheap startup summary — runs `dnf check-update --quiet`
/// and counts the update-eligible package lines. The first
/// "==" header line + blank line are skipped; everything
/// after that is a package row.
async fn read_summary() -> String {
    let (success, out) = run_capture(&["dnf", "check-update", "--quiet"]).await;
    // dnf check-update exits with code 100 when updates are
    // available, 0 when up to date, non-{0,100} on error.
    // run_capture coalesces to (success_bool, output) — we
    // can't tell 100 vs 0 from a bool, so we count package
    // lines instead.
    let count = summarise_check_update(&out);
    if count == 0 {
        if success {
            "(up to date)".into()
        } else {
            "(could not check — dnf returned no parseable output)".into()
        }
    } else {
        format!("{count} package(s) available to update")
    }
}

/// Pure helper for the summary line. Counts lines that look
/// like a package update row (3+ whitespace-separated columns:
/// `<name>.<arch>` `<version>` `<repo>`). Skips header/blank
/// lines + everything inside an `Obsoleting Packages` block
/// (those rows still parse as packages but represent the
/// obsoletion graph, not user-facing updates).
#[must_use]
pub fn summarise_check_update(output: &str) -> usize {
    let mut count = 0;
    let mut in_obsoleting = false;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Obsoleting") {
            in_obsoleting = true;
            continue;
        }
        if in_obsoleting {
            // The obsoletion block ends at the next blank line
            // or top-level header (no leading whitespace, no
            // `.` in column 0).
            if trimmed.is_empty() {
                in_obsoleting = false;
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() >= 3 && cols[0].contains('.') {
            count += 1;
        }
    }
    count
}

/// Stream subprocess stdout line-by-line. Yields one
/// `Message::OutputLine(line)` per stdout line, plus a terminal
/// `Message::Finished { argv, success, output }` when the process
/// exits. Spawn failures yield a single `Message::Error(...)`.
///
/// `argv_display` is the human-readable command form for the
/// terminal `Finished.argv` field; `argv` is the actual spawn
/// vector.
fn stream_subprocess(
    argv_display: String,
    argv: Vec<String>,
) -> impl Stream<Item = Message> + Send + 'static {
    stream! {
        let Some((bin, args)) = argv.split_first() else {
            yield Message::Error("empty command".into());
            return;
        };
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                yield Message::Error(format!("{bin} failed to spawn: {e}"));
                return;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let mut combined = String::new();
        if let Some(out) = stdout {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                combined.push_str(&line);
                combined.push('\n');
                yield Message::OutputLine(line);
            }
        }
        if let Some(err) = stderr {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                combined.push_str(&line);
                combined.push('\n');
                yield Message::OutputLine(format!("[stderr] {line}"));
            }
        }
        let status = child.wait().await;
        let success = status.as_ref().map(|s| s.success()).unwrap_or(false);
        yield Message::Finished {
            argv: argv_display,
            success,
            output: combined,
        };
    }
}

/// Convenience — collect a stream's items into a Vec. Useful for
/// integration tests; not used by the live panel.
pub async fn collect_stream<S: Stream<Item = Message> + Unpin>(mut s: S) -> Vec<Message> {
    let mut out = Vec::new();
    while let Some(msg) = s.next().await {
        out.push(msg);
    }
    out
}

/// Run a command to completion, capturing stdout+stderr. Returns
/// `(success, combined_output)`. Empty output on launch failure.
#[allow(dead_code)]
async fn run_capture(argv: &[&str]) -> (bool, String) {
    let Some((bin, args)) = argv.split_first() else {
        return (false, "empty command".into());
    };
    let Ok(output) = Command::new(bin).args(args).output().await else {
        return (false, format!("{bin} not found on PATH"));
    };
    let mut combined = String::from_utf8(output.stdout).unwrap_or_default();
    let stderr = String::from_utf8(output.stderr).unwrap_or_default();
    if !stderr.is_empty() {
        if !combined.ends_with('\n') && !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    (output.status.success(), combined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarise_check_update_counts_update_rows() {
        let out = "\
Last metadata expiration check: 0:01:00 ago.

firefox.x86_64            128.0.1-1.fc44      updates
kernel.x86_64             6.10.0-1.fc44       updates
glibc.x86_64              2.39-5.fc44         updates
";
        assert_eq!(summarise_check_update(out), 3);
    }

    #[test]
    fn summarise_check_update_zero_when_up_to_date() {
        assert_eq!(summarise_check_update(""), 0);
        assert_eq!(
            summarise_check_update("Last metadata expiration check: now.\n"),
            0
        );
    }

    #[test]
    fn summarise_check_update_skips_obsoleting_block() {
        let out = "\
firefox.x86_64    128.0.1-1.fc44   updates

Obsoleting Packages
  oldpkg.noarch  1.0-1.fc43       updates
      replaces:  oldpkg.noarch    1.0-1.fc42
";
        // Only the real firefox row should count.
        assert_eq!(summarise_check_update(out), 1);
    }

    #[test]
    fn loaded_summary_replaces_initial_checking_placeholder() {
        let mut panel = SystemUpdatePanel::new();
        assert!(panel.summary.contains("checking"));
        let _ = panel.update(Message::SummaryLoaded("(up to date)".into()));
        assert_eq!(panel.summary, "(up to date)");
    }

    #[test]
    fn check_clicked_while_busy_is_noop() {
        let mut panel = SystemUpdatePanel::new();
        panel.busy = true;
        panel.status = "Checking…".into();
        let _ = panel.update(Message::CheckClicked);
        assert_eq!(panel.status, "Checking…");
    }

    #[test]
    fn install_clicked_while_busy_is_noop() {
        let mut panel = SystemUpdatePanel::new();
        panel.busy = true;
        panel.status = "Installing…".into();
        let _ = panel.update(Message::InstallClicked);
        assert_eq!(panel.status, "Installing…");
    }

    #[test]
    fn finished_success_records_ok_status_and_clears_busy() {
        let mut panel = SystemUpdatePanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Finished {
            argv: "dnf check-update".into(),
            success: true,
            output: "firefox.x86_64    1.0    updates".into(),
        });
        assert!(!panel.busy);
        assert!(panel.status.contains("ok"));
        assert!(panel.output.contains("firefox"));
    }

    #[test]
    fn finished_failure_includes_failed_marker() {
        let mut panel = SystemUpdatePanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Finished {
            argv: "pkexec dnf upgrade".into(),
            success: false,
            output: "polkit denied".into(),
        });
        assert!(panel.status.contains("failed"));
        assert!(panel.output.contains("polkit"));
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = SystemUpdatePanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("dnf not found".into()));
        assert_eq!(panel.status, "dnf not found");
        assert!(!panel.busy);
    }

    #[test]
    fn output_line_appends_to_buffer() {
        let mut panel = SystemUpdatePanel::new();
        panel.output.clear();
        let _ = panel.update(Message::OutputLine("downloading...".into()));
        assert!(panel.output.contains("downloading..."));
        assert!(panel.output.ends_with('\n'));
    }

    #[test]
    fn output_line_accumulates_across_calls() {
        let mut panel = SystemUpdatePanel::new();
        panel.output.clear();
        let _ = panel.update(Message::OutputLine("line 1".into()));
        let _ = panel.update(Message::OutputLine("line 2".into()));
        assert!(panel.output.contains("line 1"));
        assert!(panel.output.contains("line 2"));
    }

    #[tokio::test]
    async fn stream_subprocess_emits_finished_with_error_on_missing_binary() {
        let stream = stream_subprocess("ghost".into(), vec!["/definitely-not-a-binary".into()]);
        let pinned = Box::pin(stream);
        let messages: Vec<Message> = collect_stream(pinned).await;
        assert!(!messages.is_empty());
        // First (and only) message should be an Error.
        assert!(matches!(messages[0], Message::Error(_)));
    }

    #[tokio::test]
    async fn stream_subprocess_yields_lines_then_finished() {
        // `printf "a\nb\nc\n"` exits 0; stream should yield 3
        // OutputLine + 1 Finished.
        let stream = stream_subprocess("printf".into(), vec!["printf".into(), "a\nb\nc\n".into()]);
        let pinned = Box::pin(stream);
        let messages: Vec<Message> = collect_stream(pinned).await;
        let lines: Vec<&str> = messages
            .iter()
            .filter_map(|m| match m {
                Message::OutputLine(l) => Some(l.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec!["a", "b", "c"]);
        assert!(matches!(
            messages.last(),
            Some(Message::Finished { success: true, .. })
        ));
    }

    #[tokio::test]
    async fn stream_subprocess_empty_argv_yields_error() {
        let stream = stream_subprocess("(empty)".into(), vec![]);
        let pinned = Box::pin(stream);
        let messages: Vec<Message> = collect_stream(pinned).await;
        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0], Message::Error(_)));
    }
}
