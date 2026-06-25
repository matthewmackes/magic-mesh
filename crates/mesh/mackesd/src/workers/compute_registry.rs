//! VIRT-1 (v5.0.0) — unified KVM + Podman compute inventory.
//!
//! Polls `virsh list --all --uuid` + `virsh dominfo <uuid>` +
//! `virsh domblklist <uuid>` for KVM guests and `podman ps --all
//! --format json` + `podman stats --no-stream --format json` for
//! containers, assembles the per-peer inventory described in
//! `docs/design/v5.0.0-compute.md` §3, and publishes it to
//! `compute/inventory/<peer-nebula-addr>` on the Mackes Bus every
//! 10 s.
//!
//! ## Design locks (v5.0.0-compute.md §1..3)
//!
//! - 10 s **poll** cadence (§1 / §3): virsh/podman are polled every
//!   10 s so VM state-transition events (VIRT-21) and the replicated
//!   QNM-Shared inventory file stay timely.
//! - **Bus publish is on-change + a slow heartbeat** (BUS-RUN-FULL-1,
//!   `docs/DECISIONS.md` ADR-0005). The `compute/inventory/<peer>` bus
//!   topic has exactly one consumer — *this* node's own Workloads
//!   source (`read_local_inventory`); the cross-node fleet view reads
//!   the replicated `compute-inventory.json` file, not the bus. The
//!   consumer only ever wants the *latest* doc, so republishing an
//!   identical body every 10 s just grew the append-only Persist log
//!   (8 640 redundant msgs/peer/day at idle). We now publish only when
//!   the body changed since the last publish, plus an unconditional
//!   heartbeat every [`PUBLISH_HEARTBEAT`] so a freshly-pruned topic /
//!   late subscriber still finds a recent doc.
//! - Subprocess-based polling — no libvirt-rs FFI, no system
//!   libvirt-dev dep. Matches the `firewall_monitor` PEERVER pattern.
//! - libvirtd is socket-activated (§5); the first `virsh` call
//!   triggers libvirtd.service via `libvirtd.socket`.
//! - `meshfs_available: bool` is host-level (mount-point check) and
//!   mirrored into every VM entry so the Workbench can flag VMs that
//!   asked for `share_meshfs=true` but the host can't honor it.
//! - Silent no-op when `virsh` / `podman` binaries are absent —
//!   matches the lighthouse + container-stripped peer profiles.
//! - `nebula_ip` is read from a sidecar file written by
//!   `compute_provision` (VIRT-6) at VM creation; empty until that
//!   worker ships, which is fine for VIRT-1 since the first cycle on
//!   a fresh peer publishes an empty `vms` list anyway.
//! - `cpu_pct` is computed from the delta of `virsh domstats --vcpu`
//!   total VCPU nanoseconds between consecutive ticks; the first tick
//!   for a given VM reports 0.0 because no prior sample exists.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// 10 s poll cadence per design doc §1 / §3.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(10);

/// BUS-RUN-FULL-1 — slow heartbeat for the `compute/inventory/<peer>` bus publish.
///
/// Between heartbeats we publish only when the inventory body changed; once
/// this interval elapses we republish unconditionally so a freshly-pruned topic
/// or a late subscriber still finds a recent doc. 60 s republishes ~6× less than
/// the old every-tick publish at idle while keeping the local Workloads source's
/// bus copy current within a minute.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);

/// Directory holding per-VM Nebula-IP sidecar files
/// (`<vm-storage>/<uuid>.nebula-ip`) written by `compute_provision`.
pub const DEFAULT_VM_STORAGE: &str = "/var/lib/mde-vms";

/// One VM entry in the published inventory.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VmEntry {
    /// libvirt domain UUID.
    pub id: String,
    /// libvirt domain name.
    pub name: String,
    /// libvirt state string (`running`, `shut off`, `paused`, ...).
    pub state: String,
    /// CPU percent over the last tick interval (sum across vcpus,
    /// capped at `vcpus * 100`). 0.0 on the first sample.
    pub cpu_pct: f64,
    /// Currently-used RAM in MiB (parsed from `Used memory` KiB).
    pub ram_mb: u64,
    /// Path to the first non-cdrom disk source.
    pub disk_path: String,
    /// VM's Nebula overlay IP, sourced from a sidecar file written by
    /// `compute_provision` (VIRT-6). Empty when no sidecar exists.
    pub nebula_ip: String,
    /// Mirror of the host's `meshfs_available`: lets the Workbench
    /// badge VMs whose `share_meshfs=true` request can't be honored.
    pub meshfs_available: bool,
}

/// One container entry in the published inventory.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ContainerEntry {
    /// Podman container ID (full, not truncated).
    pub id: String,
    /// First entry from podman's `Names` array.
    pub name: String,
    /// Podman state string (`running`, `exited`, ...).
    pub state: String,
    /// Image reference.
    pub image: String,
    /// CPU percent from `podman stats --no-stream` (parsed `"3.10%"`).
    pub cpu_pct: f64,
    /// Used memory in MiB (parsed from `MemUsage`'s "512MiB / 16GiB").
    pub ram_mb: u64,
    /// Pod this container belongs to (`""` when standalone). VIRT-18.b —
    /// `#[serde(default)]` so pre-pod inventory still deserializes.
    #[serde(default)]
    pub pod: String,
}

/// Top-level inventory document published per tick.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Inventory {
    /// This peer's Nebula overlay IP (the topic suffix).
    pub peer: String,
    /// `/etc/hostname` content (display label for the Workbench list).
    pub hostname: String,
    /// VM rows discovered via virsh.
    pub vms: Vec<VmEntry>,
    /// Container rows discovered via podman.
    pub containers: Vec<ContainerEntry>,
}

/// Parsed virsh dominfo fields used by inventory assembly. Public for
/// test access only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DominfoFields {
    /// `Name` field.
    pub name: String,
    /// `State` field (free-form virsh string).
    pub state: String,
    /// `Used memory` converted from KiB to MiB.
    pub ram_mb: u64,
    /// `CPU(s)` count (used for cpu_pct cap).
    pub vcpus: u32,
}

/// Parse a `virsh list --all --uuid` payload into UUIDs (one per
/// non-blank line).
pub fn parse_virsh_uuid_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Parse a `virsh dominfo <uuid>` payload. Returns `None` only when
/// the required Name + State fields are absent.
pub fn parse_virsh_dominfo(stdout: &str) -> Option<DominfoFields> {
    let mut out = DominfoFields::default();
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "Name" => out.name = value.to_string(),
            "State" => out.state = value.to_string(),
            "Used memory" => {
                // Format: `2097152 KiB` — first whitespace-delimited
                // token is the KiB count.
                if let Some(kib_str) = value.split_whitespace().next() {
                    if let Ok(kib) = kib_str.parse::<u64>() {
                        out.ram_mb = kib / 1024;
                    }
                }
            }
            "CPU(s)" => {
                if let Ok(n) = value.parse::<u32>() {
                    out.vcpus = n;
                }
            }
            _ => {}
        }
    }
    if out.name.is_empty() || out.state.is_empty() {
        return None;
    }
    Some(out)
}

/// Parse a `virsh domblklist --details <uuid>` payload. Returns the
/// first non-cdrom disk source path, or `None` when no disk row is
/// present.
pub fn parse_virsh_domblklist(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Expected row: `Type Device Target Source`.
        if cols.len() < 4 {
            continue;
        }
        let device = cols[1];
        let source = cols[3];
        if device == "disk" && source != "-" {
            return Some(source.to_string());
        }
    }
    None
}

/// Parse a `virsh domstats <uuid> --vcpu` payload into total VCPU
/// time in nanoseconds (sum across all `vcpu.N.time` rows). Returns
/// `None` when no `vcpu.*.time` rows are present.
pub fn parse_virsh_domstats_vcpu_time(stdout: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut any = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        // Rows look like `vcpu.0.time=12345`.
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        if !key.starts_with("vcpu.") || !key.ends_with(".time") {
            continue;
        }
        if let Ok(ns) = value.parse::<u64>() {
            total = total.saturating_add(ns);
            any = true;
        }
    }
    if any {
        Some(total)
    } else {
        None
    }
}

/// Compute CPU percent from two cumulative VCPU-time samples.
///
/// Returns 0.0 when the previous sample is absent (first tick), when
/// the interval is non-positive, or when the counter decreased
/// (VM reset / migrated). Caps at `vcpus * 100.0` to prevent
/// double-counting if a sample races a counter rollover.
pub fn cpu_pct_delta(prev_ns: Option<u64>, cur_ns: u64, interval_secs: f64, vcpus: u32) -> f64 {
    let Some(prev) = prev_ns else { return 0.0 };
    if cur_ns < prev || interval_secs <= 0.0 {
        return 0.0;
    }
    let delta_ns = (cur_ns - prev) as f64;
    let delta_secs = delta_ns / 1_000_000_000.0;
    let pct = (delta_secs / interval_secs) * 100.0;
    let cap = (vcpus.max(1) as f64) * 100.0;
    pct.min(cap)
}

/// Parse the JSON payload of `podman ps --all --format json` into
/// container rows. Empty array / missing podman returns an empty Vec.
pub fn parse_podman_ps_json(stdout: &str) -> Vec<ContainerEntry> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return vec![];
    };
    rows.into_iter()
        .filter_map(|row| {
            let id = row.get("Id")?.as_str()?.to_string();
            let name = row
                .get("Names")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let state = row
                .get("State")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let image = row
                .get("Image")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let pod = row
                .get("PodName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(ContainerEntry {
                id,
                name,
                state,
                image,
                cpu_pct: 0.0,
                ram_mb: 0,
                pod,
            })
        })
        .collect()
}

/// Parse the JSON payload of `podman stats --no-stream --format json
/// --no-trunc` into `(container_id → (cpu_pct, ram_mb))` so callers
/// can merge stats into the rows from [`parse_podman_ps_json`].
pub fn parse_podman_stats_json(stdout: &str) -> BTreeMap<String, (f64, u64)> {
    let mut out = BTreeMap::new();
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return out;
    };
    for row in rows {
        let Some(id) = row
            .get("ContainerID")
            .or_else(|| row.get("Id"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let cpu_pct = row
            .get("CPU")
            .and_then(|v| v.as_str())
            .and_then(|s| s.trim_end_matches('%').trim().parse::<f64>().ok())
            .unwrap_or(0.0);
        let ram_mb = row
            .get("MemUsage")
            .and_then(|v| v.as_str())
            .map(parse_podman_mem_usage)
            .unwrap_or(0);
        out.insert(id.to_string(), (cpu_pct, ram_mb));
    }
    out
}

/// Parse a podman `MemUsage` string ("512MiB / 16GiB") into MiB. The
/// first whitespace-delimited token before any `/` is the used
/// amount; we honor MiB / GiB / KiB suffixes.
pub fn parse_podman_mem_usage(value: &str) -> u64 {
    let head = value.split('/').next().unwrap_or("").trim();
    let (num_part, unit_part) = head
        .find(|c: char| c.is_alphabetic())
        .map(|i| (&head[..i], &head[i..]))
        .unwrap_or((head, ""));
    let num: f64 = num_part.trim().parse().unwrap_or(0.0);
    match unit_part.trim().to_ascii_uppercase().as_str() {
        "KIB" | "KB" => (num / 1024.0) as u64,
        "MIB" | "MB" | "" => num as u64,
        "GIB" | "GB" => (num * 1024.0) as u64,
        "TIB" | "TB" => (num * 1024.0 * 1024.0) as u64,
        _ => num as u64,
    }
}

/// Returns `true` when `mount_path` appears in `/proc/mounts`.
/// Returns `false` if `/proc/mounts` is unreadable (containerised CI
/// peer, non-Linux test machine).
pub fn is_meshfs_mounted(mount_path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string("/proc/mounts") else {
        return false;
    };
    let needle = mount_path.to_string_lossy();
    content
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .any(|m| m == needle)
}

/// Read `<storage_dir>/<uuid>.nebula-ip` if present, returning the
/// trimmed contents. Returns empty string when the file is missing —
/// `compute_provision` (VIRT-6) writes it at VM creation, so for VMs
/// created via raw virsh before VIRT-6 ships the field stays empty.
pub fn read_vm_nebula_ip(storage_dir: &Path, uuid: &str) -> String {
    let path = storage_dir.join(format!("{uuid}.nebula-ip"));
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Build the published inventory document from already-collected
/// VM and container rows. `meshfs_available` is mirrored into every
/// VM entry per §3 schema.
pub fn build_inventory(
    peer: &str,
    hostname: &str,
    vms: Vec<VmEntry>,
    containers: Vec<ContainerEntry>,
    meshfs_available: bool,
) -> Inventory {
    let vms_with_meshfs: Vec<VmEntry> = vms
        .into_iter()
        .map(|mut v| {
            v.meshfs_available = meshfs_available;
            v
        })
        .collect();
    Inventory {
        peer: peer.to_string(),
        hostname: hostname.to_string(),
        vms: vms_with_meshfs,
        containers,
    }
}

/// Read the local peer's Nebula overlay IP from
/// `ip -4 addr show nebula1`. Empty string when the interface is
/// absent (peer not yet enrolled / Nebula stopped).
fn local_nebula_addr() -> String {
    let Ok(output) = Command::new("ip")
        .args(["-4", "addr", "show", "nebula1"])
        .output()
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("inet ") {
            // `inet 10.42.0.1/17 ...` — first token before `/`.
            if let Some(ip) = rest.split('/').next() {
                return ip.to_string();
            }
        }
    }
    String::new()
}

fn binary_present(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok()
}

fn run_virsh(args: &[&str]) -> String {
    Command::new("virsh")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn run_podman(args: &[&str]) -> String {
    Command::new("podman")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Publish an inventory document to `compute/inventory/<peer>` via
/// the `mde-bus` CLI. Pub so `compute_provision` (VIRT-6) can fire
/// an immediate post-create publish without instantiating a full
/// registry worker.
pub fn publish_inventory(peer: &str, inv: &Inventory) {
    let topic = format!("compute/inventory/{peer}");
    let Ok(body) = serde_json::to_string(inv) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// BUS-RUN-FULL-1 — decide whether this tick should publish the bus
/// inventory. Publishes when the serialized body differs from the last
/// published one (on-change), or when at least `heartbeat` has elapsed
/// since the last publish (slow heartbeat so a pruned topic / late
/// subscriber still finds a recent doc). `last_publish == None` (first
/// tick) always publishes. Pure so the cadence policy is unit-tested.
#[must_use]
pub fn should_publish(
    last_body: Option<&str>,
    cur_body: &str,
    last_publish: Option<std::time::Instant>,
    now: std::time::Instant,
    heartbeat: Duration,
) -> bool {
    match (last_body, last_publish) {
        (Some(prev), Some(at)) => prev != cur_body || now.duration_since(at) >= heartbeat,
        // No prior publish (worker just started) ⇒ always publish.
        _ => true,
    }
}

/// File name this node's inventory is mirrored to under its QNM-Shared dir.
pub const SHARED_INVENTORY_FILE: &str = "compute-inventory.json";

/// WORKLOAD-FLEET-1 — mirror this node's inventory to the replicated
/// QNM-Shared plane at `<mount>/<hostname>/compute-inventory.json`.
///
/// Every node's Workbench reads these files to render fleet-wide workloads. The
/// bus publish is per-node (no federation worker); QNM-Shared is the cross-node
/// plane, exactly like `shell-status.json`. Atomic (tmp + rename) so a reader
/// never sees a half-written file. Best-effort: a missing mount / write error is
/// logged, never fatal (the caller only calls this when the mount is available).
pub fn write_shared_inventory(mount: &Path, hostname: &str, inv: &Inventory) {
    if hostname.is_empty() {
        return;
    }
    let dir = mount.join(hostname);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("compute_registry: mkdir {} failed: {e}", dir.display());
        return;
    }
    let Ok(body) = serde_json::to_string(inv) else {
        return;
    };
    let tmp = dir.join("compute-inventory.json.tmp");
    let final_path = dir.join(SHARED_INVENTORY_FILE);
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        tracing::warn!("compute_registry: write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        tracing::warn!("compute_registry: rename inventory failed: {e}");
    }
}

/// One VM state-change event published to `compute/event/<peer>` when
/// `compute_registry` detects a transition between ticks (VIRT-21).
/// `hostname` is a superset of the design-doc §3 schema (`vm_id`,
/// `vm_name`, `event`, `peer`) so the FDO toast can read
/// "… on <hostname>" without a second inventory lookup.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComputeEvent {
    /// libvirt UUID of the VM.
    pub vm_id: String,
    /// VM display name.
    pub vm_name: String,
    /// `"started"`, `"stopped"`, or `"crashed"`.
    pub event: String,
    /// Publishing peer's Nebula overlay address.
    pub peer: String,
    /// Publishing peer's hostname (toast body).
    pub hostname: String,
}

/// Classify a VM state transition into a notifiable event, or `None`
/// when the change isn't toast-worthy.
///
/// Returns `None` on first sight (`prev == None`) so a worker (re)start
/// doesn't toast every already-running VM. Maps the three transitions
/// VIRT-21 cares about:
/// - any → `crashed`        ⇒ `"crashed"`
/// - non-running → `running` ⇒ `"started"`
/// - `running` → `shut off`  ⇒ `"stopped"`
#[must_use]
pub fn classify_transition(prev: Option<&str>, cur: &str) -> Option<&'static str> {
    let prev = prev?.trim();
    let cur = cur.trim();
    if cur == prev {
        return None;
    }
    if cur == "crashed" {
        return Some("crashed");
    }
    if prev != "running" && cur == "running" {
        return Some("started");
    }
    if prev == "running" && cur == "shut off" {
        return Some("stopped");
    }
    None
}

/// Bus topic a VM event is published to.
#[must_use]
pub fn event_topic(peer: &str) -> String {
    format!("compute/event/{peer}")
}

/// Publish a [`ComputeEvent`] to `compute/event/<peer>` via the
/// `mde-bus` CLI (mirrors [`publish_inventory`]).
pub fn publish_event(peer: &str, ev: &ComputeEvent) {
    let topic = event_topic(peer);
    let Ok(body) = serde_json::to_string(ev) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Collect VM entries via virsh. `prev` carries cumulative
/// vcpu-time-ns per uuid across calls so `cpu_pct` is a true
/// per-interval delta; pass a fresh empty map for a one-shot
/// snapshot (every VM then reports `cpu_pct = 0.0`, which is correct
/// since there's no prior sample). `meshfs_available` is left
/// `false` on each entry here and overwritten by [`build_inventory`].
pub fn collect_vm_entries(
    vm_storage: &Path,
    interval_secs: f64,
    prev: &mut BTreeMap<String, u64>,
) -> Vec<VmEntry> {
    if !binary_present("virsh") {
        return vec![];
    }
    let list_stdout = run_virsh(&["list", "--all", "--uuid"]);
    let uuids = parse_virsh_uuid_list(&list_stdout);
    let mut entries = Vec::with_capacity(uuids.len());
    for uuid in uuids {
        let info_stdout = run_virsh(&["dominfo", &uuid]);
        let Some(info) = parse_virsh_dominfo(&info_stdout) else {
            continue;
        };
        let blk_stdout = run_virsh(&["domblklist", "--details", &uuid]);
        let disk_path = parse_virsh_domblklist(&blk_stdout).unwrap_or_default();
        let stats_stdout = run_virsh(&["domstats", &uuid, "--vcpu"]);
        let cur_ns = parse_virsh_domstats_vcpu_time(&stats_stdout).unwrap_or(0);
        let prev_ns = prev.get(&uuid).copied();
        let cpu_pct = cpu_pct_delta(prev_ns, cur_ns, interval_secs, info.vcpus);
        prev.insert(uuid.clone(), cur_ns);
        entries.push(VmEntry {
            id: uuid.clone(),
            name: info.name,
            state: info.state,
            cpu_pct,
            ram_mb: info.ram_mb,
            disk_path,
            nebula_ip: read_vm_nebula_ip(vm_storage, &uuid),
            meshfs_available: false, // overwritten in build_inventory
        });
    }
    entries
}

/// Collect container entries via podman (`ps` + `stats` merge).
pub fn collect_container_entries() -> Vec<ContainerEntry> {
    if !binary_present("podman") {
        return vec![];
    }
    let ps_stdout = run_podman(&["ps", "--all", "--format", "json"]);
    let mut containers = parse_podman_ps_json(&ps_stdout);
    let stats_stdout = run_podman(&["stats", "--no-stream", "--format", "json", "--no-trunc"]);
    let stats = parse_podman_stats_json(&stats_stdout);
    for c in &mut containers {
        if let Some((cpu, ram)) = stats.get(&c.id) {
            c.cpu_pct = *cpu;
            c.ram_mb = *ram;
        }
    }
    containers
}

/// One-shot inventory snapshot with `cpu_pct = 0.0` on every VM (no
/// prior sample). Used by `compute_provision` (VIRT-6) for the
/// immediate post-create `compute/inventory/<peer>` publish so a
/// freshly-created VM appears within the §3 5 s budget rather than
/// waiting up to a full 10 s registry tick.
#[must_use]
pub fn snapshot_inventory(
    hostname: &str,
    nebula_addr: &str,
    meshfs_mount: &Path,
    vm_storage: &Path,
) -> Inventory {
    let meshfs_available = is_meshfs_mounted(meshfs_mount);
    let mut empty = BTreeMap::new();
    let vms = collect_vm_entries(vm_storage, 0.0, &mut empty);
    let containers = collect_container_entries();
    build_inventory(nebula_addr, hostname, vms, containers, meshfs_available)
}

/// Worker handle.
pub struct ComputeRegistryWorker {
    hostname: String,
    nebula_addr: String,
    tick: Duration,
    meshfs_mount: PathBuf,
    vm_storage: PathBuf,
    /// `uuid → previous vcpu-time-ns sample` for cpu_pct deltas.
    prev_cpu_ns: Mutex<BTreeMap<String, u64>>,
    /// `uuid → previous libvirt state` for VIRT-21 transition events.
    prev_state: Mutex<BTreeMap<String, String>>,
    /// BUS-RUN-FULL-1 — heartbeat for the on-change bus publish.
    publish_heartbeat: Duration,
    /// BUS-RUN-FULL-1 — last published inventory body + when, so we can
    /// publish on-change and only heartbeat-republish an unchanged doc.
    last_publish: Mutex<Option<(String, std::time::Instant)>>,
}

impl ComputeRegistryWorker {
    /// Construct with production defaults. `hostname` is the value
    /// written into the inventory's `hostname` field;
    /// `nebula_addr_hint` overrides the auto-detected nebula1 address
    /// (use empty string to defer to runtime detection).
    #[must_use]
    pub fn new(hostname: String, nebula_addr_hint: String) -> Self {
        Self {
            hostname,
            nebula_addr: nebula_addr_hint,
            tick: DEFAULT_TICK_INTERVAL,
            meshfs_mount: crate::default_qnm_shared_root(),
            vm_storage: PathBuf::from(DEFAULT_VM_STORAGE),
            prev_cpu_ns: Mutex::new(BTreeMap::new()),
            prev_state: Mutex::new(BTreeMap::new()),
            publish_heartbeat: PUBLISH_HEARTBEAT,
            last_publish: Mutex::new(None),
        }
    }

    /// Override the mesh-storage mount path. Used in tests.
    #[must_use]
    pub fn with_meshfs_mount(mut self, p: PathBuf) -> Self {
        self.meshfs_mount = p;
        self
    }

    /// Override the VM storage directory. Used in tests.
    #[must_use]
    pub fn with_vm_storage(mut self, p: PathBuf) -> Self {
        self.vm_storage = p;
        self
    }

    fn resolve_nebula_addr(&self) -> String {
        if !self.nebula_addr.is_empty() {
            return self.nebula_addr.clone();
        }
        local_nebula_addr()
    }

    fn collect_vms(&self, interval_secs: f64) -> Vec<VmEntry> {
        let mut prev = self
            .prev_cpu_ns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        collect_vm_entries(&self.vm_storage, interval_secs, &mut prev)
    }

    fn tick_once(&self) {
        let peer = self.resolve_nebula_addr();
        let meshfs_available = is_meshfs_mounted(&self.meshfs_mount);
        let vms = self.collect_vms(self.tick.as_secs_f64());
        let containers = collect_container_entries();
        let inventory = build_inventory(&peer, &self.hostname, vms, containers, meshfs_available);
        // BUS-RUN-FULL-1: publish on-change + a slow heartbeat instead of
        // every tick. The sole bus consumer (this node's own
        // `read_local_inventory`) only wants the latest doc, so an
        // identical body each 10 s just grew the append-only Persist log.
        // Serialize once and compare against the last published body; the
        // heartbeat still republishes an unchanged doc periodically so a
        // pruned topic / late subscriber finds a recent one. Publish even
        // when peer is empty (Nebula not yet up) so the topic-shape is
        // consistent — subscribers can ignore peer=="".
        if let Ok(body) = serde_json::to_string(&inventory) {
            let mut last = self
                .last_publish
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = std::time::Instant::now();
            let prev_body = last.as_ref().map(|(b, _)| b.as_str());
            let prev_at = last.as_ref().map(|(_, at)| *at);
            if should_publish(prev_body, &body, prev_at, now, self.publish_heartbeat) {
                publish_inventory(&peer, &inventory);
                *last = Some((body, now));
            }
        }
        // WORKLOAD-FLEET-1: also mirror to the replicated QNM-Shared plane so
        // peers' Workbenches can show this node's workloads (the bus is
        // per-node). Only when the mount is real — never write to a bare local
        // dir masquerading as the share.
        if meshfs_available {
            write_shared_inventory(&self.meshfs_mount, &self.hostname, &inventory);
        }

        // VIRT-21: detect VM state transitions against the prior tick and
        // publish `compute/event/<peer>` for each notable change, then
        // replace the snapshot with current states. VMs that vanished
        // (undefined since last tick) drop out silently — no event.
        {
            let mut prev = self
                .prev_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for vm in &inventory.vms {
                if let Some(ev) =
                    classify_transition(prev.get(&vm.id).map(String::as_str), &vm.state)
                {
                    publish_event(
                        &peer,
                        &ComputeEvent {
                            vm_id: vm.id.clone(),
                            vm_name: vm.name.clone(),
                            event: ev.to_string(),
                            peer: peer.clone(),
                            hostname: self.hostname.clone(),
                        },
                    );
                }
            }
            *prev = inventory
                .vms
                .iter()
                .map(|v| (v.id.clone(), v.state.clone()))
                .collect();
        }
    }
}

#[async_trait::async_trait]
impl Worker for ComputeRegistryWorker {
    fn name(&self) -> &'static str {
        "compute_registry"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => {
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

    // --- parse_virsh_uuid_list ---

    #[test]
    fn parse_uuid_list_drops_blank_lines() {
        let raw = "\nabc-123\n\ndef-456\n\n";
        let v = parse_virsh_uuid_list(raw);
        assert_eq!(v, vec!["abc-123".to_string(), "def-456".to_string()]);
    }

    // --- VIRT-21: classify_transition + ComputeEvent schema ---

    #[test]
    fn classify_transition_detects_crashed() {
        assert_eq!(
            classify_transition(Some("running"), "crashed"),
            Some("crashed")
        );
        // Already crashed ⇒ no repeat event.
        assert_eq!(classify_transition(Some("crashed"), "crashed"), None);
    }

    #[test]
    fn classify_transition_started_and_stopped() {
        assert_eq!(
            classify_transition(Some("shut off"), "running"),
            Some("started")
        );
        assert_eq!(
            classify_transition(Some("running"), "shut off"),
            Some("stopped")
        );
        // paused → running counts as started; running → paused isn't notable.
        assert_eq!(
            classify_transition(Some("paused"), "running"),
            Some("started")
        );
        assert_eq!(classify_transition(Some("running"), "paused"), None);
    }

    #[test]
    fn classify_transition_first_sight_and_noop_are_silent() {
        // No prior sample (worker just started) ⇒ never toast.
        assert_eq!(classify_transition(None, "running"), None);
        assert_eq!(classify_transition(None, "crashed"), None);
        // No change ⇒ no event.
        assert_eq!(classify_transition(Some("running"), "running"), None);
    }

    #[test]
    fn compute_event_schema_round_trips() {
        let ev = ComputeEvent {
            vm_id: "uuid-1".into(),
            vm_name: "web1".into(),
            event: "started".into(),
            peer: "10.42.0.5".into(),
            hostname: "host-b".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        // Design-doc §3 required fields present.
        assert!(json.contains("\"vm_id\":\"uuid-1\""));
        assert!(json.contains("\"vm_name\":\"web1\""));
        assert!(json.contains("\"event\":\"started\""));
        assert!(json.contains("\"peer\":\"10.42.0.5\""));
        let back: ComputeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn event_topic_is_per_peer() {
        assert_eq!(event_topic("10.42.0.5"), "compute/event/10.42.0.5");
    }

    // --- BUS-RUN-FULL-1: should_publish (on-change + slow heartbeat) ---

    #[test]
    fn should_publish_always_on_first_tick() {
        // No prior publish ⇒ publish regardless of body.
        let now = std::time::Instant::now();
        assert!(should_publish(
            None,
            "{}",
            None,
            now,
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn should_publish_skips_unchanged_within_heartbeat() {
        // Same body, well inside the heartbeat ⇒ skip the redundant publish.
        let at = std::time::Instant::now();
        let now = at + Duration::from_secs(10);
        assert!(!should_publish(
            Some("{\"a\":1}"),
            "{\"a\":1}",
            Some(at),
            now,
            Duration::from_secs(60),
        ));
    }

    #[test]
    fn should_publish_on_changed_body() {
        // Body changed inside the heartbeat ⇒ publish immediately.
        let at = std::time::Instant::now();
        let now = at + Duration::from_secs(10);
        assert!(should_publish(
            Some("{\"a\":1}"),
            "{\"a\":2}",
            Some(at),
            now,
            Duration::from_secs(60),
        ));
    }

    #[test]
    fn should_publish_heartbeat_republishes_unchanged() {
        // Unchanged body but the heartbeat elapsed ⇒ republish so a pruned
        // topic / late subscriber still finds a recent doc.
        let at = std::time::Instant::now();
        let now = at + Duration::from_secs(60);
        assert!(should_publish(
            Some("{\"a\":1}"),
            "{\"a\":1}",
            Some(at),
            now,
            Duration::from_secs(60),
        ));
    }

    // --- parse_virsh_dominfo ---

    fn dominfo_running() -> &'static str {
        "Id:             1\nName:           dev-server\nUUID:           abc-123\nOS Type:        hvm\nState:          running\nCPU(s):         2\nCPU time:       1234.5s\nMax memory:     2097152 KiB\nUsed memory:    2097152 KiB\nPersistent:     yes\n"
    }

    fn dominfo_shut_off() -> &'static str {
        "Id:             -\nName:           dev-server\nUUID:           abc-123\nOS Type:        hvm\nState:          shut off\nCPU(s):         2\nMax memory:     2097152 KiB\nUsed memory:    2097152 KiB\n"
    }

    #[test]
    fn parse_dominfo_running_vm() {
        let info = parse_virsh_dominfo(dominfo_running()).expect("running parse");
        assert_eq!(info.name, "dev-server");
        assert_eq!(info.state, "running");
        assert_eq!(info.ram_mb, 2048); // 2_097_152 KiB / 1024
        assert_eq!(info.vcpus, 2);
    }

    #[test]
    fn parse_dominfo_shut_off_vm() {
        let info = parse_virsh_dominfo(dominfo_shut_off()).expect("shut off parse");
        assert_eq!(info.state, "shut off");
    }

    #[test]
    fn parse_dominfo_returns_none_when_name_missing() {
        let raw = "State: running\n";
        assert!(parse_virsh_dominfo(raw).is_none());
    }

    // --- parse_virsh_domblklist ---

    #[test]
    fn parse_domblklist_skips_cdrom() {
        let raw = " Type   Device   Target   Source\n--------\n file   cdrom    sda      /isos/fedora.iso\n file   disk     vda      /var/lib/mde-vms/abc.qcow2\n";
        let path = parse_virsh_domblklist(raw).expect("disk path");
        assert_eq!(path, "/var/lib/mde-vms/abc.qcow2");
    }

    #[test]
    fn parse_domblklist_none_when_no_disk() {
        let raw = " Type   Device   Target   Source\n file   cdrom    sda      /isos/fedora.iso\n";
        assert!(parse_virsh_domblklist(raw).is_none());
    }

    // --- parse_virsh_domstats_vcpu_time ---

    #[test]
    fn parse_domstats_sums_vcpu_time() {
        let raw = "Domain: 'dev'\n  vcpu.0.time=1000\n  vcpu.1.time=2500\n  net.0.rx.bytes=42\n";
        let total = parse_virsh_domstats_vcpu_time(raw).expect("total");
        assert_eq!(total, 3500);
    }

    #[test]
    fn parse_domstats_none_when_no_vcpu_rows() {
        let raw = "Domain: 'dev'\n  net.0.rx.bytes=42\n";
        assert!(parse_virsh_domstats_vcpu_time(raw).is_none());
    }

    // --- cpu_pct_delta ---

    #[test]
    fn cpu_pct_zero_when_prev_absent() {
        assert_eq!(cpu_pct_delta(None, 10_000_000_000, 10.0, 2), 0.0);
    }

    #[test]
    fn cpu_pct_basic_delta() {
        // 1s of vcpu time over 10s interval, 2 vcpus → 10.0%
        let pct = cpu_pct_delta(Some(0), 1_000_000_000, 10.0, 2);
        assert!((pct - 10.0).abs() < 0.01, "got {pct}");
    }

    #[test]
    fn cpu_pct_caps_at_vcpu_ceiling() {
        // Sample claims 100s of vcpu time in 10s interval — physically
        // impossible on a 2-vcpu VM. Should cap at 2 * 100 = 200.
        let pct = cpu_pct_delta(Some(0), 100_000_000_000, 10.0, 2);
        assert_eq!(pct, 200.0);
    }

    #[test]
    fn cpu_pct_zero_when_counter_resets() {
        // VM restarted / migrated → cur < prev → drop sample.
        assert_eq!(cpu_pct_delta(Some(1000), 500, 10.0, 1), 0.0);
    }

    // --- parse_podman_ps_json / stats_json ---

    fn podman_ps_one() -> &'static str {
        r#"[{"Id":"abc123","Names":["mediasoup"],"Image":"ghcr.io/mde/mediasoup:latest","State":"running"}]"#
    }

    fn podman_stats_one() -> &'static str {
        r#"[{"ContainerID":"abc123","Name":"mediasoup","CPU":"3.10%","MemUsage":"512MiB / 16GiB"}]"#
    }

    #[test]
    fn parse_podman_ps_empty_array() {
        assert_eq!(parse_podman_ps_json("[]"), Vec::<ContainerEntry>::new());
    }

    #[test]
    fn parse_podman_ps_one_container() {
        let v = parse_podman_ps_json(podman_ps_one());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "abc123");
        assert_eq!(v[0].name, "mediasoup");
        assert_eq!(v[0].image, "ghcr.io/mde/mediasoup:latest");
        assert_eq!(v[0].state, "running");
        assert_eq!(v[0].pod, ""); // no PodName ⇒ standalone
    }

    #[test]
    fn parse_podman_ps_reads_pod_name() {
        let json = r#"[{"Id":"c1","Names":["db"],"Image":"postgres","State":"running","PodName":"app-pod"}]"#;
        let v = parse_podman_ps_json(json);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].pod, "app-pod");
    }

    #[test]
    fn parse_podman_stats_extracts_cpu_and_mem() {
        let stats = parse_podman_stats_json(podman_stats_one());
        let (cpu, mem) = stats.get("abc123").expect("stats row");
        assert!((cpu - 3.10).abs() < 0.001);
        assert_eq!(*mem, 512);
    }

    #[test]
    fn parse_podman_mem_usage_units() {
        assert_eq!(parse_podman_mem_usage("512MiB / 16GiB"), 512);
        assert_eq!(parse_podman_mem_usage("1GiB / 16GiB"), 1024);
        assert_eq!(parse_podman_mem_usage("2048KiB / 16GiB"), 2);
        assert_eq!(parse_podman_mem_usage("400 / 16GiB"), 400);
    }

    // --- read_vm_nebula_ip ---

    #[test]
    fn read_nebula_ip_missing_file_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_vm_nebula_ip(tmp.path(), "no-such"), "");
    }

    #[test]
    fn read_nebula_ip_trims_whitespace() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("abc.nebula-ip");
        std::fs::write(&path, "  10.42.128.7\n").unwrap();
        assert_eq!(read_vm_nebula_ip(tmp.path(), "abc"), "10.42.128.7");
    }

    // --- write_shared_inventory (WORKLOAD-FLEET-1 cross-node plane) ---

    #[test]
    fn write_shared_inventory_lands_under_hostname_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let inv = build_inventory(
            "10.42.0.3",
            "fedora",
            vec![vm("MDE-KVM-1", "running")],
            vec![],
            true,
        );
        write_shared_inventory(tmp.path(), "fedora", &inv);
        // Lands at <mount>/<hostname>/compute-inventory.json and parses back.
        let path = tmp.path().join("fedora").join(SHARED_INVENTORY_FILE);
        let body = std::fs::read_to_string(&path).expect("inventory file written");
        let back: Inventory = serde_json::from_str(&body).unwrap();
        assert_eq!(back.hostname, "fedora");
        assert_eq!(back.vms.len(), 1);
        assert_eq!(back.vms[0].name, "MDE-KVM-1");
        // No stray temp file left behind after the atomic rename.
        assert!(!tmp
            .path()
            .join("fedora")
            .join("compute-inventory.json.tmp")
            .exists());
    }

    #[test]
    fn write_shared_inventory_skips_empty_hostname() {
        let tmp = tempfile::tempdir().unwrap();
        let inv = build_inventory("", "", vec![], vec![], false);
        write_shared_inventory(tmp.path(), "", &inv);
        // Nothing created for an empty hostname.
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    // --- build_inventory + the 6 required scenarios ---

    fn vm(name: &str, state: &str) -> VmEntry {
        VmEntry {
            id: format!("{name}-uuid"),
            name: name.to_string(),
            state: state.to_string(),
            cpu_pct: 0.0,
            ram_mb: 2048,
            disk_path: format!("/var/lib/mde-vms/{name}.qcow2"),
            nebula_ip: String::new(),
            meshfs_available: false,
        }
    }

    fn container(name: &str) -> ContainerEntry {
        ContainerEntry {
            id: format!("{name}-id"),
            name: name.to_string(),
            state: "running".into(),
            image: "ghcr.io/example/img:latest".into(),
            cpu_pct: 0.0,
            ram_mb: 256,
            pod: String::new(),
        }
    }

    #[test]
    fn scenario_empty_inventory() {
        let inv = build_inventory("10.42.0.1", "alice", vec![], vec![], true);
        assert!(inv.vms.is_empty());
        assert!(inv.containers.is_empty());
        assert_eq!(inv.peer, "10.42.0.1");
        assert_eq!(inv.hostname, "alice");
    }

    #[test]
    fn scenario_one_vm_running() {
        let inv = build_inventory(
            "10.42.0.1",
            "alice",
            vec![vm("dev", "running")],
            vec![],
            true,
        );
        assert_eq!(inv.vms.len(), 1);
        assert_eq!(inv.vms[0].state, "running");
        assert!(inv.vms[0].meshfs_available);
    }

    #[test]
    fn scenario_one_vm_stopped() {
        let inv = build_inventory(
            "10.42.0.1",
            "alice",
            vec![vm("dev", "shut off")],
            vec![],
            true,
        );
        assert_eq!(inv.vms.len(), 1);
        assert_eq!(inv.vms[0].state, "shut off");
    }

    #[test]
    fn scenario_one_container() {
        let inv = build_inventory(
            "10.42.0.1",
            "alice",
            vec![],
            vec![container("mediasoup")],
            true,
        );
        assert!(inv.vms.is_empty());
        assert_eq!(inv.containers.len(), 1);
        assert_eq!(inv.containers[0].name, "mediasoup");
    }

    #[test]
    fn scenario_mixed_vm_and_container() {
        let inv = build_inventory(
            "10.42.0.1",
            "alice",
            vec![vm("dev", "running")],
            vec![container("mediasoup")],
            true,
        );
        assert_eq!(inv.vms.len(), 1);
        assert_eq!(inv.containers.len(), 1);
    }

    #[test]
    fn scenario_meshfs_unavailable_marks_every_vm() {
        let inv = build_inventory(
            "10.42.0.1",
            "alice",
            vec![vm("dev1", "running"), vm("dev2", "shut off")],
            vec![],
            false,
        );
        assert_eq!(inv.vms.len(), 2);
        assert!(inv.vms.iter().all(|v| !v.meshfs_available));
    }

    // --- inventory JSON shape matches design doc §3 ---

    #[test]
    fn inventory_json_serializes_with_design_doc_fields() {
        let inv = build_inventory(
            "10.42.0.1",
            "alice",
            vec![vm("dev", "running")],
            vec![container("mediasoup")],
            true,
        );
        let s = serde_json::to_string(&inv).unwrap();
        for field in [
            "\"peer\"",
            "\"hostname\"",
            "\"vms\"",
            "\"containers\"",
            "\"id\"",
            "\"name\"",
            "\"state\"",
            "\"cpu_pct\"",
            "\"ram_mb\"",
            "\"disk_path\"",
            "\"nebula_ip\"",
            "\"meshfs_available\"",
            "\"image\"",
        ] {
            assert!(s.contains(field), "missing field {field} in {s}");
        }
    }
}
