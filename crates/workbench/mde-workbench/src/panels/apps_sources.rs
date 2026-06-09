//! Apps → Sources & Repos panel — dnf repo enable/disable.
//!
//! CB-1.3 partial: ports the dnf-repolist slice of
//! `mackes/workbench/apps/sources.py`. The v1.x panel also
//! covered Flathub + RPM Fusion + fedora-workstation-repos
//! sections; those land as a separate CB-1.3 follow-up since
//! each needs a specific install workflow (flatpak
//! remote-add, dnf install of a release RPM, etc.).
//!
//! Reads via `dnf repolist --all --quiet`; writes via
//! `pkexec dnf config-manager setopt <id>.enabled=1|0` —
//! the `config-manager` plugin ships in dnf5's
//! `dnf5-plugins` package which is install-by-default on
//! Fedora workstation.

use iced::widget::{column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Task};
use mde_theme::Palette;
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoRow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AppsSourcesPanel {
    pub repos: Vec<RepoRow>,
    pub filter: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<RepoRow>),
    Error(String),
    FilterChanged(String),
    ToggleClicked {
        id: String,
        enable: bool,
    },
    ToggleFinished {
        id: String,
        success: bool,
    },
    RefreshClicked,
    /// CB-1.3 follow-up — add the Flathub remote
    /// per-user via `flatpak remote-add --user`.
    AddFlathubClicked,
    /// CB-1.3 follow-up — install the RPM Fusion free
    /// release RPM (provides the `rpmfusion-free` repo).
    AddRpmFusionFreeClicked,
    /// CB-1.3 follow-up — install the RPM Fusion nonfree
    /// release RPM (Chrome's friend; nvidia, codecs).
    AddRpmFusionNonfreeClicked,
    /// CB-1.3 follow-up — install
    /// `fedora-workstation-repositories` (ships Chrome,
    /// Steam, NVIDIA repos as disabled).
    AddFedoraWorkstationReposClicked,
    /// Generic finish for the four "add a known source"
    /// actions above.
    SourceAddFinished {
        label: String,
        success: bool,
    },
}

impl AppsSourcesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let raw = run_dnf_repolist().await;
                Message::Loaded(parse_dnf_repolist(&raw))
            },
            crate::Message::AppsSources,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(repos) => {
                self.repos = repos;
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
            Message::ToggleClicked { id, enable } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!(
                    "{} {id} (polkit will prompt)…",
                    if enable { "Enabling" } else { "Disabling" },
                );
                Task::perform(
                    async move {
                        let success = run_dnf_config_manager(&id, enable).await;
                        Message::ToggleFinished { id, success }
                    },
                    crate::Message::AppsSources,
                )
            }
            Message::ToggleFinished { id, success } => {
                self.status = if success {
                    format!("Updated {id}.")
                } else {
                    format!("Updating {id} failed (see journalctl).")
                };
                self.busy = false;
                // Reload to reflect the new enabled state.
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
            Message::AddFlathubClicked => self.dispatch_source_add(
                "Flathub",
                vec![
                    "flatpak".into(),
                    "remote-add".into(),
                    "--user".into(),
                    "--if-not-exists".into(),
                    "flathub".into(),
                    "https://flathub.org/repo/flathub.flatpakrepo".into(),
                ],
                false,
            ),
            Message::AddRpmFusionFreeClicked => self.dispatch_source_add(
                "RPM Fusion free",
                vec![
                    "dnf".into(),
                    "install".into(),
                    "-y".into(),
                    "--allowerasing".into(),
                    rpmfusion_release_url("free").into(),
                ],
                true,
            ),
            Message::AddRpmFusionNonfreeClicked => self.dispatch_source_add(
                "RPM Fusion nonfree",
                vec![
                    "dnf".into(),
                    "install".into(),
                    "-y".into(),
                    "--allowerasing".into(),
                    rpmfusion_release_url("nonfree").into(),
                ],
                true,
            ),
            Message::AddFedoraWorkstationReposClicked => self.dispatch_source_add(
                "fedora-workstation-repositories",
                vec![
                    "dnf".into(),
                    "install".into(),
                    "-y".into(),
                    "fedora-workstation-repositories".into(),
                ],
                true,
            ),
            Message::SourceAddFinished { label, success } => {
                self.busy = false;
                self.status = if success {
                    format!("Added {label}.")
                } else {
                    format!("Adding {label} failed (see journalctl).")
                };
                Self::load()
            }
        }
    }

    fn dispatch_source_add(
        &mut self,
        label: &str,
        argv: Vec<String>,
        pkexec: bool,
    ) -> Task<crate::Message> {
        if self.busy {
            return Task::none();
        }
        self.busy = true;
        self.status = format!("Adding {label}…");
        let label_owned = label.to_string();
        Task::perform(
            async move {
                let success = run_source_add(&argv, pkexec).await;
                Message::SourceAddFinished {
                    label: label_owned,
                    success,
                }
            },
            crate::Message::AppsSources,
        )
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let filter_input = text_input("Filter…", &self.filter)
            .on_input(|v| crate::Message::AppsSources(Message::FilterChanged(v)));
        // UX-7.a — refresh routed through the shared button variant.
        let palette = Palette::dark();
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::AppsSources(Message::RefreshClicked)),
            palette,
        );

        let filtered: Vec<&RepoRow> = self
            .repos
            .iter()
            .filter(|r| matches_filter(&r.id, &r.name, &self.filter))
            .collect();

        let rows_view = filtered.iter().fold(column![], |col, r| {
            let id = r.id.clone();
            let next_enable = !r.enabled;
            let btn_label = if r.enabled { "Disable" } else { "Enable" };
            // UX-7.a — per-row enable/disable toggle → Secondary.
            let toggle_btn = variant_button(
                btn_label,
                ButtonVariant::Secondary,
                (!self.busy).then(|| {
                    crate::Message::AppsSources(Message::ToggleClicked {
                        id,
                        enable: next_enable,
                    })
                }),
                palette,
            );
            let state_label = if r.enabled { "enabled" } else { "disabled" };
            col.push(
                row![
                    text(&r.id).width(Length::Fixed(220.0)),
                    text(&r.name).width(Length::Fixed(280.0)),
                    text(state_label).width(Length::Fixed(80.0)),
                    toggle_btn,
                ]
                .spacing(12),
            )
        });

        let busy = self.busy;
        // UX-7.a — third-party source add → Secondary.
        let add_button = |label: &'static str, msg: Message| {
            variant_button(
                label,
                ButtonVariant::Secondary,
                (!busy).then(|| crate::Message::AppsSources(msg)),
                palette,
            )
        };

        column![
            row![filter_input, refresh_btn].spacing(12),
            scrollable(container(rows_view.spacing(4))).height(Length::Fill),
            text(format!(
                "{} matching / {} total ({} enabled)",
                filtered.len(),
                self.repos.len(),
                self.repos.iter().filter(|r| r.enabled).count(),
            ))
            .size(13),
            text("Known third-party sources").size(16),
            text(
                "One-click installs for the canonical sources MDE doesn't \
                 ship enabled by default. Each runs under pkexec or \
                 flatpak; refresh the list after to see new repos appear.",
            )
            .size(13),
            row![
                add_button("Add Flathub", Message::AddFlathubClicked),
                add_button("RPM Fusion free", Message::AddRpmFusionFreeClicked),
                add_button("RPM Fusion nonfree", Message::AddRpmFusionNonfreeClicked),
                add_button(
                    "fedora-workstation-repos",
                    Message::AddFedoraWorkstationReposClicked,
                ),
            ]
            .spacing(8),
            text(&self.status).size(13),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Case-insensitive substring match against either the repo
/// id or its display name. Empty filter matches all rows.
#[must_use]
pub fn matches_filter(id: &str, name: &str, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    if f.is_empty() {
        return true;
    }
    id.to_lowercase().contains(&f) || name.to_lowercase().contains(&f)
}

/// Pure parser for `dnf repolist --all` output.
///
/// dnf5's `repolist --all` emits a 3-column table:
///   `repo id                  repo name           status`
/// The status column is the literal string `enabled` or
/// `disabled`. Header + blank lines are skipped. Repo names
/// can contain spaces, so we parse by stripping the
/// last-whitespace-separated column (status) and the
/// first-whitespace-separated column (id) — everything
/// between is the name.
#[must_use]
pub fn parse_dnf_repolist(raw: &str) -> Vec<RepoRow> {
    let mut rows: Vec<RepoRow> = raw.lines().filter_map(parse_repolist_line).collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

fn parse_repolist_line(line: &str) -> Option<RepoRow> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("repo id") || trimmed.starts_with("Last metadata") {
        return None;
    }
    // Find the rightmost whitespace-separated word; if it's
    // not "enabled" or "disabled" the line isn't a repo row.
    let last_space = trimmed.rfind(char::is_whitespace)?;
    let status = trimmed[last_space + 1..].trim();
    let enabled = match status {
        "enabled" => true,
        "disabled" => false,
        _ => return None,
    };
    let rest = trimmed[..last_space].trim();
    // First word is the repo id.
    let first_space = rest.find(char::is_whitespace)?;
    let id = rest[..first_space].trim().to_string();
    let name = rest[first_space..].trim().to_string();
    if id.is_empty() {
        return None;
    }
    Some(RepoRow { id, name, enabled })
}

/// Shell out to `dnf repolist --all --quiet`. Returns stdout
/// on success; empty string on any failure.
pub async fn run_dnf_repolist() -> String {
    let Ok(output) = Command::new("dnf")
        .args(["repolist", "--all", "--quiet"])
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

/// Shell out to `pkexec dnf config-manager setopt
/// <id>.enabled=0|1`. Returns `true` on a zero exit.
/// Build the canonical RPM Fusion release-RPM URL for the
/// current Fedora release. Format matches the official
/// install docs: `https://download1.rpmfusion.org/{free,
/// nonfree}/fedora/rpmfusion-{free, nonfree}-release-$(rpm
/// -E %fedora).noarch.rpm`. The fedora release id comes from
/// `/etc/os-release`'s VERSION_ID; defaults to "44" when the
/// file can't be read (matching Fedora 44 — our build target).
#[must_use]
pub fn rpmfusion_release_url(flavour: &str) -> String {
    let release = read_fedora_version_id().unwrap_or_else(|| "44".to_string());
    format!(
        "https://download1.rpmfusion.org/{flavour}/fedora/rpmfusion-{flavour}-release-{release}.noarch.rpm",
    )
}

fn read_fedora_version_id() -> Option<String> {
    let raw = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("VERSION_ID=") {
            let trimmed = rest.trim_matches('"');
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Run a "source add" command. When `pkexec` is true we
/// prepend `pkexec` to the argv (for dnf install commands
/// that need root); otherwise the command runs as the
/// current user (flatpak remote-add --user). Returns
/// success.
pub async fn run_source_add(argv: &[String], pkexec: bool) -> bool {
    let mut effective: Vec<String> = if pkexec {
        std::iter::once("pkexec".to_string())
            .chain(argv.iter().cloned())
            .collect()
    } else {
        argv.to_vec()
    };
    let Some(bin) = effective.first().cloned() else {
        return false;
    };
    let args = effective.split_off(1);
    let Ok(output) = Command::new(&bin).args(&args).output().await else {
        return false;
    };
    output.status.success()
}

pub async fn run_dnf_config_manager(id: &str, enable: bool) -> bool {
    let setopt = format!("{id}.enabled={}", if enable { 1 } else { 0 });
    let Ok(output) = Command::new("pkexec")
        .args(["dnf", "config-manager", "setopt", &setopt])
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

    const SAMPLE: &str = "\
repo id                          repo name                              status
fedora                           Fedora 44 - x86_64                     enabled
fedora-cisco-openh264            Fedora 44 openh264 (From Cisco)        disabled
google-chrome                    google-chrome                          disabled
updates                          Fedora 44 - x86_64 - Updates           enabled
";

    #[test]
    fn parse_dnf_repolist_extracts_id_name_and_status() {
        let rows = parse_dnf_repolist(SAMPLE);
        assert_eq!(rows.len(), 4);
        // Sorted by id.
        assert_eq!(rows[0].id, "fedora");
        assert_eq!(rows[0].name, "Fedora 44 - x86_64");
        assert!(rows[0].enabled);
        assert_eq!(rows[1].id, "fedora-cisco-openh264");
        assert!(!rows[1].enabled);
        assert_eq!(rows[3].id, "updates");
        assert!(rows[3].enabled);
    }

    #[test]
    fn parse_dnf_repolist_handles_repo_names_with_spaces() {
        let raw = "rpmfusion-free   RPM Fusion for Fedora 44 - Free     enabled\n";
        let rows = parse_dnf_repolist(raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "rpmfusion-free");
        assert_eq!(rows[0].name, "RPM Fusion for Fedora 44 - Free");
        assert!(rows[0].enabled);
    }

    #[test]
    fn parse_dnf_repolist_skips_headers_and_blanks() {
        let raw = "\
repo id   repo name   status

Last metadata expiration check: now.
fedora    Fedora 44   enabled
";
        let rows = parse_dnf_repolist(raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "fedora");
    }

    #[test]
    fn parse_dnf_repolist_rejects_lines_without_status() {
        let raw = "fedora Fedora 44 maybe\n";
        assert!(parse_dnf_repolist(raw).is_empty());
    }

    #[test]
    fn parse_dnf_repolist_empty_on_empty_input() {
        assert!(parse_dnf_repolist("").is_empty());
    }

    #[test]
    fn matches_filter_searches_id_and_name() {
        assert!(matches_filter("rpmfusion-free", "RPM Fusion - Free", ""));
        assert!(matches_filter(
            "rpmfusion-free",
            "RPM Fusion - Free",
            "rpmfusion"
        ));
        // Name-side match.
        assert!(matches_filter(
            "repo123",
            "Fedora Workstation",
            "workstation"
        ));
        assert!(!matches_filter("fedora", "Fedora 44", "ubuntu"));
    }

    #[test]
    fn loaded_records_repos_and_clears_status() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        let rows = parse_dnf_repolist(SAMPLE);
        let _ = panel.update(Message::Loaded(rows.clone()));
        assert_eq!(panel.repos, rows);
        assert!(!panel.busy);
    }

    #[test]
    fn toggle_clicked_while_busy_is_noop() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::ToggleClicked {
            id: "fedora".into(),
            enable: false,
        });
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn toggle_finished_success_reports_updated() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::ToggleFinished {
            id: "rpmfusion-free".into(),
            success: true,
        });
        assert!(!panel.busy);
        assert!(panel.status.contains("Updated rpmfusion-free"));
    }

    #[test]
    fn toggle_finished_failure_includes_failed_marker() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::ToggleFinished {
            id: "fedora".into(),
            success: false,
        });
        assert!(panel.status.contains("failed"));
        assert!(panel.status.contains("fedora"));
    }

    #[test]
    fn refresh_clicked_while_busy_is_noop() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn filter_changed_mutates_filter() {
        let mut panel = AppsSourcesPanel::new();
        let _ = panel.update(Message::FilterChanged("rpmfusion".into()));
        assert_eq!(panel.filter, "rpmfusion");
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("dnf not on PATH".into()));
        assert_eq!(panel.status, "dnf not on PATH");
        assert!(!panel.busy);
    }

    #[test]
    fn rpmfusion_release_url_matches_canonical_format() {
        let free = rpmfusion_release_url("free");
        assert!(free.starts_with("https://download1.rpmfusion.org/free/fedora/"));
        assert!(free.contains("rpmfusion-free-release-"));
        assert!(free.ends_with(".noarch.rpm"));
        let nonfree = rpmfusion_release_url("nonfree");
        assert!(nonfree.contains("rpmfusion-nonfree-release-"));
    }

    #[test]
    fn add_flathub_clicked_sets_busy_and_status() {
        let mut panel = AppsSourcesPanel::new();
        let _ = panel.update(Message::AddFlathubClicked);
        assert!(panel.busy);
        assert!(panel.status.contains("Flathub"));
    }

    #[test]
    fn add_rpmfusion_free_clicked_sets_busy_and_status() {
        let mut panel = AppsSourcesPanel::new();
        let _ = panel.update(Message::AddRpmFusionFreeClicked);
        assert!(panel.busy);
        assert!(panel.status.contains("RPM Fusion free"));
    }

    #[test]
    fn source_add_clicked_while_busy_is_noop() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::AddFlathubClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn source_add_finished_success_clears_busy_and_records_label() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::SourceAddFinished {
            label: "Flathub".into(),
            success: true,
        });
        assert!(!panel.busy);
        assert!(panel.status.contains("Added Flathub"));
    }

    #[test]
    fn source_add_finished_failure_records_label_and_failed() {
        let mut panel = AppsSourcesPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::SourceAddFinished {
            label: "RPM Fusion free".into(),
            success: false,
        });
        assert!(panel.status.contains("RPM Fusion free"));
        assert!(panel.status.contains("failed"));
    }
}
