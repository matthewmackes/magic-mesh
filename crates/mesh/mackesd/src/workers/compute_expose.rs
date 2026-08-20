//! VIRT-7 (v5.0.0) — per-network firewalld port forwarding for
//! VMs.
//!
//! Each peer drains its own
//! `compute/expose/<own-peer-nebula-addr>` +
//! `compute/unexpose/<own-peer-nebula-addr>` Bus topics. For each
//! `expose` request, builds a `firewall-cmd --permanent` rich rule
//! per selected network mapping the host port to the VM's Nebula IP
//! and applies it via `firewall-cmd --reload`. For `unexpose`,
//! removes the matching rules. Publishes the current active-rule
//! set to `compute/exposed/<own-peer-nebula-addr>` so the
//! Workbench can render the per-VM expose state without re-querying
//! firewalld.
//!
//! ## Network → zone mapping (design doc §7)
//!
//! - **`mesh`** → `trusted` zone (Nebula interface, `nebula1`).
//!   Rich rule scoped to the local Nebula overlay IP as
//!   `destination address` so the forward only fires for packets
//!   already on the overlay.
//! - **`lan`**  → `public` zone (LAN interface). No destination
//!   filter — any LAN packet to the host port is forwarded.
//! - **`wan`**  → WAN zone, detected at startup via
//!   `nmcli -t -f DEVICE,TYPE,STATE device` + the default-gateway
//!   interface's `firewall-cmd --get-zone-of-interface=<dev>`.
//!   Falls back to `public` when detection fails (single-network
//!   hosts where LAN + WAN are the same zone).
//!
//! ## Active-rule shadow set
//!
//! The worker tracks `(network, vm_nebula_ip, host_port, proto)`
//! tuples in-memory; this is the authoritative source for the
//! `compute/exposed/<peer>` published topic. firewalld stores the
//! rules durably (`--permanent`), so the rules survive worker
//! restarts; the shadow set does NOT (it starts empty on every
//! mackesd boot) — that's a follow-up (VIRT-7.followup: rebuild
//! shadow set from `firewall-cmd --list-rich-rules` on startup).
//! Acceptable until the Workbench Compute panel ships, since the
//! next expose/unexpose request always re-publishes the live set.
//!
//! ## Silent no-op
//!
//! When `firewall-cmd` is absent on PATH (containerised CI peer,
//! lighthouse profile), the worker logs once and quiesces until shutdown
//! without repeatedly probing a statically unavailable provider.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

use super::proc::{output_with_timeout, status_with_timeout, DEFAULT_CMD_TIMEOUT};
use super::{ShutdownToken, Worker};

/// Default poll cadence — control surface (firewalld changes are
/// not on a human's interactive path).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// Closed capability verb for adding compute firewall forwards.
pub const COMPUTE_EXPOSE_AUTH_VERB: &str = "compute-expose";

/// Closed capability verb for removing compute firewall forwards.
pub const COMPUTE_UNEXPOSE_AUTH_VERB: &str = "compute-unexpose";

/// Stable node scope shared by the compute firewall capabilities.
pub const COMPUTE_EXPOSE_NODE_SCOPE: &str = "compute";

/// Nebula overlay interface name (firewall_monitor + compute_registry
/// both bind here; matches the v2.5 NF-6.1 enrollment convention).
pub const DEFAULT_NEBULA_INTERFACE: &str = "nebula1";

/// firewalld zone for the `mesh` network selector.
pub const MESH_ZONE: &str = "trusted";

/// firewalld zone for the `lan` network selector.
pub const LAN_ZONE: &str = "public";

/// Fallback zone when WAN-zone detection fails (single-network
/// hosts where LAN + WAN coincide).
pub const DEFAULT_WAN_ZONE: &str = "public";

/// Root-owned crash journal for consumed compute firewall actions.
pub const DEFAULT_ACTION_JOURNAL_PATH: &str = "/var/lib/mackesd/compute-expose/action-journal.json";
const ACTION_JOURNAL_SCHEMA_VERSION: u16 = 1;
const ACTION_JOURNAL_MAX_ENTRIES: usize = 1024;
const ACTION_JOURNAL_MAX_BYTES: u64 = 1024 * 1024;
const BUS_LANE_MAX_MESSAGES: usize = 4096;
const BUS_LANE_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ComputeExposeBusIdentity {
    device: u64,
    inode: u64,
}

/// Which network the expose rule applies to.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    /// Nebula overlay — `trusted` zone with `destination address`
    /// filter on the local overlay IP.
    Mesh,
    /// LAN — `public` zone, no destination filter.
    Lan,
    /// WAN — detected zone (or [`DEFAULT_WAN_ZONE`] fallback).
    Wan,
}

impl Network {
    /// Parse the lowercase wire name. Unknown strings yield `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mesh" => Some(Self::Mesh),
            "lan" => Some(Self::Lan),
            "wan" => Some(Self::Wan),
            _ => None,
        }
    }

    /// Wire name (matches the JSON serde rename).
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Mesh => "mesh",
            Self::Lan => "lan",
            Self::Wan => "wan",
        }
    }
}

/// Expose-request payload per design doc §3.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExposeRequest {
    /// VM's Nebula overlay IP — the rich rule's `to-addr`.
    pub vm_nebula_ip: String,
    /// Port inside the guest. The host port is set equal to this
    /// per the v1 1:1 mapping (operator can change after by editing
    /// rules manually; future schema rev can add an explicit
    /// `host_port` field).
    pub guest_port: u16,
    /// Protocol — `tcp` or `udp`. Free-form to keep tests cheap.
    pub proto: String,
    /// Which networks to expose on. Subset of `{mesh, lan, wan}`.
    pub networks: Vec<Network>,
}

/// Unexpose-request payload per design doc §3 / VIRT-7 task body.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnexposeRequest {
    /// VM's Nebula overlay IP.
    pub vm_nebula_ip: String,
    /// Host port to remove forwarding for (matches the prior
    /// `expose`'s guest_port under the v1 1:1 mapping).
    pub host_port: u16,
    /// Protocol.
    pub proto: String,
}

/// One active forwarding rule tracked in the shadow set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct ActiveRule {
    /// Which network the rule lives on.
    pub network: Network,
    /// VM's Nebula overlay IP.
    pub vm_nebula_ip: String,
    /// Host port.
    pub host_port: u16,
    /// Protocol.
    pub proto: String,
}

/// Published `compute/exposed/<peer>` payload.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExposedState {
    /// Owning peer (this peer's Nebula overlay IP).
    pub peer: String,
    /// Active forwarding rules in deterministic order.
    pub rules: Vec<ActiveRule>,
}

/// Terminal outcome for one authorized firewall action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FirewallActionOutcome {
    /// Every requested change became active, or the desired state already held.
    Applied,
    /// Some, but not all, effects succeeded or permanent changes could not reload.
    Partial,
    /// No requested change became active.
    Failed,
}

/// Bounded reply correlated to the exact Bus message that consumed the capability.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FirewallActionResult {
    /// Exact result schema.
    pub schema_version: u16,
    /// ULID of the consumed `compute/expose` or `compute/unexpose` message.
    pub request_ulid: String,
    /// Closed action name (`expose` or `unexpose`), never a raw command.
    pub action: String,
    /// Honest terminal classification.
    pub outcome: FirewallActionOutcome,
    /// Number of firewall rule changes requested after idempotence filtering.
    pub attempted: u16,
    /// Number of changes reflected in the active-rule projection.
    pub applied: u16,
    /// Number of requested changes not reflected in that projection.
    pub failed: u16,
    /// Bounded operator-safe explanation with no command or stderr content.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PreparedFirewallAction {
    action: String,
    planned: Vec<ActiveRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal: Option<FirewallActionResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FirewallActionJournal {
    schema_version: u16,
    entries: BTreeMap<String, PreparedFirewallAction>,
}

impl Default for FirewallActionJournal {
    fn default() -> Self {
        Self {
            schema_version: ACTION_JOURNAL_SCHEMA_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

/// Standard exact-request reply lane used by action workers.
#[must_use]
pub fn action_result_topic(request_ulid: &str) -> String {
    format!("reply/{request_ulid}")
}

/// Resolve the firewalld zone for a `Network`. `wan_zone` is the
/// auto-detected zone for the WAN network (operator-overridable in
/// the future via a config field).
#[must_use]
pub fn zone_for_network(network: Network, wan_zone: &str) -> String {
    match network {
        Network::Mesh => MESH_ZONE.to_string(),
        Network::Lan => LAN_ZONE.to_string(),
        Network::Wan => wan_zone.to_string(),
    }
}

/// Build a firewalld rich-rule body per design doc §7. `nebula_ip`
/// is the local peer's overlay IP (used as `destination address`
/// for the mesh rule; ignored for lan + wan).
#[must_use]
pub fn build_rich_rule_body(
    network: Network,
    nebula_ip: &str,
    vm_nebula_ip: &str,
    host_port: u16,
    proto: &str,
) -> String {
    match network {
        Network::Mesh => format!(
            r#"rule family="ipv4" destination address="{nebula_ip}" port port="{host_port}" protocol="{proto}" forward-port port="{host_port}" protocol="{proto}" to-addr="{vm_nebula_ip}" to-port="{host_port}""#,
        ),
        Network::Lan | Network::Wan => format!(
            r#"rule family="ipv4" port port="{host_port}" protocol="{proto}" forward-port port="{host_port}" protocol="{proto}" to-addr="{vm_nebula_ip}" to-port="{host_port}""#,
        ),
    }
}

/// Read one firewalld rich-rule attribute (`key="value"`), returning
/// the value. Used by [`parse_rich_rule`] to reverse a rule line.
fn rich_rule_attr(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Reverse one of our managed forward-port rich rules (built by
/// [`build_rich_rule_body`]) back into an [`ActiveRule`]. `network` is
/// supplied by the caller from the zone that was queried. Returns `None`
/// for rules that aren't ours — anything without a `forward-port` to a
/// VM-subnet (`10.42.*`) `to-addr` — so unrelated zone rules are skipped.
/// (VIRT-7.followup: seed the shadow set on startup.)
#[must_use]
pub fn parse_rich_rule(network: Network, line: &str) -> Option<ActiveRule> {
    let line = line.trim();
    if !line.contains("forward-port") {
        return None;
    }
    let vm_nebula_ip = rich_rule_attr(line, "to-addr")?;
    // Only our managed rules forward to a VM overlay IP (10.42.128.0/17,
    // all under the 10.42. mesh prefix).
    if !vm_nebula_ip.starts_with("10.42.") {
        return None;
    }
    let host_port = rich_rule_attr(line, "port")?.parse::<u16>().ok()?;
    let proto = rich_rule_attr(line, "protocol")?;
    Some(ActiveRule {
        network,
        vm_nebula_ip,
        host_port,
        proto,
    })
}

/// Parse an expose-request body. Bad JSON / unknown network
/// strings surface as descriptive errors so the caller can log +
/// drop the message.
///
/// # Errors
///
/// Returns a human-readable error string on parse failure or
/// unknown network name.
pub fn parse_expose_request(body: &str) -> Result<ExposeRequest, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed expose request: {e}"))?;
    let vm_nebula_ip = value
        .get("vm_nebula_ip")
        .and_then(|v| v.as_str())
        .ok_or("expose request missing `vm_nebula_ip`")?
        .to_string();
    let guest_port: u16 = value
        .get("guest_port")
        .and_then(|v| v.as_u64())
        .and_then(|n| u16::try_from(n).ok())
        .ok_or("expose request missing or out-of-range `guest_port`")?;
    let proto = value
        .get("proto")
        .and_then(|v| v.as_str())
        .ok_or("expose request missing `proto`")?
        .to_string();
    let networks_raw = value
        .get("networks")
        .and_then(|v| v.as_array())
        .ok_or("expose request missing `networks` array")?;
    let mut networks = Vec::with_capacity(networks_raw.len());
    for n in networks_raw {
        let s = n.as_str().ok_or("network entry not a string")?;
        let net = Network::parse(s).ok_or_else(|| format!("unknown network: {s}"))?;
        networks.push(net);
    }
    Ok(ExposeRequest {
        vm_nebula_ip,
        guest_port,
        proto,
        networks,
    })
}

/// Parse an unexpose-request body. See [`parse_expose_request`]
/// for the error semantics.
///
/// # Errors
///
/// Returns a human-readable error string on parse failure.
pub fn parse_unexpose_request(body: &str) -> Result<UnexposeRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed unexpose request: {e}"))
}

/// Stable semantic target for a firewall forward. The exact raw body is also
/// HMAC-bound by [`ActionAuthorizer`], while this target prevents a capability
/// for one VM endpoint from being used in another semantic context.
#[must_use]
fn expose_auth_target(req: &ExposeRequest) -> String {
    format!(
        "vm:{}:{}:{}",
        req.vm_nebula_ip.trim(),
        req.guest_port,
        req.proto.trim()
    )
}

/// Stable semantic target for removing a firewall forward.
#[must_use]
fn unexpose_auth_target(req: &UnexposeRequest) -> String {
    format!(
        "vm:{}:{}:{}",
        req.vm_nebula_ip.trim(),
        req.host_port,
        req.proto.trim()
    )
}

/// Verify an expose request before any firewalld command is invoked.
fn authorize_expose_request(
    authorizer: &ActionAuthorizer,
    body: &str,
    req: &ExposeRequest,
) -> Result<(), String> {
    let target = expose_auth_target(req);
    authorizer.authorize(
        body,
        MutationContext {
            verb: COMPUTE_EXPOSE_AUTH_VERB,
            node: COMPUTE_EXPOSE_NODE_SCOPE,
            target: &target,
        },
    )
}

/// Verify an unexpose request before any firewalld command is invoked.
fn authorize_unexpose_request(
    authorizer: &ActionAuthorizer,
    body: &str,
    req: &UnexposeRequest,
) -> Result<(), String> {
    let target = unexpose_auth_target(req);
    authorizer.authorize(
        body,
        MutationContext {
            verb: COMPUTE_UNEXPOSE_AUTH_VERB,
            node: COMPUTE_EXPOSE_NODE_SCOPE,
            target: &target,
        },
    )
}

/// Parse the default-gateway device name from a
/// `nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device` payload. The
/// returned device is the first non-loopback `connected` ethernet
/// or wifi interface — best-effort.
#[must_use]
pub fn parse_default_gateway_device(nmcli_stdout: &str) -> Option<String> {
    for line in nmcli_stdout.lines() {
        let cols: Vec<&str> = line.split(':').collect();
        if cols.len() < 3 {
            continue;
        }
        let device = cols[0];
        let typ = cols[1];
        let state = cols[2];
        if state != "connected" {
            continue;
        }
        if device == "lo" || device.starts_with("nebula") || device.starts_with("docker") {
            continue;
        }
        if !matches!(typ, "ethernet" | "wifi") {
            continue;
        }
        return Some(device.to_string());
    }
    None
}

/// Parse the firewalld zone for an interface from a
/// `firewall-cmd --get-zone-of-interface=<dev>` payload. The
/// command outputs the zone name on a single line.
#[must_use]
pub fn parse_zone_of_interface(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "no zone" {
        return None;
    }
    Some(trimmed.to_string())
}

/// Diff an `expose` request against the current active set.
/// Returns the `ActiveRule`s that would be NET-NEW (not already
/// active); idempotent re-expose reports zero new rules.
#[must_use]
pub fn diff_expose(active: &BTreeSet<ActiveRule>, req: &ExposeRequest) -> Vec<ActiveRule> {
    let mut out = Vec::new();
    for &net in &req.networks {
        let rule = ActiveRule {
            network: net,
            vm_nebula_ip: req.vm_nebula_ip.clone(),
            host_port: req.guest_port,
            proto: req.proto.clone(),
        };
        if !active.contains(&rule) {
            out.push(rule);
        }
    }
    out
}

/// Diff an `unexpose` request against the current active set.
/// Returns the `ActiveRule`s that would be removed. Idempotent
/// unexpose of an unknown rule reports zero removals.
#[must_use]
pub fn diff_unexpose(active: &BTreeSet<ActiveRule>, req: &UnexposeRequest) -> Vec<ActiveRule> {
    active
        .iter()
        .filter(|r| {
            r.vm_nebula_ip == req.vm_nebula_ip
                && r.host_port == req.host_port
                && r.proto == req.proto
        })
        .cloned()
        .collect()
}

fn binary_present(bin: &str) -> bool {
    let mut command = Command::new(bin);
    command.arg("--version");
    output_with_timeout(command, DEFAULT_CMD_TIMEOUT).is_ok()
}

trait ComputeExposeRuntime: Send + Sync {
    fn firewall_provider_available(&self) -> bool;

    fn resolve_bus_root(&self, override_root: Option<&Path>) -> Option<PathBuf>;

    fn open_bus(&self, root: PathBuf) -> Result<Persist, String>;
}

struct SystemComputeExposeRuntime;

impl ComputeExposeRuntime for SystemComputeExposeRuntime {
    fn firewall_provider_available(&self) -> bool {
        binary_present("firewall-cmd")
    }

    fn resolve_bus_root(&self, override_root: Option<&Path>) -> Option<PathBuf> {
        Some(compute_bus_root(
            override_root.map(Path::to_path_buf),
            default_bus_root(),
        ))
    }

    fn open_bus(&self, root: PathBuf) -> Result<Persist, String> {
        Persist::open(root).map_err(|error| error.to_string())
    }
}

fn compute_bus_root(override_root: Option<PathBuf>, default_root: Option<PathBuf>) -> PathBuf {
    override_root
        .or(default_root)
        .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn run_firewall_cmd(args: &[String]) -> bool {
    let mut command = Command::new("firewall-cmd");
    command.args(args);
    status_with_timeout(command, DEFAULT_CMD_TIMEOUT)
        .map(|status| status.success())
        .unwrap_or(false)
}

trait FirewallMutationRunner: Send + Sync {
    fn run(&self, args: &[String]) -> bool;
}

trait ActionResultWriter: Send + Sync {
    fn publish(&self, persist: &Persist, result: &FirewallActionResult) -> bool;
}

struct PersistActionResultWriter;

impl ActionResultWriter for PersistActionResultWriter {
    fn publish(&self, persist: &Persist, result: &FirewallActionResult) -> bool {
        write_action_result(persist, result)
    }
}

struct SystemFirewallMutationRunner;

impl FirewallMutationRunner for SystemFirewallMutationRunner {
    fn run(&self, args: &[String]) -> bool {
        run_firewall_cmd(args)
    }
}

/// Run `firewall-cmd <args>` and capture stdout (empty string on
/// failure). Used for read-only queries like `--list-rich-rules`.
fn firewall_cmd_stdout(args: &[String]) -> String {
    let mut command = Command::new("firewall-cmd");
    command.args(args);
    output_with_timeout(command, DEFAULT_CMD_TIMEOUT)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn detect_wan_zone() -> String {
    let mut nmcli = Command::new("nmcli");
    nmcli.args(["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]);
    let nmcli_out = output_with_timeout(nmcli, DEFAULT_CMD_TIMEOUT)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let Some(dev) = parse_default_gateway_device(&nmcli_out) else {
        return DEFAULT_WAN_ZONE.to_string();
    };
    let arg = format!("--get-zone-of-interface={dev}");
    let mut firewall = Command::new("firewall-cmd");
    firewall.arg(&arg);
    let zone_out = output_with_timeout(firewall, DEFAULT_CMD_TIMEOUT)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    parse_zone_of_interface(&zone_out).unwrap_or_else(|| DEFAULT_WAN_ZONE.to_string())
}

fn local_nebula_addr(interface: &str) -> String {
    let mut command = Command::new("ip");
    command.args(["-4", "addr", "show", interface]);
    let Some(output) = output_with_timeout(command, DEFAULT_CMD_TIMEOUT)
        .ok()
        .filter(|output| output.status.success())
    else {
        return String::new();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("inet ") {
            if let Some(ip) = rest.split('/').next() {
                return ip.to_string();
            }
        }
    }
    String::new()
}

/// Build the firewall-cmd args to ADD a rich rule.
#[must_use]
pub fn add_rich_rule_args(zone: &str, rule_body: &str) -> Vec<String> {
    vec![
        "--permanent".into(),
        format!("--zone={zone}"),
        format!("--add-rich-rule={rule_body}"),
    ]
}

/// Build the firewall-cmd args to REMOVE a rich rule.
#[must_use]
pub fn remove_rich_rule_args(zone: &str, rule_body: &str) -> Vec<String> {
    vec![
        "--permanent".into(),
        format!("--zone={zone}"),
        format!("--remove-rich-rule={rule_body}"),
    ]
}

/// Worker handle.
pub struct ComputeExposeWorker {
    nebula_interface: String,
    nebula_addr_hint: String,
    poll_interval: Duration,
    bus_root_override: Option<PathBuf>,
    runtime: Arc<dyn ComputeExposeRuntime>,
    authorizer: Arc<ActionAuthorizer>,
    mutation_runner: Arc<dyn FirewallMutationRunner>,
    result_writer: Arc<dyn ActionResultWriter>,
    journal_path: PathBuf,
    active: Mutex<BTreeSet<ActiveRule>>,
}

impl Default for ComputeExposeWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputeExposeWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nebula_interface: DEFAULT_NEBULA_INTERFACE.into(),
            nebula_addr_hint: String::new(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            runtime: Arc::new(SystemComputeExposeRuntime),
            authorizer: Arc::new(ActionAuthorizer::production()),
            mutation_runner: Arc::new(SystemFirewallMutationRunner),
            result_writer: Arc::new(PersistActionResultWriter),
            journal_path: PathBuf::from(DEFAULT_ACTION_JOURNAL_PATH),
            active: Mutex::new(BTreeSet::new()),
        }
    }

    /// Override the local peer's Nebula address (skips runtime
    /// detection via `ip addr`).
    #[must_use]
    pub fn with_nebula_addr_hint(mut self, addr: String) -> Self {
        self.nebula_addr_hint = addr;
        self
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Inject deterministic provider and Bus availability boundaries.
    #[cfg(test)]
    #[must_use]
    fn with_runtime(mut self, runtime: Arc<dyn ComputeExposeRuntime>) -> Self {
        self.runtime = runtime;
        self
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    /// Production always uses the root-only systemd-credential-backed
    /// authorizer and fails closed when it is unavailable.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Inject a deterministic mutation boundary for hostile failure tests.
    #[cfg(test)]
    #[must_use]
    fn with_mutation_runner(mut self, runner: Arc<dyn FirewallMutationRunner>) -> Self {
        self.mutation_runner = runner;
        self
    }

    /// Inject a deterministic result-publication boundary for retry tests.
    #[cfg(test)]
    #[must_use]
    fn with_result_writer(mut self, writer: Arc<dyn ActionResultWriter>) -> Self {
        self.result_writer = writer;
        self
    }

    /// Override the root-owned crash journal path for hostile restart tests.
    #[cfg(test)]
    #[must_use]
    fn with_journal_path(mut self, path: PathBuf) -> Self {
        self.journal_path = path;
        self
    }

    /// Override the poll cadence. Used in tests.
    #[must_use]
    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Snapshot the active-rule shadow set. Used in tests.
    #[must_use]
    pub fn active_snapshot(&self) -> Vec<ActiveRule> {
        self.active
            .lock()
            .expect("active mutex")
            .iter()
            .cloned()
            .collect()
    }
}

fn resolve_nebula_addr(worker: &ComputeExposeWorker) -> String {
    if !worker.nebula_addr_hint.is_empty() {
        return worker.nebula_addr_hint.clone();
    }
    local_nebula_addr(&worker.nebula_interface)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApplySummary {
    outcome: FirewallActionOutcome,
    attempted: u16,
    applied: u16,
    detail: &'static str,
}

impl ApplySummary {
    fn failed(self) -> u16 {
        self.attempted.saturating_sub(self.applied)
    }

    fn changed(self) -> bool {
        self.applied > 0
    }
}

fn classify_apply(attempted: usize, succeeded: usize, reload_succeeded: bool) -> ApplySummary {
    let attempted = u16::try_from(attempted).unwrap_or(u16::MAX);
    let succeeded = u16::try_from(succeeded).unwrap_or(u16::MAX);
    if attempted == 0 {
        return ApplySummary {
            outcome: FirewallActionOutcome::Applied,
            attempted,
            applied: 0,
            detail: "requested firewall state was already satisfied",
        };
    }
    if succeeded == 0 {
        return ApplySummary {
            outcome: FirewallActionOutcome::Failed,
            attempted,
            applied: 0,
            detail: "every requested firewall change failed",
        };
    }
    if !reload_succeeded {
        return ApplySummary {
            outcome: FirewallActionOutcome::Partial,
            attempted,
            applied: 0,
            detail: "permanent firewall changes succeeded but activation reload failed",
        };
    }
    if succeeded == attempted {
        ApplySummary {
            outcome: FirewallActionOutcome::Applied,
            attempted,
            applied: succeeded,
            detail: "all requested firewall changes are active",
        }
    } else {
        ApplySummary {
            outcome: FirewallActionOutcome::Partial,
            attempted,
            applied: succeeded,
            detail: "only part of the requested firewall changes became active",
        }
    }
}

fn apply_expose(
    worker: &ComputeExposeWorker,
    nebula_ip: &str,
    wan_zone: &str,
    req: &ExposeRequest,
) -> ApplySummary {
    let mut active = worker
        .active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let new_rules = diff_expose(&active, req);
    if new_rules.is_empty() {
        return classify_apply(0, 0, true);
    }
    let mut succeeded = Vec::new();
    for rule in &new_rules {
        let zone = zone_for_network(rule.network, wan_zone);
        let body = build_rich_rule_body(
            rule.network,
            nebula_ip,
            &rule.vm_nebula_ip,
            rule.host_port,
            &rule.proto,
        );
        let args = add_rich_rule_args(&zone, &body);
        if worker.mutation_runner.run(&args) {
            succeeded.push(rule.clone());
        } else {
            tracing::warn!(
                vm_ip = %rule.vm_nebula_ip,
                port = rule.host_port,
                network = rule.network.wire_name(),
                "compute_expose: firewall-cmd add-rich-rule failed"
            );
        }
    }
    let reload_succeeded =
        succeeded.is_empty() || worker.mutation_runner.run(&["--reload".to_string()]);
    let summary = classify_apply(new_rules.len(), succeeded.len(), reload_succeeded);
    if reload_succeeded {
        active.extend(succeeded);
    }
    summary
}

fn apply_unexpose(
    worker: &ComputeExposeWorker,
    nebula_ip: &str,
    wan_zone: &str,
    req: &UnexposeRequest,
) -> ApplySummary {
    let mut active = worker
        .active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let removals = diff_unexpose(&active, req);
    if removals.is_empty() {
        return classify_apply(0, 0, true);
    }
    let mut succeeded = Vec::new();
    for rule in &removals {
        let zone = zone_for_network(rule.network, wan_zone);
        let body = build_rich_rule_body(
            rule.network,
            nebula_ip,
            &rule.vm_nebula_ip,
            rule.host_port,
            &rule.proto,
        );
        let args = remove_rich_rule_args(&zone, &body);
        if worker.mutation_runner.run(&args) {
            succeeded.push(rule.clone());
        } else {
            tracing::warn!(
                vm_ip = %rule.vm_nebula_ip,
                port = rule.host_port,
                network = rule.network.wire_name(),
                "compute_expose: firewall-cmd remove-rich-rule failed"
            );
        }
    }
    let reload_succeeded =
        succeeded.is_empty() || worker.mutation_runner.run(&["--reload".to_string()]);
    let summary = classify_apply(removals.len(), succeeded.len(), reload_succeeded);
    if reload_succeeded {
        for rule in succeeded {
            active.remove(&rule);
        }
    }
    summary
}

fn publish_exposed_state(persist: &Persist, peer: &str, worker: &ComputeExposeWorker) {
    let rules: Vec<ActiveRule> = worker
        .active
        .lock()
        .expect("active mutex")
        .iter()
        .cloned()
        .collect();
    let state = ExposedState {
        peer: peer.to_string(),
        rules,
    };
    let Ok(body) = serde_json::to_string(&state) else {
        return;
    };
    let topic = format!("compute/exposed/{peer}");
    if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&body)) {
        tracing::warn!(error = %e, topic, "compute_expose: publish failed");
    }
}

fn build_action_result(
    request_ulid: &str,
    action: &str,
    summary: ApplySummary,
) -> FirewallActionResult {
    FirewallActionResult {
        schema_version: 1,
        request_ulid: request_ulid.to_string(),
        action: action.to_string(),
        outcome: summary.outcome,
        attempted: summary.attempted,
        applied: summary.applied,
        failed: summary.failed(),
        detail: summary.detail.to_string(),
    }
}

fn write_action_result(persist: &Persist, result: &FirewallActionResult) -> bool {
    let Ok(body) = serde_json::to_string(&result) else {
        return false;
    };
    let topic = action_result_topic(&result.request_ulid);
    if let Err(error) = persist.write(&topic, Priority::Default, None, Some(&body)) {
        tracing::warn!(
            ulid = %result.request_ulid,
            error = %error,
            "compute_expose: exact action result publish failed"
        );
        return false;
    }
    true
}

fn load_action_journal(path: &Path) -> Result<FirewallActionJournal, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FirewallActionJournal::default());
        }
        Err(error) => return Err(format!("action journal metadata unavailable: {error}")),
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err("action journal is not a regular file".to_string());
    }
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("action journal ownership or mode is unsafe".to_string());
    }
    if metadata.len() > ACTION_JOURNAL_MAX_BYTES {
        return Err("action journal exceeds its size bound".to_string());
    }
    let bytes =
        std::fs::read(path).map_err(|error| format!("action journal read failed: {error}"))?;
    let journal: FirewallActionJournal = serde_json::from_slice(&bytes)
        .map_err(|error| format!("action journal decode failed: {error}"))?;
    if journal.schema_version != ACTION_JOURNAL_SCHEMA_VERSION
        || journal.entries.len() > ACTION_JOURNAL_MAX_ENTRIES
        || journal.entries.iter().any(|(ulid, entry)| {
            !matches!(entry.action.as_str(), "expose" | "unexpose")
                || entry.planned.len() > 3
                || entry.terminal.as_ref().is_some_and(|result| {
                    &result.request_ulid != ulid
                        || result.action != entry.action
                        || result.attempted > 3
                        || result.applied > result.attempted
                        || result.failed != result.attempted.saturating_sub(result.applied)
                })
        })
    {
        return Err("action journal invariants failed".to_string());
    }
    Ok(journal)
}

fn store_action_journal(path: &Path, mut journal: FirewallActionJournal) -> Result<(), String> {
    while journal.entries.len() > ACTION_JOURNAL_MAX_ENTRIES {
        let Some(oldest) = journal.entries.keys().next().cloned() else {
            break;
        };
        journal.entries.remove(&oldest);
    }
    let body = serde_json::to_vec(&journal)
        .map_err(|error| format!("action journal encode failed: {error}"))?;
    if u64::try_from(body.len()).unwrap_or(u64::MAX) > ACTION_JOURNAL_MAX_BYTES {
        return Err("action journal encoding exceeds its size bound".to_string());
    }
    crate::ca::seal::write_atomic_sealed(path, &body)
        .map_err(|error| format!("action journal atomic write failed: {error}"))
}

fn prepare_action(
    worker: &ComputeExposeWorker,
    request_ulid: &str,
    action: &str,
    planned: Vec<ActiveRule>,
) -> Result<(), String> {
    let mut journal = load_action_journal(&worker.journal_path)?;
    if journal.entries.contains_key(request_ulid) {
        return Ok(());
    }
    journal.entries.insert(
        request_ulid.to_string(),
        PreparedFirewallAction {
            action: action.to_string(),
            planned,
            terminal: None,
        },
    );
    store_action_journal(&worker.journal_path, journal)
}

fn terminalize_action(
    worker: &ComputeExposeWorker,
    result: FirewallActionResult,
) -> Result<(), String> {
    let mut journal = load_action_journal(&worker.journal_path)?;
    let entry = journal
        .entries
        .get_mut(&result.request_ulid)
        .ok_or_else(|| "prepared action is missing from the journal".to_string())?;
    if entry.action != result.action {
        return Err("prepared action kind changed".to_string());
    }
    entry.terminal = Some(result);
    store_action_journal(&worker.journal_path, journal)
}

fn recovered_summary(worker: &ComputeExposeWorker, entry: &PreparedFirewallAction) -> ApplySummary {
    let active = worker
        .active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let applied = entry
        .planned
        .iter()
        .filter(|rule| {
            if entry.action == "expose" {
                active.contains(rule)
            } else {
                !active.contains(rule)
            }
        })
        .count();
    let attempted = entry.planned.len();
    if attempted == 0 {
        classify_apply(0, 0, true)
    } else if applied == 0 {
        ApplySummary {
            outcome: FirewallActionOutcome::Failed,
            attempted: u16::try_from(attempted).unwrap_or(u16::MAX),
            applied: 0,
            detail: "prepared firewall action had no active effect after restart",
        }
    } else if applied == attempted {
        ApplySummary {
            outcome: FirewallActionOutcome::Applied,
            attempted: u16::try_from(attempted).unwrap_or(u16::MAX),
            applied: u16::try_from(applied).unwrap_or(u16::MAX),
            detail: "prepared firewall action was fully recovered from active state",
        }
    } else {
        ApplySummary {
            outcome: FirewallActionOutcome::Partial,
            attempted: u16::try_from(attempted).unwrap_or(u16::MAX),
            applied: u16::try_from(applied).unwrap_or(u16::MAX),
            detail: "prepared firewall action was partially recovered from active state",
        }
    }
}

fn publish_journaled_result(
    persist: &Persist,
    worker: &ComputeExposeWorker,
    request_ulid: &str,
) -> Result<bool, String> {
    let journal = load_action_journal(&worker.journal_path)?;
    let Some(result) = journal
        .entries
        .get(request_ulid)
        .and_then(|entry| entry.terminal.as_ref())
    else {
        return Ok(false);
    };
    if persist
        .read_latest(&action_result_topic(request_ulid))
        .ok()
        .flatten()
        .and_then(|message| message.body)
        .and_then(|body| serde_json::from_str::<FirewallActionResult>(&body).ok())
        .as_ref()
        == Some(result)
    {
        return Ok(true);
    }
    Ok(worker.result_writer.publish(persist, result))
}

fn recover_or_publish_journaled(
    persist: &Persist,
    worker: &ComputeExposeWorker,
    request_ulid: &str,
) -> Result<Option<bool>, String> {
    let journal = load_action_journal(&worker.journal_path)?;
    let Some(entry) = journal.entries.get(request_ulid).cloned() else {
        return Ok(None);
    };
    if entry.terminal.is_none() {
        let summary = recovered_summary(worker, &entry);
        terminalize_action(
            worker,
            build_action_result(request_ulid, &entry.action, summary),
        )?;
    }
    publish_journaled_result(persist, worker, request_ulid).map(Some)
}

fn poll_once(
    persist: &Persist,
    bus_root: &Path,
    bus_identity: ComputeExposeBusIdentity,
    worker: &ComputeExposeWorker,
    nebula_ip: &str,
    wan_zone: &str,
    expose_cursor: &mut Option<String>,
    unexpose_cursor: &mut Option<String>,
    authorizer: &ActionAuthorizer,
) -> Result<(), String> {
    let expose_topic = format!("compute/expose/{nebula_ip}");
    let unexpose_topic = format!("compute/unexpose/{nebula_ip}");

    // Acquire a complete, bounded snapshot of both command lanes before any
    // authorization claim or firewall effect. This prevents a replacement
    // between lane reads from creating a mixed-generation transaction.
    let expose_messages = persist
        .list_since(&expose_topic, expose_cursor.as_deref())
        .map_err(|error| format!("expose lane read failed: {error}"))?;
    let unexpose_messages = persist
        .list_since(&unexpose_topic, unexpose_cursor.as_deref())
        .map_err(|error| format!("unexpose lane read failed: {error}"))?;
    for (lane, messages) in [
        ("expose", expose_messages.as_slice()),
        ("unexpose", unexpose_messages.as_slice()),
    ] {
        let body_bytes = messages.iter().try_fold(0_usize, |total, message| {
            total.checked_add(message.body.as_deref().map_or(0, str::len))
        });
        if messages.len() > BUS_LANE_MAX_MESSAGES
            || body_bytes.is_none_or(|bytes| bytes > BUS_LANE_MAX_BODY_BYTES)
        {
            return Err(format!("{lane} lane exceeds its complete-read bound"));
        }
    }
    verify_compute_expose_bus(persist, bus_root, bus_identity)?;

    let mut changed = false;
    {
        for msg in expose_messages {
            verify_compute_expose_bus(persist, bus_root, bus_identity)?;
            match recover_or_publish_journaled(persist, worker, &msg.ulid) {
                Ok(Some(true)) => {
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    *expose_cursor = Some(msg.ulid.clone());
                    continue;
                }
                Ok(Some(false)) => break,
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(ulid = %msg.ulid, error = %error, "compute_expose: action journal recovery failed");
                    break;
                }
            }
            let body = msg.body.as_deref().unwrap_or("");
            match parse_expose_request(body) {
                Ok(req) => {
                    let target = expose_auth_target(&req);
                    if let Err(error) = authorizer.verify_exact_body(
                        body,
                        MutationContext {
                            verb: COMPUTE_EXPOSE_AUTH_VERB,
                            node: COMPUTE_EXPOSE_NODE_SCOPE,
                            target: &target,
                        },
                    ) {
                        tracing::warn!(ulid = %msg.ulid, error = %error, "compute_expose: refused unauthorized expose request");
                        *expose_cursor = Some(msg.ulid.clone());
                        continue;
                    }
                    let planned = {
                        let active = worker
                            .active
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        diff_expose(&active, &req)
                    };
                    if let Err(error) = prepare_action(worker, &msg.ulid, "expose", planned.clone())
                    {
                        tracing::warn!(ulid = %msg.ulid, error = %error, "compute_expose: could not durably prepare expose request");
                        break;
                    }
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    if let Err(error) = authorize_expose_request(authorizer, body, &req) {
                        tracing::warn!(
                            ulid = %msg.ulid,
                            error = %error,
                            "compute_expose: refused unauthorized expose request"
                        );
                        let summary = ApplySummary {
                            outcome: FirewallActionOutcome::Failed,
                            attempted: u16::try_from(planned.len()).unwrap_or(u16::MAX),
                            applied: 0,
                            detail: "capability claim failed before firewall mutation",
                        };
                        if terminalize_action(
                            worker,
                            build_action_result(&msg.ulid, "expose", summary),
                        )
                        .is_err()
                            || !publish_journaled_result(persist, worker, &msg.ulid)
                                .unwrap_or(false)
                        {
                            break;
                        }
                        verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                        *expose_cursor = Some(msg.ulid.clone());
                        continue;
                    }
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    let summary = apply_expose(worker, nebula_ip, wan_zone, &req);
                    changed |= summary.changed();
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    let result = build_action_result(&msg.ulid, "expose", summary);
                    if let Err(error) = terminalize_action(worker, result) {
                        tracing::warn!(ulid = %msg.ulid, error = %error, "compute_expose: could not durably terminalize expose request");
                        break;
                    }
                    if !publish_journaled_result(persist, worker, &msg.ulid).unwrap_or(false) {
                        break;
                    }
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    *expose_cursor = Some(msg.ulid.clone());
                }
                Err(e) => {
                    tracing::warn!(ulid = %msg.ulid, error = %e, "compute_expose: bad expose request");
                    *expose_cursor = Some(msg.ulid.clone());
                }
            }
        }
    }

    {
        for msg in unexpose_messages {
            verify_compute_expose_bus(persist, bus_root, bus_identity)?;
            match recover_or_publish_journaled(persist, worker, &msg.ulid) {
                Ok(Some(true)) => {
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    *unexpose_cursor = Some(msg.ulid.clone());
                    continue;
                }
                Ok(Some(false)) => break,
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(ulid = %msg.ulid, error = %error, "compute_expose: action journal recovery failed");
                    break;
                }
            }
            let body = msg.body.as_deref().unwrap_or("");
            match parse_unexpose_request(body) {
                Ok(req) => {
                    let target = unexpose_auth_target(&req);
                    if let Err(error) = authorizer.verify_exact_body(
                        body,
                        MutationContext {
                            verb: COMPUTE_UNEXPOSE_AUTH_VERB,
                            node: COMPUTE_EXPOSE_NODE_SCOPE,
                            target: &target,
                        },
                    ) {
                        tracing::warn!(ulid = %msg.ulid, error = %error, "compute_expose: refused unauthorized unexpose request");
                        *unexpose_cursor = Some(msg.ulid.clone());
                        continue;
                    }
                    let planned = {
                        let active = worker
                            .active
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        diff_unexpose(&active, &req)
                    };
                    if let Err(error) =
                        prepare_action(worker, &msg.ulid, "unexpose", planned.clone())
                    {
                        tracing::warn!(ulid = %msg.ulid, error = %error, "compute_expose: could not durably prepare unexpose request");
                        break;
                    }
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    if let Err(error) = authorize_unexpose_request(authorizer, body, &req) {
                        tracing::warn!(
                            ulid = %msg.ulid,
                            error = %error,
                            "compute_expose: refused unauthorized unexpose request"
                        );
                        let summary = ApplySummary {
                            outcome: FirewallActionOutcome::Failed,
                            attempted: u16::try_from(planned.len()).unwrap_or(u16::MAX),
                            applied: 0,
                            detail: "capability claim failed before firewall mutation",
                        };
                        if terminalize_action(
                            worker,
                            build_action_result(&msg.ulid, "unexpose", summary),
                        )
                        .is_err()
                            || !publish_journaled_result(persist, worker, &msg.ulid)
                                .unwrap_or(false)
                        {
                            break;
                        }
                        verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                        *unexpose_cursor = Some(msg.ulid.clone());
                        continue;
                    }
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    let summary = apply_unexpose(worker, nebula_ip, wan_zone, &req);
                    changed |= summary.changed();
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    let result = build_action_result(&msg.ulid, "unexpose", summary);
                    if let Err(error) = terminalize_action(worker, result) {
                        tracing::warn!(ulid = %msg.ulid, error = %error, "compute_expose: could not durably terminalize unexpose request");
                        break;
                    }
                    if !publish_journaled_result(persist, worker, &msg.ulid).unwrap_or(false) {
                        break;
                    }
                    verify_compute_expose_bus(persist, bus_root, bus_identity)?;
                    *unexpose_cursor = Some(msg.ulid.clone());
                }
                Err(e) => {
                    tracing::warn!(ulid = %msg.ulid, error = %e, "compute_expose: bad unexpose request");
                    *unexpose_cursor = Some(msg.ulid.clone());
                }
            }
        }
    }

    if changed {
        verify_compute_expose_bus(persist, bus_root, bus_identity)?;
        publish_exposed_state(persist, nebula_ip, worker);
        verify_compute_expose_bus(persist, bus_root, bus_identity)?;
    }
    Ok(())
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

fn compute_expose_bus_identity(root: &Path) -> Result<ComputeExposeBusIdentity, String> {
    let metadata = std::fs::metadata(root.join("index.sqlite"))
        .map_err(|error| format!("Bus index identity unavailable: {error}"))?;
    if !metadata.is_file() {
        return Err("Bus index is not a regular file".to_string());
    }
    Ok(ComputeExposeBusIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn open_current_compute_expose_bus(
    runtime: &dyn ComputeExposeRuntime,
    root: PathBuf,
) -> Result<(Persist, ComputeExposeBusIdentity), String> {
    let before = compute_expose_bus_identity(&root).ok();
    let persist = runtime.open_bus(root.clone())?;
    let after = compute_expose_bus_identity(&root)?;
    if before.is_some_and(|identity| identity != after)
        || persist.index_inode() != Some(after.inode)
    {
        return Err("Bus changed while opening an identity-bound connection".to_string());
    }
    Ok((persist, after))
}

fn verify_compute_expose_bus(
    persist: &Persist,
    root: &Path,
    expected: ComputeExposeBusIdentity,
) -> Result<(), String> {
    let current = compute_expose_bus_identity(root)?;
    if current != expected || persist.index_inode() != Some(expected.inode) {
        return Err("Bus generation changed during compute exposure transaction".to_string());
    }
    Ok(())
}

/// Seed the active-rule shadow set from firewalld's persisted rich rules
/// at startup (VIRT-7.followup). After a mackesd restart the in-memory
/// `active` set would otherwise be empty until the next expose/unexpose,
/// leaving the first `compute/exposed/<peer>` publish stale; this
/// reconstructs it from the `--permanent` rules firewalld actually holds.
/// Only our managed forward-port rules are picked up.
///
/// Each distinct zone is queried once; when two networks resolve to the
/// same zone (e.g. a WAN interface that also sits in `public`), the zone
/// is attributed to the more-exposed network (Wan before Lan) so the
/// display never under-reports reach.
fn seed_active_from_firewalld(worker: &ComputeExposeWorker, wan_zone: &str) {
    let mut active = worker
        .active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut seen_zones: BTreeSet<String> = BTreeSet::new();
    for network in [Network::Mesh, Network::Wan, Network::Lan] {
        let zone = zone_for_network(network, wan_zone);
        if !seen_zones.insert(zone.clone()) {
            continue;
        }
        let stdout =
            firewall_cmd_stdout(&["--list-rich-rules".to_string(), format!("--zone={zone}")]);
        for line in stdout.lines() {
            if let Some(rule) = parse_rich_rule(network, line) {
                active.insert(rule);
            }
        }
    }
    tracing::info!(
        target: "mackesd::compute_expose",
        count = active.len(),
        "seeded active-rule shadow set from firewalld --permanent rules",
    );
}

#[async_trait::async_trait]
impl Worker for ComputeExposeWorker {
    fn name(&self) -> &'static str {
        "compute_expose"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        if !self.runtime.firewall_provider_available() {
            tracing::debug!("compute_expose: firewall-cmd absent; worker quiescent");
            shutdown.wait().await;
            return Ok(());
        }

        let wan_zone = detect_wan_zone();
        // VIRT-7.followup: seed the shadow set from firewalld's persisted
        // (--permanent) rules so the first compute/exposed publish reflects
        // reality after a restart instead of an empty set.
        seed_active_from_firewalld(self, &wan_zone);
        let mut expose_cursor: Option<String> = None;
        let mut unexpose_cursor: Option<String> = None;
        let mut active_identity: Option<ComputeExposeBusIdentity> = None;
        let mut tick = tokio::time::interval(self.poll_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let nebula_ip = resolve_nebula_addr(self);
                    if nebula_ip.is_empty() {
                        // Nebula not yet up — skip this tick.
                        continue;
                    }
                    let Some(bus_root) = self.runtime.resolve_bus_root(
                        self.bus_root_override.as_deref(),
                    ) else {
                        tracing::debug!("compute_expose: no bus root; retrying");
                        continue;
                    };
                    let (persist, identity) = match open_current_compute_expose_bus(
                        self.runtime.as_ref(),
                        bus_root.clone(),
                    ) {
                        Ok(opened) => opened,
                        Err(error) => {
                            tracing::debug!(error = %error, "compute_expose: identity-bound Bus open failed; retrying");
                            continue;
                        }
                    };
                    if active_identity.is_some_and(|active| active != identity) {
                        // A replacement may retain old rows. Floor both lanes at
                        // the replacement's current tail so only corrected-forward
                        // requests can trigger a new external effect.
                        expose_cursor = persist
                            .read_latest(&format!("compute/expose/{nebula_ip}"))
                            .ok()
                            .flatten()
                            .map(|message| message.ulid);
                        unexpose_cursor = persist
                            .read_latest(&format!("compute/unexpose/{nebula_ip}"))
                            .ok()
                            .flatten()
                            .map(|message| message.ulid);
                        if let Err(error) = verify_compute_expose_bus(&persist, &bus_root, identity) {
                            tracing::debug!(error = %error, "compute_expose: replacement changed during activation");
                            continue;
                        }
                    }
                    active_identity = Some(identity);
                    if let Err(error) = poll_once(
                        &persist,
                        &bus_root,
                        identity,
                        self,
                        &nebula_ip,
                        &wan_zone,
                        &mut expose_cursor,
                        &mut unexpose_cursor,
                        self.authorizer.as_ref(),
                    ) {
                        tracing::debug!(error = %error, "compute_expose: transaction deferred for a fresh Bus sweep");
                    }
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
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const AUTH_KEY: &[u8] = b"compute-expose-action-auth-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    struct AlwaysFailFirewall;

    impl FirewallMutationRunner for AlwaysFailFirewall {
        fn run(&self, _args: &[String]) -> bool {
            false
        }
    }

    struct ReloadFailFirewall;

    impl FirewallMutationRunner for ReloadFailFirewall {
        fn run(&self, args: &[String]) -> bool {
            args.len() != 1 || args[0] != "--reload"
        }
    }

    struct PanicFirewall;

    impl FirewallMutationRunner for PanicFirewall {
        fn run(&self, _args: &[String]) -> bool {
            panic!("recovered action must not rerun a consumed mutation")
        }
    }

    #[derive(Default)]
    struct RecordingFirewall {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl FirewallMutationRunner for RecordingFirewall {
        fn run(&self, args: &[String]) -> bool {
            self.calls.lock().expect("calls").push(args.to_vec());
            true
        }
    }

    struct FailFirstResultWriter {
        attempts: AtomicUsize,
    }

    impl ActionResultWriter for FailFirstResultWriter {
        fn publish(&self, persist: &Persist, result: &FirewallActionResult) -> bool {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                return false;
            }
            write_action_result(persist, result)
        }
    }

    struct ScriptedRuntime {
        firewall_available: bool,
        resolved_root: Option<PathBuf>,
        unresolved_attempts: AtomicUsize,
        open_failures: AtomicUsize,
        resolve_calls: AtomicUsize,
        open_calls: AtomicUsize,
    }

    impl ScriptedRuntime {
        fn consume(counter: &AtomicUsize) -> bool {
            counter
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
        }
    }

    impl ComputeExposeRuntime for ScriptedRuntime {
        fn firewall_provider_available(&self) -> bool {
            self.firewall_available
        }

        fn resolve_bus_root(&self, override_root: Option<&Path>) -> Option<PathBuf> {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            if Self::consume(&self.unresolved_attempts) {
                return None;
            }
            override_root
                .map(Path::to_path_buf)
                .or_else(|| self.resolved_root.clone())
        }

        fn open_bus(&self, root: PathBuf) -> Result<Persist, String> {
            self.open_calls.fetch_add(1, Ordering::SeqCst);
            if Self::consume(&self.open_failures) {
                return Err("injected transient open failure".to_string());
            }
            Persist::open(root).map_err(|error| error.to_string())
        }
    }

    #[test]
    fn default_bus_root_uses_the_shared_mde_bus_resolver() {
        assert_eq!(default_bus_root(), mde_bus::default_data_dir());
        assert_eq!(
            compute_bus_root(None, None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            compute_bus_root(Some(PathBuf::from("/tmp/compute-bus")), None),
            PathBuf::from("/tmp/compute-bus")
        );
    }

    #[tokio::test]
    async fn missing_firewall_provider_quiesces_until_prompt_shutdown() {
        let temp = tempfile::tempdir().expect("temp root");
        let unconfigured_root = temp.path().join("must-not-materialize");
        let runtime = Arc::new(ScriptedRuntime {
            firewall_available: false,
            resolved_root: Some(unconfigured_root.clone()),
            unresolved_attempts: AtomicUsize::new(0),
            open_failures: AtomicUsize::new(0),
            resolve_calls: AtomicUsize::new(0),
            open_calls: AtomicUsize::new(0),
        });
        let runtime_seam: Arc<dyn ComputeExposeRuntime> = runtime.clone();
        let mut worker = ComputeExposeWorker::new()
            .with_runtime(runtime_seam)
            .with_poll_interval(Duration::from_millis(10));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::sleep(Duration::from_millis(35)).await;
        assert!(!task.is_finished(), "static provider absence must quiesce");
        assert_eq!(runtime.resolve_calls.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.open_calls.load(Ordering::SeqCst), 0);
        assert!(!unconfigured_root.exists());

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_millis(250), task)
            .await
            .expect("shutdown must be prompt")
            .expect("worker task")
            .expect("worker result");
    }

    #[tokio::test]
    async fn unresolved_bus_root_retries_without_early_exit_and_stops_promptly() {
        let runtime = Arc::new(ScriptedRuntime {
            firewall_available: true,
            resolved_root: None,
            unresolved_attempts: AtomicUsize::new(usize::MAX),
            open_failures: AtomicUsize::new(0),
            resolve_calls: AtomicUsize::new(0),
            open_calls: AtomicUsize::new(0),
        });
        let runtime_seam: Arc<dyn ComputeExposeRuntime> = runtime.clone();
        let mut worker = ComputeExposeWorker::new()
            .with_runtime(runtime_seam)
            .with_nebula_addr_hint("10.42.0.15".to_string())
            .with_poll_interval(Duration::from_millis(10));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if runtime.resolve_calls.load(Ordering::SeqCst) >= 2 {
                    break;
                }
                assert!(
                    !task.is_finished(),
                    "missing Bus root must remain retryable"
                );
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("worker must retry unresolved Bus discovery");
        assert!(
            !task.is_finished(),
            "missing Bus root must remain retryable"
        );
        assert!(runtime.resolve_calls.load(Ordering::SeqCst) >= 2);
        assert_eq!(runtime.open_calls.load(Ordering::SeqCst), 0);

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_millis(250), task)
            .await
            .expect("shutdown must interrupt retry sleep")
            .expect("worker task")
            .expect("worker result");
    }

    #[tokio::test]
    async fn transient_bus_resolution_and_open_failure_recovers_forward_without_restart() {
        let bus_root = tempfile::tempdir().expect("bus root");
        let persist = Persist::open(bus_root.path().to_path_buf()).expect("setup persist");
        let unsigned = r#"{"vm_nebula_ip":"10.42.128.1","guest_port":8080,"proto":"tcp","networks":["mesh"],"schema_version":1}"#;
        let request = parse_expose_request(unsigned).expect("valid expose request");
        let armed = authorize_test_body(
            AUTH_KEY,
            unsigned,
            MutationContext {
                verb: COMPUTE_EXPOSE_AUTH_VERB,
                node: COMPUTE_EXPOSE_NODE_SCOPE,
                target: &expose_auth_target(&request),
            },
            "compute-expose-bus-recovery-once",
            AUTH_NOW + 30_000,
        );
        let request_message = persist
            .write(
                "compute/expose/10.42.0.15",
                Priority::Default,
                None,
                Some(&armed),
            )
            .expect("request write");
        drop(persist);

        let auth_root = tempfile::tempdir().expect("auth root");
        let journal_root = tempfile::tempdir().expect("journal root");
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            auth_root.path().to_path_buf(),
            AUTH_NOW,
        ));
        let firewall = Arc::new(RecordingFirewall::default());
        let runtime = Arc::new(ScriptedRuntime {
            firewall_available: true,
            resolved_root: Some(bus_root.path().to_path_buf()),
            unresolved_attempts: AtomicUsize::new(1),
            open_failures: AtomicUsize::new(1),
            resolve_calls: AtomicUsize::new(0),
            open_calls: AtomicUsize::new(0),
        });
        let runtime_seam: Arc<dyn ComputeExposeRuntime> = runtime.clone();
        let firewall_seam: Arc<dyn FirewallMutationRunner> = firewall.clone();
        let mut worker = ComputeExposeWorker::new()
            .with_runtime(runtime_seam)
            .with_authorizer(authorizer)
            .with_mutation_runner(firewall_seam)
            .with_journal_path(journal_root.path().join("action-journal.json"))
            .with_nebula_addr_hint("10.42.0.15".to_string())
            .with_poll_interval(Duration::from_millis(10));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let persist = Persist::open(bus_root.path().to_path_buf()).expect("poll persist");
                if !persist
                    .list_since(&action_result_topic(&request_message.ulid), None)
                    .expect("result query")
                    .is_empty()
                {
                    break;
                }
                drop(persist);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker must activate after Bus recovery");

        assert!(runtime.resolve_calls.load(Ordering::SeqCst) >= 3);
        assert!(runtime.open_calls.load(Ordering::SeqCst) >= 2);
        {
            let calls = firewall.calls.lock().expect("calls");
            assert_eq!(calls.len(), 2, "one add plus one reload expected");
            assert!(calls[0]
                .iter()
                .any(|arg| arg.starts_with("--add-rich-rule=")));
            assert_eq!(calls[1], vec!["--reload"]);
        }

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_millis(250), task)
            .await
            .expect("active worker shutdown must be prompt")
            .expect("worker task")
            .expect("worker result");
    }

    #[tokio::test]
    async fn same_path_bus_replacement_skips_retained_request_and_runs_forward_once() {
        let parent = tempfile::tempdir().expect("parent");
        let bus_path = parent.path().join("bus");
        let retired_path = parent.path().join("retired-bus");
        let initial = Persist::open(bus_path.clone()).expect("initial Bus");
        let initial_unsigned = r#"{"vm_nebula_ip":"10.42.128.1","guest_port":8080,"proto":"tcp","networks":["mesh"],"schema_version":1}"#;
        let initial_request = parse_expose_request(initial_unsigned).expect("initial request");
        let initial_armed = authorize_test_body(
            AUTH_KEY,
            initial_unsigned,
            MutationContext {
                verb: COMPUTE_EXPOSE_AUTH_VERB,
                node: COMPUTE_EXPOSE_NODE_SCOPE,
                target: &expose_auth_target(&initial_request),
            },
            "compute-expose-initial-generation",
            AUTH_NOW + 30_000,
        );
        let initial_message = initial
            .write(
                "compute/expose/10.42.0.15",
                Priority::Default,
                None,
                Some(&initial_armed),
            )
            .expect("initial write");

        let auth_root = tempfile::tempdir().expect("auth root");
        let journal_root = tempfile::tempdir().expect("journal root");
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            auth_root.path().to_path_buf(),
            AUTH_NOW,
        ));
        let firewall = Arc::new(RecordingFirewall::default());
        let runtime = Arc::new(ScriptedRuntime {
            firewall_available: true,
            resolved_root: Some(bus_path.clone()),
            unresolved_attempts: AtomicUsize::new(0),
            open_failures: AtomicUsize::new(0),
            resolve_calls: AtomicUsize::new(0),
            open_calls: AtomicUsize::new(0),
        });
        let mut worker = ComputeExposeWorker::new()
            .with_runtime(runtime.clone())
            .with_authorizer(authorizer)
            .with_mutation_runner(firewall.clone())
            .with_journal_path(journal_root.path().join("action-journal.json"))
            .with_nebula_addr_hint("10.42.0.15".to_string())
            .with_poll_interval(Duration::from_millis(10));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if initial
                    .read_latest(&action_result_topic(&initial_message.ulid))
                    .expect("initial result query")
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("initial generation must drain");
        drop(initial);

        std::fs::rename(&bus_path, &retired_path).expect("retire Bus at same path");
        let replacement = Persist::open(bus_path.clone()).expect("replacement Bus");
        let retained_unsigned = r#"{"vm_nebula_ip":"10.42.128.2","guest_port":8081,"proto":"tcp","networks":["mesh"],"schema_version":1}"#;
        let retained_request = parse_expose_request(retained_unsigned).expect("retained request");
        let retained_armed = authorize_test_body(
            AUTH_KEY,
            retained_unsigned,
            MutationContext {
                verb: COMPUTE_EXPOSE_AUTH_VERB,
                node: COMPUTE_EXPOSE_NODE_SCOPE,
                target: &expose_auth_target(&retained_request),
            },
            "compute-expose-retained-replacement",
            AUTH_NOW + 30_000,
        );
        let retained_message = replacement
            .write(
                "compute/expose/10.42.0.15",
                Priority::Default,
                None,
                Some(&retained_armed),
            )
            .expect("retained replacement write");
        let open_floor = runtime.open_calls.load(Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.open_calls.load(Ordering::SeqCst) <= open_floor + 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("replacement must activate");

        let forward_unsigned = r#"{"vm_nebula_ip":"10.42.128.3","guest_port":8082,"proto":"tcp","networks":["mesh"],"schema_version":1}"#;
        let forward_request = parse_expose_request(forward_unsigned).expect("forward request");
        let forward_armed = authorize_test_body(
            AUTH_KEY,
            forward_unsigned,
            MutationContext {
                verb: COMPUTE_EXPOSE_AUTH_VERB,
                node: COMPUTE_EXPOSE_NODE_SCOPE,
                target: &expose_auth_target(&forward_request),
            },
            "compute-expose-forward-replacement",
            AUTH_NOW + 30_000,
        );
        let forward_message = replacement
            .write(
                "compute/expose/10.42.0.15",
                Priority::Default,
                None,
                Some(&forward_armed),
            )
            .expect("forward write");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if replacement
                    .read_latest(&action_result_topic(&forward_message.ulid))
                    .expect("forward result query")
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("forward request must drain");

        assert!(replacement
            .read_latest(&action_result_topic(&retained_message.ulid))
            .expect("retained result query")
            .is_none());
        {
            let calls = firewall.calls.lock().expect("calls");
            assert_eq!(calls.len(), 4, "exactly two add/reload transactions");
            assert!(calls.iter().all(|call| !call.iter().any(|arg| {
                arg.starts_with("--add-rich-rule=") && arg.contains("10.42.128.2")
            })));
        }

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_millis(250), task)
            .await
            .expect("shutdown must be prompt")
            .expect("worker task")
            .expect("worker result");
    }

    #[test]
    fn expose_rejects_unsigned_tampered_and_replayed_bodies() {
        let unsigned = r#"{"vm_nebula_ip":"10.42.128.1","guest_port":8080,"proto":"tcp","networks":["mesh","lan"],"schema_version":1}"#;
        let request = parse_expose_request(unsigned).expect("valid expose request");
        let target = expose_auth_target(&request);
        let armed = authorize_test_body(
            AUTH_KEY,
            unsigned,
            MutationContext {
                verb: COMPUTE_EXPOSE_AUTH_VERB,
                node: COMPUTE_EXPOSE_NODE_SCOPE,
                target: &target,
            },
            "compute-expose-once",
            AUTH_NOW + 30_000,
        );
        let tampered = armed.replace("10.42.128.1", "10.42.128.2");
        let tampered_request = parse_expose_request(&tampered).expect("tamper remains parseable");
        let auth_root = tempfile::tempdir().expect("auth root");
        let authorizer =
            ActionAuthorizer::for_test(AUTH_KEY, auth_root.path().to_path_buf(), AUTH_NOW);

        assert!(authorize_expose_request(&authorizer, unsigned, &request).is_err());
        assert!(authorize_expose_request(&authorizer, &tampered, &tampered_request).is_err());
        assert!(authorize_expose_request(&authorizer, &armed, &request).is_ok());
        assert!(authorize_expose_request(&authorizer, &armed, &request).is_err());
    }

    #[test]
    fn failed_expose_reply_retry_and_reload_counts_are_honest() {
        let unsigned = r#"{"vm_nebula_ip":"10.42.128.1","guest_port":8080,"proto":"tcp","networks":["mesh","lan"],"schema_version":1}"#;
        let request = parse_expose_request(unsigned).expect("valid expose request");
        let armed = authorize_test_body(
            AUTH_KEY,
            unsigned,
            MutationContext {
                verb: COMPUTE_EXPOSE_AUTH_VERB,
                node: COMPUTE_EXPOSE_NODE_SCOPE,
                target: &expose_auth_target(&request),
            },
            "compute-expose-failing-once",
            AUTH_NOW + 30_000,
        );
        let bus_root = tempfile::tempdir().expect("bus root");
        let persist = Persist::open(bus_root.path().to_path_buf()).expect("persist");
        let auth_root = tempfile::tempdir().expect("auth root");
        let journal_root = tempfile::tempdir().expect("journal root");
        let journal_path = journal_root.path().join("action-journal.json");
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            auth_root.path().to_path_buf(),
            AUTH_NOW,
        ));
        let worker = ComputeExposeWorker::new()
            .with_authorizer(Arc::clone(&authorizer))
            .with_mutation_runner(Arc::new(AlwaysFailFirewall))
            .with_journal_path(journal_path.clone())
            .with_result_writer(Arc::new(FailFirstResultWriter {
                attempts: AtomicUsize::new(0),
            }));
        let request_message = persist
            .write(
                "compute/expose/10.42.0.15",
                Priority::Default,
                None,
                Some(&armed),
            )
            .expect("request write");
        let mut expose_cursor = None;
        let mut unexpose_cursor = None;

        poll_once(
            &persist,
            bus_root.path(),
            compute_expose_bus_identity(bus_root.path()).expect("Bus identity"),
            &worker,
            "10.42.0.15",
            "public",
            &mut expose_cursor,
            &mut unexpose_cursor,
            authorizer.as_ref(),
        )
        .expect("first poll");

        assert!(
            expose_cursor.is_none(),
            "failed reply must withhold acknowledgement"
        );
        assert!(worker.active_snapshot().is_empty());
        assert!(persist
            .list_since("compute/exposed/10.42.0.15", None)
            .expect("state query")
            .is_empty());
        assert!(persist
            .list_since(&action_result_topic(&request_message.ulid), None)
            .expect("failed result query")
            .is_empty());
        let journal = load_action_journal(&journal_path).expect("durable journal");
        assert!(journal.entries[&request_message.ulid].terminal.is_some());
        assert!(authorize_expose_request(&authorizer, &armed, &request).is_err());

        let restarted_worker = ComputeExposeWorker::new()
            .with_authorizer(Arc::clone(&authorizer))
            .with_mutation_runner(Arc::new(PanicFirewall))
            .with_journal_path(journal_path.clone());

        poll_once(
            &persist,
            bus_root.path(),
            compute_expose_bus_identity(bus_root.path()).expect("Bus identity"),
            &restarted_worker,
            "10.42.0.15",
            "public",
            &mut expose_cursor,
            &mut unexpose_cursor,
            authorizer.as_ref(),
        )
        .expect("restart poll");

        assert_eq!(
            expose_cursor.as_deref(),
            Some(request_message.ulid.as_str())
        );
        let replies = persist
            .list_since(&action_result_topic(&request_message.ulid), None)
            .expect("result query");
        assert_eq!(replies.len(), 1);
        let result: FirewallActionResult =
            serde_json::from_str(replies[0].body.as_deref().expect("result body"))
                .expect("typed result");
        assert_eq!(result.request_ulid, request_message.ulid);
        assert_eq!(result.action, "expose");
        assert_eq!(result.outcome, FirewallActionOutcome::Failed);
        assert_eq!((result.attempted, result.applied, result.failed), (2, 0, 2));
        assert!(!result.detail.contains("firewall-cmd"));

        poll_once(
            &persist,
            bus_root.path(),
            compute_expose_bus_identity(bus_root.path()).expect("Bus identity"),
            &restarted_worker,
            "10.42.0.15",
            "public",
            &mut expose_cursor,
            &mut unexpose_cursor,
            authorizer.as_ref(),
        )
        .expect("idempotence poll");
        assert_eq!(
            persist
                .list_since(&action_result_topic(&request_message.ulid), None)
                .expect("terminal result query")
                .len(),
            1,
            "consumed capability must remain terminally acknowledged"
        );

        let prepared_unsigned = r#"{"vm_nebula_ip":"10.42.128.9","guest_port":9443,"proto":"tcp","networks":["mesh"],"schema_version":1}"#;
        let prepared_request = parse_expose_request(prepared_unsigned).expect("prepared request");
        let prepared_armed = authorize_test_body(
            AUTH_KEY,
            prepared_unsigned,
            MutationContext {
                verb: COMPUTE_EXPOSE_AUTH_VERB,
                node: COMPUTE_EXPOSE_NODE_SCOPE,
                target: &expose_auth_target(&prepared_request),
            },
            "compute-expose-prepared-crash",
            AUTH_NOW + 30_000,
        );
        let prepared_message = persist
            .write(
                "compute/expose/10.42.0.15",
                Priority::Default,
                None,
                Some(&prepared_armed),
            )
            .expect("prepared request write");
        prepare_action(
            &restarted_worker,
            &prepared_message.ulid,
            "expose",
            diff_expose(&BTreeSet::new(), &prepared_request),
        )
        .expect("write-ahead prepare");
        authorize_expose_request(&authorizer, &prepared_armed, &prepared_request)
            .expect("consume before simulated crash");
        let second_restart = ComputeExposeWorker::new()
            .with_authorizer(Arc::clone(&authorizer))
            .with_mutation_runner(Arc::new(PanicFirewall))
            .with_journal_path(journal_path.clone());
        poll_once(
            &persist,
            bus_root.path(),
            compute_expose_bus_identity(bus_root.path()).expect("Bus identity"),
            &second_restart,
            "10.42.0.15",
            "public",
            &mut expose_cursor,
            &mut unexpose_cursor,
            authorizer.as_ref(),
        )
        .expect("prepared recovery poll");
        assert_eq!(
            expose_cursor.as_deref(),
            Some(prepared_message.ulid.as_str())
        );
        let prepared_reply = persist
            .read_latest(&action_result_topic(&prepared_message.ulid))
            .expect("prepared result query")
            .expect("prepared result");
        let prepared_result: FirewallActionResult = serde_json::from_str(
            prepared_reply
                .body
                .as_deref()
                .expect("prepared result body"),
        )
        .expect("prepared typed result");
        assert_eq!(prepared_result.outcome, FirewallActionOutcome::Failed);
        assert_eq!(
            (
                prepared_result.attempted,
                prepared_result.applied,
                prepared_result.failed,
            ),
            (1, 0, 1)
        );

        let reload_worker =
            ComputeExposeWorker::new().with_mutation_runner(Arc::new(ReloadFailFirewall));
        let reload_summary = apply_expose(
            &reload_worker,
            "10.42.0.15",
            "public",
            &expose_req("10.42.128.2", 8443, &[Network::Mesh, Network::Lan]),
        );
        let reload_result = build_action_result("01RELOADFAIL", "expose", reload_summary);
        assert_eq!(reload_result.outcome, FirewallActionOutcome::Partial);
        assert_eq!(
            (
                reload_result.attempted,
                reload_result.applied,
                reload_result.failed,
            ),
            (2, 0, 2),
            "reload failure leaves every requested change failed-to-activate"
        );
        assert!(reload_worker.active_snapshot().is_empty());

        let recording = Arc::new(RecordingFirewall::default());
        let mesh_worker = ComputeExposeWorker::new().with_mutation_runner(recording.clone());
        let mesh_request = expose_req("10.42.128.10", 3389, &[Network::Mesh]);
        assert_eq!(
            apply_expose(&mesh_worker, "10.42.0.15", "public", &mesh_request,).outcome,
            FirewallActionOutcome::Applied
        );
        assert_eq!(
            apply_unexpose(
                &mesh_worker,
                "10.42.0.15",
                "public",
                &unexpose_req("10.42.128.10", 3389),
            )
            .outcome,
            FirewallActionOutcome::Applied
        );
        let calls = recording.calls.lock().expect("calls");
        let added_body = calls[0][2]
            .strip_prefix("--add-rich-rule=")
            .expect("add body");
        let removed_body = calls[2][2]
            .strip_prefix("--remove-rich-rule=")
            .expect("remove body");
        assert_eq!(
            added_body, removed_body,
            "Mesh removal must exactly match the local-destination add body"
        );
    }

    #[test]
    fn unexpose_rejects_unsigned_tampered_and_replayed_bodies() {
        let unsigned =
            r#"{"vm_nebula_ip":"10.42.128.1","host_port":8080,"proto":"tcp","schema_version":1}"#;
        let request = parse_unexpose_request(unsigned).expect("valid unexpose request");
        let target = unexpose_auth_target(&request);
        let armed = authorize_test_body(
            AUTH_KEY,
            unsigned,
            MutationContext {
                verb: COMPUTE_UNEXPOSE_AUTH_VERB,
                node: COMPUTE_EXPOSE_NODE_SCOPE,
                target: &target,
            },
            "compute-unexpose-once",
            AUTH_NOW + 30_000,
        );
        let tampered = armed.replace("8080", "8081");
        let tampered_request = parse_unexpose_request(&tampered).expect("tamper remains parseable");
        let auth_root = tempfile::tempdir().expect("auth root");
        let authorizer =
            ActionAuthorizer::for_test(AUTH_KEY, auth_root.path().to_path_buf(), AUTH_NOW);

        assert!(authorize_unexpose_request(&authorizer, unsigned, &request).is_err());
        assert!(authorize_unexpose_request(&authorizer, &tampered, &tampered_request).is_err());
        assert!(authorize_unexpose_request(&authorizer, &armed, &request).is_ok());
        assert!(authorize_unexpose_request(&authorizer, &armed, &request).is_err());
    }

    // ── VIRT-7.followup: parse_rich_rule (reverse of build_rich_rule_body) ──

    #[test]
    fn parse_rich_rule_round_trips_build() {
        // Lan/Wan form (no leading destination address).
        let body = build_rich_rule_body(Network::Lan, "10.42.0.1", "10.42.128.7", 8080, "tcp");
        assert_eq!(
            parse_rich_rule(Network::Lan, &body),
            Some(ActiveRule {
                network: Network::Lan,
                vm_nebula_ip: "10.42.128.7".to_string(),
                host_port: 8080,
                proto: "tcp".to_string(),
            })
        );
        // Mesh form (has a leading `destination address="…"`).
        let mesh = build_rich_rule_body(Network::Mesh, "10.42.0.1", "10.42.200.3", 443, "tcp");
        assert_eq!(
            parse_rich_rule(Network::Mesh, &mesh),
            Some(ActiveRule {
                network: Network::Mesh,
                vm_nebula_ip: "10.42.200.3".to_string(),
                host_port: 443,
                proto: "tcp".to_string(),
            })
        );
    }

    #[test]
    fn parse_rich_rule_skips_unmanaged() {
        // A plain service rule (no forward-port).
        assert_eq!(
            parse_rich_rule(
                Network::Lan,
                r#"rule family="ipv4" service name="ssh" accept"#
            ),
            None
        );
        // A forward-port to a non-VM address (not one of ours).
        let foreign = r#"rule family="ipv4" port port="80" protocol="tcp" forward-port port="80" protocol="tcp" to-addr="192.168.1.5" to-port="80""#;
        assert_eq!(parse_rich_rule(Network::Lan, foreign), None);
        // Blank line.
        assert_eq!(parse_rich_rule(Network::Lan, ""), None);
    }

    #[test]
    fn parse_rich_rule_multiline_list_output() {
        // Simulates `firewall-cmd --list-rich-rules` (one rule per line).
        let body1 = build_rich_rule_body(Network::Lan, "10.42.0.1", "10.42.128.7", 8080, "tcp");
        let body2 = build_rich_rule_body(Network::Lan, "10.42.0.1", "10.42.128.9", 5432, "tcp");
        let out = format!("{body1}\n{body2}\n");
        let rules: Vec<ActiveRule> = out
            .lines()
            .filter_map(|l| parse_rich_rule(Network::Lan, l))
            .collect();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].host_port, 8080);
        assert_eq!(rules[1].host_port, 5432);
    }

    // ── Network enum ──

    #[test]
    fn network_parse_round_trip() {
        for n in [Network::Mesh, Network::Lan, Network::Wan] {
            assert_eq!(Network::parse(n.wire_name()), Some(n));
        }
        assert_eq!(Network::parse("unknown"), None);
    }

    // ── zone_for_network ──

    #[test]
    fn zone_for_network_uses_design_doc_mapping() {
        assert_eq!(zone_for_network(Network::Mesh, "extern"), "trusted");
        assert_eq!(zone_for_network(Network::Lan, "extern"), "public");
        assert_eq!(zone_for_network(Network::Wan, "extern"), "extern");
    }

    // ── build_rich_rule_body ──

    #[test]
    fn rich_rule_mesh_includes_destination_address() {
        let body = build_rich_rule_body(Network::Mesh, "10.42.0.5", "10.42.128.1", 8080, "tcp");
        assert!(body.contains(r#"destination address="10.42.0.5""#));
        assert!(body.contains(r#"port port="8080""#));
        assert!(body.contains(r#"to-addr="10.42.128.1""#));
        assert!(body.contains(r#"protocol="tcp""#));
    }

    #[test]
    fn rich_rule_lan_has_no_destination_address() {
        let body = build_rich_rule_body(Network::Lan, "10.42.0.5", "10.42.128.1", 8080, "tcp");
        assert!(!body.contains("destination address"));
        assert!(body.contains(r#"to-addr="10.42.128.1""#));
    }

    // ── add/remove args ──

    #[test]
    fn add_rich_rule_args_use_permanent_and_zone() {
        let args = add_rich_rule_args("trusted", "rule ...");
        assert_eq!(args[0], "--permanent");
        assert_eq!(args[1], "--zone=trusted");
        assert!(args[2].starts_with("--add-rich-rule="));
    }

    #[test]
    fn remove_rich_rule_args_use_permanent_and_zone() {
        let args = remove_rich_rule_args("public", "rule ...");
        assert_eq!(args[0], "--permanent");
        assert_eq!(args[1], "--zone=public");
        assert!(args[2].starts_with("--remove-rich-rule="));
    }

    // ── parse_expose_request ──

    #[test]
    fn parse_expose_happy_path() {
        let body = r#"{"vm_nebula_ip":"10.42.128.1","guest_port":8080,"proto":"tcp","networks":["mesh","lan"]}"#;
        let req = parse_expose_request(body).expect("parse");
        assert_eq!(req.vm_nebula_ip, "10.42.128.1");
        assert_eq!(req.guest_port, 8080);
        assert_eq!(req.proto, "tcp");
        assert_eq!(req.networks, vec![Network::Mesh, Network::Lan]);
    }

    #[test]
    fn parse_expose_rejects_unknown_network() {
        let body =
            r#"{"vm_nebula_ip":"10.42.128.1","guest_port":8080,"proto":"tcp","networks":["pony"]}"#;
        let err = parse_expose_request(body).expect_err("unknown network");
        assert!(err.contains("pony"));
    }

    #[test]
    fn parse_expose_rejects_malformed_json() {
        let err = parse_expose_request("not json").expect_err("malformed");
        assert!(err.contains("malformed"));
    }

    #[test]
    fn parse_unexpose_happy_path() {
        let body = r#"{"vm_nebula_ip":"10.42.128.1","host_port":8080,"proto":"tcp"}"#;
        let req = parse_unexpose_request(body).expect("parse");
        assert_eq!(req.host_port, 8080);
        assert_eq!(req.proto, "tcp");
    }

    // ── nmcli parser ──

    #[test]
    fn parse_default_gateway_skips_loopback_and_nebula() {
        let raw = "lo:loopback:connected:lo\nnebula1:tun:connected:nebula\neth0:ethernet:connected:Wired\n";
        assert_eq!(parse_default_gateway_device(raw), Some("eth0".into()));
    }

    #[test]
    fn parse_default_gateway_picks_wifi_when_no_ethernet() {
        let raw = "wlan0:wifi:connected:home\n";
        assert_eq!(parse_default_gateway_device(raw), Some("wlan0".into()));
    }

    #[test]
    fn parse_default_gateway_none_when_only_disconnected() {
        let raw = "eth0:ethernet:disconnected:--\n";
        assert!(parse_default_gateway_device(raw).is_none());
    }

    #[test]
    fn parse_zone_of_interface_returns_zone() {
        assert_eq!(parse_zone_of_interface("public\n"), Some("public".into()));
    }

    #[test]
    fn parse_zone_of_interface_none_when_no_zone() {
        assert!(parse_zone_of_interface("no zone").is_none());
        assert!(parse_zone_of_interface("").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn bounded_probe_fails_closed_when_child_hangs() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 1"]);
        assert!(output_with_timeout(command, Duration::from_millis(50)).is_err());
    }

    // ── Required scenario 1: expose mesh-only ──

    fn expose_req(ip: &str, port: u16, nets: &[Network]) -> ExposeRequest {
        ExposeRequest {
            vm_nebula_ip: ip.into(),
            guest_port: port,
            proto: "tcp".into(),
            networks: nets.to_vec(),
        }
    }

    fn unexpose_req(ip: &str, port: u16) -> UnexposeRequest {
        UnexposeRequest {
            vm_nebula_ip: ip.into(),
            host_port: port,
            proto: "tcp".into(),
        }
    }

    #[test]
    fn diff_expose_mesh_only_yields_one_rule() {
        let active = BTreeSet::new();
        let req = expose_req("10.42.128.1", 8080, &[Network::Mesh]);
        let new = diff_expose(&active, &req);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].network, Network::Mesh);
    }

    // ── Required scenario 2: expose all three ──

    #[test]
    fn diff_expose_all_three_yields_three_rules() {
        let active = BTreeSet::new();
        let req = expose_req(
            "10.42.128.1",
            8080,
            &[Network::Mesh, Network::Lan, Network::Wan],
        );
        let new = diff_expose(&active, &req);
        assert_eq!(new.len(), 3);
        let networks: BTreeSet<Network> = new.iter().map(|r| r.network).collect();
        assert!(networks.contains(&Network::Mesh));
        assert!(networks.contains(&Network::Lan));
        assert!(networks.contains(&Network::Wan));
    }

    // ── Required scenario 5: idempotent re-expose ──

    #[test]
    fn diff_expose_idempotent_when_already_active() {
        let mut active = BTreeSet::new();
        active.insert(ActiveRule {
            network: Network::Mesh,
            vm_nebula_ip: "10.42.128.1".into(),
            host_port: 8080,
            proto: "tcp".into(),
        });
        let req = expose_req("10.42.128.1", 8080, &[Network::Mesh]);
        assert!(diff_expose(&active, &req).is_empty());
    }

    // ── Required scenario 3: remove one network ──

    #[test]
    fn diff_unexpose_removes_all_networks_for_matching_vm_and_port() {
        let mut active = BTreeSet::new();
        for n in [Network::Mesh, Network::Lan, Network::Wan] {
            active.insert(ActiveRule {
                network: n,
                vm_nebula_ip: "10.42.128.1".into(),
                host_port: 8080,
                proto: "tcp".into(),
            });
        }
        // Unrelated rule that must NOT be touched.
        active.insert(ActiveRule {
            network: Network::Mesh,
            vm_nebula_ip: "10.42.128.2".into(),
            host_port: 9090,
            proto: "tcp".into(),
        });
        let removals = diff_unexpose(&active, &unexpose_req("10.42.128.1", 8080));
        assert_eq!(removals.len(), 3, "should match all three networks");
        assert!(removals.iter().all(|r| r.vm_nebula_ip == "10.42.128.1"));
    }

    // ── Required scenario 4: remove all (via apply_unexpose on a
    //    worker; tests the shadow-set update directly) ──

    #[test]
    fn apply_unexpose_retains_shadow_rules_when_firewall_cmd_fails() {
        let worker = ComputeExposeWorker::new();
        {
            let mut active = worker
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            active.insert(ActiveRule {
                network: Network::Mesh,
                vm_nebula_ip: "10.42.128.1".into(),
                host_port: 8080,
                proto: "tcp".into(),
            });
            active.insert(ActiveRule {
                network: Network::Lan,
                vm_nebula_ip: "10.42.128.1".into(),
                host_port: 8080,
                proto: "tcp".into(),
            });
        }
        let summary = apply_unexpose(
            &worker,
            "10.42.0.15",
            "public",
            &unexpose_req("10.42.128.1", 8080),
        );
        let active = worker
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(summary.outcome, FirewallActionOutcome::Failed);
        assert_eq!(active.len(), 2, "failed removal cannot fabricate absence");
    }

    // ── ExposedState serializes with all required fields ──

    #[test]
    fn exposed_state_json_shape() {
        let state = ExposedState {
            peer: "10.42.0.5".into(),
            rules: vec![ActiveRule {
                network: Network::Mesh,
                vm_nebula_ip: "10.42.128.1".into(),
                host_port: 8080,
                proto: "tcp".into(),
            }],
        };
        let s = serde_json::to_string(&state).unwrap();
        for field in [
            "\"peer\"",
            "\"rules\"",
            "\"network\"",
            "\"vm_nebula_ip\"",
            "\"host_port\"",
            "\"proto\"",
            "\"mesh\"",
        ] {
            assert!(s.contains(field), "missing field {field} in {s}");
        }
    }
}
