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
    "gateway-reboot",
    "dr-backup",
    "gateway-status",
    // DATACENTER-23 — CA-only DR backup + the guided control-plane rebirth.
    "dr-ca-backup",
    "dr-rebirth",
    // LIGHTHOUSE-6 — the Workbench Lighthouses tab's full-ops actions.
    "lighthouse-restart",
    "lighthouse-promote",
    // DATACENTER-10 — host lifecycle: evacuate-first patch, pool membership,
    // console launch info.
    "host-evacuate",
    "host-patch",
    "host-pool",
    "host-console",
    // DATACENTER-14 — UniFi gateway firewall + port-forward edits (mutating).
    "gateway-firewall",
    "gateway-portforward",
];

/// Whether `verb` MUTATES (so it is RBAC-gated to `operator`). The read-only verbs
/// (`gateway-status`, `host-console`) return `false`. PURE.
#[must_use]
pub fn is_mutating(verb: &str) -> bool {
    !matches!(verb, "gateway-status" | "host-console")
}

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
///   reboot an enabled host, so it must be disabled first.
///
/// Each returned verb is later run as `xe <verb> host=<uuid>`.
///
/// # Errors
/// Returns `Err` for any `op` outside the three above.
pub fn host_power_commands(op: &str) -> Result<Vec<String>, String> {
    match op {
        "maintenance-on" => Ok(vec!["host-disable".to_string()]),
        "maintenance-off" => Ok(vec!["host-enable".to_string()]),
        "reboot" => Ok(vec!["host-disable".to_string(), "host-reboot".to_string()]),
        other => Err(format!("unknown op: {other}")),
    }
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
/// repo root. Returns the raw stored value, or `None` if the helper is missing,
/// the secret is absent, or the command exits non-zero/empty.
fn unifi_cred_from_store() -> Option<String> {
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
        None
    } else {
        Some(raw.to_string())
    }
}

/// The host-local UniFi credential file to fall back to (DATACENTER-14):
/// `MCNF_UNIFI_CRED_FILE` override, else `/root/.mcnf-unifi-cred`.
fn unifi_cred_file() -> String {
    std::env::var("MCNF_UNIFI_CRED_FILE").unwrap_or_else(|_| "/root/.mcnf-unifi-cred".to_string())
}

/// Read the UniFi SSH credential, mesh-secret-store **first**, then falling back
/// to the host-local `/root/.mcnf-unifi-cred` file (DATACENTER-14). Returns
/// `(user, password)` parsed like the orchestrator's `gather_gateway` path
/// (`user:pass`, default user `"ubnt"`), or `None` if neither source yields a
/// non-empty value.
fn unifi_cred() -> Option<(String, String)> {
    let raw = unifi_cred_from_store().or_else(|| {
        std::fs::read_to_string(unifi_cred_file())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })?;
    Some(crate::workers::datacenter_orchestrator::parse_unifi_cred(
        &raw,
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

// ---- DATACENTER-14: UniFi/EdgeOS gateway firewall + port-forward edits --------
//
// The gateway (172.20.0.1) is an EdgeOS/vyatta-config router reached over the
// same `sshpass` path as `gateway_reboot`/`gateway_status`. The mutations are
// driven through the vyatta config session (`configure` … `commit; save`), built
// from STRONGLY-validated request fields — every value is whitelisted to a safe
// charset/range, so nothing the operator types can carry a shell metacharacter
// into the remote config script (the same hardening posture as the `xe` verbs).

/// A vyatta config name token (firewall ruleset, etc.): `[A-Za-z0-9_-]`, non-empty,
/// ≤ 64 chars. PURE injection guard. Returns the same string when valid.
fn valid_cfg_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A vyatta config rule number `1..=9999`. PURE. Returns the parsed number.
fn valid_rule_number(v: &serde_json::Value) -> Option<u32> {
    let n = u32::try_from(v.as_u64()?).ok()?;
    (1..=9999).contains(&n).then_some(n)
}

/// A TCP/UDP port `1..=65535`. PURE.
fn valid_port(v: &serde_json::Value) -> Option<u32> {
    let n = u32::try_from(v.as_u64()?).ok()?;
    (1..=65535).contains(&n).then_some(n)
}

/// A firewall action keyword. PURE allow-list.
fn valid_fw_action(s: &str) -> bool {
    matches!(s, "accept" | "drop" | "reject")
}

/// A protocol keyword EdgeOS accepts in firewall/port-forward rules. PURE.
fn valid_protocol(s: &str) -> bool {
    matches!(s, "tcp" | "udp" | "tcp_udp" | "all" | "icmp")
}

/// A port-forward description: `[A-Za-z0-9 ._-]`, ≤ 64 chars (single-quoted in the
/// config line, and the charset excludes the quote itself). PURE.
fn valid_description(s: &str) -> bool {
    s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'))
}

/// Build the vyatta `set`/`delete` config lines for a firewall rule edit. PURE —
/// the security boundary (every field is whitelisted before it reaches the remote
/// config script).
///
/// Request `rule` object: `{ ruleset, number, op:"set"|"delete", action?, protocol?,
/// port? }`. A `set` requires `action`; `protocol`/`port` are optional refinements.
/// A `delete` removes the whole numbered rule.
///
/// # Errors
/// Returns `Err` for a missing/invalid ruleset, number, op, action, protocol, or
/// port.
pub fn firewall_config_lines(rule: &serde_json::Value) -> Result<Vec<String>, String> {
    let ruleset = rule
        .get("ruleset")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_cfg_name(ruleset) {
        return Err("firewall: ruleset must be [A-Za-z0-9_-]".into());
    }
    let number = rule
        .get("number")
        .and_then(valid_rule_number)
        .ok_or("firewall: number must be 1..=9999")?;
    let op = rule
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("set");
    let base = format!("firewall name {ruleset} rule {number}");
    if op == "delete" {
        return Ok(vec![format!("delete {base}")]);
    }
    if op != "set" {
        return Err("firewall: op must be set|delete".into());
    }
    let action = rule
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_fw_action(action) {
        return Err("firewall: action must be accept|drop|reject".into());
    }
    let mut lines = vec![format!("set {base} action {action}")];
    if let Some(proto) = rule.get("protocol").and_then(serde_json::Value::as_str) {
        if !valid_protocol(proto) {
            return Err("firewall: protocol must be tcp|udp|tcp_udp|all|icmp".into());
        }
        lines.push(format!("set {base} protocol {proto}"));
    }
    if let Some(p) = rule.get("port") {
        let port = valid_port(p).ok_or("firewall: port must be 1..=65535")?;
        lines.push(format!("set {base} destination port {port}"));
    }
    Ok(lines)
}

/// Build the vyatta `set`/`delete` config lines for a port-forward edit. PURE.
///
/// Request `fwd` object: `{ number, op:"set"|"delete", protocol?, original_port?,
/// forward_ip?, forward_port?, description? }`. A `set` requires `protocol`,
/// `original_port`, `forward_ip`, and `forward_port`; `description` is optional.
///
/// # Errors
/// Returns `Err` for a missing/invalid number, op, protocol, port, or forward IP.
pub fn portforward_config_lines(fwd: &serde_json::Value) -> Result<Vec<String>, String> {
    let number = fwd
        .get("number")
        .and_then(valid_rule_number)
        .ok_or("port-forward: number must be 1..=9999")?;
    let op = fwd
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("set");
    let base = format!("port-forward rule {number}");
    if op == "delete" {
        return Ok(vec![format!("delete {base}")]);
    }
    if op != "set" {
        return Err("port-forward: op must be set|delete".into());
    }
    let proto = fwd
        .get("protocol")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_protocol(proto) {
        return Err("port-forward: protocol must be tcp|udp|tcp_udp|all|icmp".into());
    }
    let original_port = fwd
        .get("original_port")
        .and_then(valid_port)
        .ok_or("port-forward: original_port must be 1..=65535")?;
    let forward_ip = fwd
        .get("forward_ip")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_ipv4(forward_ip) {
        return Err("port-forward: forward_ip must be a plain IPv4 address".into());
    }
    let forward_port = fwd
        .get("forward_port")
        .and_then(valid_port)
        .ok_or("port-forward: forward_port must be 1..=65535")?;
    let mut lines = vec![
        format!("set {base} protocol {proto}"),
        format!("set {base} original-port {original_port}"),
        format!("set {base} forward-to address {forward_ip}"),
        format!("set {base} forward-to port {forward_port}"),
    ];
    if let Some(desc) = fwd.get("description").and_then(serde_json::Value::as_str) {
        if !valid_description(desc) {
            return Err("port-forward: description must be [A-Za-z0-9 ._-], ≤64 chars".into());
        }
        lines.push(format!("set {base} description '{desc}'"));
    }
    Ok(lines)
}

/// Wrap validated vyatta config lines into a non-interactive EdgeOS config session
/// script (`configure` … `commit; save; exit`). PURE.
#[must_use]
pub fn vyatta_config_script(lines: &[String]) -> String {
    let mut s = String::from("source /opt/vyatta/etc/functions/script-template\nconfigure\n");
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s.push_str("commit\nsave\nexit\n");
    s
}

/// Run a validated EdgeOS config script on the gateway over `sshpass` (the router
/// has no mesh key). `host` must already be IPv4-validated; the script is built
/// only from whitelisted fields. Returns `Ok(())` on a zero exit.
fn run_gateway_config(host: &str, script: &str) -> Result<(), String> {
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
            script,
        ])
        .output()
        .map_err(|e| format!("sshpass failed: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&o.stderr);
        let msg = stderr.trim();
        if msg.is_empty() {
            Err("gateway config failed".into())
        } else {
            Err(msg.to_string())
        }
    }
}

/// Handle a `gateway-firewall` request: RBAC + confirm + IPv4 + validated rule →
/// run the EdgeOS firewall config edit. Body
/// `{ host, confirm:true, rule:{…}, principal? }`.
fn gateway_firewall(req_body: Option<&str>) -> Result<(), String> {
    let req = parse_gateway_edit(req_body, "gateway-firewall")?;
    let rule = req.get("rule").ok_or("gateway-firewall: missing `rule`")?;
    let lines = firewall_config_lines(rule)?;
    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    run_gateway_config(host, &vyatta_config_script(&lines))
}

/// Handle a `gateway-portforward` request: RBAC + confirm + IPv4 + validated fwd →
/// run the EdgeOS port-forward config edit. Body
/// `{ host, confirm:true, fwd:{…}, principal? }`.
fn gateway_portforward(req_body: Option<&str>) -> Result<(), String> {
    let req = parse_gateway_edit(req_body, "gateway-portforward")?;
    let fwd = req.get("fwd").ok_or("gateway-portforward: missing `fwd`")?;
    let lines = portforward_config_lines(fwd)?;
    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    run_gateway_config(host, &vyatta_config_script(&lines))
}

/// Shared front-half for the two gateway-edit verbs: parse the body, enforce RBAC
/// (mutating), require `confirm:true`, and validate `host` is a plain IPv4 — all
/// BEFORE any config line is built or any SSH is attempted. Returns the parsed
/// request value on success.
fn parse_gateway_edit(req_body: Option<&str>, verb: &str) -> Result<serde_json::Value, String> {
    let Some(body) = req_body else {
        return Err(format!("{verb}: missing request body"));
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("{verb}: bad json: {e}"))?;
    crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))?;
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(format!("{verb} requires confirm:true"));
    }
    let host = req
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_ipv4(host) {
        return Err("host must be a plain IPv4 address".into());
    }
    Ok(req)
}

// ---- DATACENTER-17: day-2 host care (evacuate-first rolling patch) -------------

/// The remote script the `host-evacuate` verb runs on a dom0 (resolve uuid →
/// disable → live-migrate every resident VM off). PURE (a constant — no operator
/// input is interpolated, so it carries no injection surface). XAPI refuses to
/// evacuate an enabled host, so it is disabled first.
#[must_use]
pub fn host_evacuate_remote() -> &'static str {
    "UUID=$(xe host-list params=uuid --minimal | cut -d, -f1); \
     [ -n \"$UUID\" ] || { echo 'host uuid not found' >&2; exit 1; }; \
     xe host-disable host=$UUID && xe host-evacuate uuid=$UUID"
}

/// The remote script the `host-patch` verb runs on a dom0: the full evacuate-first
/// rolling patch — disable → evacuate → `yum update` → reboot. PURE constant (no
/// interpolation). `xe host-reboot` returns once the reboot is accepted (same as
/// the `host-power reboot` path), so a zero exit means "patch applied + reboot
/// scheduled".
#[must_use]
pub fn host_patch_remote() -> &'static str {
    "UUID=$(xe host-list params=uuid --minimal | cut -d, -f1); \
     [ -n \"$UUID\" ] || { echo 'host uuid not found' >&2; exit 1; }; \
     xe host-disable host=$UUID && xe host-evacuate uuid=$UUID && \
     { yum clean all >/dev/null 2>&1 || true; } && yum update -y && \
     xe host-reboot host=$UUID"
}

/// Shared handler for the two day-2 host verbs: RBAC + confirm + dom0 allow-list,
/// then run the (constant) `remote` script over the mesh key. Body
/// `{ dom0, confirm:true, principal? }`.
fn host_day2(req_body: Option<&str>, verb: &str, remote: &str) -> Result<(), String> {
    let Some(body) = req_body else {
        return Err(format!("{verb}: missing request body"));
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("{verb}: bad json: {e}"))?;
    crate::ipc::dc_common::rbac_gate_mutating(crate::ipc::dc_common::body_principal(&req))?;
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(format!("{verb} requires confirm:true"));
    }
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return Err("dom0 not in allowed set".into());
    }
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    match ssh_run(&key, dom0, remote) {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                Err(format!("{verb} failed"))
            } else {
                Err(msg.to_string())
            }
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
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

/// Validate a DR-artifact path supplied on the wire. PURE.
///
/// Accepts a non-empty path of `[A-Za-z0-9._/-]` ending in `.age`, with no `..`
/// component — so an operator can name any `dr-*.age` file (incl. an absolute
/// path) but can never smuggle a shell metacharacter or a path-traversal segment.
/// (The path is also passed as a bare `Command` arg, never through a shell, so
/// this is defense-in-depth.)
#[must_use]
pub fn valid_dr_path(p: &str) -> bool {
    if p.is_empty() || !p.ends_with(".age") {
        return false;
    }
    if !p
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return false;
    }
    !p.split('/').any(|seg| seg == "..")
}

/// DATACENTER-23 — run the CA-only DR backup (confirm-gated): shells out to
/// `automation/dr/dr-ca-backup.sh`, which age-encrypts just the Nebula CA to the
/// mesh recipient and prints the artifact path. Requires `{"confirm":true}`.
fn dr_ca_backup(req_body: Option<&str>) -> Result<String, String> {
    let Some(body) = req_body else {
        return Err("dr-ca-backup: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("dr-ca-backup: bad json: {e}"))?;
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("dr-ca-backup requires confirm:true".into());
    }
    let o = std::process::Command::new("bash")
        .args(["-lc", "automation/dr/dr-ca-backup.sh"])
        .output()
        .map_err(|e| format!("dr-ca-backup: spawn failed: {e}"))?;
    if o.status.success() {
        let path = String::from_utf8_lossy(&o.stdout);
        let path = path.trim();
        if path.is_empty() {
            Err("dr-ca-backup: produced no output path".into())
        } else {
            Ok(path.to_string())
        }
    } else {
        let stderr = String::from_utf8_lossy(&o.stderr);
        let msg = stderr.trim();
        if msg.is_empty() {
            Err("dr-ca-backup failed".into())
        } else {
            Err(msg.to_string())
        }
    }
}

/// DATACENTER-23 — run the guided control-plane rebirth.
///
/// Body `{ "file": "<dr-*.age path>", "execute"?: bool, "confirm"?: bool }`.
/// `file` MUST pass [`valid_dr_path`] (checked BEFORE any spawn). DEFAULT is a
/// SAFE dry run (validate the manifest + print the plan, no writes). A live
/// rebirth (`--execute`, which clobbers etcd + the on-disk CA) requires BOTH
/// `execute:true` AND `confirm:true` — the double-gate for the destructive path.
/// Returns the script's combined stderr/stdout plan/output.
fn dr_rebirth(req_body: Option<&str>) -> Result<String, String> {
    let Some(body) = req_body else {
        return Err("dr-rebirth: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("dr-rebirth: bad json: {e}"))?;

    let file = req
        .get("file")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !valid_dr_path(file) {
        return Err("file must be a .age path ([A-Za-z0-9._/-], no '..')".into());
    }

    let execute = req
        .get("execute")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if execute && req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("dr-rebirth --execute requires confirm:true".into());
    }

    // Invoke the script with bare args (no shell) so the path can never be
    // re-parsed; the dry run is the default and only --execute is destructive.
    let mut cmd = std::process::Command::new("bash");
    cmd.arg("automation/dr/dr-rebirth.sh").arg(file);
    if execute {
        cmd.arg("--execute");
    }
    let o = cmd
        .output()
        .map_err(|e| format!("dr-rebirth: spawn failed: {e}"))?;

    // The script narrates the plan/result on stderr; surface it either way.
    let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
    out.push_str(&String::from_utf8_lossy(&o.stderr));
    let out = out.trim().to_string();
    if o.status.success() {
        Ok(out)
    } else if out.is_empty() {
        Err("dr-rebirth failed".into())
    } else {
        Err(out)
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

// ───────────────────── DATACENTER-10 — host lifecycle ─────────────────────

/// True iff `dom0` is in the configured allowed set. The SECURITY guard before any
/// host SSH (shared by the new host-lifecycle verbs).
#[must_use]
fn dom0_allowed(dom0: &str) -> bool {
    crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
}

/// Resolve a dom0's host UUID over SSH (`xe host-list params=uuid --minimal`),
/// taking the first uuid and guarding it for injection. Shared by the
/// host-lifecycle verbs that run `xe <verb> host=<uuid>`.
///
/// # Errors
/// Returns `Err` on an SSH/`xe` failure, an empty result, or a uuid carrying any
/// character that is not an ASCII hex digit or `-`.
fn resolve_host_uuid(key: &str, dom0: &str) -> Result<String, String> {
    match ssh_xe_status(key, dom0, "xe host-list params=uuid --minimal") {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            let uuid = out
                .trim()
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if uuid.is_empty() {
                return Err("host uuid not found".into());
            }
            if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return Err("host uuid contains invalid characters".into());
            }
            Ok(uuid)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            Err(if msg.is_empty() {
                "host-list failed".into()
            } else {
                msg.to_string()
            })
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// The ordered `xe` host verbs a `host-evacuate` runs (each as
/// `xe <verb> host=<uuid>`). PURE. `host-disable` first (XAPI evacuates a disabled
/// host), then `host-evacuate` (live-migrate every resident VM to other pool
/// hosts) — leaving the host drained + in maintenance.
#[must_use]
pub fn host_evacuate_commands() -> Vec<String> {
    vec!["host-disable".to_string(), "host-evacuate".to_string()]
}

/// Run a sequence of `xe <verb> host=<uuid>` commands on `dom0`, stopping at the
/// first failure. Shared by host-evacuate (and the xe steps of host-patch).
fn run_host_verbs(key: &str, dom0: &str, uuid: &str, verbs: &[String]) -> Result<(), String> {
    for v in verbs {
        let remote = format!("xe {v} host={uuid}");
        match ssh_xe_status(key, dom0, &remote) {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let msg = stderr.trim();
                return Err(if msg.is_empty() {
                    format!("{v} failed")
                } else {
                    msg.to_string()
                });
            }
            Err(e) => return Err(format!("ssh failed: {e}")),
        }
    }
    Ok(())
}

/// `host-evacuate` `{ dom0, confirm:true }` — drain a host: disable it then
/// live-migrate all resident VMs off ([`host_evacuate_commands`]). Disruptive →
/// `confirm:true`-gated. Returns `Ok(())` once both steps succeed.
fn host_evacuate(req_body: Option<&str>) -> Result<(), String> {
    let Some(body) = req_body else {
        return Err("host-evacuate: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("host-evacuate: bad json: {e}"))?;
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("evacuate requires confirm:true".into());
    }
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !dom0_allowed(dom0) {
        return Err("dom0 not in allowed set".into());
    }
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let uuid = resolve_host_uuid(&key, dom0)?;
    run_host_verbs(&key, dom0, &uuid, &host_evacuate_commands())
}

/// `host-patch` `{ dom0, confirm:true }` — rolling, evacuate-first patch: disable +
/// evacuate the host, `yum update -y` (the XCP-ng update path), then
/// `xe host-reboot` to boot the patched host. Disruptive → `confirm:true`-gated.
/// Each step is sequential; a failure stops the rollout and is reported. Returns
/// `Ok(())` once the reboot is accepted.
fn host_patch(req_body: Option<&str>) -> Result<(), String> {
    let Some(body) = req_body else {
        return Err("host-patch: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("host-patch: bad json: {e}"))?;
    if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("patch requires confirm:true".into());
    }
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !dom0_allowed(dom0) {
        return Err("dom0 not in allowed set".into());
    }
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let uuid = resolve_host_uuid(&key, dom0)?;
    // Evacuate-first (disable + migrate VMs off) so patching can't disrupt guests.
    run_host_verbs(&key, dom0, &uuid, &host_evacuate_commands())?;
    // Apply updates from the XCP-ng repos. The literal command carries no
    // operator input, so there is nothing to escape.
    match ssh_xe_status(&key, dom0, "yum update -y") {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            return Err(if msg.is_empty() {
                "yum update failed".into()
            } else {
                format!("yum update failed: {msg}")
            });
        }
        Err(e) => return Err(format!("ssh failed: {e}")),
    }
    // Reboot the (disabled) host to boot the patched kernel/toolstack.
    run_host_verbs(&key, dom0, &uuid, &["host-reboot".to_string()])
}

/// Whether a `host-pool` `op` is a recognized membership operation. PURE.
#[must_use]
pub fn host_pool_op_valid(op: &str) -> bool {
    matches!(op, "designate-master" | "eject" | "join")
}

/// Parse the XAPI credential (`automation/secrets/mcnf-secret.sh get xapi-cred`)
/// from the mesh secret store as `(user, password)` — `user:pass` (default user
/// `root`). `None` when the helper is missing / the secret is absent / empty.
fn xapi_cred() -> Option<(String, String)> {
    let o = std::process::Command::new("bash")
        .args(["-lc", "automation/secrets/mcnf-secret.sh get xapi-cred"])
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
    Some(match raw.split_once(':') {
        Some((u, p)) => (u.trim().to_string(), p.trim().to_string()),
        None => ("root".to_string(), raw.to_string()),
    })
}

/// `host-pool` `{ dom0, op, host?, master?, confirm? }` — pool membership:
///   * `designate-master` `{ host }` — promote pool member `host` (uuid) to master
///     (`xe pool-designate-new-master host-uuid=<host>`);
///   * `eject` `{ host, confirm:true }` — eject member `host` (uuid) from the pool
///     (`xe pool-eject host-uuid=<host>`), destructive → confirm-gated;
///   * `join` `{ master, confirm:true }` — join THIS dom0 to the pool whose master
///     is at IPv4 `master`, using the XAPI cred from the mesh secret store
///     (`xe pool-join master-address=<master> …`), confirm-gated.
/// We SSH to `dom0` (an allow-listed pool master / the joining host). Returns a
/// short status string on success.
fn host_pool(req_body: Option<&str>) -> Result<String, String> {
    let Some(body) = req_body else {
        return Err("host-pool: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("host-pool: bad json: {e}"))?;
    let op = req
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !host_pool_op_valid(op) {
        return Err(format!("unknown pool op: {op}"));
    }
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // Pure input validation + remote-command assembly BEFORE the dom0 allow-list
    // (the allow-list is the SSH gate, checked just before exec) — so a malformed
    // op/host/master/confirm is rejected the same regardless of allow-list state.
    let remote = match op {
        "designate-master" | "eject" => {
            // `host` is a pool host uuid — guard it for injection before use.
            let host = req
                .get("host")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if host.is_empty() || !host.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return Err("host must be a non-empty hex+dash uuid".into());
            }
            if op == "eject" {
                if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
                    return Err("eject requires confirm:true".into());
                }
                // pool-eject prompts interactively; answer it on stdin.
                format!("echo yes | xe pool-eject host-uuid={host}")
            } else {
                format!("xe pool-designate-new-master host-uuid={host}")
            }
        }
        // join
        _ => {
            if req.get("confirm").and_then(serde_json::Value::as_bool) != Some(true) {
                return Err("join requires confirm:true".into());
            }
            let master = req
                .get("master")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if !valid_ipv4(master) {
                return Err("master must be a plain IPv4 address".into());
            }
            let (user, pw) = xapi_cred().ok_or("no xapi cred in store")?;
            // The cred is not operator-supplied (mesh secret store) and the master
            // is a validated IPv4; user/pw come from our own trusted store.
            format!(
                "xe pool-join master-address={master} master-username={user} master-password={pw}"
            )
        }
    };

    // SECURITY: SSH only an allow-listed dom0, checked right before exec.
    if !dom0_allowed(dom0) {
        return Err("dom0 not in allowed set".into());
    }
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    match ssh_xe_status(&key, dom0, &remote) {
        Ok(o) if o.status.success() => Ok(format!("pool {op} ok")),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            Err(if msg.is_empty() {
                format!("pool {op} failed")
            } else {
                msg.to_string()
            })
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// `host-console` `{ dom0 }` — return the SSH connection info the Workbench uses to
/// launch a dom0 console terminal (read-only; like the lighthouse-ssh launch, the
/// terminal itself opens panel-side). The `dom0` MUST be allow-listed so we never
/// echo connection info for an arbitrary host. Returns `(ssh_target, key_path)`.
fn host_console(req_body: Option<&str>) -> Result<(String, String), String> {
    let Some(body) = req_body else {
        return Err("host-console: missing request body".into());
    };
    let req: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("host-console: bad json: {e}"))?;
    let dom0 = req
        .get("dom0")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !dom0_allowed(dom0) {
        return Err("dom0 not in allowed set".into());
    }
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    Ok((format!("root@{dom0}"), key))
}

/// Build the reply for one `action/dc/<verb>` request.
#[must_use]
pub fn build_reply(svc: &HostOpsService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    // RBAC (design §9): a mutating verb requires the caller's mesh principal to map
    // to `operator`; a `viewer` is rejected before any allow-list / SSH. A denial
    // is also audited (DATACENTER-7).
    if let Err(m) = crate::ipc::dc_rbac::authorize(req_body, is_mutating(verb)) {
        crate::ipc::dc_rbac::audit_denial(verb, req_body, &m);
        return err(m);
    }
    match verb {
        "host-power" => {}
        // DATACENTER-10 — host lifecycle.
        "host-evacuate" => {
            return match host_evacuate(req_body) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(m) => err(m),
            };
        }
        "host-patch" => {
            return match host_patch(req_body) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(m) => err(m),
            };
        }
        "host-pool" => {
            return match host_pool(req_body) {
                Ok(status) => json!({ "ok": true, "status": status }).to_string(),
                Err(m) => err(m),
            };
        }
        "host-console" => {
            return match host_console(req_body) {
                Ok((ssh, key)) => json!({ "ok": true, "ssh": ssh, "key": key }).to_string(),
                Err(m) => err(m),
            };
        }
        "gateway-reboot" => {
            return match gateway_reboot(req_body) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(m) => err(m),
            };
        }
        // DATACENTER-14 — UniFi/EdgeOS gateway firewall + port-forward EDITS.
        "gateway-firewall" => {
            return match gateway_firewall(req_body) {
                Ok(()) => json!({ "ok": true }).to_string(),
                Err(m) => err(m),
            };
        }
        "gateway-portforward" => {
            return match gateway_portforward(req_body) {
                Ok(()) => json!({ "ok": true }).to_string(),
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
        // DATACENTER-23 — CA-only DR backup.
        "dr-ca-backup" => {
            return match dr_ca_backup(req_body) {
                Ok(path) => json!({ "ok": true, "path": path }).to_string(),
                Err(m) => err(m),
            };
        }
        // DATACENTER-23 — guided control-plane rebirth (dry-run default).
        "dr-rebirth" => {
            return match dr_rebirth(req_body) {
                Ok(output) => json!({ "ok": true, "output": output }).to_string(),
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

    // SECURITY: only act on a dom0 in the configured allowed set — never SSH an
    // attacker-supplied host. Checked BEFORE building/running anything.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
    {
        return err("dom0 not in allowed set".into());
    }

    let verbs = match host_power_commands(op) {
        Ok(v) => v,
        Err(e) => return err(e),
    };

    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();

    // Resolve the host's UUID remotely. `--minimal` prints just the value.
    let uuid = match ssh_xe_status(&key, dom0, "xe host-list params=uuid --minimal") {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            // `--minimal` yields a comma-separated list for multiple hosts; on a
            // single-host pool member it's one uuid. Take the first.
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
            return if msg.is_empty() {
                err("host-list failed".into())
            } else {
                err(msg.to_string())
            };
        }
        Err(e) => return err(format!("ssh failed: {e}")),
    };
    if uuid.is_empty() {
        return err("host uuid not found".into());
    }
    // The remote uuid is XAPI-generated; guard anyway before interpolation.
    if !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return err("host uuid contains invalid characters".into());
    }

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
        assert_eq!(action_topic("gateway-reboot"), "action/dc/gateway-reboot");
        assert_eq!(action_topic("dr-backup"), "action/dc/dr-backup");
        assert_eq!(action_topic("dr-ca-backup"), "action/dc/dr-ca-backup");
        assert_eq!(action_topic("dr-rebirth"), "action/dc/dr-rebirth");
        assert_eq!(action_topic("gateway-status"), "action/dc/gateway-status");
        assert_eq!(
            action_topic("lighthouse-restart"),
            "action/dc/lighthouse-restart"
        );
        assert_eq!(
            action_topic("lighthouse-promote"),
            "action/dc/lighthouse-promote"
        );
        assert!(ACTION_VERBS.contains(&"host-power"));
        assert!(ACTION_VERBS.contains(&"gateway-reboot"));
        assert!(ACTION_VERBS.contains(&"dr-backup"));
        assert!(ACTION_VERBS.contains(&"dr-ca-backup"));
        assert!(ACTION_VERBS.contains(&"dr-rebirth"));
        assert!(ACTION_VERBS.contains(&"gateway-status"));
        assert!(ACTION_VERBS.contains(&"lighthouse-restart"));
        assert!(ACTION_VERBS.contains(&"lighthouse-promote"));
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
    fn dr_ca_backup_requires_confirm_true() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let r = build_reply(&s, "dr-ca-backup", Some(&json!({}).to_string()));
        assert!(r.contains("dr-ca-backup requires confirm:true"), "{r}");
        let r = build_reply(
            &s,
            "dr-ca-backup",
            Some(&json!({ "confirm": false }).to_string()),
        );
        assert!(r.contains("dr-ca-backup requires confirm:true"), "{r}");
    }

    #[test]
    fn dr_ca_backup_missing_body_errors() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "dr-ca-backup", None).contains("missing request body"));
    }

    #[test]
    fn valid_dr_path_accepts_age_files_and_rejects_injection() {
        // valid absolute + relative .age paths
        assert!(valid_dr_path(
            "/root/mcnf-dr-backups/dr-20260628T000000Z.age"
        ));
        assert!(valid_dr_path("dr-ca-20260628T000000Z.age"));
        // must end in .age
        assert!(!valid_dr_path("/etc/passwd"));
        assert!(!valid_dr_path("dr-backup"));
        // no shell metachars / spaces
        assert!(!valid_dr_path("dr.age; rm -rf /"));
        assert!(!valid_dr_path("dr file.age"));
        assert!(!valid_dr_path("$(whoami).age"));
        assert!(!valid_dr_path("a`b`.age"));
        // no path traversal
        assert!(!valid_dr_path("../../etc/shadow.age"));
        assert!(!valid_dr_path("a/../b.age"));
        // empty
        assert!(!valid_dr_path(""));
    }

    #[test]
    fn dr_rebirth_rejects_bad_path_before_spawn() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // injection / traversal / non-.age are all rejected at the guard.
        for bad in ["/etc/passwd", "x.age; reboot", "../secret.age", ""] {
            let body = json!({ "file": bad }).to_string();
            let r = build_reply(&s, "dr-rebirth", Some(&body));
            assert!(r.contains("file must be a .age path"), "{bad} -> {r}");
        }
    }

    #[test]
    fn dr_rebirth_execute_requires_confirm() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // A valid path but execute without confirm → refused before any spawn.
        let body = json!({ "file": "dr-20260628T000000Z.age", "execute": true }).to_string();
        let r = build_reply(&s, "dr-rebirth", Some(&body));
        assert!(
            r.contains("dr-rebirth --execute requires confirm:true"),
            "{r}"
        );
    }

    #[test]
    fn dr_rebirth_missing_body_errors() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        assert!(build_reply(&s, "dr-rebirth", None).contains("missing request body"));
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
    }

    #[test]
    fn host_power_commands_unknown_op_errors() {
        assert!(host_power_commands("destroy").is_err());
        assert!(host_power_commands("").is_err());
        assert!(host_power_commands("shutdown").is_err());
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

    // ---- DATACENTER-10: host lifecycle ----

    #[test]
    fn host_lifecycle_verbs_in_action_set() {
        for v in ["host-evacuate", "host-patch", "host-pool", "host-console"] {
            assert!(ACTION_VERBS.contains(&v), "missing {v}");
            assert_eq!(action_topic(v), format!("action/dc/{v}"));
        }
    }

    // ---- DATACENTER-14: gateway firewall + port-forward edits -----------------

    #[test]
    fn new_dc14_dc17_verbs_in_lock() {
        for v in [
            "gateway-firewall",
            "gateway-portforward",
            "host-evacuate",
            "host-patch",
        ] {
            assert_eq!(action_topic(v), format!("action/dc/{v}"));
            assert!(ACTION_VERBS.contains(&v), "{v} missing from ACTION_VERBS");
        }
    }

    #[test]
    fn is_mutating_marks_reads_readonly() {
        assert!(!is_mutating("gateway-status"));
        assert!(!is_mutating("host-console"));
        for v in ["host-power", "host-evacuate", "host-patch", "host-pool"] {
            assert!(is_mutating(v), "{v}");
        }
    }

    #[test]
    fn host_evacuate_commands_disable_then_evacuate() {
        assert_eq!(
            host_evacuate_commands(),
            vec!["host-disable".to_string(), "host-evacuate".to_string()]
        );
    }

    #[test]
    fn firewall_config_lines_builds_set_with_refinements() {
        let rule = json!({
            "ruleset": "WAN_IN", "number": 30, "op": "set",
            "action": "accept", "protocol": "tcp", "port": 443
        });
        let lines = firewall_config_lines(&rule).unwrap();
        assert_eq!(
            lines,
            vec![
                "set firewall name WAN_IN rule 30 action accept".to_string(),
                "set firewall name WAN_IN rule 30 protocol tcp".to_string(),
                "set firewall name WAN_IN rule 30 destination port 443".to_string(),
            ]
        );
    }

    #[test]
    fn host_pool_op_valid_set() {
        assert!(host_pool_op_valid("designate-master"));
        assert!(host_pool_op_valid("eject"));
        assert!(host_pool_op_valid("join"));
        assert!(!host_pool_op_valid("delete"));
        assert!(!host_pool_op_valid(""));
    }

    #[test]
    fn host_evacuate_and_patch_require_confirm() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        for verb in ["host-evacuate", "host-patch"] {
            let body = json!({ "dom0": "10.0.0.1" }).to_string();
            let r = build_reply(&s, verb, Some(&body));
            assert!(r.contains("requires confirm:true"), "{verb}: {r}");
        }
    }

    #[test]
    fn host_lifecycle_rejects_unlisted_dom0() {
        // With confirm:true the next gate is the (empty) dom0 allow-list.
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "dom0": "10.0.0.1", "confirm": true }).to_string();
        assert!(build_reply(&s, "host-evacuate", Some(&body)).contains("dom0 not in allowed set"));
        assert!(build_reply(&s, "host-patch", Some(&body)).contains("dom0 not in allowed set"));
        // host-console (read-only) also allow-lists the dom0.
        let body = json!({ "dom0": "10.0.0.1" }).to_string();
        assert!(build_reply(&s, "host-console", Some(&body)).contains("dom0 not in allowed set"));
        // host-pool: unknown op rejected before the dom0 check.
        let body = json!({ "dom0": "10.0.0.1", "op": "bogus" }).to_string();
        assert!(build_reply(&s, "host-pool", Some(&body)).contains("unknown pool op"));
        // a valid op then hits the allow-list.
        let body =
            json!({ "dom0": "10.0.0.1", "op": "designate-master", "host": "1111" }).to_string();
        assert!(build_reply(&s, "host-pool", Some(&body)).contains("dom0 not in allowed set"));
    }

    #[test]
    fn host_pool_eject_and_join_require_confirm_and_validate() {
        // Input validation now PRECEDES the dom0 allow-list, so these are reachable
        // with the allowed set empty (no shared-env mutation → no test race).
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // eject without confirm.
        let body = json!({ "dom0": "10.0.0.1", "op": "eject", "host": "1111-2222" }).to_string();
        let r = build_reply(&s, "host-pool", Some(&body));
        assert!(r.contains("eject requires confirm:true"), "{r}");
        // join without a valid master IPv4.
        let body =
            json!({ "dom0": "10.0.0.1", "op": "join", "confirm": true, "master": "not-an-ip" })
                .to_string();
        let r = build_reply(&s, "host-pool", Some(&body));
        assert!(r.contains("master must be a plain IPv4 address"), "{r}");
        // designate-master with a bad host uuid (injection-bearing).
        let body =
            json!({ "dom0": "10.0.0.1", "op": "designate-master", "host": "a;b" }).to_string();
        let r = build_reply(&s, "host-pool", Some(&body));
        assert!(r.contains("host must be a non-empty hex+dash uuid"), "{r}");
    }

    #[test]
    fn host_console_rejects_unlisted_dom0() {
        // The success path just formats `root@<dom0>` + the key; the security-
        // relevant path is the allow-list rejection, testable without env mutation.
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body = json!({ "dom0": "10.0.0.1" }).to_string();
        let r = build_reply(&s, "host-console", Some(&body));
        assert!(r.contains("dom0 not in allowed set"), "{r}");
        // Missing body errors cleanly.
        assert!(build_reply(&s, "host-console", None).contains("missing request body"));
    }

    #[test]
    fn rbac_viewer_rejected_on_mutating_host_verb() {
        // The shared crate test lock serializes the role-map mutation across the
        // RBAC integration tests so the parallel runner can't observe a torn map.
        let _g = crate::ipc::dc_rbac::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        std::env::set_var(crate::ipc::dc_rbac::ROLE_MAP_ENV, "bob=viewer");
        let body =
            json!({ "principal": "bob", "dom0": "10.0.0.1", "op": "maintenance-on" }).to_string();
        let r = build_reply(&s, "host-power", Some(&body));
        std::env::remove_var(crate::ipc::dc_rbac::ROLE_MAP_ENV);
        assert!(r.contains("rbac"), "{r}");
        assert!(r.contains("viewer"), "{r}");
    }

    #[test]
    fn firewall_config_lines_delete_drops_whole_rule() {
        let rule = json!({ "ruleset": "WAN_IN", "number": 30, "op": "delete" });
        assert_eq!(
            firewall_config_lines(&rule).unwrap(),
            vec!["delete firewall name WAN_IN rule 30".to_string()]
        );
    }

    #[test]
    fn firewall_config_lines_reject_bad_fields() {
        // injection in ruleset
        assert!(
            firewall_config_lines(&json!({"ruleset":"a;b","number":1,"action":"accept"})).is_err()
        );
        // out-of-range number
        assert!(
            firewall_config_lines(&json!({"ruleset":"WAN","number":0,"action":"accept"})).is_err()
        );
        assert!(
            firewall_config_lines(&json!({"ruleset":"WAN","number":99999,"action":"accept"}))
                .is_err()
        );
        // bad action
        assert!(
            firewall_config_lines(&json!({"ruleset":"WAN","number":1,"action":"nuke"})).is_err()
        );
        // set without action
        assert!(firewall_config_lines(&json!({"ruleset":"WAN","number":1,"op":"set"})).is_err());
        // bad protocol / port
        assert!(firewall_config_lines(
            &json!({"ruleset":"WAN","number":1,"action":"drop","protocol":"raw"})
        )
        .is_err());
        assert!(firewall_config_lines(
            &json!({"ruleset":"WAN","number":1,"action":"drop","port":70000})
        )
        .is_err());
        // unknown op
        assert!(firewall_config_lines(&json!({"ruleset":"WAN","number":1,"op":"flush"})).is_err());
    }

    #[test]
    fn portforward_config_lines_builds_full_set() {
        let fwd = json!({
            "number": 1, "op": "set", "protocol": "tcp",
            "original_port": 443, "forward_ip": "172.20.0.5",
            "forward_port": 8443, "description": "ingress 1"
        });
        let lines = portforward_config_lines(&fwd).unwrap();
        assert_eq!(
            lines,
            vec![
                "set port-forward rule 1 protocol tcp".to_string(),
                "set port-forward rule 1 original-port 443".to_string(),
                "set port-forward rule 1 forward-to address 172.20.0.5".to_string(),
                "set port-forward rule 1 forward-to port 8443".to_string(),
                "set port-forward rule 1 description 'ingress 1'".to_string(),
            ]
        );
        // delete
        assert_eq!(
            portforward_config_lines(&json!({ "number": 1, "op": "delete" })).unwrap(),
            vec!["delete port-forward rule 1".to_string()]
        );
    }

    #[test]
    fn portforward_config_lines_reject_bad_fields() {
        // missing required set fields
        assert!(
            portforward_config_lines(&json!({"number":1,"op":"set","protocol":"tcp"})).is_err()
        );
        // bad forward ip (injection)
        assert!(portforward_config_lines(&json!({
            "number":1,"protocol":"tcp","original_port":443,
            "forward_ip":"1.2.3.4; reboot","forward_port":443
        }))
        .is_err());
        // bad description charset (a quote would break out)
        assert!(portforward_config_lines(&json!({
            "number":1,"protocol":"tcp","original_port":443,
            "forward_ip":"172.20.0.5","forward_port":443,"description":"a'b"
        }))
        .is_err());
    }

    #[test]
    fn vyatta_config_script_wraps_session() {
        let s = vyatta_config_script(&["set port-forward rule 1 protocol tcp".to_string()]);
        assert!(s.starts_with("source /opt/vyatta/etc/functions/script-template\nconfigure\n"));
        assert!(s.contains("set port-forward rule 1 protocol tcp\n"));
        assert!(s.trim_end().ends_with("exit"));
        assert!(s.contains("commit\nsave\n"));
    }

    #[test]
    fn gateway_firewall_gates_before_ssh() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        // missing body
        assert!(build_reply(&s, "gateway-firewall", None).contains("missing request body"));
        // confirm required
        let body = json!({ "host": "172.20.0.1", "rule": { "ruleset": "WAN", "number": 1, "action": "accept" } }).to_string();
        let r = build_reply(&s, "gateway-firewall", Some(&body));
        assert!(r.contains("requires confirm:true"), "{r}");
        // bad host rejected before building config
        let body = json!({ "host": "1.2.3.4; reboot", "confirm": true, "rule": { "ruleset": "WAN", "number": 1, "action": "accept" } }).to_string();
        let r = build_reply(&s, "gateway-firewall", Some(&body));
        assert!(r.contains("host must be a plain IPv4 address"), "{r}");
        // missing rule object
        let body = json!({ "host": "172.20.0.1", "confirm": true }).to_string();
        let r = build_reply(&s, "gateway-firewall", Some(&body));
        assert!(r.contains("missing `rule`"), "{r}");
    }

    #[test]
    fn gateway_portforward_gates_before_ssh() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        let body =
            json!({ "host": "172.20.0.1", "fwd": { "number": 1, "op": "delete" } }).to_string();
        let r = build_reply(&s, "gateway-portforward", Some(&body));
        assert!(r.contains("requires confirm:true"), "{r}");
        let body = json!({ "host": "172.20.0.1", "confirm": true }).to_string();
        let r = build_reply(&s, "gateway-portforward", Some(&body));
        assert!(r.contains("missing `fwd`"), "{r}");
    }

    #[test]
    fn host_day2_remote_scripts_carry_the_key_verbs() {
        assert!(host_evacuate_remote().contains("xe host-evacuate"));
        assert!(host_evacuate_remote().contains("xe host-disable"));
        let patch = host_patch_remote();
        assert!(patch.contains("xe host-evacuate"));
        assert!(patch.contains("yum update -y"));
        assert!(patch.contains("xe host-reboot"));
    }

    #[test]
    fn host_evacuate_and_patch_gate_confirm_then_dom0() {
        let s = HostOpsService::new(std::path::PathBuf::from("/tmp"));
        for verb in ["host-evacuate", "host-patch"] {
            // confirm required
            let body = json!({ "dom0": "172.20.0.9" }).to_string();
            let r = build_reply(&s, verb, Some(&body));
            assert!(r.contains("requires confirm:true"), "{verb}: {r}");
            // confirmed but dom0 not in the (empty) allowed set
            let body = json!({ "dom0": "172.20.0.9", "confirm": true }).to_string();
            let r = build_reply(&s, verb, Some(&body));
            assert!(r.contains("dom0 not in allowed set"), "{verb}: {r}");
        }
    }
}
