//! PLANES-17 (W72/W73) — Nebula topology as fleet state: hop routes,
//! exit nodes, and external VPN client profiles.
//!
//! The mesh's *internal* topology (who's a lighthouse, who relays,
//! punchy) is already rendered from the CA bundle by `nebula_supervisor`.
//! This module adds the **routed-edge** topology an operator configures:
//!
//!   * **Hop nodes** (W72) — a node advertises one or more *underlay*
//!     subnets it can reach (a branch-office LAN, a lab segment); every
//!     other peer then routes that subnet through the hop's overlay IP via
//!     `tun.unsafe_routes`. Advertisement is **own-row** fleet state (a
//!     hop declares its own reachable subnets), so it converges with no
//!     fixed center.
//!   * **Exit nodes** (W73) — a hop whose advertised set includes the
//!     default route `0.0.0.0/0` is a *full exit*: peers can send all
//!     egress through it. Because a bad exit silently blackholes a peer's
//!     internet, the default route is **gated on a passing validation
//!     verdict** (PLANES-19): [`derive_routes`] drops every `0.0.0.0/0`
//!     edge until `exits_validated` is true ("exit path covered by
//!     validation before the toggle ships").
//!   * **External VPN client profiles** — WireGuard / OpenVPN configs a
//!     node uses to reach *external* networks. These are strictly client
//!     profiles, **never the mesh transport** (§1 — Nebula is the only
//!     overlay); they're stored + materialised, not wired into routing.
//!
//! Pure model + replicated store + route derivation; the render fragment
//! feeds `nebula_supervisor`'s `tun.unsafe_routes`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The default-route CIDR — a hop advertising this is a full exit (W73).
pub const EXIT_ROUTE: &str = "0.0.0.0/0";

/// One hop node's advertisement (own-row fleet state): the underlay
/// subnets it can route on the fleet's behalf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HopAdvert {
    /// The advertising node's hostname.
    pub hop: String,
    /// The hop's overlay IP — the `via` other peers route through.
    pub overlay_ip: String,
    /// Reachable subnets in CIDR form. `0.0.0.0/0` makes this an exit.
    #[serde(default)]
    pub subnets: Vec<String>,
}

impl HopAdvert {
    /// Whether this hop offers a full default-route exit (W73).
    #[must_use]
    pub fn is_exit(&self) -> bool {
        self.subnets.iter().any(|s| s == EXIT_ROUTE)
    }
}

/// The hop-advertisement directory.
#[must_use]
pub fn hops_dir(root: &Path) -> PathBuf {
    root.join("topology").join("hops")
}

/// Write a hop's advertisement (own-row authority, atomic).
///
/// # Errors
/// IO / serialization failures.
pub fn write_advert(root: &Path, advert: &HopAdvert) -> io::Result<PathBuf> {
    let dir = hops_dir(root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", advert.hop));
    let tmp = dir.join(format!(".{}.json.tmp", advert.hop));
    std::fs::write(&tmp, serde_json::to_string_pretty(advert)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read every parseable hop advertisement (junk-tolerant, sorted by hop).
#[must_use]
pub fn read_adverts(root: &Path) -> Vec<HopAdvert> {
    let Ok(entries) = std::fs::read_dir(hops_dir(root)) else {
        return Vec::new();
    };
    let mut out: Vec<HopAdvert> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_json::from_str(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.hop.cmp(&b.hop));
    out
}

/// Derive the `(route, via)` unsafe-route edges THIS node should install,
/// given every hop advertisement.
///
/// * A node never routes a hop's subnet back through the hop itself
///   (`advert.overlay_ip == self_overlay_ip` is skipped).
/// * The default-route exit (`0.0.0.0/0`) is emitted **only** when
///   `exits_validated` — the W73 gate that keeps an unproven exit from
///   blackholing egress.
/// Deterministic: sorted + de-duplicated so every node computes the same
/// set with no coordination.
#[must_use]
pub fn derive_routes(
    adverts: &[HopAdvert],
    self_overlay_ip: &str,
    exits_validated: bool,
) -> Vec<(String, String)> {
    let mut routes: Vec<(String, String)> = Vec::new();
    for advert in adverts {
        if advert.overlay_ip == self_overlay_ip {
            continue; // never route my own advertised subnet to myself
        }
        for subnet in &advert.subnets {
            if subnet == EXIT_ROUTE && !exits_validated {
                continue; // W73 — exit stays off until validation passes
            }
            routes.push((subnet.clone(), advert.overlay_ip.clone()));
        }
    }
    routes.sort();
    routes.dedup();
    routes
}

/// Render derived routes as `tun.unsafe_routes` list items (the lines that
/// continue an already-open `unsafe_routes:` list — 4-space indent).
#[must_use]
pub fn render_unsafe_route_items(routes: &[(String, String)]) -> String {
    let mut out = String::new();
    for (route, via) in routes {
        out.push_str(&format!("    - route: {route}\n      via: {via}\n"));
    }
    out
}

/// Whether the fleet's most recent overlay-reachability validation run
/// passed (PLANES-19) — the gate [`derive_routes`] consults for exits.
/// Absent any verdict, exits stay OFF (fail-safe).
#[must_use]
pub fn exits_validated(workgroup_root: &Path) -> bool {
    let ids = magic_fleet::validation::list_run_ids(workgroup_root);
    for id in ids.into_iter().rev() {
        let path = magic_fleet::validation::run_dir(workgroup_root, &id).join("verdict.json");
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(passed) = v.get("passed").and_then(serde_json::Value::as_bool) {
                    return passed; // newest verdict wins
                }
            }
        }
    }
    false
}

// ── External VPN client profiles (never transport, §1) ─────────────────

/// A client-VPN profile kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VpnKind {
    /// WireGuard `.conf`.
    Wireguard,
    /// OpenVPN `.ovpn`.
    Openvpn,
}

impl VpnKind {
    /// On-disk extension for this profile kind.
    #[must_use]
    pub const fn ext(self) -> &'static str {
        match self {
            VpnKind::Wireguard => "conf",
            VpnKind::Openvpn => "ovpn",
        }
    }
}

/// An external VPN client profile: a config blob a node uses to reach an
/// *external* network. NOT the mesh transport (§1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpnProfile {
    /// Profile name (filename stem).
    pub name: String,
    /// Profile kind.
    pub kind: VpnKind,
    /// The raw config file body.
    pub config: String,
}

/// The VPN-profiles directory.
#[must_use]
pub fn vpn_profiles_dir(root: &Path) -> PathBuf {
    root.join("topology").join("vpn-profiles")
}

/// Write a VPN client profile (atomic).
///
/// # Errors
/// IO failures.
pub fn write_vpn_profile(root: &Path, profile: &VpnProfile) -> io::Result<PathBuf> {
    let dir = vpn_profiles_dir(root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.{}", profile.name, profile.kind.ext()));
    let tmp = dir.join(format!(".{}.tmp", profile.name));
    std::fs::write(&tmp, &profile.config)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// List the VPN profiles present (by name + kind, sorted by name).
#[must_use]
pub fn list_vpn_profiles(root: &Path) -> Vec<(String, VpnKind)> {
    let Ok(entries) = std::fs::read_dir(vpn_profiles_dir(root)) else {
        return Vec::new();
    };
    let mut out: Vec<(String, VpnKind)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            let kind = match path.extension()?.to_str()? {
                "conf" => VpnKind::Wireguard,
                "ovpn" => VpnKind::Openvpn,
                _ => return None,
            };
            Some((stem, kind))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advert(hop: &str, ip: &str, subnets: &[&str]) -> HopAdvert {
        HopAdvert {
            hop: hop.into(),
            overlay_ip: ip.into(),
            subnets: subnets.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn adverts_round_trip_through_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        write_advert(tmp.path(), &advert("gw", "10.42.0.9", &["192.168.50.0/24"])).unwrap();
        let back = read_adverts(tmp.path());
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].subnets, vec!["192.168.50.0/24"]);
        assert!(!back[0].is_exit());
    }

    #[test]
    fn derive_routes_installs_hop_subnets_but_not_my_own() {
        let adverts = vec![
            advert("gw", "10.42.0.9", &["192.168.50.0/24"]),
            advert("me", "10.42.0.2", &["10.0.0.0/8"]),
        ];
        // From the perspective of "me" (10.42.0.2): take gw's subnet, skip
        // my own advertisement (I don't route my LAN back to myself).
        let routes = derive_routes(&adverts, "10.42.0.2", false);
        assert_eq!(routes, vec![("192.168.50.0/24".into(), "10.42.0.9".into())]);
    }

    #[test]
    fn exit_route_is_gated_on_validation() {
        let adverts = vec![advert("exit", "10.42.0.9", &["0.0.0.0/0"])];
        // Unvalidated: the default-route exit is withheld (W73).
        assert!(derive_routes(&adverts, "10.42.0.2", false).is_empty());
        // Validated: the exit edge is installed.
        assert_eq!(
            derive_routes(&adverts, "10.42.0.2", true),
            vec![("0.0.0.0/0".into(), "10.42.0.9".into())]
        );
    }

    #[test]
    fn exits_validated_reads_the_newest_passing_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!exits_validated(root), "no verdict → exits off (fail-safe)");
        // Seed a passing verdict for the newest run.
        let dir = magic_fleet::validation::run_dir(root, "v-200");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.json"), "{}").unwrap();
        std::fs::write(dir.join("verdict.json"), r#"{"passed":true}"#).unwrap();
        assert!(exits_validated(root));
    }

    #[test]
    fn render_items_continue_an_unsafe_routes_list() {
        let items = render_unsafe_route_items(&[
            ("192.168.50.0/24".into(), "10.42.0.9".into()),
            ("0.0.0.0/0".into(), "10.42.0.9".into()),
        ]);
        assert!(items.contains("    - route: 192.168.50.0/24\n      via: 10.42.0.9\n"));
        assert!(items.contains("    - route: 0.0.0.0/0\n      via: 10.42.0.9\n"));
    }

    #[test]
    fn vpn_profiles_store_and_list_by_kind() {
        let tmp = tempfile::tempdir().unwrap();
        write_vpn_profile(
            tmp.path(),
            &VpnProfile {
                name: "branch-office".into(),
                kind: VpnKind::Wireguard,
                config: "[Interface]\nPrivateKey=...\n".into(),
            },
        )
        .unwrap();
        let listed = list_vpn_profiles(tmp.path());
        assert_eq!(
            listed,
            vec![("branch-office".to_string(), VpnKind::Wireguard)]
        );
    }
}
