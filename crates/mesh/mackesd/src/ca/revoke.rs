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
//!    running workers (peer_cap, etc.) converge without
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
    // Both identities become replicated filenames below: self_node_id selects
    // the local ban-list directory and node_id selects the data-plane
    // blocklist record. Validate before touching SQLite so a hostile direct
    // CLI argument cannot escape the workgroup root or leave a half-applied
    // revocation behind.
    validate_revoke_identity(self_node_id, "self node-id")?;
    validate_revoke_identity(node_id, "target node-id")?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // ENT-3 (C2) — capture the active certs' PEMs BEFORE marking, so
    // the data-plane blocklist can carry their fingerprints.
    let pems: Vec<String> = conn
        .prepare("SELECT cert_pem FROM nebula_peer_certs WHERE node_id = ?1 AND revoked_at IS NULL")
        .and_then(|mut st| {
            st.query_map([node_id], |r| r.get::<_, String>(0))
                .map(|rows| rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();

    let rows = crate::store::writer::request_or_execute(
        conn,
        crate::store::writer::WriteOp::RevokePeerCert {
            node_id: node_id.to_owned(),
            revoked_at: now_ms,
        },
    )
    .and_then(crate::store::writer::WriteResponse::into_count)
    .context("revoke: update nebula_peer_certs")?;

    crate::ca::ban_list::add_banned(workgroup_root, self_node_id, node_id)
        .map_err(|e| anyhow::anyhow!("revoke: ban-list write failed: {e}"))?;

    // ENT-3 (C2) — evict the DATA PLANE, not just the books: record
    // the revoked certs' Nebula fingerprints on the replicated
    // blocklist; every peer's supervisor folds them into
    // `pki.blocklist` + reloads nebula. Failure here is LOUD — a
    // revoke that doesn't evict is a standing security hole.
    let fingerprints: Vec<String> = pems
        .iter()
        .filter_map(|pem| crate::ca::blocklist::fingerprint_cert_pem(pem))
        .collect();
    if fingerprints.len() < pems.len() {
        tracing::error!(
            target: "mackesd::ca",
            node_id = %node_id,
            certs = pems.len(),
            fingerprinted = fingerprints.len(),
            "ENT-3: could not fingerprint every revoked cert (nebula-cert missing?) — \
             the data plane may keep trusting this node until cert expiry",
        );
    }
    if !fingerprints.is_empty() {
        // SEC-6 — sign the retract with this node's persisted key so
        // peers can attribute + tamper-check it; unsigned fallback only
        // when the key store itself is broken (still loudly warned).
        let write_result = match crate::node_key::load_or_create(std::path::Path::new(
            crate::node_key::DEFAULT_KEY_PATH,
        )) {
            Ok(key) => crate::ca::blocklist::record_revoked_signed(
                workgroup_root,
                node_id,
                &fingerprints,
                self_node_id,
                &key,
            ),
            Err(e) => {
                tracing::warn!(error = %e, "SEC-6: node signing key unavailable — writing unsigned retract");
                crate::ca::blocklist::record_revoked(workgroup_root, node_id, &fingerprints)
            }
        };
        if let Err(e) = write_result {
            tracing::error!(
                target: "mackesd::ca",
                node_id = %node_id, error = %e,
                "ENT-3: blocklist write failed — data-plane eviction NOT recorded",
            );
        }
    }

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
    let body = serde_json::json!({ "node_id": node_id, "ok": true }).to_string();
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Node IDs are used as path components and Bus-topic components by the
/// direct revocation authority. Keep the accepted mesh identifier shape broad
/// enough for existing `peer:` IDs, while refusing traversal, absolute paths,
/// separators, controls, and surrounding whitespace.
fn validate_revoke_identity(value: &str, label: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 255
            && value.trim() == value
            && value != "."
            && value != ".."
            && !value
                .chars()
                .any(|character| character == '/' || character == '\\' || character.is_control()),
        "invalid {label}: must be one path-safe mesh identity component"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::store::migrate(&conn).expect("migrate test store");
        conn
    }

    fn seed_peer(conn: &rusqlite::Connection, node_id: &str, epoch: i64) {
        let peer = crate::store::writer::CaPeerCertWrite {
            node_id: node_id.into(),
            epoch,
            cert_pem: format!("cert:{node_id}:{epoch}"),
            overlay_ip: format!("10.42.0.{}", epoch + 1),
            public_key_pem: Some(format!("public:{node_id}")),
            created_at: None,
            expires_at: 4_102_444_800,
        };
        let active_epoch = conn
            .query_row(
                "SELECT epoch FROM nebula_ca WHERE mesh_id = 'mesh:test' AND retired_at IS NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .ok();
        let operation = match active_epoch {
            None => crate::store::writer::WriteOp::SeedLighthouseCa {
                mesh_id: "mesh:test".into(),
                epoch,
                ca_cert_pem: format!("ca:{epoch}"),
            },
            Some(active_epoch) if active_epoch != epoch => {
                crate::store::writer::WriteOp::RotateCa {
                    mesh_id: "mesh:test".into(),
                    expected_active_epoch: Some(active_epoch),
                    new_epoch: epoch,
                    ca_cert_pem: format!("ca:{epoch}"),
                    peer_certs: vec![peer.clone()],
                }
            }
            Some(_) => crate::store::writer::WriteOp::UpsertPeerCert {
                mesh_id: "mesh:test".into(),
                expected_epoch: epoch,
                peer: peer.clone(),
            },
        };
        crate::store::writer::request_or_execute(conn, operation)
            .and_then(crate::store::writer::WriteResponse::into_count)
            .expect("seed peer certificate");
        if active_epoch.is_none() {
            crate::store::writer::request_or_execute(
                conn,
                crate::store::writer::WriteOp::UpsertPeerCert {
                    mesh_id: "mesh:test".into(),
                    expected_epoch: epoch,
                    peer,
                },
            )
            .and_then(crate::store::writer::WriteResponse::into_count)
            .expect("seed peer certificate");
        }
    }

    #[test]
    fn revoke_marks_rows_and_bans_node() {
        let conn = setup_db();
        seed_peer(&conn, "peer:anvil", 1);
        seed_peer(&conn, "peer:anvil", 2);

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
        seed_peer(&conn, "peer:anvil", 1);
        crate::store::writer::request_or_execute(
            &conn,
            crate::store::writer::WriteOp::RevokePeerCert {
                node_id: "peer:anvil".into(),
                revoked_at: 9999,
            },
        )
        .and_then(crate::store::writer::WriteResponse::into_count)
        .expect("seed revocation");
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

    #[test]
    fn unsafe_identity_is_rejected_before_database_or_filesystem_mutation() {
        for (self_id, target_id) in [("../escape", "peer:anvil"), ("peer:self", "../escape")] {
            let conn = setup_db();
            seed_peer(&conn, "peer:anvil", 1);
            let tmp = tempfile::tempdir().expect("workgroup root");

            let error = revoke_peer(&conn, tmp.path(), self_id, target_id)
                .expect_err("path-shaped identity must be refused");
            assert!(error.to_string().contains("invalid"), "{error:#}");
            let active: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = 'peer:anvil' AND revoked_at IS NULL",
                    [],
                    |row| row.get(0),
                )
                .expect("count active certs");
            assert_eq!(active, 1, "rejection must precede the DB mutation");
            assert!(
                tmp.path().read_dir().unwrap().next().is_none(),
                "rejection must not create a replicated path"
            );
        }
    }
}
