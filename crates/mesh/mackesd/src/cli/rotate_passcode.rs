//! `RotatePasscode` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `rotate-passcode` subcommand.
#[allow(unreachable_code)]
pub fn run(store: bool, cred_path: Option<PathBuf>) -> anyhow::Result<()> {
    {
        // Phase 12.10.2 — generate fresh passcode; peer
        // redistribution wires through the reconcile loop (12.5).
        let code = mackesd_core::passcode::generate();
        println!("{code}");
        if store {
            let path = cred_path.unwrap_or_else(mackesd_core::passcode_creds::default_cred_path);
            mackesd_core::passcode_creds::store(
                &code,
                &path,
                mackesd_core::passcode_creds::CRED_NAME,
            )
            .map_err(|e| anyhow::anyhow!("rotate-passcode --store: {e}"))?;
            eprintln!(
                "rotation: stored (encrypted via systemd-creds) at {}; \
                     peers refresh their bearer tokens on next heartbeat.",
                path.display()
            );
        } else {
            eprintln!(
                "rotation: encrypt at rest with `mackesd rotate-passcode \
                     --store`; peers refresh their bearer tokens on next \
                     heartbeat."
            );
        }
    }
    Ok(())
}
