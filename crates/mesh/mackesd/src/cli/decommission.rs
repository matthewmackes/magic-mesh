//! `Decommission` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `decommission` subcommand.
#[allow(unreachable_code)]
pub fn run(node_id: String, force: bool, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // Phase 12.3.4 — soft-delete the node row and emit a
        // hash-chained Lifecycle event so the audit trail
        // records the operator action. `--force` only changes
        // the audit kind label; the SQL effect is identical
        // (CHECK constraint enforces the same allowed roles).
        let mut conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        let updated = mackesd_core::store::set_node_role(&conn, &node_id, "decommissioned")?;
        if updated == 0 {
            eprintln!("mackesd decommission: no node row matches {node_id}");
            std::process::exit(2);
        }
        let payload = serde_json::json!({
            "kind":  if force { "forced" } else { "soft" },
            "node":  node_id,
            "event": "decommission",
        })
        .to_string();
        mackesd_core::store::insert_event(&mut conn, "lifecycle", &default_node_id(), &payload)?;
        let report = serde_json::json!({
            "decommission":     node_id,
            "kind":             if force { "forced" } else { "soft" },
            "history_retained": true,
            "audit_logged":     true,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}
