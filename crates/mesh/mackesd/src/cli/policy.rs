//! `Policy` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `policy` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: PolicyCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // PLANES-13 — the policy engine surface. Evaluates the loaded
        // policies (core pack + on-disk TOML) against the live
        // directory and reports per-policy compliance.
        use mackesd_core::policy_engine;
        let PolicyCmd::List { json } = cmd;
        let root = mackesd_core::default_qnm_shared_root();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let svc = mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
        let dir = svc.build_directory(now);
        let peers: Vec<(String, serde_json::Value)> = dir["peers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["hostname"].as_str().map(|h| (h.to_string(), p.clone())))
            .collect();
        let policies = policy_engine::load_policies(&root);
        // For each policy, the peers that currently violate it.
        let rows: Vec<serde_json::Value> = policies
            .iter()
            .map(|pol| {
                let violated: Vec<&str> = peers
                    .iter()
                    .filter(|(_, rec)| !pol.holds(rec))
                    .map(|(h, _)| h.as_str())
                    .collect();
                serde_json::json!({
                    "name": pol.name,
                    "description": pol.description,
                    "field": pol.field,
                    "op": pol.op,
                    "expected": pol.expected,
                    "severity": pol.severity,
                    "violated_peers": violated,
                })
            })
            .collect();
        if json {
            println!("{}", serde_json::to_string(&rows)?);
        } else {
            println!(
                "{:<22} {:<8} {:<24} {:<8}",
                "POLICY", "SEVERITY", "ASSERTION", "STATUS"
            );
            for (pol, row) in policies.iter().zip(&rows) {
                let n = row["violated_peers"].as_array().map_or(0, Vec::len);
                let status = if n == 0 {
                    "ok".to_string()
                } else {
                    format!("{n} violating")
                };
                println!(
                    "{:<22} {:<8} {:<24} {:<8}",
                    pol.name,
                    pol.severity.as_str(),
                    format!("{} {:?} {}", pol.field, pol.op, pol.expected),
                    status
                );
            }
        }
        return Ok(());
    }
    Ok(())
}
