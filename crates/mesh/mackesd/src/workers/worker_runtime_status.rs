//! WL-ARCH-009 — bounded worker-runtime status publication.
//!
//! This module is the daemon-side seam between the registry's admitted
//! [`mackes_mesh_types::worker_runtime::WorkerContract`] and a node's explicit
//! runtime observation. Callers supply the state, generation, timestamps, and
//! history, plus an already-open Bus persistence handle when publishing. No
//! default state, process probe, credential, log body, or Bus root is created
//! here.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mackes_mesh_types::worker_runtime as runtime;
use mde_bus::persist::Persist;
use serde::{de, Deserialize, Deserializer, Serialize};

/// The node-scoped status topic required by the Workers release contract.
pub const NODE_STATUS_TOPIC_PREFIX: &str = "state/mackesd";

/// The maximum bytes accepted by this adapter's JSON helpers.
pub const MAX_STATUS_WIRE_BYTES: usize = runtime::MAX_WORKER_RUNTIME_WIRE_BYTES;

/// Maximum number of supervisor-owned workers retained in one node snapshot.
pub const MAX_NODE_STATUS_WORKERS: usize = 256;

/// Maximum encoded size of the aggregate node status file/Bus record.
pub const MAX_NODE_STATUS_WIRE_BYTES: usize = 4 * 1024 * 1024;

const RUNTIME_FRESHNESS_MS: u64 = 15_000;
static STATUS_FILE_NONCE: AtomicU64 = AtomicU64::new(1);

/// The normal supervisor status cadence.  The cadence is retained for
/// freshness/contract compatibility, but each node is assigned a stable phase
/// within the cadence by [`status_phase_delay`].
pub const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Maximum time between unchanged retained publications.  Lifecycle changes
/// bypass this interval; it only bounds the age of an unchanged Bus/file
/// projection.
pub const STATUS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// Initial delay for a rejected supervisor status sample.
pub const STATUS_FAILURE_RETRY_INITIAL: Duration = STATUS_POLL_INTERVAL;
/// Maximum delay for a repeated rejected supervisor status sample.
pub const STATUS_FAILURE_RETRY_MAX: Duration = Duration::from_secs(60);

/// Return a stable, node-specific delay until the next five-second status
/// phase.  This uses the node identity rather than process-local randomness so
/// restarts keep their phase and a fleet does not converge back to one wakeup
/// boundary after every daemon restart.
#[must_use]
pub fn status_phase_delay(node_id: &str, now_ms: u64) -> Duration {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;

    let mut hash = FNV_OFFSET;
    for byte in node_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    let period_ms = STATUS_POLL_INTERVAL.as_millis() as u64;
    let phase_ms = hash % period_ms;
    let period_start = now_ms / period_ms * period_ms;
    let candidate = period_start.saturating_add(phase_ms);
    let next = if candidate > now_ms {
        candidate
    } else {
        candidate.saturating_add(period_ms)
    };
    Duration::from_millis(next.saturating_sub(now_ms))
}

/// Add a small stable offset to an error retry.  The exponential retry ladder
/// remains unchanged, while simultaneous failure paths avoid recreating one
/// exact retry boundary across every seat.
#[must_use]
pub fn status_retry_jitter(node_id: &str) -> Duration {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;
    let mut hash = FNV_OFFSET;
    for byte in node_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    Duration::from_millis(hash % 501)
}

/// Return the next bounded retry delay for a rejected supervisor sample.
#[must_use]
pub fn next_status_failure_retry(current: Duration) -> Duration {
    current.saturating_mul(2).min(STATUS_FAILURE_RETRY_MAX)
}

/// One canonical retained lane written by a status publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerRuntimeStatusLane {
    /// The worker group's canonical per-worker status topic.
    Worker,
    /// The canonical node-scoped status topic.
    Node,
}

/// Receipt for one admitted status written to both canonical retained lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRuntimeStatusPublication {
    /// The exact canonical status retained directly and inside the node aggregate.
    pub status: WorkerRuntimeStatus,
    /// Canonical per-worker topic written by this publication.
    pub worker_topic: String,
    /// Bus message identity returned for the per-worker write.
    pub worker_message_id: String,
    /// Canonical node topic written by this publication.
    pub node_topic: String,
    /// Bus message identity returned for the node write.
    pub node_message_id: String,
}

/// Receipt for one aggregate publication: one retained row per worker and one
/// canonical node snapshot. The node message is written last, so its presence
/// proves every worker row in that aggregate was retained first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRuntimeNodePublication {
    /// `(worker_id, Bus message id)` for every committed worker row.
    pub worker_message_ids: Vec<(String, String)>,
    /// Canonical aggregate node topic committed after all worker rows.
    pub node_topic: String,
    /// Bus message identity returned for the aggregate node write.
    pub node_message_id: String,
}

/// A failure while admitting or encoding a daemon worker-runtime publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerRuntimeStatusError {
    /// The worker contract was not admitted.
    Contract(runtime::WorkerRuntimeContractError),
    /// The runtime snapshot was not admitted.
    Snapshot(runtime::WorkerRuntimeContractError),
    /// A staged change-set payload was not admitted.
    ChangeSet(runtime::WorkerRuntimeContractError),
    /// The contract and explicit runtime observation identify different data.
    ContractSnapshotMismatch(&'static str),
    /// A topic segment is not a bounded identifier.
    InvalidTopicSegment,
    /// A derived topic exceeds the shared topic bound.
    TopicTooLong,
    /// A validated body could not be retained on one canonical Bus lane.
    Publication(WorkerRuntimeStatusLane),
    /// A supervisor row has no canonical registry contract.
    UnregisteredWorker,
    /// An aggregate node snapshot is malformed, conflicting, or over capacity.
    NodeSnapshot(&'static str),
    /// The credential-free runtime file could not be written safely.
    RuntimeFile(&'static str),
}

impl fmt::Display for WorkerRuntimeStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "worker contract rejected: {error}"),
            Self::Snapshot(error) => write!(formatter, "worker snapshot rejected: {error}"),
            Self::ChangeSet(error) => write!(formatter, "worker change set rejected: {error}"),
            Self::ContractSnapshotMismatch(field) => {
                write!(formatter, "worker contract/snapshot mismatch: {field}")
            }
            Self::InvalidTopicSegment => formatter.write_str("invalid worker status topic segment"),
            Self::TopicTooLong => formatter.write_str("worker status topic is too long"),
            Self::Publication(WorkerRuntimeStatusLane::Worker) => {
                formatter.write_str("worker status publication failed on per-worker lane")
            }
            Self::Publication(WorkerRuntimeStatusLane::Node) => {
                formatter.write_str("worker status publication failed on node lane")
            }
            Self::UnregisteredWorker => {
                formatter.write_str("supervisor status contains an unregistered worker")
            }
            Self::NodeSnapshot(field) => write!(formatter, "invalid node snapshot: {field}"),
            Self::RuntimeFile(step) => write!(formatter, "runtime status file failed: {step}"),
        }
    }
}

impl std::error::Error for WorkerRuntimeStatusError {}

/// A typed status payload that can be encoded on a worker's state lane.
///
/// Change-set payloads remain separate typed variants rather than becoming an
/// open JSON map.  Each variant is validated against the shared contract and,
/// when attached to a status projection, against the node and generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum WorkerRuntimeChangeSet {
    /// A not-yet-expired preview/commit/cancel request.
    Request(runtime::WorkerChangeSetRequest),
    /// A typed result with per-item outcomes.
    Result(runtime::WorkerChangeSetResult),
}

/// An admitted contract plus one explicit node-scoped runtime observation.
///
/// The projection stores no inferred state.  In particular, an expired
/// `running` observation remains `running` in the retained record; consumers
/// can use [`runtime::WorkerRuntimeSnapshot::effective_state`] when presenting
/// it at a later clock value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerRuntimeStatus {
    /// The validated registry declaration for this worker.
    pub contract: runtime::WorkerContract,
    /// The explicit, bounded runtime observation.
    pub snapshot: runtime::WorkerRuntimeSnapshot,
}

/// One bounded, credential-free aggregate for `/run/mde/mackesd-status.json`
/// and `state/mackesd/<node>`. Individual worker topics retain the same
/// admitted [`WorkerRuntimeStatus`] rows; this aggregate prevents the node lane
/// from being overwritten once per worker and exposing only the last row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerRuntimeNodeStatus {
    /// Shared worker-runtime schema version.
    pub schema_version: u16,
    /// Node whose supervisor produced every worker observation.
    pub node_id: String,
    /// Caller-supplied observation time shared by the complete sample.
    pub observed_at_ms: u64,
    /// Deterministically ordered admitted worker observations.
    pub workers: Vec<WorkerRuntimeStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRuntimeNodeStatusWire {
    schema_version: u16,
    node_id: String,
    observed_at_ms: u64,
    workers: Vec<WorkerRuntimeStatus>,
}

impl<'de> Deserialize<'de> for WorkerRuntimeNodeStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorkerRuntimeNodeStatusWire::deserialize(deserializer)?;
        let status = Self {
            schema_version: wire.schema_version,
            node_id: wire.node_id,
            observed_at_ms: wire.observed_at_ms,
            workers: wire.workers,
        };
        status
            .validate_at(status.observed_at_ms)
            .map_err(de::Error::custom)?;
        Ok(status)
    }
}

impl WorkerRuntimeNodeStatus {
    /// Validate node identity, bounds, ordering, uniqueness, and every nested row.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), WorkerRuntimeStatusError> {
        if self.schema_version != runtime::WORKER_RUNTIME_SCHEMA_VERSION {
            return Err(WorkerRuntimeStatusError::NodeSnapshot("schema_version"));
        }
        node_status_topic(&self.node_id)?;
        if self.observed_at_ms == 0 || now_ms < self.observed_at_ms {
            return Err(WorkerRuntimeStatusError::NodeSnapshot("observed_at_ms"));
        }
        if self.workers.len() > MAX_NODE_STATUS_WORKERS {
            return Err(WorkerRuntimeStatusError::NodeSnapshot("workers.capacity"));
        }
        let mut identities = BTreeSet::new();
        let mut previous: Option<(runtime::WorkerGroup, &str)> = None;
        for worker in &self.workers {
            worker
                .contract
                .validate()
                .map_err(WorkerRuntimeStatusError::Contract)?;
            worker
                .snapshot
                .validate_at(now_ms)
                .map_err(WorkerRuntimeStatusError::Snapshot)?;
            validate_status_shape(worker)?;
            if worker.snapshot.node_id != self.node_id {
                return Err(WorkerRuntimeStatusError::NodeSnapshot("workers.node_id"));
            }
            if worker.snapshot.observed_at_ms != self.observed_at_ms {
                return Err(WorkerRuntimeStatusError::NodeSnapshot(
                    "workers.observed_at_ms",
                ));
            }
            if !identities.insert(worker.contract.worker_id.as_str()) {
                return Err(WorkerRuntimeStatusError::NodeSnapshot("workers.duplicate"));
            }
            let current = (worker.contract.group, worker.contract.worker_id.as_str());
            if previous.is_some_and(|prior| prior > current) {
                return Err(WorkerRuntimeStatusError::NodeSnapshot("workers.order"));
            }
            previous = Some(current);
        }
        Ok(())
    }

    /// Validate and encode the aggregate under the node wire-size bound.
    pub fn to_json(&self) -> Result<String, WorkerRuntimeStatusError> {
        self.validate_at(self.observed_at_ms)?;
        let body = serde_json::to_string(self)
            .map_err(|_| WorkerRuntimeStatusError::NodeSnapshot("json"))?;
        if body.len() > MAX_NODE_STATUS_WIRE_BYTES {
            return Err(WorkerRuntimeStatusError::NodeSnapshot("wire.capacity"));
        }
        Ok(body)
    }

    /// Decode and validate an untrusted aggregate at the caller's current clock.
    pub fn from_json(body: &str, now_ms: u64) -> Result<Self, WorkerRuntimeStatusError> {
        if body.len() > MAX_NODE_STATUS_WIRE_BYTES {
            return Err(WorkerRuntimeStatusError::NodeSnapshot("wire.capacity"));
        }
        let status: Self = serde_json::from_str(body)
            .map_err(|_| WorkerRuntimeStatusError::NodeSnapshot("json"))?;
        status.validate_at(now_ms)?;
        Ok(status)
    }
}

/// The fields that describe a worker's meaning, excluding publication clock
/// and snapshot identity fields that necessarily change on every sample.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerRuntimeCoalescingKey {
    schema_version: u16,
    node_id: String,
    workers: Vec<(runtime::WorkerContract, runtime::WorkerRuntimeSnapshot)>,
}

impl WorkerRuntimeCoalescingKey {
    fn from_node(node: &WorkerRuntimeNodeStatus) -> Self {
        let workers = node
            .workers
            .iter()
            .map(|status| {
                let mut snapshot = status.snapshot.clone();
                // These fields are publication metadata, not lifecycle state.
                // Keeping them out of the key lets an unchanged supervisor row
                // coalesce while preserving the complete typed body whenever a
                // worker actually changes.
                snapshot.snapshot_id.clear();
                snapshot.generation = 0;
                snapshot.observed_at_ms = 0;
                snapshot.published_at_ms = 0;
                snapshot.fresh_until_ms = 0;
                (status.contract.clone(), snapshot)
            })
            .collect();
        Self {
            schema_version: node.schema_version,
            node_id: node.node_id.clone(),
            workers,
        }
    }
}

/// Coalesce unchanged supervisor samples without allowing the retained
/// runtime projection to become stale indefinitely.
#[derive(Debug, Default)]
pub struct WorkerRuntimeStatusCoalescer {
    last_key: Option<WorkerRuntimeCoalescingKey>,
    last_published_at_ms: Option<u64>,
}

impl WorkerRuntimeStatusCoalescer {
    /// Return whether `node` should be written to the file and Bus lanes.
    /// Changes publish immediately; unchanged samples publish at the bounded
    /// heartbeat interval.
    pub fn should_publish(&self, node: &WorkerRuntimeNodeStatus, now_ms: u64) -> bool {
        let key = WorkerRuntimeCoalescingKey::from_node(node);
        let changed = self.last_key.as_ref() != Some(&key);
        let heartbeat_due = self.last_published_at_ms.map_or(true, |last| {
            now_ms.saturating_sub(last) >= STATUS_HEARTBEAT_INTERVAL.as_millis() as u64
        });
        changed || heartbeat_due
    }

    /// Commit a successful file/Bus publication. Keeping this separate from
    /// [`Self::should_publish`] ensures a failed write remains eligible for a
    /// retry on the next supervisor tick.
    pub fn mark_published(&mut self, node: &WorkerRuntimeNodeStatus, now_ms: u64) {
        self.last_key = Some(WorkerRuntimeCoalescingKey::from_node(node));
        self.last_published_at_ms = Some(now_ms);
    }
}

#[derive(Debug, Clone)]
struct RuntimeTrack {
    generation: u64,
    state: runtime::WorkerRuntimeState,
    state_since_ms: u64,
    restart_count: u32,
    next_event_sequence: u64,
    timeline: VecDeque<runtime::WorkerTimelineEvent>,
}

/// Stateful adapter from the supervisor's explicit lifecycle map to the shared
/// worker-runtime contract. It never probes a PID or turns a missing row into a
/// healthy worker. Unknown supervisor rows are excluded from the published
/// aggregate so registry drift cannot suppress valid registered rows.
#[derive(Debug, Default)]
pub struct WorkerRuntimeSampler {
    tracks: BTreeMap<String, RuntimeTrack>,
}

impl WorkerRuntimeSampler {
    /// Sample the explicit supervisor map into one deterministic node aggregate.
    pub fn sample(
        &mut self,
        statuses: &crate::workers::WorkerStatusMap,
        node_id: &str,
        now_ms: u64,
    ) -> Result<WorkerRuntimeNodeStatus, WorkerRuntimeStatusError> {
        node_status_topic(node_id)?;
        if now_ms == 0 {
            return Err(WorkerRuntimeStatusError::NodeSnapshot("observed_at_ms"));
        }
        let rows = statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut admitted_rows = Vec::with_capacity(rows.len());
        for row in rows {
            match crate::worker_role::worker_contract_for(row.name)
                .map_err(WorkerRuntimeStatusError::Contract)?
            {
                Some(contract) => admitted_rows.push((row, contract)),
                None => {
                    // The status map is shared with transitional/optional
                    // workers. Unknown rows are never published, but they must
                    // not suppress valid registered rows in this sample.
                    tracing::debug!(worker = row.name, "ignoring unregistered worker status row");
                }
            }
        }
        if admitted_rows.len() > MAX_NODE_STATUS_WORKERS {
            return Err(WorkerRuntimeStatusError::NodeSnapshot("workers.capacity"));
        }

        let mut workers = Vec::with_capacity(admitted_rows.len());
        for (row, contract) in admitted_rows {
            let (state, state_reason) = supervisor_state(&row);
            let track = self
                .tracks
                .entry(contract.worker_id.clone())
                .or_insert_with(|| RuntimeTrack {
                    generation: 0,
                    state,
                    state_since_ms: now_ms,
                    restart_count: row.restarts,
                    next_event_sequence: 1,
                    timeline: VecDeque::new(),
                });
            track.generation = track
                .generation
                .checked_add(1)
                .ok_or(WorkerRuntimeStatusError::NodeSnapshot("generation"))?;
            if track.timeline.is_empty() {
                push_timeline(
                    track,
                    &contract.worker_id,
                    now_ms,
                    runtime::WorkerTimelineEventKind::Registered,
                    Some(state),
                    "worker registered",
                )?;
            }
            if row.restarts > track.restart_count {
                push_timeline(
                    track,
                    &contract.worker_id,
                    now_ms,
                    runtime::WorkerTimelineEventKind::Restarted,
                    Some(state),
                    "worker restarted",
                )?;
                track.restart_count = row.restarts;
            }
            if state != track.state {
                track.state = state;
                track.state_since_ms = now_ms;
                push_timeline(
                    track,
                    &contract.worker_id,
                    now_ms,
                    runtime::WorkerTimelineEventKind::StateChanged,
                    Some(state),
                    "worker state changed",
                )?;
            }

            let fresh_until_ms = now_ms
                .checked_add(RUNTIME_FRESHNESS_MS)
                .ok_or(WorkerRuntimeStatusError::NodeSnapshot("fresh_until_ms"))?;
            // The shared constructor validates immediately and therefore
            // cannot construct a state whose closed reason is required before
            // that reason is attached. Start from a reason-free valid state,
            // then set the complete explicit supervisor observation and run
            // the normal admission below.
            let constructor_state = if state.requires_reason() {
                runtime::WorkerRuntimeState::Running
            } else {
                state
            };
            let mut snapshot = runtime::WorkerRuntimeSnapshot::new(
                format!("{}-{}", contract.worker_id, track.generation),
                node_id,
                contract.worker_id.clone(),
                contract.group,
                track.generation,
                constructor_state,
                track.state_since_ms,
                now_ms,
                now_ms,
                fresh_until_ms,
            )
            .map_err(WorkerRuntimeStatusError::Snapshot)?;
            snapshot.state = state;
            snapshot.restart_count = row.restarts;
            snapshot.state_reason = state_reason;
            snapshot.timeline = track.timeline.iter().cloned().collect();
            workers.push(project_status(&contract, snapshot, now_ms)?);
        }
        workers.sort_by(|left, right| {
            (left.contract.group, left.contract.worker_id.as_str())
                .cmp(&(right.contract.group, right.contract.worker_id.as_str()))
        });
        let node = WorkerRuntimeNodeStatus {
            schema_version: runtime::WORKER_RUNTIME_SCHEMA_VERSION,
            node_id: node_id.to_owned(),
            observed_at_ms: now_ms,
            workers,
        };
        node.validate_at(now_ms)?;
        Ok(node)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRuntimeStatusWire {
    contract: runtime::WorkerContract,
    snapshot: runtime::WorkerRuntimeSnapshot,
}

impl<'de> Deserialize<'de> for WorkerRuntimeStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorkerRuntimeStatusWire::deserialize(deserializer)?;
        admit_shape(wire.contract, wire.snapshot).map_err(de::Error::custom)
    }
}

impl WorkerRuntimeStatus {
    /// Admit a validated contract and an explicit snapshot at `now_ms`.
    ///
    /// The clock is supplied by the caller.  This function does not sample a
    /// clock, inspect a process, or turn absence into a runtime state.
    pub fn admit(
        contract: runtime::WorkerContract,
        snapshot: runtime::WorkerRuntimeSnapshot,
        now_ms: u64,
    ) -> Result<Self, WorkerRuntimeStatusError> {
        let status = admit_shape(contract, snapshot)?;
        status
            .snapshot
            .validate_at(now_ms)
            .map_err(WorkerRuntimeStatusError::Snapshot)?;
        Ok(status)
    }

    /// Project an explicit snapshot under a borrowed validated contract.
    pub fn project(
        contract: &runtime::WorkerContract,
        snapshot: runtime::WorkerRuntimeSnapshot,
        now_ms: u64,
    ) -> Result<Self, WorkerRuntimeStatusError> {
        Self::admit(contract.clone(), snapshot, now_ms)
    }

    /// Decode and admit a bounded status publication at `now_ms`.
    pub fn from_json(body: &str, now_ms: u64) -> Result<Self, WorkerRuntimeStatusError> {
        if body.len() > MAX_STATUS_WIRE_BYTES {
            return Err(WorkerRuntimeStatusError::Snapshot(
                runtime::WorkerRuntimeContractError::PayloadTooLarge,
            ));
        }
        let status = serde_json::from_str::<Self>(body).map_err(|_| {
            WorkerRuntimeStatusError::Snapshot(runtime::WorkerRuntimeContractError::MalformedWire)
        })?;
        status
            .snapshot
            .validate_at(now_ms)
            .map_err(WorkerRuntimeStatusError::Snapshot)?;
        Ok(status)
    }

    /// Encode the admitted status as deterministic bounded JSON.
    pub fn to_json(&self) -> Result<String, WorkerRuntimeStatusError> {
        admit_shape(self.contract.clone(), self.snapshot.clone())?;
        let body = serde_json::to_string(self).map_err(|_| {
            WorkerRuntimeStatusError::Snapshot(runtime::WorkerRuntimeContractError::MalformedWire)
        })?;
        if body.len() > MAX_STATUS_WIRE_BYTES {
            return Err(WorkerRuntimeStatusError::Snapshot(
                runtime::WorkerRuntimeContractError::PayloadTooLarge,
            ));
        }
        Ok(body)
    }

    /// Return the canonical per-worker state topic used by the registry's
    /// group ownership model.
    pub fn topic(&self) -> Result<String, WorkerRuntimeStatusError> {
        worker_status_topic(&self.contract)
    }

    /// Return the canonical node snapshot topic for this observation.
    pub fn node_topic(&self) -> Result<String, WorkerRuntimeStatusError> {
        node_status_topic(&self.snapshot.node_id)
    }

    /// Admit a typed change-set payload against this worker's node and current
    /// generation.  Request expiry and all shared count/redaction bounds are
    /// checked before the payload can be encoded.
    pub fn admit_change_set(
        &self,
        payload: WorkerRuntimeChangeSet,
        now_ms: u64,
    ) -> Result<WorkerRuntimeChangeSet, WorkerRuntimeStatusError> {
        match &payload {
            WorkerRuntimeChangeSet::Request(request) => {
                request
                    .validate_at(now_ms)
                    .map_err(WorkerRuntimeStatusError::ChangeSet)?;
                if request.expected_generation != self.snapshot.generation {
                    return Err(WorkerRuntimeStatusError::ContractSnapshotMismatch(
                        "change_set.expected_generation",
                    ));
                }
                validate_change_set_target(&request.target, self)?;
            }
            WorkerRuntimeChangeSet::Result(result) => {
                result
                    .validate()
                    .map_err(WorkerRuntimeStatusError::ChangeSet)?;
                if result.completed_at_ms > now_ms {
                    return Err(WorkerRuntimeStatusError::ChangeSet(
                        runtime::WorkerRuntimeContractError::InvalidFreshness(
                            "change_set_result.completed_at_ms",
                        ),
                    ));
                }
                if result.expected_generation != self.snapshot.generation {
                    return Err(WorkerRuntimeStatusError::ContractSnapshotMismatch(
                        "change_set.expected_generation",
                    ));
                }
                if result.actual_generation < result.expected_generation {
                    return Err(WorkerRuntimeStatusError::ContractSnapshotMismatch(
                        "change_set.actual_generation",
                    ));
                }
                validate_change_set_target(&result.target, self)?;
            }
        }
        Ok(canonical_change_set(payload))
    }

    /// Validate and encode a typed change-set payload deterministically.
    pub fn change_set_json(
        &self,
        payload: WorkerRuntimeChangeSet,
        now_ms: u64,
    ) -> Result<String, WorkerRuntimeStatusError> {
        let admitted = self.admit_change_set(payload, now_ms)?;
        serde_json::to_string(&admitted).map_err(|_| {
            WorkerRuntimeStatusError::ChangeSet(runtime::WorkerRuntimeContractError::MalformedWire)
        })
    }
}

/// Project a contract and explicit snapshot without requiring a caller to
/// name the implementation type.
pub fn project_status(
    contract: &runtime::WorkerContract,
    snapshot: runtime::WorkerRuntimeSnapshot,
    now_ms: u64,
) -> Result<WorkerRuntimeStatus, WorkerRuntimeStatusError> {
    WorkerRuntimeStatus::project(contract, snapshot, now_ms)
}

/// Admit and retain one caller-supplied runtime observation on its canonical
/// per-worker and node-scoped status topics.
///
/// The worker lane receives the compact worker body and the node lane receives
/// the same row inside the canonical aggregate envelope, so every writer uses
/// one node-lane wire type. The caller owns the persistence handle and clock;
/// this seam never opens a default store, samples a process, or manufactures a
/// state. If the node write fails after the worker write,
/// [`WorkerRuntimeStatusError::Publication`] identifies the failed lane and the
/// successful per-worker message remains retained.
pub fn publish_status(
    persist: &mut Persist,
    contract: &runtime::WorkerContract,
    snapshot: runtime::WorkerRuntimeSnapshot,
    now_ms: u64,
) -> Result<WorkerRuntimeStatusPublication, WorkerRuntimeStatusError> {
    let status = project_status(contract, snapshot, now_ms)?;
    let worker_body = status.to_json()?;
    let worker_topic = status.topic()?;
    let node_topic = status.node_topic()?;
    let node_body = WorkerRuntimeNodeStatus {
        schema_version: runtime::WORKER_RUNTIME_SCHEMA_VERSION,
        node_id: status.snapshot.node_id.clone(),
        observed_at_ms: status.snapshot.observed_at_ms,
        workers: vec![status.clone()],
    }
    .to_json()?;

    let worker_message = crate::bus_publish::publish_body(persist, &worker_topic, &worker_body)
        .ok_or(WorkerRuntimeStatusError::Publication(
            WorkerRuntimeStatusLane::Worker,
        ))?;
    let node_message = crate::bus_publish::publish_body(persist, &node_topic, &node_body).ok_or(
        WorkerRuntimeStatusError::Publication(WorkerRuntimeStatusLane::Node),
    )?;

    Ok(WorkerRuntimeStatusPublication {
        status,
        worker_topic,
        worker_message_id: worker_message.ulid,
        node_topic,
        node_message_id: node_message.ulid,
    })
}

/// Retain an aggregate supervisor sample without repeatedly overwriting the
/// node lane with single-worker bodies. Each per-worker row lands first; the
/// aggregate node record lands last and is therefore the commit marker for the
/// complete sample.
pub fn publish_node_status(
    persist: &mut Persist,
    node: &WorkerRuntimeNodeStatus,
) -> Result<WorkerRuntimeNodePublication, WorkerRuntimeStatusError> {
    node.validate_at(node.observed_at_ms)?;
    let mut worker_message_ids = Vec::with_capacity(node.workers.len());
    for worker in &node.workers {
        let topic = worker.topic()?;
        let body = worker.to_json()?;
        let message = crate::bus_publish::publish_body(persist, &topic, &body).ok_or(
            WorkerRuntimeStatusError::Publication(WorkerRuntimeStatusLane::Worker),
        )?;
        worker_message_ids.push((worker.contract.worker_id.clone(), message.ulid));
    }
    let node_topic = node_status_topic(&node.node_id)?;
    let body = node.to_json()?;
    let node_message = crate::bus_publish::publish_body(persist, &node_topic, &body).ok_or(
        WorkerRuntimeStatusError::Publication(WorkerRuntimeStatusLane::Node),
    )?;
    Ok(WorkerRuntimeNodePublication {
        worker_message_ids,
        node_topic,
        node_message_id: node_message.ulid,
    })
}

/// Atomically replace the credential-free runtime status file. Parent and
/// destination symlinks are rejected before any write; the temporary file is
/// created with `O_EXCL` in the same directory and renamed only after sync.
pub fn write_runtime_status_file(
    path: &Path,
    node: &WorkerRuntimeNodeStatus,
) -> Result<(), WorkerRuntimeStatusError> {
    let body = node.to_json()?;
    let parent = path
        .parent()
        .ok_or(WorkerRuntimeStatusError::RuntimeFile("parent"))?;
    if let Ok(metadata) = std::fs::symlink_metadata(parent) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(WorkerRuntimeStatusError::RuntimeFile("parent_type"));
        }
    } else {
        std::fs::create_dir_all(parent)
            .map_err(|_| WorkerRuntimeStatusError::RuntimeFile("create_parent"))?;
        let metadata = std::fs::symlink_metadata(parent)
            .map_err(|_| WorkerRuntimeStatusError::RuntimeFile("parent_metadata"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(WorkerRuntimeStatusError::RuntimeFile("parent_type"));
        }
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(WorkerRuntimeStatusError::RuntimeFile("destination_type"));
        }
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(WorkerRuntimeStatusError::RuntimeFile("file_name"))?;
    let nonce = STATUS_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o644);
    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|_| WorkerRuntimeStatusError::RuntimeFile("create_temporary"))?;
        file.write_all(body.as_bytes())
            .map_err(|_| WorkerRuntimeStatusError::RuntimeFile("write"))?;
        file.write_all(b"\n")
            .map_err(|_| WorkerRuntimeStatusError::RuntimeFile("write_newline"))?;
        file.sync_all()
            .map_err(|_| WorkerRuntimeStatusError::RuntimeFile("sync"))?;
        std::fs::rename(&temporary, path)
            .map_err(|_| WorkerRuntimeStatusError::RuntimeFile("rename"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Return the canonical state topic for one admitted worker contract.
///
/// The group prefixes mirror `worker_role::WorkerGroup::state_topic_prefix`;
/// keeping this mapping here avoids coupling the shared wire contract to the
/// daemon registry implementation.
pub fn worker_status_topic(
    contract: &runtime::WorkerContract,
) -> Result<String, WorkerRuntimeStatusError> {
    contract
        .validate()
        .map_err(WorkerRuntimeStatusError::Contract)?;
    let topic = format!(
        "{}/{}",
        group_state_topic_prefix(contract.group),
        contract.worker_id
    );
    bounded_topic(topic)
}

/// Return the node-scoped snapshot topic required by the Workers read model.
pub fn node_status_topic(node_id: &str) -> Result<String, WorkerRuntimeStatusError> {
    if !is_topic_identifier(node_id) {
        return Err(WorkerRuntimeStatusError::InvalidTopicSegment);
    }
    bounded_topic(format!("{NODE_STATUS_TOPIC_PREFIX}/{node_id}"))
}

/// Map a shared runtime group to the daemon registry's canonical state prefix.
#[must_use]
pub const fn group_state_topic_prefix(group: runtime::WorkerGroup) -> &'static str {
    match group {
        runtime::WorkerGroup::Control => "state/mackesd/control/workers",
        runtime::WorkerGroup::Observation => "state/mackesd/observation/workers",
        runtime::WorkerGroup::Actions => "state/mackesd/actions/workers",
        runtime::WorkerGroup::Data => "state/mackesd/data/workers",
        runtime::WorkerGroup::Compute => "state/mackesd/compute/workers",
        runtime::WorkerGroup::Integrations => "state/mackesd/integrations/workers",
    }
}

fn admit_shape(
    mut contract: runtime::WorkerContract,
    mut snapshot: runtime::WorkerRuntimeSnapshot,
) -> Result<WorkerRuntimeStatus, WorkerRuntimeStatusError> {
    contract = contract
        .admitted()
        .map_err(WorkerRuntimeStatusError::Contract)?;
    snapshot = snapshot
        .admitted()
        .map_err(WorkerRuntimeStatusError::Snapshot)?;
    if snapshot.worker_id != contract.worker_id {
        return Err(WorkerRuntimeStatusError::ContractSnapshotMismatch(
            "worker_id",
        ));
    }
    if snapshot.group != contract.group {
        return Err(WorkerRuntimeStatusError::ContractSnapshotMismatch("group"));
    }

    // Relations are a set-like graph projection.  Timeline order is semantic
    // history and was already checked by the shared contract, so it is left
    // untouched.  Sorting before serialization makes equivalent registry
    // input deterministic without dropping or inventing an edge.
    contract.applicability.roles.sort();
    contract.applicability.capabilities.sort();
    contract.dependencies.sort();
    contract.publications.sort();
    contract.subscriptions.sort();
    contract.actions.sort_by_key(|action| action.action);
    snapshot
        .relations
        .sort_by(|left, right| left.relation_id.cmp(&right.relation_id));

    Ok(WorkerRuntimeStatus { contract, snapshot })
}

fn validate_status_shape(status: &WorkerRuntimeStatus) -> Result<(), WorkerRuntimeStatusError> {
    admit_shape(status.contract.clone(), status.snapshot.clone()).map(|_| ())
}

fn supervisor_state(
    row: &crate::workers::WorkerStatus,
) -> (
    runtime::WorkerRuntimeState,
    Option<runtime::WorkerRuntimeReason>,
) {
    if row.breaker_tripped {
        return (
            runtime::WorkerRuntimeState::Failed,
            Some(runtime::WorkerRuntimeReason::CrashLoop),
        );
    }
    if row.alive {
        return (runtime::WorkerRuntimeState::Running, None);
    }
    match row.last_exit_ok {
        Some(true) => (runtime::WorkerRuntimeState::Stopped, None),
        Some(false) => (
            runtime::WorkerRuntimeState::Failed,
            Some(runtime::WorkerRuntimeReason::Unknown),
        ),
        None => (runtime::WorkerRuntimeState::Starting, None),
    }
}

fn push_timeline(
    track: &mut RuntimeTrack,
    worker_id: &str,
    occurred_at_ms: u64,
    kind: runtime::WorkerTimelineEventKind,
    state: Option<runtime::WorkerRuntimeState>,
    summary: &'static str,
) -> Result<(), WorkerRuntimeStatusError> {
    let sequence = track.next_event_sequence;
    if sequence == u64::MAX {
        return Err(WorkerRuntimeStatusError::NodeSnapshot("timeline.sequence"));
    }
    track.next_event_sequence += 1;
    let event = runtime::WorkerTimelineEvent::new(
        format!("{worker_id}-event-{sequence}"),
        sequence,
        worker_id,
        occurred_at_ms,
        kind,
        state,
        summary,
        None,
    )
    .map_err(WorkerRuntimeStatusError::Snapshot)?;
    if track.timeline.len() == runtime::MAX_WORKER_TIMELINE_EVENTS {
        track.timeline.pop_front();
    }
    track.timeline.push_back(event);
    Ok(())
}

fn validate_change_set_target(
    target: &runtime::WorkerChangeSetTarget,
    status: &WorkerRuntimeStatus,
) -> Result<(), WorkerRuntimeStatusError> {
    if target.node_id != status.snapshot.node_id {
        return Err(WorkerRuntimeStatusError::ContractSnapshotMismatch(
            "change_set.target.node_id",
        ));
    }
    if target.worker_id.as_deref() != Some(status.contract.worker_id.as_str()) {
        return Err(WorkerRuntimeStatusError::ContractSnapshotMismatch(
            "change_set.target.worker_id",
        ));
    }
    Ok(())
}

fn canonical_change_set(payload: WorkerRuntimeChangeSet) -> WorkerRuntimeChangeSet {
    match payload {
        WorkerRuntimeChangeSet::Request(mut request) => {
            request
                .items
                .sort_by(|left, right| left.item_id.cmp(&right.item_id));
            WorkerRuntimeChangeSet::Request(request)
        }
        WorkerRuntimeChangeSet::Result(mut result) => {
            result
                .items
                .sort_by(|left, right| left.item_id.cmp(&right.item_id));
            WorkerRuntimeChangeSet::Result(result)
        }
    }
}

fn bounded_topic(topic: String) -> Result<String, WorkerRuntimeStatusError> {
    if topic.len() > runtime::MAX_WORKER_TOPIC_BYTES {
        return Err(WorkerRuntimeStatusError::TopicTooLong);
    }
    Ok(topic)
}

fn is_topic_identifier(value: &str) -> bool {
    value.len() <= runtime::MAX_WORKER_IDENTIFIER_BYTES
        && !value.is_empty()
        && value.trim() == value
        && value.is_ascii()
        && value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> runtime::WorkerContract {
        runtime::WorkerContract::new("host-state", runtime::WorkerGroup::Control, "Host State")
            .expect("valid contract")
    }

    fn snapshot() -> runtime::WorkerRuntimeSnapshot {
        let mut snapshot = runtime::WorkerRuntimeSnapshot::new(
            "snapshot-1",
            "node-a",
            "host-state",
            runtime::WorkerGroup::Control,
            3,
            runtime::WorkerRuntimeState::Running,
            1_000,
            2_000,
            2_000,
            5_000,
        )
        .expect("valid snapshot");
        snapshot.relations.push(
            runtime::WorkerRelation::new(
                "relation-b",
                runtime::WorkerRelationKind::Publishes,
                runtime::WorkerRelationEndpoint::Worker {
                    worker_id: "host-state".to_owned(),
                },
                runtime::WorkerRelationEndpoint::Topic {
                    topic: "state/z".to_owned(),
                },
                None,
            )
            .expect("valid relation"),
        );
        snapshot.relations.push(
            runtime::WorkerRelation::new(
                "relation-a",
                runtime::WorkerRelationKind::Supports,
                runtime::WorkerRelationEndpoint::Worker {
                    worker_id: "host-state".to_owned(),
                },
                runtime::WorkerRelationEndpoint::Node {
                    node_id: "node-a".to_owned(),
                },
                None,
            )
            .expect("valid relation"),
        );
        snapshot.timeline.push(
            runtime::WorkerTimelineEvent::new(
                "event-1",
                1,
                "host-state",
                1_500,
                runtime::WorkerTimelineEventKind::Registered,
                None,
                "worker registered",
                None,
            )
            .expect("valid event"),
        );
        snapshot.timeline.push(
            runtime::WorkerTimelineEvent::new(
                "event-2",
                2,
                "host-state",
                2_000,
                runtime::WorkerTimelineEventKind::StateChanged,
                Some(runtime::WorkerRuntimeState::Running),
                "worker running",
                None,
            )
            .expect("valid event"),
        );
        snapshot.validate().expect("fixture remains valid");
        snapshot
    }

    fn change_set_request() -> runtime::WorkerChangeSetRequest {
        runtime::WorkerChangeSetRequest {
            schema_version: runtime::WORKER_RUNTIME_SCHEMA_VERSION,
            request_id: "request-1".to_owned(),
            operation: runtime::WorkerChangeSetOperation::Preview,
            target: runtime::WorkerChangeSetTarget {
                node_id: "node-a".to_owned(),
                worker_id: Some("host-state".to_owned()),
            },
            expected_generation: 3,
            items: vec![
                runtime::WorkerChangeSetItem {
                    item_id: "item-b".to_owned(),
                    worker_id: "host-state".to_owned(),
                    action: runtime::WorkerAction::Refresh,
                },
                runtime::WorkerChangeSetItem {
                    item_id: "item-a".to_owned(),
                    worker_id: "host-state".to_owned(),
                    action: runtime::WorkerAction::Restart,
                },
            ],
            impact: "refreshes the host observation".to_owned(),
            recovery: "the worker remains restartable".to_owned(),
            arming: runtime::WorkerArmingRequirement::Confirmation,
            digest: format!("sha256:{}", "a".repeat(64)),
            requested_at_ms: 2_000,
            expires_at_ms: 3_000,
        }
    }

    #[test]
    fn status_round_trip_uses_registry_topics_and_preserves_explicit_state() {
        let status = project_status(&contract(), snapshot(), 2_500).expect("admit status");
        assert_eq!(
            status.topic().expect("worker topic"),
            "state/mackesd/control/workers/host-state"
        );
        assert_eq!(
            status.node_topic().expect("node topic"),
            "state/mackesd/node-a"
        );
        assert_eq!(status.snapshot.state, runtime::WorkerRuntimeState::Running);
        let body = status.to_json().expect("encode status");
        let decoded = WorkerRuntimeStatus::from_json(&body, 2_500).expect("decode status");
        assert_eq!(decoded, status);
        assert_eq!(body, decoded.to_json().expect("re-encode status"));
    }

    #[test]
    fn publication_retains_the_exact_admitted_status_in_both_canonical_wire_types() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("open bus");

        let published = publish_status(&mut persist, &contract(), snapshot(), 2_500)
            .expect("publish admitted status");
        assert_eq!(
            published.worker_topic,
            "state/mackesd/control/workers/host-state"
        );
        assert_eq!(published.node_topic, "state/mackesd/node-a");
        assert_eq!(
            published.status.snapshot.state,
            runtime::WorkerRuntimeState::Running
        );

        let worker_message = persist
            .read_latest(&published.worker_topic)
            .expect("read worker lane")
            .expect("worker message");
        let node_message = persist
            .read_latest(&published.node_topic)
            .expect("read node lane")
            .expect("node message");
        assert_eq!(worker_message.ulid, published.worker_message_id);
        assert_eq!(node_message.ulid, published.node_message_id);
        assert_eq!(worker_message.priority, "default");
        assert_eq!(node_message.priority, "default");
        assert!(worker_message.title.is_none());
        assert!(node_message.title.is_none());
        assert!(worker_message.actions.is_empty());
        assert!(node_message.actions.is_empty());
        assert!(worker_message.reply_to.is_none());
        assert!(node_message.reply_to.is_none());

        let worker_body = worker_message.body.expect("worker body");
        let node_body = node_message.body.expect("node body");
        assert!(worker_body.len() <= MAX_STATUS_WIRE_BYTES);
        assert_eq!(
            WorkerRuntimeStatus::from_json(&worker_body, 2_500).expect("decode retained status"),
            published.status
        );
        let aggregate =
            WorkerRuntimeNodeStatus::from_json(&node_body, 2_500).expect("decode node aggregate");
        assert_eq!(aggregate.workers, vec![published.status]);
    }

    #[test]
    fn hostile_or_over_capacity_observations_are_rejected_before_any_write() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let worker_topic = worker_status_topic(&contract()).expect("worker topic");
        let node_topic = node_status_topic("node-a").expect("node topic");

        let mut secret_shaped = snapshot();
        secret_shaped.timeline[1].detail = Some("token=do-not-retain".to_owned());
        assert!(matches!(
            publish_status(&mut persist, &contract(), secret_shaped, 2_500),
            Err(WorkerRuntimeStatusError::Snapshot(
                runtime::WorkerRuntimeContractError::SecretShapedValue(
                    "worker_timeline_event.detail"
                )
            ))
        ));

        let mut over_capacity = snapshot();
        over_capacity.timeline = (1..=runtime::MAX_WORKER_TIMELINE_EVENTS as u64 + 1)
            .map(|sequence| {
                runtime::WorkerTimelineEvent::new(
                    format!("event-{sequence}"),
                    sequence,
                    "host-state",
                    1_000 + sequence,
                    runtime::WorkerTimelineEventKind::Started,
                    None,
                    "worker event",
                    None,
                )
                .expect("bounded event")
            })
            .collect();
        assert!(matches!(
            publish_status(&mut persist, &contract(), over_capacity, 2_500),
            Err(WorkerRuntimeStatusError::Snapshot(
                runtime::WorkerRuntimeContractError::CapacityExceeded {
                    field: "worker_runtime_snapshot.timeline",
                    max: runtime::MAX_WORKER_TIMELINE_EVENTS
                }
            ))
        ));

        assert!(persist
            .read_latest(&worker_topic)
            .expect("read worker lane")
            .is_none());
        assert!(persist
            .read_latest(&node_topic)
            .expect("read node lane")
            .is_none());
    }

    #[test]
    fn second_lane_persistence_failure_is_reported_without_fabricating_fallback_state() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let blocked_node_path = bus.path().join("state/mackesd/node-a");
        std::fs::create_dir_all(blocked_node_path.parent().expect("node topic parent"))
            .expect("create topic parent");
        std::fs::write(&blocked_node_path, b"blocks the node topic directory")
            .expect("block node topic");

        assert!(matches!(
            publish_status(&mut persist, &contract(), snapshot(), 2_500),
            Err(WorkerRuntimeStatusError::Publication(
                WorkerRuntimeStatusLane::Node
            ))
        ));

        let worker_topic = worker_status_topic(&contract()).expect("worker topic");
        let retained = persist
            .read_latest(&worker_topic)
            .expect("read successful first lane")
            .expect("worker lane remains retained");
        let retained =
            WorkerRuntimeStatus::from_json(retained.body.as_deref().expect("status body"), 2_500)
                .expect("decode retained status");
        let expected = project_status(&contract(), snapshot(), 2_500).expect("expected status");
        assert_eq!(retained.snapshot, expected.snapshot);
        assert_eq!(
            retained.snapshot.state,
            runtime::WorkerRuntimeState::Running
        );
        assert!(persist
            .read_latest("state/mackesd/node-a")
            .expect("read failed node lane")
            .is_none());
    }

    #[test]
    fn equivalent_relation_order_has_deterministic_json_and_change_set_order() {
        let mut reversed = snapshot();
        reversed.relations.reverse();
        let first = project_status(&contract(), snapshot(), 2_500).expect("first status");
        let second = project_status(&contract(), reversed, 2_500).expect("second status");
        assert_eq!(
            first.to_json().expect("first json"),
            second.to_json().expect("second json")
        );

        let payload = WorkerRuntimeChangeSet::Request(change_set_request());
        let body = first
            .change_set_json(payload, 2_500)
            .expect("encode change set");
        let decoded: WorkerRuntimeChangeSet = serde_json::from_str(&body).expect("decode set");
        let admitted = first
            .admit_change_set(decoded, 2_500)
            .expect("admit decoded set");
        let WorkerRuntimeChangeSet::Request(request) = admitted else {
            panic!("expected request")
        };
        assert_eq!(
            request
                .items
                .iter()
                .map(|item| item.item_id.as_str())
                .collect::<Vec<_>>(),
            vec!["item-a", "item-b"]
        );
    }

    #[test]
    fn hostile_contract_snapshot_topic_and_generation_inputs_fail_closed() {
        let mut hostile = snapshot();
        hostile.timeline[0].summary = "$(id)".to_owned();
        assert!(project_status(&contract(), hostile, 2_500).is_err());
        assert!(node_status_topic("../etc").is_err());

        let mut mismatched = snapshot();
        mismatched.worker_id = "other-worker".to_owned();
        assert!(matches!(
            project_status(&contract(), mismatched, 2_500),
            Err(WorkerRuntimeStatusError::Snapshot(_))
                | Err(WorkerRuntimeStatusError::ContractSnapshotMismatch(_))
        ));

        let status = project_status(&contract(), snapshot(), 2_500).expect("status");
        let mut request = change_set_request();
        request.expected_generation = 2;
        assert!(matches!(
            status.admit_change_set(WorkerRuntimeChangeSet::Request(request), 2_500),
            Err(WorkerRuntimeStatusError::ContractSnapshotMismatch(
                "change_set.expected_generation"
            ))
        ));

        let body = status.to_json().expect("encode status");
        let hostile_json = body.replacen("\"snapshot\":{", "\"snapshot\":{\"command\":\"id\",", 1);
        assert!(WorkerRuntimeStatus::from_json(&hostile_json, 2_500).is_err());
    }

    #[test]
    fn shared_relation_timeline_and_change_set_caps_are_admitted_not_truncated() {
        let mut relation_overflow = snapshot();
        relation_overflow.relations = (0..=runtime::MAX_WORKER_RELATIONS)
            .map(|index| {
                runtime::WorkerRelation::new(
                    format!("relation-{index}"),
                    runtime::WorkerRelationKind::Publishes,
                    runtime::WorkerRelationEndpoint::Worker {
                        worker_id: "host-state".to_owned(),
                    },
                    runtime::WorkerRelationEndpoint::Topic {
                        topic: format!("state/topic-{index}"),
                    },
                    None,
                )
                .expect("valid relation")
            })
            .collect();
        assert!(matches!(
            project_status(&contract(), relation_overflow, 2_500),
            Err(WorkerRuntimeStatusError::Snapshot(
                runtime::WorkerRuntimeContractError::CapacityExceeded {
                    field: "worker_runtime_snapshot.relations",
                    max: runtime::MAX_WORKER_RELATIONS
                }
            ))
        ));

        let mut timeline_overflow = snapshot();
        timeline_overflow.timeline = (1..=runtime::MAX_WORKER_TIMELINE_EVENTS as u64 + 1)
            .map(|sequence| {
                runtime::WorkerTimelineEvent::new(
                    format!("event-{sequence}"),
                    sequence,
                    "host-state",
                    1_000 + sequence,
                    runtime::WorkerTimelineEventKind::Started,
                    None,
                    "worker event",
                    None,
                )
                .expect("valid event")
            })
            .collect();
        assert!(matches!(
            project_status(&contract(), timeline_overflow, 2_500),
            Err(WorkerRuntimeStatusError::Snapshot(
                runtime::WorkerRuntimeContractError::CapacityExceeded {
                    field: "worker_runtime_snapshot.timeline",
                    max: runtime::MAX_WORKER_TIMELINE_EVENTS
                }
            ))
        ));

        let status = project_status(&contract(), snapshot(), 2_500).expect("status");
        let mut too_many = change_set_request();
        too_many.items = (0..=runtime::MAX_WORKER_CHANGE_SET_ITEMS)
            .map(|index| runtime::WorkerChangeSetItem {
                item_id: format!("item-{index}"),
                worker_id: "host-state".to_owned(),
                action: runtime::WorkerAction::Refresh,
            })
            .collect();
        assert!(matches!(
            status.admit_change_set(WorkerRuntimeChangeSet::Request(too_many), 2_500),
            Err(WorkerRuntimeStatusError::ChangeSet(
                runtime::WorkerRuntimeContractError::CapacityExceeded {
                    field: "change_set_request.items",
                    max: runtime::MAX_WORKER_CHANGE_SET_ITEMS
                }
            ))
        ));
    }

    fn supervisor_row(
        alive: bool,
        restarts: u32,
        breaker_tripped: bool,
        last_exit_ok: Option<bool>,
    ) -> crate::workers::WorkerStatus {
        crate::workers::WorkerStatus {
            name: "cloud",
            alive,
            restarts,
            breaker_tripped,
            breaker_trips: u32::from(breaker_tripped),
            last_exit_ok,
        }
    }

    #[test]
    fn supervisor_sampler_publishes_explicit_lifecycle_and_transition_history() {
        let statuses = crate::workers::new_status_map();
        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(true, 0, false, None));
        let mut sampler = WorkerRuntimeSampler::default();
        let first = sampler
            .sample(&statuses, "node-a", 10_000)
            .expect("first sample");
        assert_eq!(first.workers.len(), 1);
        assert_eq!(
            first.workers[0].snapshot.state,
            runtime::WorkerRuntimeState::Running
        );
        assert_eq!(first.workers[0].snapshot.generation, 1);
        assert_eq!(first.workers[0].snapshot.timeline.len(), 1);

        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(false, 1, true, Some(false)));
        let second = sampler
            .sample(&statuses, "node-a", 11_000)
            .expect("second sample");
        let worker = &second.workers[0];
        assert_eq!(worker.snapshot.generation, 2);
        assert_eq!(worker.snapshot.restart_count, 1);
        assert_eq!(worker.snapshot.state, runtime::WorkerRuntimeState::Failed);
        assert_eq!(
            worker.snapshot.state_reason,
            Some(runtime::WorkerRuntimeReason::CrashLoop)
        );
        assert_eq!(worker.snapshot.state_since_ms, 11_000);
        assert_eq!(worker.snapshot.timeline.len(), 3);
        assert_eq!(
            worker.snapshot.timeline[1].kind,
            runtime::WorkerTimelineEventKind::Restarted
        );
        assert_eq!(
            worker.snapshot.timeline[2].kind,
            runtime::WorkerTimelineEventKind::StateChanged
        );

        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(false, 1, false, Some(false)));
        let third = sampler
            .sample(&statuses, "node-a", 12_000)
            .expect("third sample");
        assert_eq!(
            third.workers[0].snapshot.state_reason,
            Some(runtime::WorkerRuntimeReason::Unknown),
            "a failed exit without a tripped breaker must not be called a crash loop"
        );
    }

    #[test]
    fn supervisor_sampler_rejects_generation_overflow_instead_of_reusing_identity() {
        let statuses = crate::workers::new_status_map();
        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(true, 0, false, None));
        let mut sampler = WorkerRuntimeSampler::default();
        sampler
            .sample(&statuses, "node-a", 10_000)
            .expect("seed sampler track");
        sampler
            .tracks
            .get_mut("cloud")
            .expect("cloud track")
            .generation = u64::MAX;

        assert_eq!(
            sampler.sample(&statuses, "node-a", 11_000),
            Err(WorkerRuntimeStatusError::NodeSnapshot("generation"))
        );
    }

    #[test]
    fn rejected_supervisor_samples_use_a_bounded_retry_ladder() {
        let mut retry = STATUS_FAILURE_RETRY_INITIAL;
        let mut delays = Vec::new();
        for _ in 0..7 {
            delays.push(retry);
            retry = next_status_failure_retry(retry);
        }
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(20),
                Duration::from_secs(40),
                Duration::from_secs(60),
                Duration::from_secs(60),
                Duration::from_secs(60),
            ]
        );
    }

    #[test]
    fn status_phases_are_stable_and_spread_across_the_poll_period() {
        let first = status_phase_delay("seat-a", 1_234);
        let next_period = status_phase_delay("seat-a", 6_234);
        assert_eq!(first, next_period);
        assert!(first > Duration::ZERO);
        assert!(first <= STATUS_POLL_INTERVAL);
        assert!(STATUS_HEARTBEAT_INTERVAL <= Duration::from_millis(RUNTIME_FRESHNESS_MS));
        assert!(status_retry_jitter("seat-a") <= Duration::from_millis(500));
    }

    #[test]
    fn unchanged_supervisor_samples_coalesce_but_heartbeat_and_changes_publish() {
        let statuses = crate::workers::new_status_map();
        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(true, 0, false, None));
        let mut sampler = WorkerRuntimeSampler::default();
        let first = sampler
            .sample(&statuses, "node-a", 10_000)
            .expect("first sample");
        let unchanged = sampler
            .sample(&statuses, "node-a", 11_000)
            .expect("unchanged sample");
        let mut coalescer = WorkerRuntimeStatusCoalescer::default();
        assert!(coalescer.should_publish(&first, 10_000));
        coalescer.mark_published(&first, 10_000);
        assert!(!coalescer.should_publish(&unchanged, 11_000));
        assert!(coalescer.should_publish(&unchanged, 40_000));
        coalescer.mark_published(&unchanged, 40_000);

        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(false, 1, true, Some(false)));
        let changed = sampler
            .sample(&statuses, "node-a", 41_000)
            .expect("changed sample");
        assert!(coalescer.should_publish(&changed, 41_000));
        assert!(coalescer.should_publish(&changed, 42_000));
    }

    #[test]
    fn aggregate_publication_commits_complete_node_body_after_worker_rows() {
        let statuses = crate::workers::new_status_map();
        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(true, 0, false, None));
        let node = WorkerRuntimeSampler::default()
            .sample(&statuses, "node-a", 10_000)
            .expect("sample");
        let bus = tempfile::tempdir().expect("bus tempdir");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let receipt = publish_node_status(&mut persist, &node).expect("publish node");
        assert_eq!(receipt.worker_message_ids.len(), 1);
        assert_eq!(receipt.node_topic, "state/mackesd/node-a");

        let retained = persist
            .read_latest(&receipt.node_topic)
            .expect("read node lane")
            .expect("node message");
        assert_eq!(retained.ulid, receipt.node_message_id);
        let decoded = WorkerRuntimeNodeStatus::from_json(
            retained.body.as_deref().expect("node body"),
            10_000,
        )
        .expect("decode aggregate");
        assert_eq!(decoded, node);

        let worker_topic = node.workers[0].topic().expect("worker topic");
        assert!(persist
            .read_latest(&worker_topic)
            .expect("read worker lane")
            .is_some());
    }

    #[test]
    fn runtime_file_is_atomic_decodable_and_rejects_destination_symlink() {
        let statuses = crate::workers::new_status_map();
        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(true, 0, false, None));
        let node = WorkerRuntimeSampler::default()
            .sample(&statuses, "node-a", 10_000)
            .expect("sample");
        let directory = tempfile::tempdir().expect("runtime tempdir");
        let path = directory.path().join("mackesd-status.json");
        write_runtime_status_file(&path, &node).expect("write runtime file");
        let body = std::fs::read_to_string(&path).expect("read runtime file");
        assert_eq!(
            WorkerRuntimeNodeStatus::from_json(body.trim(), 10_000).expect("decode file"),
            node
        );

        #[cfg(unix)]
        {
            let target = directory.path().join("target");
            std::fs::write(&target, b"unchanged").expect("write target");
            let link = directory.path().join("status-link.json");
            std::os::unix::fs::symlink(&target, &link).expect("create symlink");
            assert!(matches!(
                write_runtime_status_file(&link, &node),
                Err(WorkerRuntimeStatusError::RuntimeFile("destination_type"))
            ));
            assert_eq!(
                std::fs::read_to_string(target).expect("read target"),
                "unchanged"
            );
        }
    }

    #[test]
    fn aggregate_ignores_unregistered_rows_and_rejects_duplicate_rows() {
        let statuses = crate::workers::new_status_map();
        statuses.lock().expect("status map").insert(
            "not-registered",
            crate::workers::WorkerStatus {
                name: "not-registered",
                alive: true,
                restarts: 0,
                breaker_tripped: false,
                breaker_trips: 0,
                last_exit_ok: None,
            },
        );
        statuses
            .lock()
            .expect("status map")
            .insert("cloud", supervisor_row(true, 0, false, None));
        let node = WorkerRuntimeSampler::default()
            .sample(&statuses, "node-a", 10_000)
            .expect("registered rows remain publishable");
        assert_eq!(node.workers.len(), 1);
        assert_eq!(node.workers[0].contract.worker_id, "cloud");

        let mut duplicate_node = WorkerRuntimeNodeStatus {
            schema_version: runtime::WORKER_RUNTIME_SCHEMA_VERSION,
            node_id: "node-a".to_owned(),
            observed_at_ms: 2_000,
            workers: vec![project_status(&contract(), snapshot(), 2_500).expect("status")],
        };
        duplicate_node
            .workers
            .push(duplicate_node.workers[0].clone());
        assert!(matches!(
            duplicate_node.validate_at(2_500),
            Err(WorkerRuntimeStatusError::NodeSnapshot("workers.duplicate"))
        ));
    }
}
