//! Mesh media-server model + peer overlay-IP enumeration.
//!
//! Originally (EPIC-SYNC-APP-CONFIG) this did mesh-peer TCP-probe
//! discovery for app_sync. **MESH-PROBE-7 (2026-05-28) retired that
//! TCP-probe path**: media discovery now reads the shared probe
//! inventory via `probe_nmap::peers_with_service` (one prober — the
//! probe worker — feeds every consumer), so the bespoke
//! `discover`/`scan_probe`/`probe_port`/`dedupe` TCP-probe is gone.
//!
//! What remains is the small shared model both still need:
//!   * [`MediaServer`] + [`server_from_probe`] — the server type
//!     app_sync's config writers consume (app_sync builds these from
//!     probe-inventory `HostService` rows).
//!   * [`peer_overlay_ips`] — enumerate every peer's Nebula overlay IP
//!     from the GFS-replicated `nebula-bundle.json` files; the probe
//!     worker's [`crate::probe_nmap::mesh_targets`] uses this to know
//!     which peers to scan.
//!
//! ## MEDIA-7 — the mesh service registration
//!
//! A `Lighthouse_Media` node ([`crate::worker_role`]'s `Capability::Media`
//! gate) runs the media-registry worker ([`crate::workers::media_registry`]),
//! which publishes its `navidrome` instance into the SAME mesh service
//! registry the other published services use — the replicated QNM-Shared
//! plane every node already reads (`compute-inventory.json`,
//! `running-apps.json`) plus the per-peer Bus topic — so the media service is
//! discoverable mesh-wide. [`MediaRegistration`] is that registry document,
//! [`probe_navidrome`] is the per-instance health probe behind its `health`
//! field, and [`MEDIA_REGISTRY_FILE`] / [`media_registry_topic`] name the
//! registry locations. The live-stream / bucket acceptance (MEDIA-2) is a
//! separate unit; this is the registry-publish + health half.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Minimal projection of `nebula-bundle.json` — we only need the
/// overlay IP. Serde ignores the bundle's other fields (cert PEMs,
/// lighthouses, etc.), so this stays decoupled from
/// [`crate::ca::bundle::NebulaBundle`]'s full shape.
#[derive(Deserialize)]
struct BundleOverlayIp {
    overlay_ip: String,
}

/// Airsonic / Subsonic media-server kind tag.
pub const KIND_AIRSONIC: &str = "airsonic";
/// Jellyfin media-server kind tag.
pub const KIND_JELLYFIN: &str = "jellyfin";

/// Default Airsonic/Subsonic port.
pub const AIRSONIC_PORT: u16 = 4040;
/// Default Jellyfin port.
pub const JELLYFIN_PORT: u16 = 8096;

/// One media server reachable on the mesh. Built by app_sync from a
/// probe-inventory `HostService` row; consumed by app_sync's Sublime
/// Music / Delfin config writers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaServer {
    /// [`KIND_AIRSONIC`] or [`KIND_JELLYFIN`].
    pub kind: String,
    /// Hostname (peer node-id) for display.
    pub host: String,
    /// Resolved overlay IP.
    pub ip: String,
    /// Service port.
    pub port: u16,
}

impl MediaServer {
    /// `http://<ip>:<port>` — mesh-internal; Nebula provides the
    /// trust layer, so plain HTTP over the overlay is intentional.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}:{}", self.ip, self.port)
    }
}

/// Build a [`MediaServer`] from its parts. Pure constructor.
#[must_use]
pub fn server_from_probe(kind: &str, host: &str, ip: &str, port: u16) -> MediaServer {
    MediaServer {
        kind: kind.to_owned(),
        host: host.to_owned(),
        ip: ip.to_owned(),
        port,
    }
}

/// Enumerate every peer's `(node_id, overlay_ip)` from the
/// GFS-replicated nebula bundles under `workgroup_root`. Includes the local
/// peer's own bundle. Missing root or unreadable/malformed bundles are
/// skipped (best-effort). Used by the probe worker
/// ([`crate::probe_nmap::mesh_targets`]) to resolve mesh-peer scan
/// targets.
#[must_use]
pub fn peer_overlay_ips(workgroup_root: &Path) -> Vec<(String, String)> {
    let entries = match std::fs::read_dir(workgroup_root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let Some(node_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let bundle_path = entry.path().join("mackesd").join("nebula-bundle.json");
        let Ok(bytes) = std::fs::read(&bundle_path) else {
            continue;
        };
        let Ok(bundle) = serde_json::from_slice::<BundleOverlayIp>(&bytes) else {
            continue;
        };
        if !bundle.overlay_ip.is_empty() {
            out.push((node_id, bundle.overlay_ip));
        }
    }
    out.sort();
    out
}

// ─────────────────────────────────────────────────────────────────────────
// MEDIA-7 — registering the navidrome/media service into the mesh registry.
// ─────────────────────────────────────────────────────────────────────────

/// Default Navidrome (Subsonic/airsonic-family) port — the pinned localhost
/// port the descriptor probe (`descriptors::MEDIA_PORTS`) and probe-nmap
/// already key media off. The media-registry worker probes + publishes this
/// instance.
pub const NAVIDROME_PORT: u16 = 4533;

/// The registered media service's stable kind tag. A media node always
/// registers `navidrome` (the foundation media service MEDIA-3 spawns);
/// keeping it a constant matches the `WORKER_CAPABILITIES` table's worker
/// name so the gate + the registration speak the same token.
pub const NAVIDROME_KIND: &str = "navidrome";

/// File name a media node mirrors its registration to under its QNM-Shared
/// dir — the SAME replicated registry plane the other published services use
/// (`compute-inventory.json`, `running-apps.json`). Every node reads these to
/// see the fleet's published services.
pub const MEDIA_REGISTRY_FILE: &str = "media-registry.json";

/// Per-instance health budget — localhost answers in microseconds; 200 ms is
/// generous and matches `descriptors::CONNECT_TIMEOUT` so the probe can never
/// stall the worker tick.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// Health of a registered media instance. `up` when the service answers on
/// its port, `down` when it doesn't — the per-instance health field MEDIA-7
/// requires so a consumer reading the registry knows whether the published
/// navidrome is actually serving, not merely declared.
pub const HEALTH_UP: &str = "up";
/// See [`HEALTH_UP`].
pub const HEALTH_DOWN: &str = "down";

/// The Bus topic a media node publishes its registration to:
/// `mesh/services/media/<peer>`. Mirrors the per-peer topic shape the other
/// published services use (`compute/inventory/<peer>`); `<peer>` is the
/// node-id so registrations don't collide.
#[must_use]
pub fn media_registry_topic(peer: &str) -> String {
    format!("mesh/services/media/{peer}")
}

/// One media service registered into the mesh service registry by a
/// `Lighthouse_Media` node — the document MEDIA-7 publishes. Carries the
/// per-instance `health` field ([`HEALTH_UP`] / [`HEALTH_DOWN`]) so a
/// consumer knows whether the published instance is actually serving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRegistration {
    /// Registering node-id (the registry key / topic suffix).
    pub node_id: String,
    /// Service kind — always [`NAVIDROME_KIND`] today.
    pub kind: String,
    /// Port the instance is bound to.
    pub port: u16,
    /// Per-instance health: [`HEALTH_UP`] when the service answers on its
    /// port, else [`HEALTH_DOWN`].
    pub health: String,
}

impl MediaRegistration {
    /// `true` when the registered instance answered its health probe.
    #[must_use]
    pub fn is_up(&self) -> bool {
        self.health == HEALTH_UP
    }
}

/// Probe a localhost port and map the result to the per-instance health
/// string. A successful TCP connect → [`HEALTH_UP`], else [`HEALTH_DOWN`].
/// Pure-ish (only a localhost connect, bounded by [`HEALTH_PROBE_TIMEOUT`]);
/// the same localhost-connect liveness check `descriptors::listening` uses,
/// so a port the descriptor scan reports as a media service is exactly the
/// one this registers as `up`.
#[must_use]
pub fn probe_navidrome(port: u16) -> String {
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    if TcpStream::connect_timeout(&addr, HEALTH_PROBE_TIMEOUT).is_ok() {
        HEALTH_UP.to_owned()
    } else {
        HEALTH_DOWN.to_owned()
    }
}

/// Build this node's media registration from a probed health string. Pure
/// constructor so the registration shape is unit-tested without a socket.
#[must_use]
pub fn registration(node_id: &str, port: u16, health: &str) -> MediaRegistration {
    MediaRegistration {
        node_id: node_id.to_owned(),
        kind: NAVIDROME_KIND.to_owned(),
        port,
        health: health.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_is_http_over_overlay() {
        let s = server_from_probe(KIND_AIRSONIC, "peer-a", "10.42.0.5", AIRSONIC_PORT);
        assert_eq!(s.url(), "http://10.42.0.5:4040");
    }

    #[test]
    fn server_from_probe_sets_fields() {
        let s = server_from_probe(KIND_JELLYFIN, "peer-b", "10.42.0.6", JELLYFIN_PORT);
        assert_eq!(s.kind, KIND_JELLYFIN);
        assert_eq!(s.host, "peer-b");
        assert_eq!(s.ip, "10.42.0.6");
        assert_eq!(s.port, 8096);
    }

    #[test]
    fn peer_overlay_ips_empty_for_missing_root() {
        let out = peer_overlay_ips(Path::new("/nonexistent/qnm/root/xyz"));
        assert!(out.is_empty());
    }

    #[test]
    fn peer_overlay_ips_reads_bundles() {
        let tmp = std::env::temp_dir().join(format!("mde-mediatest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for (peer, ip) in [("peer-a", "10.42.0.5"), ("peer-b", "10.42.0.6")] {
            let dir = tmp.join(peer).join("mackesd");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("nebula-bundle.json"),
                format!(r#"{{"overlay_ip":"{ip}","node_id":"{peer}"}}"#),
            )
            .unwrap();
        }
        let out = peer_overlay_ips(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(
            out,
            vec![
                ("peer-a".to_string(), "10.42.0.5".to_string()),
                ("peer-b".to_string(), "10.42.0.6".to_string()),
            ]
        );
    }

    // ── MEDIA-7: the mesh service registration ──

    #[test]
    fn registry_topic_is_per_peer() {
        assert_eq!(
            media_registry_topic("peer:eagle"),
            "mesh/services/media/peer:eagle"
        );
    }

    #[test]
    fn registration_pins_navidrome_kind_and_round_trips() {
        let reg = registration("peer:eagle", NAVIDROME_PORT, HEALTH_UP);
        assert_eq!(reg.kind, NAVIDROME_KIND);
        assert_eq!(reg.port, 4533);
        assert!(reg.is_up());
        let json = serde_json::to_string(&reg).unwrap();
        // The per-instance health field MEDIA-7 requires is on the wire.
        assert!(json.contains("\"health\":\"up\""));
        assert!(json.contains("\"kind\":\"navidrome\""));
        let back: MediaRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reg);
    }

    #[test]
    fn health_down_when_port_closed() {
        // Port 1 is privileged + unbound in CI → connect fails → down.
        // (No service is started; the probe must degrade to `down`, never
        // hang — the timeout bounds it.)
        assert_eq!(probe_navidrome(1), HEALTH_DOWN);
        let reg = registration("peer:host", NAVIDROME_PORT, &probe_navidrome(1));
        assert!(!reg.is_up());
    }
}
