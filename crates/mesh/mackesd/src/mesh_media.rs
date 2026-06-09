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

use std::path::Path;

use serde::Deserialize;

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
}
