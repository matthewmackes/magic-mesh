//! WL-FUNC-008 — the mackesd `service_aggregator` worker: the unified
//! service-provenance/health view.
//!
//! Three service sources existed and were never unified — the published KDC
//! directory (`kdc-services/<host>.json`), the nmap probe inventory
//! (`probe-inventory.json`), and the Explorer's `service → openable-action`
//! enrichment map. This worker merges all three into one deduped
//! [`ServiceRecord`](mackes_mesh_types::service_record::ServiceRecord) set (with
//! stale-entry TTL age-out) and publishes it on `state/services/<node>`, so the
//! shell's Services view stays a thin renderer (§6 — scanning + privilege live in
//! the daemon, never the GUI).
//!
//! ## Shape (mirrors the EXPLORER-1 `unit_aggregator` / QC-2 `openstack` workers)
//!
//! - **Two injectable source seams** ([`PublishedSource`] / [`ProbeSource`]), each
//!   headless-testable with a fake: [`DirectoryPublished`] reads the replicated KDC
//!   directory; [`InventoryProbe`] reads the merged probe inventory. Enrichment is
//!   the pure `service → action` map applied inside [`aggregate::aggregate`].
//! - **A pure fold** ([`aggregate::aggregate`]): merge by `(host, kind)`, stamp
//!   health from source + freshness, age out stale probe-only entries.
//! - **The `state/services/<node>` mirror** ([`ServicesState`]) — published on
//!   change + a heartbeat via the in-process `mde-bus` publish path (the same idiom
//!   `state/units/<node>` / `state/storage/<node>` use).
//!
//! Universal (rank 0) like `unit_aggregator` / `storage` / `openstack`: every node
//! folds + publishes its OWN mesh-wide merge of the (replicated) sources, no center.

#![cfg(feature = "async-services")]

pub mod aggregate;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mackes_mesh_types::service_record::ServicesState;

use aggregate::{aggregate, ProbeInput, PublishedInput};

use super::{ShutdownToken, Worker};

/// Fold cadence — one directory + inventory read per interval. Same order of cost
/// as the sibling `unit_aggregator` worker's heartbeat.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Unconditional mirror republish cadence (between change-driven publishes).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);

/// Default stale-entry age-out window: a probe-only service not re-seen within it
/// expires from the set (see [`aggregate::aggregate`]).
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// The per-node mirror topic: `state/services/<node>`.
#[must_use]
pub fn state_topic(node: &str) -> String {
    format!("state/services/{node}")
}

/// Wall-clock milliseconds since the Unix epoch (i64 to match the wire types).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The default Bus root (the persisted message tree), matching every mackesd worker.
fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Publish a JSON state-mirror body in-process (no fork+exec of the `mde-bus` CLI),
/// through the SAME bus root every other mackesd worker's mirror uses. Best-effort.
fn publish_json<T: serde::Serialize>(bus_root: Option<&Path>, topic: &str, body: &T) {
    if let Some(mut persist) = crate::bus_publish::open_bus(bus_root.map(Path::to_path_buf)) {
        crate::bus_publish::publish_json(&mut persist, topic, body);
    }
}

/// The published-directory source seam — the KDC service directory rows.
pub trait PublishedSource: Send + Sync {
    /// Every node's published service row, reduced to the merge inputs.
    fn read(&self) -> Vec<PublishedInput>;
}

/// The probe-inventory source seam — the nmap-discovered open services.
pub trait ProbeSource: Send + Sync {
    /// Every probed `(host, service)` open port, reduced to the merge inputs.
    fn read(&self) -> Vec<ProbeInput>;
}

/// Production published source: reads `<workgroup>/kdc-services/*.json` via
/// [`mde_kdc_host::service_directory::collect_all_services`].
pub struct DirectoryPublished {
    workgroup_root: PathBuf,
}

impl DirectoryPublished {
    /// Read the directory under `workgroup_root`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

impl PublishedSource for DirectoryPublished {
    fn read(&self) -> Vec<PublishedInput> {
        mde_kdc_host::service_directory::collect_all_services(&self.workgroup_root)
            .into_iter()
            .map(|n| PublishedInput {
                host: n.node_host,
                endpoint_ip: n.overlay_ip,
                services: n.services,
                updated_ms: n.updated_ms,
            })
            .collect()
    }
}

/// Production probe source: reads the merged `probe-inventory.json` cards via
/// [`crate::probe_nmap::inventory`], flattening each host's open-service children.
pub struct InventoryProbe {
    workgroup_root: PathBuf,
}

impl InventoryProbe {
    /// Read the inventory under `workgroup_root`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

impl ProbeSource for InventoryProbe {
    fn read(&self) -> Vec<ProbeInput> {
        let mut out = Vec::new();
        for host in crate::probe_nmap::inventory(&self.workgroup_root) {
            let Some(hf) = crate::card::probe::host_facts(&host) else {
                continue;
            };
            let host_name = if hf.hostname.is_empty() {
                hf.ip.clone()
            } else {
                hf.hostname.clone()
            };
            // Probe last-seen is Unix seconds; the merge keys on Unix-ms.
            let last_seen_ms = i64::try_from(hf.last_seen)
                .unwrap_or(i64::MAX)
                .saturating_mul(1000);
            for child in &host.children {
                let Some(sf) = crate::card::probe::service_facts(child) else {
                    continue;
                };
                let kind = if sf.service_kind.is_empty() {
                    format!("port/{}", sf.port)
                } else {
                    sf.service_kind.clone()
                };
                out.push(ProbeInput {
                    host: host_name.clone(),
                    ip: hf.ip.clone(),
                    port: sf.port,
                    kind,
                    last_seen_ms,
                });
            }
        }
        out
    }
}

/// The WL-FUNC-008 `service_aggregator` worker.
pub struct ServiceAggregatorWorker {
    /// This node's id — the mirror `host` stamp + topic namespace.
    host: String,
    /// The published-directory half.
    published: Arc<dyn PublishedSource>,
    /// The probe-inventory half.
    probe: Arc<dyn ProbeSource>,
    /// The Bus root for the mirror publish (`None` ⇒ publish is a no-op).
    bus_root: Option<PathBuf>,
    /// Fold cadence.
    poll: Duration,
    /// Mirror republish heartbeat.
    heartbeat: Duration,
    /// Stale-entry age-out window.
    ttl: Duration,
}

impl ServiceAggregatorWorker {
    /// Construct with production defaults: the replicated KDC directory + the merged
    /// probe inventory under `workgroup_root`, the persisted Bus tree, and the
    /// default cadences. `host` is this node's id.
    #[must_use]
    pub fn new(host: String, workgroup_root: PathBuf) -> Self {
        Self {
            host,
            published: Arc::new(DirectoryPublished::new(workgroup_root.clone())),
            probe: Arc::new(InventoryProbe::new(workgroup_root)),
            bus_root: default_bus_root(),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            ttl: DEFAULT_TTL,
        }
    }

    /// Inject the published source (tests).
    #[must_use]
    pub fn with_published(mut self, published: Arc<dyn PublishedSource>) -> Self {
        self.published = published;
        self
    }

    /// Inject the probe source (tests).
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn ProbeSource>) -> Self {
        self.probe = probe;
        self
    }

    /// Override the Bus root (tests point it at a tempdir / `None`).
    #[must_use]
    pub fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_root = bus_root;
        self
    }

    /// Override the fold cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the stale age-out window (tests).
    #[must_use]
    pub const fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Read both sources and fold them into the current [`ServicesState`]. No
    /// publish — the pure step the tick + tests share.
    fn fold_state(&self) -> ServicesState {
        let published = self.published.read();
        let probes = self.probe.read();
        let now = now_ms();
        let ttl_ms = i64::try_from(self.ttl.as_millis()).unwrap_or(i64::MAX);
        let records = aggregate(&published, &probes, now, ttl_ms);
        ServicesState {
            host: self.host.clone(),
            records,
            published_at_ms: now,
        }
    }

    /// One fold cycle: build the current state, publish on content-change or the
    /// heartbeat (mirroring `unit_aggregator` / `openstack`).
    fn cycle_and_publish(
        &self,
        last: &mut Option<ServicesState>,
        last_pub_at: &mut Option<Instant>,
    ) {
        let state = self.fold_state();
        let now = Instant::now();
        let changed = last
            .as_ref()
            .is_none_or(|prev| !prev.same_ignoring_time(&state));
        let heartbeat_due = last_pub_at.is_none_or(|at| now.duration_since(at) >= self.heartbeat);
        if changed || heartbeat_due {
            publish_json(self.bus_root.as_deref(), &state_topic(&self.host), &state);
            *last_pub_at = Some(now);
        }
        *last = Some(state);
    }
}

#[async_trait::async_trait]
impl Worker for ServiceAggregatorWorker {
    fn name(&self) -> &'static str {
        "service_aggregator"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut last: Option<ServicesState> = None;
        let mut last_pub_at: Option<Instant> = None;
        // Fold + publish immediately so a surface doesn't wait a full tick for the
        // first mirror row.
        self.cycle_and_publish(&mut last, &mut last_pub_at);
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.cycle_and_publish(&mut last, &mut last_pub_at);
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
    use mackes_mesh_types::service_record::{ServiceHealth, ServiceProvenance};

    struct FakePublished(Vec<PublishedInput>);
    impl PublishedSource for FakePublished {
        fn read(&self) -> Vec<PublishedInput> {
            self.0.clone()
        }
    }

    struct FakeProbe(Vec<ProbeInput>);
    impl ProbeSource for FakeProbe {
        fn read(&self) -> Vec<ProbeInput> {
            self.0.clone()
        }
    }

    fn worker_with(
        published: Vec<PublishedInput>,
        probes: Vec<ProbeInput>,
    ) -> ServiceAggregatorWorker {
        ServiceAggregatorWorker::new("me".into(), PathBuf::from("/tmp"))
            .with_bus_root(None)
            .with_published(Arc::new(FakePublished(published)))
            .with_probe(Arc::new(FakeProbe(probes)))
            // A generous TTL so a fixed test timestamp stays fresh.
            .with_ttl(Duration::from_secs(3_600 * 24 * 365 * 100))
    }

    #[test]
    fn name_and_topic_match_the_census_and_convention() {
        let w = ServiceAggregatorWorker::new("node".into(), PathBuf::from("/tmp"));
        assert_eq!(w.name(), "service_aggregator");
        assert_eq!(state_topic("node-a"), "state/services/node-a");
        assert!(state_topic("x").starts_with("state/"));
    }

    #[test]
    fn fold_state_merges_both_seams_into_the_mirror() {
        let now = now_ms();
        let published = vec![PublishedInput {
            host: "alpha".into(),
            endpoint_ip: Some("10.42.0.5".into()),
            services: vec!["ssh".into()],
            updated_ms: now,
        }];
        let probes = vec![ProbeInput {
            host: "alpha".into(),
            ip: "10.42.0.5".into(),
            port: 22,
            kind: "ssh".into(),
            last_seen_ms: now,
        }];
        let w = worker_with(published, probes);
        let state = w.fold_state();
        assert_eq!(state.host, "me");
        assert_eq!(state.records.len(), 1, "the two seams fold into one record");
        let r = &state.records[0];
        assert_eq!(r.endpoint.as_deref(), Some("10.42.0.5:22"));
        assert_eq!(r.health, ServiceHealth::Up);
        assert!(r.attested_by(ServiceProvenance::Published));
        assert!(r.attested_by(ServiceProvenance::Probe));
        assert!(r.attested_by(ServiceProvenance::Enrichment));
    }

    #[tokio::test]
    async fn tick_loop_exits_promptly_on_shutdown() {
        let mut w = worker_with(vec![], vec![]).with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
