//! INST-7 prerequisite — peer certificate revocation.
//!
//! Implements `mackesd ca revoke <node-id>` so the `mde-install`
//! wipe sequence can cleanly depart the mesh without needing a
//! D-Bus connection to mackesd. Three steps execute in order:
//!
//! 1. **DB mark** — set `nebula_peer_certs.revoked_at` for every row
//!    belonging to `node_id` so the enrollment gate rejects re-use
//!    of the cert serial.
//! 2. **Ban list** — add `node_id` to this peer's ban list in
//!    QNM-Shared so the identity is refused mesh-wide even after a CA
//!    rotation. GFS replication propagates the ban automatically.
//! 3. **Bus event** (best-effort) — publish `ca/revoke/<node-id>` so
//!    running workers (meshfs_worker, peer_cap, etc.) converge without
//!    waiting for their next tick.
//!
//! This replaces the originally-planned `dev.mackes.MDE.Ca.Revoke`
//! D-Bus method. D-Bus retires by 1.0 per AI_GOVERNANCE §3.3; the
//! dbus-shape lint blocks net-new MDE-internal interfaces. A CLI
//! subcommand is the correct surface: it's synchronous, operator-
//! auditable, and usable from `mde-install` via `Command::new`.

use std::path::Path;

use anyhow::Context as _;

/// Revoke a peer certificate.
///
/// Marks every row for `node_id` in `nebula_peer_certs` as revoked,
/// adds the node-id to this peer's ban list so the identity can't
/// re-enroll, and fires a best-effort Bus event.
///
/// `workgroup_root` is the QNM-Shared / mesh-home root (used to locate
/// the local ban-list file). `self_node_id` is the local peer's
/// stable node-id (the ban list is keyed by it).
///
/// Returns the number of database rows marked revoked (0 when the
/// node had no active certs — the operation is still considered
/// successful, and the ban-list write happens regardless).
///
/// # Errors
/// Database write failures or ban-list I/O errors are returned.
/// The Bus publish step never fails the function — it is
/// best-effort and any error is logged + ignored.
pub fn revoke_peer(
    conn: &rusqlite::Connection,
    workgroup_root: &Path,
    self_node_id: &str,
    node_id: &str,
) -> anyhow::Result<u32> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let rows = conn
        .execute(
            "UPDATE nebula_peer_certs SET revoked_at = ?1 \
             WHERE node_id = ?2 AND revoked_at IS NULL",
            rusqlite::params![now_ms, node_id],
        )
        .context("revoke: update nebula_peer_certs")?;

    crate::ca::ban_list::add_banned(workgroup_root, self_node_id, node_id)
        .map_err(|e| anyhow::anyhow!("revoke: ban-list write failed: {e}"))?;

    publish_revoke_event(node_id);

    Ok(rows as u32)
}

/// Fire-and-forget Bus event `ca/revoke/<node-id>`.
///
/// Shells `mde-bus publish ca/revoke/<node-id> --body-flag <json>`.
/// Callers never see failures from this step — it is intentionally
/// best-effort (the DB mark + ban-list write are the durable parts).
fn publish_revoke_event(node_id: &str) {
    let topic = format!("ca/revoke/{node_id}");
    let body = format!(r#"{{"node_id":"{node_id}","ok":true}}"#);
    let _ = std::process::Command::new("mde-bus")
        .args(["publish", &topic, "--body-flag", &body])
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE nebula_peer_certs (
                id INTEGER PRIMARY KEY,
                node_id TEXT NOT NULL,
                epoch INTEGER NOT NULL,
                revoked_at INTEGER
            );",
        )
        .expect("create table");
        conn
    }

    #[test]
    fn revoke_marks_rows_and_bans_node() {
        let conn = setup_db();
        conn.execute_batch(
            "INSERT INTO nebula_peer_certs (node_id, epoch, revoked_at)
             VALUES ('peer:anvil', 1, NULL),
                    ('peer:anvil', 2, NULL);",
        )
        .expect("insert rows");

        let tmp = tempfile::tempdir().expect("tempdir");
        let workgroup_root = tmp.path();
        let self_id = "peer:lighthouse";

        let count = revoke_peer(&conn, workgroup_root, self_id, "peer:anvil").expect("revoke");

        assert_eq!(count, 2, "both rows marked revoked");

        let still_active: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id='peer:anvil' AND revoked_at IS NULL",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(still_active, 0);

        assert!(
            crate::ca::ban_list::is_banned(workgroup_root, "peer:anvil"),
            "node should be in ban list"
        );
    }

    #[test]
    fn revoke_no_active_certs_still_bans() {
        let conn = setup_db();
        let tmp = tempfile::tempdir().expect("tempdir");
        let count = revoke_peer(&conn, tmp.path(), "peer:self", "peer:ghost").expect("revoke");
        assert_eq!(count, 0, "no rows to mark");
        assert!(crate::ca::ban_list::is_banned(tmp.path(), "peer:ghost"));
    }

    #[test]
    fn revoke_already_revoked_rows_skips_them() {
        let conn = setup_db();
        conn.execute_batch(
            "INSERT INTO nebula_peer_certs (node_id, epoch, revoked_at)
             VALUES ('peer:anvil', 1, 9999);",
        )
        .expect("insert");
        let tmp = tempfile::tempdir().expect("tempdir");
        let count = revoke_peer(&conn, tmp.path(), "peer:self", "peer:anvil").expect("revoke");
        assert_eq!(count, 0, "already-revoked rows not re-touched");
    }

    #[test]
    fn revoke_is_idempotent_on_ban_list() {
        let conn = setup_db();
        let tmp = tempfile::tempdir().expect("tempdir");
        revoke_peer(&conn, tmp.path(), "peer:self", "peer:anvil").expect("first revoke");
        revoke_peer(&conn, tmp.path(), "peer:self", "peer:anvil").expect("second revoke");
        assert!(crate::ca::ban_list::is_banned(tmp.path(), "peer:anvil"));
    }
}
