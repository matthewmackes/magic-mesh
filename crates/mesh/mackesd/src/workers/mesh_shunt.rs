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
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// KDC-MESH-3 (design #5) — the pinned SHA-256 cert fingerprint from THIS
    /// host's TOFU pairing of the phone. Replicated so a neighbor recognizes the
    /// phone WITHOUT re-pairing: the neighbor trusts this pin and enforces it
    /// against the live handshake. The fingerprint is the PUBLIC cert hash (shown
    /// in every TLS handshake), never a secret. Empty ⇒ the row is relayed for
    /// discovery only, not as trust (the honest gate). `#[serde(default)]` +
    /// skip-when-empty keeps the legacy (name-relay) file shape parseable + compact.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint: String,
    /// KDC-MESH-3 (design #5) — Unix-ms when THIS host first paired the phone,
    /// carried with the pin so a synced pairing is attributable (audit). Optional
    /// on the wire (skipped when zero) for back-compat + compactness.
    #[serde(default, skip_serializing_if = "is_zero_ms")]
    pub paired_at_ms: i64,
}

/// serde skip predicate: a zero `paired_at_ms` is omitted from the published JSON
/// (a name-relay-only row), keeping the legacy file shape byte-compatible.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde `skip_serializing_if` needs &T
const fn is_zero_ms(v: &i64) -> bool {
    *v == 0
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
            if let Some(ip) = roster
                .host_overlay_ip
                .as_deref()
                .and_then(parse_dialable_ip)
            {
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

/// One mesh-replicated pairing collected from a neighbor's roster (KDC-MESH-3 #5).
///
/// The phone's trust record (id, name, the origin node's pinned cert fingerprint,
/// when it paired) plus the `origin_host` that owns it. The `kdc_host` worker maps
/// this to a [`mde_kdc_host::MeshPairing`] and folds it into the local store so
/// THIS node recognizes the phone without re-pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedPairing {
    /// The phone's KDE-Connect device id (the shared mesh-wide identity).
    pub device_id: String,
    /// The phone's friendly name as the origin host recorded it.
    pub device_name: String,
    /// The origin node's pinned SHA-256 cert fingerprint (non-empty — the honest
    /// gate drops pin-less rows before they reach here).
    pub fingerprint: String,
    /// Unix-ms when the origin node first paired the phone (audit).
    pub paired_at_ms: i64,
    /// The mesh host that owns (TOFU-paired) this phone.
    pub origin_host: String,
}

/// KDC-MESH-3 (design #5) — collect the mesh-wide PAIRINGS from every neighbor's
/// published roster.
///
/// Reads each neighbor file (skipping our own — own-row authority) and returns
/// every relayed phone that carries a real pinned fingerprint. A row without a
/// fingerprint is a name/discovery relay, NOT a trust record, and is honestly
/// omitted here (it never makes a node fake trust). The caller feeds the result
/// to [`PairingStore::replace_synced`], so a phone paired on peer-A is recognized
/// on peer-B once A's roster (with its pin) has replicated — and stops being
/// recognized once it leaves the substrate. Junk / half-replicated files are
/// skipped, like every other replicated reader.
///
/// [`PairingStore::replace_synced`]: mde_kdc_host::PairingStore::replace_synced
#[must_use]
pub fn collect_pairings(workgroup_root: &Path, self_hostname: &str) -> Vec<CollectedPairing> {
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
        // Own-row authority: never fold our own published pairings back in.
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
            // Honest gate: only a device carrying a real pin is a trusted pairing.
            if d.fingerprint.is_empty() {
                continue;
            }
            out.push(CollectedPairing {
                device_id: d.device_id,
                device_name: d.device_name,
                fingerprint: d.fingerprint,
                paired_at_ms: d.paired_at_ms,
                origin_host: stem.to_string(),
            });
        }
    }
    out
}

// ── KDC-MESH-5: the replicated phone-notification relay ──────────────────────
//
// Design #6/#9: a phone notification must appear on EVERY node's desktop notify
// feed, not just the node the phone happens to be connected to. The receiving
// node writes the notification into its own row of a replicated relay dir
// (`<root>/kdc-notify/<hostname>.json`, own-row authority — same substrate the
// phone roster rides); every peer reads its neighbors' rows each shunt tick and
// republishes any it hasn't seen onto its LOCAL `event/notify/phone` bus lane (the
// CHAT-FIX-2 producer lane the chat worker folds into the desktop feed). The
// per-node de-dup (a bounded seen-set in `kdc_host`) keeps one phone notification
// from becoming N toasts on a single desktop even when the phone is connected to
// several nodes at once. Overlay-carried: the relay dir replicates over the Nebula
// overlay via Syncthing (SUBSTRATE-V2), never a public port.

/// The replicated directory holding every peer's relayed phone notifications.
#[must_use]
pub fn notify_relay_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("kdc-notify")
}

/// One relayed phone notification (the JSON shape on the volume).
///
/// Carries the de-dup `key`, the phone identity, the pre-rendered feed fields, and
/// the `origin_host` (the node that received it from the phone) + `ts_ms`
/// (freshness).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RelayedNotification {
    /// The mesh-wide de-dup key — [`notify_relay_key`] of the phone id + the
    /// KDE-Connect notification id + the cancel flag.
    pub key: String,
    /// The phone's KDE-Connect device id (the paired-once mesh identity).
    pub phone_id: String,
    /// The phone's friendly name (for the feed line).
    pub phone_name: String,
    /// The emitting Android app (`appName`).
    pub app_name: String,
    /// The pre-rendered one-line feed summary (`"App: text"`).
    pub summary: String,
    /// The chat severity tag (`"info"`/`"warning"`) so the fold classifies it.
    pub severity: String,
    /// The mesh host that received the notification from the phone.
    pub origin_host: String,
    /// Unix-ms when the origin host received it (the freshness stamp).
    pub ts_ms: i64,
}

/// The mesh-wide de-dup key for a phone notification.
///
/// The phone id + the KDE-Connect notification id + whether it's a cancel. Two
/// nodes that both receive the same phone notification compute the same key, so
/// each republishes it onto its own feed exactly once.
#[must_use]
pub fn notify_relay_key(phone_id: &str, notif_id: &str, is_cancel: bool) -> String {
    format!(
        "{phone_id}:{notif_id}:{}",
        if is_cancel { "c" } else { "n" }
    )
}

/// Append one relayed notification to THIS host's own relay row.
///
/// Own-row authority — only this box writes `<hostname>.json`. Idempotent per `key`
/// (a re-received notification refreshes its stamp rather than duplicating), and
/// bounded to the newest `cap` entries so the row can't grow without limit. Atomic
/// temp + rename, like [`publish_roster`].
///
/// # Errors
/// IO / serialization failures.
pub fn append_notify_relay(
    workgroup_root: &Path,
    hostname: &str,
    entry: &RelayedNotification,
    cap: usize,
) -> std::io::Result<PathBuf> {
    let dir = notify_relay_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hostname}.json"));
    let mut entries: Vec<RelayedNotification> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    // Idempotent per key: drop a prior copy, then push the fresh one.
    entries.retain(|e| e.key != entry.key);
    entries.push(entry.clone());
    // Bound to the newest `cap` by timestamp.
    if entries.len() > cap.max(1) {
        entries.sort_by_key(|e| e.ts_ms);
        let start = entries.len() - cap.max(1);
        entries.drain(0..start);
    }
    let body = serde_json::to_string_pretty(&entries)?;
    let tmp = dir.join(format!(".{hostname}.json.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read every neighbor's relayed notifications (skipping our own row).
///
/// Own-row authority; keeps only entries fresher than `stale_ms` so a rejoining
/// node doesn't replay ancient notifications. Junk / half-replicated files are
/// skipped, like every other replicated reader. The caller de-dups by `key`
/// against its seen-set before republishing each onto its local feed.
#[must_use]
pub fn collect_notify_relay(
    workgroup_root: &Path,
    self_hostname: &str,
    now_ms: i64,
    stale_ms: i64,
) -> Vec<RelayedNotification> {
    let Ok(entries) = std::fs::read_dir(notify_relay_dir(workgroup_root)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if stem == self_hostname || path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(list) = serde_json::from_str::<Vec<RelayedNotification>>(&raw) else {
            continue;
        };
        for n in list {
            if now_ms.saturating_sub(n.ts_ms) <= stale_ms {
                out.push(n);
            }
        }
    }
    out
}

/// Every de-dup key currently present in the relay dir (own row + neighbors').
///
/// The `kdc_host` worker primes its seen-set with these at startup so a restart
/// doesn't re-toast notifications already on the substrate.
#[must_use]
pub fn all_notify_relay_keys(workgroup_root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(notify_relay_dir(workgroup_root)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(list) = serde_json::from_str::<Vec<RelayedNotification>>(&raw) {
            out.extend(list.into_iter().map(|n| n.key));
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
            ..Default::default()
        }
    }

    fn dev_ip(id: &str, name: &str, ip: &str) -> PublishedDevice {
        PublishedDevice {
            device_id: id.into(),
            device_name: name.into(),
            overlay_ip: Some(ip.into()),
            ..Default::default()
        }
    }

    /// KDC-MESH-3 — a published device carrying the pinned fingerprint (a real
    /// mesh-wide pairing, not just a name/discovery relay).
    fn dev_paired(id: &str, name: &str, fp: &str) -> PublishedDevice {
        PublishedDevice {
            device_id: id.into(),
            device_name: name.into(),
            fingerprint: fp.into(),
            paired_at_ms: 100,
            ..Default::default()
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
        assert_eq!(
            collect_overlay_directory(root, "pine"),
            RosterOverlay::default()
        );
    }

    // ── KDC-MESH-2: overlay IPs flow through the roster ──────────────────────

    #[test]
    fn published_device_overlay_ip_is_optional_on_the_wire() {
        // `None` overlay IP is skipped (compact, back-compat); `Some` is carried.
        let bare = dev("d", "n");
        let s = serde_json::to_string(&bare).unwrap();
        assert!(
            !s.contains("overlay_ip"),
            "None overlay_ip must not serialize"
        );
        assert_eq!(serde_json::from_str::<PublishedDevice>(&s).unwrap(), bare);
        let withip = dev_ip("d", "n", "10.42.0.7");
        assert!(serde_json::to_string(&withip)
            .unwrap()
            .contains("10.42.0.7"));
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

    // ── KDC-MESH-3 (#5): the pairing pin replicates through the roster ───────

    #[test]
    fn published_device_carries_the_pin_optionally_on_the_wire() {
        // A name-relay row (no pin) omits both KDC-MESH-3 fields (compact,
        // back-compat); a paired row carries the fingerprint + paired_at.
        let bare = dev("d", "n");
        let s = serde_json::to_string(&bare).unwrap();
        assert!(!s.contains("fingerprint"), "empty pin must not serialize");
        assert!(
            !s.contains("paired_at_ms"),
            "zero paired_at must not serialize"
        );
        assert_eq!(serde_json::from_str::<PublishedDevice>(&s).unwrap(), bare);

        let paired = dev_paired("d", "n", "AA:BB:CC");
        let sp = serde_json::to_string(&paired).unwrap();
        assert!(sp.contains("AA:BB:CC") && sp.contains("paired_at_ms"));
        assert_eq!(
            serde_json::from_str::<PublishedDevice>(&sp).unwrap(),
            paired
        );
    }

    #[test]
    fn collect_pairings_returns_neighbor_pins_and_honest_gates_the_pinless() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A neighbor (oak) publishes one PAIRED phone (with a pin) + one name-relay
        // phone (no pin). Only the pinned one is a trust record.
        publish_phones(
            root,
            "oak",
            &[
                dev_paired("o1", "Oak Phone", "AA:BB:CC"),
                dev("o2", "Oak Tab"),
            ],
        )
        .unwrap();
        // Our own file carries a pin too but must be skipped (own-row authority).
        publish_phones(root, "pine", &[dev_paired("p1", "Pine", "DD:EE:FF")]).unwrap();

        let pairings = collect_pairings(root, "pine");
        assert_eq!(
            pairings.len(),
            1,
            "only oak's pinned phone is a trusted pairing"
        );
        let p = &pairings[0];
        assert_eq!(p.device_id, "o1");
        assert_eq!(p.fingerprint, "AA:BB:CC");
        assert_eq!(p.paired_at_ms, 100);
        assert_eq!(p.origin_host, "oak", "recognition is attributable to oak");
        // The pin-less o2 is honestly omitted (discovery relay, not trust); our own
        // p1 is never folded back.
        assert!(!pairings.iter().any(|c| c.device_id == "o2"));
        assert!(!pairings.iter().any(|c| c.device_id == "p1"));
    }

    #[test]
    fn collect_pairings_is_empty_without_neighbors() {
        // A fresh mesh with no neighbor rosters yields no synced pairings — the
        // honest gate at the collection layer (a node recognizes nothing it hasn't
        // synced).
        let tmp = tempfile::tempdir().unwrap();
        assert!(collect_pairings(tmp.path(), "pine").is_empty());
    }

    // ── KDC-MESH-5: the replicated phone-notification relay ──────────────────

    fn relayed(key: &str, origin: &str, ts: i64) -> RelayedNotification {
        RelayedNotification {
            key: key.into(),
            phone_id: "moto".into(),
            phone_name: "Moto".into(),
            app_name: "Signal".into(),
            summary: "Signal: new message".into(),
            severity: "info".into(),
            origin_host: origin.into(),
            ts_ms: ts,
        }
    }

    #[test]
    fn notify_relay_key_distinguishes_cancel_from_show() {
        assert_eq!(notify_relay_key("moto", "n1", false), "moto:n1:n");
        assert_ne!(
            notify_relay_key("moto", "n1", false),
            notify_relay_key("moto", "n1", true),
            "a cancel and a show of the same notification are distinct keys"
        );
    }

    #[test]
    fn append_notify_relay_is_idempotent_per_key_and_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Re-appending the same key refreshes rather than duplicates.
        append_notify_relay(root, "oak", &relayed("k1", "oak", 10), 4).unwrap();
        append_notify_relay(root, "oak", &relayed("k1", "oak", 20), 4).unwrap();
        let raw = std::fs::read_to_string(notify_relay_dir(root).join("oak.json")).unwrap();
        let list: Vec<RelayedNotification> = serde_json::from_str(&raw).unwrap();
        assert_eq!(list.len(), 1, "same key is idempotent");
        assert_eq!(list[0].ts_ms, 20, "the newer stamp wins");
        // A flood past the cap keeps only the newest `cap` entries.
        for i in 0..10 {
            append_notify_relay(root, "oak", &relayed(&format!("f{i}"), "oak", 100 + i), 4)
                .unwrap();
        }
        let raw = std::fs::read_to_string(notify_relay_dir(root).join("oak.json")).unwrap();
        let list: Vec<RelayedNotification> = serde_json::from_str(&raw).unwrap();
        assert!(list.len() <= 4, "relay row is bounded to the cap");
    }

    #[test]
    fn collect_notify_relay_reads_neighbors_skips_self_and_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A neighbor (oak) relays a fresh + a stale notification.
        append_notify_relay(root, "oak", &relayed("fresh", "oak", 9_000), 16).unwrap();
        append_notify_relay(root, "oak", &relayed("stale", "oak", 1_000), 16).unwrap();
        // Our own row must never be relayed back to us.
        append_notify_relay(root, "pine", &relayed("mine", "pine", 9_500), 16).unwrap();

        // now=10_000, stale window=5_000 → only oak's fresh entry survives.
        let got = collect_notify_relay(root, "pine", 10_000, 5_000);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "fresh");
        assert_eq!(got[0].origin_host, "oak");
    }

    #[test]
    fn all_notify_relay_keys_covers_every_row_for_the_startup_prime() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        append_notify_relay(root, "oak", &relayed("a", "oak", 1), 8).unwrap();
        append_notify_relay(root, "pine", &relayed("b", "pine", 2), 8).unwrap();
        let mut keys = all_notify_relay_keys(root);
        keys.sort();
        assert_eq!(keys, vec!["a".to_string(), "b".to_string()]);
        // Empty dir → no keys (no panic).
        let empty = tempfile::tempdir().unwrap();
        assert!(all_notify_relay_keys(empty.path()).is_empty());
    }
}
