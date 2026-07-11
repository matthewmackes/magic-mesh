//! `SetExternalAddr` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `set-external-addr` subcommand.
#[allow(unreachable_code)]
pub fn run(addr: String) -> anyhow::Result<()> {
    {
        // Normalize to ip:port (default 4242) so the directory + roster carry
        // a dialable underlay address.
        let normalized = if addr.contains(':') {
            addr.clone()
        } else {
            format!("{addr}:4242")
        };
        mackesd_core::lighthouse_addr::write_external_addr(&normalized)
            .with_context(|| format!("persisting external-addr {normalized}"))?;
        println!(
            "external address set to {normalized} (published on the next heartbeat; \
                 every node's enroll roster will include this lighthouse)"
        );
        return Ok(());
    }
    Ok(())
}
