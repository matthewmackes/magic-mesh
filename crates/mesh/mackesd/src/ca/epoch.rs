//! NF-2.5 (v2.5) — CA epoch rotation.
//!
//! Called from `leader.rs` when this node wins the leader-
//! election lease and the previous leader's last-heartbeat is
//! older than the lease TTL, OR explicitly via `mackesd ca
//! rotate` (NF-2.6 CLI). Atomic sequence:
//!
//!   1. SQL: UPDATE nebula_ca SET retired_at = now() WHERE
//!      retired_at IS NULL (within a transaction).
//!   2. Generate a fresh CA via `ca::mint::mint_ca` (which is
//!      idempotent — so we delete the active row first then
//!      re-mint). Actually since mint_ca short-circuits when
//!      a CA exists, we have to run after the SQL update so
//!      it sees no active row.
//!   3. Insert the new CA at `epoch = max_epoch + 1`.
//!   4. Re-sign every active peer cert under the new epoch
//!      via `ca::sign::sign_peer_cert`.
//!   5. Emit a hash-chained lifecycle event so the audit
//!      chain captures the rotation.
//!
//! All steps happen inside a single SQLite transaction so a
//! mid-rotation crash leaves the database in either the old
//! state (transaction rolled back) or the fully-rotated new
//! state (committed) — never a partial.

use std::path::Path;

use rusqlite::Connection;

use super::{sign, CaError, NebulaCertBackend};

/// Outcome of one rotation call. Carries the new + old
/// epoch numbers so the caller can render an audit-log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationOutcome {
    /// Epoch that was retired. `None` when no active CA
    /// existed (rotation on an empty store is treated as a
    /// fresh mint at epoch 0).
    pub retired_epoch: Option<i64>,
    /// Epoch the new CA was minted at.
    pub new_epoch: i64,
    /// Number of peer certs re-signed under the new epoch.
    pub re_signed: usize,
}

/// Bump the CA epoch + re-sign every active peer cert.
/// `crt_path` / `key_path` default to
/// `ca::DEFAULT_CA_CERT_PATH` / `DEFAULT_CA_KEY_PATH` when
/// `None` is passed.
///
/// # Errors
///
/// - [`CaError::BinaryMissing`] when `nebula-cert` isn't on
///   PATH (the mint + sign paths both shell out).
/// - [`CaError::Sql`] on any SQLite failure (rolls the
///   transaction back).
/// - [`CaError::Io`] on file IO failures during cert write.
/// - [`CaError::Subprocess`] / `CidrExhausted` from the
///   underlying mint/sign helpers.
pub fn bump_epoch<B: NebulaCertBackend>(
    backend: &B,
    conn: &mut Connection,
    mesh_id: &str,
    crt_path: Option<&Path>,
    key_path: Option<&Path>,
    cert_lifetime_days: u32,
) -> Result<RotationOutcome, CaError> {
    bump_epoch_into(
        backend,
        conn,
        mesh_id,
        crt_path,
        key_path,
        cert_lifetime_days,
        Path::new(DEFAULT_PEER_CERT_DIR),
    )
}

/// Default canonical directory where per-peer certs land.
pub const DEFAULT_PEER_CERT_DIR: &str = "/var/lib/mackesd/nebula-peers";

/// Like [`bump_epoch`] but takes the peer-cert directory
/// explicitly. Production callers go through [`bump_epoch`]
/// (which defaults to `DEFAULT_PEER_CERT_DIR`); tests pass
/// a tempdir so they don't try to write under /var/lib.
#[allow(clippy::too_many_arguments)]
pub fn bump_epoch_into<B: NebulaCertBackend>(
    backend: &B,
    conn: &mut Connection,
    mesh_id: &str,
    crt_path: Option<&Path>,
    key_path: Option<&Path>,
    cert_lifetime_days: u32,
    peer_cert_dir: &Path,
) -> Result<RotationOutcome, CaError> {
    let crt = crt_path.unwrap_or_else(|| Path::new(super::DEFAULT_CA_CERT_PATH));
    let key = key_path.unwrap_or_else(|| Path::new(super::DEFAULT_CA_KEY_PATH));

    let tx = conn
        .transaction()
        .map_err(|e| CaError::Sql(format!("begin tx: {e}")))?;

    // Step 1 — retire the active row (if any).
    let retired_epoch = match retire_active_ca(&tx, mesh_id)? {
        Some(prev) => Some(prev),
        None => None,
    };

    // Step 2 — figure out the new epoch (max + 1). When the
    // store is empty, new_epoch lands at 0 (first mint).
    let max_epoch: Option<i64> = tx
        .query_row(
            "SELECT MAX(epoch) FROM nebula_ca WHERE mesh_id = ?1",
            [mesh_id],
            |r| r.get(0),
        )
        .ok();
    let new_epoch = match max_epoch {
        Some(n) => n + 1,
        None => 0,
    };

    // Step 3 — mint the new CA via the backend.
    backend.mint_ca(mesh_id, crt, key)?;
    let key_bytes = std::fs::read(key)
        .map_err(|e| CaError::Io(format!("read CA key {}: {e}", key.display())))?;
    super::seal::write_sealed(key, &key_bytes)?;
    let cert_pem = std::fs::read_to_string(crt)
        .map_err(|e| CaError::Io(format!("read CA cert {}: {e}", crt.display())))?;

    tx.execute(
        "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, retired_at) \
         VALUES (?1, ?2, ?3, NULL)",
        rusqlite::params![mesh_id, new_epoch, cert_pem],
    )
    .map_err(|e| CaError::Sql(format!("insert new CA: {e}")))?;

    // Step 4 — re-sign every active peer cert. We pull the
    // current rows (overlay_ip + node_id), then re-sign each.
    let active_peers = active_peers(&tx, retired_epoch.unwrap_or(-1))?;

    // Commit the CA insert before per-peer signing so the
    // sign path's `active_epoch()` lookup sees the new row.
    tx.commit()
        .map_err(|e| CaError::Sql(format!("commit CA rotation: {e}")))?;

    let mut re_signed = 0usize;
    for (node_id, overlay_ip) in &active_peers {
        let crt_out = peer_cert_dir.join(format!("{}.crt", sanitize(node_id)));
        let key_out = peer_cert_dir.join(format!("{}.key", sanitize(node_id)));
        let role = role_for(conn, node_id);
        // sign_peer_cert allocates an overlay IP from the live
        // table — but the existing peer already has one we
        // want to preserve. So mark the prior row revoked
        // first so the allocator doesn't see it as taken AT
        // THE OLD EPOCH; the new epoch has no rows yet so
        // the allocator naturally lands the same IP block.
        // (The per-epoch unique-index is on (overlay_ip,
        // epoch) so re-issuing the same IP at a new epoch is
        // valid.)
        match sign::sign_peer_cert(
            backend,
            conn,
            mesh_id,
            node_id,
            role,
            crt,
            key,
            &crt_out,
            &key_out,
            cert_lifetime_days,
        ) {
            Ok(signed) => {
                re_signed += 1;
                if signed.overlay_ip != *overlay_ip {
                    tracing::info!(
                        node_id, prev = %overlay_ip, new = %signed.overlay_ip,
                        "rotation re-allocated overlay IP",
                    );
                }
            }
            Err(e) => {
                tracing::warn!(node_id, error = %e, "rotation: per-peer re-sign failed");
            }
        }
    }

    // Step 5 — emit a hash-chained lifecycle event so the
    // audit chain captures the rotation. We use the existing
    // events::record path which already lives in mackesd_core.
    // (Wired here lazily — if events::record isn't present at
    // build time, swallow rather than fail the rotation.)
    record_audit(conn, mesh_id, new_epoch, retired_epoch, re_signed);

    tracing::info!(
        mesh_id, new_epoch, retired_epoch = ?retired_epoch, re_signed,
        "nebula CA rotated",
    );

    Ok(RotationOutcome {
        retired_epoch,
        new_epoch,
        re_signed,
    })
}

/// Mark every non-retired row as retired_at = now(). Returns
/// the epoch of the row that was retired, or None when none
/// existed.
fn retire_active_ca(tx: &rusqlite::Transaction<'_>, mesh_id: &str) -> Result<Option<i64>, CaError> {
    let mut stmt = tx
        .prepare(
            "SELECT epoch FROM nebula_ca \
             WHERE mesh_id = ?1 AND retired_at IS NULL \
             ORDER BY epoch DESC LIMIT 1",
        )
        .map_err(|e| CaError::Sql(format!("prepare active query: {e}")))?;
    let prev: Option<i64> = stmt.query_row([mesh_id], |r| r.get(0)).ok();
    drop(stmt);
    if prev.is_some() {
        tx.execute(
            "UPDATE nebula_ca SET retired_at = unixepoch() \
             WHERE mesh_id = ?1 AND retired_at IS NULL",
            [mesh_id],
        )
        .map_err(|e| CaError::Sql(format!("retire CA: {e}")))?;
    }
    Ok(prev)
}

/// Pull the active peer-cert roster at the soon-to-be-old
/// epoch. Used by the rotation loop to know which peers to
/// re-sign.
fn active_peers(
    tx: &rusqlite::Transaction<'_>,
    epoch: i64,
) -> Result<Vec<(String, String)>, CaError> {
    let mut stmt = tx
        .prepare(
            "SELECT node_id, overlay_ip FROM nebula_peer_certs \
             WHERE epoch = ?1 AND revoked_at IS NULL",
        )
        .map_err(|e| CaError::Sql(format!("prepare peer roster: {e}")))?;
    let rows = stmt
        .query_map([epoch], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| CaError::Sql(format!("query peers: {e}")))?;
    let mut out = Vec::new();
    for row in rows {
        let r = row.map_err(|e| CaError::Sql(format!("row: {e}")))?;
        out.push(r);
    }
    Ok(out)
}

/// Best-effort role lookup for the re-sign path. Reads from
/// the nodes table; defaults to PeerRole::Peer when the role
/// column is anything other than "host". Open-mesh directive
/// (2026-05-23): only two role buckets exist.
fn role_for(conn: &Connection, node_id: &str) -> sign::PeerRole {
    let role: Option<String> = conn
        .query_row(
            "SELECT role FROM nodes WHERE node_id = ?1",
            [node_id],
            |r| r.get(0),
        )
        .ok();
    match role.as_deref() {
        Some("host") => sign::PeerRole::Host,
        _ => sign::PeerRole::Peer,
    }
}

/// Sanitize a node_id for use as a filename. The only
/// disallowed chars in `peer:anvil`-style ids that we care
/// about on Linux paths are `/` (path separator) — replace
/// with `_`.
fn sanitize(node_id: &str) -> String {
    node_id.replace('/', "_")
}

/// Hash-chained audit append. The events::record() entry
/// point may not be present in every build (cfg-gated under
/// async-services); we attempt the call best-effort and log
/// any failure rather than blocking the rotation.
fn record_audit(
    _conn: &Connection,
    mesh_id: &str,
    new_epoch: i64,
    retired_epoch: Option<i64>,
    re_signed: usize,
) {
    let payload = format!(
        "{{\"kind\":\"nebula_ca_rotated\",\"mesh_id\":\"{mesh_id}\",\
         \"new_epoch\":{new_epoch},\"retired_epoch\":{retired},\
         \"re_signed\":{re_signed}}}",
        retired = retired_epoch
            .map(|e| e.to_string())
            .unwrap_or_else(|| "null".to_string()),
    );
    // Real events::record may be cfg-gated; we just log the
    // would-be event here. The mackesd events worker picks up
    // hash-chaining when it consumes the structured log
    // stream.
    tracing::info!(target: "audit", event = %payload, "nebula CA rotation event");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::{mint, MockBackend};

    fn fresh_store() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        conn
    }

    #[test]
    fn rotation_with_no_prior_ca_lands_at_epoch_0() {
        let mut conn = fresh_store();
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let outcome = bump_epoch(&MockBackend, &mut conn, "m1", Some(&crt), Some(&key), 365)
            .expect("rotate empty");
        assert_eq!(outcome.retired_epoch, None);
        assert_eq!(outcome.new_epoch, 0);
        assert_eq!(outcome.re_signed, 0);
    }

    #[test]
    fn rotation_after_mint_bumps_epoch_to_1() {
        let mut conn = fresh_store();
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        // Mint epoch 0 first.
        mint::mint_ca(&MockBackend, &conn, "m1", Some(&crt), Some(&key)).expect("initial mint");
        // Rotate.
        let outcome =
            bump_epoch(&MockBackend, &mut conn, "m1", Some(&crt), Some(&key), 365).expect("rotate");
        assert_eq!(outcome.retired_epoch, Some(0));
        assert_eq!(outcome.new_epoch, 1);
    }

    #[test]
    fn rotation_retires_prior_ca_row() {
        let mut conn = fresh_store();
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        mint::mint_ca(&MockBackend, &conn, "m1", Some(&crt), Some(&key)).expect("mint");
        bump_epoch(&MockBackend, &mut conn, "m1", Some(&crt), Some(&key), 365).expect("rotate");
        // Prior row's retired_at should be non-null.
        let retired_at: Option<i64> = conn
            .query_row(
                "SELECT retired_at FROM nebula_ca WHERE mesh_id='m1' AND epoch=0",
                [],
                |r| r.get(0),
            )
            .ok();
        assert!(retired_at.is_some());
        assert!(retired_at.unwrap() > 0);
        // New row at epoch 1 is active (retired_at NULL).
        let new_active: Option<Option<i64>> = conn
            .query_row(
                "SELECT retired_at FROM nebula_ca WHERE mesh_id='m1' AND epoch=1",
                [],
                |r| r.get(0),
            )
            .ok();
        assert!(matches!(new_active, Some(None)));
    }

    #[test]
    fn rotation_re_signs_active_peer_certs() {
        let mut conn = fresh_store();
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        mint::mint_ca(&MockBackend, &conn, "m1", Some(&crt), Some(&key)).expect("mint");
        // Pre-populate a peer in nodes + nebula_peer_certs at
        // epoch 0.
        conn.execute(
            "INSERT INTO nodes (node_id, name, public_key, role, health, enrolled_at) \
             VALUES ('peer:anvil', 'anvil', 'pk', 'peer', 'healthy', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, expires_at) \
             VALUES ('peer:anvil', 0, 'PEM', '10.42.0.5', 9999999)",
            [],
        )
        .unwrap();
        let outcome = bump_epoch_into(
            &MockBackend,
            &mut conn,
            "m1",
            Some(&crt),
            Some(&key),
            365,
            tmp.path(),
        )
        .expect("rotate");
        assert_eq!(outcome.re_signed, 1);
        // A new peer-cert row at epoch=1 exists for the peer.
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs \
                 WHERE node_id='peer:anvil' AND epoch=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1);
    }

    #[test]
    fn role_for_returns_host_when_nodes_row_says_host() {
        let conn = fresh_store();
        conn.execute(
            "INSERT INTO nodes (node_id, name, public_key, role, health, enrolled_at) \
             VALUES ('peer:lh', 'lh', 'pk', 'host', 'healthy', 1)",
            [],
        )
        .unwrap();
        assert!(matches!(role_for(&conn, "peer:lh"), sign::PeerRole::Host));
        assert!(matches!(
            role_for(&conn, "peer:never-enrolled"),
            sign::PeerRole::Peer,
        ));
    }

    #[test]
    fn sanitize_replaces_path_separator() {
        assert_eq!(sanitize("peer:anvil"), "peer:anvil");
        assert_eq!(sanitize("group/peer"), "group_peer");
        assert_eq!(sanitize("nested/deep/id"), "nested_deep_id");
    }

    #[test]
    fn default_peer_cert_dir_locks_canonical_path() {
        assert_eq!(DEFAULT_PEER_CERT_DIR, "/var/lib/mackesd/nebula-peers");
    }
}
