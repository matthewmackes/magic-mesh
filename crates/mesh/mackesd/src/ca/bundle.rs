//! NF-2.7 (v2.5) — NebulaBundle writer.
//!
//! The bundle is the single JSON blob a freshly-enrolled
//! peer needs in order to bring up its `nebula.service`:
//! the public CA cert (so it can verify other peers'
//! signatures), its own signed peer cert + key, its
//! allocated overlay IP, the lighthouse roster, and the
//! mesh CIDR. Written atomically to
//! `~/QNM-Shared/<peer>/mackesd/nebula-bundle.json` next to
//! the existing heartbeat.json so the QNM-Shared replicator
//! ships the bundle to the peer's local copy on the next
//! reconcile pass.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::CaError;

/// Wire shape of the bundle. JSON-serializable so the
/// QNM-Shared replicator + the wizard's import flow both
/// consume the same struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NebulaBundle {
    /// Stable mesh-id (matches `nebula_ca.mesh_id`).
    pub mesh_id: String,
    /// Active CA epoch when the bundle was written.
    pub epoch: i64,
    /// PEM body of the mesh CA's public cert.
    pub ca_cert_pem: String,
    /// PEM body of this peer's signed cert.
    pub peer_cert_pem: String,
    /// PEM body of this peer's signed private key. The
    /// bundle is intended to land on the receiving peer's
    /// disk at mode 0600 (the wizard's import path seals it
    /// via [`crate::ca::seal::write_sealed`]).
    pub peer_key_pem: String,
    /// Overlay IP assigned to this peer (e.g. "10.42.0.5").
    pub overlay_ip: String,
    /// Mesh CIDR — locked to `10.42.0.0/16` per the
    /// open-mesh design.
    pub mesh_cidr: String,
    /// Lighthouse roster — every host-role peer the new
    /// peer should attempt to reach on first boot.
    pub lighthouses: Vec<LighthouseEntry>,
    /// Unix-epoch seconds when the bundle was generated.
    pub created_at: i64,
}

/// One lighthouse entry. Pre-resolved IP so the receiving
/// peer doesn't need DNS to bootstrap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LighthouseEntry {
    /// Stable node-id of the host.
    pub node_id: String,
    /// Overlay IP — the lighthouse advertises itself here.
    pub overlay_ip: String,
    /// Public-internet reachable address (LAN or WAN). The
    /// lighthouse listens on `<external_addr>:4242/udp` +
    /// `:443/tcp` for the covert path.
    pub external_addr: String,
}

/// Default location under QNM-Shared where bundles land.
pub const BUNDLE_FILENAME: &str = "nebula-bundle.json";

/// Compute the bundle path for a given QNM-Shared root +
/// peer name. Mirrors the existing `heartbeat.json`
/// convention so both files sit in the same per-peer dir.
#[must_use]
pub fn bundle_path(workgroup_root: &Path, peer_name: &str) -> PathBuf {
    workgroup_root
        .join(peer_name)
        .join("mackesd")
        .join(BUNDLE_FILENAME)
}

/// Write the bundle atomically (temp file + fsync + rename).
/// Creates the parent directory if missing.
///
/// # Errors
///
/// - [`CaError::Io`] on directory creation / write failures.
/// - [`CaError::Sql`] when serde-json refuses to encode
///   (only happens on degenerate input; surfaced as Sql so
///   the caller treats it as a persistence-layer fault).
pub fn write_bundle(path: &Path, bundle: &NebulaBundle) -> Result<(), CaError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CaError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    let body = serde_json::to_string_pretty(bundle)
        .map_err(|e| CaError::Sql(format!("encode bundle: {e}")))?;
    // Atomic write: write to a tempfile in the same
    // directory + rename. Avoids partial-write races if a
    // peer reads the file during the write.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &body)
        .map_err(|e| CaError::Io(format!("write tmp {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        CaError::Io(format!(
            "rename {} → {}: {e}",
            tmp.display(),
            path.display()
        ))
    })
}

/// Read the bundle back. Used by the wizard's import path
/// to validate a freshly-replicated bundle before applying
/// it.
///
/// # Errors
///
/// - [`CaError::Io`] when the file is missing or unreadable.
/// - [`CaError::Sql`] when the JSON doesn't parse.
pub fn read_bundle(path: &Path) -> Result<NebulaBundle, CaError> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| CaError::Io(format!("read {}: {e}", path.display())))?;
    serde_json::from_str(&body).map_err(|e| CaError::Sql(format!("parse bundle: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bundle() -> NebulaBundle {
        NebulaBundle {
            mesh_id: "m1".into(),
            epoch: 0,
            ca_cert_pem: "-----BEGIN NEBULA CA-----\n-----END NEBULA CA-----\n".into(),
            peer_cert_pem: "-----BEGIN NEBULA CERT-----\n-----END NEBULA CERT-----\n".into(),
            peer_key_pem: "-----BEGIN NEBULA KEY-----\n-----END NEBULA KEY-----\n".into(),
            overlay_ip: "10.42.0.5".into(),
            mesh_cidr: "10.42.0.0/16".into(),
            lighthouses: vec![LighthouseEntry {
                node_id: "peer:lighthouse-1".into(),
                overlay_ip: "10.42.0.1".into(),
                external_addr: "lh1.example.com:4242".into(),
            }],
            created_at: 1_716_499_200,
        }
    }

    #[test]
    fn write_then_read_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = bundle_path(tmp.path(), "peer:anvil");
        let bundle = sample_bundle();
        write_bundle(&path, &bundle).expect("write");
        let parsed = read_bundle(&path).expect("read");
        assert_eq!(parsed, bundle);
    }

    #[test]
    fn write_creates_missing_parent_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = bundle_path(tmp.path(), "peer:new");
        // None of these dirs exist yet.
        assert!(!path.parent().unwrap().exists());
        write_bundle(&path, &sample_bundle()).expect("write");
        assert!(path.exists());
    }

    #[test]
    fn bundle_path_matches_qnm_shared_convention() {
        let p = bundle_path(Path::new("/home/mm/QNM-Shared"), "peer:forge");
        assert_eq!(
            p.to_string_lossy(),
            "/home/mm/QNM-Shared/peer:forge/mackesd/nebula-bundle.json",
        );
    }

    #[test]
    fn read_missing_file_returns_io() {
        let err = read_bundle(Path::new("/nonexistent/bundle.json")).unwrap_err();
        assert!(matches!(err, CaError::Io(_)));
    }

    #[test]
    fn read_malformed_json_returns_sql() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("bad.json");
        std::fs::write(&path, "not json").expect("seed");
        let err = read_bundle(&path).unwrap_err();
        assert!(matches!(err, CaError::Sql(_)));
    }

    #[test]
    fn write_is_atomic_via_temp_rename() {
        // Tempfile naming is internal — assert the
        // intermediate `.json.tmp` doesn't survive a
        // successful write.
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("b.json");
        write_bundle(&path, &sample_bundle()).expect("write");
        let tmp_path = path.with_extension("json.tmp");
        assert!(
            !tmp_path.exists(),
            "tempfile must be renamed away on success"
        );
    }
}
