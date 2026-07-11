//! `FleetStatus` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `fleet-status` subcommand.
#[allow(unreachable_code)]
pub fn run(json: bool, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // Roster source is the replicated directory, not the local
        // sqlite `nodes` table (empty mesh-wide — see
        // directory_to_node_rows). This is what makes Fleet Rollup
        // group the real fleet instead of "no enrolled nodes".
        let root = mackesd_core::default_qnm_shared_root();
        let svc = mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let nodes = directory_to_node_rows(&svc.build_directory(now));
        let pairs: Vec<(String, String)> = nodes
            .iter()
            .map(|n| (n.role.clone(), n.health.clone()))
            .collect();
        let groups = mackesd_core::fleet_rollup::rollup(&pairs);
        if json {
            println!(
                "{}",
                serde_json::json!({ "total": nodes.len(), "groups": groups })
            );
        } else if groups.is_empty() {
            println!("fleet empty (no enrolled nodes)");
        } else {
            println!("{:<14} {:>5}  {:<12}", "ROLE", "TOTAL", "WORST HEALTH");
            for g in &groups {
                println!("{:<14} {:>5}  {:<12}", g.role, g.total, g.worst_health);
            }
        }
    }
    Ok(())
}
