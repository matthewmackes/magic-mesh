//! Apps → Installed panel — searchable RPM list with
//! per-row remove (under pkexec).
//!
//! CB-1.3 partial: replaces the v1.x
//! `mackes/workbench/apps/installed.py`. The Python panel used
//! `mackes.app_mgmt.list_installed_packages` (rpm -qa wrapper)
//! + `remove_packages` (sudo dnf remove); the Iced port shells
//! out to `rpm -qa --queryformat=...` and `pkexec dnf remove
//! <name>` directly so it doesn't need to depend on the v1.x
//! Python library through the rebrand window.

use iced::widget::{column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Task};
use mde_theme::Palette;
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageRow {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default)]
pub struct AppsInstalledPanel {
    pub packages: Vec<PackageRow>,
    pub filter: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<PackageRow>),
    Error(String),
    FilterChanged(String),
    RemoveClicked(String),
    RemoveFinished { name: String, success: bool },
    RefreshClicked,
}

impl AppsInstalledPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let raw = run_rpm_qa().await;
                Message::Loaded(parse_rpm_qa(&raw))
            },
            crate::Message::AppsInstalled,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(rows) => {
                self.packages = rows;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::FilterChanged(v) => {
                self.filter = v;
                Task::none()
            }
            Message::RemoveClicked(name) => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Removing {name} (polkit will prompt)…");
                Task::perform(
                    async move {
                        let success = run_pkexec_dnf_remove(&name).await;
                        Message::RemoveFinished { name, success }
                    },
                    crate::Message::AppsInstalled,
                )
            }
            Message::RemoveFinished { name, success } => {
                self.status = if success {
                    format!("Removed {name}.")
                } else {
                    format!("Removing {name} failed (see journalctl).")
                };
                self.busy = false;
                // Reload to drop the removed row from the list.
                Self::load()
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
        let filter_input = text_input("Filter…", &self.filter)
            .on_input(|v| crate::Message::AppsInstalled(Message::FilterChanged(v)));
        // UX-7.a — refresh routed through the shared button variant.
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::AppsInstalled(Message::RefreshClicked)),
            Palette::dark(),
        );

        let filtered: Vec<&PackageRow> = self
            .packages
            .iter()
            .filter(|p| matches_filter(&p.name, &self.filter))
            .collect();

        let rows_view = filtered.iter().fold(column![], |col, p| {
            let name = p.name.clone();
            // UX-7.a — per-row Remove routed through Ghost
            // (destructive — but Secondary feels too prominent
            // for a removal-from-list affordance).
            let remove_btn = variant_button(
                "Remove",
                ButtonVariant::Ghost,
                (!self.busy).then(|| crate::Message::AppsInstalled(Message::RemoveClicked(name))),
                Palette::dark(),
            );
            col.push(
                row![
                    text(&p.name).width(Length::Fixed(280.0)),
                    text(&p.version).width(Length::Fixed(180.0)),
                    remove_btn,
                ]
                .spacing(12),
            )
        });

        column![
            row![filter_input, refresh_btn].spacing(12),
            scrollable(container(rows_view.spacing(4))).height(Length::Fill),
            text(format!(
                "{} matching / {} installed",
                filtered.len(),
                self.packages.len()
            ))
            .size(13),
            text(&self.status).size(13),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Case-insensitive substring match. Empty filter matches
/// every package (the panel's no-filter default).
#[must_use]
pub fn matches_filter(name: &str, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    if f.is_empty() {
        return true;
    }
    name.to_lowercase().contains(&f)
}

/// Pure parser for `rpm -qa --queryformat='%{NAME}\t%{VERSION}\n'`.
/// One package per line, tab-separated. Lines without exactly
/// two tab-separated columns are skipped.
#[must_use]
pub fn parse_rpm_qa(raw: &str) -> Vec<PackageRow> {
    let mut rows: Vec<PackageRow> = raw
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let name = parts.next()?.trim();
            let version = parts.next()?.trim();
            if name.is_empty() || version.is_empty() {
                return None;
            }
            Some(PackageRow {
                name: name.to_string(),
                version: version.to_string(),
            })
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// Shell out to `rpm -qa` with a deterministic queryformat.
/// Returns the raw stdout; empty string on any failure.
pub async fn run_rpm_qa() -> String {
    let Ok(output) = Command::new("rpm")
        .args(["-qa", "--queryformat=%{NAME}\t%{VERSION}\n"])
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

/// Shell out to `pkexec dnf remove -y <name>`. Returns `true`
/// on a zero exit, `false` on any other outcome (user cancelled
/// polkit prompt, package not removable, dependency held, etc.).
pub async fn run_pkexec_dnf_remove(name: &str) -> bool {
    let Ok(output) = Command::new("pkexec")
        .args(["dnf", "remove", "-y", name])
        .output()
        .await
    else {
        return false;
    };
    output.status.success()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "firefox\t128.0.1\n\
                          kernel\t6.10.0\n\
                          glibc\t2.39\n";

    #[test]
    fn parse_rpm_qa_extracts_name_and_version() {
        let rows = parse_rpm_qa(SAMPLE);
        assert_eq!(rows.len(), 3);
        // Output is sorted by name.
        assert_eq!(rows[0].name, "firefox");
        assert_eq!(rows[0].version, "128.0.1");
        assert_eq!(rows[1].name, "glibc");
        assert_eq!(rows[2].name, "kernel");
    }

    #[test]
    fn parse_rpm_qa_skips_malformed_lines() {
        let raw = "good\t1.0\n\
                   missing-version\n\
                   \tbad-empty-name\n\
                   second-good\t2.0\n";
        let rows = parse_rpm_qa(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "good");
        assert_eq!(rows[1].name, "second-good");
    }

    #[test]
    fn parse_rpm_qa_empty_on_empty_input() {
        assert!(parse_rpm_qa("").is_empty());
    }

    #[test]
    fn matches_filter_handles_empty_and_case() {
        assert!(matches_filter("firefox", ""));
        assert!(matches_filter("firefox", "fire"));
        assert!(matches_filter("FireFox", "fox"));
        assert!(!matches_filter("kernel", "fire"));
    }

    #[test]
    fn loaded_records_packages_and_clears_status() {
        let mut panel = AppsInstalledPanel::new();
        panel.busy = true;
        let rows = parse_rpm_qa(SAMPLE);
        let _ = panel.update(Message::Loaded(rows.clone()));
        assert_eq!(panel.packages, rows);
        assert!(!panel.busy);
    }

    #[test]
    fn filter_changed_mutates_filter() {
        let mut panel = AppsInstalledPanel::new();
        let _ = panel.update(Message::FilterChanged("fire".into()));
        assert_eq!(panel.filter, "fire");
    }

    #[test]
    fn remove_clicked_while_busy_is_noop() {
        let mut panel = AppsInstalledPanel::new();
        panel.busy = true;
        panel.status = "Removing…".into();
        let _ = panel.update(Message::RemoveClicked("firefox".into()));
        assert_eq!(panel.status, "Removing…");
    }

    #[test]
    fn refresh_clicked_while_busy_is_noop() {
        let mut panel = AppsInstalledPanel::new();
        panel.busy = true;
        panel.status = "Refreshing…".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "Refreshing…");
    }

    #[test]
    fn remove_finished_success_reports_removed() {
        let mut panel = AppsInstalledPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::RemoveFinished {
            name: "firefox".into(),
            success: true,
        });
        assert!(!panel.busy);
        assert!(panel.status.contains("Removed firefox"));
    }

    #[test]
    fn remove_finished_failure_reports_failed() {
        let mut panel = AppsInstalledPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::RemoveFinished {
            name: "kernel".into(),
            success: false,
        });
        assert!(panel.status.contains("failed"));
        assert!(panel.status.contains("kernel"));
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = AppsInstalledPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("rpm not on PATH".into()));
        assert_eq!(panel.status, "rpm not on PATH");
        assert!(!panel.busy);
    }
}
