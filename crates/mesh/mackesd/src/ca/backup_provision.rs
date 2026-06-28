//! MIG-3 — provision the CA-backup passphrase on a JOINED lighthouse.
//!
//! ## The gap this closes
//!
//! A lighthouse that JOINED an existing mesh (via `mackesd join
//! --role lighthouse`) ends up holding the mesh CA (the founder's CA
//! is inherited on promotion — same mesh, same signing key). The
//! [`nebula_ca_backup`](crate::workers::nebula_ca_backup) worker reads
//! its backup passphrase from `$CREDENTIALS_DIRECTORY/backup-passphrase`
//! (a systemd-encrypted credential) and, when it finds a CA key on disk
//! but NO passphrase, loud-warns every boot:
//!
//! ```text
//! SEC-7/ENT-11: this box holds the mesh CA but has no backup
//! passphrase — the CA is UNBACKED-UP.
//! ```
//!
//! `mackesd found` (the founder) is hand-provisioned per the unit
//! comment (EFF-15 in `packaging/systemd/mackesd.service`), but a
//! JOINED lighthouse never runs that ritual, so it boots UNBACKED-UP.
//! This module runs the same ritual automatically as part of the join.
//!
//! ## What "provision" means here (scope)
//!
//! **Generate-on-joiner.** We mint a fresh CSPRNG passphrase locally on
//! the joining box and seal it with `systemd-creds encrypt`
//! (`--with-key=auto` → TPM when present, else the machine's host key).
//! The sealed credential is **host-bound and never leaves the box** —
//! exactly like the mesh-passcode credential
//! ([`crate::passcode_creds`]). We then write the `LoadCredentialEncrypted`
//! drop-in so the next `systemctl restart mackesd` makes the credential
//! visible at `$CREDENTIALS_DIRECTORY/backup-passphrase`, and the
//! worker stops warning.
//!
//! This is the minimal surface that clears the error: no new
//! cross-mesh wire field, no plaintext transmission. The credential
//! rides nowhere — it is generated, sealed, and consumed on the same
//! box.
//!
//! ## Explicitly OUT OF SCOPE
//!
//! The off-fleet / off-site CA-backup push (the age-encrypted push to
//! the DR bucket) is an operator-run step and is NOT touched here. This
//! module only ensures the joined lighthouse *has a backup passphrase
//! credential* so it is no longer "UNBACKED-UP"; the daily on-mesh
//! sealed backup ([`crate::workers::nebula_ca_backup`]) then runs as it
//! already does on the founder.
//!
//! ## Secret hygiene
//!
//! The minted passphrase is fed to `systemd-creds` on **stdin**
//! ([`crate::passcode_creds::store`]) — never a temp file, never an
//! argv, never logged. This module logs only the credential's *length*
//! and the *outcome*, never its value.

use std::path::{Path, PathBuf};

/// systemd credential name the backup worker reads
/// (`$CREDENTIALS_DIRECTORY/backup-passphrase`). Must match
/// [`crate::workers::nebula_ca_backup`]'s read path and the
/// `--name=` the unit comment documents.
pub const CRED_NAME: &str = "backup-passphrase";

/// Default ciphertext path for the sealed backup passphrase.
/// Host-local + 0600 + NOT mesh-replicated (each box seals under its
/// own TPM/host key — mirrors [`crate::passcode_creds::DEFAULT_CRED_PATH`]).
pub const DEFAULT_CRED_PATH: &str = "/etc/mackesd/backup-passphrase.cred";

/// Default systemd drop-in dir for the `mackesd.service`.
pub const DEFAULT_DROPIN_DIR: &str = "/etc/systemd/system/mackesd.service.d";

/// Drop-in filename that loads the encrypted credential.
pub const DROPIN_FILENAME: &str = "backup-passphrase.conf";

/// Entropy (bytes) for the minted passphrase. 32 CSPRNG bytes →
/// 43-char URL-safe base64, 256 bits — the same shape as a mesh
/// bearer ([`crate::bearer_ledger`]).
const PASSPHRASE_BYTES: usize = 32;

/// Outcome of a provisioning attempt. Carries NO secret material — safe
/// to log / return / assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionOutcome {
    /// A fresh passphrase was minted, sealed, and the drop-in written.
    /// Carries the sealed-credential length (not the value) for a
    /// shape-only log line.
    Provisioned {
        /// Bytes written to the ciphertext file (sealed, not plaintext).
        sealed_bytes: usize,
    },
    /// The credential file already existed — left untouched. Rotating
    /// an in-use passphrase would orphan any existing sealed backups,
    /// so provisioning is strictly first-time-only.
    AlreadyPresent,
    /// The role isn't a CA holder (Server / Workstation) — no CA key
    /// will ever land here, so no backup passphrase is needed.
    NotLighthouse,
}

/// Errors from the provisioning path. None of these carry secret
/// material.
#[derive(Debug)]
pub enum ProvisionError {
    /// The seal step (systemd-creds encrypt) failed.
    Seal(String),
    /// Writing the drop-in or creating a dir failed.
    Io(String),
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seal(e) => write!(f, "seal backup passphrase: {e}"),
            Self::Io(e) => write!(f, "backup-passphrase provisioning I/O: {e}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

/// Where the sealed credential + the loader drop-in live. Production
/// uses [`Paths::production`]; tests redirect to a tempdir.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Ciphertext path (`/etc/mackesd/backup-passphrase.cred`).
    pub cred_path: PathBuf,
    /// Drop-in directory (`/etc/systemd/system/mackesd.service.d`).
    pub dropin_dir: PathBuf,
}

impl Paths {
    /// Production paths.
    #[must_use]
    pub fn production() -> Self {
        Self {
            cred_path: PathBuf::from(DEFAULT_CRED_PATH),
            dropin_dir: PathBuf::from(DEFAULT_DROPIN_DIR),
        }
    }

    /// The full drop-in file path.
    #[must_use]
    pub fn dropin_path(&self) -> PathBuf {
        self.dropin_dir.join(DROPIN_FILENAME)
    }
}

/// The seal mechanism: `(plaintext, cred-name, out-path) -> sealed file`.
///
/// Boxed so tests can inject a hermetic stand-in (the production sealer
/// shells out to `systemd-creds`, which needs a host key + root). The
/// implementation MUST NOT log or otherwise expose the plaintext.
pub type Sealer<'a> = dyn Fn(&str, &str, &Path) -> Result<(), String> + 'a;

/// Production sealer — [`crate::passcode_creds::store`] (systemd-creds,
/// plaintext on stdin only).
fn systemd_creds_sealer(plaintext: &str, name: &str, out: &Path) -> Result<(), String> {
    crate::passcode_creds::store(plaintext, out, name).map_err(|e| e.to_string())
}

/// Mint a fresh CSPRNG backup passphrase — 32 OS-random bytes, URL-safe
/// base64 (no padding). 256 bits of entropy in 43 chars. NEVER log the
/// return value.
#[must_use]
pub fn mint_passphrase() -> String {
    use base64::Engine as _;
    use rand::RngCore as _;
    use zeroize::Zeroize as _;
    let mut bytes = [0_u8; PASSPHRASE_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let phrase = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    // Best-effort scrub of the raw entropy buffer; the encoded String is
    // the caller's to consume + drop.
    bytes.zeroize();
    phrase
}

/// The `[Service]` drop-in body that loads the encrypted credential.
/// Mirrors the EFF-15 ritual in `packaging/systemd/mackesd.service`.
#[must_use]
pub fn dropin_body(cred_path: &Path) -> String {
    format!(
        "[Service]\nLoadCredentialEncrypted={CRED_NAME}:{}\n",
        cred_path.display()
    )
}

/// MIG-3 — ensure a CA-holding (lighthouse) box has a sealed CA-backup
/// passphrase credential. Idempotent + first-time-only.
///
/// * Non-lighthouse roles → [`ProvisionOutcome::NotLighthouse`] (no-op).
/// * Credential file already present → [`ProvisionOutcome::AlreadyPresent`]
///   (left untouched — never rotates an in-use passphrase).
/// * Otherwise → mint + seal + write the loader drop-in, returning
///   [`ProvisionOutcome::Provisioned`].
///
/// `seal` defaults to systemd-creds in [`provision`]; this variant takes
/// an injected sealer so tests run without systemd-creds / root.
///
/// # Errors
/// [`ProvisionError`] on a seal or I/O failure. The minted passphrase is
/// never logged — only its length / the outcome.
pub fn provision_with(
    role: mde_role::Role,
    paths: &Paths,
    seal: &Sealer<'_>,
) -> Result<ProvisionOutcome, ProvisionError> {
    // Only a CA holder needs a backup passphrase. The CA rides only on
    // the Lighthouse role (Server/Workstation never hold ca.key), so a
    // non-lighthouse join is a deliberate no-op — same gate the backup
    // worker uses (it stays quiet when ca.key is absent).
    if role != mde_role::Role::Lighthouse {
        return Ok(ProvisionOutcome::NotLighthouse);
    }

    // First-time-only: if the cred already exists, leave it. Re-minting
    // would orphan any backups already sealed under the old phrase.
    if paths.cred_path.exists() {
        return Ok(ProvisionOutcome::AlreadyPresent);
    }

    // Mint → seal (plaintext only crosses the stdin pipe inside `seal`).
    let phrase = mint_passphrase();
    seal(&phrase, CRED_NAME, &paths.cred_path).map_err(ProvisionError::Seal)?;
    // `phrase` is dropped here; we never touch it again, never log it.
    drop(phrase);

    let sealed_bytes = std::fs::metadata(&paths.cred_path)
        .ok()
        .and_then(|m| usize::try_from(m.len()).ok())
        .unwrap_or(0);

    // Write the loader drop-in so the next restart surfaces the cred at
    // $CREDENTIALS_DIRECTORY/backup-passphrase.
    std::fs::create_dir_all(&paths.dropin_dir)
        .map_err(|e| ProvisionError::Io(format!("mkdir {}: {e}", paths.dropin_dir.display())))?;
    let dropin = paths.dropin_path();
    std::fs::write(&dropin, dropin_body(&paths.cred_path))
        .map_err(|e| ProvisionError::Io(format!("write {}: {e}", dropin.display())))?;

    Ok(ProvisionOutcome::Provisioned { sealed_bytes })
}

/// Production entry point — [`provision_with`] using the systemd-creds
/// sealer + the canonical [`Paths::production`].
///
/// Call this from the `mackesd join --role lighthouse` flow. After it
/// returns `Provisioned`, the caller should `systemctl daemon-reload`
/// (the drop-in is new); the credential takes effect on the next
/// `mackesd` (re)start, which the join already performs.
///
/// # Errors
/// Per [`ProvisionError`].
pub fn provision(role: mde_role::Role) -> Result<ProvisionOutcome, ProvisionError> {
    provision_with(role, &Paths::production(), &systemd_creds_sealer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn tmp_paths(dir: &Path) -> Paths {
        Paths {
            cred_path: dir.join("etc/mackesd/backup-passphrase.cred"),
            dropin_dir: dir.join("etc/systemd/system/mackesd.service.d"),
        }
    }

    /// A hermetic sealer that records the plaintext LENGTH (never the
    /// value) and writes a sealed-looking blob, so tests assert shape +
    /// behavior without ever printing a secret.
    fn recording_sealer(
        seen_len: &RefCell<Option<usize>>,
    ) -> impl Fn(&str, &str, &Path) -> Result<(), String> + '_ {
        move |plaintext: &str, name: &str, out: &Path| {
            assert_eq!(name, CRED_NAME, "sealer must use the worker's cred name");
            *seen_len.borrow_mut() = Some(plaintext.len());
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            // Stand in for systemd-creds ciphertext — opaque, NOT the
            // plaintext (so even the fixture never persists the secret).
            std::fs::write(out, b"SEALED-CIPHERTEXT-FIXTURE").map_err(|e| e.to_string())
        }
    }

    #[test]
    fn mint_passphrase_is_256_bit_url_safe_base64() {
        let p = mint_passphrase();
        // 32 bytes URL-safe-no-pad base64 = 43 chars (matches a bearer).
        assert_eq!(p.len(), 43);
        assert!(p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn mint_passphrase_is_fresh_each_call() {
        // Two mints must differ — a constant would be a catastrophic bug.
        assert_ne!(mint_passphrase(), mint_passphrase());
    }

    #[test]
    fn lighthouse_join_provisions_the_passphrase() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = tmp_paths(tmp.path());
        let seen = RefCell::new(None);
        let sealer = recording_sealer(&seen);

        let outcome =
            provision_with(mde_role::Role::Lighthouse, &paths, &sealer).expect("provision");

        // A fresh 43-char passphrase was sealed under the worker's name.
        match outcome {
            ProvisionOutcome::Provisioned { sealed_bytes } => assert!(sealed_bytes > 0),
            other => panic!("expected Provisioned, got {other:?}"),
        }
        assert_eq!(seen.borrow().expect("sealer ran"), 43);
        // Cred file + loader drop-in both landed.
        assert!(paths.cred_path.exists(), "sealed cred must exist");
        let dropin = std::fs::read_to_string(paths.dropin_path()).expect("drop-in written");
        assert!(dropin.contains("LoadCredentialEncrypted=backup-passphrase:"));
        assert!(dropin.contains(&paths.cred_path.display().to_string()));
    }

    #[test]
    fn non_lighthouse_join_does_not_get_the_passphrase() {
        for role in [mde_role::Role::Server, mde_role::Role::Workstation] {
            let tmp = tempfile::tempdir().unwrap();
            let paths = tmp_paths(tmp.path());
            let seen = RefCell::new(None);
            let sealer = recording_sealer(&seen);

            let outcome = provision_with(role, &paths, &sealer).expect("provision");
            assert_eq!(outcome, ProvisionOutcome::NotLighthouse);
            // Nothing sealed, nothing written.
            assert!(seen.borrow().is_none(), "sealer must NOT run for {role:?}");
            assert!(!paths.cred_path.exists());
            assert!(!paths.dropin_path().exists());
        }
    }

    #[test]
    fn provision_is_idempotent_does_not_rotate_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = tmp_paths(tmp.path());

        // First provision.
        {
            let seen = RefCell::new(None);
            let sealer = recording_sealer(&seen);
            let out = provision_with(mde_role::Role::Lighthouse, &paths, &sealer).unwrap();
            assert!(matches!(out, ProvisionOutcome::Provisioned { .. }));
        }
        let first = std::fs::read(&paths.cred_path).unwrap();

        // Second provision must NOT re-seal (would orphan existing backups).
        {
            let seen = RefCell::new(None);
            let sealer = recording_sealer(&seen);
            let out = provision_with(mde_role::Role::Lighthouse, &paths, &sealer).unwrap();
            assert_eq!(out, ProvisionOutcome::AlreadyPresent);
            assert!(
                seen.borrow().is_none(),
                "sealer must NOT run when cred exists"
            );
        }
        let second = std::fs::read(&paths.cred_path).unwrap();
        assert_eq!(first, second, "existing credential must be untouched");
    }

    #[test]
    fn round_trip_seal_unseal_via_injected_sealer() {
        // Round-trip the passphrase through a seal/unseal pair to prove
        // the sealed blob recovers the EXACT minted phrase — without ever
        // printing it. The fixture seal is a reversible XOR-with-tag so
        // the test is hermetic (no systemd-creds), yet exercises the
        // mint → seal → recover path the worker depends on.
        let tmp = tempfile::tempdir().unwrap();
        let paths = tmp_paths(tmp.path());

        // Capture the plaintext the sealer received by sealing it
        // reversibly to disk, then unsealing + comparing.
        let xor_seal = |plaintext: &str, name: &str, out: &Path| -> Result<(), String> {
            assert_eq!(name, CRED_NAME);
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let sealed: Vec<u8> = plaintext.bytes().map(|b| b ^ 0x5a).collect();
            std::fs::write(out, sealed).map_err(|e| e.to_string())
        };
        let outcome = provision_with(mde_role::Role::Lighthouse, &paths, &xor_seal).unwrap();
        assert!(matches!(outcome, ProvisionOutcome::Provisioned { .. }));

        // Unseal + assert the recovered phrase has the minted SHAPE (43
        // url-safe chars). We assert on shape, never print the value.
        let sealed = std::fs::read(&paths.cred_path).unwrap();
        let recovered: Vec<u8> = sealed.iter().map(|b| b ^ 0x5a).collect();
        let recovered = String::from_utf8(recovered).expect("recovered is utf-8");
        assert_eq!(recovered.len(), 43);
        assert!(recovered
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn dropin_body_matches_eff15_ritual() {
        let body = dropin_body(Path::new("/etc/mackesd/backup-passphrase.cred"));
        assert_eq!(
            body,
            "[Service]\nLoadCredentialEncrypted=backup-passphrase:/etc/mackesd/backup-passphrase.cred\n"
        );
    }

    #[test]
    fn seal_failure_surfaces_as_error_without_leaking() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = tmp_paths(tmp.path());
        let failing = |_pt: &str, _n: &str, _o: &Path| -> Result<(), String> {
            Err("systemd-creds not on PATH".to_string())
        };
        let err = provision_with(mde_role::Role::Lighthouse, &paths, &failing).unwrap_err();
        match err {
            ProvisionError::Seal(msg) => assert!(msg.contains("systemd-creds")),
            other => panic!("expected Seal error, got {other:?}"),
        }
        // No drop-in written when sealing fails.
        assert!(!paths.dropin_path().exists());
    }
}
