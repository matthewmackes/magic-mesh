//! `Reenroll` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `reenroll` subcommand.
pub fn run(node_id: String, db_path: PathBuf) -> anyhow::Result<()> {
    let root = mackesd_core::default_qnm_shared_root();
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().max(1))
        .unwrap_or(1);
    let plan = mackes_mesh_types::lifecycle::LifecyclePlanV1 {
        schema_version: 1,
        request_id: format!("reenroll-{node_id}-{generation}"),
        target_id: node_id.clone(),
        intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::VerifyAndCorrect,
        generation,
        steps: vec!["identity".into()],
    };
    let mut authority =
        mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, plan)
            .map_err(|error| anyhow::anyhow!("cannot acquire reenroll authority: {error:?}"))?;
    let result =
        authority.run_next(|_| run_inner(node_id, db_path).map_err(|error| error.to_string()));
    if let Err(error) = result {
        let _ = authority.finish();
        return Err(anyhow::anyhow!("reenroll lifecycle failure: {error:?}"));
    }
    authority
        .finish()
        .map_err(|error| anyhow::anyhow!("cannot release reenroll authority: {error:?}"))
}

fn run_inner(node_id: String, db_path: PathBuf) -> anyhow::Result<()> {
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
    Ok(())
}
