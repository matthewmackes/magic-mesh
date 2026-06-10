//! ENT-4 — `mackesd mesh-init`: the one-command lighthouse bootstrap.
//!
//! On a clean box, one command yields a working CA-signing lighthouse
//! plus a join token the first peer enrolls with. The composition is
//! deliberately all existing machinery — mint (NF-7), self-sign
//! (`sign_peer_cert`), the bundle the supervisor materializes from,
//! the ENT-2 role pin, and the ENT-1 bearer ledger — wired in order
//! with honest per-step failures.

use std::path::{Path, PathBuf};

use crate::ca::bundle::{bundle_path, LighthouseEntry, NebulaBundle};
use crate::ca::sign::{sign_peer_cert, PeerRole};
use crate::ca::NebulaCertBackend;

/// What `mesh_init` accomplished — printed by the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshInitReport {
    pub mesh_id: String,
    pub overlay_ip: String,
    pub bundle_path: PathBuf,
    /// The full wire-form join token (single-use bearer, ENT-1).
    pub join_token: String,
    /// `Some` when this call pinned the role (unpinned box).
    pub pinned_role: Option<String>,
}

/// Bootstrap this box as the mesh's founding lighthouse.
///
/// Steps (each failure is the step's own honest error):
/// 1. Pin the deployment role IF unpinned (default `lighthouse`;
///    an already-pinned box keeps its role — §5's superset chain
///    means higher ranks carry lighthouse duty too).
/// 2. Mint the CA (idempotent when an active epoch exists).
/// 3. Self-sign this node's peer cert as `role:host` and write the
///    bundle the nebula supervisor materializes `/etc/nebula` from.
/// 4. Mint a single-use enrollment bearer + assemble the join token.
///
/// # Errors
/// Human-actionable step failures.
#[allow(clippy::too_many_arguments)]
pub fn mesh_init<B: NebulaCertBackend>(
    backend: &B,
    conn: &rusqlite::Connection,
    workgroup_root: &Path,
    node_id: &str,
    mesh_id: &str,
    external_addr: &str,
    ca_crt: &Path,
    ca_key: &Path,
    scratch_dir: &Path,
    pin_role: mde_role::Role,
) -> anyhow::Result<MeshInitReport> {
    // 1. Role pin — only when unpinned (ENT-2 owns the semantics).
    let pinned_role = match mde_role::load() {
        Ok(existing) => {
            tracing::info!(role = %existing, "mesh-init: role already pinned, keeping it");
            None
        }
        Err(mde_role::LoadError::NotPinned) => {
            mde_role::pin(pin_role)
                .map_err(|e| anyhow::anyhow!("mesh-init step 1 (role pin): {e}"))?;
            Some(pin_role.as_str().to_string())
        }
        Err(e) => anyhow::bail!("mesh-init step 1 (role read): {e}"),
    };

    // 2. Mint the CA (idempotent on an active epoch).
    crate::ca::mint::mint_ca(backend, conn, mesh_id, Some(ca_crt), Some(ca_key))
        .map_err(|e| anyhow::anyhow!("mesh-init step 2 (CA mint): {e}"))?;

    // 3. Self-sign + write our own bundle (we are the first peer AND
    //    the lighthouse the static_host_map points at).
    std::fs::create_dir_all(scratch_dir)
        .map_err(|e| anyhow::anyhow!("mesh-init step 3 (scratch dir): {e}"))?;
    let crt_out = scratch_dir.join("self.crt");
    let key_out = scratch_dir.join("self.key");
    let signed = sign_peer_cert(
        backend,
        conn,
        mesh_id,
        node_id,
        PeerRole::Host,
        ca_crt,
        ca_key,
        &crt_out,
        &key_out,
    )
    .map_err(|e| anyhow::anyhow!("mesh-init step 3 (self-sign): {e}"))?;
    let epoch = crate::ca::sign::active_epoch(conn, mesh_id)
        .map_err(|e| anyhow::anyhow!("mesh-init step 3 (epoch read): {e}"))?
        .unwrap_or(0);
    let ca_cert_pem = std::fs::read_to_string(ca_crt)
        .map_err(|e| anyhow::anyhow!("mesh-init step 3 (read CA cert): {e}"))?;
    let peer_cert_pem = std::fs::read_to_string(&crt_out)
        .map_err(|e| anyhow::anyhow!("mesh-init step 3 (read signed cert): {e}"))?;
    let peer_key_pem = std::fs::read_to_string(&key_out)
        .map_err(|e| anyhow::anyhow!("mesh-init step 3 (read signed key): {e}"))?;
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let bundle = NebulaBundle {
        mesh_id: mesh_id.to_string(),
        epoch,
        ca_cert_pem,
        peer_cert_pem,
        peer_key_pem,
        overlay_ip: signed.overlay_ip.clone(),
        mesh_cidr: format!("{}/16", crate::ca::sign::DEFAULT_MESH_CIDR_BASE),
        lighthouses: vec![LighthouseEntry {
            node_id: node_id.to_string(),
            overlay_ip: signed.overlay_ip.clone(),
            external_addr: external_addr.to_string(),
        }],
        created_at: now_s,
    };
    let bpath = bundle_path(workgroup_root, node_id);
    crate::ca::bundle::write_bundle(&bpath, &bundle)
        .map_err(|e| anyhow::anyhow!("mesh-init step 3 (bundle write): {e}"))?;

    // 4. The first peer's invitation (ENT-1 single-use bearer).
    let bearer = crate::bearer_ledger::issue(workgroup_root, "mesh-init founding token")
        .map_err(|e| anyhow::anyhow!("mesh-init step 4 (bearer mint): {e}"))?;
    let join_token = format!("mesh:{mesh_id}@{}:4242#{bearer}", signed.overlay_ip);

    Ok(MeshInitReport {
        mesh_id: mesh_id.to_string(),
        overlay_ip: signed.overlay_ip,
        bundle_path: bpath,
        join_token,
        pinned_role,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::MockBackend;

    #[test]
    fn mesh_init_yields_a_signing_lighthouse_and_a_redeemable_token() {
        let tmp = tempfile::tempdir().unwrap();
        // Hermetic role pin: redirect role.toml into the tempdir so the
        // test never touches (or depends on) the privileged
        // /var/lib/mde/role.toml — it ran green on a dev box that already
        // had a pinned role but failed on a fresh CI runner that couldn't
        // write the system path.
        std::env::set_var("MDE_ROLE_PATH", tmp.path().join("role.toml"));
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let ca_crt = tmp.path().join("ca.crt");
        let ca_key = tmp.path().join("ca.key");
        let report = mesh_init(
            &MockBackend,
            &conn,
            tmp.path(),
            "peer:founder",
            "smoke-mesh",
            "203.0.113.7:4242",
            &ca_crt,
            &ca_key,
            &tmp.path().join("scratch"),
            mde_role::Role::Lighthouse,
        )
        .expect("mesh init");
        // The bundle the supervisor materializes from exists + parses.
        let bundle = crate::ca::bundle::read_bundle(&report.bundle_path).unwrap();
        assert_eq!(bundle.mesh_id, "smoke-mesh");
        assert_eq!(bundle.lighthouses.len(), 1);
        assert_eq!(bundle.lighthouses[0].external_addr, "203.0.113.7:4242");
        assert_eq!(bundle.overlay_ip, report.overlay_ip);
        // The join token parses + its bearer is pending (ENT-1).
        let token =
            crate::nebula_enroll::parse_join_token(&report.join_token).expect("token parses");
        assert_eq!(token.mesh_id, "smoke-mesh");
        assert!(crate::bearer_ledger::is_pending(tmp.path(), &token.bearer));
        // And the CA can sign a SECOND peer (the "CA-signing
        // lighthouse" acceptance): place + sign a CSR.
        // (sign_pending_csr's own suite covers the full path; here we
        // assert the active epoch exists.)
        assert!(crate::ca::sign::active_epoch(&conn, "smoke-mesh")
            .unwrap()
            .is_some());
    }
}
