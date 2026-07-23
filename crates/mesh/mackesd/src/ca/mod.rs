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
// MIG-3 — provision the sealed CA-backup passphrase credential on a
// JOINED lighthouse so it boots without the SEC-7/ENT-11 "UNBACKED-UP"
// warning. Called from the `mackesd join --role lighthouse` flow.
pub mod backup_provision;
// EPIC-SEC-BANLIST (Q53) — compromised-node ban list checked at
// the CSR sign gate (nebula_enroll::sign_pending_csr).
pub mod ban_list;
pub mod blocklist;
// EFF-11 — CA-cert expiry probe (days-remaining via nebula-cert
// print -json). Consumed by the metrics_exporter worker for the
// mackesd_ca_cert_days_remaining metric + threshold alert.
pub mod expiry;
pub mod rotation_gate;
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
    /// Generate a Nebula X25519 keypair on the requester, writing the private
    /// and public PEM files to caller-owned local paths.
    fn generate_keypair(&self, key_out: &Path, public_out: &Path) -> Result<(), CaError> {
        let _ = (key_out, public_out);
        Err(CaError::Subprocess(
            "backend does not support requester-side key generation".into(),
        ))
    }

    /// Equivalent of `nebula-cert ca -name <mesh_id> -out-crt <crt_out> -out-key <key_out>`.
    /// Implementations write the CA cert + key to the given
    /// paths.
    fn mint_ca(&self, mesh_id: &str, crt_out: &Path, key_out: &Path) -> Result<(), CaError>;

    /// Legacy test fixture for modeling the retired CA-generated peer-key path.
    /// Production builds expose only requester-public-key signing.
    #[cfg(test)]
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
        let _ = (
            ca_crt,
            ca_key,
            node_id,
            overlay_ip,
            cidr_prefix,
            groups,
            crt_out,
            key_out,
        );
        Err(CaError::Subprocess(
            "central peer-private-key generation is retired".into(),
        ))
    }

    /// Sign a requester-generated Nebula public key. Equivalent to
    /// `nebula-cert sign ... -in-pub <public_key> -out-crt <crt_out>` and
    /// deliberately has no private-key output path.
    fn sign_peer_with_public_key(
        &self,
        ca_crt: &Path,
        ca_key: &Path,
        node_id: &str,
        overlay_ip: &str,
        cidr_prefix: u8,
        groups: &[&str],
        public_key: &Path,
        crt_out: &Path,
    ) -> Result<(), CaError> {
        let _ = (
            ca_crt,
            ca_key,
            node_id,
            overlay_ip,
            cidr_prefix,
            groups,
            public_key,
            crt_out,
        );
        Err(CaError::Subprocess(
            "backend does not support requester-owned public-key signing".into(),
        ))
    }
}

/// Production backend that shells out to `nebula-cert`.
pub struct SubprocessBackend;

impl NebulaCertBackend for SubprocessBackend {
    fn generate_keypair(&self, key_out: &Path, public_out: &Path) -> Result<(), CaError> {
        generate_pair_in_private_staging("requester-keygen", public_out, key_out, |public, key| {
            run_nebula_cert(&[
                "keygen",
                "-out-key",
                &key.display().to_string(),
                "-out-pub",
                &public.display().to_string(),
            ])
        })
    }

    fn mint_ca(&self, mesh_id: &str, crt_out: &Path, key_out: &Path) -> Result<(), CaError> {
        generate_pair_in_private_staging("ca-mint", crt_out, key_out, |crt, key| {
            run_nebula_cert(&[
                "ca",
                "-name",
                mesh_id,
                "-out-crt",
                &crt.display().to_string(),
                "-out-key",
                &key.display().to_string(),
            ])
        })
    }

    fn sign_peer_with_public_key(
        &self,
        ca_crt: &Path,
        ca_key: &Path,
        node_id: &str,
        overlay_ip: &str,
        cidr_prefix: u8,
        groups: &[&str],
        public_key: &Path,
        crt_out: &Path,
    ) -> Result<(), CaError> {
        let ip_with_mask = format!("{overlay_ip}/{cidr_prefix}");
        let groups_joined = groups.join(",");
        let ca_crt_s = ca_crt.display().to_string();
        let ca_key_s = ca_key.display().to_string();
        let public_key_s = public_key.display().to_string();
        let crt_out_s = crt_out.display().to_string();
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
            "-in-pub",
            public_key_s.as_str(),
        ];
        if !groups.is_empty() {
            args.push("-groups");
            args.push(groups_joined.as_str());
        }
        run_nebula_cert(&args)
    }
}

fn generate_pair_in_private_staging<F>(
    label: &str,
    public_out: &Path,
    secret_out: &Path,
    generate: F,
) -> Result<(), CaError>
where
    F: FnOnce(&Path, &Path) -> Result<(), CaError>,
{
    use std::os::unix::fs::DirBuilderExt as _;

    let parent = public_out.parent().ok_or_else(|| {
        CaError::Io(format!(
            "generated output has no parent: {}",
            public_out.display()
        ))
    })?;
    if secret_out.parent() != Some(parent) {
        return Err(CaError::Io(
            "generated certificate/key outputs must share one directory".into(),
        ));
    }
    std::fs::create_dir_all(parent)
        .map_err(|e| CaError::Io(format!("mkdir {}: {e}", parent.display())))?;
    let staging = (0..16)
        .find_map(|_| {
            let candidate = parent.join(format!(
                ".{label}-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&candidate) {
                Ok(()) => Some(Ok(candidate)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(CaError::Io(format!(
                    "create private staging {}: {error}",
                    candidate.display()
                )))),
            }
        })
        .unwrap_or_else(|| Err(CaError::Io(format!("{label} staging collisions"))))?;
    let result = (|| {
        let staged_public = staging.join("public.pem");
        let staged_secret = staging.join("private.key");
        generate(&staged_public, &staged_secret)?;
        let public = std::fs::read(&staged_public).map_err(|e| {
            CaError::Io(format!(
                "read generated public {}: {e}",
                staged_public.display()
            ))
        })?;
        let secret = std::fs::read(&staged_secret).map_err(|e| {
            CaError::Io(format!(
                "read generated secret {}: {e}",
                staged_secret.display()
            ))
        })?;
        if public.is_empty() || secret.is_empty() {
            return Err(CaError::Io(format!("{label} emitted an empty output")));
        }
        seal::write_atomic_pair(public_out, &public, secret_out, &secret)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn run_nebula_cert(args: &[&str]) -> Result<(), CaError> {
    // Keep the umask child-local without `pre_exec` (this crate forbids unsafe
    // code). The script is constant and caller arguments are forwarded only as
    // positional parameters, so no path or identity value is shell-evaluated.
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "umask 077; exec nebula-cert \"$@\"", "nebula-cert"]);
    command.args(args);
    let output = command.output().map_err(|e| {
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
        // The shell wrapper above keeps the child-local umask without unsafe
        // `pre_exec`, but it also means a missing `nebula-cert` is reported by
        // `sh` as exit 127 rather than `Command::output` returning NotFound.
        // Preserve the typed error contract for that common dev/CI case.
        if output.status.code() == Some(127) && stderr.to_ascii_lowercase().contains("nebula-cert")
        {
            return Err(CaError::BinaryMissing);
        }
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
    fn generate_keypair(&self, key_out: &Path, public_out: &Path) -> Result<(), CaError> {
        seal::write_atomic_pair(
            public_out,
            b"-----BEGIN NEBULA X25519 PUBLIC KEY-----\nAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n-----END NEBULA X25519 PUBLIC KEY-----\n",
            key_out,
            b"-----BEGIN NEBULA X25519 PRIVATE KEY-----\nmock\n-----END NEBULA X25519 PRIVATE KEY-----\n",
        )
    }

    fn mint_ca(&self, mesh_id: &str, crt_out: &Path, key_out: &Path) -> Result<(), CaError> {
        let cert = format!("-----BEGIN NEBULA CA-----\nmesh={mesh_id}\n-----END NEBULA CA-----\n");
        let key =
            format!("-----BEGIN NEBULA CA KEY-----\nmesh={mesh_id}\n-----END NEBULA CA KEY-----\n");
        seal::write_atomic_pair(crt_out, cert.as_bytes(), key_out, key.as_bytes())
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

    fn sign_peer_with_public_key(
        &self,
        _ca_crt: &Path,
        _ca_key: &Path,
        node_id: &str,
        overlay_ip: &str,
        cidr_prefix: u8,
        groups: &[&str],
        public_key: &Path,
        crt_out: &Path,
    ) -> Result<(), CaError> {
        let public_key_pem = std::fs::read_to_string(public_key)
            .map_err(|e| CaError::Io(format!("read public key {}: {e}", public_key.display())))?;
        if public_key_pem.trim().is_empty() {
            return Err(CaError::Io("requester public key is empty".into()));
        }
        if let Some(parent) = crt_out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CaError::Io(e.to_string()))?;
        }
        std::fs::write(
            crt_out,
            format!(
                "-----BEGIN NEBULA CERT-----\nname={node_id}\nip={overlay_ip}/{cidr_prefix}\n\
                 groups={}\nrequester_public_key=true\n-----END NEBULA CERT-----\n",
                groups.join(",")
            ),
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
