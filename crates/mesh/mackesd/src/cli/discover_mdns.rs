//! `DiscoverMdns` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `discover-mdns` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{
            arp_neigh_map, classify, collect_mdns, enrich_hosts, hosts_from_mdns, load_system_oui,
            refine_unknown_with_http, refine_unknown_with_nmap_os, reverse_dns, HostSignals,
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let records = collect_mdns("avahi-browse");
        let mut hosts = hosts_from_mdns(&records, now_ms);
        for host in &mut hosts {
            // Fill a missing hostname via reverse-DNS, then let the
            // console hostname-hint re-refine the type.
            if host.hostname.is_empty() {
                if let Some(name) = reverse_dns(&host.ip) {
                    host.hostname = name;
                    let sig = HostSignals {
                        mdns_services: host.services.clone(),
                        hostname: host.hostname.clone(),
                        ..Default::default()
                    };
                    host.host_type = classify(&sig);
                }
            }
        }
        // MESH-A-4.c.1 — ARP-MAC + OUI-vendor enrichment over the
        // local neighbour table, re-typing mDNS-less hosts.
        let mut hosts = enrich_hosts(hosts, &arp_neigh_map(), &load_system_oui());
        // MESH-A-4.c.3 — HTTP-banner refine for still-Unknown hosts.
        refine_unknown_with_http(&mut hosts);
        // MESH-A-4.c.3.b — active nmap -O fingerprint, last-resort
        // refine for hosts still Unknown after the HTTP banner.
        refine_unknown_with_nmap_os(&mut hosts);
        for host in &hosts {
            println!("{}", serde_json::to_string(host)?);
        }
    }
    Ok(())
}
