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
    build_identity_seed, build_join_seed, mde_vm_hostname, precheck_host_memory, HostCapacity,
    HostTarget, Hypervisor, VmInfo, XeSsh, DEFAULT_SERVER_VM_MEM_BYTES, HOST_MEM_HEADROOM_KIB,
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

/// The role every provisioned MDE-VM pins on join — headless **Server** (design
/// A2/§5; XPA-1 right-sizes its memory, XPA-7 mints its join token for this role).
pub const SPAWN_ROLE: &str = "server";

/// XCP-6 — max age of a published `compute/xcp-host/<node>` capacity advert before
/// the host-capacity consumer ignores it and direct-probes the dom0 instead. The
/// `xcp_host` worker republishes every 15 s ([`super::xcp_host::INTERVAL`]), so
/// 60 s tolerates a few missed ticks while still catching a dom0 gone quiet.
pub const PUBLISHED_CAP_MAX_AGE_MS: u64 = 60_000;

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

/// Drive the full A-plane spawn over a [`Hypervisor`].
///
/// Precheck → clone → right-size memory (XPA-1) → reset VIF MACs (XPA-4) → attach
/// the fresh identity seed (with the XPA-7 self-join when `join_token` is set) →
/// start → poll for the IP. Generic over the hypervisor so the whole flow's
/// reachability is provable with a mock (no live dom0).
///
/// - `op_ssh_key` is the operator's authorized public key the seed installs.
/// - `role` is the role a self-joining clone pins (headless [`SPAWN_ROLE`]).
/// - `join_token` is the **v3 add-peer** join token (XPA-7) the clone runs
///   `mackesd join` with on first boot; `None` ⇒ no auto-join (the clone still
///   boots with the op key + hostname, the operator joins it).
/// - `wait`/`poll` bound the IP wait (0 ⇒ skip the wait).
///
/// Returns the success-ack body; `Err(description)` becomes an error-ack.
///
/// # Errors
/// The host-memory precheck (XPA-1) failing, or any `xe`/`ssh` step (capacity /
/// clone / memory / VIF / seed / start) failing, aborts the spawn.
pub fn run_spawn<H: Hypervisor>(
    hv: &H,
    name: &str,
    op_ssh_key: &str,
    role: &str,
    join_token: Option<&str>,
    wait: Duration,
    poll: Duration,
) -> Result<String, String> {
    let hostname = mde_vm_hostname(name);

    // 1. XPA-1 — host-free-memory precheck BEFORE the clone, so an over-committed
    //    host fails fast with a clear message instead of cloning a VM that then
    //    can't start ("a 15.9 GB host couldn't start 4×4 GB VMs + the base").
    let cap = hv
        .host_capacity()
        .map_err(|e| format!("probe host capacity (pre-spawn): {e}"))?;
    let per_vm_kib = DEFAULT_SERVER_VM_MEM_BYTES / 1024;
    precheck_host_memory(&cap, per_vm_kib, 1, HOST_MEM_HEADROOM_KIB)?;

    // 2a. Clone the golden template into the new MDE-VM.
    let uuid = hv
        .clone_golden(GOLDEN_TEMPLATE, &hostname)
        .map_err(|e| format!("clone {GOLDEN_TEMPLATE} → {hostname}: {e}"))?;

    // 2b. XPA-1 — right-size the clone to the headless-Server default (2 GB), not
    //     the golden's larger size (must happen while halted, before start).
    hv.set_memory(&uuid, DEFAULT_SERVER_VM_MEM_BYTES)
        .map_err(|e| format!("right-size memory on {uuid}: {e}"))?;

    // 2c. XPA-4 — give every VIF a fresh MAC so clones don't collide on the
    //     golden's copied MAC (also while halted).
    hv.reset_vif_macs(&uuid)
        .map_err(|e| format!("reset VIF MAC on {uuid}: {e}"))?;

    // 2d. THE load-bearing step — attach the fresh cloud-init identity seed so the
    //     clone boots with a new hostname/host-keys/machine-id + the op key (A2).
    //     XPA-7: when a v3 join token was minted, the seed ALSO self-enrolls the
    //     clone via `mackesd join` against the PUBLIC /enroll endpoint the token
    //     pins (NOT the legacy enroll-token's unreachable overlay IP — subsumes
    //     XPA-5). The instance-id is the new uuid (cloud-init first-boot trigger).
    let seed = join_token.map_or_else(
        || build_identity_seed(name, op_ssh_key, &uuid),
        |tok| build_join_seed(name, op_ssh_key, &uuid, tok, role),
    );
    hv.set_identity_seed(&uuid, &seed)
        .map_err(|e| format!("attach identity seed to {uuid}: {e}"))?;

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
    let hv = XeSsh::new(hv_target_for(&resolve_dom0(
        req.host.as_deref(),
        &allowed_dom0s(),
    )?));
    // XPA-7 — mint a single-use v3 join token (add-peer) so the clone self-enrolls
    // on first boot against the PUBLIC /enroll endpoint. Best-effort: a non-founded
    // provisioning node degrades to no auto-join rather than failing the spawn.
    let join_token = mint_join_token(SPAWN_ROLE, &req.name);
    run_spawn(
        &hv,
        &req.name,
        &operator_ssh_key(),
        SPAWN_ROLE,
        join_token.as_deref(),
        IP_WAIT_TIMEOUT,
        IP_WAIT_POLL,
    )
}

/// Build the `mackesd add-peer` argv that mints a **v3** join token for a clone
/// (XPA-7). Letting add-peer auto-detect this lighthouse's public IPv4 + use the
/// default `/enroll` port is exactly what pins the *reachable* endpoint + cert
/// fingerprint into the token (the v3 contract) — the fix for the legacy
/// `enroll-token`, which advertised the unreachable Nebula overlay IP (XPA-5).
#[must_use]
pub fn mackesd_add_peer_argv(role: &str, note: &str) -> Vec<String> {
    vec![
        "add-peer".into(),
        "--role".into(),
        role.into(),
        "--note".into(),
        note.into(),
    ]
}

/// XPA-7 — mint a single-use v3 join token for a clone by running
/// `mackesd add-peer` on THIS (founded-lighthouse / leader) node, reusing the
/// same binary that's currently executing (`current_exe`). The minted token pins
/// the public `/enroll` endpoint (`:4243`) + the endpoint cert fingerprint, so a
/// fresh LAN/internet clone can actually reach it.
///
/// Returns `None` — degrade to no auto-join, the spawn still succeeds — when
/// add-peer isn't possible (this node isn't a founded lighthouse, the binary is
/// missing, or it emitted no parseable token). Validated through the canonical
/// [`crate::nebula_enroll::parse_join_token`] so a non-token stdout never rides
/// into the seed.
fn mint_join_token(role: &str, name: &str) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let note = format!("xcp-provision {name}");
    let out = std::process::Command::new(&exe)
        .args(mackesd_add_peer_argv(role, &note))
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::warn!(
            role, name,
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "xcp_provision: `mackesd add-peer` failed; clone will boot without auto-join"
        );
        return None;
    }
    // add-peer prints the token on stdout; status/help goes to stderr.
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if crate::nebula_enroll::parse_join_token(&token).is_some() {
        Some(token)
    } else {
        tracing::warn!(
            role,
            name,
            "xcp_provision: add-peer produced no parseable join token; no auto-join"
        );
        None
    }
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

/// Resolve each configured dom0's capacity for a [`HOSTS_TOPIC`] reply (XCP-6).
///
/// **Primary source: the published advert.** The `xcp_host` worker publishes each
/// dom0's capacity to `compute/xcp-host/<node>` on a 15 s cadence; this consumer
/// prefers a FRESH one of those (matched to the dom0 by its advertised address /
/// hostname) over a fresh SSH probe — so the common "list spawn targets" path
/// costs a local bus read, not N round-trips of `xe` over SSH. A direct probe is
/// the fallback **only** when no fresh advert matches (absent / stale dom0), so
/// the result is never worse than the prior always-probe behaviour; a per-host
/// probe failure records an error row (so the panel can grey it out).
fn probe_all_hosts(bus_root: &Path) -> Vec<HostRow> {
    // One store resolution for the whole probe sweep (XCP-7 efficiency).
    let store = dom0_secret_store();
    // One bus read of all published adverts for the whole sweep (XCP-6).
    let published = read_published_caps(bus_root);
    let now = now_ms();
    allowed_dom0s()
        .into_iter()
        .map(|dom0| {
            // XCP-6: a fresh published advert wins — no SSH needed.
            if let Some(p) =
                select_published_capacity(&dom0, &published, now, PUBLISHED_CAP_MAX_AGE_MS)
            {
                return HostRow::ok(&dom0, &p.capacity);
            }
            // Fallback: the advert is absent/stale → probe the dom0 directly.
            let hv = XeSsh::new(hv_target_with_store(&dom0, &store));
            match hv.host_capacity() {
                Ok(cap) => HostRow::ok(&dom0, &cap),
                Err(e) => HostRow::failed(&dom0, &e.to_string()),
            }
        })
        .collect()
}

/// A parsed `compute/xcp-host/<node>` capacity advert (XCP-6). The provisioner
/// prefers a fresh one of these over a direct SSH probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedCapacity {
    /// The dom0's reachable address the advert carried (matches an
    /// `MCNF_XEN_DOM0S` allow-list entry — the primary match key).
    pub address: String,
    /// The dom0 hostname (secondary match key, for an allow-list keyed by name).
    pub hostname: String,
    /// Publish timestamp (ms since epoch) — drives the staleness check.
    pub ts_ms: u64,
    /// The advertised capacity.
    pub capacity: HostCapacity,
}

/// Parse one `compute/xcp-host/<node>` advert body (the [`super::xcp_host`] doc)
/// into a [`PublishedCapacity`]. `None` when the body isn't an `xcp-host` doc /
/// is malformed. Pure so the selection logic is testable without the bus.
#[must_use]
pub fn parse_published_capacity(body: &str) -> Option<PublishedCapacity> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if v.get("kind").and_then(serde_json::Value::as_str) != Some("xcp-host") {
        return None;
    }
    let cap = v.get("capacity")?;
    let u64f = |k: &str| cap.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let u32f = |k: &str| u32::try_from(u64f(k)).unwrap_or(0);
    let strf = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(PublishedCapacity {
        address: strf("address"),
        hostname: strf("hostname"),
        ts_ms: v
            .get("ts_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        capacity: HostCapacity {
            cpu_count: u32f("cpu_count"),
            mem_total_kib: u64f("mem_total_kib"),
            mem_free_kib: u64f("mem_free_kib"),
            sr_free_bytes: u64f("sr_free_bytes"),
            running_vms: u32f("running_vms"),
        },
    })
}

/// XCP-6 selection: the freshest published advert for `dom0` (matched by the
/// advert's address or hostname) that is within `max_age_ms` of `now_ms`, or
/// `None` when none match / all are stale (the caller then direct-probes).
#[must_use]
pub fn select_published_capacity<'a>(
    dom0: &str,
    published: &'a [PublishedCapacity],
    now_ms: u64,
    max_age_ms: u64,
) -> Option<&'a PublishedCapacity> {
    published
        .iter()
        .filter(|p| p.address == dom0 || p.hostname == dom0)
        .filter(|p| now_ms.saturating_sub(p.ts_ms) <= max_age_ms)
        .max_by_key(|p| p.ts_ms)
}

/// Read the latest published `compute/xcp-host/*` advert per topic from the bus
/// (XCP-6). A bus fault / missing topic degrades to an empty list → every dom0
/// direct-probes (the prior behaviour), so this is a safe optimization, never a
/// regression.
fn read_published_caps(bus_root: &Path) -> Vec<PublishedCapacity> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(topics) = persist.list_topics() else {
        return vec![];
    };
    let prefix = super::xcp_host::TOPIC_PREFIX;
    let mut out = Vec::new();
    for topic in topics {
        if !topic.starts_with(prefix) {
            continue;
        }
        // The newest message on the topic is the current advert.
        let Ok(msgs) = persist.list_since(&topic, None) else {
            continue;
        };
        if let Some(cap) = msgs
            .last()
            .and_then(|m| m.body.as_deref())
            .and_then(parse_published_capacity)
        {
            out.push(cap);
        }
    }
    out
}

/// Wall-clock ms since the epoch (stamps the XCP-6 freshness check).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
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
    write_reply(
        bus_root,
        req_ulid,
        &build_hosts_reply(&probe_all_hosts(bus_root)),
    );
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
        fn reset_vif_macs(&self, uuid: &str) -> Result<(), XcpError> {
            self.calls
                .borrow_mut()
                .push(format!("reset_vif_macs({uuid})"));
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
            // The XPA-1 precheck probes capacity first; report a roomy host so the
            // happy-path flow proceeds to the clone.
            self.calls.borrow_mut().push("host_capacity".to_string());
            Ok(HostCapacity {
                cpu_count: 8,
                mem_total_kib: 32 * 1024 * 1024,
                mem_free_kib: 16 * 1024 * 1024,
                sr_free_bytes: 500_000_000_000,
                running_vms: 1,
            })
        }
    }

    #[test]
    fn run_spawn_drives_the_full_flow_in_order() {
        let hv = MockHv::default();
        let ack = run_spawn(
            &hv,
            "web1",
            "ssh-ed25519 OPKEY op@host",
            SPAWN_ROLE,
            None,           // no join token → plain identity seed
            Duration::ZERO, // skip the wait — vm_ip is probed once
            Duration::from_millis(1),
        )
        .expect("spawn ok");

        // REACHABILITY ASSERTION — the full XPA-1/4 + seed flow runs in order:
        // precheck (host_capacity) → clone → right-size memory → reset VIF MACs →
        // identity seed → start → ip. This proves every step is actually reached.
        let calls = hv.calls.borrow().clone();
        assert_eq!(
            calls,
            vec![
                "host_capacity".to_string(),
                "clone(MDE-VM-golden->MDE-VM-web1)".to_string(),
                format!("set_memory(uuid-xyz,{})", 2u64 * 1024 * 1024 * 1024),
                "reset_vif_macs(uuid-xyz)".to_string(),
                "seed(uuid-xyz)".to_string(),
                "start(uuid-xyz)".to_string(),
                "vm_ip".to_string(),
            ],
            "precheck→clone→memory→vif→seed→start must run in order"
        );

        // The seed was attached to the freshly cloned uuid…
        assert_eq!(hv.seeded_uuid.borrow().as_deref(), Some("uuid-xyz"));
        // …and it carries the op key + the MDE-VM hostname + the uuid as the
        // first-boot instance-id (the A2 fresh-identity rule). No join token ⇒ no
        // self-join runcmd.
        let seed = hv.seed.borrow().clone().expect("seed recorded");
        assert!(seed.user_data.contains("hostname: MDE-VM-web1"));
        assert!(seed.user_data.contains("ssh-ed25519 OPKEY op@host"));
        assert!(!seed.user_data.contains("mackesd"));
        assert_eq!(seed.instance_id, "uuid-xyz");

        // The ack reports the booted VM identity + the resolved IP.
        let v: serde_json::Value = serde_json::from_str(&ack).unwrap();
        assert_eq!(v["uuid"], "uuid-xyz");
        assert_eq!(v["hostname"], "MDE-VM-web1");
        assert_eq!(v["ip"], "10.42.0.9");
    }

    #[test]
    fn run_spawn_embeds_the_v3_self_join_when_a_token_is_minted() {
        // XPA-7 — with a v3 join token the seed self-enrolls the clone via
        // `mackesd join` against the token's PUBLIC endpoint (subsumes XPA-5).
        let hv = MockHv::default();
        let token = "mesh:home@203.0.113.5:4243#BEARERxyz?fp=deadbeefdeadbeef";
        run_spawn(
            &hv,
            "web1",
            "ssh-ed25519 OPKEY op@host",
            SPAWN_ROLE,
            Some(token),
            Duration::ZERO,
            Duration::from_millis(1),
        )
        .expect("spawn ok");
        let seed = hv.seed.borrow().clone().expect("seed recorded");
        // The clone runs `mackesd join '<token>' --role server` as an exec list —
        // the token (with its overlay-IP-killing public endpoint) rides verbatim.
        assert!(
            seed.user_data.contains(&format!(
                "[ 'mackesd', 'join', '{token}', '--role', 'server' ]"
            )),
            "missing v3 self-join runcmd: {}",
            seed.user_data
        );
    }

    #[test]
    fn run_spawn_aborts_early_when_the_host_is_over_committed() {
        // XPA-1 — the precheck fails BEFORE any clone, with a clear message.
        struct TightHost;
        impl Hypervisor for TightHost {
            fn host_capacity(&self) -> Result<HostCapacity, XcpError> {
                // Only 1 GiB free — can't fit a 2 GiB Server VM + headroom.
                Ok(HostCapacity {
                    mem_free_kib: 1024 * 1024,
                    ..HostCapacity::default()
                })
            }
            fn clone_golden(&self, _g: &str, _n: &str) -> Result<String, XcpError> {
                panic!("clone must not run after a failed precheck");
            }
            fn set_identity_seed(&self, _u: &str, _s: &IdentitySeed) -> Result<(), XcpError> {
                unreachable!()
            }
            fn set_memory(&self, _u: &str, _b: u64) -> Result<(), XcpError> {
                unreachable!()
            }
            fn reset_vif_macs(&self, _u: &str) -> Result<(), XcpError> {
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
            &TightHost,
            "web1",
            "ssh-ed25519 K op@h",
            SPAWN_ROLE,
            None,
            Duration::ZERO,
            Duration::from_millis(1),
        )
        .expect_err("over-committed host must fail the precheck");
        assert!(err.contains("insufficient host free memory"), "{err}");
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
                Ok(())
            }
            fn reset_vif_macs(&self, _u: &str) -> Result<(), XcpError> {
                Ok(())
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
                // Roomy host so the precheck passes and we reach the seed step.
                Ok(HostCapacity {
                    mem_free_kib: 16 * 1024 * 1024,
                    ..HostCapacity::default()
                })
            }
        }
        let err = run_spawn(
            &SeedFails,
            "web1",
            "ssh-ed25519 K op@h",
            SPAWN_ROLE,
            None,
            Duration::ZERO,
            Duration::from_millis(1),
        )
        .expect_err("seed failure must abort");
        assert!(err.contains("identity seed"), "{err}");
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

    // ── XPA-7: the add-peer token-minting command ──

    #[test]
    fn mackesd_add_peer_argv_mints_a_v3_token_for_the_server_role() {
        // Auto-detected lighthouse + default enroll port is exactly what pins the
        // reachable /enroll endpoint + fp into the v3 token (the XPA-5 fix).
        assert_eq!(
            mackesd_add_peer_argv("server", "xcp-provision web1"),
            vec![
                "add-peer",
                "--role",
                "server",
                "--note",
                "xcp-provision web1"
            ]
        );
    }

    // ── XCP-6: consume the published capacity advert ──

    fn xcp_host_body(address: &str, hostname: &str, ts_ms: u64, mem_free_kib: u64) -> String {
        // The exact shape the `xcp_host` worker publishes (super::xcp_host::xcp_host_doc).
        serde_json::json!({
            "ok": true,
            "kind": "xcp-host",
            "role": "hypervisor",
            "node_id": format!("peer:{hostname}"),
            "hostname": hostname,
            "address": address,
            "ts_ms": ts_ms,
            "capacity": {
                "cpu_count": 8,
                "mem_total_kib": 32 * 1024 * 1024_u64,
                "mem_free_kib": mem_free_kib,
                "sr_free_bytes": 500_000_000_000_u64,
                "running_vms": 2,
            },
        })
        .to_string()
    }

    #[test]
    fn parse_published_capacity_decodes_an_xcp_host_doc() {
        let p = parse_published_capacity(&xcp_host_body(
            "172.20.0.9",
            "xen-home",
            42,
            9 * 1024 * 1024,
        ))
        .expect("parse");
        assert_eq!(p.address, "172.20.0.9");
        assert_eq!(p.hostname, "xen-home");
        assert_eq!(p.ts_ms, 42);
        assert_eq!(p.capacity.cpu_count, 8);
        assert_eq!(p.capacity.mem_free_kib, 9 * 1024 * 1024);
        assert_eq!(p.capacity.running_vms, 2);
        // A non-xcp-host body / garbage is rejected.
        assert!(parse_published_capacity(r#"{"kind":"something-else"}"#).is_none());
        assert!(parse_published_capacity("not json").is_none());
    }

    #[test]
    fn select_published_capacity_prefers_fresh_matching_adverts() {
        let now = 1_000_000_u64;
        let fresh = parse_published_capacity(&xcp_host_body(
            "172.20.0.9",
            "xen-home",
            now - 5_000, // 5 s old → fresh
            9 * 1024 * 1024,
        ))
        .unwrap();
        let stale = parse_published_capacity(&xcp_host_body(
            "172.20.0.51",
            "xen-kvm1",
            now - 120_000, // 2 min old → stale
            7 * 1024 * 1024,
        ))
        .unwrap();
        let published = vec![fresh, stale];

        // A fresh advert matched by ADDRESS is chosen (no direct probe needed).
        let hit =
            select_published_capacity("172.20.0.9", &published, now, PUBLISHED_CAP_MAX_AGE_MS)
                .expect("fresh advert selected");
        assert_eq!(hit.capacity.mem_free_kib, 9 * 1024 * 1024);
        // A stale advert is rejected → caller falls back to a direct probe.
        assert!(select_published_capacity(
            "172.20.0.51",
            &published,
            now,
            PUBLISHED_CAP_MAX_AGE_MS
        )
        .is_none());
        // An allow-listed dom0 with no advert at all → None (direct probe).
        assert!(select_published_capacity(
            "172.20.0.99",
            &published,
            now,
            PUBLISHED_CAP_MAX_AGE_MS
        )
        .is_none());
        // Matching by HOSTNAME also works (an allow-list keyed by name).
        assert!(
            select_published_capacity("xen-home", &published, now, PUBLISHED_CAP_MAX_AGE_MS)
                .is_some()
        );
    }

    #[test]
    fn select_published_capacity_picks_the_newest_when_several_match() {
        let now = 2_000_000_u64;
        let older = parse_published_capacity(&xcp_host_body(
            "172.20.0.9",
            "xen-home",
            now - 30_000,
            5 * 1024 * 1024,
        ))
        .unwrap();
        let newer = parse_published_capacity(&xcp_host_body(
            "172.20.0.9",
            "xen-home",
            now - 2_000,
            8 * 1024 * 1024,
        ))
        .unwrap();
        let published = vec![older, newer];
        let hit =
            select_published_capacity("172.20.0.9", &published, now, PUBLISHED_CAP_MAX_AGE_MS)
                .expect("a fresh advert");
        // The newest advert wins (its free-memory figure, not the older one's).
        assert_eq!(hit.capacity.mem_free_kib, 8 * 1024 * 1024);
        assert_eq!(hit.ts_ms, now - 2_000);
    }
}
