//! VPN-GW-2 — tunnel-secret distribution over the mesh secret store
//! (design: `docs/design/vpn-gateway.md`).
//!
//! The VPN model ([`mackes_mesh_types::vpn_providers`]) keeps secret material
//! (the rendered `wg-quick` `.conf` / `.ovpn` body — it carries the private key)
//! OUT of the durable [`TunnelDef`], which only references it by `creds_ref`.
//! This module is the missing half: the node that sets a tunnel up
//! **age-encrypts** that secret and writes it to the **replicated** secret store
//! keyed by `creds_ref`, and any enrolled node resolves `creds_ref` → reads the
//! ciphertext → decrypts → materializes the config where `wg-quick`/`openvpn`
//! reads it.
//!
//! It is GLUE over the crypto MCNF already ships — there is no new cipher here:
//!
//!   * **Mesh store (production):** `automation/secrets/mcnf-secret.sh`
//!     (DATACENTER-3 / DS-8) — `age`-encrypts to the mesh recipient and stores
//!     the ciphertext in etcd (`/mcnf/secret/<name>`), replicated to every
//!     leader-eligible node holding the mesh age identity. Reached the same way
//!     `dc_health` / `host_ops` / `datacenter_orchestrator` already reach it: a
//!     `bash -lc` shell-out from the repo dir. This is the canonical store.
//!   * **Local AEAD fallback (single-node / no etcd):** the audited
//!     Argon2id + XChaCha20-Poly1305 envelope from [`crate::ca::backup`], keyed
//!     by the mesh age identity bytes, written under the workgroup root. Real
//!     crypto (the same primitive the CA disaster-recovery bundles use), so a
//!     box without a reachable etcd still gets at-rest-encrypted, durable
//!     secrets rather than plaintext.
//!
//! Honest states only: a [`SecretStore::get`] of an undistributed secret returns
//! `Ok(None)` (→ the bring-up path reports "secret distribution pending"), and a
//! store/tooling failure returns `Err` — never a fake success.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The path (relative to the repo root) of the mesh secret-store helper.
///
/// Single-sourced so a move only touches one line; matches the path the other
/// secret-store callers (`dc_health`, `host_ops`) use.
pub const MCNF_SECRET_SCRIPT: &str = "automation/secrets/mcnf-secret.sh";

/// Default deployed repo root holding `automation/secrets/mcnf-secret.sh`. The
/// daemon's systemd unit runs with cwd `/`, so the script can NOT be found
/// relative to the process cwd — the repo root is resolved explicitly from
/// `MCNF_REPO` (the project-wide convention, e.g. `disk-watchdog.sh`,
/// `mcnf-farm-reconcile.service`), defaulting here.
const DEFAULT_REPO_ROOT: &str = "/root/magic-mesh";

/// The mesh age **identity** (private) — the only host-local artifact of the
/// secret store, distributed to leader-eligible nodes like the mesh SSH key.
/// Overridable via `MCNF_AGE_KEY` to match the script's own default. Used by the
/// local-AEAD fallback to derive its key (the mesh store proper uses the key via
/// the `age` CLI inside the script).
const DEFAULT_AGE_KEY: &str = "/root/.mcnf-age-key";

/// Derive the secret-store key for a tunnel's materialized config from its
/// interface name (`name` in the mesh store / file stem in the fallback).
///
/// The `vpn/` prefix namespaces VPN creds away from the datacenter secrets
/// (`do-token`, `xapi-password`, …) that share the store. Pure + stable.
#[must_use]
pub fn creds_ref_for(ifname: &str) -> String {
    format!("vpn/{ifname}")
}

/// A keyed, encrypted, distribution-capable secret store. Both backends do real
/// encryption; the choice is driven by what the node can reach (see
/// [`SecretStore::resolve`]).
#[derive(Debug, Clone)]
pub enum SecretStore {
    /// The mesh `age` + etcd store via the `mcnf-secret.sh` helper, run from
    /// `repo_dir` (the worker cwd). Replicated; this is the production store.
    Mesh {
        /// The directory the `bash -lc <script>` is run from (the repo root /
        /// worker cwd), so a relative [`MCNF_SECRET_SCRIPT`] resolves.
        repo_dir: PathBuf,
    },
    /// A local, real-AEAD store: ciphertext files under `dir`, sealed with the
    /// [`crate::ca::backup`] envelope keyed by the mesh age identity at
    /// `key_path`. Single-node fallback when etcd isn't reachable, and the
    /// backend the round-trip tests drive.
    LocalAead {
        /// Where the per-secret ciphertext files live.
        dir: PathBuf,
        /// The mesh age identity file whose bytes key the AEAD.
        key_path: PathBuf,
    },
}

impl SecretStore {
    /// Pick the store this node should use: the mesh `age`+etcd store when its
    /// helper script is found under `repo_dir` (the canonical, replicated path),
    /// else the local-AEAD fallback rooted under `workgroup_root`.
    ///
    /// `repo_dir` must be the deployed repo ROOT (where `automation/secrets/...`
    /// lives) — NOT the process cwd, which is `/` under systemd. Callers resolve
    /// it via [`repo_root`].
    #[must_use]
    pub fn resolve(repo_dir: &Path, workgroup_root: &Path) -> Self {
        if repo_dir.join(MCNF_SECRET_SCRIPT).is_file() {
            Self::Mesh {
                repo_dir: repo_dir.to_path_buf(),
            }
        } else {
            Self::LocalAead {
                dir: workgroup_root.join("vpn").join("secrets"),
                key_path: age_key_path(),
            }
        }
    }

    /// Encrypt `plaintext` and write it to the store under `name`. The leader
    /// calls this when a tunnel's secret is produced, so enrolled nodes can read
    /// it back. Replicated when the backend is [`SecretStore::Mesh`].
    ///
    /// # Errors
    ///
    /// A tooling / I/O / crypto failure, with an operator-readable message. A
    /// failure here is reported honestly (the caller surfaces it) rather than
    /// claiming the secret was distributed.
    pub fn put(&self, name: &str, plaintext: &str) -> Result<(), String> {
        match self {
            Self::Mesh { repo_dir } => mesh_put(repo_dir, name, plaintext),
            Self::LocalAead { dir, key_path } => local_put(dir, key_path, name, plaintext),
        }
    }

    /// Read + decrypt the secret stored under `name`. `Ok(None)` when the secret
    /// isn't in the store yet (an honest "not distributed" — the bring-up path
    /// turns that into "secret distribution pending"). `Ok(Some(_))` is the
    /// decrypted config body.
    ///
    /// # Errors
    ///
    /// A tooling / I/O failure, or a decrypt failure (wrong key / tampered
    /// ciphertext). Distinguished from `Ok(None)` so a real fault isn't silently
    /// read as "pending".
    pub fn get(&self, name: &str) -> Result<Option<String>, String> {
        match self {
            Self::Mesh { repo_dir } => mesh_get(repo_dir, name),
            Self::LocalAead { dir, key_path } => local_get(dir, key_path, name),
        }
    }
}

/// The mesh age identity path (`MCNF_AGE_KEY` env, else [`DEFAULT_AGE_KEY`]) —
/// matches the script's own default so both backends key off the same artifact.
#[must_use]
fn age_key_path() -> PathBuf {
    std::env::var_os("MCNF_AGE_KEY").map_or_else(|| PathBuf::from(DEFAULT_AGE_KEY), PathBuf::from)
}

/// The deployed repo root holding the mesh secret-store helper.
///
/// `MCNF_REPO` (the project-wide convention) when set, else [`DEFAULT_REPO_ROOT`].
/// Used by [`SecretStore::resolve`] to find `automation/secrets/mcnf-secret.sh` —
/// NOT the process cwd, which is `/` for the systemd-launched daemon (so a
/// cwd-relative lookup would never find the script and would silently pick the
/// non-replicated local store).
#[must_use]
pub fn repo_root() -> PathBuf {
    std::env::var_os("MCNF_REPO").map_or_else(|| PathBuf::from(DEFAULT_REPO_ROOT), PathBuf::from)
}

// ── mesh store (age + etcd) via the mcnf-secret.sh helper ──

/// `mcnf-secret.sh put <name>` with `plaintext` on stdin (the script age-encrypts
/// stdin to the mesh recipient and stores the ciphertext in etcd). Run from
/// `repo_dir` so the relative script path resolves.
fn mesh_put(repo_dir: &Path, name: &str, plaintext: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(format!("{MCNF_SECRET_SCRIPT} put {}", shell_quote(name)))
        .current_dir(repo_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("secret store put: spawn failed: {e}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "secret store put: no stdin handle".to_string())?
        .write_all(plaintext.as_bytes())
        .map_err(|e| format!("secret store put: write stdin: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("secret store put: wait failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "secret store put exit {}: {}",
            out.status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string()),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Exit code `mcnf-secret.sh get` returns for a genuinely ABSENT secret (vs. a
/// real fault). The script fetches the ciphertext before decrypting precisely so
/// this stays distinguishable: absent → 3, any other non-zero → a fault.
const SECRET_ABSENT_EXIT: i32 = 3;

/// `mcnf-secret.sh get <name>` → decrypted body on stdout.
///
/// Exit-code discrimination (the script is written to make this honest):
///   * exit 0 → `Ok(Some(body))` (the decrypted secret),
///   * exit 3 → `Ok(None)` — the secret is genuinely not in the store yet
///     (honest "not distributed"),
///   * any other non-zero → `Err` — a real tooling fault (etcd unreachable,
///     missing/wrong age key, decrypt failure). Never silently swallowed as
///     "pending", so a broken store surfaces instead of stalling tunnel-up.
fn mesh_get(repo_dir: &Path, name: &str) -> Result<Option<String>, String> {
    let out = Command::new("bash")
        .arg("-lc")
        .arg(format!("{MCNF_SECRET_SCRIPT} get {}", shell_quote(name)))
        .current_dir(repo_dir)
        .output()
        .map_err(|e| format!("secret store get: spawn failed: {e}"))?;
    if out.status.success() {
        // A successfully-decrypted secret is non-empty (a wg `.conf`/`.ovpn`);
        // an empty body on a 0 exit would be a corrupt store entry, not "absent".
        let body = String::from_utf8_lossy(&out.stdout).to_string();
        if body.is_empty() {
            Err("secret store get: decrypted to empty (corrupt store entry)".to_string())
        } else {
            Ok(Some(body))
        }
    } else if out.status.code() == Some(SECRET_ABSENT_EXIT) {
        // The script's distinct "absent" code — honestly "not distributed yet".
        Ok(None)
    } else {
        Err(format!(
            "secret store get exit {}: {}",
            out.status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string()),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Single-quote `s` for a `bash -lc` argument. The secret `name` is a derived
/// `vpn/<ifname>` where `<ifname>` is `mvpn-` + alphanumerics ([`creds_ref_for`]
/// over [`mackes_mesh_types::vpn::TunnelDef::ifname`]), so it never contains a
/// quote — but quote defensively so a future caller can't inject.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ── local AEAD fallback (ca::backup envelope, mesh-age-keyed) ──

/// The on-disk ciphertext path for `name` under `dir`. `name` is `vpn/<ifname>`;
/// the `/` becomes a `__` so it's a single flat file (no nested-dir surprises),
/// and the parent is created on write.
fn local_secret_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.age", name.replace('/', "__")))
}

/// Derive the local-AEAD passphrase from the mesh age identity bytes. The
/// identity is the same artifact gating the mesh store, so a node that can
/// decrypt mesh secrets can decrypt these and vice-versa (same trust root). The
/// raw key bytes (not a typed-passphrase) feed Argon2id inside the envelope.
///
/// # Errors
///
/// When the mesh age identity is absent — without it there is no trust root to
/// key the local store, so we fail honestly rather than invent one.
fn local_passphrase(key_path: &Path) -> Result<String, String> {
    use std::fmt::Write as _;
    let bytes = std::fs::read(key_path).map_err(|e| {
        format!(
            "local secret store: mesh age identity {} unreadable: {e}",
            key_path.display()
        )
    })?;
    if bytes.is_empty() {
        return Err(format!(
            "local secret store: mesh age identity {} is empty",
            key_path.display()
        ));
    }
    // Hex the key bytes into a stable, non-empty passphrase for the envelope's
    // Argon2id KDF (it wants a `&str`). Format-stable: this hex IS the on-disk
    // key-derivation, so changing the encoding would orphan every sealed file.
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in &bytes {
        let _ = write!(hex, "{b:02x}");
    }
    Ok(hex)
}

/// Seal `plaintext` under the [`crate::ca::backup`] envelope and write the
/// ciphertext file 0600 (it decrypts to the private key).
fn local_put(dir: &Path, key_path: &Path, name: &str, plaintext: &str) -> Result<(), String> {
    let passphrase = local_passphrase(key_path)?;
    let sealed = crate::ca::backup::seal_bytes(&passphrase, plaintext.as_bytes())
        .map_err(|e| format!("local secret store: seal: {e}"))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("local secret store: mkdir {}: {e}", dir.display()))?;
    let path = local_secret_path(dir, name);
    std::fs::write(&path, &sealed)
        .map_err(|e| format!("local secret store: write {}: {e}", path.display()))?;
    set_owner_only(&path);
    Ok(())
}

/// Read + decrypt the ciphertext file. Missing file → `Ok(None)` (not
/// distributed); a decrypt failure → `Err` (wrong key / tamper).
fn local_get(dir: &Path, key_path: &Path, name: &str) -> Result<Option<String>, String> {
    let path = local_secret_path(dir, name);
    let sealed = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("local secret store: read {}: {e}", path.display())),
    };
    let passphrase = local_passphrase(key_path)?;
    let plain = crate::ca::backup::unseal_bytes(&passphrase, &sealed)
        .map_err(|e| format!("local secret store: unseal {}: {e}", path.display()))?;
    String::from_utf8(plain)
        .map(Some)
        .map_err(|e| format!("local secret store: secret not utf-8: {e}"))
}

/// Best-effort 0600 on a secret file (Unix).
fn set_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real WG private key body so the secret is realistic (44-char base64).
    const PK: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    fn wg_conf() -> String {
        format!(
            "[Interface]\nPrivateKey = {PK}\nAddress = 10.64.0.2/32\nDNS = 10.64.0.1\n\n\
             [Peer]\nPublicKey = BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=\n\
             AllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = us-nyc.relays.example:51820\n\
             PersistentKeepalive = 25\n"
        )
    }

    /// Stand up a `LocalAead` store with a real (random-ish) age identity file.
    fn local_store() -> (tempfile::TempDir, SecretStore) {
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("mcnf-age-key");
        // A realistic age identity line (the bytes are all that key the AEAD).
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        let store = SecretStore::LocalAead {
            dir: tmp.path().join("secrets"),
            key_path,
        };
        (tmp, store)
    }

    #[test]
    fn creds_ref_namespaces_under_vpn() {
        assert_eq!(creds_ref_for("mvpn-mullvad1"), "vpn/mvpn-mullvad1");
    }

    #[test]
    fn local_round_trip_encrypt_store_read_decrypt() {
        let (_t, store) = local_store();
        let name = creds_ref_for("mvpn-mullvad1");
        // Not distributed yet → honest None.
        assert_eq!(store.get(&name).unwrap(), None);
        // Leader distributes.
        let secret = wg_conf();
        store.put(&name, &secret).unwrap();
        // Any enrolled node reads it back decrypted, byte-for-byte.
        assert_eq!(store.get(&name).unwrap(), Some(secret));
    }

    #[test]
    fn local_ciphertext_on_disk_is_not_plaintext() {
        let (_t, store) = local_store();
        let name = creds_ref_for("mvpn-x");
        let secret = wg_conf();
        store.put(&name, &secret).unwrap();
        let SecretStore::LocalAead { dir, .. } = &store else {
            unreachable!()
        };
        let raw = std::fs::read(local_secret_path(dir, &name)).unwrap();
        // The private key never appears in the at-rest ciphertext.
        assert!(
            !raw.windows(PK.len()).any(|w| w == PK.as_bytes()),
            "private key leaked into the at-rest secret file"
        );
        // It IS our envelope (magic header), i.e. real sealing happened.
        assert_eq!(&raw[..4], crate::ca::backup::BUNDLE_MAGIC);
    }

    #[test]
    fn local_wrong_key_fails_decrypt_not_silently_none() {
        let (_t, store) = local_store();
        let name = creds_ref_for("mvpn-y");
        store.put(&name, &wg_conf()).unwrap();
        // A node with a DIFFERENT mesh identity can't read it: a decrypt error,
        // never a fake None ("pending") or a fake success.
        let SecretStore::LocalAead { dir, .. } = &store else {
            unreachable!()
        };
        let other_key = _t.path().join("other-key");
        std::fs::write(&other_key, "AGE-SECRET-KEY-1DIFFERENTKEYBYTESZZZ\n").unwrap();
        let other = SecretStore::LocalAead {
            dir: dir.clone(),
            key_path: other_key,
        };
        assert!(other.get(&name).is_err());
    }

    #[test]
    fn local_missing_age_identity_is_honest_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SecretStore::LocalAead {
            dir: tmp.path().join("secrets"),
            key_path: tmp.path().join("does-not-exist"),
        };
        assert!(store.put("vpn/mvpn-z", "x").is_err());
    }

    #[test]
    fn resolve_picks_mesh_when_script_present_else_local() {
        let repo = tempfile::tempdir().unwrap();
        let wg = tempfile::tempdir().unwrap();
        // No script → local fallback.
        match SecretStore::resolve(repo.path(), wg.path()) {
            SecretStore::LocalAead { dir, .. } => {
                assert!(dir.starts_with(wg.path()));
            }
            other => panic!("expected LocalAead, got {other:?}"),
        }
        // Script present → mesh store.
        let script = repo.path().join(MCNF_SECRET_SCRIPT);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "#!/usr/bin/env bash\n").unwrap();
        match SecretStore::resolve(repo.path(), wg.path()) {
            SecretStore::Mesh { repo_dir } => assert_eq!(repo_dir, repo.path()),
            other => panic!("expected Mesh, got {other:?}"),
        }
    }
}
