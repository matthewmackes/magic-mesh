//! `GeneratePasscode` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `generate-passcode` subcommand.
#[allow(unreachable_code)]
pub fn run(store: bool, cred_path: Option<PathBuf>) -> anyhow::Result<()> {
    {
        let code = mackesd_core::passcode::generate();
        println!("{code}");
        if store {
            let path = cred_path.unwrap_or_else(mackesd_core::passcode_creds::default_cred_path);
            mackesd_core::passcode_creds::store(
                &code,
                &path,
                mackesd_core::passcode_creds::CRED_NAME,
            )
            .map_err(|e| anyhow::anyhow!("generate-passcode --store: {e}"))?;
            eprintln!(
                "stored (encrypted via systemd-creds) at {}. Share the code \
                     above with peers; the plaintext is not on disk.",
                path.display()
            );
        } else {
            eprintln!(
                "(encrypt at rest with: mackesd generate-passcode --store, \
                     or save to libsecret manually)"
            );
        }
    }
    Ok(())
}
