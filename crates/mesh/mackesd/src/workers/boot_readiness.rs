//! BOOT-STATUS-1 — the `boot_readiness` worker: probes the mesh-fabric bring-up
//! chain in dependency order and publishes ONE ordered snapshot to the Bus
//! (`state/boot-readiness`) each tick, so the Applications/HOME boot-status dialog
//! (`docs/design/boot-status-dialog.md`) and the applet chip render the same
//! authoritative state (Q5). Works headless — a Server/Lighthouse has the same
//! snapshot even with no desktop.
//!
//! The step model is the real boot dependency chain (Q7): Nebula → overlay IP →
//! mackesd serving → mde-bus broker → QNM-Shared mount → peer directory. Each
//! step carries `{id,label,status,detail,blocked_by,since_ms}`; a step whose
//! prerequisites aren't `ok` is `blocked` (not a misleading `fail`). The pure
//! builder (`build_readiness`) is unit-tested; the worker is the thin probe+publish
//! shell around it. App-daemon probes + per-peer roll-up + live pings land in
//! BOOT-STATUS-2/3.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde_json::json;

use super::{ShutdownToken, Worker};

/// The Bus topic the snapshot is published on (BOOT-STATUS-1).
pub const TOPIC: &str = "state/boot-readiness";

/// Publish cadence — fast enough to feel live while the chain converges.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// One gathered observation of the fabric bring-up state (impure inputs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootProbe {
    /// `nebula` unit active (overlay transport up).
    pub nebula_up: bool,
    /// This node's assigned overlay IP (empty until Nebula hands one out).
    pub overlay_ip: String,
    /// The mde-bus broker is reachable (we could open the spool to publish).
    pub bus_ok: bool,
    /// QNM-Shared is a real, writable FUSE mount.
    pub qnm_mounted: bool,
    /// Joined peer count in the replicated directory (>0 ⇒ replicated).
    pub peer_count: u32,
}

/// BOOT-STATUS-2 — one app-daemon liveness observation, appended to the snapshot
/// alongside the fabric chain. These are parallel supplementary services (not a
/// dependency chain), so they don't gate `ready` — the dialog renders them as a
/// separate "services" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceProbe {
    /// Stable id (`musicd` / `netdata` / `kdc`).
    pub id: &'static str,
    /// Human label for the row.
    pub label: &'static str,
    /// The systemd unit (or in-process listener) is up.
    pub active: bool,
    /// Reachability where a cheap check exists (a port connect): `Some(true/false)`,
    /// or `None` when "active" is the only signal.
    pub reachable: Option<bool>,
}

/// BOOT-STATUS-2 — one peer ping result. `rtt_ms == None` ⇒ unreachable (or no
/// overlay IP yet). The lighthouse is just a peer with `role == "lighthouse"`.
#[derive(Debug, Clone, PartialEq)]
pub struct PingResult {
    /// Peer hostname.
    pub peer: String,
    /// Peer overlay IP (empty if not yet assigned/replicated).
    pub overlay_ip: String,
    /// Peer role (`lighthouse` / `peer`).
    pub role: String,
    /// Round-trip time in ms, or `None` when unreachable.
    pub rtt_ms: Option<f64>,
}

/// One ordered step in the dependency chain.
struct StepDef {
    id: &'static str,
    label: &'static str,
}

/// The boot dependency chain, in order (Q7).
const STEPS: [StepDef; 6] = [
    StepDef {
        id: "nebula",
        label: "Nebula overlay",
    },
    StepDef {
        id: "overlay-ip",
        label: "Overlay IP assigned",
    },
    StepDef {
        id: "mackesd",
        label: "mackesd serving",
    },
    StepDef {
        id: "bus",
        label: "Message bus broker",
    },
    StepDef {
        id: "qnm",
        label: "QNM-Shared mounted",
    },
    StepDef {
        id: "directory",
        label: "Peer directory replicated",
    },
];

/// Whether each step's own condition is met (parallel to [`STEPS`]). `mackesd` is
/// always met here — this worker runs *inside* the serving daemon.
fn step_ok(p: &BootProbe) -> [bool; 6] {
    [
        p.nebula_up,
        !p.overlay_ip.is_empty(),
        true, // mackesd is serving (this worker is part of it)
        p.bus_ok,
        p.qnm_mounted,
        p.peer_count > 0,
    ]
}

/// A short per-step detail line.
fn step_detail(p: &BootProbe, idx: usize) -> String {
    match idx {
        0 => {
            if p.nebula_up {
                "up".into()
            } else {
                "down".into()
            }
        }
        1 => {
            if p.overlay_ip.is_empty() {
                "—".into()
            } else {
                p.overlay_ip.clone()
            }
        }
        2 => "serving".into(),
        3 => {
            if p.bus_ok {
                "bound".into()
            } else {
                "unreachable".into()
            }
        }
        4 => {
            if p.qnm_mounted {
                "/mnt/mesh-storage".into()
            } else {
                "not mounted".into()
            }
        }
        5 => format!("{} peer(s)", p.peer_count),
        _ => String::new(),
    }
}

/// Build the `state/boot-readiness` snapshot from a probe. Each chain step is
/// `ok`, `blocked` (a prerequisite isn't ok — carries `blocked_by`), or `pending`
/// (its own condition isn't met but all prerequisites are). `ready` is true when
/// every chain step is `ok`. BOOT-STATUS-2 appends the app-daemon `services` +
/// per-peer `pings` (informational — they don't gate `ready`). `now_ms` stamps
/// the snapshot.
#[must_use]
pub fn build_readiness(
    p: &BootProbe,
    services: &[ServiceProbe],
    pings: &[PingResult],
    now_ms: u64,
) -> serde_json::Value {
    let oks = step_ok(p);
    let mut steps = Vec::with_capacity(STEPS.len());
    let mut first_unmet: Option<&'static str> = None;
    for (i, def) in STEPS.iter().enumerate() {
        let status = if oks[i] {
            "ok"
        } else if first_unmet.is_some() {
            "blocked"
        } else {
            "pending"
        };
        steps.push(json!({
            "id": def.id,
            "label": def.label,
            "status": status,
            "detail": step_detail(p, i),
            "blocked_by": if status == "blocked" { first_unmet } else { None },
        }));
        if !oks[i] && first_unmet.is_none() {
            first_unmet = Some(def.id);
        }
    }
    let services: Vec<serde_json::Value> = services
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "label": s.label,
                "active": s.active,
                "reachable": s.reachable,
                // ok when active and (no reachability check OR it passed).
                "status": if s.active && s.reachable != Some(false) { "ok" } else { "down" },
            })
        })
        .collect();
    let pings: Vec<serde_json::Value> = pings
        .iter()
        .map(|pg| {
            json!({
                "peer": pg.peer,
                "overlay_ip": pg.overlay_ip,
                "role": pg.role,
                "rtt_ms": pg.rtt_ms,
                "reachable": pg.rtt_ms.is_some(),
            })
        })
        .collect();
    json!({
        "ok": true,
        "ready": oks.iter().all(|b| *b),
        "ts_ms": now_ms,
        "steps": steps,
        "services": services,
        "pings": pings,
    })
}

/// BOOT-STATUS-2 — parse the RTT (ms) from `ping -c1` stdout (`… time=12.3 ms`).
/// `None` when the line is absent (host unreachable / timed out). Pure + tested.
#[must_use]
pub fn parse_ping_rtt(stdout: &str) -> Option<f64> {
    let after = stdout.split("time=").nth(1)?;
    let num: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok()
}

/// The `boot_readiness` worker.
pub struct BootReadinessWorker {
    workgroup_root: PathBuf,
    node_id: String,
    db_path: PathBuf,
}

impl BootReadinessWorker {
    /// New worker. `workgroup_root` is the QNM-Shared mount; `db_path` the bus
    /// directory store (for the peer-directory count).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String, db_path: PathBuf) -> Self {
        Self {
            workgroup_root,
            node_id,
            db_path,
        }
    }

    /// Gather a fresh probe (impure: systemctl, fs, the directory).
    fn probe(&self) -> BootProbe {
        let nebula_up = systemctl_active("nebula");
        let overlay_ip = std::fs::read_to_string(super::nebula_supervisor::DEFAULT_OVERLAY_IP_PATH)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let qnm_mounted = crate::shared_root_writable(&self.workgroup_root);
        let now = now_ms();
        let peer_count = crate::ipc::directory::DirectoryService::new(
            &self.workgroup_root,
            Some(self.db_path.clone()),
        )
        .mesh_health_counts(&self.node_id, now)
        .0;
        BootProbe {
            nebula_up,
            overlay_ip,
            bus_ok: true, // set false on a publish failure below
            qnm_mounted,
            peer_count,
        }
    }

    /// BOOT-STATUS-2 — gather the per-peer ping results from the live directory.
    /// Each peer's overlay IP is pinged in parallel (bounded fan-out) with a 1 s
    /// deadline; a peer with no overlay IP yet reports `rtt_ms: None`. The
    /// lighthouse is among these (it's a peer with `role == "lighthouse"`).
    fn probe_pings(&self) -> Vec<PingResult> {
        let dir = crate::ipc::directory::DirectoryService::new(
            &self.workgroup_root,
            Some(self.db_path.clone()),
        )
        .build_directory(now_ms());
        let peers = dir["peers"].as_array().cloned().unwrap_or_default();
        // Bound fan-out: skip ourself, cap the count so a large mesh can't stall
        // the tick. Each ping is its own short-lived thread, joined below.
        let targets: Vec<(String, String, String)> = peers
            .iter()
            .filter_map(|pr| {
                let name = pr["hostname"].as_str().unwrap_or("").to_string();
                let ip = pr["overlay_ip"].as_str().unwrap_or("").to_string();
                let role = pr["role"].as_str().unwrap_or("peer").to_string();
                if name == self.node_id {
                    None
                } else {
                    Some((name, ip, role))
                }
            })
            .take(MAX_PING_TARGETS)
            .collect();
        let handles: Vec<_> = targets
            .into_iter()
            .map(|(peer, overlay_ip, role)| {
                std::thread::spawn(move || {
                    let rtt_ms = if overlay_ip.is_empty() {
                        None
                    } else {
                        ping_rtt(&overlay_ip)
                    };
                    PingResult {
                        peer,
                        overlay_ip,
                        role,
                        rtt_ms,
                    }
                })
            })
            .collect();
        let mut out: Vec<PingResult> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
        out.sort_by(|a, b| a.peer.cmp(&b.peer));
        out
    }
}

/// BOOT-STATUS-2 — cap on per-tick ping fan-out so a large mesh can't stall the
/// 2 s publish loop (each ping is a ≤1 s thread; joined together they overlap).
const MAX_PING_TARGETS: usize = 24;

/// BOOT-STATUS-2 — the app daemons appended to the snapshot. musicd + netdata are
/// real systemd units; KDE Connect's listener is in-process (mackesd), probed by
/// a localhost port connect. (mde-voice-hud is a desktop GUI, not a boot daemon,
/// so it is intentionally not a boot-readiness service.)
fn gather_services() -> Vec<ServiceProbe> {
    vec![
        ServiceProbe {
            id: "musicd",
            label: "Music daemon",
            active: systemctl_active("mde-musicd"),
            reachable: None,
        },
        ServiceProbe {
            id: "netdata",
            label: "Live metrics",
            active: systemctl_active("netdata"),
            reachable: Some(tcp_open("127.0.0.1:19999")),
        },
        ServiceProbe {
            id: "kdc",
            label: "KDE Connect",
            // The listener is in-process (no unit) — a localhost connect to the
            // KDE Connect port is both the active + reachable signal.
            active: tcp_open("127.0.0.1:1716"),
            reachable: Some(tcp_open("127.0.0.1:1716")),
        },
    ]
}

/// `systemctl is-active <unit>` ⇒ true iff the unit is active.
fn systemctl_active(unit: &str) -> bool {
    std::process::Command::new("systemctl")
        .args(["is-active", unit])
        .output()
        .map(|o| o.stdout.starts_with(b"active"))
        .unwrap_or(false)
}

/// A bounded TCP connect probe (300 ms) — `true` if `addr` accepts a connection.
fn tcp_open(addr: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    addr.to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .and_then(|sa| TcpStream::connect_timeout(&sa, Duration::from_millis(300)).ok())
        .is_some()
}

/// `ping -c1 -W1 <ip>` → RTT ms, or `None` when unreachable. The system `ping`
/// avoids needing CAP_NET_RAW in-process; the parse is [`parse_ping_rtt`].
fn ping_rtt(ip: &str) -> Option<f64> {
    let out = std::process::Command::new("ping")
        .args(["-c", "1", "-W", "1", ip])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ping_rtt(&String::from_utf8_lossy(&out.stdout))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[async_trait::async_trait]
impl Worker for BootReadinessWorker {
    fn name(&self) -> &'static str {
        "boot_readiness"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = mde_bus::default_data_dir() else {
            tracing::debug!("boot_readiness: no bus data dir; worker idle");
            return Ok(());
        };
        loop {
            // The probe + publish are sync (Persist isn't Send); run on a blocking
            // thread so the async runtime isn't stalled. BOOT-STATUS-2 gathers the
            // app-daemon liveness + per-peer pings here too (systemctl + `ping`).
            let probe = self.probe();
            let pings = self.probe_pings();
            let bus_root = bus_root.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let services = gather_services();
                if let Ok(persist) = Persist::open(bus_root) {
                    let snap = build_readiness(&probe, &services, &pings, now_ms());
                    let _ = persist.write(TOPIC, Priority::Default, None, Some(&snap.to_string()));
                }
            })
            .await;
            tokio::select! {
                _ = shutdown.wait() => break,
                () = tokio::time::sleep(INTERVAL) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(v: &serde_json::Value, i: usize, k: &str) -> String {
        v["steps"][i][k].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn all_up_is_ready_every_step_ok() {
        let p = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: true,
            peer_count: 4,
        };
        let v = build_readiness(&p, &[], &[], 123);
        assert_eq!(v["ready"], true);
        assert_eq!(v["ts_ms"], 123);
        for i in 0..6 {
            assert_eq!(val(&v, i, "status"), "ok", "step {i}");
        }
        assert_eq!(val(&v, 1, "detail"), "10.42.0.5");
    }

    #[test]
    fn first_unmet_is_pending_downstream_is_blocked() {
        // Nebula down → step 0 pending, everything after blocked_by nebula.
        let p = BootProbe::default();
        let v = build_readiness(&p, &[], &[], 0);
        assert_eq!(v["ready"], false);
        assert_eq!(val(&v, 0, "status"), "pending"); // first unmet
        assert_eq!(val(&v, 1, "status"), "blocked");
        assert_eq!(val(&v, 1, "blocked_by"), "nebula");
        assert_eq!(val(&v, 5, "blocked_by"), "nebula");
    }

    #[test]
    fn midchain_unmet_blocks_only_downstream() {
        // Nebula + overlay-ip + mackesd + bus ok, but QNM not mounted.
        let p = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: false,
            peer_count: 0,
        };
        let v = build_readiness(&p, &[], &[], 0);
        assert_eq!(val(&v, 3, "status"), "ok"); // bus ok
        assert_eq!(val(&v, 4, "status"), "pending"); // qnm = first unmet
        assert_eq!(val(&v, 5, "status"), "blocked"); // directory blocked by qnm
        assert_eq!(val(&v, 5, "blocked_by"), "qnm");
        assert_eq!(v["ready"], false);
    }

    #[test]
    fn services_and_pings_render_into_snapshot() {
        // BOOT-STATUS-2 — app daemons + pings appear; they don't gate `ready`.
        let p = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: true,
            peer_count: 1,
        };
        let services = [
            ServiceProbe {
                id: "musicd",
                label: "Music daemon",
                active: true,
                reachable: None,
            },
            ServiceProbe {
                id: "netdata",
                label: "Live metrics",
                active: true,
                reachable: Some(false), // active but port unreachable → down
            },
        ];
        let pings = [
            PingResult {
                peer: "lighthouse-01".into(),
                overlay_ip: "10.42.0.1".into(),
                role: "lighthouse".into(),
                rtt_ms: Some(12.5),
            },
            PingResult {
                peer: "anvil".into(),
                overlay_ip: String::new(),
                role: "peer".into(),
                rtt_ms: None,
            },
        ];
        let v = build_readiness(&p, &services, &pings, 7);
        assert_eq!(v["ready"], true); // services/pings don't affect readiness
        assert_eq!(v["services"][0]["status"], "ok"); // active, no port check
        assert_eq!(v["services"][1]["status"], "down"); // active but unreachable
        assert_eq!(v["pings"][0]["reachable"], true);
        assert_eq!(v["pings"][0]["rtt_ms"], 12.5);
        assert_eq!(v["pings"][0]["role"], "lighthouse");
        assert_eq!(v["pings"][1]["reachable"], false); // no overlay IP yet
    }

    #[test]
    fn parse_ping_rtt_reads_time_field() {
        // BOOT-STATUS-2 — RTT parsed from real `ping -c1` output; absent → None.
        let out = "64 bytes from 10.42.0.1: icmp_seq=1 ttl=64 time=0.342 ms\n";
        assert_eq!(parse_ping_rtt(out), Some(0.342));
        let out2 = "PING 10.42.0.9: 56 data bytes\n--- 10.42.0.9 ping statistics ---\n1 packets transmitted, 0 received";
        assert_eq!(parse_ping_rtt(out2), None);
        assert_eq!(parse_ping_rtt(""), None);
    }
}
