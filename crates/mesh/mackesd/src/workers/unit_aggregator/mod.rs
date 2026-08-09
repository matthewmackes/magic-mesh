//! EXPLORER-1 — the mackesd `unit_aggregator` worker: the daemon spine of the
//! Hero unit explorer (`docs/design/unit-explorer.md`, locked 2026-07-04).
//!
//! One worker unions three sources into a typed [`unit::Unit`] stream and
//! publishes it on the Bus, so the Discovery-surface hero fold (EXPLORER-3) stays
//! a thin renderer (§6 — scanning + privilege live in the daemon, never the GUI).
//!
//! ## Shape (mirrors the `cloud` / BUG-STORAGE-1 `storage` workers)
//!
//! - **Three injectable seams** ([`sources`]), each headless-testable with a fake
//!   ([`testkit`]): [`sources::MeshMirrorSource`] (the peer directory + leader +
//!   health — source (a), lock #2), [`sources::CloudMirrorSource`] (the union
//!   of every node's `state/cloud/<node>` mirror — source (b), lock #20), and
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
//!   fire-and-reap path (the same idiom `state/cloud/<node>` uses).
//! - **The E9 read verb** ([`verb`]) — `action/units/get-stream` → a
//!   `reply/<ulid>` carrying the current stream (units + edges), for any Rust/CLI
//!   mesh client.
//!
//! ## Seams the later EXPLORER slices fill
//! - EXPLORER-2 replaces [`sources::NoScan`] with the real mDNS/ARP/ping-sweep
//!   scan behind [`sources::LanScanSource`], honouring the [`scan_flag`] the
//!   surface toggles (lock #24). The `LanHost` unit producer already lands here.
//! - EXPLORER-9 ([`enrich`]) fills [`unit::Extras`] (offline MAC-OUI vendor,
//!   service→openable-action, fingerprint→type) + the [`unit::CloudDetail`] E4
//!   instance/volume detail folded from the cloud mirror objects. Every field
//!   an unprobed source can't answer stays an explicit `None`/empty (§7).
//!
//! [`scan_flag`]: UnitAggregatorWorker::scan_flag

#![cfg(feature = "async-services")]

pub mod edges;
pub mod enrich;
pub mod fold;
pub mod lan_scan;
pub mod sources;
#[cfg(test)]
pub(crate) mod testkit;
pub mod unit;
pub mod verb;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    BusCloudMirror, CloudMirrorSource, LanScanSource, MeshDirectoryMirror, MeshMirrorSource,
};
use unit::UnitsState;
use verb::{handle_units_request, UNITS_REQUEST_TOPIC};

/// Fold cadence — one mesh + cloud read (+ the gated scan tick) per interval.
/// Same order of cost as the sibling `cloud` worker's heartbeat.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Unconditional mirror republish cadence.
///
/// Between heartbeats the mirror is published only on a content change, so a late
/// subscriber still finds a recent row without the Bus filling with identical
/// bodies.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

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
fn unit_aggregator_bus_root(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn default_bus_root() -> PathBuf {
    unit_aggregator_bus_root(mde_bus::default_data_dir())
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

fn require_live_bus_index(persist: &Persist, bus_root: &Path) -> Result<(), String> {
    let index = bus_root.join("index.sqlite");
    let metadata = std::fs::metadata(&index)
        .map_err(|error| format!("inspect live Bus index {}: {error}", index.display()))?;
    if !metadata.is_file() {
        return Err(format!("live Bus index is not a file: {}", index.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if persist.index_inode() != Some(metadata.ino()) {
            return Err(format!(
                "Bus index identity changed without a successful reopen: {}",
                index.display()
            ));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct PendingReply {
    request_ulid: String,
    topic: String,
    body: String,
}

#[derive(Debug)]
struct RequestState {
    cursor: Option<String>,
    pending: Option<PendingReply>,
}

/// The EXPLORER-1 `unit_aggregator` worker.
pub struct UnitAggregatorWorker {
    /// This node's id — the mirror `host` stamp + topic namespace + self unit.
    host: String,
    /// The mesh half (source (a)).
    mesh: Arc<dyn MeshMirrorSource>,
    /// The cloud half (source (b)).
    /// An injected cloud seam. `None` means the production Bus mirror, resolved
    /// against the current live handle rather than frozen during construction.
    cloud: Option<Arc<dyn CloudMirrorSource>>,
    /// The off-mesh half (EXPLORER-2 producer seam).
    scan: Arc<dyn LanScanSource>,
    /// The surface-gated scan-active flag (lock #24) — the shell sets it only
    /// while Discovery is visible. `NoScan` ignores it today.
    scan_active: Arc<AtomicBool>,
    /// Concrete Bus root. An unresolved user root falls back to the canonical
    /// system spool and is retried by this worker until it becomes available.
    bus_root: PathBuf,
    /// Fold cadence.
    poll: Duration,
    /// Mirror republish heartbeat.
    heartbeat: Duration,
    /// First/last-seen memory across ticks (E10).
    seen: HashMap<String, u64>,
    #[cfg(test)]
    bus_write_gate: Option<Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>>,
}

impl UnitAggregatorWorker {
    /// Construct with production defaults: the replicated peer directory + etcd
    /// leader as the mesh seam, the persisted Bus tree as the cloud-union
    /// seam, the surface-gated active LAN scan ([`LanScan`], EXPLORER-2) as the
    /// off-mesh seam, and the default cadences. `host` is this node's id;
    /// `workgroup_root` seeds the peer-directory reader.
    #[must_use]
    pub fn new(host: String, workgroup_root: PathBuf) -> Self {
        Self {
            mesh: Arc::new(MeshDirectoryMirror::new(workgroup_root, host.clone())),
            host,
            cloud: None,
            scan: Arc::new(LanScan::live()),
            scan_active: Arc::new(AtomicBool::new(false)),
            bus_root: default_bus_root(),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            seen: HashMap::new(),
            #[cfg(test)]
            bus_write_gate: None,
        }
    }

    /// Inject the mesh mirror source (tests).
    #[must_use]
    pub fn with_mesh(mut self, mesh: Arc<dyn MeshMirrorSource>) -> Self {
        self.mesh = mesh;
        self
    }

    /// Inject the cloud-union source (tests).
    #[must_use]
    pub fn with_cloud(mut self, cloud: Arc<dyn CloudMirrorSource>) -> Self {
        self.cloud = Some(cloud);
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
        self.bus_root = unit_aggregator_bus_root(bus_root);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_write_gate(
        mut self,
        gate: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
    ) -> Self {
        self.bus_write_gate = Some(gate);
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
    fn stage_state(
        &self,
        persist: Option<&Persist>,
    ) -> Result<(UnitsState, HashMap<String, u64>), String> {
        let scan_active = self.scan_active.load(Ordering::Relaxed);
        // All three reads are synchronous (the mesh read rides the runtime-aware
        // etcd bridge; the cloud read is fs; the scan is local), completing before
        // any await — so the fold never pins the async runtime.
        let mesh = self.mesh.read();
        let cloud = match self.cloud.as_ref() {
            Some(cloud) => cloud.read_strict()?,
            None => {
                let persist = persist
                    .ok_or_else(|| "production cloud read requires an activated Bus".to_owned())?;
                BusCloudMirror::new(self.bus_root.clone()).read_from(persist)?
            }
        };
        let lan = self.scan.scan(scan_active);
        let now = now_ms();
        // `aggregate` owns the legacy tracker seam. Build with a scratch tracker,
        // then stamp from a cloned durable map so the worker can commit first-seen
        // memory only after every required source and publication succeeds.
        let mut scratch = SeenTracker::new();
        let mut units = aggregate(&mesh, &cloud, &lan, &mut scratch, now);
        let mut staged_seen = self.seen.clone();
        for unit in &mut units {
            let first = *staged_seen.entry(unit.id.clone()).or_insert(now);
            unit.first_seen_ms = first;
            unit.last_seen_ms = now;
        }
        // Derive the typed edge set from the SAME three sources (EXPLORER-7,
        // E2/E8) — no new probes; absent sources yield no edges (§7).
        let edges = derive_edges(&mesh, &cloud, &lan);
        Ok((
            UnitsState {
                host: self.host.clone(),
                units,
                edges,
                published_at_ms: now,
            },
            staged_seen,
        ))
    }

    #[cfg(test)]
    fn fold_state(&mut self) -> UnitsState {
        let (state, seen) = self
            .stage_state(None)
            .expect("injected test sources must fold");
        self.seen = seen;
        state
    }

    /// One fold cycle: build the current state, and publish it when the content
    /// changed or the heartbeat elapsed (publish-on-change, mirroring the
    /// cloud worker).
    fn cycle_and_publish(
        &mut self,
        persist: &mut Persist,
        last: &mut Option<UnitsState>,
        last_pub_at: &mut Option<Instant>,
    ) -> Result<bool, String> {
        persist.reopen_if_index_changed();
        require_live_bus_index(persist, &self.bus_root)?;
        let (state, staged_seen) = self.stage_state(Some(persist))?;
        let body = serde_json::to_string(&state)
            .map_err(|error| format!("encode units mirror: {error}"))?;
        let now = Instant::now();
        let changed = last
            .as_ref()
            .is_none_or(|prev| !prev.same_ignoring_time(&state));
        let heartbeat_due = last_pub_at.is_none_or(|at| now.duration_since(at) >= self.heartbeat);
        if changed || heartbeat_due {
            let topic = state_topic(&self.host);
            #[cfg(test)]
            if let Some(gate) = self.bus_write_gate.as_ref() {
                gate(&topic)?;
            }
            persist
                .write(&topic, Priority::Default, None, Some(&body))
                .map_err(|error| format!("publish {topic}: {error}"))?;
            require_live_bus_index(persist, &self.bus_root)?;
            *last_pub_at = Some(now);
        }
        self.seen = staged_seen;
        *last = Some(state);
        Ok(changed || heartbeat_due)
    }

    fn write_pending_reply(
        &self,
        persist: &Persist,
        requests: &mut RequestState,
    ) -> Result<bool, String> {
        let Some(pending) = requests.pending.as_ref() else {
            return Ok(false);
        };
        #[cfg(test)]
        if let Some(gate) = self.bus_write_gate.as_ref() {
            gate(&pending.topic)?;
        }
        persist
            .write(&pending.topic, Priority::Default, None, Some(&pending.body))
            .map_err(|error| format!("write units reply {}: {error}", pending.topic))?;
        // The reply is the effect. A process crash before this in-memory cursor
        // commit can lose the request on restart because activation deliberately
        // tail-primes retained requests; exactly-once across process crashes needs
        // a durable request/reply ledger, which this protocol does not provide.
        requests.cursor = Some(pending.request_ulid.clone());
        requests.pending = None;
        Ok(true)
    }

    fn drain_one_request(
        &self,
        persist: &Persist,
        requests: &mut RequestState,
        current: &UnitsState,
    ) -> Result<bool, String> {
        if requests.pending.is_some() {
            return self.write_pending_reply(persist, requests);
        }
        let messages = persist
            .list_since(UNITS_REQUEST_TOPIC, requests.cursor.as_deref())
            .map_err(|error| format!("read units requests: {error}"))?;
        let Some(message) = messages.into_iter().next() else {
            return Ok(false);
        };
        let reply = handle_units_request(message.body.as_deref().unwrap_or_default(), current);
        requests.pending = Some(PendingReply {
            topic: reply_topic(&message.ulid),
            request_ulid: message.ulid,
            body: reply.to_body(),
        });
        self.write_pending_reply(persist, requests)
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
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        // Opening the Bus and reading the transient lane tail are one activation
        // transaction. Neither a missing root nor a failed tail read activates a
        // cursor at `None`, so retained requests cannot replay.
        let (mut persist, cursor) = loop {
            let activation = Persist::open(self.bus_root.clone())
                .map_err(|error| format!("open Bus: {error}"))
                .and_then(|persist| {
                    require_live_bus_index(&persist, &self.bus_root)?;
                    let cursor = persist
                        .latest_ulid(UNITS_REQUEST_TOPIC)
                        .map_err(|error| format!("tail-prime units requests: {error}"))?;
                    Ok((persist, cursor))
                });
            match activation {
                Ok(activated) => break activated,
                Err(error) => tracing::warn!(
                    target: "mackesd::units",
                    host = %self.host,
                    %error,
                    "Unit Aggregator Bus unavailable; activation will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = next_bus_retry_interval(retry_interval);
        };
        let mut requests = RequestState {
            cursor,
            pending: None,
        };
        // Fold + publish immediately so a surface doesn't wait a full tick for the
        // first mirror row (lock #23 — self shows instantly).
        if let Err(error) = self.cycle_and_publish(&mut persist, &mut last, &mut last_pub_at) {
            tracing::warn!(target: "mackesd::units", host = %self.host, %error, "Unit Aggregator cycle deferred");
        }
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let replaced = persist.reopen_if_index_changed();
                    if let Err(error) = require_live_bus_index(&persist, &self.bus_root) {
                        tracing::warn!(target: "mackesd::units", host = %self.host, %error, "Unit Aggregator Bus unavailable; cycle deferred");
                        continue;
                    }
                    if replaced && requests.pending.is_none() {
                        match persist.latest_ulid(UNITS_REQUEST_TOPIC) {
                            Ok(cursor) => requests.cursor = cursor,
                            Err(error) => {
                                tracing::warn!(target: "mackesd::units", host = %self.host, %error, "Unit Aggregator replacement Bus tail-prime deferred");
                                continue;
                            }
                        }
                    }
                    if let Err(error) = self.cycle_and_publish(&mut persist, &mut last, &mut last_pub_at) {
                        tracing::warn!(target: "mackesd::units", host = %self.host, %error, "Unit Aggregator cycle deferred");
                        continue;
                    }
                    if let Some(state) = last.as_ref() {
                        if let Err(error) = self.drain_one_request(&persist, &mut requests, state) {
                            tracing::warn!(target: "mackesd::units", host = %self.host, %error, "Unit Aggregator request effect deferred");
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
    use super::sources::{CloudKind, CloudObjectRecord, LanHostRecord, MeshSnapshot, NoScan};
    use super::testkit::{FakeCloud, FakeLanScan, FakeMeshMirror};
    use super::unit::UnitKind;
    use super::*;
    use mackes_mesh_types::peers::PeerRecord;
    use std::sync::atomic::AtomicBool;

    struct FailingCloud {
        fail: Arc<AtomicBool>,
        records: Vec<CloudObjectRecord>,
    }

    impl CloudMirrorSource for FailingCloud {
        fn read(&self) -> Vec<CloudObjectRecord> {
            self.records.clone()
        }

        fn read_strict(&self) -> Result<Vec<CloudObjectRecord>, String> {
            if self.fail.load(Ordering::SeqCst) {
                Err("injected final cloud lane read failure".into())
            } else {
                Ok(self.records.clone())
            }
        }
    }

    fn empty_state(host: &str) -> UnitsState {
        UnitsState {
            host: host.into(),
            units: vec![],
            edges: vec![],
            published_at_ms: 7,
        }
    }

    async fn wait_for_units(bus_root: &Path, needle: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(persist) = Persist::open(bus_root.to_path_buf()) {
                    if let Ok(Some(message)) = persist.read_latest(&state_topic("node")) {
                        if message
                            .body
                            .as_deref()
                            .is_some_and(|body| body.contains(needle))
                        {
                            break;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("timed out waiting for units mirror");
    }

    async fn wait_for_reply(bus_root: &Path, request_ulid: &str) {
        let topic = reply_topic(request_ulid);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(persist) = Persist::open(bus_root.to_path_buf()) {
                    if persist.read_latest(&topic).ok().flatten().is_some() {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("timed out waiting for units reply");
    }

    fn worker_with(
        mesh: MeshSnapshot,
        cloud: Vec<CloudObjectRecord>,
        scan: Arc<FakeLanScan>,
    ) -> UnitAggregatorWorker {
        UnitAggregatorWorker::new("me".into(), PathBuf::from("/tmp"))
            .with_bus_root(None)
            .with_mesh(Arc::new(FakeMeshMirror::new(mesh)))
            .with_cloud(Arc::new(FakeCloud::new(cloud)))
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
            detail: super::unit::CloudDetail::default(),
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
        let worker = UnitAggregatorWorker::new("node-a".into(), PathBuf::from("/tmp"))
            .with_bus_root(Some(bus.path().to_path_buf()));
        let mut requests = RequestState {
            cursor: None,
            pending: None,
        };
        worker
            .drain_one_request(&persist, &mut requests, &state)
            .expect("drain request");
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
        assert_eq!(requests.cursor.as_deref(), Some(req.ulid.as_str()));
    }

    #[test]
    fn final_cloud_lane_failure_has_zero_output_and_seen_mutation() {
        let bus = tempfile::tempdir().expect("temp Bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("open Bus");
        let fail = Arc::new(AtomicBool::new(true));
        let cloud = FailingCloud {
            fail: Arc::clone(&fail),
            records: vec![CloudObjectRecord {
                node: "node".into(),
                id: "new-object".into(),
                kind: CloudKind::Instance,
                name: "must-not-stage".into(),
                address: None,
                links: Default::default(),
                detail: Default::default(),
            }],
        };
        let mut worker = UnitAggregatorWorker::new("node".into(), PathBuf::from("/tmp"))
            .with_bus_root(Some(bus.path().to_path_buf()))
            .with_mesh(Arc::new(FakeMeshMirror::new(MeshSnapshot {
                self_host: "node".into(),
                ..Default::default()
            })))
            .with_cloud(Arc::new(cloud))
            .with_scan(Arc::new(NoScan));
        let mut last = None;
        let mut last_pub_at = None;

        assert!(worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .is_err());
        assert!(worker.seen.is_empty());
        assert!(last.is_none());
        assert!(last_pub_at.is_none());
        assert!(persist
            .read_latest(&state_topic("node"))
            .expect("read output")
            .is_none());
    }

    #[test]
    fn mirror_write_failure_preserves_immediate_retry_and_seen_state() {
        let bus = tempfile::tempdir().expect("temp Bus");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("open Bus");
        let fail = Arc::new(AtomicBool::new(true));
        let gate_fail = Arc::clone(&fail);
        let mut worker = UnitAggregatorWorker::new("node".into(), PathBuf::from("/tmp"))
            .with_bus_root(Some(bus.path().to_path_buf()))
            .with_mesh(Arc::new(FakeMeshMirror::new(MeshSnapshot {
                self_host: "node".into(),
                ..Default::default()
            })))
            .with_cloud(Arc::new(FakeCloud::new(vec![])))
            .with_scan(Arc::new(NoScan))
            .with_bus_write_gate(Arc::new(move |topic| {
                if topic.starts_with("state/units/") && gate_fail.load(Ordering::SeqCst) {
                    return Err("injected mirror write failure".into());
                }
                Ok(())
            }));
        let mut last = None;
        let mut last_pub_at = None;

        assert!(worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .is_err());
        assert!(worker.seen.is_empty());
        assert!(last.is_none());
        assert!(last_pub_at.is_none());

        fail.store(false, Ordering::SeqCst);
        assert!(worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .expect("corrected-forward retry"));
        assert_eq!(worker.seen.len(), 1);
        assert!(last.is_some());
        assert!(last_pub_at.is_some());
    }

    #[test]
    fn reply_write_failure_retries_cached_reply_once_before_cursor() {
        let bus = tempfile::tempdir().expect("temp Bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("open Bus");
        let request = persist
            .write(UNITS_REQUEST_TOPIC, Priority::Default, None, Some("{}"))
            .expect("write request");
        let failures = Arc::new(AtomicBool::new(true));
        let gate_failures = Arc::clone(&failures);
        let worker = UnitAggregatorWorker::new("node".into(), PathBuf::from("/tmp"))
            .with_bus_root(Some(bus.path().to_path_buf()))
            .with_bus_write_gate(Arc::new(move |topic| {
                if topic.starts_with("reply/") && gate_failures.swap(false, Ordering::SeqCst) {
                    return Err("injected reply failure".into());
                }
                Ok(())
            }));
        let mut requests = RequestState {
            cursor: None,
            pending: None,
        };
        let original = empty_state("original");

        assert!(worker
            .drain_one_request(&persist, &mut requests, &original)
            .is_err());
        assert!(requests.cursor.is_none());
        assert_eq!(
            requests.pending.as_ref().unwrap().request_ulid,
            request.ulid
        );

        worker
            .drain_one_request(&persist, &mut requests, &empty_state("changed"))
            .expect("retry cached reply");
        assert_eq!(requests.cursor.as_deref(), Some(request.ulid.as_str()));
        assert!(requests.pending.is_none());
        let replies = persist
            .list_since(&reply_topic(&request.ulid), None)
            .expect("list replies");
        assert_eq!(replies.len(), 1);
        assert!(replies[0].body.as_deref().unwrap().contains("original"));
    }

    #[tokio::test]
    async fn same_worker_recovers_late_bus_tail_primes_and_follows_replacement() {
        let base = tempfile::tempdir().expect("base tempdir");
        let bus_root = base.path().join("live-bus");
        std::fs::write(&bus_root, b"temporarily unavailable").expect("block Bus root");
        let seeded = base.path().join("seeded-bus");
        let seeded_persist = Persist::open(seeded.clone()).expect("open seeded Bus");
        seeded_persist
            .write(
                "state/cloud/node-a",
                Priority::Default,
                None,
                Some(r#"{"objects":[{"id":"first","kind":"instance","name":"Initial Cloud"}]}"#),
            )
            .expect("seed cloud");
        let retained = seeded_persist
            .write(UNITS_REQUEST_TOPIC, Priority::Default, None, Some("{}"))
            .expect("seed retained request");
        drop(seeded_persist);

        let mut worker = UnitAggregatorWorker::new("node".into(), PathBuf::from("/tmp"))
            .with_bus_root(Some(bus_root.clone()))
            .with_mesh(Arc::new(FakeMeshMirror::new(MeshSnapshot {
                self_host: "node".into(),
                ..Default::default()
            })))
            .with_scan(Arc::new(NoScan))
            .with_poll(Duration::from_millis(5));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(!task.is_finished(), "late Bus must not terminate worker");
        std::fs::remove_file(&bus_root).expect("unblock Bus root");
        std::fs::rename(&seeded, &bus_root).expect("activate seeded Bus");
        wait_for_units(&bus_root, "Initial Cloud").await;
        let live = Persist::open(bus_root.clone()).expect("open live Bus");
        assert!(live
            .read_latest(&reply_topic(&retained.ulid))
            .expect("read retained reply")
            .is_none());

        live.write(
            "state/cloud/node-a",
            Priority::Default,
            None,
            Some(r#"{"objects":[{"id":"forward","kind":"instance","name":"Forward Cloud"}]}"#),
        )
        .expect("external cloud update");
        let forward = live
            .write(UNITS_REQUEST_TOPIC, Priority::Default, None, Some("{}"))
            .expect("forward request");
        wait_for_units(&bus_root, "Forward Cloud").await;
        wait_for_reply(&bus_root, &forward.ulid).await;
        drop(live);

        let detached = base.path().join("detached-bus");
        std::fs::rename(&bus_root, &detached).expect("detach Bus");
        let replacement = base.path().join("replacement-bus");
        let replacement_persist = Persist::open(replacement.clone()).expect("open replacement");
        replacement_persist
            .write(
                "state/cloud/node-b",
                Priority::Default,
                None,
                Some(r#"{"objects":[{"id":"replacement","kind":"volume","name":"Replacement Cloud"}]}"#),
            )
            .expect("replacement cloud");
        drop(replacement_persist);
        std::fs::rename(&replacement, &bus_root).expect("install replacement Bus");
        wait_for_units(&bus_root, "Replacement Cloud").await;
        let replacement_live = Persist::open(bus_root.clone()).expect("open replacement live");
        let replacement_forward = replacement_live
            .write(UNITS_REQUEST_TOPIC, Priority::Default, None, Some("{}"))
            .expect("replacement forward request");
        wait_for_reply(&bus_root, &replacement_forward.ulid).await;

        shutdown_tx.send(true).expect("signal shutdown");
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("shutdown timeout")
            .expect("join worker")
            .expect("worker result");
    }

    #[tokio::test]
    async fn tick_loop_exits_promptly_on_shutdown() {
        let base = tempfile::tempdir().expect("base tempdir");
        let unavailable = base.path().join("unavailable-bus");
        std::fs::write(&unavailable, b"not a directory").expect("block Bus root");
        let mesh = MeshSnapshot {
            self_host: "node".into(),
            leader: None,
            peers: vec![],
        };
        let mut w = UnitAggregatorWorker::new("node".into(), PathBuf::from("/tmp"))
            .with_bus_root(Some(unavailable))
            .with_mesh(Arc::new(FakeMeshMirror::new(mesh)))
            .with_cloud(Arc::new(FakeCloud::new(vec![])))
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

    #[test]
    fn default_bus_root_uses_the_shared_mde_bus_resolver() {
        assert_eq!(
            default_bus_root(),
            unit_aggregator_bus_root(mde_bus::default_data_dir())
        );
        assert_eq!(
            unit_aggregator_bus_root(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
    }
}
