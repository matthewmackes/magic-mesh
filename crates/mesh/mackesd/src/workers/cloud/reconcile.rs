//! Workloads U5 — the per-node reconcile engine.
//!
//! This is the placement-node slice of the Workloads control loop U4 (`set-desired`
//! / `plan`) and the drift tick both drive:
//!
//! - **Per-node desired store** ([`read_desired_slice`] / [`write_desired_doc`] /
//!   [`remove_desired_doc`]) — the node's declared workloads live under
//!   `<state_root>/mcnf/cloud/desired/<node>/<name>.json` (the local realization of
//!   the `/mcnf/cloud/desired/<node>/<name>` key). A worker on node N only ever
//!   reads its own `<N>/*` slice, so N's `terraform.tfvars.json` carries only N's
//!   workloads (per-node apply — no node renders another node's set).
//! - **The plan/render bridge** ([`plan_counts_for_node`]) — renders the slice into
//!   tfvars ([`super::render`]) and shells `tofu plan -json` through the injectable
//!   [`CloudRunner`] seam, returning the pending-change [`PlanCounts`].
//! - **The drift tick** ([`drift_snapshot`]) — the throttled periodic plan (cadence
//!   decoupled from the state-mirror heartbeat) that folds the node's live domains +
//!   its desired slice into per-workload [`WorkloadRow`]s carrying a [`DriftFlag`],
//!   plus the node [`DriftSummary`] the `state/cloud/<node>` mirror publishes.
//!
//! Honest by construction (§7): a plan that fails to run leaves each row's drift
//! [`DriftFlag::Unknown`] rather than a fabricated `InSync`; a missing desired dir is
//! an honest empty slice, never an invented workload.

use std::path::{Path, PathBuf};

use mackes_mesh_types::cloud::{
    CloudInstance, DriftFlag, DriftSummary, PlanCounts, WorkloadRow, WorkloadSpec,
};

use super::path_key;
use super::render;
use super::runner::CloudRunner;

/// The desired-state root subtree, relative to the worker's `state_root`: the local
/// realization of the `/mcnf/cloud/desired/…` key namespace.
const DESIRED_SUBTREE: &str = "mcnf/cloud/desired";
const DESIRED_DOC_SUFFIX: &str = ".json";

/// The directory holding node `node`'s desired-state docs (`<state_root>/mcnf/cloud/
/// desired/<node>/`).
#[must_use]
pub(crate) fn desired_dir(state_root: &Path, node: &str) -> Result<PathBuf, String> {
    Ok(state_root
        .join(DESIRED_SUBTREE)
        .join(path_key::segment("node", node)?))
}

/// The path of workload `name`'s desired-state doc on node `node`.
#[must_use]
pub(crate) fn desired_doc_path(
    state_root: &Path,
    node: &str,
    name: &str,
) -> Result<PathBuf, String> {
    Ok(desired_dir(state_root, node)?.join(format!(
        "{}{}",
        path_key::file_stem("name", name, DESIRED_DOC_SUFFIX)?,
        DESIRED_DOC_SUFFIX
    )))
}

/// Read node `node`'s desired-state slice — every `*.json` doc under its desired
/// dir, parsed into a [`WorkloadSpec`], sorted by name for a deterministic render.
///
/// Honest: a missing dir (nothing declared yet) is an empty slice, and a single
/// unparseable/foreign doc is skipped rather than failing the whole read — the
/// declared, well-formed workloads still converge.
#[must_use]
pub(crate) fn read_desired_slice(state_root: &Path, node: &str) -> Vec<WorkloadSpec> {
    let Ok(dir) = desired_dir(state_root, node) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut specs: Vec<WorkloadSpec> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(spec) = serde_json::from_str::<WorkloadSpec>(&body) {
            specs.push(spec);
        }
    }
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
}

/// Persist workload `spec`'s desired-state doc under its placement node's desired
/// dir (creating the tree). The stable key is `spec.node` / `spec.name`.
///
/// # Errors
/// An I/O failure creating the tree or writing the doc (surfaced honestly to the
/// caller — never a silent success).
pub(crate) fn write_desired_doc(state_root: &Path, spec: &WorkloadSpec) -> Result<(), String> {
    let node = path_key::segment("node", &spec.node)?;
    let name = path_key::file_stem("name", &spec.name, DESIRED_DOC_SUFFIX)?;
    let dir = desired_dir(state_root, node)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create desired dir {}: {e}", dir.display()))?;
    let body =
        serde_json::to_string_pretty(spec).map_err(|e| format!("serialize desired doc: {e}"))?;
    let path = desired_doc_path(state_root, node, name)?;
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Remove workload `name`'s desired-state doc from node `node`. Returns whether a
/// doc was actually removed (a no-op remove of an absent workload is honest `false`,
/// not an error).
///
/// # Errors
/// An I/O failure other than "the doc was already absent".
pub(crate) fn remove_desired_doc(
    state_root: &Path,
    node: &str,
    name: &str,
) -> Result<bool, String> {
    let path = desired_doc_path(state_root, node, name)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("remove {}: {e}", path.display())),
    }
}

/// Render node `node`'s desired slice into tfvars and shell `tofu plan -json`
/// through `runner`, returning the pending-change [`PlanCounts`].
///
/// # Errors
/// An honest gate reason when the backend can't run the plan (tool absent / plan
/// failed) or the plan emitted no change summary — never a fabricated in-sync plan.
pub(crate) fn plan_counts_for_node(
    runner: &dyn CloudRunner,
    state_root: &Path,
    node: &str,
    libvirt_uri: &str,
) -> Result<PlanCounts, String> {
    path_key::segment("node", node)?;
    let specs = read_desired_slice(state_root, node);
    let tfvars = render::render_tfvars(node, &specs, libvirt_uri);
    let ndjson = runner.plan_json(&tfvars)?;
    render::parse_plan_counts(&ndjson)
}

/// The pure drift verdict for a node's rendered slice: a no-op plan is [`DriftFlag::
/// InSync`], any pending change is [`DriftFlag::Drift`].
#[must_use]
pub(crate) const fn drift_flag_from_plan(counts: PlanCounts) -> DriftFlag {
    if counts.is_noop() {
        DriftFlag::InSync
    } else {
        DriftFlag::Drift
    }
}

/// Fold a node's desired `specs` + its live `roster` (from `virsh list`) into the
/// per-workload [`WorkloadRow`]s the `state/cloud/<node>` mirror carries, stamping
/// each with the node-level `drift` verdict.
///
/// A desired workload with a matching live domain reads that domain's status +
/// reachable; a desired workload with no live domain reads `absent` + unreachable
/// (honest — it is declared but not running), which is itself a drift signal.
#[must_use]
pub(crate) fn fold_workload_rows(
    specs: &[WorkloadSpec],
    roster: &[CloudInstance],
    drift: DriftFlag,
) -> Vec<WorkloadRow> {
    specs
        .iter()
        .map(|spec| {
            let live = roster
                .iter()
                .find(|i| i.name == spec.name || i.id == spec.name);
            let status = live.map_or_else(|| "absent".to_string(), |i| i.status.to_lowercase());
            let reachable = live.is_some_and(|i| i.status.eq_ignore_ascii_case("ACTIVE"));
            // A declared-but-absent domain is drift regardless of the plan verdict.
            let row_drift = if live.is_none() && drift == DriftFlag::InSync {
                DriftFlag::Drift
            } else {
                drift
            };
            WorkloadRow {
                name: spec.name.clone(),
                delivery_type: spec.delivery_type,
                node: spec.node.clone(),
                status,
                cpu_pct: 0,
                mem_mb: 0,
                disk_gb: spec.disk_gb,
                reachable,
                drift: row_drift,
            }
        })
        .collect()
}

/// Run one drift tick for node `node`: read its desired slice, plan it, fold the
/// live roster into per-workload rows + the node [`DriftSummary`].
///
/// Honest by construction (§7):
/// - an empty desired slice is an honest "nothing declared" — zero rows, `drift_count
///   = 0`, `last_plan_ms = now` (no plan was needed);
/// - a plan that fails to run leaves each row [`DriftFlag::Unknown`] (never a
///   fabricated `InSync`) and `drift_count = 0` (drift is indeterminate, not zero).
#[must_use]
pub(crate) fn drift_snapshot(
    runner: &dyn CloudRunner,
    state_root: &Path,
    node: &str,
    libvirt_uri: &str,
    now_ms: i64,
) -> (Vec<WorkloadRow>, DriftSummary) {
    let specs = read_desired_slice(state_root, node);
    if specs.is_empty() {
        return (
            Vec::new(),
            DriftSummary {
                drift_count: 0,
                last_plan_ms: now_ms,
            },
        );
    }
    let roster = runner.list_instances().unwrap_or_default();
    let drift = match plan_counts_for_node(runner, state_root, node, libvirt_uri) {
        Ok(counts) => drift_flag_from_plan(counts),
        // A plan we couldn't run leaves drift indeterminate — never a faked in-sync.
        Err(_) => DriftFlag::Unknown,
    };
    let rows = fold_workload_rows(&specs, &roster, drift);
    let drift_count =
        u32::try_from(rows.iter().filter(|r| r.drift == DriftFlag::Drift).count()).unwrap_or(0);
    (
        rows,
        DriftSummary {
            drift_count,
            last_plan_ms: now_ms,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::runner::fake::{instance, FakeRunner};
    use super::*;
    use mackes_mesh_types::cloud::DeliveryType;

    fn spec(name: &str, node: &str) -> WorkloadSpec {
        WorkloadSpec {
            name: name.to_string(),
            delivery_type: DeliveryType::ServiceVm,
            node: node.to_string(),
            vcpu: 2,
            memory_mb: 2048,
            disk_gb: 20,
            image: None,
            network_isolation: false,
            raw_hcl: None,
        }
    }

    #[test]
    fn desired_docs_round_trip_per_node_and_remove_is_honest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Two workloads on node "eagle", one on "otter".
        write_desired_doc(root, &spec("web", "eagle")).unwrap();
        write_desired_doc(root, &spec("db", "eagle")).unwrap();
        write_desired_doc(root, &spec("far", "otter")).unwrap();

        // A node reads ONLY its own slice (per-node apply), name-sorted.
        let eagle = read_desired_slice(root, "eagle");
        assert_eq!(eagle.len(), 2);
        assert_eq!(eagle[0].name, "db");
        assert_eq!(eagle[1].name, "web");
        assert_eq!(read_desired_slice(root, "otter").len(), 1);
        // A node with nothing declared is an honest empty slice, never invented.
        assert!(read_desired_slice(root, "ghost").is_empty());

        // Remove is idempotent + honest about whether it did anything.
        assert!(remove_desired_doc(root, "eagle", "web").unwrap());
        assert!(!remove_desired_doc(root, "eagle", "web").unwrap());
        assert_eq!(read_desired_slice(root, "eagle").len(), 1);
    }

    #[test]
    fn write_rejects_a_spec_missing_its_node_or_name() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(write_desired_doc(tmp.path(), &spec("x", "  ")).is_err());
        assert!(write_desired_doc(tmp.path(), &spec("  ", "n")).is_err());
    }

    #[test]
    fn desired_store_rejects_absolute_parent_and_separator_segments() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("state");
        let outside = tmp.path().join("outside");
        let absolute_node = outside.to_string_lossy().into_owned();
        let absolute_name = outside.join("victim").to_string_lossy().into_owned();

        for bad_node in [&absolute_node, "../escape", "node/child", ".", ".."] {
            let err = write_desired_doc(&root, &spec("proof", bad_node)).unwrap_err();
            assert!(err.contains("path-safe"), "unexpected error: {err}");
        }
        for bad_name in [&absolute_name, "../escape", "name/child", ".", ".."] {
            let err = write_desired_doc(&root, &spec(bad_name, "eagle")).unwrap_err();
            assert!(err.contains("path-safe"), "unexpected error: {err}");
        }
        assert!(
            !outside.exists(),
            "an untrusted absolute node/name must not create an outside path"
        );
    }

    #[test]
    fn desired_store_rejects_suffix_overflow_before_creating_the_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("state");
        let long_name = "x".repeat(251);

        let err = write_desired_doc(&root, &spec(&long_name, "eagle")).unwrap_err();
        assert!(err.contains("too long"), "unexpected error: {err}");
        assert!(!root.exists(), "invalid filename must fail before I/O");
        assert!(remove_desired_doc(&root, "eagle", &long_name).is_err());
    }

    #[test]
    fn desired_remove_rejects_escape_and_preserves_the_outside_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("state");
        let outside = tmp.path().join("victim.json");
        std::fs::write(&outside, "keep").unwrap();
        let attack_name = tmp.path().join("victim").to_string_lossy().into_owned();

        assert!(remove_desired_doc(&root, "eagle", &attack_name).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
        assert!(remove_desired_doc(&root, "../escape", "victim").is_err());
    }

    #[test]
    fn desired_store_accepts_hostname_and_workload_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        write_desired_doc(tmp.path(), &spec("web_api.v2", "node-1.example_lab")).unwrap();
        let slice = read_desired_slice(tmp.path(), "node-1.example_lab");
        assert_eq!(slice.len(), 1);
        assert_eq!(slice[0].name, "web_api.v2");
    }

    #[test]
    fn drift_flag_maps_noop_to_in_sync_and_change_to_drift() {
        assert_eq!(
            drift_flag_from_plan(PlanCounts::default()),
            DriftFlag::InSync
        );
        assert_eq!(
            drift_flag_from_plan(PlanCounts {
                add: 1,
                ..Default::default()
            }),
            DriftFlag::Drift
        );
    }

    #[test]
    fn fold_marks_a_running_domain_in_sync_and_an_absent_one_drifted() {
        let specs = vec![spec("web", "eagle"), spec("db", "eagle")];
        let roster = vec![instance("web", "ACTIVE")];
        let rows = fold_workload_rows(&specs, &roster, DriftFlag::InSync);
        assert_eq!(rows.len(), 2);
        let web = rows.iter().find(|r| r.name == "web").unwrap();
        assert_eq!(web.status, "active");
        assert!(web.reachable);
        assert_eq!(web.drift, DriftFlag::InSync);
        // The declared-but-absent "db" is drift even though the node plan was a no-op.
        let db = rows.iter().find(|r| r.name == "db").unwrap();
        assert_eq!(db.status, "absent");
        assert!(!db.reachable);
        assert_eq!(db.drift, DriftFlag::Drift);
    }

    #[test]
    fn drift_snapshot_of_an_empty_slice_is_honestly_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();
        let (rows, summary) = drift_snapshot(&runner, tmp.path(), "eagle", "qemu:///system", 100);
        assert!(rows.is_empty());
        assert_eq!(summary.drift_count, 0);
        assert_eq!(summary.last_plan_ms, 100);
    }

    #[test]
    fn drift_snapshot_plans_the_slice_and_counts_drift() {
        let tmp = tempfile::tempdir().unwrap();
        write_desired_doc(tmp.path(), &spec("web", "eagle")).unwrap();
        // A plan that reports pending changes ⇒ the workload is drifted.
        let runner = FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            plan_ndjson: Some(
                r#"{"type":"change_summary","changes":{"add":1,"change":0,"remove":0}}"#.into(),
            ),
            ..Default::default()
        };
        let (rows, summary) = drift_snapshot(&runner, tmp.path(), "eagle", "qemu:///system", 200);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].drift, DriftFlag::Drift);
        assert_eq!(summary.drift_count, 1);
        assert_eq!(summary.last_plan_ms, 200);
        // The renderer wrote the node's tfvars through the seam.
        assert_eq!(runner.tfvars_written.lock().unwrap().len(), 1);
    }

    #[test]
    fn drift_snapshot_leaves_drift_unknown_when_the_plan_cannot_run() {
        let tmp = tempfile::tempdir().unwrap();
        write_desired_doc(tmp.path(), &spec("web", "eagle")).unwrap();
        // §7 — a plan that couldn't run is Unknown drift, never a fabricated InSync.
        let runner = FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            plan_err: Some("tofu: command not found".into()),
            ..Default::default()
        };
        let (rows, summary) = drift_snapshot(&runner, tmp.path(), "eagle", "qemu:///system", 300);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].drift, DriftFlag::Unknown);
        assert_eq!(summary.drift_count, 0, "indeterminate is not zero-drift");
    }
}
