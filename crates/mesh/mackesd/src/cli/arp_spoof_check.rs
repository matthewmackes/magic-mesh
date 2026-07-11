//! `ArpSpoofCheck` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `arp-spoof-check` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{arp_neigh_map, arp_spoof_suspects};
        for (mac, ips) in arp_spoof_suspects(&arp_neigh_map()) {
            println!("{mac}\t{}", ips.join(","));
        }
    }
    Ok(())
}
