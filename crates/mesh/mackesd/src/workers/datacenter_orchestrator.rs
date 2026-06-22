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
fn xen_dom0s() -> Vec<String> {
    std::env::var("MCNF_XEN_DOM0S")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// SSH key used to reach the dom0s (passwordless root via the mesh key).
fn xen_ssh_key() -> String {
    std::env::var("MCNF_XEN_SSH_KEY").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        format!("{home}/.ssh/mackes_mesh_ed25519")
    })
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

/// Run a remote `xe` command on a dom0 over SSH (best-effort).
fn ssh_xe(key: &str, dom0: &str, remote: &str) -> Option<String> {
    let o = std::process::Command::new("ssh")
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
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Sample the Xen (dev) zone: each configured dom0 becomes a `host` resource and
/// each of its non-control VMs a `vm` resource. Reads XAPI via `xe` over the
/// mesh-key SSH (the no-XO read path proven by DATACENTER-1) — best-effort. This
/// is interim glue; it swaps to XAPI-over-overlay (DATACENTER-4) without changing
/// the brain or the Bus contract.
fn gather_xen() -> Vec<DcResource> {
    let key = xen_ssh_key();
    let mut out = Vec::new();
    for dom0 in xen_dom0s() {
        if let Some(hn) = ssh_xe(&key, &dom0, "xe host-list params=name-label --minimal") {
            let hn = hn.trim();
            if !hn.is_empty() {
                let sig = serde_json::json!({
                    "kind": "host", "id": dom0, "name": hn, "status": "up", "zone": "dev"
                })
                .to_string();
                out.push(DcResource::new("host", dom0.clone(), sig));
            }
        }
        let script = "for u in $(xe vm-list is-control-domain=false params=uuid --minimal | tr , ' '); \
             do echo \"$u|$(xe vm-param-get uuid=$u param-name=name-label)|$(xe vm-param-get uuid=$u param-name=power-state)\"; done";
        if let Some(vmout) = ssh_xe(&key, &dom0, script) {
            for (u, n, s) in parse_xe_vms(&vmout) {
                let sig = serde_json::json!({
                    "kind": "vm", "id": u, "name": n, "status": s, "host": dom0, "zone": "dev"
                })
                .to_string();
                out.push(DcResource::new("vm", u, sig));
            }
        }
    }
    out
}

/// Seam: the UniFi gateway (DATACENTER-14, cred from the mesh store). Empty until then.
fn gather_gateway() -> Vec<DcResource> {
    Vec::new()
}

/// Emit a datacenter event onto the Bus (best-effort, fire-and-reap — same lane
/// shape as the other workers' events).
fn publish(ev: &DcEvent) {
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &ev.topic(), "--body-flag", &ev.body()]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// The supervised worker. Leader-gated (only the elected node samples + publishes,
/// so a multi-node mesh doesn't multi-publish) and best-effort.
pub struct DatacenterOrchestratorWorker {
    core: DatacenterOrchestrator,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
}

impl DatacenterOrchestratorWorker {
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            core: DatacenterOrchestrator::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
        }
    }

    /// Only the directory leader orchestrates (no-fixed-center: any eligible node
    /// can be it, the elected one publishes). Reuses the shared leader lock.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    fn tick_once(&mut self) {
        if !self.is_leader() {
            return;
        }
        let mut current = gather_do();
        current.extend(gather_xen());
        current.extend(gather_gateway());
        for ev in self.core.reconcile(&current) {
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
}
