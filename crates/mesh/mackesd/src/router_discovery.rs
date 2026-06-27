//! ROUTER-1 / ROUTER-2 — per-node router/firewall discovery + Vyatta-CLI
//! fingerprint.
//!
//! Each node may sit behind its own router/firewall — an Ubiquiti EdgeRouter
//! (EdgeOS) or a VyOS box, both driven through the same Vyatta CLI. This module
//! finds the node's PRIMARY appliance (the lowest-metric default route), resolves
//! its stable id (the gateway MAC), and fingerprints the vendor/OS. Vendor scope
//! is the Vyatta-CLI family only (design: docs/design/router-control.md, lock #6);
//! anything else is surfaced read-only as unknown (lock #4).
//!
//! The parsers are pure + unit-tested; `discover_primary` is the thin shell-out
//! that mirrors the `ip route` / `ip neigh` pattern used by [`crate::workers::netassess`].

use std::process::Command;

/// The router/OS family behind a discovered appliance. Scope is the Vyatta-CLI
/// family (EdgeOS / VyOS); everything else is `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterVendor {
    /// Ubiquiti EdgeRouter (EdgeOS — a Vyatta fork).
    EdgeOs,
    /// VyOS (Vyatta fork, same CLI surface).
    VyOs,
    /// Reached + Vyatta-shaped, but the version string was unrecognized.
    UnknownVyatta,
    /// Not fingerprinted / not a Vyatta-CLI device.
    Unknown,
}

impl RouterVendor {
    /// Whether this appliance is controllable via the Vyatta CLI adapter.
    #[must_use]
    pub fn is_vyatta(self) -> bool {
        matches!(
            self,
            RouterVendor::EdgeOs | RouterVendor::VyOs | RouterVendor::UnknownVyatta
        )
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RouterVendor::EdgeOs => "edgeos",
            RouterVendor::VyOs => "vyos",
            RouterVendor::UnknownVyatta => "vyatta-unknown",
            RouterVendor::Unknown => "unknown",
        }
    }
}

/// A discovered router/firewall candidate behind this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterCandidate {
    /// Management IP (the default-route gateway, or a LAN management appliance).
    pub ip: String,
    /// Gateway MAC (lowercase colon form) — the stable id keying `router/<mac>`.
    pub mac: String,
    /// True when this is the node's primary (lowest-metric) default route.
    pub is_default: bool,
    /// Passive vendor hint from MAC-OUI (e.g. "ubiquiti"); `None` if unknown.
    pub oui_hint: Option<String>,
}

impl RouterCandidate {
    /// The secret-store / registry key for this appliance: `router/<mac>`
    /// (lock #9/#10 — keyed by gateway MAC; reuses the `xcp/<host>` keyspace).
    #[must_use]
    pub fn cred_ref(&self) -> String {
        format!("router/{}", self.mac)
    }
}

/// Parse `ip route show default` into (gateway_ip, metric) pairs. A line looks
/// like `default via 10.0.0.1 dev eth0 proto dhcp metric 100`. Absent metric
/// defaults to 0 (kernel default).
#[must_use]
pub fn parse_default_routes(stdout: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.first() != Some(&"default") {
            continue;
        }
        let Some(pos) = toks.iter().position(|t| *t == "via") else {
            continue;
        };
        let Some(gw) = toks.get(pos + 1) else {
            continue;
        };
        let metric = toks
            .iter()
            .position(|t| *t == "metric")
            .and_then(|m| toks.get(m + 1))
            .and_then(|m| m.parse::<u32>().ok())
            .unwrap_or(0);
        out.push(((*gw).to_string(), metric));
    }
    out
}

/// The lowest-metric default-route gateway IP (lock #3 — manage the primary
/// default only). `None` when there is no default route.
#[must_use]
pub fn primary_default_gateway(route_stdout: &str) -> Option<String> {
    parse_default_routes(route_stdout)
        .into_iter()
        .min_by_key(|(_, metric)| *metric)
        .map(|(ip, _)| ip)
}

/// Resolve the MAC for `ip` from `ip neigh` output
/// (`<ip> dev <if> lladdr <mac> REACHABLE`). Returns the lowercase colon-form
/// MAC, or `None` if absent/incomplete (e.g. a FAILED/INCOMPLETE neighbor).
#[must_use]
pub fn mac_for_ip(neigh_stdout: &str, ip: &str) -> Option<String> {
    for line in neigh_stdout.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.first() != Some(&ip) {
            continue;
        }
        if let Some(pos) = toks.iter().position(|t| *t == "lladdr") {
            if let Some(mac) = toks.get(pos + 1) {
                return Some(mac.to_ascii_lowercase());
            }
        }
    }
    None
}

/// Passive OUI hint: the lowercased vendor token when it's a known network-gear
/// vendor (Ubiquiti is the relevant one for the Vyatta scope). `None` otherwise.
#[must_use]
pub fn oui_hint(vendor: &str) -> Option<String> {
    let v = vendor.to_ascii_lowercase();
    for needle in ["ubiquiti", "mikrotik", "vyos", "cisco", "juniper", "fortinet"] {
        if v.contains(needle) {
            return Some(needle.to_string());
        }
    }
    None
}

/// Fingerprint the vendor/OS from a Vyatta `show version` (lock #5/#7). EdgeOS
/// prints "EdgeOS"/"EdgeRouter"/"UBNT"; VyOS prints "VyOS". A Vyatta-shaped but
/// unrecognized output is `UnknownVyatta`; anything else `Unknown`.
#[must_use]
pub fn fingerprint_from_version(show_version_stdout: &str) -> RouterVendor {
    let s = show_version_stdout.to_ascii_lowercase();
    if s.contains("vyos") {
        RouterVendor::VyOs
    } else if s.contains("edgeos") || s.contains("edgerouter") || s.contains("ubnt") {
        RouterVendor::EdgeOs
    } else if s.contains("vyatta") {
        RouterVendor::UnknownVyatta
    } else {
        RouterVendor::Unknown
    }
}

fn run_stdout(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Discover the node's PRIMARY router appliance: the lowest-metric default-route
/// gateway + its MAC. Returns `None` when the node has no default route or the
/// gateway MAC can't be resolved (safe no-op per lock #2). Shells out to
/// `ip route` / `ip neigh`; the parsing is the tested pure code above.
#[must_use]
pub fn discover_primary() -> Option<RouterCandidate> {
    let route = run_stdout("ip", &["route", "show", "default"])?;
    let ip = primary_default_gateway(&route)?;
    let neigh = run_stdout("ip", &["neigh"]).unwrap_or_default();
    let mac = mac_for_ip(&neigh, &ip)?;
    Some(RouterCandidate {
        ip,
        mac,
        is_default: true,
        oui_hint: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowest_metric_default_wins() {
        let out = "default via 10.0.0.1 dev eth0 proto dhcp metric 100\n\
                   default via 192.168.1.1 dev wlan0 proto dhcp metric 600\n";
        assert_eq!(parse_default_routes(out).len(), 2);
        assert_eq!(primary_default_gateway(out).as_deref(), Some("10.0.0.1"));
    }

    #[test]
    fn no_default_route_is_none() {
        assert_eq!(primary_default_gateway("10.0.0.0/24 dev eth0\n"), None);
    }

    #[test]
    fn default_without_metric_defaults_to_zero() {
        let out = "default via 172.20.0.1 dev eth0\n";
        assert_eq!(parse_default_routes(out), vec![("172.20.0.1".into(), 0)]);
        assert_eq!(primary_default_gateway(out).as_deref(), Some("172.20.0.1"));
    }

    #[test]
    fn mac_for_ip_reads_lladdr() {
        let neigh = "172.20.0.1 dev eno1 lladdr 46:6A:7C:96:E8:AA REACHABLE\n\
                     172.20.0.9 dev eno1 FAILED\n";
        assert_eq!(
            mac_for_ip(neigh, "172.20.0.1").as_deref(),
            Some("46:6a:7c:96:e8:aa")
        );
        // No lladdr (FAILED) → None.
        assert_eq!(mac_for_ip(neigh, "172.20.0.9"), None);
        assert_eq!(mac_for_ip(neigh, "10.0.0.5"), None);
    }

    #[test]
    fn fingerprint_distinguishes_vyatta_forks() {
        assert_eq!(
            fingerprint_from_version("Version: v2.0.9-hotfix.7\nEdgeOS ER-8"),
            RouterVendor::EdgeOs
        );
        assert_eq!(
            fingerprint_from_version("UBNT EdgeRouter"),
            RouterVendor::EdgeOs
        );
        assert_eq!(
            fingerprint_from_version("Version:          VyOS 1.4-rolling"),
            RouterVendor::VyOs
        );
        assert_eq!(
            fingerprint_from_version("Vyatta Core 6.6"),
            RouterVendor::UnknownVyatta
        );
        assert_eq!(
            fingerprint_from_version("RouterOS 7.1"),
            RouterVendor::Unknown
        );
    }

    #[test]
    fn vyatta_family_gate() {
        assert!(RouterVendor::EdgeOs.is_vyatta());
        assert!(RouterVendor::VyOs.is_vyatta());
        assert!(RouterVendor::UnknownVyatta.is_vyatta());
        assert!(!RouterVendor::Unknown.is_vyatta());
    }

    #[test]
    fn oui_hint_flags_ubiquiti() {
        assert_eq!(oui_hint("Ubiquiti Inc").as_deref(), Some("ubiquiti"));
        assert_eq!(oui_hint("Dell Inc"), None);
    }

    #[test]
    fn cred_ref_keys_by_mac() {
        let c = RouterCandidate {
            ip: "172.20.0.1".into(),
            mac: "46:6a:7c:96:e8:aa".into(),
            is_default: true,
            oui_hint: Some("ubiquiti".into()),
        };
        assert_eq!(c.cred_ref(), "router/46:6a:7c:96:e8:aa");
    }
}
