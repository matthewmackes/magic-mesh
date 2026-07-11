//! `CaptivePortalCheck` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `captive-portal-check` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{detect_captive_portal, CAPTIVE_PROBE_URL};
        if let Some(portal) = detect_captive_portal(CAPTIVE_PROBE_URL) {
            if portal.is_empty() {
                eprintln!("CAPTIVE-PORTAL: detected (splash intercept; no redirect URL)");
            } else {
                println!("{portal}");
                eprintln!("CAPTIVE-PORTAL: redirected to {portal}");
            }
            std::process::exit(1);
        }
    }
    Ok(())
}
