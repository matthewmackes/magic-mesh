//! DATACENTER (action layer) — `action/dc/host-power` → Xen host (dom0)
//! maintenance + reboot control, plus the LIGHTHOUSE-6 anchor-node ops.
//!
//! Companion to the VM power responder ([`crate::ipc::datacenter`]): where that
//! acts on a guest VM, this acts on the host (pool member) itself. Same
//! dedicated-OS-thread, `action/dc/<verb>` Bus-RPC shape; the reads/exec are
//! synchronous SSH calls over the mesh key.
//!
//! Request body `{ "dom0", "op": "maintenance-on"|"maintenance-off"|"reboot" }`:
//!   * `op` maps to a sequence of `xe` host verbs
//!     ([`host_power_commands`]) — `maintenance-on`→`host-disable`,
//!     `maintenance-off`→`host-enable`, `reboot`→`host-disable`+`host-reboot`
//!     (XAPI requires the host be disabled before it will reboot it);
//!   * `dom0` MUST be in the configured allowed set
//!     ([`crate::workers::datacenter_orchestrator::xen_dom0s`]) before any SSH.
//! The host UUID is resolved remotely (`xe host-list params=uuid --minimal`),
//! then each verb runs as `xe <verb> host=<uuid>` in sequence.
//! Reply `{"ok":true}` when every step succeeds, `{"error":"<message>"}` otherwise.
//!
//! LIGHTHOUSE-6 — the Workbench Lighthouses tab's full-ops actions land here too
//! (this is the already-spawned, mesh-key-SSH ops responder; the actions reuse
//! its remote-exec + the daemon's leader-lease plumbing, no new transport):
//!   * `lighthouse-restart` `{ "overlay_ip", "confirm": true }` — restart the
//!     anchor's core fabric units (`mackesd` + `nebula`) over the mesh key
//!     ([`lighthouse_restart`]). `overlay_ip` is validated as a plain IPv4
//!     ([`valid_ipv4`]) before any SSH, so it can never carry shell metachars.
//!   * `lighthouse-promote` `{ "node", "confirm": true }` — promote a shadow
//!     anchor to mesh leader via the EXISTING leader-lease force-take (substrate-
//!     aware: etcd `force` when on the coordination plane, else the fs lockfile
//!     [`crate::leader::force_take`]). Idempotent: refuses if `node` already
//!     holds the lease ([`lighthouse_promote`]).
//! (The `lighthouse-ssh` action is a pure Workbench-side terminal launch — it
//! opens a local `cosmic-term ssh` to the overlay IP and never round-trips the
//! daemon, so there is no `lighthouse-ssh` responder verb here.)

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The host power-control responder — rooted at the shared workgroup root (carried
/// for parity with the other action services; the allowed-dom0 set + ssh key come
/// from the orchestrator's env-driven config).
#[derive(Debug, Clone)]
pub struct HostOpsService {
    // The shared workgroup root — read by the LIGHTHOUSE-6 promote verb to locate
    // the `.mackesd-leader.lock` for the fs-lockfile leader path. (The dom0 SSH
    // key + allowed-dom0 set come from the orchestrator's env config, not here.)
    workgroup_root: PathBuf,
}

impl HostOpsService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 14] = [
    "host-power",
    // DATACENTER-10 — the Hosts tab's impact preview + pool read.
    "host-impact",
    "host-pool",
    "gateway-reboot",
    "dr-backup",
    "gateway-status",
    // LIGHTHOUSE-6 — the Workbench Lighthouses tab's full-ops actions.
    "lighthouse-restart",
    "lighthouse-promote",
    // DATACENTER-13 — the Network tab's L2 read + VLAN create.
    "host-net",
    "host-vlan-create",
    // DATACENTER-14 — the Gateway tab's EdgeOS DHCP read (reservations + leases).
    "gateway-dhcp",
    // ROUTER-3 — seal a per-appliance router credential (router/<mac>) into the mesh store.
    "router-seal-cred",
    // DATACENTER-21 — ephemeral test-mesh (list) + build-farm autoscale (reconcile/plan, no apply).
    "testbed-list",
    "farm-scale",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Map a host power `op` to the ordered list of `xe` host verbs it runs. PURE.
///
/// * `maintenance-on`  → `["host-disable"]` (enters maintenance mode);
/// * `maintenance-off` → `["host-enable"]` (leaves maintenance mode);
/// * `reboot`          → `["host-disable", "host-reboot"]` — XAPI refuses to
///   reboot an enabled host, so it must be disabled first;
/// * `shutdown`        → `["host-disable", "host-shutdown"]` — same gate: XAPI
///   refuses to shut down an enabled host, so disable it first;
/// * `evacuate`        → `["host-disable", "host-evacuate"]` — disable so no new
///   VMs land, then live-migrate every resident guest off onto other pool
///   members (the day-2 "drain before patch/reboot" primitive).
///
/// Each returned verb is later run as `xe <verb> host=<uuid>`.
///
/// # Errors
/// Returns `Err` for any `op` outside the five above.
pub fn host_power_commands(op: &str) -> Result<Vec<String>, String> {
    match op {
        "maintenance-on" => Ok(vec!["host-disable".to_string()]),
        "maintenance-off" => Ok(vec!["host-enable".to_string()]),
        "reboot" => Ok(vec!["host-disable".to_string(), "host-reboot".to_string()]),
        "shutdown" => Ok(vec![
            "host-disable".to_string(),
            "host-shutdown".to_string(),
        ]),
        "evacuate" => Ok(vec![
            "host-disable".to_string(),
            "host-evacuate".to_string(),
        ]),
        other => Err(format!("unknown op: {other}")),
    }
}

/// Whether a host-power `op` is *destructive / irreversible* and so must be
/// `confirm:true`-gated. PURE.
///
/// `reboot` / `shutdown` bounce the whole host (every resident guest goes down);
/// `evacuate` live-migrates every running guest off — all three are disruptive
/// fleet-level operations on the §8/§9 flat-trust mesh and get the same
/// typed-confirm contract as `vm-delete` / `vdi-detach` / `tofu-destroy`. The
/// reversible maintenance toggles (`maintenance-on` / `maintenance-off`) are NOT
/// gated — they only flip the host's scheduling flag and are trivially undone.
/// An unknown op is treated as non-destructive here (the `host_power_commands`
/// map rejects it on its own merits before any SSH).
#[must_use]
pub fn host_power_is_destructive(op: &str) -> bool {
    matches!(op, "reboot" | "shutdown" | "evacuate")
}

/// Parse the `xe vm-list resident-on=<uuid> power-state=running --minimal` reply
/// (a comma-separated list of running-guest uuids, possibly empty) into the count
/// of guests that would be affected by draining/rebooting/shutting down the host.
/// PURE — the impact-preview number the panel renders before a destructive op.
#[must_use]
pub fn parse_running_count(minimal: &str) -> usize {
    minimal
        .trim()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .count()
}

/// Parse the two `xe`-minimal lines that read a host's pool placement into the
/// `(pool_name, master_uuid, is_master)` triple the panel renders. PURE.
///
/// * `pool_line`  — `xe pool-list params=name-label --minimal` (the single pool's
///   name; empty on a pool-of-one with no label);
/// * `master_line`— `xe pool-list params=master --minimal` (the master host's
///   uuid);
/// * `this_uuid`  — this host's own uuid, already resolved by the caller.
///
/// `is_master` is `true` iff this host's uuid equals the pool master's — so the
/// panel can badge the master and gate join/leave accordingly.
#[must_use]
pub fn parse_pool(pool_line: &str, master_line: &str, this_uuid: &str) -> (String, String, bool) {
    let pool = pool_line.trim().to_string();
    let master = master_line.trim().to_string();
    let is_master = !master.is_empty() && master == this_uuid.trim();
    (pool, master, is_master)
}

/// DATACENTER-13 (Network tab) — one physical interface (PIF) on a dom0.
///
/// Decoded from the `host-net` read's `pif` block. The Network tab's L2 view
/// renders these as the NIC inventory (device, MAC, the network it backs,
/// carrier/management flags, and the VLAN tag when this PIF is a VLAN
/// sub-interface).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PifInfo {
    /// The PIF uuid.
    pub uuid: String,
    /// The NIC device name (e.g. `eth0`, or `eth0.100` for a VLAN PIF).
    pub device: String,
    /// The hardware MAC address.
    pub mac: String,
    /// The VLAN tag (`-1` = not a VLAN / untagged trunk).
    pub vlan: i64,
    /// Whether the link is up (`carrier`).
    pub carrier: bool,
    /// Whether this PIF carries the host's management interface.
    pub management: bool,
    /// The uuid of the XAPI network this PIF attaches to.
    pub network: String,
}

/// DATACENTER-13 (Network tab) — one L2 network (bridge) on a dom0.
///
/// Decoded from the `host-net` read's `net` block. Mirrors the orchestrator's
/// `event/dc/net/*` shape but carries the extra L2 detail (bridge + MTU + VLAN
/// association) the Network tab's create/inspect flows need that the lightweight
/// event omits.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct NetInfo {
    /// The network uuid.
    pub uuid: String,
    /// The network's name-label.
    pub name: String,
    /// The Linux bridge (e.g. `xenbr0`).
    pub bridge: String,
    /// The MTU (bytes); `0` when the read returned none.
    pub mtu: u32,
}

/// DATACENTER-13 — parse the `host-net` PIF block into [`PifInfo`]s.
///
/// Fed the pipe-delimited `uuid|device|mac|VLAN|carrier|management|network` lines.
/// PURE. Skips blank/short lines; the booleans accept the XAPI `true`/`false`
/// spelling (anything else is `false`); the VLAN parses as a signed integer (XAPI
/// uses `-1` for a non-VLAN PIF), defaulting to `-1`.
#[must_use]
pub fn parse_pifs(output: &str) -> Vec<PifInfo> {
    output
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(7, '|');
            let uuid = p.next()?.trim();
            if uuid.is_empty() {
                return None;
            }
            let device = p.next().unwrap_or("").trim();
            let mac = p.next().unwrap_or("").trim();
            let vlan = p.next().unwrap_or("").trim().parse::<i64>().unwrap_or(-1);
            let carrier = p.next().unwrap_or("").trim() == "true";
            let management = p.next().unwrap_or("").trim() == "true";
            let network = p.next().unwrap_or("").trim();
            Some(PifInfo {
                uuid: uuid.to_string(),
                device: device.to_string(),
                mac: mac.to_string(),
                vlan,
                carrier,
                management,
                network: network.to_string(),
            })
        })
        .collect()
}

/// DATACENTER-13 — parse the `host-net` network block into [`NetInfo`]s.
///
/// Fed the pipe-delimited `uuid|name|bridge|MTU` lines. PURE. Skips blank/short
/// lines; the MTU parses as a `u32`, defaulting to `0` when empty/unparseable.
#[must_use]
pub fn parse_nets(output: &str) -> Vec<NetInfo> {
    output
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '|');
            let uuid = p.next()?.trim();
            if uuid.is_empty() {
                return None;
            }
            let name = p.next().unwrap_or("").trim();
            let bridge = p.next().unwrap_or("").trim();
            let mtu = p.next().unwrap_or("").trim().parse::<u32>().unwrap_or(0);
            Some(NetInfo {
                uuid: uuid.to_string(),
                name: name.to_string(),
                bridge: bridge.to_string(),
                mtu,
            })
        })
        .collect()
}

/// DATACENTER-13 — validate a VLAN-create request and build its `xe` command pair.
///
/// Given the request fields `pif` / `vlan` / `network_name`, validates each and (on
/// success) returns the ordered, fully-escaped two-step `xe` recipe. PURE so the
/// validation + command shape are unit-testable without touching the network.
///
/// * `pif` — the trunk PIF uuid the VLAN rides on; XAPI-uuid-shaped
///   (`[0-9a-f-]`), so it can never carry shell metacharacters;
/// * `vlan` — the 802.1Q tag, `1..=4094`;
/// * `network_name` — the new bridge network's name-label, `[A-Za-z0-9._-]` only.
///
/// The command is `pool-vlan-create`, the XAPI primitive that both makes the VLAN
/// sub-interface and binds it to a fresh network across the pool. The new network
/// is created first (`network-create`) and its uuid is substituted by the caller,
/// so this returns the two-step recipe markers, not a single literal.
///
/// # Errors
/// Returns `Err(<message>)` for an empty/metachar-bearing `pif`, a `vlan` outside
/// `1..=4094`, or an empty/metachar-bearing `network_name`.
pub fn vlan_create_commands(
    pif: &str,
    vlan: i64,
    network_name: &str,
) -> Result<(String, String), String> {
    if pif.is_empty() {
        return Err("empty pif".into());
    }
    if !pif.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("pif contains invalid characters".into());
    }
    if !(1..=4094).contains(&vlan) {
        return Err("vlan tag out of range (1..=4094)".into());
    }
    if network_name.is_empty() {
        return Err("empty network_name".into());
    }
    if !network_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("network_name contains invalid characters".into());
    }
    // Step 1: create the backing network (returns its uuid on stdout).
    let create_net = format!("xe network-create name-label={network_name}");
    // Step 2: bind the VLAN — `@NET@` is substituted with step-1's uuid by the
    // handler once it has been created + uuid-validated.
    let create_vlan = format!("xe pool-vlan-create pif-uuid={pif} vlan={vlan} network-uuid=@NET@");
    Ok((create_net, create_vlan))
}

/// Run a remote `xe` command on a dom0 over SSH, returning the process result.
/// Mirrors the exact ssh arg style of `ssh_xe` in the orchestrator. Thin alias
/// over [`ssh_run`] (a dom0 IS just a `root@<host>` mesh-key target) so the two
/// remote-exec paths can never drift on their SSH hardening flags.
fn ssh_xe_status(key: &str, dom0: &str, remote: &str) -> std::io::Result<std::process::Output> {
    ssh_run(key, dom0, remote)
}

/// Validate that `s` is a plain dotted-quad IPv4 address: only ASCII digits and
/// dots, exactly four octets, each parsing as `0..=255`. PURE.
///
/// Rejects anything with non-`[0-9.]` characters (so it can never carry shell
/// metacharacters into an SSH argument), the wrong number of octets, empty
/// octets, or an octet out of range.
#[must_use]
pub fn valid_ipv4(s: &str) -> bool {
    if !s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    let octets: Vec<&str> = s.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    octets
        .iter()
        .all(|o| !o.is_empty() && o.parse::<u8>().is_ok())
}

/// Read the UniFi SSH credential best-effort from the mesh secret store by
/// shelling out to `automation/secrets/mcnf-secret.sh get unifi-cred` from the
/// repo root. Returns `(user, password)` parsed like the orchestrator's
/// `gather_gateway` path (`user:pass`, default user `"ubnt"`), or `None` if the
/// helper is missing, the secret is absent, or the command exits non-zero/empty.
fn unifi_cred() -> Option<(String, String)> {
    let o = std::process::Command::new("bash")
        .args(["-lc", "automation/secrets/mcnf-secret.sh get unifi-cred"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&o.stdout);
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(crate::workers::datacenter_orchestrator::parse_unifi_cred(
        raw,
    ))
}

/// Reboot the UniFi gateway over `sshpass` (the router has no mesh key, so this
/// uses password auth). `host` must already be validated as a plain IPv4.
/// Returns `Ok(())` on a zero exit, `Err(<message>)` otherwise.
fn gateway_reboot(req_body: Option<&str>) -> Result<(), String> {
    let Some(body) = req_body else {
        return Err("gateway-reboot: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("gateway-reboot: bad json: {e}"))?;

    let confirm = req
        .get("confirm")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !confirm {
        return Err("reboot requires confirm:true".into());
    }

    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_ipv4(host) {
        return Err("host must be a plain IPv4 address".into());
    }

    let (user, pw) = unifi_cred().ok_or("no unifi cred in store")?;

    let o = std::process::Command::new("sshpass")
        .args([
            "-p",
            &pw,
            "ssh",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=8",
            &format!("{user}@{host}"),
            "reboot",
        ])
        .output()
        .map_err(|e| format!("sshpass failed: {e}"))?;

    if o.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&o.stderr);
        let msg = stderr.trim();
        if msg.is_empty() {
            Err("gateway reboot failed".into())
        } else {
            Err(msg.to_string())
        }
    }
}

/// Parse the three raw output lines from the gateway-status SSH probe into the
/// `(leases, uptime, model)` reply triple. PURE.
///
/// * `leases_line` — the DHCP lease count; parsed as `u32`, defaulting to `0`
///   when empty or unparseable (the probe falls back across two sources and may
///   yield nothing);
/// * `uptime_line` — the integer uptime-in-seconds string, trimmed;
/// * `model_line`  — the gateway model string, trimmed.
#[must_use]
pub fn parse_gateway_status(
    leases_line: &str,
    uptime_line: &str,
    model_line: &str,
) -> (u32, String, String) {
    let leases = leases_line.trim().parse::<u32>().unwrap_or(0);
    (
        leases,
        uptime_line.trim().to_string(),
        model_line.trim().to_string(),
    )
}

/// Read-only live gateway status over `sshpass` (DATACENTER-14): the gateway has
/// no mesh key, so this uses password auth like [`gateway_reboot`]. `host` must
/// already be validated as a plain IPv4.
///
/// Over one SSH session it gathers three newline-separated lines — DHCP lease
/// count, integer uptime seconds, and model — then [`parse_gateway_status`]
/// turns them into the reply triple. Returns `Err(<message>)` on a missing cred,
/// an SSH spawn/exit failure, or empty output.
fn gateway_status(req_body: Option<&str>) -> Result<(u32, String, String), String> {
    let Some(body) = req_body else {
        return Err("gateway-status: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("gateway-status: bad json: {e}"))?;

    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_ipv4(host) {
        return Err("host must be a plain IPv4 address".into());
    }

    let (user, pw) = unifi_cred().ok_or("no unifi cred in store")?;

    // One probe, three lines on stdout in a fixed order. Each sub-command is
    // best-effort and falls back so a single missing tool can't blank the whole
    // reply; the markers are literal so parsing stays positional.
    let remote = "grep -c . /run/dhcpd.leases 2>/dev/null || ip neigh | grep -c REACHABLE; \
         cat /proc/uptime | cut -d. -f1; \
         mca-cli-op info 2>/dev/null | head -1 || echo UniFi";

    let o = std::process::Command::new("sshpass")
        .args([
            "-p",
            &pw,
            "ssh",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=8",
            &format!("{user}@{host}"),
            remote,
        ])
        .output()
        .map_err(|e| format!("sshpass failed: {e}"))?;

    if !o.status.success() {
        let stderr = String::from_utf8_lossy(&o.stderr);
        let msg = stderr.trim();
        return if msg.is_empty() {
            Err("gateway status failed".into())
        } else {
            Err(msg.to_string())
        };
    }

    let stdout = String::from_utf8_lossy(&o.stdout);
    let mut lines = stdout.lines();
    let leases_line = lines.next().unwrap_or("");
    let uptime_line = lines.next().unwrap_or("");
    let model_line = lines.next().unwrap_or("");
    Ok(parse_gateway_status(leases_line, uptime_line, model_line))
}

/// DATACENTER-14 (Gateway tab) — one tofu-managed DHCP static reservation on the
/// EdgeRouter (a `managed_reservations` output entry, `name => {mac, ip}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct GatewayReservation {
    /// The reservation's name (the EdgeOS static-mapping name).
    pub name: String,
    /// The reserved MAC.
    pub mac: String,
    /// The reserved IPv4.
    pub ip: String,
}

/// DATACENTER-14 (Gateway tab) — one live DHCP lease on the EdgeRouter (a
/// `dhcp_leases` output entry, decoded from `ip => "mac|expiry|hostname"`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct GatewayLease {
    /// The leased IPv4.
    pub ip: String,
    /// The lessee's MAC.
    pub mac: String,
    /// The lease expiry (the EdgeOS-formatted timestamp string).
    pub expiry: String,
    /// The client hostname (may be empty when the client sent none).
    pub hostname: String,
}

/// Parse the `managed_reservations` tofu-output value (a JSON object
/// `name => {mac, ip}`) into a name-sorted reservation list. PURE.
///
/// `out` is the `.value` of the `tofu output -json` block for
/// `managed_reservations`. A missing/non-object value yields an empty list (the
/// workspace simply has no reservations, not an error).
#[must_use]
pub fn parse_reservations(out: &serde_json::Value) -> Vec<GatewayReservation> {
    let Some(map) = out.as_object() else {
        return Vec::new();
    };
    let mut rows: Vec<GatewayReservation> = map
        .iter()
        .map(|(name, v)| GatewayReservation {
            name: name.clone(),
            mac: v
                .get("mac")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            ip: v
                .get("ip")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// Parse the `dhcp_leases` tofu-output value (a JSON object `ip =>
/// "mac|expiry|hostname"`) into an IP-sorted lease list. PURE.
///
/// `out` is the `.value` of the `tofu output -json` block for `dhcp_leases`. Each
/// value is the pipe-joined `mac|expiry|hostname` the poll script emits; a value
/// with fewer fields fills the rest blank. A missing/non-object value yields an
/// empty list.
#[must_use]
pub fn parse_leases(out: &serde_json::Value) -> Vec<GatewayLease> {
    let Some(map) = out.as_object() else {
        return Vec::new();
    };
    let mut rows: Vec<GatewayLease> = map
        .iter()
        .map(|(ip, v)| {
            let raw = v.as_str().unwrap_or("");
            let mut parts = raw.splitn(3, '|');
            GatewayLease {
                ip: ip.clone(),
                mac: parts.next().unwrap_or("").trim().to_string(),
                expiry: parts.next().unwrap_or("").trim().to_string(),
                hostname: parts.next().unwrap_or("").trim().to_string(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.ip.cmp(&b.ip));
    rows
}

/// DATACENTER-14 (Gateway tab) — read the EdgeOS DHCP state from the `edgeos` tofu
/// workspace outputs: the tofu-managed static reservations + the live DHCP leases.
///
/// Runs `tofu output -json` in `infra/tofu/edgeos` (read-only — the live-lease
/// poll is an external-data read, and `managed_reservations` echoes the desired
/// state), parses the `managed_reservations` + `dhcp_leases` outputs, and returns
/// the two structured lists. Reservation CHANGES never go through this read — they
/// go through the tofu-gated `tofu-plan`/`tofu-apply` on the `edgeos` workspace.
///
/// # Errors
/// Returns `Err` on a spawn failure, a non-zero `tofu output` exit (e.g. no
/// state yet), or unparseable JSON.
fn gateway_dhcp(
    workgroup_root: &std::path::Path,
) -> Result<(Vec<GatewayReservation>, Vec<GatewayLease>), String> {
    let repo = workgroup_root.display();
    // `repo` is process-owned and the dir is a fixed literal, so this is not an
    // injection surface. `tofu output -json` is read-only.
    let script = format!("cd {repo}/infra/tofu/edgeos && tofu output -json 2>&1");
    let o = std::process::Command::new("bash")
        .args(["-lc", &script])
        .output()
        .map_err(|e| format!("gateway-dhcp exec failed: {e}"))?;
    if !o.status.success() {
        let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
        out.push_str(&String::from_utf8_lossy(&o.stderr));
        let msg = out.trim();
        return Err(if msg.is_empty() {
            "gateway-dhcp: tofu output failed".into()
        } else {
            msg.to_string()
        });
    }
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&o.stdout))
        .map_err(|e| format!("gateway-dhcp: bad tofu output json: {e}"))?;
    // `tofu output -json` shape: { "<name>": { "value": <v>, "type": .. }, .. }.
    let reservations = parse_reservations(
        v.get("managed_reservations")
            .and_then(|o| o.get("value"))
            .unwrap_or(&serde_json::Value::Null),
    );
    let leases = parse_leases(
        v.get("dhcp_leases")
            .and_then(|o| o.get("value"))
            .unwrap_or(&serde_json::Value::Null),
    );
    Ok((reservations, leases))
}

/// Run the DATACENTER-23 disaster-recovery backup (confirm-gated): shells out to
/// `automation/dr/dr-backup.sh` from the repo root, which dumps the recoverable
/// etcd state (`/tofu/state/*`, `/mcnf/secret/*`, `/mcnf/age-recipient`) into an
/// age-encrypted manifest and prints the output path on stdout.
///
/// Requires `{"confirm":true}`. On success returns the trimmed path the script
/// printed; on failure returns the script's stderr (or a generic message).
fn dr_backup(req_body: Option<&str>) -> Result<String, String> {
    let Some(body) = req_body else {
        return Err("dr-backup: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("dr-backup: bad json: {e}"))?;

    let confirm = req
        .get("confirm")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !confirm {
        return Err("dr-backup requires confirm:true".into());
    }

    // Repo-root-relative like the unifi-cred helper; the responder runs from the
    // repo root, and the script is read-only on etcd.
    let o = std::process::Command::new("bash")
        .args(["-lc", "automation/dr/dr-backup.sh"])
        .output()
        .map_err(|e| format!("dr-backup: spawn failed: {e}"))?;

    if o.status.success() {
        // The script prints ONLY the artifact path on stdout (the separate-key
        // reminder goes to stderr), so the trimmed stdout is the path.
        let path = String::from_utf8_lossy(&o.stdout);
        let path = path.trim();
        if path.is_empty() {
            Err("dr-backup: produced no output path".into())
        } else {
            Ok(path.to_string())
        }
    } else {
        let stderr = String::from_utf8_lossy(&o.stderr);
        let msg = stderr.trim();
        if msg.is_empty() {
            Err("dr-backup failed".into())
        } else {
            Err(msg.to_string())
        }
    }
}

/// LIGHTHOUSE-6 — run a remote command on a mesh node over the mesh key,
/// returning the process result. The lighthouse counterpart of [`ssh_xe_status`]
/// (same arg style + `BatchMode`/`ConnectTimeout` hardening), generalized off the
/// fixed `xe` target so it can drive `systemctl` on an anchor node.
fn ssh_run(key: &str, host: &str, remote: &str) -> std::io::Result<std::process::Output> {
    std::process::Command::new("ssh")
        .args([
            "-i",
            key,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            &format!("root@{host}"),
            remote,
        ])
        .output()
}

/// LIGHTHOUSE-6 — restart a lighthouse's core fabric units over the mesh key.
///
/// Body `{ "overlay_ip", "confirm": true }`. The `overlay_ip` MUST be a plain
/// dotted-quad ([`valid_ipv4`]) — checked BEFORE any SSH so it can never carry a
/// shell metacharacter into the remote command — and the destructive restart is
/// `confirm:true`-gated like [`gateway_reboot`]/[`dr_backup`].
///
/// **Transport-aware ordering (CRITICAL):** this SSH rides the *overlay* IP, i.e.
/// the Nebula tunnel that restarting `nebula` itself tears down — so the units
/// can't both be restarted with a normal blocking `systemctl restart` (bouncing
/// nebula would cut our own session and we'd misreport a healthy restart as a
/// failure). Instead:
///   1. `systemctl restart mackesd` runs **to completion** — it's the control
///      plane, NOT our SSH transport, so we get its real exit.
///   2. `nebula` is restarted with `systemctl --no-block`, which enqueues the
///      restart and returns *before* the overlay bounces. The command exit is the
///      honest "the restart was accepted" signal — we deliberately do not (and
///      cannot) observe nebula's post-bounce state over a tunnel we just dropped;
///      the card's live beacon re-greens on the next directory refresh once the
///      overlay is back.
/// Combining both into ONE remote shell keeps it to a single SSH round-trip on
/// the responder thread. `Ok(())` once the command returns zero.
fn lighthouse_restart(req_body: Option<&str>) -> Result<(), String> {
    let Some(body) = req_body else {
        return Err("lighthouse-restart: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("lighthouse-restart: bad json: {e}"))?;

    let confirm = req
        .get("confirm")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !confirm {
        return Err("lighthouse restart requires confirm:true".into());
    }

    let overlay_ip = req
        .get("overlay_ip")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_ipv4(overlay_ip) {
        return Err("overlay_ip must be a plain IPv4 address".into());
    }

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();

    // One round-trip: mackesd to completion (real exit), then nebula `--no-block`
    // so the overlay only bounces AFTER the command has returned (see the
    // transport-aware ordering note above). The literal verbs carry no
    // operator/untrusted input, so there's nothing to escape.
    let remote = "systemctl restart mackesd && systemctl --no-block restart nebula";
    match ssh_run(&key, overlay_ip, remote) {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                Err("lighthouse restart failed".into())
            } else {
                Err(format!("restart failed: {msg}"))
            }
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// LIGHTHOUSE-6 — promote a shadow anchor to mesh leader via the EXISTING
/// leader-lease force-take (the same primitive `mackesd take-leadership --force`
/// uses). Substrate-aware exactly like [`crate::ipc::directory::Directory`]'s
/// leader read: the etcd lease `force` when the coordination plane is configured
/// (endpoints present), else the fs lockfile [`crate::leader::force_take`].
///
/// Body `{ "node", "confirm": true }`. `confirm:true`-gated. **Idempotent guard
/// (§ the task's "refuse if already master"):** reads the current leader first
/// and refuses with an error if `node` already holds the lease, so a double-click
/// can't needlessly bump the epoch. Returns the bare hostname now leading.
///
/// Thin wrapper over [`promote_with_endpoints`] that supplies the node's real
/// configured etcd endpoints ([`crate::substrate::etcd::default_endpoints`]); the
/// inner fn takes them explicitly so a test stays hermetic (passes `&[]` for the
/// fs-lockfile path regardless of whether the host is provisioned onto etcd).
fn lighthouse_promote(
    workgroup_root: &std::path::Path,
    req_body: Option<&str>,
) -> Result<String, String> {
    promote_with_endpoints(
        workgroup_root,
        &crate::substrate::etcd::default_endpoints(),
        req_body,
    )
}

/// LIGHTHOUSE-6 — the promote core, taking the etcd `endpoints` explicitly (empty
/// = off the coordination plane, use the fs lockfile). See [`lighthouse_promote`].
fn promote_with_endpoints(
    workgroup_root: &std::path::Path,
    etcd_endpoints: &[String],
    req_body: Option<&str>,
) -> Result<String, String> {
    let Some(body) = req_body else {
        return Err("lighthouse-promote: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("lighthouse-promote: bad json: {e}"))?;

    let confirm = req
        .get("confirm")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !confirm {
        return Err("lighthouse promote requires confirm:true".into());
    }

    // The Workbench passes the bare directory hostname; the cluster's lease
    // node_id convention is `peer:<host>` (see `default_node_id`, the
    // leader-election campaign, and `take-leadership --force`). Normalize to that
    // canonical form so the force-taken lease is byte-identical to what the live
    // election loop would next write — otherwise the lease diverges and the next
    // renewal by the real leader churns it. Accept either spelling on the wire.
    // `bare` drives the idempotent guard + the reply (readers strip `peer:`).
    let node = req
        .get("node")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    let bare = node.strip_prefix("peer:").unwrap_or(node);
    if bare.is_empty() {
        return Err("lighthouse-promote: missing `node`".into());
    }
    let canonical = format!("peer:{bare}");

    // Idempotent guard: who leads now? Compare on the bare hostname (the lease
    // node_id may carry the `peer:` prefix), matching the directory responder.
    let current = if etcd_endpoints.is_empty() {
        crate::leader::read_current_lease(&workgroup_root.join(".mackesd-leader.lock"))
            .map(|l| l.node_id)
    } else {
        crate::substrate::leader::current_leader_blocking(etcd_endpoints).map(|l| l.node_id)
    };
    if let Some(cur) = &current {
        let cur_bare = cur.strip_prefix("peer:").unwrap_or(cur);
        if cur_bare == bare {
            return Err(format!("{bare} is already the master"));
        }
    }

    // Force-take leadership for the named node via the EXISTING primitive (the
    // same one `mackesd take-leadership --force` uses): the fs lockfile force-take
    // off-substrate, else the substrate-aware blocking etcd `force`. Writes the
    // canonical `peer:<host>` node_id.
    let lease = if etcd_endpoints.is_empty() {
        crate::leader::force_take(&workgroup_root.join(".mackesd-leader.lock"), &canonical)
            .map_err(|e| format!("promote: {e}"))?
    } else {
        crate::substrate::leader::force_blocking(etcd_endpoints, &canonical)
            .map_err(|e| format!("promote: {e}"))?
    };

    Ok(lease
        .node_id
        .strip_prefix("peer:")
        .unwrap_or(&lease.node_id)
        .to_string())
}

/// Resolve `dom0`'s own host uuid over SSH (`xe host-list params=uuid
/// --minimal`, first value), validating the dom0 against the allow-list FIRST so
/// an attacker-supplied host never reaches SSH. Shared by host-power (the
/// destructive path) and the host-impact / host-pool reads so all three resolve
/// the uuid identically. Returns `Ok((key, uuid))` (the ssh key is returned so
/// the caller's follow-up `xe` calls reuse it) or `Err(<message>)`.
fn resolve_dom0_uuid(dom0: &str) -> Result<(String, String), String> {
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return Err("dom0 not in allowed set".into());
    }
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let uuid = match ssh_xe_status(&key, dom0, "xe host-list params=uuid --minimal") {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            out.trim()
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            return Err(if msg.is_empty() {
                "host-list failed".into()
            } else {
                msg.to_string()
            });
        }
        Err(e) => return Err(format!("ssh failed: {e}")),
    };
    if uuid.is_empty() {
        return Err("host uuid not found".into());
    }
    // The remote uuid is XAPI-generated; guard anyway before interpolation.
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("host uuid contains invalid characters".into());
    }
    Ok((key, uuid))
}

/// DATACENTER-10 — count the running guests resident on `dom0`, the impact-preview
/// number the panel shows before a drain / reboot / shutdown ("N running VM(s)
/// will be migrated/stopped"). Read-only: resolves the host uuid then runs
/// `xe vm-list resident-on=<uuid> power-state=running --minimal`. Body
/// `{ "dom0" }`. Returns the count or `Err(<message>)`.
fn host_impact(req_body: Option<&str>) -> Result<usize, String> {
    let Some(body) = req_body else {
        return Err("host-impact: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("host-impact: bad json: {e}"))?;
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let (key, uuid) = resolve_dom0_uuid(dom0)?;
    let remote = format!("xe vm-list resident-on={uuid} power-state=running --minimal");
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => Ok(parse_running_count(&String::from_utf8_lossy(&o.stdout))),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            Err(if msg.is_empty() {
                "vm-list failed".into()
            } else {
                msg.to_string()
            })
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// DATACENTER-10 — read `dom0`'s pool placement (membership / master), the data
/// the Hosts tab's pool panel renders. Read-only: resolves the host uuid, then
/// reads the pool's name + master uuid over one SSH session, and folds them with
/// [`parse_pool`] into `(pool_name, master_uuid, is_master)`. Body `{ "dom0" }`.
fn host_pool(req_body: Option<&str>) -> Result<(String, String, bool), String> {
    let Some(body) = req_body else {
        return Err("host-pool: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("host-pool: bad json: {e}"))?;
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let (key, uuid) = resolve_dom0_uuid(dom0)?;
    // Two positional minimal reads over ONE ssh round-trip (literal markers so the
    // parse stays positional even when a field is empty). The literal verbs carry
    // no operator input, so there's nothing to escape.
    let remote = "xe pool-list params=name-label --minimal; echo '@@'; \
         xe pool-list params=master --minimal";
    match ssh_xe_status(&key, dom0, remote) {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let mut parts = stdout.split("@@");
            let pool_line = parts.next().unwrap_or("");
            let master_line = parts.next().unwrap_or("");
            Ok(parse_pool(pool_line, master_line, &uuid))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            Err(if msg.is_empty() {
                "pool-list failed".into()
            } else {
                msg.to_string()
            })
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// DATACENTER-13 (Network tab) — read a dom0's L2 inventory (networks, PIFs/NICs,
/// and the VLAN tags) over ONE SSH round-trip, the data the Network tab's L2 view
/// renders. Read-only: validates the dom0 against the allow-list FIRST (via
/// [`resolve_dom0_uuid`]), then runs two minimal `xe` reads separated by a literal
/// `@@` marker so the parse stays positional. Body `{ "dom0" }`. Returns
/// `(nets, pifs)` or `Err(<message>)`.
fn host_net(req_body: Option<&str>) -> Result<(Vec<NetInfo>, Vec<PifInfo>), String> {
    let Some(body) = req_body else {
        return Err("host-net: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("host-net: bad json: {e}"))?;
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let (key, _uuid) = resolve_dom0_uuid(dom0)?;
    // Two pipe-delimited reads over one ssh session. The `xe` verbs + param names
    // are all literal — no operator input reaches the remote shell here, so there
    // is nothing to escape. `@@` separates the two blocks for positional parsing.
    let remote = "for u in $(xe network-list params=uuid --minimal | tr , ' '); do \
         echo \"$u|$(xe network-param-get uuid=$u param-name=name-label)|\
$(xe network-param-get uuid=$u param-name=bridge)|\
$(xe network-param-get uuid=$u param-name=MTU)\"; done; \
         echo '@@'; \
         for p in $(xe pif-list params=uuid --minimal | tr , ' '); do \
         echo \"$p|$(xe pif-param-get uuid=$p param-name=device)|\
$(xe pif-param-get uuid=$p param-name=MAC)|\
$(xe pif-param-get uuid=$p param-name=VLAN)|\
$(xe pif-param-get uuid=$p param-name=carrier)|\
$(xe pif-param-get uuid=$p param-name=management)|\
$(xe pif-param-get uuid=$p param-name=network-uuid)\"; done";
    match ssh_xe_status(&key, dom0, remote) {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let mut parts = stdout.split("@@");
            let net_block = parts.next().unwrap_or("");
            let pif_block = parts.next().unwrap_or("");
            Ok((parse_nets(net_block), parse_pifs(pif_block)))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            Err(if msg.is_empty() {
                "network read failed".into()
            } else {
                msg.to_string()
            })
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// DATACENTER-13 (Network tab) — create a VLAN sub-interface on a dom0's trunk
/// PIF, bound to a fresh pool-wide network. Confirm-gated + dom0-allow-listed.
/// Body `{ "dom0", "pif", "vlan", "network_name", "confirm": true }`. Two `xe`
/// steps over the mesh key: `network-create` (yields the new network uuid), then
/// `pool-vlan-create` with that uuid substituted (and re-validated as XAPI-shaped
/// before interpolation). Returns the new network uuid or `Err(<message>)`.
fn host_vlan_create(req_body: Option<&str>) -> Result<String, String> {
    let Some(body) = req_body else {
        return Err("host-vlan-create: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("host-vlan-create: bad json: {e}"))?;
    let confirm = req
        .get("confirm")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !confirm {
        return Err("vlan-create requires confirm:true".into());
    }
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let pif = req
        .get("pif")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let vlan = req
        .get("vlan")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(-1);
    let network_name = req
        .get("network_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // Validate every operator-supplied field + build the command recipe BEFORE any
    // SSH — an invalid pif/vlan/name never reaches the network.
    let (create_net, create_vlan) = vlan_create_commands(pif, vlan, network_name)?;
    // Allow-list check + key (the uuid return is unused: we act on the PIF, not the
    // host uuid, but resolving still proves the dom0 is reachable + allow-listed).
    let (key, _uuid) = resolve_dom0_uuid(dom0)?;
    // Step 1 — create the backing network; capture its uuid.
    let net_uuid = match ssh_xe_status(&key, dom0, &create_net) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            return Err(if msg.is_empty() {
                "network-create failed".into()
            } else {
                msg.to_string()
            });
        }
        Err(e) => return Err(format!("ssh failed: {e}")),
    };
    if net_uuid.is_empty() {
        return Err("network-create returned no uuid".into());
    }
    // The uuid is XAPI-generated; guard anyway before interpolation into step 2.
    if !net_uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("new network uuid contains invalid characters".into());
    }
    // Step 2 — bind the VLAN to that network.
    let vlan_cmd = create_vlan.replace("@NET@", &net_uuid);
    match ssh_xe_status(&key, dom0, &vlan_cmd) {
        Ok(o) if o.status.success() => Ok(net_uuid),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            Err(if msg.is_empty() {
                "pool-vlan-create failed".into()
            } else {
                msg.to_string()
            })
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// DATACENTER-13 (Network tab) — build the reply for the two network verbs
/// (`host-net` read + `host-vlan-create`), factored out of [`build_reply`] so the
/// top-level dispatcher stays a thin match. Returns the JSON reply string.
fn net_build_reply(verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    match verb {
        "host-net" => match host_net(req_body) {
            Ok((nets, pifs)) => json!({ "ok": true, "nets": nets, "pifs": pifs }).to_string(),
            Err(m) => err(m),
        },
        "host-vlan-create" => match host_vlan_create(req_body) {
            Ok(network) => json!({ "ok": true, "network": network }).to_string(),
            Err(m) => err(m),
        },
        other => err(format!("net_build_reply: unknown verb {other}")),
    }
}

/// Strict MAC validation (lowercase colon form `aa:bb:cc:dd:ee:ff`). The MAC is
/// the only operator-influenced input that flows into the `router/<mac>` secret
/// key, so it must be hex+colons ONLY — no shell/path metacharacters can reach
/// `mcnf-secret.sh`.
#[must_use]
pub fn valid_mac(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// ROUTER-3 (action layer) — seal a per-appliance router credential into the
/// MESH secret store under `router/<mac>`. Request `{"mac":"aa:bb:..","cred":"user:pass"}`.
/// The cred is fed to `mcnf-secret.sh put` via **STDIN** (never argv/`ps`); the
/// MAC is strictly validated so it can't inject into the secret name.
fn router_seal_cred(req_body: Option<&str>) -> Result<(), String> {
    use std::io::Write;
    let body = req_body.ok_or("router-seal-cred: missing body")?;
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("router-seal-cred: bad json: {e}"))?;
    let mac = v
        .get("mac")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let cred = v
        .get("cred")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_mac(&mac) {
        return Err(format!("router-seal-cred: invalid MAC {mac:?}"));
    }
    if cred.trim().is_empty() {
        return Err("router-seal-cred: empty cred".into());
    }
    // mac is hex+colons (valid_mac), safe single-quoted into the helper arg.
    let cmd = format!("automation/secrets/mcnf-secret.sh put 'router/{mac}'");
    let mut child = std::process::Command::new("bash")
        .args(["-lc", &cmd])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("router-seal-cred: spawn mcnf-secret.sh: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("router-seal-cred: no stdin")?
        .write_all(cred.as_bytes())
        .map_err(|e| format!("router-seal-cred: write cred: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("router-seal-cred: wait: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "router-seal-cred: put failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// One ephemeral test VM (DATACENTER-21): name + IP from `farm-testbed.sh ips`.
#[derive(serde::Serialize)]
struct TestVm {
    name: String,
    ip: String,
}

/// Parse `farm-testbed.sh ips` output ("name ip" per line) into rows. Pure.
#[must_use]
fn parse_testbed_ips(stdout: &str) -> Vec<TestVm> {
    stdout
        .lines()
        .filter_map(|l| {
            let mut p = l.split_whitespace();
            let name = p.next()?.to_string();
            if name.is_empty() {
                return None;
            }
            let ip = p.next().unwrap_or("").to_string();
            Some(TestVm { name, ip })
        })
        .collect()
}

/// Run a repo-root-relative helper script (the responder's CWD is the repo root,
/// like the unifi-cred path). Returns stdout on success, a trimmed error else.
fn run_repo_script(cmd: &str) -> Result<String, String> {
    let o = std::process::Command::new("bash")
        .args(["-lc", cmd])
        .output()
        .map_err(|e| format!("exec failed: {e}"))?;
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    if o.status.success() {
        Ok(out)
    } else {
        let err = String::from_utf8_lossy(&o.stderr);
        let msg = if out.trim().is_empty() {
            err.trim()
        } else {
            out.trim()
        };
        Err(if msg.is_empty() {
            "script failed".into()
        } else {
            msg.to_string()
        })
    }
}

/// DATACENTER-21 — list the running ephemeral test VMs (`farm-testbed.sh ips`).
fn testbed_list() -> Result<Vec<TestVm>, String> {
    Ok(parse_testbed_ips(&run_repo_script(
        "automation/testbed/farm-testbed.sh ips",
    )?))
}

/// DATACENTER-21 — run the build-farm autoscale reconcile (writes the demand-based
/// shape + `tofu plan`; NEVER applies — operator-gated). Returns the plan/decision text.
fn farm_scale() -> Result<String, String> {
    run_repo_script("install-helpers/farm-autoscale.sh")
}

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(svc: &HostOpsService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    match verb {
        "host-power" => {}
        // DATACENTER-10 — impact preview: how many running guests a drain/reboot/
        // shutdown of this host would move/stop.
        "host-impact" => {
            return match host_impact(req_body) {
                Ok(running) => json!({ "ok": true, "running": running }).to_string(),
                Err(m) => err(m),
            };
        }
        // DATACENTER-10 — pool placement read (membership / master).
        "host-pool" => {
            return match host_pool(req_body) {
                Ok((pool, master, is_master)) => json!({
                    "ok": true,
                    "pool": pool,
                    "master": master,
                    "is_master": is_master,
                })
                .to_string(),
                Err(m) => err(m),
            };
        }
        "gateway-reboot" => {
            return match gateway_reboot(req_body) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(m) => err(m),
            };
        }
        // ROUTER-3 — seal a per-appliance router credential (router/<mac>) into
        // the mesh secret store; cred via stdin, never argv.
        "router-seal-cred" => {
            return match router_seal_cred(req_body) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(m) => err(m),
            };
        }
        // DATACENTER-21 — list ephemeral test VMs.
        "testbed-list" => {
            return match testbed_list() {
                Ok(vms) => json!({ "ok": true, "vms": vms }).to_string(),
                Err(m) => err(m),
            };
        }
        // DATACENTER-21 — autoscale reconcile (plan only, never applies).
        "farm-scale" => {
            return match farm_scale() {
                Ok(plan) => json!({ "ok": true, "plan": plan }).to_string(),
                Err(m) => err(m),
            };
        }
        // LIGHTHOUSE-6 — restart the anchor's core fabric units over the mesh key.
        "lighthouse-restart" => {
            return match lighthouse_restart(req_body) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(m) => err(m),
            };
        }
        // LIGHTHOUSE-6 — promote a shadow anchor to leader (idempotent).
        "lighthouse-promote" => {
            return match lighthouse_promote(&svc.workgroup_root, req_body) {
                Ok(leader) => json!({ "ok": true, "leader": leader }).to_string(),
                Err(m) => err(m),
            };
        }
        "dr-backup" => {
            return match dr_backup(req_body) {
                Ok(path) => json!({ "ok": true, "path": path }).to_string(),
                Err(m) => err(m),
            };
        }
        "gateway-status" => {
            return match gateway_status(req_body) {
                Ok((leases, uptime, model)) => json!({
                    "ok": true,
                    "leases": leases,
                    "uptime": uptime,
                    "model": model,
                })
                .to_string(),
                Err(m) => err(m),
            };
        }
        // DATACENTER-14 — the Gateway tab's EdgeOS DHCP read: the tofu-managed
        // static reservations + the live DHCP leases, read from the edgeos tofu
        // workspace outputs (read-only; reservation CHANGES go through the
        // tofu-gated apply, never a blind apply from the GUI).
        "gateway-dhcp" => {
            return match gateway_dhcp(&svc.workgroup_root) {
                Ok((reservations, leases)) => json!({
                    "ok": true,
                    "reservations": reservations,
                    "leases": leases,
                })
                .to_string(),
                Err(m) => err(m),
            };
        }
        // DATACENTER-13 — the Network tab's L2 read + VLAN create (factored out so
        // `build_reply` stays a thin dispatcher; the two verbs reply-shape together).
        "host-net" | "host-vlan-create" => return net_build_reply(verb, req_body),
        _ => return err("unknown dc verb".into()),
    }
    let Some(body) = req_body else {
        return err("host-power: missing request body".into());
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("host-power: bad json: {e}")),
    };
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let op = req
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // DESTRUCTIVE: reboot / shutdown / evacuate bounce the host or live-migrate
    // every guest off it. Refuse unless the caller explicitly confirms — the same
    // fail-closed typed-confirm contract as `vm-delete` / `vdi-detach` /
    // `tofu-destroy`, and the §8/§9-aligned way to guard the dangerous ops without
    // RBAC. Checked BEFORE the op→verb map and the dom0 allow-list so an
    // unconfirmed destructive op never reaches the network. The reversible
    // maintenance toggles are NOT gated.
    if host_power_is_destructive(op)
        && req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return err(format!("host {op} requires confirm:true"));
    }

    // Map the op to its `xe` verb sequence BEFORE any SSH — an unknown op never
    // reaches the network.
    let verbs = match host_power_commands(op) {
        Ok(v) => v,
        Err(e) => return err(e),
    };

    // SECURITY + resolution in one place (shared with the host-impact / host-pool
    // reads): allow-list check FIRST, then resolve+validate the host uuid. Never
    // SSHes an attacker-supplied host.
    let (key, uuid) = match resolve_dom0_uuid(dom0) {
        Ok(pair) => pair,
        Err(m) => return err(m),
    };

    // Run each verb in sequence; stop at the first failure.
    for v in &verbs {
        let remote = format!("xe {v} host={uuid}");
        match ssh_xe_status(&key, dom0, &remote) {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let msg = stderr.trim();
                return if msg.is_empty() {
                    err(format!("{v} failed"))
                } else {
                    err(msg.to_string())
                };
            }
            Err(e) => return err(format!("ssh failed: {e}")),
        }
    }
    json!({ "ok": true }).to_string()
}

/// Run the host-ops Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &HostOpsService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &HostOpsService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "host-ops responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "host-ops responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("host-power"), "action/dc/host-power");
        assert_eq!(action_topic("host-impact"), "action/dc/host-impact");
        assert_eq!(action_topic("host-pool"), "action/dc/host-pool");
        assert_eq!(action_topic("gateway-reboot"), "action/dc/gateway-reboot");
        assert_eq!(action_topic("dr-backup"), "action/dc/dr-backup");
        assert_eq!(action_topic("gateway-status"), "action/dc/gateway-status");
        assert_eq!(
            action_topic("lighthouse-restart"),
            "action/dc/lighthouse-restart"
        );
        assert_eq!(
            action_topic("lighthouse-promote"),
            "action/dc/lighthouse-promote"
        );
        assert_eq!(action_topic("host-net"), "action/dc/host-net");
        assert_eq!(
            action_topic("host-vlan-create"),
            "action/dc/host-vlan-create"
        );
        assert!(ACTION_VERBS.contains(&"host-power"));
        assert!(ACTION_VERBS.contains(&"host-impact"));
        assert!(ACTION_VERBS.contains(&"host-pool"));
        assert!(ACTION_VERBS.contains(&"gateway-reboot"));
        assert!(ACTION_VERBS.contains(&"dr-backup"));
        assert!(ACTION_VERBS.contains(&"gateway-status"));
        assert!(ACTION_VERBS.contains(&"lighthouse-restart"));
        assert!(ACTION_VERBS.contains(&"lighthouse-promote"));
        assert!(ACTION_VERBS.contains(&"host-net"));
        assert!(ACTION_VERBS.contains(&"host-vlan-create"));
    }

    #[test]
    fn router_seal_cred_verb_and_guards() {
        assert!(ACTION_VERBS.contains(&"router-seal-cred"));
        assert_eq!(
            action_topic("router-seal-cred"),
            "action/dc/router-seal-cred"
        );
        // strict MAC gate — only hex+colons reach the secret name
        assert!(valid_mac("46:6a:7c:96:e8:aa"));
        assert!(!valid_mac("46:6a:7c:96:e8")); // too short
        assert!(!valid_mac("zz:6a:7c:96:e8:aa")); // non-hex
        assert!(!valid_mac("46:6a:7c:96:e8:aa; rm -rf /")); // injection attempt
        // handler rejects bad input BEFORE shelling mcnf-secret.sh
        assert!(router_seal_cred(None).is_err());
        assert!(router_seal_cred(Some("{bad json")).is_err());
        assert!(router_seal_cred(Some(r#"{"mac":"nothex","cred":"u:p"}"#)).is_err());
        assert!(router_seal_cred(Some(r#"{"mac":"46:6a:7c:96:e8:aa","cred":"  "}"#)).is_err());
    }

    #[test]
    fn testbed_farmscale_verbs_and_parse() {
        assert!(ACTION_VERBS.contains(&"testbed-list"));
        assert!(ACTION_VERBS.contains(&"farm-scale"));
        assert_eq!(action_topic("farm-scale"), "action/dc/farm-scale");
        let vms = parse_testbed_ips("mcnf-test-1 172.20.0.61\nmcnf-test-2 172.20.0.62\n\n   \n");
        assert_eq!(vms.len(), 2);
        assert_eq!(vms[0].name, "mcnf-test-1");
        assert_eq!(vms[0].ip, "172.20.0.61");
        // a name with no IP still parses (ip empty), blank lines skipped
        let solo = parse_testbed_ips("solo\n");
        assert_eq!(solo.len(), 1);
        assert_eq!(solo[0].ip, "");
    }

    #[test]
    fn parse_pifs_decodes_devices_vlans_and_flags() {
        let raw = "uuid-0|eth0|aa:bb:cc:dd:ee:ff|-1|true|true|net-0\n\
                   uuid-1|eth0.100|aa:bb:cc:dd:ee:ff|100|true|false|net-1\n\
                   |skip-empty-uuid|x|0|false|false|x";
        let pifs = parse_pifs(raw);
        assert_eq!(pifs.len(), 2);
        assert_eq!(pifs[0].device, "eth0");
        assert_eq!(pifs[0].vlan, -1);
        assert!(pifs[0].carrier && pifs[0].management);
        assert_eq!(pifs[0].network, "net-0");
        assert_eq!(pifs[1].device, "eth0.100");
        assert_eq!(pifs[1].vlan, 100);
        assert!(!pifs[1].management);
    }

    #[test]
    fn parse_nets_decodes_bridge_and_mtu() {
        let raw = "net-0|Pool-wide network|xenbr0|1500\n\
                   net-1|Internal|xenbr1|\n\
                   |skip||";
        let nets = parse_nets(raw);
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[0].bridge, "xenbr0");
        assert_eq!(nets[0].mtu, 1500);
        // Unparseable MTU defaults to 0, not an error.
        assert_eq!(nets[1].mtu, 0);
    }

    #[test]
    fn vlan_create_commands_validates_and_builds() {
        // Happy path: a XAPI-shaped PIF uuid + a valid tag + a clean name.
        let (net, vlan) =
            vlan_create_commands("0a1b-2c3d", 100, "vlan100").expect("valid vlan-create");
        assert!(net.contains("network-create name-label=vlan100"), "{net}");
        assert!(
            vlan.contains("pool-vlan-create pif-uuid=0a1b-2c3d vlan=100 network-uuid=@NET@"),
            "{vlan}"
        );
        // A metachar-bearing PIF never builds a command.
        assert!(vlan_create_commands("0a1b; reboot", 100, "vlan100").is_err());
        // Out-of-range tags are rejected.
        assert!(vlan_create_commands("0a1b", 0, "v").is_err());
        assert!(vlan_create_commands("0a1b", 4095, "v").is_err());
        // A metachar-bearing network name never builds a command.
        assert!(vlan_create_commands("0a1b", 100, "v;rm -rf /").is_err());
    }

    #[test]
    fn host_vlan_create_requires_confirm_true() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "dom0": "172.20.0.9", "pif": "0a1b", "vlan": 100,
                           "network_name": "vlan100" })
        .to_string();
        let r = build_reply(&s, "host-vlan-create", Some(&body));
        assert!(r.contains("vlan-create requires confirm:true"), "{r}");
    }

    #[test]
    fn host_vlan_create_rejects_bad_fields_before_ssh() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // A metachar-bearing pif is rejected before any allow-list/SSH step.
        let body = json!({ "dom0": "172.20.0.9", "pif": "0a1b; reboot", "vlan": 100,
                           "network_name": "vlan100", "confirm": true })
        .to_string();
        let r = build_reply(&s, "host-vlan-create", Some(&body));
        assert!(r.contains("pif contains invalid characters"), "{r}");
    }

    #[test]
    fn host_net_missing_body_errors() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "host-net", None);
        assert!(r.contains("missing request body"), "{r}");
    }

    #[test]
    fn lighthouse_restart_requires_confirm_true() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // confirm omitted — must be rejected BEFORE any SSH.
        let body = json!({ "overlay_ip": "10.42.0.5" }).to_string();
        let r = build_reply(&s, "lighthouse-restart", Some(&body));
        assert!(
            r.contains("lighthouse restart requires confirm:true"),
            "{r}"
        );
        // confirm:false — same gate.
        let body = json!({ "overlay_ip": "10.42.0.5", "confirm": false }).to_string();
        let r = build_reply(&s, "lighthouse-restart", Some(&body));
        assert!(
            r.contains("lighthouse restart requires confirm:true"),
            "{r}"
        );
    }

    #[test]
    fn lighthouse_restart_rejects_bad_overlay_ip_before_ssh() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // A shell-metachar-bearing "ip" is rejected before any remote exec.
        let body = json!({ "overlay_ip": "10.42.0.5; reboot", "confirm": true }).to_string();
        let r = build_reply(&s, "lighthouse-restart", Some(&body));
        assert!(r.contains("overlay_ip must be a plain IPv4 address"), "{r}");
        // A hostname (non-IPv4) is rejected too.
        let body = json!({ "overlay_ip": "anvil.mesh", "confirm": true }).to_string();
        let r = build_reply(&s, "lighthouse-restart", Some(&body));
        assert!(r.contains("overlay_ip must be a plain IPv4 address"), "{r}");
    }

    #[test]
    fn lighthouse_restart_missing_body_errors() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "lighthouse-restart", None);
        assert!(r.contains("missing request body"), "{r}");
    }

    #[test]
    fn lighthouse_promote_requires_confirm_true() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "node": "anvil" }).to_string();
        let r = build_reply(&s, "lighthouse-promote", Some(&body));
        assert!(
            r.contains("lighthouse promote requires confirm:true"),
            "{r}"
        );
    }

    #[test]
    fn lighthouse_promote_missing_node_errors() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "confirm": true }).to_string();
        let r = build_reply(&s, "lighthouse-promote", Some(&body));
        assert!(r.contains("missing `node`"), "{r}");
    }

    #[test]
    fn lighthouse_promote_refuses_when_node_already_master() {
        // Stand up an fs leader lockfile with `anvil` already holding the lease,
        // then a promote of `anvil` must refuse idempotently. Drives the inner
        // `promote_with_endpoints` with `&[]` so the fs path is taken regardless
        // of whether the test host happens to be provisioned onto etcd (hermetic).
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        crate::leader::force_take(&root.join(".mackesd-leader.lock"), "anvil")
            .expect("seed leader lease");
        let body = json!({ "node": "anvil", "confirm": true }).to_string();
        let r = promote_with_endpoints(&root, &[], Some(&body));
        assert_eq!(r, Err("anvil is already the master".to_string()), "{r:?}");
    }

    #[test]
    fn lighthouse_promote_force_takes_for_a_shadow() {
        // With `anvil` leading, promoting the shadow `forge` must succeed and
        // report `forge` now leads (fs lockfile path; `&[]` endpoints, hermetic).
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        crate::leader::force_take(&root.join(".mackesd-leader.lock"), "anvil")
            .expect("seed leader lease");
        let body = json!({ "node": "forge", "confirm": true }).to_string();
        let leader = promote_with_endpoints(&root, &[], Some(&body)).expect("promote ok");
        // The reply is the BARE hostname for display.
        assert_eq!(leader, "forge");
        // But the lockfile records the canonical `peer:forge` lease node_id, so
        // it's byte-identical to what the live election loop next writes.
        let lease = crate::leader::read_current_lease(&root.join(".mackesd-leader.lock"))
            .expect("lease after promote");
        assert_eq!(lease.node_id, "peer:forge");
    }

    #[test]
    fn lighthouse_promote_accepts_a_prefixed_node_on_the_wire() {
        // A caller that passes the already-`peer:`-prefixed node id gets the same
        // canonical lease + bare reply (no double-prefix).
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        crate::leader::force_take(&root.join(".mackesd-leader.lock"), "peer:anvil")
            .expect("seed leader lease");
        let body = json!({ "node": "peer:forge", "confirm": true }).to_string();
        let leader = promote_with_endpoints(&root, &[], Some(&body)).expect("promote ok");
        assert_eq!(leader, "forge");
        let lease = crate::leader::read_current_lease(&root.join(".mackesd-leader.lock"))
            .expect("lease after promote");
        assert_eq!(lease.node_id, "peer:forge");
    }

    #[test]
    fn lighthouse_promote_strips_peer_prefix_in_idempotent_guard() {
        // The lease node_id may carry the `peer:` prefix; the guard compares bare
        // hostnames, so promoting `anvil` when `peer:anvil` leads still refuses.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        crate::leader::force_take(&root.join(".mackesd-leader.lock"), "peer:anvil")
            .expect("seed leader lease");
        let body = json!({ "node": "anvil", "confirm": true }).to_string();
        let r = promote_with_endpoints(&root, &[], Some(&body));
        assert_eq!(r, Err("anvil is already the master".to_string()), "{r:?}");
    }

    #[test]
    fn parse_gateway_status_parses_triple() {
        let (leases, uptime, model) =
            parse_gateway_status("42\n", " 99887 ", "  UniFi Dream Machine \n");
        assert_eq!(leases, 42);
        assert_eq!(uptime, "99887");
        assert_eq!(model, "UniFi Dream Machine");
    }

    #[test]
    fn parse_gateway_status_defaults_lease_count_to_zero() {
        // empty / non-numeric lease line → 0, the other fields still trim.
        let (leases, uptime, model) = parse_gateway_status("", "0", "UniFi");
        assert_eq!(leases, 0);
        assert_eq!(uptime, "0");
        assert_eq!(model, "UniFi");
        let (leases, _, _) = parse_gateway_status("not-a-number", "", "");
        assert_eq!(leases, 0);
    }

    #[test]
    fn gateway_status_rejects_bad_host_before_ssh() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "host": "1.2.3.4; reboot" }).to_string();
        let r = build_reply(&s, "gateway-status", Some(&body));
        assert!(r.contains("host must be a plain IPv4 address"), "{r}");
    }

    #[test]
    fn gateway_status_missing_body_errors() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "gateway-status", None);
        assert!(r.contains("missing request body"), "{r}");
    }

    // ── DATACENTER-14 — Gateway tab (EdgeOS DHCP) ────────────────────────────

    #[test]
    fn gateway_dhcp_verb_is_served() {
        assert!(ACTION_VERBS.contains(&"gateway-dhcp"));
        assert_eq!(action_topic("gateway-dhcp"), "action/dc/gateway-dhcp");
    }

    #[test]
    fn parse_reservations_sorts_by_name() {
        let v = json!({
            "rocky9-kvm1": { "mac": "00:23:24:c2:0f:1c", "ip": "172.20.145.193" },
            "mcnf-a":      { "mac": "f2:f2:0b:c5:dc:00", "ip": "172.20.121.10" },
        });
        let rows = parse_reservations(&v);
        assert_eq!(rows.len(), 2);
        // Name-sorted: mcnf-a before rocky9-kvm1.
        assert_eq!(rows[0].name, "mcnf-a");
        assert_eq!(rows[0].mac, "f2:f2:0b:c5:dc:00");
        assert_eq!(rows[0].ip, "172.20.121.10");
        assert_eq!(rows[1].name, "rocky9-kvm1");
    }

    #[test]
    fn parse_reservations_empty_or_non_object() {
        assert!(parse_reservations(&serde_json::Value::Null).is_empty());
        assert!(parse_reservations(&json!([])).is_empty());
        assert!(parse_reservations(&json!({})).is_empty());
    }

    #[test]
    fn parse_leases_decodes_pipe_fields_and_sorts_by_ip() {
        let v = json!({
            "172.20.145.33": "2c:54:91:0d:fc:30|2026/06/25 10:00:00|xbox",
            "172.20.121.10": "f2:f2:0b:c5:dc:00|2026/06/25 09:00:00|mcnf-a",
        });
        let rows = parse_leases(&v);
        assert_eq!(rows.len(), 2);
        // IP-sorted (string sort): .121.10 before .145.33.
        assert_eq!(rows[0].ip, "172.20.121.10");
        assert_eq!(rows[0].mac, "f2:f2:0b:c5:dc:00");
        assert_eq!(rows[0].expiry, "2026/06/25 09:00:00");
        assert_eq!(rows[0].hostname, "mcnf-a");
        assert_eq!(rows[1].ip, "172.20.145.33");
        assert_eq!(rows[1].hostname, "xbox");
    }

    #[test]
    fn parse_leases_tolerates_missing_fields() {
        // A lease value with no hostname (or no expiry) fills the rest blank.
        let v = json!({ "10.0.0.5": "aa:bb:cc:dd:ee:ff|2026/01/01 00:00:00" });
        let rows = parse_leases(&v);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(rows[0].expiry, "2026/01/01 00:00:00");
        assert_eq!(rows[0].hostname, "");
        // Empty / non-object → empty.
        assert!(parse_leases(&serde_json::Value::Null).is_empty());
        assert!(parse_leases(&json!("nope")).is_empty());
    }

    #[test]
    fn dr_backup_requires_confirm_true() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // confirm omitted — must be rejected BEFORE any backup is attempted.
        let body = json!({}).to_string();
        let r = build_reply(&s, "dr-backup", Some(&body));
        assert!(r.contains("dr-backup requires confirm:true"), "{r}");
        // confirm:false — same gate.
        let body = json!({ "confirm": false }).to_string();
        let r = build_reply(&s, "dr-backup", Some(&body));
        assert!(r.contains("dr-backup requires confirm:true"), "{r}");
    }

    #[test]
    fn dr_backup_missing_body_errors() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "dr-backup", None);
        assert!(r.contains("missing request body"), "{r}");
    }

    #[test]
    fn valid_ipv4_accepts_and_rejects() {
        // valid
        assert!(valid_ipv4("172.20.0.1"));
        assert!(valid_ipv4("0.0.0.0"));
        assert!(valid_ipv4("255.255.255.255"));
        // too few octets
        assert!(!valid_ipv4("1.2.3"));
        // injection / non-digit chars
        assert!(!valid_ipv4("a;b"));
        assert!(!valid_ipv4("1.2.3.4; reboot"));
        // octet out of range
        assert!(!valid_ipv4("1.2.3.999"));
        // misc
        assert!(!valid_ipv4(""));
        assert!(!valid_ipv4("1.2.3.4.5"));
        assert!(!valid_ipv4("1..3.4"));
    }

    #[test]
    fn gateway_reboot_requires_confirm_true() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // confirm omitted
        let body = json!({ "host": "172.20.0.1" }).to_string();
        let r = build_reply(&s, "gateway-reboot", Some(&body));
        assert!(r.contains("reboot requires confirm:true"), "{r}");
        // confirm:false
        let body = json!({ "host": "172.20.0.1", "confirm": false }).to_string();
        let r = build_reply(&s, "gateway-reboot", Some(&body));
        assert!(r.contains("reboot requires confirm:true"), "{r}");
    }

    #[test]
    fn gateway_reboot_rejects_bad_host_before_ssh() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "host": "1.2.3.4; reboot", "confirm": true }).to_string();
        let r = build_reply(&s, "gateway-reboot", Some(&body));
        assert!(r.contains("host must be a plain IPv4 address"), "{r}");
    }

    #[test]
    fn gateway_reboot_missing_body_errors() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "gateway-reboot", None);
        assert!(r.contains("missing request body"), "{r}");
    }

    #[test]
    fn host_power_commands_maps_each_valid_op() {
        assert_eq!(
            host_power_commands("maintenance-on").unwrap(),
            vec!["host-disable".to_string()]
        );
        assert_eq!(
            host_power_commands("maintenance-off").unwrap(),
            vec!["host-enable".to_string()]
        );
        assert_eq!(
            host_power_commands("reboot").unwrap(),
            vec!["host-disable".to_string(), "host-reboot".to_string()]
        );
        // DATACENTER-10 — shutdown + evacuate both disable first (XAPI gate), then
        // their terminal verb.
        assert_eq!(
            host_power_commands("shutdown").unwrap(),
            vec!["host-disable".to_string(), "host-shutdown".to_string()]
        );
        assert_eq!(
            host_power_commands("evacuate").unwrap(),
            vec!["host-disable".to_string(), "host-evacuate".to_string()]
        );
    }

    #[test]
    fn host_power_commands_unknown_op_errors() {
        assert!(host_power_commands("destroy").is_err());
        assert!(host_power_commands("").is_err());
        // A bogus op is rejected; the five mapped ops above are not.
        assert!(host_power_commands("poweroff").is_err());
    }

    #[test]
    fn host_power_is_destructive_classifies_the_dangerous_ops() {
        // The three host-level ops that bounce the host / move every guest.
        assert!(host_power_is_destructive("reboot"));
        assert!(host_power_is_destructive("shutdown"));
        assert!(host_power_is_destructive("evacuate"));
        // The reversible maintenance toggles are NOT gated.
        assert!(!host_power_is_destructive("maintenance-on"));
        assert!(!host_power_is_destructive("maintenance-off"));
        // An unknown op is not classified destructive (host_power_commands rejects
        // it on its own).
        assert!(!host_power_is_destructive("poweroff"));
    }

    #[test]
    fn host_power_destructive_ops_require_confirm_true() {
        // Each destructive op refuses without an explicit confirm:true — checked
        // BEFORE the op→verb map + dom0 allow-list, so a present-but-unconfirmed
        // body is rejected on the confirm gate (mirrors vm-delete / vdi-detach).
        let svc = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        for op in ["reboot", "shutdown", "evacuate"] {
            // confirm missing → rejected on the gate.
            let no_confirm = json!({ "dom0": "172.20.0.9", "op": op }).to_string();
            let r = build_reply(&svc, "host-power", Some(&no_confirm));
            assert!(
                r.contains(&format!("host {op} requires confirm:true")),
                "{op} without confirm: {r}"
            );
            // confirm:false → also rejected (fail-closed).
            let false_confirm =
                json!({ "dom0": "172.20.0.9", "op": op, "confirm": false }).to_string();
            let r = build_reply(&svc, "host-power", Some(&false_confirm));
            assert!(
                r.contains("requires confirm:true"),
                "{op} confirm:false: {r}"
            );
            // confirm as a non-bool string does NOT satisfy the gate.
            let str_confirm =
                json!({ "dom0": "172.20.0.9", "op": op, "confirm": "true" }).to_string();
            let r = build_reply(&svc, "host-power", Some(&str_confirm));
            assert!(
                r.contains("requires confirm:true"),
                "{op} confirm:'true': {r}"
            );
            // With confirm:true the gate passes — it then falls to the dom0
            // allow-list (empty in test), proving the confirm check is no longer
            // the blocker.
            let confirmed = json!({ "dom0": "172.20.0.9", "op": op, "confirm": true }).to_string();
            let r = build_reply(&svc, "host-power", Some(&confirmed));
            assert!(
                r.contains("dom0 not in allowed set"),
                "{op} confirm:true should pass the gate: {r}"
            );
        }
    }

    #[test]
    fn host_power_reversible_ops_are_not_confirm_gated() {
        // The maintenance toggles need no confirm — they fall straight through to
        // the dom0 allow-list (empty in test), never the confirm gate.
        let svc = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        for op in ["maintenance-on", "maintenance-off"] {
            let body = json!({ "dom0": "172.20.0.9", "op": op }).to_string();
            let r = build_reply(&svc, "host-power", Some(&body));
            assert!(
                r.contains("dom0 not in allowed set"),
                "{op} should not be confirm-gated: {r}"
            );
            assert!(!r.contains("requires confirm:true"), "{op}: {r}");
        }
    }

    #[test]
    fn parse_running_count_counts_nonblank_uuids() {
        // Empty reply → zero affected guests.
        assert_eq!(parse_running_count(""), 0);
        assert_eq!(parse_running_count("  \n"), 0);
        // One uuid.
        assert_eq!(parse_running_count("uuid-a\n"), 1);
        // A comma-separated minimal list, with a trailing blank.
        assert_eq!(parse_running_count("a,b,c"), 3);
        assert_eq!(parse_running_count("a, b ,c,"), 3);
    }

    #[test]
    fn parse_pool_flags_the_master() {
        // This host IS the master.
        let (pool, master, is_master) = parse_pool(" lab-pool \n", " m-uuid \n", "m-uuid");
        assert_eq!(pool, "lab-pool");
        assert_eq!(master, "m-uuid");
        assert!(is_master);
        // A pool member that is NOT the master.
        let (_, _, is_master) = parse_pool("lab-pool", "m-uuid", "other-uuid");
        assert!(!is_master);
        // An empty master line never falsely flags a host as master.
        let (_, master, is_master) = parse_pool("lab-pool", "", "");
        assert_eq!(master, "");
        assert!(!is_master);
    }

    #[test]
    fn host_impact_and_pool_reject_dom0_not_in_allowed_set() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so both reads bail
        // out BEFORE any SSH (the shared `resolve_dom0_uuid` allow-list guard).
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "host-impact", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
        let r = build_reply(&s, "host-pool", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }

    #[test]
    fn host_impact_and_pool_missing_body_error() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "host-impact", None).contains("missing request body"));
        assert!(build_reply(&s, "host-pool", None).contains("missing request body"));
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "host-power", None).contains("missing request body"));
    }

    #[test]
    fn dom0_not_in_allowed_set_is_rejected() {
        // With MCNF_XEN_DOM0S unset the allowed set is empty, so any dom0 is
        // rejected before any SSH is attempted.
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({
            "dom0": "10.0.0.1",
            "op": "maintenance-on"
        })
        .to_string();
        let r = build_reply(&s, "host-power", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
    }
}
