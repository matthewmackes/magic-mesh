//! `ShowPasscode` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `show-passcode` subcommand.
#[allow(unreachable_code)]
pub fn run(cred_path: Option<PathBuf>) -> anyhow::Result<()> {
    {
        // EPIC-SEC-PASSCODE-CREDS — decrypt + print the stored
        // passcode. The inverse of generate/rotate --store.
        let path = cred_path.unwrap_or_else(mackesd_core::passcode_creds::default_cred_path);
        let code =
            mackesd_core::passcode_creds::load(&path, mackesd_core::passcode_creds::CRED_NAME)
                .map_err(|e| anyhow::anyhow!("show-passcode: {e}"))?;
        println!("{code}");
    }
    Ok(())
}
