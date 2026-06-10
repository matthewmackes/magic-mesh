//! NF-2 (v2.5) — Nebula CA module.
//!
//! Mints + manages the mesh-wide Certificate Authority and
//! issues per-peer overlay certificates. The CA private key
//! lives at `/var/lib/mackesd/nebula-ca/ca.key` sealed at mode
//! 0600 by the [`seal`] helpers; the public chain lives in
//! the `nebula_ca` + `nebula_peer_certs` SQLite tables added
//! by migration `0011_nebula_ca.sql`.
//!
//! ## Open-mesh design (2026-05-23 directive)
//!
//! Per the open-mesh / flat-trust directive, every signed
//! peer cert lands in the same `groups=[role:host|peer]`
//! basket. No per-node ACLs, no role-based group splits, no
//! per-service auth scopes. Once a peer holds a valid cert,
//! it sees + reaches every other peer + every published
//! service. The single shared passcode is the only gate
//! between "not on the mesh" and "fully trusted on the
//! mesh."
//!
//! ## Cargo of submodules
//!
//! | Module | NF task | Role |
//! |--------|---------|------|
//! | [`seal`]   | NF-2.4 | mode-0600 enforcement + owner check |
//! | [`mint`]   | NF-2.2 | mint the mesh CA (idempotent) |
//! | [`sign`]   | NF-2.3 | sign one peer cert under the active CA |
//! | [`bundle`] | NF-2.7 | write the NebulaBundle JSON to QNM-Shared |
//!
//! ## NebulaCertBackend
//!
//! All subprocess calls to `nebula-cert` (the Fedora package's
//! CLI binary) go through the [`NebulaCertBackend`] trait. The
//! default [`SubprocessBackend`] shells out; tests pass a
//! [`MockBackend`] that returns canned PEM so they don't
//! depend on the binary being installed.

pub mod backup;
// EPIC-SEC-BANLIST (Q53) — compromised-node ban list checked at
// the CSR sign gate (nebula_enroll::sign_pending_csr).
pub mod ban_list;
pub mod blocklist;
// INST-7 prerequisite — peer cert revocation CLI (`mackesd ca
// revoke <node-id>`). Replaces the originally-planned D-Bus method;
// consumed by `mde-install`'s wipe sequence via subprocess.
pub mod bundle;
pub mod revoke;
// NF-2.5 (v2.5) — CA epoch rotation. Called by leader.rs
// on promotion + by the `mackesd ca rotate` CLI (NF-2.6).
pub mod epoch;
pub mod mint;
pub mod seal;
pub mod sign;

use std::path::Path;

/// Default canonical path for the sealed CA private key.
pub const DEFAULT_CA_KEY_PATH: &str = "/var/lib/mackesd/nebula-ca/ca.key";

/// Default canonical path for the public CA cert.
pub const DEFAULT_CA_CERT_PATH: &str = "/var/lib/mackesd/nebula-ca/ca.crt";

/// Default mesh CIDR. Locked per design doc — overlay IPs
/// allocated from `10.42.0.0/16`. Operators who need a
/// different CIDR override via `MDE_MESH_CIDR` at first mint.
pub const DEFAULT_MESH_CIDR: &str = "10.42.0.0/16";

/// Abstraction over the `nebula-cert` subprocess so tests
/// don't depend on the binary being installed. Production
/// uses [`SubprocessBackend`]; unit tests use
/// [`MockBackend`].
pub trait NebulaCertBackend: Send + Sync {
    /// Equivalent of `nebula-cert ca -name <mesh_id> -out-crt <crt_out> -out-key <key_out>`.
    /// Implementations write the CA cert + key to the given
    /// paths.
    fn mint_ca(&self, mesh_id: &str, crt_out: &Path, key_out: &Path) -> Result<(), CaError>;

    /// Equivalent of `nebula-cert sign -ca-crt <ca_crt> -ca-key <ca_key>
    /// -name <node_id> -ip <overlay_ip>/16 -groups <groups>
    /// -out-crt <crt_out> -out-key <key_out>`.
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
    ) -> Result<(), CaError>;
}

/// Production backend that shells out to `nebula-cert`.
pub struct SubprocessBackend;

impl NebulaCertBackend for SubprocessBackend {
    fn mint_ca(&self, mesh_id: &str, crt_out: &Path, key_out: &Path) -> Result<(), CaError> {
        run_nebula_cert(&[
            "ca",
            "-name",
            mesh_id,
            "-out-crt",
            &crt_out.display().to_string(),
            "-out-key",
            &key_out.display().to_string(),
        ])
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
        let ip_with_mask = format!("{overlay_ip}/{cidr_prefix}");
        let groups_joined = groups.join(",");
        let ca_crt_s = ca_crt.display().to_string();
        let ca_key_s = ca_key.display().to_string();
        let crt_out_s = crt_out.display().to_string();
        let key_out_s = key_out.display().to_string();
        let mut args: Vec<&str> = vec![
            "sign",
            "-ca-crt",
            ca_crt_s.as_str(),
            "-ca-key",
            ca_key_s.as_str(),
            "-name",
            node_id,
            "-ip",
            ip_with_mask.as_str(),
            "-out-crt",
            crt_out_s.as_str(),
            "-out-key",
            key_out_s.as_str(),
        ];
        if !groups.is_empty() {
            args.push("-groups");
            args.push(groups_joined.as_str());
        }
        run_nebula_cert(&args)
    }
}

fn run_nebula_cert(args: &[&str]) -> Result<(), CaError> {
    let output = std::process::Command::new("nebula-cert")
        .args(args)
        .output()
        .map_err(|e| {
            // Missing binary is the dominant failure mode on
            // dev boxes without the Fedora `nebula` package.
            // Surface it cleanly so the caller can decide
            // whether to skip CA ops or hard-fail.
            if e.kind() == std::io::ErrorKind::NotFound {
                CaError::BinaryMissing
            } else {
                CaError::Subprocess(format!("nebula-cert: {e}"))
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CaError::Subprocess(format!(
            "nebula-cert exit {:?}: {stderr}",
            output.status.code()
        )));
    }
    Ok(())
}

/// CA module error set.
#[derive(Debug, thiserror::Error)]
pub enum CaError {
    /// `nebula-cert` binary missing from PATH. Most common
    /// on dev / CI machines without the Fedora `nebula`
    /// package; operators should `dnf install nebula`.
    #[error("nebula-cert binary missing from PATH (dnf install nebula)")]
    BinaryMissing,
    /// Subprocess returned a non-zero exit or otherwise
    /// failed to execute.
    #[error("nebula-cert subprocess: {0}")]
    Subprocess(String),
    /// File IO failed.
    #[error("io: {0}")]
    Io(String),
    /// File permission check rejected the key (mode bits or
    /// owner mismatch).
    #[error("insecure permissions on {path}: mode {mode:#o}")]
    InsecurePermissions {
        /// Path the caller tried to read.
        path: String,
        /// Octal mode bits the file actually has.
        mode: u32,
    },
    /// SQLite operation failed.
    #[error("sqlite: {0}")]
    Sql(String),
    /// Overlay IP allocator ran out of free addresses in the
    /// mesh CIDR.
    #[error("overlay-IP allocator exhausted in {0}")]
    CidrExhausted(String),
    /// The CIDR string passed to the IP allocator didn't
    /// parse.
    #[error("bad mesh CIDR: {0}")]
    BadCidr(String),
}

/// Test-only mock backend that writes canned PEM strings.
/// Used by every unit test in the CA module so they don't
/// depend on `nebula-cert` being installed.
#[cfg(test)]
pub struct MockBackend;

#[cfg(test)]
impl NebulaCertBackend for MockBackend {
    fn mint_ca(&self, mesh_id: &str, crt_out: &Path, key_out: &Path) -> Result<(), CaError> {
        if let Some(parent) = crt_out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CaError::Io(e.to_string()))?;
        }
        std::fs::write(
            crt_out,
            format!("-----BEGIN NEBULA CA-----\nmesh={mesh_id}\n-----END NEBULA CA-----\n"),
        )
        .map_err(|e| CaError::Io(e.to_string()))?;
        std::fs::write(
            key_out,
            format!("-----BEGIN NEBULA CA KEY-----\nmesh={mesh_id}\n-----END NEBULA CA KEY-----\n"),
        )
        .map_err(|e| CaError::Io(e.to_string()))?;
        Ok(())
    }

    fn sign_peer(
        &self,
        _ca_crt: &Path,
        _ca_key: &Path,
        node_id: &str,
        overlay_ip: &str,
        cidr_prefix: u8,
        groups: &[&str],
        crt_out: &Path,
        key_out: &Path,
    ) -> Result<(), CaError> {
        if let Some(parent) = crt_out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CaError::Io(e.to_string()))?;
        }
        std::fs::write(
            crt_out,
            format!(
                "-----BEGIN NEBULA CERT-----\n\
                 name={node_id}\nip={overlay_ip}/{cidr_prefix}\n\
                 groups={}\n-----END NEBULA CERT-----\n",
                groups.join(",")
            ),
        )
        .map_err(|e| CaError::Io(e.to_string()))?;
        std::fs::write(
            key_out,
            format!("-----BEGIN NEBULA KEY-----\nname={node_id}\n-----END NEBULA KEY-----\n"),
        )
        .map_err(|e| CaError::Io(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_paths_lock_design_doc_values() {
        assert_eq!(DEFAULT_CA_KEY_PATH, "/var/lib/mackesd/nebula-ca/ca.key");
        assert_eq!(DEFAULT_CA_CERT_PATH, "/var/lib/mackesd/nebula-ca/ca.crt");
    }

    #[test]
    fn default_mesh_cidr_locked() {
        assert_eq!(DEFAULT_MESH_CIDR, "10.42.0.0/16");
    }

    #[test]
    fn mock_backend_mint_writes_pem() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        let backend = MockBackend;
        backend.mint_ca("test-mesh", &crt, &key).expect("mint");
        let crt_body = std::fs::read_to_string(&crt).expect("read crt");
        assert!(crt_body.contains("BEGIN NEBULA CA"));
        assert!(crt_body.contains("mesh=test-mesh"));
        let key_body = std::fs::read_to_string(&key).expect("read key");
        assert!(key_body.contains("BEGIN NEBULA CA KEY"));
    }

    #[test]
    fn mock_backend_sign_writes_pem_with_ip_and_groups() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crt = tmp.path().join("peer.crt");
        let key = tmp.path().join("peer.key");
        let backend = MockBackend;
        backend
            .sign_peer(
                Path::new("/tmp/ca.crt"),
                Path::new("/tmp/ca.key"),
                "peer:anvil",
                "10.42.0.5",
                16,
                &["role:peer"],
                &crt,
                &key,
            )
            .expect("sign");
        let body = std::fs::read_to_string(&crt).expect("read");
        assert!(body.contains("name=peer:anvil"));
        assert!(body.contains("ip=10.42.0.5/16"));
        assert!(body.contains("groups=role:peer"));
    }

    #[test]
    fn ca_error_display_includes_context() {
        let e = CaError::InsecurePermissions {
            path: "/var/lib/mackesd/nebula-ca/ca.key".to_string(),
            mode: 0o644,
        };
        let s = format!("{e}");
        assert!(s.contains("0o644") || s.contains("0644") || s.contains("644"));
    }
}
