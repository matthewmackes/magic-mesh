//! `DiscoverMdePeers` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `discover-mde-peers` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{collect_mdns, hosts_from_mdns, mde_peer_candidates};
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let hosts = hosts_from_mdns(&collect_mdns("avahi-browse"), now_ms);
        for (ip, hostname) in mde_peer_candidates(&hosts) {
            println!("{ip}\t{hostname}");
        }
    }
    Ok(())
}
