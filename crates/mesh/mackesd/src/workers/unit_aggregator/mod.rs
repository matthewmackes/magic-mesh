//! EXPLORER-1 — the mackesd `unit_aggregator` worker: the daemon spine of the
//! Hero unit explorer (`docs/design/unit-explorer.md`, locked 2026-07-04).
//!
//! One worker unions three sources into a typed [`unit::Unit`] stream and
//! publishes it on the Bus, so the Discovery-surface hero fold (EXPLORER-3) stays
//! a thin renderer (§6 — scanning + privilege live in the daemon, never the GUI).
//!
//! ## Shape (mirrors the QC-2 `openstack` / BUG-STORAGE-1 `storage` workers)
//!
//! - **Three injectable seams** ([`sources`]), each headless-testable with a fake
//!   ([`testkit`]): [`sources::MeshMirrorSource`] (the peer directory + leader +
//!   health — source (a), lock #2), [`sources::OpenstackMirrorSource`] (the union
//!   of every node's `state/openstack/<node>` mirror — source (b), lock #20), and
//!   [`sources::LanScanSource`] (the surface-gated active LAN scan — the
//!   EXPLORER-2 producer seam, [`sources::NoScan`] today).
//! - **A pure fold** ([`fold::aggregate`]): self-first (lock #23), then peers,
//!   LAN, cloud; cloud deduped by object id across nodes (lock #20); first/last-
//!   seen stamped across ticks (E10). Unprobed fields stay explicit `None` (§7).
//! - **A pure edge derivation** ([`edges::derive_edges`], EXPLORER-7, E2/E8): the
//!   five typed relationship kinds ([`edges::EdgeKind`]) computed from the SAME
//!   three sources (no new probes, §7) — mesh tunnels, cloud attachments, L2/L3
//!   adjacency, host placement, storage usage — deduped + sorted.
//! - **The `state/units/<node>` mirror** ([`unit::UnitsState`]) — the folded units
//!   AND the derived edges, published on change + a heartbeat via the `mde-bus`
//!   fire-and-reap path (the same idiom `state/openstack/<node>` uses).
//! - **The E9 read verb** ([`verb`]) — `action/units/get-stream` → a
//!   `reply/<ulid>` carrying the current stream (units + edges), for any Rust/CLI
//!   mesh client.
//!
//! ## Seams the later EXPLORER slices fill
//! - EXPLORER-2 replaces [`sources::NoScan`] with the real mDNS/ARP/ping-sweep
//!   scan behind [`sources::LanScanSource`], honouring the [`scan_flag`] the
//!   surface toggles (lock #24). The `LanHost` unit producer already lands here.
//! - EXPLORER-9 fills [`unit::Extras`] (enrichment) + the instance detail (E4);
//!   the model already carries the `Option` slots.
//!
//! [`scan_flag`]: UnitAggregatorWorker::scan_flag

#![cfg(feature = "async-services")]

pub mod edges;
pub mod fold;
pub mod lan_scan;
pub mod sources;
#[cfg(test)]
pub(crate) mod testkit;
pub mod unit;
pub mod verb;

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

use edges::derive_edges;
use fold::{aggregate, SeenTracker};
use lan_scan::LanScan;
use sources::{
    BusOpenstackMirror, LanScanSource, MeshDirectoryMirror, MeshMirrorSource, NoCloud,
    OpenstackMirrorSource,
};
use unit::UnitsState;
use verb::{handle_units_request, UNITS_REQUEST_TOPIC};

/// Fold cadence — one mesh + cloud read (+ the gated scan tick) per interval.
/// Same order of cost as the sibling `openstack` worker's heartbeat.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Unconditional mirror republish cadence.
///
/// Between heartbeats the mirror is published only on a content change, so a late
/// subscriber still finds a recent row without the Bus filling with identical
/// bodies.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);

/// The per-node mirror topic: `state/units/<node>`.
#[must_use]
pub fn state_topic(node: &str) -> String {
    format!("state/units/{node}")
}

/// Wall-clock milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The default Bus root (the persisted message tree), matching every other
/// mackesd worker's resolution.
fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Publish a JSON body to `topic` via the `mde-bus` CLI — the same fire-and-reap
/// path the `openstack`/`storage` workers use. Best-effort: a missing `mde-bus`
/// binary (pre-RPM dev box) is swallowed.
fn publish_json<T: serde::Serialize>(topic: &str, body: &T) {
    let Ok(json) = serde_json::to_string(body) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", topic, "--body-flag", &json]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Drain net-new `action/units/get-stream` requests since `cursor` and answer
/// each on `reply/<ulid>` with `current`. Best-effort — a read/write failure is
/// logged + the cursor still advances (a stale request never re-answers).
fn drain_requests(persist: &Persist, cursor: &mut Option<String>, current: &UnitsState) {
    let msgs = match persist.list_since(UNITS_REQUEST_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(target: "mackesd::units", error = %e, "units verb: list_since failed");
            return;
        }
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.unwrap_or_default();
        let reply = handle_units_request(&body, current);
        if let Err(e) = persist.write(
            &reply_topic(&msg.ulid),
            Priority::Default,
            None,
            Some(&reply.to_body()),
        ) {
            tracing::warn!(target: "mackesd::units", ulid = %msg.ulid, error = %e, "units verb: reply write failed");
        }
    }
}

/// The EXPLORER-1 `unit_aggregator` worker.
pub struct UnitAggregatorWorker {
    /// This node's id — the mirror `host` stamp + topic namespace + self unit.
    host: String,
    /// The mesh half (source (a)).
    mesh: Arc<dyn MeshMirrorSource>,
    /// The cloud half (source (b)).
    cloud: Arc<dyn OpenstackMirrorSource>,
    /// The off-mesh half (EXPLORER-2 producer seam).
    scan: Arc<dyn LanScanSource>,
    /// The surface-gated scan-active flag (lock #24) — the shell sets it only
    /// while Discovery is visible. `NoScan` ignores it today.
    scan_active: Arc<AtomicBool>,
    /// The Bus root for the verb drain (`None` ⇒ the verb is idle).
    bus_root: Option<PathBuf>,
    /// Fold cadence.
    poll: Duration,
    /// Mirror republish heartbeat.
    heartbeat: Duration,
    /// First/last-seen memory across ticks (E10).
    seen: SeenTracker,
}

impl UnitAggregatorWorker {
    /// Construct with production defaults: the replicated peer directory + etcd
    /// leader as the mesh seam, the persisted Bus tree as the openstack-union
    /// seam, the surface-gated active LAN scan ([`LanScan`], EXPLORER-2) as the
    /// off-mesh seam, and the default cadences. `host` is this node's id;
    /// `workgroup_root` seeds the peer-directory reader.
    #[must_use]
    pub fn new(host: String, workgroup_root: PathBuf) -> Self {
        let bus_root = default_bus_root();
        let cloud: Arc<dyn OpenstackMirrorSource> = bus_root.clone().map_or_else(
            || Arc::new(NoCloud) as Arc<dyn OpenstackMirrorSource>,
            |root| Arc::new(BusOpenstackMirror::new(root)),
        );
        Self {
            mesh: Arc::new(MeshDirectoryMirror::new(workgroup_root, host.clone())),
            host,
            cloud,
            scan: Arc::new(LanScan::live()),
            scan_active: Arc::new(AtomicBool::new(false)),
            bus_root,
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            seen: SeenTracker::new(),
        }
    }

    /// Inject the mesh mirror source (tests).
    #[must_use]
    pub fn with_mesh(mut self, mesh: Arc<dyn MeshMirrorSource>) -> Self {
        self.mesh = mesh;
        self
    }

    /// Inject the openstack-union source (tests).
    #[must_use]
    pub fn with_cloud(mut self, cloud: Arc<dyn OpenstackMirrorSource>) -> Self {
        self.cloud = cloud;
        self
    }

    /// Inject the LAN scan source (tests / EXPLORER-2's real scan).
    #[must_use]
    pub fn with_scan(mut self, scan: Arc<dyn LanScanSource>) -> Self {
        self.scan = scan;
        self
    }

    /// Override the Bus root (tests point it at a tempdir).
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

    /// The surface-gated scan-active flag (lock #24). EXPLORER-3 clones this and
    /// sets it `true` only while the Discovery surface is visible; the LAN scan
    /// seam reads it each tick.
    #[must_use]
    pub fn scan_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.scan_active)
    }

    /// Read the three sources and fold them into the current [`UnitsState`]
    /// (stamping first/last-seen). No publish — the pure step the tick + tests
    /// share.
    fn fold_state(&mut self) -> UnitsState {
        let scan_active = self.scan_active.load(Ordering::Relaxed);
        // All three reads are synchronous (the mesh read rides the runtime-aware
        // etcd bridge; the cloud read is fs; the scan is local), completing before
        // any await — so the fold never pins the async runtime.
        let mesh = self.mesh.read();
        let cloud = self.cloud.read();
        let lan = self.scan.scan(scan_active);
        let now = now_ms();
        let units = aggregate(&mesh, &cloud, &lan, &mut self.seen, now);
        // Derive the typed edge set from the SAME three sources (EXPLORER-7,
        // E2/E8) — no new probes; absent sources yield no edges (§7).
        let edges = derive_edges(&mesh, &cloud, &lan);
        UnitsState {
            host: self.host.clone(),
            units,
            edges,
            published_at_ms: now,
        }
    }

    /// One fold cycle: build the current state, and publish it when the content
    /// changed or the heartbeat elapsed (publish-on-change, mirroring the
    /// openstack worker).
    fn cycle_and_publish(
        &mut self,
        last: &mut Option<UnitsState>,
        last_pub_at: &mut Option<Instant>,
    ) {
        let state = self.fold_state();
        let now = Instant::now();
        let changed = last
            .as_ref()
            .is_none_or(|prev| !prev.same_ignoring_time(&state));
        let heartbeat_due = last_pub_at.is_none_or(|at| now.duration_since(at) >= self.heartbeat);
        if changed || heartbeat_due {
            publish_json(&state_topic(&self.host), &state);
            *last_pub_at = Some(now);
        }
        *last = Some(state);
    }
}

#[async_trait::async_trait]
impl Worker for UnitAggregatorWorker {
    fn name(&self) -> &'static str {
        "unit_aggregator"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut last: Option<UnitsState> = None;
        let mut last_pub_at: Option<Instant> = None;
        // The verb responder shares the Bus store; seed the request cursor at the
        // tail so a restart doesn't replay stale requests.
        let persist = self
            .bus_root
            .clone()
            .and_then(|root| Persist::open(root).ok());
        let mut req_cursor: Option<String> = persist
            .as_ref()
            .and_then(|p| p.latest_ulid(UNITS_REQUEST_TOPIC).ok().flatten());
        // Fold + publish immediately so a surface doesn't wait a full tick for the
        // first mirror row (lock #23 — self shows instantly).
        self.cycle_and_publish(&mut last, &mut last_pub_at);
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.cycle_and_publish(&mut last, &mut last_pub_at);
                    if let (Some(p), Some(state)) = (persist.as_ref(), last.as_ref()) {
                        drain_requests(p, &mut req_cursor, state);
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
    use super::sources::{CloudKind, CloudObjectRecord, LanHostRecord, MeshSnapshot, NoScan};
    use super::testkit::{FakeLanScan, FakeMeshMirror, FakeOpenstack};
    use super::unit::UnitKind;
    use super::*;
    use mackes_mesh_types::peers::PeerRecord;

    fn worker_with(
        mesh: MeshSnapshot,
        cloud: Vec<CloudObjectRecord>,
        scan: Arc<FakeLanScan>,
    ) -> UnitAggregatorWorker {
        UnitAggregatorWorker::new("me".into(), PathBuf::from("/tmp"))
            .with_bus_root(None)
            .with_mesh(Arc::new(FakeMeshMirror::new(mesh)))
            .with_cloud(Arc::new(FakeOpenstack::new(cloud)))
            .with_scan(scan)
    }

    #[test]
    fn name_and_topic_match_the_census_and_convention() {
        let w = UnitAggregatorWorker::new("node".into(), PathBuf::from("/tmp"));
        assert_eq!(w.name(), "unit_aggregator");
        assert_eq!(state_topic("node-a"), "state/units/node-a");
        assert!(state_topic("x").starts_with("state/"));
    }

    #[test]
    fn fold_state_wires_all_three_seams_in_proximity_order() {
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![PeerRecord::now("me", None, "healthy")],
        };
        let cloud = vec![CloudObjectRecord {
            node: "node-a".into(),
            id: "i1".into(),
            kind: CloudKind::Instance,
            name: "web".into(),
            address: None,
            links: super::sources::CloudLinks::default(),
        }];
        let scan = Arc::new(FakeLanScan::new(vec![LanHostRecord {
            key: "aa:bb".into(),
            name: "printer".into(),
            address: Some("172.20.0.50".into()),
            ..Default::default()
        }]));
        let mut w = worker_with(mesh, cloud, Arc::clone(&scan));
        // Surface visible → the scan runs (lock #24).
        w.scan_flag().store(true, Ordering::Relaxed);
        let state = w.fold_state();
        assert_eq!(state.host, "me");
        let kinds: Vec<UnitKind> = state.units.iter().map(|u| u.kind).collect();
        assert_eq!(
            kinds,
            vec![UnitKind::Peer, UnitKind::LanHost, UnitKind::Instance]
        );
        // Self is first (lock #23).
        assert_eq!(state.units[0].id, super::unit::peer_unit_id("me"));
        // The scan seam saw the active flag.
        assert_eq!(scan.last_active(), Some(true));
        // EXPLORER-7: the fold derives edges from the SAME sources — the cloud
        // instance on node-a yields a HostPlacement edge to that node's peer.
        let placement = state
            .edges
            .iter()
            .find(|e| e.kind == super::edges::EdgeKind::HostPlacement)
            .expect("a host-placement edge for the cloud instance");
        assert_eq!(placement.from, "cloud:instance:i1");
        assert_eq!(placement.to, super::unit::peer_unit_id("node-a"));
    }

    #[test]
    fn scan_gate_off_yields_no_lan_hosts() {
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![],
        };
        let scan = Arc::new(FakeLanScan::new(vec![LanHostRecord {
            key: "aa:bb".into(),
            name: "printer".into(),
            address: None,
            ..Default::default()
        }]));
        let mut w = worker_with(mesh, vec![], Arc::clone(&scan));
        // Default: scan flag false (surface closed) → no probing, no LAN units.
        let state = w.fold_state();
        assert!(state.units.iter().all(|u| u.kind != UnitKind::LanHost));
        assert_eq!(scan.last_active(), Some(false));
    }

    #[test]
    fn verb_drain_answers_a_request_with_the_current_stream() {
        let bus = tempfile::tempdir().unwrap();
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        // A client fires the read verb.
        let req = persist
            .write(UNITS_REQUEST_TOPIC, Priority::Default, None, Some("{}"))
            .unwrap();
        let state = UnitsState {
            host: "node-a".into(),
            units: vec![],
            edges: vec![],
            published_at_ms: 7,
        };
        let mut cursor = None;
        drain_requests(&persist, &mut cursor, &state);
        // The reply landed on reply/<ulid>, ok + carrying the stream.
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        let body = replies
            .into_iter()
            .next_back()
            .and_then(|m| m.body)
            .expect("a reply body");
        let reply: verb::UnitsReply = serde_json::from_str(&body).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.state.expect("state").host, "node-a");
        // The cursor advanced past the handled request.
        assert_eq!(cursor.as_deref(), Some(req.ulid.as_str()));
    }

    #[tokio::test]
    async fn tick_loop_exits_promptly_on_shutdown() {
        let mesh = MeshSnapshot {
            self_host: "node".into(),
            leader: None,
            peers: vec![],
        };
        let mut w = UnitAggregatorWorker::new("node".into(), PathBuf::from("/tmp"))
            .with_bus_root(None)
            .with_mesh(Arc::new(FakeMeshMirror::new(mesh)))
            .with_cloud(Arc::new(FakeOpenstack::new(vec![])))
            .with_scan(Arc::new(NoScan))
            .with_poll(Duration::from_millis(10));
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
