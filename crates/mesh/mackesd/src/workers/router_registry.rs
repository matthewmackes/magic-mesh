//! ROUTER-3 / ROUTER-4 — the router-registry worker.
//!
//! Per-node + always-on (lock #2): every node may sit behind its own
//! router/firewall. Each tick it
//!   1. discovers the node's primary appliance ([`router_discovery::discover_primary`]
//!      — lowest-metric default route + gateway MAC),
//!   2. matches a sealed `router/<mac>` credential (ROUTER-3); when present it
//!      fingerprints the device over the Vyatta CLI (`show version`), otherwise
//!      the appliance is surfaced read-only as `needs_creds` (lock #4),
//!   3. publishes a [`RouterEntry`] into the SAME mesh registry plane the other
//!      published services use — the per-appliance Bus topic
//!      `mesh/devices/router/<mac>` AND the replicated QNM-Shared mirror at
//!      `<mount>/<host>/router-registry.json`.
//!
//! Publish cadence mirrors [`media_registry`](super::media_registry) /
//! [`compute_registry`](super::compute_registry): on-change + a slow heartbeat;
//! the QNM-Shared mirror is written every tick (atomic tmp+rename). A node with
//! no default route emits nothing (safe no-op). Read slice of
//! `docs/design/router-control.md`.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{ShutdownToken, Worker};
use crate::ipc::secret_store::{self, SecretStore};
use crate::router_discovery::{self, RouterCandidate, RouterEntry, RouterVendor};

/// 60 s tick — a router appliance is slow-changing; the on-change publish below
/// still propagates a flip on the next tick.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(60);

/// Slow heartbeat for the on-change Bus publish (mirrors the other registries).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(300);

/// The QNM-Shared mirror filename.
pub const ROUTER_REGISTRY_FILE: &str = "router-registry.json";

/// Bus topic a router entry publishes to: `mesh/devices/router/<mac>`.
#[must_use]
pub fn router_topic(mac: &str) -> String {
    format!("mesh/devices/router/{mac}")
}

/// Publish a router entry to its per-appliance Bus topic via the `mde-bus` CLI
/// (typed argv, §9 — no shell). Best-effort: a missing/failing `mde-bus` is
/// reaped, never fatal.
pub fn publish_entry(entry: &RouterEntry) {
    let topic = router_topic(&entry.id);
    publish_entry_to(
        crate::bus_publish::default_bus_root().as_deref(),
        &topic,
        entry,
    );
}

/// Root-injectable in-process publish for [`publish_entry`] (perf-10 / arch-6) —
/// no fork+exec of the `mde-bus` CLI per router entry. Fresh-opens the Bus at
/// `bus_root` (the CLI-equivalent [`crate::bus_publish::default_bus_root`] in
/// production, honouring `MDE_BUS_ROOT`) and writes the compact `serde_json` of
/// `entry` — the exact body the old `--body-flag` carried. Best-effort; tests
/// pass a temp root.
fn publish_entry_to(bus_root: Option<&std::path::Path>, topic: &str, entry: &RouterEntry) {
    if let Some(mut persist) =
        crate::bus_publish::open_bus(bus_root.map(std::path::Path::to_path_buf))
    {
        crate::bus_publish::publish_json(&mut persist, topic, entry);
    }
}

/// Mirror a router entry to the replicated QNM-Shared plane at
/// `<mount>/<hostname>/router-registry.json` (atomic tmp+rename). Best-effort:
/// a missing mount / write error is logged, never fatal.
pub fn write_shared_entry(mount: &Path, hostname: &str, entry: &RouterEntry) {
    if hostname.is_empty() {
        return;
    }
    let dir = mount.join(hostname);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("router_registry: mkdir {} failed: {e}", dir.display());
        return;
    }
    let Ok(body) = serde_json::to_string(entry) else {
        return;
    };
    let tmp = dir.join("router-registry.json.tmp");
    let final_path = dir.join(ROUTER_REGISTRY_FILE);
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        tracing::warn!("router_registry: write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        tracing::warn!("router_registry: rename entry failed: {e}");
    }
}

/// Active Vyatta `show version` over SSH. The password (from the sealed cred) is
/// fed via the `SSHPASS` env so it never reaches argv / `ps` (mirrors the
/// EdgeOS tofu `sshpass -f` discipline). Best-effort: `None` on any failure.
fn ssh_show_version(ip: &str, user: &str, pass: &str) -> Option<String> {
    let target = format!("{user}@{ip}");
    let mut cmd = Command::new("sshpass");
    cmd.arg("-e")
        .args([
            "ssh",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "PreferredAuthentications=password",
            "-o",
            "PubkeyAuthentication=no",
            &target,
            "show version",
        ])
        .env("SSHPASS", pass);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The router-registry worker (per-node, always-on).
pub struct RouterRegistryWorker {
    /// The node this appliance sits behind (`peer:<host>`) — the entry's `node_id`.
    node_id: String,
    /// Hostname the QNM-Shared mirror is keyed under.
    hostname: String,
    /// Tick cadence.
    tick: Duration,
    /// Replicated QNM-Shared registry root.
    mount: PathBuf,
    /// The secret store the `router/<mac>` cred is read from (resolved once).
    secret_store: SecretStore,
    /// Slow heartbeat for the on-change Bus publish.
    publish_heartbeat: Duration,
    /// Last published body + when (on-change + heartbeat-republish).
    last_publish: Mutex<Option<(String, Instant)>>,
}

impl RouterRegistryWorker {
    /// Construct with production defaults. `node_id` is the entry's owner;
    /// `hostname` keys the QNM-Shared mirror.
    #[must_use]
    pub fn new(node_id: String, hostname: String) -> Self {
        let mount = crate::default_qnm_shared_root();
        let secret_store = SecretStore::resolve(&secret_store::repo_root(), &mount);
        Self {
            node_id,
            hostname,
            tick: DEFAULT_TICK_INTERVAL,
            mount,
            secret_store,
            publish_heartbeat: PUBLISH_HEARTBEAT,
            last_publish: Mutex::new(None),
        }
    }

    /// Override the QNM-Shared root (honors `--workgroup-root` at the spawn site).
    #[must_use]
    pub fn with_mount(mut self, p: PathBuf) -> Self {
        self.mount = p;
        self
    }

    /// Override the secret store (tests drive a seeded `LocalAead`).
    #[must_use]
    pub fn with_secret_store(mut self, store: SecretStore) -> Self {
        self.secret_store = store;
        self
    }

    /// Build this tick's entry from a discovered candidate: cred-match → active
    /// `show version` fingerprint. Pure given the cred lookup + ssh closure are
    /// the side-effects; see [`build_entry_from`] for the testable core.
    fn build_entry(&self, candidate: &RouterCandidate) -> RouterEntry {
        let cred = self.secret_store.get(&candidate.cred_ref()).ok().flatten();
        build_entry_from(&self.node_id, candidate, cred.as_deref(), ssh_show_version)
    }

    fn tick_once(&self) {
        let Some(candidate) = router_discovery::discover_primary() else {
            return; // no default route → nothing behind this node (safe no-op)
        };
        let entry = self.build_entry(&candidate);
        if let Ok(body) = serde_json::to_string(&entry) {
            let mut last = self
                .last_publish
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let prev_body = last.as_ref().map(|(b, _)| b.as_str());
            let prev_at = last.as_ref().map(|(_, at)| *at);
            if crate::workers::compute_registry::should_publish(
                prev_body,
                &body,
                prev_at,
                now,
                self.publish_heartbeat,
            ) {
                publish_entry(&entry);
                *last = Some((body, now));
            }
        }
        write_shared_entry(&self.mount, &self.hostname, &entry);
    }
}

/// The testable core of [`RouterRegistryWorker::build_entry`]: given the node id,
/// a discovered candidate, an optional sealed cred body, and a `show version`
/// prober, produce the [`RouterEntry`]. No cred → `needs_creds`, vendor unknown
/// (surfaced read-only, lock #4). Cred present → fingerprint via the prober.
fn build_entry_from(
    node_id: &str,
    candidate: &RouterCandidate,
    cred: Option<&str>,
    probe: impl Fn(&str, &str, &str) -> Option<String>,
) -> RouterEntry {
    let (vendor, version, managed, needs_creds) = match cred {
        Some(body) => {
            let (user, pass) = router_discovery::parse_router_cred(body);
            match probe(&candidate.ip, &user, &pass) {
                Some(v) => {
                    let vendor = router_discovery::fingerprint_from_version(&v);
                    let first = v
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    (vendor, first, true, false)
                }
                // cred sealed but unreachable: managed, vendor not yet known.
                None => (RouterVendor::Unknown, String::new(), true, false),
            }
        }
        None => (RouterVendor::Unknown, String::new(), false, true),
    };
    RouterEntry {
        id: candidate.mac.clone(),
        ip: candidate.ip.clone(),
        node_id: node_id.to_string(),
        vendor: vendor.as_str().to_string(),
        version,
        managed,
        needs_creds,
        is_default: candidate.is_default,
    }
}

#[async_trait::async_trait]
impl Worker for RouterRegistryWorker {
    fn name(&self) -> &'static str {
        "router_registry"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => {
                    self.tick_once();
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

    fn candidate() -> RouterCandidate {
        RouterCandidate {
            ip: "172.20.0.1".into(),
            mac: "46:6a:7c:96:e8:aa".into(),
            is_default: true,
            oui_hint: Some("ubiquiti".into()),
        }
    }

    #[test]
    fn no_cred_is_unmanaged_needs_creds() {
        let e = build_entry_from("peer:eagle", &candidate(), None, |_, _, _| {
            panic!("must not probe without a cred")
        });
        assert!(!e.managed);
        assert!(e.needs_creds);
        assert_eq!(e.vendor, "unknown");
        assert_eq!(e.id, "46:6a:7c:96:e8:aa");
        assert_eq!(e.node_id, "peer:eagle");
        assert!(e.is_default);
    }

    #[test]
    fn cred_present_fingerprints_via_show_version() {
        let e = build_entry_from("peer:eagle", &candidate(), Some("ubnt:pw"), |_, user, _| {
            assert_eq!(user, "ubnt");
            Some("Version: v2.0.9\nEdgeOS ER-8".into())
        });
        assert!(e.managed);
        assert!(!e.needs_creds);
        assert_eq!(e.vendor, "edgeos");
        assert_eq!(e.version, "Version: v2.0.9");
    }

    #[test]
    fn cred_present_but_unreachable_is_managed_unknown() {
        let e = build_entry_from("peer:eagle", &candidate(), Some("ubnt:pw"), |_, _, _| None);
        assert!(e.managed);
        assert!(!e.needs_creds);
        assert_eq!(e.vendor, "unknown");
        assert!(e.version.is_empty());
    }

    #[test]
    fn topic_keys_by_mac() {
        assert_eq!(
            router_topic("46:6a:7c:96:e8:aa"),
            "mesh/devices/router/46:6a:7c:96:e8:aa"
        );
    }

    #[test]
    fn shared_mirror_writes_atomic_and_skips_empty_host() {
        let tmp = std::env::temp_dir().join(format!("mde-routerreg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let e = build_entry_from("peer:eagle", &candidate(), None, |_, _, _| None);
        write_shared_entry(&tmp, "eagle", &e);
        let path = tmp.join("eagle").join(ROUTER_REGISTRY_FILE);
        let back: RouterEntry =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back, e);
        assert!(!tmp.join("eagle").join("router-registry.json.tmp").exists());
        // empty hostname writes nothing
        write_shared_entry(&tmp.join("none"), "", &e);
        assert!(!tmp.join("none").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
