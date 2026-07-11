//! `Reenroll` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `reenroll` subcommand.
#[allow(unreachable_code)]
pub fn run(node_id: String, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // Phase 12.3.5 — mint a fresh keypair and write its
        // hex public key into the existing node row. Lifecycle
        // event records the old fingerprint so a forensic
        // walker can correlate before/after.
        let mut conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        let prior = mackesd_core::store::list_nodes(&conn)?
            .into_iter()
            .find(|n| n.node_id == node_id);
        let new_identity = mackesd_core::enrollment::build_identity();
        let new_fp = new_identity.key.fingerprint();
        let updated = mackesd_core::store::refresh_node_credentials(&conn, &node_id, &new_fp)?;
        if updated == 0 {
            eprintln!("mackesd reenroll: no node row matches {node_id}");
            std::process::exit(2);
        }
        let payload = serde_json::json!({
            "event":           "reenroll",
            "node":            node_id,
            "old_fingerprint": prior.map(|p| p.public_key),
            "new_fingerprint": &new_fp,
        })
        .to_string();
        mackesd_core::store::insert_event(&mut conn, "lifecycle", &default_node_id(), &payload)?;
        let report = serde_json::json!({
            "reenroll":         node_id,
            "new_fingerprint":  new_fp,
            "history_retained": true,
            "audit_logged":     true,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}
