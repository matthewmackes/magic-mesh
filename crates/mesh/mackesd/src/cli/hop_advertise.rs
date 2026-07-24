//! `HopAdvertise` CLI verb handler.
//!
//! Extracted from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1: CLI verb
//! handlers). This module also owns the own-row input validation immediately
//! before the topology advertisement is written.
use crate::*;
use std::net::Ipv4Addr;

use mackesd_core::nebula_topology::HopAdvert;

/// Handle the `hop-advertise` subcommand.
#[allow(unreachable_code)]
pub fn run(subnets: Option<String>, exit: bool) -> anyhow::Result<()> {
    {
        use mackesd_core::nebula_topology::{write_advert, HopAdvert, EXIT_ROUTE};
        let root = mackesd_core::default_qnm_shared_root();
        let host = local_hostname();
        validate_host_identity(&host)?;
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
            overlay_ip: overlay_ip.clone(),
            subnets: nets.clone(),
        };
        validate_hop_advertisement(&advert, &host, &overlay_ip)?;
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

/// Validate the own-row data before it is used to construct the replicated
/// topology path. The caller supplies the values observed locally so a
/// hand-built or future refactored advert cannot publish another node's
/// identity or overlay address.
fn validate_hop_advertisement(
    advert: &HopAdvert,
    local_host: &str,
    local_overlay_ip: &str,
) -> anyhow::Result<()> {
    validate_host_identity(local_host)?;
    validate_host_identity(&advert.hop)?;
    if advert.hop != local_host {
        anyhow::bail!(
            "hop advertisement owner mismatch: hop {:?} is not local host {:?}",
            advert.hop,
            local_host
        );
    }

    let local_ip = validate_overlay_ip(local_overlay_ip)?;
    let advert_ip = validate_overlay_ip(&advert.overlay_ip)?;
    if advert_ip != local_ip {
        anyhow::bail!(
            "hop advertisement overlay owner mismatch: advertised {} is not local {}",
            advert.overlay_ip,
            local_overlay_ip
        );
    }

    if advert.subnets.is_empty() {
        anyhow::bail!("hop advertisement must contain at least one subnet");
    }
    for subnet in &advert.subnets {
        validate_cidr(subnet)?;
    }
    Ok(())
}

/// Hostnames become path components in `topology/hops/<host>.json`; require a
/// canonical DNS hostname shape and reject the placeholder returned when the
/// local hostname command is unavailable.
fn validate_host_identity(host: &str) -> anyhow::Result<()> {
    if host.is_empty() || host == "unknown" {
        anyhow::bail!("local hop hostname is unavailable");
    }
    if host.len() > 253 || host != host.trim() || !host.is_ascii() {
        anyhow::bail!("unsafe hop hostname {:?}", host);
    }

    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            anyhow::bail!("unsafe hop hostname {:?}", host);
        }
    }
    Ok(())
}

/// Require the plain, canonical IPv4 text used by Nebula's overlay address.
fn validate_overlay_ip(value: &str) -> anyhow::Result<Ipv4Addr> {
    let ip = value
        .parse::<Ipv4Addr>()
        .map_err(|_| anyhow::anyhow!("overlay IP must be a canonical IPv4 address: {value:?}"))?;
    if ip.to_string() != value {
        anyhow::bail!("overlay IP must be a canonical IPv4 address: {value:?}");
    }
    let octets = ip.octets();
    if ip.is_unspecified() || ip.is_multicast() || octets == [255, 255, 255, 255] {
        anyhow::bail!("overlay IP is not a usable unicast address: {value:?}");
    }
    Ok(ip)
}

/// Require canonical IPv4 network notation (`a.b.c.d/prefix`). In
/// particular, host bits must be zero and the prefix must not contain a
/// textual alias such as `024`.
fn validate_cidr(value: &str) -> anyhow::Result<()> {
    let (address, prefix) = value
        .split_once('/')
        .filter(|(_, prefix)| !prefix.contains('/'))
        .ok_or_else(|| anyhow::anyhow!("advertised subnet must be an IPv4 CIDR: {value:?}"))?;
    let ip = address.parse::<Ipv4Addr>().map_err(|_| {
        anyhow::anyhow!("advertised subnet must use a canonical IPv4 address: {value:?}")
    })?;
    if ip.to_string() != address {
        anyhow::bail!("advertised subnet must use a canonical IPv4 address: {value:?}");
    }

    let prefix_len = prefix.parse::<u8>().map_err(|_| {
        anyhow::anyhow!("advertised subnet has an invalid prefix length: {value:?}")
    })?;
    if prefix_len.to_string() != prefix || prefix_len > 32 {
        anyhow::bail!("advertised subnet has a non-canonical prefix length: {value:?}");
    }

    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };
    if u32::from(ip) & mask != u32::from(ip) {
        anyhow::bail!("advertised subnet is not a canonical network CIDR: {value:?}");
    }
    Ok(())
}

/// This node's Nebula overlay IP via `ip -4 addr show nebula1`, if up.
fn local_overlay_ip() -> Option<String> {
    let out = std::process::Command::new("ip")
        .args(["-4", "addr", "show", "nebula1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        l.trim()
            .strip_prefix("inet ")
            .and_then(|rest| rest.split('/').next())
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advert(hop: &str, overlay_ip: &str, subnets: &[&str]) -> HopAdvert {
        HopAdvert {
            hop: hop.to_string(),
            overlay_ip: overlay_ip.to_string(),
            subnets: subnets.iter().map(|subnet| (*subnet).to_string()).collect(),
        }
    }

    #[test]
    fn accepts_a_locally_owned_canonical_advertisement() {
        let advert = advert(
            "hop-a.mesh",
            "10.42.0.7",
            &["192.168.40.0/24", "192.168.40.7/32", "0.0.0.0/0"],
        );

        validate_hop_advertisement(&advert, "hop-a.mesh", "10.42.0.7")
            .expect("canonical local advertisement should validate");
    }

    #[test]
    fn rejects_unsafe_or_mismatched_hop_owners() {
        for (hop, expected) in [
            ("../escape", "unsafe hop hostname"),
            ("hop/child", "unsafe hop hostname"),
            ("hop\\child", "unsafe hop hostname"),
            ("hop..mesh", "unsafe hop hostname"),
            ("-hop", "unsafe hop hostname"),
            ("hop-", "unsafe hop hostname"),
            ("", "local hop hostname is unavailable"),
        ] {
            let advert = advert(hop, "10.42.0.7", &["192.168.40.0/24"]);
            let error = validate_hop_advertisement(&advert, "hop-a.mesh", "10.42.0.7")
                .expect_err("unsafe owner must be rejected");
            assert!(error.to_string().contains(expected), "{error:#}");
        }

        let advert = advert("other-hop", "10.42.0.7", &["192.168.40.0/24"]);
        let error = validate_hop_advertisement(&advert, "hop-a", "10.42.0.7")
            .expect_err("another node must not publish this row");
        assert!(error.to_string().contains("owner mismatch"), "{error:#}");
    }

    #[test]
    fn rejects_overlay_addresses_not_owned_by_the_local_node() {
        for overlay_ip in [
            "10.42.0.8",
            "10.42.0.7/24",
            "::1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            let advert = advert("hop-a", overlay_ip, &["192.168.40.0/24"]);
            let error = validate_hop_advertisement(&advert, "hop-a", "10.42.0.7")
                .expect_err("unusable or non-local overlay address must be rejected");
            assert!(
                error.to_string().contains("overlay"),
                "unexpected error for {overlay_ip}: {error:#}"
            );
        }
    }

    #[test]
    fn rejects_malformed_and_non_canonical_cidrs() {
        for subnet in [
            "192.168.40.1/24",   // host bits set
            "192.168.040.0/24",  // non-canonical IPv4 text
            "192.168.40.0/024",  // non-canonical prefix text
            "192.168.40.0/33",   // prefix out of range
            "192.168.40.0",      // missing prefix
            "192.168.40.0/24/1", // multiple separators
            "2001:db8::/64",     // IPv6 is not a Nebula overlay route
            "192.168.40.0 /24",  // embedded whitespace
        ] {
            let advert = advert("hop-a", "10.42.0.7", &[subnet]);
            let error = validate_hop_advertisement(&advert, "hop-a", "10.42.0.7")
                .expect_err("malformed or non-canonical CIDR must be rejected");
            assert!(
                error.to_string().contains("subnet"),
                "unexpected error for {subnet}: {error:#}"
            );
        }
    }
}
