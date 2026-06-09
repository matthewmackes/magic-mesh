//! EPIC-SEC-PASSCODE-CREDS (Q52) — encrypt the mesh passcode at rest
//! with `systemd-creds`.
//!
//! The shared mesh passcode gates enrollment + service auth. Until
//! now it was stashed in libsecret (gnome-keyring), which needs an
//! unlocked login keyring — awkward for a headless daemon and tied
//! to the desktop session. `systemd-creds` stores it as a host-bound
//! encrypted credential instead: encrypted with the TPM when one is
//! present, falling back to the machine's `/var/lib/systemd/credential.secret`
//! host key otherwise (`--with-key=auto`). Disk-image theft alone
//! can't decrypt it — the TPM/host key stays on the machine.
//!
//! ## Files
//!
//! The ciphertext lives at [`DEFAULT_CRED_PATH`]
//! (`/var/lib/mackesd/mesh-passcode.cred`), 0600. It is **host-bound
//! and NOT mesh-replicated** — each peer encrypts its own copy of
//! the shared passcode under its own key, so the cred file must not
//! live on the GFS mesh-home (a peer can't decrypt another peer's
//! cred).
//!
//! ## Plaintext-never-on-disk invariant
//!
//! [`store`] feeds the plaintext to `systemd-creds encrypt` on
//! **stdin** (`-` input) and captures the ciphertext to the cred
//! file — the plaintext never touches a temp file. [`load`] runs
//! `systemd-creds decrypt` and reads the plaintext from the child's
//! **stdout**. Plaintext exists only in stdin/stdout pipes + process
//! memory.
//!
//! ## Scope (this module)
//!
//! Ships the mechanism + the `mackesd generate-passcode --store` /
//! `rotate-passcode --store` / `show-passcode` CLI surface. Wiring
//! the Birthright wizard to call `--store` (EPIC-SEC-PASSCODE-CREDS.
//! birthright-wire) and the mded enrollment path to call [`load`] at
//! startup (.mded-read) are follow-on tasks.

#![cfg_attr(not(test), allow(dead_code))]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Default ciphertext path — host-local, 0600, NOT mesh-replicated.
pub const DEFAULT_CRED_PATH: &str = "/var/lib/mackesd/mesh-passcode.cred";

/// Credential name `systemd-creds` binds the ciphertext to. Decrypt
/// must pass the same `--name` or systemd-creds refuses — stops a
/// cred copied into a different slot from being decrypted.
pub const CRED_NAME: &str = "mde.mesh.passcode";

/// The system binary we shell out to.
const SYSTEMD_CREDS: &str = "systemd-creds";

/// Errors from the systemd-creds passcode path.
#[derive(Debug)]
pub enum PasscodeCredsError {
    /// `systemd-creds` isn't on PATH (not a systemd host, or the
    /// package providing it isn't installed).
    BinaryMissing,
    /// The encrypt/decrypt subprocess exited non-zero. Carries the
    /// captured stderr for the operator.
    CommandFailed {
        /// Which systemd-creds verb failed (`encrypt` / `decrypt`).
        verb: &'static str,
        /// Captured stderr from the failed invocation.
        stderr: String,
    },
    /// Filesystem or pipe I/O failure.
    Io(String),
    /// Decrypted bytes weren't valid UTF-8 (corrupt cred / wrong
    /// name).
    NotUtf8,
}

impl std::fmt::Display for PasscodeCredsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BinaryMissing => write!(
                f,
                "systemd-creds not on PATH — needs a systemd host with \
                 the systemd package providing `systemd-creds`",
            ),
            Self::CommandFailed { verb, stderr } => {
                write!(f, "systemd-creds {verb} failed: {stderr}")
            }
            Self::Io(e) => write!(f, "passcode-creds I/O: {e}"),
            Self::NotUtf8 => write!(
                f,
                "decrypted passcode wasn't valid UTF-8 — wrong --name or \
                 a corrupt cred file?",
            ),
        }
    }
}

impl std::error::Error for PasscodeCredsError {}

/// Build the `systemd-creds encrypt` argv. Plaintext arrives on
/// stdin (`-`); ciphertext is written to `output`. `--with-key=auto`
/// picks the TPM when present, else the host key.
#[must_use]
pub fn encrypt_argv(name: &str, output: &Path) -> Vec<String> {
    vec![
        "encrypt".to_string(),
        format!("--name={name}"),
        "--with-key=auto".to_string(),
        "-".to_string(),
        output.display().to_string(),
    ]
}

/// Build the `systemd-creds decrypt` argv. Reads the cred at `input`,
/// writes plaintext to stdout (`-`). The `--name` must match what
/// [`encrypt_argv`] used.
#[must_use]
pub fn decrypt_argv(name: &str, input: &Path) -> Vec<String> {
    vec![
        "decrypt".to_string(),
        format!("--name={name}"),
        input.display().to_string(),
        "-".to_string(),
    ]
}

/// Map a subprocess-spawn error to either [`PasscodeCredsError::BinaryMissing`]
/// (ENOENT) or an I/O error.
fn spawn_err(verb: &'static str, e: &std::io::Error) -> PasscodeCredsError {
    if e.kind() == std::io::ErrorKind::NotFound {
        PasscodeCredsError::BinaryMissing
    } else {
        PasscodeCredsError::Io(format!("{verb}: {e}"))
    }
}

/// Encrypt `plaintext` to `cred_path` under `name` via
/// `systemd-creds encrypt`. Feeds the plaintext on stdin (never a
/// temp file) and chmods the result 0600. Creates the parent dir if
/// needed.
///
/// # Errors
/// [`PasscodeCredsError::BinaryMissing`] when systemd-creds is
/// absent; [`PasscodeCredsError::CommandFailed`] on a non-zero exit;
/// [`PasscodeCredsError::Io`] on pipe/fs failure.
pub fn store(plaintext: &str, cred_path: &Path, name: &str) -> Result<(), PasscodeCredsError> {
    if let Some(parent) = cred_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PasscodeCredsError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    let mut child = Command::new(SYSTEMD_CREDS)
        .args(encrypt_argv(name, cred_path))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_err("encrypt", &e))?;
    // Write the plaintext to the child's stdin, then drop it to
    // signal EOF so systemd-creds finishes reading.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| PasscodeCredsError::Io("encrypt: no stdin pipe".to_string()))?;
        stdin
            .write_all(plaintext.as_bytes())
            .map_err(|e| PasscodeCredsError::Io(format!("encrypt stdin: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| PasscodeCredsError::Io(format!("encrypt wait: {e}")))?;
    if !out.status.success() {
        return Err(PasscodeCredsError::CommandFailed {
            verb: "encrypt",
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    // Lock the ciphertext down to owner-only.
    set_owner_only(cred_path)?;
    Ok(())
}

/// Decrypt the cred at `cred_path` under `name` via
/// `systemd-creds decrypt`, returning the plaintext passcode.
///
/// # Errors
/// [`PasscodeCredsError::BinaryMissing`] when systemd-creds is
/// absent; [`PasscodeCredsError::CommandFailed`] on a non-zero exit
/// (missing cred, wrong key, name mismatch);
/// [`PasscodeCredsError::NotUtf8`] on a corrupt cred.
pub fn load(cred_path: &Path, name: &str) -> Result<String, PasscodeCredsError> {
    let out = Command::new(SYSTEMD_CREDS)
        .args(decrypt_argv(name, cred_path))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| spawn_err("decrypt", &e))?;
    if !out.status.success() {
        return Err(PasscodeCredsError::CommandFailed {
            verb: "decrypt",
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    // systemd-creds emits the raw plaintext with no trailing newline;
    // trim a stray one defensively in case a future version adds it.
    let text = String::from_utf8(out.stdout)
        .map_err(|_| PasscodeCredsError::NotUtf8)?
        .trim_end_matches('\n')
        .to_string();
    Ok(text)
}

/// Default cred path as a [`PathBuf`].
#[must_use]
pub fn default_cred_path() -> PathBuf {
    PathBuf::from(DEFAULT_CRED_PATH)
}

/// chmod `path` to 0600 (owner read/write only). No-op on non-Unix.
fn set_owner_only(path: &Path) -> Result<(), PasscodeCredsError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| PasscodeCredsError::Io(format!("chmod 600 {}: {e}", path.display())))?;
    }
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_argv_feeds_stdin_and_names_the_cred() {
        let argv = encrypt_argv("mde.mesh.passcode", Path::new("/var/lib/mackesd/x.cred"));
        assert_eq!(argv[0], "encrypt");
        assert!(argv.contains(&"--name=mde.mesh.passcode".to_string()));
        assert!(argv.contains(&"--with-key=auto".to_string()));
        // stdin input marker comes before the output path.
        let dash = argv.iter().position(|a| a == "-").unwrap();
        let out = argv
            .iter()
            .position(|a| a == "/var/lib/mackesd/x.cred")
            .unwrap();
        assert!(dash < out, "stdin `-` must precede the output path");
    }

    #[test]
    fn decrypt_argv_reads_cred_writes_stdout() {
        let argv = decrypt_argv("mde.mesh.passcode", Path::new("/var/lib/mackesd/x.cred"));
        assert_eq!(argv[0], "decrypt");
        assert!(argv.contains(&"--name=mde.mesh.passcode".to_string()));
        // input path precedes the stdout `-`.
        let inp = argv
            .iter()
            .position(|a| a == "/var/lib/mackesd/x.cred")
            .unwrap();
        let dash = argv.iter().rposition(|a| a == "-").unwrap();
        assert!(inp < dash, "cred input must precede the stdout `-`");
    }

    #[test]
    fn default_cred_path_is_host_local_not_mesh() {
        let p = default_cred_path();
        assert_eq!(p, PathBuf::from("/var/lib/mackesd/mesh-passcode.cred"));
        // Must NOT be under a mesh-home / QNM-Shared path — the cred
        // is host-bound + can't decrypt on another peer.
        let s = p.display().to_string();
        assert!(!s.contains("QNM-Shared"));
        assert!(!s.contains(".mde-mesh"));
    }

    #[test]
    fn store_surfaces_binary_missing_when_systemd_creds_absent() {
        // Point PATH at an empty dir so the spawn fails ENOENT, then
        // assert we map it to BinaryMissing rather than a raw IO error.
        // We can't safely mutate PATH process-wide in a parallel test
        // runner, so instead exercise spawn_err directly — the same
        // mapping store()/load() use.
        let enoent = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(matches!(
            spawn_err("encrypt", &enoent),
            PasscodeCredsError::BinaryMissing
        ));
        let other = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(
            spawn_err("encrypt", &other),
            PasscodeCredsError::Io(_)
        ));
    }

    #[test]
    fn error_messages_are_actionable() {
        assert!(PasscodeCredsError::BinaryMissing
            .to_string()
            .contains("systemd-creds"));
        assert!(PasscodeCredsError::NotUtf8.to_string().contains("name"));
        let cf = PasscodeCredsError::CommandFailed {
            verb: "decrypt",
            stderr: "no such credential".to_string(),
        };
        assert!(cf.to_string().contains("decrypt"));
        assert!(cf.to_string().contains("no such credential"));
    }

    // A real encrypt→decrypt round-trip needs `systemd-creds` + a
    // host key (often root). Gated behind --ignored so the unit
    // suite stays hermetic; run on a real host with:
    //   cargo test -p mackesd --features async-services \
    //     passcode_creds::tests::round_trip -- --ignored
    #[test]
    #[ignore = "needs systemd-creds + host key (run with --ignored on a real host)"]
    fn round_trip_on_real_host() {
        let tmp = tempfile::tempdir().unwrap();
        let cred = tmp.path().join("rt.cred");
        let secret = "Abc-123_XYZabc01";
        store(secret, &cred, CRED_NAME).expect("store");
        assert!(cred.exists());
        let back = load(&cred, CRED_NAME).expect("load");
        assert_eq!(back, secret);
    }
}
