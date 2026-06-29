//! NF-2.3 (v2.5) — sign one peer cert under the active CA.
//!
//! Per the open-mesh directive (2026-05-23): every peer
//! cert lands in the same `groups=["role:host"]` or
//! `["role:peer"]` basket. No per-node ACL groups, no
//! per-service scopes. The single shared passcode at
//! enrollment is the sole gate.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{seal, CaError, NebulaCertBackend};

/// Default CIDR prefix length on a node's overlay cert.
///
/// **/17, not /16** — the `10.42.0.0/16` mesh is split into the peer
/// subnet (`10.42.0.0/17`) and the VM subnet (`10.42.128.0/17`, reached
/// via a `tun.unsafe_route` to the guest's host — see
/// [`crate::workers::nebula_supervisor::VM_SUBNET_CIDR`]). nebula refuses
/// an `unsafe_route` *contained within* the cert's own network, so a node
/// cert MUST be the lower /17; a /16 cert silently broke the overlay
/// (`nebula -config` exits 1, no `nebula1` interface) on every node that
/// rendered the VM route. Found bringing up the local VM bed 2026-06-10.
pub const DEFAULT_CIDR_PREFIX: u8 = 17;

/// Default mesh CIDR; allocator walks it sequentially.
pub const DEFAULT_MESH_CIDR_BASE: &str = "10.42.0.0";

/// Maximum number of distinct active (non-revoked) peer certs the
/// CA will sign at the current epoch without an explicit override.
///
/// Raised to **12** by operator directive (2026-06-14, §8): the mesh envelope
/// is now **up to 3 lighthouses + 9 Headless/Full peers = 12 nodes** (was ≤ 8,
/// single lighthouse). The cap counts all active signed certs (lighthouses
/// self-sign/enroll as certs too), so 12 total accommodates 3 LH + 9 peers.
/// (A role-aware split — ≤3 Host + ≤9 Peer — is a refinement tracked in the
/// SETUP epic; 12-total is the v1 cap.) TUNE-11 (2026-05-26) keeps this as a
/// runtime check on the CSR sign path.
///
/// Operators with a legitimate need to exceed the cap must invoke
/// `mackesd ca sign-csr --override-cap` per
/// `docs/design/cap-overrides.md`. Each override is audit-logged.
pub const MAX_PEER_CAP: u32 = 12;

/// Outcome of one signing call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedPeer {
    /// Stable peer id (matches `nodes.node_id`).
    pub node_id: String,
    /// Allocated overlay IP (e.g. "10.42.0.7").
    pub overlay_ip: String,
    /// Signed cert PEM body (also persisted to
    /// `nebula_peer_certs.cert_pem`).
    pub cert_pem: String,
    /// Path the cert was written to on disk.
    pub cert_path: PathBuf,
    /// Path the matching private key was written to.
    pub key_path: PathBuf,
}

/// Sign a peer certificate under the active CA. Side effects:
///
///   1. Allocates the next free `10.42.x.y` from the mesh
///      CIDR (skipping IPs already in `nebula_peer_certs`).
///   2. Calls `backend.sign_peer(...)`.
///   3. Re-seals the produced private key at mode 0600.
///   4. Inserts a row into `nebula_peer_certs` at the active
///      epoch.
///
/// # Errors
///
/// - [`CaError::Sql`] if no active CA exists or any database
///   op fails.
/// - [`CaError::CidrExhausted`] when every IP in the /16
///   pool is already allocated.
/// - [`CaError::BinaryMissing`] / [`CaError::Subprocess`] on
///   `nebula-cert` failures.
/// - [`CaError::Io`] on file IO failures.
#[allow(clippy::too_many_arguments)]
pub fn sign_peer_cert<B: NebulaCertBackend + ?Sized>(
    backend: &B,
    conn: &Connection,
    mesh_id: &str,
    node_id: &str,
    role: PeerRole,
    ca_crt_path: &Path,
    ca_key_path: &Path,
    crt_out: &Path,
    key_out: &Path,
    extra_taken: &std::collections::HashSet<String>,
) -> Result<SignedPeer, CaError> {
    let active_epoch = active_epoch(conn, mesh_id)?
        .ok_or_else(|| CaError::Sql(format!("no active CA for mesh {mesh_id}")))?;

    // Bed fix #8: a re-sign of a node that already holds an active cert at
    // this epoch (re-enroll, operator re-issue, the auto-signer retrying)
    // must REUSE that node's overlay IP, not allocate a fresh one. Allocating
    // anew both churned the node's IP every re-enroll and — together with the
    // plain INSERT below — collided on the (node_id, epoch) primary key
    // ("UNIQUE constraint failed: nebula_peer_certs.node_id, .epoch"), which
    // silently wedged the auto-signer. Reusing the IP keeps re-enroll
    // idempotent and steers clear of the (overlay_ip, epoch) unique index.
    let allocated_ip = match existing_overlay_ip(conn, node_id, active_epoch)? {
        Some(ip) => ip,
        None => allocate_overlay_ip_excluding(conn, active_epoch, extra_taken)?,
    };

    let groups: &[&str] = match role {
        PeerRole::Host => &["role:host"],
        PeerRole::Peer => &["role:peer"],
    };

    // Bed fix #7/#9: nebula-cert hard-refuses to overwrite an existing
    // cert ("refusing to overwrite existing cert: <crt_out>"), so a stale
    // file from a prior sign at this same output path wedges the re-sign.
    // Clearing here — the single point every signer funnels through —
    // makes BOTH the peer-enroll path (sign_pending_csr) and the
    // founding-lighthouse self-sign (mesh_init, which re-runs onto
    // scratch/self.crt) idempotent. These outputs are an ephemeral
    // hand-off buffer (read straight into the bundle below), so removing a
    // leftover is always safe. Ignore NotFound.
    let _ = std::fs::remove_file(crt_out);
    let _ = std::fs::remove_file(key_out);

    backend.sign_peer(
        ca_crt_path,
        ca_key_path,
        node_id,
        &allocated_ip,
        DEFAULT_CIDR_PREFIX,
        groups,
        crt_out,
        key_out,
    )?;

    // Seal the produced private key.
    let key_bytes = std::fs::read(key_out)
        .map_err(|e| CaError::Io(format!("read peer key {}: {e}", key_out.display())))?;
    seal::write_sealed(key_out, &key_bytes)?;

    let cert_pem = std::fs::read_to_string(crt_out)
        .map_err(|e| CaError::Io(format!("read peer cert {}: {e}", crt_out.display())))?;

    // SEC-1 (Q19) — peer certs do not expire mid-epoch: the backend
    // passes no -duration, so nebula signs to the CA's own lifetime,
    // and the books now say the same. `0` is the epoch-lifetime
    // sentinel (turnover is rotation/revocation, never a quiet expiry
    // — ENT-3 makes revocation real).
    let expires_at: i64 = 0;

    // Bed fix #8: upsert on the (node_id, epoch) primary key so a re-sign
    // replaces the node's row in place (fresh cert PEM, same reused IP) and
    // clears any prior revocation — instead of failing the whole sign on a
    // PK collision.
    conn.execute(
        "INSERT INTO nebula_peer_certs \
         (node_id, epoch, cert_pem, overlay_ip, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(node_id, epoch) DO UPDATE SET \
           cert_pem = excluded.cert_pem, \
           overlay_ip = excluded.overlay_ip, \
           expires_at = excluded.expires_at, \
           revoked_at = NULL",
        rusqlite::params![node_id, active_epoch, cert_pem, allocated_ip, expires_at],
    )
    .map_err(|e| CaError::Sql(e.to_string()))?;

    tracing::info!(
        node_id, overlay_ip = %allocated_ip, epoch = active_epoch,
        "nebula peer cert signed"
    );

    Ok(SignedPeer {
        node_id: node_id.to_string(),
        overlay_ip: allocated_ip,
        cert_pem,
        cert_path: crt_out.to_path_buf(),
        key_path: key_out.to_path_buf(),
    })
}

/// Per-cert role group. The open-mesh directive flattens
/// this to two values: Host (lighthouse-eligible) + Peer
/// (everything else). No per-service scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRole {
    /// Lighthouse / leader-eligible node.
    Host,
    /// Regular mesh peer.
    Peer,
}

/// Pull the active CA epoch from the database.
pub fn active_epoch(conn: &Connection, mesh_id: &str) -> Result<Option<i64>, CaError> {
    let mut stmt = conn
        .prepare(
            "SELECT epoch FROM nebula_ca \
             WHERE mesh_id = ?1 AND retired_at IS NULL \
             ORDER BY epoch DESC LIMIT 1",
        )
        .map_err(|e| CaError::Sql(e.to_string()))?;
    let row: Option<i64> = stmt.query_row([mesh_id], |r| r.get(0)).ok();
    Ok(row)
}

/// Allocate the next free overlay IP at `epoch`. Walks
/// `10.42.0.1`..`10.42.255.254` sequentially, skipping IPs
/// already allocated in `nebula_peer_certs` for the given
/// epoch. Skips `.0` (network address) + `.255` (broadcast)
/// on each /24 — keeps the allocation human-readable.
pub fn allocate_overlay_ip(conn: &Connection, epoch: i64) -> Result<String, CaError> {
    allocate_overlay_ip_excluding(conn, epoch, &std::collections::HashSet::new())
}

/// Like [`allocate_overlay_ip`], but also skips every IP in `extra_taken` — the
/// overlay IPs already assigned MESH-WIDE per the shared etcd peer directory.
///
/// MULTI-LH-IP-ALLOC (caught live 2026-06-27): a JOINED lighthouse's local
/// `nebula_peer_certs` table holds only the certs IT signed, so a local-only
/// scan restarts at `10.42.0.1` and COLLIDES with the founding lighthouse's
/// assignments — a node enrolled via a new lighthouse was handed `10.42.0.1`
/// (lh1's own IP). The enroll path passes the directory's overlay IPs here so
/// every signing lighthouse allocates from the same global view, not just its
/// own store. (Concurrent signs on two lighthouses are still serialized by the
/// operator's sequential enroll flow; a fully-atomic etcd allocation is a
/// follow-up, tracked in the worklist.)
///
/// # Errors
/// [`CaError::CidrExhausted`] when the whole /16 is allocated; [`CaError::Sql`]
/// on a database read failure.
pub fn allocate_overlay_ip_excluding(
    conn: &Connection,
    epoch: i64,
    extra_taken: &std::collections::HashSet<String>,
) -> Result<String, CaError> {
    let mut taken = load_taken_ips(conn, epoch)?;
    taken.extend(extra_taken.iter().cloned());
    for octet_b in 0u8..=255 {
        for octet_c in 1u8..=254 {
            let ip = format!("10.42.{octet_b}.{octet_c}");
            if !taken.contains(&ip) {
                return Ok(ip);
            }
        }
    }
    Err(CaError::CidrExhausted("10.42.0.0/16".to_string()))
}

/// Count of distinct active (non-revoked) peer certs at `epoch`.
///
/// Used by TUNE-11 to gate the [`MAX_PEER_CAP`] check on
/// [`crate::nebula_enroll::sign_pending_csr`]. Distinct on
/// `node_id` so the same peer rotating its cert at the same
/// epoch counts once, not twice.
///
/// # Errors
/// [`CaError::Sql`] on database failure.
pub fn count_active_peers(conn: &Connection, mesh_id: &str) -> Result<u32, CaError> {
    let epoch = match active_epoch(conn, mesh_id)? {
        Some(e) => e,
        None => return Ok(0),
    };
    let mut stmt = conn
        .prepare(
            "SELECT COUNT(DISTINCT node_id) FROM nebula_peer_certs \
             WHERE epoch = ?1 AND revoked_at IS NULL",
        )
        .map_err(|e| CaError::Sql(e.to_string()))?;
    let count: i64 = stmt
        .query_row([epoch], |r| r.get(0))
        .map_err(|e| CaError::Sql(e.to_string()))?;
    Ok(u32::try_from(count.max(0)).unwrap_or(u32::MAX))
}

/// The overlay IP a node already holds via a non-revoked cert at
/// `epoch`, if any. Used to keep re-enroll idempotent (bed fix #8) —
/// a node re-signing at the same epoch keeps its IP rather than
/// churning to a freshly-allocated one.
///
/// # Errors
/// [`CaError::Sql`] on database failure.
pub fn existing_overlay_ip(
    conn: &Connection,
    node_id: &str,
    epoch: i64,
) -> Result<Option<String>, CaError> {
    let mut stmt = conn
        .prepare(
            "SELECT overlay_ip FROM nebula_peer_certs \
             WHERE node_id = ?1 AND epoch = ?2 AND revoked_at IS NULL \
             LIMIT 1",
        )
        .map_err(|e| CaError::Sql(e.to_string()))?;
    let ip: Option<String> = stmt
        .query_row(rusqlite::params![node_id, epoch], |r| r.get(0))
        .ok();
    Ok(ip)
}

fn load_taken_ips(
    conn: &Connection,
    epoch: i64,
) -> Result<std::collections::HashSet<String>, CaError> {
    let mut stmt = conn
        .prepare(
            "SELECT overlay_ip FROM nebula_peer_certs \
             WHERE epoch = ?1 AND revoked_at IS NULL",
        )
        .map_err(|e| CaError::Sql(e.to_string()))?;
    let rows = stmt
        .query_map([epoch], |r| r.get::<_, String>(0))
        .map_err(|e| CaError::Sql(e.to_string()))?;
    let mut out = std::collections::HashSet::new();
    for row in rows {
        let s = row.map_err(|e| CaError::Sql(e.to_string()))?;
        out.insert(s);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::{mint, MockBackend};

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        crate::store::migrate(&conn).expect("migrate");
        conn
    }

    fn mint_one(conn: &Connection) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        mint::mint_ca(&MockBackend, conn, "m1", Some(&crt), Some(&key)).expect("mint");
        tmp
    }

    /// Backend that models real nebula-cert's refusal to overwrite an
    /// existing cert file (MockBackend `fs::write`s unconditionally and so
    /// can't reproduce it). Used to lock bed fix #7/#9: sign_peer_cert must
    /// clear a stale output before signing, for EVERY caller — the peer
    /// enroll path and the mesh-init self-sign alike.
    struct RefuseOverwriteBackend;
    impl crate::ca::NebulaCertBackend for RefuseOverwriteBackend {
        fn mint_ca(&self, m: &str, c: &Path, k: &Path) -> Result<(), CaError> {
            MockBackend.mint_ca(m, c, k)
        }
        #[allow(clippy::too_many_arguments)]
        fn sign_peer(
            &self,
            ca_crt: &Path,
            ca_key: &Path,
            node_id: &str,
            overlay_ip: &str,
            cidr: u8,
            groups: &[&str],
            crt_out: &Path,
            key_out: &Path,
        ) -> Result<(), CaError> {
            if crt_out.exists() {
                return Err(CaError::Io(format!(
                    "refusing to overwrite existing cert: {}",
                    crt_out.display()
                )));
            }
            MockBackend.sign_peer(
                ca_crt, ca_key, node_id, overlay_ip, cidr, groups, crt_out, key_out,
            )
        }
    }

    #[test]
    fn sign_peer_cert_clears_stale_output_before_signing() {
        // Bed fix #9 (and #7): a leftover cert at the output path — e.g.
        // mesh-init re-running onto scratch/self.crt — must not wedge the
        // sign. RefuseOverwriteBackend models nebula-cert's refusal; seed a
        // stale file and assert sign_peer_cert still succeeds.
        let conn = fresh_conn();
        let tmp = mint_one(&conn);
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let crt_out = scratch.join("self.crt");
        let key_out = scratch.join("self.key");
        std::fs::write(&crt_out, b"STALE SELF CERT").unwrap();
        sign_peer_cert(
            &RefuseOverwriteBackend,
            &conn,
            "m1",
            "peer:self",
            PeerRole::Host,
            &tmp.path().join("ca.crt"),
            &tmp.path().join("ca.key"),
            &crt_out,
            &key_out,
            &std::collections::HashSet::new(),
        )
        .expect("re-sign onto a stale output must succeed");
    }

    #[test]
    fn count_active_peers_returns_zero_for_unminted_mesh() {
        let conn = fresh_conn();
        // No CA + no rows → zero.
        assert_eq!(count_active_peers(&conn, "m1").unwrap(), 0);
    }

    #[test]
    fn count_active_peers_counts_active_node_ids_at_active_epoch() {
        // Four peers at epoch 0; one revoked → counts as 3.
        // The UNIQUE(node_id, epoch) constraint means each
        // active node_id appears exactly once per epoch, so
        // COUNT(DISTINCT node_id) is the right shape regardless.
        let conn = fresh_conn();
        let _tmp = mint_one(&conn);
        conn.execute(
            "INSERT INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, expires_at) \
             VALUES ('peer:a', 0, 'pem', '10.42.0.1', 9999999), \
                    ('peer:b', 0, 'pem', '10.42.0.2', 9999999), \
                    ('peer:c', 0, 'pem', '10.42.0.3', 9999999), \
                    ('peer:d', 0, 'pem', '10.42.0.4', 9999999)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE nebula_peer_certs SET revoked_at = 1234567890 \
             WHERE node_id = 'peer:d'",
            [],
        )
        .unwrap();
        assert_eq!(count_active_peers(&conn, "m1").unwrap(), 3);
    }

    #[test]
    fn allocator_starts_at_10_42_0_1() {
        let conn = fresh_conn();
        let _tmp = mint_one(&conn);
        let ip = allocate_overlay_ip(&conn, 0).unwrap();
        assert_eq!(ip, "10.42.0.1");
    }

    #[test]
    fn allocator_skips_taken_ips() {
        let conn = fresh_conn();
        let _tmp = mint_one(&conn);
        conn.execute(
            "INSERT INTO nebula_peer_certs \
             (node_id, epoch, cert_pem, overlay_ip, expires_at) \
             VALUES ('peer:a', 0, 'pem', '10.42.0.1', 9999999)",
            [],
        )
        .unwrap();
        let next = allocate_overlay_ip(&conn, 0).unwrap();
        assert_eq!(next, "10.42.0.2");
    }

    #[test]
    fn allocator_excluding_skips_global_directory_ips() {
        // MULTI-LH-IP-ALLOC: a joined lighthouse's LOCAL store is empty, so a
        // local-only scan hands out 10.42.0.1 — colliding with the founding
        // lighthouse. Passing the mesh-wide assigned set (the etcd directory)
        // makes it skip them and allocate the first GLOBALLY-free IP.
        let conn = fresh_conn();
        let _tmp = mint_one(&conn);
        let global: std::collections::HashSet<String> = [
            "10.42.0.1",
            "10.42.0.2",
            "10.42.0.3",
            "10.42.0.4",
            "10.42.0.5",
            "10.42.0.6",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let ip = allocate_overlay_ip_excluding(&conn, 0, &global).unwrap();
        assert_eq!(
            ip, "10.42.0.7",
            "must skip every globally-assigned IP, not restart at .1"
        );
        // Sanity: with no global set it WOULD collide at .1 (the bug it fixes).
        let bug =
            allocate_overlay_ip_excluding(&conn, 0, &std::collections::HashSet::new()).unwrap();
        assert_eq!(bug, "10.42.0.1");
    }

    #[test]
    fn resigning_a_node_is_idempotent_keeps_ip_and_upserts_row() {
        // Bed fix #8: signing the same node twice at the same epoch
        // (re-enroll / re-issue / auto-signer retry) must NOT collide on
        // the (node_id, epoch) primary key, must keep the node's overlay
        // IP, and must leave exactly one row (the cert replaced in place).
        let conn = fresh_conn();
        let tmp = mint_one(&conn);
        let ca_crt = tmp.path().join("ca.crt");
        let ca_key = tmp.path().join("ca.key");
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let crt_out = scratch.join("peer:anvil.crt");
        let key_out = scratch.join("peer:anvil.key");

        let first = sign_peer_cert(
            &MockBackend,
            &conn,
            "m1",
            "peer:anvil",
            PeerRole::Peer,
            &ca_crt,
            &ca_key,
            &crt_out,
            &key_out,
            &std::collections::HashSet::new(),
        )
        .expect("first sign");

        // Re-sign — the second time must succeed, not hit a PK collision.
        let second = sign_peer_cert(
            &MockBackend,
            &conn,
            "m1",
            "peer:anvil",
            PeerRole::Peer,
            &ca_crt,
            &ca_key,
            &crt_out,
            &key_out,
            &std::collections::HashSet::new(),
        )
        .expect("re-sign must succeed (upsert, not collide)");

        assert_eq!(first.overlay_ip, second.overlay_ip, "IP must be stable");
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = 'peer:anvil'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "re-sign upserts in place — exactly one row");
    }

    #[test]
    fn sign_writes_pem_inserts_row_and_returns_overlay_ip() {
        let conn = fresh_conn();
        let tmp = mint_one(&conn);
        let ca_crt = tmp.path().join("ca.crt");
        let ca_key = tmp.path().join("ca.key");
        let crt = tmp.path().join("peer.crt");
        let key = tmp.path().join("peer.key");
        let signed = sign_peer_cert(
            &MockBackend,
            &conn,
            "m1",
            "peer:anvil",
            PeerRole::Peer,
            &ca_crt,
            &ca_key,
            &crt,
            &key,
            &std::collections::HashSet::new(),
        )
        .expect("sign");
        assert_eq!(signed.overlay_ip, "10.42.0.1");
        // /17 (the peer subnet) — NOT /16, so the VM /17 unsafe-route is
        // outside the cert's network and nebula accepts the config.
        assert!(signed.cert_pem.contains("ip=10.42.0.1/17"));
        assert!(signed.cert_pem.contains("groups=role:peer"));
        // Row landed.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id='peer:anvil'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn host_role_emits_role_host_group() {
        let conn = fresh_conn();
        let tmp = mint_one(&conn);
        let ca_crt = tmp.path().join("ca.crt");
        let ca_key = tmp.path().join("ca.key");
        let crt = tmp.path().join("host.crt");
        let key = tmp.path().join("host.key");
        let signed = sign_peer_cert(
            &MockBackend,
            &conn,
            "m1",
            "peer:lighthouse",
            PeerRole::Host,
            &ca_crt,
            &ca_key,
            &crt,
            &key,
            &std::collections::HashSet::new(),
        )
        .expect("sign host");
        assert!(signed.cert_pem.contains("groups=role:host"));
    }

    #[test]
    fn sign_rejects_when_no_active_ca() {
        let conn = fresh_conn();
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = sign_peer_cert(
            &MockBackend,
            &conn,
            "never-minted",
            "peer:x",
            PeerRole::Peer,
            &tmp.path().join("ca.crt"),
            &tmp.path().join("ca.key"),
            &tmp.path().join("peer.crt"),
            &tmp.path().join("peer.key"),
            &std::collections::HashSet::new(),
        )
        .unwrap_err();
        assert!(matches!(err, CaError::Sql(_)));
    }

    #[test]
    fn peer_key_sealed_at_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let conn = fresh_conn();
        let tmp = mint_one(&conn);
        let signed = sign_peer_cert(
            &MockBackend,
            &conn,
            "m1",
            "peer:a",
            PeerRole::Peer,
            &tmp.path().join("ca.crt"),
            &tmp.path().join("ca.key"),
            &tmp.path().join("peer.crt"),
            &tmp.path().join("peer.key"),
            &std::collections::HashSet::new(),
        )
        .expect("sign");
        let mode = std::fs::metadata(&signed.key_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
