//! `RogueDhcpCheck` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `rogue-dhcp-check` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::detect_dhcp_servers;
        let servers = detect_dhcp_servers();
        for s in &servers {
            println!("{s}");
        }
        if servers.len() >= 2 {
            eprintln!(
                "ROGUE-DHCP: {} DHCP servers responding (expected 1)",
                servers.len()
            );
            std::process::exit(1);
        }
    }
    Ok(())
}
