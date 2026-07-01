//! Maintain → Repair panel — one-click recovery actions for
//! common MDE problems.
//!
//! CB-1.7 partial: replaces the v1.x
//! `mackes/workbench/maintain/repair.py`. The v1.x panel ran
//! 4 XFCE-era actions (re-apply preset, rebuild menu folder,
//! restore xfce4-settings entries, re-install Mackes
//! .desktop); v2.0.0 retires all four target surfaces. The
//! Iced port ships a reframed action set against the
//! v2.0.0 MDE stack:
//!
//!   * Restart mackesd (kicks the user systemd unit if a worker
//!     wedged)
//!   * Re-install the MDE .desktop launcher (copies the
//!     system-wide entry under
//!     `/usr/share/applications/mde.desktop` into
//!     `~/.local/share/applications/` so a per-user override
//!     in that dir is reset to the canonical version)
//!   * Restore my preset (snapshot the current configuration,
//!     then re-apply the active preset across every section
//!     via `mackes snapshot create` + `mackes maintain reset`)
//!
//! The first three are safe + idempotent. Restore-my-preset
//! overwrites your settings to match the active preset, so it
//! snapshots the current configuration first — roll back from
//! Maintain → Snapshots if needed. The panel runs each action
//! one at a time with a per-row button.
//!
//! Restore-my-preset absorbs
//! `EPIC-RETIRE-PY-WORKBENCH.port-reset-to-preset`: the v1.x
//! `maintain/reset_to_preset.py` panel folds in here as a
//! single recovery action (the v1.x picker + per-section
//! toggles are the wizard's domain — Repair restores the
//! preset the operator is already on).

use cosmic::iced::widget::{column, container, row, scrollable, text};
use cosmic::iced::{Element, Length, Padding, Task};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default)]
pub struct RepairPanel {
    pub output: String,
    pub busy: bool,
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    RestartMackesdClicked,
    ReinstallDesktopClicked,
    RestorePresetClicked,
    Finished { argv: String, output: String },
}

impl RepairPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::RestartMackesdClicked => self.dispatch(
                "systemctl --user restart mackesd",
                vec!["systemctl", "--user", "restart", "mackesd"],
            ),
            Message::ReinstallDesktopClicked => self.dispatch_async_fn("reinstall mde.desktop"),
            Message::RestorePresetClicked => self.dispatch_restore("restore my preset"),
            Message::Finished { argv, output } => {
                self.busy = false;
                self.output = output;
                self.status = format!("{argv}: done");
                Task::none()
            }
        }
    }

    fn dispatch(&mut self, label: &str, argv: Vec<&'static str>) -> Task<crate::Message> {
        if self.busy {
            return Task::none();
        }
        self.busy = true;
        self.status = format!("Running {label}…");
        let label_owned = label.to_string();
        let argv_owned: Vec<String> = argv.into_iter().map(String::from).collect();
        Task::perform(
            async move {
                let output = run_capture(&argv_owned).await;
                Message::Finished {
                    argv: label_owned,
                    output,
                }
            },
            crate::Message::Repair,
        )
    }

    fn dispatch_async_fn(&mut self, label: &str) -> Task<crate::Message> {
        if self.busy {
            return Task::none();
        }
        self.busy = true;
        self.status = format!("Running {label}…");
        let label_owned = label.to_string();
        Task::perform(
            async move {
                let output = reinstall_mde_desktop().await;
                Message::Finished {
                    argv: label_owned,
                    output,
                }
            },
            crate::Message::Repair,
        )
    }

    fn dispatch_restore(&mut self, label: &str) -> Task<crate::Message> {
        if self.busy {
            return Task::none();
        }
        self.busy = true;
        self.status = format!("Running {label}…");
        let label_owned = label.to_string();
        Task::perform(
            async move {
                let output = restore_active_preset().await;
                Message::Finished {
                    argv: label_owned,
                    output,
                }
            },
            crate::Message::Repair,
        )
    }

    pub fn view(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        // UX-7.a — three repair actions routed through Secondary
        // (none of them is THE primary action; user picks based
        // on the problem).
        let palette = crate::live_theme::palette();
        let restart_btn = variant_button(
            "Restart mackesd",
            ButtonVariant::Secondary,
            (!self.busy).then(|| crate::Message::Repair(Message::RestartMackesdClicked)),
            palette,
        );
        let reinstall_btn = variant_button(
            "Re-install MDE launcher",
            ButtonVariant::Secondary,
            (!self.busy).then(|| crate::Message::Repair(Message::ReinstallDesktopClicked)),
            palette,
        );
        let restore_btn = variant_button(
            "Restore my preset",
            ButtonVariant::Secondary,
            (!self.busy).then(|| crate::Message::Repair(Message::RestorePresetClicked)),
            palette,
        );

        // CTRLSURF-8 — the page title ("Repair") is already rendered once by
        // `app.rs` (breadcrumb + `page_title`); the panel no longer repeats it.
        column![
            text(
                "Safe one-click fixes for common MDE problems. Each repair \
                 runs on its own — none of them touch your personal files."
            )
            .size(13),
            row![
                column![
                    text("Restart mackesd").size(14),
                    text("Kicks the user systemd unit when a mackesd worker wedges.").size(12)
                ]
                .spacing(2)
                .width(Length::Fill),
                restart_btn,
            ]
            .spacing(12),
            row![
                column![
                    text("Re-install MDE launcher").size(14),
                    text("Refreshes the .desktop entry under ~/.local/share/applications/.")
                        .size(12)
                ]
                .spacing(2)
                .width(Length::Fill),
                reinstall_btn,
            ]
            .spacing(12),
            row![
                column![
                    text("Restore my preset").size(14),
                    text(
                        "Snapshot your current settings, then re-apply your active \
                         preset across every section. Roll back from Maintain → Snapshots."
                    )
                    .size(12)
                ]
                .spacing(2)
                .width(Length::Fill),
                restore_btn,
            ]
            .spacing(12),
            text("Output").size(14),
            // CTRLSURF-8 — the output box flexes to fill the remaining height
            // instead of a hardcoded 220 px band (which left dead space when
            // the output was short and clipped it when long).
            scrollable(
                container(text(&self.output).size(12))
                    .padding(Padding::new(12.0))
                    .width(Length::Fill),
            )
            .height(Length::Fill),
            text(&self.status).size(13),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

async fn run_capture(argv: &[String]) -> String {
    let Some((bin, args)) = argv.split_first() else {
        return "empty command".into();
    };
    let Ok(output) = Command::new(bin).args(args).output().await else {
        return format!("{bin} not found on PATH");
    };
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    let stderr = String::from_utf8(output.stderr).unwrap_or_default();
    let mut combined = String::new();
    if !stdout.is_empty() {
        combined.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    if combined.is_empty() {
        format!("(exit {:?})", output.status.code())
    } else {
        combined
    }
}

/// Re-install the per-user `mde.desktop` launcher. Walks the
/// known system-wide locations, copies the first one found to
/// `~/.local/share/applications/mde.desktop`. Returns a
/// human-readable status message.
async fn reinstall_mde_desktop() -> String {
    let candidates = [
        "/usr/share/applications/mde.desktop",
        "/usr/local/share/applications/mde.desktop",
        // Legacy fallback during the rebrand window.
        "/usr/share/applications/mackes-shell.desktop",
    ];
    let Some(src) = candidates.iter().find(|p| std::path::Path::new(p).exists()) else {
        return "no canonical mde.desktop found in /usr/share/applications/.".into();
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let dst_dir = std::path::Path::new(&home).join(".local/share/applications");
    let dst = dst_dir.join("mde.desktop");
    if let Err(e) = tokio::fs::create_dir_all(&dst_dir).await {
        return format!("creating {}: {e}", dst_dir.display());
    }
    match tokio::fs::copy(src, &dst).await {
        Ok(_) => format!("copied {src} → {}", dst.display()),
        Err(e) => format!("copy {src} → {} failed: {e}", dst.display()),
    }
}

/// Snapshot the current configuration, then re-apply the
/// active preset across every section. Mirrors the v1.x
/// Maintain → Reset to Preset panel: a snapshot is taken
/// first (so the operator can roll back from Maintain →
/// Snapshots), then `mackes maintain reset` re-applies the
/// active preset. Returns the combined snapshot + apply
/// output. If no active preset is set, `mackes maintain reset`
/// exits non-zero and the message surfaces in the output.
async fn restore_active_preset() -> String {
    let snap = run_capture(&[
        "mackes".into(),
        "snapshot".into(),
        "create".into(),
        "pre-restore-preset".into(),
    ])
    .await;
    let apply = run_capture(&["mackes".into(), "maintain".into(), "reset".into()]).await;
    format!("snapshot: {snap}\n\n{apply}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_panel_starts_idle() {
        let panel = RepairPanel::new();
        assert!(!panel.busy);
        assert!(panel.status.is_empty());
        assert!(panel.output.is_empty());
    }

    #[test]
    fn restart_mackesd_clicked_sets_busy_and_status() {
        let mut panel = RepairPanel::new();
        let _ = panel.update(Message::RestartMackesdClicked);
        assert!(panel.busy);
        assert!(panel.status.contains("systemctl"));
    }

    #[test]
    fn reinstall_clicked_sets_busy_and_status() {
        let mut panel = RepairPanel::new();
        let _ = panel.update(Message::ReinstallDesktopClicked);
        assert!(panel.busy);
        assert!(panel.status.contains("mde.desktop"));
    }

    #[test]
    fn restore_preset_clicked_sets_busy_and_status() {
        let mut panel = RepairPanel::new();
        let _ = panel.update(Message::RestorePresetClicked);
        assert!(panel.busy);
        assert!(panel.status.contains("restore my preset"));
    }

    #[test]
    fn restore_preset_while_busy_is_noop() {
        let mut panel = RepairPanel::new();
        panel.busy = true;
        panel.status = "Running …".into();
        let _ = panel.update(Message::RestorePresetClicked);
        assert_eq!(panel.status, "Running …");
    }

    #[test]
    fn second_click_while_busy_is_noop() {
        let mut panel = RepairPanel::new();
        panel.busy = true;
        panel.status = "Running …".into();
        let _ = panel.update(Message::RestartMackesdClicked);
        assert_eq!(panel.status, "Running …");
    }

    #[test]
    fn finished_clears_busy_and_records_output() {
        let mut panel = RepairPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Finished {
            argv: "systemctl --user restart mackesd".into(),
            output: "ok".into(),
        });
        assert!(!panel.busy);
        assert!(panel.status.contains("done"));
        assert_eq!(panel.output, "ok");
    }

    #[tokio::test]
    async fn run_capture_returns_friendly_message_for_missing_binary() {
        let out = run_capture(&["/nonexistent-mde-test-binary-7234923".into()]).await;
        assert!(out.contains("not found"));
    }
}
