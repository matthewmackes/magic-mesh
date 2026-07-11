//! `HopAdvertise` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `hop-advertise` subcommand.
#[allow(unreachable_code)]
pub fn run(subnets: Option<String>, exit: bool) -> anyhow::Result<()> {
    {
        use mackesd_core::nebula_topology::{write_advert, HopAdvert, EXIT_ROUTE};
        let root = mackesd_core::default_qnm_shared_root();
        let host = local_hostname();
        let overlay_ip = local_overlay_ip().ok_or_else(|| {
            anyhow::anyhow!("no overlay IP on nebula1 — is this node enrolled and up?")
        })?;
        let mut nets: Vec<String> = subnets
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if exit && !nets.iter().any(|s| s == EXIT_ROUTE) {
            nets.push(EXIT_ROUTE.to_string());
        }
        if nets.is_empty() {
            anyhow::bail!("nothing to advertise — pass --subnets <cidr,...> and/or --exit");
        }
        let advert = HopAdvert {
            hop: host.clone(),
            overlay_ip,
            subnets: nets.clone(),
        };
        write_advert(&root, &advert)?;
        tracing::info!(
            target: "mackesd::audit",
            event = "topology.hop_advertise",
            host = %host,
            subnets = %nets.join(","),
            "PLANES-17: hop advertisement updated"
        );
        println!("hop {host} now advertises: {}", nets.join(", "));
        return Ok(());
    }
    Ok(())
}
