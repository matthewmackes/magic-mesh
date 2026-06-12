//! PLANES-10 — the **Jobs** panel (Controller plane), absorbing Playbooks
//! (W40).
//!
//! Read-only view over the replicated jobs store the `magic_fleet::jobs`
//! engine writes: saved **templates** (`<root>/jobs/templates/*.yaml` —
//! playbook ref + variables + a tag/role/peer target selector + an
//! optional cron schedule, W30/W31/W38) and **run history**
//! (`<root>/jobs/runs/<run-id>/run.json` + per-target `<host>.json`
//! results, W36). The panel deserializes these into local rows — no new
//! cross-crate dependency, the established pattern (inventory/hardware
//! read their stores the same way).
//!
//! Build-now-defer-visual: the load + projection are pure and unit-tested;
//! the interactive **launch form** (which drives `action/jobs/launch` over
//! the Bus) and the on-Cosmic `/preview` pass are the deferred tail.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Padding, Task};
use mde_theme::{EmptyState, Icon};
use serde::Deserialize;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{empty_state, panel_container, status_badge, BadgeSeverity};

/// The target selector (mirrors `magic_fleet::jobs::TargetSelector`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TargetSelectorRow {
    pub tags: Vec<String>,
    pub roles: Vec<String>,
    pub peers: Vec<String>,
}

impl TargetSelectorRow {
    /// A one-line "where it runs" summary for the list.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.tags.is_empty() {
            parts.push(format!("tags: {}", self.tags.join(",")));
        }
        if !self.roles.is_empty() {
            parts.push(format!("roles: {}", self.roles.join(",")));
        }
        if !self.peers.is_empty() {
            parts.push(format!("peers: {}", self.peers.join(",")));
        }
        if parts.is_empty() {
            "(no targets — never runs)".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// A saved job template (mirrors `magic_fleet::jobs::JobTemplate`).
#[derive(Debug, Clone, Deserialize)]
pub struct TemplateRow {
    pub id: String,
    #[serde(default)]
    pub description: String,
    pub playbook: String,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    #[serde(default)]
    pub targets: TargetSelectorRow,
    #[serde(default)]
    pub schedule: Option<String>,
}

/// One run manifest (mirrors `magic_fleet::jobs::JobRun`).
#[derive(Debug, Clone, Deserialize)]
pub struct RunRow {
    pub run_id: String,
    pub playbook: String,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub launched_by: String,
    #[serde(default)]
    pub at: u64,
}

/// One target's result within a run (mirrors `TargetResult`).
#[derive(Debug, Clone, Deserialize)]
pub struct ResultRow {
    pub hostname: String,
    pub status: String,
    #[serde(default)]
    pub detail: String,
}

/// `MDE_WORKGROUP_ROOT`-or-`/mnt/mesh-storage` (workbench doesn't depend
/// on mackesd; the default is duplicated, matching network_hosts).
#[must_use]
pub fn workgroup_root() -> PathBuf {
    std::env::var_os("MDE_WORKGROUP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/mesh-storage"))
}

/// Read every `jobs/templates/*.yaml`, sorted by id (junk-tolerant).
#[must_use]
pub fn load_templates(root: &Path) -> Vec<TemplateRow> {
    let dir = root.join("jobs").join("templates");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<TemplateRow> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_yaml::from_str(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Read every run manifest, newest first (junk-tolerant).
#[must_use]
pub fn load_runs(root: &Path) -> Vec<RunRow> {
    let runs_dir = root.join("jobs").join("runs");
    let Ok(entries) = std::fs::read_dir(runs_dir) else {
        return Vec::new();
    };
    let mut out: Vec<RunRow> = entries
        .filter_map(Result::ok)
        .filter_map(|e| std::fs::read_to_string(e.path().join("run.json")).ok())
        .filter_map(|raw| serde_json::from_str(&raw).ok())
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at)); // newest first
    out
}

/// Read one run's per-target results, sorted by hostname.
#[must_use]
pub fn load_results(root: &Path, run_id: &str) -> Vec<ResultRow> {
    let dir = root.join("jobs").join("runs").join(run_id);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<ResultRow> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.ends_with(".json") && n != "run.json"
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_json::from_str(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    out
}

/// What the panel is focused on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    List,
    Template(String),
    Run(String),
}

/// The Jobs panel state.
#[derive(Debug, Clone, Default)]
pub struct JobsPanel {
    pub templates: Vec<TemplateRow>,
    pub runs: Vec<RunRow>,
    pub results: Vec<ResultRow>,
    pub status: String,
    pub busy: bool,
    pub focus: Focus,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<TemplateRow>, Vec<RunRow>),
    FocusTemplate(String),
    FocusRun(String, Vec<ResultRow>),
    OpenRun(String),
    Back,
    RefreshClicked,
    /// PLANES-10 / W38 — launch the focused template now (resolves its
    /// selector + writes a run via `action/jobs/launch`).
    LaunchClicked(String),
    /// The launch verb replied — show the outcome line.
    Launched(String),
}

impl JobsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let root = workgroup_root();
                Message::Loaded(load_templates(&root), load_runs(&root))
            },
            crate::Message::Jobs,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(templates, runs) => {
                self.templates = templates;
                self.runs = runs;
                self.busy = false;
                self.status.clear();
                Task::none()
            }
            Message::FocusTemplate(id) => {
                self.focus = Focus::Template(id);
                Task::none()
            }
            Message::OpenRun(run_id) => {
                let results = load_results(&workgroup_root(), &run_id);
                Task::perform(async move { (run_id, results) }, |(id, r)| {
                    crate::Message::Jobs(Message::FocusRun(id, r))
                })
            }
            Message::FocusRun(run_id, results) => {
                self.results = results;
                self.focus = Focus::Run(run_id);
                Task::none()
            }
            Message::Back => {
                self.focus = Focus::List;
                self.results.clear();
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
            Message::LaunchClicked(id) => {
                if self.busy {
                    return Task::none();
                }
                let Some(t) = self.templates.iter().find(|t| t.id == id) else {
                    self.status = "template no longer present".into();
                    return Task::none();
                };
                // A selector that matches nothing would be refused by the
                // verb anyway — say so up front rather than round-trip.
                if t.targets.tags.is_empty()
                    && t.targets.roles.is_empty()
                    && t.targets.peers.is_empty()
                {
                    self.status = "this template has no targets — it would match no nodes".into();
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("launching {id}…");
                let body = serde_json::json!({
                    "playbook": t.playbook,
                    "targets": {
                        "tags": t.targets.tags,
                        "roles": t.targets.roles,
                        "peers": t.targets.peers,
                    },
                    "vars": t.vars,
                })
                .to_string();
                Task::perform(
                    async move {
                        let reply = tokio::task::spawn_blocking(move || {
                            crate::dbus::action_request_with_body(
                                "action/jobs/launch",
                                Some(&body),
                                Duration::from_secs(3),
                            )
                        })
                        .await
                        .ok()
                        .flatten();
                        match reply
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                        {
                            Some(v) if v["ok"] == true => {
                                let n = v["targets"].as_array().map_or(0, Vec::len);
                                let run = v["run_id"].as_str().unwrap_or("?").to_string();
                                format!("launched run {run} → {n} target(s) — Refresh for results")
                            }
                            Some(v) => {
                                format!(
                                    "launch failed: {}",
                                    v["error"].as_str().unwrap_or("unknown")
                                )
                            }
                            None => "launch failed: mackesd not answering on the Bus".into(),
                        }
                    },
                    |msg| crate::Message::Jobs(Message::Launched(msg)),
                )
            }
            Message::Launched(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        match &self.focus {
            Focus::List => self.view_list(),
            Focus::Template(id) => self.view_template(id),
            Focus::Run(run_id) => self.view_run(run_id),
        }
    }

    fn view_list(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let refresh = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then_some(crate::Message::Jobs(Message::RefreshClicked)),
            palette,
        );

        if self.templates.is_empty() && self.runs.is_empty() {
            let state = EmptyState::with_cta(
                "No job templates yet",
                "Define a job template (a playbook + variables + a tag/role/peer \
                 target) and it appears here with its run history (PLANES-10).",
                "Refresh",
            )
            .with_icon(Icon::Inventory);
            return panel_container(
                empty_state(state, palette, || {
                    crate::Message::Jobs(Message::RefreshClicked)
                }),
                density,
            );
        }

        let mut tcol = column![text("Templates").size(16)].spacing(8);
        for t in &self.templates {
            let sched = t
                .schedule
                .as_deref()
                .map(|s| format!("  ⏱ {s}"))
                .unwrap_or_default();
            let open = variant_button(
                "View",
                ButtonVariant::Ghost,
                Some(crate::Message::Jobs(Message::FocusTemplate(t.id.clone()))),
                palette,
            );
            tcol = tcol.push(
                container(
                    row![
                        column![
                            text(format!("{}{sched}", t.id)).size(15),
                            text(t.targets.summary()).size(12),
                        ]
                        .spacing(2),
                        open,
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                )
                .padding(Padding::from(10)),
            );
        }

        let mut rcol = column![text("Recent runs").size(16)].spacing(8);
        for r in self.runs.iter().take(20) {
            let open = variant_button(
                "Results",
                ButtonVariant::Ghost,
                Some(crate::Message::Jobs(Message::OpenRun(r.run_id.clone()))),
                palette,
            );
            rcol = rcol.push(
                container(
                    row![
                        column![
                            text(r.run_id.clone()).size(14),
                            text(format!("{} → {} target(s)", r.playbook, r.targets.len()))
                                .size(12),
                        ]
                        .spacing(2),
                        open,
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                )
                .padding(Padding::from(10)),
            );
        }

        panel_container(
            column![
                refresh,
                // CV-3 — density-aware section gap (space.lg).
                scrollable(
                    column![tcol, rcol].spacing(crate::panel_chrome::column_gap(density))
                )
                .height(Length::Fill)
            ]
            .spacing(16)
            .width(Length::Fill)
            .into(),
            density,
        )
    }

    fn view_template(&self, id: &str) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let back = variant_button(
            "‹ Back",
            ButtonVariant::Ghost,
            Some(crate::Message::Jobs(Message::Back)),
            palette,
        );
        let Some(t) = self.templates.iter().find(|t| t.id == id) else {
            return panel_container(
                column![back, text("Template no longer present.")]
                    .spacing(16)
                    .into(),
                density,
            );
        };
        // W38 — read-only YAML-ish projection of the template.
        let mut yaml = format!(
            "id: {}\ndescription: {}\nplaybook: {}\n",
            t.id, t.description, t.playbook
        );
        if let Some(s) = &t.schedule {
            yaml.push_str(&format!("schedule: \"{s}\"\n"));
        }
        yaml.push_str("targets:\n");
        yaml.push_str(&format!("  tags: {:?}\n", t.targets.tags));
        yaml.push_str(&format!("  roles: {:?}\n", t.targets.roles));
        yaml.push_str(&format!("  peers: {:?}\n", t.targets.peers));
        if !t.vars.is_empty() {
            yaml.push_str("vars:\n");
            for (k, v) in &t.vars {
                yaml.push_str(&format!("  {k}: {v}\n"));
            }
        }
        // W38 — the interactive launch: fire the template's playbook +
        // selector at action/jobs/launch. Disabled while busy or when the
        // template targets nothing (it would match no nodes).
        let no_targets =
            t.targets.tags.is_empty() && t.targets.roles.is_empty() && t.targets.peers.is_empty();
        let launch = variant_button(
            if self.busy {
                "Launching…"
            } else {
                "Launch now"
            },
            ButtonVariant::Primary,
            (!self.busy && !no_targets)
                .then_some(crate::Message::Jobs(Message::LaunchClicked(t.id.clone()))),
            palette,
        );
        panel_container(
            column![
                row![back, launch].spacing(12),
                text(format!("Template: {}", t.id)).size(20),
                text(yaml).size(13),
                text(self.status.clone()).size(13),
            ]
            .spacing(12)
            .into(),
            density,
        )
    }

    fn view_run(&self, run_id: &str) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let back = variant_button(
            "‹ Back",
            ButtonVariant::Ghost,
            Some(crate::Message::Jobs(Message::Back)),
            palette,
        );
        let mut col = column![back, text(format!("Run: {run_id}")).size(20)].spacing(10);
        if self.results.is_empty() {
            col = col.push(text("No target results reported yet.").size(13));
        } else {
            for r in &self.results {
                let severity = match r.status.as_str() {
                    "ok" => BadgeSeverity::Success,
                    "changed" => BadgeSeverity::Neutral,
                    _ => BadgeSeverity::Warning,
                };
                col = col.push(
                    row![
                        text(r.hostname.clone()).width(Length::Fixed(200.0)),
                        status_badge(r.status.clone(), severity, palette),
                        text(r.detail.clone()).size(12),
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                );
            }
        }
        panel_container(scrollable(col).height(Length::Fill).into(), density)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_template(root: &Path, id: &str) {
        let dir = root.join("jobs").join("templates");
        std::fs::create_dir_all(&dir).unwrap();
        let yaml = format!(
            "id: {id}\ndescription: dnf upgrade\nplaybook: playbooks/patch.yml\n\
             targets:\n  tags: [execution]\n  roles: []\n  peers: []\n\
             schedule: \"0 3 * * *\"\nvars:\n  reboot: \"false\"\n"
        );
        std::fs::write(dir.join(format!("{id}.yaml")), yaml).unwrap();
    }

    fn seed_run(root: &Path, run_id: &str, at: u64) {
        let dir = root.join("jobs").join("runs").join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        let run = serde_json::json!({
            "run_id": run_id, "playbook": "playbooks/patch.yml",
            "targets": ["pine", "oak"], "launched_by": "peer:pine", "at": at,
        });
        std::fs::write(dir.join("run.json"), run.to_string()).unwrap();
        let res = serde_json::json!({ "hostname": "pine", "status": "ok", "detail": "" });
        std::fs::write(dir.join("pine.json"), res.to_string()).unwrap();
    }

    #[test]
    fn templates_load_sorted_with_target_summary() {
        let tmp = tempfile::tempdir().unwrap();
        seed_template(tmp.path(), "patch-all");
        let tpls = load_templates(tmp.path());
        assert_eq!(tpls.len(), 1);
        assert_eq!(tpls[0].id, "patch-all");
        assert_eq!(tpls[0].schedule.as_deref(), Some("0 3 * * *"));
        assert!(tpls[0].targets.summary().contains("tags: execution"));
    }

    #[test]
    fn runs_load_newest_first_and_results_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        seed_run(tmp.path(), "r-old", 100);
        seed_run(tmp.path(), "r-new", 200);
        let runs = load_runs(tmp.path());
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].run_id, "r-new", "newest first");
        let results = load_results(tmp.path(), "r-new");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hostname, "pine");
        assert_eq!(results[0].status, "ok");
    }

    #[test]
    fn empty_target_selector_summary_is_explicit() {
        assert_eq!(
            TargetSelectorRow::default().summary(),
            "(no targets — never runs)"
        );
    }

    fn template(id: &str, targets: TargetSelectorRow) -> TemplateRow {
        TemplateRow {
            id: id.into(),
            description: String::new(),
            playbook: "playbooks/p.yml".into(),
            vars: BTreeMap::new(),
            targets,
            schedule: None,
        }
    }

    #[test]
    fn launch_with_no_targets_warns_and_stays_idle() {
        // W38 — a target-less template can't run; the panel says so
        // without a Bus round-trip and without going busy.
        let mut panel = JobsPanel::new();
        panel.templates = vec![template("empty", TargetSelectorRow::default())];
        let _ = panel.update(Message::LaunchClicked("empty".into()));
        assert!(!panel.busy);
        assert!(panel.status.contains("no targets"));
    }

    #[test]
    fn launch_unknown_template_reports_missing() {
        let mut panel = JobsPanel::new();
        let _ = panel.update(Message::LaunchClicked("ghost".into()));
        assert!(!panel.busy);
        assert!(panel.status.contains("no longer present"));
    }

    #[test]
    fn launch_with_targets_goes_busy() {
        // A targeted template fires the verb (busy until the reply).
        let mut panel = JobsPanel::new();
        panel.templates = vec![template(
            "patch",
            TargetSelectorRow {
                tags: vec!["execution".into()],
                ..Default::default()
            },
        )];
        let _ = panel.update(Message::LaunchClicked("patch".into()));
        assert!(panel.busy);
        assert!(panel.status.contains("launching patch"));
    }

    #[test]
    fn launched_sets_status_and_clears_busy() {
        let mut panel = JobsPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Launched("launched run 01H… → 2 target(s)".into()));
        assert!(!panel.busy);
        assert!(panel.status.contains("launched run"));
    }

    #[test]
    fn focus_transitions() {
        let mut p = JobsPanel::new();
        let _ = p.update(Message::FocusTemplate("t1".into()));
        assert_eq!(p.focus, Focus::Template("t1".into()));
        let _ = p.update(Message::FocusRun("r1".into(), vec![]));
        assert_eq!(p.focus, Focus::Run("r1".into()));
        let _ = p.update(Message::Back);
        assert_eq!(p.focus, Focus::List);
    }
}
