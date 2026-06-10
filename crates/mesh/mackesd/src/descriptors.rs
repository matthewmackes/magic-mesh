//! PD-2 — the local service-descriptor probe.
//!
//! Gathers what THIS box offers the mesh — remote-access listeners,
//! Podman containers (L10), libvirt guests (L11), media services on
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
    AlarmSummary, ContainerInfo, MediaService, RemoteAccess, ServiceDescriptors, VmInfo,
};

/// The pinned localhost media-port scan list (L12) — a constant,
/// never user input.
pub const MEDIA_PORTS: [(&str, u16); 4] = [
    ("jellyfin", 8096),
    ("navidrome-airsonic", 4533),
    ("mpd", 6600),
    ("dlna", 8200),
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
        containers: probe_podman(),
        vms: probe_libvirt(),
        media: probe_media(),
        alarms: probe_netdata_alarms(),
        lan_macs: probe_lan_macs(),
    }
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

/// `podman ps --all --format json` → L10 rows. Empty without podman.
#[must_use]
pub fn probe_podman() -> Vec<ContainerInfo> {
    let Ok(out) = Command::new("podman")
        .args(["ps", "--all", "--format", "json"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_podman_ps(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `podman ps --format json` output (pure, testable).
#[must_use]
pub fn parse_podman_ps(raw: &str) -> Vec<ContainerInfo> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let name = c
                        .get("Names")
                        .and_then(|n| n.as_array())
                        .and_then(|a| a.first())
                        .and_then(|s| s.as_str())?
                        .to_string();
                    let image = c
                        .get("Image")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let state = c
                        .get("State")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let ports = c
                        .get("Ports")
                        .and_then(|p| p.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|p| {
                                    let host = p.get("host_port").and_then(|v| v.as_u64())?;
                                    let cont = p.get("container_port").and_then(|v| v.as_u64())?;
                                    let proto =
                                        p.get("protocol").and_then(|v| v.as_str()).unwrap_or("tcp");
                                    Some(format!("{host}->{cont}/{proto}"))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(ContainerInfo {
                        name,
                        image,
                        state,
                        ports,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `virsh list --all` (+ best-effort dominfo / agent addresses) → L11
/// rows. Empty without libvirt.
#[must_use]
pub fn probe_libvirt() -> Vec<VmInfo> {
    let Ok(out) = Command::new("virsh").args(["-q", "list", "--all"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut vms = parse_virsh_list(&String::from_utf8_lossy(&out.stdout));
    for vm in &mut vms {
        // Specs (L11) — best-effort; absent on any failure.
        if let Ok(info) = Command::new("virsh")
            .args(["-q", "dominfo", &vm.name])
            .output()
        {
            if info.status.success() {
                let (vcpus, mem) = parse_dominfo(&String::from_utf8_lossy(&info.stdout));
                vm.vcpus = vcpus;
                vm.memory_mb = mem;
            }
        }
        // Agent addresses — only meaningful for running guests.
        if vm.state == "running" {
            if let Ok(ifaddr) = Command::new("virsh")
                .args(["-q", "domifaddr", "--source", "agent", &vm.name])
                .output()
            {
                if ifaddr.status.success() {
                    vm.addresses = parse_domifaddr(&String::from_utf8_lossy(&ifaddr.stdout));
                }
            }
        }
    }
    vms
}

/// Parse `virsh -q list --all` lines: ` <id|-> <name> <state...>`.
#[must_use]
pub fn parse_virsh_list(raw: &str) -> Vec<VmInfo> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _id = parts.next()?;
            let name = parts.next()?.to_string();
            let state = parts.collect::<Vec<_>>().join(" ");
            if name.is_empty() || state.is_empty() {
                return None;
            }
            Some(VmInfo {
                name,
                state,
                vcpus: None,
                memory_mb: None,
                addresses: Vec::new(),
            })
        })
        .collect()
}

/// Parse `virsh dominfo` for `CPU(s)` + `Max memory` (KiB → MiB).
#[must_use]
pub fn parse_dominfo(raw: &str) -> (Option<u32>, Option<u64>) {
    let mut vcpus = None;
    let mut mem = None;
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("CPU(s):") {
            vcpus = v.trim().parse().ok();
        }
        if let Some(v) = line.strip_prefix("Max memory:") {
            let kib: Option<u64> = v.trim().trim_end_matches(" KiB").trim().parse().ok();
            mem = kib.map(|k| k / 1024);
        }
    }
    (vcpus, mem)
}

/// Parse `virsh domifaddr --source agent` for ipv4/ipv6 addresses.
#[must_use]
pub fn parse_domifaddr(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            // <iface> <mac> <protocol> <address/prefix>
            let addr = cols.get(3)?;
            let ip = addr.split('/').next()?;
            if ip.contains('.') || ip.contains(':') {
                Some(ip.to_string())
            } else {
                None
            }
        })
        .filter(|ip| !ip.starts_with("127.") && ip != "::1")
        .collect()
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
    fn podman_ps_json_parses_l10_fields() {
        let raw = r#"[{"Names":["nginx-web"],"Image":"docker.io/nginx:latest","State":"running",
            "Ports":[{"host_port":8080,"container_port":80,"protocol":"tcp"}]},
            {"Names":["pihole"],"Image":"pihole/pihole","State":"exited","Ports":null}]"#;
        let rows = parse_podman_ps(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "nginx-web");
        assert_eq!(rows[0].ports, ["8080->80/tcp"]);
        assert_eq!(rows[1].state, "exited");
        assert!(rows[1].ports.is_empty());
    }

    #[test]
    fn virsh_list_and_dominfo_parse_l11_fields() {
        let vms = parse_virsh_list(" 1    win11    running\n -    fedora-lab   shut off\n");
        assert_eq!(vms.len(), 2);
        assert_eq!(vms[0].name, "win11");
        assert_eq!(vms[1].state, "shut off");
        let (vcpus, mem) = parse_dominfo("Id: 1\nCPU(s):         4\nMax memory:     8388608 KiB\n");
        assert_eq!(vcpus, Some(4));
        assert_eq!(mem, Some(8192));
    }

    #[test]
    fn domifaddr_extracts_guest_ips() {
        let raw = " vnet0      52:54:00:aa:bb:cc    ipv4         192.168.122.50/24\n";
        assert_eq!(parse_domifaddr(raw), ["192.168.122.50"]);
    }

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
            ["jellyfin", "navidrome-airsonic", "mpd", "dlna"]
        );
    }
}
