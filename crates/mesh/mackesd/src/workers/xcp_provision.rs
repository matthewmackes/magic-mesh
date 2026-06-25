//! XCP-3 — the `xcp_provision` worker: the A-plane provision flow.
//!
//! Spawns an `MDE-VM` on an XCP-ng dom0 over the [`mackes_xcp`]
//! hypervisor-access layer (design: `docs/design/xcp-ng-integration.md`, A-plane).
//!
//! This is the runtime caller for the XCP-1 `Hypervisor` primitives —
//! `clone_golden → set_identity_seed → start → vm_ip` — so a provisioned VM
//! actually gets a fresh identity seed (the A2 "fresh identity per clone" rule:
//! `MDE-VM-<name>` hostname, the operator's key, regenerated SSH host keys +
//! `machine-id`). Before this wiring `set_identity_seed` was dead code; the
//! whole point of the unit is that it is now reachable from `mackesd serve`.
//!
//! ## Flow (design A-plane steps 1–3)
//!
//! - Resolve the target dom0 (request `host`, else the first `MCNF_XEN_DOM0S`) +
//!   the mesh SSH key → a [`mackes_xcp::HostTarget::Ssh`]; the dom0 must be in the
//!   allow-list (`MCNF_XEN_DOM0S`) — the guard the datacenter IPC uses.
//! - `xe vm-clone MDE-VM-golden → MDE-VM-<name>`, then **attach the fresh
//!   cloud-init seed** ([`mackes_xcp::build_identity_seed`] →
//!   [`mackes_xcp::Hypervisor::set_identity_seed`]) — the load-bearing step.
//! - Start the VM, then poll [`mackes_xcp::Hypervisor::vm_ip`] for the
//!   guest-agent IPv4 (best-effort within a short window).
//!
//! Steps 4–5 of the design (`dnf upgrade` / `role-pin` / `network-enroll join`
//! over SSH, directory rollup) ride on the booted VM's own first-boot units +
//! the existing enroll path and are out of this worker's scope; the seed this
//! worker attaches is what carries the op key + hostname that make those land.
//!
//! ## Async / Persist
//! `Persist` is `!Sync`, so it's never held across an `.await`. The spawn-topic
//! read is a short sync open-read-drop each tick; each spawn runs on
//! `spawn_blocking` (it shells out via `xe`/`ssh` and polls for the IP) with its
//! own `Persist` handle, keeping the run-loop responsive — mirrors
//! [`super::compute_provision`].
//!
//! The pure request/ack codec + the generic [`run_spawn`] flow (driven by a mock
//! [`mackes_xcp::Hypervisor`] in tests) are unit-tested here; the live
//! clone+seed+start+ip against a real dom0 is host-gated (XCP-3 acceptance).

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mackes_xcp::{
    build_identity_seed, mde_vm_hostname, HostCapacity, HostTarget, Hypervisor, VmInfo, XeSsh,
    MDE_VM_DEFAULT_MEM_BYTES,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use crate::ipc::secret_store::{self, SecretStore};

use super::{ShutdownToken, Worker};

/// Bus topic this worker drains for spawn requests (the A4 surface).
pub const SPAWN_TOPIC: &str = "action/provision/spawn";

/// Reply-topic prefix for spawn acks (suffix = request ULID).
pub const SPAWN_ACK_PREFIX: &str = "action/provision/spawn-ack/";

/// Enumerate the VMs (with power-state) across every configured dom0 (XCP-4).
///
/// The Workbench Provisioning panel queries this. Read-only; the reply lands on
/// the generic `reply/<request-ulid>` RPC lane.
pub const LIST_TOPIC: &str = "action/provision/list";

/// Bus action topic the panel uses to destroy a named VM (XCP-4). The request
/// body is a [`DestroyRequest`]; the reply lands on `reply/<request-ulid>`.
pub const DESTROY_TOPIC: &str = "action/provision/destroy";

/// (Re)start an already-cloned, halted VM by name (XCP-4).
///
/// Distinct from `spawn`, which clones a *new* VM from the golden; `start`
/// issues `xe vm-start` on the existing VM the request names. The body is a
/// [`StartRequest`]; the reply lands on `reply/<request-ulid>`.
pub const START_TOPIC: &str = "action/provision/start";

/// Bus action topic the panel queries for the dom0 host roster + per-host
/// capacity that feeds the spawn target picker (XCP-4). Read-only; the reply
/// lands on `reply/<request-ulid>`.
pub const HOSTS_TOPIC: &str = "action/provision/hosts";

/// XCP-7 — bus action topic the Provisioning panel fires to **set/rotate** a
/// dom0's XAPI/root credential. The body is a [`SetCredsRequest`]; the daemon
/// age-encrypts it into the mesh secret store under `xcp/<host>`. The reply lands
/// on `reply/<request-ulid>`. Leader-managed: any authorized node can drive it
/// and every enrolled node then reads the same replicated secret.
pub const SET_CREDS_TOPIC: &str = "action/provision/set-creds";

/// The golden template every spawn clones (design A2).
pub const GOLDEN_TEMPLATE: &str = "MDE-VM-golden";

/// Poll cadence for the spawn topic.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// How long to poll for the guest-agent IPv4 after start before giving up (the
/// ack still reports success without an IP — the guest agent may report later).
pub const IP_WAIT_TIMEOUT: Duration = Duration::from_secs(90);

/// Cadence between `vm_ip` polls.
pub const IP_WAIT_POLL: Duration = Duration::from_secs(3);

/// A spawn request on [`SPAWN_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpawnRequest {
    /// Correlation ULID; the ack lands on `action/provision/spawn-ack/<ulid>`.
    pub request_ulid: String,
    /// Short VM name; the guest hostname becomes `MDE-VM-<name>`.
    pub name: String,
    /// Target dom0 address; `None` ⇒ the first configured `MCNF_XEN_DOM0S`.
    #[serde(default)]
    pub host: Option<String>,
}

/// The spawn ack — `{uuid, hostname, ip?}` on success, `{error}` on failure.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpawnAck {
    /// New VM uuid on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// The applied `MDE-VM-<name>` guest hostname on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// The guest-agent-reported IPv4, if it surfaced within the wait window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Error description on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parse a spawn-request body.
///
/// # Errors
/// A human-readable string on malformed JSON / missing required fields.
pub fn parse_spawn_request(body: &str) -> Result<SpawnRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed spawn request: {e}"))
}

/// Build a success spawn-ack JSON body.
#[must_use]
pub fn build_spawn_ack_ok(uuid: &str, hostname: &str, ip: Option<&str>) -> String {
    let ack = SpawnAck {
        uuid: Some(uuid.to_string()),
        hostname: Some(hostname.to_string()),
        ip: ip.map(str::to_string),
        error: None,
    };
    serde_json::to_string(&ack).unwrap_or_else(|_| r#"{"error":"ack encode failed"}"#.into())
}

/// Build an error spawn-ack JSON body.
#[must_use]
pub fn build_spawn_ack_error(message: &str) -> String {
    let ack = SpawnAck {
        uuid: None,
        hostname: None,
        ip: None,
        error: Some(message.to_string()),
    };
    serde_json::to_string(&ack).unwrap_or_else(|_| r#"{"error":"ack encode failed"}"#.into())
}

// ───────────────────────── XCP-4 list/destroy/hosts ─────────────────────────
// The Workbench Provisioning panel's three read/destroy verbs. Unlike the spawn
// flow (which acks on its own `action/provision/spawn-ack/<ulid>` topic keyed by
// a body `request_ulid`), these reply on the generic `reply/<request-ulid>` RPC
// lane so the panel's `mde_bus::rpc::request` round-trip (via
// `crate::dbus::action_request*`) resolves them with no extra correlation.

/// One VM in a [`LIST_TOPIC`] reply — a [`VmInfo`] tagged with the dom0 it lives
/// on (the panel needs the host to target a destroy at the right dom0).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ListedVm {
    /// XAPI VM uuid.
    pub uuid: String,
    /// `name-label`.
    pub name: String,
    /// `power-state` (`running` / `halted` / …).
    pub power_state: String,
    /// dom0 this VM is hosted on (one of the `MCNF_XEN_DOM0S`).
    pub host: String,
}

/// One dom0's capacity row in a [`HOSTS_TOPIC`] reply.
///
/// Carries the host address plus either its [`HostCapacity`] or the probe error
/// that host returned. The panel renders reachable hosts as pickable spawn
/// targets and surfaces the error for any that failed to probe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostRow {
    /// dom0 address (one of the `MCNF_XEN_DOM0S`).
    pub host: String,
    /// Physical CPU count.
    #[serde(default)]
    pub cpu_count: u32,
    /// Total host memory (KiB).
    #[serde(default)]
    pub mem_total_kib: u64,
    /// Free host memory (KiB).
    #[serde(default)]
    pub mem_free_kib: u64,
    /// Largest free SR space (bytes) — the spawn ceiling.
    #[serde(default)]
    pub sr_free_bytes: u64,
    /// Running VMs on the host.
    #[serde(default)]
    pub running_vms: u32,
    /// Probe error for this host, if it was unreachable / `xe` failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl HostRow {
    /// A reachable host row built from a successful capacity probe.
    #[must_use]
    pub fn ok(host: &str, cap: &HostCapacity) -> Self {
        Self {
            host: host.to_string(),
            cpu_count: cap.cpu_count,
            mem_total_kib: cap.mem_total_kib,
            mem_free_kib: cap.mem_free_kib,
            sr_free_bytes: cap.sr_free_bytes,
            running_vms: cap.running_vms,
            error: None,
        }
    }

    /// An error row for a host whose capacity probe failed.
    #[must_use]
    pub fn failed(host: &str, message: &str) -> Self {
        Self {
            host: host.to_string(),
            error: Some(message.to_string()),
            ..Self::ok(host, &HostCapacity::default())
        }
    }
}

/// A [`DESTROY_TOPIC`] request — destroy the VM named `name` (the operator picks
/// it from the list). `host` pins the dom0 (the list reply carries it); `None`
/// falls back to the first configured dom0.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NamedVmRequest {
    /// `name-label` of the target VM.
    pub name: String,
    /// dom0 the VM lives on; `None` ⇒ the first configured `MCNF_XEN_DOM0S`.
    #[serde(default)]
    pub host: Option<String>,
}

/// A [`DESTROY_TOPIC`] / [`START_TOPIC`] request — both name a VM on a dom0.
pub type DestroyRequest = NamedVmRequest;
/// A [`START_TOPIC`] request (alias of [`NamedVmRequest`]).
pub type StartRequest = NamedVmRequest;

/// Parse a name-on-a-dom0 request body (the `destroy` / `start` verbs share the
/// shape). `verb` names the request in the error message.
///
/// # Errors
/// A human-readable string on malformed JSON / an empty `name` (we'd have no VM
/// to act on).
pub fn parse_named_vm_request(body: &str, verb: &str) -> Result<NamedVmRequest, String> {
    let req: NamedVmRequest =
        serde_json::from_str(body).map_err(|e| format!("malformed {verb} request: {e}"))?;
    if req.name.trim().is_empty() {
        return Err(format!("{verb} request: name is empty"));
    }
    Ok(req)
}

/// Build a `{"vms":[…]}` list-reply body (or `{"error":…}` on encode failure).
#[must_use]
pub fn build_list_reply(vms: &[ListedVm]) -> String {
    serde_json::to_string(&serde_json::json!({ "vms": vms }))
        .unwrap_or_else(|_| r#"{"error":"list encode failed"}"#.into())
}

/// Build a `{"hosts":[…]}` hosts-reply body (or `{"error":…}` on encode failure).
#[must_use]
pub fn build_hosts_reply(hosts: &[HostRow]) -> String {
    serde_json::to_string(&serde_json::json!({ "hosts": hosts }))
        .unwrap_or_else(|_| r#"{"error":"hosts encode failed"}"#.into())
}

/// Build a `{"destroyed":<name>}` reply for a completed destroy.
#[must_use]
pub fn build_destroy_reply_ok(name: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "destroyed": name }))
        .unwrap_or_else(|_| r#"{"error":"destroy encode failed"}"#.into())
}

/// Build a `{"started":<name>}` reply for a completed start.
#[must_use]
pub fn build_start_reply_ok(name: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "started": name }))
        .unwrap_or_else(|_| r#"{"error":"start encode failed"}"#.into())
}

/// Build a `{"error":<message>}` reply body — the shape the panel's
/// `reply_error` decoder looks for across all three verbs.
#[must_use]
pub fn build_error_reply(message: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "error": message }))
        .unwrap_or_else(|_| r#"{"error":"encode failed"}"#.into())
}

// ───────────────────────── XCP-7 set/rotate dom0 credential ─────────────────────────

/// A [`SET_CREDS_TOPIC`] request — store/rotate the `password` for dom0 `host`'s
/// XAPI/root login in the mesh secret store under `xcp/<host>`.
///
/// `password` is the secret; it is age-encrypted at rest and NEVER logged or put
/// in `ps`. The request body is the only place it appears in transit (the bus is
/// the local replicated store, not a process argv).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SetCredsRequest {
    /// dom0 address the credential authenticates (must be allow-listed).
    pub host: String,
    /// The XAPI/root password to seal into the store.
    pub password: String,
}

// Hand-rolled Debug so a `{:?}` of the request can't leak the password into a log
// line (XCP-7: the credential must never be logged).
impl std::fmt::Debug for SetCredsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetCredsRequest")
            .field("host", &self.host)
            .field("password", &"***")
            .finish()
    }
}

/// Parse a set-creds request body.
///
/// # Errors
/// Malformed JSON, an empty `host` (nothing to key on), or an empty `password`
/// (we don't store a blank credential — that would mask the honest "no credential
/// stored" state).
pub fn parse_set_creds_request(body: &str) -> Result<SetCredsRequest, String> {
    let req: SetCredsRequest =
        serde_json::from_str(body).map_err(|e| format!("malformed set-creds request: {e}"))?;
    if req.host.trim().is_empty() {
        return Err("set-creds request: host is empty".to_string());
    }
    if req.password.is_empty() {
        return Err("set-creds request: password is empty".to_string());
    }
    Ok(req)
}

/// Build a `{"stored":<host>}` reply for a completed credential write (the host,
/// never the secret).
#[must_use]
pub fn build_set_creds_reply_ok(host: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "stored": host }))
        .unwrap_or_else(|_| r#"{"error":"set-creds encode failed"}"#.into())
}

/// Resolve the uuid of the VM named `name` from a roster (the destroy path:
/// `destroy` takes a uuid, but the operator names a VM).
///
/// # Errors
/// `Err` when no VM in `vms` carries that `name-label`.
pub fn resolve_uuid_by_name(vms: &[VmInfo], name: &str) -> Result<String, String> {
    vms.iter()
        .find(|v| v.name == name)
        .map(|v| v.uuid.clone())
        .ok_or_else(|| format!("no VM named {name:?} on the target dom0"))
}

/// Resolve the dom0 the request targets.
///
/// Enforces the `MCNF_XEN_DOM0S` allow-list (same guard the datacenter IPC
/// applies before any SSH). Returns the chosen dom0 address, or an error
/// describing the rejection.
///
/// # Errors
/// `Err` when no dom0 is configured, or when the requested `host` isn't in the
/// allow-list.
pub fn resolve_dom0(requested: Option<&str>, allowed: &[String]) -> Result<String, String> {
    match requested {
        Some(h) if allowed.iter().any(|a| a == h) => Ok(h.to_string()),
        Some(h) => Err(format!(
            "dom0 {h:?} is not in the MCNF_XEN_DOM0S allow-list"
        )),
        None => allowed
            .first()
            .cloned()
            .ok_or_else(|| "no dom0 configured (MCNF_XEN_DOM0S empty)".to_string()),
    }
}

/// Free memory the host must have before a spawn fans out, in KiB (XPA-1).
///
/// The right-sized headless footprint ([`MDE_VM_DEFAULT_MEM_BYTES`], 2 GiB). The
/// pre-check refuses early when a host has less than this free, so a fan-out
/// can't overcommit the host (the 4 GiB golden clones did).
pub const SPAWN_MIN_FREE_MEM_KIB: u64 = MDE_VM_DEFAULT_MEM_BYTES / 1024;

/// XPA-1 capacity pre-check ahead of a spawn fan-out.
///
/// The host must have at least [`SPAWN_MIN_FREE_MEM_KIB`] free. `Ok(())` to
/// proceed; `Err` with a CLEAR, actionable message (the host, its free RAM, and
/// the requirement — not a vague "no suitable hosts") when the host can't fit
/// the right-sized VM. Pure so the boundary is testable without a live dom0.
///
/// # Errors
/// `Err` when `cap.mem_free_kib < SPAWN_MIN_FREE_MEM_KIB`.
pub fn check_spawn_capacity(host: &str, cap: &HostCapacity) -> Result<(), String> {
    if cap.mem_free_kib >= SPAWN_MIN_FREE_MEM_KIB {
        return Ok(());
    }
    Err(format!(
        "dom0 {host} has only {free} MiB free; a spawn needs {need} MiB \
         (the 2 GiB headless default) — free memory on the host (halt/destroy a VM) \
         or target another dom0 before spawning",
        free = cap.mem_free_kib / 1024,
        need = SPAWN_MIN_FREE_MEM_KIB / 1024,
    ))
}

/// Drive the full A-plane spawn over a [`Hypervisor`].
///
/// Pre-checks the host's free memory ([`check_spawn_capacity`], XPA-1) so the
/// fan-out can't overcommit it, then clones the golden, attaches the fresh
/// identity seed, **right-sizes the clone's RAM to the 2 GiB headless default**
/// ([`MDE_VM_DEFAULT_MEM_BYTES`], XPA-1 — so it doesn't inherit the oversized
/// golden footprint), starts, and polls for the IP. Generic over the hypervisor
/// so the reachability of `set_identity_seed`/`set_memory` is provable with a mock
/// (no live dom0). `op_ssh_key` is the operator's authorized public key the seed
/// installs. `wait`/`poll` bound the IP wait (0 ⇒ skip the wait).
///
/// Returns the success-ack body; `Err(description)` becomes an error-ack.
///
/// # Errors
/// The capacity pre-check failing (host can't fit the VM) aborts BEFORE any
/// clone; any `xe`/`ssh` step failing (clone / seed / set-memory / start)
/// aborts the spawn.
pub fn run_spawn<H: Hypervisor>(
    hv: &H,
    host: &str,
    name: &str,
    op_ssh_key: &str,
    wait: Duration,
    poll: Duration,
) -> Result<String, String> {
    let hostname = mde_vm_hostname(name);

    // 1. XPA-1 pre-check: refuse BEFORE fanning out a clone if the host can't fit
    //    the right-sized VM. A probe error is itself a clear, actionable failure
    //    (we won't clone onto a host whose capacity we can't read).
    let cap = hv
        .host_capacity()
        .map_err(|e| format!("probe dom0 {host} capacity before spawn: {e}"))?;
    check_spawn_capacity(host, &cap)?;

    // 2a. Clone the golden template into the new MDE-VM.
    let uuid = hv
        .clone_golden(GOLDEN_TEMPLATE, &hostname)
        .map_err(|e| format!("clone {GOLDEN_TEMPLATE} → {hostname}: {e}"))?;

    // 2b. THE load-bearing step — attach the fresh cloud-init identity seed so
    //     the clone boots with a new hostname/host-keys/machine-id + the op key
    //     (A2). The instance-id is the new uuid, which makes cloud-init treat the
    //     clone as a first boot.
    let seed = build_identity_seed(name, op_ssh_key, &uuid);
    hv.set_identity_seed(&uuid, &seed)
        .map_err(|e| format!("attach identity seed to {uuid}: {e}"))?;

    // 2c. XPA-1: right-size the clone's RAM to the 2 GiB headless default (the
    //     clone otherwise inherits the golden's oversized footprint). Done while
    //     still halted, before start.
    hv.set_memory(&uuid, MDE_VM_DEFAULT_MEM_BYTES)
        .map_err(|e| format!("set memory on {uuid}: {e}"))?;

    // 3a. Start (UEFI inherited from the golden).
    hv.start(&uuid).map_err(|e| format!("start {uuid}: {e}"))?;

    // 3b. Best-effort: poll for the guest-agent IPv4 within the wait window.
    let ip = poll_vm_ip(hv, &uuid, wait, poll);

    Ok(build_spawn_ack_ok(&uuid, &hostname, ip.as_deref()))
}

/// Poll [`Hypervisor::vm_ip`] until an IPv4 surfaces or `wait` elapses. Probe
/// errors are tolerated (the guest agent may not be up yet); `None` on timeout.
fn poll_vm_ip<H: Hypervisor>(hv: &H, uuid: &str, wait: Duration, poll: Duration) -> Option<String> {
    if wait.is_zero() {
        return hv.vm_ip(uuid).ok().flatten();
    }
    let deadline = Instant::now() + wait;
    loop {
        if let Ok(Some(ip)) = hv.vm_ip(uuid) {
            return Some(ip);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(poll);
    }
}

/// Read new spawn requests on [`SPAWN_TOPIC`] since `cursor`. Opens + drops a
/// `Persist` synchronously so it never crosses an `.await`.
fn read_new_spawns(bus_root: &Path, cursor: &mut Option<String>) -> Vec<SpawnRequest> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(SPAWN_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_spawn_request(body) {
            Ok(req) => out.push(req),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "xcp_provision: bad spawn request");
            }
        }
    }
    out
}

/// dom0s this node is allowed to drive — reuses the datacenter env config so the
/// allow-list is single-sourced.
fn allowed_dom0s() -> Vec<String> {
    super::datacenter_orchestrator::xen_dom0s()
}

/// SSH key reaching the dom0s (the mesh key) — reuses the datacenter env config.
fn dom0_ssh_key() -> String {
    super::datacenter_orchestrator::xen_ssh_key()
}

/// The operator's authorized public key the seed installs. Read from
/// `MCNF_OP_SSH_KEY` (a path or the literal key line), else the public half of
/// the mesh key alongside [`dom0_ssh_key`] (`<key>.pub`). Empty when neither is
/// readable (cloud-init then just regenerates host keys, no op key).
fn operator_ssh_key() -> String {
    if let Ok(v) = std::env::var("MCNF_OP_SSH_KEY") {
        let v = v.trim();
        if v.starts_with("ssh-") {
            return v.to_string();
        }
        if let Ok(contents) = std::fs::read_to_string(v) {
            return contents.trim().to_string();
        }
    }
    let pub_path = format!("{}.pub", dom0_ssh_key());
    std::fs::read_to_string(&pub_path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Run one spawn end-to-end on a blocking thread + write the ack. Never panics.
fn run_spawn_blocking(bus_root: PathBuf, req: &SpawnRequest) {
    let ack_topic = format!("{SPAWN_ACK_PREFIX}{}", req.request_ulid);
    let ack_body = match spawn_over_ssh(req) {
        Ok(body) => body,
        Err(e) => {
            tracing::warn!(req = %req.request_ulid, error = %e, "xcp_provision: spawn failed");
            build_spawn_ack_error(&e)
        }
    };
    let persist = match Persist::open(bus_root) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "xcp_provision: persist open failed; cannot ack");
            return;
        }
    };
    if let Err(e) = persist.write(&ack_topic, Priority::Default, None, Some(&ack_body)) {
        tracing::warn!(error = %e, topic = ack_topic, "xcp_provision: ack write failed");
    }
}

/// Build the live [`XeSsh`] target for the request + run the spawn. Split from
/// [`run_spawn`] so the latter stays host-free + unit-testable.
fn spawn_over_ssh(req: &SpawnRequest) -> Result<String, String> {
    let dom0 = resolve_dom0(req.host.as_deref(), &allowed_dom0s())?;
    let hv = XeSsh::new(hv_target_for(&dom0));
    run_spawn(
        &hv,
        &dom0,
        &req.name,
        &operator_ssh_key(),
        IP_WAIT_TIMEOUT,
        IP_WAIT_POLL,
    )
}

/// The xe-over-SSH target reaching a single dom0 (the same target
/// [`spawn_over_ssh`] builds — the destroy/list/hosts handlers reuse it).
///
/// XCP-7: when a per-host XAPI/root credential is stored in the mesh secret store
/// under `xcp/<dom0>`, it is attached to the target so the runner authenticates
/// over `sshpass` (password on stdin, never argv) for hosts the mesh key can't
/// reach. When no credential is stored, the target is key-only — identical to the
/// prior behaviour, so key-reachable dom0s are unaffected.
///
/// Resolves the secret store fresh — for the single-host spawn/destroy/start
/// paths. Loops over many dom0s (`list_all_vms`, `probe_all_hosts`) should
/// instead resolve the store ONCE and call [`hv_target_with_store`] per dom0, so
/// the per-dom0 work is one credential read, not a fresh store resolution each.
fn hv_target_for(dom0: &str) -> HostTarget {
    hv_target_with_store(dom0, &dom0_secret_store())
}

/// As [`hv_target_for`] but with a caller-resolved `store`, so a loop over dom0s
/// resolves the store once and pays only one credential read per host.
fn hv_target_with_store(dom0: &str, store: &SecretStore) -> HostTarget {
    HostTarget::ssh_root_with_password(
        dom0.to_string(),
        Some(dom0_ssh_key()),
        read_dom0_password(store, dom0),
    )
}

/// The mesh secret store for dom0 credentials, anchored on
/// [`secret_store::repo_root`] (so the replicated `age`+etcd store is found under
/// the deployed repo, not the systemd cwd `/`) with the local-AEAD fallback rooted
/// under the workgroup volume.
fn dom0_secret_store() -> SecretStore {
    SecretStore::resolve(
        &secret_store::repo_root(),
        &crate::default_qnm_shared_root(),
    )
}

/// Store-injected core of [`dom0_password`] — read `xcp/<dom0>` from `store`,
/// degrading a fault to `None` (key-only auth) after logging the error (never the
/// secret). Split out so the honest absent / present paths are testable with an
/// injected `LocalAead` store, no env juggling.
fn read_dom0_password(store: &SecretStore, dom0: &str) -> Option<String> {
    let name = secret_store::xcp_creds_ref(dom0);
    match store.get(&name) {
        Ok(Some(pw)) => Some(pw),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(dom0 = %dom0, error = %e, "xcp_provision: dom0 credential read failed; using key-only auth");
            None
        }
    }
}

/// XCP-7 — seal `password` into the mesh secret store under `xcp/<host>` (set or
/// rotate the dom0 credential). The host must be allow-listed (`MCNF_XEN_DOM0S`),
/// the same guard the spawn/list/destroy verbs apply, so a node can't be made to
/// store a credential for an arbitrary host. Returns the host on success.
///
/// # Errors
/// Host not allow-listed, or a secret-store write failure (surfaced honestly —
/// the panel shows the failure rather than claiming the credential was stored).
/// The error string never carries the password.
fn store_dom0_credential(req: &SetCredsRequest) -> Result<String, String> {
    store_dom0_credential_in(&dom0_secret_store(), req, &allowed_dom0s())
}

/// Store-injected core of [`store_dom0_credential`] — allow-list-guard `host`,
/// then seal `password` into `store` under `xcp/<host>`. Split out so the
/// round-trip + the allow-list rejection are testable with an injected store and
/// an explicit allow-list, no env juggling.
fn store_dom0_credential_in(
    store: &SecretStore,
    req: &SetCredsRequest,
    allowed: &[String],
) -> Result<String, String> {
    let host = resolve_dom0(Some(req.host.trim()), allowed)?;
    let name = secret_store::xcp_creds_ref(&host);
    store.put(&name, &req.password)?;
    Ok(host)
}

/// Enumerate every configured dom0's VMs (each tagged with its host) for a
/// [`LIST_TOPIC`] reply. A per-host probe failure is skipped (the panel still
/// shows the VMs on the hosts that answered) rather than failing the whole list.
fn list_all_vms() -> Vec<ListedVm> {
    let mut out = Vec::new();
    // Resolve the secret store once for the whole sweep (one credential read per
    // dom0, not a fresh store resolution each — XCP-7 efficiency).
    let store = dom0_secret_store();
    for dom0 in allowed_dom0s() {
        let hv = XeSsh::new(hv_target_with_store(&dom0, &store));
        match hv.list() {
            Ok(vms) => out.extend(vms.into_iter().map(|v| ListedVm {
                uuid: v.uuid,
                name: v.name,
                power_state: v.power_state,
                host: dom0.clone(),
            })),
            Err(e) => {
                tracing::warn!(dom0 = %dom0, error = %e, "xcp_provision: list over dom0 failed");
            }
        }
    }
    out
}

/// Probe every configured dom0's capacity for a [`HOSTS_TOPIC`] reply, recording
/// an error row for any host that didn't answer (so the panel can grey it out).
fn probe_all_hosts() -> Vec<HostRow> {
    // One store resolution for the whole probe sweep (XCP-7 efficiency).
    let store = dom0_secret_store();
    allowed_dom0s()
        .into_iter()
        .map(|dom0| {
            let hv = XeSsh::new(hv_target_with_store(&dom0, &store));
            match hv.host_capacity() {
                Ok(cap) => HostRow::ok(&dom0, &cap),
                Err(e) => HostRow::failed(&dom0, &e.to_string()),
            }
        })
        .collect()
}

/// Resolve a name-on-a-dom0 request to its live `(XeSsh, uuid)`: resolve the
/// dom0 (allow-list guard), list its VMs, map `name` → uuid. Shared preamble for
/// the `destroy` + `start` handlers (both act on an existing named VM).
///
/// # Errors
/// Dom0 not allow-listed / `xe list` failed / no VM by that name.
fn resolve_named_vm(req: &NamedVmRequest) -> Result<(XeSsh, String), String> {
    let dom0 = resolve_dom0(req.host.as_deref(), &allowed_dom0s())?;
    let hv = XeSsh::new(hv_target_for(&dom0));
    let vms = hv.list().map_err(|e| format!("list {dom0}: {e}"))?;
    let uuid = resolve_uuid_by_name(&vms, req.name.trim())?;
    Ok((hv, uuid))
}

/// Destroy the named VM on the resolved dom0 (force-shutdown + uninstall).
/// Returns the destroyed name on success.
///
/// # Errors
/// Per [`resolve_named_vm`], plus an `xe` uninstall failure.
fn destroy_named_vm(req: &DestroyRequest) -> Result<String, String> {
    let (hv, uuid) = resolve_named_vm(req)?;
    hv.destroy(&uuid)
        .map_err(|e| format!("destroy {} ({uuid}): {e}", req.name))?;
    Ok(req.name.clone())
}

/// Start the named (already-cloned, halted) VM on the resolved dom0 via
/// `xe vm-start` — the real "restart an existing VM" path, distinct from `spawn`
/// (which clones a *new* VM from the golden). Returns the started name.
///
/// # Errors
/// Per [`resolve_named_vm`], plus an `xe vm-start` failure.
fn start_named_vm(req: &StartRequest) -> Result<String, String> {
    let (hv, uuid) = resolve_named_vm(req)?;
    hv.start(&uuid)
        .map_err(|e| format!("start {} ({uuid}): {e}", req.name))?;
    Ok(req.name.clone())
}

/// One request on a responder topic — the request ULID (for the
/// `reply/<ulid>` lane) plus its body. Opens + drops a `Persist` synchronously
/// so it never crosses an `.await` (the file's `!Sync` invariant).
struct PendingRequest {
    ulid: String,
    body: String,
}

/// Read new messages on `topic` since `cursor`, advancing the cursor. Mirrors
/// [`read_new_spawns`] but generic over the topic (the three XCP-4 responders
/// share it) and keeps the raw body so each handler parses its own request.
fn read_new_requests(
    bus_root: &Path,
    topic: &str,
    cursor: &mut Option<String>,
) -> Vec<PendingRequest> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(topic, cursor.as_deref()) else {
        return vec![];
    };
    msgs.into_iter()
        .map(|msg| {
            *cursor = Some(msg.ulid.clone());
            PendingRequest {
                body: msg.body.unwrap_or_default(),
                ulid: msg.ulid,
            }
        })
        .collect()
}

/// Write `body` to the generic `reply/<ulid>` RPC lane the panel waits on.
/// Never panics; a persist failure is logged and dropped (the caller times out).
fn write_reply(bus_root: &Path, req_ulid: &str, body: &str) {
    let topic = reply_topic(req_ulid);
    let persist = match Persist::open(bus_root.to_path_buf()) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "xcp_provision: persist open failed; cannot reply");
            return;
        }
    };
    if let Err(e) = persist.write(&topic, Priority::Default, None, Some(body)) {
        tracing::warn!(error = %e, topic = topic, "xcp_provision: reply write failed");
    }
}

/// Handle one [`LIST_TOPIC`] request on a blocking thread + write the reply.
fn handle_list_blocking(bus_root: &Path, req_ulid: &str) {
    write_reply(bus_root, req_ulid, &build_list_reply(&list_all_vms()));
}

/// Handle one [`HOSTS_TOPIC`] request on a blocking thread + write the reply.
fn handle_hosts_blocking(bus_root: &Path, req_ulid: &str) {
    write_reply(bus_root, req_ulid, &build_hosts_reply(&probe_all_hosts()));
}

/// Handle one [`DESTROY_TOPIC`] request on a blocking thread + write the reply.
fn handle_destroy_blocking(bus_root: &Path, req_ulid: &str, body: &str) {
    let reply = match parse_named_vm_request(body, "destroy").and_then(|req| destroy_named_vm(&req))
    {
        Ok(name) => build_destroy_reply_ok(&name),
        Err(e) => {
            tracing::warn!(req = req_ulid, error = %e, "xcp_provision: destroy failed");
            build_error_reply(&e)
        }
    };
    write_reply(bus_root, req_ulid, &reply);
}

/// Handle one [`START_TOPIC`] request on a blocking thread + write the reply.
fn handle_start_blocking(bus_root: &Path, req_ulid: &str, body: &str) {
    let reply = match parse_named_vm_request(body, "start").and_then(|req| start_named_vm(&req)) {
        Ok(name) => build_start_reply_ok(&name),
        Err(e) => {
            tracing::warn!(req = req_ulid, error = %e, "xcp_provision: start failed");
            build_error_reply(&e)
        }
    };
    write_reply(bus_root, req_ulid, &reply);
}

/// Handle one [`SET_CREDS_TOPIC`] request on a blocking thread + write the reply.
///
/// XCP-7: parses the (password-bearing) body, age-encrypts the credential into the
/// mesh secret store under `xcp/<host>`, and replies with the host on success.
/// The password is never logged: only the host + any error string reach `tracing`.
fn handle_set_creds_blocking(bus_root: &Path, req_ulid: &str, body: &str) {
    let reply = match parse_set_creds_request(body).and_then(|req| store_dom0_credential(&req)) {
        Ok(host) => build_set_creds_reply_ok(&host),
        Err(e) => {
            tracing::warn!(req = req_ulid, error = %e, "xcp_provision: set-creds failed");
            build_error_reply(&e)
        }
    };
    write_reply(bus_root, req_ulid, &reply);
}

/// Cursors for the XCP-4 responder topics, advanced per drained request so each
/// request is handled exactly once across ticks. Seeded at each topic's tail on
/// worker start ([`ResponderCursors::seed_at_tail`]) so a restart does NOT
/// replay still-retained requests — load-bearing for the mutating `destroy` /
/// `start` verbs, which must not re-fire an old teardown / boot.
#[derive(Default)]
struct ResponderCursors {
    list: Option<String>,
    destroy: Option<String>,
    start: Option<String>,
    hosts: Option<String>,
    set_creds: Option<String>,
}

impl ResponderCursors {
    /// Seed every cursor past the current tail of its topic so only requests
    /// that arrive *after* the worker starts are handled (no backlog replay).
    fn seed_at_tail(bus_root: &Path) -> Self {
        let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
            return Self::default();
        };
        let tail = |topic: &str| persist.latest_ulid(topic).ok().flatten();
        Self {
            list: tail(LIST_TOPIC),
            destroy: tail(DESTROY_TOPIC),
            start: tail(START_TOPIC),
            hosts: tail(HOSTS_TOPIC),
            set_creds: tail(SET_CREDS_TOPIC),
        }
    }
}

/// Drain one tick of the XCP-4 responder topics, replying on each request's
/// `reply/<ulid>` lane. Each request's `xe`/`ssh` work runs on a blocking thread
/// (keeping the async loop responsive + `Persist` off the `.await`), exactly
/// like the spawn drain. Separate cursors mean these never interfere with the
/// spawn flow.
async fn drain_responders(bus_root: &Path, cursors: &mut ResponderCursors) {
    for req in read_new_requests(bus_root, LIST_TOPIC, &mut cursors.list) {
        let bus_root = bus_root.to_path_buf();
        run_blocking(move || handle_list_blocking(&bus_root, &req.ulid), "list").await;
    }
    for req in read_new_requests(bus_root, HOSTS_TOPIC, &mut cursors.hosts) {
        let bus_root = bus_root.to_path_buf();
        run_blocking(move || handle_hosts_blocking(&bus_root, &req.ulid), "hosts").await;
    }
    for req in read_new_requests(bus_root, START_TOPIC, &mut cursors.start) {
        let bus_root = bus_root.to_path_buf();
        run_blocking(
            move || handle_start_blocking(&bus_root, &req.ulid, &req.body),
            "start",
        )
        .await;
    }
    for req in read_new_requests(bus_root, DESTROY_TOPIC, &mut cursors.destroy) {
        let bus_root = bus_root.to_path_buf();
        run_blocking(
            move || handle_destroy_blocking(&bus_root, &req.ulid, &req.body),
            "destroy",
        )
        .await;
    }
    // XCP-7: the set/rotate-credential verb. Mutating (writes the secret store),
    // so its cursor is seeded at the tail like start/destroy — no replay of an
    // old credential write on a worker restart.
    for req in read_new_requests(bus_root, SET_CREDS_TOPIC, &mut cursors.set_creds) {
        let bus_root = bus_root.to_path_buf();
        run_blocking(
            move || handle_set_creds_blocking(&bus_root, &req.ulid, &req.body),
            "set-creds",
        )
        .await;
    }
}

/// Run a responder handler on `spawn_blocking`, logging a join failure under the
/// responder's `verb` label. Shared by the three responder drains.
async fn run_blocking<F: FnOnce() + Send + 'static>(f: F, verb: &str) {
    if let Err(e) = tokio::task::spawn_blocking(f).await {
        tracing::warn!(error = %e, verb, "xcp_provision: responder task join failed");
    }
}

/// Worker handle.
pub struct XcpProvisionWorker {
    poll_interval: Duration,
    bus_root_override: Option<PathBuf>,
}

impl Default for XcpProvisionWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl XcpProvisionWorker {
    /// Construct with production defaults.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Override the Bus root. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }
}

#[async_trait::async_trait]
impl Worker for XcpProvisionWorker {
    fn name(&self) -> &'static str {
        "xcp_provision"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::debug!("xcp_provision: no bus root; worker idle");
            return Ok(());
        };
        let mut cursor: Option<String> = None;
        // Seed the responder cursors at each topic's tail so a worker restart
        // doesn't replay still-retained requests (critical for the mutating
        // start/destroy verbs). The spawn cursor stays at None to match the
        // existing spawn flow's behavior.
        let mut responder_cursors = ResponderCursors::seed_at_tail(&bus_root);
        let mut tick = tokio::time::interval(self.poll_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let new_reqs = read_new_spawns(&bus_root, &mut cursor);
                    for req in new_reqs {
                        let bus_root = bus_root.clone();
                        // Each spawn shells out via xe/ssh + polls for the IP
                        // (up to IP_WAIT_TIMEOUT) on a blocking thread so the
                        // async run-loop stays responsive.
                        if let Err(e) =
                            tokio::task::spawn_blocking(move || run_spawn_blocking(bus_root, &req)).await
                        {
                            tracing::warn!(error = %e, "xcp_provision: spawn task join failed");
                        }
                    }
                    // XCP-4: drain the list/destroy/hosts responder topics the
                    // Workbench Provisioning panel queries — separate cursors,
                    // so they never disturb the spawn flow above.
                    drain_responders(&bus_root, &mut responder_cursors).await;
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_xcp::{HostCapacity, IdentitySeed, VmInfo, XcpError};
    use std::cell::RefCell;

    // ── parse / ack codecs ──

    #[test]
    fn parse_spawn_happy_path() {
        let body = r#"{"request_ulid":"01JAN","name":"web1","host":"172.20.0.4"}"#;
        let req = parse_spawn_request(body).expect("parse");
        assert_eq!(req.request_ulid, "01JAN");
        assert_eq!(req.name, "web1");
        assert_eq!(req.host.as_deref(), Some("172.20.0.4"));
    }

    #[test]
    fn parse_spawn_host_defaults_to_none() {
        let req = parse_spawn_request(r#"{"request_ulid":"01","name":"db"}"#).expect("parse");
        assert!(req.host.is_none());
    }

    #[test]
    fn parse_spawn_rejects_malformed() {
        assert!(parse_spawn_request("nope").is_err());
    }

    #[test]
    fn spawn_ack_ok_shape() {
        let body = build_spawn_ack_ok("u-1", "MDE-VM-web1", Some("10.42.0.9"));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["uuid"], "u-1");
        assert_eq!(v["hostname"], "MDE-VM-web1");
        assert_eq!(v["ip"], "10.42.0.9");
        assert!(!v.as_object().unwrap().contains_key("error"));
    }

    #[test]
    fn spawn_ack_ok_omits_ip_when_absent() {
        let body = build_spawn_ack_ok("u-1", "MDE-VM-web1", None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(!v.as_object().unwrap().contains_key("ip"));
    }

    #[test]
    fn spawn_ack_error_shape() {
        let body = build_spawn_ack_error("clone failed");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().contains("clone failed"));
        assert!(!v.as_object().unwrap().contains_key("uuid"));
    }

    // ── resolve_dom0 (allow-list guard) ──

    #[test]
    fn resolve_dom0_defaults_to_first_allowed() {
        let allowed = vec!["172.20.0.4".to_string(), "172.20.0.5".to_string()];
        assert_eq!(resolve_dom0(None, &allowed).unwrap(), "172.20.0.4");
    }

    #[test]
    fn resolve_dom0_accepts_an_allowed_request() {
        let allowed = vec!["172.20.0.4".to_string(), "172.20.0.5".to_string()];
        assert_eq!(
            resolve_dom0(Some("172.20.0.5"), &allowed).unwrap(),
            "172.20.0.5"
        );
    }

    #[test]
    fn resolve_dom0_rejects_unlisted_host() {
        let allowed = vec!["172.20.0.4".to_string()];
        assert!(resolve_dom0(Some("9.9.9.9"), &allowed).is_err());
    }

    #[test]
    fn resolve_dom0_errors_when_none_configured() {
        assert!(resolve_dom0(None, &[]).is_err());
    }

    // ── run_spawn reachability: a mock Hypervisor records the call order ──

    /// Records every trait call so the test can assert the real flow drives
    /// `set_identity_seed` (the unit's reachability requirement) between
    /// `clone_golden` and `start` — exercising the SAME `run_spawn` the live
    /// worker calls, just with a mock backend instead of `XeSsh`.
    #[derive(Default)]
    struct MockHv {
        calls: RefCell<Vec<String>>,
        seed: RefCell<Option<IdentitySeed>>,
        seeded_uuid: RefCell<Option<String>>,
    }

    impl Hypervisor for MockHv {
        fn clone_golden(&self, golden: &str, new_name: &str) -> Result<String, XcpError> {
            self.calls
                .borrow_mut()
                .push(format!("clone({golden}->{new_name})"));
            Ok("uuid-xyz".to_string())
        }
        fn set_identity_seed(&self, uuid: &str, seed: &IdentitySeed) -> Result<(), XcpError> {
            self.calls.borrow_mut().push(format!("seed({uuid})"));
            *self.seed.borrow_mut() = Some(seed.clone());
            *self.seeded_uuid.borrow_mut() = Some(uuid.to_string());
            Ok(())
        }
        fn set_memory(&self, uuid: &str, bytes: u64) -> Result<(), XcpError> {
            self.calls
                .borrow_mut()
                .push(format!("set_memory({uuid},{bytes})"));
            Ok(())
        }
        fn start(&self, uuid: &str) -> Result<(), XcpError> {
            self.calls.borrow_mut().push(format!("start({uuid})"));
            Ok(())
        }
        fn vm_ip(&self, _uuid: &str) -> Result<Option<String>, XcpError> {
            self.calls.borrow_mut().push("vm_ip".to_string());
            Ok(Some("10.42.0.9".to_string()))
        }
        fn destroy(&self, _uuid: &str) -> Result<(), XcpError> {
            unreachable!("destroy is not part of the spawn flow")
        }
        fn list(&self) -> Result<Vec<VmInfo>, XcpError> {
            unreachable!("list is not part of the spawn flow")
        }
        fn host_capacity(&self) -> Result<HostCapacity, XcpError> {
            // XPA-1: the spawn pre-check probes capacity first; return a host with
            // ample free RAM so the happy-path spawn proceeds.
            self.calls.borrow_mut().push("host_capacity".to_string());
            Ok(HostCapacity {
                cpu_count: 8,
                mem_total_kib: 16 * 1024 * 1024,
                mem_free_kib: 8 * 1024 * 1024,
                sr_free_bytes: 1 << 40,
                running_vms: 1,
            })
        }
    }

    #[test]
    fn run_spawn_calls_set_identity_seed_between_clone_and_start() {
        let hv = MockHv::default();
        let ack = run_spawn(
            &hv,
            "172.20.0.4",
            "web1",
            "ssh-ed25519 OPKEY op@host",
            Duration::ZERO, // skip the wait — vm_ip is probed once
            Duration::from_millis(1),
        )
        .expect("spawn ok");

        // REACHABILITY ASSERTION — the spawn probes capacity first (XPA-1
        // pre-check), then set_identity_seed runs between the clone and the start
        // and set_memory right-sizes the clone (XPA-1) before start. This is the
        // whole point of the unit: a provisioned VM gets its identity seed AND a
        // right-sized footprint, and the host is checked before fan-out.
        let calls = hv.calls.borrow().clone();
        assert_eq!(
            calls,
            vec![
                "host_capacity".to_string(),
                "clone(MDE-VM-golden->MDE-VM-web1)".to_string(),
                "seed(uuid-xyz)".to_string(),
                "set_memory(uuid-xyz,2147483648)".to_string(),
                "start(uuid-xyz)".to_string(),
                "vm_ip".to_string(),
            ],
            "capacity pre-check → clone → seed → set_memory → start order"
        );

        // The seed was attached to the freshly cloned uuid…
        assert_eq!(hv.seeded_uuid.borrow().as_deref(), Some("uuid-xyz"));
        // …and it carries the op key + the MDE-VM hostname + the uuid as the
        // first-boot instance-id (the A2 fresh-identity rule).
        let seed = hv.seed.borrow().clone().expect("seed recorded");
        assert!(seed.user_data.contains("hostname: MDE-VM-web1"));
        assert!(seed.user_data.contains("ssh-ed25519 OPKEY op@host"));
        assert_eq!(seed.instance_id, "uuid-xyz");

        // The ack reports the booted VM identity + the resolved IP.
        let v: serde_json::Value = serde_json::from_str(&ack).unwrap();
        assert_eq!(v["uuid"], "uuid-xyz");
        assert_eq!(v["hostname"], "MDE-VM-web1");
        assert_eq!(v["ip"], "10.42.0.9");
    }

    /// A failing `set_identity_seed` must abort the spawn (the VM never starts
    /// without its identity) and surface a clear error — not silently boot a
    /// mis-identified clone.
    #[test]
    fn run_spawn_aborts_when_seeding_fails() {
        struct SeedFails;
        impl Hypervisor for SeedFails {
            fn clone_golden(&self, _g: &str, _n: &str) -> Result<String, XcpError> {
                Ok("u-1".to_string())
            }
            fn set_identity_seed(&self, _u: &str, _s: &IdentitySeed) -> Result<(), XcpError> {
                Err(XcpError::Parse("boom".into()))
            }
            fn set_memory(&self, _u: &str, _b: u64) -> Result<(), XcpError> {
                panic!("set_memory must not run after a seed failure");
            }
            fn start(&self, _u: &str) -> Result<(), XcpError> {
                panic!("start must not run after a seed failure");
            }
            fn vm_ip(&self, _u: &str) -> Result<Option<String>, XcpError> {
                unreachable!()
            }
            fn destroy(&self, _u: &str) -> Result<(), XcpError> {
                unreachable!()
            }
            fn list(&self) -> Result<Vec<VmInfo>, XcpError> {
                unreachable!()
            }
            fn host_capacity(&self) -> Result<HostCapacity, XcpError> {
                // The pre-check runs first; give it ample RAM so the spawn gets
                // past it and reaches the (failing) seed step under test.
                Ok(HostCapacity {
                    mem_free_kib: 8 * 1024 * 1024,
                    ..HostCapacity::default()
                })
            }
        }
        let err = run_spawn(
            &SeedFails,
            "172.20.0.4",
            "web1",
            "ssh-ed25519 K op@h",
            Duration::ZERO,
            Duration::from_millis(1),
        )
        .expect_err("seed failure must abort");
        assert!(err.contains("identity seed"), "{err}");
    }

    // ── XPA-1: 2 GiB memory default + free-memory pre-check ──

    #[test]
    fn check_spawn_capacity_passes_when_host_has_room() {
        // Exactly the 2 GiB requirement free → OK (the boundary is inclusive).
        let cap = HostCapacity {
            mem_free_kib: SPAWN_MIN_FREE_MEM_KIB,
            ..HostCapacity::default()
        };
        assert!(check_spawn_capacity("172.20.0.4", &cap).is_ok());
        assert_eq!(SPAWN_MIN_FREE_MEM_KIB, 2 * 1024 * 1024); // 2 GiB in KiB
    }

    #[test]
    fn check_spawn_capacity_rejects_with_a_clear_message() {
        // 1 GiB free, the VM needs 2 GiB → reject. The XPA-1 requirement: a CLEAR
        // message (the host, its free RAM, the requirement) — NOT "no suitable
        // hosts".
        let cap = HostCapacity {
            mem_free_kib: 1024 * 1024, // 1 GiB
            ..HostCapacity::default()
        };
        let err = check_spawn_capacity("172.20.0.4", &cap).expect_err("too small");
        assert!(err.contains("172.20.0.4"), "names the host: {err}");
        assert!(err.contains("1024 MiB free"), "states free RAM: {err}");
        assert!(err.contains("2048 MiB"), "states the requirement: {err}");
        assert!(
            !err.contains("no suitable host"),
            "must not be the vague message: {err}"
        );
    }

    #[test]
    fn run_spawn_aborts_before_clone_when_host_is_overcommitted() {
        // XPA-1: the pre-check runs BEFORE any clone. A host short on RAM aborts
        // the spawn without fanning out a clone (which would overcommit it).
        struct TooSmall;
        impl Hypervisor for TooSmall {
            fn host_capacity(&self) -> Result<HostCapacity, XcpError> {
                Ok(HostCapacity {
                    mem_free_kib: 512 * 1024, // 512 MiB — far below 2 GiB
                    ..HostCapacity::default()
                })
            }
            fn clone_golden(&self, _g: &str, _n: &str) -> Result<String, XcpError> {
                panic!("clone must NOT run when the host can't fit the VM");
            }
            fn set_identity_seed(&self, _u: &str, _s: &IdentitySeed) -> Result<(), XcpError> {
                unreachable!()
            }
            fn set_memory(&self, _u: &str, _b: u64) -> Result<(), XcpError> {
                unreachable!()
            }
            fn start(&self, _u: &str) -> Result<(), XcpError> {
                unreachable!()
            }
            fn vm_ip(&self, _u: &str) -> Result<Option<String>, XcpError> {
                unreachable!()
            }
            fn destroy(&self, _u: &str) -> Result<(), XcpError> {
                unreachable!()
            }
            fn list(&self) -> Result<Vec<VmInfo>, XcpError> {
                unreachable!()
            }
        }
        let err = run_spawn(
            &TooSmall,
            "172.20.0.4",
            "web1",
            "ssh-ed25519 K op@h",
            Duration::ZERO,
            Duration::from_millis(1),
        )
        .expect_err("overcommit must abort the spawn");
        // The clear capacity message, surfaced as the spawn error (not a vague one).
        assert!(err.contains("only 512 MiB free"), "{err}");
        assert!(err.contains("2048 MiB"), "{err}");
    }

    // ── topic locks ──

    #[test]
    fn topic_locks_match_design_surface() {
        assert_eq!(SPAWN_TOPIC, "action/provision/spawn");
        assert!(SPAWN_ACK_PREFIX.starts_with("action/provision/"));
        assert_eq!(GOLDEN_TEMPLATE, "MDE-VM-golden");
        // XCP-4 responder topics live under the same action namespace so the
        // panel's `mde_bus::rpc::request` accepts them.
        assert_eq!(LIST_TOPIC, "action/provision/list");
        assert_eq!(DESTROY_TOPIC, "action/provision/destroy");
        assert_eq!(START_TOPIC, "action/provision/start");
        assert_eq!(HOSTS_TOPIC, "action/provision/hosts");
        // XCP-7 set/rotate-credential verb.
        assert_eq!(SET_CREDS_TOPIC, "action/provision/set-creds");
    }

    // ── XCP-4 list/destroy/hosts codecs ──

    #[test]
    fn list_reply_carries_each_vm_with_its_host() {
        let vms = vec![
            ListedVm {
                uuid: "u-1".into(),
                name: "MDE-VM-web1".into(),
                power_state: "running".into(),
                host: "172.20.0.4".into(),
            },
            ListedVm {
                uuid: "u-2".into(),
                name: "MDE-VM-db".into(),
                power_state: "halted".into(),
                host: "172.20.0.5".into(),
            },
        ];
        let v: serde_json::Value = serde_json::from_str(&build_list_reply(&vms)).unwrap();
        let arr = v["vms"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["uuid"], "u-1");
        assert_eq!(arr[0]["name"], "MDE-VM-web1");
        assert_eq!(arr[0]["power_state"], "running");
        assert_eq!(arr[0]["host"], "172.20.0.4");
        assert_eq!(arr[1]["host"], "172.20.0.5");
    }

    #[test]
    fn hosts_reply_distinguishes_ok_and_failed_rows() {
        let cap = HostCapacity {
            cpu_count: 8,
            mem_total_kib: 1024,
            mem_free_kib: 512,
            sr_free_bytes: 9000,
            running_vms: 3,
        };
        let rows = vec![
            HostRow::ok("172.20.0.4", &cap),
            HostRow::failed("172.20.0.5", "unreachable"),
        ];
        let v: serde_json::Value = serde_json::from_str(&build_hosts_reply(&rows)).unwrap();
        let arr = v["hosts"].as_array().unwrap();
        assert_eq!(arr[0]["host"], "172.20.0.4");
        assert_eq!(arr[0]["cpu_count"], 8);
        assert_eq!(arr[0]["running_vms"], 3);
        // An OK row carries no error key (skip_serializing_if).
        assert!(!arr[0].as_object().unwrap().contains_key("error"));
        // A failed row surfaces the probe error.
        assert_eq!(arr[1]["host"], "172.20.0.5");
        assert_eq!(arr[1]["error"], "unreachable");
    }

    #[test]
    fn parse_named_vm_request_happy_and_rejects_empty_name() {
        let req =
            parse_named_vm_request(r#"{"name":"MDE-VM-web1","host":"172.20.0.4"}"#, "destroy")
                .expect("parse");
        assert_eq!(req.name, "MDE-VM-web1");
        assert_eq!(req.host.as_deref(), Some("172.20.0.4"));
        // host defaults to None.
        let req = parse_named_vm_request(r#"{"name":"db"}"#, "start").expect("parse");
        assert!(req.host.is_none());
        // Empty / missing name is rejected (we'd have no VM to act on); the verb
        // is named in the error.
        let err = parse_named_vm_request(r#"{"name":"  "}"#, "start").expect_err("empty");
        assert!(err.contains("start"), "{err}");
        assert!(parse_named_vm_request("nope", "destroy").is_err());
    }

    #[test]
    fn destroy_and_start_reply_ok_and_error_shapes() {
        let ok: serde_json::Value =
            serde_json::from_str(&build_destroy_reply_ok("MDE-VM-web1")).unwrap();
        assert_eq!(ok["destroyed"], "MDE-VM-web1");
        assert!(!ok.as_object().unwrap().contains_key("error"));
        let started: serde_json::Value =
            serde_json::from_str(&build_start_reply_ok("MDE-VM-web1")).unwrap();
        assert_eq!(started["started"], "MDE-VM-web1");
        let err: serde_json::Value = serde_json::from_str(&build_error_reply("boom")).unwrap();
        assert_eq!(err["error"], "boom");
    }

    #[test]
    fn resolve_uuid_by_name_maps_or_errors() {
        let vms = vec![
            VmInfo {
                uuid: "u-1".into(),
                name: "MDE-VM-web1".into(),
                power_state: "running".into(),
            },
            VmInfo {
                uuid: "u-2".into(),
                name: "MDE-VM-db".into(),
                power_state: "halted".into(),
            },
        ];
        assert_eq!(resolve_uuid_by_name(&vms, "MDE-VM-db").unwrap(), "u-2");
        assert!(resolve_uuid_by_name(&vms, "MDE-VM-ghost").is_err());
    }

    // ── XCP-7 dom0 credential: codec + store round-trip ──

    /// A `LocalAead` secret store over a tempdir with a real-ish age identity —
    /// the same shape the secret_store tests drive (the bytes key the AEAD).
    fn local_store() -> (tempfile::TempDir, SecretStore) {
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("mcnf-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        let store = SecretStore::LocalAead {
            dir: tmp.path().join("secrets"),
            key_path,
        };
        (tmp, store)
    }

    #[test]
    fn parse_set_creds_happy_and_rejects_blank_fields() {
        let req =
            parse_set_creds_request(r#"{"host":"172.20.0.4","password":"pw"}"#).expect("parse");
        assert_eq!(req.host, "172.20.0.4");
        assert_eq!(req.password, "pw");
        // A blank host (nothing to key on) or a blank password (would mask the
        // honest "no credential stored" state) is rejected.
        assert!(parse_set_creds_request(r#"{"host":"  ","password":"pw"}"#).is_err());
        assert!(parse_set_creds_request(r#"{"host":"h","password":""}"#).is_err());
        assert!(parse_set_creds_request("not json").is_err());
    }

    #[test]
    fn set_creds_request_debug_redacts_the_password() {
        // XCP-7: the request must not leak the password into a log/panic line.
        let req = SetCredsRequest {
            host: "172.20.0.4".into(),
            password: "top-secret-pw".into(),
        };
        let dbg = format!("{req:?}");
        assert!(
            !dbg.contains("top-secret-pw"),
            "password leaked into Debug: {dbg}"
        );
        assert!(dbg.contains("***"));
        assert!(dbg.contains("172.20.0.4"));
    }

    #[test]
    fn set_creds_reply_ok_carries_host_not_secret() {
        let v: serde_json::Value =
            serde_json::from_str(&build_set_creds_reply_ok("172.20.0.4")).unwrap();
        assert_eq!(v["stored"], "172.20.0.4");
        assert!(!v.as_object().unwrap().contains_key("error"));
        // The success reply never carries a password field.
        assert!(!v.as_object().unwrap().contains_key("password"));
    }

    #[test]
    fn dom0_credential_round_trips_and_absent_is_honest() {
        // XCP-7 acceptance: store a dom0 credential, then any authorized node
        // reads it back; an unstored host is the honest "no credential" (None).
        let (_t, store) = local_store();
        let allowed = vec!["172.20.0.4".to_string(), "172.20.0.5".to_string()];
        // Absent before set → key-only auth path.
        assert_eq!(read_dom0_password(&store, "172.20.0.4"), None);
        // The leader stores it…
        let req = SetCredsRequest {
            host: "172.20.0.4".into(),
            password: "dom0-pw-xyz".into(),
        };
        let host = store_dom0_credential_in(&store, &req, &allowed).expect("stored");
        assert_eq!(host, "172.20.0.4");
        // …and it reads back decrypted, byte-for-byte (any enrolled node).
        assert_eq!(
            read_dom0_password(&store, "172.20.0.4").as_deref(),
            Some("dom0-pw-xyz")
        );
        // A different, un-credentialed allow-listed host stays honestly absent.
        assert_eq!(read_dom0_password(&store, "172.20.0.5"), None);
    }

    #[test]
    fn set_creds_rejects_a_host_not_in_the_allow_list() {
        // The same allow-list guard the spawn/destroy verbs apply: a node can't
        // be made to store a credential for an arbitrary host.
        let (_t, store) = local_store();
        let allowed = vec!["172.20.0.4".to_string()];
        let req = SetCredsRequest {
            host: "9.9.9.9".into(),
            password: "pw".into(),
        };
        let err = store_dom0_credential_in(&store, &req, &allowed).expect_err("rejected");
        assert!(err.contains("allow-list"), "{err}");
        // Nothing was written for the rejected host.
        assert_eq!(read_dom0_password(&store, "9.9.9.9"), None);
    }
}
