//! DATACENTER-13 (action layer) — `action/dc/{net-list,net-create,vlan-set,
//! pif-config,ipdns}` → Xen L2 network control + the unified IP/DNS read.
//!
//! The network half of the DATACENTER plane: where
//! [`crate::workers::datacenter_orchestrator`] PUBLISHES network (bridge) state to
//! `event/dc/net/*`, this responder lets the Workbench Network tab ACT on it. Same
//! dedicated-OS-thread, `action/dc/<verb>` Bus-RPC shape; the `xe` verbs run over
//! the mesh-key SSH against an allow-listed dom0.
//!
//! Gating mirrors the storage/VM responders: **RBAC** ([`crate::ipc::dc_rbac`])
//! first (a `viewer` may read `net-list`/`ipdns`, never mutate), then the **dom0
//! allow-list**, then per-verb input validation (the command-injection guard).
//!
//! Verbs:
//!   * `net-list` `{ dom0 }` (read) → networks (uuid/name/bridge/MTU);
//!   * `net-create` `{ dom0, name, description? }` → a new internal network;
//!   * `vlan-set` `{ dom0, pif, vlan, network }` → tag a PIF onto a network as a VLAN;
//!   * `pif-config` `{ dom0, pif, mode, ip?, netmask?, gateway?, dns? }` → reconfigure
//!     a PIF's IP (dhcp/static/none);
//!   * `ipdns` `{ }` (read) → the unified IP/DNS correlation: DO DNS records ↔ the
//!     gateway's DHCP leases ↔ the mesh overlay roster.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

/// The network responder — rooted at the shared workgroup root (used by `ipdns` to
/// locate the optional roster-export file for the overlay correlation).
#[derive(Debug, Clone)]
pub struct DcNetworkService {
    workgroup_root: PathBuf,
}

impl DcNetworkService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

/// Action verbs served on `action/dc/<verb>`.
pub const ACTION_VERBS: [&str; 5] = ["net-list", "net-create", "vlan-set", "pif-config", "ipdns"];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/dc/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/dc/{verb}")
}

/// Whether `verb` MUTATES the network (RBAC-gated to `operator`). The reads
/// (`net-list`, `ipdns`) return `false`. PURE.
#[must_use]
pub fn is_mutating(verb: &str) -> bool {
    !matches!(verb, "net-list" | "ipdns")
}

/// True iff `dom0` is in the configured allowed set. The SSH security gate.
#[must_use]
fn dom0_allowed(dom0: &str) -> bool {
    crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == dom0)
}

/// Run a remote `xe` command on a dom0 over SSH (orchestrator hardening flags).
fn ssh_xe_status(key: &str, dom0: &str, remote: &str) -> std::io::Result<std::process::Output> {
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
            &format!("root@{dom0}"),
            remote,
        ])
        .output()
}

/// Non-empty hex+`-` uuid guard.
///
/// # Errors
/// Returns `Err` for an empty value or any non-`[0-9a-fA-F-]` character.
fn validate_uuid(field: &str, v: &str) -> Result<(), String> {
    if v.is_empty() {
        return Err(format!("empty {field}"));
    }
    if !v.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(format!("{field} contains invalid characters"));
    }
    Ok(())
}

/// Non-empty `[A-Za-z0-9._-]` token guard (a name-label).
///
/// # Errors
/// Returns `Err` for an empty value or any character outside `[A-Za-z0-9._-]`.
fn validate_token(field: &str, v: &str) -> Result<(), String> {
    if v.is_empty() {
        return Err(format!("empty {field}"));
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!("{field} contains invalid characters"));
    }
    Ok(())
}

// ───────────────────────── pure command builders ─────────────────────────

/// Remote script that lists each network as `uuid|name|bridge|MTU`. PURE.
/// [`parse_net_rows`] decodes the output.
#[must_use]
pub fn net_list_script() -> String {
    "for u in $(xe network-list params=uuid --minimal | tr , ' '); \
     do echo \"$u|$(xe network-param-get uuid=$u param-name=name-label)|\
$(xe network-param-get uuid=$u param-name=bridge)|\
$(xe network-param-get uuid=$u param-name=MTU)\"; done"
        .to_string()
}

/// Parse the [`net_list_script`] output into `(uuid,name,bridge,mtu)` rows. PURE.
/// Skips empty-uuid lines.
#[must_use]
pub fn parse_net_rows(out: &str) -> Vec<(String, String, String, String)> {
    out.lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '|');
            let u = p.next()?.trim();
            if u.is_empty() {
                return None;
            }
            Some((
                u.to_string(),
                p.next().unwrap_or("").trim().to_string(),
                p.next().unwrap_or("").trim().to_string(),
                p.next().unwrap_or("").trim().to_string(),
            ))
        })
        .collect()
}

/// `network-create name-label=<name> [name-description=<desc>]` — a new internal
/// network (no PIF; a VLAN/PIF is attached separately via [`vlan_set_command`]).
/// PURE.
///
/// # Errors
/// Returns `Err` for a bad `name` or (when present) `description` token.
pub fn net_create_command(name: &str, description: Option<&str>) -> Result<String, String> {
    validate_token("name", name)?;
    let mut cmd = format!("network-create name-label={name}");
    if let Some(d) = description.filter(|s| !s.is_empty()) {
        validate_token("description", d)?;
        cmd.push_str(&format!(" name-description={d}"));
    }
    Ok(cmd)
}

/// `vlan-create pif-uuid=<pif> vlan=<n> network-uuid=<network>` — tag a PIF onto a
/// network as VLAN `n`. PURE. `vlan` is bounded `0..=4094`.
///
/// # Errors
/// Returns `Err` for a bad `pif`/`network` uuid or an out-of-range `vlan`.
pub fn vlan_set_command(pif: &str, vlan: u32, network: &str) -> Result<String, String> {
    validate_uuid("pif", pif)?;
    validate_uuid("network", network)?;
    if vlan > 4094 {
        return Err("vlan must be 0..=4094".into());
    }
    Ok(format!(
        "vlan-create pif-uuid={pif} vlan={vlan} network-uuid={network}"
    ))
}

/// `pif-reconfigure-ip uuid=<pif> mode=<mode> …` — reconfigure a PIF's IP. PURE.
///
/// `mode` ∈ {`dhcp`, `static`, `none`}. For `static`, `ip` + `netmask` are
/// REQUIRED and `gateway`/`dns` optional; every IP-shaped value is validated as a
/// plain dotted-quad (`dns` may be a comma-separated list). For `dhcp`/`none` the
/// address fields are ignored.
///
/// # Errors
/// Returns `Err` for an unknown `mode`, a bad `pif` uuid, a `static` mode missing
/// `ip`/`netmask`, or any address field that is not a plain IPv4.
pub fn pif_config_command(
    pif: &str,
    mode: &str,
    ip: &str,
    netmask: &str,
    gateway: &str,
    dns: &str,
) -> Result<String, String> {
    validate_uuid("pif", pif)?;
    match mode {
        "dhcp" | "none" => Ok(format!("pif-reconfigure-ip uuid={pif} mode={mode}")),
        "static" => {
            if !crate::ipc::host_ops::valid_ipv4(ip) {
                return Err("ip must be a plain IPv4 address".into());
            }
            if !crate::ipc::host_ops::valid_ipv4(netmask) {
                return Err("netmask must be a plain IPv4 address".into());
            }
            let mut cmd =
                format!("pif-reconfigure-ip uuid={pif} mode=static IP={ip} netmask={netmask}");
            if !gateway.is_empty() {
                if !crate::ipc::host_ops::valid_ipv4(gateway) {
                    return Err("gateway must be a plain IPv4 address".into());
                }
                cmd.push_str(&format!(" gateway={gateway}"));
            }
            if !dns.is_empty() {
                // DNS may be a comma-separated list of resolver IPs.
                for d in dns.split(',') {
                    if !crate::ipc::host_ops::valid_ipv4(d.trim()) {
                        return Err("dns must be comma-separated IPv4 addresses".into());
                    }
                }
                cmd.push_str(&format!(" DNS={dns}"));
            }
            Ok(cmd)
        }
        other => Err(format!("unknown mode: {other}")),
    }
}

// ───────────────────────── ipdns: unified IP/DNS read ─────────────────────────

/// Parse `doctl compute domain records list <domain> -o json` into `(type,name,
/// data)` triples. PURE. Non-array / unparsable input yields `[]`.
#[must_use]
pub fn parse_do_dns(json: &str) -> Vec<(String, String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|r| {
            let g = |k: &str| {
                r.get(k)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            (g("type"), g("name"), g("data"))
        })
        .collect()
}

/// Parse an ISC `dhcpd.leases` document into `(ip, mac, hostname)` triples. PURE.
///
/// The file is a sequence of `lease <ip> { … }` blocks; within a block we pull the
/// `hardware ethernet <mac>;` and (optional) `client-hostname "<name>";` lines.
/// A block missing the MAC is still reported (mac empty). Robust to extra
/// whitespace; tolerant of unknown lines.
#[must_use]
pub fn parse_isc_leases(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut cur_ip: Option<String> = None;
    let mut mac = String::new();
    let mut host = String::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("lease ") {
            // start of a block: "lease <ip> {"
            let ip = rest.split_whitespace().next().unwrap_or("").to_string();
            cur_ip = Some(ip);
            mac.clear();
            host.clear();
        } else if let Some(rest) = t.strip_prefix("hardware ethernet ") {
            mac = rest.trim_end_matches(';').trim().to_string();
        } else if let Some(rest) = t.strip_prefix("client-hostname ") {
            host = rest
                .trim_end_matches(';')
                .trim()
                .trim_matches('"')
                .to_string();
        } else if t == "}" {
            if let Some(ip) = cur_ip.take() {
                if !ip.is_empty() {
                    out.push((ip, std::mem::take(&mut mac), std::mem::take(&mut host)));
                }
            }
        }
    }
    out
}

/// Parse a mesh roster-export JSON (an array of `{node_id|name, overlay_ip}`) into
/// `(name, overlay_ip)` pairs. PURE. Tolerates either `name` or `node_id` for the
/// label; non-array / unparsable input yields `[]`.
#[must_use]
pub fn parse_roster_overlay(json: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|r| {
            let overlay = r.get("overlay_ip").and_then(serde_json::Value::as_str)?;
            if overlay.is_empty() {
                return None;
            }
            let name = r
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| r.get("node_id").and_then(serde_json::Value::as_str))
                .unwrap_or("")
                .to_string();
            Some((name, overlay.to_string()))
        })
        .collect()
}

/// The DO DNS domain to read for `ipdns` (`MCNF_DO_DNS_DOMAIN`); empty ⇒ skip the
/// DO DNS gather.
fn do_dns_domain() -> String {
    std::env::var("MCNF_DO_DNS_DOMAIN")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Best-effort DO DNS records via `doctl` (empty on any failure / no domain).
fn gather_do_dns() -> Vec<(String, String, String)> {
    let domain = do_dns_domain();
    if domain.is_empty() {
        return Vec::new();
    }
    let context = std::env::var("MCNF_DOCTL_CONTEXT").unwrap_or_else(|_| "mackes".to_string());
    let out = std::process::Command::new("doctl")
        .args([
            "compute",
            "domain",
            "records",
            "list",
            &domain,
            "--context",
            &context,
            "-o",
            "json",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_do_dns(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// Best-effort gateway DHCP leases over `sshpass` (the same cred path the gateway
/// gather uses). Empty on any failure / no gateway configured.
fn gather_gateway_leases() -> Vec<(String, String, String)> {
    let host = crate::workers::datacenter_orchestrator::unifi_host();
    if host.is_empty() {
        return Vec::new();
    }
    // Reuse the orchestrator's cred parse via the secret helper.
    let cred = std::process::Command::new("bash")
        .args(["-lc", "automation/secrets/mcnf-secret.sh get unifi-cred"])
        .output();
    let Some((user, pw)) = cred.ok().filter(|o| o.status.success()).and_then(|o| {
        let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if raw.is_empty() {
            None
        } else {
            Some(crate::workers::datacenter_orchestrator::parse_unifi_cred(
                &raw,
            ))
        }
    }) else {
        return Vec::new();
    };
    // ISC dhcpd lease file across the common EdgeOS/UniFi locations.
    let remote = "cat /var/run/dhcpd.leases 2>/dev/null || cat /config/dhcpd.leases 2>/dev/null \
                  || cat /var/lib/dhcp/dhcpd.leases 2>/dev/null";
    let out = std::process::Command::new("sshpass")
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
        .output();
    match out {
        Ok(o) if o.status.success() => parse_isc_leases(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// Best-effort mesh overlay roster from an export file
/// (`MCNF_NEBULA_ROSTER_JSON`, else `<workgroup_root>/nebula/roster.json`). Empty
/// when no export exists.
fn gather_overlay(workgroup_root: &std::path::Path) -> Vec<(String, String)> {
    let path = std::env::var("MCNF_NEBULA_ROSTER_JSON")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| workgroup_root.join("nebula").join("roster.json"));
    match std::fs::read_to_string(&path) {
        Ok(s) => parse_roster_overlay(&s),
        Err(_) => Vec::new(),
    }
}

/// Build the `ipdns` reply: correlate DO DNS records, the gateway's DHCP leases,
/// and the mesh overlay roster. Each source is best-effort (empty when its
/// dependency is absent), so a partially-reachable environment still answers.
fn ipdns_reply(workgroup_root: &std::path::Path) -> String {
    let dns: Vec<serde_json::Value> = gather_do_dns()
        .into_iter()
        .map(|(ty, name, data)| json!({ "type": ty, "name": name, "data": data }))
        .collect();
    let leases: Vec<serde_json::Value> = gather_gateway_leases()
        .into_iter()
        .map(|(ip, mac, host)| json!({ "ip": ip, "mac": mac, "hostname": host }))
        .collect();
    let overlay: Vec<serde_json::Value> = gather_overlay(workgroup_root)
        .into_iter()
        .map(|(name, ip)| json!({ "name": name, "overlay_ip": ip }))
        .collect();
    json!({ "ok": true, "dns": dns, "leases": leases, "overlay": overlay }).to_string()
}

// ───────────────────────── reply handlers ─────────────────────────

/// Map an `xe` process result to `{"ok":true}` / `{"error":...}`.
fn xe_ok(out: std::io::Result<std::process::Output>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    match out {
        Ok(o) if o.status.success() => json!({ "ok": true }).to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = stderr.trim();
            if msg.is_empty() {
                err("xe failed".into())
            } else {
                err(msg.to_string())
            }
        }
        Err(e) => err(format!("ssh failed: {e}")),
    }
}

/// Build the reply for one `action/dc/<verb>` network request.
#[must_use]
pub fn build_reply(svc: &DcNetworkService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    if let Err(m) = crate::ipc::dc_rbac::authorize(req_body, is_mutating(verb)) {
        return err(m);
    }
    if !ACTION_VERBS.contains(&verb) {
        return err("unknown dc verb".into());
    }
    // `ipdns` is the one verb that takes no dom0 (it reads DO/gateway/overlay).
    if verb == "ipdns" {
        return ipdns_reply(&svc.workgroup_root);
    }

    let Some(body) = req_body else {
        return err(format!("{verb}: missing request body"));
    };
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("{verb}: bad json: {e}")),
    };
    let getf = |f: &str| {
        req.get(f)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let dom0 = getf("dom0");
    if !dom0_allowed(&dom0) {
        return err("dom0 not in allowed set".into());
    }
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();

    match verb {
        "net-list" => match ssh_xe_status(&key, &dom0, &net_list_script()) {
            Ok(o) if o.status.success() => {
                let nets: Vec<serde_json::Value> =
                    parse_net_rows(&String::from_utf8_lossy(&o.stdout))
                        .into_iter()
                        .map(|(uuid, name, bridge, mtu)| {
                            json!({ "uuid": uuid, "name": name, "bridge": bridge, "mtu": mtu })
                        })
                        .collect();
                json!({ "ok": true, "nets": nets }).to_string()
            }
            other => xe_ok(other),
        },
        "net-create" => {
            let desc = getf("description");
            let cmd = match net_create_command(
                &getf("name"),
                Some(desc.as_str()).filter(|s| !s.is_empty()),
            ) {
                Ok(c) => c,
                Err(e) => return err(e),
            };
            match ssh_xe_status(&key, &dom0, &format!("xe {cmd}")) {
                Ok(o) if o.status.success() => {
                    let uuid = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    json!({ "ok": true, "network": uuid }).to_string()
                }
                other => xe_ok(other),
            }
        }
        "vlan-set" => {
            let vlan = match req.get("vlan").and_then(serde_json::Value::as_u64) {
                Some(n) => u32::try_from(n).unwrap_or(u32::MAX),
                None => return err("vlan must be an integer 0..=4094".into()),
            };
            let cmd = match vlan_set_command(&getf("pif"), vlan, &getf("network")) {
                Ok(c) => c,
                Err(e) => return err(e),
            };
            match ssh_xe_status(&key, &dom0, &format!("xe {cmd}")) {
                Ok(o) if o.status.success() => {
                    let uuid = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    json!({ "ok": true, "vlan": uuid }).to_string()
                }
                other => xe_ok(other),
            }
        }
        "pif-config" => {
            let cmd = match pif_config_command(
                &getf("pif"),
                &getf("mode"),
                &getf("ip"),
                &getf("netmask"),
                &getf("gateway"),
                &getf("dns"),
            ) {
                Ok(c) => c,
                Err(e) => return err(e),
            };
            xe_ok(ssh_xe_status(&key, &dom0, &format!("xe {cmd}")))
        }
        _ => err("unknown dc verb".into()),
    }
}

/// Run the network Bus responder loop on the current thread until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &DcNetworkService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the action verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &DcNetworkService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc-network responder: list_since failed");
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
                tracing::warn!(ulid = %msg.ulid, error = %e, "dc-network responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_and_verbs_lock() {
        for v in ACTION_VERBS {
            assert_eq!(action_topic(v), format!("action/dc/{v}"));
        }
        assert!(ACTION_VERBS.contains(&"net-list"));
        assert!(ACTION_VERBS.contains(&"vlan-set"));
        assert!(ACTION_VERBS.contains(&"ipdns"));
    }

    #[test]
    fn is_mutating_marks_reads_readonly() {
        assert!(!is_mutating("net-list"));
        assert!(!is_mutating("ipdns"));
        for v in ["net-create", "vlan-set", "pif-config"] {
            assert!(is_mutating(v), "{v}");
        }
    }

    #[test]
    fn parse_net_rows_reads_four_fields() {
        let out = "n1|Pool network|xenbr0|1500\n|skip|||\n";
        let rows = parse_net_rows(out);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "n1");
        assert_eq!(rows[0].1, "Pool network");
        assert_eq!(rows[0].2, "xenbr0");
        assert_eq!(rows[0].3, "1500");
    }

    #[test]
    fn net_create_command_optional_description() {
        assert_eq!(
            net_create_command("dmz", None).unwrap(),
            "network-create name-label=dmz"
        );
        assert_eq!(
            net_create_command("dmz", Some("guest-net")).unwrap(),
            "network-create name-label=dmz name-description=guest-net"
        );
        assert!(net_create_command("bad name", None).is_err());
        assert!(net_create_command("ok", Some("bad desc")).is_err());
    }

    #[test]
    fn vlan_set_command_bounds_and_guards() {
        // pif/network uuids are XAPI UUIDs (hex+dash).
        assert_eq!(
            vlan_set_command("abcd-01", 100, "abcd-02").unwrap(),
            "vlan-create pif-uuid=abcd-01 vlan=100 network-uuid=abcd-02"
        );
        assert!(vlan_set_command("abcd-01", 4095, "abcd-02").is_err());
        // pif injection rejected.
        assert!(vlan_set_command("a;b", 1, "abcd").is_err());
        // network injection rejected (pif ok, network bad).
        assert!(vlan_set_command("abcd", 1, "n;rm").is_err());
    }

    #[test]
    fn pif_config_modes_and_validation() {
        // pif is an XAPI UUID (hex+dash).
        assert_eq!(
            pif_config_command("ab01", "dhcp", "", "", "", "").unwrap(),
            "pif-reconfigure-ip uuid=ab01 mode=dhcp"
        );
        assert_eq!(
            pif_config_command("ab01", "none", "", "", "", "").unwrap(),
            "pif-reconfigure-ip uuid=ab01 mode=none"
        );
        assert_eq!(
            pif_config_command(
                "ab01",
                "static",
                "10.0.0.5",
                "255.255.255.0",
                "10.0.0.1",
                "1.1.1.1,8.8.8.8"
            )
            .unwrap(),
            "pif-reconfigure-ip uuid=ab01 mode=static IP=10.0.0.5 netmask=255.255.255.0 \
             gateway=10.0.0.1 DNS=1.1.1.1,8.8.8.8"
        );
        // static requires valid ip + netmask.
        assert!(pif_config_command("ab01", "static", "", "255.255.255.0", "", "").is_err());
        assert!(pif_config_command("ab01", "static", "10.0.0.5", "nope", "", "").is_err());
        // bad gateway / dns / mode.
        assert!(
            pif_config_command("ab01", "static", "10.0.0.5", "255.255.255.0", "bad", "").is_err()
        );
        assert!(pif_config_command(
            "ab01",
            "static",
            "10.0.0.5",
            "255.255.255.0",
            "",
            "1.1.1.1,bad"
        )
        .is_err());
        assert!(pif_config_command("ab01", "bridged", "", "", "", "").is_err());
    }

    #[test]
    fn parse_do_dns_reads_records() {
        let json = r#"[
            {"id":1,"type":"A","name":"lighthouse","data":"24.144.92.8","ttl":1800},
            {"id":2,"type":"CNAME","name":"www","data":"@"}
        ]"#;
        let recs = parse_do_dns(json);
        assert_eq!(recs.len(), 2);
        assert_eq!(
            recs[0],
            ("A".into(), "lighthouse".into(), "24.144.92.8".into())
        );
        assert_eq!(recs[1].0, "CNAME");
        assert!(parse_do_dns("garbage").is_empty());
        assert!(parse_do_dns("{}").is_empty());
    }

    #[test]
    fn parse_isc_leases_reads_blocks() {
        let text = "\
lease 172.20.0.42 {
  starts 4 2026/06/28 12:00:00;
  hardware ethernet aa:bb:cc:dd:ee:ff;
  client-hostname \"build-50\";
}
lease 172.20.0.43 {
  hardware ethernet 11:22:33:44:55:66;
}
";
        let leases = parse_isc_leases(text);
        assert_eq!(leases.len(), 2);
        assert_eq!(
            leases[0],
            (
                "172.20.0.42".into(),
                "aa:bb:cc:dd:ee:ff".into(),
                "build-50".into()
            )
        );
        // Second block has no client-hostname → empty host, mac still read.
        assert_eq!(leases[1].0, "172.20.0.43");
        assert_eq!(leases[1].1, "11:22:33:44:55:66");
        assert_eq!(leases[1].2, "");
        assert!(parse_isc_leases("no leases here").is_empty());
    }

    #[test]
    fn parse_roster_overlay_reads_pairs() {
        let json = r#"[
            {"node_id":"peer:sfo3","name":"sfo3","overlay_ip":"10.42.0.6"},
            {"node_id":"peer:nyc3","overlay_ip":"10.42.0.4"},
            {"node_id":"peer:noip","overlay_ip":""}
        ]"#;
        let rows = parse_roster_overlay(json);
        assert_eq!(rows.len(), 2); // the empty-overlay row is dropped
        assert_eq!(rows[0], ("sfo3".into(), "10.42.0.6".into()));
        // falls back to node_id when name absent.
        assert_eq!(rows[1], ("peer:nyc3".into(), "10.42.0.4".into()));
        assert!(parse_roster_overlay("nope").is_empty());
    }

    #[test]
    fn ipdns_is_well_formed_without_any_source() {
        // With no domain/gateway/roster configured every gather is empty, but the
        // reply is still a well-formed ok envelope (graceful degrade, not error).
        let s = DcNetworkService::new(PathBuf::from("/nonexistent-root"));
        let r = build_reply(&s, "ipdns", Some("{}"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v["dns"].is_array());
        assert!(v["leases"].is_array());
        assert!(v["overlay"].is_array());
    }

    #[test]
    fn unknown_verb_and_missing_body_error() {
        let s = DcNetworkService::new(PathBuf::from("/tmp"));
        assert!(build_reply(&s, "bogus", None).contains("unknown dc verb"));
        assert!(build_reply(&s, "net-list", None).contains("missing request body"));
    }

    #[test]
    fn verbs_reject_unlisted_dom0() {
        let s = DcNetworkService::new(PathBuf::from("/tmp"));
        for verb in ["net-list", "net-create", "vlan-set", "pif-config"] {
            let body = json!({ "dom0": "10.0.0.1" }).to_string();
            let r = build_reply(&s, verb, Some(&body));
            assert!(r.contains("dom0 not in allowed set"), "{verb}: {r}");
        }
    }
}
