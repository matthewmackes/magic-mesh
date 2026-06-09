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
//! lighthouse profile), the worker logs and exits without retrying.

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Default poll cadence — control surface (firewalld changes are
/// not on a human's interactive path).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

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
    Command::new(bin).arg("--version").output().is_ok()
}

fn run_firewall_cmd(args: &[String]) -> bool {
    Command::new("firewall-cmd")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `firewall-cmd <args>` and capture stdout (empty string on
/// failure). Used for read-only queries like `--list-rich-rules`.
fn firewall_cmd_stdout(args: &[String]) -> String {
    Command::new("firewall-cmd")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn detect_wan_zone() -> String {
    let nmcli_out = Command::new("nmcli")
        .args(["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let Some(dev) = parse_default_gateway_device(&nmcli_out) else {
        return DEFAULT_WAN_ZONE.to_string();
    };
    let arg = format!("--get-zone-of-interface={dev}");
    let zone_out = Command::new("firewall-cmd")
        .arg(&arg)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    parse_zone_of_interface(&zone_out).unwrap_or_else(|| DEFAULT_WAN_ZONE.to_string())
}

fn local_nebula_addr(interface: &str) -> String {
    let Ok(output) = Command::new("ip")
        .args(["-4", "addr", "show", interface])
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

fn apply_expose(
    worker: &ComputeExposeWorker,
    nebula_ip: &str,
    wan_zone: &str,
    req: &ExposeRequest,
) {
    let mut active = worker.active.lock().expect("active mutex");
    let new_rules = diff_expose(&active, req);
    if new_rules.is_empty() {
        return;
    }
    let mut any_applied = false;
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
        if run_firewall_cmd(&args) {
            active.insert(rule.clone());
            any_applied = true;
        } else {
            tracing::warn!(
                vm_ip = %rule.vm_nebula_ip,
                port = rule.host_port,
                network = rule.network.wire_name(),
                "compute_expose: firewall-cmd add-rich-rule failed"
            );
        }
    }
    if any_applied {
        let _ = run_firewall_cmd(&["--reload".to_string()]);
    }
}

fn apply_unexpose(worker: &ComputeExposeWorker, wan_zone: &str, req: &UnexposeRequest) {
    let mut active = worker.active.lock().expect("active mutex");
    let removals = diff_unexpose(&active, req);
    if removals.is_empty() {
        return;
    }
    let mut any_removed = false;
    for rule in &removals {
        let zone = zone_for_network(rule.network, wan_zone);
        let body = build_rich_rule_body(
            rule.network,
            "", // destination address unused for lan/wan; mesh used the original at insert time
            &rule.vm_nebula_ip,
            rule.host_port,
            &rule.proto,
        );
        let args = remove_rich_rule_args(&zone, &body);
        if run_firewall_cmd(&args) {
            active.remove(rule);
            any_removed = true;
        } else {
            // Even if firewalld rejects (rule already gone), drop
            // from shadow set so the published topic catches up.
            active.remove(rule);
            tracing::debug!(
                vm_ip = %rule.vm_nebula_ip,
                port = rule.host_port,
                network = rule.network.wire_name(),
                "compute_expose: firewall-cmd remove-rich-rule non-zero; dropped from shadow set anyway"
            );
        }
    }
    if any_removed {
        let _ = run_firewall_cmd(&["--reload".to_string()]);
    }
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

fn poll_once(
    persist: &Persist,
    worker: &ComputeExposeWorker,
    nebula_ip: &str,
    wan_zone: &str,
    expose_cursor: &mut Option<String>,
    unexpose_cursor: &mut Option<String>,
) {
    let expose_topic = format!("compute/expose/{nebula_ip}");
    let unexpose_topic = format!("compute/unexpose/{nebula_ip}");

    let mut changed = false;
    match persist.list_since(&expose_topic, expose_cursor.as_deref()) {
        Ok(msgs) => {
            for msg in msgs {
                *expose_cursor = Some(msg.ulid.clone());
                let body = msg.body.as_deref().unwrap_or("");
                match parse_expose_request(body) {
                    Ok(req) => {
                        apply_expose(worker, nebula_ip, wan_zone, &req);
                        changed = true;
                    }
                    Err(e) => {
                        tracing::warn!(ulid = %msg.ulid, error = %e, "compute_expose: bad expose request");
                    }
                }
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, topic = expose_topic, "compute_expose: list_since failed")
        }
    }

    match persist.list_since(&unexpose_topic, unexpose_cursor.as_deref()) {
        Ok(msgs) => {
            for msg in msgs {
                *unexpose_cursor = Some(msg.ulid.clone());
                let body = msg.body.as_deref().unwrap_or("");
                match parse_unexpose_request(body) {
                    Ok(req) => {
                        apply_unexpose(worker, wan_zone, &req);
                        changed = true;
                    }
                    Err(e) => {
                        tracing::warn!(ulid = %msg.ulid, error = %e, "compute_expose: bad unexpose request");
                    }
                }
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, topic = unexpose_topic, "compute_expose: list_since failed")
        }
    }

    if changed {
        publish_exposed_state(persist, nebula_ip, worker);
    }
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
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
    let mut active = worker.active.lock().expect("active mutex");
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
        if !binary_present("firewall-cmd") {
            tracing::debug!("compute_expose: firewall-cmd absent; worker idle");
            return Ok(());
        }
        let bus_root = match self.bus_root_override.clone().or_else(default_bus_root) {
            Some(r) => r,
            None => {
                tracing::debug!("compute_expose: no bus root; worker idle");
                return Ok(());
            }
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(error = %e, "compute_expose: persist open failed; worker idle");
                return Ok(());
            }
        };
        let wan_zone = detect_wan_zone();
        // VIRT-7.followup: seed the shadow set from firewalld's persisted
        // (--permanent) rules so the first compute/exposed publish reflects
        // reality after a restart instead of an empty set.
        seed_active_from_firewalld(self, &wan_zone);
        let mut expose_cursor: Option<String> = None;
        let mut unexpose_cursor: Option<String> = None;
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
                    poll_once(
                        &persist,
                        self,
                        &nebula_ip,
                        &wan_zone,
                        &mut expose_cursor,
                        &mut unexpose_cursor,
                    );
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
    fn apply_unexpose_drops_shadow_rules_even_when_firewall_cmd_unavailable() {
        let worker = ComputeExposeWorker::new();
        {
            let mut active = worker.active.lock().expect("active mutex");
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
        // apply_unexpose attempts firewall-cmd (absent in test env)
        // but per the design always drops from the shadow set so the
        // published `compute/exposed/<peer>` stays consistent with
        // operator intent.
        apply_unexpose(&worker, "public", &unexpose_req("10.42.128.1", 8080));
        let active = worker.active.lock().expect("active mutex");
        assert!(
            active.is_empty(),
            "shadow set should be empty after unexpose"
        );
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
