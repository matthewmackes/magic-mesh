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

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    pub device_id: String,
    pub device_name: String,
}

/// Write this peer's paired devices to its own published file
/// (atomic temp + rename).
///
/// # Errors
/// IO / serialization failures.
pub fn publish_phones(
    workgroup_root: &Path,
    hostname: &str,
    devices: &[PublishedDevice],
) -> std::io::Result<PathBuf> {
    let dir = phones_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hostname}.json"));
    let body = serde_json::to_string_pretty(devices)?;
    let tmp = dir.join(format!(".{hostname}.json.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
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
        let Ok(devices) = serde_json::from_str::<Vec<PublishedDevice>>(&raw) else {
            continue;
        };
        for d in devices {
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

    fn dev(id: &str, name: &str) -> PublishedDevice {
        PublishedDevice {
            device_id: id.into(),
            device_name: name.into(),
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
    }

}
