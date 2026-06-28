//! UNIFY-14 — the `service_status` worker: publishes THIS node's mesh
//! service-status map so peers can render the Unified Workbench node×service
//! matrix (`docs/design/workbench/Workbench.dc.html`) with real per-node data.
//!
//! Each tick the worker samples which of the nine canonical mesh services are
//! live on this box and publishes a [`ServiceStatusMap`] on two lanes, mirroring
//! [`compute_registry`](super::compute_registry):
//!
//! - **Bus** (`state/service-status/<overlay_ip>`) — the local-node lane,
//!   on-change + a slow heartbeat (reuses [`compute_registry::should_publish`]
//!   so the cadence policy is single-sourced, BUS-RUN-FULL-1 / ADR-0005).
//! - **QNM-Shared** (`<mount>/<hostname>/service-status.json`) — the replicated
//!   cross-node lane every peer's Workbench reads to fill the matrix (the Bus
//!   topic is per-node; the cross-node fleet view reads the mirrored file,
//!   exactly like `compute-inventory.json`).
//!
//! ## Real liveness signals (§7 — no fabricated up/down)
//!
//! The nine services are heterogeneous, so each carries its own real probe:
//!
//! | service     | signal                                                        |
//! |-------------|---------------------------------------------------------------|
//! | `nebula`    | `systemctl is-active nebula`                                   |
//! | `etcd`      | `systemctl is-active etcd`                                     |
//! | `syncthing` | `systemctl is-active syncthing`                                |
//! | `voice`     | `systemctl is-active kamailio-mde` (SIP control plane)         |
//! | `music`     | `systemctl is-active mcnf-navidrome` (the `music.mesh` server) |
//! | `bus`       | the embedded broker's spool `index.sqlite` exists (no unit)    |
//! | `dns`       | the `mesh_dns` managed `/etc/hosts` block is present           |
//! | `kdc`       | the in-process KDE Connect listener accepts a localhost connect|
//! | `workbench` | the `mde-workbench` desktop process is running                 |
//!
//! When a node genuinely can't determine a signal (e.g. `systemctl` is absent on
//! a non-systemd host) the service reports [`ServiceState::Unknown`] — an honest
//! "—", never a guessed value.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mackes_mesh_types::service_status::{MeshService, ServiceState, ServiceStatusMap};

use super::compute_registry::should_publish;
use super::{ShutdownToken, Worker};

/// Sample cadence. Service liveness changes rarely, so a 30 s slow tick is
/// plenty — the on-change publish makes a real transition visible promptly.
pub const SAMPLE_TICK: Duration = Duration::from_secs(30);

/// Slow heartbeat for the Bus publish: between heartbeats we publish only when
/// the map changed; once this elapses we republish unchanged so a pruned topic /
/// late subscriber still finds a recent doc (mirrors
/// [`compute_registry::PUBLISH_HEARTBEAT`](super::compute_registry::PUBLISH_HEARTBEAT),
/// stretched because service status is far less volatile than VM inventory).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(300);

/// Bus topic prefix; the per-node topic is `state/service-status/<overlay_ip>`.
pub const TOPIC_PREFIX: &str = "state/service-status";

/// File name the per-node map is mirrored to under its QNM-Shared dir.
pub const SHARED_STATUS_FILE: &str = "service-status.json";

/// systemd units sampled via `systemctl is-active`, paired with their service.
/// `voice` is keyed on the Kamailio SIP control plane (`rtpengine-mde` is the
/// paired media relay); `music` on Navidrome, the server `music.mesh` resolves
/// to (`media_overlay_ips` in `mesh_dns`).
const SYSTEMD_UNITS: &[(MeshService, &str)] = &[
    (MeshService::Etcd, "etcd"),
    (MeshService::Syncthing, "syncthing"),
    (MeshService::Nebula, "nebula"),
    (MeshService::Voice, "kamailio-mde"),
    (MeshService::Music, "mcnf-navidrome"),
];

/// KDE Connect's stock UDP/TCP port (mirrors `kdc_host::KDC_PORT`, duplicated
/// here so this probe doesn't depend on that worker's private const).
const KDC_PORT: u16 = 1716;

/// The Workbench desktop binary (`packaging/applications/org.magicmesh.Workbench.desktop`).
const WORKBENCH_BIN: &str = "mde-workbench";

/// One full sample of the nine services' liveness (the impure inputs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSample {
    /// Mackes Bus broker.
    pub bus: ServiceState,
    /// etcd coordination plane.
    pub etcd: ServiceState,
    /// Syncthing replication plane.
    pub syncthing: ServiceState,
    /// Nebula overlay transport.
    pub nebula: ServiceState,
    /// Mesh DNS.
    pub dns: ServiceState,
    /// Voice (SIP/RTP).
    pub voice: ServiceState,
    /// Music (Navidrome).
    pub music: ServiceState,
    /// KDE Connect host.
    pub kdc: ServiceState,
    /// Workbench desktop surface.
    pub workbench: ServiceState,
}

/// Classify a `systemctl is-active <unit>` result into a [`ServiceState`].
///
/// `spawn_ok == false` means the `systemctl` invocation itself failed (e.g. no
/// systemd on this host) — that's a genuine [`ServiceState::Unknown`], not a
/// fabricated down. Otherwise the unit is [`ServiceState::Active`] only on the
/// exact `active` token (`activating` / `failed` / `inactive` / `unknown` all
/// map to [`ServiceState::Inactive`]). Pure + tested.
#[must_use]
pub fn classify_systemctl(spawn_ok: bool, stdout: &str) -> ServiceState {
    if !spawn_ok {
        return ServiceState::Unknown;
    }
    if stdout.trim() == "active" {
        ServiceState::Active
    } else {
        ServiceState::Inactive
    }
}

/// Map a deterministic boolean liveness signal (a file / port / process probe,
/// which can't be "unknown") to Active/Inactive. Pure + tested.
#[must_use]
pub fn bool_state(active: bool) -> ServiceState {
    if active {
        ServiceState::Active
    } else {
        ServiceState::Inactive
    }
}

/// `dns` is daemonless: the `mesh_dns` worker always merges a managed
/// `<host>.mesh` block into `/etc/hosts` (its bulletproof fallback, applied even
/// where `resolvectl` is absent). The block's presence is the real "mesh DNS is
/// configured on this node" signal. Pure over the hosts-file contents + tested.
#[must_use]
pub fn classify_dns(hosts_contents: &str) -> ServiceState {
    bool_state(hosts_contents.contains(super::mesh_dns::HOSTS_BEGIN))
}

/// The Bus per-node topic for `overlay_ip`. Pure + tested.
#[must_use]
pub fn status_topic(overlay_ip: &str) -> String {
    format!("{TOPIC_PREFIX}/{overlay_ip}")
}

/// Assemble the published map from a gathered [`ServiceSample`] + node identity.
/// The pure core (mirrors `compute_registry::build_inventory`) so the wire shape
/// is unit-tested without touching `systemctl` / the fs. Every one of the nine
/// canonical services is recorded, so the matrix always has a full row.
#[must_use]
pub fn assemble(
    hostname: &str,
    overlay_ip: &str,
    ts_ms: u64,
    sample: &ServiceSample,
) -> ServiceStatusMap {
    ServiceStatusMap::new(hostname, overlay_ip, ts_ms)
        .with(MeshService::Bus, sample.bus)
        .with(MeshService::Etcd, sample.etcd)
        .with(MeshService::Syncthing, sample.syncthing)
        .with(MeshService::Nebula, sample.nebula)
        .with(MeshService::Dns, sample.dns)
        .with(MeshService::Voice, sample.voice)
        .with(MeshService::Music, sample.music)
        .with(MeshService::Kdc, sample.kdc)
        .with(MeshService::Workbench, sample.workbench)
}

/// Mirror the map to the replicated QNM-Shared plane at
/// `<mount>/<hostname>/service-status.json` (mirrors
/// `compute_registry::write_shared_inventory`). Atomic (tmp + rename) so a
/// reader never sees a half-written file; best-effort + non-fatal.
pub fn write_shared_status(mount: &Path, hostname: &str, map: &ServiceStatusMap) {
    if hostname.is_empty() {
        return;
    }
    let dir = mount.join(hostname);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("service_status: mkdir {} failed: {e}", dir.display());
        return;
    }
    let Ok(body) = serde_json::to_string(map) else {
        return;
    };
    let tmp = dir.join("service-status.json.tmp");
    let final_path = dir.join(SHARED_STATUS_FILE);
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        tracing::warn!("service_status: write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        tracing::warn!("service_status: rename status failed: {e}");
    }
}

/// Publish a map to `state/service-status/<overlay_ip>` via the `mde-bus` CLI
/// (mirrors `compute_registry::publish_inventory`).
pub fn publish_status(overlay_ip: &str, map: &ServiceStatusMap) {
    let topic = status_topic(overlay_ip);
    let Ok(body) = serde_json::to_string(map) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// `systemctl is-active <unit>` → [`ServiceState`] via [`classify_systemctl`].
fn systemd_state(unit: &str) -> ServiceState {
    match Command::new("systemctl").args(["is-active", unit]).output() {
        Ok(out) => classify_systemctl(true, &String::from_utf8_lossy(&out.stdout)),
        // The binary couldn't be invoked at all (non-systemd host) ⇒ Unknown.
        Err(_) => classify_systemctl(false, ""),
    }
}

/// The embedded Bus broker has no systemd unit (mackesd embeds it as a library);
/// the broker creates `index.sqlite` in its spool root, which `mde_bus`
/// itself uses as the "a system bus is live here" signal ([`mde_bus::client_data_dir`]).
fn bus_state() -> ServiceState {
    let live = mde_bus::default_data_dir().is_some_and(|d| d.join("index.sqlite").exists());
    bool_state(live)
}

/// Mesh DNS state from the managed `/etc/hosts` block.
fn dns_state(hosts_path: &Path) -> ServiceState {
    match std::fs::read_to_string(hosts_path) {
        Ok(contents) => classify_dns(&contents),
        // Can't read /etc/hosts ⇒ can't tell ⇒ honest Unknown.
        Err(_) => ServiceState::Unknown,
    }
}

/// A bounded localhost TCP connect (300 ms) — `true` if `addr` accepts it.
/// Used for the in-process KDE Connect listener (no systemd unit).
fn tcp_open(addr: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    addr.to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .and_then(|sa| TcpStream::connect_timeout(&sa, Duration::from_millis(300)).ok())
        .is_some()
}

/// `true` if any process's `comm` matches `name` (a dependency-free `pgrep`).
/// The kernel truncates `comm` to 15 bytes, so callers must pass a ≤15-char
/// binary name (`mde-workbench` is 13).
fn process_running(name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Only numeric /proc/<pid> dirs.
        let is_pid = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !is_pid {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(path.join("comm")) {
            if comm.trim() == name {
                return true;
            }
        }
    }
    false
}

/// Read this node's overlay IP from the path `nebula_supervisor` writes, trimmed.
/// Empty until Nebula assigns one (peer not yet enrolled).
fn local_overlay_ip() -> String {
    std::fs::read_to_string(super::nebula_supervisor::DEFAULT_OVERLAY_IP_PATH)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// The `service_status` worker handle.
pub struct ServiceStatusWorker {
    hostname: String,
    /// Overlay-IP override; empty ⇒ runtime-detect from the nebula_supervisor path.
    overlay_ip: String,
    tick: Duration,
    publish_heartbeat: Duration,
    hosts_path: PathBuf,
    /// QNM-Shared mount the cross-node mirror is written under.
    mount: PathBuf,
    /// On-change + slow-heartbeat Bus-publish state: last body + when.
    last_publish: Mutex<Option<(String, Instant)>>,
}

impl ServiceStatusWorker {
    /// Construct with production defaults. `overlay_ip_hint` overrides runtime
    /// detection (empty string defers to the nebula_supervisor overlay-ip path).
    #[must_use]
    pub fn new(hostname: String, overlay_ip_hint: String) -> Self {
        Self {
            hostname,
            overlay_ip: overlay_ip_hint,
            tick: SAMPLE_TICK,
            publish_heartbeat: PUBLISH_HEARTBEAT,
            hosts_path: PathBuf::from("/etc/hosts"),
            mount: crate::default_qnm_shared_root(),
            last_publish: Mutex::new(None),
        }
    }

    /// Override the QNM-Shared mount (tests / non-standard deploys).
    #[must_use]
    pub fn with_mount(mut self, p: PathBuf) -> Self {
        self.mount = p;
        self
    }

    /// Override the hosts-file path (tests).
    #[must_use]
    pub fn with_hosts_path(mut self, p: PathBuf) -> Self {
        self.hosts_path = p;
        self
    }

    /// Override the sample cadence (tests).
    #[must_use]
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    fn resolve_overlay_ip(&self) -> String {
        if self.overlay_ip.is_empty() {
            local_overlay_ip()
        } else {
            self.overlay_ip.clone()
        }
    }

    /// Gather the live nine-service sample (impure: systemctl, fs, net, /proc).
    fn sample(&self) -> ServiceSample {
        let mut units = std::collections::BTreeMap::new();
        for (svc, unit) in SYSTEMD_UNITS {
            units.insert(*svc, systemd_state(unit));
        }
        let unit = |svc: MeshService| units.get(&svc).copied().unwrap_or(ServiceState::Unknown);
        ServiceSample {
            bus: bus_state(),
            etcd: unit(MeshService::Etcd),
            syncthing: unit(MeshService::Syncthing),
            nebula: unit(MeshService::Nebula),
            dns: dns_state(&self.hosts_path),
            voice: unit(MeshService::Voice),
            music: unit(MeshService::Music),
            kdc: bool_state(tcp_open(&format!("127.0.0.1:{KDC_PORT}"))),
            workbench: bool_state(process_running(WORKBENCH_BIN)),
        }
    }

    fn tick_once(&self) {
        let overlay_ip = self.resolve_overlay_ip();
        let sample = self.sample();
        let map = assemble(&self.hostname, &overlay_ip, now_ms(), &sample);

        // Bus lane: publish on-change + slow heartbeat. ts_ms advances every
        // tick, so we feed the cadence policy the stamp-free [`change_key`] —
        // otherwise the body would always differ and the on-change short-circuit
        // could never skip an unchanged sample. We reuse the compute_registry
        // policy over that key.
        {
            let key = change_key(&map);
            let mut last = self
                .last_publish
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let prev_body = last.as_ref().map(|(b, _)| b.as_str());
            let prev_at = last.as_ref().map(|(_, at)| *at);
            if should_publish(prev_body, &key, prev_at, now, self.publish_heartbeat) {
                publish_status(&overlay_ip, &map);
                *last = Some((key, now));
            }
        }

        // QNM-Shared lane: mirror to the replicated plane peers read for the
        // cross-node matrix. Only when the mount is a real meshfs mount — never
        // write to a bare local dir masquerading as the share (reuses
        // compute_registry's /proc/mounts check).
        if super::compute_registry::is_meshfs_mounted(&self.mount) {
            write_shared_status(&self.mount, &self.hostname, &map);
        }
    }
}

/// Stamp-free serialization of the map for the on-change comparison: the
/// `ts_ms` advances every tick, so including it would defeat the on-change
/// short-circuit and republish identical service state every tick. Pure +
/// tested.
#[must_use]
pub fn change_key(map: &ServiceStatusMap) -> String {
    let mut stamp_free = map.clone();
    stamp_free.ts_ms = 0;
    serde_json::to_string(&stamp_free).unwrap_or_default()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[async_trait::async_trait]
impl Worker for ServiceStatusWorker {
    fn name(&self) -> &'static str {
        "service_status"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => {
                    // Sync sample + publish (brief systemctl/fs/net probes,
                    // bounded by the 300 ms tcp connect + fire-and-reap publish),
                    // mirroring compute_registry's tick. The supervisor wraps this
                    // worker in RestartPolicy::Always.
                    self.tick_once();
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- classify_systemctl ---

    #[test]
    fn systemctl_active_token_only_is_active() {
        assert_eq!(classify_systemctl(true, "active\n"), ServiceState::Active);
        assert_eq!(classify_systemctl(true, "  active  "), ServiceState::Active);
        // activating is NOT yet active.
        assert_eq!(
            classify_systemctl(true, "activating\n"),
            ServiceState::Inactive
        );
        assert_eq!(
            classify_systemctl(true, "inactive\n"),
            ServiceState::Inactive
        );
        assert_eq!(classify_systemctl(true, "failed\n"), ServiceState::Inactive);
        // systemctl prints "unknown" for an unloaded unit — that's a real
        // "not running here", i.e. Inactive (the host CAN answer).
        assert_eq!(
            classify_systemctl(true, "unknown\n"),
            ServiceState::Inactive
        );
    }

    #[test]
    fn systemctl_spawn_failure_is_unknown() {
        // No systemd on the host ⇒ honest Unknown, never a fabricated down.
        assert_eq!(classify_systemctl(false, ""), ServiceState::Unknown);
        assert_eq!(classify_systemctl(false, "active"), ServiceState::Unknown);
    }

    // --- bool_state / classify_dns ---

    #[test]
    fn bool_state_maps_deterministic_signal() {
        assert_eq!(bool_state(true), ServiceState::Active);
        assert_eq!(bool_state(false), ServiceState::Inactive);
    }

    #[test]
    fn classify_dns_reads_managed_block_marker() {
        let with_block = format!(
            "127.0.0.1 localhost\n{}\n10.42.0.2 forge.mesh\n{}\n",
            super::super::mesh_dns::HOSTS_BEGIN,
            super::super::mesh_dns::HOSTS_END
        );
        assert_eq!(classify_dns(&with_block), ServiceState::Active);
        // No managed block ⇒ mesh DNS not configured here.
        assert_eq!(
            classify_dns("127.0.0.1 localhost\n"),
            ServiceState::Inactive
        );
    }

    // --- status_topic ---

    #[test]
    fn status_topic_is_per_overlay_ip() {
        assert_eq!(status_topic("10.42.0.5"), "state/service-status/10.42.0.5");
        // Empty overlay (pre-enrollment) keeps the topic shape consistent.
        assert_eq!(status_topic(""), "state/service-status/");
    }

    // --- assemble (the sampling → wire-shape core) ---

    fn sample_all(state: ServiceState) -> ServiceSample {
        ServiceSample {
            bus: state,
            etcd: state,
            syncthing: state,
            nebula: state,
            dns: state,
            voice: state,
            music: state,
            kdc: state,
            workbench: state,
        }
    }

    #[test]
    fn assemble_records_every_canonical_service() {
        let sample = sample_all(ServiceState::Active);
        let map = assemble("forge", "10.42.0.3", 7, &sample);
        assert_eq!(map.hostname, "forge");
        assert_eq!(map.overlay_ip, "10.42.0.3");
        assert_eq!(map.ts_ms, 7);
        // All nine present + Active.
        assert_eq!(map.services.len(), 9);
        for svc in MeshService::ALL {
            assert_eq!(map.state(svc), ServiceState::Active, "{}", svc.id());
        }
    }

    #[test]
    fn assemble_preserves_mixed_states() {
        let sample = ServiceSample {
            bus: ServiceState::Active,
            etcd: ServiceState::Active,
            syncthing: ServiceState::Inactive,
            nebula: ServiceState::Active,
            dns: ServiceState::Active,
            voice: ServiceState::Inactive,
            music: ServiceState::Inactive,
            kdc: ServiceState::Unknown,
            workbench: ServiceState::Inactive,
        };
        let map = assemble("anvil", "", 0, &sample);
        assert_eq!(map.state(MeshService::Bus), ServiceState::Active);
        assert_eq!(map.state(MeshService::Syncthing), ServiceState::Inactive);
        assert_eq!(map.state(MeshService::Kdc), ServiceState::Unknown);
        // Round-trips on the wire with the mixed states intact.
        let json = serde_json::to_string(&map).unwrap();
        let back: ServiceStatusMap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, map);
    }

    // --- change_key (stamp-free on-change comparison) ---

    #[test]
    fn change_key_ignores_timestamp_but_tracks_state() {
        let sample = sample_all(ServiceState::Active);
        let a = assemble("forge", "10.42.0.3", 1, &sample);
        let b = assemble("forge", "10.42.0.3", 999, &sample);
        // Same services, different ts_ms ⇒ same change key (no spurious republish).
        assert_eq!(change_key(&a), change_key(&b));
        // A real state change flips the key.
        let mut changed = b.clone();
        changed.set(MeshService::Nebula, ServiceState::Inactive);
        assert_ne!(change_key(&b), change_key(&changed));
    }

    // --- write_shared_status (cross-node mirror) ---

    #[test]
    fn write_shared_status_lands_under_hostname_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let map = assemble("forge", "10.42.0.3", 5, &sample_all(ServiceState::Active));
        write_shared_status(tmp.path(), "forge", &map);
        let path = tmp.path().join("forge").join(SHARED_STATUS_FILE);
        let body = std::fs::read_to_string(&path).expect("status file written");
        let back: ServiceStatusMap = serde_json::from_str(&body).unwrap();
        assert_eq!(back, map);
        // Atomic write leaves no temp file behind.
        assert!(!tmp
            .path()
            .join("forge")
            .join("service-status.json.tmp")
            .exists());
    }

    #[test]
    fn write_shared_status_skips_empty_hostname() {
        let tmp = tempfile::tempdir().unwrap();
        let map = assemble("", "", 0, &sample_all(ServiceState::Unknown));
        write_shared_status(tmp.path(), "", &map);
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    // --- dns_state over a real temp hosts file (impure shell, pure decision) ---

    #[test]
    fn dns_state_reads_temp_hosts_file() {
        let tmp = tempfile::tempdir().unwrap();
        let hosts = tmp.path().join("hosts");
        std::fs::write(
            &hosts,
            format!(
                "127.0.0.1 localhost\n{}\n10.42.0.2 forge.mesh\n{}\n",
                super::super::mesh_dns::HOSTS_BEGIN,
                super::super::mesh_dns::HOSTS_END
            ),
        )
        .unwrap();
        assert_eq!(dns_state(&hosts), ServiceState::Active);
        // A missing hosts file ⇒ honest Unknown (can't read ⇒ can't tell).
        assert_eq!(dns_state(&tmp.path().join("absent")), ServiceState::Unknown);
    }

    #[test]
    fn worker_name_is_stable() {
        let w = ServiceStatusWorker::new("forge".into(), String::new());
        assert_eq!(w.name(), "service_status");
    }
}
