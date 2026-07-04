//! SEC-5 / KDC2-4 — the KDC mesh-shunt worker.
//!
//! Stock KDE-Connect only sees phones that broadcast on the local
//! LAN segment. The shunt makes the *whole mesh* a discovery domain:
//! each peer publishes the devices it has paired into the replicated
//! `<root>/kdc-phones/<hostname>.json` (own-row authority — only this
//! box writes its own file); every peer reads its neighbors' files
//! and feeds each entry into the discovery layer as a
//! [`SyntheticAnnounce`] via [`DiscoveryRegistry::inject_synthetic`],
//! so a phone paired on peer-A is reachable from peer-B without a
//! direct broadcast.
//!
//! This is what finally consumes the KDC2-2.1 seam
//! (`SyntheticAnnounce` / `inject_synthetic` / `is_fresh` /
//! `take_fresh`) the H8 audit flagged as forward-declared.
//!
//! **Accept any relayer (Q26/27):** a synthetic announce from any
//! enrolled peer is honored — the trust gate is the per-device cert
//! fingerprint pin (SEC-4), identical for real and relayed
//! announces, so relaying carries no new trust. Stale entries
//! (`is_fresh`) and self-relays are dropped.
//!
//! **KDC-MESH-2 (2026-07-04) — the roster carries overlay IPs.** Each host's
//! published file is now a [`PublishedRoster`]: the host's own KDC device id +
//! Nebula overlay IP, plus each paired phone with the phone's overlay IP when
//! known. Neighbors fold both into the [`OverlayTransport`] peer directory
//! ([`collect_overlay_directory`]) so `open(&PeerId)` dials the phone/host
//! directly by overlay IP — a directed unicast, never a UDP broadcast (design
//! #2, which Nebula doesn't carry). A row without an overlay IP is an honest
//! gate: the device is relayed for its *name* but is not dialable until its
//! overlay IP flows from the enroll/roster (§7). The legacy pre-KDC-MESH-2 file
//! shape (a bare `[PublishedDevice, …]` array) still parses ([`parse_roster`]).
//!
//! [`OverlayTransport`]: mde_kdc_host::OverlayTransport

#![cfg(feature = "async-services")]

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use mde_kdc_proto::discovery::{Announce, DeviceType, DiscoveryRegistry, SyntheticAnnounce};

/// Republish + relay cadence — phones change rarely; 30 s keeps a
/// newly-paired phone visible mesh-wide within half a minute.
pub const TICK: Duration = Duration::from_secs(30);

/// The replicated directory holding every peer's published phones.
#[must_use]
pub fn phones_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("kdc-phones")
}

/// One published device entry (the JSON shape on the volume).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublishedDevice {
    /// KDE-Connect device identifier (stable across renames).
    pub device_id: String,
    /// Human-readable device name as reported by KDE-Connect.
    pub device_name: String,
    /// KDC-MESH-2 — the phone's Nebula overlay IP, when the publishing host
    /// knows it (it flows from the enroll/roster). Neighbors fold it into the
    /// `OverlayTransport` peer directory so `open(phone)` dials it directly
    /// (design #2, no broadcast). `None` until enroll records it — an honest
    /// gate: no overlay IP ⇒ relayed for its name but not dialable (§7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<String>,
}

/// KDC-MESH-2 — the published roster document.
///
/// The publishing host's own overlay identity plus its paired phones. Written by
/// [`publish_roster`] (a bare [`publish_phones`] omits the host fields); read
/// back via [`parse_roster`], which also accepts the legacy pre-KDC-MESH-2 file
/// shape (a bare `[PublishedDevice, …]` array) for a seamless upgrade.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublishedRoster {
    /// The publishing host's KDC device id (its `/etc/machine-id`) — the key
    /// its overlay IP lands under in the peer directory, so neighbors can dial
    /// this host by overlay IP too (design #2). Empty when unpublished.
    #[serde(default)]
    pub host_device_id: String,
    /// The publishing host's Nebula overlay IP. `None` until the host is on the
    /// mesh (an honest gate — a host with no overlay IP is not dialable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_overlay_ip: Option<String>,
    /// The phones this host has paired (own-row authority — never a relayed
    /// one).
    #[serde(default)]
    pub devices: Vec<PublishedDevice>,
}

/// KDC-MESH-2 — the overlay IPs a host learns from its neighbors' rosters.
///
/// Split by kind so the caller can dial both but only ever directed-*announce*
/// to phones. Each entry is `(KDC device_id, overlay IP)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RosterOverlay {
    /// Relayed phones reachable by overlay IP.
    pub phones: Vec<(String, IpAddr)>,
    /// Neighbor hosts reachable by overlay IP.
    pub hosts: Vec<(String, IpAddr)>,
}

/// Parse a published-roster file body into a [`PublishedRoster`].
///
/// Tolerates both the KDC-MESH-2 object and the legacy bare
/// `[PublishedDevice, …]` array (wrapped into a host-less roster); `None` on
/// unparseable junk.
#[must_use]
pub fn parse_roster(raw: &str) -> Option<PublishedRoster> {
    if let Ok(roster) = serde_json::from_str::<PublishedRoster>(raw) {
        return Some(roster);
    }
    serde_json::from_str::<Vec<PublishedDevice>>(raw)
        .ok()
        .map(|devices| PublishedRoster {
            devices,
            ..PublishedRoster::default()
        })
}

/// Parse a published overlay-IP string into a **dialable** address: a valid IP
/// that is not the wildcard `0.0.0.0`/`::` (which the overlay transport refuses
/// to dial). Mirrors the `OverlayTransport` honest gate one layer up so a junk
/// or wildcard row is simply not resolvable rather than a bad dial.
fn parse_dialable_ip(raw: &str) -> Option<IpAddr> {
    let ip = raw.trim().parse::<IpAddr>().ok()?;
    (!ip.is_unspecified()).then_some(ip)
}

/// Write this peer's full [`PublishedRoster`] to its own published file (atomic
/// temp + rename).
///
/// Carries the host's overlay identity (`host_device_id` + `host_overlay_ip`)
/// plus its paired `devices` — the KDC-MESH-2 form with overlay IPs;
/// [`publish_phones`] is the host-less shorthand.
///
/// # Errors
/// IO / serialization failures.
pub fn publish_roster(
    workgroup_root: &Path,
    hostname: &str,
    host_device_id: &str,
    host_overlay_ip: Option<String>,
    devices: &[PublishedDevice],
) -> std::io::Result<PathBuf> {
    let dir = phones_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hostname}.json"));
    let roster = PublishedRoster {
        host_device_id: host_device_id.to_string(),
        host_overlay_ip,
        devices: devices.to_vec(),
    };
    let body = serde_json::to_string_pretty(&roster)?;
    let tmp = dir.join(format!(".{hostname}.json.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Write this peer's paired devices with no host overlay identity — the
/// shorthand over [`publish_roster`] for callers/tests that don't publish the
/// host's own overlay IP.
///
/// # Errors
/// IO / serialization failures.
pub fn publish_phones(
    workgroup_root: &Path,
    hostname: &str,
    devices: &[PublishedDevice],
) -> std::io::Result<PathBuf> {
    publish_roster(workgroup_root, hostname, "", None, devices)
}

/// Read every neighbor's published phones (skipping our own file)
/// into [`SyntheticAnnounce`] records stamped `now_ms`. Junk /
/// half-replicated files are skipped, like every other replicated
/// reader in the platform.
#[must_use]
pub fn collect_synthetic(
    workgroup_root: &Path,
    self_hostname: &str,
    now_ms: i64,
) -> Vec<SyntheticAnnounce> {
    let Ok(entries) = std::fs::read_dir(phones_dir(workgroup_root)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        // Own-row authority: never relay our own published file back.
        if stem == self_hostname || path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(roster) = parse_roster(&raw) else {
            continue;
        };
        for d in roster.devices {
            out.push(SyntheticAnnounce {
                announce: Announce {
                    device_id: d.device_id,
                    device_name: d.device_name,
                    device_type: DeviceType::Phone,
                    protocol_version: 7,
                    incoming_capabilities: Vec::new(),
                    outgoing_capabilities: Vec::new(),
                },
                relayed_by: format!("peer:{stem}"),
                relayed_at_ms: now_ms,
            });
        }
    }
    out
}

/// KDC-MESH-2 — collect the overlay IPs from every neighbor's published roster.
///
/// Reads each neighbor file (skipping our own) for its host overlay IP and each
/// relayed phone's overlay IP. The caller folds these into the
/// `OverlayTransport` peer directory so `open(&PeerId)` dials directly by
/// overlay IP (design #2). Rows without a (dialable) overlay IP are honestly
/// omitted — relayed for their name, not yet dialable. Junk / half-replicated
/// files are skipped, like every other replicated reader.
#[must_use]
pub fn collect_overlay_directory(workgroup_root: &Path, self_hostname: &str) -> RosterOverlay {
    let mut out = RosterOverlay::default();
    let Ok(entries) = std::fs::read_dir(phones_dir(workgroup_root)) else {
        return out;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        // Own-row authority: never fold our own file back into the directory.
        if stem == self_hostname || path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(roster) = parse_roster(&raw) else {
            continue;
        };
        // The neighbor host itself, resolvable by overlay IP (host↔host reach).
        if !roster.host_device_id.is_empty() {
            if let Some(ip) = roster.host_overlay_ip.as_deref().and_then(parse_dialable_ip) {
                out.hosts.push((roster.host_device_id, ip));
            }
        }
        // Each relayed phone that carries a dialable overlay IP.
        for d in roster.devices {
            if let Some(ip) = d.overlay_ip.as_deref().and_then(parse_dialable_ip) {
                out.phones.push((d.device_id, ip));
            }
        }
    }
    out
}

/// Inject every fresh synthetic announce into `registry` (accept any
/// relayer — Q26/27). Returns how many were injected.
pub fn inject_fresh(
    registry: &Mutex<DiscoveryRegistry>,
    synthetics: Vec<SyntheticAnnounce>,
    now_ms: i64,
) -> usize {
    let Ok(mut reg) = registry.lock() else {
        return 0;
    };
    let mut n = 0;
    for syn in synthetics {
        if syn.is_fresh(now_ms) {
            reg.inject_synthetic(syn);
            n += 1;
        }
    }
    n
}

// AUD-14 (2026-06-11): the `MeshShuntWorker` struct + its `impl Worker` were
// removed as dead — never instantiated/spawned anywhere (the live SEC-5 shunt
// tick is driven inline by `kdc_host::run_shunt_tick`, which consumes the free
// functions above: `publish_phones` / `collect_synthetic` / `inject_fresh`).

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc; // test-only (the registry fixture)

    fn dev(id: &str, name: &str) -> PublishedDevice {
        PublishedDevice {
            device_id: id.into(),
            device_name: name.into(),
            overlay_ip: None,
        }
    }

    fn dev_ip(id: &str, name: &str, ip: &str) -> PublishedDevice {
        PublishedDevice {
            device_id: id.into(),
            device_name: name.into(),
            overlay_ip: Some(ip.into()),
        }
    }

    #[test]
    fn publish_then_collect_relays_neighbors_not_self() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        publish_phones(root, "pine", &[dev("p1", "Pine Pixel")]).unwrap();
        publish_phones(root, "oak", &[dev("o1", "Oak Phone"), dev("o2", "Oak Tab")]).unwrap();
        // From pine's view, only oak's two devices are synthetic.
        let syn = collect_synthetic(root, "pine", 1_000);
        assert_eq!(syn.len(), 2);
        assert!(syn.iter().all(|s| s.relayed_by == "peer:oak"));
        assert!(syn.iter().any(|s| s.announce.device_id == "o1"));
        // And oak sees pine's one device.
        assert_eq!(collect_synthetic(root, "oak", 1_000).len(), 1);
    }

    #[test]
    fn inject_drops_stale_and_honors_any_relayer() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        publish_phones(root, "oak", &[dev("o1", "Oak")]).unwrap();
        let registry = Arc::new(Mutex::new(DiscoveryRegistry::new()));
        // Fresh: injected (accept-any-relayer — no allowlist).
        let fresh = collect_synthetic(root, "pine", 5_000);
        assert_eq!(inject_fresh(&registry, fresh, 5_000), 1);
        assert_eq!(
            registry.lock().unwrap().relayer_for("o1"),
            Some("peer:oak"),
            "relayed device is attributable to its relayer"
        );
        // Stale: a synthetic stamped 10 min ago is dropped.
        let stale = collect_synthetic(root, "pine", 0);
        let way_later = mde_kdc_proto::discovery::STALE_WINDOW_MS + 1_000;
        assert_eq!(inject_fresh(&registry, stale, way_later), 0);
    }

    #[test]
    fn junk_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(phones_dir(root)).unwrap();
        std::fs::write(phones_dir(root).join("oak.json"), "{{not json").unwrap();
        std::fs::write(phones_dir(root).join("README.txt"), "hi").unwrap();
        assert!(collect_synthetic(root, "pine", 1).is_empty());
        assert_eq!(collect_overlay_directory(root, "pine"), RosterOverlay::default());
    }

    // ── KDC-MESH-2: overlay IPs flow through the roster ──────────────────────

    #[test]
    fn published_device_overlay_ip_is_optional_on_the_wire() {
        // `None` overlay IP is skipped (compact, back-compat); `Some` is carried.
        let bare = dev("d", "n");
        let s = serde_json::to_string(&bare).unwrap();
        assert!(!s.contains("overlay_ip"), "None overlay_ip must not serialize");
        assert_eq!(serde_json::from_str::<PublishedDevice>(&s).unwrap(), bare);
        let withip = dev_ip("d", "n", "10.42.0.7");
        assert!(serde_json::to_string(&withip).unwrap().contains("10.42.0.7"));
    }

    #[test]
    fn parse_roster_accepts_the_legacy_bare_array() {
        // A pre-KDC-MESH-2 file (a bare array of {device_id, device_name}) still
        // parses — the name relay + directory readers stay back-compatible.
        let legacy = r#"[{"device_id":"d1","device_name":"Old Phone"}]"#;
        let roster = parse_roster(legacy).expect("legacy array parses");
        assert!(roster.host_device_id.is_empty() && roster.host_overlay_ip.is_none());
        assert_eq!(roster.devices, vec![dev("d1", "Old Phone")]);
        // A KDC-MESH-2 object round-trips through publish/parse with its IPs.
        let modern = serde_json::to_string(&PublishedRoster {
            host_device_id: "h".into(),
            host_overlay_ip: Some("10.42.0.5".into()),
            devices: vec![dev_ip("p", "P", "10.42.0.9")],
        })
        .unwrap();
        let back = parse_roster(&modern).expect("object parses");
        assert_eq!(back.host_overlay_ip.as_deref(), Some("10.42.0.5"));
        assert_eq!(back.devices[0].overlay_ip.as_deref(), Some("10.42.0.9"));
        // Junk is None.
        assert!(parse_roster("{{not json").is_none());
    }

    #[test]
    fn overlay_directory_resolves_phone_and_host_ips_skipping_self_and_wildcard() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A neighbor publishes its own overlay IP + a phone with an overlay IP,
        // plus a phone with NO overlay IP (honest gate) and a wildcard (refused).
        publish_roster(
            root,
            "oak",
            "oak-dev",
            Some("10.42.0.9".into()),
            &[
                dev_ip("o1", "Oak Phone", "10.42.0.77"),
                dev("o2", "Oak Tab"),
                dev_ip("o3", "Oak Bad", "0.0.0.0"),
            ],
        )
        .unwrap();
        // Our own file carries IPs too but must be skipped (own-row authority).
        publish_roster(
            root,
            "pine",
            "pine-dev",
            Some("10.42.0.5".into()),
            &[dev_ip("p1", "Pine", "10.42.0.55")],
        )
        .unwrap();

        let dir = collect_overlay_directory(root, "pine");
        assert_eq!(
            dir.hosts,
            vec![("oak-dev".to_string(), "10.42.0.9".parse().unwrap())],
            "the neighbor host resolves by overlay IP; our own row is skipped"
        );
        assert_eq!(
            dir.phones,
            vec![("o1".to_string(), "10.42.0.77".parse().unwrap())],
            "the phone with a dialable overlay IP resolves; no-IP + wildcard omitted"
        );
    }
}
