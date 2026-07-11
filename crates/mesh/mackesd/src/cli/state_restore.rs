//! `StateRestore` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `state-restore` subcommand.
#[allow(unreachable_code)]
pub fn run(
    bundle: std::path::PathBuf,
    verify: bool,
    passphrase_env: String,
    db_path: PathBuf,
) -> anyhow::Result<()> {
    {
        // EFF-28 / MESHFS-14.1 — bundle decode + CA restore.
        let passphrase = std::env::var(&passphrase_env).with_context(|| {
            format!(
                "passphrase env-var {passphrase_env} unset — \
                     export it before running state restore",
            )
        })?;
        let armored = std::fs::read_to_string(&bundle)
            .with_context(|| format!("reading bundle {}", bundle.display()))?;
        let sealed = mackesd_core::ca::backup::dearmor(&armored).context("ASCII-armor decode")?;
        let plaintext = mackesd_core::ca::backup::unseal(&passphrase, &sealed)
            .context("AEAD unseal — wrong passphrase OR tampered bundle")?;

        // EFF-28 — --verify: report + stop before any mutation.
        if verify {
            eprintln!(
                "[state-restore --verify] bundle OK: mesh '{}' · exported_at unix:{} · \
                     {} CA cert(s) · {} peer cert(s)",
                plaintext.mesh_id,
                plaintext.exported_at,
                plaintext.ca_certs.len(),
                plaintext.peer_certs.len(),
            );
            eprintln!(
                "[state-restore --verify] dry-run complete — nothing was written. \
                     Re-run without --verify to restore."
            );
            return Ok(());
        }

        let conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        mackesd_core::ca::backup::restore_to_store(&conn, &plaintext)
            .context("restoring CA + peer rows to store")?;
        eprintln!(
            "[state-restore] CA: {ca_n} cert(s) + {peer_n} peer cert(s) restored",
            ca_n = plaintext.ca_certs.len(),
            peer_n = plaintext.peer_certs.len(),
        );
    }
    Ok(())
}
