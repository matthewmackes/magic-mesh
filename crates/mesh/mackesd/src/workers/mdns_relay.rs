//! MESH-MDNS-RELAY — native cross-LAN-segment mDNS service relay.
//!
//! Rebuilds (operator decision, 2026-06-05) the v1.x `mackes/mdns_relay.py`
//! relay natively, with **no python and no avahi shell-outs** — `mdns-sd` does
//! both the local browse and the LAN republish.
//!
//! On each peer:
//!   1. **Browse** the local LAN for the curated relayed service types
//!      (`_jellyfin._tcp`, `_googlecast._tcp`, …) via `mdns_sd::ServiceDaemon`.
//!   2. **Publish** each discovered local service to the `mesh/mdns/announce`
//!      Bus topic as an [`MdnsAnnounce`] tagged with this peer's mesh IP.
//!   3. **Republish (inbound half)** — poll the announce topic for other
//!      peers' services and republish them on the LOCAL LAN, substituting
//!      the originating peer's mesh IP for the source LAN IP (see the
//!      INBOUND block in `run_relay_blocking`; landed, no longer a
//!      follow-up).
//!
//! **Anti-loop:** republished services carry an `mde-relay-origin` TXT record;
//! the browse step skips anything carrying it, so a relayed service is never
//! re-relayed. Each announce is tagged with its origin peer, and the inbound
//! half drops announces whose origin is ourselves.
//!
//! **Type policy (v1.x §9 lock):** only the media/discovery allowlist is
//! relayed; the privacy-sensitive types (ssh / smb / printers) never are.
//!
//! **Graceful degrade:** no `nebula1` interface (pre-enrolment) or no
//! multicast-capable interface → the worker idles until shutdown, never panics.

#![cfg(feature = "async-services")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

/// Bus topic every peer writes its discovered local services to. Readers
/// filter by origin (`peer != self`) and republish on their own LAN.
pub const ANNOUNCE_TOPIC: &str = "mesh/mdns/announce";

/// TXT key marking a service WE republished from a peer — the browse step
/// skips these so a relayed service is never re-relayed (anti-loop).
pub const RELAY_ORIGIN_TXT: &str = "mde-relay-origin";

/// Idle sleep between browse-drain passes when no events are pending.
const IDLE_SLEEP: Duration = Duration::from_millis(500);

/// Service types relayed by default (v1.x §9 lock — media + discovery).
pub const RELAYED_TYPES: &[&str] = &[
    "_jellyfin._tcp",
    "_googlecast._tcp",
    "_airplay._tcp",
    "_spotify-connect._tcp",
    "_home-assistant._tcp",
    "_syncthing._tcp",
    "_netdata._tcp",
    "_subsonic._tcp",
];

/// Service types NEVER relayed (privacy — printers, file shares, ssh).
pub const PRIVATE_TYPES: &[&str] = &[
    "_ipp._tcp",
    "_pdl-datastream._tcp",
    "_smb._tcp",
    "_afpovertcp._tcp",
    "_ssh._tcp",
];

/// A relayed service announce — the JSON body that crosses the Bus to peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MdnsAnnounce {
    /// Origin peer's mesh IP (anti-loop key + the host clients connect to).
    pub peer: String,
    /// mDNS instance name (e.g. `Jellyfin Media Server`).
    pub service: String,
    /// Bare service type (e.g. `_jellyfin._tcp`).
    pub service_type: String,
    /// Advertised port.
    pub port: u16,
    /// TXT records (key, value) — forwarded for client compatibility.
    pub txt: Vec<(String, String)>,
}

/// The mdns-sd browse string for a bare type (`_jellyfin._tcp` →
/// `_jellyfin._tcp.local.`).
fn browse_type(bare: &str) -> String {
    format!("{bare}.local.")
}

/// True when `service_type` is on the relayed allowlist (and not private).
///
/// Accepts a bare type, a `.local.`-qualified type, or a fullname —
/// `_jellyfin._tcp.local.` / `_ssh._tcp` / `Name._airplay._tcp.local.` all
/// resolve to their bare type first.
#[must_use]
pub fn is_relayed(service_type: &str) -> bool {
    let base = bare_type(service_type);
    !PRIVATE_TYPES.contains(&base.as_str()) && RELAYED_TYPES.contains(&base.as_str())
}

/// Extract the trailing `_proto._tcp`/`_udp` token from a type string or
/// fullname, stripping a trailing `.local.` domain.
fn bare_type(s: &str) -> String {
    let s = s.trim_end_matches('.');
    let s = s.strip_suffix(".local").unwrap_or(s);
    // The bare type is the last two dot-separated tokens (`_x._tcp`); a
    // fullname (`Name._x._tcp`) has the instance before them.
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() >= 2 {
        let last2 = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        if (last2.ends_with("._tcp") || last2.ends_with("._udp")) && last2.starts_with('_') {
            return last2;
        }
    }
    s.to_string()
}

/// The instance name from a resolved service's fullname, stripping the
/// `.<type>.local.` suffix.
fn instance_name(info: &ServiceInfo, bare: &str) -> String {
    let full = info.get_fullname();
    full.strip_suffix(&format!(".{}", browse_type(bare)))
        .unwrap_or(full)
        .trim_end_matches('.')
        .to_string()
}

/// Build an [`MdnsAnnounce`] from a resolved local service, or `None` when it
/// shouldn't be relayed.
///
/// Skips non-allowlisted types and any service WE republished (it carries
/// [`RELAY_ORIGIN_TXT`]) — the anti-loop guard. `own_ip` is this peer's mesh IP,
/// stamped as the announce origin + the host clients connect to.
#[must_use]
pub fn announce_from_info(
    bare_type: &str,
    info: &ServiceInfo,
    own_ip: &str,
) -> Option<MdnsAnnounce> {
    if !is_relayed(bare_type) {
        return None;
    }
    if info.get_property_val_str(RELAY_ORIGIN_TXT).is_some() {
        return None; // already a relayed service — don't loop it back
    }
    let txt: Vec<(String, String)> = info
        .get_properties()
        .iter()
        .map(|p| (p.key().to_string(), p.val_str().to_string()))
        .collect();
    Some(MdnsAnnounce {
        peer: own_ip.to_string(),
        service: instance_name(info, bare_type),
        service_type: bare_type.to_string(),
        port: info.get_port(),
        txt,
    })
}

/// Publish an announce to the Bus (best-effort; absent Persist = no-op).
fn publish_announce(persist: Option<&Persist>, ann: &MdnsAnnounce) {
    let Some(p) = persist else { return };
    if let Ok(body) = serde_json::to_string(ann) {
        let _ = p.write(ANNOUNCE_TOPIC, Priority::Default, None, Some(&body));
    }
}

/// Peer-suffixed instance name for a republished service — avoids colliding
/// with the peer's own LAN advertisement and with other peers' services.
fn republish_name(ann: &MdnsAnnounce) -> String {
    format!("{}-{}", ann.service, ann.peer.replace('.', "-"))
}

/// Dedup key for an inbound announce (origin peer + type + instance).
fn service_key(ann: &MdnsAnnounce) -> String {
    format!("{}|{}|{}", ann.peer, ann.service_type, ann.service)
}

/// Build the `ServiceInfo` to register a peer's service on the LOCAL LAN:
/// advertised at the peer's **mesh IP** (so LAN clients connect over the
/// overlay), peer-suffixed instance name, carrying the [`RELAY_ORIGIN_TXT`] tag
/// so our own browse skips it (anti-loop). `None` when `peer` isn't a valid IP.
fn build_republish_info(ann: &MdnsAnnounce) -> Option<ServiceInfo> {
    let ip: std::net::IpAddr = ann.peer.parse().ok()?;
    let instance = republish_name(ann);
    let hostname = format!("{instance}.local.");
    let mut txt = ann.txt.clone();
    txt.push((RELAY_ORIGIN_TXT.to_string(), ann.peer.clone()));
    let txt_refs: Vec<(&str, &str)> = txt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    ServiceInfo::new(
        &browse_type(&ann.service_type),
        &instance,
        &hostname,
        ip,
        ann.port,
        &txt_refs[..],
    )
    .ok()
}

/// This host's mesh IP (`nebula1`), or `None` pre-enrolment.
fn own_mesh_ip() -> Option<String> {
    crate::voip_rtt::own_nebula_ip()
}

/// The relay loop (blocking). Each pass does BOTH halves: the **outbound** half
/// drains the mDNS browsers and publishes discovered local services to the Bus;
/// the **inbound** half polls the Bus for peers' announces and registers them on
/// the local LAN (at the peer's mesh IP). Runs until `stop` is set. Idles
/// gracefully when there's no mesh IP yet or no multicast-capable interface.
fn run_relay_blocking(stop: &AtomicBool) {
    let Some(own_ip) = own_mesh_ip() else {
        tracing::info!("mdns_relay: no nebula1 mesh IP (pre-enrolment); relay idle");
        wait_until_stop(stop);
        return;
    };
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "mdns_relay: no mDNS daemon; relay idle");
            wait_until_stop(stop);
            return;
        }
    };
    let persist = mde_bus::default_data_dir().and_then(|d| Persist::open(d).ok());

    let mut browsers = Vec::new();
    for bare in RELAYED_TYPES {
        match daemon.browse(&browse_type(bare)) {
            Ok(rx) => browsers.push((*bare, rx)),
            Err(e) => tracing::warn!(error = %e, service_type = bare, "mdns_relay: browse failed"),
        }
    }

    // Inbound republish state: a cursor over the announce topic + the set of
    // already-registered service keys (a peer service is registered once).
    let mut cursor: Option<String> = None;
    let mut registered: std::collections::HashSet<String> = std::collections::HashSet::new();

    while !stop.load(Ordering::Relaxed) {
        let mut got_any = false;

        // OUTBOUND — drain every browser, publish local services.
        for (bare, rx) in &browsers {
            while let Ok(event) = rx.try_recv() {
                got_any = true;
                if let ServiceEvent::ServiceResolved(info) = event {
                    if let Some(ann) = announce_from_info(bare, &info, &own_ip) {
                        publish_announce(persist.as_ref(), &ann);
                    }
                }
            }
        }

        // INBOUND — poll the Bus for peers' announces, republish locally.
        if let Some(p) = persist.as_ref() {
            if let Ok(msgs) = p.list_since(ANNOUNCE_TOPIC, cursor.as_deref()) {
                for msg in msgs {
                    got_any = true;
                    cursor = Some(msg.ulid.clone());
                    let Some(body) = msg.body.as_deref() else {
                        continue;
                    };
                    let Ok(ann) = serde_json::from_str::<MdnsAnnounce>(body) else {
                        continue;
                    };
                    if ann.peer == own_ip {
                        continue; // anti-loop: our own announce
                    }
                    if registered.insert(service_key(&ann)) {
                        if let Some(info) = build_republish_info(&ann) {
                            if let Err(e) = daemon.register(info) {
                                tracing::warn!(error = %e, service = %ann.service, "mdns_relay: republish failed");
                            }
                        }
                    }
                }
            }
        }

        if !got_any {
            std::thread::sleep(IDLE_SLEEP);
        }
    }
}

/// Park the thread until `stop` is set (the graceful-degrade idle path).
fn wait_until_stop(stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(IDLE_SLEEP);
    }
}

/// Supervised worker: runs the outbound relay on a blocking thread, stopping it
/// when the supervisor signals shutdown.
pub struct MdnsRelayWorker;

impl Default for MdnsRelayWorker {
    fn default() -> Self {
        Self
    }
}

impl MdnsRelayWorker {
    /// Construct the relay worker.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Worker for MdnsRelayWorker {
    fn name(&self) -> &'static str {
        "mdns_relay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = tokio::task::spawn_blocking(move || run_relay_blocking(&stop2));
        shutdown.wait().await;
        stop.store(true, Ordering::Relaxed);
        let _ = handle.await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_relayed_allows_media_types() {
        assert!(is_relayed("_jellyfin._tcp"));
        assert!(is_relayed("_googlecast._tcp"));
        assert!(is_relayed("_subsonic._tcp.local."));
    }

    #[test]
    fn is_relayed_rejects_private_and_unknown() {
        assert!(!is_relayed("_ssh._tcp"));
        assert!(!is_relayed("_smb._tcp"));
        assert!(!is_relayed("_ipp._tcp.local."));
        assert!(!is_relayed("_http._tcp"));
    }

    #[test]
    fn bare_type_reduces_fullnames_and_domains() {
        assert_eq!(bare_type("_jellyfin._tcp.local."), "_jellyfin._tcp");
        assert_eq!(
            bare_type("Living Room._airplay._tcp.local."),
            "_airplay._tcp"
        );
        assert_eq!(bare_type("_ssh._tcp"), "_ssh._tcp");
    }

    #[test]
    fn announce_round_trips_through_json() {
        let ann = MdnsAnnounce {
            peer: "10.42.0.3".into(),
            service: "Jellyfin".into(),
            service_type: "_jellyfin._tcp".into(),
            port: 8096,
            txt: vec![("Path".into(), "/web".into())],
        };
        let body = serde_json::to_string(&ann).unwrap();
        let back: MdnsAnnounce = serde_json::from_str(&body).unwrap();
        assert_eq!(ann, back);
    }

    fn svc(bare: &str, instance: &str, port: u16, txt: &[(&str, &str)]) -> ServiceInfo {
        ServiceInfo::new(
            &browse_type(bare),
            instance,
            &format!("{instance}.local."),
            "192.168.1.50",
            port,
            txt,
        )
        .unwrap()
    }

    #[test]
    fn announce_from_info_lifts_a_relayed_service() {
        let info = svc("_jellyfin._tcp", "Jellyfin", 8096, &[("Path", "/web")]);
        let ann = announce_from_info("_jellyfin._tcp", &info, "10.42.0.3").unwrap();
        assert_eq!(ann.peer, "10.42.0.3"); // origin = our mesh IP, not the LAN IP
        assert_eq!(ann.service, "Jellyfin");
        assert_eq!(ann.service_type, "_jellyfin._tcp");
        assert_eq!(ann.port, 8096);
        assert!(ann.txt.iter().any(|(k, v)| k == "Path" && v == "/web"));
    }

    #[test]
    fn announce_from_info_skips_non_relayed_types() {
        let info = svc("_ssh._tcp", "shell", 22, &[]);
        assert!(announce_from_info("_ssh._tcp", &info, "10.42.0.3").is_none());
    }

    #[test]
    fn announce_from_info_skips_our_own_relayed_services_anti_loop() {
        // A service WE republished carries the relay-origin TXT — don't loop it.
        let info = svc(
            "_jellyfin._tcp",
            "Jellyfin-peerB",
            8096,
            &[(RELAY_ORIGIN_TXT, "10.42.0.9")],
        );
        assert!(announce_from_info("_jellyfin._tcp", &info, "10.42.0.3").is_none());
    }

    fn ann(peer: &str, service: &str, ty: &str, port: u16) -> MdnsAnnounce {
        MdnsAnnounce {
            peer: peer.into(),
            service: service.into(),
            service_type: ty.into(),
            port,
            txt: vec![],
        }
    }

    #[test]
    fn republish_name_is_peer_suffixed_and_collision_safe() {
        let a = ann("10.42.0.9", "Jellyfin", "_jellyfin._tcp", 8096);
        assert_eq!(republish_name(&a), "Jellyfin-10-42-0-9");
    }

    #[test]
    fn service_key_distinguishes_peer_type_instance() {
        let a = ann("10.42.0.9", "Jellyfin", "_jellyfin._tcp", 8096);
        let b = ann("10.42.0.8", "Jellyfin", "_jellyfin._tcp", 8096);
        assert_ne!(service_key(&a), service_key(&b)); // different peer
        assert_eq!(service_key(&a), service_key(&a)); // stable
    }

    #[test]
    fn build_republish_info_advertises_peer_mesh_ip_and_origin_tag() {
        let a = ann("10.42.0.9", "Jellyfin", "_jellyfin._tcp", 8096);
        let info = build_republish_info(&a).expect("valid mesh IP");
        assert_eq!(info.get_port(), 8096);
        // peer-suffixed instance name + the relay-origin TXT (anti-loop).
        assert!(info.get_fullname().starts_with("Jellyfin-10-42-0-9."));
        assert_eq!(
            info.get_property_val_str(RELAY_ORIGIN_TXT).as_deref(),
            Some("10.42.0.9")
        );
        // advertised at the peer's mesh IP, not our LAN address.
        assert!(info
            .get_addresses()
            .iter()
            .any(|ip| ip.to_string() == "10.42.0.9"));
    }

    #[test]
    fn build_republish_info_rejects_a_non_ip_peer() {
        let a = ann("not-an-ip", "Jellyfin", "_jellyfin._tcp", 8096);
        assert!(build_republish_info(&a).is_none());
    }
}
