//! PLANES-9 (W29–W36) — the jobs engine store + model.
//!
//! A job is an **Ansible playbook reference + variables + a target
//! selector** (W29) — the same `apply` primitive the FPG baseline
//! converge uses, so there is one execution path for config and
//! jobs. State lives on the replicated volume (W33), the FPG-2
//! pattern: `jobs/templates/<id>.yaml` (the reusable
//! parameterizations) and `jobs/runs/<run-id>/` (per-run status +
//! per-target results). The TARGET runs its own playbook locally —
//! no push-SSH (W32); a `mackesd` job worker drains runs addressed
//! to it. Target resolution is leaderless + deterministic: tags,
//! roles, and explicit peers, unioned (W31).
//!
//! This module is the store + pure model; the dispatch worker + the
//! `action/jobs/*` Bus verbs (PLANES-10) consume it.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A target selector (W31): any union of capability tags, roles, and
/// explicit peer hostnames, resolved against the directory at launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetSelector {
    /// Capability tags (`execution`, `hop`, …) — a node matches if it
    /// carries any listed tag.
    pub tags: Vec<String>,
    /// Roles (`lighthouse`, `server`, `workstation`).
    pub roles: Vec<String>,
    /// Explicit peer hostnames.
    pub peers: Vec<String>,
}

/// One candidate node the selector resolves against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The node's mesh hostname.
    pub hostname: String,
    /// The node's declared role (`lighthouse`, `server`, `workstation`).
    pub role: String,
    /// Capability tags the node advertises (`execution`, `hop`, …).
    pub tags: BTreeSet<String>,
}

impl TargetSelector {
    /// Resolve to the matching hostnames (W31) — a node matches if it
    /// is named, carries a listed tag, or holds a listed role. Empty
    /// selector matches nothing (a job must say where it runs).
    #[must_use]
    pub fn resolve(&self, candidates: &[Candidate]) -> Vec<String> {
        let mut out: Vec<String> = candidates
            .iter()
            .filter(|c| {
                self.peers.iter().any(|p| p == &c.hostname)
                    || self.roles.iter().any(|r| r == &c.role)
                    || self.tags.iter().any(|t| c.tags.contains(t))
            })
            .map(|c| c.hostname.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// A saved job template (W30): the AWX-minimal core + an optional
/// cron schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobTemplate {
    /// Stable template id (filename stem).
    pub id: String,
    /// Human-readable description of what this template does.
    pub description: String,
    /// Playbook ref — a path under the replicated `playbooks/` dir.
    pub playbook: String,
    /// Variable defaults (overridable at launch).
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, String>,
    /// Target selector applied at launch to determine which nodes run the playbook.
    pub targets: TargetSelector,
    /// Optional cron schedule; the leader fires it (W35).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
}

/// The templates directory.
#[must_use]
pub fn templates_dir(root: &Path) -> PathBuf {
    root.join("jobs").join("templates")
}

/// The runs directory.
#[must_use]
pub fn runs_dir(root: &Path) -> PathBuf {
    root.join("jobs").join("runs")
}

/// Write a template (atomic).
///
/// # Errors
/// IO / serialization failures.
pub fn write_template(root: &Path, tpl: &JobTemplate) -> io::Result<PathBuf> {
    let dir = templates_dir(root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.yaml", tpl.id));
    let yaml = serde_yaml::to_string(tpl).map_err(io::Error::other)?;
    let tmp = dir.join(format!(".{}.yaml.tmp", tpl.id));
    std::fs::write(&tmp, yaml)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read every parseable template, sorted by id (junk-tolerant).
#[must_use]
pub fn read_templates(root: &Path) -> Vec<JobTemplate> {
    let Ok(entries) = std::fs::read_dir(templates_dir(root)) else {
        return Vec::new();
    };
    let mut out: Vec<JobTemplate> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_yaml::from_str(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// One run's manifest (`jobs/runs/<run-id>/run.json`): what to run,
/// where, and the resolved target list at launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRun {
    /// Unique run identifier (UUIDv4 or similar); doubles as the subdirectory name under `runs/`.
    pub run_id: String,
    /// Playbook path executed by every target (relative to the replicated `playbooks/` dir).
    pub playbook: String,
    /// Variable overrides applied for this run (merged over template defaults at launch).
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, String>,
    /// The resolved target hostnames (selector already applied).
    pub targets: Vec<String>,
    /// Launching node (advisory).
    #[serde(default)]
    pub launched_by: String,
    /// Launch time, Unix seconds.
    pub at: u64,
}

/// One target's result within a run
/// (`jobs/runs/<run-id>/<hostname>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetResult {
    /// The target node's mesh hostname.
    pub hostname: String,
    /// `ok` | `changed` | `failed`.
    pub status: String,
    /// Optional human-readable detail from the playbook run (stderr snippet, task name, etc.).
    #[serde(default)]
    pub detail: String,
}

/// The directory for one run.
#[must_use]
pub fn run_dir(root: &Path, run_id: &str) -> PathBuf {
    runs_dir(root).join(run_id)
}

/// Write a run manifest.
///
/// # Errors
/// IO / serialization failures.
pub fn write_run(root: &Path, run: &JobRun) -> io::Result<PathBuf> {
    let dir = run_dir(root, &run.run_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("run.json");
    std::fs::write(&path, serde_json::to_string_pretty(run)?)?;
    Ok(path)
}

/// Read a run manifest, if present.
#[must_use]
pub fn read_run(root: &Path, run_id: &str) -> Option<JobRun> {
    let raw = std::fs::read_to_string(run_dir(root, run_id).join("run.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Write a target's result into the run dir.
///
/// # Errors
/// IO / serialization failures.
pub fn write_target_result(root: &Path, run_id: &str, result: &TargetResult) -> io::Result<()> {
    let dir = run_dir(root, run_id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{}.json", result.hostname)),
        serde_json::to_string_pretty(result)?,
    )
}

/// Read every target result for a run (sorted by hostname).
#[must_use]
pub fn read_target_results(root: &Path, run_id: &str) -> Vec<TargetResult> {
    let Ok(entries) = std::fs::read_dir(run_dir(root, run_id)) else {
        return Vec::new();
    };
    let mut out: Vec<TargetResult> = entries
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

/// Whether `self_hostname` is a pending target of `run` — it is when
/// the run lists it AND no result file exists yet. The job worker's
/// "is there work for me" check (W32 — the target runs its own).
#[must_use]
pub fn run_pending_for(root: &Path, run: &JobRun, self_hostname: &str) -> bool {
    run.targets.iter().any(|t| t == self_hostname)
        && !run_dir(root, &run.run_id)
            .join(format!("{self_hostname}.json"))
            .exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(host: &str, role: &str, tags: &[&str]) -> Candidate {
        Candidate {
            hostname: host.into(),
            role: role.into(),
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
        }
    }

    #[test]
    fn selector_unions_tags_roles_and_peers() {
        let cands = vec![
            cand("pine", "workstation", &["execution"]),
            cand("oak", "server", &[]),
            cand("elm", "lighthouse", &["hop"]),
        ];
        // execution tag → pine; server role → oak; explicit elm.
        let sel = TargetSelector {
            tags: vec!["execution".into()],
            roles: vec!["server".into()],
            peers: vec!["elm".into()],
        };
        assert_eq!(sel.resolve(&cands), ["elm", "oak", "pine"]);
        // Empty selector matches nothing.
        assert!(TargetSelector::default().resolve(&cands).is_empty());
    }

    #[test]
    fn templates_round_trip_through_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let tpl = JobTemplate {
            id: "patch-all".into(),
            description: "dnf upgrade".into(),
            playbook: "playbooks/patch.yml".into(),
            vars: [("reboot".to_string(), "false".to_string())].into(),
            targets: TargetSelector {
                tags: vec!["execution".into()],
                ..Default::default()
            },
            schedule: Some("0 3 * * *".into()),
        };
        write_template(tmp.path(), &tpl).unwrap();
        let back = read_templates(tmp.path());
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], tpl);
    }

    #[test]
    fn run_and_results_round_trip_and_pending_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let run = JobRun {
            run_id: "r-1".into(),
            playbook: "playbooks/patch.yml".into(),
            vars: Default::default(),
            targets: vec!["pine".into(), "oak".into()],
            launched_by: "peer:pine".into(),
            at: 100,
        };
        write_run(tmp.path(), &run).unwrap();
        assert_eq!(read_run(tmp.path(), "r-1").unwrap().targets.len(), 2);
        // pine is pending until it writes its result.
        assert!(run_pending_for(tmp.path(), &run, "pine"));
        assert!(!run_pending_for(tmp.path(), &run, "stranger"), "non-target");
        write_target_result(
            tmp.path(),
            "r-1",
            &TargetResult {
                hostname: "pine".into(),
                status: "ok".into(),
                detail: String::new(),
            },
        )
        .unwrap();
        assert!(
            !run_pending_for(tmp.path(), &run, "pine"),
            "result clears pending"
        );
        assert_eq!(read_target_results(tmp.path(), "r-1").len(), 1);
    }
}
