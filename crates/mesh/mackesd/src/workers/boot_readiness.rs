//! BOOT-STATUS-1 — the `boot_readiness` worker: probes the mesh-fabric bring-up
//! chain in dependency order and publishes ONE ordered snapshot to the Bus
//! (`state/boot-readiness`) each tick, so the Applications/HOME boot-status dialog
//! (`docs/design/boot-status-dialog.md`) and the applet chip render the same
//! authoritative state (Q5). Works headless — a Server/Lighthouse has the same
//! snapshot even with no desktop.
//!
//! The step model is the real boot dependency chain (Q7): Nebula → overlay IP →
//! mackesd serving → mde-bus broker → mesh coordination (etcd) → peer directory. Each
//! step carries `{id,label,status,detail,blocked_by,since_ms}`; a step whose
//! prerequisites aren't `ok` is `blocked` (not a misleading `fail`). The pure
//! builder (`build_readiness`) is unit-tested; the worker is the thin probe+publish
//! shell around it. App-daemon probes + per-peer roll-up + live pings land in
//! BOOT-STATUS-2/3.

#![cfg(feature = "async-services")]

use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use mackes_mesh_types::peers::PeerRecord;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde_json::json;

use super::{ShutdownToken, Worker};

/// The Bus topic the snapshot is published on (BOOT-STATUS-1).
pub const TOPIC: &str = "state/boot-readiness";

/// Publish cadence — fast enough to feel live while the chain converges.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// Maximum deterministic spread before the first full blocking probe batch.
/// The first batch still begins within the normal publication cadence, while
/// different node identities avoid forking `systemctl`/`ping` and opening the
/// directory store in lockstep after a fleet restart.
pub const MAX_INITIAL_PHASE: Duration = Duration::from_millis(1_500);

/// Failed blocking probes must not re-fork/reconnect on every publish tick.
/// Keep the first retry responsive, then back off to a bounded steady-state.
pub const FAILURE_BACKOFF_INITIAL: Duration = Duration::from_secs(4);
/// Maximum delay between retries for a continuously failed probe group.
pub const FAILURE_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Healthy source probes need not repeat at the 2-second publication cadence.
/// The cached result is still published every tick, while this slower source
/// recheck avoids repeatedly spawning `systemctl`, TCP, directory, and `ping`
/// work on every node when readiness is stable.
pub const HEALTHY_RECHECK_INTERVAL: Duration = Duration::from_secs(10);

/// Allow ordinary timer jitter without treating it as a wake event. A larger
/// overrun is safety-significant: cached readiness was not observed on the
/// cadence under which it was admitted.
pub const SCHEDULING_GAP_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
struct FailureBackoff {
    failures: u32,
    retry_at: Option<Instant>,
}

impl FailureBackoff {
    fn due(&self, now: Instant) -> bool {
        self.retry_at.map_or(true, |retry_at| now >= retry_at)
    }

    fn record(&mut self, failed: bool, now: Instant) {
        if failed {
            self.retry_at = Some(now + failure_backoff_delay(self.failures));
            self.failures = self.failures.saturating_add(1);
        } else {
            self.failures = 0;
            self.retry_at = Some(now + HEALTHY_RECHECK_INTERVAL);
        }
    }
}

#[must_use]
fn failure_backoff_delay(failures: u32) -> Duration {
    FAILURE_BACKOFF_INITIAL
        .saturating_mul(1_u32 << failures.min(4))
        .min(FAILURE_BACKOFF_MAX)
}

/// Return a stable bounded phase for the first expensive probe batch.
///
/// FNV-1a is sufficient here because this is scheduling spread, not a security
/// primitive. An empty identity deliberately disables the hash phase; the
/// first-delay calculation still preserves the existing cadence deadline.
fn initial_phase_for(node_id: &str, interval: Duration) -> Duration {
    let window_ms = interval.as_millis().min(MAX_INITIAL_PHASE.as_millis());
    if node_id.is_empty() || window_ms == 0 {
        return Duration::ZERO;
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in node_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Duration::from_millis((u128::from(hash) % (window_ms + 1)) as u64)
}

/// One gathered observation of the fabric bring-up state (impure inputs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootProbe {
    /// `nebula` unit active (overlay transport up).
    pub nebula_up: bool,
    /// This node's assigned overlay IP (empty until Nebula hands one out).
    pub overlay_ip: String,
    /// The mde-bus broker is reachable (we could open the spool to publish).
    pub bus_ok: bool,
    /// The shared-state plane is up. SUBSTRATE-10: when on the etcd coordination
    /// plane this is "etcd reachable"; otherwise it's "the shared dir exists"
    /// (the file plane / Syncthing is non-critical to liveness). The field keeps
    /// its `qnm_mounted` name for the snapshot's wire-compat.
    pub qnm_mounted: bool,
    /// SUBSTRATE-10 — true when shared-state liveness is sourced from etcd (the
    /// endpoints file is present), so the step renders as "Mesh coordination
    /// (etcd)" / "etcd reachable".
    pub on_etcd: bool,
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
        label: "Mesh coordination",
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
            if p.on_etcd {
                if p.qnm_mounted {
                    "etcd reachable".into()
                } else {
                    "etcd unreachable".into()
                }
            } else if p.qnm_mounted {
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
        // SUBSTRATE-10 — the shared-state step renders by substrate: "Mesh
        // coordination (etcd)" on an etcd node, the plain shared-dir otherwise.
        let label = if def.id == "qnm" && p.on_etcd {
            "Mesh coordination (etcd)"
        } else {
            def.label
        };
        steps.push(json!({
            "id": def.id,
            "label": label,
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
    /// New worker. `workgroup_root` is the shared-storage dir; `db_path` the bus
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
        let overlay_path = std::path::Path::new(super::nebula_supervisor::DEFAULT_OVERLAY_IP_PATH);
        let nebula_before = systemctl_active("nebula");
        let overlay_before = read_overlay_generation(overlay_path);
        // SUBSTRATE-10 — shared-state liveness by substrate: when on the etcd
        // coordination plane (endpoints file present) it's "is etcd reachable"
        // (a cheap TCP connect to the client port — no async runtime needed in
        // this on-executor probe); otherwise it's "the shared dir exists".
        let etcd_endpoints = crate::substrate::etcd::default_endpoints();
        let on_etcd = !etcd_endpoints.is_empty();
        let qnm_mounted = if on_etcd {
            etcd_first_reachable(&etcd_endpoints)
        } else {
            crate::shared_root_writable(&self.workgroup_root)
        };
        let directory = if on_etcd {
            // Configuring etcd makes its lease-backed directory authoritative.
            // DirectoryService intentionally falls back to the filesystem for
            // display availability, but boot admission must not let retained
            // filesystem rows substitute for a failed current etcd read.
            DirectoryPeerObservation::Etcd(crate::substrate::peers::read_peers_blocking(
                &etcd_endpoints,
            ))
        } else {
            DirectoryPeerObservation::Filesystem(mackes_mesh_types::peers::read_peers(
                &mackes_mesh_types::peers::peers_dir(&self.workgroup_root),
            ))
        };
        let overlay_after = read_overlay_generation(overlay_path);
        let overlay_ip =
            stable_overlay_ip(overlay_before.as_ref(), overlay_after.as_ref()).unwrap_or_default();
        let nebula_up = nebula_before && systemctl_active("nebula");
        let peer_count = if nebula_up {
            authoritative_peer_count(directory, &self.node_id, &overlay_ip)
        } else {
            0
        };
        BootProbe {
            nebula_up,
            overlay_ip: overlay_ip.to_owned(),
            bus_ok: true, // set false on a publish failure below
            qnm_mounted,
            on_etcd,
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

#[derive(Debug, PartialEq, Eq)]
struct OverlayGeneration {
    overlay_ip: String,
    device: u64,
    inode: u64,
}

const MAX_OVERLAY_MARKER_BYTES: u64 = 64;

/// Read the supervisor's readiness marker as one bounded descriptor identity.
/// The supervisor atomically replaces this file after it has verified Nebula;
/// a symlink or a replacement between the two observations must therefore be
/// treated as a different overlay generation, even when the text is unchanged.
fn read_overlay_generation(path: &std::path::Path) -> Option<OverlayGeneration> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(0o400000 | 0o2000000); // O_NOFOLLOW | O_CLOEXEC
    let mut file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_OVERLAY_MARKER_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_OVERLAY_MARKER_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_OVERLAY_MARKER_BYTES {
        return None;
    }
    let overlay_ip = std::str::from_utf8(&bytes).ok()?.trim();
    let address = overlay_ip.parse::<std::net::Ipv4Addr>().ok()?;
    let octets = address.octets();
    if address.to_string() != overlay_ip || octets[0] != 10 || octets[1] != 42 || octets[2] > 127 {
        return None;
    }
    Some(OverlayGeneration {
        overlay_ip: overlay_ip.to_owned(),
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn stable_overlay_ip<'a>(
    before: Option<&'a OverlayGeneration>,
    after: Option<&OverlayGeneration>,
) -> Option<&'a str> {
    let before = before?;
    (Some(before) == after).then_some(before.overlay_ip.as_str())
}

enum DirectoryPeerObservation {
    Etcd(Option<Vec<PeerRecord>>),
    Filesystem(Vec<PeerRecord>),
}

/// Admit a directory only when it carries exactly one row for this daemon's
/// hostname and that row exclusively owns the stable local overlay address.
/// A retained row from the pre-restart identity (or another peer claiming the
/// same address) must not make an otherwise populated directory look ready.
fn authoritative_peer_count(
    observation: DirectoryPeerObservation,
    node_id: &str,
    overlay_ip: &str,
) -> u32 {
    let records = match observation {
        DirectoryPeerObservation::Etcd(Some(records))
        | DirectoryPeerObservation::Filesystem(records) => records,
        DirectoryPeerObservation::Etcd(None) => return 0,
    };
    let hostname = node_id.strip_prefix("peer:").unwrap_or(node_id);
    if hostname.is_empty() || overlay_ip.is_empty() {
        return 0;
    }
    let local_rows: Vec<&PeerRecord> = records
        .iter()
        .filter(|record| record.hostname == hostname)
        .collect();
    if local_rows.len() != 1
        || local_rows[0].overlay_ip.as_deref() != Some(overlay_ip)
        || records.iter().any(|record| {
            record.hostname != hostname && record.overlay_ip.as_deref() == Some(overlay_ip)
        })
    {
        return 0;
    }
    u32::try_from(records.len()).unwrap_or(u32::MAX)
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

/// A false fabric observation means at least one of the blocking readiness
/// checks is still failing. Cached values remain visible while its retry gate
/// is closed, so the published status does not flap to an invented success.
/// Healthy sources are also rechecked on a slower cadence; publication remains
/// fast without making every node repeatedly fork/process/network-probe the same
/// stable state.
fn fabric_probe_failed(probe: &BootProbe) -> bool {
    !probe.nebula_up
        || probe.overlay_ip.is_empty()
        || !probe.bus_ok
        || !probe.qnm_mounted
        || probe.peer_count == 0
}

/// Only count peers with an address as failed ping probes. Missing overlay IPs
/// are a normal boot-pending state and do not justify retrying `ping` sooner.
fn ping_probe_failed(pings: &[PingResult]) -> bool {
    pings
        .iter()
        .any(|ping| !ping.overlay_ip.is_empty() && ping.rtt_ms.is_none())
}

fn services_probe_failed(services: &[ServiceProbe]) -> bool {
    services
        .iter()
        .any(|service| !service.active || service.reachable == Some(false))
}

/// `systemctl is-active <unit>` ⇒ true iff the unit is active.
fn systemctl_active(unit: &str) -> bool {
    let mut command = std::process::Command::new("systemctl");
    command.args(["is-active", unit]);
    bounded_command_stdout(&mut command, MAX_SYSTEMCTL_STDOUT_BYTES)
        .map(|stdout| stdout.starts_with(b"active"))
        .unwrap_or(false)
}

const MAX_SYSTEMCTL_STDOUT_BYTES: usize = 4096;

/// Read a command's stdout with a hard cap. An oversized producer is killed
/// and treated as a failed probe, so host-command output cannot grow without
/// bound in the readiness worker.
fn bounded_command_stdout(
    command: &mut std::process::Command,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut bytes = Vec::new();
    let read_result = stdout
        .by_ref()
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes);
    if read_result.is_err() || bytes.len() > max_bytes {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    child.wait().ok()?;
    Some(bytes)
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

/// SUBSTRATE-10 — `true` if any etcd client endpoint accepts a TCP connection.
/// A cheap liveness check (no async etcd client / runtime) for the on-executor
/// boot probe: strips the `http://`/`https://` scheme off each endpoint and
/// `tcp_open`s the `host:port`. Pure-ish (network I/O only); [`endpoint_authority`]
/// is the testable parse.
fn etcd_first_reachable(endpoints: &[String]) -> bool {
    endpoints
        .iter()
        .filter_map(|e| endpoint_authority(e))
        .any(|hostport| tcp_open(&hostport))
}

/// Strip the URL scheme off an etcd endpoint, yielding `host:port`. Pure.
#[must_use]
fn endpoint_authority(endpoint: &str) -> Option<String> {
    let s = endpoint.trim();
    let s = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
        .unwrap_or(s);
    let authority = s.split('/').next().unwrap_or(s).trim();
    (!authority.is_empty()).then(|| authority.to_string())
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

/// Linux boot time includes time spent suspended, unlike the monotonic clock
/// used by Tokio timers. Reading it only around the existing publication sleep
/// lets the worker distinguish a resume from ordinary active runtime without
/// adding another timer or periodic task.
fn boot_elapsed() -> Option<Duration> {
    let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
    let seconds = uptime.split_whitespace().next()?.parse::<f64>().ok()?;
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

fn sleep_was_discontinuous(monotonic_elapsed: Duration, boot_elapsed: Option<Duration>) -> bool {
    monotonic_elapsed > INTERVAL.saturating_add(SCHEDULING_GAP_GRACE)
        || boot_elapsed
            .is_some_and(|elapsed| elapsed > monotonic_elapsed.saturating_add(SCHEDULING_GAP_GRACE))
}

/// Resolve the daemon's Bus spool even when it starts without a user home.
///
/// `mde_bus::default_data_dir` intentionally returns `None` in that service
/// context and documents the system spool as the fallback.  Keeping that
/// fallback here prevents the readiness authority from disappearing for the
/// lifetime of mackesd merely because HOME/XDG were absent at process start.
fn default_bus_root() -> PathBuf {
    bus_root_or_system(mde_bus::default_data_dir())
}

fn bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

/// Replace any readiness record left by an earlier daemon generation before
/// this worker enters its deliberately phased first-probe delay. Without this
/// barrier, a consumer can observe the previous process's `ready: true` for up
/// to one full publication interval after a restart and admit work before the
/// restarted daemon has revalidated any dependency.
fn publish_startup_barrier(bus_root: PathBuf, observed_at_ms: u64) -> Result<(), String> {
    let persist = Persist::open(bus_root).map_err(|error| error.to_string())?;
    let mut snapshot = build_readiness(&BootProbe::default(), &[], &[], observed_at_ms);
    snapshot["phase"] = json!("probing");
    persist
        .write(TOPIC, Priority::Default, None, Some(&snapshot.to_string()))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn publish_gap_barrier_if_needed(
    bus_root: PathBuf,
    observed_at_ms: u64,
    monotonic_elapsed: Duration,
    boot_elapsed: Option<Duration>,
) -> Result<bool, String> {
    if !sleep_was_discontinuous(monotonic_elapsed, boot_elapsed) {
        return Ok(false);
    }
    publish_startup_barrier(bus_root, observed_at_ms)?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn record_published_probe_batch(
    publication: Result<(), String>,
    observed_at: Instant,
    probe_due: bool,
    pings_due: bool,
    services_due: bool,
    probe: BootProbe,
    pings: Vec<PingResult>,
    services: Vec<ServiceProbe>,
    probe_backoff: &mut FailureBackoff,
    ping_backoff: &mut FailureBackoff,
    service_backoff: &mut FailureBackoff,
    cached_probe: &mut Option<BootProbe>,
    cached_pings: &mut Option<Vec<PingResult>>,
    cached_services: &mut Option<Vec<ServiceProbe>>,
) -> Result<(), String> {
    if let Err(error) = publication {
        // A healthy observation is authoritative only if the generation that
        // observed it also published it. Retaining this batch would allow a
        // recovered/replaced Bus to receive cached `ready: true` without any
        // dependency being revalidated after the publication gap.
        *probe_backoff = FailureBackoff::default();
        *ping_backoff = FailureBackoff::default();
        *service_backoff = FailureBackoff::default();
        *cached_probe = None;
        *cached_pings = None;
        *cached_services = None;
        return Err(error);
    }

    if probe_due {
        probe_backoff.record(fabric_probe_failed(&probe), observed_at);
        *cached_probe = Some(probe);
    }
    if pings_due {
        ping_backoff.record(ping_probe_failed(&pings), observed_at);
        *cached_pings = Some(pings);
    }
    if services_due {
        service_backoff.record(services_probe_failed(&services), observed_at);
        *cached_services = Some(services);
    }
    Ok(())
}

#[async_trait::async_trait]
impl Worker for BootReadinessWorker {
    fn name(&self) -> &'static str {
        "boot_readiness"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = default_bus_root();
        let mut probe_backoff = FailureBackoff::default();
        let mut ping_backoff = FailureBackoff::default();
        let mut service_backoff = FailureBackoff::default();
        let mut cached_probe: Option<BootProbe> = None;
        let mut cached_pings: Option<Vec<PingResult>> = None;
        let mut cached_services: Option<Vec<ServiceProbe>> = None;

        // A Bus topic survives a daemon process restart. Invalidate an old
        // generation's optimistic state synchronously before waiting for the
        // node-specific first-probe phase; the first real probe below replaces
        // this fail-safe barrier with current observations.
        publish_startup_barrier(bus_root.clone(), now_ms()).map_err(anyhow::Error::msg)?;

        // Keep the first full probe batch within the old two-second freshness
        // deadline, but anchor it to a stable node-specific phase so seats do
        // not all fork processes and perform network/filesystem work together.
        // Selecting shutdown during the delay keeps daemon cancellation prompt.
        let first_delay = INTERVAL.saturating_sub(initial_phase_for(&self.node_id, INTERVAL));
        tokio::select! {
            _ = shutdown.wait() => return Ok(()),
            _ = tokio::time::sleep(first_delay) => {}
        }
        loop {
            let now = Instant::now();
            let probe_due = probe_backoff.due(now);
            let pings_due = ping_backoff.due(now);
            let services_due = service_backoff.due(now);
            let workgroup_root = self.workgroup_root.clone();
            let node_id = self.node_id.clone();
            let db_path = self.db_path.clone();
            let publish_bus_root = bus_root.clone();
            let previous_probe = cached_probe.clone();
            let previous_pings = cached_pings.clone();
            let previous_services = cached_services.clone();

            // All probes are synchronous (systemctl, filesystem, TCP, directory,
            // and `ping`) and must stay off the async executor. Failed groups are
            // gated by bounded backoff; the cached result still gets published at
            // the normal cadence while a retry is pending.
            let result = tokio::task::spawn_blocking(move || {
                let worker = BootReadinessWorker::new(workgroup_root, node_id, db_path);
                let probe = if probe_due {
                    worker.probe()
                } else {
                    previous_probe.unwrap_or_default()
                };
                let pings = if pings_due {
                    worker.probe_pings()
                } else {
                    previous_pings.unwrap_or_default()
                };
                let services = if services_due {
                    gather_services()
                } else {
                    previous_services.unwrap_or_default()
                };
                let publication = Persist::open(publish_bus_root)
                    .map_err(|error| error.to_string())
                    .and_then(|persist| {
                        let snap = build_readiness(&probe, &services, &pings, now_ms());
                        persist
                            .write(TOPIC, Priority::Default, None, Some(&snap.to_string()))
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    });
                (probe, pings, services, publication)
            })
            .await;

            match result {
                Ok((probe, pings, services, publication)) => {
                    if let Err(error) = record_published_probe_batch(
                        publication,
                        now,
                        probe_due,
                        pings_due,
                        services_due,
                        probe,
                        pings,
                        services,
                        &mut probe_backoff,
                        &mut ping_backoff,
                        &mut service_backoff,
                        &mut cached_probe,
                        &mut cached_pings,
                        &mut cached_services,
                    ) {
                        tracing::debug!(%error, "boot_readiness: snapshot publication failed");
                    }
                }
                Err(error) => {
                    tracing::debug!(%error, "boot_readiness: blocking probe batch failed");
                    if probe_due {
                        probe_backoff.record(true, now);
                    }
                    if pings_due {
                        ping_backoff.record(true, now);
                    }
                    if services_due {
                        service_backoff.record(true, now);
                    }
                }
            }
            let sleep_started = Instant::now();
            let boot_before_sleep = boot_elapsed();
            tokio::select! {
                _ = shutdown.wait() => break,
                () = tokio::time::sleep(INTERVAL) => {}
            }
            let monotonic_elapsed = sleep_started.elapsed();
            let boot_sleep_elapsed = boot_before_sleep
                .zip(boot_elapsed())
                .and_then(|(before, after)| after.checked_sub(before));
            if publish_gap_barrier_if_needed(
                bus_root.clone(),
                now_ms(),
                monotonic_elapsed,
                boot_sleep_elapsed,
            )
            .map_err(anyhow::Error::msg)?
            {
                // The barrier must remain authoritative until every source has
                // been freshly observed. Resetting both schedules and caches
                // makes the next loop perform all real probes rather than
                // republishing a pre-suspend healthy result.
                probe_backoff = FailureBackoff::default();
                ping_backoff = FailureBackoff::default();
                service_backoff = FailureBackoff::default();
                cached_probe = None;
                cached_pings = None;
                cached_services = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(hostname: &str, overlay_ip: &str) -> PeerRecord {
        PeerRecord {
            hostname: hostname.to_owned(),
            mde_version: None,
            last_seen_ms: 1,
            health: "healthy".into(),
            descriptors: None,
            overlay_ip: Some(overlay_ip.to_owned()),
            role: Some("workstation".into()),
            external_addr: None,
            media: false,
        }
    }

    #[test]
    fn bus_root_has_the_documented_system_fallback() {
        assert_eq!(bus_root_or_system(None), PathBuf::from("/run/mde-bus"));
        assert_eq!(
            bus_root_or_system(Some(PathBuf::from("/tmp/explicit-bus"))),
            PathBuf::from("/tmp/explicit-bus")
        );
    }

    #[test]
    fn configured_etcd_read_failure_cannot_substitute_stale_filesystem_peers() {
        let filesystem_rows = vec![peer("node-a", "10.42.0.5")];

        assert_eq!(
            authoritative_peer_count(DirectoryPeerObservation::Etcd(None), "node-a", "10.42.0.5",),
            0,
            "an unavailable configured authority must leave boot pending"
        );
        assert_eq!(
            authoritative_peer_count(
                DirectoryPeerObservation::Filesystem(filesystem_rows),
                "node-a",
                "10.42.0.5",
            ),
            1,
            "filesystem rows remain authoritative only in filesystem mode"
        );
    }

    #[test]
    fn replaced_overlay_generation_cannot_join_retained_directory_identity() {
        let tmp = tempfile::tempdir().expect("temporary overlay root");
        let marker = tmp.path().join("overlay-ip");
        std::fs::write(&marker, b"10.42.0.5\n").expect("seed old overlay generation");
        let before = read_overlay_generation(&marker).expect("read old generation");

        let replacement = tmp.path().join("overlay-ip.next");
        std::fs::write(&replacement, b"10.42.0.9\n").expect("stage replacement generation");
        std::fs::rename(&replacement, &marker).expect("replace overlay generation");
        let after = read_overlay_generation(&marker).expect("read replacement generation");

        let admitted_overlay = stable_overlay_ip(Some(&before), Some(&after)).unwrap_or_default();
        assert_eq!(
            authoritative_peer_count(
                DirectoryPeerObservation::Etcd(Some(vec![
                    peer("node-a", "10.42.0.5"),
                    peer("node-b", "10.42.0.6"),
                ])),
                "node-a",
                admitted_overlay,
            ),
            0,
            "a retained self row must not bridge an in-flight overlay replacement"
        );

        let corrected = read_overlay_generation(&marker).expect("read corrected generation");
        assert_eq!(
            authoritative_peer_count(
                DirectoryPeerObservation::Etcd(Some(vec![
                    peer("node-a", "10.42.0.9"),
                    peer("node-b", "10.42.0.6"),
                ])),
                "node-a",
                stable_overlay_ip(Some(&corrected), Some(&corrected)).unwrap_or_default(),
            ),
            2,
            "only the corrected-forward marker and self row may restore readiness"
        );
    }

    #[test]
    fn startup_barrier_supersedes_persisted_ready_snapshot() {
        let tmp = tempfile::tempdir().expect("temporary Bus root");
        let root = tmp.path().to_path_buf();
        let persist = Persist::open(root.clone()).expect("open Bus");
        let healthy = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: true,
            on_etcd: true,
            peer_count: 3,
        };
        let old = build_readiness(&healthy, &[], &[], 41);
        assert_eq!(old["ready"], true);
        persist
            .write(TOPIC, Priority::Default, None, Some(&old.to_string()))
            .expect("persist previous daemon generation");

        publish_startup_barrier(root.clone(), 42).expect("publish restart barrier");

        let rows = Persist::open(root)
            .expect("reopen Bus")
            .list_since(TOPIC, None)
            .expect("read readiness history");
        assert_eq!(rows.len(), 2);
        let current: serde_json::Value = serde_json::from_str(
            rows.last()
                .and_then(|row| row.body.as_deref())
                .expect("current readiness body"),
        )
        .expect("parse current readiness");
        assert_eq!(current["ts_ms"], 42);
        assert_eq!(current["phase"], "probing");
        assert_eq!(current["ready"], false);
        assert_eq!(current["steps"][0]["status"], "pending");

        let blocked_root = tmp.path().join("not-a-directory");
        std::fs::write(&blocked_root, b"occupied").expect("create invalid Bus root");
        assert!(publish_startup_barrier(blocked_root, 43).is_err());
    }

    #[test]
    fn wake_or_scheduling_gap_invalidates_cached_readiness_before_reuse() {
        let tmp = tempfile::tempdir().expect("temporary Bus root");
        let root = tmp.path().to_path_buf();
        let persist = Persist::open(root.clone()).expect("open Bus");
        let healthy = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: true,
            on_etcd: true,
            peer_count: 3,
        };
        persist
            .write(
                TOPIC,
                Priority::Default,
                None,
                Some(&build_readiness(&healthy, &[], &[], 50).to_string()),
            )
            .expect("persist healthy snapshot");

        assert!(
            !publish_gap_barrier_if_needed(root.clone(), 51, INTERVAL, Some(INTERVAL),)
                .expect("normal cadence remains valid")
        );
        assert!(publish_gap_barrier_if_needed(
            root.clone(),
            52,
            INTERVAL + SCHEDULING_GAP_GRACE + Duration::from_millis(1),
            Some(INTERVAL),
        )
        .expect("scheduling gap publishes barrier"));
        persist
            .write(
                TOPIC,
                Priority::Default,
                None,
                Some(&build_readiness(&healthy, &[], &[], 53).to_string()),
            )
            .expect("persist healthy snapshot after scheduling gap");
        assert!(publish_gap_barrier_if_needed(
            root.clone(),
            54,
            INTERVAL,
            Some(INTERVAL + SCHEDULING_GAP_GRACE + Duration::from_millis(1)),
        )
        .expect("resume gap publishes barrier"));

        let rows = Persist::open(root)
            .expect("reopen Bus")
            .list_since(TOPIC, None)
            .expect("read readiness history");
        assert_eq!(rows.len(), 4);
        let current: serde_json::Value = serde_json::from_str(
            rows.last()
                .and_then(|row| row.body.as_deref())
                .expect("current readiness body"),
        )
        .expect("parse current readiness");
        assert_eq!(current["ts_ms"], 54);
        assert_eq!(current["phase"], "probing");
        assert_eq!(current["ready"], false);
    }

    #[test]
    fn failed_publication_discards_healthy_caches_before_bus_recovery() {
        let now = Instant::now();
        let healthy = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: true,
            on_etcd: true,
            peer_count: 3,
        };
        let pings = vec![PingResult {
            peer: "peer-a".into(),
            overlay_ip: "10.42.0.6".into(),
            role: "peer".into(),
            rtt_ms: Some(1.0),
        }];
        let services = vec![ServiceProbe {
            id: "musicd",
            label: "Music daemon",
            active: true,
            reachable: None,
        }];
        let mut probe_backoff = FailureBackoff::default();
        let mut ping_backoff = FailureBackoff::default();
        let mut service_backoff = FailureBackoff::default();
        probe_backoff.record(false, now);
        ping_backoff.record(false, now);
        service_backoff.record(false, now);
        let mut cached_probe = Some(healthy.clone());
        let mut cached_pings = Some(pings.clone());
        let mut cached_services = Some(services.clone());

        let result = record_published_probe_batch(
            Err("replacement Bus rejected the write".into()),
            now,
            true,
            true,
            true,
            healthy,
            pings,
            services,
            &mut probe_backoff,
            &mut ping_backoff,
            &mut service_backoff,
            &mut cached_probe,
            &mut cached_pings,
            &mut cached_services,
        );

        assert!(result.is_err());
        assert!(cached_probe.is_none());
        assert!(cached_pings.is_none());
        assert!(cached_services.is_none());
        assert!(probe_backoff.due(now));
        assert!(ping_backoff.due(now));
        assert!(service_backoff.due(now));
    }

    fn val(v: &serde_json::Value, i: usize, k: &str) -> String {
        v["steps"][i][k].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn failed_probe_backoff_is_exponential_and_bounded() {
        assert_eq!(failure_backoff_delay(0), Duration::from_secs(4));
        assert_eq!(failure_backoff_delay(1), Duration::from_secs(8));
        assert_eq!(failure_backoff_delay(2), Duration::from_secs(16));
        assert_eq!(failure_backoff_delay(3), Duration::from_secs(32));
        assert_eq!(failure_backoff_delay(4), FAILURE_BACKOFF_MAX);
        assert_eq!(failure_backoff_delay(100), FAILURE_BACKOFF_MAX);
    }

    #[test]
    fn successful_probe_resets_failed_probe_backoff() {
        let now = Instant::now();
        let mut backoff = FailureBackoff::default();
        assert!(backoff.due(now));

        backoff.record(true, now);
        assert!(!backoff.due(now + Duration::from_secs(3)));
        assert!(backoff.due(now + FAILURE_BACKOFF_INITIAL));

        backoff.record(true, now + FAILURE_BACKOFF_INITIAL);
        backoff.record(false, now + Duration::from_secs(12));
        assert!(!backoff.due(now + Duration::from_secs(12)));
        assert!(backoff.due(now + Duration::from_secs(22)));

        backoff.record(true, now + Duration::from_secs(22));
        assert_eq!(backoff.retry_at, Some(now + Duration::from_secs(26)));
    }

    #[test]
    fn only_actual_failed_blocking_probes_trigger_backoff() {
        assert!(!ping_probe_failed(&[PingResult {
            peer: "pending".into(),
            overlay_ip: String::new(),
            role: "peer".into(),
            rtt_ms: None,
        }]));
        assert!(ping_probe_failed(&[PingResult {
            peer: "offline".into(),
            overlay_ip: "10.42.0.9".into(),
            role: "peer".into(),
            rtt_ms: None,
        }]));

        assert!(!services_probe_failed(&[ServiceProbe {
            id: "musicd",
            label: "Music daemon",
            active: true,
            reachable: None,
        }]));
        assert!(services_probe_failed(&[ServiceProbe {
            id: "netdata",
            label: "Live metrics",
            active: true,
            reachable: Some(false),
        }]));
    }

    #[test]
    fn initial_phase_is_stable_bounded_and_preserves_probe_deadline() {
        let phase = initial_phase_for("peer:seat15", INTERVAL);
        assert_eq!(phase, initial_phase_for("peer:seat15", INTERVAL));
        assert!(phase <= MAX_INITIAL_PHASE);
        assert_ne!(phase, initial_phase_for("peer:seat16", INTERVAL));

        let first_delay = INTERVAL.saturating_sub(phase);
        assert!(first_delay <= INTERVAL);
        assert!(first_delay >= INTERVAL - MAX_INITIAL_PHASE);
        assert_eq!(initial_phase_for("", INTERVAL), Duration::ZERO);

        let short_interval = Duration::from_millis(100);
        assert!(initial_phase_for("peer:seat15", short_interval) <= short_interval);
    }

    #[test]
    fn all_up_is_ready_every_step_ok() {
        let p = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: true,
            on_etcd: false,
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
    fn endpoint_authority_strips_scheme_and_path() {
        assert_eq!(
            endpoint_authority("http://10.42.0.1:2379").as_deref(),
            Some("10.42.0.1:2379")
        );
        assert_eq!(
            endpoint_authority("https://lh:2379/").as_deref(),
            Some("lh:2379")
        );
        assert_eq!(
            endpoint_authority("10.42.0.2:2379").as_deref(),
            Some("10.42.0.2:2379")
        );
        assert!(endpoint_authority("").is_none());
        assert!(endpoint_authority("http://").is_none());
    }

    #[test]
    fn etcd_mode_renders_coordination_step() {
        // SUBSTRATE-10 — on the etcd plane the shared-state step reads as etcd.
        let p = BootProbe {
            nebula_up: true,
            overlay_ip: "10.42.0.5".into(),
            bus_ok: true,
            qnm_mounted: true,
            on_etcd: true,
            peer_count: 2,
        };
        let v = build_readiness(&p, &[], &[], 0);
        assert_eq!(val(&v, 4, "label"), "Mesh coordination (etcd)");
        assert_eq!(val(&v, 4, "detail"), "etcd reachable");
        // etcd down → "etcd unreachable".
        let p2 = BootProbe {
            qnm_mounted: false,
            ..p
        };
        let v2 = build_readiness(&p2, &[], &[], 0);
        assert_eq!(val(&v2, 4, "detail"), "etcd unreachable");
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
            on_etcd: false,
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
            on_etcd: false,
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

    #[test]
    fn oversized_host_command_output_fails_closed() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "printf '%05000d' 0"]);
        let output = bounded_command_stdout(&mut command, MAX_SYSTEMCTL_STDOUT_BYTES);
        assert_eq!(output, None);
    }
}
