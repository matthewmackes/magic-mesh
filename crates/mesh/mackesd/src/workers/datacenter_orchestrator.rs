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
//! DATACENTER-5 (this increment): **per-zone leaders** + a **storage/net/gateway
//! rollup**. The single shared `.mackesd-leader.lock` is split into one lock *per
//! zone* ([`Zone::lock_name`]) so each substrate is led independently:
//! * the **DO / global** zone can be led by *any* eligible node (it reaches DO over
//!   the public internet);
//! * the **Xen** + **gateway** zones are **on-LAN-gated** — only a node that holds a
//!   `172.20.x` lab-LAN address ([`node_on_lan`]) may take their leadership, because
//!   only an on-LAN node can `xe`/`ssh` the dom0s and the router. An off-LAN node
//!   simply never contends for those locks, so leadership lands where the reach is.
//!
//! Killing a zone leader still re-elects within the 60 s lease window
//! ([`crate::leader`]) — now independently per zone. On top of the per-resource
//! events the leader also publishes one **rollup** per zone
//! (`event/dc/rollup/<zone>`): a deduped storage/net/gateway/compute summary
//! ([`zone_rollups`]) so the cross-zone capacity headline is first-class mesh state,
//! not only a panel-local reduction.
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
    ///
    /// Full-scope: every previously-seen resource is a gone candidate. The worker
    /// uses [`Self::reconcile_scoped`] so it only gone's resources in the zones it
    /// currently leads.
    pub fn reconcile(&mut self, current: &[DcResource]) -> Vec<DcEvent> {
        self.reconcile_scoped(current, None)
    }

    /// DATACENTER-5 — reconcile, scoping gone-detection to `active_zones`.
    ///
    /// New/changed emission is unconditional (we only ever gather the zones we
    /// lead). The gone sweep is the subtle part under per-zone leadership: when this
    /// node *loses* a zone's leadership, that zone's resources vanish from `current`
    /// — but they're still alive (the new zone leader publishes them now), so we must
    /// NOT emit a flood of false `gone` events for them. `active_zones = Some(set)`
    /// restricts gone-detection (and the forget) to resources whose stored `zone`
    /// matches the set; a resource in an un-led zone is left frozen in `published`
    /// (no re-emit, no gone), exactly mirroring the old whole-worker lost-leadership
    /// freeze. `None` ⇒ every zone active (the plain [`Self::reconcile`] contract).
    pub fn reconcile_scoped(
        &mut self,
        current: &[DcResource],
        active_zones: Option<&BTreeSet<String>>,
    ) -> Vec<DcEvent> {
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
        // Anything previously published, absent now, AND in an active (led) zone →
        // a real `gone` event, then drop. Resources in un-led zones are left
        // untouched (frozen) so losing a zone's leadership doesn't false-gone it.
        let absent: Vec<String> = self
            .published
            .iter()
            .filter(|(k, (_, _, sig))| {
                !seen.contains(*k)
                    && active_zones.is_none_or(|zs| zs.contains(&signature_zone(sig)))
            })
            .map(|(k, _)| k.clone())
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

// ---- per-zone leadership ------------------------------------------------------
//
// DATACENTER-5: each substrate is led independently. The DO/global zone reaches
// DigitalOcean over the public internet, so ANY eligible node can lead it. The Xen
// + gateway zones live on the `172.20.x` lab LAN, so only an ON-LAN node can reach
// them — and so only an on-LAN node should ever take their leadership. Splitting the
// one shared lock into a lock-per-zone is what makes "kill the leader → a survivor
// re-assumes" hold *per zone*: a roaming laptop that was the DO leader dropping off
// doesn't strand the Xen zone (a different on-LAN node already leads it), and vice
// versa.

/// One datacenter zone the orchestrator leads + samples independently. The variant
/// set is closed: the substrate is DO (production), Xen (dev hosts/VMs/SR/net), and
/// the on-prem UniFi gateway.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    /// DigitalOcean / production — reachable over the public internet from anywhere.
    Do,
    /// The on-prem Xen (XCP-ng) dev substrate — dom0s on the lab LAN.
    Xen,
    /// The on-prem UniFi gateway (the lab router) — also on the lab LAN.
    Gateway,
}

impl Zone {
    /// Every zone, in sample order (DO first — it's the always-available one).
    pub const ALL: [Self; 3] = [Self::Do, Self::Xen, Self::Gateway];

    /// Stable token used in the per-zone leader lock filename + logs.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Do => "do",
            Self::Xen => "xen",
            Self::Gateway => "gateway",
        }
    }

    /// The `zone` field a resource from this zone carries in its signature — the
    /// label the panel groups on and the brain's gone-scope matches against. DO ⇒
    /// `prod`; Xen + gateway are both the on-prem `dev` substrate. (Two distinct
    /// leader *locks* can share one resource-zone label — the gateway rides the
    /// dev zone's rollup.)
    #[must_use]
    pub const fn resource_zone(self) -> &'static str {
        match self {
            Self::Do => "prod",
            Self::Xen | Self::Gateway => "dev",
        }
    }

    /// The per-zone leader-lock filename under the workgroup root. A distinct lock
    /// per zone means each zone elects its own leader (60 s lease, [`crate::leader`])
    /// without contending with the others.
    #[must_use]
    pub fn lock_name(self) -> String {
        format!(".mackesd-dc-{}-leader.lock", self.token())
    }

    /// Is a node eligible to lead this zone given its on-LAN status? The DO zone is
    /// reachable from anywhere (always eligible); the Xen + gateway zones require an
    /// on-LAN node (only it can reach the dom0s / router), so an off-LAN node never
    /// contends for them. PURE — decidable from data, unit-tested without I/O.
    #[must_use]
    pub const fn eligible(self, on_lan: bool) -> bool {
        match self {
            Self::Do => true,
            Self::Xen | Self::Gateway => on_lan,
        }
    }
}

// ---- DATACENTER-5: the storage/net/gateway (+ compute) rollup -----------------
//
// Mirrors `fleet_rollup`'s shape: a pure reduction over the gathered resources into
// one compact per-zone summary the panel's Overview can read straight off the Bus
// (`event/dc/rollup/<zone>`) instead of re-deriving it from every row. The headline
// numbers are storage (SR count + summed bytes), network count, gateway up/leases,
// and the compute footprint (hosts/VMs/droplets) — exactly the rollup the
// Datacenter plane wants.

/// One zone's storage/net/gateway/compute rollup, summed from that zone's resources.
/// Pure data; serialized into the `event/dc/rollup/<zone>` signature.
#[derive(Clone, PartialEq, Eq, Debug, Default, serde::Serialize)]
pub struct ZoneRollup {
    /// Zone token (`prod` for DO, `dev` for Xen — the same `zone` field the
    /// per-resource signatures carry, so the panel groups them consistently).
    pub zone: String,
    /// Hosts (dom0s) in the zone.
    pub hosts: usize,
    /// VMs in the zone.
    pub vms: usize,
    /// Droplets in the zone.
    pub droplets: usize,
    /// Storage repositories (SRs) in the zone.
    pub srs: usize,
    /// Summed SR physical size (bytes) across the zone's storage.
    pub storage_total_bytes: u64,
    /// Summed SR physical utilisation (bytes) across the zone's storage.
    pub storage_used_bytes: u64,
    /// Networks (bridges) in the zone.
    pub nets: usize,
    /// Whether the zone's gateway reported up (only the gateway zone has one).
    pub gateway_up: bool,
    /// Active DHCP leases reported by the gateway (0 when there's no gateway).
    pub gateway_leases: u64,
}

/// The `zone` field carried in a resource's signature JSON (`"prod"` / `"dev"`),
/// or `""` when it has none. Pure; reads the already-built signature so the rollup
/// (and the brain's gone-scope) reuse exactly the zone the per-resource events
/// publish.
fn signature_zone(sig: &str) -> String {
    serde_json::from_str::<serde_json::Value>(sig)
        .ok()
        .and_then(|v| {
            v.get("zone")
                .and_then(|z| z.as_str())
                .map(ToString::to_string)
        })
        .unwrap_or_default()
}

/// Read a numeric field out of a resource signature, tolerating both a JSON number
/// and a JSON string (the `xe` helpers emit byte sizes as strings). `0` when absent
/// or unparseable. Pure.
fn sig_u64(sig: &serde_json::Value, field: &str) -> u64 {
    match sig.get(field) {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Reduce a full resource set into one [`ZoneRollup`] per non-empty zone, as
/// `rollup`-kind [`DcResource`]s (id = the zone token) so they flow through the same
/// dedup brain + Bus contract as every other event. PURE — fed the gathered
/// resources, no I/O, fully unit-tested. A zone with no resources produces no
/// rollup (so we never publish an all-zero `prod`/`dev` card before the first
/// sample lands).
#[must_use]
pub fn zone_rollups(resources: &[DcResource]) -> Vec<DcResource> {
    let mut by_zone: BTreeMap<String, ZoneRollup> = BTreeMap::new();
    for r in resources {
        // The rollup is itself a resource kind — never roll the rollups up.
        if r.kind == "rollup" {
            continue;
        }
        let zone = signature_zone(&r.signature);
        if zone.is_empty() {
            continue;
        }
        let entry = by_zone.entry(zone.clone()).or_default();
        entry.zone = zone;
        let sig: serde_json::Value =
            serde_json::from_str(&r.signature).unwrap_or(serde_json::Value::Null);
        match r.kind.as_str() {
            "host" => entry.hosts += 1,
            "vm" => entry.vms += 1,
            "droplet" => entry.droplets += 1,
            "sr" => {
                entry.srs += 1;
                entry.storage_total_bytes += sig_u64(&sig, "size");
                entry.storage_used_bytes += sig_u64(&sig, "used");
            }
            "net" => entry.nets += 1,
            "gateway" => {
                entry.gateway_up = sig.get("status").and_then(|s| s.as_str()) == Some("up");
                entry.gateway_leases += sig_u64(&sig, "leases");
            }
            _ => {}
        }
    }
    by_zone
        .into_values()
        .map(|roll| {
            let id = roll.zone.clone();
            let signature = serde_json::json!({
                "kind": "rollup",
                "id": id,
                "zone": roll.zone,
                "hosts": roll.hosts,
                "vms": roll.vms,
                "droplets": roll.droplets,
                "srs": roll.srs,
                "storage_total_bytes": roll.storage_total_bytes,
                "storage_used_bytes": roll.storage_used_bytes,
                "nets": roll.nets,
                "gateway_up": roll.gateway_up,
                "gateway_leases": roll.gateway_leases,
            })
            .to_string();
            DcResource::new("rollup", id, signature)
        })
        .collect()
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
                let sig = serde_json::json!({
                    "kind": "sr", "id": u, "name": n, "size": size, "used": used, "host": dom0, "zone": "dev"
                })
                .to_string();
                out.push(DcResource::new("sr", u, sig));
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

/// The supervised worker. **Per-zone leader-gated** (DATACENTER-5): each zone is
/// led independently — the DO/global zone by any eligible node, the Xen + gateway
/// zones by an on-LAN node — so a multi-node mesh publishes each change once and
/// killing one zone's leader re-elects only that zone. Best-effort throughout.
pub struct DatacenterOrchestratorWorker {
    core: DatacenterOrchestrator,
    tick_interval: Duration,
    node_id: String,
    /// The workgroup root — the per-zone leader locks live directly under it
    /// ([`Zone::lock_name`]), the same dir the shared `.mackesd-leader.lock` did.
    workgroup_root: PathBuf,
}

impl DatacenterOrchestratorWorker {
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            core: DatacenterOrchestrator::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            workgroup_root,
            node_id,
        }
    }

    /// Does this node lead `zone`? It must be **eligible** for the zone (DO: always;
    /// Xen/gateway: on-LAN only — `on_lan` is resolved once per tick) AND win that
    /// zone's own lease. Splitting the lock per zone is what lets leadership land
    /// where the reach is + re-elect per zone on a leader's death.
    fn leads_zone(&self, zone: Zone, on_lan: bool) -> bool {
        if !zone.eligible(on_lan) {
            return false;
        }
        let lock = self.workgroup_root.join(zone.lock_name());
        matches!(
            crate::leader::try_acquire(&lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    fn tick_once(&mut self) {
        // Resolve on-LAN status once per tick — it gates Xen/gateway eligibility
        // and the Xen source's own route selection reads it again internally.
        let on_lan = node_on_lan();
        // Gather only the zones this node leads. An off-LAN node leads neither Xen
        // nor gateway (it can't reach them), so it never even attempts those
        // sources. `active` collects the resource-zone label of each led zone so the
        // brain's gone-sweep is scoped to them (losing a zone must not false-gone it).
        let mut current = Vec::new();
        let mut active: BTreeSet<String> = BTreeSet::new();
        for zone in Zone::ALL {
            if !self.leads_zone(zone, on_lan) {
                continue;
            }
            active.insert(zone.resource_zone().to_string());
            current.extend(match zone {
                Zone::Do => gather_do(),
                Zone::Xen => gather_xen(),
                Zone::Gateway => gather_gateway(),
            });
        }
        // Lead nothing this tick (off-LAN with no DO lease, or simply a follower) ⇒
        // don't reconcile at all, so the brain stays frozen and emits no spurious
        // gone — the same freeze the single-lock design had on losing leadership.
        if active.is_empty() {
            return;
        }
        // DATACENTER-5: the storage/net/gateway (+ compute) rollup — one `rollup`
        // resource per led zone, summed from the gathered set, flowing through the
        // same dedup brain so it only re-publishes when a zone's headline changes.
        let rollups = zone_rollups(&current);
        current.extend(rollups);
        for ev in self.core.reconcile_scoped(&current, Some(&active)) {
            publish(&ev);
        }
    }
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

    // ---- DATACENTER-5: per-zone leadership -------------------------------------

    #[test]
    fn zone_lock_names_are_distinct_per_zone() {
        // Each zone gets its own lock file ⇒ each elects independently. None of
        // them collide with each other or with the legacy shared lock.
        let names: Vec<String> = Zone::ALL.iter().map(|z| z.lock_name()).collect();
        assert_eq!(
            names,
            vec![
                ".mackesd-dc-do-leader.lock",
                ".mackesd-dc-xen-leader.lock",
                ".mackesd-dc-gateway-leader.lock",
            ]
        );
        // Distinct from one another.
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "no two zones share a lock");
        // And distinct from the old shared lock (so a split deploy can't clash).
        assert!(!names.iter().any(|n| n == ".mackesd-leader.lock"));
    }

    #[test]
    fn zone_eligibility_gates_xen_and_gateway_to_on_lan() {
        // DO is reachable from anywhere ⇒ eligible regardless of on-LAN status.
        assert!(Zone::Do.eligible(true));
        assert!(Zone::Do.eligible(false));
        // Xen + gateway are on the lab LAN ⇒ an off-LAN node is NOT eligible (it
        // can't reach the dom0s / router, so it must never take their leadership).
        assert!(Zone::Xen.eligible(true));
        assert!(!Zone::Xen.eligible(false));
        assert!(Zone::Gateway.eligible(true));
        assert!(!Zone::Gateway.eligible(false));
    }

    #[test]
    fn zone_tokens_are_stable() {
        assert_eq!(Zone::Do.token(), "do");
        assert_eq!(Zone::Xen.token(), "xen");
        assert_eq!(Zone::Gateway.token(), "gateway");
    }

    // ---- DATACENTER-5: the storage/net/gateway rollup --------------------------

    fn dev_host(id: &str) -> DcResource {
        let sig = serde_json::json!({"kind":"host","id":id,"zone":"dev"}).to_string();
        DcResource::new("host", id, sig)
    }
    fn dev_sr(id: &str, size: u64, used: u64) -> DcResource {
        // The xe helper emits byte sizes as STRINGS — the rollup must parse them.
        let sig = serde_json::json!({
            "kind":"sr","id":id,"zone":"dev","size":size.to_string(),"used":used.to_string()
        })
        .to_string();
        DcResource::new("sr", id, sig)
    }
    fn dev_net(id: &str) -> DcResource {
        let sig = serde_json::json!({"kind":"net","id":id,"zone":"dev"}).to_string();
        DcResource::new("net", id, sig)
    }
    fn prod_droplet(id: &str) -> DcResource {
        let sig = serde_json::json!({"kind":"droplet","id":id,"zone":"prod"}).to_string();
        DcResource::new("droplet", id, sig)
    }
    fn dev_gateway(id: &str, up: bool, leases: u64) -> DcResource {
        let sig = serde_json::json!({
            "kind":"gateway","id":id,"zone":"dev","status": if up {"up"} else {"down"},"leases":leases
        })
        .to_string();
        DcResource::new("gateway", id, sig)
    }

    #[test]
    fn zone_rollups_summarise_storage_net_gateway_per_zone() {
        let resources = vec![
            dev_host("d1"),
            dev_sr("s1", 200, 50),
            dev_sr("s2", 100, 25),
            dev_net("n1"),
            dev_net("n2"),
            dev_gateway("g1", true, 7),
            prod_droplet("100"),
            prod_droplet("101"),
        ];
        let rollups = zone_rollups(&resources);
        // One rollup per non-empty zone, keyed by zone token; BTreeMap order ⇒
        // dev before prod.
        assert_eq!(rollups.len(), 2);
        assert_eq!(rollups[0].kind, "rollup");
        assert_eq!(rollups[0].id, "dev");
        assert_eq!(rollups[1].id, "prod");

        let dev: serde_json::Value = serde_json::from_str(&rollups[0].signature).unwrap();
        assert_eq!(dev["zone"], "dev");
        assert_eq!(dev["hosts"], 1);
        assert_eq!(dev["srs"], 2);
        // Byte sizes summed across both SRs (parsed from their string fields).
        assert_eq!(dev["storage_total_bytes"], 300);
        assert_eq!(dev["storage_used_bytes"], 75);
        assert_eq!(dev["nets"], 2);
        assert_eq!(dev["gateway_up"], true);
        assert_eq!(dev["gateway_leases"], 7);

        let prod: serde_json::Value = serde_json::from_str(&rollups[1].signature).unwrap();
        assert_eq!(prod["zone"], "prod");
        assert_eq!(prod["droplets"], 2);
        assert_eq!(prod["srs"], 0);
        assert_eq!(prod["gateway_up"], false);
    }

    #[test]
    fn zone_rollups_skips_zoneless_and_rollup_resources() {
        // A resource with no `zone` field contributes to no rollup, and an existing
        // `rollup` resource is never re-rolled (so a re-tick is idempotent).
        let zoneless = DcResource::new("route", "x", r#"{"kind":"route","id":"x"}"#);
        let prior_rollup = DcResource::new(
            "rollup",
            "dev",
            r#"{"kind":"rollup","id":"dev","zone":"dev"}"#,
        );
        let resources = vec![zoneless, prior_rollup, dev_host("d1")];
        let rollups = zone_rollups(&resources);
        assert_eq!(
            rollups.len(),
            1,
            "only the dev zone (from the host) rolls up"
        );
        assert_eq!(rollups[0].id, "dev");
        let dev: serde_json::Value = serde_json::from_str(&rollups[0].signature).unwrap();
        assert_eq!(dev["hosts"], 1);
    }

    #[test]
    fn zone_rollups_empty_for_no_resources() {
        // No resources ⇒ no rollup card (don't publish an all-zero prod/dev before
        // the first real sample lands).
        assert!(zone_rollups(&[]).is_empty());
    }

    #[test]
    fn zone_rollup_event_topic_is_event_dc_rollup_zone() {
        // The rollup flows through the brain → a `event/dc/rollup/<zone>` topic.
        let mut o = DatacenterOrchestrator::new();
        let rollups = zone_rollups(&[dev_host("d1")]);
        let events = o.reconcile(&rollups);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].topic(), "event/dc/rollup/dev");
    }

    #[test]
    fn losing_a_zones_leadership_does_not_false_gone_its_resources() {
        // The crux of per-zone leadership: this node leads BOTH zones, publishes a
        // prod droplet + a dev host, then loses the dev (Xen) zone. The next tick
        // only gathers prod — but dev's host must NOT be gone'd (the new Xen leader
        // is publishing it now). Scoping the gone-sweep to the led zones is what
        // makes "kill the leader → a survivor re-assumes" clean per zone.
        let mut o = DatacenterOrchestrator::new();
        let both = vec![prod_droplet("100"), dev_host("d1")];
        let active_both: BTreeSet<String> = ["prod".into(), "dev".into()].into_iter().collect();
        let e = o.reconcile_scoped(&both, Some(&active_both));
        assert_eq!(e.len(), 2, "both resources first-seen → 2 events");

        // Now we lead ONLY prod. dev's host is absent from `current` but dev is not
        // in the active set ⇒ no gone for it.
        let active_prod: BTreeSet<String> = ["prod".into()].into_iter().collect();
        let e = o.reconcile_scoped(&[prod_droplet("100")], Some(&active_prod));
        assert!(
            e.is_empty(),
            "prod unchanged + dev frozen (not led) ⇒ no events, got {e:?}"
        );

        // A genuinely-removed prod resource (a led zone) still gone's normally.
        let e = o.reconcile_scoped(&[], Some(&active_prod));
        assert_eq!(e.len(), 1);
        assert!(e[0].signature.is_empty(), "prod droplet really gone");
        assert_eq!(e[0].topic(), "event/dc/droplet/100");
    }

    #[test]
    fn full_scope_reconcile_still_gones_everything_absent() {
        // The plain `reconcile` (active_zones = None) keeps the original contract:
        // every previously-seen resource absent now is gone'd.
        let mut o = DatacenterOrchestrator::new();
        o.reconcile(&[prod_droplet("100"), dev_host("d1")]);
        let e = o.reconcile(&[]);
        assert_eq!(e.len(), 2, "both gone under full-scope");
        assert!(e.iter().all(|ev| ev.signature.is_empty()));
    }

    #[test]
    fn zone_resource_zone_maps_do_to_prod_and_lan_to_dev() {
        // The rollup label + the gone-scope key. Two distinct locks (xen, gateway)
        // share the on-prem `dev` resource-zone.
        assert_eq!(Zone::Do.resource_zone(), "prod");
        assert_eq!(Zone::Xen.resource_zone(), "dev");
        assert_eq!(Zone::Gateway.resource_zone(), "dev");
    }
}
