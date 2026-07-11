//! `Revisions` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `revisions` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: RevisionsCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // v2.0.0 Phase F.12 — desired_config revision management.
        let conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        match cmd {
            RevisionsCmd::List { json } => {
                let rows = list_revisions(&conn)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else {
                    print_revisions_table(&rows);
                }
            }
            RevisionsCmd::Diff { from, to } => {
                let a = load_revision_payload(&conn, &from)?;
                let b = load_revision_payload(&conn, &to)?;
                let report = serde_json::json!({
                    "from":     from,
                    "to":       to,
                    "from_len": a.len(),
                    "to_len":   b.len(),
                    // Surface the raw payloads so the operator + the
                    // Workbench panel can diff them visually.
                    "from_payload": a,
                    "to_payload":   b,
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            RevisionsCmd::Rollback {
                target_id,
                author,
                peers,
            } => {
                let payload = load_revision_payload(&conn, &target_id)?;
                let author = author.unwrap_or_else(default_node_id);
                let summary = format!("Rollback to {target_id} (peers={peers})");
                let mut conn = conn;
                let now = chrono::Utc::now().to_rfc3339();
                let revision_id = mackesd_core::store::with_transaction(&mut conn, |tx| {
                    tx.execute(
                        "INSERT INTO desired_config \
                                 (author, message, spec_json, state, created_at) \
                                 VALUES (?, ?, ?, 'approved', ?)",
                        (&author, &summary, &payload, &now),
                    )
                    .map_err(|e| anyhow::anyhow!("inserting rollback revision: {e}"))?;
                    Ok(tx.last_insert_rowid())
                })?;
                let report = serde_json::json!({
                    "rollback":      target_id,
                    "new_revision":  revision_id,
                    "author":        author,
                    "peers":         peers,
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        }
    }
    Ok(())
}
