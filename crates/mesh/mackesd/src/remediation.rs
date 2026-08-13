//! PLANES-11 — the remediation layer (W41/W42).
//!
//! A **remediation plan** maps a policy violation (a drift event, W49)
//! to a **job template** plus **event-var bindings**, and carries a
//! per-plan **auto** flag (W42, default off — an auto plan fires the
//! moment its drift appears, and fires *loud*: the fire is audit-logged
//! like any operator-initiated run).
//!
//! This is the pure core: plans are TOML on the Syncthing share
//! (`<workgroup_root>/remediation/*.toml`, W88 — fleet state is TOML
//! dirs + typed Bus verbs), junk-tolerant on read, plus a built-in
//! **core pack** that pairs the W50 core policies with their stock
//! remediation templates. The `mackesd remediate` CLI verb (match /
//! fire) and the Controller ▸ Remediation panel render on top; the
//! leader sweep (W48) fires the auto plans.
//!
//! No raw shell channel — a fire enqueues a signed job bundle that the
//! TARGET runs locally (W21/W32). This module only resolves *which*
//! template + vars; the job system owns execution.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::policy_engine::Violation;

/// Maximum number of candidate plan files inspected in one load.
pub const MAX_REMEDIATION_PLAN_FILES: usize = 128;
/// Maximum encoded size of one remediation plan.
pub const MAX_REMEDIATION_PLAN_BYTES: u64 = 16 * 1024;
/// Maximum number of static bindings admitted from one plan.
pub const MAX_REMEDIATION_BINDINGS: usize = 32;
const MAX_REMEDIATION_ID_BYTES: usize = 96;
const MAX_REMEDIATION_VALUE_BYTES: usize = 512;

/// One remediation plan: when policy `policy` is violated, fire job
/// template `template` with `bindings` (static vars) plus the event
/// vars bound from the violation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemediationPlan {
    /// Stable id (also the audit-event `remediate.<name>`).
    pub name: String,
    /// The policy this plan remediates. `"*"` matches any policy (a
    /// catch-all plan); otherwise an exact policy-name match (W41).
    pub policy: String,
    /// The job-template id fired against the drifted peer (W41 — a
    /// template ref, not an inline playbook).
    pub template: String,
    /// W42 — auto-fire flag. Default **off**: an operator fires the
    /// plan from the panel. When on, the leader sweep fires it the
    /// moment the drift appears (loud — audit-logged).
    #[serde(default)]
    pub auto: bool,
    /// Static template-var bindings, merged under the event vars.
    #[serde(default)]
    pub bindings: BTreeMap<String, String>,
}

/// A drift event matched against the loaded plans: the violation, the
/// plan that remediates it (if any), and the resolved fire inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatchedDrift {
    /// The originating violation (W49 — a violation *is* a drift event).
    pub violation: Violation,
    /// The matched plan's name, or `None` when no plan covers it.
    pub plan: Option<String>,
    /// The job template the matched plan would fire.
    pub template: Option<String>,
    /// The matched plan's auto flag (W42).
    pub auto: bool,
    /// The fully-bound template vars (event vars + the plan's static
    /// bindings), ready to hand the job system on fire.
    pub vars: BTreeMap<String, String>,
}

/// The remediation-plans directory (`<root>/remediation/`).
#[must_use]
pub fn remediation_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("remediation")
}

/// Read bounded, regular plan TOML files plus the built-in core pack.
///
/// One uniquely named, valid on-disk plan may override a core plan. Duplicate
/// names are refused as a group instead of making filesystem enumeration order
/// recovery authority. Symlinks, non-regular files, oversized files, and
/// malformed or unbounded contracts are ignored.
#[must_use]
pub fn load_plans(workgroup_root: &Path) -> Vec<RemediationPlan> {
    let mut by_name: BTreeMap<String, RemediationPlan> = core_pack()
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();
    let mut candidates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(remediation_dir(workgroup_root)) {
        candidates.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
    }
    candidates.sort();
    candidates.truncate(MAX_REMEDIATION_PLAN_FILES);

    let mut admitted: BTreeMap<String, Option<RemediationPlan>> = BTreeMap::new();
    for path in candidates {
        if path.extension().is_none_or(|extension| extension != "toml") {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_REMEDIATION_PLAN_BYTES {
            continue;
        }
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        if raw.len() as u64 > MAX_REMEDIATION_PLAN_BYTES {
            continue;
        }
        let Ok(raw) = std::str::from_utf8(&raw) else {
            continue;
        };
        let Ok(plan) = toml::from_str::<RemediationPlan>(raw) else {
            continue;
        };
        if !valid_plan(&plan) {
            continue;
        }
        admitted
            .entry(plan.name.clone())
            .and_modify(|candidate| *candidate = None)
            .or_insert(Some(plan));
    }
    for (name, plan) in admitted {
        if let Some(plan) = plan {
            by_name.insert(name, plan);
        }
    }
    by_name.into_values().collect()
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REMEDIATION_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_plan(plan: &RemediationPlan) -> bool {
    valid_identifier(&plan.name)
        && (plan.policy == "*" || valid_identifier(&plan.policy))
        && valid_identifier(&plan.template)
        && plan.bindings.len() <= MAX_REMEDIATION_BINDINGS
        && plan.bindings.iter().all(|(key, value)| {
            valid_identifier(key)
                && value.len() <= MAX_REMEDIATION_VALUE_BYTES
                && !key.starts_with("drift_")
        })
}

/// The platform's stock remediations for the W50 core policies — both
/// **default off** (operator fires them; W42). They pair the two core
/// invariants with the typed jobs that fix them.
#[must_use]
pub fn core_pack() -> Vec<RemediationPlan> {
    vec![
        RemediationPlan {
            name: "resync-behind-node".into(),
            policy: "all-nodes-current".into(),
            template: "reconcile-config".into(),
            auto: false,
            bindings: BTreeMap::new(),
        },
        RemediationPlan {
            name: "clear-critical-alarm".into(),
            policy: "no-critical-alarms".into(),
            template: "restart-mesh-services".into(),
            auto: false,
            bindings: BTreeMap::new(),
        },
    ]
}

/// Find the plan that remediates `violation`: an exact policy-name
/// match wins over a `"*"` catch-all (W41). Returns the first exact
/// match, else the first catch-all, else `None`.
#[must_use]
pub fn match_plan<'a>(
    plans: &'a [RemediationPlan],
    violation: &Violation,
) -> Option<&'a RemediationPlan> {
    plans
        .iter()
        .find(|p| p.policy == violation.policy)
        .or_else(|| plans.iter().find(|p| p.policy == "*"))
}

/// Bind the event vars from a violation onto a plan's static bindings
/// (W41 — event var bindings). Event vars use the `drift_` prefix so
/// they never collide with a plan's own keys; the plan's static
/// bindings are applied first, event vars layered on top.
#[must_use]
pub fn bind_vars(plan: &RemediationPlan, violation: &Violation) -> BTreeMap<String, String> {
    let mut vars = plan.bindings.clone();
    vars.insert("drift_peer".into(), violation.peer.clone());
    vars.insert("drift_policy".into(), violation.policy.clone());
    vars.insert("drift_severity".into(), violation.severity.clone());
    vars.insert("drift_detail".into(), violation.detail.clone());
    vars
}

/// Match every violation against the plan set, resolving the fire
/// inputs for the ones a plan covers (the panel + the leader sweep
/// both consume this).
#[must_use]
pub fn match_all(plans: &[RemediationPlan], violations: &[Violation]) -> Vec<MatchedDrift> {
    violations
        .iter()
        .map(|v| match match_plan(plans, v) {
            Some(p) => MatchedDrift {
                violation: v.clone(),
                plan: Some(p.name.clone()),
                template: Some(p.template.clone()),
                auto: p.auto,
                vars: bind_vars(p, v),
            },
            None => MatchedDrift {
                violation: v.clone(),
                plan: None,
                template: None,
                auto: false,
                vars: BTreeMap::new(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn violation(policy: &str, peer: &str) -> Violation {
        Violation {
            policy: policy.into(),
            peer: peer.into(),
            severity: "warn".into(),
            detail: "x != y — failed".into(),
        }
    }

    #[test]
    fn core_pack_plans_are_default_off() {
        // W42 — auto fire is opt-in; the shipped plans never auto-fire.
        for p in core_pack() {
            assert!(!p.auto, "core plan {} must default to auto=off", p.name);
        }
    }

    #[test]
    fn core_pack_covers_the_two_core_policies() {
        let plans = core_pack();
        for policy in ["all-nodes-current", "no-critical-alarms"] {
            assert!(
                match_plan(&plans, &violation(policy, "pine")).is_some(),
                "core pack must remediate {policy}"
            );
        }
    }

    #[test]
    fn exact_policy_match_beats_catch_all() {
        let plans = vec![
            RemediationPlan {
                name: "catch".into(),
                policy: "*".into(),
                template: "generic".into(),
                auto: false,
                bindings: BTreeMap::new(),
            },
            RemediationPlan {
                name: "specific".into(),
                policy: "all-nodes-current".into(),
                template: "reconcile-config".into(),
                auto: false,
                bindings: BTreeMap::new(),
            },
        ];
        let m = match_plan(&plans, &violation("all-nodes-current", "pine")).unwrap();
        assert_eq!(m.name, "specific");
        // An unmatched policy falls through to the catch-all.
        let c = match_plan(&plans, &violation("some-other-policy", "pine")).unwrap();
        assert_eq!(c.name, "catch");
    }

    #[test]
    fn unmatched_violation_yields_no_plan() {
        let plans = core_pack(); // no catch-all
        let matched = match_all(&plans, &[violation("never-heard-of-it", "pine")]);
        assert_eq!(matched.len(), 1);
        assert!(matched[0].plan.is_none());
        assert!(matched[0].template.is_none());
        assert!(matched[0].vars.is_empty());
    }

    #[test]
    fn bind_vars_carries_the_event_into_template_vars() {
        let plan = RemediationPlan {
            name: "p".into(),
            policy: "all-nodes-current".into(),
            template: "reconcile-config".into(),
            auto: false,
            bindings: BTreeMap::from([("mode".into(), "safe".into())]),
        };
        let vars = bind_vars(&plan, &violation("all-nodes-current", "birch"));
        assert_eq!(vars.get("mode").map(String::as_str), Some("safe"));
        assert_eq!(vars.get("drift_peer").map(String::as_str), Some("birch"));
        assert_eq!(
            vars.get("drift_policy").map(String::as_str),
            Some("all-nodes-current")
        );
    }

    #[test]
    fn on_disk_plan_overrides_a_core_plan_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(remediation_dir(tmp.path())).unwrap();
        std::fs::write(
            remediation_dir(tmp.path()).join("resync.toml"),
            "name = \"resync-behind-node\"\npolicy = \"all-nodes-current\"\n\
             template = \"custom-resync\"\nauto = true\n",
        )
        .unwrap();
        let plans = load_plans(tmp.path());
        let p = plans
            .iter()
            .find(|p| p.name == "resync-behind-node")
            .unwrap();
        // The on-disk plan replaced the core one (template + auto).
        assert_eq!(p.template, "custom-resync");
        assert!(p.auto);
        // Still only one plan with that name (override, not duplicate).
        assert_eq!(
            plans
                .iter()
                .filter(|p| p.name == "resync-behind-node")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn hostile_duplicate_symlink_and_unbounded_plans_cannot_substitute_recovery_authority() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let dir = remediation_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        let substituted = "name = \"resync-behind-node\"\npolicy = \"*\"\n\
             template = \"hostile-recovery\"\nauto = true\n";
        std::fs::write(dir.join("a.toml"), substituted).unwrap();
        std::fs::write(dir.join("b.toml"), substituted.replace("hostile", "second")).unwrap();

        let outside = tmp.path().join("outside.toml");
        std::fs::write(
            &outside,
            "name = \"symlink-plan\"\npolicy = \"*\"\ntemplate = \"escape\"\nauto = true\n",
        )
        .unwrap();
        symlink(&outside, dir.join("symlink.toml")).unwrap();

        let oversized = std::fs::File::create(dir.join("oversized.toml")).unwrap();
        oversized
            .set_len(MAX_REMEDIATION_PLAN_BYTES + 1)
            .unwrap();
        std::fs::write(
            dir.join("reserved-binding.toml"),
            "name = \"reserved\"\npolicy = \"*\"\ntemplate = \"safe\"\nauto = true\n\
             [bindings]\ndrift_peer = \"substituted\"\n",
        )
        .unwrap();

        let plans = load_plans(tmp.path());
        let core = plans
            .iter()
            .find(|plan| plan.name == "resync-behind-node")
            .expect("duplicate override must leave the core authority intact");
        assert_eq!(core.template, "reconcile-config");
        assert!(!core.auto);
        assert!(!plans.iter().any(|plan| {
            matches!(plan.name.as_str(), "symlink-plan" | "reserved")
                || plan.template.contains("hostile")
                || plan.template.contains("second")
        }));
    }
}
