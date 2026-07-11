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

/// Read a revision's `spec_json` payload by id.
fn load_revision_payload(conn: &rusqlite::Connection, revision_id: &str) -> anyhow::Result<String> {
    let rev: i64 = revision_id
        .parse()
        .map_err(|_| anyhow::anyhow!("revision id must be an integer (got {revision_id})"))?;
    let payload: String = conn
        .query_row(
            "SELECT spec_json FROM desired_config WHERE revision_id = ?",
            [rev],
            |r| r.get(0),
        )
        .with_context(|| format!("loading revision {revision_id}"))?;
    Ok(payload)
}

/// List every revision (descending by id).
fn list_revisions(conn: &rusqlite::Connection) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut stmt = conn
        .prepare(
            "SELECT revision_id, author, message, state, created_at \
             FROM desired_config ORDER BY revision_id DESC",
        )
        .context("preparing revisions list")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(serde_json::json!({
                "revision_id":  r.get::<_, i64>(0)?.to_string(),
                "author":       r.get::<_, String>(1)?,
                "summary":      r.get::<_, String>(2)?,
                "state":        r.get::<_, String>(3)?,
                "created_at":   r.get::<_, String>(4)?,
            }))
        })
        .context("executing revisions list")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("materializing revisions list")?;
    Ok(rows)
}

fn print_revisions_table(rows: &[serde_json::Value]) {
    if rows.is_empty() {
        println!("(no revisions)");
        return;
    }
    for row in rows {
        let rid = row
            .get("revision_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let st = row.get("state").and_then(|v| v.as_str()).unwrap_or("?");
        let aut = row.get("author").and_then(|v| v.as_str()).unwrap_or("?");
        let cre = row
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let sm = row.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        println!("{rid:>6}  [{st}]  {aut:<16}  {cre}  {sm}");
    }
}
