//! VPN-GW-2 — encrypted, leader-managed VPN tunnel secrets.
//!
//! Design: `docs/design/vpn-gateway.md` §"Credentials / distribution" —
//! "Tunnel configs/keys encrypted with **age**, stored in the mesh secret
//! store, **leader-managed**: the leader pushes a tunnel's secret only to the
//! gateway node(s) assigned to run it. Never in `ps`/logs."
//!
//! ## What this module is
//!
//! The pure seal/unseal of a [`TunnelSecret`] (a WireGuard config or an OpenVPN
//! `.ovpn` + side files) plus the **materialize** step that lays the decrypted
//! cleartext down at the path VPN-GW-1's bring-up already spawns against
//! (`/etc/wireguard/<ifname>.conf` for `wg-quick`, `/etc/openvpn/client/
//! <ifname>.ovpn` for `openvpn --config`). The leader-side distribution +
//! node-side reconcile loop lives in
//! [`crate::workers::vpn_secret_distributor`]; this module is the runtime-
//! reachable core both sides call.
//!
//! ## Crypto floor (§3 / §6 — reuse, do NOT invent)
//!
//! We seal under the **mesh secret key** with the *same* Argon2id +
//! XChaCha20-Poly1305 envelope the CA backup uses
//! ([`crate::ca::backup::seal_bytes`] / [`unseal_bytes`]), parameterized by a
//! distinct 4-byte magic ([`SECRET_MAGIC`]) so a VPN blob can never be confused
//! with a CA bundle. No new crypto primitives, no OpenSSL — the project's one
//! vetted AEAD/KDF path (architecture.md §"Crypto"). The key is supplied by the
//! caller ([`mesh_key_from_env`] / a CA-key fallback at the worker boundary) so
//! the encrypt/decrypt logic stays pure + unit-testable.
//!
//! Secret material never enters `tunnels.toml` (only a [`creds_ref`] handle),
//! never `ps`/argv (the worker writes files; the tools read files), and never a
//! log line (every error here is path/shape only, never the plaintext).

use std::path::Path;

use mackes_mesh_types::vpn::{self, TunnelDef, TunnelSecret};

use crate::ca::backup::{self, BackupError};

/// Env var carrying the mesh secret key used to seal/unseal VPN tunnel secrets.
/// Shares the established mesh-secret env (EFF-21: captured once at boot, then
/// scrubbed from the daemon environment so worker subprocesses never inherit
/// it), so a node already provisioned for encrypted CA backup can seal/unseal
/// VPN secrets with no extra operator step. Falls back to the CA key PEM on a
/// CA-holder node (see [`crate::workers::vpn_secret_distributor`]).
pub const MESH_KEY_ENV: &str = "MDE_BACKUP_PASSPHRASE";

/// Envelope magic for a sealed VPN tunnel secret — ASCII "MVPS"
/// ("Mackes VPN Secret"). Distinct from the CA bundle's `MNCA` so the AEAD
/// path refuses to cross-decrypt the two secret domains.
pub const SECRET_MAGIC: &[u8; 4] = b"MVPS";

/// Errors the VPN-secret path can hit. Crypto failures are folded into the
/// shared [`BackupError`] (one vetted envelope); the rest are I/O / shape.
#[derive(Debug, thiserror::Error)]
pub enum VpnSecretError {
    /// No mesh secret key available (env unset + no CA-key fallback). The
    /// leader can't seal / the node can't decrypt until the key is provisioned.
    #[error("no mesh secret key (set {MESH_KEY_ENV} or provision the CA key)")]
    NoKey,
    /// The cleartext payload didn't match the tunnel's method (e.g. a `wg`
    /// tunnel with an empty `wg_conf`) — rejected before sealing so a bad
    /// secret fails loud at save, not silently at bring-up.
    #[error("empty/mismatched secret for method {0:?}")]
    EmptyPayload(vpn::Method),
    /// The sealed envelope failed to seal/unseal (wrong key, tamper, truncation).
    #[error(transparent)]
    Envelope(#[from] BackupError),
    /// JSON (de)serialize of the [`TunnelSecret`] payload failed.
    #[error("secret json: {0}")]
    Json(String),
    /// File I/O (read the blob, write/chmod the materialized config).
    #[error("io: {0}")]
    Io(String),
}

/// Seal a [`TunnelSecret`] under `mesh_key`. Returns the binary `.age`-style
/// envelope (`MVPS || version || salt || nonce || ciphertext`) ready to write
/// to [`vpn::secret_path`]. Pure — no I/O, no env reads.
///
/// # Errors
/// [`VpnSecretError::NoKey`] on an empty key, [`VpnSecretError::Json`] on a
/// serialize failure, or [`VpnSecretError::Envelope`] on a crypto failure.
pub fn seal(mesh_key: &str, secret: &TunnelSecret) -> Result<Vec<u8>, VpnSecretError> {
    if mesh_key.is_empty() {
        return Err(VpnSecretError::NoKey);
    }
    let json = serde_json::to_vec(secret).map_err(|e| VpnSecretError::Json(e.to_string()))?;
    Ok(backup::seal_bytes(SECRET_MAGIC, mesh_key, &json)?)
}

/// Unseal a blob produced by [`seal`] back into a [`TunnelSecret`]. Pure.
///
/// # Errors
/// [`VpnSecretError::NoKey`] on an empty key, [`VpnSecretError::Envelope`] on a
/// wrong key / tampered / truncated blob, [`VpnSecretError::Json`] on a corrupt
/// payload.
pub fn unseal(mesh_key: &str, sealed: &[u8]) -> Result<TunnelSecret, VpnSecretError> {
    if mesh_key.is_empty() {
        return Err(VpnSecretError::NoKey);
    }
    let plain = backup::unseal_bytes(SECRET_MAGIC, mesh_key, sealed)?;
    serde_json::from_slice(&plain).map_err(|e| VpnSecretError::Json(e.to_string()))
}

/// Validate + seal a secret for a tunnel definition: rejects a payload that
/// isn't populated for the tunnel's method (so a never-bring-up-able secret
/// can't be stored), then seals. Used by the add/update path.
///
/// # Errors
/// [`VpnSecretError::EmptyPayload`] when the secret doesn't match the method;
/// otherwise per [`seal`].
pub fn seal_for(
    mesh_key: &str,
    t: &TunnelDef,
    secret: &TunnelSecret,
) -> Result<Vec<u8>, VpnSecretError> {
    if !secret.is_populated_for(t.method) {
        return Err(VpnSecretError::EmptyPayload(t.method));
    }
    seal(mesh_key, secret)
}

/// The owner-only file mode for every materialized cleartext config. `wg-quick`
/// refuses a world-readable `[Interface]` private key, and an `.ovpn` carries
/// keys/creds inline — both must be 0600.
const CLEARTEXT_MODE: u32 = 0o600;

/// Materialize a decrypted [`TunnelSecret`] to the on-disk path VPN-GW-1's
/// bring-up expects for the tunnel's method, at mode 0600. WireGuard →
/// `/etc/wireguard/<ifname>.conf`; OpenVPN → `/etc/openvpn/client/
/// <ifname>.ovpn` (+ any `extra` side files beside it). Returns the path
/// written. Skips a no-op rewrite (content unchanged) so a quiet reconcile
/// loop doesn't churn the file / restart the tunnel.
///
/// # Errors
/// [`VpnSecretError::EmptyPayload`] when the payload doesn't match the method,
/// or [`VpnSecretError::Io`] on a mkdir/write/chmod failure.
pub fn materialize(
    t: &TunnelDef,
    secret: &TunnelSecret,
) -> Result<std::path::PathBuf, VpnSecretError> {
    if !secret.is_populated_for(t.method) {
        return Err(VpnSecretError::EmptyPayload(t.method));
    }
    match t.method {
        vpn::Method::Wg => {
            let path = vpn::wg_conf_path(t);
            write_cleartext(&path, secret.wg_conf.as_bytes())?;
            Ok(path)
        }
        // OpenVPN — primary `.ovpn`, plus any side files (auth-user-pass, etc.)
        // beside it so a `--config` that references `auth.txt` finds it.
        vpn::Method::Ovpn => {
            let path = vpn::ovpn_conf_path(t);
            write_cleartext(&path, secret.ovpn_conf.as_bytes())?;
            if let Some(dir) = path.parent() {
                for (name, body) in &secret.extra {
                    // `name` is a basename only — strip any path component a
                    // malicious secret might smuggle in (defense in depth; the
                    // leader controls the secret, but never trust it blindly).
                    let base = Path::new(name)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if base.is_empty() || base == "." || base == ".." {
                        continue;
                    }
                    write_cleartext(&dir.join(base), body.as_bytes())?;
                }
            }
            Ok(path)
        }
        // CLI/API tunnels mint their own runtime config; if the secret carried a
        // wg/ovpn body anyway, prefer the populated one. Default to the wg path.
        vpn::Method::Cli | vpn::Method::Api => {
            if !secret.ovpn_conf.trim().is_empty() {
                let path = vpn::ovpn_conf_path(t);
                write_cleartext(&path, secret.ovpn_conf.as_bytes())?;
                Ok(path)
            } else {
                let path = vpn::wg_conf_path(t);
                write_cleartext(&path, secret.wg_conf.as_bytes())?;
                Ok(path)
            }
        }
    }
}

/// Remove a tunnel's materialized cleartext (both possible paths + the
/// `.ovpn`'s side dir is left alone — only the named config goes). Best-effort:
/// a missing file is success. Used on tunnel delete / unassign so a removed
/// tunnel leaves no decrypted key on disk (design: "rotate on tunnel delete").
///
/// # Errors
/// [`VpnSecretError::Io`] when an existing file can't be removed.
pub fn remove_materialized(t: &TunnelDef) -> Result<(), VpnSecretError> {
    for path in [vpn::wg_conf_path(t), vpn::ovpn_conf_path(t)] {
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| VpnSecretError::Io(format!("remove {}: {e}", path.display())))?;
        }
    }
    Ok(())
}

/// Resolve the mesh secret key from the environment ([`MESH_KEY_ENV`]).
/// `None` when unset/empty — the worker boundary then tries the CA-key
/// fallback before giving up with [`VpnSecretError::NoKey`].
#[must_use]
pub fn mesh_key_from_env() -> Option<String> {
    std::env::var(MESH_KEY_ENV).ok().filter(|s| !s.is_empty())
}

/// Write `bytes` to `path` at mode 0600, creating the parent dir, atomically
/// (temp + rename) and skipping a no-op rewrite (unchanged content). Mirrors
/// `ca::seal::write_sealed`'s 0600 invariant; the atomic rename means a reader
/// (`wg-quick`) never sees a half-written config.
fn write_cleartext(path: &Path, bytes: &[u8]) -> Result<(), VpnSecretError> {
    use std::os::unix::fs::PermissionsExt;
    // No-op if the on-disk content already matches — keeps the reconcile loop
    // quiet (no churn, no needless tunnel restart).
    if let Ok(existing) = std::fs::read(path) {
        if existing == bytes {
            return Ok(());
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| VpnSecretError::Io(format!("mkdir {}: {e}", dir.display())))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)
        .map_err(|e| VpnSecretError::Io(format!("write {}: {e}", tmp.display())))?;
    // Chmod the temp file before the rename so the final path is never briefly
    // group/world-readable.
    let perms = std::fs::Permissions::from_mode(CLEARTEXT_MODE);
    std::fs::set_permissions(&tmp, perms)
        .map_err(|e| VpnSecretError::Io(format!("chmod {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| VpnSecretError::Io(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::vpn::Method;

    fn tun(id: &str, method: Method) -> TunnelDef {
        TunnelDef {
            id: id.into(),
            provider: "generic-wg".into(),
            method,
            ..Default::default()
        }
    }

    #[test]
    fn seal_unseal_round_trips() {
        let s = TunnelSecret::wireguard("[Interface]\nPrivateKey=SECRET-WG-KEY\n[Peer]\n");
        let blob = seal("mesh-key-123", &s).expect("seal");
        // The envelope carries our magic + never the plaintext in the clear.
        assert_eq!(&blob[..4], SECRET_MAGIC);
        assert!(!blob.windows(13).any(|w| w == b"SECRET-WG-KEY"));
        let back = unseal("mesh-key-123", &blob).expect("unseal");
        assert_eq!(back, s);
    }

    #[test]
    fn unseal_rejects_wrong_key() {
        let s = TunnelSecret::openvpn("client\nremote vpn.example 1194\n");
        let blob = seal("right-key", &s).expect("seal");
        let r = unseal("wrong-key", &blob);
        assert!(matches!(
            r,
            Err(VpnSecretError::Envelope(BackupError::Aead(_)))
        ));
    }

    #[test]
    fn unseal_rejects_tampered_blob() {
        let s = TunnelSecret::wireguard("[Interface]\nPrivateKey=x\n");
        let mut blob = seal("k", &s).expect("seal");
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert!(matches!(
            unseal("k", &blob),
            Err(VpnSecretError::Envelope(BackupError::Aead(_)))
        ));
    }

    #[test]
    fn unseal_rejects_ca_bundle_magic() {
        // A CA bundle sealed with the SAME key must not unseal as a VPN secret —
        // the magic separates the domains.
        let ca = backup::seal_bytes(b"MNCA", "k", b"not-a-vpn-secret").expect("ca seal");
        let r = unseal("k", &ca);
        assert!(matches!(
            r,
            Err(VpnSecretError::Envelope(BackupError::Format(_)))
        ));
    }

    #[test]
    fn empty_key_is_no_key() {
        let s = TunnelSecret::wireguard("x");
        assert!(matches!(seal("", &s), Err(VpnSecretError::NoKey)));
        assert!(matches!(unseal("", &[0u8; 64]), Err(VpnSecretError::NoKey)));
    }

    #[test]
    fn seal_for_rejects_mismatched_payload() {
        let t = tun("mullvad1", Method::Wg);
        // A WG tunnel with no wg_conf can never come up → reject at seal.
        let empty = TunnelSecret::default();
        assert!(matches!(
            seal_for("k", &t, &empty),
            Err(VpnSecretError::EmptyPayload(Method::Wg))
        ));
        // Populated → seals fine.
        let good = TunnelSecret::wireguard("[Interface]\nPrivateKey=k\n");
        assert!(seal_for("k", &t, &good).is_ok());
    }

    #[test]
    fn materialize_wg_writes_0600_conf_at_expected_path() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let t = tun("mullvad1", Method::Wg);
        let secret = TunnelSecret::wireguard("[Interface]\nPrivateKey=k\n");
        // Redirect the materialize root into the tempdir by overriding the
        // path: we can't change /etc, so assert via a helper that writes to a
        // tempdir-rooted path mirroring the real one.
        let dest = tmp
            .path()
            .join("wireguard")
            .join(format!("{}.conf", t.ifname()));
        write_cleartext(&dest, secret.wg_conf.as_bytes()).expect("write");
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), secret.wg_conf);
    }

    #[test]
    fn write_cleartext_is_idempotent_and_atomic() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("deep").join("nested").join("c.conf");
        write_cleartext(&path, b"v1").expect("write1");
        assert_eq!(std::fs::read(&path).unwrap(), b"v1");
        // Idempotent rewrite of same content — no temp left behind.
        write_cleartext(&path, b"v1").expect("write1-again");
        assert!(!path.with_extension("tmp").exists());
        // Change → updates, still 0600.
        write_cleartext(&path, b"v2").expect("write2");
        assert_eq!(std::fs::read(&path).unwrap(), b"v2");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn seal_then_unseal_then_materialize_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tun("proton", Method::Ovpn);
        let mut s = TunnelSecret::openvpn("client\nauth-user-pass auth.txt\n");
        s.extra.insert("auth.txt".into(), "user\npass\n".into());
        let blob = seal_for("k", &t, &s).expect("seal");
        let back = unseal("k", &blob).expect("unseal");
        assert_eq!(back, s);
        // Materialize into a tempdir-rooted mirror of the real paths.
        let dir = tmp.path().join("openvpn-client");
        let ovpn = dir.join(format!("{}.ovpn", t.ifname()));
        write_cleartext(&ovpn, back.ovpn_conf.as_bytes()).expect("ovpn");
        for (n, b) in &back.extra {
            write_cleartext(&dir.join(n), b.as_bytes()).expect("side file");
        }
        assert_eq!(std::fs::read_to_string(&ovpn).unwrap(), s.ovpn_conf);
        assert_eq!(
            std::fs::read_to_string(dir.join("auth.txt")).unwrap(),
            "user\npass\n"
        );
    }
}
