//! `Probe` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `probe` subcommand.
#[allow(unreachable_code)]
pub fn run(action: ProbeCmd) -> anyhow::Result<()> {
    match action {
        ProbeCmd::Scan {
            targets,
            deep,
            source,
            nse_dir,
        } => {
            use mackesd_core::card::probe::HostSource;
            use mackesd_core::probe_nmap::{scan, Profile};
            let src = match source.as_str() {
                "lan" => HostSource::Lan,
                "arbitrary" => HostSource::Arbitrary,
                _ => HostSource::Mesh,
            };
            let profile = if deep { Profile::Deep } else { Profile::Fast };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let cards = scan("nmap", profile, &targets, &[], &nse_dir, src, now);
            // One JSON line per host card (each carries its service
            // children). Empty output = no hosts found / nmap absent.
            for card in &cards {
                println!("{}", serde_json::to_string(card)?);
            }
        }
        ProbeCmd::Refresh {
            workgroup_root,
            node_id,
            nse_dir,
        } => {
            // MESH-PROBE-4 manual refresh — one deep cycle that
            // writes probe-inventory.json + announces probe/changed.
            let workgroup_root =
                workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            let node_id = node_id.unwrap_or_else(default_node_id);
            let home =
                std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
            let n = mackesd_core::probe_nmap::run_probe_cycle(
                &workgroup_root,
                &node_id,
                &home,
                "nmap",
                &nse_dir,
                true,
            );
            println!("probe refresh: {n} host(s) in inventory");
        }
        ProbeCmd::List {
            workgroup_root,
            service,
        } => {
            // MESH-PROBE-6 — read the merged mesh-wide inventory.
            let workgroup_root =
                workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
            match service {
                Some(kind) => {
                    for hs in mackesd_core::probe_nmap::peers_with_service(&workgroup_root, &kind) {
                        println!(
                            "{}\t{}\t{}:{}",
                            hs.host.ip, hs.service.service_kind, hs.host.hostname, hs.service.port
                        );
                    }
                }
                None => {
                    for card in &mackesd_core::probe_nmap::inventory(&workgroup_root) {
                        println!("{}", serde_json::to_string(card)?);
                    }
                }
            }
        }
    }
    Ok(())
}
