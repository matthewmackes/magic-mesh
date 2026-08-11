//! MV-5a — `scheduler`: the **placement slice** of the no-center scheduler.
//!
//! Where MV-3 ([`super::vm_lifecycle`]) and MV-4 ([`super::container`]) are the
//! capability-gated per-node *actuators* and MV-2 ([`super::kvm_health`]) is the
//! per-node *capacity signal* (`event/kvm/services`), MV-5a is only the
//! *chooser*: it turns a host-agnostic `action/schedule/place` request into an
//! auditable placement proposal and desired-state record.
//!
//! The scheduler deliberately never publishes to either privileged lifecycle
//! topic and never receives or mints an arming capability. Execution must come
//! later through an operator-authorized typed lane. This keeps an unsigned
//! request on the mesh-writable placement topic from becoming root
//! `virsh`/`podman` execution.
//!
//! ## Shape (mirrors `vm_lifecycle`)
//!
//! - The **pure core** is fully unit-tested with no bus: [`fold_capacity`]
//!   (newest-publication-by-host fold of `event/kvm/services` with replay and
//!   equivocation quarantine), [`choose_node`] (the placement decision), and
//!   [`plan_placement`] (what to publish) never touch the bus or a clock
//!   (`now_ms` is passed in).
//! - The sole outward seam is an injectable [`Publisher`] (production
//!   [`BusPublisher`] writes only non-privileged placement, desired-state, and
//!   correlated reply topics). Each pass resolves and generation-checks a fresh
//!   Bus connection, stages complete bounded reads before publication, and uses
//!   a durable host-local outbox to recover required outputs without repeating
//!   already-visible proposal rows. Every newly observed Bus index atomically
//!   tail-primes retained transient actions before admitting forward work.
//! - Rank-0-default like `vm_lifecycle` / `container` (runs on every node). An
//!   **interim** lowest-node-id single-actor election ([`is_leader`]) keeps N
//!   nodes each running this worker from emitting N duplicate placements — it's
//!   a pure function of the shared capacity view, so no consensus is needed.
//!
//! ## Scope — placement (MV-5a) + failover (MV-5b)
//!
//! MV-5a is *place-on-request*: choose a node and record a proposal. It does not
//! execute that proposal.
//!
//! **MV-5b** adds the *survives-node-loss* half on the SAME seams — no new
//! worker, no new consensus:
//! - **Desired-state persistence:** every decided placement is also persisted as
//!   a [`DesiredPlacement`] (`{kind, spec, chosen_host, request_id}`) to the
//!   [`DESIRED_TOPIC`] on the same bus [`Persist`] MV-5a already reads/writes, so
//!   the intent outlives a restart or a leader change. Read back latest-wins-by-key
//!   ([`fold_desired`]).
//! - **Pure re-placement:** [`replace_decisions`] re-picks a live host (via
//!   [`choose_node`] over the surviving capacity) for any persisted placement whose
//!   node has left the mesh — never the dead node, skipped when nothing is live.
//! - **Failover tick + HA re-election:** the leader (now the lowest *live* node —
//!   [`is_failover_leader`], the re-election MV-5a's stale-capacity [`is_leader`]
//!   deferred) records a new proposal for the replacement node and updates the
//!   persisted desired-state, in the existing worker loop. It still does not
//!   bypass operator authorization to execute the workload.
//!
//! The live-node set is the etcd-lease-backed peer directory
//! ([`crate::substrate::peers::read_directory`], seam [`LiveDirectory`]): liveness
//! IS the keepalive lease — a departed node's row auto-deletes — so no staleness
//! guess is needed.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use sha2::{Digest, Sha256};

use super::kvm_health::KvmHealth;
use super::{ShutdownToken, Worker};

/// Bus topic the worker drains for placement requests (host-agnostic — the
/// request's optional `host` is a placement *pin*, not a per-scheduler target).
pub const ACTION_TOPIC: &str = "action/schedule/place";

/// Bus topic the worker publishes each placement decision to.
pub const PLACEMENTS_TOPIC: &str = "event/schedule/placements";

/// MV-5b — bus topic the worker persists desired-state ([`DesiredPlacement`]) to.
/// An `event/` topic (persisted + mesh-replicated like `event/kvm/services`), so
/// the intent survives a restart / leader change and every node folds the same
/// desired-state view. Read back latest-wins-by-key ([`fold_desired`]).
pub const DESIRED_TOPIC: &str = "event/schedule/desired";

/// Action-drain cadence. The bus read is a cheap local log scan; placement is a
/// slow, operator-visible event, so a 2 s poll is responsive without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A mesh node identity (the `host` a [`KvmHealth`] summary is stamped with, and
/// the key of the capacity map).
pub type NodeId = String;

// ───────────────────────────── data model ─────────────────────────────

/// The kind of workload represented by a placement proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    /// A libvirt/KVM VM proposal.
    Vm,
    /// A Podman container proposal.
    Container,
}

/// One placement request drained off [`ACTION_TOPIC`]. The `spec` is opaque and
/// retained in the proposal; the scheduler never interprets or executes it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlaceRequest {
    /// Whether to place a VM or a container.
    pub kind: PlaceKind,
    /// The proposed workload spec, retained untouched.
    pub spec: serde_json::Value,
    /// An optional node *pin* — honored iff that node is a healthy candidate,
    /// otherwise the scheduler falls back to the healthiest node.
    #[serde(default)]
    pub host: Option<NodeId>,
    /// An optional caller correlation id, echoed into the placement decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Parse a [`PlaceRequest`] body.
///
/// # Errors
/// A human-readable message on malformed JSON / unknown `kind`.
pub fn parse_request(body: &str) -> Result<PlaceRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed place request: {e}"))
}

/// The decision published to [`PLACEMENTS_TOPIC`] — an audit trail of the node
/// the scheduler recommends. The full spec lives in [`DESIRED_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlacementDecision {
    /// The caller's correlation id, if the request carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The placed workload kind.
    pub kind: PlaceKind,
    /// The node the workload was placed on.
    pub chosen_host: NodeId,
    /// How many candidate nodes the decision considered.
    pub candidates: usize,
    /// Wall-clock decision time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

/// The concrete, non-executing outcome of a placement decision. Returned by the
/// pure [`plan_placement`] so the request → event wiring is testable without a
/// bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The chosen node.
    pub chosen_host: NodeId,
    /// The complete desired-state proposal. This is data, not actuator input.
    pub desired: DesiredPlacement,
    /// The decision to publish to [`PLACEMENTS_TOPIC`].
    pub decision: PlacementDecision,
}

/// MV-5b — the persisted *desired state* of one placed workload: the minimal
/// intent (`{kind, spec, chosen_host, request_id}`) needed to rebuild — or
/// **re-place** — a [`Placement`] after a restart or a node loss. Persisted to
/// [`DESIRED_TOPIC`] and folded latest-wins-by-key ([`fold_desired`]); it is a
/// lossless projection of a `Placement` (the audit-only `candidates` /
/// `published_at_ms` are recomputed, not stored).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesiredPlacement {
    /// Whether a VM or a container was placed.
    pub kind: PlaceKind,
    /// The proposed workload spec, retained untouched (opaque).
    pub spec: serde_json::Value,
    /// The node the workload is currently desired to run on.
    pub chosen_host: NodeId,
    /// The caller correlation id, if the request carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl DesiredPlacement {
    /// Build a non-executing [`Placement`] proposal for this desired-state.
    /// `candidates` / `now_ms` seed the audit decision.
    #[must_use]
    pub fn to_placement(&self, candidates: usize, now_ms: u64) -> Placement {
        Placement {
            chosen_host: self.chosen_host.clone(),
            desired: self.clone(),
            decision: PlacementDecision {
                request_id: self.request_id.clone(),
                kind: self.kind,
                chosen_host: self.chosen_host.clone(),
                candidates,
                published_at_ms: now_ms,
            },
        }
    }
}

// ─────────────────────────── pure: decision ───────────────────────────

/// Fold a stream of `event/kvm/services` bodies into a newest-publication-by-host
/// capacity map. Bus delivery order is not health-history authority: an older
/// replicated row cannot roll a host back, and conflicting bodies at the same
/// publication time quarantine that host until a strictly newer observation
/// arrives. Exact duplicate delivery remains idempotent. Unparseable bodies are
/// skipped.
#[must_use]
pub fn fold_capacity<'a>(bodies: impl IntoIterator<Item = &'a str>) -> BTreeMap<NodeId, KvmHealth> {
    let mut map = BTreeMap::<NodeId, KvmHealth>::new();
    let mut equivocated_at = BTreeMap::<NodeId, u64>::new();
    for body in bodies {
        if let Ok(h) = serde_json::from_str::<KvmHealth>(body) {
            let host = h.host.clone();
            if equivocated_at
                .get(&host)
                .is_some_and(|watermark| h.published_at_ms <= *watermark)
            {
                continue;
            }

            match map.get(&host) {
                Some(current) if h.published_at_ms < current.published_at_ms => {}
                Some(current) if h.published_at_ms == current.published_at_ms => {
                    if current != &h {
                        map.remove(&host);
                        equivocated_at.insert(host, h.published_at_ms);
                    }
                }
                _ => {
                    equivocated_at.remove(&host);
                    map.insert(host, h);
                }
            }
        }
    }
    map
}

/// The pure placement decision: pick the target node for `req` from
/// `candidates` (a `(node_id, health)` slice — the folded capacity map).
///
/// - If `req.host` is `Some` **and** that node is a healthy candidate
///   ([`KvmHealth::all_healthy`]), honor the pin.
/// - Otherwise the node with the most active services
///   ([`KvmHealth::active`]), deterministic tie-break by `node_id` ascending.
/// - `None` iff there are no candidates.
///
/// No I/O — fully unit-testable.
#[must_use]
pub fn choose_node(candidates: &[(NodeId, KvmHealth)], req: &PlaceRequest) -> Option<NodeId> {
    if candidates.is_empty() {
        return None;
    }
    // 1. Honor an explicit, *healthy* pin. An absent-or-unhealthy pin falls
    //    through to the capacity-ranked pick.
    if let Some(pin) = req.host.as_deref() {
        if let Some((id, health)) = candidates.iter().find(|(id, _)| id == pin) {
            if health.all_healthy {
                return Some(id.clone());
            }
        }
    }
    // 2. Most active services wins; ties break to the smallest node_id. The
    //    node_id half of the key is unique, so the pick is order-independent
    //    (no reliance on `max_by`'s last-of-equals rule).
    candidates
        .iter()
        .max_by(|x, y| x.1.active.cmp(&y.1.active).then_with(|| y.0.cmp(&x.0)))
        .map(|(id, _)| id.clone())
}

/// Compose the full placement outcome for `req` over the folded `capacity`:
/// choose the node ([`choose_node`]) then build the desired-state proposal and
/// decision record. `None` when there is no candidate to place onto.
/// Pure — driven directly by tests without a bus.
#[must_use]
pub fn plan_placement(
    capacity: &BTreeMap<NodeId, KvmHealth>,
    req: &PlaceRequest,
    now_ms: u64,
) -> Option<Placement> {
    let candidates: Vec<(NodeId, KvmHealth)> = capacity
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let chosen = choose_node(&candidates, req)?;
    // Retain the opaque spec with the chosen node as persistable desired state.
    // No privileged lifecycle request is synthesized here.
    let desired = DesiredPlacement {
        kind: req.kind,
        spec: req.spec.clone(),
        chosen_host: chosen,
        request_id: req.request_id.clone(),
    };
    Some(desired.to_placement(candidates.len(), now_ms))
}

/// Interim single-actor election: the node whose id sorts first among the nodes
/// currently reporting KVM health is this slice's active scheduler. Every node
/// folds the same `event/kvm/services` view, so they agree on one actor without
/// consensus — which keeps N rank-0 schedulers from emitting N duplicate
/// placements. `false` on an empty map (no capacity ⇒ nothing to place onto).
/// HA re-election on leader loss is MV-5b.
#[must_use]
pub fn is_leader(host: &str, capacity: &BTreeMap<NodeId, KvmHealth>) -> bool {
    capacity.keys().next().map(String::as_str) == Some(host)
}

// ─────────────────────── pure: failover (MV-5b) ───────────────────────

/// Strip the `peer:` node-id prefix to the bare hostname. The capacity map +
/// [`Placement::chosen_host`] are keyed by the full node-id (`peer:<host>`), while
/// the mesh peer directory stores the bare `hostname` (the `telemetry` heartbeat
/// strips `peer:` before writing its `PeerRecord`). Normalizing both sides here is
/// what lets [`live_node_ids`] compare the two namespaces.
fn bare_host(id: &str) -> &str {
    id.strip_prefix("peer:").unwrap_or(id)
}

/// Reconcile the mesh peer `directory` (bare hostnames — liveness IS the etcd
/// keepalive lease, so a departed node is simply absent) against the node-id-keyed
/// `capacity` map: a capacity node is **live** iff its bare hostname is present in
/// the directory. The returned set is in the capacity / [`Placement::chosen_host`]
/// node-id namespace, so [`replace_decisions`] compares it directly. A node that
/// reports capacity but has left the directory (its lease lapsed) is *not* live —
/// which is exactly the node-loss [`replace_decisions`] re-places away from. Pure.
#[must_use]
pub fn live_node_ids(
    directory: &BTreeSet<NodeId>,
    capacity: &BTreeMap<NodeId, KvmHealth>,
) -> BTreeSet<NodeId> {
    let bare_dir: BTreeSet<&str> = directory.iter().map(|h| bare_host(h)).collect();
    capacity
        .keys()
        .filter(|id| bare_dir.contains(bare_host(id.as_str())))
        .cloned()
        .collect()
}

/// The failover actor: the lowest node-id among the **live** nodes. Unlike the
/// placement-path [`is_leader`] (lowest in the never-expiring capacity map, which
/// keeps electing a node that has since died), this re-elects over the live set —
/// so when the leader itself is the lost node, the next live node picks up
/// re-placement. This is the HA re-election MV-5a deferred. `false` on an empty
/// live set (nobody to lead / nothing live to place onto).
#[must_use]
pub fn is_failover_leader(host: &str, live: &BTreeSet<NodeId>) -> bool {
    live.iter().next().map(String::as_str) == Some(host)
}

/// The mesh-scoped key a [`DesiredPlacement`] is folded under: the caller's
/// `request_id` when present, else the workload identity `(kind, spec)`. Stable
/// across a re-placement (only `chosen_host` changes), so a re-placement's new
/// record shadows the prior one in [`fold_desired`] rather than duplicating it.
fn desired_key(d: &DesiredPlacement) -> String {
    d.request_id.clone().unwrap_or_else(|| {
        format!(
            "{}:{}",
            serde_json::to_string(&d.kind).unwrap_or_default(),
            serde_json::to_string(&d.spec).unwrap_or_default(),
        )
    })
}

/// Fold a stream of [`DESIRED_TOPIC`] bodies into a latest-wins-by-key desired-state
/// map (later records for the same [`desired_key`] overwrite earlier ones — exactly
/// like [`fold_capacity`]). Unparseable bodies are skipped. This is what makes the
/// failover tick idempotent: after a re-placement re-persists a workload onto its
/// new node, the next fold sees only that latest record, not the stale one.
#[must_use]
pub fn fold_desired<'a>(
    bodies: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, DesiredPlacement> {
    let mut map = BTreeMap::new();
    for body in bodies {
        if let Ok(d) = serde_json::from_str::<DesiredPlacement>(body) {
            map.insert(desired_key(&d), d);
        }
    }
    map
}

/// The pure re-placement decision: for each persisted [`Placement`] whose
/// `chosen_host` is **not** in `live`, re-pick a target from the surviving live
/// capacity ([`choose_node`] over `capacity ∩ live`) and rebuild a non-executing
/// [`Placement`] proposal for the new node. A placement whose node is still live
/// is left alone; one with no live candidate to move to is skipped. The dead node
/// is never a candidate (it is absent from `live`, hence from the filtered
/// capacity), so a workload is never re-placed back onto the node it is failing
/// away from.
/// Deterministic (input order preserved, [`choose_node`] tie-break) and clock-free
/// — the failover tick stamps the fresh audit time on the returned decisions.
#[must_use]
pub fn replace_decisions(
    persisted: &[Placement],
    live: &BTreeSet<NodeId>,
    capacity: &BTreeMap<NodeId, KvmHealth>,
) -> Vec<Placement> {
    // Candidate pool = capacity entries for still-live nodes only. The lost node
    // is excluded here, so `choose_node` can never re-pick it.
    let live_candidates: Vec<(NodeId, KvmHealth)> = capacity
        .iter()
        .filter(|(id, _)| live.contains(*id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut out = Vec::new();
    for p in persisted {
        if live.contains(&p.chosen_host) {
            continue; // node still alive — the workload keeps running, nothing to do
        }
        // Clone the desired workload, then re-choose over the LIVE capacity. A
        // pin-less request: the original pin, if any, was the node we're leaving.
        let desired = p.desired.clone();
        let req = PlaceRequest {
            kind: desired.kind,
            spec: serde_json::Value::Null, // choose_node reads only the (absent) pin
            host: None,
            request_id: desired.request_id.clone(),
        };
        let Some(new_host) = choose_node(&live_candidates, &req) else {
            continue; // nothing live to place onto — leave the intent as-is
        };
        let replaced = DesiredPlacement {
            chosen_host: new_host,
            ..desired
        };
        out.push(replaced.to_placement(live_candidates.len(), p.decision.published_at_ms));
    }
    out
}

// ─────────────────────────── bus + worker ───────────────────────────

const MAX_ACTIONS_PER_SWEEP: usize = 64;
const MAX_TOPIC_PAGE: usize = 64;
const OUTBOX_DIR: &str = "scheduler-outbox";
const MAX_OUTBOX_RECORDS: usize = 128;
const MAX_OUTBOX_RECORD_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BusIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SchedulerReply {
    schema_version: u16,
    accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decision: Option<PlacementDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PendingOutput {
    topic: String,
    body: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SchedulerOutboxRecord {
    schema_version: u16,
    record_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_ulid: Option<String>,
    outputs: Vec<PendingOutput>,
}

struct SchedulerOutbox {
    root: PathBuf,
}

impl SchedulerOutbox {
    fn open(state_root: &Path) -> io::Result<Self> {
        fs::create_dir_all(state_root)?;
        let root = state_root.join(OUTBOX_DIR);
        fs::create_dir_all(&root)?;
        let metadata = fs::symlink_metadata(&root)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "scheduler outbox is not a regular directory",
            ));
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        Ok(Self { root })
    }

    fn path(&self, record_id: &str) -> PathBuf {
        self.root.join(format!("{record_id}.json"))
    }

    fn validate(record: &SchedulerOutboxRecord) -> io::Result<()> {
        if record.schema_version != 1
            || record.record_id.is_empty()
            || record.record_id.len() > 96
            || !record
                .record_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || record.outputs.is_empty()
            || record.outputs.len() > 3
            || record.outputs.iter().any(|output| {
                output.topic.starts_with("action/")
                    || !(output.topic == PLACEMENTS_TOPIC
                        || output.topic == DESIRED_TOPIC
                        || output.topic.starts_with("reply/"))
            })
        {
            return Err(io::Error::other(
                "scheduler outbox record failed validation",
            ));
        }
        Ok(())
    }

    fn store(&self, record: &SchedulerOutboxRecord) -> io::Result<()> {
        Self::validate(record)?;
        let destination = self.path(&record.record_id);
        if !destination.exists() {
            let retained = fs::read_dir(&self.root)?
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|value| value == "json")
                })
                .count();
            if retained >= MAX_OUTBOX_RECORDS {
                return Err(io::Error::other("scheduler outbox is at capacity"));
            }
        }
        let body = serde_json::to_vec(record).map_err(io_other)?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > MAX_OUTBOX_RECORD_BYTES {
            return Err(io::Error::other("scheduler outbox record is oversized"));
        }
        let temporary = self.root.join(format!(
            ".{}.{}.{}.tmp",
            record.record_id,
            std::process::id(),
            now_ms()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        if let Err(error) = file.write_all(&body).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        fs::File::open(&self.root)?.sync_all()
    }

    fn decode(&self, path: &Path) -> io::Result<SchedulerOutboxRecord> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_OUTBOX_RECORD_BYTES
        {
            return Err(io::Error::other(
                "scheduler outbox contains an unsafe record",
            ));
        }
        let record: SchedulerOutboxRecord =
            serde_json::from_slice(&fs::read(path)?).map_err(io_other)?;
        Self::validate(&record)?;
        if self.path(&record.record_id) != path {
            return Err(io::Error::other(
                "scheduler outbox filename does not match its record",
            ));
        }
        Ok(record)
    }

    fn load(&self, record_id: &str) -> io::Result<Option<SchedulerOutboxRecord>> {
        let path = self.path(record_id);
        if !path.exists() {
            return Ok(None);
        }
        self.decode(&path).map(Some)
    }

    fn pending(&self) -> io::Result<Vec<SchedulerOutboxRecord>> {
        let mut paths = fs::read_dir(&self.root)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|value| value == "json"))
            .collect::<Vec<_>>();
        paths.sort();
        if paths.len() > MAX_OUTBOX_RECORDS {
            return Err(io::Error::other(
                "scheduler outbox exceeds its bounded capacity",
            ));
        }
        paths.iter().map(|path| self.decode(path)).collect()
    }

    fn remove(&self, record_id: &str) -> io::Result<()> {
        match fs::remove_file(self.path(record_id)) {
            Ok(()) => fs::File::open(&self.root)?.sync_all(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// The outward publication seam. Production writes through the already-open,
/// generation-checked Bus transaction; tests may inject precise write faults.
pub trait Publisher {
    /// Compatibility publication seam used by sibling onboarding workers.
    fn publish(&self, topic: &str, body: &str);

    /// Scheduler transaction publication. Existing external implementors retain
    /// their two-argument API; production overrides this method so failures and
    /// the exact staged Bus connection remain visible.
    fn publish_transaction(&self, _persist: &Persist, topic: &str, body: &str) -> io::Result<()> {
        self.publish(topic, body);
        Ok(())
    }
}

/// Production publisher. Scheduler-owned transactions use
/// [`Publisher::publish_transaction`] so write errors remain visible.
#[derive(Debug, Clone, Copy, Default)]
pub struct BusPublisher;

impl Publisher for BusPublisher {
    fn publish(&self, topic: &str, body: &str) {
        if let Ok(persist) =
            Persist::open(scheduler_bus_root_or_system(mde_bus::default_data_dir()))
        {
            let _ = persist.write(topic, Priority::Default, None, Some(body));
        }
    }

    fn publish_transaction(&self, persist: &Persist, topic: &str, body: &str) -> io::Result<()> {
        persist
            .write(topic, Priority::Default, None, Some(body))
            .map(|_| ())
            .map_err(io_other)
    }
}

#[derive(Clone, Copy)]
struct BusTransaction<'a> {
    persist: &'a Persist,
    root: &'a Path,
    identity: BusIdentity,
}

impl BusTransaction<'_> {
    fn verify_current(self) -> io::Result<()> {
        if bus_identity(self.root)? != self.identity {
            return Err(io::Error::other(
                "scheduler Bus index changed during transaction",
            ));
        }
        Ok(())
    }
}

struct StagedRecord {
    record: SchedulerOutboxRecord,
    already_present: Vec<bool>,
}

struct StagedSweep {
    pending: Vec<StagedRecord>,
    actions: Vec<mde_bus::persist::StoredMessage>,
    capacity: BTreeMap<NodeId, KvmHealth>,
    desired: Vec<DesiredPlacement>,
}

#[cfg(test)]
#[derive(Default)]
struct SchedulerBusFaults {
    fail_action_reads: std::sync::atomic::AtomicU64,
    fail_capacity_reads: std::sync::atomic::AtomicU64,
    fail_desired_reads: std::sync::atomic::AtomicU64,
    replace_index_after_open: std::sync::Mutex<Option<PathBuf>>,
}

/// MV-5b — the "who is alive right now" seam the failover tick reads. Production
/// wires [`PeerDirectory`] (the etcd lease-backed mesh directory); tests wire a
/// fixed set so the pure re-placement wiring runs without etcd.
pub trait LiveDirectory {
    /// The hostnames currently present in the mesh peer directory (bare — the
    /// `peer:` prefix is normalized against capacity in [`live_node_ids`]).
    fn live_hostnames(&self) -> BTreeSet<NodeId>;
}

/// Production [`LiveDirectory`]: the canonical etcd-first peer directory
/// ([`crate::substrate::peers::read_directory`]), where **liveness is the etcd
/// keepalive lease** — a departed node's row auto-deletes, so a stale
/// `last_seen_ms` guess is never needed. Falls back to the replicated fs union
/// under `workgroup_root` when the coordination plane is un-provisioned (same
/// precedence every other directory reader uses).
pub struct PeerDirectory {
    /// Shared-storage root — the fs-union fallback when etcd is absent.
    workgroup_root: PathBuf,
}

impl LiveDirectory for PeerDirectory {
    fn live_hostnames(&self) -> BTreeSet<NodeId> {
        crate::substrate::peers::read_directory(&self.workgroup_root)
            .into_iter()
            .map(|r| r.hostname)
            .collect()
    }
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn bus_identity(root: &Path) -> io::Result<BusIdentity> {
    let metadata = fs::metadata(root.join("index.sqlite"))?;
    if !metadata.is_file() {
        return Err(io::Error::other(
            "scheduler Bus index is not a regular file",
        ));
    }
    Ok(BusIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn scheduler_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn read_topic_complete(
    persist: &Persist,
    topic: &str,
) -> io::Result<Vec<mde_bus::persist::StoredMessage>> {
    let mut messages = Vec::new();
    let mut cursor = None;
    loop {
        let page = persist
            .list_since_limit(topic, cursor.as_deref(), MAX_TOPIC_PAGE)
            .map_err(io_other)?;
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|message| message.ulid.clone());
        let complete = page.len() < MAX_TOPIC_PAGE;
        messages.extend(page);
        if complete {
            break;
        }
    }
    Ok(messages)
}

fn topic_contains(persist: &Persist, topic: &str, body: &str) -> io::Result<bool> {
    Ok(read_topic_complete(persist, topic)?
        .iter()
        .any(|message| message.body.as_deref() == Some(body)))
}

#[cfg(test)]
fn take_fault(counter: &std::sync::atomic::AtomicU64) -> bool {
    use std::sync::atomic::Ordering;
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
}

#[cfg(test)]
fn install_replacement_index(root: &Path, replacement: &Path) -> io::Result<()> {
    for sidecar in ["index.sqlite-wal", "index.sqlite-shm"] {
        match fs::remove_file(root.join(sidecar)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::rename(replacement, root.join("index.sqlite"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The placement (MV-5a) + failover (MV-5b) worker.
pub struct SchedulerWorker {
    /// This node's id — its identity in the [`is_leader`] /
    /// [`is_failover_leader`] elections.
    host: NodeId,
    /// The injectable publish seam (production: [`BusPublisher`]).
    publisher: Box<dyn Publisher + Send + Sync>,
    /// The injectable live-node seam (production: [`PeerDirectory`]).
    live_dir: Box<dyn LiveDirectory + Send + Sync>,
    /// Action-drain cadence.
    poll: Duration,
    /// Bus root override (tests). `None` resolves current/system per pass.
    bus_root_override: Option<PathBuf>,
    bus_identity: Option<BusIdentity>,
    cursor: Option<String>,
    state_root: PathBuf,
    #[cfg(test)]
    bus_faults: Arc<SchedulerBusFaults>,
}

impl SchedulerWorker {
    /// Construct with production defaults: the live [`BusPublisher`], the etcd
    /// [`PeerDirectory`], the default cadence, and the auto-resolved bus root.
    /// `host` is this node's id.
    #[must_use]
    pub fn new(host: NodeId) -> Self {
        Self {
            host,
            publisher: Box::new(BusPublisher),
            live_dir: Box::new(PeerDirectory {
                workgroup_root: crate::default_qnm_shared_root(),
            }),
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            bus_identity: None,
            cursor: None,
            state_root: crate::default_db_path()
                .parent()
                .map(|parent| parent.join("scheduler"))
                .unwrap_or_else(|| PathBuf::from("/var/lib/mde/scheduler")),
            #[cfg(test)]
            bus_faults: Arc::new(SchedulerBusFaults::default()),
        }
    }

    /// Inject a publisher (tests). Production uses the [`BusPublisher`] default.
    #[must_use]
    pub fn with_publisher(mut self, publisher: Box<dyn Publisher + Send + Sync>) -> Self {
        self.publisher = publisher;
        self
    }

    /// Inject a live-node directory (tests). Production uses the etcd-backed
    /// [`PeerDirectory`] default.
    #[must_use]
    pub fn with_live_directory(mut self, live_dir: Box<dyn LiveDirectory + Send + Sync>) -> Self {
        self.live_dir = live_dir;
        self
    }

    /// Override the action-drain cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    #[cfg(test)]
    fn with_state_root(mut self, root: PathBuf) -> Self {
        self.state_root = root;
        self
    }

    #[cfg(test)]
    fn with_bus_faults(mut self, faults: Arc<SchedulerBusFaults>) -> Self {
        self.bus_faults = faults;
        self
    }

    fn bus_root(&self) -> PathBuf {
        scheduler_bus_root_or_system(
            self.bus_root_override
                .clone()
                .or_else(mde_bus::default_data_dir),
        )
    }

    fn open_bus(&self) -> io::Result<(PathBuf, Persist, BusIdentity)> {
        let root = self.bus_root();
        let identity_before = match bus_identity(&root) {
            Ok(identity) => identity,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                drop(Persist::open(root.clone()).map_err(io_other)?);
                bus_identity(&root)?
            }
            Err(error) => return Err(error),
        };
        let persist = Persist::open(root.clone()).map_err(io_other)?;
        #[cfg(test)]
        if let Some(replacement) = self
            .bus_faults
            .replace_index_after_open
            .lock()
            .expect("scheduler open replacement mutex")
            .take()
        {
            install_replacement_index(&root, &replacement)?;
        }
        let identity_after = bus_identity(&root)?;
        if identity_before != identity_after {
            return Err(io::Error::other(
                "scheduler Bus index changed while opening transaction",
            ));
        }
        Ok((root, persist, identity_after))
    }

    fn stage_pending(
        &self,
        persist: &Persist,
        outbox: &SchedulerOutbox,
    ) -> io::Result<Vec<StagedRecord>> {
        outbox
            .pending()?
            .into_iter()
            .map(|record| {
                let already_present = record
                    .outputs
                    .iter()
                    .map(|output| topic_contains(persist, &output.topic, &output.body))
                    .collect::<io::Result<Vec<_>>>()?;
                Ok(StagedRecord {
                    record,
                    already_present,
                })
            })
            .collect()
    }

    fn stage_sweep(
        &self,
        transaction: BusTransaction<'_>,
        outbox: &SchedulerOutbox,
    ) -> io::Result<StagedSweep> {
        #[cfg(test)]
        if take_fault(&self.bus_faults.fail_action_reads) {
            return Err(io::Error::other("injected scheduler action read failure"));
        }
        let actions = transaction
            .persist
            .list_since_limit(ACTION_TOPIC, self.cursor.as_deref(), MAX_ACTIONS_PER_SWEEP)
            .map_err(io_other)?;
        #[cfg(test)]
        if take_fault(&self.bus_faults.fail_capacity_reads) {
            return Err(io::Error::other("injected scheduler capacity read failure"));
        }
        let capacity_messages =
            read_topic_complete(transaction.persist, super::kvm_health::SERVICES_TOPIC)?;
        #[cfg(test)]
        if take_fault(&self.bus_faults.fail_desired_reads) {
            return Err(io::Error::other("injected scheduler desired read failure"));
        }
        let desired_messages = read_topic_complete(transaction.persist, DESIRED_TOPIC)?;
        let pending = self.stage_pending(transaction.persist, outbox)?;
        transaction.verify_current()?;
        let capacity_bodies = capacity_messages
            .iter()
            .map(|message| message.body.as_deref().unwrap_or(""));
        let desired_bodies = desired_messages
            .iter()
            .map(|message| message.body.as_deref().unwrap_or(""));
        Ok(StagedSweep {
            pending,
            actions,
            capacity: fold_capacity(capacity_bodies),
            desired: fold_desired(desired_bodies).into_values().collect(),
        })
    }

    fn deliver_record(
        &self,
        transaction: BusTransaction<'_>,
        outbox: &SchedulerOutbox,
        staged: &StagedRecord,
        cleanup: bool,
    ) -> io::Result<()> {
        for (output, present) in staged.record.outputs.iter().zip(&staged.already_present) {
            if !present {
                self.publisher.publish_transaction(
                    transaction.persist,
                    &output.topic,
                    &output.body,
                )?;
                transaction.verify_current()?;
            }
        }
        if cleanup {
            transaction.verify_current()?;
            outbox.remove(&staged.record.record_id)?;
            if let Err(error) = transaction.verify_current() {
                outbox.store(&staged.record)?;
                return Err(error);
            }
        }
        Ok(())
    }

    fn activate(
        &mut self,
        transaction: BusTransaction<'_>,
        outbox: &SchedulerOutbox,
    ) -> io::Result<()> {
        let tail = transaction
            .persist
            .latest_ulid(ACTION_TOPIC)
            .map_err(io_other)?;
        let pending = self.stage_pending(transaction.persist, outbox)?;
        transaction.verify_current()?;
        for staged in &pending {
            self.deliver_record(transaction, outbox, staged, false)?;
        }
        transaction.verify_current()?;
        let mut removed = Vec::new();
        for staged in &pending {
            if let Err(error) = outbox.remove(&staged.record.record_id) {
                for prior in &removed {
                    let _ = outbox.store(prior);
                }
                return Err(error);
            }
            removed.push(staged.record.clone());
        }
        if let Err(error) = transaction.verify_current() {
            for record in &removed {
                let _ = outbox.store(record);
            }
            return Err(error);
        }
        self.cursor = tail;
        self.bus_identity = Some(transaction.identity);
        Ok(())
    }

    fn action_record(
        &self,
        message: &mde_bus::persist::StoredMessage,
        capacity: &BTreeMap<NodeId, KvmHealth>,
    ) -> io::Result<SchedulerOutboxRecord> {
        let parsed = parse_request(message.body.as_deref().unwrap_or(""));
        let (mut outputs, reply) = match parsed {
            Err(error) => (
                Vec::new(),
                SchedulerReply {
                    schema_version: 1,
                    accepted: false,
                    request_id: None,
                    decision: None,
                    error: Some(error),
                },
            ),
            Ok(request) if !is_leader(&self.host, capacity) => (
                Vec::new(),
                SchedulerReply {
                    schema_version: 1,
                    accepted: false,
                    request_id: request.request_id,
                    decision: None,
                    error: Some("this node is not the elected scheduler".into()),
                },
            ),
            Ok(request) => match plan_placement(capacity, &request, now_ms()) {
                Some(placement) => {
                    let desired_body =
                        serde_json::to_string(&placement.desired).map_err(io_other)?;
                    let decision_body =
                        serde_json::to_string(&placement.decision).map_err(io_other)?;
                    let reply = SchedulerReply {
                        schema_version: 1,
                        accepted: true,
                        request_id: request.request_id,
                        decision: Some(placement.decision),
                        error: None,
                    };
                    (
                        vec![
                            PendingOutput {
                                topic: DESIRED_TOPIC.into(),
                                body: desired_body,
                            },
                            PendingOutput {
                                topic: PLACEMENTS_TOPIC.into(),
                                body: decision_body,
                            },
                        ],
                        reply,
                    )
                }
                None => (
                    Vec::new(),
                    SchedulerReply {
                        schema_version: 1,
                        accepted: false,
                        request_id: request.request_id,
                        decision: None,
                        error: Some("no healthy placement candidate".into()),
                    },
                ),
            },
        };
        outputs.push(PendingOutput {
            topic: reply_topic(&message.ulid),
            body: serde_json::to_string(&reply).map_err(io_other)?,
        });
        Ok(SchedulerOutboxRecord {
            schema_version: 1,
            record_id: message.ulid.clone(),
            action_ulid: Some(message.ulid.clone()),
            outputs,
        })
    }

    fn failover_record(placement: Placement) -> io::Result<SchedulerOutboxRecord> {
        let desired_body = serde_json::to_string(&placement.desired).map_err(io_other)?;
        let decision_body = serde_json::to_string(&placement.decision).map_err(io_other)?;
        let mut digest = Sha256::new();
        digest.update(desired_body.as_bytes());
        digest.update(decision_body.as_bytes());
        let record_id = format!("failover-{:x}", digest.finalize());
        Ok(SchedulerOutboxRecord {
            schema_version: 1,
            record_id,
            action_ulid: None,
            outputs: vec![
                PendingOutput {
                    topic: DESIRED_TOPIC.into(),
                    body: desired_body,
                },
                PendingOutput {
                    topic: PLACEMENTS_TOPIC.into(),
                    body: decision_body,
                },
            ],
        })
    }

    fn tick_transaction(
        &mut self,
        transaction: BusTransaction<'_>,
        outbox: &SchedulerOutbox,
    ) -> io::Result<()> {
        let staged = self.stage_sweep(transaction, outbox)?;
        if !staged.pending.is_empty() {
            for pending in &staged.pending {
                self.deliver_record(transaction, outbox, pending, true)?;
                if let Some(action_ulid) = &pending.record.action_ulid {
                    self.cursor = Some(action_ulid.clone());
                }
            }
            return Ok(());
        }

        if !staged.actions.is_empty() {
            for message in staged.actions {
                let record = self.action_record(&message, &staged.capacity)?;
                outbox.store(&record)?;
                let staged_record = StagedRecord {
                    already_present: vec![false; record.outputs.len()],
                    record,
                };
                self.deliver_record(transaction, outbox, &staged_record, true)?;
                self.cursor = Some(message.ulid);
            }
            return Ok(());
        }

        if staged.desired.is_empty() {
            return Ok(());
        }
        let live = live_node_ids(&self.live_dir.live_hostnames(), &staged.capacity);
        if !is_failover_leader(&self.host, &live) {
            return Ok(());
        }
        let persisted = staged
            .desired
            .iter()
            .map(|desired| desired.to_placement(0, 0))
            .collect::<Vec<_>>();
        for mut placement in replace_decisions(&persisted, &live, &staged.capacity) {
            placement.decision.published_at_ms = now_ms();
            let record = Self::failover_record(placement)?;
            if outbox.load(&record.record_id)?.is_none() {
                outbox.store(&record)?;
            }
            let staged_record = StagedRecord {
                already_present: vec![false; record.outputs.len()],
                record,
            };
            self.deliver_record(transaction, outbox, &staged_record, true)?;
        }
        Ok(())
    }

    #[cfg(test)]
    async fn drain_and_place(&self, bus_root: &Path, cursor: &mut Option<String>) {
        let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
            return;
        };
        let Ok(actions) = persist.list_since(ACTION_TOPIC, cursor.as_deref()) else {
            return;
        };
        let Ok(capacity_messages) =
            read_topic_complete(&persist, super::kvm_health::SERVICES_TOPIC)
        else {
            return;
        };
        let capacity = fold_capacity(
            capacity_messages
                .iter()
                .map(|message| message.body.as_deref().unwrap_or("")),
        );
        for message in actions {
            *cursor = Some(message.ulid.clone());
            let Ok(request) = parse_request(message.body.as_deref().unwrap_or("")) else {
                continue;
            };
            if !is_leader(&self.host, &capacity) {
                continue;
            }
            let Some(placement) = plan_placement(&capacity, &request, now_ms()) else {
                continue;
            };
            if let Ok(body) = serde_json::to_string(&placement.decision) {
                self.publisher.publish(PLACEMENTS_TOPIC, &body);
            }
            if let Ok(body) = serde_json::to_string(&placement.desired) {
                self.publisher.publish(DESIRED_TOPIC, &body);
            }
        }
    }

    #[cfg(test)]
    async fn failover_once(&self, bus_root: &Path) {
        let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
            return;
        };
        let Ok(desired_messages) = read_topic_complete(&persist, DESIRED_TOPIC) else {
            return;
        };
        let desired = fold_desired(
            desired_messages
                .iter()
                .map(|message| message.body.as_deref().unwrap_or("")),
        )
        .into_values()
        .collect::<Vec<_>>();
        let Ok(capacity_messages) =
            read_topic_complete(&persist, super::kvm_health::SERVICES_TOPIC)
        else {
            return;
        };
        let capacity = fold_capacity(
            capacity_messages
                .iter()
                .map(|message| message.body.as_deref().unwrap_or("")),
        );
        let live = live_node_ids(&self.live_dir.live_hostnames(), &capacity);
        if !is_failover_leader(&self.host, &live) {
            return;
        }
        let persisted = desired
            .iter()
            .map(|desired| desired.to_placement(0, 0))
            .collect::<Vec<_>>();
        for mut placement in replace_decisions(&persisted, &live, &capacity) {
            placement.decision.published_at_ms = now_ms();
            if let Ok(body) = serde_json::to_string(&placement.decision) {
                self.publisher.publish(PLACEMENTS_TOPIC, &body);
            }
            if let Ok(body) = serde_json::to_string(&placement.desired) {
                self.publisher.publish(DESIRED_TOPIC, &body);
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for SchedulerWorker {
    fn name(&self) -> &'static str {
        "scheduler"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let outbox = SchedulerOutbox::open(&self.state_root)?;
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    match self.open_bus() {
                        Ok((root, persist, identity)) => {
                            let transaction = BusTransaction {
                                persist: &persist,
                                root: &root,
                                identity,
                            };
                            let result = if self.bus_identity != Some(identity) {
                                self.activate(transaction, &outbox)
                            } else {
                                self.tick_transaction(transaction, &outbox)
                            };
                            if let Err(error) = result {
                                tracing::warn!(%error, "scheduler Bus transaction deferred");
                            }
                        }
                        Err(error) => tracing::warn!(%error, "scheduler Bus unavailable; retrying"),
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
    use std::sync::Mutex;

    /// A `KvmHealth` shaped just enough for the placement decision (only
    /// `host` / `active` / `all_healthy` are read by [`choose_node`]).
    fn health(host: &str, active: usize, all_healthy: bool) -> KvmHealth {
        KvmHealth {
            host: host.to_string(),
            services: vec![],
            active,
            total: active,
            all_healthy,
            published_at_ms: 0,
        }
    }

    fn candidates(pairs: &[(&str, usize, bool)]) -> Vec<(NodeId, KvmHealth)> {
        pairs
            .iter()
            .map(|(id, active, ok)| ((*id).to_string(), health(id, *active, *ok)))
            .collect()
    }

    fn req(kind: PlaceKind, pin: Option<&str>) -> PlaceRequest {
        PlaceRequest {
            kind,
            spec: serde_json::json!({"name": "w1"}),
            host: pin.map(str::to_string),
            request_id: None,
        }
    }

    // ── choose_node (the required pure tests) ──

    #[test]
    fn choose_node_honors_a_healthy_pin() {
        // node-b has more active services, but the healthy pin wins.
        let c = candidates(&[("node-a", 1, true), ("node-b", 9, true)]);
        let chosen = choose_node(&c, &req(PlaceKind::Vm, Some("node-a")));
        assert_eq!(chosen.as_deref(), Some("node-a"));
    }

    #[test]
    fn choose_node_picks_the_healthiest() {
        // No pin ⇒ the most active node.
        let c = candidates(&[
            ("node-a", 2, true),
            ("node-b", 5, true),
            ("node-c", 3, true),
        ]);
        let chosen = choose_node(&c, &req(PlaceKind::Container, None));
        assert_eq!(chosen.as_deref(), Some("node-b"));
    }

    #[test]
    fn choose_node_tie_breaks_by_node_id_ascending() {
        // Equal active ⇒ smallest node_id, regardless of input order.
        let fwd = candidates(&[("node-a", 4, true), ("node-b", 4, true)]);
        let rev = candidates(&[("node-b", 4, true), ("node-a", 4, true)]);
        assert_eq!(
            choose_node(&fwd, &req(PlaceKind::Vm, None)).as_deref(),
            Some("node-a")
        );
        assert_eq!(
            choose_node(&rev, &req(PlaceKind::Vm, None)).as_deref(),
            Some("node-a")
        );
    }

    #[test]
    fn choose_node_none_with_no_candidates() {
        assert_eq!(choose_node(&[], &req(PlaceKind::Vm, Some("node-a"))), None);
        assert_eq!(choose_node(&[], &req(PlaceKind::Container, None)), None);
    }

    #[test]
    fn choose_node_ignores_an_unhealthy_pin_and_falls_back_to_healthiest() {
        // Pinned node-a is unhealthy (and not the most active) ⇒ fall back to
        // the healthiest, node-b.
        let c = candidates(&[("node-a", 1, false), ("node-b", 3, true)]);
        let chosen = choose_node(&c, &req(PlaceKind::Vm, Some("node-a")));
        assert_eq!(chosen.as_deref(), Some("node-b"));
    }

    #[test]
    fn choose_node_ignores_an_absent_pin() {
        // A pin for a node not in the capacity map falls back to the healthiest.
        let c = candidates(&[("node-a", 2, true), ("node-b", 5, true)]);
        let chosen = choose_node(&c, &req(PlaceKind::Vm, Some("ghost")));
        assert_eq!(chosen.as_deref(), Some("node-b"));
    }

    // ── fold_capacity (newest authoritative publication by host) ──

    #[test]
    fn fold_capacity_is_latest_wins_by_host() {
        let mut older_health = health("node-a", 1, false);
        older_health.published_at_ms = 1;
        let older = serde_json::to_string(&older_health).unwrap();
        let mut newer_health = health("node-a", 6, true);
        newer_health.published_at_ms = 2;
        let newer = serde_json::to_string(&newer_health).unwrap();
        let other = serde_json::to_string(&health("node-b", 3, true)).unwrap();
        let map = fold_capacity([older.as_str(), other.as_str(), newer.as_str(), "garbage"]);
        assert_eq!(map.len(), 2);
        // node-a's newer publication wins.
        assert_eq!(map["node-a"].active, 6);
        assert!(map["node-a"].all_healthy);
        assert_eq!(map["node-b"].active, 3);
    }

    #[test]
    fn replayed_or_equivocated_health_history_cannot_authorize_capacity_after_restart() {
        let body = |active, healthy, published_at_ms| {
            let mut summary = health("node-a", active, healthy);
            summary.published_at_ms = published_at_ms;
            serde_json::to_string(&summary).expect("encode health fixture")
        };
        let current = body(5, true, 20);
        let replay = body(1, false, 10);
        let conflict = body(4, true, 20);
        let replayed_current = body(5, true, 20);
        let recovered = body(6, true, 21);

        let quarantined = fold_capacity([
            current.as_str(),
            replay.as_str(),
            conflict.as_str(),
            replayed_current.as_str(),
        ]);
        assert!(
            !quarantined.contains_key("node-a"),
            "equal-time equivocation must fail closed and replay cannot restore capacity"
        );

        let corrected = fold_capacity([
            current.as_str(),
            replay.as_str(),
            conflict.as_str(),
            replayed_current.as_str(),
            recovered.as_str(),
        ]);
        assert_eq!(corrected["node-a"].published_at_ms, 21);
        assert_eq!(corrected["node-a"].active, 6);
    }

    // ── plan_placement (the non-executing proposal) ──

    #[test]
    fn plan_placement_builds_a_vm_proposal() {
        let cap = fold_capacity([
            serde_json::to_string(&health("node-a", 2, true))
                .unwrap()
                .as_str(),
            serde_json::to_string(&health("node-b", 5, true))
                .unwrap()
                .as_str(),
        ]);
        let r = PlaceRequest {
            kind: PlaceKind::Vm,
            spec: serde_json::json!({"name": "web1", "vcpus": 2}),
            host: None,
            request_id: Some("req-42".into()),
        };
        let p = plan_placement(&cap, &r, 1234).expect("a placement");
        assert_eq!(p.chosen_host, "node-b"); // healthiest
        assert_eq!(p.desired.kind, PlaceKind::Vm);
        assert_eq!(p.desired.chosen_host, "node-b");
        assert_eq!(p.desired.spec["name"], "web1");
        assert_eq!(p.desired.spec["vcpus"], 2);
        assert_eq!(p.decision.request_id.as_deref(), Some("req-42"));
        assert_eq!(p.decision.candidates, 2);
        assert_eq!(p.decision.published_at_ms, 1234);
    }

    #[test]
    fn plan_placement_builds_a_container_proposal() {
        let cap = fold_capacity([serde_json::to_string(&health("n1", 3, true))
            .unwrap()
            .as_str()]);
        let r = req(PlaceKind::Container, None);
        let p = plan_placement(&cap, &r, 0).expect("a placement");
        assert_eq!(p.desired.kind, PlaceKind::Container);
        assert_eq!(p.desired.chosen_host, "n1");
        assert_eq!(p.desired.spec["name"], "w1");
    }

    #[test]
    fn plan_placement_none_without_capacity() {
        let cap: BTreeMap<NodeId, KvmHealth> = BTreeMap::new();
        assert!(plan_placement(&cap, &req(PlaceKind::Vm, None), 0).is_none());
    }

    // ── is_leader (interim single-actor election) ──

    #[test]
    fn is_leader_is_the_lowest_node_id() {
        let cap = fold_capacity([
            serde_json::to_string(&health("node-b", 1, true))
                .unwrap()
                .as_str(),
            serde_json::to_string(&health("node-a", 1, true))
                .unwrap()
                .as_str(),
        ]);
        assert!(is_leader("node-a", &cap));
        assert!(!is_leader("node-b", &cap));
        // No capacity ⇒ nobody is the leader.
        assert!(!is_leader("node-a", &BTreeMap::new()));
    }

    // ── request parsing + topics ──

    #[test]
    fn parse_request_round_trips_and_defaults_optional_fields() {
        let r = parse_request(
            r#"{"kind":"vm","spec":{"name":"d","vcpus":1},"host":"node-a","request_id":"r1"}"#,
        )
        .expect("parse");
        assert_eq!(r.kind, PlaceKind::Vm);
        assert_eq!(r.host.as_deref(), Some("node-a"));
        assert_eq!(r.request_id.as_deref(), Some("r1"));
        // host + request_id default to None; kind snake-cases to "container".
        let bare = parse_request(r#"{"kind":"container","spec":{}}"#).expect("parse");
        assert_eq!(bare.kind, PlaceKind::Container);
        assert!(bare.host.is_none());
        assert!(bare.request_id.is_none());
        assert!(parse_request("nope").is_err());
        assert!(parse_request(r#"{"kind":"teleport","spec":{}}"#).is_err());
    }

    #[test]
    fn topics_are_namespaced() {
        assert_eq!(ACTION_TOPIC, "action/schedule/place");
        assert!(ACTION_TOPIC.starts_with("action/"));
        assert_eq!(PLACEMENTS_TOPIC, "event/schedule/placements");
        assert!(PLACEMENTS_TOPIC.starts_with("event/"));
        assert_eq!(DESIRED_TOPIC, "event/schedule/desired");
        assert!(DESIRED_TOPIC.starts_with("event/"));
    }

    #[test]
    fn worker_name_matches_module() {
        let w = SchedulerWorker::new("node".to_string());
        assert_eq!(w.name(), "scheduler");
    }

    // ─────────────────────── MV-5b: failover ───────────────────────

    /// A desired-state record (`{kind, spec, chosen_host, request_id}`).
    fn dp(kind: PlaceKind, host: &str, name: &str, rid: Option<&str>) -> DesiredPlacement {
        DesiredPlacement {
            kind,
            spec: serde_json::json!({ "name": name }),
            chosen_host: host.to_string(),
            request_id: rid.map(str::to_string),
        }
    }

    /// A node-id-keyed capacity map (what `read_capacity` folds to).
    fn cap(pairs: &[(&str, usize, bool)]) -> BTreeMap<NodeId, KvmHealth> {
        pairs
            .iter()
            .map(|(id, active, ok)| ((*id).to_string(), health(id, *active, *ok)))
            .collect()
    }

    /// A live-node set in the capacity/`chosen_host` namespace.
    fn live_set(ids: &[&str]) -> BTreeSet<NodeId> {
        ids.iter().map(|s| (*s).to_string()).collect()
    }

    // ── replace_decisions (the required pure tests) ──

    #[test]
    fn replace_decisions_reassigns_a_lost_node_to_healthiest_live() {
        // w1 was on peer:b; peer:b's lease lapsed (absent from live) though its
        // stale capacity lingers. Re-placed onto the healthiest LIVE node, peer:c.
        let persisted = vec![dp(PlaceKind::Vm, "peer:b", "w1", Some("r1")).to_placement(0, 100)];
        let capacity = cap(&[
            ("peer:a", 2, true),
            ("peer:b", 9, true),
            ("peer:c", 5, true),
        ]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a", "peer:c"]), &capacity);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chosen_host, "peer:c");
    }

    #[test]
    fn replace_decisions_leaves_a_live_node_untouched() {
        // peer:a is still live ⇒ the workload keeps running, no re-placement.
        let persisted = vec![dp(PlaceKind::Vm, "peer:a", "w1", None).to_placement(0, 100)];
        let capacity = cap(&[("peer:a", 2, true), ("peer:c", 5, true)]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a", "peer:c"]), &capacity);
        assert!(out.is_empty());
    }

    #[test]
    fn replace_decisions_skips_when_no_live_candidate() {
        let persisted = vec![dp(PlaceKind::Vm, "peer:b", "w1", None).to_placement(0, 100)];
        let capacity = cap(&[("peer:b", 1, true)]);
        // (a) nothing live at all.
        assert!(replace_decisions(&persisted, &BTreeSet::new(), &capacity).is_empty());
        // (b) a node is live but has no capacity to place onto ⇒ still skipped.
        let out = replace_decisions(&persisted, &live_set(&["peer:x"]), &capacity);
        assert!(out.is_empty());
    }

    #[test]
    fn replace_decisions_never_targets_the_dead_node() {
        // peer:b is the MOST active in capacity but is dead — must not be re-picked;
        // the only live candidate, peer:a, wins.
        let persisted = vec![dp(PlaceKind::Vm, "peer:b", "w1", None).to_placement(0, 100)];
        let capacity = cap(&[("peer:a", 1, true), ("peer:b", 99, true)]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a"]), &capacity);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chosen_host, "peer:a");
    }

    #[test]
    fn replace_decisions_retargets_the_proposal_to_the_new_node() {
        // A container remains a container, its opaque spec + request_id are
        // preserved, and the proposal names the NEW node.
        let persisted =
            vec![dp(PlaceKind::Container, "peer:b", "svc1", Some("r7")).to_placement(0, 100)];
        let capacity = cap(&[("peer:a", 3, true)]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a"]), &capacity);
        assert_eq!(out.len(), 1);
        let p = &out[0];
        assert_eq!(p.desired.kind, PlaceKind::Container);
        assert_eq!(p.desired.chosen_host, "peer:a");
        assert_eq!(p.desired.spec["name"], "svc1");
        assert_eq!(p.chosen_host, "peer:a");
        assert_eq!(p.decision.chosen_host, "peer:a");
        assert_eq!(p.decision.kind, PlaceKind::Container);
        assert_eq!(p.decision.request_id.as_deref(), Some("r7"));
    }

    #[test]
    fn replace_decisions_is_deterministic_and_tie_breaks_by_node_id() {
        // Equal-active live candidates ⇒ smallest node_id, and repeat runs are
        // byte-identical.
        let persisted = vec![dp(PlaceKind::Vm, "peer:dead", "w1", None).to_placement(0, 100)];
        let capacity = cap(&[("peer:a", 4, true), ("peer:b", 4, true)]);
        let live = live_set(&["peer:a", "peer:b"]);
        let out1 = replace_decisions(&persisted, &live, &capacity);
        let out2 = replace_decisions(&persisted, &live, &capacity);
        assert_eq!(out1, out2);
        assert_eq!(out1[0].chosen_host, "peer:a");
    }

    #[test]
    fn replace_decisions_handles_a_mix_of_live_and_lost() {
        // One workload on a live node (kept), one on a lost node (re-placed).
        let persisted = vec![
            dp(PlaceKind::Vm, "peer:a", "keep", None).to_placement(0, 100),
            dp(PlaceKind::Vm, "peer:dead", "move", Some("r2")).to_placement(0, 100),
        ];
        let capacity = cap(&[("peer:a", 2, true), ("peer:c", 5, true)]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a", "peer:c"]), &capacity);
        assert_eq!(out.len(), 1, "only the lost workload is re-placed");
        assert_eq!(out[0].decision.request_id.as_deref(), Some("r2"));
        assert_eq!(out[0].chosen_host, "peer:c");
    }

    // ── live_node_ids (the peer: prefix reconciliation) ──

    #[test]
    fn live_node_ids_reconciles_the_peer_prefix() {
        // The directory stores BARE hostnames (`a`); capacity is node-id-keyed
        // (`peer:a`). peer:a is live; peer:b (absent from the directory) is gone.
        let capacity = cap(&[("peer:a", 1, true), ("peer:b", 1, true)]);
        let directory: BTreeSet<NodeId> = live_set(&["a"]);
        assert_eq!(live_node_ids(&directory, &capacity), live_set(&["peer:a"]));
    }

    #[test]
    fn live_node_ids_tolerates_a_prefixed_directory_and_empty() {
        let capacity = cap(&[("peer:a", 1, true)]);
        // Already-prefixed directory rows still reconcile.
        assert_eq!(
            live_node_ids(&live_set(&["peer:a"]), &capacity),
            live_set(&["peer:a"])
        );
        // Empty directory ⇒ nothing live.
        assert!(live_node_ids(&BTreeSet::new(), &capacity).is_empty());
    }

    #[test]
    fn is_failover_leader_is_the_lowest_live_node() {
        let live = live_set(&["peer:b", "peer:a", "peer:c"]);
        assert!(is_failover_leader("peer:a", &live));
        assert!(!is_failover_leader("peer:b", &live));
        // A lost leader (not in the live set) is not the leader — the next live
        // node takes over.
        let after_loss = live_set(&["peer:b", "peer:c"]);
        assert!(is_failover_leader("peer:b", &after_loss));
        // Empty live set ⇒ nobody leads.
        assert!(!is_failover_leader("peer:a", &BTreeSet::new()));
    }

    // ── desired-state persistence (fold + round-trip) ──

    #[test]
    fn fold_desired_is_latest_wins_by_request_id() {
        let d1 = serde_json::to_string(&dp(PlaceKind::Vm, "peer:a", "w1", Some("r1"))).unwrap();
        let moved = serde_json::to_string(&dp(PlaceKind::Vm, "peer:c", "w1", Some("r1"))).unwrap();
        let other =
            serde_json::to_string(&dp(PlaceKind::Container, "peer:a", "w2", Some("r2"))).unwrap();
        let map = fold_desired([d1.as_str(), other.as_str(), moved.as_str(), "garbage"]);
        assert_eq!(map.len(), 2);
        assert_eq!(map["r1"].chosen_host, "peer:c", "the later r1 record wins");
        assert_eq!(map["r2"].chosen_host, "peer:a");
    }

    #[test]
    fn fold_desired_keys_requestless_records_by_workload_identity() {
        // No request_id ⇒ keyed by (kind, spec), so a re-placement of the SAME
        // workload (new host) overwrites rather than duplicating.
        let a = serde_json::to_string(&dp(PlaceKind::Vm, "peer:a", "w9", None)).unwrap();
        let moved = serde_json::to_string(&dp(PlaceKind::Vm, "peer:c", "w9", None)).unwrap();
        let distinct = serde_json::to_string(&dp(PlaceKind::Vm, "peer:a", "other", None)).unwrap();
        let map = fold_desired([a.as_str(), moved.as_str(), distinct.as_str()]);
        assert_eq!(map.len(), 2);
        let w9 = map.values().find(|d| d.spec["name"] == "w9").expect("w9");
        assert_eq!(w9.chosen_host, "peer:c");
    }

    #[test]
    fn desired_placement_is_retained_in_the_proposal() {
        let d = dp(PlaceKind::Container, "peer:a", "svc", Some("rid"));
        let p = d.to_placement(3, 42);
        // The audit envelope carried the seeds…
        assert_eq!(p.decision.candidates, 3);
        assert_eq!(p.decision.published_at_ms, 42);
        // …and the intent is retained losslessly as data, not an action body.
        assert_eq!(p.desired, d);
    }

    #[test]
    fn plan_placement_matches_the_desired_envelope() {
        // The MV-5a refactor is behavior-preserving: plan_placement's Placement is
        // exactly the desired-state for the chosen node built through to_placement.
        let capacity = cap(&[("peer:a", 2, true), ("peer:b", 5, true)]);
        let r = PlaceRequest {
            kind: PlaceKind::Vm,
            spec: serde_json::json!({ "name": "x" }),
            host: None,
            request_id: Some("r".into()),
        };
        let got = plan_placement(&capacity, &r, 7).expect("a placement");
        let expected = dp(PlaceKind::Vm, "peer:b", "x", Some("r")).to_placement(2, 7);
        assert_eq!(got, expected);
    }

    // ── failover tick wiring (seeded temp bus + injected directory) ──

    /// A [`LiveDirectory`] returning a fixed hostname set — the Fake seam.
    struct FakeDirectory(BTreeSet<NodeId>);
    impl LiveDirectory for FakeDirectory {
        fn live_hostnames(&self) -> BTreeSet<NodeId> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn failover_tick_proposes_and_repersists_on_node_loss() {
        use mde_bus::hooks::config::Priority;
        // Seed a temp bus: capacity for two live nodes + one desired placement on a
        // node whose lease has since lapsed (peer:b).
        let dir = std::env::temp_dir().join(format!("mde-sched-failover-{}", now_ms()));
        {
            let persist = Persist::open(dir.clone()).expect("open bus");
            for h in [health("peer:a", 2, true), health("peer:c", 5, true)] {
                persist
                    .write(
                        super::super::kvm_health::SERVICES_TOPIC,
                        Priority::Default,
                        None,
                        Some(&serde_json::to_string(&h).unwrap()),
                    )
                    .expect("write capacity");
            }
            persist
                .write(
                    DESIRED_TOPIC,
                    Priority::Default,
                    None,
                    Some(
                        &serde_json::to_string(&dp(PlaceKind::Vm, "peer:b", "w1", Some("r1")))
                            .unwrap(),
                    ),
                )
                .expect("write desired");
        }

        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        // Directory reports only a + c (bare hostnames — peer:b is gone). host is
        // peer:a = lowest live ⇒ this node is the failover leader.
        let w = SchedulerWorker::new("peer:a".to_string())
            .with_publisher(Box::new(rec))
            .with_live_directory(Box::new(FakeDirectory(live_set(&["a", "c"]))));
        w.failover_once(&dir).await;

        let sent = log.lock().expect("recorder mutex");
        assert!(
            sent.iter().all(|(topic, _)| !topic.starts_with("action/")),
            "failover must never bypass the privileged actuator gates: {sent:?}"
        );
        // 1. Persisted desired-state was proposed for the new home.
        let updated = sent
            .iter()
            .find(|(t, _)| t == DESIRED_TOPIC)
            .expect("re-persisted desired-state");
        let ud: DesiredPlacement = serde_json::from_str(&updated.1).unwrap();
        assert_eq!(ud.chosen_host, "peer:c");
        assert_eq!(ud.spec["name"], "w1");
        assert_eq!(ud.request_id.as_deref(), Some("r1"));
        // 2. Audit trail emitted; these are the only two outward events.
        assert!(sent.iter().any(|(t, _)| t == PLACEMENTS_TOPIC));
        assert_eq!(sent.len(), 2);
        drop(sent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn failover_tick_is_a_noop_for_a_non_leader() {
        use mde_bus::hooks::config::Priority;
        let dir = std::env::temp_dir().join(format!("mde-sched-nolead-{}", now_ms()));
        {
            let persist = Persist::open(dir.clone()).expect("open bus");
            for h in [health("peer:a", 2, true), health("peer:c", 5, true)] {
                persist
                    .write(
                        super::super::kvm_health::SERVICES_TOPIC,
                        Priority::Default,
                        None,
                        Some(&serde_json::to_string(&h).unwrap()),
                    )
                    .expect("write capacity");
            }
            persist
                .write(
                    DESIRED_TOPIC,
                    Priority::Default,
                    None,
                    Some(
                        &serde_json::to_string(&dp(PlaceKind::Vm, "peer:b", "w1", Some("r1")))
                            .unwrap(),
                    ),
                )
                .expect("write desired");
        }
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        // host peer:c is NOT the lowest live node (peer:a is) ⇒ it must not act.
        let w = SchedulerWorker::new("peer:c".to_string())
            .with_publisher(Box::new(rec))
            .with_live_directory(Box::new(FakeDirectory(live_set(&["a", "c"]))));
        w.failover_once(&dir).await;
        assert!(
            log.lock().expect("recorder mutex").is_empty(),
            "a non-leader re-places nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn placement_requests_publish_only_non_privileged_proposal_events() {
        use mde_bus::hooks::config::Priority;
        // VM + container requests and one healthy node (leader + only candidate).
        let dir = std::env::temp_dir().join(format!("mde-sched-persist-{}", now_ms()));
        {
            let persist = Persist::open(dir.clone()).expect("open bus");
            persist
                .write(
                    super::super::kvm_health::SERVICES_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&health("peer:a", 3, true)).unwrap()),
                )
                .expect("write capacity");
            for (kind, name, request_id) in [
                (PlaceKind::Vm, "web1", "r1"),
                (PlaceKind::Container, "api1", "r2"),
            ] {
                let request = PlaceRequest {
                    kind,
                    spec: serde_json::json!({ "name": name }),
                    host: None,
                    request_id: Some(request_id.into()),
                };
                persist
                    .write(
                        ACTION_TOPIC,
                        Priority::Default,
                        None,
                        Some(&serde_json::to_string(&request).unwrap()),
                    )
                    .expect("write request");
            }
        }
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        let w = SchedulerWorker::new("peer:a".to_string()).with_publisher(Box::new(rec));
        let mut cursor = None;
        w.drain_and_place(&dir, &mut cursor).await;

        let sent = log.lock().expect("recorder mutex");
        assert!(
            sent.iter().all(|(topic, _)| topic.starts_with("event/")),
            "an unsigned placement must never publish a privileged action: {sent:?}"
        );
        assert_eq!(
            sent.iter()
                .filter(|(topic, _)| topic == PLACEMENTS_TOPIC)
                .count(),
            2
        );
        let desired: Vec<DesiredPlacement> = sent
            .iter()
            .filter(|(topic, _)| topic == DESIRED_TOPIC)
            .map(|(_, body)| serde_json::from_str(body).expect("desired proposal"))
            .collect();
        assert_eq!(desired.len(), 2);
        assert!(desired
            .iter()
            .any(|proposal| proposal.kind == PlaceKind::Vm && proposal.spec["name"] == "web1"));
        assert!(desired.iter().any(|proposal| {
            proposal.kind == PlaceKind::Container && proposal.spec["name"] == "api1"
        }));
        assert!(desired
            .iter()
            .all(|proposal| proposal.chosen_host == "peer:a"));
        assert_eq!(sent.len(), 4, "two audit + two desired events only");
        drop(sent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── run loop (injected recorder, no bus binary) ──

    /// A [`Publisher`] that records every publish for assertions — the Fake
    /// seam. The log is an `Arc` so a test can clone a handle to it before
    /// moving the worker into its task.
    #[derive(Clone, Default)]
    struct RecordingPublisher {
        sent: std::sync::Arc<Mutex<Vec<(String, String)>>>,
    }

    impl Publisher for RecordingPublisher {
        fn publish(&self, topic: &str, body: &str) {
            self.sent
                .lock()
                .expect("recorder mutex")
                .push((topic.to_string(), body.to_string()));
        }
    }

    #[derive(Default)]
    struct FaultPublisherState {
        attempts: usize,
        fail_on: BTreeSet<usize>,
    }

    #[derive(Clone, Default)]
    struct FaultPublisher {
        state: Arc<Mutex<FaultPublisherState>>,
    }

    impl FaultPublisher {
        fn fail_on(attempts: impl IntoIterator<Item = usize>) -> Self {
            Self {
                state: Arc::new(Mutex::new(FaultPublisherState {
                    attempts: 0,
                    fail_on: attempts.into_iter().collect(),
                })),
            }
        }

        fn fail_next(&self) {
            let mut state = self.state.lock().expect("fault publisher mutex");
            let next = state.attempts + 1;
            state.fail_on.insert(next);
        }
    }

    impl Publisher for FaultPublisher {
        fn publish(&self, _topic: &str, _body: &str) {}

        fn publish_transaction(
            &self,
            persist: &Persist,
            topic: &str,
            body: &str,
        ) -> io::Result<()> {
            let mut state = self.state.lock().expect("fault publisher mutex");
            state.attempts += 1;
            let attempt = state.attempts;
            if state.fail_on.remove(&attempt) {
                return Err(io::Error::other("injected scheduler publication failure"));
            }
            drop(state);
            persist
                .write(topic, Priority::Default, None, Some(body))
                .map(|_| ())
                .map_err(io_other)
        }
    }

    fn seed_capacity(persist: &Persist, host: &str) {
        persist
            .write(
                super::super::kvm_health::SERVICES_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&health(host, 3, true)).expect("capacity wire")),
            )
            .expect("capacity row");
    }

    fn placement_request(request_id: &str, name: &str) -> PlaceRequest {
        PlaceRequest {
            kind: PlaceKind::Vm,
            spec: serde_json::json!({"name": name}),
            host: None,
            request_id: Some(request_id.into()),
        }
    }

    fn transaction<'a>(persist: &'a Persist, root: &'a Path) -> BusTransaction<'a> {
        BusTransaction {
            persist,
            root,
            identity: bus_identity(root).expect("Bus identity"),
        }
    }

    async fn wait_for_row(root: &Path, topic: &str) -> mde_bus::persist::StoredMessage {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(persist) = Persist::open(root.to_path_buf()) {
                    if let Ok(Some(message)) = persist.read_latest(topic) {
                        return message;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for scheduler Bus row")
    }

    #[tokio::test]
    async fn worker_recovers_late_and_replaced_bus_and_skips_retained_actions() {
        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        fs::write(&bus_root, b"unopenable").expect("block Bus root");
        let staged_root = temp.path().join("staged");
        let staged = Persist::open(staged_root.clone()).expect("staged Bus");
        seed_capacity(&staged, "peer:a");
        let retained = staged
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(
                    &serde_json::to_string(&placement_request("retained", "old"))
                        .expect("retained wire"),
                ),
            )
            .expect("retained action");
        drop(staged);

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let mut worker = SchedulerWorker::new("peer:a".into())
            .with_bus_root(bus_root.clone())
            .with_state_root(temp.path().join("state"))
            .with_poll(Duration::from_millis(10));
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!task.is_finished(), "late Bus terminated scheduler");

        fs::remove_file(&bus_root).expect("remove Bus blocker");
        fs::rename(&staged_root, &bus_root).expect("install late Bus");
        tokio::time::sleep(Duration::from_millis(80)).await;
        let bus = Persist::open(bus_root.clone()).expect("late Bus");
        assert!(bus
            .list_since(&reply_topic(&retained.ulid), None)
            .expect("retained reply query")
            .is_empty());
        let forward = bus
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(
                    &serde_json::to_string(&placement_request("forward-1", "first"))
                        .expect("forward wire"),
                ),
            )
            .expect("forward action");
        wait_for_row(&bus_root, &reply_topic(&forward.ulid)).await;
        assert_eq!(
            bus.list_since(PLACEMENTS_TOPIC, None)
                .expect("placement rows")
                .len(),
            1
        );
        drop(bus);

        let replacement_root = temp.path().join("replacement");
        let replacement = Persist::open(replacement_root.clone()).expect("replacement Bus");
        seed_capacity(&replacement, "peer:a");
        let retained_replacement = replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(
                    &serde_json::to_string(&placement_request("retained-2", "old-2"))
                        .expect("replacement retained wire"),
                ),
            )
            .expect("replacement retained action");
        drop(replacement);
        install_replacement_index(&bus_root, &replacement_root.join("index.sqlite"))
            .expect("replace Bus index");
        tokio::time::sleep(Duration::from_millis(80)).await;
        let current = Persist::open(bus_root.clone()).expect("current Bus");
        assert!(current
            .list_since(&reply_topic(&retained_replacement.ulid), None)
            .expect("replacement retained reply query")
            .is_empty());
        let forward_replacement = current
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(
                    &serde_json::to_string(&placement_request("forward-2", "second"))
                        .expect("replacement forward wire"),
                ),
            )
            .expect("replacement forward action");
        wait_for_row(&bus_root, &reply_topic(&forward_replacement.ulid)).await;
        assert_eq!(
            current
                .list_since(PLACEMENTS_TOPIC, None)
                .expect("replacement placement rows")
                .len(),
            1
        );

        shutdown_tx.send(true).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("scheduler shutdown timeout")
            .expect("scheduler task joins")
            .expect("scheduler shutdown succeeds");
        assert_eq!(
            scheduler_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
    }

    #[test]
    fn complete_reads_and_durable_reply_recovery_do_not_repeat_outputs() {
        use std::sync::atomic::Ordering;

        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("Bus");
        seed_capacity(&persist, "peer:a");
        let state_root = temp.path().join("state");
        let faults = Arc::new(SchedulerBusFaults::default());
        let publisher = FaultPublisher::fail_on([3]);
        let mut worker = SchedulerWorker::new("peer:a".into())
            .with_bus_root(bus_root.clone())
            .with_state_root(state_root.clone())
            .with_publisher(Box::new(publisher.clone()))
            .with_bus_faults(Arc::clone(&faults));
        let outbox = SchedulerOutbox::open(&state_root).expect("outbox");
        worker
            .activate(transaction(&persist, &bus_root), &outbox)
            .expect("activation");
        let action = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(
                    &serde_json::to_string(&placement_request("r1", "workload"))
                        .expect("action wire"),
                ),
            )
            .expect("action");

        faults.fail_desired_reads.store(1, Ordering::SeqCst);
        assert!(worker
            .tick_transaction(transaction(&persist, &bus_root), &outbox)
            .is_err());
        assert!(worker.cursor.is_none());
        assert!(persist
            .list_since(DESIRED_TOPIC, None)
            .expect("desired after read failure")
            .is_empty());

        assert!(worker
            .tick_transaction(transaction(&persist, &bus_root), &outbox)
            .is_err());
        assert!(worker.cursor.is_none());
        assert_eq!(
            persist
                .list_since(DESIRED_TOPIC, None)
                .expect("desired after reply failure")
                .len(),
            1
        );
        assert_eq!(
            persist
                .list_since(PLACEMENTS_TOPIC, None)
                .expect("placement after reply failure")
                .len(),
            1
        );
        assert!(persist
            .list_since(&reply_topic(&action.ulid), None)
            .expect("failed reply")
            .is_empty());

        let mut restarted = SchedulerWorker::new("peer:a".into())
            .with_bus_root(bus_root.clone())
            .with_state_root(state_root)
            .with_publisher(Box::new(publisher));
        restarted
            .activate(transaction(&persist, &bus_root), &outbox)
            .expect("durable corrected-forward activation");
        assert_eq!(restarted.cursor.as_deref(), Some(action.ulid.as_str()));
        assert_eq!(
            persist
                .list_since(DESIRED_TOPIC, None)
                .expect("desired final")
                .len(),
            1
        );
        assert_eq!(
            persist
                .list_since(PLACEMENTS_TOPIC, None)
                .expect("placement final")
                .len(),
            1
        );
        assert_eq!(
            persist
                .list_since(&reply_topic(&action.ulid), None)
                .expect("recovered reply")
                .len(),
            1
        );
    }

    #[test]
    fn failover_publication_recovers_without_duplicate_proposals() {
        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("Bus");
        seed_capacity(&persist, "peer:a");
        persist
            .write(
                DESIRED_TOPIC,
                Priority::Default,
                None,
                Some(
                    &serde_json::to_string(&dp(PlaceKind::Vm, "peer:dead", "move", Some("move-1")))
                        .expect("desired wire"),
                ),
            )
            .expect("lost desired row");
        let state_root = temp.path().join("state");
        let publisher = FaultPublisher::fail_on([2]);
        let mut worker = SchedulerWorker::new("peer:a".into())
            .with_bus_root(bus_root.clone())
            .with_state_root(state_root.clone())
            .with_live_directory(Box::new(FakeDirectory(live_set(&["a"]))))
            .with_publisher(Box::new(publisher.clone()));
        let outbox = SchedulerOutbox::open(&state_root).expect("outbox");
        worker
            .activate(transaction(&persist, &bus_root), &outbox)
            .expect("activation");
        assert!(worker
            .tick_transaction(transaction(&persist, &bus_root), &outbox)
            .is_err());
        assert_eq!(
            persist
                .list_since(DESIRED_TOPIC, None)
                .expect("desired after partial publication")
                .len(),
            2
        );
        assert!(persist
            .list_since(PLACEMENTS_TOPIC, None)
            .expect("failed placement audit")
            .is_empty());

        let mut restarted = SchedulerWorker::new("peer:a".into())
            .with_bus_root(bus_root.clone())
            .with_state_root(state_root)
            .with_live_directory(Box::new(FakeDirectory(live_set(&["a"]))))
            .with_publisher(Box::new(publisher));
        restarted
            .activate(transaction(&persist, &bus_root), &outbox)
            .expect("recover pending failover publication");
        restarted
            .tick_transaction(transaction(&persist, &bus_root), &outbox)
            .expect("settled failover sweep");
        assert_eq!(
            persist
                .list_since(DESIRED_TOPIC, None)
                .expect("desired final")
                .len(),
            2
        );
        assert_eq!(
            persist
                .list_since(PLACEMENTS_TOPIC, None)
                .expect("placement final")
                .len(),
            1
        );
    }

    #[test]
    fn malformed_and_gated_replies_retry_without_cursor_loss() {
        let temp = tempfile::tempdir().expect("temp");
        for (case, host, body) in [
            ("malformed", "peer:a", "not-json".to_string()),
            (
                "gated",
                "peer:z",
                serde_json::to_string(&placement_request("gated-1", "blocked"))
                    .expect("gated wire"),
            ),
        ] {
            let bus_root = temp.path().join(format!("{case}-bus"));
            let state_root = temp.path().join(format!("{case}-state"));
            let persist = Persist::open(bus_root.clone()).expect("Bus");
            seed_capacity(&persist, "peer:a");
            let publisher = FaultPublisher::default();
            let mut worker = SchedulerWorker::new(host.into())
                .with_bus_root(bus_root.clone())
                .with_state_root(state_root.clone())
                .with_publisher(Box::new(publisher.clone()));
            let outbox = SchedulerOutbox::open(&state_root).expect("outbox");
            worker
                .activate(transaction(&persist, &bus_root), &outbox)
                .expect("activation");
            let action = persist
                .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
                .expect("action");
            publisher.fail_next();
            assert!(worker
                .tick_transaction(transaction(&persist, &bus_root), &outbox)
                .is_err());
            assert!(worker.cursor.is_none(), "{case} cursor advanced on failure");
            worker
                .tick_transaction(transaction(&persist, &bus_root), &outbox)
                .expect("corrected-forward reply");
            assert_eq!(worker.cursor.as_deref(), Some(action.ulid.as_str()));
            assert_eq!(
                persist
                    .list_since(&reply_topic(&action.ulid), None)
                    .expect("reply rows")
                    .len(),
                1
            );
            assert!(persist
                .list_since(PLACEMENTS_TOPIC, None)
                .expect("proposal rows")
                .is_empty());
            assert!(persist
                .list_since(DESIRED_TOPIC, None)
                .expect("desired rows")
                .is_empty());
        }
    }

    #[test]
    fn replacement_during_open_is_rejected_before_current_reopen() {
        const MARKER: &str = "test/scheduler/open-generation";

        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let retired = Persist::open(bus_root.clone()).expect("retired Bus");
        retired
            .write(MARKER, Priority::Default, None, Some("retired"))
            .expect("retired marker");
        drop(retired);
        let replacement_root = temp.path().join("replacement");
        let replacement = Persist::open(replacement_root.clone()).expect("replacement Bus");
        replacement
            .write(MARKER, Priority::Default, None, Some("current"))
            .expect("current marker");
        drop(replacement);
        let faults = Arc::new(SchedulerBusFaults::default());
        *faults
            .replace_index_after_open
            .lock()
            .expect("replacement mutex") = Some(replacement_root.join("index.sqlite"));
        let worker = SchedulerWorker::new("peer:a".into())
            .with_bus_root(bus_root.clone())
            .with_bus_faults(faults);

        let error = match worker.open_bus() {
            Err(error) => error,
            Ok(_) => panic!("mixed-generation open must be rejected"),
        };
        assert!(error.to_string().contains("changed while opening"));
        let (root, current, identity) = worker.open_bus().expect("reopen current Bus");
        assert_eq!(root, bus_root);
        assert_eq!(identity, bus_identity(&root).expect("current identity"));
        assert_eq!(
            current
                .read_latest(MARKER)
                .expect("marker read")
                .and_then(|message| message.body)
                .as_deref(),
            Some("current")
        );
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        // An empty temp bus root ⇒ the drain reads nothing and publishes
        // nothing; the injected recorder means no `mde-bus` binary is needed.
        let dir = std::env::temp_dir().join(format!("mde-sched-test-{}", now_ms()));
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = SchedulerWorker::new("node".to_string())
            .with_publisher(Box::new(rec))
            .with_bus_root(dir)
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        // Nothing to place from an empty bus ⇒ nothing published.
        assert!(log.lock().expect("recorder mutex").is_empty());
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
