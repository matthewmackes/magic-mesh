//! DATACENTER-5 — the mackesd `datacenter_orchestrator` worker.
//!
//! The no-fixed-center engine behind the Workbench **Datacenter** plane
//! (`docs/design/datacenter-control.md`). It samples the datacenter substrate and
//! publishes per-resource state to the Bus as `event/dc/<kind>/<id>` so hosts,
//! VMs, droplets, storage, network, and the gateway are first-class mesh state —
//! readable by the panel (and the Notification Hub) with no AI in the loop, the
//! same way [`super::farm_orchestrator`] surfaces farm jobs.
//!
//! Design (mirrors `farm_orchestrator`): the *brain* ([`DatacenterOrchestrator`]) is
//! a pure, deduped snapshot differ — it emits an event only when a resource's
//! signature changes — and the worker is thin I/O around it. It is **leader-gated**
//! so a multi-node mesh publishes each change once.
//!
//! Phase note: this first increment reads the **DigitalOcean** zone via `doctl`
//! (the one substrate fully available today — Zone 1 / production). The Xen (XAPI)
//! and UniFi gateway sources are explicit seams ([`gather_xen`], [`gather_gateway`])
//! that light up with their Phase-0 dependencies (DATACENTER-1 XAPI provider,
//! DATACENTER-4 XAPI-over-overlay, DATACENTER-3 mesh secrets) without touching the
//! brain or the Bus contract.
//!
//! DATACENTER-4 (done): the Xen source now routes its `xe`-over-SSH per node —
//! [`resolve_xe_route`] picks **Direct** when this node is on the `172.20.0.0/16`
//! lab LAN (or no relay is set) and **ProxyJump** through an on-LAN relay peer
//! (`MCNF_XEN_RELAY`, an overlay IP) when it's off-LAN, so an off-LAN node can
//! still read XAPI over the overlay. The chosen path is published to
//! `event/dc/route/xen/<dom0>`, and a failed relay hop degrades cleanly to a
//! Direct attempt (published `relay down`).

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// Sweep cadence — 15 s (datacenter state is coarse; doctl/XAPI calls aren't free).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(15);

/// DATACENTER-12 — SR capacity **warning** line. An SR whose physical utilisation
/// is at or above this percentage of its capacity emits a `warn` onto the health
/// lane (`event/dc/health/sr:<uuid>`) so the operator (and the copilot triage) sees
/// a filling store before it wedges. Matches the panel's default
/// `storage_threshold_pct`; the gather side carries its own copy because the worker
/// can't read the panel's UI state. Overridable via `MCNF_SR_WARN_PCT`.
pub const SR_WARN_PCT: u64 = 85;

/// DATACENTER-12 — SR capacity **critical** line. At or above this percentage the
/// store is one bad write from full, so the health event escalates `warn`→`fail`
/// (a `fail` the triage clusters as high severity). Mirrors the panel's
/// `SR_CRITICAL_PCT`. Overridable via `MCNF_SR_CRIT_PCT`.
pub const SR_CRIT_PCT: u64 = 95;

/// One datacenter resource as last sampled: a `kind` (droplet/host/vm/…), a stable
/// `id`, and a `signature` JSON body. The signature is what the brain diffs on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DcResource {
    pub kind: String,
    pub id: String,
    pub signature: String,
}

impl DcResource {
    pub fn new(
        kind: impl Into<String>,
        id: impl Into<String>,
        signature: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
            signature: signature.into(),
        }
    }
    /// The dedup key — unique per resource across kinds.
    fn key(&self) -> String {
        format!("{}/{}", self.kind, self.id)
    }
}

/// One Bus event the orchestrator decided to emit (a resource appeared or changed,
/// or — with `signature` empty — disappeared).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DcEvent {
    pub kind: String,
    pub id: String,
    /// The resource body; empty string ⇒ a `gone` event.
    pub signature: String,
}

impl DcEvent {
    /// Bus topic: `event/dc/<kind>/<id>`.
    #[must_use]
    pub fn topic(&self) -> String {
        format!("event/dc/{}/{}", self.kind, self.id)
    }
    /// JSON body for `mde-bus publish` — the signature for a live resource, or a
    /// `{"gone":true}` marker when the resource vanished.
    #[must_use]
    pub fn body(&self) -> String {
        if self.signature.is_empty() {
            format!(
                r#"{{"kind":"{}","id":"{}","gone":true}}"#,
                self.kind, self.id
            )
        } else {
            self.signature.clone()
        }
    }
}

/// Pure orchestration core: tracks the last-published signature per resource key
/// and returns ONLY the changes (new/changed/gone) on each reconcile — so the Bus
/// never sees a duplicate for an unchanged resource.
#[derive(Default)]
pub struct DatacenterOrchestrator {
    published: BTreeMap<String, (String, String, String)>, // key -> (kind, id, signature)
}

impl DatacenterOrchestrator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconcile against the full current resource set. Emits an event for each
    /// resource whose signature is new or changed, plus a `gone` event for each
    /// previously-seen resource no longer present. Advances internal state.
    pub fn reconcile(&mut self, current: &[DcResource]) -> Vec<DcEvent> {
        let mut events = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for r in current {
            let k = r.key();
            seen.insert(k.clone());
            let changed = self
                .published
                .get(&k)
                .is_none_or(|(_, _, sig)| sig != &r.signature);
            if changed {
                self.published
                    .insert(k, (r.kind.clone(), r.id.clone(), r.signature.clone()));
                events.push(DcEvent {
                    kind: r.kind.clone(),
                    id: r.id.clone(),
                    signature: r.signature.clone(),
                });
            }
        }
        // Anything previously published but now absent → a `gone` event, then drop.
        let absent: Vec<String> = self
            .published
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect();
        for k in absent {
            if let Some((kind, id, _)) = self.published.remove(&k) {
                events.push(DcEvent {
                    kind,
                    id,
                    signature: String::new(),
                });
            }
        }
        events
    }
}

// ---- thin I/O: sample the substrate, emit via the Bus ----

/// The doctl context to read DigitalOcean through (the authed `mackes` context;
/// the `default` context is empty). Overridable for tests/CI.
fn doctl_context() -> String {
    std::env::var("MCNF_DOCTL_CONTEXT").unwrap_or_else(|_| "mackes".to_string())
}

/// Parse `doctl compute droplet list -o json` into DcResources (`kind="droplet"`).
/// Pure — fed the raw JSON. A signature change (status/IP/region) re-publishes.
#[must_use]
pub fn parse_droplets(json: &str) -> Vec<DcResource> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for d in arr {
        let Some(id) = d.get("id").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let name = d.get("name").and_then(|x| x.as_str()).unwrap_or("");
        let status = d.get("status").and_then(|x| x.as_str()).unwrap_or("");
        let region = d
            .get("region")
            .and_then(|r| r.get("slug"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        // public IPv4: first v4 network entry of type "public"
        let ip = d
            .get("networks")
            .and_then(|n| n.get("v4"))
            .and_then(|v| v.as_array())
            .and_then(|a| {
                a.iter()
                    .find(|n| n.get("type").and_then(|t| t.as_str()) == Some("public"))
                    .and_then(|n| n.get("ip_address").and_then(|x| x.as_str()))
            })
            .unwrap_or("");
        let signature = format!(
            r#"{{"kind":"droplet","id":"{id}","name":"{name}","status":"{status}","region":"{region}","ip":"{ip}","zone":"prod"}}"#
        );
        out.push(DcResource::new("droplet", id.to_string(), signature));
    }
    out
}

/// Sample the DigitalOcean zone via `doctl` (best-effort: a missing/failed doctl
/// yields no resources, never an error).
fn gather_do() -> Vec<DcResource> {
    let out = std::process::Command::new("doctl")
        .args([
            "compute",
            "droplet",
            "list",
            "--context",
            &doctl_context(),
            "-o",
            "json",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_droplets(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// dom0s to sample the Xen (dev) zone from — `MCNF_XEN_DOM0S` (comma-separated
/// IPs). Empty by default, so the Xen source is a safe no-op until a node is
/// explicitly configured with dom0 reach (keeps generic mesh nodes unaffected).
pub(crate) fn xen_dom0s() -> Vec<String> {
    std::env::var("MCNF_XEN_DOM0S")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// SSH key used to reach the dom0s (passwordless root via the mesh key).
pub(crate) fn xen_ssh_key() -> String {
    std::env::var("MCNF_XEN_SSH_KEY").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        format!("{home}/.ssh/mackes_mesh_ed25519")
    })
}

// ---- DATACENTER-4: XAPI-over-overlay routing ----------------------------------
//
// The dom0s live on the on-prem lab LAN (`172.20.0.0/16`). A node that is
// physically ON that LAN can `ssh root@<dom0>` directly. A node OFF the LAN (a
// roaming laptop, a cloud lighthouse) has no route to a `172.20.x` address — it
// has to hop through an on-LAN mesh peer that DOES, reaching the dom0 via that
// peer's overlay IP with SSH `-J` (ProxyJump). `MCNF_XEN_RELAY` is that peer's
// overlay IP. Route selection is pure (`resolve_xe_route`) and unit-tested against
// every on_lan/relay combination, exactly like the `parse_*` helpers; the only
// live part is the thin argv assembly + the reachability probe.

/// The `/16` lab LAN the dom0s sit on. A node is "on-LAN" iff it holds a local
/// IPv4 in this network (then it can reach a dom0 directly; otherwise it must
/// relay through an on-LAN peer).
pub(crate) const XEN_LAN_PREFIX: &str = "172.20.";

/// Overlay IP of an on-LAN relay peer to ProxyJump XAPI/SSH through when this node
/// is off-LAN — `MCNF_XEN_RELAY`. Empty/unset by default, so off-LAN nodes with no
/// relay configured simply fall back to a (best-effort, likely-unreachable) direct
/// attempt rather than erroring. Trimmed; empty ⇒ "no relay".
pub(crate) fn xen_relay_peer() -> Option<String> {
    std::env::var("MCNF_XEN_RELAY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The SSH route chosen for an `xe` call to a dom0: straight in, or hopped through
/// a relay peer's overlay IP. Pure output of [`resolve_xe_route`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SshRoute {
    /// `ssh root@<dom0>` — this node is on the lab LAN (or no relay is configured,
    /// so there's nothing better to try).
    Direct,
    /// `ssh -J root@<relay> root@<dom0>` — this node is off-LAN, so reach the dom0
    /// through an on-LAN relay peer at the carried overlay IP.
    ProxyJump(String),
}

impl SshRoute {
    /// Stable path label for the `event/dc/route/xen` Bus signal + logs.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ProxyJump(_) => "relay",
        }
    }
    /// The relay overlay IP, when this route hops through one.
    #[must_use]
    pub fn relay(&self) -> Option<&str> {
        match self {
            Self::Direct => None,
            Self::ProxyJump(r) => Some(r.as_str()),
        }
    }
}

/// Pick the SSH route to a dom0. PURE — the whole point of DATACENTER-4 is that
/// this is decidable from data, not I/O:
/// * on-LAN (or no relay configured) ⇒ [`SshRoute::Direct`];
/// * off-LAN **and** a relay is configured ⇒ [`SshRoute::ProxyJump`] through it.
///
/// The relay never carries the path to a relay reaching itself: if `relay` equals
/// `dom0` (a misconfig) the hop would be pointless, so we go Direct.
#[must_use]
pub fn resolve_xe_route(dom0: &str, on_lan: bool, relay: Option<&str>) -> SshRoute {
    match relay {
        Some(r) if !on_lan && r != dom0 => SshRoute::ProxyJump(r.to_string()),
        _ => SshRoute::Direct,
    }
}

/// Pure: does this node hold a local IPv4 on the dom0 lab LAN? Fed the addresses
/// parsed from `ip -j addr`; a single `172.20.x.y` is enough. Mirrors the pure
/// `parse_*` shape so it's unit-testable without touching the network.
#[must_use]
pub fn node_on_lan_for(local_ipv4s: &[String]) -> bool {
    local_ipv4s.iter().any(|ip| ip.starts_with(XEN_LAN_PREFIX))
}

/// Pull every local IPv4 string out of `ip -j addr` JSON (any interface). Pure —
/// the live wrapper [`node_on_lan`] feeds it real output. Reuses the same JSON
/// shape [`crate::probe_nmap::lan_cidrs_from_ip_json`] reads.
#[must_use]
pub fn local_ipv4s_from_ip_json(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(ifaces) = v.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for iface in ifaces {
        let Some(addrs) = iface.get("addr_info").and_then(|a| a.as_array()) else {
            continue;
        };
        for a in addrs {
            if a.get("family").and_then(|f| f.as_str()) != Some("inet") {
                continue;
            }
            if let Some(ip) = a.get("local").and_then(|l| l.as_str()) {
                out.push(ip.to_string());
            }
        }
    }
    out
}

/// Is this node on the dom0 lab LAN? Best-effort live probe via `ip -j addr`;
/// when `ip` is missing/errors we assume **on-LAN** (Direct) — the conservative
/// default that keeps the existing on-prem behaviour for nodes that were reaching
/// dom0s directly before DATACENTER-4 (those nodes ARE on-LAN). An explicit
/// `MCNF_XEN_ON_LAN=0`/`1` overrides the probe for tests + odd topologies.
fn node_on_lan() -> bool {
    if let Ok(v) = std::env::var("MCNF_XEN_ON_LAN") {
        let v = v.trim();
        if v == "1" || v.eq_ignore_ascii_case("true") {
            return true;
        }
        if v == "0" || v.eq_ignore_ascii_case("false") {
            return false;
        }
    }
    match std::process::Command::new("ip")
        .args(["-j", "addr"])
        .output()
    {
        Ok(o) if o.status.success() => node_on_lan_for(&local_ipv4s_from_ip_json(
            &String::from_utf8_lossy(&o.stdout),
        )),
        // No `ip`/probe failed: keep the legacy direct-reach behaviour.
        _ => true,
    }
}

/// Publish the chosen XAPI route for a dom0 onto the Bus (`event/dc/route/xen`) so
/// the path (direct vs relay) — and any relay-down fallback — is observable mesh
/// state, the same fire-and-reap lane as the resource events.
fn publish_route(dom0: &str, route: &SshRoute, note: &str) {
    let body = serde_json::json!({
        "kind": "route", "id": dom0, "target": "xen",
        "path": route.path(), "relay": route.relay(), "note": note,
    })
    .to_string();
    let topic = format!("event/dc/route/xen/{dom0}");
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Parse the remote `xe` helper's pipe-delimited `uuid|name|power-state` lines
/// into `(uuid, name, power)` triples. Pure — fed the raw stdout.
#[must_use]
pub fn parse_xe_vms(output: &str) -> Vec<(String, String, String)> {
    output
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '|');
            let u = p.next()?.trim();
            if u.is_empty() {
                return None;
            }
            let n = p.next().unwrap_or("").trim();
            let s = p.next().unwrap_or("").trim();
            Some((u.to_string(), n.to_string(), s.to_string()))
        })
        .collect()
}

/// Parse `uuid|name|physical-size|physical-utilisation` lines into SR tuples. Pure.
#[must_use]
pub fn parse_xe_srs(output: &str) -> Vec<(String, String, String, String)> {
    output
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '|');
            let u = p.next()?.trim();
            if u.is_empty() {
                return None;
            }
            let n = p.next().unwrap_or("").trim();
            let sz = p.next().unwrap_or("").trim();
            let used = p.next().unwrap_or("").trim();
            Some((
                u.to_string(),
                n.to_string(),
                sz.to_string(),
                used.to_string(),
            ))
        })
        .collect()
}

/// The SR-capacity **warning** threshold (percent) — `MCNF_SR_WARN_PCT` when set
/// and parseable, else [`SR_WARN_PCT`]. Clamped to `1..=100` so a fat-fingered env
/// value can't disable the alert (0) or sit above full (over 100). Pure-ish (reads
/// the env once per call; the gather calls it once per pass).
#[must_use]
pub fn sr_warn_pct() -> u64 {
    std::env::var("MCNF_SR_WARN_PCT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(SR_WARN_PCT, |p| p.clamp(1, 100))
}

/// The SR-capacity **critical** threshold (percent) — `MCNF_SR_CRIT_PCT` when set
/// and parseable, else [`SR_CRIT_PCT`]. Clamped to `1..=100` for the same reason as
/// [`sr_warn_pct`].
#[must_use]
pub fn sr_crit_pct() -> u64 {
    std::env::var("MCNF_SR_CRIT_PCT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(SR_CRIT_PCT, |p| p.clamp(1, 100))
}

/// DATACENTER-12 — the SR capacity-threshold alert. Given one SR's id/name and its
/// `physical-size` / `physical-utilisation` (the raw `xe` strings the gather already
/// reads), build a health-lane resource ONLY when the store is at or above the
/// `warn_pct` line. PURE — unit-tested without touching XAPI.
///
/// The returned [`DcResource`] is `kind="health"`, `id="sr:<uuid>"`, so its Bus
/// topic is `event/dc/health/sr:<uuid>` — the SAME lane [`super::dc_health`] writes,
/// the one the copilot triage ([`crate::workers::copilot`]) and the panel's
/// `parse_health_event` read. The signature carries the `{check,status,detail}` body
/// those consumers expect (status `warn`, escalating to `fail` at/above `crit_pct`),
/// plus the `kind`/`id` the orchestrator's reconcile needs to emit a well-formed
/// `gone` marker when the store later drops back below the line (the alert clears —
/// the `gone` body has no `status`, so the triage/panel treat it as resolved).
///
/// `None` (no alert) when the store is below `warn_pct`, or when `size`/`used` don't
/// parse or `size` is 0 — a store with no readable capacity isn't an alert, it's
/// just unknown (the gather already skips zero-size SRs upstream).
#[must_use]
pub fn sr_capacity_health(
    id: &str,
    name: &str,
    size: &str,
    used: &str,
    warn_pct: u64,
    crit_pct: u64,
) -> Option<DcResource> {
    let size_b = size.trim().parse::<u128>().ok().filter(|s| *s > 0)?;
    let used_b = used.trim().parse::<u128>().unwrap_or(0);
    // Saturating + clamped: a mid-sample race can read used>size; cap at 100.
    let pct = u64::try_from(used_b.saturating_mul(100) / size_b)
        .unwrap_or(100)
        .min(100);
    if pct < warn_pct {
        return None;
    }
    let label = if name.is_empty() { id } else { name };
    let (status, free_pct) = if pct >= crit_pct {
        ("fail", 100 - pct)
    } else {
        ("warn", 100 - pct)
    };
    let check = format!("sr:{id}");
    let detail = format!("SR '{label}' {pct}% full ({free_pct}% free)");
    let sig = serde_json::json!({
        "kind": "health",
        "id": check,
        "check": check,
        "status": status,
        "detail": detail,
        "pct": pct,
        "zone": "dev",
    })
    .to_string();
    Some(DcResource::new("health", check, sig))
}

/// One VDI (virtual disk) the gather sampled: its uuid/name, the SR it lives on,
/// its virtual size (bytes, as the raw `xe` string), and — when attached — the VBD
/// connecting it and the VM uuid it is attached to (both empty for an unattached
/// disk). The `vbd` is the detach handle; the `vm` is the "attached to" readout.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Vdi {
    /// The VDI uuid.
    pub uuid: String,
    /// The VDI's name-label.
    pub name: String,
    /// The uuid of the SR this VDI lives on (correlates it to its SR card).
    pub sr: String,
    /// The VDI's virtual size in bytes (the raw `xe` string; empty when unknown).
    pub size: String,
    /// The VBD uuid attaching this VDI to a VM, or empty when unattached. The
    /// detach handle the panel's `vdi-detach` passes.
    pub vbd: String,
    /// The uuid of the VM this VDI is attached to, or empty when unattached.
    pub vm: String,
}

/// Parse the VDI gather's pipe-delimited
/// `uuid|name|sr-uuid|virtual-size|vbd-uuid|vm-uuid` lines into [`Vdi`]s. Pure —
/// fed the raw stdout. Skips lines with an empty uuid (mirrors [`parse_xe_srs`]);
/// a missing trailing field (an unattached VDI has empty vbd/vm) defaults to empty.
#[must_use]
pub fn parse_xe_vdis(output: &str) -> Vec<Vdi> {
    output
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(6, '|');
            let uuid = p.next()?.trim();
            if uuid.is_empty() {
                return None;
            }
            Some(Vdi {
                uuid: uuid.to_string(),
                name: p.next().unwrap_or("").trim().to_string(),
                sr: p.next().unwrap_or("").trim().to_string(),
                size: p.next().unwrap_or("").trim().to_string(),
                vbd: p.next().unwrap_or("").trim().to_string(),
                vm: p.next().unwrap_or("").trim().to_string(),
            })
        })
        .collect()
}

/// Parse the remote `xe` network helper's pipe-delimited `uuid|name|bridge`
/// lines into `(uuid, name, bridge)` triples. Pure — fed the raw stdout. Skips
/// lines with an empty uuid (mirrors [`parse_xe_srs`]).
#[must_use]
pub fn parse_xe_nets(output: &str) -> Vec<(String, String, String)> {
    output
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '|');
            let u = p.next()?.trim();
            if u.is_empty() {
                return None;
            }
            let n = p.next().unwrap_or("").trim();
            let b = p.next().unwrap_or("").trim();
            Some((u.to_string(), n.to_string(), b.to_string()))
        })
        .collect()
}

/// Parse the host-metric helper's `cpu|mem_total_mb|mem_free_mb|load` line into
/// its four fields (any missing field → empty string). Pure.
#[must_use]
pub fn parse_host_metrics(line: &str) -> (String, String, String, String) {
    let mut p = line.splitn(4, '|');
    (
        p.next().unwrap_or("").trim().to_string(),
        p.next().unwrap_or("").trim().to_string(),
        p.next().unwrap_or("").trim().to_string(),
        p.next().unwrap_or("").trim().to_string(),
    )
}

/// Assemble the `ssh` argv for one `xe` call, inserting `-J root@<relay>`
/// (ProxyJump) for the relay route. Pure — built from the resolved [`SshRoute`] so
/// the argv shape is unit-testable without spawning ssh.
#[must_use]
pub fn ssh_xe_argv(key: &str, dom0: &str, route: &SshRoute, remote: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "-i".into(),
        key.into(),
        "-o".into(),
        "StrictHostKeyChecking=no".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=8".into(),
    ];
    if let SshRoute::ProxyJump(relay) = route {
        argv.push("-J".into());
        argv.push(format!("root@{relay}"));
    }
    argv.push(format!("root@{dom0}"));
    argv.push(remote.into());
    argv
}

/// Run a remote `xe` command on a dom0 over SSH along an explicit route
/// (best-effort). The argv (incl. any `-J root@<relay>` ProxyJump) comes from
/// [`ssh_xe_argv`], so this is the only spot that actually spawns ssh.
fn ssh_xe(key: &str, dom0: &str, route: &SshRoute, remote: &str) -> Option<String> {
    let o = std::process::Command::new("ssh")
        .args(ssh_xe_argv(key, dom0, route, remote))
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// The per-dom0 XAPI route for one gather pass: the resolved [`SshRoute`] plus a
/// latch so a relay hop that fails once degrades to Direct for the rest of the
/// pass (and is published `relay down` exactly once, not per `xe` call).
struct XenRoute {
    dom0: String,
    route: SshRoute,
    /// Set once a relay hop has been observed down + the Direct fallback published.
    relay_down: bool,
}

impl XenRoute {
    /// Resolve + publish the chosen path for a dom0 at the top of its gather.
    fn open(dom0: &str, on_lan: bool, relay: Option<&str>) -> Self {
        let route = resolve_xe_route(dom0, on_lan, relay);
        publish_route(dom0, &route, route.path());
        Self {
            dom0: dom0.to_string(),
            route,
            relay_down: false,
        }
    }

    /// Run one `xe` call along the current route; if a relay hop fails, latch to
    /// Direct (publishing a `relay down` note once) and retry directly.
    fn run(&mut self, key: &str, remote: &str) -> Option<String> {
        if let Some(out) = ssh_xe(key, &self.dom0, &self.route, remote) {
            return Some(out);
        }
        if matches!(self.route, SshRoute::ProxyJump(_)) {
            if !self.relay_down {
                publish_route(&self.dom0, &SshRoute::Direct, "relay down");
                self.relay_down = true;
            }
            self.route = SshRoute::Direct;
            return ssh_xe(key, &self.dom0, &SshRoute::Direct, remote);
        }
        None
    }
}

/// Sample the Xen (dev) zone: each configured dom0 becomes a `host` resource and
/// each of its non-control VMs a `vm` resource. Reads XAPI via `xe` over the
/// mesh-key SSH (the no-XO read path proven by DATACENTER-1) — best-effort. The SSH
/// is routed per DATACENTER-4 ([`XenRoute`]): Direct on-LAN, ProxyJump through a
/// relay peer off-LAN — without changing the brain or the Bus contract.
fn gather_xen() -> Vec<DcResource> {
    let key = xen_ssh_key();
    // DATACENTER-4: resolve the XAPI route once per pass — on-LAN nodes go Direct,
    // off-LAN nodes ProxyJump through the configured relay peer. Resolved per dom0
    // below (the path is published + can degrade to Direct independently).
    let on_lan = node_on_lan();
    let relay = xen_relay_peer();
    // DATACENTER-12 — SR capacity-alert thresholds, resolved once per pass (env read
    // once, not per SR) and fed to `sr_capacity_health` in the SR gather below.
    let warn_pct = sr_warn_pct();
    let crit_pct = sr_crit_pct();
    let mut out = Vec::new();
    for dom0 in xen_dom0s() {
        let mut route = XenRoute::open(&dom0, on_lan, relay.as_deref());
        // Track this dom0's host name (for the power signal) and its running-VM
        // count so we can emit the DATACENTER-16 idle signal after the gather.
        let mut host_name: Option<String> = None;
        let mut running_vms: usize = 0;
        if let Some(hn) = route.run(&key, "xe host-list params=name-label --minimal") {
            let hn = hn.trim();
            if !hn.is_empty() {
                host_name = Some(hn.to_string());
                // Best-effort host metrics from the Xen toolstack: `xl info` gives
                // the host's REAL physical cpu count + total/free memory (MB), not
                // dom0's capped view; load from /proc/loadavg. One ssh round-trip.
                let metric_script = "L=$(cut -d' ' -f1 /proc/loadavg); I=$(xl info 2>/dev/null); \
                     C=$(echo \"$I\"|awk -F: '/nr_cpus/{gsub(/ /,\"\",$2);print $2}'); \
                     T=$(echo \"$I\"|awk -F: '/total_memory/{gsub(/ /,\"\",$2);print $2}'); \
                     F=$(echo \"$I\"|awk -F: '/free_memory/{gsub(/ /,\"\",$2);print $2}'); \
                     echo \"$C|$T|$F|$L\"";
                let (cpu, mem_total, mem_free, load) = route
                    .run(&key, metric_script)
                    .map(|o| parse_host_metrics(o.trim()))
                    .unwrap_or_default();
                let sig = serde_json::json!({
                    "kind": "host", "id": dom0, "name": hn, "status": "up", "zone": "dev",
                    "cpu": cpu, "mem_total_mb": mem_total, "mem_free_mb": mem_free, "load": load
                })
                .to_string();
                out.push(DcResource::new("host", dom0.clone(), sig));
            }
        }
        let script = "for u in $(xe vm-list is-control-domain=false params=uuid --minimal | tr , ' '); \
             do echo \"$u|$(xe vm-param-get uuid=$u param-name=name-label)|$(xe vm-param-get uuid=$u param-name=power-state)\"; done";
        if let Some(vmout) = route.run(&key, script) {
            for (u, n, s) in parse_xe_vms(&vmout) {
                if s == "running" {
                    running_vms += 1;
                }
                let sig = serde_json::json!({
                    "kind": "vm", "id": u, "name": n, "status": s, "host": dom0, "zone": "dev"
                })
                .to_string();
                out.push(DcResource::new("vm", u, sig));
            }
        }
        // SRs with real capacity (skip the empty/virtual ones) → storage visibility (DC-12).
        let sr_script = "for u in $(xe sr-list params=uuid --minimal | tr , ' '); \
             do ps=$(xe sr-param-get uuid=$u param-name=physical-size 2>/dev/null); \
             [ \"${ps:-0}\" -gt 0 ] || continue; \
             echo \"$u|$(xe sr-param-get uuid=$u param-name=name-label)|$ps|$(xe sr-param-get uuid=$u param-name=physical-utilisation)\"; done";
        if let Some(srout) = route.run(&key, sr_script) {
            for (u, n, size, used) in parse_xe_srs(&srout) {
                // DATACENTER-12 — a filling SR raises a health-lane alert
                // (`event/dc/health/sr:<uuid>`) off the SAME sample, so the gather
                // costs no extra SSH. The reconcile sieve dedups it like any other
                // resource and emits a `gone` (alert cleared) when usage later drops
                // back below the warn line.
                if let Some(alert) = sr_capacity_health(&u, &n, &size, &used, warn_pct, crit_pct) {
                    out.push(alert);
                }
                let sig = serde_json::json!({
                    "kind": "sr", "id": u, "name": n, "size": size, "used": used, "host": dom0, "zone": "dev"
                })
                .to_string();
                out.push(DcResource::new("sr", u, sig));
            }
        }
        // VDIs (the virtual disks living on the SRs) → per-SR disk visibility +
        // attach/detach targets (DC-12). One line per managed VDI:
        // `uuid|name|sr-uuid|virtual-size|vbd-uuid|vm-uuid` — the VBD+VM come from
        // the VDI's first non-empty VBD (its attachment), empty when unattached.
        // `managed=false` VDIs (snapshots/metadata) are skipped to keep the list to
        // real, attachable disks.
        let vdi_script = "for u in $(xe vdi-list managed=true params=uuid --minimal | tr , ' '); \
             do b=$(xe vbd-list vdi-uuid=$u params=uuid --minimal | cut -d, -f1); \
             vm=$(xe vbd-list vdi-uuid=$u params=vm-uuid --minimal | cut -d, -f1); \
             echo \"$u|$(xe vdi-param-get uuid=$u param-name=name-label)|$(xe vdi-param-get uuid=$u param-name=sr-uuid)|$(xe vdi-param-get uuid=$u param-name=virtual-size)|$b|$vm\"; done";
        if let Some(vdiout) = route.run(&key, vdi_script) {
            for vdi in parse_xe_vdis(&vdiout) {
                let sig = serde_json::json!({
                    "kind": "vdi", "id": vdi.uuid, "name": vdi.name, "sr": vdi.sr,
                    "size": vdi.size, "vbd": vdi.vbd, "vm": vdi.vm, "host": dom0, "zone": "dev"
                })
                .to_string();
                out.push(DcResource::new("vdi", vdi.uuid.clone(), sig));
            }
        }
        // Networks (bridges) → network visibility (DC-13).
        let net_script = "for u in $(xe network-list params=uuid --minimal | tr , ' '); \
             do echo \"$u|$(xe network-param-get uuid=$u param-name=name-label)|$(xe network-param-get uuid=$u param-name=bridge)\"; done";
        if let Some(netout) = route.run(&key, net_script) {
            for (u, n, b) in parse_xe_nets(&netout) {
                let sig = serde_json::json!({
                    "kind": "net", "id": u, "name": n, "bridge": b, "host": dom0, "zone": "dev"
                })
                .to_string();
                out.push(DcResource::new("net", u, sig));
            }
        }
        // DATACENTER-16: idle-host (energy) signal — one `power` resource per dom0
        // carrying its running-VM count + an idle hint. READ-ONLY (the panel/operator
        // decides; no auto-shutdown). Emitted only for dom0s whose host was readable,
        // so the name is real. Best-effort, same as the rest of the gather.
        if let Some(hn) = host_name {
            let sig = idle_power_signal(&dom0, &hn, running_vms);
            out.push(DcResource::new("power", dom0.clone(), sig));
        }
    }
    out
}

/// DATACENTER-16 — the idle-host (energy) signal. Build the `power` resource
/// signature for one dom0 from its running-VM count: a READ-ONLY hint the panel
/// (or the operator) can act on. A host with zero running VMs is a
/// `candidate-for-shutdown`; anything running keeps it `in-use`. No auto-shutdown
/// is implied — this only surfaces the signal. Pure.
#[must_use]
pub fn idle_power_signal(dom0: &str, host_name: &str, running_vms: usize) -> String {
    let idle = running_vms == 0;
    serde_json::json!({
        "kind": "power",
        "id": dom0,
        "name": host_name,
        "zone": "dev",
        "running_vms": running_vms,
        "idle": idle,
        "hint": if idle { "candidate-for-shutdown" } else { "in-use" }
    })
    .to_string()
}

/// Host of the on-prem UniFi gateway (the router) to sample over SSH —
/// `MCNF_UNIFI_HOST` (e.g. "172.20.0.1"). Empty/unset by default, so the gateway
/// source is a safe no-op until a node is explicitly configured with reach to the
/// router (mirrors [`xen_dom0s`] keeping generic mesh nodes unaffected).
pub(crate) fn unifi_host() -> String {
    std::env::var("MCNF_UNIFI_HOST")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Parse the UniFi SSH credential as stored in the mesh secret store under
/// `unifi-cred`. The stored value is either `"user:password"` (split once on the
/// first `:`, so passwords containing `:` are preserved) or a bare password, in
/// which case the UniFi factory default user `"ubnt"` is assumed. Returns
/// `(user, password)`, both trimmed. Pure.
#[must_use]
pub fn parse_unifi_cred(raw: &str) -> (String, String) {
    let raw = raw.trim();
    match raw.split_once(':') {
        Some((user, pass)) => (user.trim().to_string(), pass.trim().to_string()),
        None => ("ubnt".to_string(), raw.to_string()),
    }
}

/// Read the UniFi SSH credential from the mesh secret store best-effort by
/// shelling out to `automation/secrets/mcnf-secret.sh get unifi-cred` from the
/// repo root (the worker's current dir). `None` on any failure (helper missing,
/// secret absent, non-zero exit) so the gateway source degrades to a no-op.
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
    Some(parse_unifi_cred(raw))
}

/// Run a remote command on the UniFi gateway over `sshpass` (password auth — the
/// router has no mesh key). Best-effort: `None` on any failure.
fn ssh_unifi(pw: &str, user: &str, host: &str, remote: &str) -> Option<String> {
    let o = std::process::Command::new("sshpass")
        .args([
            "-p",
            pw,
            "ssh",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=8",
            &format!("{user}@{host}"),
            remote,
        ])
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

// ---- DATACENTER-5: storage / net / gateway rollup -----------------------------
//
// The per-resource `event/dc/{sr,net,gateway}/*` deltas above are the truth, but
// the Datacenter **Overview** tab (DC-9) and the Storage/Network sub-tab headers
// want a single per-zone *rollup* — "how much storage total, how full, how many
// networks, is the gateway up" — without re-deriving it from N card events on the
// panel side. [`rollup_zone`] folds a zone's gathered resources into one
// `event/dc/rollup/<zone>` signature; it is PURE (fed the already-gathered
// `DcResource`s), so it's unit-tested exactly like the `parse_*` helpers and adds
// no extra I/O — it reuses the same sample the resource events came from.

/// The folded storage/net/gateway summary for one zone.
///
/// Published as the body of `event/dc/rollup/<zone>` so the panel reads one row per
/// zone instead of summing cards. Counts/bytes are `0` when a zone has no resources
/// of that kind (a clean, always-present rollup rather than a missing field).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ZoneRollup {
    /// Number of storage repositories (`kind="sr"`) seen in the zone.
    pub sr_count: u64,
    /// Summed physical capacity across the zone's SRs, in bytes.
    pub storage_total_bytes: u128,
    /// Summed physical utilisation across the zone's SRs, in bytes.
    pub storage_used_bytes: u128,
    /// Number of networks/bridges (`kind="net"`) seen in the zone.
    pub net_count: u64,
    /// Whether a gateway (`kind="gateway"`) was sampled up in the zone.
    pub gateway_up: bool,
    /// Active DHCP lease count from the gateway (`0` when absent).
    pub gateway_leases: u64,
}

impl ZoneRollup {
    /// Utilisation percent (0–100, integer) of the zone's summed storage, or `0`
    /// when the zone reports no capacity. Saturating + checked so a zero total can
    /// never divide-by-zero and an over-100 reading (mid-sample race) is clamped.
    #[must_use]
    pub fn storage_pct(&self) -> u64 {
        if self.storage_total_bytes == 0 {
            return 0;
        }
        let pct = self.storage_used_bytes.saturating_mul(100) / self.storage_total_bytes;
        u64::try_from(pct).unwrap_or(100).min(100)
    }

    /// The rollup body for the Bus — a stable JSON object keyed for the panel. The
    /// `zone` is carried so a single `event/dc/rollup/*` subscription self-labels.
    #[must_use]
    pub fn signature(&self, zone: &str) -> String {
        serde_json::json!({
            "kind": "rollup",
            "id": zone,
            "zone": zone,
            "sr_count": self.sr_count,
            "storage_total_bytes": self.storage_total_bytes.to_string(),
            "storage_used_bytes": self.storage_used_bytes.to_string(),
            "storage_pct": self.storage_pct(),
            "net_count": self.net_count,
            "gateway_up": self.gateway_up,
            "gateway_leases": self.gateway_leases,
        })
        .to_string()
    }
}

/// Fold a zone's gathered resources into its storage/net/gateway [`ZoneRollup`].
/// PURE — fed the same `DcResource`s the per-resource events came from, filtered to
/// `zone`, so the rollup is always consistent with the cards. Unknown/garbage
/// numeric fields contribute 0 (best-effort, never an error), and only `sr`/`net`/
/// `gateway` kinds participate (hosts/vms/droplets/power are summarised elsewhere).
#[must_use]
pub fn rollup_zone(zone: &str, resources: &[DcResource]) -> ZoneRollup {
    let mut roll = ZoneRollup::default();
    for r in resources {
        // Only this zone's resources contribute.
        let v: serde_json::Value = match serde_json::from_str(&r.signature) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("zone").and_then(|z| z.as_str()) != Some(zone) {
            continue;
        }
        match r.kind.as_str() {
            "sr" => {
                roll.sr_count += 1;
                roll.storage_total_bytes += field_bytes(&v, "size")
                    .or_else(|| str_u128(&v, "size"))
                    .unwrap_or(0);
                roll.storage_used_bytes += field_bytes(&v, "used")
                    .or_else(|| str_u128(&v, "used"))
                    .unwrap_or(0);
            }
            "net" => roll.net_count += 1,
            "gateway" => {
                roll.gateway_up =
                    roll.gateway_up || v.get("status").and_then(|s| s.as_str()) == Some("up");
                roll.gateway_leases += v
                    .get("leases")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
            }
            _ => {}
        }
    }
    roll
}

/// Read a numeric JSON field as `u128` (the SR sizes arrive as JSON numbers when
/// they fit; large byte counts may overflow `u64` JSON and arrive as strings —
/// handled by [`str_u128`]). `None` when absent or non-numeric.
fn field_bytes(v: &serde_json::Value, key: &str) -> Option<u128> {
    v.get(key)
        .and_then(serde_json::Value::as_u64)
        .map(u128::from)
}

/// Read a numeric JSON field that arrived as a string (the `xe` sizes are emitted
/// as quoted strings in the SR signature) into `u128`. `None` when absent/unparsable.
fn str_u128(v: &serde_json::Value, key: &str) -> Option<u128> {
    v.get(key)
        .and_then(|x| x.as_str())
        .and_then(|s| s.trim().parse::<u128>().ok())
}

/// Build the rollup `DcResource`s for every zone present in `resources`.
///
/// A zone with no sampled resources publishes nothing, and a vanished zone's rollup
/// goes `gone` through the normal reconcile path. One `kind="rollup"` resource per
/// zone.
#[must_use]
pub fn rollup_resources(resources: &[DcResource]) -> Vec<DcResource> {
    // Distinct zones present, in stable order.
    let mut zones: Vec<String> = resources
        .iter()
        .filter_map(|r| serde_json::from_str::<serde_json::Value>(&r.signature).ok())
        .filter_map(|v| {
            v.get("zone")
                .and_then(|z| z.as_str())
                .map(std::string::ToString::to_string)
        })
        .collect();
    zones.sort();
    zones.dedup();
    zones
        .into_iter()
        .map(|zone| {
            let roll = rollup_zone(&zone, resources);
            DcResource::new("rollup", zone.clone(), roll.signature(&zone))
        })
        .collect()
}

/// Sample the on-prem UniFi gateway (the dev-zone router): one `gateway` resource
/// carrying its live status and active DHCP lease count. Reads over `sshpass` SSH
/// using the cred from the mesh secret store (DATACENTER-3) — best-effort, so a
/// missing host/cred/sshpass or an unreachable router yields no resource, never an
/// error. Mirrors [`gather_xen`]'s env-gated, fire-and-forget shape.
fn gather_gateway() -> Vec<DcResource> {
    let host = unifi_host();
    if host.is_empty() {
        return Vec::new();
    }
    let Some((user, pw)) = unifi_cred() else {
        return Vec::new();
    };
    // Confirm reach + liveness: the model/uptime banner (`mca-cli-op info`) or, on
    // gateways without it, the kernel uptime. Either succeeding marks the router up.
    let up = ssh_unifi(
        &pw,
        &user,
        &host,
        "mca-cli-op info 2>/dev/null || cat /proc/uptime 2>/dev/null",
    );
    let Some(up) = up.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let _ = up; // liveness probe; presence is what marks the gateway "up".
                // Active DHCP leases — the dhcpd lease file when present, else the reachable
                // neighbour count as a proxy. `0` when neither is readable.
    let leases: u64 = ssh_unifi(
        &pw,
        &user,
        &host,
        "grep -c . /run/dhcpd.leases 2>/dev/null || ip neigh | grep -c REACHABLE",
    )
    .and_then(|s| s.trim().parse::<u64>().ok())
    .unwrap_or(0);
    let sig = serde_json::json!({
        "kind": "gateway", "id": host, "name": "UniFi Gateway",
        "status": "up", "leases": leases, "zone": "dev"
    })
    .to_string();
    vec![DcResource::new("gateway", host, sig)]
}

/// Emit a datacenter event onto the Bus (best-effort, fire-and-reap — same lane
/// shape as the other workers' events).
fn publish(ev: &DcEvent) {
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &ev.topic(), "--body-flag", &ev.body()]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// A datacenter control zone, each with its OWN leader election (the design's
/// "one [worker] per zone" — §3 of `datacenter-control.md`). The two zones elect
/// **independently** off separate lock files so the node that leads Xen need not be
/// the node that leads DO, and losing one zone's leader never disturbs the other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Zone {
    /// The on-prem Xen (dev) zone **plus the UniFi gateway**: its dom0s + router sit
    /// on the `172.20.0.0/16` lab LAN, so only an **on-LAN** node is leader-eligible
    /// (an off-LAN node can read XAPI over a relay but can't be the always-on Xen
    /// control point). Carries hosts/vms/srs/nets/gateway/power.
    Xen,
    /// The DigitalOcean (prod) zone: the DO API is internet-reachable, so **any**
    /// eligible node can be its leader. Carries droplets.
    Do,
}

impl Zone {
    /// The lock-file basename for this zone's independent leader election (under
    /// the workgroup root, beside the shared `.mackesd-leader.lock`).
    const fn lock_name(self) -> &'static str {
        match self {
            Self::Xen => ".mackesd-dc-xen-leader.lock",
            Self::Do => ".mackesd-dc-do-leader.lock",
        }
    }

    /// Is this node ELIGIBLE to lead this zone at all? The Xen/gateway zone is
    /// restricted to on-LAN nodes (the substrate is LAN-only); the DO zone is open
    /// to any node. Ineligible nodes never contend, so they don't churn the lock.
    fn eligible(self, on_lan: bool) -> bool {
        match self {
            Self::Xen => on_lan,
            Self::Do => true,
        }
    }
}

/// The supervised worker. **Per-zone leader-gated**: it runs an independent leader
/// election for each [`Zone`] (Xen+gateway / DO) so a multi-node mesh publishes each
/// zone's deltas from exactly one node, the right-placed one (Xen from an on-LAN
/// node, DO from anywhere) — and killing one zone's leader leaves the other zone
/// publishing uninterrupted. Best-effort throughout.
pub struct DatacenterOrchestratorWorker {
    /// Independent dedup core per zone — a zone we don't lead is left untouched, so
    /// we never emit a spurious `gone` for resources another node owns.
    xen_core: DatacenterOrchestrator,
    do_core: DatacenterOrchestrator,
    tick_interval: Duration,
    node_id: String,
    workgroup_root: PathBuf,
}

impl DatacenterOrchestratorWorker {
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            xen_core: DatacenterOrchestrator::new(),
            do_core: DatacenterOrchestrator::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            workgroup_root,
            node_id,
        }
    }

    /// Does this node currently hold `zone`'s leader lease? Each zone has its own
    /// lock file, so the two elections are fully independent (no-fixed-center: any
    /// eligible node can be it, the elected one publishes).
    fn leads(&self, zone: Zone) -> bool {
        let lock = self.workgroup_root.join(zone.lock_name());
        matches!(
            crate::leader::try_acquire(&lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    fn tick_once(&mut self) {
        // Eligibility is decided once per tick (a single `ip -j addr` probe) and
        // gates which zones this node may even contend for.
        let on_lan = node_on_lan();

        // DO (prod) zone — any eligible node may lead it.
        if Zone::Do.eligible(on_lan) && self.leads(Zone::Do) {
            let current = gather_do();
            let mut events = self.do_core.reconcile(&with_rollup(current));
            for ev in events.drain(..) {
                publish(&ev);
            }
        }

        // Xen (dev) zone + the on-LAN gateway — only an on-LAN node may lead it.
        if Zone::Xen.eligible(on_lan) && self.leads(Zone::Xen) {
            let mut current = gather_xen();
            current.extend(gather_gateway());
            let mut events = self.xen_core.reconcile(&with_rollup(current));
            for ev in events.drain(..) {
                publish(&ev);
            }
        }
    }
}

/// Append the per-zone storage/net/gateway rollup resources to a freshly-gathered
/// set, so the rollup flows through the SAME dedup/`gone` reconcile as the cards it
/// summarises (one `event/dc/rollup/<zone>` per zone present).
fn with_rollup(mut resources: Vec<DcResource>) -> Vec<DcResource> {
    let rollups = rollup_resources(&resources);
    resources.extend(rollups);
    resources
}

#[async_trait::async_trait]
impl Worker for DatacenterOrchestratorWorker {
    fn name(&self) -> &'static str {
        "datacenter_orchestrator"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            self.tick_once();
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_emits_on_new_and_change_only() {
        let mut o = DatacenterOrchestrator::new();
        let r1 = DcResource::new("droplet", "1", r#"{"status":"active"}"#);
        // First sight → one event.
        let e = o.reconcile(&[r1.clone()]);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].topic(), "event/dc/droplet/1");
        // Unchanged → no event.
        assert!(o.reconcile(&[r1.clone()]).is_empty());
        // Signature change → one event.
        let r1b = DcResource::new("droplet", "1", r#"{"status":"off"}"#);
        let e = o.reconcile(&[r1b]);
        assert_eq!(e.len(), 1);
        assert!(e[0].body().contains(r#""status":"off""#));
    }

    #[test]
    fn reconcile_emits_gone_when_absent() {
        let mut o = DatacenterOrchestrator::new();
        o.reconcile(&[DcResource::new("droplet", "1", "{}")]);
        // Now absent → a gone event, then forgotten.
        let e = o.reconcile(&[]);
        assert_eq!(e.len(), 1);
        assert!(e[0].body().contains(r#""gone":true"#));
        assert_eq!(e[0].topic(), "event/dc/droplet/1");
        // Re-appears → seen as new again.
        let e = o.reconcile(&[DcResource::new("droplet", "1", "{}")]);
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn parse_droplets_reads_id_status_region_ip() {
        let json = r#"[
          {"id":579112110,"name":"lighthouse-01","status":"active",
           "region":{"slug":"nyc3"},
           "networks":{"v4":[{"type":"private","ip_address":"10.0.0.3"},
                             {"type":"public","ip_address":"174.138.68.216"}]}}
        ]"#;
        let r = parse_droplets(json);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, "droplet");
        assert_eq!(r[0].id, "579112110");
        assert!(r[0].signature.contains(r#""status":"active""#));
        assert!(r[0].signature.contains(r#""region":"nyc3""#));
        assert!(r[0].signature.contains(r#""ip":"174.138.68.216""#));
        assert!(r[0].signature.contains(r#""zone":"prod""#));
    }

    #[test]
    fn parse_droplets_tolerates_garbage() {
        assert!(parse_droplets("not json").is_empty());
        assert!(parse_droplets("{}").is_empty());
        assert!(parse_droplets("[]").is_empty());
    }

    #[test]
    fn parse_xe_vms_reads_pipe_lines() {
        let out = "abc-1|mcnf-build-51|running\ndef-2|mcnf-golden|halted\n|skip-empty-uuid|x\n";
        let vms = parse_xe_vms(out);
        assert_eq!(vms.len(), 2); // the empty-uuid line is skipped
        assert_eq!(
            vms[0],
            ("abc-1".into(), "mcnf-build-51".into(), "running".into())
        );
        assert_eq!(vms[1].1, "mcnf-golden");
        assert_eq!(vms[1].2, "halted");
    }

    #[test]
    fn parse_xe_nets_reads_pipe_lines() {
        let out = "n1|Pool-wide network associated with eth0|xenbr0\nn2|Host internal management network|xenapi\n|skip-empty-uuid|br0\n";
        let nets = parse_xe_nets(out);
        assert_eq!(nets.len(), 2); // the empty-uuid line is skipped
        assert_eq!(
            nets[0],
            (
                "n1".into(),
                "Pool-wide network associated with eth0".into(),
                "xenbr0".into()
            )
        );
        assert_eq!(nets[1].1, "Host internal management network");
        assert_eq!(nets[1].2, "xenapi");
    }

    #[test]
    fn parse_host_metrics_splits_four_fields() {
        let (c, t, f, l) = parse_host_metrics("4|23469|2171|0.15");
        assert_eq!(
            (c.as_str(), t.as_str(), f.as_str(), l.as_str()),
            ("4", "23469", "2171", "0.15")
        );
        // missing fields (xl absent) → empty, load still present
        let (c, t, f, l) = parse_host_metrics("||0.00");
        assert_eq!(c, "");
        assert_eq!(t, "");
        assert_eq!(f, "0.00");
        assert_eq!(l, "");
    }

    #[test]
    fn parse_unifi_cred_handles_user_pass_and_bare() {
        // "user:password" → split once on the first ':'.
        assert_eq!(
            parse_unifi_cred("admin:s3cret"),
            ("admin".to_string(), "s3cret".to_string())
        );
        // Password containing ':' is preserved (split_once, not splitn-everywhere).
        assert_eq!(
            parse_unifi_cred("admin:a:b:c"),
            ("admin".to_string(), "a:b:c".to_string())
        );
        // Bare password → factory-default "ubnt" user.
        assert_eq!(
            parse_unifi_cred("hunter2"),
            ("ubnt".to_string(), "hunter2".to_string())
        );
        // Surrounding whitespace (trailing newline from the secret helper) is trimmed.
        assert_eq!(
            parse_unifi_cred("  ubnt:pw  \n"),
            ("ubnt".to_string(), "pw".to_string())
        );
    }

    #[test]
    fn idle_power_signal_marks_idle_when_no_running_vms() {
        let sig = idle_power_signal("172.20.0.9", "xcp-host-a", 0);
        let v: serde_json::Value = serde_json::from_str(&sig).unwrap();
        assert_eq!(v["kind"], "power");
        assert_eq!(v["id"], "172.20.0.9");
        assert_eq!(v["name"], "xcp-host-a");
        assert_eq!(v["zone"], "dev");
        assert_eq!(v["running_vms"], 0);
        assert_eq!(v["idle"], true);
        assert_eq!(v["hint"], "candidate-for-shutdown");
    }

    #[test]
    fn idle_power_signal_marks_in_use_when_vms_running() {
        let sig = idle_power_signal("172.20.145.193", "xcp-host-b", 3);
        let v: serde_json::Value = serde_json::from_str(&sig).unwrap();
        assert_eq!(v["running_vms"], 3);
        assert_eq!(v["idle"], false);
        assert_eq!(v["hint"], "in-use");
    }

    #[test]
    fn parse_xe_srs_reads_capacity() {
        let out = "s1|Local storage|207296921600|42949672960\n|skip||\n";
        let srs = parse_xe_srs(out);
        assert_eq!(srs.len(), 1); // empty-uuid line skipped
        assert_eq!(srs[0].0, "s1");
        assert_eq!(srs[0].1, "Local storage");
        assert_eq!(srs[0].2, "207296921600");
        assert_eq!(srs[0].3, "42949672960");
    }

    #[test]
    fn parse_xe_vdis_reads_attachment() {
        // An attached VDI (vbd+vm present) and an unattached one (trailing empties).
        let out = "v1|disk0|sr-9|42949672960|vbd-7|vm-3\n\
                   v2|spare|sr-9|10737418240||\n\
                   |skip|||\n";
        let vdis = parse_xe_vdis(out);
        assert_eq!(vdis.len(), 2); // empty-uuid line skipped
        assert_eq!(vdis[0].uuid, "v1");
        assert_eq!(vdis[0].name, "disk0");
        assert_eq!(vdis[0].sr, "sr-9");
        assert_eq!(vdis[0].size, "42949672960");
        assert_eq!(vdis[0].vbd, "vbd-7");
        assert_eq!(vdis[0].vm, "vm-3");
        // Unattached: vbd + vm are empty.
        assert_eq!(vdis[1].uuid, "v2");
        assert_eq!(vdis[1].vbd, "");
        assert_eq!(vdis[1].vm, "");
    }

    // ---- DATACENTER-12: SR capacity-threshold alert ----------------------------

    #[test]
    fn sr_capacity_health_quiet_below_warn() {
        // 20% full, warn at 85 → no alert.
        assert!(sr_capacity_health("s1", "Local", "100", "20", 85, 95).is_none());
        // Exactly one below the warn line → still quiet.
        assert!(sr_capacity_health("s1", "Local", "100", "84", 85, 95).is_none());
    }

    #[test]
    fn sr_capacity_health_warns_at_threshold() {
        // 90% full, warn 85 / crit 95 → a warn on the health lane.
        let r = sr_capacity_health("abc-123", "Local storage", "100", "90", 85, 95)
            .expect("90% >= 85% warns");
        assert_eq!(r.kind, "health");
        assert_eq!(r.id, "sr:abc-123");
        // Topic is the health lane the triage/panel read.
        let ev = DcEvent {
            kind: r.kind.clone(),
            id: r.id.clone(),
            signature: r.signature.clone(),
        };
        assert_eq!(ev.topic(), "event/dc/health/sr:abc-123");
        let v: serde_json::Value = serde_json::from_str(&r.signature).unwrap();
        assert_eq!(v["check"], "sr:abc-123");
        assert_eq!(v["status"], "warn");
        assert_eq!(v["pct"], 90);
        assert!(v["detail"].as_str().unwrap().contains("Local storage"));
        assert!(v["detail"].as_str().unwrap().contains("90% full"));
    }

    #[test]
    fn sr_capacity_health_escalates_to_fail_at_critical() {
        // 96% full, crit at 95 → status escalates warn→fail (high-severity triage).
        let r = sr_capacity_health("s1", "", "100", "96", 85, 95).expect("96% >= 95% fails");
        let v: serde_json::Value = serde_json::from_str(&r.signature).unwrap();
        assert_eq!(v["status"], "fail");
        // No name → the detail falls back to the id.
        assert!(v["detail"].as_str().unwrap().contains("s1"));
    }

    #[test]
    fn sr_capacity_health_handles_bad_or_full_sizes() {
        // Unparseable / zero size → no alert (unknown capacity isn't an alert).
        assert!(sr_capacity_health("s1", "n", "", "10", 85, 95).is_none());
        assert!(sr_capacity_health("s1", "n", "0", "10", 85, 95).is_none());
        assert!(sr_capacity_health("s1", "n", "notnum", "10", 85, 95).is_none());
        // used > size (a mid-sample race) clamps pct to 100, still a fail.
        let r = sr_capacity_health("s1", "n", "100", "150", 85, 95).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r.signature).unwrap();
        assert_eq!(v["pct"], 100);
        assert_eq!(v["status"], "fail");
    }

    #[test]
    fn sr_capacity_health_clears_via_reconcile_gone() {
        // An over-threshold SR publishes a health alert; when it drops back below
        // the line the next pass omits it and reconcile emits a `gone` (the
        // alert-cleared marker the panel/triage read as resolved).
        let mut core = DatacenterOrchestrator::new();
        let hot = sr_capacity_health("s1", "Local", "100", "90", 85, 95).unwrap();
        let evs = core.reconcile(&[hot]);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].topic(), "event/dc/health/sr:s1");
        // Next pass: the SR is now only 50% full → no alert resource → a `gone`.
        let evs = core.reconcile(&[]);
        assert_eq!(evs.len(), 1);
        assert!(evs[0].body().contains(r#""gone":true"#));
        // The `gone` body has no `status`, so the panel's health parse drops it.
        assert!(!evs[0].body().contains(r#""status""#));
    }

    #[test]
    fn sr_warn_and_crit_pct_defaults_and_clamp() {
        // Defaults when the env is unset (the common case in CI).
        std::env::remove_var("MCNF_SR_WARN_PCT");
        std::env::remove_var("MCNF_SR_CRIT_PCT");
        assert_eq!(sr_warn_pct(), SR_WARN_PCT);
        assert_eq!(sr_crit_pct(), SR_CRIT_PCT);
    }

    // ---- DATACENTER-4: XAPI-over-overlay route selection -----------------------

    #[test]
    fn resolve_xe_route_direct_when_on_lan() {
        // On-LAN: always Direct, even with a relay configured (no need to hop).
        assert_eq!(
            resolve_xe_route("172.20.0.9", true, Some("10.42.0.7")),
            SshRoute::Direct
        );
        assert_eq!(resolve_xe_route("172.20.0.9", true, None), SshRoute::Direct);
    }

    #[test]
    fn resolve_xe_route_proxyjump_when_off_lan_with_relay() {
        // Off-LAN + a relay ⇒ hop through the relay's overlay IP.
        let r = resolve_xe_route("172.20.0.9", false, Some("10.42.0.7"));
        assert_eq!(r, SshRoute::ProxyJump("10.42.0.7".to_string()));
        assert_eq!(r.path(), "relay");
        assert_eq!(r.relay(), Some("10.42.0.7"));
    }

    #[test]
    fn resolve_xe_route_falls_back_direct_off_lan_no_relay() {
        // Off-LAN but no relay configured ⇒ a (best-effort) Direct attempt, not an
        // error — keeps a misconfigured off-LAN node degrading cleanly.
        let r = resolve_xe_route("172.20.0.9", false, None);
        assert_eq!(r, SshRoute::Direct);
        assert_eq!(r.path(), "direct");
        assert_eq!(r.relay(), None);
    }

    #[test]
    fn resolve_xe_route_ignores_self_relay() {
        // A relay equal to the dom0 is a pointless hop ⇒ Direct.
        assert_eq!(
            resolve_xe_route("172.20.0.9", false, Some("172.20.0.9")),
            SshRoute::Direct
        );
    }

    #[test]
    fn ssh_xe_argv_inserts_proxyjump_only_for_relay() {
        // Direct: no `-J`, ends with `root@<dom0>` then the remote command.
        let direct = ssh_xe_argv("/k", "172.20.0.9", &SshRoute::Direct, "xe host-list");
        assert!(!direct.iter().any(|a| a == "-J"));
        assert_eq!(direct[0], "-i");
        assert_eq!(direct[1], "/k");
        assert_eq!(direct[direct.len() - 2], "root@172.20.0.9");
        assert_eq!(direct[direct.len() - 1], "xe host-list");

        // Relay: a `-J root@<relay>` pair sits before the `root@<dom0>` target.
        let relayed = ssh_xe_argv(
            "/k",
            "172.20.0.9",
            &SshRoute::ProxyJump("10.42.0.7".into()),
            "xe host-list",
        );
        let j = relayed.iter().position(|a| a == "-J").expect("has -J");
        assert_eq!(relayed[j + 1], "root@10.42.0.7");
        let target = relayed
            .iter()
            .position(|a| a == "root@172.20.0.9")
            .expect("has dom0 target");
        assert!(j < target, "ProxyJump must precede the dom0 target");
        assert_eq!(relayed[relayed.len() - 1], "xe host-list");
    }

    #[test]
    fn node_on_lan_for_detects_lab_lan_ipv4() {
        // A 172.20.x address ⇒ on-LAN.
        assert!(node_on_lan_for(&[
            "127.0.0.1".to_string(),
            "172.20.145.192".to_string()
        ]));
        // Only an overlay + loopback ⇒ off-LAN.
        assert!(!node_on_lan_for(&[
            "127.0.0.1".to_string(),
            "10.42.0.7".to_string()
        ]));
        // No addresses ⇒ off-LAN.
        assert!(!node_on_lan_for(&[]));
    }

    #[test]
    fn local_ipv4s_reads_inet_addrs_only() {
        let json = r#"[
          {"ifname":"lo","addr_info":[{"family":"inet","local":"127.0.0.1","prefixlen":8}]},
          {"ifname":"eth0","addr_info":[
             {"family":"inet","local":"172.20.145.192","prefixlen":16},
             {"family":"inet6","local":"fe80::1","prefixlen":64}]},
          {"ifname":"nebula1","addr_info":[{"family":"inet","local":"10.42.0.7","prefixlen":24}]}
        ]"#;
        let ips = local_ipv4s_from_ip_json(json);
        assert_eq!(ips, vec!["127.0.0.1", "172.20.145.192", "10.42.0.7"]); // v6 skipped
        assert!(node_on_lan_for(&ips));
    }

    #[test]
    fn local_ipv4s_tolerates_garbage() {
        assert!(local_ipv4s_from_ip_json("not json").is_empty());
        assert!(local_ipv4s_from_ip_json("{}").is_empty());
        assert!(local_ipv4s_from_ip_json("[]").is_empty());
    }

    // ---- DATACENTER-5: per-zone leaders -----------------------------------------

    #[test]
    fn zone_eligibility_gates_xen_to_on_lan_only() {
        // The Xen/gateway zone is LAN-only; the DO zone is open to any node.
        assert!(Zone::Xen.eligible(true));
        assert!(!Zone::Xen.eligible(false));
        assert!(Zone::Do.eligible(true));
        assert!(Zone::Do.eligible(false));
    }

    #[test]
    fn zone_lock_names_are_distinct_so_elections_are_independent() {
        // Two different lock files ⇒ the Xen leader and the DO leader can be
        // different nodes, and one zone's leader loss never touches the other.
        assert_ne!(Zone::Xen.lock_name(), Zone::Do.lock_name());
        assert!(Zone::Xen.lock_name().contains("xen"));
        assert!(Zone::Do.lock_name().contains("do"));
    }

    // ---- DATACENTER-5: storage / net / gateway rollup ---------------------------

    fn sr(id: &str, zone: &str, size: &str, used: &str) -> DcResource {
        let sig = serde_json::json!({
            "kind":"sr","id":id,"name":id,"size":size,"used":used,"zone":zone
        })
        .to_string();
        DcResource::new("sr", id, sig)
    }
    fn net(id: &str, zone: &str) -> DcResource {
        let sig = serde_json::json!({"kind":"net","id":id,"name":id,"zone":zone}).to_string();
        DcResource::new("net", id, sig)
    }
    fn gateway(id: &str, zone: &str, up: bool, leases: u64) -> DcResource {
        let sig = serde_json::json!({
            "kind":"gateway","id":id,"status": if up {"up"} else {"down"},"leases":leases,"zone":zone
        })
        .to_string();
        DcResource::new("gateway", id, sig)
    }

    #[test]
    fn rollup_zone_sums_storage_counts_nets_and_reads_gateway() {
        let res = vec![
            sr("s1", "dev", "200", "50"),
            sr("s2", "dev", "100", "50"),
            net("n1", "dev"),
            net("n2", "dev"),
            net("n3", "dev"),
            gateway("g1", "dev", true, 42),
            // a prod resource must NOT contribute to the dev rollup
            sr("s9", "prod", "999", "999"),
        ];
        let roll = rollup_zone("dev", &res);
        assert_eq!(roll.sr_count, 2);
        assert_eq!(roll.storage_total_bytes, 300);
        assert_eq!(roll.storage_used_bytes, 100);
        assert_eq!(roll.storage_pct(), 33); // 100/300
        assert_eq!(roll.net_count, 3);
        assert!(roll.gateway_up);
        assert_eq!(roll.gateway_leases, 42);
    }

    #[test]
    fn rollup_zone_storage_pct_is_zero_when_no_capacity() {
        let roll = rollup_zone("dev", &[net("n1", "dev")]);
        assert_eq!(roll.storage_total_bytes, 0);
        assert_eq!(roll.storage_pct(), 0); // no divide-by-zero
        assert_eq!(roll.net_count, 1);
        assert!(!roll.gateway_up);
    }

    #[test]
    fn rollup_zone_clamps_over_full_storage_to_100() {
        // A mid-sample read where used > total must clamp, never exceed 100.
        let roll = rollup_zone("dev", &[sr("s1", "dev", "100", "150")]);
        assert_eq!(roll.storage_pct(), 100);
    }

    #[test]
    fn rollup_resources_emits_one_rollup_per_zone() {
        let res = vec![
            sr("s1", "dev", "100", "10"),
            net("n1", "prod"),
            gateway("g1", "dev", true, 5),
        ];
        let rolls = rollup_resources(&res);
        // One rollup per distinct zone (dev, prod).
        assert_eq!(rolls.len(), 2);
        assert!(rolls.iter().all(|r| r.kind == "rollup"));
        let dev = rolls.iter().find(|r| r.id == "dev").expect("dev rollup");
        let v: serde_json::Value = serde_json::from_str(&dev.signature).unwrap();
        assert_eq!(v["zone"], "dev");
        assert_eq!(v["sr_count"], 1);
        assert_eq!(v["gateway_up"], true);
        assert_eq!(v["storage_total_bytes"], "100");
        assert_eq!(v["storage_pct"], 10);
        // Topic is the panel-facing `event/dc/rollup/<zone>`.
        let ev = DcEvent {
            kind: dev.kind.clone(),
            id: dev.id.clone(),
            signature: dev.signature.clone(),
        };
        assert_eq!(ev.topic(), "event/dc/rollup/dev");
    }

    #[test]
    fn rollup_handles_string_byte_sizes_from_xe() {
        // The `xe` SR sizes arrive as quoted-string bytes (can exceed u64-as-JSON).
        let res = vec![sr("s1", "dev", "207296921600", "42949672960")];
        let roll = rollup_zone("dev", &res);
        assert_eq!(roll.storage_total_bytes, 207_296_921_600);
        assert_eq!(roll.storage_used_bytes, 42_949_672_960);
        assert_eq!(roll.storage_pct(), 20); // ~20.7%
    }

    #[test]
    fn rollup_flows_through_reconcile_dedup_and_gone() {
        // A rollup resource is just another DcResource: new→event, unchanged→silent,
        // absent→gone. Proves the rollup rides the same Bus contract as the cards.
        let mut core = DatacenterOrchestrator::new();
        let with = with_rollup(vec![sr("s1", "dev", "100", "10")]);
        let ev = core.reconcile(&with);
        assert!(ev.iter().any(|e| e.topic() == "event/dc/rollup/dev"));
        // Same sample again → no rollup event (deduped).
        assert!(core
            .reconcile(&with_rollup(vec![sr("s1", "dev", "100", "10")]))
            .iter()
            .all(|e| e.topic() != "event/dc/rollup/dev"));
        // Zone empties → the rollup goes `gone`.
        let gone = core.reconcile(&[]);
        let roll_gone = gone
            .iter()
            .find(|e| e.topic() == "event/dc/rollup/dev")
            .expect("rollup gone");
        assert!(roll_gone.body().contains(r#""gone":true"#));
    }
}
