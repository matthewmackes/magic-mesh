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

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Read};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The default-route CIDR — a hop advertising this is a full exit (W73).
pub const EXIT_ROUTE: &str = "0.0.0.0/0";

/// Keep replicated topology input bounded before JSON deserialization can
/// allocate it. A normal advert is only a few hundred bytes; this leaves room
/// for a deliberately large, but still useful, underlay list without making a
/// single hostile file an unbounded-memory input.
const MAX_ADVERT_BYTES: usize = 64 * 1024;
/// A fleet roster larger than this is not a usable local snapshot. Fail closed
/// rather than installing a partial route set selected by directory order.
const MAX_ADVERT_FILES: usize = 4096;
/// Keep one peer from turning its row into an unbounded route fan-out.
const MAX_SUBNETS_PER_ADVERT: usize = 256;
/// Keep external VPN profile names path-safe and bounded before they become
/// replicated filenames.
const MAX_VPN_PROFILE_NAME_CHARS: usize = 128;
/// A profile is configuration, not an arbitrary blob store; reject oversized
/// input before creating or replacing its on-disk leaf.
const MAX_VPN_PROFILE_BYTES: usize = 256 * 1024;
/// Validation verdicts are tiny JSON records. Keep a replicated verdict
/// bounded before `serde_json` materializes it.
const MAX_VERDICT_BYTES: usize = 256 * 1024;

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
    let advert = validate_advert(advert.clone()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "hop advertisement is invalid or contains an unsafe identity",
        )
    })?;
    let dir = hops_dir(root);
    let path = dir.join(format!("{}.json", advert.hop));
    let body = serde_json::to_vec_pretty(&advert)?;
    crate::ca::seal::write_atomic_sealed(&path, &body)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(path)
}

/// Read every valid hop advertisement (junk-tolerant, sorted by hop).
///
/// The directory is replicated input, not a trusted API. Each JSON file is
/// bounded before parsing; malformed rows, non-canonical identities, and
/// duplicate subnet entries are discarded. A duplicated hop identity is
/// ambiguous, so all rows for that hop are omitted rather than letting an
/// arbitrary directory order choose its routes. An oversized roster fails
/// closed instead of installing a partial snapshot.
#[must_use]
pub fn read_adverts(root: &Path) -> Vec<HopAdvert> {
    let Ok(entries) = std::fs::read_dir(hops_dir(root)) else {
        return Vec::new();
    };

    let mut entries: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().ok().is_some_and(|kind| kind.is_file())
                && entry.path().extension().is_some_and(|x| x == "json")
        })
        .collect();
    entries.sort_by_key(|entry| entry.path());
    if entries.len() > MAX_ADVERT_FILES {
        return Vec::new();
    }

    let mut by_hop: BTreeMap<String, Vec<HopAdvert>> = BTreeMap::new();
    for entry in entries {
        let Some(advert) = read_advert(&entry.path()) else {
            continue;
        };
        by_hop.entry(advert.hop.clone()).or_default().push(advert);
    }

    by_hop
        .into_iter()
        .filter_map(|(_, mut adverts)| (adverts.len() == 1).then(|| adverts.pop().unwrap()))
        .collect()
}

/// Read one replicated row with a hard byte ceiling and validate it before it
/// becomes part of the route-derivation input.
fn read_advert(path: &Path) -> Option<HopAdvert> {
    let bytes = read_bounded_regular_file(path, MAX_ADVERT_BYTES)?;
    let raw = std::str::from_utf8(&bytes).ok()?;
    validate_advert(serde_json::from_str(raw).ok()?)
}

/// Read a replicated text record from the descriptor that will actually be
/// consumed. Reject a final symlink, blocking special file, oversized input,
/// and growth beyond the bound before callers parse or materialize it.
fn read_bounded_regular_file(path: &Path, max_bytes: usize) -> Option<Vec<u8>> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?
        .into()
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if !metadata.file_type().is_file() {
            return None;
        }
        std::fs::File::open(path).ok()?
    };

    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes as u64 {
        return None;
    }
    let capacity = usize::try_from(metadata.len())
        .unwrap_or(max_bytes)
        .min(max_bytes)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    file.take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= max_bytes).then_some(bytes)
}

/// Validate the canonical grammar emitted by `hop-advertise`. Keeping this
/// check at the replicated-input boundary means callers cannot accidentally
/// bypass the CLI's local-owner validation with a hand-written roster row.
fn validate_advert(advert: HopAdvert) -> Option<HopAdvert> {
    if !valid_hop_identity(&advert.hop) || !valid_overlay_ip(&advert.overlay_ip) {
        return None;
    }
    if advert.subnets.is_empty() || advert.subnets.len() > MAX_SUBNETS_PER_ADVERT {
        return None;
    }

    let mut seen_subnets = HashSet::with_capacity(advert.subnets.len());
    for subnet in &advert.subnets {
        if !valid_cidr(subnet) || !seen_subnets.insert(subnet) {
            return None;
        }
    }
    Some(advert)
}

fn valid_hop_identity(host: &str) -> bool {
    if host.is_empty()
        || host == "unknown"
        || host.len() > 253
        || host.trim() != host
        || !host.is_ascii()
    {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

fn valid_overlay_ip(value: &str) -> bool {
    let Ok(ip) = value.parse::<Ipv4Addr>() else {
        return false;
    };
    ip.to_string() == value
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && ip.octets() != [255, 255, 255, 255]
}

fn valid_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    if prefix.contains('/') {
        return false;
    }
    let Ok(ip) = address.parse::<Ipv4Addr>() else {
        return false;
    };
    let Ok(prefix_len) = prefix.parse::<u8>() else {
        return false;
    };
    if ip.to_string() != address || prefix_len.to_string() != prefix || prefix_len > 32 {
        return false;
    }
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };
    u32::from(ip) & mask == u32::from(ip)
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
        if let Some(bytes) = read_bounded_regular_file(&path, MAX_VERDICT_BYTES) {
            if let Ok(raw) = std::str::from_utf8(&bytes) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
                    if let Some(passed) = v.get("passed").and_then(serde_json::Value::as_bool) {
                        return passed; // newest verdict wins
                    }
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
    if !valid_vpn_profile_name(&profile.name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "VPN profile name is empty, oversized, or path-shaped",
        ));
    }
    if profile.config.len() > MAX_VPN_PROFILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "VPN profile exceeds the bounded configuration size",
        ));
    }
    let dir = vpn_profiles_dir(root);
    let path = dir.join(format!("{}.{}", profile.name, profile.kind.ext()));
    crate::ca::seal::write_atomic_sealed(&path, profile.config.as_bytes())
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(path)
}

fn valid_vpn_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_VPN_PROFILE_NAME_CHARS
        && name == name.trim()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name.is_ascii()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
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

    fn write_raw(root: &Path, name: &str, raw: &str) {
        std::fs::create_dir_all(hops_dir(root)).unwrap();
        std::fs::write(hops_dir(root).join(name), raw).unwrap();
    }

    fn write_json(root: &Path, name: &str, advert: &HopAdvert) {
        write_raw(root, name, &serde_json::to_string(advert).unwrap());
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
    fn write_advert_rejects_path_shaped_identity_before_touching_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let error = write_advert(
            tmp.path(),
            &advert("../escape", "10.42.0.9", &["192.168.50.0/24"]),
        )
        .expect_err("path-shaped hop identity must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!tmp.path().join("escape.json").exists());
        assert!(!tmp.path().join("topology/hops").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_advert_replaces_a_hostile_final_symlink_without_following_it() {
        let tmp = tempfile::tempdir().unwrap();
        let hops = hops_dir(tmp.path());
        std::fs::create_dir_all(&hops).unwrap();
        let victim = tmp.path().join("victim.json");
        std::fs::write(&victim, b"victim").unwrap();
        std::os::unix::fs::symlink(&victim, hops.join("gw.json")).unwrap();

        write_advert(tmp.path(), &advert("gw", "10.42.0.9", &["192.168.50.0/24"]))
            .expect("atomic writer replaces the link itself");
        assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
        assert_eq!(
            read_adverts(tmp.path()),
            vec![advert("gw", "10.42.0.9", &["192.168.50.0/24"],)]
        );
    }

    #[test]
    fn read_adverts_rejects_oversized_and_malformed_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let oversized = format!(
            r#"{{"hop":"large","overlay_ip":"10.42.0.9","subnets":["{}"]}}"#,
            "x".repeat(MAX_ADVERT_BYTES)
        );
        write_raw(tmp.path(), "large.json", &oversized);
        for (name, raw) in [
            (
                "bad-host.json",
                r#"{"hop":"../escape","overlay_ip":"10.42.0.9","subnets":["192.168.50.0/24"]}"#,
            ),
            (
                "bad-ip.json",
                r#"{"hop":"bad-ip","overlay_ip":"10.42.0.999","subnets":["192.168.50.0/24"]}"#,
            ),
            (
                "bad-cidr.json",
                r#"{"hop":"bad-cidr","overlay_ip":"10.42.0.9","subnets":["192.168.50.7/24"]}"#,
            ),
            (
                "duplicate-subnet.json",
                r#"{"hop":"duplicate-subnet","overlay_ip":"10.42.0.9","subnets":["192.168.50.0/24","192.168.50.0/24"]}"#,
            ),
            (
                "missing-subnets.json",
                r#"{"hop":"missing-subnets","overlay_ip":"10.42.0.9"}"#,
            ),
        ] {
            write_raw(tmp.path(), name, raw);
        }
        write_json(
            tmp.path(),
            "valid.json",
            &advert("valid", "10.42.0.9", &["192.168.50.0/24"]),
        );

        assert_eq!(
            read_adverts(tmp.path()),
            vec![advert("valid", "10.42.0.9", &["192.168.50.0/24"])]
        );
    }

    #[test]
    fn read_adverts_omits_ambiguous_duplicate_hop_rows() {
        let tmp = tempfile::tempdir().unwrap();
        write_json(
            tmp.path(),
            "duplicate-a.json",
            &advert("duplicate", "10.42.0.9", &["192.168.50.0/24"]),
        );
        write_json(
            tmp.path(),
            "duplicate-b.json",
            &advert("duplicate", "10.42.0.10", &["192.168.60.0/24"]),
        );
        write_json(
            tmp.path(),
            "unique.json",
            &advert("unique", "10.42.0.11", &["192.168.70.0/24"]),
        );

        assert_eq!(
            read_adverts(tmp.path()),
            vec![advert("unique", "10.42.0.11", &["192.168.70.0/24"])]
        );
    }

    #[test]
    fn read_adverts_fails_closed_on_an_oversized_roster() {
        let tmp = tempfile::tempdir().unwrap();
        for index in 0..=MAX_ADVERT_FILES {
            write_json(
                tmp.path(),
                &format!("peer-{index}.json"),
                &advert(&format!("peer-{index}"), "10.42.0.9", &["192.168.50.0/24"]),
            );
        }

        assert!(read_adverts(tmp.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn replicated_hop_rows_reject_final_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("outside.json");
        std::fs::write(
            &target,
            serde_json::to_string(&advert("linked", "10.42.0.9", &["192.168.50.0/24"])).unwrap(),
        )
        .unwrap();
        let hops = hops_dir(tmp.path());
        std::fs::create_dir_all(&hops).unwrap();
        std::os::unix::fs::symlink(&target, hops.join("linked.json")).unwrap();

        assert!(read_adverts(tmp.path()).is_empty());
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

    #[cfg(unix)]
    #[test]
    fn validation_verdict_reads_reject_oversized_and_final_symlink_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let oversized_dir = magic_fleet::validation::run_dir(root, "v-300");
        std::fs::create_dir_all(&oversized_dir).unwrap();
        std::fs::write(oversized_dir.join("run.json"), "{}").unwrap();
        std::fs::write(
            oversized_dir.join("verdict.json"),
            format!(
                r#"{{"passed":true,"padding":"{}"}}"#,
                "x".repeat(MAX_VERDICT_BYTES)
            ),
        )
        .unwrap();
        assert!(!exits_validated(root));

        let target = root.join("outside-verdict.json");
        std::fs::write(&target, r#"{"passed":true}"#).unwrap();
        let linked_dir = magic_fleet::validation::run_dir(root, "v-400");
        std::fs::create_dir_all(&linked_dir).unwrap();
        std::fs::write(linked_dir.join("run.json"), "{}").unwrap();
        std::os::unix::fs::symlink(&target, linked_dir.join("verdict.json")).unwrap();
        assert!(!exits_validated(root));
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

    #[test]
    fn vpn_profile_writer_rejects_path_names_and_oversized_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path_error = write_vpn_profile(
            tmp.path(),
            &VpnProfile {
                name: "../outside".into(),
                kind: VpnKind::Openvpn,
                config: "client\n".into(),
            },
        )
        .expect_err("profile names must not escape the profile directory");
        assert_eq!(path_error.kind(), io::ErrorKind::InvalidInput);
        assert!(!tmp.path().join("outside.ovpn").exists());

        let size_error = write_vpn_profile(
            tmp.path(),
            &VpnProfile {
                name: "large".into(),
                kind: VpnKind::Wireguard,
                config: "x".repeat(MAX_VPN_PROFILE_BYTES + 1),
            },
        )
        .expect_err("profile bodies must stay bounded");
        assert_eq!(size_error.kind(), io::ErrorKind::InvalidInput);
        assert!(!vpn_profiles_dir(tmp.path()).exists());
    }
}
