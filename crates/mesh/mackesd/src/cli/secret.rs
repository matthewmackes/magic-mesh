//! Secret CLI verb handlers (`secret`, `secret-seal`, `secret-unseal`).
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved.
use crate::*;

/// Keep operator-provided secret material bounded before it reaches the store
/// or the Argon2id envelope. The armored form has room for base64 expansion and
/// framing around the bounded plaintext form.
const MAX_SECRET_PLAINTEXT_BYTES: usize = 1024 * 1024;
const MAX_SECRET_ARMORED_BYTES: usize = 2 * 1024 * 1024;
const MAX_PASSPHRASE_FILE_BYTES: usize = 64 * 1024;

fn read_bounded_input(
    reader: impl std::io::Read,
    max_bytes: usize,
    label: &str,
) -> anyhow::Result<Vec<u8>> {
    use std::io::Read as _;

    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    reader
        .take(
            u64::try_from(max_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {label}"))?;
    if bytes.len() > max_bytes {
        anyhow::bail!("{label} exceeds {max_bytes}-byte limit");
    }
    Ok(bytes)
}

/// DATACENTER-3 — seal/read a leader-managed mesh secret from the CLI. `put` reads
/// plaintext from stdin and age-encrypts it; `get` decrypts to stdout (exit 3 if
/// absent). `--local` forces the Syncthing-replicated LocalAead store so a repo
/// node can seal a secret the lighthouses then read via their own LocalAead store
/// (keyed by the shared mesh age identity) — the operational put-path the readers
/// (`media_registry`, VPN, DR) always assumed but no CLI exposed.
pub fn run(cmd: SecretCmd) -> anyhow::Result<()> {
    use mackesd_core::ipc::secret_store::{age_key_path, repo_root, SecretStore};
    let workgroup_root = mackesd_core::default_qnm_shared_root();
    let store_for = |local: bool| -> SecretStore {
        if local {
            SecretStore::LocalAead {
                dir: workgroup_root.join("vpn").join("secrets"),
                key_path: age_key_path(),
            }
        } else {
            SecretStore::resolve(&repo_root(), &workgroup_root)
        }
    };
    match cmd {
        SecretCmd::Put { name, local } => {
            let store = store_for(local);
            mackesd_core::ipc::secret_store::SecretStore::validate_name(&name)
                .map_err(|e| anyhow::anyhow!(e))?;
            let plaintext = String::from_utf8(read_bounded_input(
                std::io::stdin().lock(),
                MAX_SECRET_PLAINTEXT_BYTES,
                "secret plaintext from stdin",
            )?)
            .context("secret plaintext from stdin is not UTF-8")?;
            store
                .put(&name, &plaintext)
                .map_err(|e| anyhow::anyhow!(e))?;
            eprintln!(
                "mackesd secret: sealed '{name}' ({} bytes){}",
                plaintext.len(),
                if local {
                    " into the Syncthing-replicated LocalAead store"
                } else {
                    ""
                }
            );
        }
        SecretCmd::Get { name, local } => match store_for(local)
            .get(&name)
            .map_err(|e| anyhow::anyhow!(e))?
        {
            Some(v) => print!("{v}"),
            None => {
                eprintln!("mackesd secret: '{name}' is not in the store");
                std::process::exit(3);
            }
        },
    }
    Ok(())
}

/// DAR-2 — read a single-line passphrase from `path` for `secret-seal`/`-unseal`.
///
/// The passphrase is sourced from a FILE (not argv/env) so it never appears in
/// `ps`, `/proc/<pid>/cmdline`, or an inherited environment. The first line is
/// used with any trailing `\r`/`\n` stripped — so an operator can write the
/// phrase with a plain `echo > file` without a stray newline becoming part of
/// the secret. An empty passphrase is rejected here (the envelope rejects it
/// too, but failing early gives an operator-actionable message). The phrase is
/// NEVER logged — only its presence/length feeds the error path.
fn read_passphrase_file(path: &std::path::Path) -> anyhow::Result<String> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting passphrase file {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "passphrase file {} is a symlink; use a root-owned regular file",
            path.display()
        );
    }
    if !metadata.is_file() {
        anyhow::bail!("passphrase path {} is not a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 {
            anyhow::bail!(
                "passphrase file {} is group/world accessible (mode {:o}); use 0600 or stricter",
                path.display(),
                mode & 0o777
            );
        }
    }
    #[cfg(unix)]
    let mut file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .with_context(|| format!("opening passphrase file {}", path.display()))?
        .into()
    };
    #[cfg(not(unix))]
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening passphrase file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let opened = file
            .metadata()
            .with_context(|| format!("inspecting opened passphrase file {}", path.display()))?;
        if !opened.is_file() {
            anyhow::bail!("passphrase path {} is not a regular file", path.display());
        }
        let mode = opened.permissions().mode();
        if mode & 0o077 != 0 {
            anyhow::bail!(
                "passphrase file {} became group/world accessible (mode {:o})",
                path.display(),
                mode & 0o777
            );
        }
    }
    let raw = String::from_utf8(read_bounded_input(
        &mut file,
        MAX_PASSPHRASE_FILE_BYTES,
        &format!("passphrase file {}", path.display()),
    )?)
    .with_context(|| format!("passphrase file {} is not UTF-8", path.display()))?;
    // Take the first line; strip a single trailing CR/LF pair, not interior bytes.
    let phrase = raw.lines().next().unwrap_or("").to_string();
    if phrase.is_empty() {
        anyhow::bail!(
            "passphrase file {} is empty (first line blank) — write the passphrase to it 0600",
            path.display()
        );
    }
    Ok(phrase)
}

/// DAR-2 — `mackesd secret-seal --passphrase-file <f>`: read arbitrary bytes
/// from stdin, seal them under the canonical `ca::backup` envelope, and write
/// the ASCII-armored bundle to stdout.
///
/// This reuses the ONE audited Argon2id + XChaCha20-Poly1305 path
/// (`ca::backup::seal_bytes` + `armor`) rather than re-rolling crypto. It is the
/// thin CLI the DR CA/identity bundle (DAR-42) uses — explicitly NOT the
/// control-VM bootstrap, which mints its own age key and is granted read by
/// re-seal (no passphrase in tofu state).
///
/// The plaintext is held only in-process and never logged; only its byte length
/// is reported on stderr.
pub fn seal(passphrase_file: &std::path::Path) -> anyhow::Result<()> {
    let passphrase = read_passphrase_file(passphrase_file)?;
    let plaintext = read_bounded_input(
        std::io::stdin().lock(),
        MAX_SECRET_PLAINTEXT_BYTES,
        "secret-seal plaintext",
    )?;
    if plaintext.is_empty() {
        anyhow::bail!("secret-seal: stdin was empty — nothing to seal");
    }
    let sealed = mackesd_core::ca::backup::seal_bytes(&passphrase, &plaintext)
        .map_err(|e| anyhow::anyhow!("secret-seal: {e}"))?;
    let exported_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let armored = mackesd_core::ca::backup::armor(&sealed, exported_at);
    print!("{armored}");
    eprintln!(
        "mackesd secret-seal: sealed {} byte(s) under the Argon2id+XChaCha20 envelope",
        plaintext.len()
    );
    Ok(())
}

/// DAR-2 — `mackesd secret-unseal --passphrase-file <f>`: inverse of
/// `secret-seal`. Reads the armored bundle from stdin, de-armors + unseals, and
/// writes the exact original plaintext bytes to stdout. A wrong/empty
/// passphrase or a tampered bundle surfaces as the existing AEAD error and emits
/// NO plaintext.
pub fn unseal(passphrase_file: &std::path::Path) -> anyhow::Result<()> {
    use std::io::Write as _;
    let passphrase = read_passphrase_file(passphrase_file)?;
    let armored = String::from_utf8(read_bounded_input(
        std::io::stdin().lock(),
        MAX_SECRET_ARMORED_BYTES,
        "secret-unseal armored bundle",
    )?)
    .context("secret-unseal armored bundle from stdin is not UTF-8")?;
    let binary = mackesd_core::ca::backup::dearmor(&armored)
        .map_err(|e| anyhow::anyhow!("secret-unseal: {e}"))?;
    let plain = mackesd_core::ca::backup::unseal_bytes(&passphrase, &binary)
        .map_err(|e| anyhow::anyhow!("secret-unseal: {e}"))?;
    std::io::stdout()
        .write_all(&plain)
        .context("writing unsealed plaintext to stdout")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{read_bounded_input, read_passphrase_file, MAX_SECRET_PLAINTEXT_BYTES};

    #[test]
    fn stdin_reader_accepts_exact_limit_and_rejects_overflow() {
        let exact = vec![b'x'; MAX_SECRET_PLAINTEXT_BYTES];
        assert_eq!(
            read_bounded_input(
                std::io::Cursor::new(exact),
                MAX_SECRET_PLAINTEXT_BYTES,
                "secret",
            )
            .unwrap()
            .len(),
            MAX_SECRET_PLAINTEXT_BYTES
        );

        let oversized = vec![b'x'; MAX_SECRET_PLAINTEXT_BYTES + 1];
        let error = read_bounded_input(
            std::io::Cursor::new(oversized),
            MAX_SECRET_PLAINTEXT_BYTES,
            "secret",
        )
        .expect_err("oversized stdin must fail closed");
        assert!(error.to_string().contains("exceeds"));
    }

    #[cfg(unix)]
    #[test]
    fn passphrase_reader_rejects_symlinks_and_loose_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let tmp = tempfile::tempdir().unwrap();
        let passphrase = tmp.path().join("passphrase");
        std::fs::write(&passphrase, "correct horse battery staple\n").unwrap();
        std::fs::set_permissions(&passphrase, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_passphrase_file(&passphrase).is_err());

        std::fs::set_permissions(&passphrase, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            read_passphrase_file(&passphrase).unwrap(),
            "correct horse battery staple"
        );

        let link = tmp.path().join("passphrase-link");
        symlink(&passphrase, &link).unwrap();
        assert!(read_passphrase_file(&link).is_err());
    }
}
