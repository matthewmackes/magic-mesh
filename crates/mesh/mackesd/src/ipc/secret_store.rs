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

const MAX_SECRET_NAME_BYTES: usize = 192;

/// Keep local ciphertext reads bounded while leaving room for the existing
/// secret plaintext ceiling plus the MNCA envelope's fixed framing and AEAD
/// tag. The extra byte read below is intentional: it detects a file that grows
/// past the limit after the descriptor is opened without allocating unbounded
/// storage.
const MAX_SECRET_CIPHERTEXT_BYTES: usize = 1024 * 1024 + 64;

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

// arch-7 — the mesh age **identity** path (`MCNF_AGE_KEY` env, else
// `/root/.mcnf-age-key`) moved into the shared `mde-seal` crate alongside the
// seal/unseal primitives, so consumers that key against it (the local-AEAD
// fallback here and `browser_passkeys`) reach it without depending on `mackesd`.
// Re-exported so `secret_store::age_key_path` callers + this module's own
// `resolve` use it unchanged.
pub use mde_seal::age_key_path;

/// Derive the secret-store key for a tunnel's materialized config from its
/// interface name (`name` in the mesh store / file stem in the fallback).
///
/// The `vpn/` prefix namespaces VPN creds away from the datacenter secrets
/// (`do-token`, `xapi-password`, …) that share the store. Pure + stable.
#[must_use]
pub fn creds_ref_for(ifname: &str) -> String {
    format!("vpn/{ifname}")
}

/// XCP-7 — derive the secret-store key for a dom0's XAPI/root credential from
/// its host address. The `xcp/` prefix namespaces these alongside the `vpn/`
/// tunnel creds (the design's `<QNM-Shared>/secrets/xcp/<host>.age` intent), so
/// any authorized node resolves `xcp/<host>` → reads the ciphertext → decrypts
/// the dom0 password. Pure + stable: this string IS the on-disk / etcd key, so a
/// change orphans every stored credential.
///
/// `host` is sanitized to the address charset (alnum, `.`, `-`, `:`) so a stray
/// separator can't widen the key namespace; the `vpn/` half quotes defensively
/// for the same reason ([`shell_quote`]).
#[must_use]
pub fn xcp_creds_ref(host: &str) -> String {
    let safe: String = host
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
        .collect();
    format!("xcp/{safe}")
}

/// FRONTDOOR-9 — derive the secret-store key for the Copilot codex API key.
///
/// The `copilot/` prefix namespaces the AI backend's credential alongside the
/// `vpn/` tunnel creds and `xcp/` dom0 creds (and the bare datacenter secrets
/// `do-token`, `xapi-password`, …) that share the store. The codex worker runs
/// LEADER-only, so only the elected node ever resolves this and shells codex —
/// the key is sealed in the mesh `age`+etcd store and read back on demand.
///
/// Pure + stable: this string IS the on-disk / etcd key, so a change orphans the
/// stored credential (rotation is "set once" per the design Q93 lock).
#[must_use]
pub fn codex_creds_ref() -> String {
    "copilot/codex-api-key".to_string()
}

/// MEDIA-8 — the secret-store key for the media-spaces secret (the DO Spaces S3
/// creds + the Navidrome `ND_ADMIN_USER`/`ND_ADMIN_PASS` shared account, sealed
/// by MEDIA-2). A BARE name (no `vpn/`/`xcp/` prefix) — it's a datacenter-tier
/// secret like `do-token`/`xapi-password` that `setup-media-navidrome.sh`
/// already consumes from a root-only env file; the Lighthouse_Media media
/// registry worker reads the SAME sealed secret to publish the shared account.
///
/// Pure + stable: this string IS the etcd/on-disk key, so a change orphans the
/// stored secret.
#[must_use]
pub fn media_spaces_creds_ref() -> String {
    "media-spaces".to_string()
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
    /// Validate a secret reference before it reaches either the replicated
    /// helper or the local filesystem. References are a small, stable key
    /// language (`namespace/name`); accepting arbitrary strings here would
    /// make the CLI an authority over unrelated etcd keys and would also make
    /// the local flat-file encoding ambiguous.
    pub fn validate_name(name: &str) -> Result<(), String> {
        validate_secret_name(name)
    }

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
        validate_secret_name(name)?;
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
        validate_secret_name(name)?;
        match self {
            Self::Mesh { repo_dir } => mesh_get(repo_dir, name),
            Self::LocalAead { dir, key_path } => local_get(dir, key_path, name),
        }
    }
}

/// The secret store is a keyed authority, not a general path/etcd namespace.
/// Keep the accepted grammar deliberately narrower than shell or filesystem
/// syntax. `@` is retained for SIP userinfo and `:` for XCP host:port refs.
fn validate_secret_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("secret store: empty secret reference".to_string());
    }
    if name.len() > MAX_SECRET_NAME_BYTES {
        return Err(format!(
            "secret store: secret reference exceeds {MAX_SECRET_NAME_BYTES} bytes"
        ));
    }
    if name.contains("__") {
        return Err(
            "secret store: secret reference cannot contain consecutive underscores".to_string(),
        );
    }
    for (index, segment) in name.split('/').enumerate() {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!(
                "secret store: invalid empty/dot segment at position {index}"
            ));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '@'))
        {
            return Err(format!(
                "secret store: invalid character in secret reference '{name}'"
            ));
        }
    }
    Ok(())
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

/// Refuse symlink traversal for a configured secret-store path. Missing
/// components are allowed because the caller creates the store directory, but
/// every existing component must be a directory (except the final file).
fn ensure_no_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        ) {
            return Err(format!(
                "local secret store: unsafe path component in {}",
                path.display()
            ));
        }
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "local secret store: refusing symlink component {}",
                    current.display()
                ));
            }
            Ok(meta) if current != path && !meta.is_dir() => {
                return Err(format!(
                    "local secret store: non-directory path component {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "local secret store: inspect {}: {e}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn set_mode_exact(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| {
            format!(
                "local secret store: set permissions {mode:o} on {}: {e}",
                path.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// Read a regular file through an `O_NOFOLLOW|O_CLOEXEC` descriptor on Unix.
/// The metadata preflight remains useful for actionable diagnostics, while the
/// descriptor closes the check/read race for replicated or operator-managed
/// secret paths.
fn read_regular_no_follow(path: &Path) -> std::io::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};

        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let file: std::fs::File = fd.into();
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "secret path is not a regular file",
            ));
        }
        read_bounded_ciphertext(file)
    }
    #[cfg(not(unix))]
    {
        read_bounded_ciphertext(std::fs::File::open(path)?)
    }
}

/// Read at most one sentinel byte beyond the local ciphertext ceiling. On
/// Unix this runs on the already-open `O_NOFOLLOW` descriptor; on other
/// platforms it preserves the prior `File::open` symlink behavior while still
/// preventing an unbounded allocation.
fn read_bounded_ciphertext(file: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(MAX_SECRET_CIPHERTEXT_BYTES + 1);
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, (MAX_SECRET_CIPHERTEXT_BYTES + 1) as u64),
        &mut bytes,
    )?;
    if bytes.len() > MAX_SECRET_CIPHERTEXT_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("secret ciphertext exceeds {MAX_SECRET_CIPHERTEXT_BYTES}-byte limit"),
        ));
    }
    Ok(bytes)
}

/// Write an encrypted blob without following a target/temp symlink. The
/// temporary file is created with `O_EXCL`, written and synced with the final
/// mode, then atomically renamed into place; failed writes leave the old
/// ciphertext untouched.
fn write_secret_atomic(path: &Path, body: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "local secret store: secret path {} has no parent",
            path.display()
        )
    })?;
    ensure_no_symlink_components(parent)?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("local secret store: mkdir {}: {e}", parent.display()))?;
    ensure_no_symlink_components(path)?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.is_file() {
            return Err(format!(
                "local secret store: target {} is not a regular file",
                path.display()
            ));
        }
    }

    use rand::RngCore as _;
    use std::io::Write as _;
    let file_name = path.file_name().ok_or_else(|| {
        format!(
            "local secret store: secret path {} has no filename",
            path.display()
        )
    })?;
    let mut temp = None;
    let mut file = None;
    for _ in 0..16 {
        let mut nonce = [0u8; 8];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let candidate = parent.join(format!(
            ".{}.tmp-{:016x}",
            file_name.to_string_lossy(),
            u64::from_be_bytes(nonce)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(opened) => {
                temp = Some(candidate);
                file = Some(opened);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "local secret store: create temporary file in {}: {e}",
                    parent.display()
                ));
            }
        }
    }
    let temp = temp.ok_or_else(|| {
        format!(
            "local secret store: unable to allocate a unique temporary file in {}",
            parent.display()
        )
    })?;
    let mut file = file.expect("temporary path and file are created together");
    let result = (|| {
        set_mode_exact(&temp, 0o600)?;
        file.write_all(body)
            .map_err(|e| format!("local secret store: write {}: {e}", temp.display()))?;
        file.sync_all()
            .map_err(|e| format!("local secret store: sync {}: {e}", temp.display()))?;
        std::fs::rename(&temp, path)
            .map_err(|e| format!("local secret store: rename into {}: {e}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
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
    ensure_no_symlink_components(key_path)?;
    let bytes = read_regular_no_follow(key_path).map_err(|e| {
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
    validate_secret_name(name)?;
    let passphrase = local_passphrase(key_path)?;
    let sealed = crate::ca::backup::seal_bytes(&passphrase, plaintext.as_bytes())
        .map_err(|e| format!("local secret store: seal: {e}"))?;
    ensure_no_symlink_components(dir)?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("local secret store: mkdir {}: {e}", dir.display()))?;
    ensure_no_symlink_components(dir)?;
    set_mode_exact(dir, 0o700)?;
    let path = local_secret_path(dir, name);
    write_secret_atomic(&path, &sealed)
}

/// Read + decrypt the ciphertext file. Missing file → `Ok(None)` (not
/// distributed); a decrypt failure → `Err` (wrong key / tamper).
fn local_get(dir: &Path, key_path: &Path, name: &str) -> Result<Option<String>, String> {
    validate_secret_name(name)?;
    let path = local_secret_path(dir, name);
    ensure_no_symlink_components(dir)?;
    ensure_no_symlink_components(&path)?;
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if !meta.is_file() {
            return Err(format!(
                "local secret store: target {} is not a regular file",
                path.display()
            ));
        }
    }
    let sealed = match read_regular_no_follow(&path) {
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
    fn xcp_creds_ref_namespaces_under_xcp_and_sanitizes() {
        // The dom0 address keys the credential under xcp/<host>.
        assert_eq!(xcp_creds_ref("172.20.0.4"), "xcp/172.20.0.4");
        // A hostname / overlay address with a port is preserved.
        assert_eq!(xcp_creds_ref("dom0.lab.local:22"), "xcp/dom0.lab.local:22");
        // Whitespace is trimmed and stray separators that could widen the key
        // namespace (a `/`, a space) are dropped.
        assert_eq!(xcp_creds_ref("  10.0.0.5 "), "xcp/10.0.0.5");
        assert_eq!(xcp_creds_ref("a/b c"), "xcp/abc");
    }

    #[test]
    fn secret_refs_reject_namespace_escape_and_flat_file_collisions() {
        for invalid in ["", "/vpn", "vpn/", "vpn//key", "vpn/../key", "vpn/key\n"] {
            assert!(
                SecretStore::validate_name(invalid).is_err(),
                "invalid secret ref was accepted: {invalid:?}"
            );
        }
        assert!(SecretStore::validate_name("sip/bob@corp").is_ok());
        assert!(SecretStore::validate_name("xcp/dom0.lab.local:22").is_ok());
        assert!(SecretStore::validate_name("vpn/a__b").is_err());
    }

    #[test]
    fn xcp_credential_round_trips_through_the_store() {
        // XCP-7: a dom0 XAPI/root password stored under xcp/<host> reads back
        // decrypted, byte-for-byte, and is honestly absent before it's stored.
        let (_t, store) = local_store();
        let name = xcp_creds_ref("172.20.0.4");
        assert_eq!(store.get(&name).unwrap(), None, "absent before set");
        let password = "dom0-XAPI-r00t-pw!";
        store.put(&name, password).unwrap();
        assert_eq!(store.get(&name).unwrap().as_deref(), Some(password));
        // A different dom0's slot is independent + still honestly absent.
        assert_eq!(store.get(&xcp_creds_ref("172.20.0.5")).unwrap(), None);
    }

    #[test]
    fn xcp_credential_ciphertext_on_disk_is_not_plaintext() {
        // The dom0 password never appears in the at-rest ciphertext file.
        let (_t, store) = local_store();
        let name = xcp_creds_ref("172.20.0.9");
        let password = "PLAINTEXT-SHOULD-NOT-LEAK";
        store.put(&name, password).unwrap();
        let SecretStore::LocalAead { dir, .. } = &store else {
            unreachable!()
        };
        let raw = std::fs::read(local_secret_path(dir, &name)).unwrap();
        assert!(
            !raw.windows(password.len())
                .any(|w| w == password.as_bytes()),
            "dom0 password leaked into the at-rest secret file"
        );
        assert_eq!(&raw[..4], crate::ca::backup::BUNDLE_MAGIC);
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

    #[cfg(unix)]
    #[test]
    fn local_ciphertext_is_owner_only_and_store_dir_is_private() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_t, store) = local_store();
        let name = creds_ref_for("mvpn-perms");
        store.put(&name, "secret").unwrap();
        let SecretStore::LocalAead { dir, .. } = &store else {
            unreachable!()
        };
        assert_eq!(
            std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(local_secret_path(dir, &name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_put_refuses_a_symlink_target_without_touching_target() {
        use std::os::unix::fs::symlink;

        let (tmp, store) = local_store();
        let SecretStore::LocalAead { dir, .. } = &store else {
            unreachable!()
        };
        std::fs::create_dir_all(dir).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::write(&outside, b"sentinel").unwrap();
        let target = local_secret_path(dir, "vpn/mvpn-symlink");
        symlink(&outside, &target).unwrap();

        let error = store.put("vpn/mvpn-symlink", "must-not-write").unwrap_err();
        assert!(error.contains("symlink"), "unexpected error: {error}");
        assert_eq!(std::fs::read(&outside).unwrap(), b"sentinel");
        assert!(std::fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn local_ciphertext_reader_accepts_exact_limit_and_rejects_one_byte_over() {
        let tmp = tempfile::tempdir().unwrap();
        let exact = tmp.path().join("exact.age");
        let oversized = tmp.path().join("oversized.age");
        std::fs::write(&exact, vec![0xA5; MAX_SECRET_CIPHERTEXT_BYTES]).unwrap();
        std::fs::write(&oversized, vec![0xA5; MAX_SECRET_CIPHERTEXT_BYTES + 1]).unwrap();

        let bytes = read_regular_no_follow(&exact).unwrap();
        assert_eq!(bytes.len(), MAX_SECRET_CIPHERTEXT_BYTES);

        let error = read_regular_no_follow(&oversized).expect_err("oversized ciphertext");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds"));
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
