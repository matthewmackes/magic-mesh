//! Secret CLI verb handlers (`secret`, `secret-seal`, `secret-unseal`).
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved.
use crate::*;

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
            let mut plaintext = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut plaintext)
                .context("reading secret plaintext from stdin")?;
            store_for(local)
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
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading passphrase file {}", path.display()))?;
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
    use std::io::Read as _;
    let passphrase = read_passphrase_file(passphrase_file)?;
    let mut plaintext = Vec::new();
    std::io::stdin()
        .read_to_end(&mut plaintext)
        .context("reading plaintext bytes from stdin")?;
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
    use std::io::{Read as _, Write as _};
    let passphrase = read_passphrase_file(passphrase_file)?;
    let mut armored = String::new();
    std::io::stdin()
        .read_to_string(&mut armored)
        .context("reading armored bundle from stdin")?;
    let binary = mackesd_core::ca::backup::dearmor(&armored)
        .map_err(|e| anyhow::anyhow!("secret-unseal: {e}"))?;
    let plain = mackesd_core::ca::backup::unseal_bytes(&passphrase, &binary)
        .map_err(|e| anyhow::anyhow!("secret-unseal: {e}"))?;
    std::io::stdout()
        .write_all(&plain)
        .context("writing unsealed plaintext to stdout")?;
    Ok(())
}
