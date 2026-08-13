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
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

// Replicated job leaves are peer-provided input. Keep enough room for useful
// job definitions while bounding the bytes handed to YAML/JSON parsers.
const MAX_TEMPLATE_BYTES: usize = 4 * 1024 * 1024;
const MAX_RUN_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 256 * 1024;

/// Read one replicated leaf through a descriptor without following its final
/// symlink, accepting only regular files and no more than `max_bytes`.
///
/// The descriptor metadata check rejects directories and other special files
/// before their contents are consumed. Reading one byte beyond the bound
/// closes the race where a peer grows a regular file after the first metadata
/// snapshot, while invalid UTF-8 is rejected by [`read_bounded_text`] before
/// any serializer materializes the document.
fn read_bounded_regular_file(path: &Path, max_bytes: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400_000 | 0o4000); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK

        // Keep unsupported Unix targets fail-closed for a symlink leaf even
        // when their standard library does not expose an O_NOFOLLOW value.
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
            return None;
        }
    }

    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }

    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let max_bytes_u64 = u64::try_from(max_bytes).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes_u64 {
        return None;
    }

    let capacity = usize::try_from(metadata.len())
        .ok()?
        .min(max_bytes)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(max_bytes_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= max_bytes).then_some(bytes)
}

fn read_bounded_text(path: &Path, max_bytes: usize) -> Option<String> {
    String::from_utf8(read_bounded_regular_file(path, max_bytes)?).ok()
}

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

/// Normalize a replicated playbook reference to the only execution namespace
/// accepted by the job worker.
///
/// Bare references are kept compatible with the
/// original CLI and become `playbooks/<ref>`; explicit `playbooks/` references
///
/// are preserved. Absolute paths, parent traversal, and dot components are
/// refused before a path is ever handed to a backend.
///
/// The normalized value is always relative to the replicated playbooks
/// directory.
///
/// # Errors
/// Returns an error for empty, absolute, traversal-containing, or directory references.
pub fn normalize_playbook_ref(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("playbook reference is empty".into());
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err("playbook reference must be relative".into());
    }
    let components: Vec<_> = path.components().collect();
    if components.is_empty()
        || components.iter().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err("playbook reference contains unsafe path components".into());
    }
    let normalized = if components.first() == Some(&Component::Normal("playbooks".as_ref())) {
        path.to_path_buf()
    } else {
        Path::new("playbooks").join(path)
    };
    if normalized == Path::new("playbooks") {
        return Err("playbook reference names the playbooks directory".into());
    }
    Ok(normalized.to_string_lossy().into_owned())
}

/// Resolve a normalized playbook reference below the replicated playbooks
/// directory. The worker additionally canonicalizes the result to reject a
/// symlink that escapes that directory.
///
/// # Errors
/// Returns the same validation errors as [`normalize_playbook_ref`].
pub fn resolve_playbook_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
    let normalized = normalize_playbook_ref(raw)?;
    Ok(root.join(normalized))
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
        .filter_map(|e| read_bounded_text(&e.path(), MAX_TEMPLATE_BYTES))
        .filter_map(|raw| serde_yaml::from_str(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// One run's manifest (`jobs/runs/<run-id>/run.json`): what to run,
/// where, and the resolved target list at launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRun {
    /// Unique run identifier (`UUIDv4` or similar); doubles as the subdirectory
    /// name under `runs/`.
    pub run_id: String,
    /// Playbook path executed by every target (relative to the replicated
    /// `playbooks/` directory).
    pub playbook: String,
    /// SHA-256 of the playbook bytes observed by the privileged launcher.
    /// Legacy or unsigned runs leave this empty and are refused by executors.
    #[serde(default)]
    pub playbook_digest: String,
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
    /// Per-target, exact signed execution envelopes. A distinct capability is
    /// minted for every target so consuming one target cannot authorize another.
    #[serde(default)]
    pub execution_auth: std::collections::BTreeMap<String, String>,
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
    let raw = read_bounded_text(&run_dir(root, run_id).join("run.json"), MAX_RUN_BYTES)?;
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
        .filter_map(|e| read_bounded_text(&e.path(), MAX_RESULT_BYTES))
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
    use std::collections::BTreeMap;

    fn cand(host: &str, role: &str, tags: &[&str]) -> Candidate {
        Candidate {
            hostname: host.into(),
            role: role.into(),
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
        }
    }

    fn template(id: &str) -> JobTemplate {
        JobTemplate {
            id: id.into(),
            description: format!("{id} description"),
            playbook: "playbooks/patch.yml".into(),
            vars: BTreeMap::default(),
            targets: TargetSelector::default(),
            schedule: None,
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
    fn playbook_refs_are_confined_to_the_replicated_playbooks_tree() {
        assert_eq!(
            normalize_playbook_ref("repair.yml").unwrap(),
            "playbooks/repair.yml"
        );
        assert_eq!(
            normalize_playbook_ref("playbooks/repair.yml").unwrap(),
            "playbooks/repair.yml"
        );
        for hostile in [
            "",
            "/etc/passwd",
            "../outside.yml",
            "playbooks/../outside.yml",
            "./repair.yml",
        ] {
            assert!(
                normalize_playbook_ref(hostile).is_err(),
                "accepted {hostile:?}"
            );
        }
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
    fn template_reads_skip_hostile_leaves_and_keep_deterministic_order() {
        let tmp = tempfile::tempdir().unwrap();
        write_template(tmp.path(), &template("zulu")).unwrap();
        write_template(tmp.path(), &template("alpha")).unwrap();
        let dir = templates_dir(tmp.path());

        std::fs::write(dir.join("invalid.yaml"), [0xff, 0xfe]).unwrap();
        std::fs::write(
            dir.join("oversized.yaml"),
            vec![b'x'; MAX_TEMPLATE_BYTES + 1],
        )
        .unwrap();
        std::fs::create_dir(dir.join("directory.yaml")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("alpha.yaml"), dir.join("linked.yaml")).unwrap();

        let templates = read_templates(tmp.path());
        assert_eq!(
            templates
                .iter()
                .map(|template| template.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zulu"]
        );
    }

    #[test]
    fn run_and_results_round_trip_and_pending_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let run = JobRun {
            run_id: "r-1".into(),
            playbook: "playbooks/patch.yml".into(),
            playbook_digest: String::new(),
            vars: BTreeMap::default(),
            targets: vec!["pine".into(), "oak".into()],
            launched_by: "peer:pine".into(),
            at: 100,
            execution_auth: BTreeMap::default(),
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

    #[test]
    fn run_manifest_reads_reject_hostile_leaves() {
        let tmp = tempfile::tempdir().unwrap();
        let valid = JobRun {
            run_id: "valid".into(),
            playbook: "playbooks/patch.yml".into(),
            playbook_digest: String::new(),
            vars: BTreeMap::default(),
            targets: vec!["pine".into()],
            launched_by: String::new(),
            at: 42,
            execution_auth: BTreeMap::default(),
        };
        write_run(tmp.path(), &valid).unwrap();

        let invalid = run_dir(tmp.path(), "invalid");
        std::fs::create_dir_all(&invalid).unwrap();
        std::fs::write(invalid.join("run.json"), [0xff, 0xfe]).unwrap();

        let oversized = run_dir(tmp.path(), "oversized");
        std::fs::create_dir_all(&oversized).unwrap();
        std::fs::write(oversized.join("run.json"), vec![b'{'; MAX_RUN_BYTES + 1]).unwrap();

        let directory = run_dir(tmp.path(), "directory");
        std::fs::create_dir_all(directory.join("run.json")).unwrap();

        #[cfg(unix)]
        {
            let linked = run_dir(tmp.path(), "linked");
            std::fs::create_dir_all(&linked).unwrap();
            std::os::unix::fs::symlink(
                run_dir(tmp.path(), "valid").join("run.json"),
                linked.join("run.json"),
            )
            .unwrap();
        }

        assert_eq!(read_run(tmp.path(), "valid"), Some(valid));
        for run_id in ["invalid", "oversized", "directory"] {
            assert!(read_run(tmp.path(), run_id).is_none(), "accepted {run_id}");
        }
        #[cfg(unix)]
        assert!(read_run(tmp.path(), "linked").is_none());
    }

    #[test]
    fn target_result_reads_skip_hostile_leaves_and_keep_sorted_order() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = "results";
        write_target_result(
            tmp.path(),
            run_id,
            &TargetResult {
                hostname: "zulu".into(),
                status: "ok".into(),
                detail: String::new(),
            },
        )
        .unwrap();
        write_target_result(
            tmp.path(),
            run_id,
            &TargetResult {
                hostname: "alpha".into(),
                status: "changed".into(),
                detail: String::new(),
            },
        )
        .unwrap();
        let dir = run_dir(tmp.path(), run_id);

        std::fs::write(dir.join("invalid.json"), [0xff, 0xfe]).unwrap();
        std::fs::write(dir.join("oversized.json"), vec![b'{'; MAX_RESULT_BYTES + 1]).unwrap();
        std::fs::create_dir(dir.join("directory.json")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("alpha.json"), dir.join("linked.json")).unwrap();

        let results = read_target_results(tmp.path(), run_id);
        assert_eq!(
            results
                .iter()
                .map(|result| result.hostname.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zulu"]
        );
    }
}
