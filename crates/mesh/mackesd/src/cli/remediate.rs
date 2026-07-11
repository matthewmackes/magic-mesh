//! `Remediate` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `remediate` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: RemediateCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // PLANES-11 — the remediation layer. Wires PLANES-13's
        // policy engine (which had no caller) to the job system:
        // evaluate policies → match plans → fire signed bundles.
        use mackesd_core::{policy_engine, remediation};
        let root = mackesd_core::default_qnm_shared_root();
        match cmd {
            RemediateCmd::Plans { json } => {
                let plans = remediation::load_plans(&root);
                if json {
                    println!("{}", serde_json::to_string(&plans)?);
                } else {
                    println!(
                        "{:<22} {:<20} {:<22} {:<5}",
                        "PLAN", "POLICY", "TEMPLATE", "AUTO"
                    );
                    for p in &plans {
                        println!(
                            "{:<22} {:<20} {:<22} {:<5}",
                            p.name, p.policy, p.template, p.auto
                        );
                    }
                }
                return Ok(());
            }
            RemediateCmd::Match { json } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let svc = mackesd_core::ipc::directory::DirectoryService::new(
                    &root,
                    Some(db_path.clone()),
                );
                let dir = svc.build_directory(now);
                let peers: Vec<(String, serde_json::Value)> = dir["peers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p["hostname"].as_str().map(|h| (h.to_string(), p.clone())))
                    .collect();
                let policies = policy_engine::load_policies(&root);
                let violations = policy_engine::evaluate(&policies, &peers);
                let plans = remediation::load_plans(&root);
                let matched = remediation::match_all(&plans, &violations);
                if json {
                    println!("{}", serde_json::to_string(&matched)?);
                } else if matched.is_empty() {
                    println!("no drift — every policy holds across {} peers", peers.len());
                } else {
                    println!(
                        "{:<14} {:<20} {:<8} {:<22} {:<5}",
                        "PEER", "POLICY", "SEV", "PLAN", "AUTO"
                    );
                    for m in &matched {
                        println!(
                            "{:<14} {:<20} {:<8} {:<22} {:<5}",
                            m.violation.peer,
                            m.violation.policy,
                            m.violation.severity,
                            m.plan.as_deref().unwrap_or("(none)"),
                            m.auto
                        );
                    }
                }
                return Ok(());
            }
            RemediateCmd::Fire { plan, peer } => {
                let plans = remediation::load_plans(&root);
                let Some(p) = plans.iter().find(|x| x.name == plan) else {
                    anyhow::bail!("no remediation plan named '{plan}' (mded remediate plans)");
                };
                // Bind the event vars from a synthesized violation
                // for this (policy, peer) — the operator-fire path.
                let v = policy_engine::Violation {
                    policy: p.policy.clone(),
                    peer: peer.clone(),
                    severity: "warn".into(),
                    detail: format!("operator fire of '{plan}'"),
                };
                let vars = remediation::bind_vars(p, &v);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let run_id = format!("rem-{now}");
                let body = serde_json::json!({
                    "playbook": p.template,
                    "targets": { "peers": [peer] },
                    "vars": vars,
                });
                let jobs_svc =
                    mackesd_core::ipc::jobs::JobsService::new(&root, Some(db_path.clone()));
                let reply = mackesd_core::ipc::jobs::build_reply(
                    &jobs_svc,
                    "launch",
                    Some(&body.to_string()),
                    &run_id,
                );
                // Loud (W42): the launch reply — run id + resolved
                // targets — prints for the operator / audit trail.
                println!("{reply}");
                return Ok(());
            }
        }
    }
    Ok(())
}
