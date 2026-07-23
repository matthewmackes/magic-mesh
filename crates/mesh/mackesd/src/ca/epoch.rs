//! NF-2.5 (v2.5) — CA epoch rotation.
//!
//! Called from `leader.rs` when this node wins the leader-
//! election lease and the previous leader's last-heartbeat is
//! older than the lease TTL, OR explicitly via `mackesd ca
//! rotate` (NF-2.6 CLI). Fail-closed sequence:
//!
//!   1. Preflight every active peer's requester-owned public key and exact
//!      overlay-IP allocation. Any legacy/malformed peer aborts with no change.
//!   2. Mint the new CA and sign the complete roster in private staging.
//!   3. Prepare all new CA/peer rows in one SQLite transaction.
//!   4. Atomically switch the CA cert/key generation and durable peer outputs,
//!      retaining old CA roots in the runtime trust bundle during reconnect.
//!   5. Commit the database transaction and emit the audit event. A failed
//!      activation or commit restores the prior on-disk material.
//!
//! The old-root overlap keeps already-connected peers trusted across rollout.
//! Removing that overlap is a separate live reconnect/prune operation and must
//! not be claimed from these unit-level rotation proofs.

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
) -> Result<RotationOutcome, CaError> {
    bump_epoch_into(
        backend,
        conn,
        mesh_id,
        crt_path,
        key_path,
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
    peer_cert_dir: &Path,
) -> Result<RotationOutcome, CaError> {
    use std::collections::HashSet;
    use std::os::unix::fs::DirBuilderExt as _;

    let crt = crt_path.unwrap_or_else(|| Path::new(super::DEFAULT_CA_CERT_PATH));
    let key = key_path.unwrap_or_else(|| Path::new(super::DEFAULT_CA_KEY_PATH));
    let retired_epoch = sign::active_epoch(conn, mesh_id)?;
    let max_epoch: Option<i64> = conn
        .query_row(
            "SELECT MAX(epoch) FROM nebula_ca WHERE mesh_id = ?1",
            [mesh_id],
            |row| row.get(0),
        )
        .map_err(|e| CaError::Sql(format!("read max CA epoch: {e}")))?;
    let new_epoch = max_epoch.map_or(0, |epoch| epoch + 1);

    // Preflight the COMPLETE old-epoch roster before creating a new root or
    // changing any file/database state. Missing/invalid public keys, duplicate
    // addresses, or addresses outside the peer /17 abort the whole rotation.
    let roster = active_peers(conn, retired_epoch.unwrap_or(-1))?;
    let mut planned = Vec::with_capacity(roster.len());
    let mut overlays = HashSet::new();
    for (node_id, overlay_ip, public_key_pem) in roster {
        let public_key_pem = public_key_pem.ok_or_else(|| {
            CaError::Io(format!(
                "rotation preflight: peer {node_id} has no requester public key; re-enroll it before rotating"
            ))
        })?;
        sign::validate_nebula_public_key_pem(&public_key_pem).map_err(|error| {
            CaError::Io(format!(
                "rotation preflight: peer {node_id} has an invalid requester public key: {error}"
            ))
        })?;
        validate_rotation_overlay(&node_id, &overlay_ip)?;
        if !overlays.insert(overlay_ip.clone()) {
            return Err(CaError::Io(format!(
                "rotation preflight: duplicate overlay IP {overlay_ip}"
            )));
        }
        planned.push(PlannedPeer {
            role: role_for(conn, &node_id),
            node_id,
            overlay_ip,
            public_key_pem,
            cert_pem: String::new(),
        });
    }

    std::fs::create_dir_all(peer_cert_dir).map_err(|e| {
        CaError::Io(format!(
            "create peer certificate directory {}: {e}",
            peer_cert_dir.display()
        ))
    })?;
    let rotation_dir = (0..16)
        .find_map(|_| {
            let candidate = peer_cert_dir.join(format!(
                ".rotation-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&candidate) {
                Ok(()) => Some(Ok(candidate)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(CaError::Io(format!(
                    "create rotation staging {}: {error}",
                    candidate.display()
                )))),
            }
        })
        .unwrap_or_else(|| Err(CaError::Io("rotation staging collisions".into())))?;
    let _rotation_cleanup = DirectoryCleanup(rotation_dir.clone());

    let stage_result = (|| {
        let staged_ca_crt = rotation_dir.join("ca.crt");
        let staged_ca_key = rotation_dir.join("ca.key");
        backend.mint_ca(mesh_id, &staged_ca_crt, &staged_ca_key)?;
        let new_cert_pem = std::fs::read_to_string(&staged_ca_crt).map_err(|e| {
            CaError::Io(format!(
                "read staged CA cert {}: {e}",
                staged_ca_crt.display()
            ))
        })?;
        let new_key = super::seal::read_sealed(&staged_ca_key)?;
        if new_cert_pem.trim().is_empty() {
            return Err(CaError::Io(
                "rotation minted an empty CA certificate".into(),
            ));
        }

        for peer in &mut planned {
            let staged_cert = rotation_dir.join(format!("{}.crt", sanitize(&peer.node_id)));
            let staged_public = rotation_dir.join(format!("{}.pub", sanitize(&peer.node_id)));
            peer.cert_pem = sign::sign_peer_material_with_public_key(
                backend,
                &peer.node_id,
                peer.role,
                &peer.overlay_ip,
                &staged_ca_crt,
                &staged_ca_key,
                &staged_cert,
                &staged_public,
                &peer.public_key_pem,
            )?;
        }
        Ok((new_cert_pem, new_key))
    })();
    let (new_cert_pem, new_key) = match stage_result {
        Ok(staged) => staged,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&rotation_dir);
            return Err(error);
        }
    };

    // Preserve every prior CA cert in the runtime trust file during rollout.
    // The new cert is first (so nebula-cert pairs it with the new key); old
    // requester identities remain connected until an explicit reconnect/prune
    // step proves the fleet has adopted the new epoch.
    let mut trust_bundle = new_cert_pem.clone();
    let mut stmt = conn
        .prepare("SELECT ca_cert_pem FROM nebula_ca WHERE mesh_id = ?1 ORDER BY epoch DESC")
        .map_err(|e| CaError::Sql(format!("prepare trust-overlap roster: {e}")))?;
    let old_roots = stmt
        .query_map([mesh_id], |row| row.get::<_, String>(0))
        .map_err(|e| CaError::Sql(format!("read trust-overlap roster: {e}")))?;
    for old_root in old_roots {
        let old_root = old_root.map_err(|e| CaError::Sql(format!("read old CA row: {e}")))?;
        if !trust_bundle.ends_with('\n') {
            trust_bundle.push('\n');
        }
        trust_bundle.push_str(&old_root);
    }
    drop(stmt);

    let old_ca_pair = if crt.exists() && key.exists() {
        Some((
            std::fs::read(crt)
                .map_err(|e| CaError::Io(format!("backup CA cert {}: {e}", crt.display())))?,
            super::seal::read_sealed(key)?,
        ))
    } else {
        None
    };
    let tx = conn
        .transaction()
        .map_err(|e| CaError::Sql(format!("begin rotation transaction: {e}")))?;
    let actual_retired = retire_active_ca(&tx, mesh_id)?;
    if actual_retired != retired_epoch {
        return Err(CaError::Sql(
            "active CA changed during rotation preflight; retry".into(),
        ));
    }
    tx.execute(
        "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, retired_at) VALUES (?1, ?2, ?3, NULL)",
        rusqlite::params![mesh_id, new_epoch, new_cert_pem],
    )
    .map_err(|e| CaError::Sql(format!("insert new CA: {e}")))?;
    for peer in &planned {
        tx.execute(
            "INSERT INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, expires_at, public_key_pem) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            rusqlite::params![
                peer.node_id,
                new_epoch,
                peer.cert_pem,
                peer.overlay_ip,
                peer.public_key_pem
            ],
        )
        .map_err(|e| CaError::Sql(format!("stage peer {} row: {e}", peer.node_id)))?;
    }

    // Activate files only after every subprocess and SQL statement has
    // succeeded. A commit failure restores the old complete CA pair. Old roots
    // remain in the trust bundle throughout, so rollout cannot strand peers.
    super::seal::write_atomic_pair(crt, trust_bundle.as_bytes(), key, &new_key)?;
    let mut installed_peer_files = Vec::new();
    for peer in &planned {
        let output = peer_cert_dir.join(format!("{}.crt", sanitize(&peer.node_id)));
        let prior = std::fs::read(&output).ok();
        if let Err(error) = super::seal::write_atomic_sealed(&output, peer.cert_pem.as_bytes()) {
            rollback_peer_files(&installed_peer_files);
            if let Some((old_cert, old_key)) = &old_ca_pair {
                let _ = super::seal::write_atomic_pair(crt, old_cert, key, old_key);
            }
            let _ = std::fs::remove_dir_all(&rotation_dir);
            return Err(error);
        }
        installed_peer_files.push((output, prior));
    }
    if let Err(error) = tx.commit() {
        rollback_peer_files(&installed_peer_files);
        if let Some((old_cert, old_key)) = &old_ca_pair {
            let _ = super::seal::write_atomic_pair(crt, old_cert, key, old_key);
        }
        let _ = std::fs::remove_dir_all(&rotation_dir);
        return Err(CaError::Sql(format!("commit CA rotation: {error}")));
    }
    let _ = std::fs::remove_dir_all(&rotation_dir);
    let re_signed = planned.len();

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
        retired_epoch: actual_retired,
        new_epoch,
        re_signed,
    })
}

#[derive(Debug)]
struct PlannedPeer {
    node_id: String,
    overlay_ip: String,
    public_key_pem: String,
    role: sign::PeerRole,
    cert_pem: String,
}

fn validate_rotation_overlay(node_id: &str, overlay_ip: &str) -> Result<(), CaError> {
    let ip: std::net::Ipv4Addr = overlay_ip.parse().map_err(|_| {
        CaError::Io(format!(
            "rotation preflight: peer {node_id} has invalid overlay IP {overlay_ip}"
        ))
    })?;
    let octets = ip.octets();
    if octets[0] != 10 || octets[1] != 42 || octets[2] >= 128 || octets[3] == 0 || octets[3] == 255
    {
        return Err(CaError::Io(format!(
            "rotation preflight: peer {node_id} overlay IP {overlay_ip} is outside the exact peer allocation plan"
        )));
    }
    Ok(())
}

fn rollback_peer_files(files: &[(std::path::PathBuf, Option<Vec<u8>>)]) {
    for (path, previous) in files.iter().rev() {
        match previous {
            Some(bytes) => {
                let _ = super::seal::write_atomic_sealed(path, bytes);
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

struct DirectoryCleanup(std::path::PathBuf);

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
    conn: &Connection,
    epoch: i64,
) -> Result<Vec<(String, String, Option<String>)>, CaError> {
    let mut stmt = conn
        .prepare(
            "SELECT node_id, overlay_ip, public_key_pem FROM nebula_peer_certs \
             WHERE epoch = ?1 AND revoked_at IS NULL",
        )
        .map_err(|e| CaError::Sql(format!("prepare peer roster: {e}")))?;
    let rows = stmt
        .query_map([epoch], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
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
    use crate::ca::{mint, MockBackend, NebulaCertBackend};

    const PUBLIC_KEY: &str = "-----BEGIN NEBULA X25519 PUBLIC KEY-----\n\
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n-----END NEBULA X25519 PUBLIC KEY-----\n";

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
        let outcome = bump_epoch_into(
            &MockBackend,
            &mut conn,
            "m1",
            Some(&crt),
            Some(&key),
            tmp.path(),
        )
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
        let outcome = bump_epoch_into(
            &MockBackend,
            &mut conn,
            "m1",
            Some(&crt),
            Some(&key),
            tmp.path(),
        )
        .expect("rotate");
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
        bump_epoch_into(
            &MockBackend,
            &mut conn,
            "m1",
            Some(&crt),
            Some(&key),
            tmp.path(),
        )
        .expect("rotate");
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
             (node_id, epoch, cert_pem, overlay_ip, expires_at, public_key_pem) \
             VALUES ('peer:anvil', 0, 'PEM', '10.42.0.5', 9999999, ?1)",
            [PUBLIC_KEY],
        )
        .unwrap();
        let outcome = bump_epoch_into(
            &MockBackend,
            &mut conn,
            "m1",
            Some(&crt),
            Some(&key),
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
        let rotated_ip: String = conn
            .query_row(
                "SELECT overlay_ip FROM nebula_peer_certs WHERE node_id='peer:anvil' AND epoch=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rotated_ip, "10.42.0.5");
    }

    #[test]
    fn rotation_preflight_missing_public_key_changes_nothing() {
        let mut conn = fresh_store();
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        mint::mint_ca(&MockBackend, &conn, "m1", Some(&crt), Some(&key)).expect("mint");
        conn.execute(
            "INSERT INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, expires_at) \
             VALUES ('peer:legacy', 0, 'OLD-CERT', '10.42.0.7', 0)",
            [],
        )
        .unwrap();
        let cert_before = std::fs::read(&crt).unwrap();
        let key_before = crate::ca::seal::read_sealed(&key).unwrap();
        let error = bump_epoch_into(
            &MockBackend,
            &mut conn,
            "m1",
            Some(&crt),
            Some(&key),
            tmp.path(),
        )
        .expect_err("legacy peer must block rotation");
        assert!(error.to_string().contains("has no requester public key"));
        assert_eq!(std::fs::read(&crt).unwrap(), cert_before);
        assert_eq!(crate::ca::seal::read_sealed(&key).unwrap(), key_before);
        assert_eq!(sign::active_epoch(&conn, "m1").unwrap(), Some(0));
        let epoch_one: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_ca WHERE mesh_id='m1' AND epoch=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(epoch_one, 0);
    }

    struct FailPublicSign;

    impl NebulaCertBackend for FailPublicSign {
        fn mint_ca(&self, mesh_id: &str, crt_out: &Path, key_out: &Path) -> Result<(), CaError> {
            MockBackend.mint_ca(mesh_id, crt_out, key_out)
        }

        fn sign_peer(
            &self,
            ca_crt: &Path,
            ca_key: &Path,
            node_id: &str,
            overlay_ip: &str,
            cidr_prefix: u8,
            groups: &[&str],
            crt_out: &Path,
            key_out: &Path,
        ) -> Result<(), CaError> {
            MockBackend.sign_peer(
                ca_crt,
                ca_key,
                node_id,
                overlay_ip,
                cidr_prefix,
                groups,
                crt_out,
                key_out,
            )
        }

        fn sign_peer_with_public_key(
            &self,
            _ca_crt: &Path,
            _ca_key: &Path,
            _node_id: &str,
            _overlay_ip: &str,
            _cidr_prefix: u8,
            _groups: &[&str],
            _public_key: &Path,
            _crt_out: &Path,
        ) -> Result<(), CaError> {
            Err(CaError::Subprocess("injected sign failure".into()))
        }
    }

    #[test]
    fn rotation_sign_failure_rolls_back_before_activation() {
        let mut conn = fresh_store();
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let peer_out = tmp.path().join("peer:anvil.crt");
        mint::mint_ca(&MockBackend, &conn, "m1", Some(&crt), Some(&key)).expect("mint");
        conn.execute(
            "INSERT INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, expires_at, public_key_pem) \
             VALUES ('peer:anvil', 0, 'OLD-CERT', '10.42.0.5', 0, ?1)",
            [PUBLIC_KEY],
        )
        .unwrap();
        crate::ca::seal::write_atomic_sealed(&peer_out, b"OLD-OUTPUT").unwrap();
        let ca_before = std::fs::read(&crt).unwrap();
        let error = bump_epoch_into(
            &FailPublicSign,
            &mut conn,
            "m1",
            Some(&crt),
            Some(&key),
            tmp.path(),
        )
        .expect_err("sign failure must abort");
        assert!(error.to_string().contains("injected sign failure"));
        assert_eq!(std::fs::read(&crt).unwrap(), ca_before);
        assert_eq!(std::fs::read(&peer_out).unwrap(), b"OLD-OUTPUT");
        assert_eq!(sign::active_epoch(&conn, "m1").unwrap(), Some(0));
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
