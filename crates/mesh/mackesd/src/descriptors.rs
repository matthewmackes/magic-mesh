//! PD-2 — the local service-descriptor probe.
//!
//! Gathers what THIS box offers the mesh — remote-access listeners and media services on
//! the pinned port list (L12) — plus the Netdata alarm summary
//! (L15), for the heartbeat to fold into this peer's replicated
//! `PeerRecord` (L13: one cycle, one write). **Every probe is
//! localhost-only; nothing leaves the publishing host** (Q19 — the
//! directory never probes remotely). Every probe is best-effort: a
//! missing binary/daemon yields an empty section, never an error.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::process::Command;
use std::time::Duration;

use mackes_mesh_types::peers::{
    AlarmSummary, MediaService, MeshFsUsage, RemoteAccess, ServiceDescriptors,
};

/// The pinned localhost media-port scan list (L12) — a constant,
/// never user input.
///
/// MEDIA-15 adds `mde-media` on 9600 (= `media_server::MESH_MEDIA_PORT`, kept a
/// literal here because `descriptors` compiles without the `async-services`
/// worker layer): when this node's mesh media server is bound, the probe folds
/// `mde-media` into `descriptors.media` so peers' MEDIA-14 discovery
/// (`media_sources::SERVICE_MESH_PLAYER`) finds this node as a mesh media
/// source. No new advertisement channel is minted — this reuses the heartbeat's
/// existing descriptor probe.
pub const MEDIA_PORTS: [(&str, u16); 5] = [
    ("jellyfin", 8096),
    ("navidrome-airsonic", 4533),
    ("mpd", 6600),
    ("dlna", 8200),
    ("mde-media", 9600),
];

/// Per-port connect budget — localhost answers in microseconds; 200 ms
/// is generous and bounds a fully-closed sweep under 2 s.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);

fn listening(port: u16) -> bool {
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok()
}

/// The full local probe — what the heartbeat publishes.
#[must_use]
pub fn probe_local() -> ServiceDescriptors {
    ServiceDescriptors {
        remote_access: RemoteAccess {
            ssh: listening(22),
            rdp: listening(3389),
            vnc: listening(5900),
        },
        media: probe_media(),
        alarms: probe_netdata_alarms(),
        lan_macs: probe_lan_macs(),
        mesh_fs: probe_mesh_fs(),
    }
}

/// MESHFS-2 — `df` the Mesh-Sync mount (`CANONICAL_QNM_MOUNT`) for this peer's
/// used/avail bytes, published on the heartbeat. `present: false` when the mount
/// is absent or `df` fails — the aggregator skips such peers.
#[must_use]
pub fn probe_mesh_fs() -> MeshFsUsage {
    let mount = std::path::Path::new(crate::CANONICAL_QNM_MOUNT);
    if !mount.is_dir() {
        return MeshFsUsage::default();
    }
    match df_used_avail(mount) {
        Some((used_bytes, avail_bytes)) => MeshFsUsage {
            present: true,
            used_bytes,
            avail_bytes,
        },
        None => MeshFsUsage::default(),
    }
}

/// `df -B1 --output=used,avail <path>` → `(used, avail)` bytes; `None` on failure.
fn df_used_avail(path: &std::path::Path) -> Option<(u64, u64)> {
    let out = Command::new("df")
        .args(["-B1", "--output=used,avail"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let mut nums = body.lines().nth(1)?.split_whitespace();
    let used = nums.next()?.parse::<u64>().ok()?;
    let avail = nums.next()?.parse::<u64>().ok()?;
    Some((used, avail))
}

/// Physical-interface MACs from `/sys/class/net` (PD-12). Physical =
/// has a `device` symlink (filters lo, bridges, veths, tunnels).
#[must_use]
pub fn probe_lan_macs() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut macs: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join("device").exists())
        .filter_map(|e| std::fs::read_to_string(e.path().join("address")).ok())
        .map(|m| m.trim().to_lowercase())
        .filter(|m| m.len() == 17 && m != "00:00:00:00:00:00")
        .collect();
    macs.sort();
    macs.dedup();
    macs
}

/// The pinned-list localhost media scan (L12).
#[must_use]
pub fn probe_media() -> Vec<MediaService> {
    MEDIA_PORTS
        .iter()
        .filter(|(_, port)| listening(*port))
        .map(|(name, port)| MediaService {
            name: (*name).to_string(),
            port: *port,
        })
        .collect()
}

/// Netdata active-alarm summary via a std-only localhost HTTP/1.0 GET
/// (no HTTP-client dep — D-W1). `healthy` with `worst: None` when
/// Netdata is absent/unreachable (an unmonitored box is not thereby
/// degraded).
#[must_use]
pub fn probe_netdata_alarms() -> AlarmSummary {
    let Some(body) = local_http_get(19999, "/api/v1/alarms?active") else {
        return AlarmSummary {
            tier: "healthy".into(),
            worst: None,
        };
    };
    parse_netdata_alarms(&body)
}

/// Parse Netdata's `/api/v1/alarms` reply into the L15 3-tier summary.
#[must_use]
pub fn parse_netdata_alarms(body: &str) -> AlarmSummary {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return AlarmSummary {
            tier: "healthy".into(),
            worst: None,
        };
    };
    let mut tier = "healthy";
    let mut worst: Option<String> = None;
    if let Some(alarms) = v.get("alarms").and_then(|a| a.as_object()) {
        for (name, alarm) in alarms {
            match alarm.get("status").and_then(|s| s.as_str()) {
                Some("CRITICAL") => {
                    tier = "critical";
                    worst = Some(name.clone());
                }
                Some("WARNING") if tier != "critical" => {
                    tier = "degraded";
                    if worst.is_none() {
                        worst = Some(name.clone());
                    }
                }
                _ => {}
            }
        }
    }
    AlarmSummary {
        tier: tier.into(),
        worst,
    }
}

/// Minimal HTTP/1.0 GET against 127.0.0.1:`port` — returns the body.
fn local_http_get(port: u16, path: &str) -> Option<String> {
    use std::io::{Read, Write};
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(800)))
        .ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    raw.split_once("\r\n\r\n").map(|(_, body)| body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netdata_alarm_tiers_lock_l15() {
        let warn = r#"{"alarms":{"disk_fill":{"status":"WARNING"}}}"#;
        let crit = r#"{"alarms":{"disk_fill":{"status":"WARNING"},"oom":{"status":"CRITICAL"}}}"#;
        let none = r#"{"alarms":{}}"#;
        assert_eq!(parse_netdata_alarms(warn).tier, "degraded");
        assert_eq!(
            parse_netdata_alarms(warn).worst.as_deref(),
            Some("disk_fill")
        );
        assert_eq!(parse_netdata_alarms(crit).tier, "critical");
        assert_eq!(parse_netdata_alarms(crit).worst.as_deref(), Some("oom"));
        assert_eq!(parse_netdata_alarms(none).tier, "healthy");
        assert!(parse_netdata_alarms("not json").worst.is_none());
    }

    #[test]
    fn media_scan_list_is_the_pinned_constant() {
        // L12 — the scan list is a constant, never user input; this
        // pin makes adding a port a deliberate reviewed change.
        assert_eq!(
            MEDIA_PORTS.map(|(n, _)| n),
            ["jellyfin", "navidrome-airsonic", "mpd", "dlna", "mde-media"]
        );
    }
}
