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

/// Read every plan TOML (junk-tolerant), plus the built-in core pack
/// (the W50 core policies' stock remediations). On-disk plans with the
/// same `name` as a core plan override it.
#[must_use]
pub fn load_plans(workgroup_root: &Path) -> Vec<RemediationPlan> {
    let mut by_name: BTreeMap<String, RemediationPlan> = core_pack()
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();
    if let Ok(entries) = std::fs::read_dir(remediation_dir(workgroup_root)) {
        for e in entries.filter_map(Result::ok) {
            if e.path().extension().is_some_and(|x| x == "toml") {
                if let Ok(raw) = std::fs::read_to_string(e.path()) {
                    if let Ok(p) = toml::from_str::<RemediationPlan>(&raw) {
                        by_name.insert(p.name.clone(), p);
                    }
                }
            }
        }
    }
    by_name.into_values().collect()
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
}
