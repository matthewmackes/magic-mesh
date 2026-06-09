//! Fleet → Playbooks panel — lists every curated role under
//! `~/QNM-Shared/.qnm-sync/playbooks/roles/` + offers a local
//! `ansible-pull` Run button per role.
//!
//! CB-1.5.b: replaces the v1.x `mackes/workbench/fleet/
//! playbooks.py`. The Python panel went through
//! `mackes.fleet.list_playbooks()` (filesystem walk) +
//! `run_local_pull(playbook)` (subprocess `ansible-pull`). The
//! Rust port does the equivalent walks itself rather than
//! adding a `mded playbooks {list,run}` subcommand pair — the
//! cross-peer dispatch the worklist task sketched lives in the
//! connectivity layer (12.14+) via the existing reconcile loop,
//! so this panel only needs local Run today. The mded
//! subcommand pair is captured as a follow-up if a future
//! design lands a need for it.
//!
//! Curated playbook descriptions match the Phase 1.3.0 lock:
//! 7 roles seeded into QNM-Shared (system-update,
//! mesh-state-snapshot, selinux-permissive-toggle,
//! container-runtime-setup, xfconf-baseline, bloat-removal,
//! apps-install).

use std::path::PathBuf;

use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Task};
use mde_theme::{Density, EmptyState, Icon, Palette};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{empty_state, panel_container};

/// One role under `roles/` — `name` is the directory name, the
/// canonical Ansible role identifier.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Playbook {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct PlaybooksPanel {
    pub playbooks: Vec<Playbook>,
    pub status: String,
    /// Name of the playbook currently mid-run; `None` when idle.
    /// The reducer uses this to grey out other Run buttons until
    /// the in-flight ansible-pull returns.
    pub running: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<Playbook>),
    Error(String),
    RunClicked(String),
    RunFinished { name: String, success: bool },
    RefreshClicked,
}

impl PlaybooksPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let dir = playbooks_root();
                let entries = enumerate_role_dirs(&dir).await;
                Message::Loaded(entries.into_iter().map(playbook_from_name).collect())
            },
            crate::Message::Playbooks,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(rows) => {
                self.playbooks = rows;
                self.status.clear();
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                Task::none()
            }
            Message::RunClicked(name) => {
                if self.running.is_some() {
                    return Task::none();
                }
                self.running = Some(name.clone());
                self.status = format!("Running ansible-pull --tags={name}…");
                let tags = name.clone();
                Task::perform(
                    async move {
                        let success = run_ansible_pull(&tags).await;
                        Message::RunFinished {
                            name: tags,
                            success,
                        }
                    },
                    crate::Message::Playbooks,
                )
            }
            Message::RunFinished { name, success } => {
                self.running = None;
                self.status = if success {
                    format!("{name}: ok")
                } else {
                    format!("{name}: failed (see journalctl --user-unit ansible-pull)")
                };
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
            Some(crate::Message::Playbooks(Message::RefreshClicked)),
            Palette::dark(),
        );

        if self.playbooks.is_empty() {
            // UX-6.b — empty-state with refresh CTA.
            let _ = refresh_btn;
            let state = EmptyState::with_cta(
                "No curated playbooks found",
                "MDE reads roles from `~/QNM-Shared/.qnm-sync/playbooks/roles/`. \
                 Mount QNM-Shared (or seed the curated 7-role tree) and refresh.",
                "Refresh",
            )
            .with_icon(Icon::Playbook);
            return panel_container(
                empty_state(state, Palette::dark(), || {
                    crate::Message::Playbooks(Message::RefreshClicked)
                }),
                Density::Comfortable,
            );
        }

        let rows = self.playbooks.iter().fold(column![], |col, pb| {
            let running = self.running.is_some();
            let name = pb.name.clone();
            let run_label = if self.running.as_deref() == Some(&pb.name) {
                "Running…".to_string()
            } else {
                "Run".to_string()
            };
            // UX-7.a — per-row Run routed through Secondary;
            // Primary would over-emphasize one role over the others.
            let run_btn = variant_button(
                run_label,
                ButtonVariant::Secondary,
                (!running).then(|| crate::Message::Playbooks(Message::RunClicked(name))),
                Palette::dark(),
            );
            col.push(
                row![
                    text(&pb.name).width(Length::Fixed(240.0)),
                    text(&pb.description).width(Length::Fill),
                    run_btn,
                ]
                .spacing(12),
            )
        });

        column![
            scrollable(container(rows.spacing(8))).height(Length::Fill),
            row![
                refresh_btn,
                text(&self.status).size(13),
                text(format!("Playbooks: {}", self.playbooks.len())).size(13),
            ]
            .spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Resolve the active MDE-Workgroup (formerly QNM-Shared) playbooks
/// root. EPIC-RETIRE-QNM Phase C (2026-05-26): env-var precedence is
/// now `MDE_WORKGROUP_ROOT` (canonical, Q77) > `QNM_SHARED_ROOT`
/// (back-compat); falls back to `~/QNM-Shared/.qnm-sync/playbooks/roles`
/// when neither var is set. Matches the Phase 1.3.0 lock.
fn playbooks_root() -> PathBuf {
    let base = std::env::var("MDE_WORKGROUP_ROOT")
        .or_else(|_| std::env::var("QNM_SHARED_ROOT"))
        .map(PathBuf::from)
        .ok();
    let base = base.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("QNM-Shared"))
            .unwrap_or_else(|_| PathBuf::from("/var/empty"))
    });
    base.join(".qnm-sync").join("playbooks").join("roles")
}

/// Walk the roles directory and collect every subdirectory name.
/// Returns an empty Vec on any I/O error so the panel can show
/// its "no curated playbooks found" empty state.
async fn enumerate_role_dirs(dir: &std::path::Path) -> Vec<String> {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(name) = entry.file_name().to_str() {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    out
}

/// Map a role name to the curated description from the Phase
/// 1.3.0 lock; unrecognized names get a generic placeholder
/// (operators can still see custom roles + run them).
#[must_use]
pub fn playbook_from_name(name: String) -> Playbook {
    let description = match name.as_str() {
        "system-update" => "Apply pending dnf upgrades (gated, never runs on default tag)",
        "mesh-state-snapshot" => "Snapshot QNM-Shared state for offline review",
        "selinux-permissive-toggle" => "Flip SELinux to permissive (op-tagged, never default)",
        "container-runtime-setup" => "Install + configure podman / docker runtime",
        "xfconf-baseline" => "Apply baseline xfconf keys (default-tagged)",
        "bloat-removal" => "Remove the curated bloat package list",
        "apps-install" => "Install the curated MDE app list",
        _ => "Custom role",
    };
    Playbook {
        name,
        description: description.to_string(),
    }
}

/// Shell out to `ansible-pull` for a single role tag. Returns
/// `true` on a zero exit, `false` on any other outcome (binary
/// missing, non-zero exit, decode failure). Tag → role mapping
/// follows the v1.x `_tags_for` table — we just pass the role
/// name verbatim as the tag, matching the Python panel's
/// `run_local_pull(playbook)` shape.
pub async fn run_ansible_pull(tag: &str) -> bool {
    let Ok(output) = Command::new("ansible-pull")
        .args(["--tags", tag, "site.yml"])
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

    #[test]
    fn playbook_from_name_uses_curated_description_for_locked_roles() {
        for role in [
            "system-update",
            "mesh-state-snapshot",
            "selinux-permissive-toggle",
            "container-runtime-setup",
            "xfconf-baseline",
            "bloat-removal",
            "apps-install",
        ] {
            let pb = playbook_from_name(role.to_string());
            assert_eq!(pb.name, role);
            assert_ne!(pb.description, "Custom role");
        }
    }

    #[test]
    fn playbook_from_name_falls_back_to_custom_for_unknown_role() {
        let pb = playbook_from_name("totally-new-role".into());
        assert_eq!(pb.description, "Custom role");
    }

    #[test]
    fn loaded_message_records_rows_and_clears_status() {
        let mut panel = PlaybooksPanel::new();
        panel.status = "stale".into();
        let pbs = vec![playbook_from_name("system-update".into())];
        let _ = panel.update(Message::Loaded(pbs.clone()));
        assert_eq!(panel.playbooks, pbs);
        assert!(panel.status.is_empty());
    }

    #[test]
    fn error_message_stores_status() {
        let mut panel = PlaybooksPanel::new();
        let _ = panel.update(Message::Error("dir unreadable".into()));
        assert_eq!(panel.status, "dir unreadable");
    }

    #[test]
    fn run_clicked_sets_running_and_status() {
        let mut panel = PlaybooksPanel::new();
        let _ = panel.update(Message::RunClicked("system-update".into()));
        assert_eq!(panel.running.as_deref(), Some("system-update"));
        assert!(panel.status.contains("ansible-pull"));
    }

    #[test]
    fn run_clicked_while_other_run_in_flight_is_noop() {
        let mut panel = PlaybooksPanel::new();
        panel.running = Some("apps-install".into());
        let _ = panel.update(Message::RunClicked("bloat-removal".into()));
        assert_eq!(panel.running.as_deref(), Some("apps-install"));
    }

    #[test]
    fn run_finished_clears_running_and_reports_success() {
        let mut panel = PlaybooksPanel::new();
        panel.running = Some("system-update".into());
        let _ = panel.update(Message::RunFinished {
            name: "system-update".into(),
            success: true,
        });
        assert!(panel.running.is_none());
        assert_eq!(panel.status, "system-update: ok");
    }

    #[test]
    fn run_finished_failure_includes_remediation_hint() {
        let mut panel = PlaybooksPanel::new();
        panel.running = Some("apps-install".into());
        let _ = panel.update(Message::RunFinished {
            name: "apps-install".into(),
            success: false,
        });
        assert!(panel.status.contains("apps-install"));
        assert!(panel.status.contains("journalctl"));
    }

    #[tokio::test]
    async fn enumerate_role_dirs_empty_on_missing_dir() {
        // The home dir is never going to contain this random
        // suffix; verifies the I/O-error → empty-Vec path.
        let bogus = PathBuf::from("/nonexistent-mde-test-7234923");
        assert!(enumerate_role_dirs(&bogus).await.is_empty());
    }
}
