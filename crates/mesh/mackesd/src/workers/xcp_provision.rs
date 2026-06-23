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

use mackes_xcp::{build_identity_seed, mde_vm_hostname, HostTarget, Hypervisor, XeSsh};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Bus topic this worker drains for spawn requests (the A4 surface).
pub const SPAWN_TOPIC: &str = "action/provision/spawn";

/// Reply-topic prefix for spawn acks (suffix = request ULID).
pub const SPAWN_ACK_PREFIX: &str = "action/provision/spawn-ack/";

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
/// Clones the golden, attaches the fresh identity seed, starts, and polls for
/// the IP. Generic over the hypervisor so the reachability of `set_identity_seed`
/// is provable with a mock (no live dom0). `op_ssh_key` is the operator's
/// authorized public key the seed installs. `wait`/`poll` bound the IP wait
/// (0 ⇒ skip the wait).
///
/// Returns the success-ack body; `Err(description)` becomes an error-ack.
///
/// # Errors
/// Any `xe`/`ssh` step failing (clone / seed / start) aborts the spawn.
pub fn run_spawn<H: Hypervisor>(
    hv: &H,
    name: &str,
    op_ssh_key: &str,
    wait: Duration,
    poll: Duration,
) -> Result<String, String> {
    let hostname = mde_vm_hostname(name);

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
    let target = HostTarget::ssh_root(dom0, Some(dom0_ssh_key()));
    let hv = XeSsh::new(target);
    run_spawn(
        &hv,
        &req.name,
        &operator_ssh_key(),
        IP_WAIT_TIMEOUT,
        IP_WAIT_POLL,
    )
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
            unreachable!("host_capacity is not part of the spawn flow")
        }
    }

    #[test]
    fn run_spawn_calls_set_identity_seed_between_clone_and_start() {
        let hv = MockHv::default();
        let ack = run_spawn(
            &hv,
            "web1",
            "ssh-ed25519 OPKEY op@host",
            Duration::ZERO, // skip the wait — vm_ip is probed once
            Duration::from_millis(1),
        )
        .expect("spawn ok");

        // REACHABILITY ASSERTION — set_identity_seed is invoked, in order,
        // between the clone and the start. This is the whole point of the unit:
        // a provisioned VM actually gets its identity seed.
        let calls = hv.calls.borrow().clone();
        assert_eq!(
            calls,
            vec![
                "clone(MDE-VM-golden->MDE-VM-web1)".to_string(),
                "seed(uuid-xyz)".to_string(),
                "start(uuid-xyz)".to_string(),
                "vm_ip".to_string(),
            ],
            "set_identity_seed must run between clone_golden and start"
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
                unreachable!()
            }
        }
        let err = run_spawn(
            &SeedFails,
            "web1",
            "ssh-ed25519 K op@h",
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
    }
}
